#include <cassert>
#include <cmath>
#include "../../main/jni/pyrowave-renderer/presentation.h"

// Compile the actual shared GLSL UV mapping as C++ with its small vector type.
// This catches rotation-direction errors in both CSC and SGSR without a GPU.
struct vec2 {
    float x, y;
    explicit vec2(float v) : x(v), y(v) {}
    vec2(float x_, float y_) : x(x_), y(y_) {}
};
static vec2 operator-(vec2 a, vec2 b) { return {a.x - b.x, a.y - b.y}; }
#include "../../main/jni/pyrowave-renderer/shaders/rotation.glsl"

static void same(float actual, float expected) {
    assert(std::abs(actual - expected) < 0.00001f);
}

static void viewportIs(PyroWaveViewport actual, PyroWaveViewport expected) {
    same(actual.x, expected.x);
    same(actual.y, expected.y);
    same(actual.width, expected.width);
    same(actual.height, expected.height);
}

static void rotationMapping() {
    // Unsupported mirrors/inheritance must never silently become a rotation.
    assert(pyroRotationQuarterTurns(0x1) == 0);
    assert(pyroRotationQuarterTurns(0x2) == 1);
    assert(pyroRotationQuarterTurns(0x4) == 2);
    assert(pyroRotationQuarterTurns(0x8) == 3);
    for (uint32_t unsupported : {0u, 3u, 0x10u, 0x20u, 0x40u, 0x80u, 0x100u}) {
        assert(pyroRotationQuarterTurns(unsupported) == -1);
        assert(pyroPreTransform(unsupported, 0x1ff) == 0x1);
        assert(pyroPreTransform(unsupported, 0x1fe) == 0);
    }
    for (uint32_t transform : {0x1u, 0x2u, 0x4u, 0x8u}) {
        assert(pyroPreTransform(transform, 0xf) == transform);
        assert(pyroPreTransform(transform, transform) == transform);
    }
    assert(pyroPreTransform(0x2, 0x1) == 0x1);
    assert(pyroPreTransform(0x2, 0x4) == 0);

    const PyroWaveExtent logical{2400, 1080};
    const PyroWaveExtent oddWindow{639, 841};
    for (int rotation = 0; rotation < 4; ++rotation) {
        const auto buffer = pyroRotateExtent(logical, rotation);
        assert(buffer.width == (rotation % 2 ? 1080u : 2400u));
        assert(buffer.height == (rotation % 2 ? 2400u : 1080u));
        const auto restored = pyroRotateExtent(buffer, rotation);
        assert(restored.width == logical.width && restored.height == logical.height);
        const auto oddRestored = pyroRotateExtent(pyroRotateExtent(oddWindow, rotation), rotation);
        assert(oddRestored.width == oddWindow.width && oddRestored.height == oddWindow.height);
    }

    const PyroWaveViewport asymmetric{17, 31, 400, 300};
    viewportIs(pyroRotateViewport(asymmetric, logical, 0), asymmetric);
    viewportIs(pyroRotateViewport(asymmetric, {1080, 2400}, 1), {749, 17, 300, 400});
    viewportIs(pyroRotateViewport(asymmetric, logical, 2), {1983, 749, 400, 300});
    viewportIs(pyroRotateViewport(asymmetric, {1080, 2400}, 3), {31, 1983, 300, 400});

    // Known corner markers verify clockwise display rotation, not just a
    // self-consistent forward/inverse pair. Order: TL, TR, BL, BR.
    const vec2 bufferCorners[] = {{0, 0}, {1, 0}, {0, 1}, {1, 1}};
    const vec2 sourceCorners[4][4] = {
        {{0, 0}, {1, 0}, {0, 1}, {1, 1}},
        {{0, 1}, {0, 0}, {1, 1}, {1, 0}},
        {{1, 1}, {0, 1}, {1, 0}, {0, 0}},
        {{1, 0}, {1, 1}, {0, 0}, {0, 1}},
    };
    for (int rotation = 0; rotation < 4; ++rotation) {
        for (int corner = 0; corner < 4; ++corner) {
            const auto source = pyroSourceUv(bufferCorners[corner], rotation);
            same(source.x, sourceCorners[rotation][corner].x);
            same(source.y, sourceCorners[rotation][corner].y);
        }
        const auto center = pyroSourceUv({0.5f, 0.5f}, rotation);
        same(center.x, 0.5f);
        same(center.y, 0.5f);
    }
    const vec2 expected[] = {{0.2f, 0.7f}, {0.7f, 0.8f}, {0.8f, 0.3f}, {0.3f, 0.2f}};
    for (int rotation = 0; rotation < 4; ++rotation) {
        const auto source = pyroSourceUv({0.2f, 0.7f}, rotation);
        same(source.x, expected[rotation].x);
        same(source.y, expected[rotation].y);
    }
}

