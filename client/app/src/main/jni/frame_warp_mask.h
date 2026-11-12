// Frame Warp for the native renderers (docs/frame-warp.md): what the Vulkan video renderer and
// the PyroWave renderer share on the CPU side. The HUD mask as the host last sent it, one byte per
// cell, and the rule for drawing a picture again between the host's pictures. Free of Vulkan
// types for the host tests (app/src/test/native/frame_warp_mask_test.cpp).
#pragma once

#include <cmath>
#include <cstddef>
#include <cstdint>
#include <vector>

#include "frame_warp_api.h"

// A picture is drawn again between the host's pictures once its shift moved at least this far,
// picture pixels (GlesUpscaler.REDRAW_SHIFT_PX).
constexpr float kFrameWarpRedrawPx = 0.25f;

// Whether a shift of (x, y) picture pixels moved far enough from the one on screen, (drawnX,
// drawnY), to draw the picture again.
inline bool frameWarpMoved(float drawnX, float drawnY, float x, float y) {
    return std::hypot(x - drawnX, y - drawnY) >= kFrameWarpRedrawPx;
}

// The newest HUD mask from the host: one byte per cell (255 where the HUD stays put, 0 elsewhere),
// rows from the top. One render thread polls it.
struct FrameWarpCells {
    // No mask (generation)
    static constexpr int32_t kEmpty = -1;
    // Wider or taller than any mask the host sends (frame_warp::MASK_MAX_WIDTH cells across),
    // within every Vulkan device's image limit
    static constexpr uint32_t kMaxSide = 4096;

    std::vector<uint8_t> cells;
    uint32_t width = 0;
    uint32_t height = 0;
    int32_t generation = kEmpty;

    // A mask to keep the HUD in place with
    bool any() const { return generation >= 0 && width > 0 && height > 0; }

    // Takes a newer mask from `api`, if there is one; true when the mask changed.
    bool poll(const FrameWarpApi *api) {
        uint32_t w = 0;
        uint32_t h = 0;
        int32_t code = api->mask(generation, &w, &h, cells.empty() ? nullptr : cells.data(), cells.size());
        if (code == FRAME_WARP_MASK_TOO_SMALL) {
            if (w == 0 || h == 0 || w > kMaxSide || h > kMaxSide) return false;
            cells.resize(static_cast<size_t>(w) * h);
            code = api->mask(generation, &w, &h, cells.data(), cells.size());
        }
        if (code == FRAME_WARP_MASK_CLEARED) {
            const bool changed = generation != kEmpty;
            generation = kEmpty;
            width = 0;
            height = 0;
            return changed;
        }
        if (code < 0 || w == 0 || h == 0 || w > kMaxSide || h > kMaxSide ||
                static_cast<size_t>(w) * h > cells.size()) {
            return false;
        }
        generation = code;
        width = w;
        height = h;
        return true;
    }
};
