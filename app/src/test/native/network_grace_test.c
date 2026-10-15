// Real ENet over loopback against the real control receiver: a host that goes silent, as it
// does to the client during a Wi-Fi roam or dropout, ends the stream only after the control
// peer timeout. Without a grace period that timeout stays at its default; with one, a silence
// shorter than the grace period is ridden out and the host's later end message still arrives,
// and a silence that outlasts it ends the stream with the usual transport error (-1).
// While a grace period rides out the silence, the control stream reports lost contact so the
// input stream holds input back instead of queueing it in ENet, and wakes the input stream once
// the host is heard again. The default timeout is shortened here so the test does not wait 10
// seconds, and so is the time without a reply that counts as lost contact.
#include "Limelight-internal.h"
#include <arpa/inet.h>
#include <netinet/in.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

#define CONTROL_PEER_TIMEOUT_MS 1000
#define INPUT_CONTACT_LOST_MS 300
#include "ControlStream.c"

// Connection.c owns these for a real stream.
CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
volatile bool ConnectionInterrupted;
int AppVersionQuad[4];
STREAM_CONFIGURATION StreamConfig;
uint32_t EncryptionFeaturesEnabled;

static int reportedReason, reports, failures;

static void reportTermination(int reason) {
    reports++;
    reportedReason = reason;
}

// The receiver never decrypts, queues callbacks or forwards controller audio here.
bool PltDecryptMessage(PPLT_CRYPTO_CONTEXT ctx, int algorithm, int flags,
        unsigned char* key, int keyLength, unsigned char* iv, int ivLength,
        unsigned char* tag, int tagLength, unsigned char* input, int inputLength,
        unsigned char* output, int* outputLength) { CHECK(false); return false; }
bool PltEncryptMessage(PPLT_CRYPTO_CONTEXT ctx, int algorithm, int flags,
        unsigned char* key, int keyLength, unsigned char* iv, int ivLength,
        unsigned char* tag, int tagLength, unsigned char* input, int inputLength,
        unsigned char* output, int* outputLength) { CHECK(false); return false; }
int LbqOfferQueueItem(PLINKED_BLOCKING_QUEUE queue, void* data, PLINKED_BLOCKING_QUEUE_ENTRY entry) { CHECK(false); return -1; }
void BridgeClControllerHaptics(uint16_t controller, uint32_t sequence, const uint8_t* pcm, uint16_t frames) { CHECK(false); }
void BridgeClControllerAudio(uint16_t kind, const uint8_t* payload, int length) { CHECK(false); }
void BridgeClHostTiming(const uint8_t* payload, int length) { CHECK(false); }
void BridgeClCursorShape(const uint8_t* payload, int length) { CHECK(false); }

// The receive thread asks the input stream to check for regained contact on every pass.
static int contactLostPasses;
static bool contactRegained;
void wakeInputAfterContactLoss(void) {
    if (isControlContactLost()) {
        contactLostPasses++;
    }
    else if (contactLostPasses > 0) {
        contactRegained = true;
    }
}

// ControlStream.c also references streams and sockets this test does not run.
bool ReferenceFrameInvalidationSupported;
int NegotiatedVideoFormat;
DECODER_RENDERER_CALLBACKS VideoCallbacks;
void LbqSignalQueueShutdown(PLINKED_BLOCKING_QUEUE queue) { }
void LbqSignalQueueDrain(PLINKED_BLOCKING_QUEUE queue) { }
void shutdownTcpSocket(SOCKET s) { CHECK(false); }
void closeSocket(SOCKET s) { CHECK(false); }

static uint64_t nowMs(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000;
}

static ENetHost* createHost(ENetAddress* bound) {
    struct sockaddr_in loopback = {0};
    loopback.sin_family = AF_INET;
    loopback.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    ENetAddress address;
    enet_address_set_address(&address, (struct sockaddr*)&loopback, sizeof(loopback));
    enet_address_set_port(&address, 0);
    ENetHost* host = enet_host_create(AF_INET, &address, 1, CTRL_CHANNEL_COUNT, 0, 0);
    CHECK(host != NULL);
    CHECK(enet_socket_get_address(host->socket, bound) == 0);
    return host;
}

