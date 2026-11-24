// VulkanRenderer image-reader callback, Choreographer vsync loop, aligned front-buffer
// scheduling, present-ahead, keep-alive and Frame Warp redraws.
// Adapted from MoreOrLessSoftware/moonlight-android (see SOURCE).

#include "vulkan_renderer_internal.h"

namespace vkr {

using namespace renderer_detail;

// ---------------------------------------------------------------------------------------------
// Decoder side

void VulkanRenderer::onImageAvailableThunk(void* context, AImageReader*) {
    static_cast<VulkanRenderer*>(context)->onImageAvailable();
}

void VulkanRenderer::onImageAvailable() {
    // This runs on the image reader's own thread. Frames are timed from when it acquires them,
    // so it gets the same priority as the render thread.
    static thread_local bool prioritized = false;
    if (!prioritized) {
        prioritized = true;
        raiseThreadPriority("image reader");
    }
    static thread_local const pid_t tid = gettid();
    if (imageReaderTid_.load(std::memory_order_relaxed) != tid && imageReaderTid_.exchange(tid) != tid) {
        perfHint_.addThread();
    }
    bool presentDirect = false;
    for (;;) {
        if (closing_.load(std::memory_order_acquire)) return;

        // The image comes with a fence for the decoder finishing writing it, which we wait on
        // here. AImageReader_acquireNextImage() waits on it itself, but holding the reader's
        // lock, which AImage_delete() needs too: releasing a frame we'd shown (renderFrame())
        // then waited for the next one to decode. On an iPlay 70 mini Ultra that blocked
        // renders for 1-5 ms, and with a present delay of one vsync, 6-9% of frames reached
        // the screen a vsync late, in bursts of seconds.
        AImage* image = nullptr;
        int fenceFd = -1;
        media_status_t status = ndk_->AImageReader_acquireNextImageAsync
                ? ndk_->AImageReader_acquireNextImageAsync(reader_, &image, &fenceFd)
                : ndk_->AImageReader_acquireNextImage(reader_, &image);
        if (status != AMEDIA_OK) {
            if (status == AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED) {
                ALOGW("All reader images are held");
            }
            break;
        }
        // Presenting on arrival with the GPU wait, the frame goes on with its fence and the GPU
        // waits for it (waitForDecoder()), so its draw is recorded and queued while the decoder
        // finishes; its arrival is then when the decoder handed it over, not when it was written.
        const bool gpuWaits = fenceFd >= 0 && gpuDecodeWait_ && presentOnArrival_;
        if (fenceFd >= 0 && !gpuWaits) {
            pollfd fence {fenceFd, POLLIN, 0};
            int ready;
            do {
                ready = poll(&fence, 1, static_cast<int>(kFenceTimeoutNs / 1'000'000));
            } while (ready < 0 && errno == EINTR);
            close(fenceFd);
            if (ready <= 0) {
                ALOGE("Timed out waiting for the decoder to write a frame");
                ndk_->AImage_delete(image);
                continue;
            }
        }

        auto frame = std::make_shared<VideoFrame>(ndk_, image);
        if (gpuWaits) frame->decodeFence.reset(fenceFd);
        frame->timing.arrivalNs = nowNs();
        if (ndk_->AImage_getHardwareBuffer(image, &frame->buffer) != AMEDIA_OK || !frame->buffer) {
            ALOGE("Image has no hardware buffer");
            continue;
        }
        // MediaCodec stamps each image with the frame's presentation time, which is the host's
        // timestamp for the frame
        ndk_->AImage_getTimestamp(image, &frame->timing.hostPtsNs);
        if (warpApi_) {
            // The frame number Java noted for the time the decoder was given the frame with
            frame->frameNumber = warpApi_->frame_for_pts(frame->timing.hostPtsNs / 1000);
        }
        frame->reporter = this;
        if (ndk_->AImage_getCropRect(image, &frame->crop) != AMEDIA_OK) {
            frame->crop = {0, 0, 0, 0};
        }

        if (enqueueFrame(std::move(frame))) {
            presentDirect = true;
        }
    }
    if (presentDirect) {
        // Direct on-arrival present after draining back-to-back images (avoids 1-3+ ms eventfd wakeup).
        std::lock_guard<std::mutex> renderLock(renderMutex_);
        if (!renderStopped_) presentNewest();
    }
}

bool VulkanRenderer::enqueueFrame(FramePtr frame) {
    noteDecoded(*frame);

    std::deque<FramePtr> dropped;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (closing_.load(std::memory_order_acquire)) return false;
        pacer_.onFrameArrived(frame->timing);
        trace_.frame(frame->timing, pending_.size() + 1, queueOverflowDrops_);
        pending_.push_back(std::move(frame));
        while (pending_.size() > pacer_.maxQueued()) {
            dropped.push_back(std::move(pending_.front()));
            pending_.pop_front();
            queueOverflowDrops_++;
        }
    }

