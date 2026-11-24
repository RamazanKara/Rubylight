// VulkanRenderer synchronization, teardown, runtime controls and overlay diagnostics text.
// Adapted from MoreOrLessSoftware/moonlight-android (see SOURCE).

#include "vulkan_renderer_internal.h"

namespace vkr {

using namespace renderer_detail;

void VulkanRenderer::waitIdle() {
    if (deviceLost_) {
        return;
    }
    // vkDeviceWaitIdle has no timeout. Stopping happens on the UI thread, so first make sure the
    // work in flight does finish (a presented frame completes on its own once the GPU is idle).
    if (quit_ && !drainFences(kStopDrainNs)) {
        ALOGE("The GPU did not finish its work when stopping");
        wedged_ = true;
        return;
    }
    if (deviceLost_) {
        return;
    }
    std::lock_guard<std::mutex> lock(queueMutex_);
    vk_.vkDeviceWaitIdle(device_);
}

bool VulkanRenderer::drainFences(uint64_t budgetNs) {
    const int64_t deadlineNs = nowNs() + static_cast<int64_t>(budgetNs);
    for (int i = 0; i < kFramesInFlight; i++) {
        if (!fences_[i]) {
            continue;
        }
        const int64_t leftNs = std::max<int64_t>(deadlineNs - nowNs(), 0);
        const VkResult result = vk_.vkWaitForFences(device_, 1, &fences_[i], VK_TRUE, static_cast<uint64_t>(leftNs));
        if (result == VK_ERROR_DEVICE_LOST) {
            deviceLost_ = true;
            return true;
        }
        if (result != VK_SUCCESS) {
            return false;
        }
    }
    return true;
}

VkResult VulkanRenderer::waitFence(VkFence fence, uint64_t timeoutNs) {
    const int64_t deadlineNs = nowNs() + static_cast<int64_t>(timeoutNs);
    for (;;) {
        const int64_t leftNs = std::max<int64_t>(deadlineNs - nowNs(), 0);
        const uint64_t sliceNs = std::min<uint64_t>(static_cast<uint64_t>(leftNs), kStopWaitNs);
        const VkResult result = vk_.vkWaitForFences(device_, 1, &fence, VK_TRUE, sliceNs);
        if (result != VK_TIMEOUT || quit_ || leftNs <= static_cast<int64_t>(kStopWaitNs)) {
            return result;
        }
    }
}

void VulkanRenderer::setFatal(const std::string& reason) {
    {
        std::lock_guard<std::mutex> lock(fatalMutex_);
        if (fatal_) {
            return;
        }
        fatalReason_ = reason;
        fatal_ = true;
    }
    ALOGE("Giving up: %s", reason.c_str());
}

std::string VulkanRenderer::fatalReason() {
    std::lock_guard<std::mutex> lock(fatalMutex_);
    return fatalReason_;
}

void VulkanRenderer::noteFailure(const char* what, VkResult result, int limit) {
    if (quit_) {
        return;
    }
    if (result == VK_ERROR_DEVICE_LOST) {
        deviceLost_ = true;
        setFatal(std::string("the GPU was lost (") + what + ")");
        return;
    }
    const int failures = ++failures_;
    if (failures >= limit) {
        setFatal(std::string(what) + " failed " + std::to_string(failures) + " times in a row (" +
                 std::to_string(result) + ")");
    }
}

void VulkanRenderer::recreateSlotSync(int slot) {
    if (fences_[slot]) {
        vk_.vkDestroyFence(device_, fences_[slot], nullptr);
        fences_[slot] = VK_NULL_HANDLE;
    }
    if (imageAcquired_[slot]) {
        vk_.vkDestroySemaphore(device_, imageAcquired_[slot], nullptr);
        imageAcquired_[slot] = VK_NULL_HANDLE;
    }
    // An imported decoder fence that no submit waited for goes with its semaphore
    if (decodeDone_[slot]) {
        vk_.vkDestroySemaphore(device_, decodeDone_[slot], nullptr);
        decodeDone_[slot] = VK_NULL_HANDLE;
    }
    VkFenceCreateInfo fenceInfo {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
    VkSemaphoreCreateInfo semInfo {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    if (vk_.vkCreateFence(device_, &fenceInfo, nullptr, &fences_[slot]) != VK_SUCCESS ||
            vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &imageAcquired_[slot]) != VK_SUCCESS ||
            (gpuDecodeWait_ && vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &decodeDone_[slot]) != VK_SUCCESS)) {
        setFatal("could not recover from a failed GPU submit");
    }
}

bool VulkanRenderer::createImageReader() {
    media_status_t status = ndk_->AImageReader_newWithUsage(
            config_.streamWidth, config_.streamHeight, AIMAGE_FORMAT_PRIVATE,
            AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE, kMaxReaderImages, &reader_);
    if (status != AMEDIA_OK) {
        ALOGE("AImageReader_newWithUsage failed: %d", status);
        return false;
    }

    imageListener_.context = this;
    imageListener_.onImageAvailable = &VulkanRenderer::onImageAvailableThunk;
    ndk_->AImageReader_setImageListener(reader_, &imageListener_);

    if (ndk_->AImageReader_getWindow(reader_, &decoderWindow_) != AMEDIA_OK) {
        ALOGE("AImageReader_getWindow failed");
        return false;
    }
    return true;
}

VulkanRenderer::~VulkanRenderer() {
    stop();

    if (reader_) {
        ndk_->AImageReader_setImageListener(reader_, nullptr);
    }

    std::deque<FramePtr> pending;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        closing_.store(true, std::memory_order_release);
        pending.swap(pending_);
        trace_.close();
        traceRecording_ = false;
    }
    pending.clear();

