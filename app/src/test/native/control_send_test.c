// The real JNI control senders and encrypted ENet send path against fake ENet and JNI:
// DualSense microphone packets larger than the old 256-byte plaintext buffer, Java-thread
// sends and RTT reads racing stream teardown or the next stream's initialization, and
// phase-lock reports that need the host's announcement.
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
static atomic_uint_fast64_t fakeNowMs;

// One enet_host_service() call can be held to keep a sender inside the send path, a thread
// can be held just after it takes enetMutex, and teardown can be held inside its linger.
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t gateChanged = PTHREAD_COND_INITIALIZER;
static bool holdNextService, serviceReleased, lockReleased, holdLinger, lingerReleased;
static _Thread_local bool holdThisThreadInLock;
static atomic_bool readerInside, readerDone, lingerInside;

static void holdUntil(atomic_bool* inside, bool* released) {
    pthread_mutex_lock(&gate);
    atomic_store(inside, true);
    pthread_cond_broadcast(&gateChanged);
    while (!*released) pthread_cond_wait(&gateChanged, &gate);
    atomic_store(inside, false);
    pthread_mutex_unlock(&gate);
}

static void release(bool* released) {
    pthread_mutex_lock(&gate);
    *released = true;
    pthread_cond_broadcast(&gateChanged);
    pthread_mutex_unlock(&gate);
}

// Each stream creates and destroys its own mutex and crypto contexts.
static atomic_bool enetMutexExists;

int PltCreateMutex(PLT_MUTEX* mutex) {
    CHECK(mutex == &enetMutex && !atomic_load(&enetMutexExists));
    atomic_store(&enetMutexExists, true);
    return pthread_mutex_init(mutex, NULL);
}

void PltDeleteMutex(PLT_MUTEX* mutex) {
    CHECK(mutex == &enetMutex && atomic_load(&enetMutexExists));
    CHECK(pthread_mutex_destroy(mutex) == 0);
    atomic_store(&enetMutexExists, false);
}

void PltLockMutex(PLT_MUTEX* mutex) {
    // No sender may reach the mutex of a stream that has been destroyed.
    CHECK(mutex != &enetMutex || atomic_load(&enetMutexExists));
    CHECK(pthread_mutex_lock(mutex) == 0);
    if (holdThisThreadInLock) {
        holdThisThreadInLock = false;
        holdUntil(&readerInside, &lockReleased);
    }
}

void PltUnlockMutex(PLT_MUTEX* mutex) { CHECK(pthread_mutex_unlock(mutex) == 0); }
void PltSleepMs(int ms) { usleep((useconds_t)ms * 1000); }
uint64_t PltGetMillis(void) { return fakeNowMs; }
int PltCreateEvent(PLT_EVENT* event) { return 0; }
void PltCloseEvent(PLT_EVENT* event) { }
void PltSetEvent(PLT_EVENT* event) { }
void PltInterruptThread(PLT_THREAD* thread) { }
void PltJoinThread(PLT_THREAD* thread) { }
int LbqInitializeLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue, int sizeBound) { return 0; }
PLINKED_BLOCKING_QUEUE_ENTRY LbqDestroyLinkedBlockingQueue(PLINKED_BLOCKING_QUEUE queue) { return NULL; }
void LbqSignalQueueShutdown(PLINKED_BLOCKING_QUEUE queue) { }
void LbqSignalQueueDrain(PLINKED_BLOCKING_QUEUE queue) { }
bool isReferenceFrameInvalidationEnabled(void) { return false; }
void shutdownTcpSocket(SOCKET s) { CHECK(false); }
void closeSocket(SOCKET s) { CHECK(false); }

// Crypto contexts are tokens that are alive from creation until they are destroyed. A
// Java-thread send can be made to arrive while a stream is creating them.
#define MAX_CONTEXTS 8
static PLT_CRYPTO_CONTEXT contexts[MAX_CONTEXTS];
static bool contextAlive[MAX_CONTEXTS];
static int contextCount;
static bool sendWhileCreatingContexts;
static void sendFromAJavaThreadDuringInitialization(void);

