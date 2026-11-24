// Internal helpers, constants, push-constant layout and logging macros shared
// across the vulkan_renderer translation units.
#pragma once

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
#include <sys/system_properties.h>
#include <sys/timerfd.h>
#include <unistd.h>
#include <android/data_space.h>
#include <android/log.h>
#include <android/looper.h>

#include "output_format.h"
#include "ycbcr_components.h"
#include "../window-dataspace/window_dataspace.h"

#define LOG_TAG "VulkanRenderer"
#define ALOGE(...) ::vkr::recordError(ANDROID_LOG_ERROR, __VA_ARGS__)
#define ALOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define ALOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
// Logged, and kept as the reason the renderer isn't available (VulkanRenderer::lastError())
#define ALOGR(...) ::vkr::recordError(ANDROID_LOG_INFO, __VA_ARGS__)

namespace vkr {

__attribute__((format(printf, 2, 3)))
void recordError(int priority, const char* format, ...);

// Accessor for kVideoWarpFragSpv so shaders_spv.h is compiled into a single TU.
const uint32_t* warpFragShaderCode(size_t* sizeBytes);

namespace renderer_detail {

// How long the destructor waits for a GPU that stop() found unfinished before leaking it
inline constexpr int kWedgedWaitSeconds = 5;

// The Android release's API level, 0 if unknown (PresentScheduler::maxDelayFor())
inline int deviceApiLevel() {
    char value[PROP_VALUE_MAX] = {};
    return __system_property_get("ro.build.version.sdk", value) > 0 ? atoi(value) : 0;
}

// Keep in sync with the push constant block in the shaders
struct PushConstants {
    float uvRect[4];
    float uvClamp[4];
    float params[4];
    float params2[4];
    // Frame Warp (video_warp.frag): xy = shift of the picture's centre as fractions of the
    // picture, z = 1 to keep the HUD mask in place; the picture's UV rect, the crop less any
    // padding (xy its top left, zw its size); and the lens terms the rest of the picture is
    // turned through (xy = k, zw = n; 0, 0, 1, 1 for a flat picture). Unused by video.frag.
    float warp[4];
    float warpRect[4];
    float warpLens[4];
};
// Every Vulkan device takes at least 128 bytes of push constants (maxPushConstantsSize)
static_assert(sizeof(PushConstants) == 7 * 4 * sizeof(float) && sizeof(PushConstants) <= 128,
              "PushConstants must match the shaders' block and fit the guaranteed size");

inline constexpr float kOutputPassthrough = 0.0f;
inline constexpr float kOutputPqToSdr = 1.0f;
inline constexpr float kOutputHlgToSdr = 2.0f;
inline constexpr float kSdrWhiteNits = 203.0f;
// The BT.2100 reference display an HLG picture is mapped for when it is shown as SDR
inline constexpr float kHlgNominalPeakNits = 1000.0f;

static_assert(vkr::kVkFormatR8G8B8A8Unorm == VK_FORMAT_R8G8B8A8_UNORM &&
              vkr::kVkFormatA2B10G10R10Unorm == VK_FORMAT_A2B10G10R10_UNORM_PACK32);
static_assert(vkr::kVkColorSpaceSrgbNonlinear == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR &&
              vkr::kVkColorSpaceHdr10St2084 == VK_COLOR_SPACE_HDR10_ST2084_EXT &&
              vkr::kVkColorSpaceHdr10Hlg == VK_COLOR_SPACE_HDR10_HLG_EXT);
static_assert(vkr::kDataspaceBt2020Pq == ADATASPACE_BT2020_PQ &&
              vkr::kDataspaceBt2020Hlg == ADATASPACE_BT2020_HLG);
static_assert(vkr::kVkComponentSwizzleIdentity == VK_COMPONENT_SWIZZLE_IDENTITY &&
              vkr::kVkComponentSwizzleR == VK_COMPONENT_SWIZZLE_R &&
              vkr::kVkComponentSwizzleB == VK_COMPONENT_SWIZZLE_B &&
              vkr::kVkFormatG10X6B10X6R10X6TwoPlane420Unorm3Pack16 ==
                      VK_FORMAT_G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16);

// Quarter turns clockwise for a pre-rotation, or -1 for a transform we can't render
inline int quarterTurns(VkSurfaceTransformFlagBitsKHR transform) {
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
inline VkSurfaceTransformFlagBitsKHR choosePreTransform(const VkSurfaceCapabilitiesKHR& caps) {
    if (quarterTurns(caps.currentTransform) >= 0 && (caps.supportedTransforms & caps.currentTransform)) {
        return caps.currentTransform;
    }
    if (caps.supportedTransforms & VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR) {
        return VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
    }
    return caps.currentTransform;
}

// The window's size in the display's natural orientation, as a pre-rotated swapchain needs
inline VkExtent2D preRotatedExtent(VkExtent2D extent, VkSurfaceTransformFlagBitsKHR transform) {
    if (quarterTurns(transform) % 2 == 1) {
        std::swap(extent.width, extent.height);
    }
    return extent;
}

inline constexpr uint32_t kWakeFrame = 1;
inline constexpr uint32_t kWakeHdr = 2;
inline constexpr uint32_t kWakeQuit = 4;
inline constexpr uint32_t kWakeVsync = 8;

inline constexpr int kLooperIdWake = 1;
inline constexpr int kLooperIdAlignedTimer = 2;

// Window for the overlay's count of skipped frames
inline constexpr int64_t kRecentSkipsNs = 10'000'000'000;

// Imported buffers are kept while the decoder keeps cycling through the same ones
inline constexpr size_t kMaxImports = 24;

// Images we may hold at once: the waiting queue, the ones the GPU is reading, the one on
// screen, and one being acquired
inline constexpr int32_t kMaxReaderImages = static_cast<int32_t>(FramePacer::kMaxQueuedFrames) + 4;

inline constexpr uint64_t kFenceTimeoutNs = 1'000'000'000;
inline constexpr uint64_t kAcquireTimeoutNs = 100'000'000;
// Once the renderer is stopping, waits for the GPU or a swapchain image are cut to this so
// stop() returns promptly, and a vkDeviceWaitIdle is bounded the same way
inline constexpr uint64_t kStopWaitNs = 50'000'000;
// How long stop() waits for the GPU's work in flight before giving up on cleaning up
inline constexpr uint64_t kStopDrainNs = 250'000'000;
// Consecutive failed renders after which the renderer gives up, so the stream ends with an
// error rather than sitting on a frozen picture
inline constexpr int kFatalFailures = 10;
// Present reports Java hasn't collected yet; at 1 s between collections, well over a second of 240 fps
inline constexpr size_t kMaxPresentReports = 512;
inline constexpr int kFatalFenceTimeouts = 3;

inline void raiseThreadPriority(const char* name) {
    // ANDROID_PRIORITY_URGENT_DISPLAY, then ANDROID_PRIORITY_DISPLAY
    const pid_t tid = gettid();
    if (setpriority(PRIO_PROCESS, tid, -8) != 0 && setpriority(PRIO_PROCESS, tid, -4) != 0) {
        ALOGW("Could not raise the %s thread's priority", name);
        return;
    }
    ALOGI("%s thread priority %d", name, getpriority(PRIO_PROCESS, tid));
}

inline int64_t nowNs() {
    timespec ts {};
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return static_cast<int64_t>(ts.tv_sec) * 1'000'000'000LL + ts.tv_nsec;
}

inline bool hasExtension(const std::vector<VkExtensionProperties>& extensions, const char* name) {
    for (const auto& ext : extensions) {
        if (strcmp(ext.extensionName, name) == 0) {
            return true;
        }
    }
    return false;
}

inline bool isYcbcrFormat(VkFormat format) {
    return (format >= VK_FORMAT_G8B8G8R8_422_UNORM && format <= VK_FORMAT_G16_B16_R16_3PLANE_444_UNORM) ||
           (format >= VK_FORMAT_G8_B8R8_2PLANE_444_UNORM && format <= VK_FORMAT_G16_B16R16_2PLANE_444_UNORM);
}

}  // namespace renderer_detail
}  // namespace vkr
