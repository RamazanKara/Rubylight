#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdint>

constexpr int UPSCALE_OFF = 0;
constexpr int UPSCALE_SGSR = 1;
constexpr int UPSCALE_BILINEAR = 2;

// Shared with PyroWaveDecoderRenderer.getUpscalingReason().
enum PyroWaveUpscalingReason {
    UPSCALING_ACTIVE = 0,
    UPSCALING_DISABLED = 1,
    UPSCALING_HDR = 2,
    UPSCALING_NOT_NEEDED = 3,
    UPSCALING_GPU_ERROR = 4,
};

inline uint32_t pyroSurfaceDimension(uint32_t current, int32_t window, uint32_t minimum, uint32_t maximum) {
    return current == UINT32_MAX ? std::clamp(uint32_t(std::max(window, 0)), minimum, maximum) : current;
}

struct PyroWaveViewport {
    float x, y, width, height;
};

struct PyroWaveExtent {
    uint32_t width, height;
};

// VkSurfaceTransformFlagBitsKHR values, kept independent of Vulkan headers so
// orientation and viewport calculations can be exercised by the host tests.
inline int32_t pyroRotationQuarterTurns(uint32_t transform) {
    switch (transform) {
        case 0x1: return 0;  // IDENTITY
        case 0x2: return 1;  // ROTATE_90
        case 0x4: return 2;  // ROTATE_180
        case 0x8: return 3;  // ROTATE_270
        default: return -1;
    }
}

// Mirrored/inherited transforms stay with the compositor when identity is
// supported. Zero means neither a supported rotation nor that fallback exists.
inline uint32_t pyroPreTransform(uint32_t current, uint32_t supported) {
    if (pyroRotationQuarterTurns(current) >= 0 && (supported & current) == current) return current;
    return (supported & 0x1) != 0 ? 0x1 : 0;
}

// A quarter turn exchanges the logical display axes and the swapchain axes.
// This operation is its own inverse, so it also recovers the display extent.
inline PyroWaveExtent pyroRotateExtent(PyroWaveExtent extent, int32_t quarterTurns) {
    return quarterTurns == 1 || quarterTurns == 3 ? PyroWaveExtent{extent.height, extent.width} : extent;
}

// Map a viewport fitted in logical display coordinates into the pre-rotated
// swapchain. The buffer extent is already in the display's identity orientation.
inline PyroWaveViewport pyroRotateViewport(const PyroWaveViewport &view, PyroWaveExtent buffer,
                                           int32_t quarterTurns) {
    switch (quarterTurns) {
        case 1: return {float(buffer.width) - view.height - view.y, view.x, view.height, view.width};
        case 2: return {float(buffer.width) - view.width - view.x,
                        float(buffer.height) - view.height - view.y, view.width, view.height};
        case 3: return {view.y, float(buffer.height) - view.width - view.x, view.height, view.width};
        default: return view;
    }
}

// Layouts shared with shaders/planar_csc.frag and shaders/sgsr.frag. Rotation is
// clockwise quarter turns; each fragment maps its output UV back to source UV.
struct PyroWaveColorParams {
    int32_t tenBit, hdr, fullRange, rotationQuarterTurns;
};

struct PyroWaveSgsrParams {
    float viewportInfo[4];
    float edgeSharpness;
    int32_t fullRange, rotationQuarterTurns;
};

static_assert(sizeof(PyroWaveColorParams) == 16 && offsetof(PyroWaveColorParams, rotationQuarterTurns) == 12);
static_assert(sizeof(PyroWaveSgsrParams) == 28 && offsetof(PyroWaveSgsrParams, edgeSharpness) == 16 &&
              offsetof(PyroWaveSgsrParams, fullRange) == 20 && offsetof(PyroWaveSgsrParams, rotationQuarterTurns) == 24);

inline PyroWaveViewport pyroViewport(uint32_t width, uint32_t height, uint32_t surfaceWidth,
                                    uint32_t surfaceHeight, bool stretch) {
    const float scale = std::min(float(surfaceWidth) / float(width), float(surfaceHeight) / float(height));
    const float outputWidth = stretch ? float(surfaceWidth) : float(width) * scale;
    const float outputHeight = stretch ? float(surfaceHeight) : float(height) * scale;
    return {(float(surfaceWidth) - outputWidth) / 2.0f, (float(surfaceHeight) - outputHeight) / 2.0f,
            outputWidth, outputHeight};
}

inline PyroWaveUpscalingReason pyroUpscalingReason(int mode, bool tenBitOrHdr, uint32_t width, uint32_t height,
                                                  const PyroWaveViewport &viewport, bool sgsrReady) {
    if (mode == UPSCALE_OFF) return UPSCALING_DISABLED;
    if (tenBitOrHdr) return UPSCALING_HDR;
    if (viewport.width < float(width) || viewport.height < float(height) ||
        (viewport.width == float(width) && viewport.height == float(height))) return UPSCALING_NOT_NEEDED;
    if (mode == UPSCALE_SGSR && !sgsrReady) return UPSCALING_GPU_ERROR;
    return UPSCALING_ACTIVE;
}