static bool isAliveContext(PPLT_CRYPTO_CONTEXT ctx) {
    return ctx >= contexts && ctx < contexts + contextCount && contextAlive[ctx - contexts];
}

PPLT_CRYPTO_CONTEXT PltCreateCryptoContext(void) {
    if (sendWhileCreatingContexts) {
        sendWhileCreatingContexts = false;
        sendFromAJavaThreadDuringInitialization();
    }
    CHECK(contextCount < MAX_CONTEXTS);
    contextAlive[contextCount] = true;
    return &contexts[contextCount++];
}

void PltDestroyCryptoContext(PPLT_CRYPTO_CONTEXT ctx) {
    CHECK(isAliveContext(ctx));
    contextAlive[ctx - contexts] = false;
}

// The identity "cipher" leaves the plaintext readable behind the header and tag.
bool PltEncryptMessage(PPLT_CRYPTO_CONTEXT ctx, int algorithm, int flags,
        unsigned char* key, int keyLength, unsigned char* iv, int ivLength,
        unsigned char* tag, int tagLength, unsigned char* input, int inputLength,
        unsigned char* output, int* outputLength) {
    // Never the context of a destroyed stream.
    CHECK(isAliveContext(ctx));
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
    // Teardown must never run while a Java thread is still inside the send path or an RTT read.
    CHECK(!atomic_load(&senderInside) && !atomic_load(&readerInside));
    CHECK(host == &fakeHost && target == &fakePeer);
    if (holdLinger) {
        holdLinger = false;
        holdUntil(&lingerInside, &lingerReleased);
    }
    teardowns++;
    return 0;
}

// No host disconnect is held back here, so a stop always disconnects gracefully.
void enet_peer_disconnect_now(ENetPeer* target, enet_uint32 data) { CHECK(false); }

void enet_host_destroy(ENetHost* host) {
    CHECK(!atomic_load(&senderInside) && !atomic_load(&readerInside) && host == &fakeHost);
    atomic_store(&tornDown, true);
}

// Phase-lock windows always have a report ready in this test.
static uint8_t fakePhaseLead;
RpSlackWindow* rp_slack_window_new(void) { return (RpSlackWindow*)&fakeHost; }
void rp_slack_window_free(RpSlackWindow* window) { }
void rp_slack_window_push(RpSlackWindow* window, int64_t slack_ns, int64_t period_ns) { }
bool rp_slack_window_report(RpSlackWindow* window, size_t min_frames, uint8_t* out) {
    memset(out, 0, RP_PHASE_REPORT_BYTES);
    out[8] = fakePhaseLead;
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
    lockReleased = holdLinger = lingerReleased = false;
    atomic_store(&readerInside, false);
    atomic_store(&readerDone, false);
    atomic_store(&lingerInside, false);
    // A stream that has finished initializing: its own mutex and crypto contexts, sending open.
    if (!atomic_load(&enetMutexExists)) CHECK(PltCreateMutex(&enetMutex) == 0);
    sendWhileCreatingContexts = false;
    contextCount = 0;
    encryptionCtx = PltCreateCryptoContext();
    decryptionCtx = PltCreateCryptoContext();
    atomic_store(&enetSendsClosed, false);
    // A Sunshine host with the encrypted control stream.
    AppVersionQuad[0] = 7; AppVersionQuad[1] = 1; AppVersionQuad[2] = 431; AppVersionQuad[3] = -1;
    encryptedControlStream = true;
    EncryptionFeaturesEnabled = SS_ENC_CONTROL_V2;
    resetHostControlMessages();
    resetPhaseLockReports();
    fakeNowMs = 1000;
    fakePhaseLead = 0x10;
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
    return Java_com_limelight_nvstream_jni_MoonBridge_cSendControllerAudio(&env, NULL, kind, (jbyteArray)&array);
}

static jboolean sendCommand(const uint8_t* bytes, int length) {
    FakeByteArray array = {length, bytes};
    return Java_com_limelight_nvstream_jni_MoonBridge_cSendServerCommand(&env, NULL, (jbyteArray)&array);
}

