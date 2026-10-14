// The real JNI control senders and encrypted ENet send path against fake ENet and JNI:
// DualSense microphone packets larger than the old 256-byte plaintext buffer, Java-thread
// sends racing stream teardown, and phase-lock reports that need the host's announcement.
#include <pthread.h>
#include <unistd.h>
#include "android_control.c"

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

int AppVersionQuad[4];
STREAM_CONFIGURATION StreamConfig;
CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
volatile bool ConnectionInterrupted;
uint32_t EncryptionFeaturesEnabled;

static JNIEnv env = &fakeJniInterface;
static ENetHost fakeHost;
static ENetPeer fakePeer;

#define MAX_SENT 8
static ENetPacket* sent[MAX_SENT];
static uint8_t sentChannels[MAX_SENT];
static int sentCount, teardowns;
static atomic_bool tornDown, senderInside;
static uint64_t fakeNowMs;

// One enet_host_service() call can be held to keep a sender inside the send path.
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t gateChanged = PTHREAD_COND_INITIALIZER;
static bool holdNextService, serviceReleased;

void PltLockMutex(PLT_MUTEX* mutex) { CHECK(pthread_mutex_lock(mutex) == 0); }
void PltUnlockMutex(PLT_MUTEX* mutex) { CHECK(pthread_mutex_unlock(mutex) == 0); }
void PltSleepMs(int ms) { usleep((useconds_t)ms * 1000); }
uint64_t PltGetMillis(void) { return fakeNowMs; }
void PltSetEvent(PLT_EVENT* event) { }
void PltInterruptThread(PLT_THREAD* thread) { }
void PltJoinThread(PLT_THREAD* thread) { }
void LbqSignalQueueShutdown(PLINKED_BLOCKING_QUEUE queue) { }
void LbqSignalQueueDrain(PLINKED_BLOCKING_QUEUE queue) { }
bool isReferenceFrameInvalidationEnabled(void) { return false; }
void shutdownTcpSocket(SOCKET s) { CHECK(false); }
void closeSocket(SOCKET s) { CHECK(false); }

// The identity "cipher" leaves the plaintext readable behind the header and tag.
bool PltEncryptMessage(PPLT_CRYPTO_CONTEXT ctx, int algorithm, int flags,
        unsigned char* key, int keyLength, unsigned char* iv, int ivLength,
        unsigned char* tag, int tagLength, unsigned char* input, int inputLength,
        unsigned char* output, int* outputLength) {
    CHECK(tagLength == AES_GCM_TAG_LENGTH && inputLength >= (int)sizeof(NVCTL_ENET_PACKET_HEADER_V2));
    memset(tag, 0xA5, (size_t)tagLength);
    memcpy(output, input, (size_t)inputLength);
    *outputLength = inputLength;
    return true;
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
    CHECK(!atomic_load(&tornDown) && target != NULL && target == peer);
    CHECK(sentCount < MAX_SENT);
    sentChannels[sentCount] = channel;
    sent[sentCount++] = packet;
    return 0;
}

int enet_host_service(ENetHost* host, ENetEvent* event, enet_uint32 timeout) {
    CHECK(!atomic_load(&tornDown) && host != NULL && host == client && event == NULL && timeout == 0);
    pthread_mutex_lock(&gate);
    if (holdNextService) {
        holdNextService = false;
        atomic_store(&senderInside, true);
        pthread_cond_broadcast(&gateChanged);
        while (!serviceReleased) pthread_cond_wait(&gateChanged, &gate);
        atomic_store(&senderInside, false);
    }
    pthread_mutex_unlock(&gate);
    return 0;
}

int gracefullyDisconnectEnetPeer(ENetHost* host, ENetPeer* target, enet_uint32 lingerTimeoutMs) {
    // Teardown must never run while a Java thread is still inside the send path.
    CHECK(!atomic_load(&senderInside));
    CHECK(host == &fakeHost && target == &fakePeer);
    teardowns++;
    return 0;
}

void enet_host_destroy(ENetHost* host) {
    CHECK(!atomic_load(&senderInside) && host == &fakeHost);
    atomic_store(&tornDown, true);
}

// Phase-lock windows always have a report ready in this test.
RpSlackWindow* rp_slack_window_new(void) { return (RpSlackWindow*)&fakeHost; }
void rp_slack_window_free(RpSlackWindow* window) { }
void rp_slack_window_push(RpSlackWindow* window, int64_t slack_ns, int64_t period_ns) { }
bool rp_slack_window_report(RpSlackWindow* window, size_t min_frames, uint8_t* out) {
    memset(out, 0, RP_PHASE_REPORT_BYTES);
    out[8] = 0x10;
    return true;
}

