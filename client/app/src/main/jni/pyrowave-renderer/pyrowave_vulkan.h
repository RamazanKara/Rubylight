// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// Nightjar: Vulkan dynamic loader, custom driver handle, feature probe and slot/warp types.
#pragma once

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
#include <cstdio>
#include <cstring>
#include <deque>
#include <iterator>
#include <memory>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "frame.h"
#include "presentation.h"
#include "schedule.h"
#include "present_poll.h"
#include "present_timing.h"
#include "device_path.h"
#include "image_hold.h"
#include "rubylight_protocol.h"
#include "../window-dataspace/window_dataspace.h"
#include "../frame_warp_mask.h"

#define LOG_TAG "PyroWave"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

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
    X(CmdPushConstants)     X(CmdClearColorImage)     X(CreateQueryPool)     X(DestroyQueryPool)     X(CmdResetQueryPool)     X(CmdWriteTimestamp)     X(GetQueryPoolResults) \
    X(CreateFence) \
    X(DestroyFence) \
    X(WaitForFences) \
    X(GetFenceStatus) \
    X(ResetFences) \
    X(CreateSemaphore) \
    X(DestroySemaphore) \
    X(QueueSubmit) \
    X(CreateSwapchainKHR) \
    X(DestroySwapchainKHR) \
    X(GetSwapchainImagesKHR) \
    X(AcquireNextImageKHR) \
    X(QueuePresentKHR)

namespace pyrowave_renderer {

// Why the last ten-bit output surface failed, for Java's HDR fallback notice.
// 0 none, 1 no usable ten-bit surface format (Android 13 and 14 loaders may list none),
// 2 the display or driver refused the HDR window dataspace, 3 the same for an HLG stream,
// 4 the window took BT2020_HLG but no longer holds it after the first frame.
extern std::atomic<int> hdrSurfaceFailure;

// Serialises readiness probes, and with them the first open of each custom driver, which
// writes its CustomDriverLibrary::error.
extern std::mutex readinessMutex;

// Results returned to Java.
constexpr int SUBMIT_OK = 1;
constexpr int SUBMIT_SKIPPED = 0;
constexpr int SUBMIT_BUSY = 2;
constexpr int SUBMIT_ERROR = -1;

// Keep these reason codes aligned with PyroWaveDecoderRenderer.getReadinessSummary().
enum Readiness {
    READY = 0,
    NEEDS_VULKAN = 1,
    MISSING_SUBGROUP_OPS = 2,
    MISSING_DEVICE_FEATURES = 3,
    MISSING_IMAGE_FORMATS = 4,
    INSUFFICIENT_DEVICE_LIMITS = 5,
    MISSING_GRAPHICS_QUEUE = 6,
    MISSING_SWAPCHAIN = 7,
    MISSING_HDR = 8,
    PROBE_FAILED = 9,
    // A custom driver's own failures, before any device check (device_path.h).
    DRIVER_NOT_OPENED = PYRO_DRIVER_NOT_OPENED,
    DRIVER_NOT_LOADED = PYRO_DRIVER_NOT_LOADED,
    DRIVER_NOT_STARTED = PYRO_DRIVER_NOT_STARTED,
    DRIVER_NO_GPU = PYRO_DRIVER_NO_GPU,
    DRIVER_NOT_VULKAN = PYRO_DRIVER_NOT_VULKAN,
};

constexpr uint64_t ACQUIRE_TIMEOUT_NS = 250'000'000;
constexpr uint64_t FENCE_TIMEOUT_NS = 2'000'000'000;

// VK_KHR_global_priority and its EXT predecessor share one create-info layout.
const char *const GLOBAL_PRIORITY_EXTENSIONS[] = {"VK_KHR_global_priority", "VK_EXT_global_priority"};

const char *const INSTANCE_EXTENSIONS[] = {
    VK_KHR_SURFACE_EXTENSION_NAME,
    VK_KHR_ANDROID_SURFACE_EXTENSION_NAME,
};
const char *const DEVICE_EXTENSIONS[] = {
    VK_KHR_SWAPCHAIN_EXTENSION_NAME,
};

struct CustomDriverLibrary {
    void *adrenotools = nullptr;
    void *library = nullptr;
    uint32_t loaded = 0;
    // Why the driver did not load (the dynamic linker's message), for the app's log. Written
    // while the loader first opens the driver, which readiness probes do under readinessMutex.
    char error[512] = {};
};

struct VulkanLoader {
    void *library = nullptr;
    CustomDriverLibrary *custom;
    PFN_vkGetInstanceProcAddr GetInstanceProcAddr = nullptr;
    PFN_vkSetHdrMetadataEXT SetHdrMetadataEXT = nullptr;
    PFN_vkGetPastPresentationTimingGOOGLE GetPastPresentationTimingGOOGLE = nullptr;
    PFN_vkGetRefreshCycleDurationGOOGLE GetRefreshCycleDurationGOOGLE = nullptr;
    // Vulkan 1.2; absent on 1.1 drivers, which use fences for decode completion.
    PFN_vkWaitSemaphores WaitSemaphores = nullptr;
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

    // The hook writes 1 once the custom driver loaded, 2 when it turned the library away.
    bool usingRequestedDriver() const {
        return custom == nullptr || __atomic_load_n(&custom->loaded, __ATOMIC_ACQUIRE) == 1;
    }

