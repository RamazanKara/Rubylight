// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// PyroWave image preacquire, display clock, HDR window dataspace/metadata, timing & stats.

#include "pyrowave_renderer_internal.h"

namespace pyrowave_renderer {

// settled (settleAcquireFence) before the image is drawn to.
PyroAcquire Renderer::acquireImage(uint32_t *imageIndex, uint64_t timeoutNs) {
    const VkResult acquired = vk.AcquireNextImageKHR(device, swapchain, timeoutNs, VK_NULL_HANDLE,
                                                     acquireFence, imageIndex);
    if (acquired == VK_SUCCESS || acquired == VK_SUBOPTIMAL_KHR) return PyroAcquire::READY;
    if (acquired == VK_TIMEOUT || acquired == VK_NOT_READY) return PyroAcquire::NOT_READY;
    if (acquired == VK_ERROR_OUT_OF_DATE_KHR) return PyroAcquire::OUT_OF_DATE;
    check(acquired, "vkAcquireNextImageKHR");
    return PyroAcquire::FAILED;
}

bool Renderer::settleAcquireFence() {
    return check(vk.WaitForFences(device, 1, &acquireFence, VK_TRUE, FENCE_TIMEOUT_NS), "acquire fence") &&
           check(vk.ResetFences(device, 1, &acquireFence), "reset acquire fence");
}

// Pre-acquire follows the Debug setting "PyroWave early screen image" (off by default: it
// came out with the 3.2.3 decode reverts), which debug.nightjar.pyro_preacquire 1 or 0
// overrides. Only presents on arrival take it: paced modes acquire per frame.
void Renderer::configurePreacquire(bool setting) {
    char value[PROP_VALUE_MAX] = {};
    __system_property_get("debug.nightjar.pyro_preacquire", value);
    const bool requested = std::strcmp(value, "1") == 0 || (std::strcmp(value, "0") != 0 && setting);
    preacquireImages = requested && framePacingMode == 0;
    if (requested) {
        LOGI("PyroWave swapchain pre-acquire %s", preacquireImages ? "on" : "asked for, but off in paced modes");
    }
}

// Real VRR: this renderer's own instance of the shared present clock
// (protocol/core/src/present_clock.rs) behind presentTiming's HostTimes mode, set up as
// the MediaCodec path's is (android_present_clock.c): for a display that refreshes when
// a frame comes, with one refresh at the display's top rate as the shortest interval.
// The clock reads no time itself. The ready times it is given (a slot's completedNs)
// and the present times it answers are CLOCK_MONOTONIC, the clock of nowNs() and of
// the times VK_GOOGLE_display_timing takes on Android. False without a clock.
bool Renderer::createPresentClock(float topRefreshRate) {
    RpPresentSettings settings;
    rp_present_settings_adaptive(&settings);
    settings.min_interval_us = topRefreshRate > 1.0f && topRefreshRate <= 1000.0f ?
            int64_t(1'000'000.0f / topRefreshRate) : 0;
    presentClock = rp_present_clock_new(&settings);
    if (presentClock == nullptr) {
        LOGW("PyroWave pacing: no clock for the PC's frame times; frames are shown as they arrive");
        return false;
    }
    presentTiming.setHostSchedule([](void *clock, uint32_t rtp, uint64_t readyUs) -> uint64_t {
        return rp_present_clock_schedule(static_cast<RpPresentClock *>(clock), rtp, readyUs).present_us;
    }, presentClock);
    LOGI("PyroWave pacing: PC frame times (the display's shortest refresh is %.2f ms)",
         double(settings.min_interval_us) / 1000.0);
    return true;
}

// Presenter-owned scheduler input. Android's callback samples are copied by
// the submission thread; driver timing always refers to this output surface.
void Renderer::refreshDisplayClock() {
    int64_t phase, period;
    {
        std::lock_guard<std::mutex> lock(clockMutex);
        phase = displayClockNs;
        period = displayClockIntervalNs;
    }
    const uint64_t now = nowUs();
    const int64_t ageNs = nowNs() - phase;
    const bool freshClock = phase > 0 && period >= 1'000'000 && period <= 100'000'000 &&
                            ageNs >= 0 && ageNs < 500'000'000;
    const bool modeChanged = freshClock && driverRefreshPeriodNs > 0 &&
            (period > driverRefreshPeriodNs + driverRefreshPeriodNs / 20 ||
             period < driverRefreshPeriodNs - driverRefreshPeriodNs / 20);
    if (displayTimingSupported && vk.GetRefreshCycleDurationGOOGLE && swapchain != VK_NULL_HANDLE &&
        (lastRefreshCheckUs == 0 || now - lastRefreshCheckUs >= 1'000'000 ||
         (modeChanged && now - lastRefreshCheckUs >= 100'000))) {
        lastRefreshCheckUs = now;
        VkRefreshCycleDurationGOOGLE duration = {};
        if (vk.GetRefreshCycleDurationGOOGLE(device, swapchain, &duration) == VK_SUCCESS &&
            duration.refreshDuration >= 1'000'000 && duration.refreshDuration <= 100'000'000) {
            driverRefreshPeriodNs = int64_t(duration.refreshDuration);
        }
    }
    if (driverRefreshPeriodNs > 0) presentTiming.setRefreshPeriod(driverRefreshPeriodNs);
    else if (freshClock) {
        presentTiming.setRefreshPeriod(period);
    }
    if (phase > 0 && ageNs >= 0 && ageNs < 500'000'000) presentTiming.observeVsync(phase);
}

void Renderer::dropSlot(int index) {
    std::lock_guard<std::mutex> lock(slotMutex);
    pushRelease(slots[index].ptsUs, 0, false);
}

// Release events for Java's frame statistics, delivered by nativePollRenderedFrames.
void Renderer::pushRelease(int64_t ptsUs, int64_t releaseNs, bool shown) {
    std::lock_guard<std::mutex> lock(eventsMutex);
    if (releasedFrames.size() >= 3 * 8192) return;
    releasedFrames.push_back(ptsUs);
    releasedFrames.push_back(releaseNs);
    releasedFrames.push_back(shown ? 1 : 0);
}

// HDR changes come from Java's thread and are applied here, by the swapchain's owner.
bool Renderer::applyPendingHdr() {
    bool enabled, transfer;
    VkHdrMetadataEXT metadata;
    {
        std::lock_guard<std::mutex> lock(slotMutex);
        if (!hdrPending && !transferPending) return true;
        enabled = hdrPending ? pendingHdr : hdr;
        metadata = hdrPending ? pendingHdrMetadata : hdrMetadata;
        transfer = transferPending ? pendingHlg : hlg;
        hdrPending = transferPending = false;
    }
    const bool changed = hdr != enabled;
    hdr = enabled;
    hdrMetadata = metadata;
    if (transfer != hlg) {
        // The ANNOUNCE settled on HLG (or PQ): the format stays, the colour space or window
        // dataspace follows the transfer.
        hlg = transfer;
        dataspaceOverride = !pyroNativeHdr(surfaceFormats.data(), surfaceFormats.size(),
                                           int32_t(swapchainFormat), hlg);
        logHdrOutput();
        return recreateSwapchain();
    }
    // The dataspace override keeps the same swapchain for HDR on and off; only the window changes.
    if (changed && !dataspaceOverride) return recreateSwapchain();
    applyHdrMetadata();
    return applyWindowDataspace();
}

// Java calls this before its final event poll. Destruction repeats the
// stop safely, after the caller has received all queued completions.
void Renderer::drain() {
    stopWorkers();
    pollRenderedFrames();
}

// Present thread only: it owns the swapchain. One driver query per call, two only when
// more times are waiting than pastTimings holds.
void Renderer::pollRenderedFrames() {
    presentPoll.polled(nowNs());
    if (!displayTimingSupported || swapchain == VK_NULL_HANDLE || pendingPresents.empty()) return;
    if (pastTimings.empty()) pastTimings.resize(32);
    for (int round = 0; round < 4; ++round) {
        uint32_t count = uint32_t(pastTimings.size());
        auto &timings = pastTimings;
        auto result = vk.GetPastPresentationTimingGOOGLE(device, swapchain, &count, timings.data());
        if (result != VK_SUCCESS && result != VK_INCOMPLETE) break;
        for (uint32_t i = 0; i < count; ++i) {
            const auto &timing = timings[i];
            auto frame = std::find_if(pendingPresents.begin(), pendingPresents.end(),
                [&timing](const auto &entry) { return entry.id == timing.presentID; });
            if (frame == pendingPresents.end()) continue;
            presentationDiagnostics.record(frame->completedNs, frame->releasedNs, timing.actualPresentTime);
            presentTiming.feedback(timing.presentID, int64_t(timing.actualPresentTime),
                                   int64_t(timing.earliestPresentTime), int64_t(timing.presentMargin));
            // actualPresentTime uses CLOCK_MONOTONIC on Android, like System.nanoTime().
            if (timing.actualPresentTime != 0) {
                std::lock_guard<std::mutex> lock(eventsMutex);
                renderedFrames.push_back(frame->ptsUs);
                renderedFrames.push_back(jlong(timing.actualPresentTime));
            }
            pendingPresents.erase(frame);
        }
        if (result != VK_INCOMPLETE) break;
    }
    // Skipped frames may never get a time, even when no further timings arrive.
    const uint64_t now = nowUs();
    presentationDiagnostics.reportIfDue(now);
    while (!pendingPresents.empty() && now - pendingPresents.front().queuedUs > 500'000) {
        pendingPresents.pop_front();
    }
}

void Renderer::setHdrMode(bool enabled, const uint8_t *metadata) {
    if (!tenBit || failed) return;
    VkHdrMetadataEXT parsed = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
    if (enabled && metadata) {
        auto word = [metadata](int i) { return float(metadata[i] | (metadata[i + 1] << 8)); };
        parsed.displayPrimaryRed = {word(0) / 50000, word(2) / 50000};
        parsed.displayPrimaryGreen = {word(4) / 50000, word(6) / 50000};
        parsed.displayPrimaryBlue = {word(8) / 50000, word(10) / 50000};
        parsed.whitePoint = {word(12) / 50000, word(14) / 50000};
        parsed.maxLuminance = word(16);
        parsed.minLuminance = word(18) / 10000;
        // Windows reports no MaxCLL or MaxFALL for the desktop (0); as 0 nits a TV takes the
        // content as unknown and tone maps it washed out, so they fall back to the peak.
        parsed.maxContentLightLevel = word(20) > 0 ? word(20) : parsed.maxLuminance;
        parsed.maxFrameAverageLightLevel = word(22) > 0 ? word(22) : parsed.maxLuminance;
    }
    // The present thread applies it before its next frame.
    std::lock_guard<std::mutex> lock(slotMutex);
    pendingHdr = enabled;
    pendingHdrMetadata = parsed;
    hdrPending = true;
}

// The stream's HDR transfer, from the format its ANNOUNCE asked for: HLG, else PQ. Java sets
// it before the first frame; the present thread recreates the swapchain when it changes.
void Renderer::setTransfer(bool hlgTransfer) {
    if (!tenBit || failed) return;
    std::lock_guard<std::mutex> lock(slotMutex);
    pendingHlg = hlgTransfer;
    transferPending = true;
}

// One atomic snapshot preserves completion-before-release across JNI polls.
void Renderer::takeEvents(std::vector<jlong> &completed, std::vector<jlong> &released, std::vector<jlong> &rendered) {
    completed.clear();
    released.clear();
    rendered.clear();
    std::lock_guard<std::mutex> lock(eventsMutex);
    completed.swap(completedFrames);
    released.swap(releasedFrames);
    rendered.swap(renderedFrames);
}

// The API 28 window dataspace calls; minSdk is 21, so they are looked up at run time. The
// libraries stay loaded for the life of the process.
void *Renderer::dataspaceSymbol(const char *name) {
    return pyroFindSymbol(name, [](const char *library) { return dlopen(library, RTLD_NOW | RTLD_LOCAL); },
                          [](void *handle, const char *symbol) { return dlsym(handle, symbol); });
}

// False when HDR is on and the window refused it: the stream would look washed out, so it fails loudly.
// HDR (PQ or HLG) falls back to perform when ANativeWindow_setBuffersDataSpace refuses it (window_dataspace.h;
// Android 11 SHIELD TV refuses BT2020_PQ with -22 while perform accepts it). SRGB keeps the public call.
bool Renderer::applyWindowDataspace() {
    if (!dataspaceOverride || window == nullptr) return true;
    using SetDataSpace = int32_t (*)(ANativeWindow *, int32_t);
    static const auto setDataSpace = reinterpret_cast<SetDataSpace>(dataspaceSymbol("ANativeWindow_setBuffersDataSpace"));
    const int32_t dataspace = pyroWindowDataspace(hdr, hlg);
    const char *name = !hdr ? "SRGB" : hlg ? "BT2020_HLG" : "BT2020_PQ";
    const nwds::DataspaceOutcome outcome = nwds::setDataspace(
            window, dataspace, [](void *w, int32_t d) {
                return setDataSpace != nullptr ? setDataSpace(static_cast<ANativeWindow *>(w), d) : nwds::kMissing;
            }, hdr);
    if (!outcome.accepted()) {
        LOGE("Window dataspace %s (0x%x) was refused (ANativeWindow_setBuffersDataSpace %d%s, perform %d)", name,
             uint32_t(dataspace), outcome.publicResult, setDataSpace == nullptr ? " missing" : "",
             outcome.performResult);
        if (hdr) hdrSurfaceFailure = hlg ? 3 : 2;
        hlgReadbackPending = false;
        return !hdr;
    }
    if (outcome.call == nwds::DataspaceCall::kPerform) {
        LOGI("Window dataspace set to %s (0x%x) through ANativeWindow::perform; "
             "ANativeWindow_setBuffersDataSpace refused it (%d)", name, uint32_t(dataspace), outcome.publicResult);
    } else {
        LOGI("Window dataspace set to %s (0x%x)", name, uint32_t(dataspace));
    }
    // The first frame shown reads it back (checkHlgWindow).
    hlgReadbackPending = hdr && hlg;
    return true;
}

// After the first frame on an HLG window: the window must still hold BT2020_HLG, or the
// stream is not shown as HLG and Java says so (hdrSurfaceFailure 4).
void Renderer::checkHlgWindow() {
    if (!hlgReadbackPending || window == nullptr) return;
    hlgReadbackPending = false;
    using GetDataSpace = int32_t (*)(ANativeWindow *);
    static const auto getDataSpace = reinterpret_cast<GetDataSpace>(dataspaceSymbol("ANativeWindow_getBuffersDataSpace"));
    if (getDataSpace == nullptr) {
        LOGW("PyroWave HLG: ANativeWindow_getBuffersDataSpace missing, the window's dataspace is not read back");
        return;
    }
    const int32_t held = getDataSpace(window);
    if (held != nwds::kDataspaceBt2020Hlg) {
        LOGE("PyroWave HLG: the window holds dataspace %d (0x%x) after the first frame, not BT2020_HLG", held,
             uint32_t(held));
        hdrSurfaceFailure = 4;
        return;
    }
    LOGI("PyroWave HLG: the window holds BT2020_HLG after the first frame");
}

void Renderer::logHdrOutput() {
    if (dataspaceOverride) {
        LOGI("PyroWave HDR: 10-bit SDR swapchain with %s window dataspace (Android <= 14), %s",
             hlg ? "BT2020_HLG" : "BT2020_PQ", hlg ? "HLG" : "HDR10");
    } else {
        LOGI("PyroWave HDR: native %s swapchain", hlg ? "HLG" : "HDR10");
    }
}

// HLG carries no static metadata: the host's mastering data describes PQ. Without the
// host's metadata none is passed, rather than all zeros, so the display uses its own.
void Renderer::applyHdrMetadata() {
    if (tenBit && !hlg && vk.SetHdrMetadataEXT && swapchain && hdrMetadata.maxLuminance > 0) {
        vk.SetHdrMetadataEXT(device, 1, &swapchain, &hdrMetadata);
    }
}

void Renderer::logDecodeStatsIfDue(uint64_t now) {
    auto &stats = decodeStats;
    if (stats.startUs == 0) {
        stats.startUs = now;
        return;
    }
    if (now - stats.startUs < STATS_INTERVAL_US || stats.frames == 0) {
        return;
    }
    const double seconds = double(now - stats.startUs) / 1e6;
    LOGI("%.1f fps submitted for asynchronous decode", stats.frames / seconds);
    const uint32_t decodedFrames = stats.frames;
    logEarlyStats(stats.earlyDone, stats.earlyFallback, stats.earlyWhole);
    stats = {};
    stats.startUs = now;
    {
        // Frames replaced on either thread, as a share of the frames decoded for display.
        std::lock_guard<std::mutex> lock(slotMutex);
        lastReplacedPercent = std::min(100.0f, 100.0f * float(replacedSinceReport) / float(decodedFrames));
        replacedSinceReport = 0;
    }

    // PyroWave's own per-pass GPU timings, to see which stage dominates decode.
    pyrowave_device_report_performance_stats(pyroDevice, [](void *, const char *msg) {
        LOGI("PyroWave pass: %s", msg);
    }, nullptr, true);
}

// Frames whose chunks were not the frame decode as before, whole, but lose what early push
// gains: loud in the log, counted in the overlay.
void Renderer::logEarlyStats(uint32_t done, uint32_t fallback, uint32_t whole) {
    uint64_t totalDone = 0, totalFallback = 0;
    records.earlyStats(totalDone, totalFallback);
    if (done + fallback == 0) {
        lastEarlyPercent = -1;
        lastEarlyFallbacks = 0;
        // Chunks came earlier in the stream and stopped: every frame goes whole again.
        if (whole != 0 && totalDone + totalFallback != 0) {
            LOGW("Early record push: no frame came early, %u pushed whole", whole);
        }
        return;
    }
    const float percent = 100.0f * float(done) / float(done + fallback + whole);
    lastEarlyPercent = percent;
    lastEarlyFallbacks = fallback;
    if (fallback != 0 || whole != 0) {
        LOGW("Early record push: %.1f%% of frames, %u pushed whole after their chunks fell short, %u without "
             "chunks (stream: %llu early, %llu fell back)", percent, fallback, whole,
             (unsigned long long) totalDone, (unsigned long long) totalFallback);
    } else {
        LOGI("Early record push: every frame (stream: %llu early)", (unsigned long long) totalDone);
    }
}

void Renderer::logPresentStatsIfDue(uint64_t now) {
    auto &stats = presentStats;
    if (stats.startUs == 0) {
        stats.startUs = now;
        return;
    }
    if (now - stats.startUs < STATS_INTERVAL_US || stats.frames == 0) {
        return;
    }
    const double seconds = double(now - stats.startUs) / 1e6;
    const float imageWaitMs = float(stats.acquireWaitUs / double(stats.frames) / 1000.0);
    LOGI("%.1f fps presented: wait for swapchain image %.2f ms, replaced while waiting %u",
         stats.frames / seconds, imageWaitMs, stats.replaced);
    lastWaitImageMs = imageWaitMs;
    if (framePacingMode == HOST_TIMES_MODE) {
        // A frame without a host time is shown as it arrives and is not counted.
        float heldMs = -1;
        if (stats.heldFrames != 0) {
            heldMs = float(double(stats.heldNs) / double(stats.heldFrames) / 1e6);
            LOGI("PC frame times: %u frames held %.2f ms on average after they were ready",
                 stats.heldFrames, heldMs);
        }
        lastHostHoldMs = heldMs;
    }
    {
        std::lock_guard<std::mutex> lock(slotMutex);
        replacedSinceReport += stats.replaced;
    }
    stats = {};
    stats.startUs = now;
}

}  // namespace pyrowave_renderer
