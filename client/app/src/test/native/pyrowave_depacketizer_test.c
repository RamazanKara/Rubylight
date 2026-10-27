#define LC_DEBUG
#include "../../main/jni/moonlight-core/moonlight-common-c/src/Limelight-internal.h"
#undef NDEBUG
#include <assert.h>

// Track all packet/envelope/queue allocations. Realloc always moves, making stale internal
// pointers observable under ASan as well as checking ownership on the normal completion path.
typedef struct { void* ptr; size_t size; } Allocation;
static Allocation allocations[128];
static int liveAllocations;
static int allocationCalls;
static int failAllocation;
static int envelopeAllocations;
static size_t largestAllocation;

static int allocationIndex(void* ptr) {
    for (int i = 0; i < liveAllocations; i++) if (allocations[i].ptr == ptr) return i;
    abort();
}

static void* trackedMalloc(size_t size) {
    allocationCalls++;
    if (failAllocation == allocationCalls) return NULL;
    void* ptr = malloc(size);
    assert(ptr && liveAllocations < 128);
    allocations[liveAllocations++] = (Allocation){ptr, size};
    if (size > largestAllocation) largestAllocation = size;
    return ptr;
}

static void trackedFree(void* ptr) {
    if (!ptr) return;
    int index = allocationIndex(ptr);
    memset(ptr, 0xdd, allocations[index].size);
    allocations[index] = allocations[--liveAllocations];
    free(ptr);
}

static void* trackedRealloc(void* ptr, size_t size) {
    envelopeAllocations++;
    void* grown = trackedMalloc(size);
    if (!grown) return NULL;
    if (ptr) {
        size_t oldSize = allocations[allocationIndex(ptr)].size;
        memcpy(grown, ptr, oldSize < size ? oldSize : size);
        trackedFree(ptr);
    }
    return grown;
}

#define malloc trackedMalloc
#define realloc trackedRealloc
#define free trackedFree
#include "../../main/jni/moonlight-core/android_depacketizer.c"
#undef malloc
#undef realloc
#undef free

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
DECODER_RENDERER_CALLBACKS VideoCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4];
int NegotiatedVideoFormat;
int PyroWaveRecordsSupported;
int PyroWaveRecordsEnabled;

static PQUEUED_DECODE_UNIT delivered[16];
static int deliveredCount;
static int queueResult;
static int completedFrames;

uint64_t PltGetMicroseconds(void) { return 1000000; }
bool LiGetCurrentHostDisplayHdrMode(void) { return false; }
void notifyKeyFrameReceived(void) { }
void LiRequestIdrFrame(void) { }
void connectionReceivedCompleteFrame(uint32_t frameIndex, bool isLTR) { completedFrames++; }
bool isReferenceFrameInvalidationEnabled(void) { return false; }
void connectionDetectedFrameLoss(uint32_t start, uint32_t end) { }

int LbqInitializeLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue, int bound) { return 0; }
void LbqSignalQueueShutdown(PLINKED_BLOCKING_QUEUE queue) { }
void LbqSignalQueueUserWake(PLINKED_BLOCKING_QUEUE queue) { abort(); }
PLINKED_BLOCKING_QUEUE_ENTRY LbqDestroyLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue) { return NULL; }
int LbqWaitForQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { abort(); }
int LbqPollQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { abort(); }
int LbqPeekQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { abort(); }
int LbqGetItemCount(PLINKED_BLOCKING_QUEUE queue) { return deliveredCount; }

int LbqOfferQueueItem(PLINKED_BLOCKING_QUEUE queue, void* data, PLINKED_BLOCKING_QUEUE_ENTRY entry) {
    if (queueResult) return queueResult;
    assert(deliveredCount < 16);
    delivered[deliveredCount++] = data;
    return 0;
}

PLINKED_BLOCKING_QUEUE_ENTRY LbqFlushQueueItems(PLINKED_BLOCKING_QUEUE queue) {
    return NULL;
}

