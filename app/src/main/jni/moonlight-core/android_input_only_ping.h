#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

// Vibepollo and Apollo hosts run their video thread for an input-only session too: it waits
// ping_timeout (10 s) for the client's first video UDP ping and ends the session without one.
// So an input-only session pings the video port like a normal stream does, with no decoder,
// surface or receive side. Rubylight hosts never read that socket for input-only and ignore it.

#define RL_PING_PAYLOAD_SIZE 16
// The SS_PING of Connection: the payload and a big-endian sequence number.
#define RL_PING_MAX_SIZE (RL_PING_PAYLOAD_SIZE + 4)
#define RL_PING_INTERVAL_MS 500

// Native input-only pings the video port once RTSP has negotiated one.
static inline bool rlInputOnlyPingsVideo(bool inputOnly, uint16_t videoPort) {
    return inputOnly && videoPort != 0;
}

// Writes ping number `sequence` to `out` (RL_PING_MAX_SIZE bytes) as VideoStream.c sends it: the
// RTSP X-SS-Ping-Payload and the sequence, or the legacy "PING" when the host sent no payload.
// Returns the datagram length.
static inline size_t rlBuildVideoPing(const uint8_t* payload, uint32_t sequence, uint8_t* out) {
    if (payload[0] == 0) {
        memcpy(out, "PING", 4);
        return 4;
    }
    memcpy(out, payload, RL_PING_PAYLOAD_SIZE);
    out[RL_PING_PAYLOAD_SIZE] = (uint8_t)(sequence >> 24);
    out[RL_PING_PAYLOAD_SIZE + 1] = (uint8_t)(sequence >> 16);
    out[RL_PING_PAYLOAD_SIZE + 2] = (uint8_t)(sequence >> 8);
    out[RL_PING_PAYLOAD_SIZE + 3] = (uint8_t)sequence;
    return RL_PING_MAX_SIZE;
}
