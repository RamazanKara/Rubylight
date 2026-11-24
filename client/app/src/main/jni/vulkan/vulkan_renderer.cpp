// VulkanRenderer device probing, Vulkan instance/device/resource initialization and shaders.
// Adapted from MoreOrLessSoftware/moonlight-android (see SOURCE).

#include "vulkan_renderer_internal.h"
#include "shaders_spv.h"

namespace vkr {

using namespace renderer_detail;

namespace {

std::mutex gLastErrorMutex;
std::string gLastError;
std::atomic<bool> gProbeHdrColorSpaces {false};

const char* const kRequiredDeviceExtensions[] = {
    VK_KHR_SWAPCHAIN_EXTENSION_NAME,
    VK_ANDROID_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER_EXTENSION_NAME,
    VK_EXT_QUEUE_FAMILY_FOREIGN_EXTENSION_NAME,
};

// Checks the parts of a physical device we can't run without
bool deviceIsSuitable(VkApi& vk, VkPhysicalDevice device) {
    VkPhysicalDeviceProperties props;
    vk.vkGetPhysicalDeviceProperties(device, &props);
    if (props.apiVersion < VK_API_VERSION_1_1) {
        ALOGR("%s only supports Vulkan %u.%u", props.deviceName,
              VK_API_VERSION_MAJOR(props.apiVersion), VK_API_VERSION_MINOR(props.apiVersion));
        return false;
    }

    uint32_t count = 0;
    vk.vkEnumerateDeviceExtensionProperties(device, nullptr, &count, nullptr);
    std::vector<VkExtensionProperties> extensions(count);
    vk.vkEnumerateDeviceExtensionProperties(device, nullptr, &count, extensions.data());
    for (const char* name : kRequiredDeviceExtensions) {
        if (!hasExtension(extensions, name)) {
            ALOGR("%s lacks %s", props.deviceName, name);
            return false;
        }
    }

    VkPhysicalDeviceSamplerYcbcrConversionFeatures ycbcr {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SAMPLER_YCBCR_CONVERSION_FEATURES};
    VkPhysicalDeviceFeatures2 features {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
    features.pNext = &ycbcr;
    vk.vkGetPhysicalDeviceFeatures2(device, &features);
    if (!ycbcr.samplerYcbcrConversion) {
        ALOGR("%s lacks samplerYcbcrConversion", props.deviceName);
        return false;
    }

    return true;
}

}  // namespace

void recordError(int priority, const char* format, ...) {
    char text[256];
    va_list args;
    va_start(args, format);
    vsnprintf(text, sizeof(text), format, args);
    va_end(args);
    __android_log_write(priority, LOG_TAG, text);
    std::lock_guard<std::mutex> lock(gLastErrorMutex);
    gLastError = text;
}

const uint32_t* warpFragShaderCode(size_t* sizeBytes) {
    *sizeBytes = sizeof(kVideoWarpFragSpv);
    return kVideoWarpFragSpv;
}

bool VulkanRenderer::ConversionKey::operator==(const ConversionKey& other) const {
    return externalFormat == other.externalFormat && format == other.format && ycbcr == other.ycbcr &&
           model == other.model && range == other.range &&
           components.r == other.components.r && components.g == other.components.g &&
           components.b == other.components.b && components.a == other.components.a &&
           xChromaOffset == other.xChromaOffset && yChromaOffset == other.yChromaOffset &&
           filter == other.filter;
}

VkInstance VulkanRenderer::createVkInstance(VkApi& vk, bool* hasColorspaceExt, InstanceSetup& setup,
                                            bool frontBuffer) {
    uint32_t version = 0;
    if (vk.vkEnumerateInstanceVersion(&version) != VK_SUCCESS || version < VK_API_VERSION_1_1) {
        ALOGR("Vulkan instance version too old");
        return VK_NULL_HANDLE;
    }
    uint32_t count = 0;
    vk.vkEnumerateInstanceExtensionProperties(nullptr, &count, nullptr);
    std::vector<VkExtensionProperties> available(count);
    vk.vkEnumerateInstanceExtensionProperties(nullptr, &count, available.data());

    setup.extensions = {
        VK_KHR_SURFACE_EXTENSION_NAME,
        VK_KHR_ANDROID_SURFACE_EXTENSION_NAME,
    };
    for (const char* name : setup.extensions) {
        if (!hasExtension(available, name)) {
            ALOGR("Instance lacks %s", name);
            return VK_NULL_HANDLE;
        }
    }

    // Needed for HDR10 swapchains
    *hasColorspaceExt = hasExtension(available, VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
    if (*hasColorspaceExt) {
        setup.extensions.push_back(VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
    }
    // VK_KHR_shared_presentable_image, for a front buffer, depends on it
    if (frontBuffer && hasExtension(available, VK_KHR_GET_SURFACE_CAPABILITIES_2_EXTENSION_NAME)) {
        setup.extensions.push_back(VK_KHR_GET_SURFACE_CAPABILITIES_2_EXTENSION_NAME);
    }

    setup.app.pApplicationName = "Moonlight";
    setup.app.pEngineName = "Moonlight";
    setup.app.apiVersion = VK_API_VERSION_1_1;

    setup.info.pApplicationInfo = &setup.app;
    setup.info.enabledExtensionCount = static_cast<uint32_t>(setup.extensions.size());
    setup.info.ppEnabledExtensionNames = setup.extensions.data();

    VkInstance instance = VK_NULL_HANDLE;
    VkResult result = vk.vkCreateInstance(&setup.info, nullptr, &instance);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateInstance failed: %d", result);
        return VK_NULL_HANDLE;
    }
    return instance;
}

void VulkanRenderer::prepareDevice(uint32_t queueFamily, bool hdrMetadata, bool displayTiming, DeviceSetup& setup) {
    setup.queue.queueFamilyIndex = queueFamily;
    setup.queue.queueCount = 1;
    setup.queue.pQueuePriorities = &setup.priority;

    setup.extensions.assign(std::begin(kRequiredDeviceExtensions), std::end(kRequiredDeviceExtensions));
    if (hdrMetadata) {
        setup.extensions.push_back(VK_EXT_HDR_METADATA_EXTENSION_NAME);
    }
    // When frames actually reach the screen, for pacer traces
    if (displayTiming) {
        setup.extensions.push_back(VK_GOOGLE_DISPLAY_TIMING_EXTENSION_NAME);
    }

    setup.ycbcr.samplerYcbcrConversion = VK_TRUE;
    setup.info.pNext = &setup.ycbcr;
    setup.info.queueCreateInfoCount = 1;
    setup.info.pQueueCreateInfos = &setup.queue;
    setup.info.enabledExtensionCount = static_cast<uint32_t>(setup.extensions.size());
    setup.info.ppEnabledExtensionNames = setup.extensions.data();
}

bool VulkanRenderer::probe() {
    if (!loadNdkApi()) {
        ALOGR("The NDK media APIs the Vulkan renderer needs are missing (Android 10 or newer)");
        return false;
    }

    VkApi vk;
    if (!vk.loadGlobal()) {
        ALOGR("No Vulkan driver (libvulkan.so) could be loaded");
        return false;
    }

    bool hasColorspaceExt = false;
    InstanceSetup setup;
    VkInstance instance = createVkInstance(vk, &hasColorspaceExt, setup);
    if (instance == VK_NULL_HANDLE) {
        return false;
    }

    bool suitable = false;
    if (vk.loadInstance(instance)) {
        uint32_t count = 0;
        vk.vkEnumeratePhysicalDevices(instance, &count, nullptr);
        std::vector<VkPhysicalDevice> devices(count);
        vk.vkEnumeratePhysicalDevices(instance, &count, devices.data());
        for (VkPhysicalDevice device : devices) {
            if (deviceIsSuitable(vk, device)) {
                suitable = true;
                break;
            }
        }
    }

    vk.vkDestroyInstance(instance, nullptr);
    if (!suitable) {
        ALOGR("No Vulkan device supports what the renderer needs (see the lines above)");
    }
    gProbeHdrColorSpaces = suitable && hasColorspaceExt;
    return suitable;
}

bool VulkanRenderer::probedHdrColorSpaces() {
    return gProbeHdrColorSpaces;
}

std::string VulkanRenderer::lastError() {
    std::lock_guard<std::mutex> lock(gLastErrorMutex);
    return gLastError;
}

std::unique_ptr<VulkanRenderer> VulkanRenderer::create(ANativeWindow* output, const RendererConfig& config) {
    {
        // So a start that fails without logging an error doesn't blame an earlier one
        std::lock_guard<std::mutex> lock(gLastErrorMutex);
        gLastError.clear();
    }
    const NdkApi* ndk = loadNdkApi();
    if (!ndk) {
        return nullptr;
    }

    std::unique_ptr<VulkanRenderer> renderer(new VulkanRenderer(ndk, config));
    if (!renderer->init(output)) {
        return nullptr;
    }
    return renderer;
}

VulkanRenderer::VulkanRenderer(const NdkApi* ndk, const RendererConfig& config)
    : ndk_(ndk),
      config_(config),
      pacer_(static_cast<PacingMode>(config.framePacing), config.streamFps,
             config.displayRefreshHz > 1.0f ? static_cast<int64_t>(1e9 / config.displayRefreshHz) : 16'666'667,
             static_cast<JitterBuffer>(config.jitterBuffer)) {
    char value[PROP_VALUE_MAX] = {};
    // The setting asks for a front buffer aligned to the scan; to force a mode either way,
    // adb shell setprop debug.moonlight.front_buffer 0 (off), 1 (drawn on arrival, tearing) or 2.
    // The setting only takes it for host frame timing, which paces the front buffer's draws, and
    // lowest latency, which shows frames on arrival anyway. The other modes pick a frame at each
    // vsync, which a front buffer would bypass by drawing every frame as it arrives.
    const bool frontBufferMode = pacer_.mode() == PacingMode::HostTimed || pacer_.mode() == PacingMode::MinLatency;
    frontBufferRequested_ = config.frontBuffer && frontBufferMode;
    frontBufferAligned_ = frontBufferRequested_;
    if (__system_property_get("debug.moonlight.front_buffer", value) > 0) {
        frontBufferRequested_ = atoi(value) != 0;
        frontBufferAligned_ = atoi(value) == 2;
    }
    if (frontBufferAligned_ && __system_property_get("debug.moonlight.front_buffer_lead_us", value) > 0) {
        fixedAlignedLeadNs_ = static_cast<int64_t>(std::clamp(atoi(value), 0, 20000)) * 1000;
    }
    if (frontBufferRequested_) {
        ALOGI("Front buffer asked for%s", frontBufferAligned_ ? ", aligned to the scan" : "");
    }
}

bool VulkanRenderer::init(ANativeWindow* output) {
    ANativeWindow_acquire(output);
    outputWindow_ = output;
    hdr10PlusWindow_ = Hdr10PlusWindow(output);

    if (!vk_.loadGlobal() || !createInstance() || !pickDevice() || !createDevice() ||
            !createFrameResources() || !createShaderModules()) {
        return false;
    }

    // Frame Warp's pass, when the stream draws it. Without it the picture is drawn as it is, and
    // Java, which asks drawsFrameWarp(), tells the PC and the user.
    warpApi_ = config_.frameWarp;
    if (warpApi_ && !createWarpResources()) {
        ALOGW("Frame Warp: its pass could not be set up; drawing without it");
        destroyWarpResources();
        warpApi_ = nullptr;
    }
    if (warpApi_) {
        ALOGI("Frame Warp: drawn by the Vulkan renderer");
    }

    // Performance hints for the render threads (PerfHint), set up before any of them start. On
    // unless turned off with adb shell setprop debug.moonlight.perf_hint 0; the target each
    // render's CPU time is held to can be changed with
    // adb shell setprop debug.moonlight.perf_hint_target_us 2000.
    int64_t perfHintTargetNs = PerfHint::kDefaultTargetNs;
    {
        char value[PROP_VALUE_MAX] = {};
        if (__system_property_get("debug.moonlight.perf_hint", value) > 0 && atoi(value) == 0) {
            perfHintTargetNs = 0;
        }
        else if (__system_property_get("debug.moonlight.perf_hint_target_us", value) > 0 && atoi(value) > 0) {
            perfHintTargetNs = static_cast<int64_t>(std::clamp(atoi(value), 100, 100000)) * 1000;
        }
    }
    perfHint_.start(ndk_, perfHintTargetNs);

    // Keep-alive presents while the host sends nothing (keepAlive()): off unless switched on
    // with adb shell setprop debug.moonlight.keep_alive_ms <ms> (16 was the original default,
    // at the cost of waking the GPU while the picture is still)
    keepAliveNs_ = 0;
    {
        char value[PROP_VALUE_MAX] = {};
        if (__system_property_get("debug.moonlight.keep_alive_ms", value) > 0) {
            keepAliveNs_ = static_cast<int64_t>(std::clamp(atoi(value), 0, 1000)) * 1'000'000;
        }
    }

    if (!createImageReader()) {
        return false;
    }

    // To try a present delay or guard rather than the ones learned (PresentScheduler):
    // adb shell setprop debug.moonlight.present_delay 1 (vsyncs), and
    // adb shell setprop debug.moonlight.present_guard_us 6000. Empty or 0 delay learns as usual.
    {
        char value[PROP_VALUE_MAX] = {};
        int fixedDelay = 0;
        int64_t fixedGuardNs = -1;
        if (__system_property_get("debug.moonlight.present_delay", value) > 0) {
            fixedDelay = atoi(value);
        }
        if (__system_property_get("debug.moonlight.present_guard_us", value) > 0) {
            fixedGuardNs = static_cast<int64_t>(std::clamp(atoi(value), 0, 20000)) * 1000;
        }
        presentScheduler_.fix(fixedDelay, fixedGuardNs);
        if (fixedDelay > 0 || fixedGuardNs >= 0) {
            ALOGI("Present delay %s, guard %s", fixedDelay > 0 ? std::to_string(presentScheduler_.delayVsyncs()).c_str() : "learned",
                  fixedGuardNs >= 0 ? (std::to_string(fixedGuardNs / 1000) + " us").c_str() : "learned");
        }
    }

    // An aligned front buffer draws frames presented ahead at their scan, which needs no
    // display timing
    presentAhead_ = (hasDisplayTimingExt_ || (frontBufferAligned_ && hasSharedPresentExt_)) &&
            pacer_.mode() == PacingMode::HostTimed;
    instant_ = pacer_.mode() == PacingMode::HostTimed && pacer_.timeline().jitterBuffer() == JitterBuffer::Instant;
    pacer_.setPresentAhead(presentAhead_);
    pacer_.setPresentGuardNs(presentScheduler_.guardNs());

    char configuration[512];
    snprintf(configuration, sizeof(configuration),
             "mode=%d,jitterBuffer=%d,presentAhead=%d,streamFps=%d,width=%d,height=%d,periodNs=%lld,refreshHz=%.3f,tenBit=%d,dither=%d,"
             "perfHintUs=%lld,gpuPriority=%s,keepAliveMs=%lld,codec=%s,decoder=%s",
             config_.framePacing, config_.jitterBuffer, presentAhead_ ? 1 : 0, config_.streamFps, config_.streamWidth, config_.streamHeight,
             static_cast<long long>(pacer_.vsyncPeriodNs()), config_.displayRefreshHz, config_.tenBit ? 1 : 0,
             config_.ditherMode, static_cast<long long>(perfHintTargetNs / 1000), highPriorityQueue_ ? "high" : "default",
             static_cast<long long>(keepAliveNs_ / 1'000'000), config_.codec.c_str(), config_.decoderName.c_str());
    trace_.start(config_.traceDirectory, configuration);
    traceRecording_ = trace_.recording();

    if (trace_.recording() && timestampValidBits_ > 0 && timestampPeriodNs_ > 0 && vk_.vkCreateQueryPool &&
            vk_.vkDestroyQueryPool && vk_.vkGetQueryPoolResults && vk_.vkCmdResetQueryPool && vk_.vkCmdWriteTimestamp) {
        VkQueryPoolCreateInfo queryInfo {VK_STRUCTURE_TYPE_QUERY_POOL_CREATE_INFO};
        queryInfo.queryType = VK_QUERY_TYPE_TIMESTAMP;
        queryInfo.queryCount = kTimestampsPerFrame * kFramesInFlight;
        gpuTiming_ = vk_.vkCreateQueryPool(device_, &queryInfo, nullptr, &timestampPool_) == VK_SUCCESS;
    }

    wakeFd_ = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    if (wakeFd_ < 0) {
        ALOGE("eventfd failed");
        return false;
    }

    renderThread_ = std::thread(&VulkanRenderer::renderThreadMain, this);
    return true;
}

bool VulkanRenderer::createInstance() {
    instance_ = createVkInstance(vk_, &hasColorspaceExt_, instanceSetup_, frontBufferRequested_);
    if (instance_ == VK_NULL_HANDLE || !vk_.loadInstance(instance_)) {
        return false;
    }

    VkAndroidSurfaceCreateInfoKHR surfaceInfo {VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR};
    surfaceInfo.window = outputWindow_;
    VkResult result = vk_.vkCreateAndroidSurfaceKHR(instance_, &surfaceInfo, nullptr, &surface_);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateAndroidSurfaceKHR failed: %d", result);
        return false;
    }
    return true;
}

bool VulkanRenderer::pickDevice() {
    uint32_t count = 0;
    vk_.vkEnumeratePhysicalDevices(instance_, &count, nullptr);
    std::vector<VkPhysicalDevice> devices(count);
    vk_.vkEnumeratePhysicalDevices(instance_, &count, devices.data());

    for (VkPhysicalDevice device : devices) {
        if (!deviceIsSuitable(vk_, device)) {
            continue;
        }
        uint32_t familyCount = 0;
        vk_.vkGetPhysicalDeviceQueueFamilyProperties(device, &familyCount, nullptr);
        std::vector<VkQueueFamilyProperties> families(familyCount);
        vk_.vkGetPhysicalDeviceQueueFamilyProperties(device, &familyCount, families.data());

        for (uint32_t i = 0; i < familyCount; i++) {
            VkBool32 presentSupported = VK_FALSE;
            vk_.vkGetPhysicalDeviceSurfaceSupportKHR(device, i, surface_, &presentSupported);
            if ((families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) && presentSupported) {
                physicalDevice_ = device;
                queueFamily_ = i;

                uint32_t extCount = 0;
                vk_.vkEnumerateDeviceExtensionProperties(device, nullptr, &extCount, nullptr);
                std::vector<VkExtensionProperties> extensions(extCount);
                vk_.vkEnumerateDeviceExtensionProperties(device, nullptr, &extCount, extensions.data());
                hasHdrMetadataExt_ = hasExtension(extensions, VK_EXT_HDR_METADATA_EXTENSION_NAME);
                hasDisplayTimingExt_ = hasExtension(extensions, VK_GOOGLE_DISPLAY_TIMING_EXTENSION_NAME);
                hasSharedPresentExt_ = frontBufferRequested_ &&
                        hasExtension(extensions, VK_KHR_SHARED_PRESENTABLE_IMAGE_EXTENSION_NAME) &&
                        std::any_of(instanceSetup_.extensions.begin(), instanceSetup_.extensions.end(), [](const char* name) {
                            return strcmp(name, VK_KHR_GET_SURFACE_CAPABILITIES_2_EXTENSION_NAME) == 0;
                        });
                // On unless adb shell setprop debug.nightjar.vk_sync_fd 0 (A/B runs): the GPU waits
                // on the decoder's sync fd, which measured better on our devices than the CPU wait.
                char syncFdWait[PROP_VALUE_MAX] = {};
                __system_property_get("debug.nightjar.vk_sync_fd", syncFdWait);
                if (strcmp(syncFdWait, "0") != 0) {
                    hasSyncFdImport_ = hasExtension(extensions, VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME) &&
                            syncFdImportable(device);
                    if (!hasSyncFdImport_) {
                        ALOGW("GPU wait for the decoder unavailable: the device can't import sync fds; "
                              "the image reader thread waits for the decoder");
                    }
                }
                globalPriorityExt_ = hasExtension(extensions, VK_KHR_GLOBAL_PRIORITY_EXTENSION_NAME)
                        ? VK_KHR_GLOBAL_PRIORITY_EXTENSION_NAME
                        : hasExtension(extensions, VK_EXT_GLOBAL_PRIORITY_EXTENSION_NAME)
                        ? VK_EXT_GLOBAL_PRIORITY_EXTENSION_NAME : nullptr;

                VkPhysicalDeviceProperties props;
                vk_.vkGetPhysicalDeviceProperties(device, &props);
                timestampValidBits_ = families[i].timestampValidBits;
                timestampPeriodNs_ = props.limits.timestampPeriod;
                vendorId_ = props.vendorID;
                ALOGI("Using %s (Vulkan %u.%u.%u), queue family %u", props.deviceName,
                      VK_API_VERSION_MAJOR(props.apiVersion), VK_API_VERSION_MINOR(props.apiVersion),
                      VK_API_VERSION_PATCH(props.apiVersion), i);
                return true;
            }
        }
    }

    ALOGE("No suitable Vulkan device");
    return false;
}

bool VulkanRenderer::createDevice() {
    prepareDevice(queueFamily_, hasHdrMetadataExt_, hasDisplayTimingExt_, deviceSetup_);
    // Before the global priority extension, which is taken off the end if refused
    if (hasSharedPresentExt_) {
        deviceSetup_.extensions.push_back(VK_KHR_SHARED_PRESENTABLE_IMAGE_EXTENSION_NAME);
        deviceSetup_.info.enabledExtensionCount = static_cast<uint32_t>(deviceSetup_.extensions.size());
        deviceSetup_.info.ppEnabledExtensionNames = deviceSetup_.extensions.data();
    }
    else if (frontBufferRequested_) {
        ALOGW("Front buffer asked for, but the device has no shared presentable images");
    }
    if (hasSyncFdImport_) {
        deviceSetup_.extensions.push_back(VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME);
        deviceSetup_.info.enabledExtensionCount = static_cast<uint32_t>(deviceSetup_.extensions.size());
        deviceSetup_.info.ppEnabledExtensionNames = deviceSetup_.extensions.data();
    }

    // Our queues at high global priority, so the GPU runs a render (one small draw, due at a
    // vsync) ahead of other apps' work, and can preempt it. Android only lets some apps have
    // it, and a driver refuses the device when it won't, so it's tried and dropped if refused.
    // To switch it off: adb shell setprop debug.moonlight.gpu_priority default
    char priorityMode[PROP_VALUE_MAX] = {};
    __system_property_get("debug.moonlight.gpu_priority", priorityMode);
    const bool tryHighPriority = globalPriorityExt_ && strcmp(priorityMode, "default") != 0;
    if (tryHighPriority) {
        deviceSetup_.globalPriority.globalPriority = VK_QUEUE_GLOBAL_PRIORITY_HIGH_KHR;
        deviceSetup_.queue.pNext = &deviceSetup_.globalPriority;
        deviceSetup_.extensions.push_back(globalPriorityExt_);
        deviceSetup_.info.enabledExtensionCount = static_cast<uint32_t>(deviceSetup_.extensions.size());
        deviceSetup_.info.ppEnabledExtensionNames = deviceSetup_.extensions.data();
    }

    VkDevice device = VK_NULL_HANDLE;
    VkResult result = vk_.vkCreateDevice(physicalDevice_, &deviceSetup_.info, nullptr, &device);
    highPriorityQueue_ = tryHighPriority && result == VK_SUCCESS;
    if (tryHighPriority && result != VK_SUCCESS) {
        ALOGI("High priority GPU queue refused (%d), using the default", result);
        deviceSetup_.queue.pNext = nullptr;
        deviceSetup_.extensions.pop_back();
        deviceSetup_.info.enabledExtensionCount = static_cast<uint32_t>(deviceSetup_.extensions.size());
        deviceSetup_.info.ppEnabledExtensionNames = deviceSetup_.extensions.data();
        result = vk_.vkCreateDevice(physicalDevice_, &deviceSetup_.info, nullptr, &device);
    }
    else if (highPriorityQueue_) {
        ALOGI("High priority GPU queue (%s)", globalPriorityExt_);
    }
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateDevice failed: %d", result);
        return false;
    }
    if (!vk_.loadDevice(device)) {
        // Teardown can't rely on the device functions, so destroy it through the instance
        auto destroyDevice = reinterpret_cast<PFN_vkDestroyDevice>(vk_.vkGetInstanceProcAddr(instance_, "vkDestroyDevice"));
        if (destroyDevice) {
            destroyDevice(device, nullptr);
        }
        return false;
    }
    device_ = device;
    if (!vk_.vkSetHdrMetadataEXT) {
        hasHdrMetadataExt_ = false;
    }
    if (!vk_.vkGetPastPresentationTimingGOOGLE) {
        hasDisplayTimingExt_ = false;
    }
    gpuDecodeWait_ = hasSyncFdImport_ && vk_.vkImportSemaphoreFdKHR;
    if (hasSyncFdImport_) {
        ALOGI("Decoder fence waited for by the %s", gpuDecodeWait_ ? "GPU when presenting on arrival" :
              "image reader thread: vkImportSemaphoreFdKHR is missing");
    }