static void resetTest(void) {
    assert(!nalChainHead && !nalChainTail && nalChainDataLength == 0);
    assert(liveAllocations == 0);
    allocationCalls = failAllocation = envelopeAllocations = deliveredCount = completedFrames = 0;
    largestAllocation = 0;
    queueResult = 0;
    memset(&VideoCallbacks, 0, sizeof(VideoCallbacks));
    NegotiatedVideoFormat = VIDEO_FORMAT_PYROWAVE;
    PyroWaveRecordsEnabled = 1;
    initializeVideoDepacketizer(1400);
}

static RTPV_QUEUE_ENTRY makePacket(uint32_t frame, uint32_t spi, int block, int lastBlock,
                                 int flags, bool recordStart, const uint8_t* payload,
                                 int length, bool extension) {
    RTPV_QUEUE_ENTRY entry = {0};
    entry.length = sizeof(RTP_PACKET) + (extension ? 4 : 0) + sizeof(NV_VIDEO_PACKET) + length;
    entry.packet = trackedMalloc(entry.length);
    memset(entry.packet, 0, entry.length);
    entry.packet->header = extension ? FLAG_EXTENSION : 0;
    PNV_VIDEO_PACKET video = (PNV_VIDEO_PACKET)((char*)entry.packet + sizeof(RTP_PACKET) + (extension ? 4 : 0));
    video->frameIndex = frame;
    video->streamPacketIndex = spi << 8;
    video->flags = flags;
    video->multiFecBlocks = (block << 4) | (lastBlock << 6);
    video->multiFecFlags = recordStart ? 0x80 : 0;
    memcpy(video + 1, payload, length);
    entry.receiveTimeUs = 111;
    entry.presentationTimeUs = 222;
    entry.rtpTimestamp = 333;
    return entry;
}

static void sendPacket(uint32_t frame, uint32_t spi, int block, int lastBlock,
                       int flags, bool recordStart, const uint8_t* payload, int length) {
    RTPV_QUEUE_ENTRY entry = makePacket(frame, spi, block, lastBlock, flags, recordStart, payload, length, spi & 1);
    queueRtpPacket(&entry);
}

static void verifySingleEntry(PLENTRY entry, int length) {
    assert(entry && entry->next == NULL && entry->length == length);
    assert(entry->bufferType == BUFFER_TYPE_PICDATA);
    assert(((PLENTRY_INTERNAL)entry)->allocPtr == entry);
    assert(entry->data == (char*)((PLENTRY_INTERNAL)entry + 1));
}

static void testExactWireBytesAndMetadata(void) {
    resetTest();
    const uint8_t a[] = {0xaa, 0x34, 0x12};
    const uint8_t b[] = {0x00, 0xff};
    const uint8_t expected[] = {
        0, 0, 0, 0,
        3, 0, 0, 0, 6, 0, 0, 0, 0xaa, 0x34, 0x12,
        2, 0, 0, 0, 9, 0, 0, 0, 0x00, 0xff
    };
    sendPacket(77, 0xffffff, 0, 0, FLAG_SOF, true, a, sizeof(a));
    sendPacket(77, 0, 0, 0, FLAG_EOF, false, b, sizeof(b));
    verifySingleEntry(nalChainHead, sizeof(expected));
    assert(nalChainHead == nalChainTail && nalChainDataLength == sizeof(expected));
    assert(memcmp(nalChainHead->data, expected, sizeof(expected)) == 0);
    finishPyroWaveFrame();
    assert(deliveredCount == 1 && completedFrames == 1 && nalChainHead == NULL);
    PDECODE_UNIT du = &delivered[0]->decodeUnit;
    verifySingleEntry(du->bufferList, sizeof(expected));
    assert(du->fullLength == sizeof(expected) && du->frameNumber == 77 && du->frameType == FRAME_TYPE_IDR);
    assert(du->receiveTimeUs == 111 && du->presentationTimeUs == 222 && du->rtpTimestamp == 333);
    assert(du->frameHostProcessingLatency == 0x1234);
    LiCompleteVideoFrame(delivered[0], DR_OK);
    assert(liveAllocations == 0);
}

