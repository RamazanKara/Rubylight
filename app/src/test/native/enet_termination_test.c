// Real ENet over loopback against the real control receiver and its disconnect intercept.
// Vibepollo ends a session by sending its end message reliably, flushing it, and calling
// enet_peer_disconnect_now() a few milliseconds later. That disconnect must not erase the
// queued end message, or every normal game exit is reported as a transport failure (-1).
#include "Limelight-internal.h"
#include <arpa/inet.h>
#include <netinet/in.h>
#include <time.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

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

enum HostExit {
    END_THEN_DISCONNECT_NOW,     // Vibepollo: unsequenced 10-byte disconnect right after the end message
    END_THEN_DISCONNECT,         // GFE: reliable 12-byte disconnect after the end message
    DISCONNECT_NOW_WITHOUT_END,  // a host that drops the session without giving a reason
};

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

// What an input or loss-stats send does between control receive passes.
static void drainFromAnotherSender(void) {
    PltLockMutex(&enetMutex);
    CHECK(enet_host_service(client, NULL, 0) >= 0);
    PltUnlockMutex(&enetMutex);
}

static void run(const char* name, enum HostExit hostExit, bool otherSenderDrains, int expectedReason) {
    ENetAddress address;
    ENetHost* host = createHost(&address);
    ENetPeer* hostPeer = connectClient(host, &address);

    disconnectPending = false;
    reports = 0;
    reportedReason = 12345;
    if (hostExit != DISCONNECT_NOW_WITHOUT_END) {
        sendEndMessage(host, hostPeer);
        if (otherSenderDrains) {
            usleep(2000);
            drainFromAnotherSender();
        }
    }
    if (hostExit == END_THEN_DISCONNECT) {
        enet_peer_disconnect(hostPeer, 0);
        enet_host_flush(host);
    }
    else {
        enet_peer_disconnect_now(hostPeer, 0);
    }
    // Both datagrams are now waiting on the client socket before the receiver runs.
    usleep(20000);
    if (otherSenderDrains) {
        drainFromAnotherSender();
    }

    uint64_t started = nowMs();
    controlReceiveThreadFunc(NULL);
    uint64_t elapsed = nowMs() - started;

    // A reasonless drop is reported once the pending receives are drained, not after a
    // retransmission wait for a disconnect that is never sent again.
    bool passed = reports == 1 && reportedReason == expectedReason && elapsed < 600;
    printf("%s %s: expected %d, received %d (%d reports) after %llu ms\n", passed ? "PASS" : "FAIL",
           name, expectedReason, reportedReason, reports, (unsigned long long)elapsed);
    failures += !passed;

    enet_host_destroy(client);
    client = NULL;
    peer = NULL;
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

    run("vibepollo end message, then disconnect_now", END_THEN_DISCONNECT_NOW, false,
        ML_ERROR_GRACEFUL_TERMINATION);
    run("vibepollo exit drained by another sender", END_THEN_DISCONNECT_NOW, true,
        ML_ERROR_GRACEFUL_TERMINATION);
    run("gfe end message, then reliable disconnect", END_THEN_DISCONNECT, false,
        ML_ERROR_GRACEFUL_TERMINATION);
    run("gfe exit drained by another sender", END_THEN_DISCONNECT, true,
        ML_ERROR_GRACEFUL_TERMINATION);
    run("disconnect_now without an end message", DISCONNECT_NOW_WITHOUT_END, false, -1);
    run("disconnect_now without an end message, drained", DISCONNECT_NOW_WITHOUT_END, true, -1);

    PltDeleteMutex(&enetMutex);
    enet_deinitialize();
    if (failures) {
        fprintf(stderr, "enet termination: %d scenario(s) failed\n", failures);
        return 1;
    }
    puts("enet termination: all tests passed");
    return 0;
}
