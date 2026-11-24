// FramePacer core pacing, vsync selection, present-ahead commitment and partial deadlines.

#include "frame_pacer.h"
#include "frame_pacer_internal.h"

#include <algorithm>
#include <cmath>

namespace vkr {

using namespace pacer_detail;

FramePacer::FramePacer(PacingMode mode, int streamFps, int64_t vsyncPeriodNs, JitterBuffer jitterBuffer)
    : mode_(mode),
      streamIntervalNs_(1'000'000'000LL / std::max(streamFps, 1)),
      periodNs_(vsyncPeriodNs > 0 ? vsyncPeriodNs : 16'666'667),
      timeline_(jitterBuffer) {}

int64_t FramePacer::bufferCapNs() const {
    // Frames waiting out the buffer sit in the caller's queue (maxQueued()), which also holds
    // the frame due next and one being shown. Beyond what it holds, a bigger buffer only pushes
    // frames out of the queue unshown.
    const int64_t queueCapNs = static_cast<int64_t>(kMaxQueuedFrames - 3) * streamIntervalNs_;

    // In slots rather than stream frames: a game below the stream's frame rate sends a frame
    // a slot
    int64_t slots;
    switch (timeline_.jitterBuffer()) {
        case JitterBuffer::LowestLatency:
        case JitterBuffer::Instant:
            slots = kLowestLatencyCapSlots;
            break;
        case JitterBuffer::LowLatency:
            slots = kLowLatencyCapSlots;
            break;
        default:
            return queueCapNs;
    }
    return std::min(queueCapNs, slots * std::max(streamIntervalNs_, slotPeriodNs()));
}

void FramePacer::setVsyncPeriod(int64_t periodNs) {
    // Small differences are the caller's estimate being less exact than our own measurement.
    // A large one is a new refresh rate.
    if (periodNs > 0 && std::llabs(periodNs - periodNs_) * 50 > periodNs_) {
        periodNs_ = periodNs;
        vsyncs_.clear();
        resetPhase();
    }
}

void FramePacer::trackVsync(int64_t vsyncNs) {
    if (lastVsyncNs_ != 0) {
        // Count the vsyncs since the last callback, including any whose callbacks we missed
        vsyncIndex_ += std::max<int64_t>(1, (vsyncNs - lastVsyncNs_ + periodNs_ / 2) / periodNs_);
    }
    // A callback that missed vsyncs or came well off the period starts a refit now
    const int64_t sinceLastNs = vsyncNs - lastVsyncNs_;
    const bool offPlace = lastVsyncNs_ != 0 && periodNs_ > 0 &&
                          std::llabs(sinceLastNs - periodNs_) * 4 > periodNs_;
    lastVsyncNs_ = vsyncNs;

    vsyncs_.push_back({vsyncIndex_, vsyncNs});
    if (vsyncs_.size() > kVsyncWindow) {
        vsyncs_.pop_front();
    }
    if (vsyncs_.size() < kMinVsyncsForPeriod) {
        return;
    }
    if (!offPlace && ++vsyncsSinceFit_ < kRefitEveryVsyncs) {
        return;
    }
    vsyncsSinceFit_ = 0;

    // Least squares fit of vsync time against vsync index
    const VsyncSample& first = vsyncs_.front();
    double sx = 0, sy = 0, sxx = 0, sxy = 0;
    for (const VsyncSample& v : vsyncs_) {
        const double x = static_cast<double>(v.index - first.index);
        const double y = static_cast<double>(v.timeNs - first.timeNs);
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    const double n = static_cast<double>(vsyncs_.size());
    const double denominator = n * sxx - sx * sx;
    if (denominator <= 0) {
        return;
    }
    const int64_t measured = static_cast<int64_t>(std::llround((n * sxy - sx * sy) / denominator));
    if (std::llabs(measured - periodNs_) * 10 < periodNs_) {
        periodNs_ = measured;
    }
}

int FramePacer::heldVsyncs() const {
    // FramePacingPolicy.refreshesPerFrame's rule: a whole multiple to within half a hertz per multiple,
    // and nothing held where the screen refreshes when a frame comes or VRR is chosen
    if (adaptiveRefresh_ || periodNs_ <= 0 || streamIntervalNs_ <= 0) {
        return 1;
    }
    const double refreshHz = 1e9 / static_cast<double>(periodNs_);
    const double fps = 1e9 / static_cast<double>(streamIntervalNs_);
    for (int k = 1; k <= kMaxHeldVsyncs; k++) {
        if (std::fabs(refreshHz - fps * k) <= 0.5 * k) {
            return k;
        }
    }
    return 1;
}

size_t FramePacer::maxQueued() const {
    switch (mode_) {
        case PacingMode::MinLatency:
            return 1;
        case PacingMode::Balanced:
            return 2;
        case PacingMode::HostTimed: {
            // Enough to hold every frame inside the buffer we schedule with, plus one slot
            const int64_t held = timeline_.bufferNs() + scheduleDelayNs() + slotPeriodNs();
            const size_t frames = static_cast<size_t>(held / std::max<int64_t>(streamIntervalNs_, 1)) + 2;
            return std::min<size_t>(std::max<size_t>(frames, 2), kMaxQueuedFrames);
        }
        case PacingMode::CapFps:
        case PacingMode::Smoothness:
        default:
            return 4;
    }
}

void FramePacer::setJitterBuffer(JitterBuffer jitterBuffer) {
    if (jitterBuffer == timeline_.jitterBuffer()) {
        return;
    }
    timeline_.setJitterBuffer(jitterBuffer);
    resetPhase();
}


void FramePacer::onFrameArrived(FrameTiming& frame) {
    if (mode_ != PacingMode::HostTimed) {
        frame.targetNs = frame.arrivalNs;
        return;
    }

    if (!timeline_.checkContinuity(frame.hostPtsNs)) {
        ptsDeltas_.clear();
        lastPtsNs_ = 0;
        resetPhase();
    }

    updateSlot(frame);
    updatePhase(frame);
}

// Adds the frame to the jitter buffer, measured against its host timestamp or, with onLine, its
// place on the schedule carried along the fitted line (measuredPtsNs), and schedules it at
// scheduledPtsNs plus the buffer. Measured against the line itself while scheduled after it, the
// time between them was counted twice.
// Changing what the buffer measures against starts its window over, since the two don't mix.

int FramePacer::onVsync(int64_t vsyncNs, const FrameTiming* frames, size_t count) {
    trackVsync(vsyncNs);

    int choice = -1;
    switch (mode_) {
        case PacingMode::MinLatency:
            choice = count > 0 ? static_cast<int>(count) - 1 : -1;
            break;

        case PacingMode::Balanced:
            // Show frames in order, but no more often than the stream frame rate. This keeps
            // a stream below the refresh rate from alternating short and long frames.
            if (count > 0 && vsyncNs - lastPresentVsyncNs_ >= streamIntervalNs_ * 8 / 10) {
                choice = 0;
            }
            break;

        case PacingMode::CapFps:
        case PacingMode::Smoothness: {
            // Every frame in order, one a vsync. On a refresh 2 to 4 times the stream's rate (60 fps
            // on 120 Hz) a frame then went up at the next vsync after it came, so jitter left one up
            // for a vsync and the next for three, and no frame waited to cover it. There frames go up
            // no more often than the stream's rate, as in Balanced: an early one waits for its turn,
            // and frames that come in time stay the same number of vsyncs.
            if (count == 0) {
                break;
            }
            const int held = heldVsyncs();
            if (held <= 1) {
                earlySinceNs_ = 0;
                choice = 0;
                break;
            }
            const int64_t turnNs = streamIntervalNs_ * 8 / 10;
            const int64_t sinceLastNs = vsyncNs - lastPresentVsyncNs_;
            if (sinceLastNs < turnNs) {
                // A host whose clock runs a little ahead of the screen (60.000 fps on 119.88 Hz) brings
                // each frame a little earlier for its turn than the one before. As MediaCodec's turns do
                // (FramePacingPolicy.earlyForTurn), once frames have waited more than a vsync at their
                // turn for kEarlyForNs in a row, the oldest goes up at the vsync before its turn, which
                // leaves the frame before it up a vsync short, and the turns count from there. That
                // takes back a vsync every two seconds at most, about 4 ms a second at 120 Hz.
                const bool earlyTurn = sinceLastNs >= turnNs - periodNs_ && earlySinceNs_ != 0 &&
                                       vsyncNs - earlySinceNs_ >= kEarlyForNs && frames[0].arrivalNs > 0 &&
                                       vsyncNs - frames[0].arrivalNs >= kEarlySpareNs;
                // Nor does a frame wait more than held vsyncs past the first one it is there for, as
                // MediaCodec's frames wait at most a stream frame past the first refresh they can make
                // (FramePacingPolicy.heldTargetNs with everyFrame); of several that have, the newest goes
                // up, as there, rather than each a vsync after the last. A host further ahead than those
                // moves take back (60.000 fps on 119.1 Hz) brings each frame that old before its turn, and
                // it goes up then, a vsync early: before, the frames backed up until one gave way, 24 in
                // two minutes there. The early turns' timer runs on through this, as MediaCodec's does
                // through its limit: there this fires every 1.1-1.8 s, and clearing the timer kept the
                // early turns from ever firing, so frames waited 20.7 ms on average instead of 17-18.
                const int64_t longestNs = held * periodNs_;
                if (frames[0].arrivalNs > 0 && vsyncNs - frames[0].arrivalNs > longestNs) {
                    choice = 0;
                    while (static_cast<size_t>(choice) + 1 < count && frames[choice + 1].arrivalNs > 0 &&
                           vsyncNs - frames[choice + 1].arrivalNs > longestNs) {
                        choice++;
                    }
                }
                else if (earlyTurn) {
                    choice = 0;
                    earlySinceNs_ = 0;
                }
                break;
            }
            // At its turn the oldest frame goes up. Only when the one behind it is more than a stream
            // frame old too does it give way to it: two or more frames have backed up (a burst or a
            // stall), and the desktop likewise drops a picture more than a frame old once a newer one
            // is due (present::Pacer::take_due). One frame a turn never caught up: a burst kept its
            // backlog for good, and 60.000 fps on 119.88 Hz added a frame every 17 s until the queue
            // overflowed. Skipping any frame more than a stream frame old with a newer one behind it
            // dropped one every 17 s there instead, and steadily with frames up to 8 ms either side of
            // their time, which often come that old with the next one there.
            choice = 0;
            while (static_cast<size_t>(choice) + 1 < count && frames[choice + 1].arrivalNs > 0 &&
                   vsyncNs - frames[choice + 1].arrivalNs > streamIntervalNs_) {
                choice++;
            }
            if (frames[choice].arrivalNs <= 0 || vsyncNs - frames[choice].arrivalNs < periodNs_ + kEarlySpareNs) {
                earlySinceNs_ = 0;
            }
            else if (earlySinceNs_ == 0) {
                earlySinceNs_ = vsyncNs;
            }
            break;
        }

        case PacingMode::HostTimed: {
            if (count == 0) {
                break;
            }

            // Frames presented ahead are only shown from here when they can't be. A frame that
            // waited because its vsync was too far off to commit to (often one moved to the
            // next slot, see plannedVsyncNs()) can be committed now, and the caller does that
            // right after this, for the vsync planned for it. Shown from here, it went out a
            // vsync late: on a Pixel 10 Pro at 60 Hz, half of the frames held on screen an
            // extra vsync came right before one of these.
            if (presentAhead_ && frameVsyncNs(frames[0], true) != 0) {
                break;
            }

            const int64_t tolerance = periodNs_ / 4;
            int newestDue = -1;
            for (size_t i = 0; i < count; i++) {
                if (frames[i].targetNs + shiftNs_ + presentGuardNs() <= vsyncNs) {
                    newestDue = static_cast<int>(i);
                }
            }

            // Whether a new frame would normally be shown at this vsync, and whether the slot
            // before this one should have had one but didn't
            const int64_t sinceLast = vsyncNs - lastPresentVsyncNs_;
            const bool frameExpected = sinceLast * 2 >= (2 * slotVsyncs_ - 1) * periodNs_;
            const bool missedLastSlot = sinceLast * 2 >= (2 * slotVsyncs_ + 1) * periodNs_;

            if (phaseLocked_ && slotVsyncs_ > 1 && (vsyncIndex_ - slotParity_) % slotVsyncs_ != 0) {
                // Between slot vsyncs, frames wait for the next one, unless the frame on screen
                // has already had a full slot. That happens after a late frame, or when the
                // slot vsyncs just moved and the first new one has already passed; waiting for
                // the one after would hold this frame a whole extra slot and then skip one.
                // The oldest due frame keeps the sequence unbroken; any backlog is caught up
                // at the next slot vsync.
                if (frameExpected && newestDue >= 0) {
                    choice = 0;
                }
                break;
            }

            if (newestDue < 0) {
                // Nothing is due yet. If the oldest frame is only just short of due and the
                // stream is otherwise one frame per slot, show it now rather than repeat the
                // last frame here and have two frames due at the next vsync.
                if (!presentAhead_ && frameExpected && frames[0].targetNs + shiftNs_ <= vsyncNs + tolerance) {
                    choice = 0;
                }
            }
            else if (timeline_.jitterBuffer() == JitterBuffer::Instant) {
                // The newest frame, at the slot after the last one shown (see frameVsyncNs())
                if (slotVsyncs_ == 1 || lastPresentVsyncNs_ == 0 || frameExpected) {
                    choice = newestDue;
                }
            }
            else {
                choice = newestDue;

                // Two or more frames are due after a slot went empty. If the newest is only
                // barely due, show the one before it and leave the newest for the next vsync,
                // which gets the stream back to one frame per slot without skipping one.
                if (!presentAhead_ && newestDue >= 1 && missedLastSlot &&
                        frames[newestDue].targetNs + shiftNs_ > vsyncNs - tolerance) {
                    choice = newestDue - 1;
                }
            }
            break;
        }
    }

    if (choice >= 0) {
        framesSkipped_ += static_cast<uint64_t>(choice);
        lastPresentVsyncNs_ = vsyncNs;
    }
    return choice;
}

int64_t FramePacer::frameVsyncNs(const FrameTiming& frame, bool committable) const {
    if (mode_ != PacingMode::HostTimed || lastVsyncNs_ == 0) {
        return 0;
    }

    // The first vsync after the last one seen that the frame is due at, as onVsync() would
    // find it. A frame already due (it arrived late) goes to the next vsync.
    const int64_t due = frame.targetNs + shiftNs_ + presentGuardNs();
    int64_t ahead = 1;
    if (due > lastVsyncNs_ + periodNs_) {
        ahead = (due - lastVsyncNs_ + periodNs_ - 1) / periodNs_;
    }
    int64_t index = vsyncIndex_ + ahead;

    // Locked to slots: only slot vsyncs, unless the frame on screen by then will have had a
    // full slot (the catch-up onVsync() allows between slot vsyncs)
    if (phaseLocked_ && slotVsyncs_ > 1) {
        while (((index - slotParity_) % slotVsyncs_ + slotVsyncs_) % slotVsyncs_ != 0) {
            const int64_t vsync = lastVsyncNs_ + (index - vsyncIndex_) * periodNs_;
            if ((vsync - lastPresentVsyncNs_) * 2 >= (2 * slotVsyncs_ - 1) * periodNs_) {
                break;
            }
            index++;
        }
    }

    const bool instant = timeline_.jitterBuffer() == JitterBuffer::Instant;
    if (instant) {
        // Instant, with a frame every few vsyncs: the slot after the last frame shown, as the
        // stream's cadence has it, rather than whichever vsync the frame could make (Nightjar's
        // balanced pacing, and Moonlight's before it, show the newest frame at each such vsync).
        // Shown as soon as they could be, frames whose arrival sits near a vsync swap between
        // that vsync and the next, holding one for a vsync and the next for three. A frame
        // later than the slot goes to the vsync it can make, and frames that could all have
        // made the vsync before the slot go there (instantEarlyFrames_), so the slots follow
        // the frames a vsync at a time. With a frame a vsync, a frame for the same vsync as the
        // last replaces it: the newest is shown.
        if (slotVsyncs_ > 1 && lastPresentVsyncNs_ != 0) {
            const int64_t nextSlotIndex = vsyncIndexAt(lastPresentVsyncNs_ + slotVsyncs_ * periodNs_);
            const int64_t earliest = nextSlotIndex - (instantEarlyFrames_ >= kInstantEarlyFrames ? 1 : 0);
            index = std::max(index, earliest);
        }
    }

    // Unless the frames are locked to slots (whose schedule is already smooth), a frame whose
    // due time is right at a vsync would flip between that vsync and the next with the
    // slightest jitter: one frame replacing another at a vsync, then a vsync without a new frame.
    // Keep one frame per slot through that jitter; only a frame well off the slot moves.
    if (lastPresentVsyncNs_ != 0 && !instant && !(phaseLocked_ && slotVsyncs_ > 1)) {
        const int64_t tolerance = periodNs_ / 4;
        const int64_t planned = lastVsyncNs_ + (index - vsyncIndex_) * periodNs_;
        // The slot after the last frame committed, if it hasn't passed
        const int64_t nextSlot = lastPresentVsyncNs_ + slotVsyncs_ * periodNs_;
        const int64_t nextSlotIndex = vsyncIndex_ + (nextSlot - lastVsyncNs_ + periodNs_ / 2) / periodNs_;
        if (nextSlot > lastVsyncNs_ + periodNs_ / 2) {
            if (planned <= lastPresentVsyncNs_ + periodNs_ / 2 && due > lastPresentVsyncNs_ - tolerance) {
                // Only just due at the vsync the last frame was committed to: the next slot
                // rather than replacing that frame
                index = nextSlotIndex;
            }
            else if (planned >= nextSlot + periodNs_ / 2 && due <= nextSlot + tolerance) {
                // Only just short of the next slot: that slot rather than leaving it empty
                index = nextSlotIndex;
            }
        }
    }

    // Not further ahead than presentAheadVsyncs(). A frame due later is presented at the vsync
    // that brings its own within that, still that far early.
    if (committable && index - vsyncIndex_ > presentAheadVsyncs()) {
        return 0;
    }

    const int64_t vsync = lastVsyncNs_ + (index - vsyncIndex_) * periodNs_;
    if (vsync < lastPresentVsyncNs_ - periodNs_ / 2) {
        return 0;
    }
    // The same vsync as the last frame committed: that one arrived late and was put on the
    // next vsync, which is this frame's own. Two images asking for the same vsync, the
    // compositor shows the newer one, so this frame replaces it. Leaving this frame to its
    // vsync instead would present it with no time to spare, and it could miss the vsync too.
    return vsync;
}

void FramePacer::onPresentedAhead(int64_t vsyncNs) {
    // Replacing the frame committed to the same vsync (see plannedVsyncNs()): the compositor
    // shows only this one
    if (lastPresentVsyncNs_ != 0 && vsyncNs <= lastPresentVsyncNs_ + periodNs_ / 2) {
        framesSkipped_++;
    }
    // Instant: the slots moved a vsync sooner (frameVsyncNs()), so frames start counting again
    if (lastPresentVsyncNs_ != 0 && vsyncNs < lastPresentVsyncNs_ + slotVsyncs_ * periodNs_ - periodNs_ / 2) {
        instantEarlyFrames_ = 0;
    }
    lastPresentVsyncNs_ = vsyncNs;
}

void FramePacer::onPresentedImmediately(int64_t nowNs) {
    lastPresentVsyncNs_ = nowNs;
}

void FramePacer::addReadyCost(int64_t costNs) {
    readyCosts_.push_back(costNs);
    if (readyCosts_.size() > kPartialReadySamples) {
        readyCosts_.pop_front();
    }
}

bool FramePacer::partialDeadlineOffsetNs(const FrameTiming& frame, int64_t* offsetNs, int64_t* readyByNs,
                                         int64_t* readyCostNs) const {
    // Not through the warm-up: with no buffer then, nearly every frame would be cut
    if (mode_ != PacingMode::HostTimed || !timeline_.warmedUp() || readyCosts_.size() < kPartialMinReadySamples) {
        return false;
    }

    std::vector<int64_t>& costs = costScratch_;
    costs.assign(readyCosts_.begin(), readyCosts_.end());
    const auto percentile = costs.begin() + (costs.size() - 1) * kPartialReadyPercentile / 100;
    std::nth_element(costs.begin(), percentile, costs.end());

    // When the frame must be ready by to be shown on time. Presented ahead, that's the guard
    // before the vsync it's planned for, which can be most of a vsync after it's due (more with
    // several vsyncs a frame). Otherwise, it's when it's due. The frames after this one are
    // taken to have as long after their host timestamps as this one does.
    int64_t readyBy = frame.targetNs + shiftNs_;
    if (presentAhead_) {
        // Its vsync even if that's too far off to commit the frame to yet, as for a frame
        // that arrived early
        const int64_t vsyncNs = frameVsyncNs(frame, false);
        if (vsyncNs != 0) {
            readyBy = std::max(readyBy, vsyncNs - presentGuardNs());
        }
    }
    *offsetNs = readyBy - frame.hostPtsNs - *percentile;
    if (readyByNs) {
        *readyByNs = readyBy;
    }
    if (readyCostNs) {
        *readyCostNs = *percentile;
    }
    return true;
}


}  // namespace vkr

// Host unit tests (client/app/src/test/native/vulkan_frame_pacer_test.cpp) compile by
// #include'ing frame_pacer.cpp directly as a single translation unit. Android.mk defines
// VKR_FRAME_PACER_SEPARATE_TUS when compiling the split translation units separately.
#ifndef VKR_FRAME_PACER_SEPARATE_TUS
#include "frame_pacer_timeline.cpp"
#include "frame_pacer_phase.cpp"
#endif