    bool presentDirect = false;
    if (presentAhead_ && !presentOnArrival_) {
        std::lock_guard<std::mutex> renderLock(renderMutex_);
        if (!renderStopped_) presentAhead();
    } else if (presentOnArrival_) {
        if (!frontBufferAligned_) presentDirect = true;
        else wake(kWakeFrame);
    }
    if (vsyncParked_.load(std::memory_order_relaxed)) wake(kWakeVsync);
    return presentDirect;
}

// ---------------------------------------------------------------------------------------------
// Render thread

void VulkanRenderer::renderThreadMain() {
    // A vsync callback that runs late can miss the compositor's deadline, and the frame reaches
    // the screen a vsync late. Android runs its own render threads at display priority.
    raiseThreadPriority("render");
    perfHint_.addThread();

    ALooper* looper = ALooper_prepare(0);
    ALooper_addFd(looper, wakeFd_, kLooperIdWake, ALOOPER_EVENT_INPUT, &VulkanRenderer::onWakeThunk, this);

    choreographer_ = ndk_->AChoreographer_getInstance();
    if (!choreographer_) {
        ALOGE("No Choreographer on the render thread");
        ALooper_removeFd(looper, wakeFd_);
        return;
    }
    if (ndk_->AChoreographer_registerRefreshRateCallback) {
        ndk_->AChoreographer_registerRefreshRateCallback(choreographer_, &VulkanRenderer::onRefreshRateThunk, this);
        refreshRateCallbackRegistered_ = true;
    }
    ndk_->AChoreographer_postFrameCallback64(choreographer_, &VulkanRenderer::onVsyncThunk, this);

    if (frontBufferAligned_) {
        alignedTimerFd_ = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
        if (alignedTimerFd_ >= 0) {
            ALooper_addFd(looper, alignedTimerFd_, kLooperIdAlignedTimer, ALOOPER_EVENT_INPUT,
                          &VulkanRenderer::onAlignedTimerThunk, this);
        }
        else {
            ALOGW("timerfd_create failed; the front buffer won't be aligned to the scan");
        }
    }

    while (!quit_) {
        ALooper_pollOnce(-1, nullptr, nullptr, nullptr);
    }

    if (refreshRateCallbackRegistered_) {
        ndk_->AChoreographer_unregisterRefreshRateCallback(choreographer_, &VulkanRenderer::onRefreshRateThunk, this);
    }
    ALooper_removeFd(looper, wakeFd_);
    if (alignedTimerFd_ >= 0) {
        ALooper_removeFd(looper, alignedTimerFd_);
        close(alignedTimerFd_);
        alignedTimerFd_ = -1;
    }
}

void VulkanRenderer::onVsyncThunk(int64_t frameTimeNanos, void* data) {
    static_cast<VulkanRenderer*>(data)->onVsync(frameTimeNanos);
}

