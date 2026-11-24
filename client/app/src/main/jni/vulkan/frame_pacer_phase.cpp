// FramePacer slot-interval detection, phase-line fitting and drift lock.

#include "frame_pacer.h"
#include "frame_pacer_internal.h"

#include <algorithm>
#include <cmath>

namespace vkr {

using namespace pacer_detail;

void FramePacer::resetPhase() {
    phaseSamples_.clear();
    frameIndex_ = 0;
    shiftNs_ = 0;
    slotParity_ = 0;
    phaseLocked_ = false;
    driftPerFrame_ = 0;
    lateNs_ = 0;
    soonerSinceNs_ = 0;
    earlySinceNs_ = 0;
    hasCachedFit_ = false;
    phaseFramesSinceFit_ = 0;
}


void FramePacer::scheduleFrom(FrameTiming& frame, int64_t measuredPtsNs, bool onLine, int64_t scheduledPtsNs) {
    if (onLine != transitOnLine_) {
        transitOnLine_ = onLine;
        timeline_.restartWindow(0, onLine ? kLineDecayDivisor : 0);
    }
    timeline_.setMaxBufferNs(bufferCapNs());
    timeline_.addSample(measuredPtsNs, frame.arrivalNs);
    frame.targetNs = scheduledPtsNs + timeline_.offsetNs();
}

// How many vsyncs each frame should stay on screen, from the spacing of host timestamps.
// This is the game's frame rate, which can be below the stream's: a game capped at 71.82 fps
// is two vsyncs a frame on a 143.64 Hz panel, whatever the stream is set to.
void FramePacer::updateSlot(const FrameTiming& frame) {
    if (lastPtsNs_ != 0) {
        const int64_t delta = frame.hostPtsNs - lastPtsNs_;
        if (delta > 0 && delta < 1'000'000'000) {
            ptsDeltas_.push_back(delta);
            if (ptsDeltas_.size() > kSlotDeltaWindow) {
                ptsDeltas_.pop_front();
            }
        }
    }
    lastPtsNs_ = frame.hostPtsNs;

    int64_t slot = 1;
    if (ptsDeltas_.size() >= 8) {
        // The mean spacing, since a fixed refresh host display shows frames at uneven
        // spacings around it (a 71.82 fps game on a 165 Hz display alternates two and three
        // refreshes). Gaps from a host hitch are left out.
        std::vector<int64_t>& sorted = deltaScratch_;
        sorted.assign(ptsDeltas_.begin(), ptsDeltas_.end());
        std::nth_element(sorted.begin(), sorted.begin() + sorted.size() / 2, sorted.end());
        const int64_t median = sorted[sorted.size() / 2];
        int64_t sum = 0;
        int64_t n = 0;
        for (int64_t delta : ptsDeltas_) {
            if (delta <= median * 2) {
                sum += delta;
                n++;
            }
        }
        const int64_t mean = sum / std::max<int64_t>(n, 1);

        // Only a whole number of vsyncs makes a slot. A frame rate between two of them has no
        // steady place on the vsync grid.
        const int64_t vsyncs = (mean + periodNs_ / 2) / periodNs_;
        if (vsyncs >= 1 && vsyncs <= 8 && std::llabs(mean - vsyncs * periodNs_) * 100 <= periodNs_ * 15) {
            slot = vsyncs;
        }
    }

    if (slot != slotVsyncs_) {
        slotVsyncs_ = slot;
        timeline_.setSlotVsyncs(slot);
        resetPhase();
    }
}

// Fits a line to the phase samples from index `from` on: the game's frame interval, which with
// host and client clocks only parts per million apart is the same on our clock, and how far the
// samples sit from it. Two estimates, since host timestamps come in two shapes:
// - a least squares fit, for timestamps scattered around the line (a 71.82 fps game on a 165 Hz
//   host, alternating two and three refreshes)
// - the median spacing, for timestamps that march exactly with the host display and step by a
//   refresh now and then (a 71.82 fps game on a 143.64 Hz host), which would tilt a least squares
//   line whenever a step is in the window
// Whichever leaves the frames packed tighter around its line wins. Residuals are measured from
// the sample at `from`.
bool FramePacer::fitPhase(size_t from, PhaseFit* out) const {
    if (from >= phaseSamples_.size()) {
        return false;
    }
    const PhaseSample& first = phaseSamples_[from];
    double sx = 0, sy = 0, sxx = 0, sxy = 0;
    std::vector<double>& spacings = spacingScratch_;
    spacings.clear();
    const PhaseSample* previous = nullptr;
    for (size_t i = from; i < phaseSamples_.size(); i++) {
        const PhaseSample& s = phaseSamples_[i];
        const double x = static_cast<double>(s.frameIndex - first.frameIndex);
        const double y = static_cast<double>(s.hostPtsNs - first.hostPtsNs);
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
        if (previous) {
            spacings.push_back(static_cast<double>(s.hostPtsNs - previous->hostPtsNs) /
                               static_cast<double>(s.frameIndex - previous->frameIndex));
        }
        previous = &s;
    }
    const double n = static_cast<double>(phaseSamples_.size() - from);
    const double denominator = n * sxx - sx * sx;
    if (denominator <= 0 || spacings.empty()) {
        return false;
    }
    std::nth_element(spacings.begin(), spacings.begin() + spacings.size() / 2, spacings.end());
    const double candidates[] = {(n * sxy - sx * sy) / denominator, spacings[spacings.size() / 2]};

    out->spread = 1e18;
    for (double candidate : candidates) {
        double high = -1e18;
        double low = 1e18;
        for (size_t i = from; i < phaseSamples_.size(); i++) {
            const PhaseSample& s = phaseSamples_[i];
            const double residual = static_cast<double>(s.hostPtsNs - first.hostPtsNs) -
                                    candidate * static_cast<double>(s.frameIndex - first.frameIndex);
            high = std::max(high, residual);
            low = std::min(low, residual);
        }
        if (high - low < out->spread) {
            out->spread = high - low;
            out->interval = candidate;
            out->low = low;
            out->latest = high;
        }
    }
    return true;
}

// Frames are due at the first vsync at or after their target time.
//
// Host timestamps vary even when the game's own pacing is perfect. With a fixed refresh host
// display, each frame's timestamp is the refresh at which the host showed it, which is up to a
// host refresh after the game presented it:
// - a game capped at 71.82 fps on a 144 Hz host is shown every second refresh, with an extra
//   refresh every 2.8 seconds, so its timestamps slide by a refresh and jump back
// - on a 165 Hz host they alternate between two and three refreshes
// - on a 143.64 Hz host, the cap and the display drift slowly against each other, and while
//   the game's presents sit near a refresh, frame time jitter lands them one refresh early or
//   late at random for seconds at a time
// Following those timestamps on our vsyncs reproduces the host display's judder.
//
// When the game runs at a steady rate, host timestamp against frame number is a straight line
// plus that jitter. We fit the line over several seconds and schedule each frame on it, with the
// jitter buffer measuring how late frames arrive against the line (scheduleFrom()). Frames then
// come out evenly, one per slot (the vsyncs each frame stays on screen), and a frame the host
// showed a refresh late is still in its own slot.
//
// The buffer once measured frames against their own timestamps, and the schedule sat on the line
// as late as the latest frame in the last 16 s: two extremes added, for frames that rarely are
// both stamped late and slow to arrive. At 60 fps on a 120 Hz Pixel 10 Pro with the Balanced
// buffer, that crept up to 30-40 ms; 99% of arrivals against the line came to 5-8 ms less. The host's frame rate is rarely exactly a whole fraction of our refresh
// rate, so the schedule drifts slowly against our vsyncs; we delay it, by less than a slot, to
// keep it clear of the vsyncs where frames change, and as the drift carries it up to one we move
// it past, which holds one frame a vsync longer or shorter: the least the rate difference
// requires.
//
// If the timestamps don't fit a line (the game's frame rate varies), frames are shown at their
// own host timestamps.
void FramePacer::updatePhase(FrameTiming& frame) {
    if (lastVsyncNs_ == 0) {
        scheduleFrom(frame, frame.hostPtsNs, false, frame.hostPtsNs);
        return;
    }

    // Instant: no buffer and no schedule. Each frame is due when it arrives, and goes to the
    // next vsync it can make (frameVsyncNs()). The timeline goes on learning, for switching back.
    if (timeline_.jitterBuffer() == JitterBuffer::Instant) {
        phaseSamples_.clear();
        phaseLocked_ = false;
        hasCachedFit_ = false;
        phaseFramesSinceFit_ = 0;
        shiftNs_ = 0;
        slotParity_ = 0;
        lateNs_ = 0;
        scheduleFrom(frame, frame.hostPtsNs, false, frame.hostPtsNs);
        frame.targetNs = frame.arrivalNs;

        // With a frame every few vsyncs, frames keep to the slot after the last one shown
        // (see frameVsyncNs()). Frames that could all have gone a vsync sooner move the slots
        // that way: the host drifting fast, or a late frame that moved them later.
        if (slotVsyncs_ > 1 && lastPresentVsyncNs_ != 0) {
            const int64_t vsyncBeforeSlot = lastPresentVsyncNs_ + (slotVsyncs_ - 1) * periodNs_;
            if (frame.arrivalNs + presentGuardNs() <= vsyncBeforeSlot - periodNs_ / 4) {
                instantEarlyFrames_++;
            }
            else {
                instantEarlyFrames_ = 0;
            }
        }
        return;
    }

    const int64_t slotPeriod = slotPeriodNs();

    // At a frame a vsync, Low/Lowest latency skip phase lock; at >=2 vsyncs/frame both lock.
    if (slotVsyncs_ == 1 && (timeline_.jitterBuffer() == JitterBuffer::LowLatency ||
                             timeline_.jitterBuffer() == JitterBuffer::LowestLatency ||
                             timeline_.jitterBuffer() == JitterBuffer::Instant)) {
        phaseSamples_.clear();
        phaseLocked_ = false;
        hasCachedFit_ = false;
        phaseFramesSinceFit_ = 0;
        shiftNs_ = 0;
        slotParity_ = 0;
        lateNs_ = 0;
        scheduleFrom(frame, frame.hostPtsNs, false, frame.hostPtsNs);
        return;
    }

    if (!phaseSamples_.empty()) {
        // A gap of about two slots or more means the host didn't send a frame. Gaps of one
        // and a half slots are a frame shown a host refresh late, which comes paired with a
        // half-slot gap when the game's presents sit near a host refresh: counting it as two
        // frames would add a frame for every such pair and wreck the fit.
        const int64_t delta = frame.hostPtsNs - phaseSamples_.back().hostPtsNs;
        frameIndex_ += std::max<int64_t>(1, (delta + slotPeriod * 3 / 10) / slotPeriod);
    }
    phaseSamples_.push_back({frame.arrivalNs, frameIndex_, frame.hostPtsNs});
    while (phaseSamples_.front().timeNs < frame.arrivalNs - kPhaseWindowNs) {
        phaseSamples_.pop_front();
    }

    auto unlock = [&]() {
        // Frames go up at their own host timestamps, and a delay would only add latency
        phaseLocked_ = false;
        hasCachedFit_ = false;
        phaseFramesSinceFit_ = 0;
        shiftNs_ = 0;
        slotParity_ = 0;
        lateNs_ = 0;
        scheduleFrom(frame, frame.hostPtsNs, false, frame.hostPtsNs);
    };

    if (frame.arrivalNs - phaseSamples_.front().timeNs < kMinPhaseSpanNs) {
        unlock();
        return;
    }

    // Throttle full phase refits to every kRefitEveryFrames while locked and tight, refitting
    // immediately if the incoming sample falls outside the cached [low, latest] residual bounds.
    PhaseFit fit;
    bool needFit = !hasCachedFit_ || !phaseLocked_ ||
                   cachedFit_.spread > kTrimSpread * static_cast<double>(slotPeriod) ||
                   ++phaseFramesSinceFit_ >= kRefitEveryFrames;
    if (!needFit) {
        const double r = static_cast<double>(frame.hostPtsNs - cachedFitAnchorPtsNs_) -
                         cachedFit_.interval * static_cast<double>(frameIndex_ - cachedFitAnchorIndex_);
        if (r < cachedFit_.low || r > cachedFit_.latest) {
            needFit = true;
        } else {
            const PhaseSample& f0 = phaseSamples_.front();
            const double shift = static_cast<double>(cachedFitAnchorPtsNs_ - f0.hostPtsNs) -
                                 cachedFit_.interval * static_cast<double>(cachedFitAnchorIndex_ - f0.frameIndex);
            fit = {cachedFit_.interval, cachedFit_.low + shift, cachedFit_.latest + shift, cachedFit_.spread};
        }
    }
    if (needFit) {
        if (!fitPhase(0, &fit)) {
            unlock();
            return;
        }
        cachedFit_ = fit;
        cachedFitAnchorIndex_ = phaseSamples_.front().frameIndex;
        cachedFitAnchorPtsNs_ = phaseSamples_.front().hostPtsNs;
        phaseFramesSinceFit_ = 0;
        hasCachedFit_ = true;
    }

    bool refitted = false;
    if (fit.spread > kTrimSpread * static_cast<double>(slotPeriod) &&
            frame.arrivalNs - phaseSamples_.front().timeNs > kRelockWindowNs + kMinPhaseSpanNs) {
        size_t recentStart = 0;
        while (phaseSamples_[recentStart].timeNs < frame.arrivalNs - kRelockWindowNs) {
            recentStart++;
        }
        PhaseFit recent;
        if (fitPhase(recentStart, &recent) && recent.spread < kTightSpread * static_cast<double>(slotPeriod)) {
            const PhaseSample& oldFirst = phaseSamples_.front();
            const PhaseSample& newFirst = phaseSamples_[recentStart];
            const double moved =
                    (static_cast<double>(newFirst.hostPtsNs) + recent.interval * static_cast<double>(frameIndex_ - newFirst.frameIndex)) -
                    (static_cast<double>(oldFirst.hostPtsNs) + fit.interval * static_cast<double>(frameIndex_ - oldFirst.frameIndex));
            if (transitOnLine_ && moved > 0) {
                timeline_.shift(-static_cast<int64_t>(std::llround(moved)));
            }
            phaseSamples_.erase(phaseSamples_.begin(), phaseSamples_.begin() + static_cast<std::ptrdiff_t>(recentStart));
            fit = recent;
            cachedFit_ = fit;
            cachedFitAnchorIndex_ = phaseSamples_.front().frameIndex;
            cachedFitAnchorPtsNs_ = phaseSamples_.front().hostPtsNs;
            phaseFramesSinceFit_ = 0;
            hasCachedFit_ = true;
            refitted = true;
        }
    }
    const PhaseSample& first = phaseSamples_.front();
    const double interval = fit.interval;
    const double latest = fit.latest;
    const double spread = fit.spread;

    driftPerFrame_ = (interval - static_cast<double>(slotPeriod)) / static_cast<double>(slotPeriod);
    if (std::fabs(driftPerFrame_) > kMaxDriftPerFrame ||
            spread / static_cast<double>(slotPeriod) > (phaseLocked_ ? kUnlockArc : kLockArc)) {
        unlock();
        // Start over from the last few seconds, so a burst of uneven frames stops counting
        // against the fit once it's over rather than after the whole window (seconds of steady
        // frames left unlocked on a Pixel 10 Pro)
        while (!phaseSamples_.empty() && phaseSamples_.front().timeNs < frame.arrivalNs - kRelockWindowNs) {
            phaseSamples_.pop_front();
        }
        return;
    }

    // Schedule this frame on the line. Balanced and Smooth measure the jitter buffer against the
    // line, which covers frames the host stamped late and frames slow to arrive in one percentile,
    // and caps both together (bufferCapNs()).
    // Low and Lowest latency, whose buffers cover less, schedule frames as late as the latest
    // recent one against the line and measure the buffer against frames' own timestamps: with
    // their buffer measured against the line, the frames at the start of each stretch of late
    // host timestamps came out late (up to 18 uneven frames in two minutes where 1 is needed,
    // and 30-50% more on a Pixel 10 Pro at 120 Hz).
    //
    // A frame landing later moves the schedule at once; once late frames stop, it comes back
    // slowly, since on a fixed refresh host they tend to return (the next time the game's
    // presents cross a host refresh). The schedule is carried from frame to frame so that
    // refitting the line doesn't move it.
    const bool againstLine = timeline_.jitterBuffer() == JitterBuffer::Balanced ||
                             timeline_.jitterBuffer() == JitterBuffer::Smooth;
    const double x = static_cast<double>(frameIndex_ - first.frameIndex);
    const double line = static_cast<double>(first.hostPtsNs) + interval * x;
    double scheduled = againstLine ? line : line + latest;
    if (phaseLocked_ && !refitted) {
        const double carried = scheduledPtsNs_ + interval * static_cast<double>(frameIndex_ - scheduledIndex_);
        if (scheduled < carried) {
            scheduled = std::max(scheduled, carried - static_cast<double>(slotPeriod) * kLateDecayPerFrame);
        }
    }
    else if (!phaseLocked_) {
        lockedSinceNs_ = frame.arrivalNs;
        soonerSinceNs_ = 0;
    }
    if (againstLine) {
        const int64_t scheduledNs = static_cast<int64_t>(std::llround(scheduled));
        scheduleFrom(frame, scheduledNs, true, scheduledNs);
    }
    else {
        scheduleFrom(frame, frame.hostPtsNs, false, static_cast<int64_t>(std::llround(scheduled)));

        // The delay after the line counts toward the preset's cap along with the buffer
        // (kLowestLatencyCapSlots), and gives way first. Frames the host stamped later than the
        // schedule then come out late, shown as they arrive.
        const double over = static_cast<double>(timeline_.bufferNs() - bufferCapNs()) + (scheduled - line);
        if (over > 0) {
            scheduled -= std::min(over, scheduled - line);
            frame.targetNs = static_cast<int64_t>(std::llround(scheduled)) + timeline_.offsetNs();
        }
    }
    scheduledPtsNs_ = scheduled;
    scheduledIndex_ = frameIndex_;
    lateNs_ = scheduled - line;
    phaseLocked_ = true;

    // Where the schedule sits within the slot, as a narrow arc that allows for the fit
    // wobbling as frames enter and leave the window
    const int64_t slotStartNs = lastVsyncNs_ - (vsyncIndex_ % slotVsyncs_) * periodNs_;
    const double width = kScheduleWidth;
    const double start = wrapCycles(static_cast<double>(frame.targetNs - slotStartNs) / slotPeriod - width / 2);
    const double margin = kPhaseMargin;

    // With the slot starting `parity` vsyncs later, the delay that clears the arc from the
    // vsyncs where frames change, and how long the last frame in the arc then waits
    struct Placement {
        double shift;
        double delay;
    };
    auto place = [&](int64_t parity) {
        const double s = wrapCycles(start - static_cast<double>(parity) / slotVsyncs_);
        double shift;
        if (s >= margin && s + width <= 1.0 - margin) {
            shift = 0.0;
        }
        else if (s < margin) {
            shift = margin - s;
        }
        else {
            shift = 1.0 + margin - s;
        }
        const double end = s + shift + width;
        return Placement {shift, shift + (std::ceil(end) - end)};
    };

    // Is the arc, as currently placed, still clear of the vsyncs?
    const double current = wrapCycles(start - static_cast<double>(slotParity_) / slotVsyncs_ +
                                      static_cast<double>(shiftNs_) / slotPeriod);
    const bool clear = current >= margin / 2 && current + width <= 1.0 - margin / 2;
    const double currentEnd = current + width;
    const double currentDelay = static_cast<double>(shiftNs_) / slotPeriod + (std::ceil(currentEnd) - currentEnd);

    // Which vsync the newest frame would be shown at under a placement. A new placement may
    // move frames by at most one vsync, in the direction the host drifts: one frame held a
    // vsync shorter (host running fast) or longer (host running slow). Otherwise a change
    // meant to gain a vsync could skip a frame and then hold another one long.
    auto shownAt = [&](int64_t parity, double shift) {
        const double x = static_cast<double>(vsyncIndex_) +
                (static_cast<double>(frame.targetNs - lastVsyncNs_) + shift * slotPeriod) / periodNs_;
        int64_t n = static_cast<int64_t>(std::ceil(x - 1e-6));
        while (((n - parity) % slotVsyncs_ + slotVsyncs_) % slotVsyncs_ != 0) {
            n++;
        }
        return n;
    };
    const int64_t shownNow = shownAt(slotParity_, static_cast<double>(shiftNs_) / slotPeriod);
    const int64_t preferredStep = driftPerFrame_ < 0 ? -1 : 1;

    bool found = false;
    int64_t bestParity = slotParity_;
    Placement best {0.0, 0.0};
    int bestRank = 0;
    for (int64_t parity = 0; parity < slotVsyncs_; parity++) {
        const Placement p = place(parity);
        const int64_t step = shownAt(parity, p.shift) - shownNow;
        // No move at all is best, then one vsync with the drift, then one against it
        int rank;
        if (step == 0) {
            rank = 0;
        }
        else if (step == preferredStep) {
            rank = 1;
        }
        else if (std::llabs(step) == 1) {
            rank = 2;
        }
        else {
            rank = 3 + static_cast<int>(std::llabs(step));
        }
        if (!found || rank < bestRank || (rank == bestRank && p.delay < best.delay)) {
            found = true;
            best = p;
            bestParity = parity;
            bestRank = rank;
        }
    }

    // Move when the arc is about to reach a vsync, or when another placement would show
    // frames at least half a vsync sooner for no more than one frame held a vsync short
    // Only right after locking: later, each such move would be a visible hitch traded for
    // under a vsync of latency
    bool muchSooner = false;
    if (clear && frame.arrivalNs - lockedSinceNs_ < kPlacementSettleNs) {
        for (int64_t parity = 0; parity < slotVsyncs_; parity++) {
            const Placement p = place(parity);
            const int64_t step = shownAt(parity, p.shift) - shownNow;
            if (step == -1 && slotVsyncs_ > 1 && p.delay + 0.5 / slotVsyncs_ < currentDelay) {
                muchSooner = true;
                best = p;
                bestParity = parity;
                break;
            }
        }
    }
    if (!clear || muchSooner) {
        slotParity_ = bestParity;
        shiftNs_ = static_cast<int64_t>(best.shift * slotPeriod);
        soonerSinceNs_ = 0;
        return;
    }

    // Settled, the slots only move when frames come too close to their vsync, which a growing
    // buffer does by moving them a vsync later. Once it shrinks again, nothing moved them back:
    // on an iPlay 70 mini Ultra streaming a 72 fps game at 144 Hz, a second of network jitter
    // grew the buffer from 2.4 to 5.7 ms, frames went a vsync later, and when it came back down
    // two seconds later they stayed there for the remaining 45 s, 7 ms later than they needed
    // to be. When every frame for kSoonerHoldNs could have made the vsync before its own, with
    // room to spare so that the jitter that just passed doesn't move them straight back, the
    // slots move there: one frame held a vsync short, for a vsync less delay from then on. Not
    // when the host runs slow enough to carry them back within kSoonerKeepNs, which would only
    // trade the vsync for a second uneven frame (a host 0.03% slow at 72 fps on 144 Hz, in
    // frame_pacer_test).
    const bool keepsSooner = driftPerFrame_ <= 0 ||
            1.0 / static_cast<double>(slotVsyncs_) / driftPerFrame_ * static_cast<double>(slotPeriod) >= kSoonerKeepNs;
    if (slotVsyncs_ > 1 && keepsSooner && frame.arrivalNs - lockedSinceNs_ >= kPlacementSettleNs) {
        const int64_t guard = presentGuardNs();
        const int64_t shownIndex = shownAt(slotParity_, static_cast<double>(shiftNs_ + guard) / slotPeriod);
        const int64_t soonerVsyncNs = lastVsyncNs_ + (shownIndex - 1 - vsyncIndex_) * periodNs_;
        const int64_t room = soonerVsyncNs - (frame.targetNs + shiftNs_ + guard);
        if (room < periodNs_ / kSoonerRoomDivisor) {
            soonerSinceNs_ = 0;
        }
        else if (soonerSinceNs_ == 0) {
            soonerSinceNs_ = frame.arrivalNs;
        }
        else if (frame.arrivalNs - soonerSinceNs_ >= kSoonerHoldNs) {
            slotParity_ = (slotParity_ + slotVsyncs_ - 1) % slotVsyncs_;
            soonerSinceNs_ = 0;
        }
    }
    else {
        soonerSinceNs_ = 0;
    }
}


}  // namespace vkr