static void testPartialAndFecBlockContinuity(void) {
    resetTest();
    const uint8_t payload[] = {0x20};
    // Missing SOF stays missing. Holes, duplicate packets and missing block EOF must not gain
    // CONTIGUOUS; complete block boundaries may skip parity packet sequence numbers.
    sendPacket(9, 10, 0, 3, 0, false, payload, 1);
    sendPacket(9, 12, 0, 3, 0, true, payload, 1);
    sendPacket(9, 12, 0, 3, FLAG_EOF, false, payload, 1);
    sendPacket(9, 30, 1, 3, FLAG_SOF, true, payload, 1);
    sendPacket(9, 40, 2, 3, FLAG_SOF | FLAG_EOF, true, payload, 1);
    sendPacket(9, 50, 3, 3, FLAG_SOF | FLAG_EOF, true, payload, 1);
    const uint32_t flags[] = {0, PYRO_PACKET_RECORD_START, 0,
        PYRO_PACKET_RECORD_START | PYRO_PACKET_CONTIGUOUS,
        PYRO_PACKET_RECORD_START,
        PYRO_PACKET_RECORD_START | PYRO_PACKET_CONTIGUOUS | PYRO_PACKET_FRAME_END};
    for (unsigned int i = 0; i < sizeof(flags) / sizeof(flags[0]); i++) {
        const uint8_t* packet = (const uint8_t*)nalChainHead->data + 4 + 9 * i;
        assert(pyroReadLe32(packet) == 1 && pyroReadLe32(packet + 4) == flags[i]);
        assert(packet[8] == payload[0]);
    }
    assert(frameHostProcessingLatency == 0);
    cleanupFrameState();
    assert(liveAllocations == 0);
}

static void testVibepolloRecordBoundaries(void) {
    const uint8_t payload[] = {0x20};
    const uint32_t spi[] = {10, 11, 13};
    const uint8_t extraFlags[] = {0x80, 0x01, 0x81};
    const uint32_t expected[] = {
        PYRO_PACKET_RECORD_START,
        PYRO_PACKET_CONTIGUOUS,
        PYRO_PACKET_RECORD_START | PYRO_PACKET_FRAME_END
    };
    for (int extension = 0; extension <= 1; extension++) {
        resetTest();
        for (unsigned int i = 0; i < sizeof(spi) / sizeof(spi[0]); i++) {
            // Vibepollo's wire header puts record starts in byte 9 (extraFlags), while
            // byte 10 (multiFecFlags) stays 0x10. Start without SOF, then lose a packet.
            // The LTR bit in extraFlags must not become a record boundary.
            RTPV_QUEUE_ENTRY entry = makePacket(9, spi[i], 0, 0, i == 2 ? FLAG_EOF : 0,
                                               false, payload, sizeof(payload), extension);
            uint8_t* videoHeader = (uint8_t*)(entry.packet + 1) + (extension ? 4 : 0);
            videoHeader[9] = extraFlags[i];
            videoHeader[10] = 0x10;
            queueRtpPacket(&entry);
        }
        for (unsigned int i = 0; i < sizeof(expected) / sizeof(expected[0]); i++) {
            const uint8_t* packet = (const uint8_t*)nalChainHead->data + 4 + 9 * i;
            assert(pyroReadLe32(packet + 4) == expected[i]);
        }
        cleanupFrameState();
        assert(liveAllocations == 0);
    }
}

static void testGrowthAndQueuedOwnership(void) {
    resetTest();
    uint8_t payload[1400];
    memset(payload, 0x43, sizeof(payload));
    for (int i = 0; i < 235; i++) {
        payload[0] = i;
        sendPacket(1, i, 0, 0, i == 0 ? FLAG_SOF : i == 234 ? FLAG_EOF : 0,
                   i == 0, payload, sizeof(payload));
    }
    verifySingleEntry(nalChainHead, 4 + 235 * 1408);
    assert(envelopeAllocations == 6); // 16, 32, 64, 128, 256 and 512 KiB, not 471 allocations.
    for (int i = 0; i < 235; i++) {
        const uint8_t* packet = (const uint8_t*)nalChainHead->data + 4 + i * 1408;
        assert(pyroReadLe32(packet) == 1400 && packet[8] == (uint8_t)i && packet[1407] == 0x43);
    }
    finishPyroWaveFrame();
    PLENTRY first = delivered[0]->decodeUnit.bufferList;
    uint8_t* copy = malloc(first->length);
    assert(copy);
    memcpy(copy, first->data, first->length);
    for (int i = 0; i < 50; i++) sendPacket(2, 300 + i, 0, 0, 0, false, payload, sizeof(payload));
    assert(nalChainHead != first && memcmp(first->data, copy, first->length) == 0);
    finishPyroWaveFrame();
    assert(deliveredCount == 2 && liveAllocations == 4);
    free(copy);
    LiCompleteVideoFrame(delivered[1], DR_OK);
    assert(liveAllocations == 2);
    LiCompleteVideoFrame(delivered[0], DR_OK);
    assert(liveAllocations == 0);
}