void VulkanRenderer::onRefreshRateThunk(int64_t vsyncPeriodNanos, void* data) {
    auto* self = static_cast<VulkanRenderer*>(data);
    std::lock_guard<std::mutex> lock(self->mutex_);
    self->trace_.period(vsyncPeriodNanos);
    self->pacer_.setVsyncPeriod(vsyncPeriodNanos);
}

int VulkanRenderer::onWakeThunk(int fd, int, void* data) {
    uint64_t value;
    (void) !read(fd, &value, sizeof(value));
    static_cast<VulkanRenderer*>(data)->onWake();
    return 1;
}

void VulkanRenderer::onWake() {
    const uint32_t flags = wakeFlags_.exchange(0);
    if (quit_ || (flags & kWakeQuit)) {
        return;
    }
    if (vsyncParked_.exchange(false)) {
        ndk_->AChoreographer_postFrameCallback64(choreographer_, &VulkanRenderer::onVsyncThunk, this);
    }
    std::lock_guard<std::mutex> renderLock(renderMutex_);

    if (flags & kWakeHdr) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (hdrChanged_) {
            hdrChanged_ = false;
            hdrActive_ = hdrEnabled_;
            // New swapchain format and color space, and a new YCbCr model for the frames
            swapchainDirty_ = true;
        }
    }

    if ((flags & kWakeFrame) && presentAhead_ && !presentOnArrival_) {
        presentAhead();
    }
    else if ((flags & kWakeFrame) && frontBuffer_ && frontBufferAligned_) {
        scheduleAlignedDraw();
    }
    else if (flags & kWakeFrame) {
        // Lowest latency with a mailbox swapchain: show the newest frame right away and let
        // the display take whichever was presented last before each vsync
        presentNewest();
    }
    else if ((flags & kWakeHdr) && current_ && renderFrame(current_)) {
        // Redrawn in the new output format
    }
}

void VulkanRenderer::scheduleAlignedDraw() {
    if (alignedTimerSet_) {
        // The draw already set takes the newest frame when it runs
        return;
    }
    if (lastVsyncNs_ == 0 || alignedTimerFd_ < 0) {
        presentNewest();
        return;
    }
    int64_t periodNs;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        periodNs = pacer_.vsyncPeriodNs();
    }

    // The first scan after now that there's still time to start drawing for, counted on from
    // the last vsync callback (Choreographer's vsync times, less the app vsync offset the lead
    // allows for, match when the display shows a frame to within 0.2 ms on an iPlay 70 mini Ultra)
    const int64_t leadNs = alignedLeadNs();
    const int64_t now = nowNs();
    int64_t vsyncNs = lastVsyncNs_ + periodNs;
    if (vsyncNs - leadNs <= now) {
        vsyncNs += ((now - (vsyncNs - leadNs)) / periodNs + 1) * periodNs;
    }
    const int64_t startNs = vsyncNs - leadNs;

    itimerspec spec {};
    spec.it_value.tv_sec = static_cast<time_t>(startNs / 1'000'000'000);
    spec.it_value.tv_nsec = static_cast<long>(startNs % 1'000'000'000);
    if (timerfd_settime(alignedTimerFd_, TFD_TIMER_ABSTIME, &spec, nullptr) != 0) {
        presentNewest();
        return;
    }
    alignedTimerSet_ = true;
    alignedVsyncNs_ = vsyncNs;
}

int64_t VulkanRenderer::alignedLeadNs() const {
    // Only the offset's phase within a refresh matters: one negative or over a refresh long
    // still puts the scan the same distance before a frame time
    int64_t offsetNs = vsyncOffsetNs_;
    const int64_t periodNs = offsetPeriodNs_;
    if (periodNs > 0) {
        offsetNs = (offsetNs % periodNs + periodNs) % periodNs;
    }
    if (fixedAlignedLeadNs_ >= 0) {
        return fixedAlignedLeadNs_ + offsetNs;
    }
    if (drawSubmitNs_.size() < 8) {
        return kAlignedDefaultLeadNs + offsetNs;
    }
    // Called each vsync and draw: a reused buffer, so the render thread doesn't allocate for it
    std::vector<int64_t>& sorted = leadScratch_;
    sorted.assign(drawSubmitNs_.begin(), drawSubmitNs_.end());
    const auto p90 = sorted.begin() + (sorted.size() - 1) * 9 / 10;
    std::nth_element(sorted.begin(), p90, sorted.end());
    return *p90 + kAlignedGpuStartNs + offsetNs;
}

int VulkanRenderer::onAlignedTimerThunk(int fd, int, void* data) {
    uint64_t expirations;
    (void) !read(fd, &expirations, sizeof(expirations));
    auto* self = static_cast<VulkanRenderer*>(data);
    if (!self->quit_) {
        std::lock_guard<std::mutex> renderLock(self->renderMutex_);
        self->alignedTimerSet_ = false;
        if (self->frontBufferPaced_) {
            self->drawDueAligned();
        }
        else {
            self->presentNewest(self->alignedVsyncNs_);
        }
    }
    return 1;
}

void VulkanRenderer::presentNewest(int64_t targetVsyncNs) {
    FramePtr frame;
    uint64_t presentId = 0;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (pending_.empty()) {
            return;
        }
        frame = std::move(pending_.back());
        pending_.clear();
        const int64_t now = nowNs();
        pacer_.onPresentedImmediately(now);

        // Tagged to find out when it reached the screen, for traces and the capture to present
        // statistic. A front buffer is never presented as a new image, so it gets no reports;
        // its capture to present is counted up to the present.
        if (hasDisplayTimingExt_ && !frontBuffer_) {
            presentId = nextPresentId_++;
        }
        trace_.immediate(now, frame->timing.hostPtsNs, presentId, targetVsyncNs);
    }
    if (renderFrame(frame, presentId, 0, false, 0, true)) {
        presentedFrames_++;
    }
}

