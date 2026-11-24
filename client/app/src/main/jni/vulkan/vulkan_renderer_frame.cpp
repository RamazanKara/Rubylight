// VulkanRenderer frame draw/present submission, VK_GOOGLE_display_timing collection,
// and capture/present latency reporting.
// Adapted from MoreOrLessSoftware/moonlight-android (see SOURCE).

#include "vulkan_renderer_internal.h"

namespace vkr {

using namespace renderer_detail;

// ---------------------------------------------------------------------------------------------
// Rendering

bool VulkanRenderer::renderFrame(const FramePtr& frame, uint64_t presentId, int64_t showVsyncNs, bool ahead,
                                 int64_t commitNs, bool onArrival) {
    if (fatal_ || quit_) {
        return false;
    }
    const int64_t startNs = nowNs();
    if (!ensureSwapchain()) {
        if (!outputWindowEmpty_) {
            noteFailure("creating the swapchain", VK_ERROR_INITIALIZATION_FAILED, kFatalFailures * 3);
        }
        return false;
    }

    // MediaCodec's buffer, imported into Vulkan
    ImportedBuffer* imported = importBuffer(frame->buffer);
    if (!imported || !ensurePipeline()) {
        noteFailure(imported ? "creating the pipeline" : "importing a decoded frame", VK_ERROR_INITIALIZATION_FAILED,
                    kFatalFailures);
        return false;
    }

    const int slot = static_cast<int>(frameCounter_ % kFramesInFlight);
    // The fence is nearly always done by now, so ask before waiting on it
    VkResult result = vk_.vkGetFenceStatus ? vk_.vkGetFenceStatus(device_, fences_[slot]) : VK_NOT_READY;
    const bool fenceWaited = result != VK_SUCCESS;
    const int64_t fenceWaitNs = fenceWaited ? nowNs() : 0;
    if (fenceWaited) {
        result = waitFence(fences_[slot], kFenceTimeoutNs);
    }
    if (result != VK_SUCCESS) {
        if (!quit_) {
            ALOGE("Timed out waiting for the GPU: %d", result);
            noteFailure("waiting for the GPU", result, kFatalFenceTimeouts);
        }
        return false;
    }
    const int64_t fencedNs = nowNs();
    // Waits that cost a refresh, counted for diagnosticsText() (0 before the period is known)
    const int64_t slowWaitNs = offsetPeriodNs_.load(std::memory_order_relaxed) / 2;
    if (fenceWaited && slowWaitNs > 0 && fencedNs - fenceWaitNs > slowWaitNs) {
        slowFenceWaits_.fetch_add(1, std::memory_order_relaxed);
    }
    // Gives MediaCodec's image back to the reader (see onImageAvailable())
    slotFrames_[slot].reset();
    const int64_t releasedNs = nowNs();

    // The fence covers the last present rendered in this slot, so its timestamps are written
    if (gpuTiming_ && slotPresentIds_[slot] != 0) {
        uint64_t ticks[kTimestampsPerFrame] = {};
        if (vk_.vkGetQueryPoolResults(device_, timestampPool_, kTimestampsPerFrame * slot, kTimestampsPerFrame,
                                      sizeof(ticks), ticks, sizeof(ticks[0]), VK_QUERY_RESULT_64_BIT) == VK_SUCCESS) {
            const uint64_t mask = timestampValidBits_ >= 64 ? ~0ULL : (1ULL << timestampValidBits_) - 1;
            const auto toNs = [&](uint64_t t) {
                return static_cast<int64_t>(static_cast<double>(t & mask) * timestampPeriodNs_);
            };
            std::lock_guard<std::mutex> lock(mutex_);
            trace_.gpuTimes(slotPresentIds_[slot], toNs(ticks[0]), toNs(ticks[3]), toNs(ticks[1]), toNs(ticks[2]));
        }
    }
    slotPresentIds_[slot] = 0;

    uint32_t imageIndex = 0;
    // In slices, so stopping doesn't wait out the whole timeout
    const auto acquireImage = [&]() {
        VkResult acquired;
        const int64_t deadlineNs = nowNs() + static_cast<int64_t>(kAcquireTimeoutNs);
        do {
            acquired = vk_.vkAcquireNextImageKHR(device_, swapchain_, kStopWaitNs, imageAcquired_[slot],
                                                 VK_NULL_HANDLE, &imageIndex);
        } while (acquired == VK_TIMEOUT && !quit_ && nowNs() < deadlineNs);
        return acquired;
    };
    const int64_t acquireStartNs = nowNs();
    result = acquireImage();
    const int64_t acquireWaitNs = nowNs() - acquireStartNs;
    if (result == VK_ERROR_OUT_OF_DATE_KHR) {
        swapchainDirty_ = true;
        if (!ensureSwapchain() || !ensurePipeline()) {
            if (!outputWindowEmpty_) {
                noteFailure("recreating the swapchain", VK_ERROR_INITIALIZATION_FAILED, kFatalFailures * 3);
            }
            return false;
        }
        result = acquireImage();
    }
    if (result != VK_SUCCESS && result != VK_SUBOPTIMAL_KHR) {
        // A timeout is the compositor not taking frames (the screen is off, say), not a failure
        if (result != VK_TIMEOUT && result != VK_NOT_READY) {
            ALOGE("vkAcquireNextImageKHR failed: %d", result);
            swapchainDirty_ = true;
            noteFailure("acquiring a swapchain image", result, kFatalFailures);
        }
        return false;
    }
    if (slowWaitNs > 0 && acquireWaitNs > slowWaitNs) {
        slowAcquires_.fetch_add(1, std::memory_order_relaxed);
    }

    const int64_t acquiredNs = nowNs();

    // Where the picture is within the buffer, which the decoder may pad
    PushConstants pc {};
    const uint32_t width = imported->width;
    const uint32_t height = imported->height;
    const float bufferWidth = static_cast<float>(width);
    const float bufferHeight = static_cast<float>(height);
    AImageCropRect crop = frame->crop;
    if (crop.right <= crop.left || crop.bottom <= crop.top) {
        crop = {0, 0, static_cast<int32_t>(width), static_cast<int32_t>(height)};
    }
    pc.uvRect[0] = crop.left / bufferWidth;
    pc.uvRect[1] = crop.top / bufferHeight;
    pc.uvRect[2] = (crop.right - crop.left) / bufferWidth;
    pc.uvRect[3] = (crop.bottom - crop.top) / bufferHeight;
    // Frame Warp samples only the stream's picture. A decoder can leave an encoder's padding in
    // its crop (AMD's AV1 at 2184x1968 as 2240x1968, black at the right), which the stream view
    // pushes off screen (StreamView.setDecodedPadding()); a shift would pull it back in.
    FrameWarpRect picture {crop.left, crop.top, crop.right, crop.bottom};
    if (warpApi_) {
        picture = frameWarpPicture(picture, config_.streamWidth, config_.streamHeight);
    }
    // Stay half a texel inside the picture so filtering never pulls in padding
    pc.uvClamp[0] = (picture.left + 0.5f) / bufferWidth;
    pc.uvClamp[1] = (picture.top + 0.5f) / bufferHeight;
    pc.uvClamp[2] = (picture.right - 0.5f) / bufferWidth;
    pc.uvClamp[3] = (picture.bottom - 0.5f) / bufferHeight;
    pc.warpRect[0] = picture.left / bufferWidth;
    pc.warpRect[1] = picture.top / bufferHeight;
    pc.warpRect[2] = (picture.right - picture.left) / bufferWidth;
    pc.warpRect[3] = (picture.bottom - picture.top) / bufferHeight;
    if (warpApi_ && (width != warpLoggedWidth_ || height != warpLoggedHeight_ || crop.left != warpLoggedCrop_.left ||
            crop.top != warpLoggedCrop_.top || crop.right != warpLoggedCrop_.right ||
            crop.bottom != warpLoggedCrop_.bottom)) {
        warpLoggedWidth_ = width;
        warpLoggedHeight_ = height;
        warpLoggedCrop_ = crop;
        ALOGI("Frame Warp: buffer %ux%u, crop %d,%d-%d,%d, stream %dx%d, picture %d,%d-%d,%d, clamp %.5f,%.5f-%.5f,%.5f",
              width, height, crop.left, crop.top, crop.right, crop.bottom, config_.streamWidth, config_.streamHeight,
              picture.left, picture.top, picture.right, picture.bottom, pc.uvClamp[0], pc.uvClamp[1], pc.uvClamp[2],
              pc.uvClamp[3]);
    }

    // Low dithers at the output's own precision. High dithers at 8-bit precision even on a
    // 10-bit output, which also hides banding from an 8-bit panel or compositor further on.
    float ditherAmplitude = 0.0f;
    if (config_.ditherMode == 1) {
        ditherAmplitude = 1.0f / static_cast<float>((1 << outputBits_) - 1);
    }
    else if (config_.ditherMode == 2) {
        ditherAmplitude = 1.0f / 255.0f;
    }
    const float contentPeakNits = contentPeakNits_.load(std::memory_order_relaxed);
    pc.params[0] = ditherAmplitude;
    pc.params[1] = static_cast<float>(frameCounter_ % 64);
    const bool hlg = config_.hdrTransfer == kHdrTransferHlg;
    pc.params[2] = !hdrActive_ || outputHdr_ ? kOutputPassthrough : hlg ? kOutputHlgToSdr : kOutputPqToSdr;
    // HLG carries no mastering data: it is mapped for the BT.2100 reference display
    pc.params[3] = hlg ? kHlgNominalPeakNits : contentPeakNits;
    pc.params2[0] = kSdrWhiteNits;
    pc.params2[1] = static_cast<float>(std::max(quarterTurns(preTransform_), 0));

    // Frame Warp: the picture's shift as of now, as fractions of the picture, and the lens terms
    // the rest of it is turned through. The input count is read first, so input sent while the
    // shift is worked out shows as new at the next refresh.
    int32_t warpCode = FRAME_WARP_INACTIVE;
    float warpOut[FRAME_WARP_FLOATS];
    frameWarpStill(warpOut);
    uint32_t warpSeq = 0;
    if (warpApi_) {
        warpCells_.poll(warpApi_);
        warpSeq = warpApi_->input_seq();
        warpCode = warpApi_->warp(frame->frameNumber, warpOut);
        if (warpCode == FRAME_WARP_INACTIVE) {
            frameWarpStill(warpOut);
        }
        pc.warp[0] = warpOut[0];
        pc.warp[1] = warpOut[1];
    }
    memcpy(pc.warpLens, warpOut + 4, sizeof(pc.warpLens));

    VkCommandBuffer cmd = commandBuffers_[slot];
    vk_.vkResetCommandBuffer(cmd, 0);
    VkCommandBufferBeginInfo beginInfo {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    vk_.vkBeginCommandBuffer(cmd, &beginInfo);

    const bool timed = gpuTiming_ && presentId != 0;
    if (timed) {
        vk_.vkCmdResetQueryPool(cmd, timestampPool_, kTimestampsPerFrame * slot, kTimestampsPerFrame);
        vk_.vkCmdWriteTimestamp(cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, timestampPool_, kTimestampsPerFrame * slot);
    }

    // Take the decoder's buffer from the foreign (non-Vulkan) queue family
    VkImageMemoryBarrier acquire {VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
    acquire.srcAccessMask = 0;
    acquire.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
    acquire.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    acquire.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    acquire.srcQueueFamilyIndex = VK_QUEUE_FAMILY_FOREIGN_EXT;
    acquire.dstQueueFamilyIndex = queueFamily_;
    acquire.image = imported->image;
    acquire.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    vk_.vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                             0, 0, nullptr, 0, nullptr, 1, &acquire);
    if (timed) {
        vk_.vkCmdWriteTimestamp(cmd, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, timestampPool_, kTimestampsPerFrame * slot + 1);
    }

    if (warpApi_) {
        // The HUD stays put only with the mask the host drew this picture's HUD for
        const int32_t held = recordWarpMask(slot, cmd);
        pc.warp[2] = warpCode >= 0 && warpCode == held ? 1.0f : 0.0f;
    }

    VkRenderPassBeginInfo rpBegin {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
    rpBegin.renderPass = renderPass_;
    rpBegin.framebuffer = framebuffers_[imageIndex];
    rpBegin.renderArea = {{0, 0}, extent_};
    vk_.vkCmdBeginRenderPass(cmd, &rpBegin, VK_SUBPASS_CONTENTS_INLINE);

    VkViewport viewport {0.0f, 0.0f, static_cast<float>(extent_.width), static_cast<float>(extent_.height), 0.0f, 1.0f};
    VkRect2D scissor {{0, 0}, extent_};
    vk_.vkCmdSetViewport(cmd, 0, 1, &viewport);
    vk_.vkCmdSetScissor(cmd, 0, 1, &scissor);
    vk_.vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline_);
    vk_.vkCmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipelineLayout_, 0, 1,
                                &imported->descriptorSet, 0, nullptr);
    if (warpApi_) {
        vk_.vkCmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipelineLayout_, 1, 1,
                                    &warpMasks_[slot].set, 0, nullptr);
    }
    vk_.vkCmdPushConstants(cmd, pipelineLayout_, VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT,
                           0, sizeof(pc), &pc);
    vk_.vkCmdDraw(cmd, 3, 1, 0, 0);
    vk_.vkCmdEndRenderPass(cmd);
    if (timed) {
        vk_.vkCmdWriteTimestamp(cmd, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, timestampPool_, kTimestampsPerFrame * slot + 2);
    }

