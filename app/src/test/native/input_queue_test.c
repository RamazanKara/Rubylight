#include "../../main/jni/moonlight-core/moonlight-common-c/src/InputStream.c"
#undef NDEBUG
#include <assert.h>

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4] = {7, 1, 431, 0};
uint32_t SunshineFeatureFlags = LI_FF_CONTROLLER_TOUCH_EVENTS;

int main(void) {
    assert(initializeInputStream() == 0);
    initialized = true;
    assert(LiSendControllerMotionEvent(0, 0, 0, 0, 0) == -3);
    assert(LiSendControllerMotionEvent(0, 3, 0, 0, 0) == -3);
    assert(LiSendControllerEvent(0, 0, 0, 100, 0, 0, 0) == 0);
    assert(currentQueuedControllerPacket[0] != NULL);
    LbqSignalQueueShutdown(&packetQueue);
    destroyInputStream();

    assert(initializeInputStream() == 0);
    assert(currentQueuedControllerPacket[0] == NULL);
    assert(LiSendControllerEvent(0, 0, 0, 200, 0, 0, 0) == 0);
    assert(LbqGetItemCount(&packetQueue) == 1);

    for (int i = 1; i < MAX_QUEUED_INPUT_PACKETS; i++) {
        assert(LiSendControllerEvent(i & 1, 0, 0, 200, 0, 0, 0) == 0);
    }
    assert(LiSendControllerEvent(0, 0, 0, 300, 0, 0, 0) == LBQ_BOUND_EXCEEDED);
    assert(currentQueuedControllerPacket[0] == NULL);
    assert(LiSendControllerEvent(0, 0, 0, 400, 0, 0, 0) == LBQ_BOUND_EXCEEDED);
    assert(currentQueuedControllerPacket[0] == NULL);
    LbqSignalQueueShutdown(&packetQueue);
    destroyInputStream();
    return 0;
}
