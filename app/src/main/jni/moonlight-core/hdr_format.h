#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "Limelight.h"
#include "pyrowave_protocol.h"

// Rubylight 3.0 HDR formats beyond stock HDR10, one bit each (rubylight_protocol::hdr). A host
// lists what this stream can use in its DESCRIBE answer:
//   a=x-rl-video.hdrFormats:5
// and the client picks one bit in its ANNOUNCE, next to stock HDR (dynamicRangeMode 1):
//   a=x-rl-video[0].hdrFormat:4
// A host without the extension never sees the key, and nothing asked for streams stock PQ.

#define RL_HDR_PQ 1
#define RL_HDR_HLG 2
#define RL_HDR_DV_81 4
#define RL_HDR_DV_84 8
#define RL_HDR_DV_101 16
#define RL_HDR_ALL (RL_HDR_PQ | RL_HDR_HLG | RL_HDR_DV_81 | RL_HDR_DV_84 | RL_HDR_DV_101)
// Dolby Vision profile 8 (8.1, 8.4) is HEVC, profile 10 (10.1) AV1.
#define RL_HDR_DOLBY_VISION_HEVC (RL_HDR_DV_81 | RL_HDR_DV_84)
#define RL_HDR_DOLBY_VISION_AV1 RL_HDR_DV_101
// Offer flag, never a format: the host also takes HLG on PyroWave Main10 streams (3.0 rc hosts
// refuse it there). Offer flags count down from the top bit.
#define RL_HDR_PYROWAVE_HLG 0x80
#define RL_HDR_OFFER_FLAGS RL_HDR_PYROWAVE_HLG

// MoonBridge.requestHdrFormat for this connection (callbacks.c), the DESCRIBE answer's formats
// (android_rtsp.c) and what the ANNOUNCE then asked for (android_sdp.c), 0 when nothing.
extern uint8_t AndroidHdrFormatRequested;
extern uint8_t RlHdrFormatsOffered;
extern uint8_t RlHdrFormatAnnounced;

// The a=x-rl-video.hdrFormats bits and offer flags, as the Rust client reads them: a decimal
// 0-255 with optional spaces, unknown bits dropped; anything malformed is no offer.
static inline uint8_t rlParseHdrFormats(const char* sdp) {
    static const char prefix[] = "a=x-rl-video.hdrFormats:";
    const char* line = sdp;
    while (line != NULL && *line != 0) {
        if (strncmp(line, prefix, sizeof(prefix) - 1) == 0) {
            const char* p = line + sizeof(prefix) - 1;
            unsigned value = 0;
            int digits = 0;
            while (*p == ' ' || *p == '\t') p++;
            while (*p >= '0' && *p <= '9') {
                value = value * 10 + (unsigned)(*p - '0');
                if (value > 0xFF) return 0;
                p++;
                digits++;
            }
            while (*p == ' ' || *p == '\t') p++;
            if (digits == 0 || (*p != '\r' && *p != '\n' && *p != 0)) return 0;
            return (uint8_t)(value & (RL_HDR_ALL | RL_HDR_OFFER_FLAGS));
        }
        line = strchr(line, '\n');
        if (line != NULL) line++;
    }
    return 0;
}

// The format the ANNOUNCE asks for, or 0 for none: exactly one offered bit, on a 10-bit (HDR)
// HEVC or AV1 stream, Dolby Vision 8.1 and 8.4 only on HEVC and 10.1 only on AV1; on a 10-bit
// PyroWave stream only HLG, and only when the host offers RL_HDR_PYROWAVE_HLG. The same rule as
// the Rust client's announced_hdr_format.
static inline uint8_t rlAnnouncedHdrFormat(uint8_t asked, uint8_t offered, int videoFormat) {
    if (asked == 0 || (asked & (asked - 1)) != 0 || (asked & offered & RL_HDR_ALL) == 0) return 0;
    if ((videoFormat & VIDEO_FORMAT_MASK_10BIT) == 0) return 0;
    if ((videoFormat & VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
        return asked == RL_HDR_HLG && (offered & RL_HDR_PYROWAVE_HLG) != 0 ? asked : 0;
    }
    if ((videoFormat & (VIDEO_FORMAT_MASK_H265 | VIDEO_FORMAT_MASK_AV1)) == 0) return 0;
    if ((asked & RL_HDR_DOLBY_VISION_HEVC) != 0 && (videoFormat & VIDEO_FORMAT_MASK_H265) == 0) return 0;
    if ((asked & RL_HDR_DOLBY_VISION_AV1) != 0 && (videoFormat & VIDEO_FORMAT_MASK_AV1) == 0) return 0;
    return asked;
}
