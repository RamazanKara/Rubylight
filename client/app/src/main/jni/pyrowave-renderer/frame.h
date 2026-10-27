#pragma once
#include <stddef.h>
#include <stdint.h>

// Internal packet envelope. A zero container count distinguishes records from ordinary framing.
#define PYRO_PACKET_CONTIGUOUS 1u
#define PYRO_PACKET_RECORD_START 2u
#define PYRO_PACKET_FRAME_START 4u
#define PYRO_PACKET_FRAME_END 8u
#define PYRO_MAX_FRAME_BYTES (4092u * 1400u)

static inline void pyroWriteLe32(uint8_t *p, uint32_t value) {
    for (int i = 0; i < 4; ++i) p[i] = (uint8_t)(value >> (8 * i));
}

static inline uint32_t pyroReadLe32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
