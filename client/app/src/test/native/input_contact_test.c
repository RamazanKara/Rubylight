// While the control stream reports lost contact (a network grace period riding out a silent
// network), the input send thread must not queue input in ENet: once the network returned, ENet
// would deliver all of it, however old. It keeps the latest state of each key, mouse button and
// controller instead and sends only that when the host is heard again, releases first.
#include "../../main/jni/moonlight-core/moonlight-common-c/src/InputStream.c"
#include <stdatomic.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4] = {7, 1, 431, -1};
uint32_t SunshineFeatureFlags = LI_FF_PEN_TOUCH_EVENTS;

static atomic_bool contactLost;
bool isControlContactLost(void) { return atomic_load(&contactLost); }

#define MAX_SENT 64
static struct {
    uint32_t magic;
    uint8_t data[MAX_INPUT_PACKET_SIZE];
} sent[MAX_SENT];
static atomic_int sentCount;

int sendInputPacketOnControlStream(unsigned char* data, int length, uint8_t channelId, uint32_t flags, bool moreData) {
    int index = atomic_load(&sentCount);
    CHECK(!isControlContactLost());
    CHECK(index < MAX_SENT && length <= MAX_INPUT_PACKET_SIZE);
    memcpy(sent[index].data, data, length);
    sent[index].magic = LE32(((PNV_INPUT_HEADER)data)->magic);
    atomic_store(&sentCount, index + 1);
    return 0;
}
void flushInputOnControlStream(void) { }
bool isControlDataInTransit(void) { return false; }

static void waitForIdle(void) {
    for (int i = 0; i < 200; i++) {
        PltSleepMs(10);
        if (LbqGetItemCount(&packetQueue) == 0) {
            break;
        }
    }
    PltSleepMs(50);
    CHECK(LbqGetItemCount(&packetQueue) == 0);
}

static void checkKey(int index, uint32_t magic, short keyCode) {
    PNV_KEYBOARD_PACKET packet = (PNV_KEYBOARD_PACKET)sent[index].data;
    CHECK(sent[index].magic == magic);
    CHECK(LE16(packet->keyCode) == keyCode);
}

int main(void) {
    CHECK(initializeInputStream() == 0);
    CHECK(PltCreateThread("InputSend", inputSendThreadProc, NULL, &inputSendThread) == 0);
    initialized = true;

    // In contact: sent straight away.
    CHECK(LiSendKeyboardEvent(0x41, KEY_ACTION_DOWN, 0) == 0);
    waitForIdle();
    CHECK(atomic_load(&sentCount) == 1);
    checkKey(0, KEY_DOWN_EVENT_MAGIC, 0x41);

    // The host goes silent: a long burst of input never reaches ENet and never fills the queue.
    atomic_store(&contactLost, true);
    CHECK(LiSendKeyboardEvent(0x41, KEY_ACTION_UP, 0) == 0);
    CHECK(LiSendKeyboardEvent(0x42, KEY_ACTION_DOWN, 0) == 0);
    CHECK(LiSendKeyboardEvent(0x43, KEY_ACTION_DOWN, 0) == 0);
    CHECK(LiSendKeyboardEvent(0x43, KEY_ACTION_UP, 0) == 0);
    CHECK(LiSendMouseButtonEvent(BUTTON_ACTION_PRESS, BUTTON_LEFT) == 0);
    CHECK(LiSendMouseButtonEvent(BUTTON_ACTION_PRESS, BUTTON_RIGHT) == 0);
    CHECK(LiSendMouseButtonEvent(BUTTON_ACTION_RELEASE, BUTTON_RIGHT) == 0);
    CHECK(LiSendTouchEvent(LI_TOUCH_EVENT_DOWN, 3, 0.5f, 0.5f, 1.0f, 0, 0, LI_ROT_UNKNOWN) == 0);
    CHECK(LiSendUtf8TextEvent("stale", 5) == 0);
    for (int i = 1; i <= 1000; i++) {
        int err;
        while ((err = LiSendMouseMoveEvent(1, 1)) == LBQ_BOUND_EXCEEDED) PltSleepMs(1);
        CHECK(err == 0);
        while ((err = LiSendControllerEvent(i == 1000 ? A_FLAG : 0, 0, 0, (short)i, 0, 0, 0)) == LBQ_BOUND_EXCEEDED) PltSleepMs(1);
        CHECK(err == 0);
        if (i % 100 == 0) PltSleepMs(1);
    }
    waitForIdle();
    CHECK(atomic_load(&sentCount) == 1);
    CHECK(inputHeldForContact);

    // The host is heard again: the control stream wakes the input thread, which sends only the
    // current state, releases first, and cancels the touch that went down during the silence.
    atomic_store(&contactLost, false);
    wakeInputAfterContactLoss();
    waitForIdle();
    CHECK(!inputHeldForContact);
    CHECK(atomic_load(&sentCount) == 1 + 7);
    checkKey(1, KEY_UP_EVENT_MAGIC, 0x41);
    checkKey(2, KEY_UP_EVENT_MAGIC, 0x43);
    CHECK(sent[3].magic == MOUSE_BUTTON_UP_EVENT_MAGIC_GEN5 && ((PNV_MOUSE_BUTTON_PACKET)sent[3].data)->button == BUTTON_RIGHT);
    checkKey(4, KEY_DOWN_EVENT_MAGIC, 0x42);
    CHECK(sent[5].magic == MOUSE_BUTTON_DOWN_EVENT_MAGIC_GEN5 && ((PNV_MOUSE_BUTTON_PACKET)sent[5].data)->button == BUTTON_LEFT);
    CHECK(sent[6].magic == MULTI_CONTROLLER_MAGIC_GEN5);
    PNV_MULTI_CONTROLLER_PACKET controller = (PNV_MULTI_CONTROLLER_PACKET)sent[6].data;
    CHECK(LE16(controller->leftStickX) == 1000 && LE16(controller->buttonFlags) == A_FLAG);
    CHECK(sent[7].magic == SS_TOUCH_MAGIC && ((PSS_TOUCH_PACKET)sent[7].data)->eventType == LI_TOUCH_EVENT_CANCEL_ALL);

    // A second wake with nothing held sends nothing, and new input flows normally.
    wakeInputAfterContactLoss();
    CHECK(LiSendKeyboardEvent(0x44, KEY_ACTION_DOWN, 0) == 0);
    waitForIdle();
    CHECK(atomic_load(&sentCount) == 9);
    checkKey(8, KEY_DOWN_EVENT_MAGIC, 0x44);

    // Regained contact noticed by the next input packet rather than the wake: held state first.
    atomic_store(&contactLost, true);
    CHECK(LiSendKeyboardEvent(0x44, KEY_ACTION_UP, 0) == 0);
    waitForIdle();
    atomic_store(&contactLost, false);
    CHECK(LiSendKeyboardEvent(0x45, KEY_ACTION_DOWN, 0) == 0);
    waitForIdle();
    CHECK(atomic_load(&sentCount) == 11);
    checkKey(9, KEY_UP_EVENT_MAGIC, 0x44);
    checkKey(10, KEY_DOWN_EVENT_MAGIC, 0x45);

    initialized = false;
    LbqSignalQueueShutdown(&packetHolderFreeList);
    LbqSignalQueueDrain(&packetQueue);
    PltJoinThread(&inputSendThread);
    destroyInputStream();
    puts("input contact: all tests passed");
    return 0;
}
