// Exercise the real receiver, input sender and termination shim without sockets,
// devices or a native stream. Thread dispatch is queued until the test drains it.
#include "Limelight-internal.h"
#include <pthread.h>
#include <stdatomic.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

// A local stop at STAGE_NONE must never enter any stream or platform teardown.
static void unexpectedTeardown(void) { CHECK(false); }
#define stopInputStream unexpectedTeardown
#define stopAudioStream unexpectedTeardown
#define stopVideoStream unexpectedTeardown
#define stopControlStream unexpectedTeardown
#define destroyInputStream unexpectedTeardown
#define destroyVideoStream unexpectedTeardown
#define destroyControlStream unexpectedTeardown
#define destroyAudioStream unexpectedTeardown
#define cleanupPlatform unexpectedTeardown
#include "Connection.c"
#undef stopInputStream
#undef stopAudioStream
#undef stopVideoStream
#undef stopControlStream
#undef destroyInputStream
#undef destroyVideoStream
#undef destroyControlStream
#undef destroyAudioStream
#undef cleanupPlatform

#undef LastSocketError
#define LastSocketError() 110
static SOCK_RET unexpectedSocketIo(SOCKET socket, const void* data, size_t length, int flags) { CHECK(false); return -1; }
#define send unexpectedSocketIo
#define recv unexpectedSocketIo
#include "ControlStream.c"
#define encryptedControlStream inputEncryptedControlStream
#include "InputStream.c"
#undef encryptedControlStream
#undef send
#undef recv

static ENetHost fakeHost;
static ENetPeer fakePeer;
static ENetPacket* receivedPacket;
static ENetEventType receivedEvent;
static int serviceResult;
static bool enetLocked, sendAtDisconnect;
static int sends, disconnects, callbacks, reportedReason;
static atomic_int queuedCallbacks;
static ThreadEntry queuedEntry;
static void* queuedContext;

static void reportTermination(int reason) {
    CHECK(!enetLocked);
    callbacks++;
    reportedReason = reason;
}

int PltCreateThread(const char* name, ThreadEntry entry, void* context, PLT_THREAD* thread) {
    CHECK(strcmp(name, "AsyncTerm") == 0);
    if (atomic_fetch_add(&queuedCallbacks, 1) == 0) {
        queuedEntry = entry;
        queuedContext = context;
    }
    return 0;
}

void PltDetachThread(PLT_THREAD* thread) { }
bool PltIsThreadInterrupted(PLT_THREAD* thread) { return false; }
void PltSleepMs(int ms) { CHECK(false); }

static void sendControllerPacket(void) {
    PACKET_HOLDER holder = {0};
    holder.packet.header.size = BE32(sizeof(NV_MULTI_CONTROLLER_PACKET) - sizeof(uint32_t));
    holder.channelId = CTRL_CHANNEL_GAMEPAD_BASE;
    CHECK(!sendInputPacket(&holder, false));
}

void PltLockMutex(PLT_MUTEX* mutex) {
    CHECK(mutex == &enetMutex && !enetLocked);
    enetLocked = true;
}

void PltUnlockMutex(PLT_MUTEX* mutex) {
    CHECK(mutex == &enetMutex && enetLocked);
    enetLocked = false;
    if (sendAtDisconnect && peer->state == ENET_PEER_STATE_DISCONNECTED) {
        // Deterministically schedule a queued controller send at the old race window.
        sendAtDisconnect = false;
        sendControllerPacket();
    }
}

ENetPacket* enet_packet_create(const void* data, size_t length, enet_uint32 flags) {
    ENetPacket* packet = calloc(1, sizeof(*packet));
    CHECK(packet != NULL);
    packet->data = calloc(1, length);
    CHECK(packet->data != NULL);
    packet->dataLength = length;
    packet->flags = flags;
    if (data) memcpy(packet->data, data, length);
    return packet;
}

void enet_packet_destroy(ENetPacket* packet) {
    if (packet->freeCallback) packet->freeCallback(packet);
    free(packet->data);
    free(packet);
}

int enet_peer_send(ENetPeer* target, enet_uint8 channel, ENetPacket* packet) {
    CHECK(enetLocked && target == peer);
    CHECK(target->state == ENET_PEER_STATE_DISCONNECTED);
    CHECK(channel == CTRL_CHANNEL_GAMEPAD_BASE);
    sends++;
    return -1;
}

void enet_peer_disconnect_now(ENetPeer* target, enet_uint32 data) {
    CHECK(enetLocked && target == peer && data == 0);
    target->state = ENET_PEER_STATE_DISCONNECTED;
    disconnects++;
}