static void serverCommandsUseReliableEncryptedServerControlPackets(void) {
    // Apollo/Vibepollo's four-byte index and Rubylight's one-byte index. No SDP control-message
    // announcement is required: these commands are offered by serverinfo and checked in Java.
    for (int length = 1; length <= 4; length += 3) {
        reset();
        uint8_t command[4] = {2, 0, 0, 0};
        CHECK(sendCommand(command, length) == JNI_TRUE);
        CHECK(sentCount == 1 && sentChannels[0] == 0x08);
        CHECK(sent[0]->flags == ENET_PACKET_FLAG_RELIABLE);
        uint16_t type, payloadLength;
        const uint8_t* payload = plaintext(sent[0], &type, &payloadLength);
        CHECK(type == 0x3000 && payloadLength == length);
        CHECK(memcmp(payload, command, length) == 0);
        freeSent();
        // Older hosts with fewer channels receive the same command on the generic channel.
        fakePeer.channelCount = 1;
        CHECK(sendCommand(command, length) == JNI_TRUE);
        CHECK(sentCount == 1 && sentChannels[0] == 0);
        freeSent();
        stopping = true;
        CHECK(sendCommand(command, length) == JNI_FALSE && sentCount == 0);
        stopping = false;
        encryptedControlStream = false;
        CHECK(sendCommand(command, length) == JNI_FALSE && sentCount == 0);
    }
    reset();
    uint8_t invalid[5] = {0};
    CHECK(sendCommand(invalid, 0) == JNI_FALSE);
    CHECK(sendCommand(invalid, 2) == JNI_FALSE);
    CHECK(sendCommand(invalid, 5) == JNI_FALSE);
    CHECK(sentCount == 0);
    freeSent();
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

static void waitFor(atomic_bool* inside, atomic_bool* done) {
    pthread_mutex_lock(&gate);
    while (!atomic_load(inside) && (done == NULL || !atomic_load(done))) pthread_cond_wait(&gateChanged, &gate);
    pthread_mutex_unlock(&gate);
}

static bool rttResult;
static uint32_t rttRead, rttVarianceRead;

static void* readRttFromJava(void* context) {
    holdThisThreadInLock = true;
    rttResult = LiGetEstimatedRttInfo(&rttRead, &rttVarianceRead);
    holdThisThreadInLock = false;
    pthread_mutex_lock(&gate);
    atomic_store(&readerDone, true);
    pthread_cond_broadcast(&gateChanged);
    pthread_mutex_unlock(&gate);
    return NULL;
}

static void teardownWaitsForAnRttReadInFlight(void) {
    reset();
    fakePeer.roundTripTime = 7;
    fakePeer.roundTripTimeVariance = 3;

    // A quality timer's RTT read is inside enetMutex when the stream stops.
    pthread_t reader, stopper;
    CHECK(pthread_create(&reader, NULL, readRttFromJava, NULL) == 0);
    waitFor(&readerInside, &readerDone);
    CHECK(atomic_load(&readerInside));
    ConnectionInterrupted = true;
    CHECK(pthread_create(&stopper, NULL, stopTheStream, NULL) == 0);
    usleep(50000);
    CHECK(teardowns == 0 && !atomic_load(&tornDown));

    release(&lockReleased);
    CHECK(pthread_join(reader, NULL) == 0);
    CHECK(pthread_join(stopper, NULL) == 0);
    CHECK(rttResult && rttRead == 7 && rttVarianceRead == 3);
    CHECK(teardowns == 1 && atomic_load(&tornDown) && peer == NULL && client == NULL);
    ConnectionInterrupted = false;
}

static void rttReadsStopOnceTeardownStarts(void) {
    reset();
    fakePeer.roundTripTime = 7;
    fakePeer.roundTripTimeVariance = 3;
    uint32_t rtt = 0, variance = 0;
    CHECK(LiGetEstimatedRttInfo(&rtt, &variance) && rtt == 7 && variance == 3);

    // Teardown lingers for up to two seconds with the peer still set; an RTT read from the
    // overlay or a quality timer in that time must leave the peer alone.
    holdLinger = true;
    ConnectionInterrupted = true;
    pthread_t stopper;
    CHECK(pthread_create(&stopper, NULL, stopTheStream, NULL) == 0);
    waitFor(&lingerInside, NULL);
    CHECK(!LiGetEstimatedRttInfo(&rtt, &variance));
    release(&lingerReleased);
    CHECK(pthread_join(stopper, NULL) == 0);
    CHECK(teardowns == 1 && atomic_load(&tornDown) && peer == NULL && client == NULL);
    CHECK(!LiGetEstimatedRttInfo(&rtt, &variance));
    ConnectionInterrupted = false;
}

static const uint8_t controllerStatus[8] = {1, 1, 0, 0, 0, 0, 0, 0};

static void sendFromAJavaThreadDuringInitialization(void) {
    CHECK(!sendMessageAndForget(0x5535, sizeof(controllerStatus), controllerStatus, 8, ENET_PACKET_FLAG_RELIABLE, false));
}

static void sendingStaysClosedFromAFailedStartUntilTheNextStreamIsReady(void) {
    reset();
    // startControlStream() failed before it had a peer (enet_host_create() or
    // enet_host_connect() failed), so the stream is destroyed without being stopped.
    peer = NULL;
    client = NULL;
    stopping = true;
    destroyControlStream();
    CHECK(!atomic_load(&enetMutexExists));
    // A Java thread that passed its own checks earlier reaches the core afterwards, and the
    // quality timers keep reading the RTT.
    CHECK(!sendMessageAndForget(0x5535, sizeof(controllerStatus), controllerStatus, 8, ENET_PACKET_FLAG_RELIABLE, false));
    CHECK(!LiGetEstimatedRttInfo(NULL, NULL));

    // The next stream: a Java send that arrives while it is still creating its crypto
    // contexts must not encrypt with the destroyed stream's.
    sendWhileCreatingContexts = true;
    CHECK(initializeControlStream() == 0);
    CHECK(!sendWhileCreatingContexts && atomic_load(&enetMutexExists));
    CHECK(sentCount == 0);

    // Once initialized and connected, the new stream sends with its own contexts.
    client = &fakeHost;
    peer = &fakePeer;
    CHECK(sendMessageAndForget(0x5535, sizeof(controllerStatus), controllerStatus, 8, ENET_PACKET_FLAG_RELIABLE, false));
    CHECK(sentCount == 1);
    freeSent();
}

static void phaseLockReportsNeedTheHostsAnnouncement(void) {
    reset();
    // A host that announces other Rubylight messages but not phase lock.
    const uint16_t display[] = {0x5531};
    announce(display, 1);
    for (int i = 0; i < 3; i++) {
        CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cReportFrameSlack(&env, NULL, 2000000, 16666666) == JNI_FALSE);
        fakeNowMs += 600;
    }
    CHECK(sentCount == 0);

    reset();
    const uint16_t phase[] = {RP_PHASE_REPORT_MESSAGE_TYPE};
    announce(phase, 1);
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cReportFrameSlack(&env, NULL, 2000000, 16666666) == JNI_FALSE);
    fakeNowMs += 600;
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cReportFrameSlack(&env, NULL, 2000000, 16666666) == JNI_TRUE);
    CHECK(sentCount == 0);
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1 && sentChannels[0] == CTRL_CHANNEL_GENERIC && sent[0]->flags == 0);
    uint16_t type, length;
    plaintext(sent[0], &type, &length);
    CHECK(type == RP_PHASE_REPORT_MESSAGE_TYPE && length == RP_PHASE_REPORT_BYTES);
    freeSent();
}

