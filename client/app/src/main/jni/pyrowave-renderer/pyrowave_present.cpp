// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// PyroWave frame/chunk submission, decode worker, present worker and draw command recording.

#include "pyrowave_renderer_internal.h"

namespace pyrowave_renderer {

int Renderer::submit(const uint8_t *data, size_t length, int64_t ptsUs, int64_t hostPresentationTimeUs,
           int64_t vsyncNs, int64_t intervalNs) {
    if (failed) return SUBMIT_ERROR;
    {
        SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::CLOCK);
        // The presenter alone updates the scheduler. Copy the display callback's
        // timestamps without calling into Vulkan or waiting for presentation here.
        std::lock_guard<std::mutex> lock(clockMutex);
        displayClockNs = vsyncNs;
        displayClockIntervalNs = intervalNs;
    }
    {
        SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::PARSE);
        // Records that came early (pushChunks) are in the decoder already; a frame whose
        // chunks were not the frame, or that had none, is pushed whole as before.
        bool pushed = false;
        countEarly(records.submitFrame(data, length, [this] { pyrowave_decoder_clear(decoder); },
                                       [this, data, length] { return pushFrame(data, length); },
                                       sdkPush(), pushed));
        lastRecordLossPercent.store(records.lossPercent, std::memory_order_relaxed);
        if (!pushed) return SUBMIT_SKIPPED;
        // Intra-only: a frame missing packets still decodes, with lost detail.
        if (!pyrowave_decoder_decode_is_ready(decoder, false)) {
            return SUBMIT_SKIPPED;
        }
    }
    // Parse every frame before dropping for capacity: record framing can carry
    // recovery information that the next frame still needs.
    int index;
    {
        SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::SLOT);
        std::lock_guard<std::mutex> lock(slotMutex);
        index = reserveDecodeSlot();
    }
    if (failed) return SUBMIT_ERROR;
    if (index < 0) return SUBMIT_BUSY;
    if (decodeFrame(index, ptsUs, hostPresentationTimeUs)) {
        if (warpNumbered) {
            // Frame Warp: the host's number for this picture (noteFrameNumber())
            warpSlots[index].frameNumber = warpNextFrame;
            warpNextFrame = -1;
        }
        {
            SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::PUBLISH);
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                pendingDecodes.push_back(index);
            }
            decodeReady.notify_one();
            decodeStats.frames++;
        }
        {
            SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::STATS);
            logDecodeStatsIfDue(nowUs());
        }
        return SUBMIT_OK;
    }
    failed = true;
    decodeReady.notify_all();
    slotReady.notify_all();
    return SUBMIT_ERROR;
}

// Packets of the next frame while it arrives (the Rust engine's early chunks, a batch of
// [length][flags][payload] at a time, on the thread that submits): their records go to
// the SDK now, so submit() has only the header and the last records to push.
void Renderer::pushChunks(const uint8_t *data, size_t length, bool first) {
    if (failed) return;
    records.pushChunks(data, length, first, [this] { pyrowave_decoder_clear(decoder); }, sdkPush());
}


void Renderer::countEarly(int result) {
    if (result == RP_PYROWAVE_EARLY_DONE) decodeStats.earlyDone++;
    else if (result == RP_PYROWAVE_EARLY_FALLBACK) decodeStats.earlyFallback++;
    else decodeStats.earlyWhole++;
}

bool Renderer::pushFrame(const uint8_t *data, size_t length) {
    auto push = sdkPush();
    if (length >= 4 && pyroReadLe32(data) == 0) {
        const bool ok = records.pushFrame(data, length, push);
        lastRecordLossPercent.store(records.lossPercent, std::memory_order_relaxed);
        return ok;
    }
    records.lossPercent = 0;
    lastRecordLossPercent.store(0.0f, std::memory_order_relaxed);
    return pushPyroWaveFrame(data, length, push);
}

// Where the decoder writes the planes: compute storage writes, or colour
// attachment writes for the fragment path.
VkPipelineStageFlags Renderer::decodeWriteStage() const {
    return fragmentPath ? VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT : VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT;
}

VkAccessFlags Renderer::decodeWriteAccess() const {
    return fragmentPath ? VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT : VK_ACCESS_SHADER_WRITE_BIT;
}