    // Hand the buffer back to the decoder
    VkImageMemoryBarrier release = acquire;
    release.srcAccessMask = VK_ACCESS_SHADER_READ_BIT;
    release.dstAccessMask = 0;
    release.oldLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    release.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    release.srcQueueFamilyIndex = queueFamily_;
    release.dstQueueFamilyIndex = VK_QUEUE_FAMILY_FOREIGN_EXT;
    vk_.vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
                             0, 0, nullptr, 0, nullptr, 1, &release);

    if (timed) {
        vk_.vkCmdWriteTimestamp(cmd, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, timestampPool_, kTimestampsPerFrame * slot + 3);
    }
    result = vk_.vkEndCommandBuffer(cmd);
    if (result != VK_SUCCESS) {
        // The image is acquired and its semaphore signalled; start the swapchain over
        ALOGE("vkEndCommandBuffer failed: %d", result);
        swapchainDirty_ = true;
        recreateSlotSync(slot);
        noteFailure("recording a frame", result, kFatalFailures);
        return false;
    }

    // The swapchain image. Timed, everything waits for the image, so the start timestamp shows
    // when it was free; only a draw's vertex work could otherwise go ahead of it, which for one
    // triangle is nothing.
    const VkPipelineStageFlags waitStage =
            timed ? VK_PIPELINE_STAGE_ALL_COMMANDS_BIT : VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;

    // The decoder's writes, before the barrier that takes the image from it
    bool decoderWait = false;
    if (frame->decodeFence.pending() && !waitForDecoder(*frame, slot, &decoderWait)) {
        // The image is acquired and its semaphore signalled; start the swapchain over
        swapchainDirty_ = true;
        recreateSlotSync(slot);
        noteFailure("waiting for the decoder", VK_TIMEOUT, kFatalFenceTimeouts);
        return false;
    }
    const VkSemaphore waitSemaphores[2] = {imageAcquired_[slot], decodeDone_[slot]};
    const VkPipelineStageFlags waitStages[2] = {waitStage, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT};

    VkSubmitInfo submit {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    submit.waitSemaphoreCount = decoderWait ? 2 : 1;
    submit.pWaitSemaphores = waitSemaphores;
    submit.pWaitDstStageMask = waitStages;
    submit.commandBufferCount = 1;
    submit.pCommandBuffers = &cmd;
    submit.signalSemaphoreCount = 1;
    submit.pSignalSemaphores = &renderDone_[imageIndex];

    vk_.vkResetFences(device_, 1, &fences_[slot]);
    {
        std::lock_guard<std::mutex> lock(queueMutex_);
        result = vk_.vkQueueSubmit(queue_, 1, &submit, fences_[slot]);
    }
    if (result != VK_SUCCESS) {
        ALOGE("vkQueueSubmit failed: %d", result);
        // The fence was reset and nothing will signal it, so the next render in this slot would
        // wait on it forever; the acquired image is given back by rebuilding the swapchain
        swapchainDirty_ = true;
        if (result != VK_ERROR_DEVICE_LOST) {
            recreateSlotSync(slot);
        }
        noteFailure("submitting a frame to the GPU", result, kFatalFailures);
        return false;
    }
    if (warpApi_) {
        warpMasks_[slot].generation = warpMasks_[slot].writing;
    }
    if (frontBufferAligned_ && frontBuffer_) {
        drawSubmitNs_.push_back(nowNs() - startNs);
        if (drawSubmitNs_.size() > kAlignedLeadSamples) {
            drawSubmitNs_.pop_front();
        }
    }
    if (timed) {
        slotPresentIds_[slot] = presentId;
    }

    VkPresentInfoKHR present {VK_STRUCTURE_TYPE_PRESENT_INFO_KHR};
    present.waitSemaphoreCount = 1;
    present.pWaitSemaphores = &renderDone_[imageIndex];
    present.swapchainCount = 1;
    present.pSwapchains = &swapchain_;
    present.pImageIndices = &imageIndex;

    // Ask for the vsync the image should reach the screen at (see PresentScheduler), and tag the
    // present so we find out when it did. A frame shown on arrival is left out of
    // PresentScheduler. With the mailbox swapchain it asks for no time (0, as soon as possible);
    // otherwise (a keep-alive present), for now, which is as soon as possible too, but lets the
    // compositor drop it for a newer frame: it only drops a waiting frame that has a time of its
    // own. Without one, frames queued behind each other: on an iPlay 70 mini Ultra at 144 fps
    // shown on arrival, 44% reached the screen a vsync later than they could have.
    uint64_t taggedPresentId = 0;
    bool countedAtTag = false;
    VkPresentTimeGOOGLE presentTime {static_cast<uint32_t>(presentId), 0};
    VkPresentTimesInfoGOOGLE presentTimes {VK_STRUCTURE_TYPE_PRESENT_TIMES_INFO_GOOGLE};
    // A front buffer is drawn at its time and never presented as a new image, so it has no
    // time to ask for, and its capture to present is counted up to the present
    if (presentId != 0 && hasDisplayTimingExt_ && !frontBuffer_ && (onArrival || currentPeriodNs_ > 0)) {
        const int64_t rawVsyncNs = showVsyncNs != 0 ? showVsyncNs : currentVsyncNs_;
        const auto plan = presentScheduler_.planPresent(rawVsyncNs, currentPeriodNs_,
                                                        vsyncOffsetNs_.load(std::memory_order_relaxed));
        if (!onArrival) presentTime.desiredPresentTime = static_cast<uint64_t>(plan.desiredNs);
        else if (!presentOnArrival_) presentTime.desiredPresentTime = static_cast<uint64_t>(nowNs());
        presentTimes.swapchainCount = 1;
        presentTimes.pTimes = &presentTime;
        present.pNext = &presentTimes;

        // Slack from commit (render wait can take ms on Pixel 10 Pro and skew the learned guard).
        PendingPresent entry {plan.hwVsyncNs, presentScheduler_.delayVsyncs(), ahead,
                              ahead ? rawVsyncNs - (commitNs != 0 ? commitNs : nowNs()) : 0, rawVsyncNs};
        entry.onArrival = onArrival;
        taggedPresentId = presentId;
        if (!frame->presentCounted) {
            countedAtTag = true;
            // Counted when the present is reported, at the time it reached the screen
            entry.originNs = frame->originNs;
            entry.originSynced = frame->originSynced;
            entry.hostPtsNs = frame->timing.hostPtsNs;
            frame->presentCounted = true;
        }
        pendingPresents_.put(presentId, entry);
        if (pendingPresents_.size() > 256) {
            // Reports lost across a swapchain rebuild never arrive
            while (!pendingPresents_.empty() && pendingPresents_.front().key + 128 < presentId) {
                reportPresent(pendingPresents_.front().value.hostPtsNs, 0, 0);
                pendingPresents_.popFront();
            }
        }
    }
    passHdr10Plus(*frame);
    int64_t presentCallNs = 0;
    {
        std::lock_guard<std::mutex> lock(queueMutex_);
        presentCallNs = nowNs();
        result = vk_.vkQueuePresentKHR(queue_, &present);
    }
    const int64_t afterPresentNs = nowNs();
    // The render's CPU time for the performance hint session, without waiting for the GPU or a
    // swapchain image
    perfHint_.reportWork(afterPresentNs - startNs - (acquiredNs - releasedNs) -
                         (fenceWaited ? fencedNs - fenceWaitNs : 0));

    const bool shown = result == VK_SUCCESS || result == VK_SUBOPTIMAL_KHR;
    if (warpApi_ && shown) {
        warpDrawn_.note(warpCode, warpOut, warpSeq);
        // A present for a later vsync is asked for half a vsync ahead of it (PresentScheduler)
        warpDueNs_ = !onArrival && presentTime.desiredPresentTime != 0 ?
                static_cast<int64_t>(presentTime.desiredPresentTime) + currentPeriodNs_ / 2 : 0;
        if (presentAhead_ && warpDrawn_.active != warpHoldsAhead_) {
            // The model locked or unlocked: presenting ahead stops or starts (presentsAhead())
            warpHoldsAhead_ = warpDrawn_.active;
            std::lock_guard<std::mutex> lock(mutex_);
            pacer_.setPresentAhead(presentsAhead());
        }
    }
    if (!shown && countedAtTag) {
        // Nothing was queued, so no report will come for the tag: the frame is counted when a
        // later present of it is
        if (PendingPresent* failed = pendingPresents_.find(taggedPresentId)) {
            failed->originNs = 0;
            failed->hostPtsNs = 0;
        }
        frame->presentCounted = false;
    }
    if (shown && countedAtTag) {
        if (PendingPresent* p = pendingPresents_.find(taggedPresentId)) {
            p->queuedNs = presentCallNs;
        }
    }
    if (shown && !frame->presentQueued) {
        frame->presentQueued = true;
        if (!frame->presentCounted) {
            frame->presentCounted = true;
            if (frame->originNs != 0) {
                std::lock_guard<std::mutex> lock(latencyMutex_);
                countLatency(captureLatency_.presented, afterPresentNs - frame->originNs, frame->originSynced);
            }
            reportPresent(frame->timing.hostPtsNs, presentCallNs, kNoScanout);
        }
    }
    if (presentId != 0 && traceRecording_.load(std::memory_order_relaxed)) {
        std::lock_guard<std::mutex> lock(mutex_);
        trace_.rendered(presentId, startNs, fencedNs, acquiredNs, afterPresentNs, imageIndex, fenceWaited, releasedNs);
    }

    slotFrames_[slot] = frame;
    lastPresentNs_ = afterPresentNs;
    if (frame != current_) {
        lastNewFrameNs_ = lastPresentNs_;
    }
    current_ = frame;
    imported->lastUsed = frameCounter_;
    frameCounter_++;

    if (result == VK_ERROR_OUT_OF_DATE_KHR) {
        swapchainDirty_ = true;
    }
    else if (result == VK_SUBOPTIMAL_KHR &&
             afterPresentNs - lastSuboptimalCheckNs_ >= kSuboptimalCheckIntervalNs) {
        lastSuboptimalCheckNs_ = afterPresentNs;
        // Android reports suboptimal when the display's rotation no longer matches our
        // pre-rotation. A new rotation or size needs a new swapchain.
        VkSurfaceCapabilitiesKHR caps;
        if (vk_.vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice_, surface_, &caps) == VK_SUCCESS) {
            const VkSurfaceTransformFlagBitsKHR transform = choosePreTransform(caps);
            const VkExtent2D extent = preRotatedExtent(caps.currentExtent, transform);
            if (transform != preTransform_ ||
                    (caps.currentExtent.width != UINT32_MAX &&
                     (extent.width != extent_.width || extent.height != extent_.height))) {
                swapchainDirty_ = true;
            }
        }
    }
    else if (result != VK_SUCCESS && result != VK_SUBOPTIMAL_KHR) {
        ALOGE("vkQueuePresentKHR failed: %d", result);
        swapchainDirty_ = true;
        noteFailure("presenting a frame", result, kFatalFailures);
    }
    if (result == VK_SUCCESS || result == VK_SUBOPTIMAL_KHR || result == VK_ERROR_OUT_OF_DATE_KHR) {
        failures_ = 0;
    }

    evictImports();
    return true;
}

