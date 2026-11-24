// SPDX-License-Identifier: GPL-3.0-only
// Opt-in submission, presentation and pacing diagnostics for the PyroWave renderer.
#pragma once

#include "pyrowave_vulkan.h"

namespace pyrowave_renderer {

// Opt-in diagnostics for a physical-device release build. All state belongs to one
// renderer and only its JNI submission thread updates it. Disabled scopes read no
// clocks; enabled scopes keep bounded counters and log once per five seconds.
class SubmissionDiagnostics {
public:
    enum StageId { COPY, CLOCK, PARSE, SLOT, ORDER, SDK, PUBLISH, STATS, EARLY, TOTAL, STAGE_COUNT };

    void configure() {
        char value[PROP_VALUE_MAX] = {};
        __system_property_get("debug.nightjar.pyro_submit", value);
        enabled = std::strcmp(value, "1") == 0;
        if (enabled) LOGI("PyroWave submit profile enabled: wall/thread CPU milliseconds; restart stream to change");
    }

    bool isEnabled() const { return enabled; }

private:
    struct Stamp { uint64_t wall, cpu; };
    struct Timing {
        uint64_t count = 0, wall = 0, cpu = 0, maxWall = 0, maxCpu = 0;
    };
    struct Window {
        uint64_t start = 0, frames = 0, accepted = 0, busy = 0, invalid = 0, errors = 0;
        uint64_t bytes = 0, maxBytes = 0, recordFrames = 0, ordinaryFrames = 0;
        Timing timing[STAGE_COUNT];
    } window;
    bool enabled = false, active = false;

