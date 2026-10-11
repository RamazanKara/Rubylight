#pragma once

#include <algorithm>
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
