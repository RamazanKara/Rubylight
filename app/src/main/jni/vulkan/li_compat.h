// Force-included (see Android.mk). The renderer was written against a moonlight-common-c fork
// with an extra call that upstream, which Nightjar builds against, doesn't have. li_compat.cpp
// stands in for it, so the renderer sources stay as they are.
#pragma once
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Maps a frame's host timestamp onto the LiGetMicroseconds() clock. Reports no mapping, so the
// capture latency statistics treat each frame as measured without clock sync.
bool LiGetHostTimeOnLocalClock(uint64_t presentationTimeUs, uint64_t* localTimeUs);

#ifdef __cplusplus
}
#endif