int serviceEnetHost(ENetHost* host, ENetEvent* event, enet_uint32 timeout) {
    // A held-back host disconnect first waits 100 ms for receives still in flight.
    CHECK(enetLocked && host == client && (timeout == 0 || (timeout == 100 && disconnectPending)));
    event->type = receivedEvent;
    event->packet = receivedPacket;
    receivedPacket = NULL;
    return serviceResult;
}

int enet_host_service(ENetHost* host, ENetEvent* event, enet_uint32 timeout) { CHECK(false); return -1; }
int enet_socket_wait(ENetSocket socket, enet_uint32* condition, enet_uint32 timeout) { CHECK(false); return -1; }
void enet_host_flush(ENetHost* host) { CHECK(false); }
enet_uint32 enet_time_get(void) { CHECK(false); return 0; }
int LbqOfferQueueItem(PLINKED_BLOCKING_QUEUE queue, void* data, PLINKED_BLOCKING_QUEUE_ENTRY entry) { CHECK(false); return -1; }
void LbqSignalQueueUserWake(PLINKED_BLOCKING_QUEUE queue) { CHECK(false); }
void BridgeClControllerHaptics(uint16_t controller, uint32_t sequence, const uint8_t* pcm, uint16_t frames) { CHECK(false); }
void BridgeClControllerAudio(uint16_t kind, const uint8_t* payload, int length) { CHECK(false); }
void BridgeClHostTiming(const uint8_t* payload, int length) { CHECK(false); }
void BridgeClCursorShape(const uint8_t* payload, int length) { CHECK(false); }
void BridgeClClockSync(const uint8_t* payload, int length) { CHECK(false); }

bool PltEncryptMessage(PPLT_CRYPTO_CONTEXT ctx, int algorithm, int flags,
        unsigned char* key, int keyLength, unsigned char* iv, int ivLength,
        unsigned char* tag, int tagLength, unsigned char* input, int inputLength,
        unsigned char* output, int* outputLength) { CHECK(false); return false; }
bool PltDecryptMessage(PPLT_CRYPTO_CONTEXT ctx, int algorithm, int flags,
        unsigned char* key, int keyLength, unsigned char* iv, int ivLength,
        unsigned char* tag, int tagLength, unsigned char* input, int inputLength,
        unsigned char* output, int* outputLength) { CHECK(false); return false; }

static void reset(void) {
    memset(&fakeHost, 0, sizeof(fakeHost));
    memset(&fakePeer, 0, sizeof(fakePeer));
    client = &fakeHost;
    peer = &fakePeer;
    peer->state = ENET_PEER_STATE_CONNECTED;
    peer->channelCount = CTRL_CHANNEL_COUNT;
    AppVersionQuad[0] = 7;
    AppVersionQuad[3] = -1;
    // Use plaintext fixtures to test termination parsing independently of encryption.
    encryptedControlStream = false;
    inputEncryptedControlStream = true;
    packetTypes = (short*)packetTypesGen7Enc;
    lastSeenFrame = 1;
    ConnectionInterrupted = false;
    alreadyTerminated = false;
    terminationCallbackErrorCode = 12345;
    originalTerminationCallback = reportTermination;
    memset(&ListenerCallbacks, 0, sizeof(ListenerCallbacks));
    ListenerCallbacks.connectionTerminated = ClInternalConnectionTerminated;
    enetLocked = sendAtDisconnect = false;
    sends = disconnects = callbacks = 0;
    reportedReason = 12345;
    atomic_store(&queuedCallbacks, 0);
    queuedEntry = NULL;
    queuedContext = NULL;
    receivedEvent = ENET_EVENT_TYPE_RECEIVE;
    serviceResult = 1;
    receivedPacket = NULL;
    disconnectPending = disconnectPendingUnsequenced = false;
}

static void drainCallback(int expectedReason) {
    CHECK(atomic_load(&queuedCallbacks) == 1);
    CHECK(queuedEntry != NULL && callbacks == 0 && !enetLocked);
    queuedEntry(queuedContext);
    CHECK(callbacks == 1);
    if (reportedReason != expectedReason) {
        fprintf(stderr, "Expected termination %d, received %d\n", expectedReason, reportedReason);
        exit(1);
    }
}

static void hostTermination(uint32_t reason, bool extended, bool sawFrame, int expectedReason) {
    reset();
    lastSeenFrame = sawFrame ? 1 : 0;
    sendAtDisconnect = true;
    receivedPacket = enet_packet_create(NULL, extended ? 6 : 4, 0);
    uint16_t type = LE16(packetTypes[IDX_TERMINATION]);
    memcpy(receivedPacket->data, &type, sizeof(type));
    if (extended) {
        uint32_t encoded = BE32(reason);
        memcpy(receivedPacket->data + 2, &encoded, sizeof(encoded));
    } else {
        uint16_t encoded = LE16(reason);
        memcpy(receivedPacket->data + 2, &encoded, sizeof(encoded));
    }
    controlReceiveThreadFunc(NULL);
    CHECK(disconnects == 1 && sends == 1 && !sendAtDisconnect);
    drainCallback(expectedReason);
}