    bool leakDevice = false;
    if (device_) {
        if (deviceLost_) {
            ALOGW("The GPU was lost; releasing its resources without waiting for it");
        }
        else if (wedged_) {
            // stop() gave up on the GPU so the UI thread wouldn't wait; this runs on the stop
            // worker. Destroying the device under pending work is undefined behaviour, but a GPU
            // that never answers must not keep the process from starting another connection, so
            // wait for a while, in a thread that may outlive this object (it holds copies only).
            ALOGW("The GPU had not finished when stopping; waiting for it before releasing its resources");
            struct IdleWait {
                std::mutex mutex;
                std::condition_variable cv;
                bool done = false;
                VkResult result = VK_SUCCESS;
            };
            auto wait = std::make_shared<IdleWait>();
            const auto waitIdleFn = vk_.vkDeviceWaitIdle;
            const VkDevice device = device_;
            const int64_t startNs = nowNs();
            std::thread([wait, waitIdleFn, device] {
                const VkResult result = waitIdleFn(device);
                std::lock_guard<std::mutex> lock(wait->mutex);
                wait->result = result;
                wait->done = true;
                wait->cv.notify_all();
            }).detach();
            std::unique_lock<std::mutex> lock(wait->mutex);
            if (wait->cv.wait_for(lock, std::chrono::seconds(kWedgedWaitSeconds), [&] { return wait->done; })) {
                const VkResult idle = wait->result;
                if (idle == VK_ERROR_DEVICE_LOST) {
                    deviceLost_ = true;
                }
                ALOGW("The GPU %s after %lld ms (%d)", idle == VK_SUCCESS ? "finished" : "gave up",
                      static_cast<long long>((nowNs() - startNs) / 1'000'000), idle);
            }
            else {
                ALOGE("The GPU still had not finished after %d s; leaking the device, swapchain and surface "
                      "so the stream can end",
                      kWedgedWaitSeconds);
                leakDevice = true;
            }
        }
        else {
            waitIdle();
        }
    }
    current_.reset();
    for (auto& frame : slotFrames_) {
        frame.reset();
    }

    if (leakDevice) {
        // Nothing of the device, its instance or the surface is touched: they may still be in use
        instance_ = VK_NULL_HANDLE;
    }
    else if (device_) {
        // What stop() left alone if the GPU was wedged (a no-op otherwise)
        releaseOutput();
        destroyConversion();
        destroyWarpResources();
        for (int i = 0; i < kFramesInFlight; i++) {
            if (fences_[i]) vk_.vkDestroyFence(device_, fences_[i], nullptr);
            if (imageAcquired_[i]) vk_.vkDestroySemaphore(device_, imageAcquired_[i], nullptr);
            if (decodeDone_[i]) vk_.vkDestroySemaphore(device_, decodeDone_[i], nullptr);
        }
        if (timestampPool_) vk_.vkDestroyQueryPool(device_, timestampPool_, nullptr);
        if (commandPool_) vk_.vkDestroyCommandPool(device_, commandPool_, nullptr);
        if (vertShader_) vk_.vkDestroyShaderModule(device_, vertShader_, nullptr);
        if (fragShader_) vk_.vkDestroyShaderModule(device_, fragShader_, nullptr);
        vk_.vkDestroyDevice(device_, nullptr);
    }
    if (instance_ && vk_.vkDestroyInstance) {
        vk_.vkDestroyInstance(instance_, nullptr);
    }

