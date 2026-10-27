#include "../../main/jni/pyrowave-renderer/present_timing.h"

#include <cassert>
#include <cstdint>
#include <limits>
#include <vector>

using Timing = PyroWavePresentTiming;
static constexpr int64_t ms = 1'000'000;
static constexpr int64_t epoch = 1'000 * ms;

static void testHostCadenceAndIdempotence() {
    Timing timing;
    timing.configure(Timing::Mode::Balanced, 10 * ms);
    timing.observeVsync(epoch);
    auto first = timing.plan(1'000'000, epoch + ms);
    assert(first.paced && first.targetNs == epoch + 10 * ms);
    assert(first.submitNs == epoch + 8 * ms);
    assert(first.desiredPresentNs == epoch + 5 * ms);

    // Two source frames arrive in a burst. Their display cadence still follows
    // the host clock, whose epoch is deliberately unrelated to the local clock.
    auto second = timing.plan(1'020'000, epoch + 2 * ms);
    assert(second.targetNs - first.targetNs == 20 * ms);
    auto repeated = timing.plan(1'020'000, epoch + 50 * ms);
    assert(repeated.targetNs == second.targetNs && repeated.submitNs == second.submitNs);
    auto third = timing.plan(1'040'000, epoch + 19 * ms);
    assert(third.targetNs - second.targetNs == 20 * ms);
}

static void testModesAndMissingHostClock() {
    for (auto mode : {Timing::Mode::Immediate, Timing::Mode::Balanced,
                      Timing::Mode::CapFps, Timing::Mode::Smoothness}) {
        Timing timing;
        timing.configure(mode, 10 * ms);
        timing.observeVsync(epoch);
        auto planned = timing.plan(1'000'000, epoch + ms);
        if (mode == Timing::Mode::Immediate) {
            assert(!planned.paced && planned.submitNs == epoch + ms);
            assert(planned.desiredPresentNs == 0);
        } else {
            const int cushion = mode == Timing::Mode::Balanced ? 1 : 2;
            assert(planned.targetNs == epoch + cushion * 10 * ms);
        }
        const auto absent = timing.plan(0, epoch + 2 * ms);
        assert(!absent.paced && absent.submitNs == epoch + 2 * ms);
        assert(absent.desiredPresentNs == 0);
    }
}

static void testFractionalCadenceDoesNotAccumulateRounding() {
    Timing timing;
    constexpr int64_t period = 8'333'333;
    timing.configure(Timing::Mode::Balanced, period);
    timing.observeVsync(epoch);
    for (int64_t frame = 0; frame < 1000; ++frame) {
        const int64_t hostUs = 100'000'000 + frame * 16'683;
        const int64_t ready = epoch + ms + frame * 16'683'000;
        const int64_t ideal = epoch + 3 * ms + frame * 16'683'000;
        const auto planned = timing.plan(hostUs, ready);
        assert(planned.targetNs >= ideal);
        assert(planned.targetNs - ideal < period);
        assert((planned.targetNs - epoch) % period == 0);
    }
}