static void freeSent(void) {
    for (int i = 0; i < sentCount; i++) enet_packet_destroy(sent[i]);
    sentCount = 0;
}

static void reset(void) {
    freeSent();
    memset(&fakeHost, 0, sizeof(fakeHost));
    memset(&fakePeer, 0, sizeof(fakePeer));
    enet_list_clear(&fakePeer.sentReliableCommands);
    fakePeer.state = ENET_PEER_STATE_CONNECTED;
    fakePeer.channelCount = CTRL_CHANNEL_COUNT;
    client = &fakeHost;
    peer = &fakePeer;
    stopping = false;
    atomic_store(&tornDown, false);
    atomic_store(&senderInside, false);
    teardowns = 0;
    holdNextService = serviceReleased = false;
    // A Sunshine host with the encrypted control stream.
    AppVersionQuad[0] = 7; AppVersionQuad[1] = 1; AppVersionQuad[2] = 431; AppVersionQuad[3] = -1;
    encryptedControlStream = true;
    EncryptionFeaturesEnabled = SS_ENC_CONTROL_V2;
    resetHostControlMessages();
    resetPhaseLockReports();
    fakeNowMs = 1000;
}

static void announce(const uint16_t* ids, int count) {
    setHostControlMessages(ids, count);
}

// The decrypted V2 header and payload behind the encrypted header and tag.
static const uint8_t* plaintext(const ENetPacket* packet, uint16_t* type, uint16_t* length) {
    const uint8_t* body = packet->data + sizeof(NVCTL_ENCRYPTED_PACKET_HEADER) + AES_GCM_TAG_LENGTH;
    *type = MlHapticsRead16(body);
    *length = MlHapticsRead16(body + 2);
    return body + 4;
}

static jboolean sendAudio(int kind, const uint8_t* bytes, int length) {
    FakeByteArray array = {length, bytes};
    return Java_com_limelight_nvstream_jni_MoonBridge_sendControllerAudio(&env, NULL, kind, (jbyteArray)&array);
}

static void microphonePacketsLargerThanTheStackBufferArriveIntact(void) {
    reset();
    const uint16_t audio[] = {0x5534, 0x5535};
    announce(audio, 2);
    uint8_t microphone[ML_HAPTICS_MAX_PAYLOAD];
    memset(microphone, 0, sizeof(microphone));
    microphone[0] = 1; microphone[1] = 1;
    MlHapticsWrite32(microphone + 4, 77);
    MlHapticsWrite16(microphone + 8, ML_HAPTICS_MAX_FRAMES);
    for (size_t i = ML_HAPTICS_HEADER_SIZE; i < sizeof(microphone); i++) microphone[i] = (uint8_t)(i * 7);

    CHECK(sendAudio(0x5534, microphone, sizeof(microphone)) == JNI_TRUE);
    CHECK(sentCount == 1 && sentChannels[0] == ML_HAPTICS_CHANNEL);
    CHECK(sent[0]->flags == ENET_PACKET_FLAG_UNSEQUENCED);
    CHECK(sent[0]->dataLength == sizeof(NVCTL_ENCRYPTED_PACKET_HEADER) + AES_GCM_TAG_LENGTH + 4 + sizeof(microphone));
    uint16_t type, length;
    const uint8_t* payload = plaintext(sent[0], &type, &length);
    CHECK(type == 0x5534 && length == sizeof(microphone));
    CHECK(memcmp(payload, microphone, sizeof(microphone)) == 0);
    freeSent();
}

static void oversizeOrMalformedControllerAudioStopsAtTheBoundary(void) {
    reset();
    const uint16_t audio[] = {0x5534, 0x5535};
    announce(audio, 2);
    static uint8_t big[ML_HAPTICS_MAX_PAYLOAD + 4];
    memset(big, 0, sizeof(big));
    big[0] = 1; big[1] = 1;
    MlHapticsWrite16(big + 8, ML_HAPTICS_MAX_FRAMES + 1);
    CHECK(sendAudio(0x5534, big, ML_HAPTICS_MAX_PAYLOAD + 4) == JNI_FALSE);
    CHECK(sendAudio(0x5534, big, ML_HAPTICS_MAX_PAYLOAD + 1) == JNI_FALSE);
    uint8_t status[9] = {1, 1, 0, 0, 0, 0, 0, 0, 0};
    CHECK(sendAudio(0x5535, status, 9) == JNI_FALSE);
    CHECK(sendAudio(0x5535, status, 7) == JNI_FALSE);
    CHECK(sentCount == 0);
    // A host that did not announce controller audio gets none.
    resetHostControlMessages();
    CHECK(sendAudio(0x5535, status, 8) == JNI_FALSE);
    CHECK(sentCount == 0);
}