static pthread_barrier_t claimBarrier;
#define CLAIM_ROUNDS 128

static void barrier(void) {
    int result = pthread_barrier_wait(&claimBarrier);
    CHECK(result == 0 || result == PTHREAD_BARRIER_SERIAL_THREAD);
}

static void* compete(void* context) {
    int reason = *(int*)context;
    for (int i = 0; i < CLAIM_ROUNDS; i++) {
        barrier();
        ClInternalConnectionTerminated(reason);
        barrier();
    }
    return NULL;
}

static void concurrentClaims(void) {
    pthread_t workers[2];
    int reasons[2] = {-1, ML_ERROR_FRAME_CONVERSION};
    CHECK(pthread_barrier_init(&claimBarrier, NULL, 3) == 0);
    for (int i = 0; i < 2; i++) CHECK(pthread_create(&workers[i], NULL, compete, &reasons[i]) == 0);
    for (int i = 0; i < CLAIM_ROUNDS; i++) {
        reset();
        barrier();
        barrier();
        CHECK(atomic_load(&queuedCallbacks) == 1);
        CHECK(terminationCallbackErrorCode == reasons[0] || terminationCallbackErrorCode == reasons[1]);
        drainCallback(terminationCallbackErrorCode);
    }
    for (int i = 0; i < 2; i++) CHECK(pthread_join(workers[i], NULL) == 0);
    CHECK(pthread_barrier_destroy(&claimBarrier) == 0);
}

int main(void) {
    hostTermination(0x80030023, true, true, ML_ERROR_GRACEFUL_TERMINATION);
    hostTermination(0x0100, false, true, ML_ERROR_GRACEFUL_TERMINATION);
    hostTermination(0x80030023, true, false, ML_ERROR_UNEXPECTED_EARLY_TERMINATION);
    hostTermination(0x0100, false, false, ML_ERROR_UNEXPECTED_EARLY_TERMINATION);
    hostTermination(0x800e9403, true, true, ML_ERROR_FRAME_CONVERSION);
    hostTermination(0x800e9302, true, true, ML_ERROR_PROTECTED_CONTENT);
    hostTermination(0x80030042, true, true, (int)0x80030042);
    hostTermination(0x0123, false, true, 0x0123);

    // Transport failures with no host reason must retain their error behavior. A host's own
    // disconnect never reaches the receiver as an event ahead of its queued end message; the
    // real ENet exchange is covered by enet_termination_test.c.
    reset();
    peer->state = ENET_PEER_STATE_DISCONNECTED;
    sendControllerPacket();
    drainCallback(-1);
    // ENet reports a peer that stopped answering as a DISCONNECT event.
    reset();
    receivedEvent = ENET_EVENT_TYPE_DISCONNECT;
    controlReceiveThreadFunc(NULL);
    drainCallback(-1);
    // A host that calls enet_peer_disconnect_now() without an end message: once no receive
    // is left, -1 is reserved before the peer is released, so queued input cannot replace it.
    reset();
    client->intercept = ignoreDisconnectIntercept;
    disconnectPending = disconnectPendingUnsequenced = true;
    serviceResult = 0;
    sendAtDisconnect = true;
    controlReceiveThreadFunc(NULL);
    CHECK(disconnects == 1 && sends == 1 && !sendAtDisconnect);
    drainCallback(-1);
    reset();
    serviceResult = -1;
    controlReceiveThreadFunc(NULL);
    drainCallback(110);

    reset();
    LiStopConnection();
    ClInternalConnectionTerminated(-1);
    CHECK(atomic_load(&queuedCallbacks) == 0);
    reset();
    LiInterruptConnection();
    ClInternalConnectionTerminated(-1);
    CHECK(atomic_load(&queuedCallbacks) == 0);
    reset();
    ClInternalConnectionTerminated(ML_ERROR_GRACEFUL_TERMINATION);
    ClInternalConnectionTerminated(-1);
    drainCallback(ML_ERROR_GRACEFUL_TERMINATION);
    reset();
    ClInternalConnectionTerminated(-1);
    ClInternalConnectionTerminated(ML_ERROR_GRACEFUL_TERMINATION);
    drainCallback(-1);

    concurrentClaims();
    puts("control termination: all tests passed");
    return 0;
}