static ENetPeer* connectClient(ENetHost* host, const ENetAddress* address) {
    client = enet_host_create(AF_INET, NULL, 1, CTRL_CHANNEL_COUNT, 0, 0);
    CHECK(client != NULL);
    client->intercept = ignoreDisconnectIntercept;
    peer = enet_host_connect(client, address, CTRL_CHANNEL_COUNT, 0);
    CHECK(peer != NULL);

    ENetPeer* hostPeer = NULL;
    bool connected = false;
    ENetEvent event;
    for (int i = 0; i < 500 && (!connected || hostPeer == NULL); i++) {
        while (enet_host_service(host, &event, 1) > 0) {
            if (event.type == ENET_EVENT_TYPE_CONNECT) hostPeer = event.peer;
        }
        while (enet_host_service(client, &event, 1) > 0) {
            if (event.type == ENET_EVENT_TYPE_CONNECT) connected = true;
        }
    }
    CHECK(connected && hostPeer != NULL);
    return hostPeer;
}

static void sendEndMessage(ENetHost* host, ENetPeer* hostPeer) {
    uint8_t message[6];
    uint16_t type = LE16(packetTypes[IDX_TERMINATION]);
    uint32_t reason = BE32(0x80030023);
    memcpy(message, &type, sizeof(type));
    memcpy(message + sizeof(type), &reason, sizeof(reason));
    CHECK(enet_peer_send(hostPeer, 0, enet_packet_create(message, sizeof(message), ENET_PACKET_FLAG_RELIABLE)) == 0);
    enet_host_flush(host);
}

struct SilentHost {
    ENetHost* host;
    ENetPeer* peer;
    int silenceMs;               // < 0: the network never comes back
    atomic_bool done;
    atomic_bool lostDuringSilence; // isControlContactLost() 2 * INPUT_CONTACT_LOST_MS into the silence
};

// Answers normally, then loses every datagram from the client for the silence, then answers
// again and ends the session the way a host does when its app exits.
static void* silentHost(void* context) {
    struct SilentHost* helper = context;
    ENetEvent event;
    uint64_t started = nowMs();
    while (!atomic_load(&helper->done) && nowMs() - started < 300) {
        if (enet_host_service(helper->host, &event, 10) > 0 && event.type == ENET_EVENT_TYPE_RECEIVE) {
            enet_packet_destroy(event.packet);
        }
    }

    // Loopback measures a round trip of about 0 ms, so ENet would resend every few
    // milliseconds and soon shift 1U by 32 or more in its timeout check (enet/protocol.c),
    // which the sanitizer rejects. A Wi-Fi-like round trip keeps the resend count realistic;
    // nothing acks during the silence, so ENet keeps these values until it ends.
    PltLockMutex(&enetMutex);
    peer->roundTripTime = 150;
    peer->roundTripTimeVariance = 50;
    PltUnlockMutex(&enetMutex);

    uint64_t silenceStarted = nowMs();
    uint8_t datagram[2048];
    bool sampled = false;
    while (!atomic_load(&helper->done) &&
            (helper->silenceMs < 0 || nowMs() - silenceStarted < (uint64_t)helper->silenceMs)) {
        while (recv(helper->host->socket, datagram, sizeof(datagram), MSG_DONTWAIT) > 0) { }
        if (!sampled && nowMs() - silenceStarted >= 2 * INPUT_CONTACT_LOST_MS) {
            atomic_store(&helper->lostDuringSilence, isControlContactLost());
            sampled = true;
        }
        usleep(1000);
    }

    bool sentEnd = false;
    uint64_t resumed = nowMs();
    while (!atomic_load(&helper->done)) {
        if (enet_host_service(helper->host, &event, 10) > 0 && event.type == ENET_EVENT_TYPE_RECEIVE) {
            enet_packet_destroy(event.packet);
        }
        if (!sentEnd && nowMs() - resumed >= 200) {
            sendEndMessage(helper->host, helper->peer);
            sentEnd = true;
        }
    }
    return NULL;
}

