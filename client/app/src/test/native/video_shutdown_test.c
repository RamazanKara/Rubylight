#include "../../main/jni/moonlight-core/moonlight-common-c/src/Limelight-internal.h"

static int freedBuffers;
static int completedFrames;

static void trackedFree(void* ptr) {
    if (ptr) freedBuffers++;
    free(ptr);
}

#define free trackedFree
#include "../../main/jni/moonlight-core/moonlight-common-c/src/VideoDepacketizer.c"
#undef free
#undef NDEBUG
#include <assert.h>

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
DECODER_RENDERER_CALLBACKS VideoCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4];
int NegotiatedVideoFormat;

uint64_t PltGetMicroseconds(void) { return 1000000; }
bool LiGetCurrentHostDisplayHdrMode(void) { return false; }
void notifyKeyFrameReceived(void) { }
void LiRequestIdrFrame(void) { }
void connectionReceivedCompleteFrame(uint32_t frameIndex, bool isLTR) { completedFrames++; }
bool isReferenceFrameInvalidationEnabled(void) { return false; }
void connectionDetectedFrameLoss(uint32_t start, uint32_t end) { abort(); }

int LbqInitializeLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue, int bound) { abort(); }
void LbqSignalQueueShutdown(PLINKED_BLOCKING_QUEUE queue) { abort(); }
void LbqSignalQueueUserWake(PLINKED_BLOCKING_QUEUE queue) { abort(); }
PLINKED_BLOCKING_QUEUE_ENTRY LbqDestroyLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue) { abort(); }
int LbqWaitForQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { abort(); }
int LbqPollQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { abort(); }
int LbqPeekQueueElement(PLINKED_BLOCKING_QUEUE queue, void** data) { abort(); }
int LbqGetItemCount(PLINKED_BLOCKING_QUEUE queue) { abort(); }

int LbqOfferQueueItem(PLINKED_BLOCKING_QUEUE queue, void* data, PLINKED_BLOCKING_QUEUE_ENTRY entry) {
    return LBQ_INTERRUPTED;
}

PLINKED_BLOCKING_QUEUE_ENTRY LbqFlushQueueItems(PLINKED_BLOCKING_QUEUE queue) {
    return NULL;
}

int main(void) {
    PLENTRY_INTERNAL entry = calloc(1, sizeof(*entry));
    assert(entry != NULL);
    entry->allocPtr = entry;
    entry->entry.bufferType = BUFFER_TYPE_PICDATA;
    entry->entry.length = 1;
    nalChainHead = nalChainTail = &entry->entry;
    nalChainDataLength = 1;
    frameType = FRAME_TYPE_PFRAME;
    reassembleFrame(1, false);
    assert(freedBuffers == 2);
    assert(completedFrames == 0);
    assert(nalChainHead == NULL && nalChainTail == NULL && nalChainDataLength == 0);
    return 0;
}