void VulkanRenderer::collectPresentTimings() {
    if (!hasDisplayTimingExt_ || !swapchain_) {
        return;
    }
    uint32_t count = 0;
    if (vk_.vkGetPastPresentationTimingGOOGLE(device_, swapchain_, &count, nullptr) != VK_SUCCESS || count == 0) {
        return;
    }
    // Polled each vsync: the buffer is kept between calls
    std::vector<VkPastPresentationTimingGOOGLE>& timings = pastTimings_;
    timings.resize(count);
    if (vk_.vkGetPastPresentationTimingGOOGLE(device_, swapchain_, &count, timings.data()) < 0) {
        return;
    }
    for (uint32_t i = 0; i < count; i++) {
        const VkPastPresentationTimingGOOGLE& t = timings[i];

        // Reports come in present order, so earlier presents still without one were dropped
        while (!pendingPresents_.empty() && pendingPresents_.front().key < t.presentID) {
            const PendingPresent& missed = pendingPresents_.front().value;
            if (!missed.replaced && !missed.onArrival) {
                presentScheduler_.onMissed(missed.ahead, missed.slackNs, missed.vsyncNs,
                                           missed.delayVsyncs, currentPeriodNs_, presentsAhead());
            }
            reportPresent(missed.hostPtsNs, 0, 0);
            pendingPresents_.popFront();
        }

        PendingPresent* pending = pendingPresents_.find(t.presentID);
        if (pending != nullptr && !pending->onArrival && t.actualPresentTime > 0) {
            presentScheduler_.observeHardwareVsync(pending->rawVsyncNs,
                                                   static_cast<int64_t>(t.actualPresentTime), currentPeriodNs_);
        }
        if (pending != nullptr && pending->ahead && !pending->replaced) {
            presentScheduler_.onAheadOutcome(pending->slackNs, pending->vsyncNs,
                                             pending->delayVsyncs, true,
                                             static_cast<int64_t>(t.actualPresentTime),
                                             static_cast<int64_t>(t.earliestPresentTime),
                                             static_cast<int64_t>(t.presentMargin), currentPeriodNs_);
        }

        // With frames presented ahead, the delay is tuned for those. A frame that arrived late,
        // and so was presented at its vsync or with less time than the guard, may miss; that
        // frame is late on screen whatever the delay, and it shouldn't raise the delay for all
        // the others.
        const bool counts = pending != nullptr && !pending->onArrival &&
                (!presentsAhead() || (pending->ahead && pending->slackNs >= presentScheduler_.guardNs()));
        if (counts) {
            presentScheduler_.onPresented(pending->vsyncNs, pending->delayVsyncs, currentPeriodNs_,
                                          static_cast<int64_t>(t.actualPresentTime),
                                          static_cast<int64_t>(t.earliestPresentTime),
                                          static_cast<int64_t>(t.presentMargin),
                                          pending->ahead ? pending->slackNs : -1);
        }
        if (pending != nullptr) {
            // A frame replaced at its vsync never reached the screen
            if (!pending->replaced) {
                if (pending->originNs != 0) {
                    std::lock_guard<std::mutex> lock(latencyMutex_);
                    countLatency(captureLatency_.presented,
                                 static_cast<int64_t>(t.actualPresentTime) - pending->originNs,
                                 pending->originSynced);
                }
                reportPresent(pending->hostPtsNs, pending->queuedNs,
                              static_cast<int64_t>(t.actualPresentTime));
            }
            else {
                reportPresent(pending->hostPtsNs, 0, 0);
            }
            pendingPresents_.erase(t.presentID);
        }
    }

    std::lock_guard<std::mutex> lock(mutex_);
    presentScheduler_.setAheadWindowNs(pacer_.presentAheadVsyncs() * pacer_.vsyncPeriodNs());
    if (!frontBufferPaced_) {
        pacer_.setPresentGuardNs(presentScheduler_.guardNs());
    }
    for (uint32_t i = 0; i < count; i++) {
        const VkPastPresentationTimingGOOGLE& t = timings[i];
        if (t.presentID != 0) {
            trace_.presented(t.presentID, static_cast<int64_t>(t.actualPresentTime),
                             static_cast<int64_t>(t.earliestPresentTime), static_cast<int64_t>(t.presentMargin));
        }
    }
}

