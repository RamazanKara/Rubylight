// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// Nightjar: packet framing, feature gates, HDR/10-bit output and completion timing.
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
#if defined(__aarch64__)
#include <adrenotools/driver.h>
#endif

#include <android/data_space.h>
#include <android/log.h>
#include <android/native_window_jni.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <sys/ioctl.h>
#include <sys/system_properties.h>
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
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "shaders_spv.h"
#include "frame.h"
#include "presentation.h"
#include "schedule.h"
#include "present_timing.h"
#include "rubylight_protocol.h"
#include "../window-dataspace/window_dataspace.h"

static_assert(PYRO_DATASPACE_SRGB == ADATASPACE_SRGB && PYRO_DATASPACE_BT2020_PQ == ADATASPACE_BT2020_PQ &&
              PYRO_DATASPACE_BT2020_HLG == ADATASPACE_BT2020_HLG,
              "window dataspace constants in presentation.h must match the NDK");
static_assert(PYRO_FORMAT_A2B10G10R10 == VK_FORMAT_A2B10G10R10_UNORM_PACK32 &&
              PYRO_FORMAT_A2R10G10B10 == VK_FORMAT_A2R10G10B10_UNORM_PACK32 &&
              PYRO_FORMAT_R8G8B8A8 == VK_FORMAT_R8G8B8A8_UNORM && PYRO_FORMAT_B8G8R8A8 == VK_FORMAT_B8G8R8A8_UNORM &&
              PYRO_COLORSPACE_SRGB_NONLINEAR == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR &&
              PYRO_COLORSPACE_HDR10_ST2084 == VK_COLOR_SPACE_HDR10_ST2084_EXT &&
              PYRO_COLORSPACE_HDR10_HLG == VK_COLOR_SPACE_HDR10_HLG_EXT,
              "surface format constants in presentation.h must match Vulkan");

