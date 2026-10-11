// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// Rubylight: packet framing, feature gates, HDR/10-bit output and completion timing.
// PyroWave video renderer for Moonlight Android.
//
// PyroWave is an intra-only wavelet codec decoded with Vulkan compute, so it cannot
// go through MediaCodec. This renderer owns a Vulkan device and a swapchain on the
// stream's Surface, lends the device to PyroWave, decodes each frame into three
// Y/Cb/Cr plane images and converts them to RGB in a fragment shader.
//
// Vulkan is loaded at runtime (libvulkan.so is not available on every Android
// version this app supports), so this library links neither libvulkan nor any
// Vulkan prototypes.

#define VK_NO_PROTOTYPES
#define VK_USE_PLATFORM_ANDROID_KHR
#include <vulkan/vulkan.h>

// NDK 29's Vulkan headers predate this extension.
#ifndef VK_EXT_present_mode_fifo_latest_ready
#define VK_EXT_PRESENT_MODE_FIFO_LATEST_READY_EXTENSION_NAME "VK_EXT_present_mode_fifo_latest_ready"
constexpr VkPresentModeKHR VK_PRESENT_MODE_FIFO_LATEST_READY_EXT = static_cast<VkPresentModeKHR>(1000361000);
constexpr VkStructureType VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PRESENT_MODE_FIFO_LATEST_READY_FEATURES_EXT =
    static_cast<VkStructureType>(1000361000);
struct VkPhysicalDevicePresentModeFifoLatestReadyFeaturesEXT {
    VkStructureType sType;
    void *pNext;
    VkBool32 presentModeFifoLatestReady;
};
#endif

// PyroWave SDK 1.0 uses the Vulkan 1.4 name; NDK 29 only has the KHR one.
#ifndef VK_VERSION_1_4
typedef VkQueueGlobalPriorityKHR VkQueueGlobalPriority;
#endif

#include <pyrowave/pyrowave.h>

#include <android/log.h>
#include <android/native_window_jni.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>
#include <jni.h>

#include <sys/resource.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <deque>
#include <iterator>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "shaders_spv.h"
#include "frame.h"
#include "rubylight_protocol.h"

#define LOG_TAG "PyroWave"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace {
    // Results returned to Java.
    constexpr int SUBMIT_OK = 0;
    constexpr int SUBMIT_SKIPPED = 1;
    constexpr int SUBMIT_ERROR = -1;

    // Keep these reason codes aligned with PyroWaveDecoderRenderer.getReadinessSummary().
    enum Readiness {
        READY = 0,
        NEEDS_VULKAN_1_3 = 1,
        MISSING_SUBGROUP_OPS = 2,
        MISSING_DEVICE_FEATURES = 3,
        MISSING_IMAGE_FORMATS = 4,
        INSUFFICIENT_DEVICE_LIMITS = 5,
        MISSING_GRAPHICS_QUEUE = 6,
        MISSING_SWAPCHAIN = 7,
        MISSING_HDR = 8,
        PROBE_FAILED = 9,
    };

    constexpr uint64_t ACQUIRE_TIMEOUT_NS = 250'000'000;
    constexpr uint64_t FENCE_TIMEOUT_NS = 2'000'000'000;

    // Upscaling passes for streams smaller than the surface.
    constexpr int UPSCALE_OFF = 0;
    constexpr int UPSCALE_SGSR = 1;

    // VK_KHR_global_priority and its EXT predecessor share one create-info layout.
    const char *const GLOBAL_PRIORITY_EXTENSIONS[] = {"VK_KHR_global_priority", "VK_EXT_global_priority"};

    // Adreno power control from the kernel UAPI header msm_kgsl.h, as used by emulators to pin
    // GPU clocks: property value 0 turns power management off, 1 restores it.
    struct KgslProperty {
        unsigned int type;
        void *value;
        size_t sizebytes;
    };
    constexpr unsigned int KGSL_PROP_PWRCTRL = 0x0E;
    constexpr unsigned long KGSL_IOCTL_SETPROPERTY = _IOW(0x09, 0x32, KgslProperty);

    bool setAdrenoMaxClocks(bool enabled) {
        int fd = open("/dev/kgsl-3d0", O_RDWR | O_CLOEXEC);
        if (fd < 0) return false;
        unsigned int powerControl = enabled ? 0u : 1u;
        KgslProperty property = {KGSL_PROP_PWRCTRL, &powerControl, sizeof(powerControl)};
        bool ok = ioctl(fd, KGSL_IOCTL_SETPROPERTY, &property) == 0;
        close(fd);
        LOGI("Adreno max clocks %s: %s", enabled ? "on" : "off", ok ? "applied" : "rejected");
        return ok;
    }

    const char *const INSTANCE_EXTENSIONS[] = {
        VK_KHR_SURFACE_EXTENSION_NAME,
        VK_KHR_ANDROID_SURFACE_EXTENSION_NAME,
    };
    const char *const DEVICE_EXTENSIONS[] = {
        VK_KHR_SWAPCHAIN_EXTENSION_NAME,
    };

#define VK_GLOBAL_FUNCTIONS(X) \
    X(CreateInstance) \
    X(EnumerateInstanceVersion) \
    X(EnumerateInstanceExtensionProperties)

#define VK_INSTANCE_FUNCTIONS(X) \
    X(DestroyInstance) \
    X(EnumeratePhysicalDevices) \
    X(GetPhysicalDeviceProperties) \
    X(GetPhysicalDeviceProperties2) \
    X(GetPhysicalDeviceFormatProperties) \
    X(EnumerateDeviceExtensionProperties) \
    X(GetPhysicalDeviceFeatures2) \
    X(GetPhysicalDeviceQueueFamilyProperties) \
    X(GetPhysicalDeviceMemoryProperties) \
    X(CreateDevice) \
    X(GetDeviceProcAddr) \
    X(CreateAndroidSurfaceKHR) \
    X(DestroySurfaceKHR) \
    X(GetPhysicalDeviceSurfaceSupportKHR) \
    X(GetPhysicalDeviceSurfaceCapabilitiesKHR) \
    X(GetPhysicalDeviceSurfaceFormatsKHR) \
    X(GetPhysicalDeviceSurfacePresentModesKHR)