    static Stamp stamp() {
        timespec wall = {}, cpu = {};
        clock_gettime(CLOCK_MONOTONIC, &wall);
        clock_gettime(CLOCK_THREAD_CPUTIME_ID, &cpu);
        return {uint64_t(wall.tv_sec) * 1'000'000'000 + uint64_t(wall.tv_nsec),
                uint64_t(cpu.tv_sec) * 1'000'000'000 + uint64_t(cpu.tv_nsec)};
    }

    void add(StageId id, Stamp start, Stamp end) {
        auto &timing = window.timing[id];
        const uint64_t wall = end.wall >= start.wall ? end.wall - start.wall : 0;
        const uint64_t cpu = end.cpu >= start.cpu ? end.cpu - start.cpu : 0;
        ++timing.count;
        timing.wall += wall;
        timing.cpu += cpu;
        timing.maxWall = std::max(timing.maxWall, wall);
        timing.maxCpu = std::max(timing.maxCpu, cpu);
    }

    void reportIfDue(uint64_t now) {
        if (now - window.start < 5'000'000'000) return;
        LOGI("PyroWave submit profile: frames=%llu accepted=%llu busy=%llu invalid=%llu errors=%llu "
             "bytes_avg=%.0f bytes_max=%llu record_frames=%llu ordinary_frames=%llu nice=%d",
             (unsigned long long)window.frames, (unsigned long long)window.accepted,
             (unsigned long long)window.busy, (unsigned long long)window.invalid,
             (unsigned long long)window.errors, double(window.bytes) / double(window.frames),
             (unsigned long long)window.maxBytes, (unsigned long long)window.recordFrames,
             (unsigned long long)window.ordinaryFrames, getpriority(PRIO_PROCESS, 0));
        static const char *const names[] = {"copy", "clock", "parse", "slot", "order", "sdk", "publish", "stats", "early",
                                            "total"};
        for (int i = 0; i < STAGE_COUNT; ++i) {
            const auto &timing = window.timing[i];
            if (timing.count == 0) continue;
            const double divisor = double(timing.count) * 1e6;
            LOGI("PyroWave submit profile: stage=%s n=%llu wall_avg=%.3f wall_max=%.3f cpu_avg=%.3f cpu_max=%.3f ms",
                 names[i], (unsigned long long)timing.count, timing.wall / divisor,
                 timing.maxWall / 1e6, timing.cpu / divisor, timing.maxCpu / 1e6);
        }
        window = {};
    }

public:
    class Stage {
    public:
        Stage(SubmissionDiagnostics &owner, StageId id)
            : owner(owner.active ? &owner : nullptr), id(id) {
            if (this->owner) start = stamp();
        }
        ~Stage() { if (owner) owner->add(id, start, stamp()); }
    private:
        SubmissionDiagnostics *owner;
        StageId id;
        Stamp start{};
    };

    class Frame {
    public:
        Frame(SubmissionDiagnostics &owner, size_t bytes)
            : owner(owner.enabled ? &owner : nullptr), bytes(bytes) {
            if (this->owner) {
                owner.active = true;
                start = stamp();
                if (owner.window.start == 0) owner.window.start = start.wall;
            }
        }
        ~Frame() {
            if (!owner) return;
            const Stamp end = stamp();
            owner->add(TOTAL, start, end);
            owner->active = false;
            auto &window = owner->window;
            ++window.frames;
            if (result == SUBMIT_OK) ++window.accepted;
            else if (result == SUBMIT_BUSY) ++window.busy;
            else if (result < 0) ++window.errors;
            else ++window.invalid;
            window.bytes += bytes;
            window.maxBytes = std::max(window.maxBytes, uint64_t(bytes));
            if (framingKnown) ++(recordFraming ? window.recordFrames : window.ordinaryFrames);
            // Total includes the existing SDK statistics work, but excludes these
            // diagnostic log writes. Java's unchanged outer timer still includes both.
            owner->reportIfDue(end.wall);
        }
        void framing(bool records) { framingKnown = true; recordFraming = records; }
        int result = SUBMIT_ERROR;
    private:
        SubmissionDiagnostics *owner;
        size_t bytes;
        Stamp start{};
        bool framingKnown = false, recordFraming = false;
    };

    // One batch of early chunks, between frames: the early stage, one sample per batch,
    // outside every frame's total.
    class Chunks {
    public:
        explicit Chunks(SubmissionDiagnostics &owner) : owner(owner.enabled ? &owner : nullptr) {
            if (this->owner) start = stamp();
        }
        ~Chunks() { if (owner) owner->add(EARLY, start, stamp()); }
    private:
        SubmissionDiagnostics *owner;
        Stamp start{};
    };
};

// Only the presenter updates these counters; configuration precedes worker startup.
// Samples use existing CLOCK_MONOTONIC timestamps, never feedback arrival time.
class PresentationDiagnostics {
public:
    void configure(bool enable) { enabled = enable; }

    void record(int64_t completedNs, int64_t releasedNs, uint64_t actualNs) {
        if (!enabled) return;
        if (completedNs <= 0 || releasedNs < completedNs ||
            actualNs > uint64_t(INT64_MAX) || actualNs < uint64_t(releasedNs)) {
            ++window.invalid;
            return;
        }
        const uint64_t beforeRelease = uint64_t(releasedNs - completedNs);
        const uint64_t afterRelease = actualNs - uint64_t(releasedNs);
        ++window.frames;
        window.beforeReleaseNs += double(beforeRelease);
        window.afterReleaseNs += double(afterRelease);
        window.maxBeforeReleaseNs = std::max(window.maxBeforeReleaseNs, beforeRelease);
        window.maxAfterReleaseNs = std::max(window.maxAfterReleaseNs, afterRelease);
    }

    void reportIfDue(uint64_t nowUs) {
        if (!enabled || (window.frames == 0 && window.invalid == 0)) return;
        if (window.startUs == 0) window.startUs = nowUs;
        if (nowUs < window.startUs || nowUs - window.startUs < 5'000'000) return;
        const double divisor = window.frames == 0 ? 1.0 : double(window.frames) * 1e6;
        LOGI("PyroWave present profile: n=%llu invalid=%llu completion_to_release_avg=%.3f "
             "completion_to_release_max=%.3f release_to_actual_avg=%.3f release_to_actual_max=%.3f ms",
             (unsigned long long)window.frames, (unsigned long long)window.invalid,
             window.beforeReleaseNs / divisor, window.maxBeforeReleaseNs / 1e6,
             window.afterReleaseNs / divisor, window.maxAfterReleaseNs / 1e6);
        window = {};
        window.startUs = nowUs;
    }

private:
    struct Window {
        uint64_t startUs = 0, frames = 0, invalid = 0;
        double beforeReleaseNs = 0, afterReleaseNs = 0;
        uint64_t maxBeforeReleaseNs = 0, maxAfterReleaseNs = 0;
    } window;
    bool enabled = false;
};

// Presenter-owned, opt-in counters. Inputs are existing timing values; this
// diagnostic never reads a clock or changes a presentation decision.
class PacingDiagnostics {
public:
    void configure(bool enable) { enabled = enable; }
    void record(const PyroWavePresentTiming::Plan &plan, int64_t readyNs,
                int64_t requestedWaitNs, int64_t periodNs, int64_t leadNs) {
        if (!enabled || !plan.paced) return;
        if (window.frames == 0) {
            window.minPeriodNs = window.maxPeriodNs = periodNs;
            window.minLeadNs = window.maxLeadNs = leadNs;
            window.minAheadNs = window.maxAheadNs = plan.idealNs - readyNs;
        }
        ++window.frames;
        window.minPeriodNs = std::min(window.minPeriodNs, periodNs);
        window.maxPeriodNs = std::max(window.maxPeriodNs, periodNs);
        window.minLeadNs = std::min(window.minLeadNs, leadNs);
        window.maxLeadNs = std::max(window.maxLeadNs, leadNs);
        const int64_t aheadNs = plan.idealNs - readyNs;
        window.minAheadNs = std::min(window.minAheadNs, aheadNs);
        window.maxAheadNs = std::max(window.maxAheadNs, aheadNs);
        window.aheadNs += double(aheadNs);
        window.targetAheadNs += double(plan.targetNs - readyNs);
        const int64_t waitNs = std::max<int64_t>(0, requestedWaitNs);
        window.waitNs += double(waitNs);
        window.maxWaitNs = std::max(window.maxWaitNs, waitNs);
    }
    void reportIfDue(uint64_t nowUs) {
        if (!enabled || window.frames == 0) return;
        if (window.startUs == 0) window.startUs = nowUs;
        if (nowUs < window.startUs || nowUs - window.startUs < 5'000'000) return;
        const double divisor = double(window.frames) * 1e6;
        LOGI("PyroWave pacing profile: n=%llu period_min=%.3f period_max=%.3f "
             "lead_min=%.3f lead_max=%.3f host_ahead_avg=%.3f host_ahead_min=%.3f host_ahead_max=%.3f "
             "target_ahead_avg=%.3f requested_wait_avg=%.3f requested_wait_max=%.3f ms",
             (unsigned long long)window.frames, window.minPeriodNs / 1e6, window.maxPeriodNs / 1e6,
             window.minLeadNs / 1e6, window.maxLeadNs / 1e6, window.aheadNs / divisor,
             window.minAheadNs / 1e6, window.maxAheadNs / 1e6, window.targetAheadNs / divisor,
             window.waitNs / divisor, window.maxWaitNs / 1e6);
        window = {};
        window.startUs = nowUs;
    }
private:
    struct Window {
        uint64_t startUs = 0, frames = 0;
        int64_t minPeriodNs = 0, maxPeriodNs = 0, minLeadNs = 0, maxLeadNs = 0;
        int64_t minAheadNs = 0, maxAheadNs = 0, maxWaitNs = 0;
        double aheadNs = 0, targetAheadNs = 0, waitNs = 0;
    } window;
    bool enabled = false;
};

}  // namespace pyrowave_renderer