void Renderer::planeBarrier(VkCommandBuffer cmd, const Slot &slot, VkPipelineStageFlags srcStage, VkAccessFlags srcAccess,
                  VkPipelineStageFlags dstStage, VkAccessFlags dstAccess, VkImageLayout oldLayout) {
    VkImageMemoryBarrier barriers[3] = {};
    for (int i = 0; i < 3; ++i) {
        barriers[i].sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
        barriers[i].srcAccessMask = srcAccess;
        barriers[i].dstAccessMask = dstAccess;
        barriers[i].oldLayout = oldLayout;
        barriers[i].newLayout = VK_IMAGE_LAYOUT_GENERAL;
        barriers[i].srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        barriers[i].dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        barriers[i].image = slot.planes[i].image;
        barriers[i].subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    }
    vk.CmdPipelineBarrier(cmd, srcStage, dstStage, 0, 0, nullptr, 0, nullptr, 3, barriers);
}

// Called with slotMutex held once workers exist. Never wait for a plane
// or build a queue of stale input frames behind a busy GPU.
int Renderer::reserveDecodeSlot() {
    return schedule.reserve([this](int index) {
        const VkResult status = vk.GetFenceStatus(device, slots[index].drawFence);
        if (status == VK_NOT_READY) return false;
        if (status != VK_SUCCESS) {
            check(status, "draw fence status");
            failed = true;
            return false;
        }
        return true;
    });
}

bool Renderer::waitForDecode(int index) {
    if (legacySync) {
        return check(vk.WaitForFences(device, 1, &slots[index].decodeFence, VK_TRUE, FENCE_TIMEOUT_NS),
                     "decode completion");
    }
    VkSemaphoreWaitInfo info = {VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO};
    info.semaphoreCount = 1;
    info.pSemaphores = &decodeTimeline;
    info.pValues = &slots[index].decodeValue;
    return check(vk.WaitSemaphores(device, &info, FENCE_TIMEOUT_NS), "decode completion");
}

// legacySync: consume the binary semaphore PyroWave signaled after the decode, and
// signal the slot's fence. PyroWave flushes its batched commands only when it signals,
// so the decode is on the queue before this wait. The wait covers all commands and all
// later work on this one shared queue, ordering the next decode and the draw after it.
bool Renderer::signalDecodeFence(Slot &slot) {
    const VkPipelineStageFlags stage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
    VkSubmitInfo completion = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    completion.waitSemaphoreCount = 1;
    completion.pWaitSemaphores = &decodeSemaphore;
    completion.pWaitDstStageMask = &stage;
    std::lock_guard<std::mutex> lock(decodeQueueMutex);
    return check(vk.ResetFences(device, 1, &slot.decodeFence), "reset decode fence") &&
           check(vk.QueueSubmit(decodeQueue, 1, &completion, slot.decodeFence), "decode completion fence");
}

// Only the submission thread enters PyroWave. The SDK owns its command
// buffers and tracks their staging/query/frame-context lifetimes itself.
bool Renderer::decodeFrame(int index, int64_t ptsUs, int64_t hostPresentationTimeUs) {
    Slot &slot = slots[index];
    if (lastDecodeValue != 0 && !legacySync) {
        SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::ORDER);
        // PyroWave's fragment acquire wait only covers color output.
        // Order transfers and dequantization too: those reuse decoder
        // storage from the preceding frame. This is a GPU-side wait.
        VkTimelineSemaphoreSubmitInfo timelineInfo = {VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO};
        timelineInfo.waitSemaphoreValueCount = 1;
        timelineInfo.pWaitSemaphoreValues = &lastDecodeValue;
        const VkPipelineStageFlags stage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
        VkSubmitInfo ordering = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
        ordering.pNext = &timelineInfo;
        ordering.waitSemaphoreCount = 1;
        ordering.pWaitSemaphores = &decodeTimeline;
        ordering.pWaitDstStageMask = &stage;
        std::lock_guard<std::mutex> lock(decodeQueueMutex);
        if (!check(vk.QueueSubmit(decodeQueue, 1, &ordering, VK_NULL_HANDLE), "order consecutive decodes")) return false;
    }
    pyrowave_gpu_buffers buffers = {};
    for (int i = 0; i < 3; ++i) {
        auto &view = buffers.planes[i];
        view.image = slot.planes[i].image;
        view.width = slot.planes[i].width;
        view.height = slot.planes[i].height;
        view.image_format = planeFormat();
        view.view_format = planeFormat();
        view.aspect = VK_IMAGE_ASPECT_COLOR_BIT;
        view.swizzle = VK_COMPONENT_SWIZZLE_IDENTITY;
        view.layout = VK_IMAGE_LAYOUT_GENERAL;
    }
    const uint64_t value = lastDecodeValue + 1;
    pyrowave_gpu_sync_operation release = {};
    // A value of 0 makes PyroWave treat the semaphore as binary.
    release.sync = legacySync ? pyrowave_sync_point{decodeSemaphore, 0} :
                                pyrowave_sync_point{decodeTimeline, value};
    pyrowave_result result;
    {
        SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::SDK);
        result = pyrowave_decoder_decode_gpu_buffer(decoder, nullptr, &release, &buffers);
    }
    if (result != PYROWAVE_SUCCESS) {
        LOGE("pyrowave_decoder_decode_gpu_buffer failed: %d", result);
        return false;
    }
    if (legacySync && !signalDecodeFence(slot)) return false;
    lastDecodeValue = value;
    slot.decodeValue = value;
    slot.ptsUs = ptsUs;
    slot.hostPresentationTimeUs = hostPresentationTimeUs;
    return true;
}