    vk_.vkGetDeviceQueue(device_, queueFamily_, 0, &queue_);
    return true;
}

bool VulkanRenderer::createFrameResources() {
    VkCommandPoolCreateInfo poolInfo {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    poolInfo.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    poolInfo.queueFamilyIndex = queueFamily_;
    if (vk_.vkCreateCommandPool(device_, &poolInfo, nullptr, &commandPool_) != VK_SUCCESS) {
        return false;
    }

    VkCommandBufferAllocateInfo allocInfo {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    allocInfo.commandPool = commandPool_;
    allocInfo.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    allocInfo.commandBufferCount = kFramesInFlight;
    if (vk_.vkAllocateCommandBuffers(device_, &allocInfo, commandBuffers_) != VK_SUCCESS) {
        return false;
    }

    for (int i = 0; i < kFramesInFlight; i++) {
        VkFenceCreateInfo fenceInfo {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
        fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
        VkSemaphoreCreateInfo semInfo {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
        if (vk_.vkCreateFence(device_, &fenceInfo, nullptr, &fences_[i]) != VK_SUCCESS ||
                vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &imageAcquired_[i]) != VK_SUCCESS ||
                (gpuDecodeWait_ && vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &decodeDone_[i]) != VK_SUCCESS)) {
            return false;
        }
    }
    return true;
}

bool VulkanRenderer::syncFdImportable(VkPhysicalDevice device) const {
    auto properties = reinterpret_cast<PFN_vkGetPhysicalDeviceExternalSemaphoreProperties>(
            vk_.vkGetInstanceProcAddr(instance_, "vkGetPhysicalDeviceExternalSemaphoreProperties"));
    if (!properties) {
        return false;
    }
    VkPhysicalDeviceExternalSemaphoreInfo info {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_SEMAPHORE_INFO};
    info.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT;
    VkExternalSemaphoreProperties result {VK_STRUCTURE_TYPE_EXTERNAL_SEMAPHORE_PROPERTIES};
    properties(device, &info, &result);
    return (result.externalSemaphoreFeatures & VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT) != 0;
}

bool VulkanRenderer::waitForDecoder(VideoFrame& frame, int slot, bool* gpuWait) {
    const bool gpu = gpuDecodeWait_ && decodeDone_[slot] != VK_NULL_HANDLE;
    VkResult refused = VK_SUCCESS;
    const vkr::DecoderWait wait = vkr::waitForDecoder(frame.decodeFence, gpu,
            static_cast<int>(kFenceTimeoutNs / 1'000'000), [&](int fd) {
        // Temporary, as sync fds must be: the submit's wait consumes it and the semaphore is
        // empty again for the slot's next frame. On success Vulkan owns the fd.
        VkImportSemaphoreFdInfoKHR import {VK_STRUCTURE_TYPE_IMPORT_SEMAPHORE_FD_INFO_KHR};
        import.semaphore = decodeDone_[slot];
        import.flags = VK_SEMAPHORE_IMPORT_TEMPORARY_BIT;
        import.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT;
        import.fd = fd;
        refused = vk_.vkImportSemaphoreFdKHR(device_, &import);
        return refused == VK_SUCCESS;
    });
    if (refused != VK_SUCCESS) {
        // Once refused, the image reader thread waits for the decoder again, as without the GPU wait
        ALOGE("Could not import the decoder fence (%d); the image reader thread waits for the decoder from now on",
              refused);
        gpuDecodeWait_ = false;
    }
    *gpuWait = wait == vkr::DecoderWait::GPU;
    if (wait == vkr::DecoderWait::TIMED_OUT) {
        ALOGE("Timed out waiting for the decoder to write a frame");
        return false;
    }
    return true;
}

bool VulkanRenderer::createShaderModules() {
    VkShaderModuleCreateInfo info {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    info.codeSize = sizeof(kVideoVertSpv);
    info.pCode = kVideoVertSpv;
    if (vk_.vkCreateShaderModule(device_, &info, nullptr, &vertShader_) != VK_SUCCESS) {
        return false;
    }
    info.codeSize = sizeof(kVideoFragSpv);
    info.pCode = kVideoFragSpv;
    if (vk_.vkCreateShaderModule(device_, &info, nullptr, &fragShader_) != VK_SUCCESS) {
        return false;
    }
    return true;
}

VideoFrame::~VideoFrame() {
    if (reporter && !presentQueued && timing.hostPtsNs != 0) {
        reporter->reportDropped(timing.hostPtsNs);
    }
    if (image) ndk->AImage_delete(image);
    // decodeFence closes its fd itself
}

void VulkanRenderer::reportPresent(int64_t hostPtsNs, int64_t queuedNs, int64_t presentNs) {
    if (!hasDisplayTimingExt_ || hostPtsNs == 0) {
        return;
    }
    std::lock_guard<std::mutex> lock(latencyMutex_);
    presentReports_.push_back({hostPtsNs, queuedNs, presentNs});
    while (presentReports_.size() > kMaxPresentReports) {
        presentReports_.pop_front();
    }
}

void VulkanRenderer::reportDropped(int64_t hostPtsNs) {
    reportPresent(hostPtsNs, 0, 0);
}


}  // namespace vkr