static void queuePhaseReport(void) {
    fakeNowMs += 600;
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cReportFrameSlack(&env, NULL, 2000000, 16666666) == JNI_TRUE);
}

static void startPhaseReports(void) {
    reset();
    const uint16_t phase[] = {RP_PHASE_REPORT_MESSAGE_TYPE};
    announce(phase, 1);
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cReportFrameSlack(&env, NULL, 2000000, 16666666) == JNI_FALSE);
}

static void phaseMailboxKeepsOnlyFreshNegotiatedReports(void) {
    startPhaseReports();
    queuePhaseReport();
    fakePhaseLead = 0x20;
    queuePhaseReport();
    CHECK(sentCount == 0);
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1);
    uint16_t type, length;
    CHECK(plaintext(sent[0], &type, &length)[8] == 0x20);
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1); // Consumed once, even if another handler message was queued.

    queuePhaseReport();
    fakeNowMs += PHASE_LOCK_MAX_REPORT_AGE_MS + 1;
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1);

    queuePhaseReport();
    fakeNowMs--; // Invalid clock ordering must not revive a report.
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1);

    queuePhaseReport();
    resetHostControlMessages();
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1);
    const uint16_t phase[] = {RP_PHASE_REPORT_MESSAGE_TYPE};
    announce(phase, 1);
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1); // Unsupported reports were discarded, not retained for a new host.

    queuePhaseReport();
    resetPhaseLockReports();
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1);
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cGetPhaseLockLeadUs(&env, NULL) == INT32_MIN);
    freeSent();
}

