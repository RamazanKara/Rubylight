#include "vulkan_renderer.h"

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cinttypes>
#include <cmath>
#include <condition_variable>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <memory>
#include <thread>
#include <poll.h>
#include <sys/eventfd.h>
#include <sys/resource.h>
#include <sys/timerfd.h>
#include <unistd.h>
#include <android/log.h>
#include <android/looper.h>

#include "shaders_spv.h"

#include <sys/system_properties.h>

#include <Limelight.h>

#define LOG_TAG "VulkanRenderer"
#define ALOGE(...) recordError(ANDROID_LOG_ERROR, __VA_ARGS__)
#define ALOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define ALOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
// Logged, and kept as the reason the renderer isn't available (VulkanRenderer::lastError())
#define ALOGR(...) recordError(ANDROID_LOG_INFO, __VA_ARGS__)

namespace vkr {

namespace {
    // How long the destructor waits for a GPU that stop() found unfinished before leaking it
    constexpr int kWedgedWaitSeconds = 5;

    std::mutex gLastErrorMutex;
    std::string gLastError;

    __attribute__((format(printf, 2, 3)))
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

    // Keep in sync with the push constant block in the shaders
    struct PushConstants {
        float uvRect[4];
        float uvClamp[4];
        float params[4];
        float params2[4];
        float ycbcr[4];  // Unused: the shaders keep the layout from when PyroWave shared them
    };

    constexpr float kOutputPassthrough = 0.0f;
    constexpr float kOutputPqToSdr = 1.0f;
    constexpr float kSdrWhiteNits = 203.0f;

    // Quarter turns clockwise for a pre-rotation, or -1 for a transform we can't render
    int quarterTurns(VkSurfaceTransformFlagBitsKHR transform) {
        switch (transform) {
            case VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR: return 0;
            case VK_SURFACE_TRANSFORM_ROTATE_90_BIT_KHR: return 1;
            case VK_SURFACE_TRANSFORM_ROTATE_180_BIT_KHR: return 2;
            case VK_SURFACE_TRANSFORM_ROTATE_270_BIT_KHR: return 3;
            default: return -1;
        }
    }

    // Rendering the display's rotation ourselves keeps the layer untransformed. Display
    // hardware often can't rotate RGB layers, and then the compositor falls back to drawing
    // the layer on the GPU, which for HDR means its own tone mapping into an 8-bit target.
    VkSurfaceTransformFlagBitsKHR choosePreTransform(const VkSurfaceCapabilitiesKHR& caps) {
        if (quarterTurns(caps.currentTransform) >= 0 && (caps.supportedTransforms & caps.currentTransform)) {
            return caps.currentTransform;
        }
        if (caps.supportedTransforms & VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR) {
            return VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
        }
        return caps.currentTransform;
    }

    // The window's size in the display's natural orientation, as a pre-rotated swapchain needs
    VkExtent2D preRotatedExtent(VkExtent2D extent, VkSurfaceTransformFlagBitsKHR transform) {
        if (quarterTurns(transform) % 2 == 1) {
            std::swap(extent.width, extent.height);
        }
        return extent;
    }

    constexpr uint32_t kWakeFrame = 1;
    constexpr uint32_t kWakeHdr = 2;
    constexpr uint32_t kWakeQuit = 4;

    constexpr int kLooperIdWake = 1;
    constexpr int kLooperIdAlignedTimer = 2;

    // Window for the overlay's count of skipped frames
    constexpr int64_t kRecentSkipsNs = 10'000'000'000;

    // Imported buffers are kept while the decoder keeps cycling through the same ones
    constexpr size_t kMaxImports = 24;

    // Images we may hold at once: the waiting queue, the ones the GPU is reading, the one on
    // screen, and one being acquired
    constexpr int32_t kMaxReaderImages = static_cast<int32_t>(FramePacer::kMaxQueuedFrames) + 4;

    constexpr uint64_t kFenceTimeoutNs = 1'000'000'000;
    constexpr uint64_t kAcquireTimeoutNs = 100'000'000;
    // Once the renderer is stopping, waits for the GPU or a swapchain image are cut to this so
    // stop() returns promptly, and a vkDeviceWaitIdle is bounded the same way
    constexpr uint64_t kStopWaitNs = 50'000'000;
    // How long stop() waits for the GPU's work in flight before giving up on cleaning up
    constexpr uint64_t kStopDrainNs = 250'000'000;
    // Consecutive failed renders after which the renderer gives up, so the stream ends with an
    // error rather than sitting on a frozen picture
    constexpr int kFatalFailures = 10;
    // Present reports Java hasn't collected yet; at 1 s between collections, well over a second of 240 fps
    constexpr size_t kMaxPresentReports = 512;
    constexpr int kFatalFenceTimeouts = 3;

    void raiseThreadPriority(const char* name) {
        // ANDROID_PRIORITY_URGENT_DISPLAY, then ANDROID_PRIORITY_DISPLAY
        const pid_t tid = gettid();
        if (setpriority(PRIO_PROCESS, tid, -8) != 0 && setpriority(PRIO_PROCESS, tid, -4) != 0) {
            ALOGW("Could not raise the %s thread's priority", name);
            return;
        }
        ALOGI("%s thread priority %d", name, getpriority(PRIO_PROCESS, tid));
    }

    int64_t nowNs() {
        timespec ts {};
        clock_gettime(CLOCK_MONOTONIC, &ts);
        return static_cast<int64_t>(ts.tv_sec) * 1'000'000'000LL + ts.tv_nsec;
    }

    bool hasExtension(const std::vector<VkExtensionProperties>& extensions, const char* name) {
        for (const auto& ext : extensions) {
            if (strcmp(ext.extensionName, name) == 0) {
                return true;
            }
        }
        return false;
    }

    bool isYcbcrFormat(VkFormat format) {
        return (format >= VK_FORMAT_G8B8G8R8_422_UNORM && format <= VK_FORMAT_G16_B16_R16_3PLANE_444_UNORM) ||
               (format >= VK_FORMAT_G8_B8R8_2PLANE_444_UNORM && format <= VK_FORMAT_G16_B16R16_2PLANE_444_UNORM);
    }

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
    return suitable;
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