    bool customNotVulkan() const {
        return custom != nullptr && __atomic_load_n(&custom->loaded, __ATOMIC_ACQUIRE) == 2;
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
        WaitSemaphores = reinterpret_cast<PFN_vkWaitSemaphores>(GetDeviceProcAddr(device, "vkWaitSemaphores"));
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
    Readiness reason = NEEDS_VULKAN;
    bool float16 = false;
    // Below Vulkan 1.3: the core 1.3 features come from extensions, PyroWave reads its payload
    // through texel buffers when 8-bit storage is missing, and decode completion is a fence
    // on one shared queue instead of a timeline semaphore (see Renderer::legacySync).
    bool legacySync = false;
    bool storage8 = false;
    bool float16Extension = false;
    uint32_t apiVersion = 0;
    char name[VK_MAX_PHYSICAL_DEVICE_NAME_SIZE] = {};
};

inline uint64_t nowUs() {
    timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return uint64_t(ts.tv_sec) * 1000000u + uint64_t(ts.tv_nsec) / 1000u;
}

inline int64_t nowNs() {
    timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return int64_t(ts.tv_sec) * 1'000'000'000LL + ts.tv_nsec;
}

inline const char *presentModeName(VkPresentModeKHR mode) {
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
    // Signaled when this slot's decode completes (Renderer::legacySync only).
    VkFence decodeFence = VK_NULL_HANDLE;
    uint64_t decodeValue = 0;
    int64_t ptsUs = 0;
    int64_t hostPresentationTimeUs = 0;
    int64_t completedNs = 0;
};

// Frame Warp (docs/frame-warp.md): a slot's HUD mask image, one R8 texel per cell, and the
// host-visible buffer it is written from.
struct WarpMask {
    VkImage image = VK_NULL_HANDLE;
    VkDeviceMemory memory = VK_NULL_HANDLE;
    VkImageView view = VK_NULL_HANDLE;
    VkBuffer staging = VK_NULL_HANDLE;
    VkDeviceMemory stagingMemory = VK_NULL_HANDLE;
    void *mapped = nullptr;
    uint32_t width = 0;
    uint32_t height = 0;
};

// A mask image not written yet (WarpSlot::generation)
constexpr int32_t WARP_MASK_UNWRITTEN = -2;
// The floats of FrameWarpShift (shaders/frame_warp.glsl): vec4 shift, then vec4 lens
constexpr size_t WARP_SHIFT_FLOATS = 8;

// Frame Warp's share of a slot: what its draws read besides the planes (set 1 in
// shaders/frame_warp.glsl). Only the present thread uses it once enableFrameWarp() made it, and
// only while the slot's draw fence has signalled, as for drawCommandBuffer.
struct WarpSlot {
    WarpMask mask;
    // The mask generation in the image, and the one the recorded commands write
    int32_t generation = WARP_MASK_UNWRITTEN;
    int32_t writing = WARP_MASK_UNWRITTEN;
    // The shift and the lens terms (FrameWarpShift: WARP_SHIFT_FLOATS floats), set just before
    // each submit
    VkBuffer shift = VK_NULL_HANDLE;
    VkDeviceMemory shiftMemory = VK_NULL_HANDLE;
    float *shiftMapped = nullptr;
    VkDescriptorSet set = VK_NULL_HANDLE;
    // The host's number for the picture decoded into the slot, -1 when unknown
    int64_t frameNumber = -1;
};

// Functions only Frame Warp uses, loaded when it is set up so that a stream without it loads
// what it always did.
struct WarpFunctions {
    PFN_vkCreateBuffer CreateBuffer = nullptr;
    PFN_vkDestroyBuffer DestroyBuffer = nullptr;
    PFN_vkGetBufferMemoryRequirements GetBufferMemoryRequirements = nullptr;
    PFN_vkBindBufferMemory BindBufferMemory = nullptr;
    PFN_vkMapMemory MapMemory = nullptr;
    PFN_vkUnmapMemory UnmapMemory = nullptr;
    PFN_vkCmdCopyBufferToImage CmdCopyBufferToImage = nullptr;
};

bool setAdrenoMaxClocks(bool enabled);
CustomDriverLibrary *customDriverLibrary(const std::string &hooks, const std::string &directory,
                                         const std::string &name);
std::string jniString(JNIEnv *env, jstring value);
bool hasExtension(const std::vector<VkExtensionProperties> &extensions, const char *name);
FeatureProbe probeFeatures(const VulkanLoader &vk, VkPhysicalDevice device, bool tenBit, uint32_t instanceVersion);
uint32_t instanceApiVersion(uint32_t loaderVersion);
bool apiVersionSupported();
Readiness probeVulkan(bool tenBit, VulkanLoader &vk, PyroProbeStage &stage);
Readiness probeReadiness(bool tenBit, CustomDriverLibrary *custom);

// Accessors for warp fragment SPIR-V so shaders_spv.h is compiled into a single TU.
const uint32_t *warpPlanarCscFragSpv(size_t *sizeBytes);
const uint32_t *warpSgsrFragSpv(size_t *sizeBytes);

}  // namespace pyrowave_renderer