void VulkanRenderer::noteReceived(int64_t hostPtsNs, int64_t receiveNs, int64_t enqueueNs, int64_t hostProcessingNs,
                                  int64_t sessionNowNs) {
    // Called as the frame goes to the decoder, so the time on our clock is when decoding starts
    const int64_t queuedNs = nowNs();
    const int64_t commonToLocalNs = queuedNs - sessionNowNs;

    // Where its capture latency counts from. A frame the host gave no processing time for repeats
    // an earlier one, so it has no capture of its own.
    if (hostProcessingNs != 0) {
        FrameOrigin origin {0, false};
        uint64_t captureUs = 0;
        if (LiGetHostTimeOnLocalClock(static_cast<uint64_t>(hostPtsNs / 1000), &captureUs)) {
            origin = {static_cast<int64_t>(captureUs) * 1000 + commonToLocalNs, true};
        }
        else if (receiveNs != 0) {
            // Without clock sync, nothing measures the network transit per frame, so it's left out.
            // The host's part is on its clock and the rest on ours, so they add without the clocks
            // needing to agree.
            origin = {receiveNs + commonToLocalNs - hostProcessingNs, false};
        }
        if (origin.ns != 0) {
            std::lock_guard<std::mutex> lock(latencyMutex_);
            origins_.put(hostPtsNs, origin);
        }
    }

    if (!traceRecording_.load(std::memory_order_relaxed)) {
        return;
    }
    // The trace has no round-trip time of the engine's to read here.
    const uint32_t rttMs = 0;
    std::lock_guard<std::mutex> lock(mutex_);
    trace_.received(hostPtsNs, receiveNs, enqueueNs, queuedNs, commonToLocalNs, hostProcessingNs, rttMs);
}