void VulkanRenderer::trackVsyncPeriod(int64_t frameTimeNanos) {
    // Refined even with the refresh rate callback, which only reports changes
    if (lastVsyncNs_ != 0) {
        const int64_t delta = frameTimeNanos - lastVsyncNs_;
        vsyncDeltas_.push_back(delta);
        if (vsyncDeltas_.size() > 60) {
            vsyncDeltas_.pop_front();
        }

        std::lock_guard<std::mutex> lock(mutex_);
        const int64_t period = pacer_.vsyncPeriodNs();
        if (delta > period * 3 / 4 && delta < period * 5 / 4) {
            const int64_t refined = period + (delta - period) / 16;
            trace_.period(refined);
            pacer_.setVsyncPeriod(refined);
        }
        else if (!refreshRateCallbackRegistered_ && vsyncDeltas_.size() == 60) {
            // Without the refresh rate callback, the refresh rate changed if most recent
            // intervals are far off. A single short one isn't enough: callbacks bunch up after
            // a late one, and taking that for a new refresh rate threw the pacer off.
            std::vector<int64_t>& sorted = sortScratch_;
            sorted.assign(vsyncDeltas_.begin(), vsyncDeltas_.end());
            std::nth_element(sorted.begin(), sorted.begin() + sorted.size() / 2, sorted.end());
            const int64_t median = sorted[sorted.size() / 2];
            if (median < period * 3 / 4 || median > period * 5 / 4) {
                ALOGI("Vsync period changed to %" PRId64 " ns", median);
                trace_.period(median);
                pacer_.setVsyncPeriod(median);
                vsyncDeltas_.clear();
            }
        }
        offsetPeriodNs_ = pacer_.vsyncPeriodNs();
    }
    lastVsyncNs_ = frameTimeNanos;
}