    // Waits for any image callback still running, which sees closing_ and backs out
    if (reader_) {
        ndk_->AImageReader_delete(reader_);
    }
    if (wakeFd_ >= 0) {
        close(wakeFd_);
    }
}

void VulkanRenderer::releaseOutput() {
    if (device_) {
        destroySwapchain();
        if (renderPass_) {
            destroyPipelines();
            vk_.vkDestroyRenderPass(device_, renderPass_, nullptr);
            renderPass_ = VK_NULL_HANDLE;
        }
    }
    if (surface_) {
        vk_.vkDestroySurfaceKHR(instance_, surface_, nullptr);
        surface_ = VK_NULL_HANDLE;
    }
}

void VulkanRenderer::stop() {
    std::lock_guard<std::mutex> stopLock(stopMutex_);
    if (stopped_) {
        return;
    }
    stopped_ = true;

    if (renderThread_.joinable()) {
        quit_ = true;
        wake(kWakeQuit);
        renderThread_.join();
    }

    // The image reader may be presenting a frame: wait for it, and keep it from starting another
    std::lock_guard<std::mutex> renderLock(renderMutex_);
    renderStopped_ = true;
    alignedDraws_.clear();
    if (device_) {
        waitIdle();
    }
    // A GPU that never finished is left alone; destroying what it is using would be worse. The
    // destructor waits for it up to kWedgedWaitSeconds and then releases these, or leaks them if
    // it still has not finished.
    if (!wedged_) {
        releaseOutput();
    }
    if (outputWindow_) {
        // The surface outlives the renderer, and a stream after this one may be SDR
        if (hdr10PlusWindow_.holds()) {
            hdr10PlusWindow_.set(nullptr, 0);
        }
        if (hdr10PlusCalls_ != 0) {
            ALOGI("HDR10+ metadata set %u times, %.1f us per call on average, %.1f us at most", hdr10PlusCalls_,
                  hdr10PlusCallNs_ / 1e3 / hdr10PlusCalls_, hdr10PlusMaxCallNs_ / 1e3);
        }
        hdr10PlusWindow_ = Hdr10PlusWindow();
        ANativeWindow_release(outputWindow_);
        outputWindow_ = nullptr;
    }
}

void VulkanRenderer::setHdrMode(bool enabled, const uint8_t* metadata, size_t metadataLength) {
    HdrMetadata parsed;
    if (metadata && metadataLength >= 24) {
        uint16_t v[12];
        memcpy(v, metadata, sizeof(v));
        ALOGI("HDR metadata: max %u nits, min %.4f nits, MaxCLL %u, MaxFALL %u", v[8], v[9] * 0.0001f, v[10], v[11]);

        // A 10000 nit peak is the top of the PQ range, which hosts send when they don't know
        // the display's real peak. Passed on, it has the compositor squeeze everything above
        // the panel's own peak into a sliver, flattening the host's luminance, so leave it out.
        if (v[8] >= 10000) {
            ALOGI("Ignoring placeholder HDR metadata");
        }
        else {
            // Layout of SS_HDR_METADATA: R, G, B primaries and white point in 0.00002 units,
            // max display luminance in nits, min display luminance in 0.0001 nits, then MaxCLL
            // and MaxFALL in nits
            auto xy = [](uint16_t x, uint16_t y) { return VkXYColorEXT {x * 0.00002f, y * 0.00002f}; };
            parsed.vk.displayPrimaryRed = xy(v[0], v[1]);
            parsed.vk.displayPrimaryGreen = xy(v[2], v[3]);
            parsed.vk.displayPrimaryBlue = xy(v[4], v[5]);
            parsed.vk.whitePoint = xy(v[6], v[7]);
            parsed.vk.maxLuminance = v[8];
            parsed.vk.minLuminance = v[9] * 0.0001f;
            // Windows reports no MaxCLL or MaxFALL for the desktop (0). Passed on as 0 nits, a TV
            // such as the TCL C6K takes the content as unknown and tone maps it washed out, so a
            // missing light level falls back to the display's peak.
            parsed.vk.maxContentLightLevel = v[10] != 0 ? v[10] : v[8];
            parsed.vk.maxFrameAverageLightLevel = v[11] != 0 ? v[11] : v[8];

            if (v[10] != 0) {
                parsed.contentPeakNits = v[10];
            }
            else if (v[8] != 0) {
                parsed.contentPeakNits = v[8];
            }
        }
    }

    {
        std::lock_guard<std::mutex> lock(mutex_);
        hdrEnabled_ = enabled;
        hdrMetadata_ = parsed;
        contentPeakNits_.store(parsed.contentPeakNits, std::memory_order_relaxed);
        hdrChanged_ = true;
    }
    wake(kWakeHdr);
}