    if (!vk_.loadGlobal() || !createInstance() || !pickDevice() || !createDevice() ||
            !createFrameResources() || !createShaderModules()) {
        return false;
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
                globalPriorityExt_ = hasExtension(extensions, VK_KHR_GLOBAL_PRIORITY_EXTENSION_NAME)
                        ? VK_KHR_GLOBAL_PRIORITY_EXTENSION_NAME
                        : hasExtension(extensions, VK_EXT_GLOBAL_PRIORITY_EXTENSION_NAME)
                        ? VK_EXT_GLOBAL_PRIORITY_EXTENSION_NAME : nullptr;

                VkPhysicalDeviceProperties props;
                vk_.vkGetPhysicalDeviceProperties(device, &props);
                timestampValidBits_ = families[i].timestampValidBits;
                timestampPeriodNs_ = props.limits.timestampPeriod;
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
                vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &imageAcquired_[i]) != VK_SUCCESS) {
            return false;
        }
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
    VkFenceCreateInfo fenceInfo {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
    VkSemaphoreCreateInfo semInfo {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    if (vk_.vkCreateFence(device_, &fenceInfo, nullptr, &fences_[slot]) != VK_SUCCESS ||
            vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &imageAcquired_[slot]) != VK_SUCCESS) {
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
        closing_ = true;
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
        for (int i = 0; i < kFramesInFlight; i++) {
            if (fences_[i]) vk_.vkDestroyFence(device_, fences_[i], nullptr);
            if (imageAcquired_[i]) vk_.vkDestroySemaphore(device_, imageAcquired_[i], nullptr);
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
            parsed.vk.maxContentLightLevel = v[10];
            parsed.vk.maxFrameAverageLightLevel = v[11];

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

std::string VulkanRenderer::rendererText() {
    std::lock_guard<std::mutex> lock(mutex_);

    return "Vulkan " + outputDescription_;
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
    if (imageReaderTid_.exchange(gettid()) != gettid()) {
        perfHint_.addThread();
    }
    for (;;) {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            if (closing_) {
                return;
            }
        }

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
            return;
        }
        if (fenceFd >= 0) {
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
        frame->timing.arrivalNs = nowNs();
        if (ndk_->AImage_getHardwareBuffer(image, &frame->buffer) != AMEDIA_OK || !frame->buffer) {
            ALOGE("Image has no hardware buffer");
            continue;
        }
        // MediaCodec stamps each image with the frame's presentation time, which is the host's
        // timestamp for the frame
        ndk_->AImage_getTimestamp(image, &frame->timing.hostPtsNs);
        frame->reporter = this;
        if (ndk_->AImage_getCropRect(image, &frame->crop) != AMEDIA_OK) {
            frame->crop = {0, 0, 0, 0};
        }

        enqueueFrame(std::move(frame));
    }
}

void VulkanRenderer::enqueueFrame(FramePtr frame) {
    noteDecoded(*frame);

    std::deque<FramePtr> dropped;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (closing_) {
            return;
        }
        pacer_.onFrameArrived(frame->timing);
        trace_.frame(frame->timing, pending_.size() + 1, queueOverflowDrops_);
        pending_.push_back(std::move(frame));
        while (pending_.size() > pacer_.maxQueued()) {
            dropped.push_back(std::move(pending_.front()));
            pending_.pop_front();
            queueOverflowDrops_++;
        }
    }

    if (presentAhead_ && !presentOnArrival_) {
        // Presented from this thread rather than waking the render thread for it: on a
        // Pixel 10 Pro, the render thread took 1 ms to wake at the median and over 3 ms one
        // time in ten, time the compositor then didn't have
        std::lock_guard<std::mutex> renderLock(renderMutex_);
        if (!renderStopped_) {
            presentAhead();
        }
    }
    else if (presentOnArrival_) {
        wake(kWakeFrame);
    }
    // Frames in `dropped` are released here, outside the lock
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
    std::vector<int64_t> sorted(drawSubmitNs_.begin(), drawSubmitNs_.end());
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
            std::vector<int64_t> sorted(vsyncDeltas_.begin(), vsyncDeltas_.end());
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
    if (!frame) {
        keepAlive();
    }
    collectPresentTimings();

    ndk_->AChoreographer_postFrameCallback64(choreographer_, &VulkanRenderer::onVsyncThunk, this);
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

void VulkanRenderer::presentAhead() {
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
                auto previous = pendingPresents_.find(lastAheadPresentId_);
                if (previous != pendingPresents_.end()) {
                    previous->second.replaced = true;
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
    result = acquireImage();
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
    // Stay half a texel inside the crop so filtering never pulls in padding
    pc.uvClamp[0] = (crop.left + 0.5f) / bufferWidth;
    pc.uvClamp[1] = (crop.top + 0.5f) / bufferHeight;
    pc.uvClamp[2] = (crop.right - 0.5f) / bufferWidth;
    pc.uvClamp[3] = (crop.bottom - 0.5f) / bufferHeight;

    // Low dithers at the output's own precision. High dithers at 8-bit precision even on a
    // 10-bit output, which also hides banding from an 8-bit panel or compositor further on.
    float ditherAmplitude = 0.0f;
    if (config_.ditherMode == 1) {
        ditherAmplitude = 1.0f / static_cast<float>((1 << outputBits_) - 1);
    }
    else if (config_.ditherMode == 2) {
        ditherAmplitude = 1.0f / 255.0f;
    }
    float contentPeakNits;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        contentPeakNits = hdrMetadata_.contentPeakNits;
    }
    pc.params[0] = ditherAmplitude;
    pc.params[1] = static_cast<float>(frameCounter_ % 64);
    pc.params[2] = (hdrActive_ && !outputPq_) ? kOutputPqToSdr : kOutputPassthrough;
    pc.params[3] = contentPeakNits;
    pc.params2[0] = kSdrWhiteNits;
    pc.params2[1] = static_cast<float>(std::max(quarterTurns(preTransform_), 0));

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

    VkSubmitInfo submit {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    submit.waitSemaphoreCount = 1;
    submit.pWaitSemaphores = &imageAcquired_[slot];
    submit.pWaitDstStageMask = &waitStage;
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
        const int64_t vsyncNs = showVsyncNs != 0 ? showVsyncNs : currentVsyncNs_;
        if (!onArrival) {
            presentTime.desiredPresentTime =
                    static_cast<uint64_t>(presentScheduler_.desiredPresentNs(vsyncNs, currentPeriodNs_));
        }
        else if (!presentOnArrival_) {
            presentTime.desiredPresentTime = static_cast<uint64_t>(nowNs());
        }
        presentTimes.swapchainCount = 1;
        presentTimes.pTimes = &presentTime;
        present.pNext = &presentTimes;

        // Slack from the commit, not from now: the pacer holds frames to the guard when it
        // commits them, and on a Pixel 10 Pro rendering sometimes took milliseconds after that
        // (waiting for a swapchain image). Measured from here, the guard learned was a third of
        // what commits needed.
        PendingPresent& pending = pendingPresents_[presentId];
        pending = {vsyncNs, presentScheduler_.delayVsyncs(), ahead,
                   ahead ? vsyncNs - (commitNs != 0 ? commitNs : nowNs()) : 0};
        pending.onArrival = onArrival;
        taggedPresentId = presentId;
        if (!frame->presentCounted) {
            countedAtTag = true;
            // Counted when the present is reported, at the time it reached the screen
            pending.originNs = frame->originNs;
            pending.originSynced = frame->originSynced;
            pending.hostPtsNs = frame->timing.hostPtsNs;
            frame->presentCounted = true;
        }
        if (pendingPresents_.size() > 256) {
            // Reports lost across a swapchain rebuild never arrive
            while (!pendingPresents_.empty() && pendingPresents_.begin()->first + 128 < presentId) {
                reportPresent(pendingPresents_.begin()->second.hostPtsNs, 0, 0);
                pendingPresents_.erase(pendingPresents_.begin());
            }
        }
    }
    int64_t presentCallNs = 0;
    {
        std::lock_guard<std::mutex> lock(queueMutex_);
        presentCallNs = nowNs();
        result = vk_.vkQueuePresentKHR(queue_, &present);
    }
    // The render's CPU time for the performance hint session, without waiting for the GPU or a
    // swapchain image
    perfHint_.reportWork(nowNs() - startNs - (acquiredNs - releasedNs) - (fenceWaited ? fencedNs - fenceWaitNs : 0));

    const bool shown = result == VK_SUCCESS || result == VK_SUBOPTIMAL_KHR;
    if (!shown && countedAtTag) {
        // Nothing was queued, so no report will come for the tag: the frame is counted when a
        // later present of it is
        PendingPresent& failed = pendingPresents_[taggedPresentId];
        failed.originNs = 0;
        failed.hostPtsNs = 0;
        frame->presentCounted = false;
    }
    if (shown && countedAtTag) {
        pendingPresents_[taggedPresentId].queuedNs = presentCallNs;
    }
    if (shown && !frame->presentQueued) {
        frame->presentQueued = true;
        if (!frame->presentCounted) {
            // Not tagged for a report of when it reached the screen (a front buffer, or a period
            // not yet known): until the present was queued. A frame drawn again (across a
            // swapchain rebuild) is only counted the first time.
            frame->presentCounted = true;
            const int64_t queuedNs = nowNs();
            if (frame->originNs != 0) {
                std::lock_guard<std::mutex> lock(latencyMutex_);
                countLatency(captureLatency_.presented, queuedNs - frame->originNs, frame->originSynced);
            }
            reportPresent(frame->timing.hostPtsNs, presentCallNs, kNoScanout);
        }
    }
    if (presentId != 0) {
        const int64_t queuedNs = nowNs();
        std::lock_guard<std::mutex> lock(mutex_);
        trace_.rendered(presentId, startNs, fencedNs, acquiredNs, queuedNs, imageIndex, fenceWaited, releasedNs);
    }

    slotFrames_[slot] = frame;
    lastPresentNs_ = nowNs();
    if (frame != current_) {
        lastNewFrameNs_ = lastPresentNs_;
    }
    current_ = frame;
    imported->lastUsed = frameCounter_;
    frameCounter_++;

    if (result == VK_ERROR_OUT_OF_DATE_KHR) {
        swapchainDirty_ = true;
    }
    else if (result == VK_SUBOPTIMAL_KHR) {
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
    else if (result != VK_SUCCESS) {
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
    std::vector<VkPastPresentationTimingGOOGLE> timings(count);
    if (vk_.vkGetPastPresentationTimingGOOGLE(device_, swapchain_, &count, timings.data()) < 0) {
        return;
    }
    for (uint32_t i = 0; i < count; i++) {
        const VkPastPresentationTimingGOOGLE& t = timings[i];

        // Reports come in present order, so earlier presents still without one never reached
        // the screen: the compositor dropped them
        for (auto it = pendingPresents_.begin(); it != pendingPresents_.end() && it->first < t.presentID;) {
            if (it->second.ahead && !it->second.replaced) {
                presentScheduler_.onAheadOutcome(it->second.slackNs, it->second.vsyncNs, it->second.delayVsyncs,
                                                 false, 0, 0, 0, currentPeriodNs_);
            }
            reportPresent(it->second.hostPtsNs, 0, 0);
            it = pendingPresents_.erase(it);
        }

        auto pending = pendingPresents_.find(t.presentID);
        if (pending != pendingPresents_.end() && pending->second.ahead && !pending->second.replaced) {
            presentScheduler_.onAheadOutcome(pending->second.slackNs, pending->second.vsyncNs,
                                             pending->second.delayVsyncs, true,
                                             static_cast<int64_t>(t.actualPresentTime),
                                             static_cast<int64_t>(t.earliestPresentTime),
                                             static_cast<int64_t>(t.presentMargin), currentPeriodNs_);
        }

        // With frames presented ahead, the delay is tuned for those. A frame that arrived late,
        // and so was presented at its vsync or with less time than the guard, may miss; that
        // frame is late on screen whatever the delay, and it shouldn't raise the delay for all
        // the others.
        const bool counts = pending != pendingPresents_.end() && !pending->second.onArrival &&
                (!presentAhead_ || (pending->second.ahead && pending->second.slackNs >= presentScheduler_.guardNs()));
        if (counts) {
            presentScheduler_.onPresented(pending->second.vsyncNs, pending->second.delayVsyncs, currentPeriodNs_,
                                          static_cast<int64_t>(t.actualPresentTime),
                                          static_cast<int64_t>(t.earliestPresentTime),
                                          static_cast<int64_t>(t.presentMargin),
                                          pending->second.ahead ? pending->second.slackNs : -1);
        }
        if (pending != pendingPresents_.end()) {
            // A frame replaced at its vsync never reached the screen
            if (!pending->second.replaced) {
                if (pending->second.originNs != 0) {
                    std::lock_guard<std::mutex> lock(latencyMutex_);
                    countLatency(captureLatency_.presented,
                                 static_cast<int64_t>(t.actualPresentTime) - pending->second.originNs,
                                 pending->second.originSynced);
                }
                reportPresent(pending->second.hostPtsNs, pending->second.queuedNs,
                              static_cast<int64_t>(t.actualPresentTime));
            }
            else {
                reportPresent(pending->second.hostPtsNs, 0, 0);
            }
            pendingPresents_.erase(pending);
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

void VulkanRenderer::noteReceived(int64_t hostPtsNs, int64_t receiveNs, int64_t enqueueNs, int64_t hostProcessingNs) {
    // Called as the frame goes to the decoder, so the time on our clock is when decoding starts
    const int64_t queuedNs = nowNs();
    const int64_t commonToLocalNs = queuedNs - static_cast<int64_t>(LiGetMicroseconds()) * 1000;

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
            origins_[hostPtsNs] = origin;
            while (origins_.size() > 64) {
                origins_.erase(origins_.begin());
            }
        }
    }

    if (!traceRecording_.load(std::memory_order_relaxed)) {
        return;
    }
    uint32_t rttMs = 0, rttVarianceMs = 0;
    if (!LiGetEstimatedRttInfo(&rttMs, &rttVarianceMs)) {
        rttMs = 0;
    }
    std::lock_guard<std::mutex> lock(mutex_);
    trace_.received(hostPtsNs, receiveNs, enqueueNs, queuedNs, commonToLocalNs, hostProcessingNs, rttMs);
}

void VulkanRenderer::noteDecoded(VideoFrame& frame) {
    std::lock_guard<std::mutex> lock(latencyMutex_);
    auto it = origins_.find(frame.timing.hostPtsNs);
    if (it == origins_.end()) {
        return;
    }
    frame.originNs = it->second.ns;
    frame.originSynced = it->second.synced;

    // Frames are decoded in order, so any still waiting before this one never will be
    origins_.erase(origins_.begin(), std::next(it));
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

bool VulkanRenderer::ensureSwapchain() {
    if (!swapchainDirty_ && swapchain_) {
        return true;
    }
    if (!surface_) {
        return false;
    }
    if (!createSwapchain()) {
        return false;
    }
    swapchainDirty_ = false;
    return true;
}

bool VulkanRenderer::createSwapchain() {
    // Only a window found empty below keeps failures from counting; any other failure does
    const bool wasEmpty = outputWindowEmpty_;
    outputWindowEmpty_ = false;
    VkSurfaceCapabilitiesKHR caps;
    if (vk_.vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice_, surface_, &caps) != VK_SUCCESS) {
        return false;
    }

    VkExtent2D extent = caps.currentExtent;
    if (extent.width == UINT32_MAX) {
        extent.width = static_cast<uint32_t>(ANativeWindow_getWidth(outputWindow_));
        extent.height = static_cast<uint32_t>(ANativeWindow_getHeight(outputWindow_));
    }
    if (extent.width == 0 || extent.height == 0) {
        // Not a failure: the window is being resized or rotated, and has a size again soon
        if (!wasEmpty) {
            ALOGW("The output window has no size; not drawing until it has");
        }
        outputWindowEmpty_ = true;
        return false;
    }
    const VkSurfaceTransformFlagBitsKHR transform = choosePreTransform(caps);
    extent = preRotatedExtent(extent, transform);

    uint32_t formatCount = 0;
    vk_.vkGetPhysicalDeviceSurfaceFormatsKHR(physicalDevice_, surface_, &formatCount, nullptr);
    std::vector<VkSurfaceFormatKHR> formats(formatCount);
    vk_.vkGetPhysicalDeviceSurfaceFormatsKHR(physicalDevice_, surface_, &formatCount, formats.data());
    if (formats.empty()) {
        return false;
    }

    auto findFormat = [&](VkFormat format, VkColorSpaceKHR colorSpace, VkSurfaceFormatKHR* out) {
        for (const auto& f : formats) {
            if (f.format == format && f.colorSpace == colorSpace) {
                *out = f;
                return true;
            }
        }
        return false;
    };

    VkSurfaceFormatKHR chosen {};
    bool found = false;
    bool pq = false;
    if (hdrActive_ && hasColorspaceExt_) {
        // The stream is already PQ-encoded BT.2020, so it passes straight through
        found = pq = findFormat(VK_FORMAT_A2B10G10R10_UNORM_PACK32, VK_COLOR_SPACE_HDR10_ST2084_EXT, &chosen);
        if (!found) {
            ALOGW("Surface has no HDR10 format; tone mapping HDR to SDR");
        }
    }
    if (!found && (config_.tenBit || hdrActive_)) {
        found = findFormat(VK_FORMAT_A2B10G10R10_UNORM_PACK32, VK_COLOR_SPACE_SRGB_NONLINEAR_KHR, &chosen);
    }
    if (!found) {
        found = findFormat(VK_FORMAT_R8G8B8A8_UNORM, VK_COLOR_SPACE_SRGB_NONLINEAR_KHR, &chosen);
    }
    if (!found) {
        chosen = formats[0];
    }

    uint32_t modeCount = 0;
    vk_.vkGetPhysicalDeviceSurfacePresentModesKHR(physicalDevice_, surface_, &modeCount, nullptr);
    std::vector<VkPresentModeKHR> modes(modeCount);
    vk_.vkGetPhysicalDeviceSurfacePresentModesKHR(physicalDevice_, surface_, &modeCount, modes.data());
    VkPresentModeKHR presentMode = VK_PRESENT_MODE_FIFO_KHR;
    const bool shared = hasSharedPresentExt_ &&
            std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_SHARED_CONTINUOUS_REFRESH_KHR) != modes.end();
    if (shared) {
        // The compositor takes the image again at every vsync, so whatever was last drawn into
        // it is shown without presenting a new buffer
        presentMode = VK_PRESENT_MODE_SHARED_CONTINUOUS_REFRESH_KHR;
    }
    else if (pacer_.mode() == PacingMode::MinLatency &&
            std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_MAILBOX_KHR) != modes.end()) {
        presentMode = VK_PRESENT_MODE_MAILBOX_KHR;
    }
    if (hasSharedPresentExt_ && !shared) {
        ALOGW("Front buffer asked for, but the surface has no shared present mode");
    }

    // Frames presented ahead wait in the swapchain until their vsync: up to the jitter buffer
    // plus the present delay's worth of them at once. A shared image is the only one.
    uint32_t imageCount = std::max(caps.minImageCount, presentAhead_ ? 6u : 3u);
    if (caps.maxImageCount != 0) {
        imageCount = std::min(imageCount, caps.maxImageCount);
    }
    if (shared) {
        imageCount = 1;
    }

    VkCompositeAlphaFlagBitsKHR compositeAlpha = VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
    if (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR) {
        compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    }
    else if (!(caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR)) {
        compositeAlpha = static_cast<VkCompositeAlphaFlagBitsKHR>(
                caps.supportedCompositeAlpha & -caps.supportedCompositeAlpha);
    }

    VkSwapchainCreateInfoKHR info {VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
    info.surface = surface_;
    info.minImageCount = imageCount;
    info.imageFormat = chosen.format;
    info.imageColorSpace = chosen.colorSpace;
    info.imageExtent = extent;
    info.imageArrayLayers = 1;
    info.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
    info.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
    info.preTransform = transform;
    info.compositeAlpha = compositeAlpha;
    info.presentMode = presentMode;
    info.clipped = VK_TRUE;
    info.oldSwapchain = swapchain_;

    // Nothing may still use the old swapchain's images when we destroy them
    waitIdle();

    VkSwapchainKHR swapchain = VK_NULL_HANDLE;
    VkResult result = vk_.vkCreateSwapchainKHR(device_, &info, nullptr, &swapchain);
    destroySwapchain();
    if (result != VK_SUCCESS && shared) {
        // The driver offered the mode but refused it: the normal swapchain from now on
        ALOGW("Front buffer swapchain refused (%d); using the normal swapchain", result);
        hasSharedPresentExt_ = false;
        return createSwapchain();
    }
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateSwapchainKHR failed: %d", result);
        return false;
    }
    swapchain_ = swapchain;
    surfaceFormat_ = chosen;
    presentMode_ = presentMode;
    extent_ = extent;
    preTransform_ = transform;
    outputPq_ = pq;
    outputBits_ = chosen.format == VK_FORMAT_A2B10G10R10_UNORM_PACK32 ? 10 : 8;
    frontBuffer_ = shared;
    frontBufferUnavailable_ = frontBufferRequested_ && !shared;
    frontBufferPaced_ = shared && frontBufferAligned_ && presentAhead_;
    presentOnArrival_ = presentMode == VK_PRESENT_MODE_MAILBOX_KHR || (shared && !frontBufferPaced_);
    presentScheduler_.onSwapchainCreated();
    {
        std::lock_guard<std::mutex> lock(mutex_);
        trace_.swapchain(nowNs(), chosen.format, chosen.colorSpace, hdrActive_, extent.width, extent.height,
                         presentMode);
    }

    // A shared image stays in its own layout, in which the display reads it
    const VkImageLayout finalLayout = shared ? VK_IMAGE_LAYOUT_SHARED_PRESENT_KHR : VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    if (renderPass_ && (renderPassFormat_ != chosen.format || renderPassFinalLayout_ != finalLayout)) {
        destroyPipelines();
        vk_.vkDestroyRenderPass(device_, renderPass_, nullptr);
        renderPass_ = VK_NULL_HANDLE;
    }
    if (!renderPass_) {
        VkAttachmentDescription color {};
        color.format = chosen.format;
        color.samples = VK_SAMPLE_COUNT_1_BIT;
        color.loadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;  // Every pixel is drawn
        color.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
        color.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
        color.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
        color.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        color.finalLayout = finalLayout;

        VkAttachmentReference ref {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
        VkSubpassDescription subpass {};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = 1;
        subpass.pColorAttachments = &ref;

        // Wait for the presentation engine to be done with the image before writing it
        VkSubpassDependency dependency {};
        dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
        dependency.dstSubpass = 0;
        dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        // A shared image is acquired with nothing to wait for, so it also waits for the last
        // frame drawn into it
        dependency.srcAccessMask = shared ? VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT : 0;
        dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

        VkRenderPassCreateInfo rpInfo {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
        rpInfo.attachmentCount = 1;
        rpInfo.pAttachments = &color;
        rpInfo.subpassCount = 1;
        rpInfo.pSubpasses = &subpass;
        rpInfo.dependencyCount = 1;
        rpInfo.pDependencies = &dependency;
        if (vk_.vkCreateRenderPass(device_, &rpInfo, nullptr, &renderPass_) != VK_SUCCESS) {
            ALOGE("vkCreateRenderPass failed");
            return false;
        }
        renderPassFormat_ = chosen.format;
        renderPassFinalLayout_ = finalLayout;
    }

    uint32_t count = 0;
    vk_.vkGetSwapchainImagesKHR(device_, swapchain_, &count, nullptr);
    std::vector<VkImage> images(count);
    vk_.vkGetSwapchainImagesKHR(device_, swapchain_, &count, images.data());

    for (VkImage image : images) {
        VkImageViewCreateInfo viewInfo {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
        viewInfo.image = image;
        viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = chosen.format;
        viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        VkImageView view = VK_NULL_HANDLE;
        if (vk_.vkCreateImageView(device_, &viewInfo, nullptr, &view) != VK_SUCCESS) {
            return false;
        }
        swapchainViews_.push_back(view);

        VkFramebufferCreateInfo fbInfo {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
        fbInfo.renderPass = renderPass_;
        fbInfo.attachmentCount = 1;
        fbInfo.pAttachments = &view;
        fbInfo.width = extent.width;
        fbInfo.height = extent.height;
        fbInfo.layers = 1;
        VkFramebuffer framebuffer = VK_NULL_HANDLE;
        if (vk_.vkCreateFramebuffer(device_, &fbInfo, nullptr, &framebuffer) != VK_SUCCESS) {
            return false;
        }
        framebuffers_.push_back(framebuffer);

        VkSemaphoreCreateInfo semInfo {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
        VkSemaphore semaphore = VK_NULL_HANDLE;
        if (vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &semaphore) != VK_SUCCESS) {
            return false;
        }
        renderDone_.push_back(semaphore);
    }

    if (outputPq_) {
        applyHdrMetadata();
    }

    // A change in HDR state also changes the YCbCr model, so drop the old conversion
    {
        std::lock_guard<std::mutex> lock(mutex_);
        char description[128];
        snprintf(description, sizeof(description), "%s%s",
                 outputPq_ ? "HDR10" : (outputBits_ == 10 ? "10-bit SDR" : "8-bit SDR"),
                 (hdrActive_ && !outputPq_) ? " (tone mapped)" : "");
        outputDescription_ = description;
    }

    ALOGI("Swapchain %ux%u, format %d, color space %d, %u images, %s, pre-rotated %d degrees", extent.width,
          extent.height, chosen.format, chosen.colorSpace, count,
          shared ? "front buffer" : presentMode == VK_PRESENT_MODE_MAILBOX_KHR ? "mailbox" : "FIFO",
          quarterTurns(transform) * 90);
    return true;
}

void VulkanRenderer::destroySwapchain() {
    for (VkFramebuffer fb : framebuffers_) {
        vk_.vkDestroyFramebuffer(device_, fb, nullptr);
    }
    for (VkImageView view : swapchainViews_) {
        vk_.vkDestroyImageView(device_, view, nullptr);
    }
    for (VkSemaphore sem : renderDone_) {
        vk_.vkDestroySemaphore(device_, sem, nullptr);
    }
    framebuffers_.clear();
    swapchainViews_.clear();
    renderDone_.clear();
    if (swapchain_) {
        vk_.vkDestroySwapchainKHR(device_, swapchain_, nullptr);
        swapchain_ = VK_NULL_HANDLE;
    }
}

void VulkanRenderer::applyHdrMetadata() {
    if (!hasHdrMetadataExt_ || !swapchain_) {
        ALOGW("Can't pass HDR metadata to the compositor: %s",
              hasHdrMetadataExt_ ? "no swapchain" : "no VK_EXT_hdr_metadata");
        return;
    }
    VkHdrMetadataEXT metadata;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        metadata = hdrMetadata_.vk;
    }
    if (metadata.maxLuminance > 0) {
        vk_.vkSetHdrMetadataEXT(device_, 1, &swapchain_, &metadata);
        ALOGI("Applied HDR metadata: max %.0f nits, MaxCLL %.0f, MaxFALL %.0f", metadata.maxLuminance,
              metadata.maxContentLightLevel, metadata.maxFrameAverageLightLevel);
    }
    else {
        ALOGW("No HDR metadata from the host; the compositor will assume a default peak");
    }
}

bool VulkanRenderer::ensureConversion(const ConversionKey& key) {
    if (haveConversion_ && key == conversionKey_) {
        return true;
    }

    waitIdle();
    destroyConversion();

    if (key.ycbcr) {
        VkExternalFormatANDROID externalFormat {VK_STRUCTURE_TYPE_EXTERNAL_FORMAT_ANDROID};
        externalFormat.externalFormat = key.externalFormat;

        VkSamplerYcbcrConversionCreateInfo info {VK_STRUCTURE_TYPE_SAMPLER_YCBCR_CONVERSION_CREATE_INFO};
        info.pNext = key.externalFormat ? &externalFormat : nullptr;
        info.format = key.format;
        info.ycbcrModel = key.model;
        info.ycbcrRange = key.range;
        info.components = key.components;
        info.xChromaOffset = key.xChromaOffset;
        info.yChromaOffset = key.yChromaOffset;
        info.chromaFilter = key.filter;
        info.forceExplicitReconstruction = VK_FALSE;
        VkResult result = vk_.vkCreateSamplerYcbcrConversion(device_, &info, nullptr, &conversion_);
        if (result != VK_SUCCESS) {
            ALOGE("vkCreateSamplerYcbcrConversion failed: %d", result);
            return false;
        }
    }

    VkSamplerYcbcrConversionInfo conversionInfo {VK_STRUCTURE_TYPE_SAMPLER_YCBCR_CONVERSION_INFO};
    conversionInfo.conversion = conversion_;

    VkSamplerCreateInfo samplerInfo {VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
    samplerInfo.pNext = conversion_ ? &conversionInfo : nullptr;
    samplerInfo.magFilter = key.filter;
    samplerInfo.minFilter = key.filter;
    samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;
    samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.maxLod = 0.0f;
    samplerInfo.borderColor = VK_BORDER_COLOR_FLOAT_OPAQUE_BLACK;
    if (vk_.vkCreateSampler(device_, &samplerInfo, nullptr, &sampler_) != VK_SUCCESS) {
        ALOGE("vkCreateSampler failed");
        return false;
    }

    // A YCbCr conversion has to be baked into the layout as an immutable sampler
    VkDescriptorSetLayoutBinding binding {};
    binding.binding = 0;
    binding.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    binding.descriptorCount = 1;
    binding.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    binding.pImmutableSamplers = &sampler_;
    VkDescriptorSetLayoutCreateInfo layoutInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    layoutInfo.bindingCount = 1;
    layoutInfo.pBindings = &binding;
    if (vk_.vkCreateDescriptorSetLayout(device_, &layoutInfo, nullptr, &setLayout_) != VK_SUCCESS) {
        return false;
    }

    // Multi-planar images can take several descriptors each, so leave room
    VkDescriptorPoolSize poolSize {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, (kMaxImports + 8) * 3};
    VkDescriptorPoolCreateInfo poolInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT;
    poolInfo.maxSets = kMaxImports + 8;
    poolInfo.poolSizeCount = 1;
    poolInfo.pPoolSizes = &poolSize;
    if (vk_.vkCreateDescriptorPool(device_, &poolInfo, nullptr, &descriptorPool_) != VK_SUCCESS) {
        return false;
    }

    VkPushConstantRange range {VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PushConstants)};
    VkPipelineLayoutCreateInfo plInfo {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    plInfo.setLayoutCount = 1;
    plInfo.pSetLayouts = &setLayout_;
    plInfo.pushConstantRangeCount = 1;
    plInfo.pPushConstantRanges = &range;
    if (vk_.vkCreatePipelineLayout(device_, &plInfo, nullptr, &pipelineLayout_) != VK_SUCCESS) {
        return false;
    }

    conversionKey_ = key;
    haveConversion_ = true;
    ALOGI("YCbCr conversion: external format %" PRIu64 ", format %d, model %d, range %d, filter %d",
          key.externalFormat, key.format, key.model, key.range, key.filter);
    return true;
}

void VulkanRenderer::destroyConversion() {
    for (auto& entry : imports_) {
        destroyImport(entry.first, entry.second);
    }
    imports_.clear();

    if (pipeline_) vk_.vkDestroyPipeline(device_, pipeline_, nullptr);
    if (pipelineLayout_) vk_.vkDestroyPipelineLayout(device_, pipelineLayout_, nullptr);
    if (descriptorPool_) vk_.vkDestroyDescriptorPool(device_, descriptorPool_, nullptr);
    if (setLayout_) vk_.vkDestroyDescriptorSetLayout(device_, setLayout_, nullptr);
    if (sampler_) vk_.vkDestroySampler(device_, sampler_, nullptr);
    if (conversion_) vk_.vkDestroySamplerYcbcrConversion(device_, conversion_, nullptr);
    pipeline_ = VK_NULL_HANDLE;
    pipelineLayout_ = VK_NULL_HANDLE;
    descriptorPool_ = VK_NULL_HANDLE;
    setLayout_ = VK_NULL_HANDLE;
    sampler_ = VK_NULL_HANDLE;
    conversion_ = VK_NULL_HANDLE;
    haveConversion_ = false;
}

bool VulkanRenderer::ensurePipeline() {
    if (pipeline_) {
        return true;
    }
    return createPipeline(pipelineLayout_, fragShader_, &pipeline_);
}

void VulkanRenderer::destroyPipelines() {
    if (pipeline_) {
        vk_.vkDestroyPipeline(device_, pipeline_, nullptr);
        pipeline_ = VK_NULL_HANDLE;
    }
}

bool VulkanRenderer::createPipeline(VkPipelineLayout layout, VkShaderModule fragShader, VkPipeline* pipeline) {
    if (!renderPass_ || !layout) {
        return false;
    }

    VkPipelineShaderStageCreateInfo stages[2] {};
    stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
    stages[0].module = vertShader_;
    stages[0].pName = "main";
    stages[1].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
    stages[1].module = fragShader;
    stages[1].pName = "main";

    VkPipelineVertexInputStateCreateInfo vertexInput {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo inputAssembly {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
    inputAssembly.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;

    VkPipelineViewportStateCreateInfo viewportState {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
    viewportState.viewportCount = 1;
    viewportState.scissorCount = 1;

    VkPipelineRasterizationStateCreateInfo raster {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
    raster.polygonMode = VK_POLYGON_MODE_FILL;
    raster.cullMode = VK_CULL_MODE_NONE;
    raster.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
    raster.lineWidth = 1.0f;

    VkPipelineMultisampleStateCreateInfo multisample {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
    multisample.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;

    VkPipelineColorBlendAttachmentState blendAttachment {};
    blendAttachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                     VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
    VkPipelineColorBlendStateCreateInfo blend {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
    blend.attachmentCount = 1;
    blend.pAttachments = &blendAttachment;

    const VkDynamicState dynamicStates[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
    VkPipelineDynamicStateCreateInfo dynamic {VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO};
    dynamic.dynamicStateCount = 2;
    dynamic.pDynamicStates = dynamicStates;

    VkGraphicsPipelineCreateInfo info {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
    info.stageCount = 2;
    info.pStages = stages;
    info.pVertexInputState = &vertexInput;
    info.pInputAssemblyState = &inputAssembly;
    info.pViewportState = &viewportState;
    info.pRasterizationState = &raster;
    info.pMultisampleState = &multisample;
    info.pColorBlendState = &blend;
    info.pDynamicState = &dynamic;
    info.layout = layout;
    info.renderPass = renderPass_;
    info.subpass = 0;

    VkResult result = vk_.vkCreateGraphicsPipelines(device_, VK_NULL_HANDLE, 1, &info, nullptr, pipeline);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateGraphicsPipelines failed: %d", result);
        *pipeline = VK_NULL_HANDLE;
        return false;
    }
    return true;
}

VulkanRenderer::ImportedBuffer* VulkanRenderer::importBuffer(AHardwareBuffer* buffer) {
    VkAndroidHardwareBufferFormatPropertiesANDROID formatProps {VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID};
    VkAndroidHardwareBufferPropertiesANDROID props {VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_PROPERTIES_ANDROID};
    props.pNext = &formatProps;

    // The color model can change with HDR, so the key is checked even for known buffers
    auto it = imports_.find(buffer);
    if (it == imports_.end() || !haveConversion_) {
        VkResult result = vk_.vkGetAndroidHardwareBufferPropertiesANDROID(device_, buffer, &props);
        if (result != VK_SUCCESS) {
            ALOGE("vkGetAndroidHardwareBufferPropertiesANDROID failed: %d", result);
            return nullptr;
        }

        ConversionKey key;
        key.format = formatProps.format;
        key.externalFormat = formatProps.format == VK_FORMAT_UNDEFINED ? formatProps.externalFormat : 0;
        key.ycbcr = key.externalFormat != 0 || isYcbcrFormat(formatProps.format);
        key.components = formatProps.samplerYcbcrConversionComponents;
        key.xChromaOffset = formatProps.suggestedXChromaOffset;
        key.yChromaOffset = formatProps.suggestedYChromaOffset;
        key.filter = (!key.ycbcr || (formatProps.formatFeatures & VK_FORMAT_FEATURE_SAMPLED_IMAGE_YCBCR_CONVERSION_LINEAR_FILTER_BIT))
                     ? VK_FILTER_LINEAR : VK_FILTER_NEAREST;

        // We know how the host encoded the stream, so use that instead of what the buffer
        // suggests. Sunshine sends HDR as BT.2020.
        int colorspace = hdrActive_ ? 2 : config_.colorspace;
        key.model = colorspace == 0 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_601 :
                    colorspace == 2 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_2020 :
                                      VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_709;
        key.range = config_.fullRange ? VK_SAMPLER_YCBCR_RANGE_ITU_FULL : VK_SAMPLER_YCBCR_RANGE_ITU_NARROW;
        if (!key.ycbcr) {
            key.components = {};
        }

        if (!ensureConversion(key)) {
            return nullptr;
        }
        // A new conversion drops every import, including this buffer's
        it = imports_.find(buffer);
    }
    else {
        const int colorspace = hdrActive_ ? 2 : config_.colorspace;
        const VkSamplerYcbcrModelConversion model =
                colorspace == 0 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_601 :
                colorspace == 2 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_2020 :
                                  VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_709;
        if (conversionKey_.ycbcr && conversionKey_.model != model) {
            ConversionKey key = conversionKey_;
            key.model = model;
            if (!ensureConversion(key)) {
                return nullptr;
            }
            it = imports_.find(buffer);
        }
    }

    if (it != imports_.end()) {
        it->second.lastUsed = frameCounter_;
        return &it->second;
    }

    if (props.allocationSize == 0) {
        VkResult result = vk_.vkGetAndroidHardwareBufferPropertiesANDROID(device_, buffer, &props);
        if (result != VK_SUCCESS) {
            ALOGE("vkGetAndroidHardwareBufferPropertiesANDROID failed: %d", result);
            return nullptr;
        }
    }

    AHardwareBuffer_Desc desc {};
    ndk_->AHardwareBuffer_describe(buffer, &desc);

    ImportedBuffer imported;
    imported.width = desc.width;
    imported.height = desc.height;
    imported.lastUsed = frameCounter_;

    VkExternalFormatANDROID externalFormat {VK_STRUCTURE_TYPE_EXTERNAL_FORMAT_ANDROID};
    externalFormat.externalFormat = conversionKey_.externalFormat;
    VkExternalMemoryImageCreateInfo externalMemory {VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO};
    externalMemory.pNext = &externalFormat;
    externalMemory.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID;

    VkImageCreateInfo imageInfo {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    imageInfo.pNext = &externalMemory;
    imageInfo.imageType = VK_IMAGE_TYPE_2D;
    imageInfo.format = conversionKey_.format;
    imageInfo.extent = {desc.width, desc.height, 1};
    imageInfo.mipLevels = 1;
    imageInfo.arrayLayers = 1;
    imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
    imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
    imageInfo.usage = VK_IMAGE_USAGE_SAMPLED_BIT;
    imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    VkResult result = vk_.vkCreateImage(device_, &imageInfo, nullptr, &imported.image);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateImage for hardware buffer failed: %d", result);
        return nullptr;
    }

    VkImportAndroidHardwareBufferInfoANDROID importInfo {VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID};
    importInfo.buffer = buffer;
    VkMemoryDedicatedAllocateInfo dedicated {VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO};
    dedicated.pNext = &importInfo;
    dedicated.image = imported.image;
    VkMemoryAllocateInfo allocInfo {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    allocInfo.pNext = &dedicated;
    allocInfo.allocationSize = props.allocationSize;
    allocInfo.memoryTypeIndex = static_cast<uint32_t>(__builtin_ctz(props.memoryTypeBits));
    result = vk_.vkAllocateMemory(device_, &allocInfo, nullptr, &imported.memory);
    if (result != VK_SUCCESS) {
        ALOGE("Importing hardware buffer failed: %d", result);
        vk_.vkDestroyImage(device_, imported.image, nullptr);
        return nullptr;
    }
    vk_.vkBindImageMemory(device_, imported.image, imported.memory, 0);

    VkSamplerYcbcrConversionInfo conversionInfo {VK_STRUCTURE_TYPE_SAMPLER_YCBCR_CONVERSION_INFO};
    conversionInfo.conversion = conversion_;
    VkImageViewCreateInfo viewInfo {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    viewInfo.pNext = conversion_ ? &conversionInfo : nullptr;
    viewInfo.image = imported.image;
    viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
    viewInfo.format = conversionKey_.format;
    viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    result = vk_.vkCreateImageView(device_, &viewInfo, nullptr, &imported.view);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateImageView for hardware buffer failed: %d", result);
        vk_.vkFreeMemory(device_, imported.memory, nullptr);
        vk_.vkDestroyImage(device_, imported.image, nullptr);
        return nullptr;
    }

    VkDescriptorSetAllocateInfo setInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
    setInfo.descriptorPool = descriptorPool_;
    setInfo.descriptorSetCount = 1;
    setInfo.pSetLayouts = &setLayout_;
    result = vk_.vkAllocateDescriptorSets(device_, &setInfo, &imported.descriptorSet);
    if (result != VK_SUCCESS) {
        ALOGE("vkAllocateDescriptorSets failed: %d", result);
        vk_.vkDestroyImageView(device_, imported.view, nullptr);
        vk_.vkFreeMemory(device_, imported.memory, nullptr);
        vk_.vkDestroyImage(device_, imported.image, nullptr);
        return nullptr;
    }

    VkDescriptorImageInfo imageDescriptor {};
    imageDescriptor.imageView = imported.view;
    imageDescriptor.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    VkWriteDescriptorSet write {VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET};
    write.dstSet = imported.descriptorSet;
    write.dstBinding = 0;
    write.descriptorCount = 1;
    write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    write.pImageInfo = &imageDescriptor;
    vk_.vkUpdateDescriptorSets(device_, 1, &write, 0, nullptr);

    // Holding a reference keeps the buffer, and so this cache key, from being reused
    ndk_->AHardwareBuffer_acquire(buffer);
    return &imports_.emplace(buffer, imported).first->second;
}

void VulkanRenderer::destroyImport(AHardwareBuffer* buffer, ImportedBuffer& imported) {
    if (imported.descriptorSet) vk_.vkFreeDescriptorSets(device_, descriptorPool_, 1, &imported.descriptorSet);
    if (imported.view) vk_.vkDestroyImageView(device_, imported.view, nullptr);
    if (imported.image) vk_.vkDestroyImage(device_, imported.image, nullptr);
    if (imported.memory) vk_.vkFreeMemory(device_, imported.memory, nullptr);
    ndk_->AHardwareBuffer_release(buffer);
}

void VulkanRenderer::evictImports() {
    // Buffers the decoder stopped using. The last few frames may still be on the GPU.
    while (imports_.size() > kMaxImports) {
        auto oldest = imports_.end();
        for (auto it = imports_.begin(); it != imports_.end(); ++it) {
            if (it->second.lastUsed + kFramesInFlight + 1 < frameCounter_ &&
                    (oldest == imports_.end() || it->second.lastUsed < oldest->second.lastUsed)) {
                oldest = it;
            }
        }
        if (oldest == imports_.end()) {
            return;
        }
        destroyImport(oldest->first, oldest->second);
        imports_.erase(oldest);
    }
}

}  // namespace vkr
