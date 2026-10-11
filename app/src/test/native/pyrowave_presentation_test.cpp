#include <cassert>
#include "../../main/jni/pyrowave-renderer/presentation.h"

int main() {
    assert(pyroSurfaceDimension(1920, 1280, 64, 4096) == 1920);
    assert(pyroSurfaceDimension(UINT32_MAX, 1920, 64, 4096) == 1920);
    assert(pyroSurfaceDimension(UINT32_MAX, 5000, 64, 4096) == 4096);
    assert(pyroSurfaceDimension(UINT32_MAX, 32, 64, 4096) == 64);

    const auto swapchainWidth = pyroSurfaceDimension(UINT32_MAX, 1920, 64, 4096);
    assert(pyroSurfaceDimension(UINT32_MAX, 1920, 64, 4096) == swapchainWidth);
    assert(pyroSurfaceDimension(UINT32_MAX, 2560, 64, 4096) != swapchainWidth);
    const auto cappedWidth = pyroSurfaceDimension(UINT32_MAX, 5000, 64, 4096);
    assert(pyroSurfaceDimension(UINT32_MAX, 6000, 64, 4096) == cappedWidth);

    const auto fit = pyroViewport(1280, 720, 2400, 1080, false);
    assert(fit.x == 240 && fit.y == 0 && fit.width == 1920 && fit.height == 1080);
    assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1280, 720, fit, true) == UPSCALING_ACTIVE);
    assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1280, 720, fit, false) == UPSCALING_GPU_ERROR);
    assert(pyroUpscalingReason(UPSCALE_SGSR, true, 1280, 720, fit, true) == UPSCALING_HDR);
    assert(pyroUpscalingReason(UPSCALE_OFF, false, 1280, 720, fit, true) == UPSCALING_DISABLED);
    assert(pyroUpscalingReason(UPSCALE_BILINEAR, false, 1280, 720, fit, false) == UPSCALING_ACTIVE);

    const auto stretch = pyroViewport(1280, 720, 2400, 1080, true);
    assert(stretch.x == 0 && stretch.y == 0 && stretch.width == 2400 && stretch.height == 1080);
    assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1280, 720, stretch, true) == UPSCALING_ACTIVE);
    const auto portrait = pyroViewport(720, 1280, 1080, 2400, false);
    assert(portrait.x == 0 && portrait.y == 240 && portrait.width == 1080 && portrait.height == 1920);

    const auto native = pyroViewport(1920, 1080, 1920, 1080, false);
    assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1920, 1080, native, true) == UPSCALING_NOT_NEEDED);
    const auto smaller = pyroViewport(1920, 1080, 1280, 720, false);
    assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1920, 1080, smaller, true) == UPSCALING_NOT_NEEDED);
    const auto mixed = pyroViewport(1920, 1080, 2560, 720, true);
    assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1920, 1080, mixed, true) == UPSCALING_NOT_NEEDED);
}
