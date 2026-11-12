// Frame Warp for the native renderers (docs/frame-warp.md): what the Vulkan video renderer and
// the PyroWave renderer share on the CPU side. The HUD mask as the host last sent it, one byte per
// cell, the rules for drawing a picture again between the host's pictures, and where the picture
// is in a padded decoder buffer. Free of Vulkan types for the host tests
// (app/src/test/native/frame_warp_mask_test.cpp).
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

// What the picture on screen was last drawn with, so that a refresh between the host's pictures
// can tell without peek() (which takes the stream's lock) that a redraw has nothing new to show.
// GlesUpscaler keeps the same in Java.
struct FrameWarpDrawn {
    // The shift on screen, picture pixels
    float px[2] = {0.0f, 0.0f};
    // The draw's shift was active: the host's model is locked, or fading out after it unlocked
    bool active = false;
    // FrameWarpApi::input_seq as the draw read it, before its warp()
    uint32_t inputSeq = 0;

    // A draw that `code` and `out` came from (warp(), frame_warp_api.h), with input_seq read
    // before it
    void note(int32_t code, const float *out, uint32_t seq) {
        active = code != FRAME_WARP_INACTIVE;
        px[0] = active ? out[2] : 0.0f;
        px[1] = active ? out[3] : 0.0f;
        inputSeq = seq;
    }

    // Whether the shift may have moved since the draw: only while the model is locked, and then
    // while the shift on screen is not zero (a fade or the turn in flight still changes it), input
    // was sent since, or the stick is deflected (its turn grows without new input)
    bool mayMove(const FrameWarpApi *api) const {
        if (!active) return false;
        if (px[0] != 0.0f || px[1] != 0.0f) return true;
        return api->input_seq() != inputSeq || api->stick_deflected() != 0;
    }
};

// The most columns and rows an encoder's alignment adds beyond the stream's size (AMD's AV1: up
// to a multiple of 64 columns and 16 rows), as StreamResizePolicy.PADDING_COLUMNS and PADDING_ROWS
constexpr int32_t kFrameWarpPaddingColumns = 64;
constexpr int32_t kFrameWarpPaddingRows = 16;

struct FrameWarpRect {
    int32_t left = 0;
    int32_t top = 0;
    int32_t right = 0;
    int32_t bottom = 0;
};

// The stream's picture within a decoder's crop rect. A decoder can leave an encoder's alignment
// padding inside its crop (AMD's AV1 at 2184x1968 decodes as 2240x1968, black at the right); the
// stream view then enlarges the video so that the padding falls off screen
// (StreamResizePolicy.paddedVisibleSize), and the picture is the crop's top left at the stream's
// size. Any other crop is the picture as it is.
inline FrameWarpRect frameWarpPicture(FrameWarpRect crop, int32_t streamWidth, int32_t streamHeight) {
    const int32_t width = crop.right - crop.left;
    const int32_t height = crop.bottom - crop.top;
    const auto padded = [](int32_t extent, int32_t stream, int32_t most) {
        return stream > 0 && extent >= stream && extent < stream + most;
    };
    if (padded(width, streamWidth, kFrameWarpPaddingColumns) && padded(height, streamHeight, kFrameWarpPaddingRows)) {
        crop.right = crop.left + streamWidth;
        crop.bottom = crop.top + streamHeight;
    }
    return crop;
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