// silenceMs < 0 never ends the silence. Elapsed time is measured from the start of the silence.
static void run(const char* name, int gracePeriodMs, int silenceMs, int expectedReason,
        uint64_t minElapsedMs, uint64_t maxElapsedMs, bool expectContactLost, bool expectContactRegained) {
    LiSetNetworkGracePeriodMs(gracePeriodMs);

    ENetAddress address;
    ENetHost* host = createHost(&address);
    ENetPeer* hostPeer = connectClient(host, &address);
    // As startControlStream() configures the peer.
    enet_peer_timeout(peer, 2, controlPeerTimeoutMs(), controlPeerTimeoutMs());

    struct SilentHost helper = {host, hostPeer, silenceMs};
    atomic_init(&helper.done, false);
    atomic_init(&helper.lostDuringSilence, false);
    contactLostPasses = 0;
    contactRegained = false;
    disconnectPending = false;
    reports = 0;
    reportedReason = 12345;

    pthread_t thread;
    CHECK(pthread_create(&thread, NULL, silentHost, &helper) == 0);
    uint64_t started = nowMs();
    controlReceiveThreadFunc(NULL);
    uint64_t elapsed = nowMs() - started;
    atomic_store(&helper.done, true);
    CHECK(pthread_join(thread, NULL) == 0);
    elapsed = elapsed > 300 ? elapsed - 300 : 0;

    bool lost = atomic_load(&helper.lostDuringSilence);
    bool passed = reports == 1 && reportedReason == expectedReason &&
            elapsed >= minElapsedMs && elapsed <= maxElapsedMs &&
            lost == expectContactLost && (contactLostPasses > 0) == expectContactLost &&
            contactRegained == expectContactRegained;
    printf("%s %s: expected %d, received %d (%d reports) %llu ms into the silence, "
           "contact lost %d (%d passes), regained %d\n", passed ? "PASS" : "FAIL", name, expectedReason,
           reportedReason, reports, (unsigned long long)elapsed, lost, contactLostPasses, contactRegained);
    failures += !passed;

    enet_host_destroy(client);
    client = NULL;
    peer = NULL;
    CHECK(!isControlContactLost());
    enet_host_destroy(host);
}

int main(void) {
    CHECK(enet_initialize() == 0);
    CHECK(PltCreateMutex(&enetMutex) == 0);
    memset(&ListenerCallbacks, 0, sizeof(ListenerCallbacks));
    ListenerCallbacks.connectionTerminated = reportTermination;
    AppVersionQuad[0] = 7;
    AppVersionQuad[3] = -1;
    encryptedControlStream = false;
    packetTypes = (short*)packetTypesGen7;
    lastSeenFrame = 1;

    // Off and too-short grace periods keep the default timeout.
    LiSetNetworkGracePeriodMs(0);
    CHECK(controlPeerTimeoutMs() == CONTROL_PEER_TIMEOUT_MS);
    LiSetNetworkGracePeriodMs(-5000);
    CHECK(controlPeerTimeoutMs() == CONTROL_PEER_TIMEOUT_MS);
    LiSetNetworkGracePeriodMs(CONTROL_PEER_TIMEOUT_MS);
    CHECK(controlPeerTimeoutMs() == CONTROL_PEER_TIMEOUT_MS);
    LiSetNetworkGracePeriodMs(CONTROL_PEER_TIMEOUT_MS + 1);
    CHECK(controlPeerTimeoutMs() == CONTROL_PEER_TIMEOUT_MS + 1);

    run("no grace period: a silence longer than the default timeout ends the stream",
        0, 2500, -1, CONTROL_PEER_TIMEOUT_MS - 100, 2400, false, false);
    run("grace period: the same silence is ridden out and the host's exit still arrives",
        4000, 2500, ML_ERROR_GRACEFUL_TERMINATION, 2500, 3400, true, true);
    run("grace period: a network that never returns ends the stream when it runs out",
        2500, -1, -1, 2500 - 100, 2500 + 1500, true, false);

    PltDeleteMutex(&enetMutex);
    enet_deinitialize();
    if (failures) {
        fprintf(stderr, "network grace: %d scenario(s) failed\n", failures);
        return 1;
    }
    puts("network grace: all tests passed");
    return 0;
}