bool Renderer::startWorkers() {
    completer = std::thread([this] { completionLoop(); });
    presenter = std::thread([this] { presentLoop(); });
    return true;
}

void Renderer::stopWorkers() {
    {
        std::lock_guard<std::mutex> lock(slotMutex);
        stopPresenter = true;
    }
    decodeReady.notify_all();
    slotReady.notify_all();
    if (completer.joinable()) completer.join();
    if (presenter.joinable()) presenter.join();
    // A completed frame can still be waiting for the presenter when stop
    // is requested. Retire it after both workers have relinquished ownership.
    std::lock_guard<std::mutex> lock(slotMutex);
    if (schedule.ready() >= 0) {
        const int index = schedule.beginPresent();
        pushRelease(slots[index].ptsUs, 0, false);
        schedule.finishPresent();
    }
}

void Renderer::completionLoop() {
    setpriority(PRIO_PROCESS, 0, -8);
    while (true) {
        int index;
        {
            std::unique_lock<std::mutex> lock(slotMutex);
            decodeReady.wait(lock, [this] { return stopPresenter || failed || !pendingDecodes.empty(); });
            if (failed || (stopPresenter && pendingDecodes.empty())) return;
            index = pendingDecodes.front();
            pendingDecodes.pop_front();
        }
        // The schedule still owns this slot while the waiter is blocked.
        if (!waitForDecode(index)) {
            failed = true;
            slotReady.notify_all();
            return;
        }
        timespec completed;
        clock_gettime(CLOCK_MONOTONIC, &completed);
        const int64_t completionNs = int64_t(completed.tv_sec) * 1000000000LL + completed.tv_nsec;
        {
            std::lock_guard<std::mutex> lock(slotMutex);
            slots[index].completedNs = completionNs;
            // Register completion before publishing the frame to the presenter;
            // otherwise a fast present/drop could reach Java without an output.
            {
                std::lock_guard<std::mutex> eventsLock(eventsMutex);
                completedFrames.push_back(slots[index].ptsUs);
                completedFrames.push_back(completionNs);
            }
            const int replaced = schedule.complete(index, !stopPresenter);
            if (replaced >= 0) {
                pushRelease(slots[replaced].ptsUs, 0, false);
                replacedSinceReport++;
            }
            if (stopPresenter) pushRelease(slots[index].ptsUs, 0, false);
        }
        slotReady.notify_one();
    }
}

void Renderer::presentLoop() {
    // Like a game's render thread: frames must reach the compositor on time.
    setpriority(PRIO_PROCESS, 0, -8);
    while (true) {
        int index;
        int64_t redrawVsyncNs = 0;
        {
            std::unique_lock<std::mutex> lock(slotMutex);
            const auto woken = [this] { return stopPresenter || failed || schedule.ready() >= 0; };
            if (schedule.held() >= 0 && warpRedraws() && warpWantsVsync()) {
                // Frame Warp: until a new picture, the one on screen may be drawn again at
                // the next vsync (redrawSlot())
                redrawVsyncNs = waitForPictureOrVsync(lock, woken);
            } else {
                // Display times of shown frames arrive a little after their present; keep
                // asking for them while the stream is still (as during the latency test),
                // once per refresh: a time exists only after the display showed the frame.
                while (!pendingPresents.empty() && !woken()) {
                    const int64_t waitNs = presentPoll.waitNs(nowNs(), driverRefreshPeriodNs);
                    if (waitNs > 0 &&
                        slotReady.wait_for(lock, std::chrono::nanoseconds(waitNs), woken)) break;
                    lock.unlock();
                    pollRenderedFrames();
                    lock.lock();
                }
                slotReady.wait(lock, woken);
            }
            if (stopPresenter || failed) return;
            index = redrawVsyncNs != 0 ? schedule.beginRedraw() : schedule.beginPresent();
            if (index < 0) continue;
        }
        const bool ok = redrawVsyncNs != 0 ? redrawSlot(index, redrawVsyncNs) : presentSlot(index);
        {
            std::lock_guard<std::mutex> lock(slotMutex);
            schedule.finishPresent();
        }
        if (!ok) {
            failed = true;
            decodeReady.notify_all();
            return;
        }
    }
}