static atomic_bool phaseFlushDone, phaseResetDone, phaseSampleDone;

static void* samplePhaseFromRenderer(void* context) {
    queuePhaseReport();
    atomic_store(&phaseSampleDone, true);
    return NULL;
}

static void samplePhaseWithoutWaitingForNetwork(void) {
    atomic_store(&phaseSampleDone, false);
    pthread_t sampler;
    CHECK(pthread_create(&sampler, NULL, samplePhaseFromRenderer, NULL) == 0);
    // This is a bounded liveness check, not a render-thread performance threshold.
    // A regression fails the process instead of deadlocking the whole native suite.
    for (int i = 0; i < 2000 && !atomic_load(&phaseSampleDone); i++) usleep(1000);
    CHECK(atomic_load(&phaseSampleDone));
    CHECK(pthread_join(sampler, NULL) == 0);
}

static void* flushPhaseFromBackground(void* context) {
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    atomic_store(&phaseFlushDone, true);
    return NULL;
}

static void* resetPhaseFromConnection(void* context) {
    resetPhaseLockReports();
    atomic_store(&phaseResetDone, true);
    return NULL;
}

static void phaseSamplingNeverWaitsForTheNetworkSender(void) {
    startPhaseReports();
    PltLockMutex(&enetMutex);
    // The original JNI collector tried to send here and waited on this mutex.
    samplePhaseWithoutWaitingForNetwork();
    CHECK(sentCount == 0);

    atomic_store(&phaseFlushDone, false);
    pthread_t flusher;
    CHECK(pthread_create(&flusher, NULL, flushPhaseFromBackground, NULL) == 0);
    // Wait until the real guarded sender is blocked at ENet, away from the render thread.
    for (int i = 0; i < 2000 && atomic_load(&enetSendersInFlight) == 0; i++) usleep(1000);
    CHECK(atomic_load(&enetSendersInFlight) == 1 && !atomic_load(&phaseFlushDone));
    fakePhaseLead = 0x20;
    samplePhaseWithoutWaitingForNetwork();
    fakePhaseLead = 0x30;
    samplePhaseWithoutWaitingForNetwork();
    CHECK(sentCount == 0 && !atomic_load(&phaseFlushDone));
    PltUnlockMutex(&enetMutex);
    CHECK(pthread_join(flusher, NULL) == 0);
    CHECK(sentCount == 1);
    uint16_t type, length;
    CHECK(plaintext(sent[0], &type, &length)[8] == 0x10);
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 2 && plaintext(sent[1], &type, &length)[8] == 0x30);
    freeSent();
}

static void phaseResetWaitsForTheOldFlushAndClearsItsSuccessor(void) {
    startPhaseReports();
    queuePhaseReport();
    holdNextService = true;
    atomic_store(&phaseFlushDone, false);
    atomic_store(&phaseResetDone, false);
    pthread_t flusher, resetter;
    CHECK(pthread_create(&flusher, NULL, flushPhaseFromBackground, NULL) == 0);
    waitFor(&senderInside, &phaseFlushDone);
    CHECK(atomic_load(&senderInside));
    queuePhaseReport();
    CHECK(pthread_create(&resetter, NULL, resetPhaseFromConnection, NULL) == 0);
    usleep(50000);
    CHECK(!atomic_load(&phaseResetDone));
    release(&serviceReleased);
    CHECK(pthread_join(flusher, NULL) == 0);
    CHECK(pthread_join(resetter, NULL) == 0);
    CHECK(atomic_load(&phaseResetDone));
    Java_com_limelight_nvstream_jni_MoonBridge_cFlushPhaseLockReport(&env, NULL);
    CHECK(sentCount == 1 && !phaseLockReportPending);
    freeSent();
}

