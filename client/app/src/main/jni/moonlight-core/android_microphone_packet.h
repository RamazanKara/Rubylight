#pragma once

// Microphone packets sent from the client to the host.
//
// Ported from ClassicOldSong/moonlight-common-c src/MicrophoneStream.c at commit
// 784fa1d0f501155ab01fea7cefe8a0e9c9628b77 (GPL-3.0, Copyright (C) ClassicOldSong and the
// moonlight-common-c contributors), the client half of Apollo's microphone passthrough.
// Rubylight hosts take the same datagrams (docs/microphone.md in the host repository).
//
// One datagram per Opus packet:
//   0      flags      0
//   1      type       0x61 (Opus)
//   2..3   sequence   little-endian, +1 per packet
//   4..7   timestamp  little-endian, client milliseconds (host logs only)
//   8..11  magic      0x12345678, little-endian
//   12..   payload    AES-128-CBC of the Opus packet under the launch's rikey
// The IV is the big-endian 32-bit sum of rikeyid and the sequence number, then 12 zero bytes.
// Like the reference, the Opus packet is padded twice: PltEncryptMessage pads it to a whole
// block (nothing when it already is one) and OpenSSL then appends a full PKCS#7 block.

#include <stdint.h>
#include <string.h>

#include "PlatformCrypto.h"

#define SS_ENC_MICROPHONE 0x08
#define MIC_PACKET_TYPE_OPUS 0x61
#define MIC_PACKET_MAGIC 0x12345678
#define MAX_MIC_PACKET_SIZE 1400
#define MIC_PACKET_HEADER_SIZE 12
#define MIC_IV_LEN 16

// The DESCRIBE line a host that takes a microphone adds; without it, the mic is not set up.
// The reference also accepts a stereo offer; the client sends mono either way.
#define MIC_SDP_RTPMAP "a=rtpmap:96 opus/48000/1"
#define MIC_SDP_RTPMAP_STEREO "a=rtpmap:96 opus/48000/2"

static inline void micWriteHeader(uint8_t* out, uint16_t sequence, uint32_t timestampMs) {
    out[0] = 0;
    out[1] = MIC_PACKET_TYPE_OPUS;
    out[2] = (uint8_t)sequence;
    out[3] = (uint8_t)(sequence >> 8);
    out[4] = (uint8_t)timestampMs;
    out[5] = (uint8_t)(timestampMs >> 8);
    out[6] = (uint8_t)(timestampMs >> 16);
    out[7] = (uint8_t)(timestampMs >> 24);
    out[8] = (uint8_t)MIC_PACKET_MAGIC;
    out[9] = (uint8_t)(MIC_PACKET_MAGIC >> 8);
    out[10] = (uint8_t)(MIC_PACKET_MAGIC >> 16);
    out[11] = (uint8_t)(MIC_PACKET_MAGIC >> 24);
}

static inline void micBuildIv(uint8_t iv[MIC_IV_LEN], uint32_t riKeyId, uint16_t sequence) {
    // Unsigned addition wraps, as in the reference and the host.
    uint32_t ivSeq = riKeyId + sequence;
    memset(iv, 0, MIC_IV_LEN);
    iv[0] = (uint8_t)(ivSeq >> 24);
    iv[1] = (uint8_t)(ivSeq >> 16);
    iv[2] = (uint8_t)(ivSeq >> 8);
    iv[3] = (uint8_t)ivSeq;
}

// Builds one encrypted datagram into out (at least MAX_MIC_PACKET_SIZE bytes) and returns its
// length, or -1 when the packet is too large or encryption fails. ctx must be used only for
// microphone packets: it keeps the key after the first call.
static inline int micBuildPacket(PPLT_CRYPTO_CONTEXT ctx, const uint8_t key[16], uint32_t riKeyId,
                                 uint16_t sequence, uint32_t timestampMs,
                                 const uint8_t* opus, int opusLength, uint8_t* out) {
    // PltEncryptMessage pads in place, so it gets a copy with room for the padding rather
    // than the caller's buffer.
    uint8_t plaintext[ROUND_TO_PKCS7_PADDED_LEN(MAX_MIC_PACKET_SIZE)];
    uint8_t ciphertext[ROUND_TO_PKCS7_PADDED_LEN(MAX_MIC_PACKET_SIZE) + 16];
    uint8_t iv[MIC_IV_LEN];
    int ciphertextLength = (int)sizeof(ciphertext);

    if (ctx == NULL || opus == NULL || opusLength <= 0 ||
            opusLength > MAX_MIC_PACKET_SIZE - MIC_PACKET_HEADER_SIZE) {
        return -1;
    }

    memcpy(plaintext, opus, (size_t)opusLength);
    micBuildIv(iv, riKeyId, sequence);
    if (!PltEncryptMessage(ctx, ALGORITHM_AES_CBC,
                           CIPHER_FLAG_RESET_IV | CIPHER_FLAG_FINISH | CIPHER_FLAG_PAD_TO_BLOCK_SIZE,
                           (unsigned char*)key, 16, iv, sizeof(iv), NULL, 0,
                           plaintext, opusLength, ciphertext, &ciphertextLength)) {
        return -1;
    }
    if (ciphertextLength <= 0 || MIC_PACKET_HEADER_SIZE + ciphertextLength > MAX_MIC_PACKET_SIZE) {
        return -1;
    }

    micWriteHeader(out, sequence, timestampMs);
    memcpy(out + MIC_PACKET_HEADER_SIZE, ciphertext, (size_t)ciphertextLength);
    return MIC_PACKET_HEADER_SIZE + ciphertextLength;
}

// Whether a DESCRIBE answer offers a microphone, and the port its m=audio line names (0 when it
// names none). Only the rtpmap line decides; the port is a fallback for a SETUP reply without one.
static inline int micParseOffer(const char* sdp, uint16_t* port) {
    const char* line;
    const char* rtpmap;

    *port = 0;
    if (sdp == NULL) {
        return 0;
    }
    rtpmap = strstr(sdp, MIC_SDP_RTPMAP);
    if (rtpmap == NULL) {
        rtpmap = strstr(sdp, MIC_SDP_RTPMAP_STEREO);
    }
    if (rtpmap == NULL) {
        return 0;
    }
    // The last m=audio line before the rtpmap line describes it.
    for (line = strstr(sdp, "m=audio "); line != NULL && line < rtpmap; line = strstr(line + 1, "m=audio ")) {
        unsigned long value = 0;
        const char* digit = line + strlen("m=audio ");
        int digits = 0;
        while (*digit >= '0' && *digit <= '9' && digits < 6) {
            value = value * 10 + (unsigned long)(*digit - '0');
            digit++;
            digits++;
        }
        *port = (value > 0 && value <= 65535) ? (uint16_t)value : 0;
    }
    return 1;
}
