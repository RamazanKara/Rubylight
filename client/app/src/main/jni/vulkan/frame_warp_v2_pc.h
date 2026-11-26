// Frame Warp for the native renderers (docs/frame-warp.md): the shader row warp() writes after the
// lens terms (frame_warp_api.h, floats 8 to 11), which the Vulkan video renderer pushes as
// pc.warpV2 (shaders/video_warp.frag) and the PyroWave renderer writes as FrameWarpShift.v2
// (pyrowave-renderer/shaders/frame_warp.glsl), as GlesFrameWarpV2 sets warp.frag's uWarpV2.
//   x: the edge fill's strength, 0 to 1. Past the picture's edge the shifted position is taken
//      back inside by a quarter of how far past it is, darkened by up to x along a smoothstep over
//      8% of the picture; 0 takes it back inside without darkening.
//   y: how many HUD mask cells a pixel whose shifted position lands on the HUD looks back along
//      the shift for the scene behind it, 0 to 8; 0 keeps such a pixel in place.
//   z, w: the confidence (already in the shift) and the host's flags, for information only.
// nightjar_core's frame_warp_v2::edge_v2 and hud_probe are the references every shader matches.
// Free of Vulkan types for the host tests (app/src/test/native).
#pragma once

#include <cstddef>

#include "../frame_warp_api.h"

// Where the row starts among warp()'s floats, and its length (a vec4 in every shader)
constexpr size_t kFrameWarpV2First = 8;
constexpr size_t kFrameWarpV2Floats = 4;
static_assert(kFrameWarpV2First + kFrameWarpV2Floats == FRAME_WARP_FLOATS,
              "the shader row is the last four floats warp() writes");

// The row of a draw whose warp() wrote `out` (FRAME_WARP_FLOATS floats; frameWarpStill() for a
// picture drawn as it is), into the shader's vec4 `row`.
inline void frameWarpV2Row(const float *out, float *row) {
    for (size_t i = 0; i < kFrameWarpV2Floats; ++i) {
        row[i] = out[kFrameWarpV2First + i];
    }
}