void VulkanRenderer::onVsync(int64_t frameTimeNanos) {
    if (quit_) {
        return;
    }

    const int64_t callbackLateNs = nowNs() - frameTimeNanos;
    std::unique_lock<std::mutex> renderLock(renderMutex_);
    trackVsyncPeriod(frameTimeNanos);
    if (frontBufferPaced_) {
        // A frame is committed to a scan only if it can be drawn in time for it
        const int64_t leadNs = alignedLeadNs();
        std::lock_guard<std::mutex> lock(mutex_);
        pacer_.setPresentGuardNs(leadNs);
    }

    FramePtr frame;
    uint64_t presentId = 0;
    std::deque<FramePtr> skipped;
    if (!presentOnArrival_) {
        std::lock_guard<std::mutex> lock(mutex_);
        FrameTiming timings[FramePacer::kMaxQueuedFrames];
        const size_t count = std::min(pending_.size(), FramePacer::kMaxQueuedFrames);
        for (size_t i = 0; i < count; i++) {
            timings[i] = pending_[i]->timing;
        }

        const int choice = pacer_.onVsync(frameTimeNanos, timings, count);
        if (choice >= 0 && hasDisplayTimingExt_) {
            presentId = nextPresentId_++;
        }
        currentVsyncNs_ = frameTimeNanos;
        currentPeriodNs_ = pacer_.vsyncPeriodNs();
        trace_.vsync(frameTimeNanos, count, choice, choice >= 0 ? timings[choice].hostPtsNs : 0, pacer_,
                     callbackLateNs, presentId, hasDisplayTimingExt_ ? presentScheduler_.delayVsyncs() : 0);
        if (choice >= 0) {
            for (int i = 0; i < choice; i++) {
                skipped.push_back(std::move(pending_.front()));
                pending_.pop_front();
            }
            frame = std::move(pending_.front());
            pending_.pop_front();
        }
    }

    if (frame) {
        if (frontBufferPaced_ && !alignedDraws_.empty()) {
            // Older frames waiting for later scans would be drawn over this one
            alignedDraws_.clear();
            setAlignedTimer();
        }
        if (renderFrame(frame, presentId)) {
            presentedFrames_++;
        }
    }
    else if (swapchainDirty_ && current_) {
        // Keep the picture up across a swapchain rebuild
        renderFrame(current_);
    }
    if (presentAhead_ && !presentOnArrival_) {
        presentAhead();
    }
    if (!frame && !warpRefresh(frameTimeNanos)) {
        keepAlive();
    }
    collectPresentTimings();

    if (canParkVsync()) {
        // Nothing to show or report: no vsync callback until a frame arrives (enqueueFrame()) or
        // something else wakes the thread (onWake())
        vsyncParked_.store(true);
        // A frame that came in between the check and the flag found it unset
        std::lock_guard<std::mutex> lock(mutex_);
        if (pending_.empty()) {
            return;
        }
        vsyncParked_.store(false);
    }
    ndk_->AChoreographer_postFrameCallback64(choreographer_, &VulkanRenderer::onVsyncThunk, this);
}

// Whether the vsync callbacks can stop until the next frame arrives: no frame waiting, no present
// still to be reported, and none of the work that runs at every vsync (keep-alive, frame warp,
// aligned draws, a swapchain to rebuild), and no new frame shown for
// kKeepAliveAfterNs (the stream has paused).
bool VulkanRenderer::canParkVsync() {
    if (keepAliveNs_ > 0 || warpApi_ || swapchainDirty_ || !alignedDraws_.empty() || quit_) {
        return false;
    }
    if (!pendingPresents_.empty() && nowNs() - lastPresentNs_ < kParkAfterPresentNs) {
        return false;
    }
    // Not while the stream is running: a 60 fps stream on a 120 Hz screen finds nothing pending at
    // every other vsync, and a frame arriving after parking waits for the thread to wake before a
    // callback is posted (1 ms at the median, over 3 ms one time in ten; see enqueueFrame()), so one
    // arriving inside that margin would show a vsync late
    if (nowNs() - lastNewFrameNs_ < kKeepAliveAfterNs) {
        return false;
    }
    std::lock_guard<std::mutex> lock(mutex_);
    return pending_.empty();
}