static void testMalformedPacketsAndLimit(void) {
    resetTest();
    const uint8_t payload[1401] = {0};
    // Reject truncated headers before reading their fields, including extension truncation.
    for (int size = 0; size < (int)(sizeof(RTP_PACKET) + 4 + sizeof(NV_VIDEO_PACKET)); size++) {
        RTPV_QUEUE_ENTRY entry = {0};
        entry.length = size;
        entry.packet = trackedMalloc(size ? size : 1);
        memset(entry.packet, 0, size ? size : 1);
        if (size >= (int)sizeof(RTP_PACKET)) entry.packet->header = FLAG_EXTENSION;
        queueRtpPacket(&entry);
        assert(liveAllocations == 0 && nalChainHead == NULL);
    }
    sendPacket(1, 0, 0, 0, FLAG_SOF, true, payload, 0);
    sendPacket(1, 1, 0, 0, FLAG_SOF, true, payload, 1401);
    assert(liveAllocations == 0 && nalChainHead == NULL);
    int packet = 0;
    while ((size_t)nalChainDataLength + 1408 + (nalChainHead ? 0 : 4) <= PYRO_MAX_FRAME_BYTES) {
        sendPacket(1, packet++, 0, 0, 0, false, payload, 1400);
    }
    int remaining = PYRO_MAX_FRAME_BYTES - nalChainDataLength;
    assert(remaining > 8 && remaining <= 1408);
    sendPacket(1, packet++, 0, 0, 0, false, payload, remaining - 8);
    assert(nalChainDataLength == PYRO_MAX_FRAME_BYTES);
    PLENTRY before = nalChainHead;
    unsigned int previousSpi = lastPacketInStream;
    sendPacket(1, packet, 0, 0, FLAG_EOF, false, payload, 1);
    assert(nalChainHead == before && nalChainDataLength == PYRO_MAX_FRAME_BYTES);
    assert(lastPacketInStream == previousSpi);
    assert(largestAllocation == sizeof(LENTRY_INTERNAL) + PYRO_MAX_FRAME_BYTES);
    cleanupFrameState();
    assert(liveAllocations == 0);
}

static void testAllocationFailures(void) {
    resetTest();
    uint8_t payload[1400];
    memset(payload, 0x37, sizeof(payload));
    RTPV_QUEUE_ENTRY first = makePacket(1, 1, 0, 0, FLAG_SOF, true, payload, 1400, false);
    failAllocation = allocationCalls + 1;
    queueRtpPacket(&first);
    assert(liveAllocations == 0 && nalChainHead == NULL && nalChainDataLength == 0);
    finishPyroWaveFrame();
    assert(deliveredCount == 0);
    failAllocation = 0;
    // Lose a middle packet while growing. Existing bytes remain intact and the next accepted
    // packet is explicitly discontinuous, so the record parser can conceal/recover that gap.
    for (int i = 1; i <= 11; i++) sendPacket(2, i, 0, 0, i == 1 ? FLAG_SOF : 0, i == 1, payload, 1400);
    PLENTRY before = nalChainHead;
    int beforeLength = nalChainDataLength;
    uint8_t* copy = malloc(beforeLength);
    assert(copy);
    memcpy(copy, before->data, beforeLength);
    RTPV_QUEUE_ENTRY dropped = makePacket(2, 12, 0, 0, 0, false, payload, 1400, true);
    failAllocation = allocationCalls + 1;
    queueRtpPacket(&dropped);
    assert(liveAllocations == 1 && nalChainHead == before && nalChainDataLength == beforeLength);
    assert(lastPacketInStream == 11 && memcmp(before->data, copy, beforeLength) == 0);
    failAllocation = 0;
    sendPacket(2, 13, 0, 0, FLAG_EOF, true, payload, 1400);
    const uint8_t* recovered = (const uint8_t*)nalChainHead->data + beforeLength;
    assert(pyroReadLe32(recovered + 4) == (PYRO_PACKET_RECORD_START | PYRO_PACKET_FRAME_END));
    assert(memcmp(nalChainHead->data, copy, beforeLength) == 0);
    free(copy);
    // The decode-unit allocation can fail after a complete envelope was assembled.
    failAllocation = allocationCalls + 1;
    finishPyroWaveFrame();
    assert(deliveredCount == 0 && liveAllocations == 0 && nalChainHead == NULL);
    failAllocation = 0;
    sendPacket(3, 20, 0, 0, FLAG_SOF | FLAG_EOF, true, payload, 3);
    finishPyroWaveFrame();
    assert(deliveredCount == 1);
    LiCompleteVideoFrame(delivered[0], DR_OK);
    assert(liveAllocations == 0);
}