void VulkanRenderer::noteHdr10Plus(int64_t hostPtsNs, const uint8_t* data, size_t size) {
    auto blob = size != 0 ? std::make_shared<const std::vector<uint8_t>>(data, data + size) : nullptr;
    std::lock_guard<std::mutex> lock(latencyMutex_);
    hdr10PlusQueued_[hostPtsNs] = std::move(blob);
    // Only changes are noted, but a decoder that drops its input never decodes them
    while (hdr10PlusQueued_.size() > 64) {
        hdr10PlusQueued_.erase(hdr10PlusQueued_.begin());
    }
}

void VulkanRenderer::noteDecoded(VideoFrame& frame) {
    std::lock_guard<std::mutex> lock(latencyMutex_);
    // The metadata noted last at or before this frame, which frames keep until it changes
    auto hdr10PlusEnd = hdr10PlusQueued_.upper_bound(frame.timing.hostPtsNs);
    if (hdr10PlusEnd != hdr10PlusQueued_.begin()) {
        hdr10PlusDecoded_ = std::prev(hdr10PlusEnd)->second;
        hdr10PlusQueued_.erase(hdr10PlusQueued_.begin(), hdr10PlusEnd);
    }
    frame.hdr10Plus = hdr10PlusDecoded_;

    FrameOrigin found {};
    if (!origins_.popThrough(frame.timing.hostPtsNs, &found)) {
        return;
    }
    frame.originNs = found.ns;
    frame.originSynced = found.synced;
    countLatency(captureLatency_.decoded, frame.timing.arrivalNs - frame.originNs, frame.originSynced);
}

// Under latencyMutex_. Outside a second, the clock sync or the frame's timing must be wrong.
void VulkanRenderer::countLatency(LatencySpan& span, int64_t latencyNs, bool synced) {
    if (latencyNs < 0 || latencyNs >= 1'000'000'000) {
        return;
    }
    span.totalNs += latencyNs;
    span.frames++;
    if (!synced) {
        span.unsynced++;
    }
}

VulkanRenderer::CaptureLatency VulkanRenderer::takeCaptureLatency() {
    std::lock_guard<std::mutex> lock(latencyMutex_);
    CaptureLatency latency = captureLatency_;
    captureLatency_ = {};
    return latency;
}

size_t VulkanRenderer::takePresentReports(int64_t* out, size_t max) {
    std::lock_guard<std::mutex> lock(latencyMutex_);
    size_t n = 0;
    while (n < max && !presentReports_.empty()) {
        out[3 * n] = presentReports_.front().hostPtsNs;
        out[3 * n + 1] = presentReports_.front().queuedNs;
        out[3 * n + 2] = presentReports_.front().presentNs;
        presentReports_.pop_front();
        n++;
    }
    return n;
}


}  // namespace vkr