static jboolean senderResult;

static void* sendStatusFromAJavaThread(void* context) {
    static const uint8_t status[8] = {1, 1, 0, 0, 0, 0, 0, 0};
    senderResult = sendAudio(0x5535, status, sizeof(status));
    return NULL;
}

static void* stopTheStream(void* context) {
    stopControlStream();
    return NULL;
}

static void teardownWaitsForJavaSendersAndLaterSendsNeverReachENet(void) {
    reset();
    const uint16_t audio[] = {0x5534, 0x5535};
    announce(audio, 2);
    pthread_mutex_lock(&gate);
    holdNextService = true;
    pthread_mutex_unlock(&gate);

    pthread_t sender, stopper;
    CHECK(pthread_create(&sender, NULL, sendStatusFromAJavaThread, NULL) == 0);
    pthread_mutex_lock(&gate);
    while (!atomic_load(&senderInside)) pthread_cond_wait(&gateChanged, &gate);
    pthread_mutex_unlock(&gate);

    // The stream stops while the controller's status message is still being sent.
    ConnectionInterrupted = true;
    CHECK(pthread_create(&stopper, NULL, stopTheStream, NULL) == 0);
    usleep(50000);
    CHECK(teardowns == 0 && !atomic_load(&tornDown));

    pthread_mutex_lock(&gate);
    serviceReleased = true;
    pthread_cond_broadcast(&gateChanged);
    pthread_mutex_unlock(&gate);
    CHECK(pthread_join(sender, NULL) == 0);
    CHECK(pthread_join(stopper, NULL) == 0);
    CHECK(teardowns == 1 && atomic_load(&tornDown) && peer == NULL && client == NULL);
    CHECK(senderResult == JNI_TRUE || senderResult == JNI_FALSE);
    freeSent();

    // A Java thread that passed its checks just before teardown reaches the core afterwards.
    static const uint8_t status[8] = {1, 1, 0, 0, 0, 0, 0, 0};
    CHECK(!sendMessageAndForget(0x5535, sizeof(status), status, 8, ENET_PACKET_FLAG_RELIABLE, false));
    CHECK(sendAudio(0x5535, status, sizeof(status)) == JNI_FALSE);
    CHECK(sentCount == 0);
    ConnectionInterrupted = false;
}

static void phaseLockReportsNeedTheHostsAnnouncement(void) {
    reset();
    // A host that announces other Rubylight messages but not phase lock.
    const uint16_t display[] = {0x5531};
    announce(display, 1);
    for (int i = 0; i < 3; i++) {
        Java_com_limelight_nvstream_jni_MoonBridge_reportFrameSlack(&env, NULL, 2000000, 16666666);
        fakeNowMs += 600;
    }
    CHECK(sentCount == 0);

    reset();
    const uint16_t phase[] = {RP_PHASE_REPORT_MESSAGE_TYPE};
    announce(phase, 1);
    Java_com_limelight_nvstream_jni_MoonBridge_reportFrameSlack(&env, NULL, 2000000, 16666666);
    fakeNowMs += 600;
    Java_com_limelight_nvstream_jni_MoonBridge_reportFrameSlack(&env, NULL, 2000000, 16666666);
    CHECK(sentCount == 1 && sentChannels[0] == CTRL_CHANNEL_GENERIC && sent[0]->flags == 0);
    uint16_t type, length;
    plaintext(sent[0], &type, &length);
    CHECK(type == RP_PHASE_REPORT_MESSAGE_TYPE && length == RP_PHASE_REPORT_BYTES);
    freeSent();
}

static const struct { const char* name; void (*run)(void); } scenarios[] = {
    {"microphone", microphonePacketsLargerThanTheStackBufferArriveIntact},
    {"boundary", oversizeOrMalformedControllerAudioStopsAtTheBoundary},
    {"phase-lock", phaseLockReportsNeedTheHostsAnnouncement},
    {"teardown", teardownWaitsForJavaSendersAndLaterSendsNeverReachENet},
};

// With an argument, only the named scenario runs. Teardown runs last: the stopped control
// stream stays closed until a new stream initializes it.
int main(int argc, char** argv) {
    CHECK(pthread_mutex_init(&enetMutex, NULL) == 0);
    for (size_t i = 0; i < sizeof(scenarios) / sizeof(scenarios[0]); i++) {
        if (argc < 2 || strcmp(argv[1], scenarios[i].name) == 0) scenarios[i].run();
    }
    resetPhaseLockReports();
    CHECK(pthread_mutex_destroy(&enetMutex) == 0);
    puts("control send: all tests passed");
    return 0;
}
