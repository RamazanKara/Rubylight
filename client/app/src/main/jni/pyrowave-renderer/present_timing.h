#pragma once

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <limits>

// Host timestamps supply frame cadence; local monotonic timestamps supply display
// deadlines. The renderer serializes every call. No Vulkan or Android dependency.
class PyroWavePresentTiming {
public:
    enum class Mode { Immediate = 0, Balanced = 1, CapFps = 2, Smoothness = 3 };
    struct Plan {
        int64_t submitNs = 0;
        int64_t desiredPresentNs = 0;
        int64_t targetNs = 0;
        bool paced = false;
        int64_t idealNs = 0;  // Unrounded host mapping, for opt-in diagnostics only.
    };

    void configure(Mode mode, int64_t nominalPeriodNs) {
        mode_ = mode;
        if (validPeriod(nominalPeriodNs)) periodNs_ = nominalPeriodNs;
        reset();
    }

    // A stream/swapchain change invalidates the host mapping and old feedback.
    // Keep the selected mode and measured display period.
    void reset() {
        clearTimeline();
        phaseNs_ = 0;
        feedbackPhaseNs_ = 0;
        leadNs_ = std::min(kInitialLeadNs, maximumLeadNs());
        clearLearning();
    }

    // Use the display's measured cycle, independently of the stream's FPS.
    void setRefreshPeriod(int64_t measuredPeriodNs) {
        if (!validPeriod(measuredPeriodNs)) return;
        const int64_t difference = measuredPeriodNs > periodNs_
                ? measuredPeriodNs - periodNs_ : periodNs_ - measuredPeriodNs;
        const bool changed = difference > periodNs_ / 20;
        periodNs_ = measuredPeriodNs;
        leadNs_ = std::min(leadNs_, maximumLeadNs());
        if (changed) {
            clearTimeline();
            clearLearning();
            phaseNs_ = 0;
            feedbackPhaseNs_ = 0;
        }
    }

    // Choreographer's source timestamp, not callback arrival time. Its measured
    // interval is supplied separately by setRefreshPeriod(). Skip delayed/duplicate
    // samples, and prefer actual presentation feedback while it is fresh.
    void observeVsync(int64_t frameTimeNs) {
        if (!validTime(frameTimeNs) || frameTimeNs <= phaseNs_) return;
        if (feedbackPhaseNs_ != 0 && frameTimeNs >= feedbackPhaseNs_ &&
            frameTimeNs - feedbackPhaseNs_ < kPhaseFreshNs) return;
        phaseNs_ = frameTimeNs;
    }

    // readyNs must remain the frame's original local completion timestamp. Plans
    // are cached by host timestamp: waiting/polling the same frame cannot move its
    // deadline or accumulate another frame of latency. Local ptsUs is NOT an input.
    Plan plan(int64_t hostPresentationTimeUs, int64_t readyNs) {
        if (mode_ == Mode::Immediate || hostPresentationTimeUs <= 0 || !validTime(readyNs)) {
            clearTimeline();
            return {readyNs, 0, 0, false};
        }
        if (anchored_ && hostPresentationTimeUs == lastHostUs_) return lastPlan_;

        bool restart = !anchored_ || hostPresentationTimeUs < lastHostUs_ || readyNs < lastReadyNs_;
        if (!restart) {
            restart = readyNs - lastReadyNs_ > pauseGapNs() ||
                      hostPresentationTimeUs - lastHostUs_ > pauseGapNs() / 1000;
        }
        if (restart) anchor(hostPresentationTimeUs, readyNs);

        // Bound clock discontinuities before converting microseconds to ns.
        int64_t elapsedUs = hostPresentationTimeUs - anchorHostUs_;
        if (elapsedUs < 0 || elapsedUs > kAnchorSpanNs / 1000) {
            anchor(hostPresentationTimeUs, readyNs);
            elapsedUs = 0;
        }
        int64_t idealNs = anchorLocalNs_ + elapsedUs * 1000;
        const int64_t earliestNs = readyNs + leadNs_;
        if (idealNs < readyNs - periodNs_ * 2 || idealNs > readyNs + maximumAheadNs()) {
            anchor(hostPresentationTimeUs, readyNs);
            idealNs = anchorLocalNs_;
        }
        recoverEarlyTimeline(readyNs, idealNs);
        const int64_t targetNs = nextVsync(std::max(idealNs, earliestNs));
        lastPlan_ = {std::max(readyNs, targetNs - leadNs_),
                     phaseNs_ == 0 ? targetNs : targetNs - periodNs_ / 2,
                     targetNs, true, idealNs};
        lastHostUs_ = hostPresentationTimeUs;
        lastReadyNs_ = readyNs;

        // Preserve the unrounded host timeline when rebasing, never the last
        // display deadline. Non-integer refresh ratios must not drift a vsync/frame.
        if (elapsedUs > kRebaseNs / 1000) {
            anchorHostUs_ = hostPresentationTimeUs;
            anchorLocalNs_ = idealNs;
        }
        return lastPlan_;
    }

