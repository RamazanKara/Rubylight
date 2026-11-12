// Frame Warp for the native renderers (docs/frame-warp.md): the table of C functions that
// libnightjar_core.so keeps (client/protocol/android/src/frame_warp.rs, FrameWarpApi). Java
// hands its address over (MoonBridge.frameWarpApi()), as Android's linker namespaces keep one app
// library from resolving another's symbols portably. RustEngineJniParityTest holds the codes
// below to the Rust ones.
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FRAME_WARP_API_VERSION 2

// warp(): besides a HUD mask generation (0 to 65535)
#define FRAME_WARP_NO_MASK (-1)
#define FRAME_WARP_INACTIVE (-2)

// mask(): besides the new mask's generation
#define FRAME_WARP_MASK_UNCHANGED (-1)
#define FRAME_WARP_MASK_CLEARED (-2)
#define FRAME_WARP_MASK_TOO_SMALL (-3)

// Every function is safe to call from any thread, with or without a stream.
typedef struct FrameWarpApi {
    uint32_t version;
    // The frame number the decoder queued with presentation time ptsUs (microseconds), or -1.
    int64_t (*frame_for_pts)(int64_t ptsUs);
    // The shift for picture `frame` (-1 when unknown) drawn now. out gets four floats: the shift as
    // fractions of the picture's width and height, then in picture pixels; x right, y down, so the
    // picture is sampled at its position less the shift. Returns the HUD mask generation to keep
    // unshifted, FRAME_WARP_NO_MASK, or FRAME_WARP_INACTIVE to draw the picture as it is.
    int32_t (*warp)(int64_t frame, float* out);
    // The HUD mask when its generation is not `known` (-1 for none held): writes its width and
    // height and, given room, one byte per cell (255 where the HUD stays put, 0 elsewhere; rows
    // from the top) and returns its generation; or FRAME_WARP_MASK_UNCHANGED,
    // FRAME_WARP_MASK_CLEARED, or FRAME_WARP_MASK_TOO_SMALL with the size written.
    int32_t (*mask)(int32_t known, uint32_t* width, uint32_t* height, uint8_t* cells, size_t capacity);
    // As warp(), without counting it in the statistics: for deciding whether to draw the picture
    // on screen again. A redraw then calls warp() for what it draws.
    int32_t (*peek)(int64_t frame, float* out);
    // The count of mouse and controller inputs noted (wrapping), 0 without a stream. Read before a
    // draw's warp(): a redraw between pictures knows without a peek() that no input was sent since.
    uint32_t (*input_seq)(void);
    // 1 while the tracked right stick is deflected (its turn grows without new input), else 0.
    int32_t (*stick_deflected)(void);
} FrameWarpApi;

// The table at `address` when it is one this build understands, else null.
static inline const FrameWarpApi* frameWarpApiFrom(int64_t address) {
    const FrameWarpApi* api = (const FrameWarpApi*) (intptr_t) address;
    return api != NULL && api->version == FRAME_WARP_API_VERSION ? api : NULL;
}

#ifdef __cplusplus
}
#endif