// The encoder's own tests live in protocol/core; here it only has to carry the sequence.
void rp_clock_sync_request_encode(uint32_t sequence, uint64_t client_us, uint8_t* out) {
    memset(out, 0, RP_CLOCK_SYNC_REQUEST_BYTES);
    out[0] = 1;
    memcpy(out + 4, &sequence, sizeof(sequence));
    memcpy(out + 8, &client_us, sizeof(client_us));
}

static void clockSyncProbesNeedTheHostsAnnouncement(void) {
    reset();
    const uint16_t others[] = {0x5531, 0x5536};
    announce(others, 2);
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cSendClockSyncProbe(&env, NULL) == JNI_FALSE);
    CHECK(sentCount == 0);

    const uint16_t clock[] = {RL_CLOCK_SYNC_MESSAGE_TYPE};
    announce(clock, 1);
    for (int i = 0; i < 2; i++) {
        CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cSendClockSyncProbe(&env, NULL) == JNI_TRUE);
    }
    CHECK(sentCount == 2);
    uint32_t sequences[2];
    for (int i = 0; i < 2; i++) {
        CHECK(sentChannels[i] == CTRL_CHANNEL_GENERIC && sent[i]->flags == ENET_PACKET_FLAG_UNSEQUENCED);
        uint16_t type, length;
        const uint8_t* body = plaintext(sent[i], &type, &length);
        CHECK(type == RL_CLOCK_SYNC_MESSAGE_TYPE && length == RP_CLOCK_SYNC_REQUEST_BYTES && body[0] == 1);
        memcpy(&sequences[i], body + 4, sizeof(sequences[i]));
    }
    CHECK(sequences[1] == sequences[0] + 1);

    stopping = true;
    CHECK(Java_com_limelight_nvstream_jni_MoonBridge_cSendClockSyncProbe(&env, NULL) == JNI_FALSE);
    CHECK(sentCount == 2);
    freeSent();
}

static const struct { const char* name; void (*run)(void); } scenarios[] = {
    {"clock-sync", clockSyncProbesNeedTheHostsAnnouncement},
    {"server-command", serverCommandsUseReliableEncryptedServerControlPackets},
    {"microphone", microphonePacketsLargerThanTheStackBufferArriveIntact},
    {"boundary", oversizeOrMalformedControllerAudioStopsAtTheBoundary},
    {"phase-lock", phaseLockReportsNeedTheHostsAnnouncement},
    {"phase-mailbox", phaseMailboxKeepsOnlyFreshNegotiatedReports},
    {"phase-nonblocking", phaseSamplingNeverWaitsForTheNetworkSender},
    {"phase-reset", phaseResetWaitsForTheOldFlushAndClearsItsSuccessor},
    {"teardown", teardownWaitsForJavaSendersAndLaterSendsNeverReachENet},
    {"lifecycle", sendingStaysClosedFromAFailedStartUntilTheNextStreamIsReady},
    {"rtt-in-flight", teardownWaitsForAnRttReadInFlight},
    {"rtt-linger", rttReadsStopOnceTeardownStarts},
};

// With an argument, only the named scenario runs. Each scenario starts from a freshly
// initialized stream.
int main(int argc, char** argv) {
    for (size_t i = 0; i < sizeof(scenarios) / sizeof(scenarios[0]); i++) {
        if (argc < 2 || strcmp(argv[1], scenarios[i].name) == 0) scenarios[i].run();
    }
    resetPhaseLockReports();
    if (atomic_load(&enetMutexExists)) PltDeleteMutex(&enetMutex);
    puts("control send: all tests passed");
    return 0;
}