// Runs on the present thread, which owns the swapchain.
bool Renderer::presentSlot(int index) {
    if (!applyPendingHdr()) return false;
    pollRenderedFrames();
    refreshDisplayClock();
    uint32_t imageIndex = 0;
    const uint64_t beforeAcquire = nowUs();
    // The image taken ahead after the last present, else a fresh one (image_hold.h).
    const PyroAcquire acquired = imageHold.take(&imageIndex,
            [this](uint32_t *image) { return acquireImage(image, ACQUIRE_TIMEOUT_NS); },
            [this] { return settleAcquireFence(); });
    if (acquired == PyroAcquire::OUT_OF_DATE) {
        dropSlot(index);
        return recreateSwapchain();
    }
    if (acquired == PyroAcquire::NOT_READY) {
        dropSlot(index);  // The next frame replaces it anyway.
        return true;
    }
    if (acquired != PyroAcquire::READY) {
        dropSlot(index);
        return false;
    }
    const uint64_t afterAcquire = nowUs();

    // Immediate mode keeps the freshest output. Paced modes, the PC's frame times among
    // them, retain the selected frame while the next completed frame waits, preserving
    // host cadence.
    if (framePacingMode == 0 || slots[index].hostPresentationTimeUs <= 0) {
        std::lock_guard<std::mutex> lock(slotMutex);
        const int replaced = schedule.replaceDrawingWithReady();
        if (replaced >= 0) {
            pushRelease(slots[replaced].ptsUs, 0, false);
            presentStats.replaced++;
            index = schedule.drawing();
        }
    }
    Slot &slot = slots[index];
    const auto timing = presentTiming.plan(slot.hostPresentationTimeUs, slot.completedNs);
    if (framePacingMode == HOST_TIMES_MODE && timing.paced) {
        presentStats.heldNs += uint64_t(timing.targetNs - slot.completedNs);
        presentStats.heldFrames++;
    }

    // Frame Warp once enableFrameWarp() published it: its pipelines, and the shift below
    const FrameWarpApi *warp = warpApi.load(std::memory_order_acquire);
    int32_t warpMask = FrameWarpCells::kEmpty;
    if (!recordDraw(index, imageIndex, warp, &warpMask)) return false;
    VkCommandBuffer cmd = slot.drawCommandBuffer;

    // Acquire and record before waiting so that work does not consume the learned
    // compositor lead. The wait releases slotMutex; decoding can keep progressing.
    {
        std::unique_lock<std::mutex> lock(slotMutex);
        const auto stopping = [this] { return stopPresenter || failed; };
        const int64_t remainingNs = timing.submitNs - nowNs();
        pacingDiagnostics.record(timing, slot.completedNs, remainingNs,
                                 presentTiming.periodNs(), presentTiming.leadNs());
        if (timing.paced && remainingNs > 0) {
            slotReady.wait_for(lock, std::chrono::nanoseconds(remainingNs), stopping);
        }
        if (stopping()) {
            pushRelease(slot.ptsUs, 0, false);
            return true;
        }
    }
    // Frame Warp: the shift as of now, after the wait, for the draw just recorded
    float warpOut[FRAME_WARP_FLOATS];
    frameWarpStill(warpOut);
    int32_t warpCode = FRAME_WARP_INACTIVE;
    uint32_t warpSeq = 0;
    if (warp != nullptr) warpCode = setWarpShift(index, warp, warpMask, warpOut, &warpSeq);

    VkTimelineSemaphoreSubmitInfo timelineInfo = {VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO};
    timelineInfo.waitSemaphoreValueCount = 1;
    timelineInfo.pWaitSemaphoreValues = &slot.decodeValue;
    const uint64_t binarySignalValue = 0;
    timelineInfo.signalSemaphoreValueCount = 1;
    timelineInfo.pSignalSemaphoreValues = &binarySignalValue;
    const VkPipelineStageFlags waitStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
    VkSubmitInfo submitInfo = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    if (!legacySync) {
        submitInfo.pNext = &timelineInfo;
        submitInfo.waitSemaphoreCount = 1;
        submitInfo.pWaitSemaphores = &decodeTimeline;
        submitInfo.pWaitDstStageMask = &waitStage;
    }
    submitInfo.commandBufferCount = 1;
    submitInfo.pCommandBuffers = &cmd;
    submitInfo.signalSemaphoreCount = 1;
    submitInfo.pSignalSemaphores = &renderDone[imageIndex];
    if (!check(vk.ResetFences(device, 1, &slot.drawFence), "reset draw fence")) return false;
    VkPresentInfoKHR presentInfo = {VK_STRUCTURE_TYPE_PRESENT_INFO_KHR};
    presentInfo.waitSemaphoreCount = 1;
    presentInfo.pWaitSemaphores = &renderDone[imageIndex];
    presentInfo.swapchainCount = 1;
    presentInfo.pSwapchains = &swapchain;
    presentInfo.pImageIndices = &imageIndex;
    if (++nextPresentId == 0) ++nextPresentId;
    VkPresentTimeGOOGLE presentTime = {nextPresentId, uint64_t(timing.desiredPresentNs)};
    VkPresentTimesInfoGOOGLE presentTimes = {VK_STRUCTURE_TYPE_PRESENT_TIMES_INFO_GOOGLE};
    if (displayTimingSupported) {
        presentTimes.swapchainCount = 1;
        presentTimes.pTimes = &presentTime;
        presentInfo.pNext = &presentTimes;
    }
    timespec released;
    VkResult presented = VK_SUCCESS;
    int64_t submittedNs;
    {
        std::lock_guard<std::mutex> lock(queueCount == 2 ? renderQueueMutex : decodeQueueMutex);
        submittedNs = nowNs();
        if (!check(vk.QueueSubmit(queue, 1, &submitInfo, slot.drawFence), "vkQueueSubmit")) return false;
        clock_gettime(CLOCK_MONOTONIC, &released);
        presented = vk.QueuePresentKHR(queue, &presentInfo);
    }
    const int64_t ptsUs = slot.ptsUs;
    const bool shown = presented == VK_SUCCESS || presented == VK_SUBOPTIMAL_KHR;
    if (warp != nullptr) {
        warpSlots[index].generation = warpSlots[index].writing;
        if (shown) warpShown(index, warpOut, warpCode, warpSeq, timing.paced ? timing.targetNs : 0, true);
    }
    const int64_t releasedNs = int64_t(released.tv_sec) * 1000000000LL + released.tv_nsec;
    pushRelease(ptsUs, shown ? releasedNs : 0, shown);
    if (shown) checkHlgWindow();
    uint64_t frameEnd = nowUs();
    if (displayTimingSupported && shown) {
        if (pendingPresents.size() == 8192) pendingPresents.pop_front();
        pendingPresents.push_back({presentTime.presentID, ptsUs, frameEnd,
                                  submissionDiagnostics.isEnabled() ? slot.completedNs : 0,
                                  submissionDiagnostics.isEnabled() ? releasedNs : 0});
        presentTiming.recordPresent(presentTime.presentID, timing, submittedNs);
    }
    // No poll here: this frame's own time cannot exist yet, and the next present (or the
    // idle wait) asks once for every time that has come by then.

    presentStats.frames++;
    presentStats.acquireWaitUs += afterAcquire - beforeAcquire;
    logPresentStatsIfDue(frameEnd);
    pacingDiagnostics.reportIfDue(frameEnd);

    if (presented == VK_ERROR_OUT_OF_DATE_KHR) {
        return recreateSwapchain();
    }
    // Poll at a bounded rate even without SUBOPTIMAL to catch 180-degree turns.
    // Rebuild only for a changed extent/transform, never for every suboptimal frame.
    if (surfaceSizeChanged(frameEnd)) {
        return recreateSwapchain();
    }
    if (shown && preacquireImages) {
        // Only an image free now: the next frame's own acquire waits if none is.
        imageHold.preacquire([this](uint32_t *image) { return acquireImage(image, 0); });
    }
    return shown || check(presented, "vkQueuePresentKHR");
}