void VulkanRenderer::setJitterBuffer(JitterBuffer jitterBuffer) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (jitterBuffer == pacer_.timeline().jitterBuffer()) {
        return;
    }
    ALOGI("Jitter buffer changed to %d", static_cast<int>(jitterBuffer));
    pacer_.setJitterBuffer(jitterBuffer);
    instant_ = pacer_.mode() == PacingMode::HostTimed && jitterBuffer == JitterBuffer::Instant;
    trace_.jitterBuffer(jitterBuffer);
}

void VulkanRenderer::setAdaptiveRefresh(bool adaptive) {
    std::lock_guard<std::mutex> lock(mutex_);
    pacer_.setAdaptiveRefresh(adaptive);
}

std::string VulkanRenderer::rendererText() {
    std::lock_guard<std::mutex> lock(mutex_);

    return "Vulkan " + outputDescription_;
}

std::string VulkanRenderer::diagnosticsText() {
    std::lock_guard<std::mutex> lock(mutex_);
    if (swapchainDescription_.empty()) {
        return {};
    }
    char text[96];
    snprintf(text, sizeof(text), "; waits over half a refresh since the start: image %u, GPU %u",
             slowAcquires_.load(std::memory_order_relaxed), slowFenceWaits_.load(std::memory_order_relaxed));
    return "Vulkan output " + swapchainDescription_ + text;
}

std::string VulkanRenderer::pacingText() {
    std::lock_guard<std::mutex> lock(mutex_);

    std::string headline;
    std::string details;
    char text[96];
    if (instant_) {
        // No buffer: frames go to the next vsync they can make
        snprintf(text, sizeof(text), "instant, %.2f Hz", 1e9 / static_cast<double>(pacer_.vsyncPeriodNs()));
        headline = text;
    }
    else if (pacer_.mode() == PacingMode::HostTimed && pacer_.timeline().hasEstimate()) {
        // The display's measured refresh rate, to compare with the host's frame rate
        snprintf(text, sizeof(text), "%.1f ms buffer, %.2f Hz", (pacer_.timeline().bufferNs() + pacer_.scheduleDelayNs()) / 1e6,
                 1e9 / static_cast<double>(pacer_.vsyncPeriodNs()));
        headline = text;
        if (pacer_.phaseLocked()) {
            snprintf(text, sizeof(text), "locked at %d vsync%s/frame", static_cast<int>(pacer_.slotVsyncs()),
                     pacer_.slotVsyncs() == 1 ? "" : "s");
            details = text;
        }
        else {
            details = "unlocked";
        }
    }
    // Frames skipped recently. A running total since the stream started says little about
    // how it's going now.
    const int64_t now = nowNs();
    const uint64_t total = pacer_.framesSkipped() + queueOverflowDrops_;
    skipSamples_.push_back({now, total});
    while (skipSamples_.size() > 1 && skipSamples_[1].timeNs <= now - kRecentSkipsNs) {
        skipSamples_.pop_front();
    }
    const uint64_t skipped = total - skipSamples_.front().total;
    if (skipped > 0) {
        snprintf(text, sizeof(text), "%" PRIu64 " skipped in last %d s", skipped,
                 static_cast<int>(kRecentSkipsNs / 1'000'000'000));
        details += details.empty() ? text : std::string(", ") + text;
    }
    return headline + "\n" + details;
}

void VulkanRenderer::wake(uint32_t flags) {
    wakeFlags_.fetch_or(flags);
    if (wakeFd_ >= 0) {
        uint64_t one = 1;
        (void) !write(wakeFd_, &one, sizeof(one));
    }
}


}  // namespace vkr