// While the host sends nothing (a game paused, or its desktop with the frame rate allowed to drop
// to zero), presents the frame on screen again every keepAliveNs_. Left idle, the compositor was
// slow to show the next frame: on an iPlay 70 mini Ultra streaming PyroWave at 144 fps, the first
// frames after the host had been quiet for a few hundred milliseconds reached the screen 4-13
// vsyncs late, though rendered 2-3 ms after arriving and presented before their vsync, and the
// compositor reported they couldn't have been sooner. Each costs one render (about 1 ms on the
// GPU), only while idle. Stamped with the time it's presented, like a frame shown on arrival, so
// the compositor drops it if a new frame is due at the same vsync.
void VulkanRenderer::keepAlive() {
    // Without a time to present it at, a FIFO swapchain would show it ahead of the next new frame
    // presented for the same vsync, a vsync late
    // A front buffer is taken again at every vsync anyway
    if (keepAliveNs_ <= 0 || !current_ || swapchainDirty_ || frontBuffer_ ||
            !(hasDisplayTimingExt_ || presentOnArrival_)) {
        return;
    }
    const int64_t now = nowNs();
    if (now - lastNewFrameNs_ < kKeepAliveAfterNs || now - lastPresentNs_ < keepAliveNs_) {
        return;
    }

    uint64_t presentId = 0;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!pending_.empty()) {
            // A frame waiting for its vsync, which will be presented then
            return;
        }
        if (hasDisplayTimingExt_) {
            presentId = nextPresentId_++;
        }
        trace_.keepAlive(now, presentId);
    }
    renderFrame(current_, presentId, 0, false, 0, true);
}

// Frame Warp between the host's pictures: the one on screen is presented again, shifted for the
// turn since, when its shift moved by kFrameWarpRedrawPx or more, so a turn shows at every refresh and
// not only with the host's frames. Stamped like a keep-alive present, so a new picture due at the
// same vsync replaces it; nothing is drawn while a new picture waits for its vsync, after one was
// presented since this vsync, or before the vsync the last one asked for. Each costs one render,
// only while the view turns.
bool VulkanRenderer::warpRefresh(int64_t vsyncNs) {
    // Without a time to present it at, a FIFO swapchain would show it ahead of the next new frame
    // presented for the same vsync, a vsync late; a front buffer is not presented again
    if (!warpApi_ || !current_ || swapchainDirty_ || frontBuffer_ || !(hasDisplayTimingExt_ || presentOnArrival_) ||
            lastPresentNs_ >= vsyncNs || warpDueNs_ > vsyncNs + currentPeriodNs_ / 4) {
        return false;
    }
    // Without the stream's lock first: nothing can have moved unless the model is locked and the
    // view turns (FrameWarpDrawn::mayMove())
    if (!warpDrawn_.mayMove(warpApi_)) {
        return false;
    }
    // Peeked, so a refresh that draws nothing is not counted; renderFrame() takes the shift it draws
    float out[FRAME_WARP_FLOATS] = {};
    const bool active = warpApi_->peek(current_->frameNumber, out) != FRAME_WARP_INACTIVE;
    if (!frameWarpMoved(warpDrawn_.px[0], warpDrawn_.px[1], active ? out[2] : 0.0f, active ? out[3] : 0.0f)) {
        return false;
    }

    uint64_t presentId = 0;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!pending_.empty()) {
            // A new picture waiting for its vsync, which will be presented then
            return false;
        }
        if (hasDisplayTimingExt_) {
            presentId = nextPresentId_++;
        }
        trace_.keepAlive(nowNs(), presentId);
    }
    return renderFrame(current_, presentId, 0, false, 0, true);
}

// Presenting ahead draws a frame as it arrives for a vsync up to a few ahead, and Frame Warp's
// shift is taken as it is drawn: the turn until that vsync would be missing from it, and the
// redraws between pictures wait for that vsync (warpDueNs_), so each picture would step by up to
// a frame's turn. While the host's model is locked, frames wait for their vsync instead
// (onVsync()), drawn then for the next refresh. An aligned front buffer draws ahead-presented
// frames at their scan, so it keeps presenting ahead.
bool VulkanRenderer::presentsAhead() const {
    return presentAhead_ && (!warpHoldsAhead_ || frontBufferPaced_);
}