#define LOG_TAG "PyroWave"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace {
    // Why the last ten-bit output surface failed, for Java's HDR fallback notice.
    // 0 none, 1 no usable ten-bit surface format (Android 13 and 14 loaders may list none),
    // 2 the display or driver refused the HDR window dataspace, 3 the same for an HLG stream,
    // 4 the window took BT2020_HLG but no longer holds it after the first frame.
    std::atomic<int> hdrSurfaceFailure{0};

    // Results returned to Java.
    constexpr int SUBMIT_OK = 1;
    constexpr int SUBMIT_SKIPPED = 0;
    constexpr int SUBMIT_BUSY = 2;
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
    X(GetFenceStatus) \
    X(WaitSemaphores) \
    X(ResetFences) \
    X(CreateSemaphore) \
    X(DestroySemaphore) \
    X(QueueSubmit) \
    X(CreateSwapchainKHR) \
    X(DestroySwapchainKHR) \
    X(GetSwapchainImagesKHR) \
    X(AcquireNextImageKHR) \
    X(QueuePresentKHR)

    struct CustomDriverLibrary {
        void *adrenotools = nullptr;
        void *library = nullptr;
        uint32_t loaded = 0;
    };

    CustomDriverLibrary *customDriverLibrary(const std::string &hooks, const std::string &directory,
                                             const std::string &name) {
#if defined(__aarch64__)
        static std::mutex mutex;
        // Hook namespaces retain their parameters. Keep each immutable driver's loader alive
        // until process exit rather than unloading code another Vulkan instance may still use.
        static std::map<std::string, std::unique_ptr<CustomDriverLibrary>> libraries;
        std::lock_guard<std::mutex> lock(mutex);
        auto &driver = libraries[directory];
        if (!driver) {
            driver = std::make_unique<CustomDriverLibrary>();
            // The opt-in library's namespace resolver must never run for system-driver users.
            driver->adrenotools = dlopen("libadrenotools.so", RTLD_NOW | RTLD_LOCAL);
            auto openDriver = driver->adrenotools == nullptr ? nullptr :
                reinterpret_cast<decltype(&adrenotools_open_libvulkan)>(dlsym(driver->adrenotools, "adrenotools_open_libvulkan"));
            if (openDriver != nullptr) {
                driver->library = openDriver(RTLD_NOW | RTLD_LOCAL, ADRENOTOOLS_DRIVER_CUSTOM,
                        nullptr, hooks.c_str(), directory.c_str(), name.c_str(), nullptr, nullptr, &driver->loaded);
            }
        }
        return driver.get();
#else
        (void) hooks;
        (void) directory;
        (void) name;
        return nullptr;
#endif
    }

    std::string jniString(JNIEnv *env, jstring value) {
        if (value == nullptr) return {};
        const char *characters = env->GetStringUTFChars(value, nullptr);
        if (characters == nullptr) return {};
        std::string result(characters);
        env->ReleaseStringUTFChars(value, characters);
        return result;
    }

    struct VulkanLoader {
        void *library = nullptr;
        CustomDriverLibrary *custom;
        PFN_vkGetInstanceProcAddr GetInstanceProcAddr = nullptr;
        PFN_vkSetHdrMetadataEXT SetHdrMetadataEXT = nullptr;
        PFN_vkGetPastPresentationTimingGOOGLE GetPastPresentationTimingGOOGLE = nullptr;
        PFN_vkGetRefreshCycleDurationGOOGLE GetRefreshCycleDurationGOOGLE = nullptr;
#define X(name) PFN_vk##name name = nullptr;
        VK_GLOBAL_FUNCTIONS(X)
        VK_INSTANCE_FUNCTIONS(X)
        VK_DEVICE_FUNCTIONS(X)
#undef X

        explicit VulkanLoader(CustomDriverLibrary *custom = nullptr) : custom(custom) { }

        ~VulkanLoader() {
            if (library != nullptr && custom == nullptr) {
                dlclose(library);
            }
        }

        bool loadGlobal() {
            library = custom != nullptr ? custom->library : dlopen("libvulkan.so", RTLD_NOW | RTLD_LOCAL);
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

        bool usingRequestedDriver() const {
            return custom == nullptr || __atomic_load_n(&custom->loaded, __ATOMIC_ACQUIRE) != 0;
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
            GetRefreshCycleDurationGOOGLE = reinterpret_cast<PFN_vkGetRefreshCycleDurationGOOGLE>(
                GetDeviceProcAddr(device, "vkGetRefreshCycleDurationGOOGLE"));
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

    int64_t nowNs() {
        timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        return int64_t(ts.tv_sec) * 1'000'000'000LL + ts.tv_nsec;
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
        uint64_t decodeValue = 0;
        int64_t ptsUs = 0;
        int64_t hostPresentationTimeUs = 0;
        int64_t completedNs = 0;
    };

    // Opt-in diagnostics for a physical-device release build. All state belongs to one
    // renderer and only its JNI submission thread updates it. Disabled scopes read no
    // clocks; enabled scopes keep bounded counters and log once per five seconds.
    class SubmissionDiagnostics {
    public:
        enum StageId { COPY, CLOCK, PARSE, SLOT, ORDER, SDK, PUBLISH, STATS, TOTAL, STAGE_COUNT };

        void configure() {
            char value[PROP_VALUE_MAX] = {};
            __system_property_get("debug.nightjar.pyro_submit", value);
            enabled = std::strcmp(value, "1") == 0;
            if (enabled) LOGI("PyroWave submit profile enabled: wall/thread CPU milliseconds; restart stream to change");
        }

        bool isEnabled() const { return enabled; }

    private:
        struct Stamp { uint64_t wall, cpu; };
        struct Timing {
            uint64_t count = 0, wall = 0, cpu = 0, maxWall = 0, maxCpu = 0;
        };
        struct Window {
            uint64_t start = 0, frames = 0, accepted = 0, busy = 0, invalid = 0, errors = 0;
            uint64_t bytes = 0, maxBytes = 0, recordFrames = 0, ordinaryFrames = 0;
            Timing timing[STAGE_COUNT];
        } window;
        bool enabled = false, active = false;

        static Stamp stamp() {
            timespec wall = {}, cpu = {};
            clock_gettime(CLOCK_MONOTONIC, &wall);
            clock_gettime(CLOCK_THREAD_CPUTIME_ID, &cpu);
            return {uint64_t(wall.tv_sec) * 1'000'000'000 + uint64_t(wall.tv_nsec),
                    uint64_t(cpu.tv_sec) * 1'000'000'000 + uint64_t(cpu.tv_nsec)};
        }

        void add(StageId id, Stamp start, Stamp end) {
            auto &timing = window.timing[id];
            const uint64_t wall = end.wall >= start.wall ? end.wall - start.wall : 0;
            const uint64_t cpu = end.cpu >= start.cpu ? end.cpu - start.cpu : 0;
            ++timing.count;
            timing.wall += wall;
            timing.cpu += cpu;
            timing.maxWall = std::max(timing.maxWall, wall);
            timing.maxCpu = std::max(timing.maxCpu, cpu);
        }

        void reportIfDue(uint64_t now) {
            if (now - window.start < 5'000'000'000) return;
            LOGI("PyroWave submit profile: frames=%llu accepted=%llu busy=%llu invalid=%llu errors=%llu "
                 "bytes_avg=%.0f bytes_max=%llu record_frames=%llu ordinary_frames=%llu nice=%d",
                 (unsigned long long)window.frames, (unsigned long long)window.accepted,
                 (unsigned long long)window.busy, (unsigned long long)window.invalid,
                 (unsigned long long)window.errors, double(window.bytes) / double(window.frames),
                 (unsigned long long)window.maxBytes, (unsigned long long)window.recordFrames,
                 (unsigned long long)window.ordinaryFrames, getpriority(PRIO_PROCESS, 0));
            static const char *const names[] = {"copy", "clock", "parse", "slot", "order", "sdk", "publish", "stats", "total"};
            for (int i = 0; i < STAGE_COUNT; ++i) {
                const auto &timing = window.timing[i];
                if (timing.count == 0) continue;
                const double divisor = double(timing.count) * 1e6;
                LOGI("PyroWave submit profile: stage=%s n=%llu wall_avg=%.3f wall_max=%.3f cpu_avg=%.3f cpu_max=%.3f ms",
                     names[i], (unsigned long long)timing.count, timing.wall / divisor,
                     timing.maxWall / 1e6, timing.cpu / divisor, timing.maxCpu / 1e6);
            }
            window = {};
        }

    public:
        class Stage {
        public:
            Stage(SubmissionDiagnostics &owner, StageId id)
                : owner(owner.active ? &owner : nullptr), id(id) {
                if (this->owner) start = stamp();
            }
            ~Stage() { if (owner) owner->add(id, start, stamp()); }
        private:
            SubmissionDiagnostics *owner;
            StageId id;
            Stamp start{};
        };

        class Frame {
        public:
            Frame(SubmissionDiagnostics &owner, size_t bytes)
                : owner(owner.enabled ? &owner : nullptr), bytes(bytes) {
                if (this->owner) {
                    owner.active = true;
                    start = stamp();
                    if (owner.window.start == 0) owner.window.start = start.wall;
                }
            }
            ~Frame() {
                if (!owner) return;
                const Stamp end = stamp();
                owner->add(TOTAL, start, end);
                owner->active = false;
                auto &window = owner->window;
                ++window.frames;
                if (result == SUBMIT_OK) ++window.accepted;
                else if (result == SUBMIT_BUSY) ++window.busy;
                else if (result < 0) ++window.errors;
                else ++window.invalid;
                window.bytes += bytes;
                window.maxBytes = std::max(window.maxBytes, uint64_t(bytes));
                if (framingKnown) ++(recordFraming ? window.recordFrames : window.ordinaryFrames);
                // Total includes the existing SDK statistics work, but excludes these
                // diagnostic log writes. Java's unchanged outer timer still includes both.
                owner->reportIfDue(end.wall);
            }
            void framing(bool records) { framingKnown = true; recordFraming = records; }
            int result = SUBMIT_ERROR;
        private:
            SubmissionDiagnostics *owner;
            size_t bytes;
            Stamp start{};
            bool framingKnown = false, recordFraming = false;
        };
    };

    // Only the presenter updates these counters; configuration precedes worker startup.
    // Samples use existing CLOCK_MONOTONIC timestamps, never feedback arrival time.
    class PresentationDiagnostics {
    public:
        void configure(bool enable) { enabled = enable; }

        void record(int64_t completedNs, int64_t releasedNs, uint64_t actualNs) {
            if (!enabled) return;
            if (completedNs <= 0 || releasedNs < completedNs ||
                actualNs > uint64_t(INT64_MAX) || actualNs < uint64_t(releasedNs)) {
                ++window.invalid;
                return;
            }
            const uint64_t beforeRelease = uint64_t(releasedNs - completedNs);
            const uint64_t afterRelease = actualNs - uint64_t(releasedNs);
            ++window.frames;
            window.beforeReleaseNs += double(beforeRelease);
            window.afterReleaseNs += double(afterRelease);
            window.maxBeforeReleaseNs = std::max(window.maxBeforeReleaseNs, beforeRelease);
            window.maxAfterReleaseNs = std::max(window.maxAfterReleaseNs, afterRelease);
        }

        void reportIfDue(uint64_t nowUs) {
            if (!enabled || (window.frames == 0 && window.invalid == 0)) return;
            if (window.startUs == 0) window.startUs = nowUs;
            if (nowUs < window.startUs || nowUs - window.startUs < 5'000'000) return;
            const double divisor = window.frames == 0 ? 1.0 : double(window.frames) * 1e6;
            LOGI("PyroWave present profile: n=%llu invalid=%llu completion_to_release_avg=%.3f "
                 "completion_to_release_max=%.3f release_to_actual_avg=%.3f release_to_actual_max=%.3f ms",
                 (unsigned long long)window.frames, (unsigned long long)window.invalid,
                 window.beforeReleaseNs / divisor, window.maxBeforeReleaseNs / 1e6,
                 window.afterReleaseNs / divisor, window.maxAfterReleaseNs / 1e6);
            window = {};
            window.startUs = nowUs;
        }

    private:
        struct Window {
            uint64_t startUs = 0, frames = 0, invalid = 0;
            double beforeReleaseNs = 0, afterReleaseNs = 0;
            uint64_t maxBeforeReleaseNs = 0, maxAfterReleaseNs = 0;
        } window;
        bool enabled = false;
    };

    // Presenter-owned, opt-in counters. Inputs are existing timing values; this
    // diagnostic never reads a clock or changes a presentation decision.
    class PacingDiagnostics {
    public:
        void configure(bool enable) { enabled = enable; }
        void record(const PyroWavePresentTiming::Plan &plan, int64_t readyNs,
                    int64_t requestedWaitNs, int64_t periodNs, int64_t leadNs) {
            if (!enabled || !plan.paced) return;
            if (window.frames == 0) {
                window.minPeriodNs = window.maxPeriodNs = periodNs;
                window.minLeadNs = window.maxLeadNs = leadNs;
                window.minAheadNs = window.maxAheadNs = plan.idealNs - readyNs;
            }
            ++window.frames;
            window.minPeriodNs = std::min(window.minPeriodNs, periodNs);
            window.maxPeriodNs = std::max(window.maxPeriodNs, periodNs);
            window.minLeadNs = std::min(window.minLeadNs, leadNs);
            window.maxLeadNs = std::max(window.maxLeadNs, leadNs);
            const int64_t aheadNs = plan.idealNs - readyNs;
            window.minAheadNs = std::min(window.minAheadNs, aheadNs);
            window.maxAheadNs = std::max(window.maxAheadNs, aheadNs);
            window.aheadNs += double(aheadNs);
            window.targetAheadNs += double(plan.targetNs - readyNs);
            const int64_t waitNs = std::max<int64_t>(0, requestedWaitNs);
            window.waitNs += double(waitNs);
            window.maxWaitNs = std::max(window.maxWaitNs, waitNs);
        }
        void reportIfDue(uint64_t nowUs) {
            if (!enabled || window.frames == 0) return;
            if (window.startUs == 0) window.startUs = nowUs;
            if (nowUs < window.startUs || nowUs - window.startUs < 5'000'000) return;
            const double divisor = double(window.frames) * 1e6;
            LOGI("PyroWave pacing profile: n=%llu period_min=%.3f period_max=%.3f "
                 "lead_min=%.3f lead_max=%.3f host_ahead_avg=%.3f host_ahead_min=%.3f host_ahead_max=%.3f "
                 "target_ahead_avg=%.3f requested_wait_avg=%.3f requested_wait_max=%.3f ms",
                 (unsigned long long)window.frames, window.minPeriodNs / 1e6, window.maxPeriodNs / 1e6,
                 window.minLeadNs / 1e6, window.maxLeadNs / 1e6, window.aheadNs / divisor,
                 window.minAheadNs / 1e6, window.maxAheadNs / 1e6, window.targetAheadNs / divisor,
                 window.waitNs / divisor, window.maxWaitNs / 1e6);
            window = {};
            window.startUs = nowUs;
        }
    private:
        struct Window {
            uint64_t startUs = 0, frames = 0;
            int64_t minPeriodNs = 0, maxPeriodNs = 0, minLeadNs = 0, maxLeadNs = 0;
            int64_t minAheadNs = 0, maxAheadNs = 0, maxWaitNs = 0;
            double aheadNs = 0, targetAheadNs = 0, waitNs = 0;
        } window;
        bool enabled = false;
    };

    class Renderer {
    public:
        PyroWaveRecords records;
        SubmissionDiagnostics submissionDiagnostics;
        explicit Renderer(CustomDriverLibrary *custom = nullptr) : vk(custom) { }
        ~Renderer() {
            destroy();
        }

        bool create(ANativeWindow *nativeWindow, int streamWidth, int streamHeight, int frameRate, float displayRefreshRate, int pacingMode,
                    bool fullChroma, bool tenBitOutput, bool fullRangeOutput, bool stretchOutput, int upscale, float sharpness) {
            window = nativeWindow;
            upscaleMode = upscale;
            stretch = stretchOutput;
            edgeSharpness = sharpness;
            chroma444 = fullChroma;
            tenBit = hdr = tenBitOutput;
            if (tenBitOutput) hdrSurfaceFailure = 0;
            fullRange = fullRangeOutput;
            frameRateHz = frameRate > 0 ? frameRate : 60;
            displayRefreshRateHz = displayRefreshRate;
            framePacingMode = pacingMode >= 0 && pacingMode <= 3 ? pacingMode : 0;
            const int64_t periodNs = displayRefreshRate > 1.0f && displayRefreshRate <= 1000.0f ?
                    int64_t(1'000'000'000.0 / displayRefreshRate) : 16'666'667;
            presentTiming.configure(static_cast<PyroWavePresentTiming::Mode>(framePacingMode), periodNs);
            width = uint32_t(streamWidth);
            height = uint32_t(streamHeight);
            records = PyroWaveRecords(width, height, chroma444);
            submissionDiagnostics.configure();
            presentationDiagnostics.configure(submissionDiagnostics.isEnabled());
            pacingDiagnostics.configure(submissionDiagnostics.isEnabled());
            return apiVersionSupported() && createInstanceAndSurface() && createDevice() &&
                   createSwapchain() && checkDecoderLimits() && createDecoder() && createPlanes() &&
                   createPipeline() && createUpscaler() && createFrameResources() && warmUpDecoder() &&
                   startWorkers();
        }

        int submit(const uint8_t *data, size_t length, int64_t ptsUs, int64_t hostPresentationTimeUs,
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
                pyrowave_decoder_clear(decoder);
                if (!pushFrame(data, length)) {
                    pyrowave_decoder_clear(decoder);
                    return SUBMIT_SKIPPED;
                }
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
            appInfo.pApplicationName = "Nightjar";
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
            if (!vk.usingRequestedDriver()) {
                LOGW("Custom Vulkan driver did not load; retry with the system driver");
                return false;
            }
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
                        queueCount = families[i].queueCount >= 2 ? 2 : 1;
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

            queuePriorities[0] = queueCount == 2 ? 0.0f : 1.0f;
            queueInfo = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
            // Like a game's render queue: ask the GPU scheduler to run our work ahead of other apps.
            globalPriority = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_GLOBAL_PRIORITY_CREATE_INFO_EXT};
            globalPriority.globalPriority = VK_QUEUE_GLOBAL_PRIORITY_HIGH_EXT;
            queueInfo.queueFamilyIndex = queueFamily;
            queueInfo.queueCount = queueCount;
            queueInfo.pQueuePriorities = queuePriorities;

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
            if (tenBit && metadata) deviceExtensions.push_back(VK_EXT_HDR_METADATA_EXTENSION_NAME);
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
            if (tenBit && vk.SetHdrMetadataEXT == nullptr) LOGI("HDR metadata unavailable; HDR is signalled without it");
            displayTimingSupported = displayTimingSupported && vk.GetPastPresentationTimingGOOGLE != nullptr;
            LOGI("Actual presentation timestamps: %s", displayTimingSupported ? "VK_GOOGLE_display_timing" : "unavailable");
            // PyroWave's backend uses queue 0. Prefer a separate higher-priority
            // render queue from the same family, so no ownership transfers are needed.
            vk.GetDeviceQueue(device, queueFamily, 0, &decodeQueue);
            vk.GetDeviceQueue(device, queueFamily, queueCount == 2 ? 1 : 0, &queue);
            LOGI("Decode and presentation queues: %s", queueCount == 2 ? "separate" : "shared");
            VkPhysicalDeviceProperties props;
            vk.GetPhysicalDeviceProperties(physicalDevice, &props);
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
            // Compile and execute before advertising this format. Warm-up never
            // enters the completion/event queues or the stream's timing samples.
            const uint32_t sequence[] = {0x80000000u | (width - 1) | ((height - 1) << 14),
                                         uint32_t(chroma444) << 26};
            const int index = reserveDecodeSlot();
            if (index < 0 || pyrowave_decoder_push_packet(decoder, sequence, sizeof(sequence)) != PYROWAVE_SUCCESS ||
                !pyrowave_decoder_decode_is_ready(decoder, false) || !decodeFrame(index, 0) ||
                !waitForDecode(slots[index].decodeValue)) return false;
            schedule.complete(index, false);
            pyrowave_decoder_clear(decoder);
            decodeStats = {};
            return true;
        }

        bool createDecoder() {
            pyrowave_device_create_queue_info pyroQueue = {decodeQueue, queueFamily, 0};
            // Granite discovers extra queues from this create-info and may fetch
            // their handles itself, even if queue_info lists only one. Expose only
            // queue 0 to keep all SDK uploads/retirement under decodeQueueMutex.
            pyroQueueInfo = queueInfo;
            pyroQueueInfo.queueCount = 1;
            pyroDeviceInfo = deviceInfo;
            pyroDeviceInfo.pQueueCreateInfos = &pyroQueueInfo;
            pyrowave_device_create_info info = {};
            info.GetInstanceProcAddr = vk.GetInstanceProcAddr;
            info.instance = instance;
            info.physical_device = physicalDevice;
            info.device = device;
            info.instance_create_info = &instanceInfo;
            info.device_create_info = &pyroDeviceInfo;
            info.queue_info = &pyroQueue;
            info.queue_info_count = 1;
            // Serialize only queues PyroWave can access. On one-queue devices the
            // presenter uses this same mutex; on two-queue devices it has its own.
            info.queue_lock_callback = [](void *self) { static_cast<Renderer *>(self)->decodeQueueMutex.lock(); };
            info.queue_unlock_callback = [](void *self) { static_cast<Renderer *>(self)->decodeQueueMutex.unlock(); };
            info.userdata = this;
            auto result = pyrowave_create_device(&info, &pyroDevice);
            if (result != PYROWAVE_SUCCESS) {
                LOGE("pyrowave_create_device failed: %d", result);
                return false;
            }
            // Mobile GPUs (Adreno, Mali) decode much faster with PyroWave's fragment path,
            // which runs the inverse DWT in render passes instead of compute shaders.
            fragmentPath = pyrowave_decoder_device_prefers_fragment_path(pyroDevice);
            // Queue 0 supports both graphics and compute. Keep every SDK submission
            // on that queue, including its uploads and frame-context retirement.
            pyrowave_device_set_queue_type(pyroDevice, VK_QUEUE_GRAPHICS_BIT);

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
                surfaceFormats.clear();
                for (const auto &format : formats) {
                    surfaceFormats.push_back({int32_t(format.format), int32_t(format.colorSpace)});
                }
                const auto choice = pyroSelectSwapchainFormat(surfaceFormats.data(), surfaceFormats.size(), tenBit, hlg);
                swapchainFormat = static_cast<VkFormat>(choice.format);
                dataspaceOverride = choice.dataspaceOverride;
                if (tenBit && swapchainFormat == VK_FORMAT_UNDEFINED) {
                    hdrSurfaceFailure = 1;
                    LOGE("PyroWave HDR: the surface lists no usable 10-bit format (%zu formats)", surfaceFormats.size());
                }
                if (tenBit && swapchainFormat != VK_FORMAT_UNDEFINED) logHdrOutput();
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
            // The Android 13 and 14 loader lists no HDR colour space; HDR then rides on the window dataspace.
            swapchainColorSpace = hdr && !dataspaceOverride ? static_cast<VkColorSpaceKHR>(pyroHdrColorSpace(hlg)) :
                                  VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
            uint32_t imageCount = caps.minImageCount + 1;
            if (caps.maxImageCount > 0) {
                imageCount = std::min(imageCount, caps.maxImageCount);
            }

            VkSwapchainCreateInfoKHR info = {VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
            info.surface = surface;
            info.minImageCount = imageCount;
            info.imageFormat = swapchainFormat;
            info.imageColorSpace = swapchainColorSpace;
            // Fit in logical display coordinates, then render directly into the display's
            // natural orientation. Android can consume this without a rotation pass.
            const auto transform = pyroPreTransform(caps.currentTransform, caps.supportedTransforms);
            if (transform == 0) {
                LOGE("Surface has no supported rotation or identity transform");
                return false;
            }
            const int32_t rotation = pyroRotationQuarterTurns(transform);
            const auto logicalExtent = surfaceExtent(caps);
            const auto bufferExtent = pyroRotateExtent(logicalExtent, rotation);
            info.imageExtent = {bufferExtent.width, bufferExtent.height};
            info.imageArrayLayers = 1;
            info.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
            info.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
            info.preTransform = static_cast<VkSurfaceTransformFlagBitsKHR>(transform);
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
            displayExtent = logicalExtent;
            rotationQuarterTurns = rotation;
            surfaceTransform = caps.currentTransform;
            lastSizeCheckUs = nowUs();
            presentTiming.reset();
            lastRefreshCheckUs = 0;
            driverRefreshPeriodNs = 0;
            refreshDisplayClock();
            applyHdrMetadata();
            // The loader sets the window dataspace from the colour space on every swapchain creation.
            if (!applyWindowDataspace()) return false;

            uint32_t count = 0;
            if (!check(vk.GetSwapchainImagesKHR(device, swapchain, &count, nullptr), "swapchain image count") || count == 0) return false;
            swapchainImages.resize(count);
            if (!check(vk.GetSwapchainImagesKHR(device, swapchain, &count, swapchainImages.data()), "swapchain images")) return false;
            swapchainImages.resize(count);
            if (submissionDiagnostics.isEnabled()) {
                LOGI("PyroWave swapchain profile: min=%u max=%u requested=%u actual=%u mode=%s "
                     "logical=%ux%u buffer=%ux%u transform=%u format=%u",
                     caps.minImageCount, caps.maxImageCount, imageCount, count, presentModeName(presentMode),
                     displayExtent.width, displayExtent.height, swapchainExtent.width, swapchainExtent.height,
                     uint32_t(info.preTransform), uint32_t(swapchainFormat));
            }
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

        PyroWaveExtent surfaceExtent(const VkSurfaceCapabilitiesKHR &caps) const {
            return {pyroSurfaceDimension(caps.currentExtent.width, ANativeWindow_getWidth(window),
                                         caps.minImageExtent.width, caps.maxImageExtent.width),
                    pyroSurfaceDimension(caps.currentExtent.height, ANativeWindow_getHeight(window),
                                         caps.minImageExtent.height, caps.maxImageExtent.height)};
        }

        // Include same-size rotations (landscape to reverse landscape), which do not
        // necessarily resize the Surface or make presentation report SUBOPTIMAL.
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
            const auto extent = surfaceExtent(caps);
            return caps.currentTransform != surfaceTransform || extent.width != displayExtent.width ||
                   extent.height != displayExtent.height;
        }

        bool recreateSwapchain() {
            {
                // The two mutexes are always distinct. The single-queue fallback
                // uses decodeQueueMutex for both producers.
                std::scoped_lock lock(decodeQueueMutex, renderQueueMutex);
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
            VkPushConstantRange colorRange = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PyroWaveColorParams)};
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
            // SGSR works on display-referred SDR; ten-bit output keeps the direct path.
            if (upscaleMode == UPSCALE_SGSR && !tenBit && !createUpscalerResources()) {
                LOGW("SGSR setup failed; presenting directly");
            }
            return true;
        }

        // SGSR reads the YCbCr planes through the same descriptor set as the direct path and
        // converts colour itself, so upscaling costs one pass with no intermediate image.
        bool createUpscalerResources() {
            VkPushConstantRange range = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PyroWaveSgsrParams)};
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
            VkFenceCreateInfo fenceInfo = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
            fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
            for (auto &slot : slots) {
                if (!check(vk.CreateFence(device, &fenceInfo, nullptr, &slot.drawFence), "vkCreateFence(draw)")) return false;
            }
            VkFenceCreateInfo acquireInfo = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
            if (!check(vk.CreateFence(device, &acquireInfo, nullptr, &acquireFence), "vkCreateFence(acquire)")) return false;
            VkSemaphoreTypeCreateInfo timelineInfo = {VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO};
            timelineInfo.semaphoreType = VK_SEMAPHORE_TYPE_TIMELINE;
            VkSemaphoreCreateInfo semaphoreInfo = {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
            semaphoreInfo.pNext = &timelineInfo;
            if (!check(vk.CreateSemaphore(device, &semaphoreInfo, nullptr, &decodeTimeline), "vkCreateSemaphore(decode)")) return false;

            // CAPI-owned commands expect GENERAL images. Transition all plane sets
            // once, before the workers start, then keep that layout throughout.
            VkCommandBufferBeginInfo beginInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            if (!check(vk.BeginCommandBuffer(decodeCommandBuffer, &beginInfo), "begin plane initialization")) return false;
            for (const auto &slot : slots) {
                planeBarrier(decodeCommandBuffer, slot, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, 0,
                             decodeWriteStage(), decodeWriteAccess(), VK_IMAGE_LAYOUT_UNDEFINED);
            }
            if (!check(vk.EndCommandBuffer(decodeCommandBuffer), "end plane initialization")) return false;
            VkFence initializationFence = VK_NULL_HANDLE;
            if (!check(vk.CreateFence(device, &acquireInfo, nullptr, &initializationFence), "create plane initialization fence")) return false;
            VkSubmitInfo submitInfo = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
            submitInfo.commandBufferCount = 1;
            submitInfo.pCommandBuffers = &decodeCommandBuffer;
            const bool submitted = check(vk.QueueSubmit(decodeQueue, 1, &submitInfo, initializationFence), "initialize plane layouts");
            const bool complete = submitted && check(vk.WaitForFences(device, 1, &initializationFence, VK_TRUE, FENCE_TIMEOUT_NS), "plane initialization fence");
            // A failed wait may still leave work in flight. Teardown waits for the
            // device before destroying this pool; retain the fence there as well.
            if (!complete) {
                startupFence = initializationFence;
                return false;
            }
            vk.DestroyFence(device, initializationFence, nullptr);
            LOGI("Present mode %s, asynchronous decode with up to %d frames in flight",
                 presentModeName(presentMode), PyroWaveSchedule::MAX_IN_FLIGHT);
            return true;
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

        // Called with slotMutex held once workers exist. Never wait for a plane
        // or build a queue of stale input frames behind a busy GPU.
        int reserveDecodeSlot() {
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

        bool waitForDecode(uint64_t value) {
            VkSemaphoreWaitInfo info = {VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO};
            info.semaphoreCount = 1;
            info.pSemaphores = &decodeTimeline;
            info.pValues = &value;
            return check(vk.WaitSemaphores(device, &info, FENCE_TIMEOUT_NS), "decode completion");
        }

        // Only the submission thread enters PyroWave. The SDK owns its command
        // buffers and tracks their staging/query/frame-context lifetimes itself.
        bool decodeFrame(int index, int64_t ptsUs, int64_t hostPresentationTimeUs = 0) {
            Slot &slot = slots[index];
            if (lastDecodeValue != 0) {
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
            release.sync = {decodeTimeline, value};
            pyrowave_result result;
            {
                SubmissionDiagnostics::Stage timing(submissionDiagnostics, SubmissionDiagnostics::SDK);
                result = pyrowave_decoder_decode_gpu_buffer(decoder, nullptr, &release, &buffers);
            }
            if (result != PYROWAVE_SUCCESS) {
                LOGE("pyrowave_decoder_decode_gpu_buffer failed: %d", result);
                return false;
            }
            lastDecodeValue = value;
            slot.decodeValue = value;
            slot.ptsUs = ptsUs;
            slot.hostPresentationTimeUs = hostPresentationTimeUs;
            return true;
        }

        bool startWorkers() {
            completer = std::thread([this] { completionLoop(); });
            presenter = std::thread([this] { presentLoop(); });
            return true;
        }

        void stopWorkers() {
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

        void completionLoop() {
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
                if (!waitForDecode(slots[index].decodeValue)) {
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

        void presentLoop() {
            // Like a game's render thread: frames must reach the compositor on time.
            setpriority(PRIO_PROCESS, 0, -8);
            while (true) {
                int index;
                {
                    std::unique_lock<std::mutex> lock(slotMutex);
                    const auto woken = [this] { return stopPresenter || failed || schedule.ready() >= 0; };
                    // Display times of shown frames arrive a little after their present; keep
                    // asking for them while the stream is still (as during the latency test).
                    while (!pendingPresents.empty() && !woken()) {
                        if (slotReady.wait_for(lock, std::chrono::milliseconds(2), woken)) break;
                        lock.unlock();
                        pollRenderedFrames();
                        lock.lock();
                    }
                    slotReady.wait(lock, woken);
                    if (stopPresenter || failed) return;
                    index = schedule.beginPresent();
                }
                const bool ok = presentSlot(index);
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
        bool presentSlot(int index) {
            if (!applyPendingHdr()) return false;
            pollRenderedFrames();
            refreshDisplayClock();
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

            // Immediate mode keeps the freshest output. Paced modes retain the selected
            // frame while the next completed frame waits, preserving host cadence.
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

            VkCommandBuffer cmd = slot.drawCommandBuffer;
            VkCommandBufferBeginInfo beginInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            if (!check(vk.ResetCommandBuffer(cmd, 0), "reset render command buffer") ||
                !check(vk.BeginCommandBuffer(cmd, &beginInfo), "begin render command buffer")) return false;
            // The timeline wait below makes decoder writes visible across queues.
            // Both queues are in the same family, so no ownership transfer is needed.
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
            const auto output = pyroViewport(width, height, displayExtent.width, displayExtent.height, stretch);
            const auto rotatedOutput = pyroRotateViewport(output,
                    {swapchainExtent.width, swapchainExtent.height}, rotationQuarterTurns);
            const PyroWaveColorParams color = {tenBit, hdr, fullRange, rotationQuarterTurns};
            const auto reason = pyroUpscalingReason(upscaleMode, tenBit || hdr, width, height, output,
                                                    upscalePipeline != VK_NULL_HANDLE);
            upscalingReason = reason;
            const bool upscale = reason == UPSCALING_ACTIVE && upscaleMode == UPSCALE_SGSR;
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
                vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, upscalePipeline);
                vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, upscaleLayout, 0, 1,
                                         &slot.descriptorSet, 0, nullptr);
                vk.CmdPushConstants(cmd, upscaleLayout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(params), &params);
            } else {
                vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
                vk.CmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipelineLayout, 0, 1,
                                         &slot.descriptorSet, 0, nullptr);
                vk.CmdPushConstants(cmd, pipelineLayout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(color), &color);
            }
            vk.CmdDraw(cmd, 3, 1, 0, 0);
            vk.CmdEndRenderPass(cmd);
            if (!check(vk.EndCommandBuffer(cmd), "vkEndCommandBuffer")) {
                return false;
            }

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

            VkTimelineSemaphoreSubmitInfo timelineInfo = {VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO};
            timelineInfo.waitSemaphoreValueCount = 1;
            timelineInfo.pWaitSemaphoreValues = &slot.decodeValue;
            const uint64_t binarySignalValue = 0;
            timelineInfo.signalSemaphoreValueCount = 1;
            timelineInfo.pSignalSemaphoreValues = &binarySignalValue;
            const VkPipelineStageFlags waitStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            VkSubmitInfo submitInfo = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
            submitInfo.pNext = &timelineInfo;
            submitInfo.waitSemaphoreCount = 1;
            submitInfo.pWaitSemaphores = &decodeTimeline;
            submitInfo.pWaitDstStageMask = &waitStage;
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
            pollRenderedFrames();

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
            return shown || check(presented, "vkQueuePresentKHR");
        }

        // Presenter-owned scheduler input. Android's callback samples are copied by
        // the submission thread; driver timing always refers to this output surface.
        void refreshDisplayClock() {
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

        void destroy() {
            stopWorkers();
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
                if (startupFence != VK_NULL_HANDLE) vk.DestroyFence(device, startupFence, nullptr);
                if (decodeTimeline != VK_NULL_HANDLE) vk.DestroySemaphore(device, decodeTimeline, nullptr);
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
        // HDR is HLG, else PQ; owned by the present thread like hdr.
        bool hlg = false;
        bool fullRange = false;
        std::atomic<bool> failed{false};
        VkHdrMetadataEXT hdrMetadata = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
        std::vector<const char *> instanceExtensions, deviceExtensions;

        // Kept alive for PyroWave, which reads the create infos after device creation.
        VkApplicationInfo appInfo = {};
        VkInstanceCreateInfo instanceInfo = {};
        float queuePriorities[2] = {1.0f, 1.0f};
        uint32_t queueCount = 1;
        VkDeviceQueueCreateInfo queueInfo = {};
        VkPhysicalDeviceVulkan13Features features13 = {};
        VkPhysicalDevicePresentModeFifoLatestReadyFeaturesEXT fifoLatestReadyFeatures = {};
        VkPhysicalDeviceVulkan12Features features12 = {};
        VkPhysicalDeviceVulkan11Features features11 = {};
        VkPhysicalDeviceFeatures2 features2 = {};
        VkDeviceCreateInfo deviceInfo = {};
        VkDeviceQueueCreateInfo pyroQueueInfo = {};
        VkDeviceCreateInfo pyroDeviceInfo = {};

        VkInstance instance = VK_NULL_HANDLE;
        VkSurfaceKHR surface = VK_NULL_HANDLE;
        VkPhysicalDevice physicalDevice = VK_NULL_HANDLE;
        uint32_t queueFamily = 0;
        VkDevice device = VK_NULL_HANDLE;
        VkQueue queue = VK_NULL_HANDLE;
        VkQueue decodeQueue = VK_NULL_HANDLE;

        pyrowave_device pyroDevice = nullptr;
        pyrowave_decoder decoder = nullptr;
        // Plane sets decoded into in turn, so decoding never waits for a frame on its way to the screen.
        static constexpr int SLOT_COUNT = PyroWaveSchedule::SLOT_COUNT;
        Slot slots[SLOT_COUNT];
        PyroWaveSchedule schedule;
        PyroWavePresentTiming presentTiming;
        int framePacingMode = 0;
        std::mutex clockMutex;
        int64_t displayClockNs = 0;
        int64_t displayClockIntervalNs = 0;
        uint64_t lastRefreshCheckUs = 0;
        int64_t driverRefreshPeriodNs = 0;
        std::deque<int> pendingDecodes;
        std::condition_variable decodeReady;
        std::thread completer;

        // Guards the hand-over between the decode and present threads, and pending HDR changes.
        std::mutex slotMutex;
        std::condition_variable slotReady;
        uint32_t replacedSinceReport = 0;
        bool stopPresenter = false;
        bool hdrPending = false;
        bool pendingHdr = false;
        bool transferPending = false;
        bool pendingHlg = false;
        VkHdrMetadataEXT pendingHdrMetadata = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
        std::thread presenter;
        std::mutex decodeQueueMutex;
        std::mutex renderQueueMutex;
        bool fragmentPath = false;
        PresentationDiagnostics presentationDiagnostics;
        PacingDiagnostics pacingDiagnostics;
        uint64_t lastSizeCheckUs = 0;

        VkFormat swapchainFormat = VK_FORMAT_UNDEFINED;
        VkColorSpaceKHR swapchainColorSpace = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
        bool dataspaceOverride = false;
        // An HLG window dataspace was set and the next frame shown reads it back.
        bool hlgReadbackPending = false;
        // What the surface listed when the format was chosen; a transfer change keeps the format.
        std::vector<PyroSurfaceFormat> surfaceFormats;
        VkPresentModeKHR presentMode = VK_PRESENT_MODE_FIFO_KHR;
        VkSwapchainKHR swapchain = VK_NULL_HANDLE;
        VkExtent2D swapchainExtent = {};
        PyroWaveExtent displayExtent = {};
        VkSurfaceTransformFlagBitsKHR surfaceTransform = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
        int32_t rotationQuarterTurns = 0;
        std::vector<VkImage> swapchainImages;
        std::vector<VkImageView> swapchainViews;
        std::vector<VkFramebuffer> framebuffers;
        std::vector<VkSemaphore> renderDone;
        uint32_t nextPresentId = 0;
        struct PendingPresent {
            uint32_t id;
            int64_t ptsUs;
            uint64_t queuedUs;
            int64_t completedNs, releasedNs;
        };
        std::deque<PendingPresent> pendingPresents;

        VkRenderPass renderPass = VK_NULL_HANDLE;
        VkSampler sampler = VK_NULL_HANDLE;
        VkDescriptorSetLayout setLayout = VK_NULL_HANDLE;
        VkPipelineLayout pipelineLayout = VK_NULL_HANDLE;
        VkDescriptorPool descriptorPool = VK_NULL_HANDLE;
        VkPipeline pipeline = VK_NULL_HANDLE;

        int upscaleMode = UPSCALE_OFF;
        bool stretch = false;
        float edgeSharpness = 1.5f;
        VkPipelineLayout upscaleLayout = VK_NULL_HANDLE;
        VkPipeline upscalePipeline = VK_NULL_HANDLE;
        VkDeviceQueueGlobalPriorityCreateInfoEXT globalPriority = {};
        bool highPriorityQueue = false;

        VkCommandPool commandPool = VK_NULL_HANDLE;
        VkCommandPool drawCommandPool = VK_NULL_HANDLE;
        VkFence startupFence = VK_NULL_HANDLE;
        VkSemaphore decodeTimeline = VK_NULL_HANDLE;
        uint64_t lastDecodeValue = 0;
        VkFence acquireFence = VK_NULL_HANDLE;

        // This command buffer only initializes plane layouts before workers start.
        // SDK-owned per-pass GPU timings are logged; completion times remain in CSV.
        static constexpr uint64_t STATS_INTERVAL_US = 5'000'000;
        VkCommandBuffer decodeCommandBuffer = VK_NULL_HANDLE;
        int frameRateHz = 60;
        float displayRefreshRateHz = 0;

    public:
        // Java calls this before its final event poll. Destruction repeats the
        // stop safely, after the caller has received all queued completions.
        void drain() {
            stopWorkers();
            pollRenderedFrames();
        }

        const char *getPresentModeName() const { return presentModeName(presentMode); }
        std::string driverDescription;
        std::atomic<PyroWaveUpscalingReason> upscalingReason{UPSCALING_DISABLED};

        // The managed decoder owns its command buffers. An outer timestamp pair
        // would include CPU submission gaps, so report this metric unavailable.
        static constexpr uint32_t lastGpuDecodeUs = 0;
        // Averages over the last stats interval, shown in the overlay: the decode thread's wait
        // for a free plane set, the present thread's wait for a swapchain image, and the share of
        // decoded frames a newer one replaced before they reached the screen.
        std::atomic<float> lastWaitFreeSlotMs{0};
        std::atomic<float> lastWaitImageMs{-1};
        std::atomic<float> lastReplacedPercent{-1};
        bool displayTimingSupported = false;
        // Filled by completion/presentation workers, drained by nativePollRenderedFrames.
        std::mutex eventsMutex;
        std::vector<jlong> completedFrames;  // (ptsUs, completionNs), published before release
        std::vector<jlong> renderedFrames;   // (ptsUs, presentNs) pairs; 0 when the surface lost timing
        std::vector<jlong> releasedFrames;   // (ptsUs, releaseNs, shown) triples
        // JNI polling is serialized by PyroWaveDecoderRenderer. Keep both sides of the swap
        // alive so the presenter and poller reuse their buffers after the first two batches.
        std::vector<jlong> completedBatch;
        std::vector<jlong> renderedBatch;
        std::vector<jlong> releasedBatch;

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
                }
            }
            // Skipped frames may never get a time, even when no further timings arrive.
            const uint64_t now = nowUs();
            presentationDiagnostics.reportIfDue(now);
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

        // The stream's HDR transfer, from the format its ANNOUNCE asked for: HLG, else PQ. Java sets
        // it before the first frame; the present thread recreates the swapchain when it changes.
        void setTransfer(bool hlgTransfer) {
            if (!tenBit || failed) return;
            std::lock_guard<std::mutex> lock(slotMutex);
            pendingHlg = hlgTransfer;
            transferPending = true;
        }

        // One atomic snapshot preserves completion-before-release across JNI polls.
        void takeEvents(std::vector<jlong> &completed, std::vector<jlong> &released, std::vector<jlong> &rendered) {
            completed.clear();
            released.clear();
            rendered.clear();
            std::lock_guard<std::mutex> lock(eventsMutex);
            completed.swap(completedFrames);
            released.swap(releasedFrames);
            rendered.swap(renderedFrames);
        }

    private:
        // The API 28 window dataspace calls; minSdk is 21, so they are looked up at run time. The
        // libraries stay loaded for the life of the process.
        static void *dataspaceSymbol(const char *name) {
            return pyroFindSymbol(name, [](const char *library) { return dlopen(library, RTLD_NOW | RTLD_LOCAL); },
                                  [](void *handle, const char *symbol) { return dlsym(handle, symbol); });
        }

        // False when HDR is on and the window refused it: the stream would look washed out, so it fails loudly.
        // HLG goes through perform where ANativeWindow_setBuffersDataSpace refuses it (window_dataspace.h);
        // PQ and SRGB keep the public call, whose refusal means this window has no HDR at all.
        bool applyWindowDataspace() {
            if (!dataspaceOverride || window == nullptr) return true;
            using SetDataSpace = int32_t (*)(ANativeWindow *, int32_t);
            static const auto setDataSpace = reinterpret_cast<SetDataSpace>(dataspaceSymbol("ANativeWindow_setBuffersDataSpace"));
            const int32_t dataspace = pyroWindowDataspace(hdr, hlg);
            const char *name = !hdr ? "SRGB" : hlg ? "BT2020_HLG" : "BT2020_PQ";
            const nwds::DataspaceOutcome outcome = nwds::setDataspace(
                    window, dataspace, [](void *w, int32_t d) {
                        return setDataSpace != nullptr ? setDataSpace(static_cast<ANativeWindow *>(w), d) : nwds::kMissing;
                    }, hdr && hlg);
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
        void checkHlgWindow() {
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

        void logHdrOutput() {
            if (dataspaceOverride) {
                LOGI("PyroWave HDR: 10-bit SDR swapchain with %s window dataspace (Android <= 14), %s",
                     hlg ? "BT2020_HLG" : "BT2020_PQ", hlg ? "HLG" : "HDR10");
            } else {
                LOGI("PyroWave HDR: native %s swapchain", hlg ? "HLG" : "HDR10");
            }
        }

        // HLG carries no static metadata: the host's mastering data describes PQ.
        void applyHdrMetadata() {
            if (tenBit && !hlg && vk.SetHdrMetadataEXT && swapchain) {
                vk.SetHdrMetadataEXT(device, 1, &swapchain, &hdrMetadata);
            }
        }

    private:
        // Each thread keeps its own counters.
        struct {
            uint64_t startUs = 0;
            uint32_t frames = 0;
        } decodeStats;
        struct {
            uint64_t startUs = 0;
            uint32_t frames = 0;
            uint32_t replaced = 0;
            uint64_t acquireWaitUs = 0;
        } presentStats;

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
            LOGI("%.1f fps submitted for asynchronous decode", stats.frames / seconds);
            const uint32_t decodedFrames = stats.frames;
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

    Readiness probeReadiness(bool tenBit, CustomDriverLibrary *custom) {
        if (!apiVersionSupported()) {
            return PROBE_FAILED;
        }
        VulkanLoader vk(custom);
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
        return vk.usingRequestedDriver() ? readiness : PROBE_FAILED;
    }
}  // namespace

extern "C" {

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetHdrSurfaceFailure(JNIEnv *, jclass) {
    return hdrSurfaceFailure;
}

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetReadiness(JNIEnv *env, jclass, jboolean tenBit,
                                                                            jstring hooks, jstring directory, jstring name) {
    CustomDriverLibrary *custom = directory == nullptr ? nullptr :
        customDriverLibrary(jniString(env, hooks), jniString(env, directory), jniString(env, name));
    if (directory != nullptr && custom == nullptr) return PROBE_FAILED;
    // Selection changes use a different immutable package and receive their own capability probe.
    static std::mutex mutex;
    static std::map<std::pair<CustomDriverLibrary *, bool>, Readiness> results;
    std::lock_guard<std::mutex> lock(mutex);
    auto key = std::make_pair(custom, tenBit != 0);
    auto found = results.find(key);
    if (found == results.end()) found = results.emplace(key, probeReadiness(tenBit != 0, custom)).first;
    return found->second;
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeCreate(JNIEnv *env, jclass, jobject surface,
                                                                      jint width, jint height, jint frameRate, jfloat displayRefreshRate,
                                                                      jint pacingMode,
                                                                      jboolean chroma444, jboolean tenBit, jboolean fullRange, jboolean stretch,
                                                                      jint upscale, jfloat edgeSharpness,
                                                                      jstring hooks, jstring directory, jstring name) {
    if (surface == nullptr || width < 64 || height < 64 || width > 16384 || height > 16384 || (!chroma444 && ((width & 1) || (height & 1)))) {
        LOGE("PyroWave needs a surface and positive dimensions, even for 4:2:0 (%dx%d)", width, height);
        return 0;
    }
    CustomDriverLibrary *custom = directory == nullptr ? nullptr :
        customDriverLibrary(jniString(env, hooks), jniString(env, directory), jniString(env, name));
    if (directory != nullptr && custom == nullptr) return 0;
    ANativeWindow *window = ANativeWindow_fromSurface(env, surface);
    if (window == nullptr) {
        return 0;
    }
    auto renderer = std::make_unique<Renderer>(custom);
    if (!renderer->create(window, width, height, frameRate, displayRefreshRate, pacingMode, chroma444, tenBit, fullRange, stretch,
                          upscale, edgeSharpness)) {
        return 0;  // The renderer releases the window.
    }
    LOGI("PyroWave renderer ready for %dx%d %s", width, height, chroma444 ? "4:4:4" : "4:2:0");
    return reinterpret_cast<jlong>(renderer.release());
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitFrame(JNIEnv *env, jclass, jlong handle,
                                                                           jbyteArray data, jint length, jlong ptsUs,
                                                                           jlong hostPresentationTimeUs,
                                                                           jlong vsyncNs, jlong intervalNs) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return SUBMIT_ERROR;
    SubmissionDiagnostics::Frame timing(renderer->submissionDiagnostics, length > 0 ? size_t(length) : 0);
    if (data == nullptr || length <= 0) return SUBMIT_ERROR;
    // Copy out of the Java array so decode and present do not run inside a JNI
    // critical section. Frames arrive on one thread, so the buffer is reused.
    static thread_local std::vector<uint8_t> frame;
    {
        SubmissionDiagnostics::Stage copy(renderer->submissionDiagnostics, SubmissionDiagnostics::COPY);
        if (length > env->GetArrayLength(data)) return SUBMIT_ERROR;
        frame.resize(size_t(length));
        env->GetByteArrayRegion(data, 0, length, reinterpret_cast<jbyte *>(frame.data()));
        if (env->ExceptionCheck()) return SUBMIT_ERROR;
    }
    timing.framing(length >= 4 && pyroReadLe32(frame.data()) == 0);
    timing.result = renderer->submit(frame.data(), frame.size(), ptsUs, hostPresentationTimeUs, vsyncNs, intervalNs);
    return timing.result;
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativePollRenderedFrames(JNIEnv *env, jclass, jlong handle,
                                                                                 jobject stats) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    const jboolean timing = renderer->displayTimingSupported ? JNI_TRUE : JNI_FALSE;
    // Output completions precede their releases (including replaced frames), which
    // in turn precede display times. All three queues are drained atomically.
    auto &completed = renderer->completedBatch;
    auto &released = renderer->releasedBatch;
    auto &rendered = renderer->renderedBatch;
    renderer->takeEvents(completed, released, rendered);
    if (completed.empty() && released.empty() && rendered.empty()) return timing;
    jclass statsClass = env->GetObjectClass(stats);
    jmethodID onCompleted = env->GetMethodID(statsClass, "onDecoderOutput", "(IJJ)V");
    jmethodID onReleased = env->GetMethodID(statsClass, "onOutputReleased", "(IJZZ)V");
    jmethodID onRendered = env->GetMethodID(statsClass, "onFrameRendered", "(JJ)V");
    jmethodID discard = env->GetMethodID(statsClass, "discard", "(JLjava/lang/String;)V");
    env->DeleteLocalRef(statsClass);
    if (onCompleted == nullptr || onReleased == nullptr || onRendered == nullptr || discard == nullptr) return JNI_FALSE;
    for (size_t i = 0; i + 2 <= completed.size(); i += 2) {
        env->CallVoidMethod(stats, onCompleted, jint(completed[i]), completed[i], completed[i + 1]);
        if (env->ExceptionCheck()) return JNI_FALSE;
    }
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

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetUpscalingReason(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return renderer != nullptr ? renderer->upscalingReason.load() : UPSCALING_DISABLED;
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
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetTransfer(JNIEnv *, jclass, jlong handle,
                                                                       jboolean hlg) {
    reinterpret_cast<Renderer *>(handle)->setTransfer(hlg);
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeDestroy(JNIEnv *env, jclass clazz, jlong handle,
                                                                      jobject stats) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return;
    renderer->drain();
    if (stats != nullptr) {
        Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativePollRenderedFrames(env, clazz, handle, stats);
    }
    delete renderer;
}

}  // extern "C"