    // Call only after a successful present, with the timestamp immediately before
    // its queue submission. The fixed-size ring also bounds missing driver reports.
    void recordPresent(uint32_t id, const Plan &planned, int64_t submittedNs) {
        if (id == 0 || !planned.paced || !validTime(submittedNs)) return;
        for (auto &entry : pending_) if (entry.id == id) entry.id = 0;
        pending_[nextPending_] = {id, submittedNs, settling_ != 0};
        nextPending_ = (nextPending_ + 1) % pending_.size();
        if (settling_ != 0) --settling_;
    }

    // VK_GOOGLE_display_timing times are local monotonic ns. The earliest time
    // less the compositor margin describes the latest successful ready deadline;
    // subtracting our submission time estimates the lead this frame needed.
    void feedback(uint32_t id, int64_t actualNs, int64_t earliestNs, int64_t marginNs) {
        Entry entry;
        bool found = false;
        for (auto &pending : pending_) {
            if (pending.id == id && id != 0) {
                entry = pending;
                pending.id = 0;
                found = true;
                break;
            }
        }
        if (!found || !validTime(actualNs) || actualNs < entry.submittedNs ||
            actualNs - entry.submittedNs > kFeedbackAgeNs) return;
        if (actualNs > feedbackPhaseNs_) {
            feedbackPhaseNs_ = actualNs;
            phaseNs_ = actualNs;
        }
        // A zero margin supplies no positive slack evidence; keep the phase but
        // do not mistake an unreported margin for a whole refresh of GPU work.
        if (entry.settling || earliestNs < entry.submittedNs || earliestNs > actualNs ||
            marginNs <= 0 || marginNs > earliestNs - entry.submittedNs) return;
        const int64_t neededNs = earliestNs - entry.submittedNs - marginNs;
        if (neededNs > maximumLeadNs()) return;
        needs_[nextNeed_] = std::max(kMinLeadNs, neededNs);
        nextNeed_ = (nextNeed_ + 1) % needs_.size();
        needCount_ = std::min(needCount_ + 1, needs_.size());
        if (needCount_ < kMinimumSamples) return;

        auto sorted = needs_;
        std::sort(sorted.begin(), sorted.begin() + needCount_);
        // A bounded recent percentile rejects isolated stalls; sustained changes
        // raise the lead quickly. Reductions are deliberately slower to avoid hunts.
        const int64_t wanted = std::min(maximumLeadNs(),
                sorted[(needCount_ - 1) * 9 / 10] + kSafetyNs);
        if (wanted > leadNs_) {
            leadNs_ = wanted;
            lowerCount_ = 0;
        } else if (++lowerCount_ >= kLowerSamples) {
            leadNs_ = std::max(wanted, leadNs_ - kLowerStepNs);
            lowerCount_ = 0;
        }
    }

    int64_t leadNs() const { return leadNs_; }
    int64_t periodNs() const { return periodNs_; }
    int64_t phaseNs() const { return phaseNs_; }
    size_t pendingCount() const {
        size_t count = 0;
        for (const auto &entry : pending_) if (entry.id != 0) ++count;
        return count;
    }

private:
    static constexpr int64_t kInitialLeadNs = 2'000'000;
    static constexpr int64_t kMinLeadNs = 500'000;
    static constexpr int64_t kSafetyNs = 500'000;
    static constexpr int64_t kLowerStepNs = 250'000;
    static constexpr int64_t kPhaseFreshNs = 500'000'000;
    static constexpr int64_t kFeedbackAgeNs = 250'000'000;
    static constexpr int64_t kAnchorSpanNs = 2'000'000'000;
    static constexpr int64_t kRebaseNs = 1'000'000'000;
    static constexpr size_t kMinimumSamples = 8;
    static constexpr unsigned kLowerSamples = 16;
    static constexpr unsigned kEarlySamples = 8;
    struct Entry {
        uint32_t id = 0;
        int64_t submittedNs = 0;
        bool settling = false;
    };