static void testQueueShutdownAndOverflow(void) {
    const uint8_t payload[] = {1, 2, 3};
    const int errors[] = {LBQ_INTERRUPTED, LBQ_BOUND_EXCEEDED};
    for (unsigned int i = 0; i < sizeof(errors) / sizeof(errors[0]); i++) {
        resetTest();
        sendPacket(1, 1, 0, 0, FLAG_SOF | FLAG_EOF, true, payload, sizeof(payload));
        queueResult = errors[i];
        finishPyroWaveFrame();
        assert(deliveredCount == 0 && liveAllocations == 0 && nalChainHead == NULL);
    }
    resetTest();
    sendPacket(1, 1, 0, 0, FLAG_SOF, true, payload, sizeof(payload));
    destroyVideoDepacketizer();
    assert(liveAllocations == 0 && nalChainHead == NULL);
}

static void testOrdinaryFramingRemainsUnchanged(void) {
    const int formats[] = {VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_PYROWAVE};
    const uint8_t payload[] = {1, 0, 0, 2, 12, 0, 0, 0, 0x31, 0x42, 0x53, 0x64};
    for (unsigned int i = 0; i < sizeof(formats) / sizeof(formats[0]); i++) {
        resetTest();
        PyroWaveRecordsEnabled = 0;
        NegotiatedVideoFormat = formats[i];
        AppVersionQuad[0] = 7;
        AppVersionQuad[1] = 1;
        AppVersionQuad[2] = 450;
        RTPV_QUEUE_ENTRY entry = makePacket(1, 0, 0, 0, FLAG_SOF | FLAG_EOF, false,
                                           payload, sizeof(payload), false);
        // The core borrows the queue entry stored after the RTP data in the packet allocation.
        const size_t entryOffset = (entry.length + sizeof(void*) - 1) & ~(sizeof(void*) - 1);
        void* packet = trackedMalloc(entryOffset + sizeof(RTPV_QUEUE_ENTRY));
        memcpy(packet, entry.packet, entry.length);
        trackedFree(entry.packet);
        entry.packet = packet;
        PRTPV_QUEUE_ENTRY queued = (PRTPV_QUEUE_ENTRY)((char*)packet + entryOffset);
        *queued = entry;
        queueRtpPacket(queued);
        assert(deliveredCount == 1 && completedFrames == 1 && envelopeAllocations == 0);
        PDECODE_UNIT du = &delivered[0]->decodeUnit;
        assert(du->fullLength == 4 && du->bufferList->next == NULL);
        assert(memcmp(du->bufferList->data, payload + 8, 4) == 0);
        assert(((PLENTRY_INTERNAL)du->bufferList)->allocPtr == packet);
        LiCompleteVideoFrame(delivered[0], DR_OK);
        assert(liveAllocations == 0);
    }
}

int main(void) {
    testExactWireBytesAndMetadata();
    testPartialAndFecBlockContinuity();
    testVibepolloRecordBoundaries();
    testGrowthAndQueuedOwnership();
    testMalformedPacketsAndLimit();
    testAllocationFailures();
    testQueueShutdownAndOverflow();
    testOrdinaryFramingRemainsUnchanged();
    puts("PyroWave depacketizer: framing, gaps, growth, limits, ownership and allocation failures passed");
    return 0;
}