// Records the draw of slot `index` to swapchain image `imageIndex`, for presentSlot() and
// redrawSlot(); the slot's draw fence has signalled. With Frame Warp (`warp`), its pipelines,
// which also read the slot's HUD mask and shift (set 1); `warpMask` gets the mask generation
// the draw reads.
bool Renderer::recordDraw(int index, uint32_t imageIndex, const FrameWarpApi *warp, int32_t *warpMask) {
    Slot &slot = slots[index];
    VkCommandBuffer cmd = slot.drawCommandBuffer;
    VkCommandBufferBeginInfo beginInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    if (!check(vk.ResetCommandBuffer(cmd, 0), "reset render command buffer") ||
        !check(vk.BeginCommandBuffer(cmd, &beginInfo), "begin render command buffer")) return false;
    // The submit's timeline wait makes decoder writes visible across queues.
    // Both queues are in the same family, so no ownership transfer is needed.
    // With legacySync there is one queue, and the decode completion wait precedes this draw on it.
    planeBarrier(cmd, slot, decodeWriteStage(), decodeWriteAccess(),
                 VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT, VK_ACCESS_SHADER_READ_BIT, VK_IMAGE_LAYOUT_GENERAL);
    if (warp != nullptr) {
        // Before the render pass, which a copy cannot be in
        warpCells.poll(warp);
        *warpMask = recordWarpMask(index, cmd);
    }

    VkClearValue clear = {};
    clear.color = {{0.0f, 0.0f, 0.0f, 1.0f}};
    VkRenderPassBeginInfo rpBegin = {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
    rpBegin.renderPass = renderPass;
    rpBegin.framebuffer = framebuffers[imageIndex];
    rpBegin.renderArea = {{0, 0}, swapchainExtent};
    rpBegin.clearValueCount = 1;
    rpBegin.pClearValues = &clear;
    const auto output = pyroViewport(width, height, displayExtent.width, displayExtent.height, stretch);
    const auto rotatedOutput = pyroRotateViewport(output,
            {swapchainExtent.width, swapchainExtent.height}, rotationQuarterTurns);
    const PyroWaveColorParams color = {tenBit, hdr, fullRange, rotationQuarterTurns};
    const bool sgsrReady = (warp == nullptr ? upscalePipeline : warpUpscalePipeline) != VK_NULL_HANDLE;
    const auto reason = pyroUpscalingReason(upscaleMode, tenBit || hdr, width, height, output, sgsrReady);
    upscalingReason = reason;
    const bool upscale = reason == UPSCALING_ACTIVE && upscaleMode == UPSCALE_SGSR && sgsrReady;
    vk.CmdBeginRenderPass(cmd, &rpBegin, VK_SUBPASS_CONTENTS_INLINE);

    VkViewport viewport = {};
    viewport.width = rotatedOutput.width;
    viewport.height = rotatedOutput.height;
    viewport.x = rotatedOutput.x;
    viewport.y = rotatedOutput.y;
    viewport.maxDepth = 1.0f;
    VkRect2D scissor = {{0, 0}, swapchainExtent};
    vk.CmdSetViewport(cmd, 0, 1, &viewport);
    vk.CmdSetScissor(cmd, 0, 1, &scissor);
    if (upscale) {
        const PyroWaveSgsrParams params = {{1.0f / float(width), 1.0f / float(height), float(width), float(height)},
                                          edgeSharpness, fullRange, rotationQuarterTurns};
        const VkPipelineLayout layout = warp == nullptr ? upscaleLayout : warpUpscaleLayout;
        vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, warp == nullptr ? upscalePipeline : warpUpscalePipeline);
        vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, layout, 0, 1,
                                 &slot.descriptorSet, 0, nullptr);
        vk.CmdPushConstants(cmd, layout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(params), &params);
    } else {
        const VkPipelineLayout layout = warp == nullptr ? pipelineLayout : warpLayout;
        vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, warp == nullptr ? pipeline : warpPipeline);
        vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, layout, 0, 1,
                                 &slot.descriptorSet, 0, nullptr);
        vk.CmdPushConstants(cmd, layout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(color), &color);
    }
    if (warp != nullptr) {
        vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, upscale ? warpUpscaleLayout : warpLayout,
                                 1, 1, &warpSlots[index].set, 0, nullptr);
    }
    vk.CmdDraw(cmd, 3, 1, 0, 0);
    vk.CmdEndRenderPass(cmd);
    return check(vk.EndCommandBuffer(cmd), "vkEndCommandBuffer");
}

// Frame Warp needs a redraw between the host's pictures to be replaceable by the next new
// picture: without a time to present it at, a FIFO swapchain would show it ahead of a new
// picture presented for the same vsync, a vsync late (as VulkanRenderer::warpRefresh()).

}  // namespace pyrowave_renderer
