#include "moonlight-common-c/src/Limelight-internal.h"
#include "pyrowave_protocol.h"
#include "../pyrowave-renderer/frame.h"

// The core's opaque-frame path already handles the short header and exact FEC payload length.
// Its IDR validation also needs to accept PyroWave picture data.
#undef VIDEO_FORMAT_MASK_AV1
#define VIDEO_FORMAT_MASK_AV1 (0xF000 | VIDEO_FORMAT_MASK_PYROWAVE)
#define queueRtpPacket queueOrdinaryRtpPacket
#include "moonlight-common-c/src/VideoDepacketizer.c"
#undef queueRtpPacket

static uint8_t pyroLastBlock;
static bool pyroLastBlockEnded;
static size_t pyroEnvelopeCapacity;

// Keep the entire records envelope in one core-owned entry. The decoder callback copies one
// entry to Java and LiCompleteVideoFrame() frees it through allocPtr, just like an ordinary NAL.
// Only the receive thread grows this buffer; reassembleFrame() transfers its ownership intact.
static bool appendPyroWavePacket(const uint8_t* payload, int length, uint32_t flags) {
    const bool first = nalChainHead == NULL;
    const size_t markerLength = first ? 4 : 0;
    const size_t addedLength = markerLength + 8 + (size_t)length;
    if ((size_t)nalChainDataLength > PYRO_MAX_FRAME_BYTES - addedLength) return false;
    const size_t required = (size_t)nalChainDataLength + addedLength;
    PLENTRY_INTERNAL envelope = (PLENTRY_INTERNAL)nalChainHead;
    size_t capacity = first ? 16384 : pyroEnvelopeCapacity;
    while (capacity < required) {
        capacity = capacity > PYRO_MAX_FRAME_BYTES / 2 ? PYRO_MAX_FRAME_BYTES : capacity * 2;
    }
    if (first || capacity != pyroEnvelopeCapacity) {
        PLENTRY_INTERNAL grown = realloc(envelope, sizeof(*envelope) + capacity);
        if (grown == NULL) return false;
        envelope = grown;
        pyroEnvelopeCapacity = capacity;
    }
    // realloc() can move the entry and its inline bytes. Publish pointers only after success;
    // on allocation failure the old envelope and packet continuity remain unchanged.
    envelope->allocPtr = envelope;
    envelope->entry.next = NULL;
    envelope->entry.bufferType = BUFFER_TYPE_PICDATA;
    envelope->entry.data = (char*)(envelope + 1);
    uint8_t* dest = (uint8_t*)envelope->entry.data + nalChainDataLength;
    if (first) {
        pyroWriteLe32(dest, 0);
        dest += markerLength;
    }
    pyroWriteLe32(dest, (uint32_t)length);
    pyroWriteLe32(dest + 4, flags);
    memcpy(dest + 8, payload, (size_t)length);
    envelope->entry.length = (int)required;
    nalChainDataLength = (int)required;
    nalChainHead = nalChainTail = &envelope->entry;
    return true;
}

void finishPyroWaveFrame(void) {
    if (!nalChainHead) return;
    frameType = FRAME_TYPE_IDR;
    waitingForIdrFrame = waitingForNextSuccessfulFrame = false;
    nextFrameNumber++;
    reassembleFrame(nextFrameNumber - 1, false);
    cleanupFrameState();
}

void queueRtpPacket(PRTPV_QUEUE_ENTRY entry) {
    if (!PyroWaveRecordsEnabled) {
        queueOrdinaryRtpPacket(entry);
        return;
    }
    if (entry->length < (int)sizeof(*entry->packet)) {
        free(entry->packet);
        return;
    }
    int offset = sizeof(*entry->packet) + ((entry->packet->header & FLAG_EXTENSION) ? 4 : 0);
    if (entry->length < offset + (int)sizeof(NV_VIDEO_PACKET)) {
        free(entry->packet);
        return;
    }
    PNV_VIDEO_PACKET video = (PNV_VIDEO_PACKET)((char*)entry->packet + offset);
    const int length = entry->length - offset - sizeof(*video);
    uint32_t spi = (video->streamPacketIndex >> 8) & 0xffffff;
    uint8_t block = (video->multiFecBlocks >> 4) & 3;
    uint8_t lastBlock = (video->multiFecBlocks >> 6) & 3;
    uint32_t flags = (video->multiFecFlags & 0x80) ? PYRO_PACKET_RECORD_START : 0;
    if ((video->flags & FLAG_SOF) && block == 0) flags |= PYRO_PACKET_FRAME_START;
    if ((video->flags & FLAG_EOF) && block == lastBlock) flags |= PYRO_PACKET_FRAME_END;
    if (length <= 0 || length > 1400) {
        free(entry->packet);
        return;
    }
    const bool first = nalChainHead == NULL;
    if (!first && ((block == pyroLastBlock && spi == U24(lastPacketInStream + 1)) ||
            (block == pyroLastBlock + 1 && pyroLastBlockEnded && (video->flags & FLAG_SOF)))) {
        flags |= PYRO_PACKET_CONTIGUOUS;
    }
    if (!appendPyroWavePacket((const uint8_t*)(video + 1), length, flags)) {
        free(entry->packet);
        return;
    }
    if (first) {
        nextFrameNumber = video->frameIndex;
        firstPacketReceiveTimeUs = entry->receiveTimeUs;
        firstPacketPresentationTime = entry->presentationTimeUs;
        firstPacketRtpTimestamp = entry->rtpTimestamp;
        const uint8_t *payload = (const uint8_t*)(video + 1);
        frameHostProcessingLatency = (flags & PYRO_PACKET_FRAME_START) && length >= 3 ?
                payload[1] | ((uint16_t)payload[2] << 8) : 0;
    }
    lastPacketInStream = spi;
    pyroLastBlock = block;
    pyroLastBlockEnded = (video->flags & FLAG_EOF) != 0;
    free(entry->packet);
}