void VulkanRenderer::presentAhead() {
    if (!presentsAhead()) {
        return;
    }
    // In order, as long as each waiting frame's vsync is known and comes after the last one
    // committed. Anything else waits for its vsync.
    for (;;) {
        FramePtr frame;
        uint64_t presentId = 0;
        int64_t showVsyncNs = 0;
        int64_t commitNs = 0;
        {
            std::lock_guard<std::mutex> lock(mutex_);
            if (pending_.empty()) {
                return;
            }
            showVsyncNs = pacer_.plannedVsyncNs(pending_.front()->timing);
            if (showVsyncNs == 0) {
                return;
            }
            pacer_.onPresentedAhead(showVsyncNs);
            frame = std::move(pending_.front());
            pending_.pop_front();
            presentId = nextPresentId_++;

            // Replacing the last frame presented ahead at the same vsync: the compositor won't
            // show that one, by design
            if (lastAheadPresentId_ != 0 && showVsyncNs <= lastAheadVsyncNs_ + pacer_.vsyncPeriodNs() / 2) {
                if (PendingPresent* previous = pendingPresents_.find(lastAheadPresentId_)) {
                    previous->replaced = true;
                }
            }
            lastAheadPresentId_ = presentId;
            lastAheadVsyncNs_ = showVsyncNs;
            currentPeriodNs_ = pacer_.vsyncPeriodNs();
            commitNs = nowNs();
            trace_.ahead(commitNs, showVsyncNs, frame->timing.hostPtsNs, presentId, presentScheduler_.delayVsyncs(),
                         pacer_);
        }
        if (frontBufferPaced_) {
            queueAlignedDraw({std::move(frame), presentId, showVsyncNs, commitNs});
        }
        else if (renderFrame(frame, presentId, showVsyncNs, true, commitNs)) {
            presentedFrames_++;
        }
    }
}

void VulkanRenderer::queueAlignedDraw(AlignedDraw draw) {
    // Replacing the frame committed to the same scan, which the pacer counted as skipped
    const int64_t periodNs = currentPeriodNs_;
    while (!alignedDraws_.empty() && alignedDraws_.back().vsyncNs >= draw.vsyncNs - periodNs / 2) {
        alignedDraws_.pop_back();
    }
    alignedDraws_.push_back(std::move(draw));
    setAlignedTimer();
}

void VulkanRenderer::drawDueAligned() {
    // The newest draw whose time has come; any before it (the timer ran late) would only be
    // drawn over
    const int64_t dueNs = nowNs() + kAlignedTimerSlackNs + alignedLeadNs();
    bool found = false;
    AlignedDraw draw;
    while (!alignedDraws_.empty() && alignedDraws_.front().vsyncNs <= dueNs) {
        draw = std::move(alignedDraws_.front());
        alignedDraws_.pop_front();
        found = true;
    }
    if (found && renderFrame(draw.frame, draw.presentId, draw.vsyncNs, true, draw.commitNs)) {
        presentedFrames_++;
    }
    setAlignedTimer();
}

void VulkanRenderer::setAlignedTimer() {
    if (alignedTimerFd_ < 0) {
        return;
    }
    itimerspec spec {};
    if (!alignedDraws_.empty()) {
        // A time already past fires at once
        const int64_t startNs = std::max<int64_t>(alignedDraws_.front().vsyncNs - alignedLeadNs(), 1);
        spec.it_value.tv_sec = static_cast<time_t>(startNs / 1'000'000'000);
        spec.it_value.tv_nsec = static_cast<long>(startNs % 1'000'000'000);
    }
    timerfd_settime(alignedTimerFd_, TFD_TIMER_ABSTIME, &spec, nullptr);
    alignedTimerSet_ = !alignedDraws_.empty();
}


}  // namespace vkr