static void rotatedFitAndUpscaling() {
    for (int rotation = 0; rotation < 4; ++rotation) {
        const PyroWaveExtent logical{2400, 1080};
        const auto buffer = pyroRotateExtent(logical, rotation);
        const auto fitted = pyroViewport(1280, 720, logical.width, logical.height, false);
        const auto rotated = pyroRotateViewport(fitted, buffer, rotation);
        viewportIs(rotated, rotation % 2 ? PyroWaveViewport{0, 240, 1080, 1920}
                                        : PyroWaveViewport{240, 0, 1920, 1080});
        assert(pyroUpscalingReason(UPSCALE_SGSR, false, 1280, 720, fitted, true) == UPSCALING_ACTIVE);

        const auto stretch = pyroViewport(1280, 720, logical.width, logical.height, true);
        viewportIs(pyroRotateViewport(stretch, buffer, rotation),
                   {0, 0, float(buffer.width), float(buffer.height)});

        // A native-sized stream stays native-sized after a quarter turn.
        const auto native = pyroViewport(2400, 1080, logical.width, logical.height, false);
        assert(pyroUpscalingReason(UPSCALE_SGSR, false, 2400, 1080, native, true) == UPSCALING_NOT_NEEDED);
        viewportIs(pyroRotateViewport(native, buffer, rotation),
                   {0, 0, float(buffer.width), float(buffer.height)});
    }
}

static void swapchainFormatSelection() {
    // What the Android 13 and 14 loader lists with VK_EXT_swapchain_colorspace enabled: no HDR10_ST2084.
    const PyroSurfaceFormat android13[] = {
        {37, 0}, {43, 0}, {37, 1000104013}, {37, 1000104001},
        {PYRO_FORMAT_A2B10G10R10, 0}, {PYRO_FORMAT_A2B10G10R10, 1000104013}, {PYRO_FORMAT_A2B10G10R10, 1000104001},
    };
    const size_t androidCount = sizeof(android13) / sizeof(android13[0]);
    auto ten = pyroSelectSwapchainFormat(android13, androidCount, true);
    assert(ten.format == PYRO_FORMAT_A2B10G10R10 && ten.dataspaceOverride);
    auto eight = pyroSelectSwapchainFormat(android13, androidCount, false);
    assert(eight.format == PYRO_FORMAT_R8G8B8A8 && !eight.dataspaceOverride);

    // Android 15 and later list HDR10_ST2084 for the same format: the native path stays.
    const PyroSurfaceFormat android15[] = {
        {37, 0}, {PYRO_FORMAT_A2B10G10R10, 0}, {PYRO_FORMAT_A2B10G10R10, PYRO_COLORSPACE_HDR10_ST2084},
    };
    ten = pyroSelectSwapchainFormat(android15, 3, true);
    assert(ten.format == PYRO_FORMAT_A2B10G10R10 && !ten.dataspaceOverride);

    // A native pair on the second ten-bit format beats an override on the first.
    const PyroSurfaceFormat mixed[] = {
        {PYRO_FORMAT_A2B10G10R10, 0},
        {PYRO_FORMAT_A2R10G10B10, 0}, {PYRO_FORMAT_A2R10G10B10, PYRO_COLORSPACE_HDR10_ST2084},
    };
    ten = pyroSelectSwapchainFormat(mixed, 3, true);
    assert(ten.format == PYRO_FORMAT_A2R10G10B10 && !ten.dataspaceOverride);

    // HDR10_ST2084 alone, without the SRGB_NONLINEAR pair, is not enough: host HDR-off needs both.
    const PyroSurfaceFormat hdrOnly[] = {{37, 0}, {PYRO_FORMAT_A2B10G10R10, PYRO_COLORSPACE_HDR10_ST2084}};
    assert(pyroSelectSwapchainFormat(hdrOnly, 2, true).format == 0);

    // No ten-bit format at all: ten-bit fails, eight-bit still works.
    const PyroSurfaceFormat eightOnly[] = {{37, 0}, {44, 0}};
    assert(pyroSelectSwapchainFormat(eightOnly, 2, true).format == 0);
    assert(pyroSelectSwapchainFormat(eightOnly, 2, false).format == 37);
    assert(pyroSelectSwapchainFormat(nullptr, 0, false).format == 0);

    assert(pyroWindowDataspace(true) == 163971072);   // ADATASPACE_BT2020_PQ
    assert(pyroWindowDataspace(false) == 142671872);  // ADATASPACE_SRGB
}

int main() {
    swapchainFormatSelection();
    rotationMapping();
    rotatedFitAndUpscaling();
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