#define VK_DEVICE_FUNCTIONS(X) \
    X(DestroyDevice) \
    X(GetDeviceQueue) \
    X(DeviceWaitIdle) \
    X(CreateImage) \
    X(DestroyImage) \
    X(GetImageMemoryRequirements) \
    X(AllocateMemory) \
    X(FreeMemory) \
    X(BindImageMemory) \
    X(CreateImageView) \
    X(DestroyImageView) \
    X(CreateSampler) \
    X(DestroySampler) \
    X(CreateDescriptorSetLayout) \
    X(DestroyDescriptorSetLayout) \
    X(CreatePipelineLayout) \
    X(DestroyPipelineLayout) \
    X(CreateDescriptorPool) \
    X(DestroyDescriptorPool) \
    X(AllocateDescriptorSets) \
    X(UpdateDescriptorSets) \
    X(CreateShaderModule) \
    X(DestroyShaderModule) \
    X(CreateRenderPass) \
    X(DestroyRenderPass) \
    X(CreateGraphicsPipelines) \
    X(DestroyPipeline) \
    X(CreateFramebuffer) \
    X(DestroyFramebuffer) \
    X(CreateCommandPool) \
    X(DestroyCommandPool) \
    X(AllocateCommandBuffers) \
    X(BeginCommandBuffer) \
    X(EndCommandBuffer) \
    X(ResetCommandBuffer) \
    X(CmdPipelineBarrier) \
    X(CmdBeginRenderPass) \
    X(CmdEndRenderPass) \
    X(CmdBindPipeline) \
    X(CmdBindDescriptorSets) \
    X(CmdSetViewport) \
    X(CmdSetScissor) \
    X(CmdDraw) \
    X(CmdPushConstants) \
    X(CreateFence) \
    X(DestroyFence) \
    X(WaitForFences) \
    X(ResetFences) \
    X(CreateSemaphore) \
    X(DestroySemaphore) \
    X(QueueSubmit) \
    X(CreateSwapchainKHR) \
    X(DestroySwapchainKHR) \
    X(GetSwapchainImagesKHR) \
    X(AcquireNextImageKHR) \
    X(QueuePresentKHR) \
    X(CreateQueryPool) \
    X(DestroyQueryPool) \
    X(CmdResetQueryPool) \
    X(CmdWriteTimestamp) \
    X(GetQueryPoolResults)

    struct VulkanLoader {
        void *library = nullptr;
        PFN_vkGetInstanceProcAddr GetInstanceProcAddr = nullptr;
        PFN_vkSetHdrMetadataEXT SetHdrMetadataEXT = nullptr;
        PFN_vkGetPastPresentationTimingGOOGLE GetPastPresentationTimingGOOGLE = nullptr;
#define X(name) PFN_vk##name name = nullptr;
        VK_GLOBAL_FUNCTIONS(X)
        VK_INSTANCE_FUNCTIONS(X)
        VK_DEVICE_FUNCTIONS(X)
#undef X

        ~VulkanLoader() {
            if (library != nullptr) {
                dlclose(library);
            }
        }

        bool loadGlobal() {
            library = dlopen("libvulkan.so", RTLD_NOW | RTLD_LOCAL);
            if (library == nullptr) {
                LOGI("No Vulkan loader on this device");
                return false;
            }
            GetInstanceProcAddr = reinterpret_cast<PFN_vkGetInstanceProcAddr>(dlsym(library, "vkGetInstanceProcAddr"));
            if (GetInstanceProcAddr == nullptr) {
                return false;
            }
#define X(name) name = reinterpret_cast<PFN_vk##name>(GetInstanceProcAddr(VK_NULL_HANDLE, "vk" #name));
            VK_GLOBAL_FUNCTIONS(X)
#undef X
            // vkEnumerateInstanceVersion is missing on Vulkan 1.0 loaders, which is fine: 1.0 is not enough.
            return CreateInstance != nullptr && EnumerateInstanceVersion != nullptr &&
                   EnumerateInstanceExtensionProperties != nullptr;
        }

        bool loadInstance(VkInstance instance) {
            bool ok = true;
#define X(name) \
            name = reinterpret_cast<PFN_vk##name>(GetInstanceProcAddr(instance, "vk" #name)); \
            ok = ok && name != nullptr;
            VK_INSTANCE_FUNCTIONS(X)
#undef X
            return ok;
        }

        bool loadDevice(VkDevice device) {
            bool ok = true;
#define X(name) \
            name = reinterpret_cast<PFN_vk##name>(GetDeviceProcAddr(device, "vk" #name)); \
            ok = ok && name != nullptr;
            VK_DEVICE_FUNCTIONS(X)
#undef X
            SetHdrMetadataEXT = reinterpret_cast<PFN_vkSetHdrMetadataEXT>(GetDeviceProcAddr(device, "vkSetHdrMetadataEXT"));
            GetPastPresentationTimingGOOGLE = reinterpret_cast<PFN_vkGetPastPresentationTimingGOOGLE>(
                GetDeviceProcAddr(device, "vkGetPastPresentationTimingGOOGLE"));
            return ok;
        }
    };

    // Features PyroWave's decode kernels use unconditionally. shaderFloat16 is optional.
    struct FeatureProbe {
        Readiness reason = NEEDS_VULKAN_1_3;
        bool float16 = false;
        uint32_t apiVersion = 0;
        char name[VK_MAX_PHYSICAL_DEVICE_NAME_SIZE] = {};
    };

    FeatureProbe probeFeatures(const VulkanLoader &vk, VkPhysicalDevice device, bool tenBit) {
        FeatureProbe probe;
        VkPhysicalDeviceProperties props;
        vk.GetPhysicalDeviceProperties(device, &props);
        probe.apiVersion = props.apiVersion;
        std::memcpy(probe.name, props.deviceName, sizeof(probe.name));
        if (props.apiVersion < VK_API_VERSION_1_3) {
            return probe;
        }

        VkPhysicalDeviceVulkan13Features f13 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_FEATURES};
        VkPhysicalDeviceVulkan12Features f12 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES};
        f12.pNext = &f13;
        VkPhysicalDeviceVulkan11Features f11 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_1_FEATURES};
        f11.pNext = &f12;
        VkPhysicalDeviceFeatures2 f2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
        f2.pNext = &f11;
        vk.GetPhysicalDeviceFeatures2(device, &f2);

        VkPhysicalDeviceSubgroupProperties subgroup = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SUBGROUP_PROPERTIES};
        VkPhysicalDeviceVulkan13Properties p13 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_PROPERTIES};
        subgroup.pNext = &p13;
        VkPhysicalDeviceProperties2 p2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2};
        p2.pNext = &subgroup;
        vk.GetPhysicalDeviceProperties2(device, &p2);
        constexpr VkSubgroupFeatureFlags required = VK_SUBGROUP_FEATURE_BASIC_BIT | VK_SUBGROUP_FEATURE_VOTE_BIT |
            VK_SUBGROUP_FEATURE_BALLOT_BIT | VK_SUBGROUP_FEATURE_ARITHMETIC_BIT |
            VK_SUBGROUP_FEATURE_SHUFFLE_BIT | VK_SUBGROUP_FEATURE_SHUFFLE_RELATIVE_BIT;
        bool subgroups = (subgroup.supportedOperations & required) == required &&
            (subgroup.supportedStages & VK_SHADER_STAGE_COMPUTE_BIT) &&
            ((p13.minSubgroupSize >= 4 && p13.maxSubgroupSize <= 128) ||
             ((p13.requiredSubgroupSizeStages & VK_SHADER_STAGE_COMPUTE_BIT) &&
              p13.minSubgroupSize <= 128 && p13.maxSubgroupSize >= 4));
        if (!subgroups || !f13.subgroupSizeControl || !f13.computeFullSubgroups) {
            probe.reason = MISSING_SUBGROUP_OPS;
            return probe;
        }
        if (!f2.features.shaderStorageImageWriteWithoutFormat || !f2.features.shaderInt16 ||
            !f11.storageBuffer16BitAccess || !f12.storageBuffer8BitAccess ||
            !f12.timelineSemaphore || !f13.synchronization2) {
            probe.reason = MISSING_DEVICE_FEATURES;
            return probe;
        }
        probe.float16 = f12.shaderFloat16;
        if (props.limits.maxImageArrayLayers < 12 || props.limits.maxComputeWorkGroupInvocations < 128 ||
            props.limits.maxComputeWorkGroupSize[0] < 128) {
            probe.reason = INSUFFICIENT_DEVICE_LIMITS;
            return probe;
        }
        // Mixed-precision wavelets use FP16 high bands and FP32 low bands.
        for (auto format : {VK_FORMAT_R16_SFLOAT, VK_FORMAT_R32_SFLOAT, VK_FORMAT_R16G16_SFLOAT}) {
            VkFormatProperties formats;
            vk.GetPhysicalDeviceFormatProperties(device, format, &formats);
            VkFormatFeatureFlags required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT |
                VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT | VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT;
            if ((formats.optimalTilingFeatures & required) != required) {
                probe.reason = MISSING_IMAGE_FORMATS;
                return probe;
            }
        }
        VkFormatProperties plane;
        vk.GetPhysicalDeviceFormatProperties(device, tenBit ? VK_FORMAT_R16_UNORM : VK_FORMAT_R8_UNORM, &plane);
        VkFormatFeatureFlags sampled = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT;
        if ((plane.optimalTilingFeatures & sampled) != sampled ||
            !(plane.optimalTilingFeatures & (VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT | VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT))) {
            probe.reason = MISSING_IMAGE_FORMATS;
            return probe;
        }
        uint32_t familyCount = 0;
        vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, nullptr);
        std::vector<VkQueueFamilyProperties> families(familyCount);
        vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, families.data());
        bool graphicsCompute = false;
        for (uint32_t i = 0; i < familyCount; ++i) {
            graphicsCompute |= (families[i].queueFlags & (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT)) ==
                (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT);
        }
        if (!graphicsCompute) {
            probe.reason = MISSING_GRAPHICS_QUEUE;
            return probe;
        }
        uint32_t extensionCount = 0;
        probe.reason = PROBE_FAILED;
        if (vk.EnumerateDeviceExtensionProperties(device, nullptr, &extensionCount, nullptr) != VK_SUCCESS) return probe;
        std::vector<VkExtensionProperties> extensions(extensionCount);
        if (vk.EnumerateDeviceExtensionProperties(device, nullptr, &extensionCount, extensions.data()) != VK_SUCCESS) return probe;
        extensions.resize(extensionCount);
        bool swapchain = false, metadata = false;
        for (const auto &extension : extensions) {
            if (!strcmp(extension.extensionName, VK_KHR_SWAPCHAIN_EXTENSION_NAME)) swapchain = true;
            if (!strcmp(extension.extensionName, VK_EXT_HDR_METADATA_EXTENSION_NAME)) metadata = true;
        }
        probe.reason = !swapchain ? MISSING_SWAPCHAIN : (tenBit && !metadata ? MISSING_HDR : READY);
        return probe;
    }

    bool apiVersionSupported() {
        uint32_t major = 0, minor = 0, patch = 0;
        pyrowave_get_api_version(&major, &minor, &patch);
        if (major != PYROWAVE_API_VERSION_MAJOR || minor != PYROWAVE_API_VERSION_MINOR) {
            LOGE("PyroWave runtime %u.%u.%u does not match %u.%u", major, minor, patch,
                 PYROWAVE_API_VERSION_MAJOR, PYROWAVE_API_VERSION_MINOR);
            return false;
        }
        return true;
    }

    uint64_t nowUs() {
        timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        return uint64_t(ts.tv_sec) * 1000000u + uint64_t(ts.tv_nsec) / 1000u;
    }

    const char *presentModeName(VkPresentModeKHR mode) {
        switch (mode) {
            case VK_PRESENT_MODE_IMMEDIATE_KHR: return "IMMEDIATE";
            case VK_PRESENT_MODE_MAILBOX_KHR: return "MAILBOX";
            case VK_PRESENT_MODE_FIFO_KHR: return "FIFO";
            case VK_PRESENT_MODE_FIFO_LATEST_READY_EXT: return "FIFO_LATEST_READY";
            case VK_PRESENT_MODE_FIFO_RELAXED_KHR: return "FIFO_RELAXED";
            default: return "other";
        }
    }

    struct Plane {
        VkImage image = VK_NULL_HANDLE;
        VkDeviceMemory memory = VK_NULL_HANDLE;
        VkImageView view = VK_NULL_HANDLE;
        uint32_t width = 0;
        uint32_t height = 0;
    };

    // One decoded frame's Y/Cb/Cr planes and what drawing them needs.
    struct Slot {
        Plane planes[3];
        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        VkCommandBuffer drawCommandBuffer = VK_NULL_HANDLE;
        // Signaled whenever no draw of these planes is in flight.
        VkFence drawFence = VK_NULL_HANDLE;
        bool initialized = false;
        int64_t ptsUs = 0;
    };

    class Renderer {
    public:
        PyroWaveRecords records;
        ~Renderer() {
            destroy();
        }

        bool create(ANativeWindow *nativeWindow, int streamWidth, int streamHeight, int frameRate, float displayRefreshRate,
                    bool fullChroma, bool tenBitOutput, bool fullRangeOutput, int upscale, float sharpness) {
            window = nativeWindow;
            // SGSR works on display-referred SDR; PQ-encoded HDR keeps the direct path.
            upscaleMode = tenBitOutput || upscale != UPSCALE_SGSR ? UPSCALE_OFF : UPSCALE_SGSR;
            edgeSharpness = sharpness;
            chroma444 = fullChroma;
            tenBit = hdr = tenBitOutput;
            fullRange = fullRangeOutput;
            frameRateHz = frameRate > 0 ? frameRate : 60;
            displayRefreshRateHz = displayRefreshRate;
            width = uint32_t(streamWidth);
            height = uint32_t(streamHeight);
            records = PyroWaveRecords(width, height, chroma444);
            return apiVersionSupported() && createInstanceAndSurface() && createDevice() &&
                   createSwapchain() && checkDecoderLimits() && createDecoder() && createPlanes() &&
                   createPipeline() && createUpscaler() && createFrameResources() && warmUpDecoder() &&
                   startPresenter();
        }

        int submit(const uint8_t *data, size_t length, int64_t ptsUs) {
            completedDecodeNs = 0;
            if (failed) return SUBMIT_ERROR;
            pyrowave_decoder_clear(decoder);
            if (!pushFrame(data, length)) {
                pyrowave_decoder_clear(decoder);
                return SUBMIT_SKIPPED;
            }
            // Intra-only: a frame missing packets still decodes, with lost detail.
            if (!pyrowave_decoder_decode_is_ready(decoder, false)) {
                return SUBMIT_SKIPPED;
            }
            if (decodeFrame(true, ptsUs)) return SUBMIT_OK;
            failed = true;
            return SUBMIT_ERROR;
        }

    private:
        bool check(VkResult result, const char *what) {
            if (result != VK_SUCCESS) {
                LOGE("%s failed: %d", what, result);
                return false;
            }
            return true;
        }

        bool createInstanceAndSurface() {
            if (!vk.loadGlobal()) {
                return false;
            }
            uint32_t loaderVersion = 0;
            if (!check(vk.EnumerateInstanceVersion(&loaderVersion), "Vulkan loader version")) return false;
            if (loaderVersion < VK_API_VERSION_1_3) {
                LOGE("Vulkan loader %u.%u is older than 1.3", VK_API_VERSION_MAJOR(loaderVersion), VK_API_VERSION_MINOR(loaderVersion));
                return false;
            }

            // These create infos stay alive for the device's lifetime: PyroWave reads them.
            appInfo = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
            appInfo.pApplicationName = "Rubylight";
            appInfo.apiVersion = VK_API_VERSION_1_3;
            instanceInfo = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
            instanceInfo.pApplicationInfo = &appInfo;
            uint32_t extensionCount = 0;
            if (!check(vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, nullptr), "instance extension count")) return false;
            std::vector<VkExtensionProperties> extensions(extensionCount);
            if (!check(vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, extensions.data()), "instance extensions")) return false;
            extensions.resize(extensionCount);
            instanceExtensions.assign(std::begin(INSTANCE_EXTENSIONS), std::end(INSTANCE_EXTENSIONS));
            bool colorspace = false;
            for (const auto &ext : extensions) {
                if (!strcmp(ext.extensionName, VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME)) colorspace = true;
            }
            if (tenBit && !colorspace) return false;
            if (colorspace) instanceExtensions.push_back(VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
            instanceInfo.enabledExtensionCount = uint32_t(instanceExtensions.size());
            instanceInfo.ppEnabledExtensionNames = instanceExtensions.data();
            if (!check(vk.CreateInstance(&instanceInfo, nullptr, &instance), "vkCreateInstance")) {
                return false;
            }
            if (!vk.loadInstance(instance)) {
                LOGE("Vulkan instance is missing required functions");
                // destroy() must not call through missing pointers.
                auto destroyInstance = reinterpret_cast<PFN_vkDestroyInstance>(vk.GetInstanceProcAddr(instance, "vkDestroyInstance"));
                if (destroyInstance != nullptr) {
                    destroyInstance(instance, nullptr);
                }
                instance = VK_NULL_HANDLE;
                return false;
            }

            VkAndroidSurfaceCreateInfoKHR surfaceInfo = {VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR};
            surfaceInfo.window = window;
            return check(vk.CreateAndroidSurfaceKHR(instance, &surfaceInfo, nullptr, &surface), "vkCreateAndroidSurfaceKHR");
        }

        bool createDevice() {
            uint32_t count = 0;
            if (!check(vk.EnumeratePhysicalDevices(instance, &count, nullptr), "physical device count") || count == 0) return false;
            std::vector<VkPhysicalDevice> devices(count);
            if (!check(vk.EnumeratePhysicalDevices(instance, &count, devices.data()), "physical devices")) return false;
            devices.resize(count);

            FeatureProbe chosenProbe;
            for (auto device : devices) {
                auto probe = probeFeatures(vk, device, tenBit);
                if (probe.reason != READY) {
                    LOGI("Skipping %s: PyroWave reason %d", probe.name, probe.reason);
                    continue;
                }
                uint32_t familyCount = 0;
                vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, nullptr);
                std::vector<VkQueueFamilyProperties> families(familyCount);
                vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, families.data());
                for (uint32_t i = 0; i < familyCount; ++i) {
                    VkBool32 presentable = VK_FALSE;
                    vk.GetPhysicalDeviceSurfaceSupportKHR(device, i, surface, &presentable);
                    if ((families[i].queueFlags & (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT)) ==
                        (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT) && presentable) {
                        physicalDevice = device;
                        queueFamily = i;
                        timestampsSupported = families[i].timestampValidBits > 0;
                        timestampValidBits = families[i].timestampValidBits;
                        chosenProbe = probe;
                        break;
                    }
                }
                if (physicalDevice != VK_NULL_HANDLE) {
                    break;
                }
            }
            if (physicalDevice == VK_NULL_HANDLE) {
                LOGE("No Vulkan 1.3 device can decode PyroWave and present to this surface");
                return false;
            }

            queuePriority = 1.0f;
            queueInfo = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
            // Like a game's render queue: ask the GPU scheduler to run our work ahead of other apps.
            globalPriority = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_GLOBAL_PRIORITY_CREATE_INFO_EXT};
            globalPriority.globalPriority = VK_QUEUE_GLOBAL_PRIORITY_HIGH_EXT;
            queueInfo.queueFamilyIndex = queueFamily;
            queueInfo.queueCount = 1;
            queueInfo.pQueuePriorities = &queuePriority;

            features13 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_FEATURES};
            features13.synchronization2 = VK_TRUE;
            features13.subgroupSizeControl = VK_TRUE;
            features13.computeFullSubgroups = VK_TRUE;
            features12 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES};
            features12.pNext = &features13;
            features12.timelineSemaphore = VK_TRUE;
            features12.storageBuffer8BitAccess = VK_TRUE;
            features12.shaderFloat16 = chosenProbe.float16 ? VK_TRUE : VK_FALSE;
            features11 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_1_FEATURES};
            features11.pNext = &features12;
            features11.storageBuffer16BitAccess = VK_TRUE;
            features2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
            features2.pNext = &features11;
            features2.features.shaderInt16 = VK_TRUE;
            features2.features.shaderStorageImageWriteWithoutFormat = VK_TRUE;

            deviceInfo = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
            deviceInfo.pNext = &features2;
            deviceInfo.queueCreateInfoCount = 1;
            deviceInfo.pQueueCreateInfos = &queueInfo;
            deviceExtensions.assign(std::begin(DEVICE_EXTENSIONS), std::end(DEVICE_EXTENSIONS));
            uint32_t extensionCount = 0;
            if (!check(vk.EnumerateDeviceExtensionProperties(physicalDevice, nullptr, &extensionCount, nullptr), "device extension count")) return false;
            std::vector<VkExtensionProperties> extensions(extensionCount);
            if (!check(vk.EnumerateDeviceExtensionProperties(physicalDevice, nullptr, &extensionCount, extensions.data()), "device extensions")) return false;
            extensions.resize(extensionCount);
            bool metadata = false;
            bool fifoLatestReadyExtension = false;
            const char *priorityExtension = nullptr;
            for (const auto &ext : extensions) {
                for (const char *name : GLOBAL_PRIORITY_EXTENSIONS) {
                    if (priorityExtension == nullptr && !strcmp(ext.extensionName, name)) priorityExtension = name;
                }
                if (!strcmp(ext.extensionName, VK_EXT_HDR_METADATA_EXTENSION_NAME)) metadata = true;
                if (!strcmp(ext.extensionName, VK_GOOGLE_DISPLAY_TIMING_EXTENSION_NAME)) displayTimingSupported = true;
                if (!strcmp(ext.extensionName, VK_EXT_PRESENT_MODE_FIFO_LATEST_READY_EXTENSION_NAME)) fifoLatestReadyExtension = true;
            }
            if (fifoLatestReadyExtension) {
                fifoLatestReadyFeatures = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PRESENT_MODE_FIFO_LATEST_READY_FEATURES_EXT};
                VkPhysicalDeviceFeatures2 available = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
                available.pNext = &fifoLatestReadyFeatures;
                vk.GetPhysicalDeviceFeatures2(physicalDevice, &available);
                if (fifoLatestReadyFeatures.presentModeFifoLatestReady) {
                    features13.pNext = &fifoLatestReadyFeatures;
                    deviceExtensions.push_back(VK_EXT_PRESENT_MODE_FIFO_LATEST_READY_EXTENSION_NAME);
                }
            }
            if (displayTimingSupported) deviceExtensions.push_back(VK_GOOGLE_DISPLAY_TIMING_EXTENSION_NAME);
            // KHR present_id + present_wait provide no timestamp, and can signal for replaced images.
            // Sampling CLOCK_MONOTONIC after a wait would invent a per-frame presentation time.
            if (tenBit) {
                if (!metadata) return false;
                deviceExtensions.push_back(VK_EXT_HDR_METADATA_EXTENSION_NAME);
            }
            if (priorityExtension != nullptr) {
                deviceExtensions.push_back(priorityExtension);
                queueInfo.pNext = &globalPriority;
            }
            deviceInfo.enabledExtensionCount = uint32_t(deviceExtensions.size());
            deviceInfo.ppEnabledExtensionNames = deviceExtensions.data();
            VkResult created = vk.CreateDevice(physicalDevice, &deviceInfo, nullptr, &device);
            if (created != VK_SUCCESS && priorityExtension != nullptr) {
                // Drivers may refuse elevated priority to unprivileged apps; run at normal priority.
                LOGI("High GPU queue priority refused (%d); using normal priority", created);
                deviceExtensions.pop_back();
                queueInfo.pNext = nullptr;
                deviceInfo.enabledExtensionCount = uint32_t(deviceExtensions.size());
                priorityExtension = nullptr;
                created = vk.CreateDevice(physicalDevice, &deviceInfo, nullptr, &device);
            }
            if (!check(created, "vkCreateDevice")) {
                return false;
            }
            highPriorityQueue = priorityExtension != nullptr;
            if (!vk.loadDevice(device)) {
                LOGE("Vulkan device is missing required functions");
                auto destroyDevice = reinterpret_cast<PFN_vkDestroyDevice>(vk.GetDeviceProcAddr(device, "vkDestroyDevice"));
                if (destroyDevice != nullptr) {
                    destroyDevice(device, nullptr);
                }
                device = VK_NULL_HANDLE;
                return false;
            }
            if (tenBit && vk.SetHdrMetadataEXT == nullptr) return false;
            displayTimingSupported = displayTimingSupported && vk.GetPastPresentationTimingGOOGLE != nullptr;
            LOGI("Actual presentation timestamps: %s", displayTimingSupported ? "VK_GOOGLE_display_timing" : "unavailable");
            vk.GetDeviceQueue(device, queueFamily, 0, &queue);
            VkPhysicalDeviceProperties props;
            vk.GetPhysicalDeviceProperties(physicalDevice, &props);
            timestampPeriodNs = props.limits.timestampPeriod;
            VkPhysicalDeviceDriverProperties driverProps = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES};
            VkPhysicalDeviceProperties2 props2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2};
            props2.pNext = &driverProps;
            vk.GetPhysicalDeviceProperties2(physicalDevice, &props2);
            driverDescription = std::string(props.deviceName) + ", " + driverProps.driverName +
                                (driverProps.driverInfo[0] ? std::string(" ") + driverProps.driverInfo : std::string());
            LOGI("GPU driver: %s, queue priority %s", driverDescription.c_str(), highPriorityQueue ? "high" : "normal");

            LOGI("%s on Vulkan %u.%u (float16: %d)", chosenProbe.name, VK_API_VERSION_MAJOR(chosenProbe.apiVersion),
                 VK_API_VERSION_MINOR(chosenProbe.apiVersion), chosenProbe.float16);
            return true;
        }

        bool checkDecoderLimits() {
            VkPhysicalDeviceProperties props;
            vk.GetPhysicalDeviceProperties(physicalDevice, &props);
            return props.limits.maxImageDimension2D >= std::max((width + 127) & ~127u, (height + 127) & ~127u);
        }

        bool warmUpDecoder() {
            // A valid zero-coefficient grey frame compiles and executes the selected kernels
            // before we advertise this format. It is neither presented nor included in stream timing.
            const uint32_t sequence[] = {0x80000000u | (width - 1) | ((height - 1) << 14),
                                         uint32_t(chroma444) << 26};
            if (pyrowave_decoder_push_packet(decoder, sequence, sizeof(sequence)) != PYROWAVE_SUCCESS ||
                !pyrowave_decoder_decode_is_ready(decoder, false) || !decodeFrame(false)) return false;
            pyrowave_decoder_clear(decoder);
            decodeStats = {};
            return true;
        }

        bool createDecoder() {
            pyrowave_device_create_queue_info pyroQueue = {queue, queueFamily, 0};
            pyrowave_device_create_info info = {};
            info.GetInstanceProcAddr = vk.GetInstanceProcAddr;
            info.instance = instance;
            info.physical_device = physicalDevice;
            info.device = device;
            info.instance_create_info = &instanceInfo;
            info.device_create_info = &deviceInfo;
            info.queue_info = &pyroQueue;
            info.queue_info_count = 1;
            // The decode and present threads share the queue; PyroWave takes the same lock for
            // any submission of its own.
            info.queue_lock_callback = [](void *self) { static_cast<Renderer *>(self)->queueMutex.lock(); };
            info.queue_unlock_callback = [](void *self) { static_cast<Renderer *>(self)->queueMutex.unlock(); };
            info.userdata = this;
            auto result = pyrowave_create_device(&info, &pyroDevice);
            if (result != PYROWAVE_SUCCESS) {
                LOGE("pyrowave_create_device failed: %d", result);
                return false;
            }
            // Mobile GPUs (Adreno, Mali) decode much faster with PyroWave's fragment path,
            // which runs the inverse DWT in render passes instead of compute shaders.
            fragmentPath = pyrowave_decoder_device_prefers_fragment_path(pyroDevice);
            // Command buffers are recorded for the graphics queue, which also does compute.
            pyrowave_device_set_queue_type(pyroDevice, fragmentPath ? VK_QUEUE_GRAPHICS_BIT : VK_QUEUE_COMPUTE_BIT);

            pyrowave_decoder_create_info decoderInfo = {};
            decoderInfo.device = pyroDevice;
            decoderInfo.width = int(width);
            decoderInfo.height = int(height);
            decoderInfo.chroma = chroma444 ? PYROWAVE_CHROMA_SUBSAMPLING_444 : PYROWAVE_CHROMA_SUBSAMPLING_420;
            // The decoder writes the planes (as storage images or render targets) and the
            // CSC pass samples them directly.
            decoderInfo.fragment_path = fragmentPath;
            LOGI("Using the PyroWave %s decode path", fragmentPath ? "fragment" : "compute");
            result = pyrowave_decoder_create(&decoderInfo, &decoder);
            if (result != PYROWAVE_SUCCESS) {
                LOGE("pyrowave_decoder_create failed: %d", result);
                return false;
            }
            return true;
        }

        bool createPlane(Plane &plane, uint32_t planeWidth, uint32_t planeHeight) {
            return createImage(plane, planeWidth, planeHeight, planeFormat(),
                               (fragmentPath ? VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT : VK_IMAGE_USAGE_STORAGE_BIT) |
                               VK_IMAGE_USAGE_SAMPLED_BIT);
        }

        bool createImage(Plane &plane, uint32_t planeWidth, uint32_t planeHeight, VkFormat format, VkImageUsageFlags usage) {
            plane.width = planeWidth;
            plane.height = planeHeight;

            VkImageCreateInfo imageInfo = {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
            imageInfo.imageType = VK_IMAGE_TYPE_2D;
            imageInfo.format = format;
            imageInfo.extent = {planeWidth, planeHeight, 1};
            imageInfo.mipLevels = 1;
            imageInfo.arrayLayers = 1;
            imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
            imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
            imageInfo.usage = usage;
            imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
            imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            if (!check(vk.CreateImage(device, &imageInfo, nullptr, &plane.image), "vkCreateImage")) {
                return false;
            }

            VkMemoryRequirements requirements;
            vk.GetImageMemoryRequirements(device, plane.image, &requirements);
            VkPhysicalDeviceMemoryProperties memoryProps;
            vk.GetPhysicalDeviceMemoryProperties(physicalDevice, &memoryProps);
            uint32_t typeIndex = UINT32_MAX;
            for (uint32_t i = 0; i < memoryProps.memoryTypeCount; ++i) {
                if ((requirements.memoryTypeBits & (1u << i)) &&
                    (memoryProps.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
                    typeIndex = i;
                    break;
                }
            }
            if (typeIndex == UINT32_MAX) {
                LOGE("No device-local memory for a plane image");
                return false;
            }
            VkMemoryAllocateInfo allocInfo = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
            allocInfo.allocationSize = requirements.size;
            allocInfo.memoryTypeIndex = typeIndex;
            if (!check(vk.AllocateMemory(device, &allocInfo, nullptr, &plane.memory), "vkAllocateMemory") ||
                !check(vk.BindImageMemory(device, plane.image, plane.memory, 0), "vkBindImageMemory")) {
                return false;
            }

            VkImageViewCreateInfo viewInfo = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
            viewInfo.image = plane.image;
            viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
            viewInfo.format = format;
            viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
            return check(vk.CreateImageView(device, &viewInfo, nullptr, &plane.view), "vkCreateImageView");
        }

        VkFormat planeFormat() const { return tenBit ? VK_FORMAT_R16_UNORM : VK_FORMAT_R8_UNORM; }

        bool createPlanes() {
            VkFormatProperties props;
            vk.GetPhysicalDeviceFormatProperties(physicalDevice, planeFormat(), &props);
            VkFormatFeatureFlags required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT |
                VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT |
                (fragmentPath ? VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT : VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT);
            if ((props.optimalTilingFeatures & required) != required) return false;
            // The CSC shader samples chroma with normalised coordinates, so full-size
            // (4:4:4) planes need no shader change; its siting offset turns itself off.
            const uint32_t chromaWidth = chroma444 ? width : width / 2;
            const uint32_t chromaHeight = chroma444 ? height : height / 2;
            for (auto &slot : slots) {
                if (!createPlane(slot.planes[0], width, height) ||
                    !createPlane(slot.planes[1], chromaWidth, chromaHeight) ||
                    !createPlane(slot.planes[2], chromaWidth, chromaHeight)) return false;
            }
            return true;
        }

        bool createSwapchain() {
            VkSurfaceCapabilitiesKHR caps;
            if (!check(vk.GetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice, surface, &caps), "surface capabilities")) {
                return false;
            }
            if (caps.currentExtent.width == 0 || caps.currentExtent.height == 0) {
                LOGW("Surface has no extent yet");
                return false;
            }

            if (swapchainFormat == VK_FORMAT_UNDEFINED) {
                uint32_t formatCount = 0;
                if (!check(vk.GetPhysicalDeviceSurfaceFormatsKHR(physicalDevice, surface, &formatCount, nullptr), "surface format count")) return false;
                std::vector<VkSurfaceFormatKHR> formats(formatCount);
                if (!check(vk.GetPhysicalDeviceSurfaceFormatsKHR(physicalDevice, surface, &formatCount, formats.data()), "surface formats")) return false;
                formats.resize(formatCount);
                if (formats.empty()) {
                    LOGE("Surface reports no formats");
                    return false;
                }
                // Both HDR and SDR ten-bit pairs must exist so host HDR-off transitions preserve precision.
                const VkFormat candidates[] = { VK_FORMAT_A2B10G10R10_UNORM_PACK32,
                    VK_FORMAT_A2R10G10B10_UNORM_PACK32, VK_FORMAT_R8G8B8A8_UNORM, VK_FORMAT_B8G8R8A8_UNORM };
                for (auto candidate : candidates) {
                    bool isTen = candidate == VK_FORMAT_A2B10G10R10_UNORM_PACK32 ||
                                 candidate == VK_FORMAT_A2R10G10B10_UNORM_PACK32;
                    if (tenBit != isTen) continue;
                    bool sdrPair = false, hdrPair = false;
                    for (const auto &format : formats) {
                        if (format.format != candidate) continue;
                        sdrPair |= format.colorSpace == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
                        hdrPair |= format.colorSpace == VK_COLOR_SPACE_HDR10_ST2084_EXT;
                    }
                    if (sdrPair && (!tenBit || hdrPair)) {
                        swapchainFormat = candidate;
                        break;
                    }
                }
                if (swapchainFormat == VK_FORMAT_UNDEFINED) return false;

                uint32_t modeCount = 0;
                if (!check(vk.GetPhysicalDeviceSurfacePresentModesKHR(physicalDevice, surface, &modeCount, nullptr), "present mode count")) return false;
                std::vector<VkPresentModeKHR> modes(modeCount);
                if (!check(vk.GetPhysicalDeviceSurfacePresentModesKHR(physicalDevice, surface, &modeCount, modes.data()), "present modes")) return false;
                modes.resize(modeCount);
                if (fifoLatestReadyFeatures.presentModeFifoLatestReady &&
                    std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_FIFO_LATEST_READY_EXT) != modes.end()) {
                    presentMode = VK_PRESENT_MODE_FIFO_LATEST_READY_EXT;
                } else if (std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_MAILBOX_KHR) != modes.end()) {
                    // The newest frame replaces a queued one instead of waiting behind it. With
                    // FIFO a stream at the display's own rate keeps the queue full, a constant
                    // extra frame or two of delay.
                    presentMode = VK_PRESENT_MODE_MAILBOX_KHR;
                } else {
                    presentMode = VK_PRESENT_MODE_FIFO_KHR;
                }
            }

            if (!(caps.supportedUsageFlags & VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT)) return false;
            swapchainColorSpace = hdr ? VK_COLOR_SPACE_HDR10_ST2084_EXT : VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
            uint32_t imageCount = caps.minImageCount + 1;
            if (caps.maxImageCount > 0) {
                imageCount = std::min(imageCount, caps.maxImageCount);
            }

            VkSwapchainCreateInfoKHR info = {VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
            info.surface = surface;
            info.minImageCount = imageCount;
            info.imageFormat = swapchainFormat;
            info.imageColorSpace = swapchainColorSpace;
            info.imageExtent = caps.currentExtent;
            if (info.imageExtent.width == UINT32_MAX) {
                info.imageExtent.width = std::clamp(width, caps.minImageExtent.width, caps.maxImageExtent.width);
                info.imageExtent.height = std::clamp(height, caps.minImageExtent.height, caps.maxImageExtent.height);
            }
            info.imageArrayLayers = 1;
            info.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
            info.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
            // The shader draws unrotated; let the compositor rotate like it does for MediaCodec.
            info.preTransform = (caps.supportedTransforms & VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR) ?
                VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR : caps.currentTransform;
            info.compositeAlpha = (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR) ?
                VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR : VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
            info.presentMode = presentMode;
            info.clipped = VK_TRUE;
            info.oldSwapchain = swapchain;

            VkSwapchainKHR newSwapchain = VK_NULL_HANDLE;
            VkResult created = vk.CreateSwapchainKHR(device, &info, nullptr, &newSwapchain);
            if (!check(created, "vkCreateSwapchainKHR")) {
                return false;
            }
            if (swapchain == VK_NULL_HANDLE) {
                LOGI("Present mode: %s (display %.2f Hz, stream %d fps)",
                     presentModeName(presentMode), displayRefreshRateHz, frameRateHz);
            }
            destroySwapchainResources();
            if (swapchain != VK_NULL_HANDLE) {
                pollRenderedFrames();
                {
                    std::lock_guard<std::mutex> lock(eventsMutex);
                    // Destroying the swapchain loses its pending timings, not necessarily its frames.
                    for (const auto &frame : pendingPresents) {
                        renderedFrames.push_back(frame.ptsUs);
                        renderedFrames.push_back(0);
                    }
                }
                pendingPresents.clear();
                vk.DestroySwapchainKHR(device, swapchain, nullptr);
            }
            swapchain = newSwapchain;
            swapchainExtent = info.imageExtent;
            applyHdrMetadata();

            uint32_t count = 0;
            if (!check(vk.GetSwapchainImagesKHR(device, swapchain, &count, nullptr), "swapchain image count") || count == 0) return false;
            swapchainImages.resize(count);
            if (!check(vk.GetSwapchainImagesKHR(device, swapchain, &count, swapchainImages.data()), "swapchain images")) return false;
            swapchainImages.resize(count);
            return renderPass == VK_NULL_HANDLE || createSwapchainResources();
        }

        bool createSwapchainResources() {
            for (auto image : swapchainImages) {
                VkImageViewCreateInfo viewInfo = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
                viewInfo.image = image;
                viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
                viewInfo.format = swapchainFormat;
                viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
                VkImageView view;
                if (!check(vk.CreateImageView(device, &viewInfo, nullptr, &view), "swapchain view")) {
                    return false;
                }
                swapchainViews.push_back(view);

                VkFramebufferCreateInfo fbInfo = {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
                fbInfo.renderPass = renderPass;
                fbInfo.attachmentCount = 1;
                fbInfo.pAttachments = &view;
                fbInfo.width = swapchainExtent.width;
                fbInfo.height = swapchainExtent.height;
                fbInfo.layers = 1;
                VkFramebuffer framebuffer;
                if (!check(vk.CreateFramebuffer(device, &fbInfo, nullptr, &framebuffer), "vkCreateFramebuffer")) {
                    return false;
                }
                framebuffers.push_back(framebuffer);

                // Per image, so a present never waits on a semaphore a later submit re-signals.
                VkSemaphoreCreateInfo semInfo = {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
                VkSemaphore semaphore;
                if (!check(vk.CreateSemaphore(device, &semInfo, nullptr, &semaphore), "vkCreateSemaphore")) {
                    return false;
                }
                renderDone.push_back(semaphore);
            }
            return true;
        }

        void destroySwapchainResources() {
            for (auto framebuffer : framebuffers) {
                vk.DestroyFramebuffer(device, framebuffer, nullptr);
            }
            for (auto view : swapchainViews) {
                vk.DestroyImageView(device, view, nullptr);
            }
            for (auto semaphore : renderDone) {
                vk.DestroySemaphore(device, semaphore, nullptr);
            }
            framebuffers.clear();
            swapchainViews.clear();
            renderDone.clear();
        }

        // Checks at most every SIZE_CHECK_INTERVAL_US whether the surface extent differs
        // from the swapchain's.
        bool surfaceSizeChanged(uint64_t now) {
            constexpr uint64_t SIZE_CHECK_INTERVAL_US = 250'000;
            if (now - lastSizeCheckUs < SIZE_CHECK_INTERVAL_US) {
                return false;
            }
            lastSizeCheckUs = now;
            VkSurfaceCapabilitiesKHR caps;
            if (vk.GetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice, surface, &caps) != VK_SUCCESS) {
                return false;
            }
            return caps.currentExtent.width != swapchainExtent.width ||
                   caps.currentExtent.height != swapchainExtent.height;
        }

        bool recreateSwapchain() {
            {
                // Waiting for the device idle needs every queue held.
                std::lock_guard<std::mutex> lock(queueMutex);
                if (!check(vk.DeviceWaitIdle(device), "wait before recreating swapchain")) return false;
            }
            return createSwapchain();
        }

        VkShaderModule createShader(const uint32_t *code, size_t size) {
            VkShaderModuleCreateInfo info = {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
            info.codeSize = size;
            info.pCode = code;
            VkShaderModule module = VK_NULL_HANDLE;
            check(vk.CreateShaderModule(device, &info, nullptr, &module), "vkCreateShaderModule");
            return module;
        }

        bool createPipeline() {
            VkAttachmentDescription attachment = {};
            attachment.format = swapchainFormat;
            attachment.samples = VK_SAMPLE_COUNT_1_BIT;
            attachment.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;  // Black letterbox bars.
            attachment.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
            attachment.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
            attachment.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
            attachment.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            attachment.finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
            VkAttachmentReference colorRef = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
            VkSubpassDescription subpass = {};
            subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
            subpass.colorAttachmentCount = 1;
            subpass.pColorAttachments = &colorRef;
            // Order the layout transition after the acquire semaphore wait.
            VkSubpassDependency dependency = {};
            dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
            dependency.dstSubpass = 0;
            dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            VkRenderPassCreateInfo rpInfo = {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
            rpInfo.attachmentCount = 1;
            rpInfo.pAttachments = &attachment;
            rpInfo.subpassCount = 1;
            rpInfo.pSubpasses = &subpass;
            rpInfo.dependencyCount = 1;
            rpInfo.pDependencies = &dependency;
            if (!check(vk.CreateRenderPass(device, &rpInfo, nullptr, &renderPass), "vkCreateRenderPass")) {
                return false;
            }

            VkSamplerCreateInfo samplerInfo = {VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
            samplerInfo.magFilter = VK_FILTER_LINEAR;
            samplerInfo.minFilter = VK_FILTER_LINEAR;
            samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;
            samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
            samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
            samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
            samplerInfo.maxLod = 0.0f;
            if (!check(vk.CreateSampler(device, &samplerInfo, nullptr, &sampler), "vkCreateSampler")) {
                return false;
            }

            VkDescriptorSetLayoutBinding bindings[3] = {};
            for (uint32_t i = 0; i < 3; ++i) {
                bindings[i].binding = i;
                bindings[i].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
                bindings[i].descriptorCount = 1;
                bindings[i].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
            }
            VkDescriptorSetLayoutCreateInfo layoutInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
            layoutInfo.bindingCount = 3;
            layoutInfo.pBindings = bindings;
            if (!check(vk.CreateDescriptorSetLayout(device, &layoutInfo, nullptr, &setLayout), "vkCreateDescriptorSetLayout")) {
                return false;
            }
            VkPipelineLayoutCreateInfo plInfo = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
            plInfo.setLayoutCount = 1;
            plInfo.pSetLayouts = &setLayout;
            VkPushConstantRange colorRange = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, 3 * sizeof(int32_t)};
            plInfo.pushConstantRangeCount = 1;
            plInfo.pPushConstantRanges = &colorRange;
            if (!check(vk.CreatePipelineLayout(device, &plInfo, nullptr, &pipelineLayout), "vkCreatePipelineLayout")) {
                return false;
            }

            VkDescriptorPoolSize poolSize = {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, 3 * SLOT_COUNT};
            VkDescriptorPoolCreateInfo poolInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
            poolInfo.maxSets = SLOT_COUNT;
            poolInfo.poolSizeCount = 1;
            poolInfo.pPoolSizes = &poolSize;
            if (!check(vk.CreateDescriptorPool(device, &poolInfo, nullptr, &descriptorPool), "vkCreateDescriptorPool")) {
                return false;
            }
            for (auto &slot : slots) {
                VkDescriptorSetAllocateInfo setInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
                setInfo.descriptorPool = descriptorPool;
                setInfo.descriptorSetCount = 1;
                setInfo.pSetLayouts = &setLayout;
                if (!check(vk.AllocateDescriptorSets(device, &setInfo, &slot.descriptorSet), "vkAllocateDescriptorSets")) {
                    return false;
                }
                // The plane images never change, so the descriptors are written once.
                VkDescriptorImageInfo imageInfos[3];
                VkWriteDescriptorSet writes[3] = {};
                for (uint32_t i = 0; i < 3; ++i) {
                    imageInfos[i] = {sampler, slot.planes[i].view, VK_IMAGE_LAYOUT_GENERAL};
                    writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
                    writes[i].dstSet = slot.descriptorSet;
                    writes[i].dstBinding = i;
                    writes[i].descriptorCount = 1;
                    writes[i].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
                    writes[i].pImageInfo = &imageInfos[i];
                }
                vk.UpdateDescriptorSets(device, 3, writes, 0, nullptr);
            }

            pipeline = buildPipeline(planar_csc_frag_spv, sizeof(planar_csc_frag_spv), pipelineLayout, renderPass);
            return pipeline != VK_NULL_HANDLE && createSwapchainResources();
        }

        // A fullscreen-triangle pipeline with dynamic viewport and scissor.
        VkPipeline buildPipeline(const uint32_t *fragCode, size_t fragSize, VkPipelineLayout layout, VkRenderPass pass) {
            VkShaderModule vert = createShader(fullscreen_vert_spv, sizeof(fullscreen_vert_spv));
            VkShaderModule frag = createShader(fragCode, fragSize);
            if (vert == VK_NULL_HANDLE || frag == VK_NULL_HANDLE) {
                if (vert != VK_NULL_HANDLE) vk.DestroyShaderModule(device, vert, nullptr);
                if (frag != VK_NULL_HANDLE) vk.DestroyShaderModule(device, frag, nullptr);
                return VK_NULL_HANDLE;
            }
            VkPipelineShaderStageCreateInfo stages[2] = {};
            stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
            stages[0].module = vert;
            stages[0].pName = "main";
            stages[1].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
            stages[1].module = frag;
            stages[1].pName = "main";

            VkPipelineVertexInputStateCreateInfo vertexInput = {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
            VkPipelineInputAssemblyStateCreateInfo inputAssembly = {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
            inputAssembly.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
            VkPipelineViewportStateCreateInfo viewportState = {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
            viewportState.viewportCount = 1;
            viewportState.scissorCount = 1;
            VkPipelineRasterizationStateCreateInfo raster = {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
            raster.polygonMode = VK_POLYGON_MODE_FILL;
            raster.cullMode = VK_CULL_MODE_NONE;
            raster.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
            raster.lineWidth = 1.0f;
            VkPipelineMultisampleStateCreateInfo multisample = {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
            multisample.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
            VkPipelineColorBlendAttachmentState blendAttachment = {};
            blendAttachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                             VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
            VkPipelineColorBlendStateCreateInfo blend = {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
            blend.attachmentCount = 1;
            blend.pAttachments = &blendAttachment;
            const VkDynamicState dynamicStates[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
            VkPipelineDynamicStateCreateInfo dynamic = {VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO};
            dynamic.dynamicStateCount = 2;
            dynamic.pDynamicStates = dynamicStates;

            VkGraphicsPipelineCreateInfo pipelineInfo = {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
            pipelineInfo.stageCount = 2;
            pipelineInfo.pStages = stages;
            pipelineInfo.pVertexInputState = &vertexInput;
            pipelineInfo.pInputAssemblyState = &inputAssembly;
            pipelineInfo.pViewportState = &viewportState;
            pipelineInfo.pRasterizationState = &raster;
            pipelineInfo.pMultisampleState = &multisample;
            pipelineInfo.pColorBlendState = &blend;
            pipelineInfo.pDynamicState = &dynamic;
            pipelineInfo.layout = layout;
            pipelineInfo.renderPass = pass;
            VkPipeline created = VK_NULL_HANDLE;
            check(vk.CreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr, &created),
                  "vkCreateGraphicsPipelines");
            vk.DestroyShaderModule(device, vert, nullptr);
            vk.DestroyShaderModule(device, frag, nullptr);
            return created;
        }

        // Upscaling is optional: any failure leaves the direct CSC path in place.
        bool createUpscaler() {
            if (upscaleMode != UPSCALE_OFF && !createUpscalerResources()) {
                LOGW("SGSR setup failed; presenting directly");
                upscaleMode = UPSCALE_OFF;
            }
            return true;
        }

        // SGSR reads the YCbCr planes through the same descriptor set as the direct path and
        // converts colour itself, so upscaling costs one pass with no intermediate image.
        bool createUpscalerResources() {
            VkPushConstantRange range = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(SgsrParams)};
            VkPipelineLayoutCreateInfo plInfo = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
            plInfo.setLayoutCount = 1;
            plInfo.pSetLayouts = &setLayout;
            plInfo.pushConstantRangeCount = 1;
            plInfo.pPushConstantRanges = &range;
            if (!check(vk.CreatePipelineLayout(device, &plInfo, nullptr, &upscaleLayout), "vkCreatePipelineLayout(sgsr)")) return false;
            upscalePipeline = buildPipeline(sgsr_frag_spv, sizeof(sgsr_frag_spv), upscaleLayout, renderPass);
            LOGI("SGSR upscaling %s", upscalePipeline != VK_NULL_HANDLE ? "ready" : "failed");
            return upscalePipeline != VK_NULL_HANDLE;
        }

        bool createFrameResources() {
            VkCommandPoolCreateInfo poolInfo = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
            poolInfo.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
            poolInfo.queueFamilyIndex = queueFamily;
            if (!check(vk.CreateCommandPool(device, &poolInfo, nullptr, &commandPool), "vkCreateCommandPool")) {
                return false;
            }
            VkCommandBufferAllocateInfo allocInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
            allocInfo.commandPool = commandPool;
            allocInfo.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
            allocInfo.commandBufferCount = 1;
            if (!check(vk.AllocateCommandBuffers(device, &allocInfo, &decodeCommandBuffer), "vkAllocateCommandBuffers(decode)")) {
                return false;
            }
            // Command pools must not be accessed concurrently by the decode and presenter threads.
            if (!check(vk.CreateCommandPool(device, &poolInfo, nullptr, &drawCommandPool), "vkCreateCommandPool(draw)")) {
                return false;
            }
            allocInfo.commandPool = drawCommandPool;
            allocInfo.commandBufferCount = SLOT_COUNT;
            VkCommandBuffer buffers[SLOT_COUNT];
            if (!check(vk.AllocateCommandBuffers(device, &allocInfo, buffers), "vkAllocateCommandBuffers")) {
                return false;
            }
            for (int i = 0; i < SLOT_COUNT; ++i) slots[i].drawCommandBuffer = buffers[i];
            if (timestampsSupported) {
                VkQueryPoolCreateInfo queryInfo = {VK_STRUCTURE_TYPE_QUERY_POOL_CREATE_INFO};
                queryInfo.queryType = VK_QUERY_TYPE_TIMESTAMP;
                queryInfo.queryCount = QUERY_COUNT;
                if (vk.CreateQueryPool(device, &queryInfo, nullptr, &queryPool) != VK_SUCCESS) {
                    queryPool = VK_NULL_HANDLE;
                }
            }
            LOGI("Present mode %s, GPU timestamps %s", presentModeName(presentMode),
                 queryPool != VK_NULL_HANDLE ? "on" : "off");
            VkFenceCreateInfo fenceInfo = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
            fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
            for (auto &slot : slots) {
                if (!check(vk.CreateFence(device, &fenceInfo, nullptr, &slot.drawFence), "vkCreateFence(draw)")) return false;
            }
            // Acquire needs an unsignaled fence.
            VkFenceCreateInfo acquireInfo = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
            return check(vk.CreateFence(device, &fenceInfo, nullptr, &decodeFence), "vkCreateFence(decode)") &&
                   check(vk.CreateFence(device, &acquireInfo, nullptr, &acquireFence), "vkCreateFence(acquire)");
        }

        bool pushFrame(const uint8_t *data, size_t length) {
            auto push = [this](const uint8_t *packet, size_t size) {
                return pyrowave_decoder_push_packet(decoder, packet, size) == PYROWAVE_SUCCESS;
            };
            if (length >= 4 && pyroReadLe32(data) == 0) return records.pushFrame(data, length, push);
            records.lossPercent = 0;
            return pushPyroWaveFrame(data, length, push);
        }

        // Where the decoder writes the planes: compute storage writes, or colour
        // attachment writes for the fragment path.
        VkPipelineStageFlags decodeWriteStage() const {
            return fragmentPath ? VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT : VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT;
        }

        VkAccessFlags decodeWriteAccess() const {
            return fragmentPath ? VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT : VK_ACCESS_SHADER_WRITE_BIT;
        }

        void planeBarrier(VkCommandBuffer cmd, const Slot &slot, VkPipelineStageFlags srcStage, VkAccessFlags srcAccess,
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

        // Waits for the submitted decode and records when it finished.
        bool finishDecode() {
            if (!check(vk.WaitForFences(device, 1, &decodeFence, VK_TRUE, FENCE_TIMEOUT_NS), "decode fence")) return false;
            timespec completed;
            clock_gettime(CLOCK_MONOTONIC, &completed);
            completedDecodeNs = uint64_t(completed.tv_sec) * 1000000000ULL + completed.tv_nsec;
            queriesPending = queryPool != VK_NULL_HANDLE;
            readTimestamps();
            return true;
        }

        // A plane set neither waiting for nor being drawn by the present thread. The one drawn
        // last is avoided when possible, so its draw has the most time to finish.
        int pickFreeSlot() {
            int fallback = -1;
            for (int i = 1; i <= SLOT_COUNT; ++i) {
                const int candidate = (lastDecodedSlot + i) % SLOT_COUNT;
                if (candidate == readySlot || candidate == drawingSlot) continue;
                if (candidate != lastDrawnSlot) return candidate;
                fallback = candidate;
            }
            return fallback;
        }

        // Decoding and display run on separate threads. This (the stream's decode thread)
        // decodes each frame into a free plane set and hands it to the present thread, which
        // waits for the display on the CPU. Nothing on the GPU queue ever waits for the display,
        // so a frame's decode starts the moment it arrives instead of queueing behind the previous
        // frame's trip to the screen.
        bool decodeFrame(bool display, int64_t ptsUs = 0) {
            const uint64_t frameStart = nowUs();
            int index;
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                index = pickFreeSlot();
            }
            if (index < 0) return false;
            Slot &slot = slots[index];
            // The last draw that sampled these planes. Draws never wait for the display, so this
            // is at most a short GPU wait.
            if (!check(vk.WaitForFences(device, 1, &slot.drawFence, VK_TRUE, FENCE_TIMEOUT_NS), "draw fence")) {
                return false;
            }
            const uint64_t afterFence = nowUs();

            if (!check(vk.ResetCommandBuffer(decodeCommandBuffer, 0), "reset decode command buffer")) return false;
            VkCommandBufferBeginInfo beginInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            if (!check(vk.BeginCommandBuffer(decodeCommandBuffer, &beginInfo), "begin decode command buffer")) return false;
            if (queryPool != VK_NULL_HANDLE) {
                vk.CmdResetQueryPool(decodeCommandBuffer, queryPool, 0, QUERY_COUNT);
                vk.CmdWriteTimestamp(decodeCommandBuffer, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, queryPool, 0);
            }
            planeBarrier(decodeCommandBuffer, slot, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT, 0, decodeWriteStage(),
                         decodeWriteAccess(), slot.initialized ? VK_IMAGE_LAYOUT_GENERAL : VK_IMAGE_LAYOUT_UNDEFINED);

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
            pyrowave_device_set_command_buffer(pyroDevice, decodeCommandBuffer);
            const auto decoded = pyrowave_decoder_decode_gpu_buffer(decoder, nullptr, nullptr, &buffers);
            pyrowave_device_set_command_buffer(pyroDevice, VK_NULL_HANDLE);
            if (decoded != PYROWAVE_SUCCESS) {
                LOGE("pyrowave_decoder_decode_gpu_buffer failed: %d", decoded);
                vk.EndCommandBuffer(decodeCommandBuffer);
                return false;
            }
            slot.initialized = true;
            if (queryPool != VK_NULL_HANDLE) {
                vk.CmdWriteTimestamp(decodeCommandBuffer, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, queryPool, 1);
            }
            if (!check(vk.EndCommandBuffer(decodeCommandBuffer), "vkEndCommandBuffer(decode)")) {
                return false;
            }
            VkSubmitInfo decodeSubmit = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
            decodeSubmit.commandBufferCount = 1;
            decodeSubmit.pCommandBuffers = &decodeCommandBuffer;
            if (!check(vk.ResetFences(device, 1, &decodeFence), "reset decode fence")) return false;
            {
                std::lock_guard<std::mutex> lock(queueMutex);
                if (!check(vk.QueueSubmit(queue, 1, &decodeSubmit, decodeFence), "vkQueueSubmit(decode)")) return false;
            }

            // Handed over as soon as the decode is queued: the draw is submitted after it on the
            // same queue, so the GPU orders the two, and the present thread can wait for a
            // swapchain image while the decode runs.
            if (display) {
                std::lock_guard<std::mutex> lock(slotMutex);
                if (readySlot >= 0) {
                    // The display has not taken the previous frame yet; the newer one replaces it.
                    pushRelease(slots[readySlot].ptsUs, 0, false);
                    decodeStats.replaced++;
                }
                slot.ptsUs = ptsUs;
                readySlot = index;
                slotReady.notify_one();
            }
            lastDecodedSlot = index;

            // Waiting here keeps the decode command buffer, the timestamp queries and PyroWave's
            // own upload buffers safe to reuse on the next frame.
            if (!finishDecode()) return false;
            if (display) {
                decodeStats.frames++;
                decodeStats.slotWaitUs += afterFence - frameStart;
                logDecodeStatsIfDue(nowUs());
            }
            return true;
        }

        bool startPresenter() {
            presenter = std::thread([this] { presentLoop(); });
            return true;
        }

        void stopPresenterThread() {
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                stopPresenter = true;
            }
            slotReady.notify_all();
            if (presenter.joinable()) presenter.join();
        }

        void presentLoop() {
            // Like a game's render thread: frames must reach the compositor on time.
            setpriority(PRIO_PROCESS, 0, -8);
            while (true) {
                int index;
                {
                    std::unique_lock<std::mutex> lock(slotMutex);
                    const auto woken = [this] { return stopPresenter || readySlot >= 0; };
                    // Display times of shown frames arrive a little after their present; keep
                    // asking for them while the stream is still (as during the latency test).
                    while (!pendingPresents.empty() && !woken()) {
                        if (slotReady.wait_for(lock, std::chrono::milliseconds(2), woken)) break;
                        lock.unlock();
                        pollRenderedFrames();
                        lock.lock();
                    }
                    slotReady.wait(lock, woken);
                    if (stopPresenter) return;
                    index = readySlot;
                    readySlot = -1;
                    drawingSlot = index;
                }
                const bool ok = presentSlot(index);
                {
                    std::lock_guard<std::mutex> lock(slotMutex);
                    drawingSlot = -1;
                }
                if (!ok) {
                    failed = true;
                    return;
                }
            }
        }

        // Runs on the present thread, which owns the swapchain.
        bool presentSlot(int index) {
            if (!applyPendingHdr()) return false;
            uint32_t imageIndex = 0;
            const uint64_t beforeAcquire = nowUs();
            // A fence rather than a semaphore: the CPU waits for the display to free an image, so
            // the GPU queue never holds a draw that blocks the next decode behind it.
            auto acquired = vk.AcquireNextImageKHR(device, swapchain, ACQUIRE_TIMEOUT_NS, VK_NULL_HANDLE, acquireFence, &imageIndex);
            if (acquired == VK_ERROR_OUT_OF_DATE_KHR) {
                dropSlot(index);
                return recreateSwapchain();
            }
            if (acquired == VK_TIMEOUT || acquired == VK_NOT_READY) {
                dropSlot(index);  // The next frame replaces it anyway.
                return true;
            }
            if (acquired != VK_SUCCESS && acquired != VK_SUBOPTIMAL_KHR) {
                dropSlot(index);
                return check(acquired, "vkAcquireNextImageKHR");
            }
            if (!check(vk.WaitForFences(device, 1, &acquireFence, VK_TRUE, FENCE_TIMEOUT_NS), "acquire fence") ||
                !check(vk.ResetFences(device, 1, &acquireFence), "reset acquire fence")) return false;
            const uint64_t afterAcquire = nowUs();

            // Show the newest frame: one decoded while we waited for the image replaces this one.
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                if (readySlot >= 0) {
                    pushRelease(slots[index].ptsUs, 0, false);
                    presentStats.replaced++;
                    index = readySlot;
                    readySlot = -1;
                    drawingSlot = index;
                }
            }
            Slot &slot = slots[index];

            VkCommandBuffer cmd = slot.drawCommandBuffer;
            VkCommandBufferBeginInfo beginInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            if (!check(vk.ResetCommandBuffer(cmd, 0), "reset render command buffer") ||
                !check(vk.BeginCommandBuffer(cmd, &beginInfo), "begin render command buffer")) return false;
            // Barriers order against all earlier work on the queue, which covers the decode
            // that was submitted before this slot was handed over.
            planeBarrier(cmd, slot, decodeWriteStage(), decodeWriteAccess(),
                         VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT, VK_ACCESS_SHADER_READ_BIT, VK_IMAGE_LAYOUT_GENERAL);

            VkClearValue clear = {};
            clear.color = {{0.0f, 0.0f, 0.0f, 1.0f}};
            VkRenderPassBeginInfo rpBegin = {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
            rpBegin.renderPass = renderPass;
            rpBegin.framebuffer = framebuffers[imageIndex];
            rpBegin.renderArea = {{0, 0}, swapchainExtent};
            rpBegin.clearValueCount = 1;
            rpBegin.pClearValues = &clear;
            // Fit the stream into the surface, preserving its aspect ratio.
            const float scale = std::min(float(swapchainExtent.width) / float(width),
                                         float(swapchainExtent.height) / float(height));
            const int32_t color[] = {tenBit, hdr, fullRange};
            // Upscale only when the stream is shown larger than it is and the frame is SDR.
            const bool upscale = upscalePipeline != VK_NULL_HANDLE && !hdr && scale > 1.01f;
            upscaling = upscale;
            vk.CmdBeginRenderPass(cmd, &rpBegin, VK_SUBPASS_CONTENTS_INLINE);

            VkViewport viewport = {};
            viewport.width = float(width) * scale;
            viewport.height = float(height) * scale;
            viewport.x = (float(swapchainExtent.width) - viewport.width) / 2.0f;
            viewport.y = (float(swapchainExtent.height) - viewport.height) / 2.0f;
            viewport.maxDepth = 1.0f;
            VkRect2D scissor = {{0, 0}, swapchainExtent};
            vk.CmdSetViewport(cmd, 0, 1, &viewport);
            vk.CmdSetScissor(cmd, 0, 1, &scissor);
            if (upscale) {
                const SgsrParams params = {{1.0f / float(width), 1.0f / float(height), float(width), float(height)},
                                           edgeSharpness, fullRange};
                vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, upscalePipeline);
                vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, upscaleLayout, 0, 1,
                                         &slot.descriptorSet, 0, nullptr);
                vk.CmdPushConstants(cmd, upscaleLayout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(params), &params);
            } else {
                vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
                vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipelineLayout, 0, 1,
                                         &slot.descriptorSet, 0, nullptr);
                vk.CmdPushConstants(cmd, pipelineLayout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(color), color);
            }
            vk.CmdDraw(cmd, 3, 1, 0, 0);
            vk.CmdEndRenderPass(cmd);
            if (!check(vk.EndCommandBuffer(cmd), "vkEndCommandBuffer")) {
                return false;
            }

            VkSubmitInfo submitInfo = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
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
            VkPresentTimeGOOGLE presentTime = {++nextPresentId, 0};
            VkPresentTimesInfoGOOGLE presentTimes = {VK_STRUCTURE_TYPE_PRESENT_TIMES_INFO_GOOGLE};
            if (displayTimingSupported) {
                presentTimes.swapchainCount = 1;
                presentTimes.pTimes = &presentTime;
                presentInfo.pNext = &presentTimes;
            }
            timespec released;
            VkResult presented = VK_SUCCESS;
            {
                std::lock_guard<std::mutex> lock(queueMutex);
                if (!check(vk.QueueSubmit(queue, 1, &submitInfo, slot.drawFence), "vkQueueSubmit")) return false;
                clock_gettime(CLOCK_MONOTONIC, &released);
                presented = vk.QueuePresentKHR(queue, &presentInfo);
            }
            const int64_t ptsUs = slot.ptsUs;
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                drawingSlot = -1;
                lastDrawnSlot = index;
            }
            const bool shown = presented == VK_SUCCESS || presented == VK_SUBOPTIMAL_KHR;
            pushRelease(ptsUs, shown ? int64_t(released.tv_sec) * 1000000000LL + released.tv_nsec : 0, shown);
            uint64_t frameEnd = nowUs();
            if (displayTimingSupported && shown) {
                if (pendingPresents.size() == 8192) pendingPresents.pop_front();
                pendingPresents.push_back({presentTime.presentID, ptsUs, frameEnd});
            }
            pollRenderedFrames();

            presentStats.frames++;
            presentStats.acquireWaitUs += afterAcquire - beforeAcquire;
            logPresentStatsIfDue(frameEnd);

            if (presented == VK_ERROR_OUT_OF_DATE_KHR) {
                return recreateSwapchain();
            }
            // SUBOPTIMAL is reported on every present while the display is rotated, because
            // the swapchain leaves rotation to the compositor (IDENTITY pre-transform). Only
            // a changed surface size needs a new swapchain; recreating per frame stalls the GPU.
            if ((presented == VK_SUBOPTIMAL_KHR || acquired == VK_SUBOPTIMAL_KHR) &&
                surfaceSizeChanged(frameEnd)) {
                return recreateSwapchain();
            }
            return shown || check(presented, "vkQueuePresentKHR");
        }

        void dropSlot(int index) {
            std::lock_guard<std::mutex> lock(slotMutex);
            pushRelease(slots[index].ptsUs, 0, false);
        }

        // Release events for Java's frame statistics, delivered by nativePollRenderedFrames.
        void pushRelease(int64_t ptsUs, int64_t releaseNs, bool shown) {
            std::lock_guard<std::mutex> lock(eventsMutex);
            if (releasedFrames.size() >= 3 * 8192) return;
            releasedFrames.push_back(ptsUs);
            releasedFrames.push_back(releaseNs);
            releasedFrames.push_back(shown ? 1 : 0);
        }

        // HDR changes come from Java's thread and are applied here, by the swapchain's owner.
        bool applyPendingHdr() {
            bool enabled;
            VkHdrMetadataEXT metadata;
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                if (!hdrPending) return true;
                hdrPending = false;
                enabled = pendingHdr;
                metadata = pendingHdrMetadata;
            }
            const bool changed = hdr != enabled;
            hdr = enabled;
            hdrMetadata = metadata;
            if (changed) return recreateSwapchain();
            applyHdrMetadata();
            return true;
        }

        void destroy() {
            stopPresenterThread();
            if (device != VK_NULL_HANDLE) {
                vk.DeviceWaitIdle(device);
            }
            // PyroWave objects before the VkDevice they borrow.
            if (decoder != nullptr) {
                pyrowave_decoder_destroy(decoder);
                decoder = nullptr;
            }
            if (pyroDevice != nullptr) {
                pyrowave_device_destroy(pyroDevice);
                pyroDevice = nullptr;
            }
            if (device != VK_NULL_HANDLE) {
                destroySwapchainResources();
                if (swapchain != VK_NULL_HANDLE) vk.DestroySwapchainKHR(device, swapchain, nullptr);
                if (acquireFence != VK_NULL_HANDLE) vk.DestroyFence(device, acquireFence, nullptr);
                if (decodeFence != VK_NULL_HANDLE) vk.DestroyFence(device, decodeFence, nullptr);
                if (queryPool != VK_NULL_HANDLE) vk.DestroyQueryPool(device, queryPool, nullptr);
                if (commandPool != VK_NULL_HANDLE) vk.DestroyCommandPool(device, commandPool, nullptr);
                if (drawCommandPool != VK_NULL_HANDLE) vk.DestroyCommandPool(device, drawCommandPool, nullptr);
                if (pipeline != VK_NULL_HANDLE) vk.DestroyPipeline(device, pipeline, nullptr);
                if (upscalePipeline != VK_NULL_HANDLE) vk.DestroyPipeline(device, upscalePipeline, nullptr);
                if (upscaleLayout != VK_NULL_HANDLE) vk.DestroyPipelineLayout(device, upscaleLayout, nullptr);
                if (descriptorPool != VK_NULL_HANDLE) vk.DestroyDescriptorPool(device, descriptorPool, nullptr);
                if (pipelineLayout != VK_NULL_HANDLE) vk.DestroyPipelineLayout(device, pipelineLayout, nullptr);
                if (setLayout != VK_NULL_HANDLE) vk.DestroyDescriptorSetLayout(device, setLayout, nullptr);
                if (sampler != VK_NULL_HANDLE) vk.DestroySampler(device, sampler, nullptr);
                if (renderPass != VK_NULL_HANDLE) vk.DestroyRenderPass(device, renderPass, nullptr);
                for (auto &slot : slots) {
                    if (slot.drawFence != VK_NULL_HANDLE) vk.DestroyFence(device, slot.drawFence, nullptr);
                    for (auto &plane : slot.planes) {
                        if (plane.view != VK_NULL_HANDLE) vk.DestroyImageView(device, plane.view, nullptr);
                        if (plane.image != VK_NULL_HANDLE) vk.DestroyImage(device, plane.image, nullptr);
                        if (plane.memory != VK_NULL_HANDLE) vk.FreeMemory(device, plane.memory, nullptr);
                    }
                }
                vk.DestroyDevice(device, nullptr);
                device = VK_NULL_HANDLE;
            }
            if (instance != VK_NULL_HANDLE) {
                if (surface != VK_NULL_HANDLE) vk.DestroySurfaceKHR(instance, surface, nullptr);
                vk.DestroyInstance(instance, nullptr);
                instance = VK_NULL_HANDLE;
            }
            if (window != nullptr) {
                ANativeWindow_release(window);
                window = nullptr;
            }
        }

        VulkanLoader vk;
        ANativeWindow *window = nullptr;
        uint32_t width = 0;
        uint32_t height = 0;
        bool chroma444 = false;
        bool tenBit = false;
        bool hdr = false;
        bool fullRange = false;
        std::atomic<bool> failed{false};
        VkHdrMetadataEXT hdrMetadata = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
        std::vector<const char *> instanceExtensions, deviceExtensions;

        // Kept alive for PyroWave, which reads the create infos after device creation.
        VkApplicationInfo appInfo = {};
        VkInstanceCreateInfo instanceInfo = {};
        float queuePriority = 1.0f;
        VkDeviceQueueCreateInfo queueInfo = {};
        VkPhysicalDeviceVulkan13Features features13 = {};
        VkPhysicalDevicePresentModeFifoLatestReadyFeaturesEXT fifoLatestReadyFeatures = {};
        VkPhysicalDeviceVulkan12Features features12 = {};
        VkPhysicalDeviceVulkan11Features features11 = {};
        VkPhysicalDeviceFeatures2 features2 = {};
        VkDeviceCreateInfo deviceInfo = {};

        VkInstance instance = VK_NULL_HANDLE;
        VkSurfaceKHR surface = VK_NULL_HANDLE;
        VkPhysicalDevice physicalDevice = VK_NULL_HANDLE;
        uint32_t queueFamily = 0;
        VkDevice device = VK_NULL_HANDLE;
        VkQueue queue = VK_NULL_HANDLE;

        pyrowave_device pyroDevice = nullptr;
        pyrowave_decoder decoder = nullptr;
        // Plane sets decoded into in turn, so decoding never waits for a frame on its way to the screen.
        static constexpr int SLOT_COUNT = 3;
        Slot slots[SLOT_COUNT];
        int lastDecodedSlot = SLOT_COUNT - 1;

        // Guards the hand-over between the decode and present threads, and pending HDR changes.
        std::mutex slotMutex;
        std::condition_variable slotReady;
        int readySlot = -1;
        int drawingSlot = -1;
        int lastDrawnSlot = -1;
        uint32_t replacedSinceReport = 0;
        bool stopPresenter = false;
        bool hdrPending = false;
        bool pendingHdr = false;
        VkHdrMetadataEXT pendingHdrMetadata = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
        std::thread presenter;
        // Both threads submit to the one queue; PyroWave takes it too for any upload of its own.
        std::mutex queueMutex;
        bool fragmentPath = false;
        uint64_t lastSizeCheckUs = 0;

        VkFormat swapchainFormat = VK_FORMAT_UNDEFINED;
        VkColorSpaceKHR swapchainColorSpace = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
        VkPresentModeKHR presentMode = VK_PRESENT_MODE_FIFO_KHR;
        VkSwapchainKHR swapchain = VK_NULL_HANDLE;
        VkExtent2D swapchainExtent = {};
        std::vector<VkImage> swapchainImages;
        std::vector<VkImageView> swapchainViews;
        std::vector<VkFramebuffer> framebuffers;
        std::vector<VkSemaphore> renderDone;
        uint32_t nextPresentId = 0;
        struct PendingPresent {
            uint32_t id;
            int64_t ptsUs;
            uint64_t queuedUs;
        };
        std::deque<PendingPresent> pendingPresents;

        VkRenderPass renderPass = VK_NULL_HANDLE;
        VkSampler sampler = VK_NULL_HANDLE;
        VkDescriptorSetLayout setLayout = VK_NULL_HANDLE;
        VkPipelineLayout pipelineLayout = VK_NULL_HANDLE;
        VkDescriptorPool descriptorPool = VK_NULL_HANDLE;
        VkPipeline pipeline = VK_NULL_HANDLE;

        struct SgsrParams {
            float viewportInfo[4];
            float edgeSharpness;
            int32_t fullRange;
        };
        int upscaleMode = UPSCALE_OFF;
        float edgeSharpness = 1.5f;
        VkPipelineLayout upscaleLayout = VK_NULL_HANDLE;
        VkPipeline upscalePipeline = VK_NULL_HANDLE;
        VkDeviceQueueGlobalPriorityCreateInfoEXT globalPriority = {};
        bool highPriorityQueue = false;

        VkCommandPool commandPool = VK_NULL_HANDLE;
        VkCommandPool drawCommandPool = VK_NULL_HANDLE;
        VkFence decodeFence = VK_NULL_HANDLE;
        VkFence acquireFence = VK_NULL_HANDLE;

        // Queries bracket GPU decode; Java records the completed fence in the latency CSV.
        static constexpr uint32_t QUERY_COUNT = 2;
        static constexpr uint64_t STATS_INTERVAL_US = 5'000'000;
        VkCommandBuffer decodeCommandBuffer = VK_NULL_HANDLE;
        VkQueryPool queryPool = VK_NULL_HANDLE;
        bool timestampsSupported = false;
        uint32_t timestampValidBits = 0;
        int frameRateHz = 60;
        float displayRefreshRateHz = 0;

        bool queriesPending = false;
        float timestampPeriodNs = 1.0f;

    public:
        const char *getPresentModeName() const { return presentModeName(presentMode); }
        std::string driverDescription;
        // Whether the most recent frame went through SGSR.
        std::atomic<bool> upscaling{false};

        // GPU decode time of the most recently completed frame, or 0 when unknown.
        uint32_t lastGpuDecodeUs = 0;
        uint64_t completedDecodeNs = 0;
        // Averages over the last stats interval, shown in the overlay: the decode thread's wait
        // for a free plane set, the present thread's wait for a swapchain image, and the share of
        // decoded frames a newer one replaced before they reached the screen.
        std::atomic<float> lastWaitFreeSlotMs{-1};
        std::atomic<float> lastWaitImageMs{-1};
        std::atomic<float> lastReplacedPercent{-1};
        bool displayTimingSupported = false;
        // Filled by the present thread, drained by nativePollRenderedFrames.
        std::mutex eventsMutex;
        std::vector<jlong> renderedFrames;   // (ptsUs, presentNs) pairs; 0 when the surface lost timing
        std::vector<jlong> releasedFrames;   // (ptsUs, releaseNs, shown) triples

        // Present thread only: it owns the swapchain.
        void pollRenderedFrames() {
            if (!displayTimingSupported || swapchain == VK_NULL_HANDLE || pendingPresents.empty()) return;
            uint32_t count = 0;
            if (vk.GetPastPresentationTimingGOOGLE(device, swapchain, &count, nullptr) == VK_SUCCESS && count != 0) {
                std::vector<VkPastPresentationTimingGOOGLE> timings(count);
                auto result = vk.GetPastPresentationTimingGOOGLE(device, swapchain, &count, timings.data());
                if (result == VK_SUCCESS || result == VK_INCOMPLETE) {
                    for (uint32_t i = 0; i < count; ++i) {
                        const auto &timing = timings[i];
                        auto frame = std::find_if(pendingPresents.begin(), pendingPresents.end(),
                            [&timing](const auto &entry) { return entry.id == timing.presentID; });
                        if (frame == pendingPresents.end()) continue;
                        // actualPresentTime uses CLOCK_MONOTONIC on Android, like System.nanoTime().
                        if (timing.actualPresentTime != 0) {
                            std::lock_guard<std::mutex> lock(eventsMutex);
                            renderedFrames.push_back(frame->ptsUs);
                            renderedFrames.push_back(jlong(timing.actualPresentTime));
                        }
                        pendingPresents.erase(frame);
                    }
                }
            }
            // Skipped frames may never get a time, even when no further timings arrive.
            const uint64_t now = nowUs();
            while (!pendingPresents.empty() && now - pendingPresents.front().queuedUs > 500'000) {
                pendingPresents.pop_front();
            }
        }

        void setHdrMode(bool enabled, const uint8_t *metadata) {
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
                parsed.maxContentLightLevel = word(20);
                parsed.maxFrameAverageLightLevel = word(22);
            }
            // The present thread applies it before its next frame.
            std::lock_guard<std::mutex> lock(slotMutex);
            pendingHdr = enabled;
            pendingHdrMetadata = parsed;
            hdrPending = true;
        }

        // Hands the queued release and display-time events to the caller.
        void takeEvents(std::vector<jlong> &released, std::vector<jlong> &rendered) {
            std::lock_guard<std::mutex> lock(eventsMutex);
            released.swap(releasedFrames);
            rendered.swap(renderedFrames);
        }

    private:
        void applyHdrMetadata() {
            if (tenBit && vk.SetHdrMetadataEXT && swapchain) {
                vk.SetHdrMetadataEXT(device, 1, &swapchain, &hdrMetadata);
            }
        }

    private:
        // Each thread keeps its own counters.
        struct {
            uint64_t startUs = 0;
            uint32_t frames = 0;
            uint32_t replaced = 0;
            uint64_t gpuDecodeUs = 0;
            uint32_t gpuSamples = 0;
            uint64_t slotWaitUs = 0;
        } decodeStats;
        struct {
            uint64_t startUs = 0;
            uint32_t frames = 0;
            uint32_t replaced = 0;
            uint64_t acquireWaitUs = 0;
        } presentStats;

        void readTimestamps() {
            if (!queriesPending) {
                return;
            }
            queriesPending = false;
            uint64_t ticks[QUERY_COUNT] = {};
            if (vk.GetQueryPoolResults(device, queryPool, 0, QUERY_COUNT, sizeof(ticks), ticks, sizeof(uint64_t),
                                       VK_QUERY_RESULT_64_BIT) != VK_SUCCESS) {
                return;
            }
            const auto toUs = [this](uint64_t delta) { return uint64_t(double(delta) * timestampPeriodNs / 1000.0); };
            uint64_t mask = timestampValidBits == 64 ? UINT64_MAX : (uint64_t(1) << timestampValidBits) - 1;
            lastGpuDecodeUs = uint32_t(toUs((ticks[1] - ticks[0]) & mask));
            decodeStats.gpuDecodeUs += lastGpuDecodeUs;
            decodeStats.gpuSamples++;
        }

        void logDecodeStatsIfDue(uint64_t now) {
            auto &stats = decodeStats;
            if (stats.startUs == 0) {
                stats.startUs = now;
                return;
            }
            if (now - stats.startUs < STATS_INTERVAL_US || stats.frames == 0) {
                return;
            }
            const double seconds = double(now - stats.startUs) / 1e6;
            const double gpuFrames = stats.gpuSamples ? double(stats.gpuSamples) : 1.0;
            const float slotWaitMs = float(stats.slotWaitUs / double(stats.frames) / 1000.0);
            LOGI("%.1f fps decoded: GPU decode %.2f ms, wait for free planes %.2f ms, replaced before display %u",
                 stats.frames / seconds, stats.gpuDecodeUs / gpuFrames / 1000.0, slotWaitMs, stats.replaced);
            lastWaitFreeSlotMs = slotWaitMs;
            const uint32_t decodedFrames = stats.frames;
            const uint32_t replacedHere = stats.replaced;
            stats = {};
            stats.startUs = now;
            {
                // Frames replaced on either thread, as a share of the frames decoded for display.
                std::lock_guard<std::mutex> lock(slotMutex);
                replacedSinceReport += replacedHere;
                lastReplacedPercent = std::min(100.0f, 100.0f * float(replacedSinceReport) / float(decodedFrames));
                replacedSinceReport = 0;
            }

            // PyroWave's own per-pass GPU timings, to see which stage dominates decode.
            pyrowave_device_report_performance_stats(pyroDevice, [](void *, const char *msg) {
                LOGI("PyroWave pass: %s", msg);
            }, nullptr, true);
        }

        void logPresentStatsIfDue(uint64_t now) {
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
            {
                std::lock_guard<std::mutex> lock(slotMutex);
                replacedSinceReport += stats.replaced;
            }
            stats = {};
            stats.startUs = now;
        }
    };

    Readiness probeReadiness(bool tenBit) {
        if (!apiVersionSupported()) {
            return PROBE_FAILED;
        }
        VulkanLoader vk;
        if (!vk.loadGlobal()) {
            return NEEDS_VULKAN_1_3;
        }
        uint32_t loaderVersion = 0;
        if (vk.EnumerateInstanceVersion(&loaderVersion) != VK_SUCCESS || loaderVersion < VK_API_VERSION_1_3) {
            LOGI("Vulkan loader is older than 1.3; PyroWave unavailable");
            return NEEDS_VULKAN_1_3;
        }

        uint32_t extensionCount = 0;
        if (vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, nullptr) != VK_SUCCESS) return PROBE_FAILED;
        std::vector<VkExtensionProperties> extensions(extensionCount);
        if (vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, extensions.data()) != VK_SUCCESS) return PROBE_FAILED;
        extensions.resize(extensionCount);
        for (const char *required : INSTANCE_EXTENSIONS) {
            if (std::none_of(extensions.begin(), extensions.end(), [required](const VkExtensionProperties &ext) {
                return !strcmp(ext.extensionName, required);
            })) return MISSING_SWAPCHAIN;
        }
        if (tenBit && std::none_of(extensions.begin(), extensions.end(), [](const VkExtensionProperties &ext) {
            return !strcmp(ext.extensionName, VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
        })) return MISSING_HDR;

        VkApplicationInfo appInfo = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
        appInfo.apiVersion = VK_API_VERSION_1_3;
        VkInstanceCreateInfo instanceInfo = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
        instanceInfo.pApplicationInfo = &appInfo;
        VkInstance instance = VK_NULL_HANDLE;
        if (vk.CreateInstance(&instanceInfo, nullptr, &instance) != VK_SUCCESS) {
            return PROBE_FAILED;
        }
        // Only the functions the probe needs; surface functions may be absent here.
        vk.EnumeratePhysicalDevices = reinterpret_cast<PFN_vkEnumeratePhysicalDevices>(
            vk.GetInstanceProcAddr(instance, "vkEnumeratePhysicalDevices"));
        vk.GetPhysicalDeviceProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceProperties>(
            vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceProperties"));
        vk.GetPhysicalDeviceProperties2 = reinterpret_cast<PFN_vkGetPhysicalDeviceProperties2>(
            vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceProperties2"));
        vk.GetPhysicalDeviceFeatures2 = reinterpret_cast<PFN_vkGetPhysicalDeviceFeatures2>(
            vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceFeatures2"));
        vk.GetPhysicalDeviceFormatProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceFormatProperties>(
            vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceFormatProperties"));
        vk.GetPhysicalDeviceQueueFamilyProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceQueueFamilyProperties>(
            vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceQueueFamilyProperties"));
        vk.EnumerateDeviceExtensionProperties = reinterpret_cast<PFN_vkEnumerateDeviceExtensionProperties>(
            vk.GetInstanceProcAddr(instance, "vkEnumerateDeviceExtensionProperties"));
        vk.DestroyInstance = reinterpret_cast<PFN_vkDestroyInstance>(
            vk.GetInstanceProcAddr(instance, "vkDestroyInstance"));

        if (!vk.EnumeratePhysicalDevices || !vk.GetPhysicalDeviceProperties ||
            !vk.GetPhysicalDeviceProperties2 || !vk.GetPhysicalDeviceFeatures2 || !vk.DestroyInstance ||
            !vk.GetPhysicalDeviceFormatProperties || !vk.GetPhysicalDeviceQueueFamilyProperties ||
            !vk.EnumerateDeviceExtensionProperties) {
            if (vk.DestroyInstance) vk.DestroyInstance(instance, nullptr);
            return PROBE_FAILED;
        }

        Readiness readiness = PROBE_FAILED;
        uint32_t count = 0;
        if (vk.EnumeratePhysicalDevices(instance, &count, nullptr) == VK_SUCCESS && count > 0) {
            std::vector<VkPhysicalDevice> devices(count);
            if (vk.EnumeratePhysicalDevices(instance, &count, devices.data()) == VK_SUCCESS) {
                devices.resize(count);
                readiness = NEEDS_VULKAN_1_3;
                for (auto device : devices) {
                    auto probe = probeFeatures(vk, device, tenBit);
                    LOGI("%s (Vulkan %u.%u) PyroWave readiness: %d", probe.name, VK_API_VERSION_MAJOR(probe.apiVersion),
                         VK_API_VERSION_MINOR(probe.apiVersion), probe.reason);
                    if (probe.reason == READY) {
                        readiness = READY;
                        break;
                    }
                    readiness = std::max(readiness, probe.reason);
                }
            }
        }
        vk.DestroyInstance(instance, nullptr);
        return readiness;
    }
}  // namespace

extern "C" {

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetReadiness(JNIEnv *, jclass, jboolean tenBit) {
    // The capability is a property of the driver, so probe once per process.
    static std::once_flag once[2];
    static Readiness readiness[2] = {PROBE_FAILED, PROBE_FAILED};
    int index = tenBit ? 1 : 0;
    std::call_once(once[index], [index] { readiness[index] = probeReadiness(index != 0); });
    return readiness[index];
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeCreate(JNIEnv *env, jclass, jobject surface,
                                                                      jint width, jint height, jint frameRate, jfloat displayRefreshRate,
                                                                      jboolean chroma444, jboolean tenBit, jboolean fullRange,
                                                                      jint upscale, jfloat edgeSharpness) {
    if (surface == nullptr || width < 64 || height < 64 || width > 16384 || height > 16384 || (!chroma444 && ((width & 1) || (height & 1)))) {
        LOGE("PyroWave needs a surface and positive dimensions, even for 4:2:0 (%dx%d)", width, height);
        return 0;
    }
    ANativeWindow *window = ANativeWindow_fromSurface(env, surface);
    if (window == nullptr) {
        return 0;
    }
    auto renderer = std::make_unique<Renderer>();
    if (!renderer->create(window, width, height, frameRate, displayRefreshRate, chroma444, tenBit, fullRange,
                          upscale, edgeSharpness)) {
        return 0;  // The renderer releases the window.
    }
    LOGI("PyroWave renderer ready for %dx%d %s", width, height, chroma444 ? "4:4:4" : "4:2:0");
    return reinterpret_cast<jlong>(renderer.release());
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitFrame(JNIEnv *env, jclass, jlong handle,
                                                                           jbyteArray data, jint length, jlong ptsUs) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr || data == nullptr || length <= 0) {
        return SUBMIT_ERROR;
    }
    // Copy out of the Java array so decode and present do not run inside a JNI
    // critical section. Frames arrive on one thread, so the buffer is reused.
    static thread_local std::vector<uint8_t> frame;
    if (length > env->GetArrayLength(data)) {
        return SUBMIT_ERROR;
    }
    frame.resize(size_t(length));
    env->GetByteArrayRegion(data, 0, length, reinterpret_cast<jbyte *>(frame.data()));
    if (env->ExceptionCheck()) return SUBMIT_ERROR;
    int result = renderer->submit(frame.data(), frame.size(), ptsUs);
    return result == SUBMIT_OK ? jlong(renderer->completedDecodeNs) : (result == SUBMIT_ERROR ? -1 : 0);
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativePollRenderedFrames(JNIEnv *env, jclass, jlong handle,
                                                                                 jobject stats) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    const jboolean timing = renderer->displayTimingSupported ? JNI_TRUE : JNI_FALSE;
    // Presentation runs on the renderer's own thread, so releases (and replaced frames) are
    // reported here, ahead of the display times that follow them.
    std::vector<jlong> released, rendered;
    renderer->takeEvents(released, rendered);
    if (released.empty() && rendered.empty()) return timing;
    jclass statsClass = env->GetObjectClass(stats);
    jmethodID onReleased = env->GetMethodID(statsClass, "onOutputReleased", "(IJZZ)V");
    jmethodID onRendered = env->GetMethodID(statsClass, "onFrameRendered", "(JJ)V");
    jmethodID discard = env->GetMethodID(statsClass, "discard", "(JLjava/lang/String;)V");
    env->DeleteLocalRef(statsClass);
    if (onReleased == nullptr || onRendered == nullptr || discard == nullptr) return JNI_FALSE;
    for (size_t i = 0; i + 3 <= released.size(); i += 3) {
        // The output index Java registered for this frame: its presentation time, truncated.
        env->CallVoidMethod(stats, onReleased, jint(released[i]), released[i + 1],
                            released[i + 2] ? JNI_TRUE : JNI_FALSE, timing);
        if (env->ExceptionCheck()) return JNI_FALSE;
    }
    for (size_t i = 0; i + 2 <= rendered.size(); i += 2) {
        if (rendered[i + 1] == 0) {
            jstring reason = env->NewStringUTF("surface_changed");
            if (reason == nullptr) return JNI_FALSE;
            env->CallVoidMethod(stats, discard, rendered[i], reason);
            env->DeleteLocalRef(reason);
        } else {
            env->CallVoidMethod(stats, onRendered, rendered[i], rendered[i + 1]);
        }
        if (env->ExceptionCheck()) return JNI_FALSE;
    }
    return timing;
}

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetLastGpuDecodeUs(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return renderer != nullptr ? jint(renderer->lastGpuDecodeUs) : 0;
}

JNIEXPORT jfloatArray JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetWaits(JNIEnv *env, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    jfloat values[3] = {renderer->lastWaitFreeSlotMs, renderer->lastWaitImageMs, renderer->lastReplacedPercent};
    jfloatArray result = env->NewFloatArray(3);
    if (result != nullptr) env->SetFloatArrayRegion(result, 0, 3, values);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetPresentMode(JNIEnv *env, jclass, jlong handle) {
    return env->NewStringUTF(reinterpret_cast<Renderer *>(handle)->getPresentModeName());
}

JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetDriver(JNIEnv *env, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return env->NewStringUTF(renderer != nullptr ? renderer->driverDescription.c_str() : "");
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeIsUpscaling(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return renderer != nullptr && renderer->upscaling;
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetGpuMaxClocks(JNIEnv *, jclass, jboolean enabled) {
    return setAdrenoMaxClocks(enabled);
}

JNIEXPORT jfloat JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetLastRecordLossPercent(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return renderer != nullptr ? renderer->records.lossPercent : 0;
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetHdrMode(JNIEnv *env, jclass, jlong handle,
                                                                       jboolean enabled, jbyteArray metadata) {
    uint8_t bytes[24];
    bool valid = metadata && env->GetArrayLength(metadata) >= 24;
    if (valid) {
        env->GetByteArrayRegion(metadata, 0, 24, reinterpret_cast<jbyte *>(bytes));
        if (env->ExceptionCheck()) return;
    }
    reinterpret_cast<Renderer *>(handle)->setHdrMode(enabled, valid ? bytes : nullptr);
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeDestroy(JNIEnv *, jclass, jlong handle) {
    delete reinterpret_cast<Renderer *>(handle);
}

}  // extern "C"
