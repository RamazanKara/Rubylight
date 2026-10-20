// Drives the video receive path the Android app builds: the patched RtpVideoQueue through
// android_rtp_video.c and the patched VideoDepacketizer through android_depacketizer.c. The
// receive step mirrors VideoReceiveThreadProc in VideoStream.c without the socket.
#include "android_depacketizer.c"
#include "shim.h"

// Globals the core expects from Connection.c, Misc.c and friends.
CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
DECODER_RENDERER_CALLBACKS VideoCallbacks;
AUDIO_RENDERER_CALLBACKS AudioCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4];
int NegotiatedVideoFormat;
int PyroWaveRecordsSupported;
int PyroWaveRecordsEnabled;
int AudioPacketDuration;
bool AudioEncryptionEnabled;

uint64_t dtNow;
static int dtRfi;
static int dtHdr;
static int dtQueueBound;
static int dtEncrypted;
static RTP_VIDEO_QUEUE rtpQueue;
static PPLT_CRYPTO_CONTEXT decryptionCtx;

// Log

static uint8_t* logData;
static size_t logLength, logCapacity;

void dtLogBytes(const void* data, size_t length) {
    if (logLength + length > logCapacity) {
        logCapacity = (logLength + length) * 2 + 4096;
        logData = realloc(logData, logCapacity);
    }
    memcpy(logData + logLength, data, length);
    logLength += length;
}

void dtLogU8(uint8_t v) { dtLogBytes(&v, 1); }
void dtLogU16(uint16_t v) { dtLogBytes(&v, 2); }
void dtLogU32(uint32_t v) { dtLogBytes(&v, 4); }
void dtLogU64(uint64_t v) { dtLogBytes(&v, 8); }

const uint8_t* dt_log(size_t* length) {
    *length = logLength;
    return logData;
}

void dt_log_clear(void) { logLength = 0; }

// Platform and control stream hooks

uint64_t PltGetMicroseconds(void) { return dtNow; }
uint64_t PltGetMillis(void) { return dtNow / 1000; }
bool LiGetCurrentHostDisplayHdrMode(void) { return dtHdr; }
bool isReferenceFrameInvalidationEnabled(void) { return dtRfi; }
void notifyKeyFrameReceived(void) { }

void LiRequestIdrFrame(void) { dtLogU8('I'); }

void connectionDetectedFrameLoss(uint32_t startFrame, uint32_t endFrame) {
    dtLogU8('R');
    dtLogU32(startFrame);
    dtLogU32(endFrame);
}

void connectionReceivedCompleteFrame(uint32_t frameIndex, bool frameIsLTR) {
    dtLogU8('C');
    dtLogU32(frameIndex);
    dtLogU8(frameIsLTR);
}

void connectionSawFrame(uint32_t frameIndex) {
    dtLogU8('W');
    dtLogU32(frameIndex);
}

void connectionSendFrameFecStatus(PSS_FRAME_FEC_STATUS fecStatus) {
    dtLogU8('S');
    dtLogBytes(fecStatus, sizeof(*fecStatus));
}

// A single-threaded LBQ: the decode unit queue is the only one in this translation unit.

int LbqInitializeLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue, int bound) {
    memset(queue, 0, sizeof(*queue));
    queue->sizeBound = dtQueueBound ? dtQueueBound : bound;
    return LBQ_SUCCESS;
}

static void logFrame(PDECODE_UNIT du) {
    dtLogU8('F');
    dtLogU32((uint32_t)du->frameNumber);
    dtLogU8((uint8_t)du->frameType);
    dtLogU16(du->frameHostProcessingLatency);
    dtLogU64(du->receiveTimeUs);
    dtLogU64(du->enqueueTimeUs);
    dtLogU64(du->presentationTimeUs);
    dtLogU32(du->rtpTimestamp);
    dtLogU8(du->hdrActive);
    dtLogU8(du->colorspace);
    uint32_t count = 0;
    for (PLENTRY e = du->bufferList; e; e = e->next) count++;
    dtLogU32(count);
    for (PLENTRY e = du->bufferList; e; e = e->next) {
        dtLogU8((uint8_t)e->bufferType);
        dtLogU32((uint32_t)e->length);
        dtLogBytes(e->data, (size_t)e->length);
    }
}

int LbqOfferQueueItem(PLINKED_BLOCKING_QUEUE queue, void* data, PLINKED_BLOCKING_QUEUE_ENTRY entry) {
    if (queue->currentSize == queue->sizeBound) return LBQ_BOUND_EXCEEDED;
    logFrame(&((PQUEUED_DECODE_UNIT)data)->decodeUnit);
    entry->flink = NULL;
    entry->blink = queue->tail;
    entry->data = data;
    if (queue->tail) queue->tail->flink = entry; else queue->head = entry;
    queue->tail = entry;
    queue->currentSize++;
    return LBQ_SUCCESS;
}

int LbqPollQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) {
    if (!queue->head) return LBQ_NO_ELEMENT;
    PLINKED_BLOCKING_QUEUE_ENTRY entry = queue->head;
    queue->head = entry->flink;
    if (!queue->head) queue->tail = NULL;
    queue->currentSize--;
    *data = entry->data;
    return LBQ_SUCCESS;
}

int LbqWaitForQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { return LbqPollQueueElement(queue, data); }

int LbqPeekQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) {
    if (!queue->head) return LBQ_NO_ELEMENT;
    *data = queue->head->data;
    return LBQ_SUCCESS;
}

PLINKED_BLOCKING_QUEUE_ENTRY LbqFlushQueueItems(PLINKED_BLOCKING_QUEUE queue) {
    dtLogU8('X');
    PLINKED_BLOCKING_QUEUE_ENTRY head = queue->head;
    queue->head = queue->tail = NULL;
    queue->currentSize = 0;
    return head;
}

PLINKED_BLOCKING_QUEUE_ENTRY LbqDestroyLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue) {
    PLINKED_BLOCKING_QUEUE_ENTRY head = queue->head;
    queue->head = queue->tail = NULL;
    queue->currentSize = 0;
    return head;
}

void LbqSignalQueueShutdown(PLINKED_BLOCKING_QUEUE queue) { (void)queue; }
void LbqSignalQueueUserWake(PLINKED_BLOCKING_QUEUE queue) { (void)queue; }
int LbqGetItemCount(PLINKED_BLOCKING_QUEUE queue) { return queue->currentSize; }

// Harness

void dt_video_init(const dt_video_config* config) {
    memset(&StreamConfig, 0, sizeof(StreamConfig));
    StreamConfig.packetSize = config->packet_size;
    StreamConfig.colorSpace = config->colorspace;
    memcpy(StreamConfig.remoteInputAesKey, config->key, 16);
    memcpy(AppVersionQuad, config->app_version, sizeof(AppVersionQuad));
    NegotiatedVideoFormat = config->format;
    PyroWaveRecordsSupported = PyroWaveRecordsEnabled = config->records;
    dtRfi = config->rfi;
    dtHdr = 0;
    dtQueueBound = config->queue_bound;
    dtEncrypted = config->encrypted;
    dtNow = 0;

    // Statics that survive LiStopConnection in the C core.
    consecutiveFrameDrops = 0;
    pyroLastBlock = 0;
    pyroLastBlockEnded = false;
    pyroEnvelopeCapacity = 0;

    initializeVideoDepacketizer(config->packet_size);
    RtpvInitializeQueue(&rtpQueue);
    decryptionCtx = PltCreateCryptoContext();
    dt_log_clear();
}

void dt_video_destroy(void) {
    PltDestroyCryptoContext(decryptionCtx);
    destroyVideoDepacketizer();
    RtpvCleanupQueue(&rtpQueue);
}

void dt_video_receive(const uint8_t* data, int length, uint64_t now_us) {
    dtNow = now_us;
    int decryptedSize = StreamConfig.packetSize + MAX_RTP_HEADER_SIZE;
    int minSize = sizeof(RTP_PACKET) + (dtEncrypted ? sizeof(ENC_VIDEO_HEADER) : 0);
    int receiveSize = decryptedSize + (dtEncrypted ? sizeof(ENC_VIDEO_HEADER) : 0);
    int bufferSize = decryptedSize + sizeof(RTPV_QUEUE_ENTRY);

    // recv() truncates a datagram to the buffer.
    int err = length < receiveSize ? length : receiveSize;
    if (err < minSize) return;

    char* buffer = malloc(bufferSize);
    if (dtEncrypted) {
        uint8_t* encryptedBuffer = malloc(receiveSize);
        memcpy(encryptedBuffer, data, err);
        PENC_VIDEO_HEADER encHeader = (PENC_VIDEO_HEADER)encryptedBuffer;
        if (encHeader->frameNumber && LE32(encHeader->frameNumber) < RtpvGetCurrentFrameNumber(&rtpQueue)) {
            free(encryptedBuffer);
            free(buffer);
            return;
        }
        if (!PltDecryptMessage(decryptionCtx, ALGORITHM_AES_GCM, 0,
                               (unsigned char*)StreamConfig.remoteInputAesKey, sizeof(StreamConfig.remoteInputAesKey),
                               encHeader->iv, sizeof(encHeader->iv),
                               encHeader->tag, sizeof(encHeader->tag),
                               ((unsigned char*)(encHeader + 1)), err - sizeof(ENC_VIDEO_HEADER),
                               (unsigned char*)buffer, &err)) {
            free(encryptedBuffer);
            free(buffer);
            return;
        }
        free(encryptedBuffer);
    }
    else {
        memcpy(buffer, data, err);
    }

    PRTP_PACKET packet = (PRTP_PACKET)&buffer[0];
    packet->sequenceNumber = BE16(packet->sequenceNumber);
    packet->timestamp = BE32(packet->timestamp);
    packet->ssrc = BE32(packet->ssrc);

    if (RtpvAddPacket(&rtpQueue, packet, err, (PRTPV_QUEUE_ENTRY)&buffer[decryptedSize]) != RTPF_RET_QUEUED) {
        free(buffer);
    }
}

void dt_video_complete_all(int status) {
    VIDEO_FRAME_HANDLE handle;
    PDECODE_UNIT du;
    while (LiPollNextVideoFrame(&handle, &du)) {
        LiCompleteVideoFrame(handle, status);
    }
}

void dt_video_set_hdr(int active) { dtHdr = active; }

uint32_t dt_video_current_frame(void) { return RtpvGetCurrentFrameNumber(&rtpQueue); }

void dt_video_stats(uint32_t* video, uint32_t* fec) {
    *video = rtpQueue.stats.packetCountVideo;
    *fec = rtpQueue.stats.packetCountFec;
}