    static bool validPeriod(int64_t value) { return value >= 1'000'000 && value <= 100'000'000; }
    static bool validTime(int64_t value) {
        return value > 0 && value < std::numeric_limits<int64_t>::max() - 4'000'000'000;
    }
    int64_t maximumLeadNs() const { return std::min<int64_t>(periodNs_ * 2, 50'000'000); }
    int64_t pauseGapNs() const { return std::max<int64_t>(100'000'000, periodNs_ * 3); }
    int64_t maximumAheadNs() const { return std::max<int64_t>(periodNs_ * 4, 50'000'000); }
    int64_t nextVsync(int64_t timeNs) const {
        if (phaseNs_ == 0) return timeNs;
        const int64_t delta = timeNs - phaseNs_;
        int64_t remainder = delta % periodNs_;
        if (remainder < 0) remainder += periodNs_;
        return remainder == 0 ? timeNs : timeNs + periodNs_ - remainder;
    }
    // Delivery may improve after the host-to-local offset was established.
    // Tolerate one period of jitter beyond the lead and selected mode's cushion.
    // Eight consecutive early frames distinguish sustained improvement from a
    // short burst. Remove only the smallest excess common to that run.
    void recoverEarlyTimeline(int64_t readyNs, int64_t &idealNs) {
        const int64_t cushionNs =
                (mode_ == Mode::Smoothness || mode_ == Mode::CapFps) ? periodNs_ : 0;
        const int64_t excessNs = idealNs - readyNs - leadNs_ - cushionNs;
        if (excessNs <= periodNs_) {
            earlyCount_ = 0;
            minimumEarlyNs_ = 0;
            return;
        }
        minimumEarlyNs_ = earlyCount_ == 0 ? excessNs : std::min(minimumEarlyNs_, excessNs);
        if (++earlyCount_ < kEarlySamples) return;
        anchorLocalNs_ -= minimumEarlyNs_;
        idealNs -= minimumEarlyNs_;
        earlyCount_ = 0;
        minimumEarlyNs_ = 0;
    }

    void clearTimeline() {
        earlyCount_ = 0;
        minimumEarlyNs_ = 0;
        anchored_ = false;
        lastHostUs_ = lastReadyNs_ = 0;
        lastPlan_ = {};
        pending_ = {};
        nextPending_ = 0;
        settling_ = 3;
    }
    void clearLearning() {
        needs_ = {};
        nextNeed_ = needCount_ = 0;
        lowerCount_ = 0;
    }
    void anchor(int64_t hostUs, int64_t readyNs) {
        clearTimeline();
        clearLearning();
        anchored_ = true;
        anchorHostUs_ = hostUs;
        anchorLocalNs_ = readyNs + leadNs_ +
                ((mode_ == Mode::Smoothness || mode_ == Mode::CapFps) ? periodNs_ : 0);
    }

    Mode mode_ = Mode::Immediate;
    int64_t periodNs_ = 16'666'667;
    int64_t leadNs_ = kInitialLeadNs;
    int64_t phaseNs_ = 0;
    int64_t feedbackPhaseNs_ = 0;
    bool anchored_ = false;
    int64_t anchorHostUs_ = 0;
    int64_t anchorLocalNs_ = 0;
    int64_t lastHostUs_ = 0;
    int64_t lastReadyNs_ = 0;
    Plan lastPlan_{};
    std::array<Entry, 64> pending_{};
    size_t nextPending_ = 0;
    unsigned settling_ = 3;
    std::array<int64_t, 32> needs_{};
    size_t nextNeed_ = 0;
    size_t needCount_ = 0;
    unsigned lowerCount_ = 0;
    unsigned earlyCount_ = 0;
    int64_t minimumEarlyNs_ = 0;
};