static void testPauseReversalAndLateRecovery() {
    Timing timing;
    timing.configure(Timing::Mode::Smoothness, 10 * ms);
    timing.observeVsync(epoch);
    auto first = timing.plan(1'000'000, epoch + ms);
    timing.recordPresent(1, first, first.submitNs);
    auto resumed = timing.plan(1'300'000, epoch + 301 * ms);
    assert(resumed.targetNs >= epoch + 301 * ms);
    assert(resumed.targetNs <= epoch + 331 * ms);
    assert(timing.pendingCount() == 0);
    timing.feedback(1, epoch + 100 * ms, epoch + 100 * ms, 0);
    assert(timing.phaseNs() == epoch); // Old swap/pause feedback cannot move phase.

    auto reversed = timing.plan(50, epoch + 311 * ms);
    assert(reversed.targetNs <= epoch + 341 * ms);
    auto jump = timing.plan(9'000'000'000, epoch + 321 * ms);
    assert(jump.targetNs <= epoch + 351 * ms);
    // A short burst of lateness gets a current deadline, never an accumulating
    // number of missed refresh intervals carried from previous display deadlines.
    auto late = timing.plan(9'000'010'000, epoch + 380 * ms);
    assert(late.submitNs >= epoch + 380 * ms);
    assert(late.targetNs <= epoch + 410 * ms);
}

static void deliver(Timing &timing, uint32_t id, int64_t neededNs, int64_t compositorDelayNs = 0) {
    const int64_t ready = epoch + ms + int64_t(id) * 10 * ms;
    const auto planned = timing.plan(1'000'000 + int64_t(id) * 10'000, ready);
    const int64_t submitted = planned.submitNs;
    timing.recordPresent(id, planned, submitted);
    // Simulated compositor: choose its first refresh after rendering is ready,
    // then report the remaining margin to the earliest possible presentation.
    const int64_t workReady = submitted + neededNs;
    const int64_t earliest = epoch + ((workReady - epoch + 10 * ms - 1) / (10 * ms)) * 10 * ms;
    timing.feedback(id, earliest + compositorDelayNs, earliest, earliest - workReady);
}

static void testFeedbackAdaptsLeadAndRejectsAnIsolatedStall() {
    Timing timing;
    timing.configure(Timing::Mode::Balanced, 10 * ms);
    timing.observeVsync(epoch);
    for (uint32_t id = 1; id <= 40; ++id) deliver(timing, id, 5 * ms);
    assert(timing.leadNs() == 5 * ms + ms / 2);
    deliver(timing, 41, 15 * ms);
    assert(timing.leadNs() == 5 * ms + ms / 2);
    // The compositor may show an already-ready image later. Its earliest time and
    // margin, rather than that unrelated delay, must determine rendering lead.
    for (uint32_t id = 42; id <= 81; ++id) deliver(timing, id, 5 * ms, 20 * ms);
    assert(timing.leadNs() == 5 * ms + ms / 2);
    for (uint32_t id = 82; id <= 490; ++id) deliver(timing, id, ms / 2);
    assert(timing.leadNs() == ms); // Sustained easier work gives latency back.
    assert(timing.pendingCount() == 0);
}

static void testClockPriorityModeChangesAndInvalidFeedback() {
    Timing timing;
    timing.configure(Timing::Mode::Balanced, 10 * ms);
    timing.observeVsync(epoch);
    auto planned = timing.plan(1'000'000, epoch + ms);
    timing.recordPresent(1, planned, planned.submitNs);
    timing.feedback(1, epoch + 10 * ms, epoch + 10 * ms, ms);
    assert(timing.phaseNs() == epoch + 10 * ms);
    timing.observeVsync(epoch + 20 * ms + ms / 2);
    assert(timing.phaseNs() == epoch + 10 * ms); // Presentation clock is fresher.
    timing.observeVsync(epoch + 600 * ms);
    assert(timing.phaseNs() == epoch + 600 * ms); // Reports disappeared; fallback resumes.
    timing.observeVsync(epoch + 590 * ms);
    assert(timing.phaseNs() == epoch + 600 * ms);

    planned = timing.plan(1'010'000, epoch + 11 * ms);
    timing.recordPresent(2, planned, planned.submitNs);
    timing.feedback(2, planned.submitNs + 251 * ms, planned.submitNs + 251 * ms, 0);
    assert(timing.phaseNs() == epoch + 600 * ms);
    timing.setRefreshPeriod(8 * ms);
    assert(timing.periodNs() == 8 * ms && timing.phaseNs() == 0);
    assert(timing.pendingCount() == 0);
    timing.setRefreshPeriod(0);
    timing.setRefreshPeriod(std::numeric_limits<int64_t>::max());
    assert(timing.periodNs() == 8 * ms);
    timing.configure(Timing::Mode::Immediate, 8 * ms);
    assert(!timing.plan(1'020'000, epoch + 21 * ms).paced);
}

static void testZeroMarginDoesNotInventRenderWork() {
    Timing timing;
    timing.configure(Timing::Mode::Balanced, 10 * ms);
    timing.observeVsync(epoch);
    for (uint32_t id = 1; id <= 40; ++id) {
        auto planned = timing.plan(1'000'000 + int64_t(id) * 10'000,
                epoch + ms + int64_t(id) * 10 * ms);
        timing.recordPresent(id, planned, planned.submitNs);
        timing.feedback(id, planned.targetNs, planned.targetNs, 0);
        assert(timing.phaseNs() == planned.targetNs);
        assert(timing.leadNs() == 2 * ms);
    }
}

static void testBoundedMissingReportsAndExtremeInputs() {
    Timing timing;
    timing.configure(Timing::Mode::Balanced, 10 * ms);
    timing.observeVsync(epoch);
    for (uint32_t id = 1; id <= 1000; ++id) {
        const auto planned = timing.plan(1'000'000 + int64_t(id) * 10'000,
                epoch + int64_t(id) * 10 * ms);
        timing.recordPresent(id, planned, planned.submitNs);
        assert(timing.pendingCount() <= 64);
    }
    timing.feedback(1, epoch + 10 * ms, epoch + 10 * ms, 0);
    assert(timing.pendingCount() == 64); // Forgotten id cannot consume a newer record.
    auto hugeHost = timing.plan(std::numeric_limits<int64_t>::max(), epoch + 11'000 * ms);
    assert(hugeHost.paced && hugeHost.targetNs < epoch + 11'050 * ms);
    auto invalidNow = timing.plan(1, std::numeric_limits<int64_t>::max());
    assert(!invalidNow.paced);
    timing.reset();
    assert(timing.pendingCount() == 0 && timing.phaseNs() == 0);
}

static Timing timingWithSlowInitialFrame(Timing::Mode mode = Timing::Mode::Balanced) {
    Timing t;
    t.configure(mode, 10 * ms);
    t.observeVsync(epoch);
    t.plan(1'000'000, epoch + 30 * ms);
    return t;
}

static void sustainedImprovementPreservesModeCushion() {
    for (auto mode : {Timing::Mode::Balanced, Timing::Mode::CapFps, Timing::Mode::Smoothness}) {
        auto t = timingWithSlowInitialFrame(mode);
        const int64_t cushion = mode == Timing::Mode::Balanced ? 0 : 10 * ms;
        for (int i = 0; i < 30; ++i) {
            const int64_t host = 1'100'000 + i * 10'000;
            const int64_t ready = epoch + (110 + i * 10) * ms;
            const auto p = t.plan(host, ready);
            const int64_t expectedAhead = (i < 7 ? 22 * ms : 2 * ms) + cushion;
            assert(p.idealNs - ready == expectedAhead);
            assert(p.targetNs >= ready + t.leadNs());
            assert(p.targetNs - p.idealNs < t.periodNs());
        }
        assert(t.periodNs() == 10 * ms && t.leadNs() == 2 * ms && t.phaseNs() == epoch);
    }
}

static void burstsAndRepeatedPlansDoNotCountAsSustained() {
    auto t = timingWithSlowInitialFrame();
    for (int i = 0; i < 7; ++i) {
        const int64_t host = 1'100'000 + i * 10'000;
        const int64_t ready = epoch + (110 + i * 10) * ms;
        const auto p = t.plan(host, ready);
        assert(p.idealNs - ready == 22 * ms);
        for (int repeat = 0; repeat < 20; ++repeat) {
            const auto again = t.plan(host, ready + 90 * ms);
            assert(again.submitNs == p.submitNs && again.idealNs == p.idealNs);
        }
    }
    // A normal-latency arrival clears the entire run, rather than accumulating
    // evidence across unrelated network bursts.
    const auto normal = t.plan(1'170'000, epoch + 200 * ms);
    assert(normal.idealNs == epoch + 202 * ms);
    for (int i = 0; i < 7; ++i) {
        const int64_t ready = epoch + (210 + i * 10) * ms;
        const auto p = t.plan(1'200'000 + i * 10'000, ready);
        assert(p.idealNs - ready == 22 * ms);
    }
    const auto recovered = t.plan(1'270'000, epoch + 280 * ms);
    assert(recovered.idealNs == epoch + 282 * ms);
}

static void pausesAndModeChangesDiscardOldEvidence() {
    auto t = timingWithSlowInitialFrame();
    for (int i = 0; i < 7; ++i) t.plan(1'100'000 + i * 10'000, epoch + (110 + i * 10) * ms);
    const auto resumed = t.plan(1'500'000, epoch + 510 * ms);
    assert(resumed.idealNs == epoch + 512 * ms);
    const auto next = t.plan(1'510'000, epoch + 520 * ms);
    assert(next.idealNs == epoch + 522 * ms);
    t.setRefreshPeriod(20 * ms);
    const auto changed = t.plan(1'520'000, epoch + 530 * ms);
    assert(changed.idealNs == epoch + 532 * ms && t.periodNs() == 20 * ms);
    const auto reversed = t.plan(12, epoch + 540 * ms);
    assert(reversed.idealNs == epoch + 542 * ms);
}

static void ordinaryJitterAndImmediateModeStayUnchanged() {
    auto t = timingWithSlowInitialFrame();
    for (int i = 0; i < 300; ++i) {
        const int64_t ready = epoch + (130 + i * 10 + (i % 2 ? 3 : -3)) * ms;
        const auto p = t.plan(1'100'000 + i * 10'000, ready);
        assert(p.idealNs == epoch + (132 + i * 10) * ms);
    }
    auto immediate = timingWithSlowInitialFrame(Timing::Mode::Immediate);
    for (int i = 0; i < 30; ++i) {
        const int64_t ready = epoch + (110 + i * 10) * ms;
        const auto p = immediate.plan(1'100'000 + i * 10'000, ready);
        assert(!p.paced && p.submitNs == ready && p.desiredPresentNs == 0 && p.idealNs == 0);
    }
}

static void fractionalCadenceRemainsStableAfterRecovery() {
    Timing t;
    constexpr int64_t period = 8'333'333;
    t.configure(Timing::Mode::Balanced, period);
    t.observeVsync(epoch);
    t.plan(1'000'000, epoch + 30 * ms);
    for (int i = 0; i < 1000; ++i) {
        const int64_t deltaUs = 100'000 + int64_t(i) * 16'683;
        const int64_t ready = epoch + deltaUs * 1000 + 6 * ms;
        const auto p = t.plan(1'000'000 + deltaUs, ready);
        assert(p.idealNs - ready == (i < 7 ? 26 * ms : 2 * ms));
        assert(p.targetNs >= p.idealNs && p.targetNs - p.idealNs < period);
        assert((p.targetNs - epoch) % period == 0);
    }
}

static void recoveryPreservesLearnedLeadPhaseAndPendingFeedback() {
    auto t = timingWithSlowInitialFrame();
    // Teach a 5 ms rendering requirement, rather than relying on the initial lead.
    for (uint32_t id = 1; id <= 40; ++id) {
        const auto p = t.plan(1'000'000 + int64_t(id) * 10'000, epoch + (30 + int64_t(id) * 10) * ms);
        t.recordPresent(id, p, p.submitNs);
        const int64_t workReady = p.submitNs + 5 * ms;
        const int64_t earliest = epoch + ((workReady - epoch + 10 * ms - 1) / (10 * ms)) * (10 * ms);
        t.feedback(id, earliest, earliest, earliest - workReady);
    }
    assert(t.leadNs() == 5 * ms + ms / 2);
    t.observeVsync(epoch + 1000 * ms);
    const auto first = t.plan(2'000'000, epoch + 1000 * ms);
    t.recordPresent(100, first, first.submitNs);
    const auto phase = t.phaseNs();
    for (int i = 0; i < 8; ++i) {
        const int64_t ready = epoch + (1080 + i * 10) * ms;
        const auto p = t.plan(2'100'000 + i * 10'000, ready);
        assert(t.leadNs() == 5 * ms + ms / 2 && t.phaseNs() == phase && t.pendingCount() == 1);
        if (i == 7) assert(p.idealNs - ready == t.leadNs());
    }
    t.feedback(100, first.targetNs, first.targetNs, ms);
    assert(t.pendingCount() == 0 && t.phaseNs() == first.targetNs);
}

static void testRecoveryKeepsPresentationThroughput() {
    constexpr int64_t period = 8'333'333;
    constexpr int frames = 4800;
    struct Frame { int64_t hostUs, readyNs; };
    std::vector<Frame> input;
    for (int i = 0; i < frames; ++i) {
        const int64_t hostDeltaUs = int64_t(i) * 25'000 / 3;
        // At ten seconds, arrival/decode latency improves from 30 ms
        // to 6 ms over 48 frames. Completions remain ordered and 120 Hz input
        // continues; there are no host pauses, clock jumps or display changes.
        const int64_t reduction = std::min(std::max(i - 1200, 0), 48) * (ms / 2);
        input.push_back({100'000'000 + hostDeltaUs, epoch + hostDeltaUs * 1000 + 30 * ms - reduction});
        if (i) assert(input[i].readyNs > input[i - 1].readyNs);
    }
    Timing timing;
    timing.configure(Timing::Mode::Balanced, period);
    timing.observeVsync(epoch);
    int latest = -1, consumed = -1;
    int64_t now = input.front().readyNs;
    int stableBefore = 0, stableAfter = 0;
    double afterDelayMs = 0;
    while (consumed < frames - 1) {
        if (latest <= consumed) now = std::max(now, input[consumed + 1].readyNs);
        while (latest + 1 < frames && input[latest + 1].readyNs <= now) ++latest;
        assert(latest > consumed);
        const auto &frame = input[latest];
        consumed = latest;
        const auto plan = timing.plan(frame.hostUs, frame.readyNs);
        now = std::max(now, plan.submitNs);
        if (now >= epoch + 2'000 * ms && now < epoch + 9'000 * ms) ++stableBefore;
        if (now >= epoch + 20'000 * ms && now < epoch + 35'000 * ms) {
            ++stableAfter;
            afterDelayMs += double(now - frame.readyNs) / ms;
        }
        now += 50'000; // Bounded CPU submit / present overhead, no blocking GPU work.
    }
    const double beforeFps = stableBefore / 7.0;
    const double afterFps = stableAfter / 15.0;
    assert(beforeFps > 119 && beforeFps < 121);
    assert(afterFps > 119 && afterFps < 121);
    assert(afterDelayMs / stableAfter < 10);
    assert(timing.periodNs() == period);
}

int main() {
    sustainedImprovementPreservesModeCushion();
    burstsAndRepeatedPlansDoNotCountAsSustained();
    pausesAndModeChangesDiscardOldEvidence();
    ordinaryJitterAndImmediateModeStayUnchanged();
    fractionalCadenceRemainsStableAfterRecovery();
    recoveryPreservesLearnedLeadPhaseAndPendingFeedback();
    testRecoveryKeepsPresentationThroughput();
    testHostCadenceAndIdempotence();
    testModesAndMissingHostClock();
    testFractionalCadenceDoesNotAccumulateRounding();
    testPauseReversalAndLateRecovery();
    testFeedbackAdaptsLeadAndRejectsAnIsolatedStall();
    testClockPriorityModeChangesAndInvalidFeedback();
    testZeroMarginDoesNotInventRenderWork();
    testBoundedMissingReportsAndExtremeInputs();
}
