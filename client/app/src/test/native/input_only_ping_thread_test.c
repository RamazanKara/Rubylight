// The video ping thread of native input-only: its real start, send loop and stop against a
// loopback UDP socket, and the unwind of a ping that cannot start. Platform.c and
// PlatformSockets.c are the real ones; only the connection globals are set here.
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <sys/time.h>
#include <unistd.h>
#undef NDEBUG
#include <assert.h>
#include "Limelight-internal.h"

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4];
struct sockaddr_storage RemoteAddr;
struct sockaddr_storage LocalAddr;
SOCKADDR_LEN AddrLen;
uint16_t VideoPortNumber;
SS_PING VideoPingPayload;
uint16_t RtspPortNumber;

// Platform.c starts ENet with the platform; this test never uses it.
int enet_initialize(void) { return 0; }
void enet_deinitialize(void) {}

// Lets a test make the thread start fail after the socket was bound.
static bool failThreadCreate;
static void* pingAtThreadCreate;
static SOCKET boundSocket = INVALID_SOCKET;
static int testCreateThread(const char* name, ThreadEntry entry, void* context, PLT_THREAD* thread) {
    pingAtThreadCreate = context;
    boundSocket = *(SOCKET*)context;  // the socket is the first member of the ping
    if (failThreadCreate) return -1;
    return (PltCreateThread)(name, entry, context, thread);
}
#define PltCreateThread testCreateThread
#include "android_input_only_ping_thread.h"
#undef PltCreateThread

static SOCKET openReceiver(uint16_t* port) {
    SOCKET s = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in a = { 0 };
    socklen_t len = sizeof(a);
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    assert(s != INVALID_SOCKET);
    assert(bind(s, (struct sockaddr*)&a, sizeof(a)) == 0);
    assert(getsockname(s, (struct sockaddr*)&a, &len) == 0);
    *port = ntohs(a.sin_port);
    return s;
}

// One datagram, or -1 when none arrives within timeoutMs.
static ssize_t receive(SOCKET s, uint8_t* out, size_t size, int timeoutMs) {
    struct pollfd p = { s, POLLIN, 0 };
    if (poll(&p, 1, timeoutMs) <= 0) return -1;
    return recv(s, (char*)out, size, 0);
}

static void pointAt(uint16_t port) {
    struct sockaddr_in* remote = (struct sockaddr_in*)&RemoteAddr;
    struct sockaddr_in* local = (struct sockaddr_in*)&LocalAddr;
    memset(&RemoteAddr, 0, sizeof(RemoteAddr));
    memset(&LocalAddr, 0, sizeof(LocalAddr));
    remote->sin_family = AF_INET;
    remote->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    local->sin_family = AF_INET;
    AddrLen = sizeof(struct sockaddr_in);
    VideoPortNumber = port;
}

static bool isClosed(SOCKET s) {
    return fcntl(s, F_GETFD) == -1 && errno == EBADF;
}

int main(void) {
    uint16_t port;
    uint8_t datagram[64];

    // A host with an RTSP ping payload: the payload and a counting big-endian sequence, every 500 ms.
    SOCKET receiver = openReceiver(&port);
    pointAt(port);
    memcpy(VideoPingPayload.payload, "0123456789abcde", 16);
    RL_INPUT_ONLY_PING ping = RL_INPUT_ONLY_PING_INIT;
    rlInputOnlyPingStart(&ping);
    assert(ping.running && ping.socket != INVALID_SOCKET);
    for (uint32_t sequence = 1; sequence <= 2; sequence++) {
        ssize_t n = receive(receiver, datagram, sizeof(datagram), 3000);
        assert(n == RL_PING_MAX_SIZE);
        assert(memcmp(datagram, VideoPingPayload.payload, 16) == 0);
        assert(datagram[16] == 0 && datagram[17] == 0 && datagram[18] == 0 && datagram[19] == sequence);
    }
    // Stopping joins the thread without waiting out the interval, closes the socket, and is
    // safe to repeat; nothing is sent afterwards.
    SOCKET pingSocket = ping.socket;
    struct timeval before, after;
    gettimeofday(&before, NULL);
    rlInputOnlyPingStop(&ping);
    gettimeofday(&after, NULL);
    assert((after.tv_sec - before.tv_sec) * 1000 + (after.tv_usec - before.tv_usec) / 1000 < RL_PING_INTERVAL_MS);
    assert(!ping.running && ping.socket == INVALID_SOCKET && isClosed(pingSocket));
    rlInputOnlyPingStop(&ping);
    while (receive(receiver, datagram, sizeof(datagram), 100) > 0) {}  // at most one in flight
    assert(receive(receiver, datagram, sizeof(datagram), 1200) < 0);
    close(receiver);

    // A host that sent no payload gets the legacy "PING".
    receiver = openReceiver(&port);
    pointAt(port);
    memset(VideoPingPayload.payload, 0, 16);
    rlInputOnlyPingStart(&ping);
    assert(ping.running);
    assert(receive(receiver, datagram, sizeof(datagram), 3000) == 4 && memcmp(datagram, "PING", 4) == 0);
    rlInputOnlyPingStop(&ping);
    close(receiver);

    // No negotiated video port: nothing opens.
    pointAt(0);
    rlInputOnlyPingStart(&ping);
    assert(!ping.running && ping.socket == INVALID_SOCKET);

    // A socket that cannot be made leaves nothing running and nothing to stop.
    receiver = openReceiver(&port);
    pointAt(port);
    RemoteAddr.ss_family = 12345;
    rlInputOnlyPingStart(&ping);
    assert(!ping.running && ping.socket == INVALID_SOCKET);
    rlInputOnlyPingStop(&ping);

    // A thread that cannot start closes the socket it bound.
    pointAt(port);
    failThreadCreate = true;
    rlInputOnlyPingStart(&ping);
    failThreadCreate = false;
    assert(pingAtThreadCreate == &ping);
    assert(boundSocket != INVALID_SOCKET && isClosed(boundSocket));
    assert(!ping.running && ping.socket == INVALID_SOCKET);
    assert(receive(receiver, datagram, sizeof(datagram), 1200) < 0);
    close(receiver);

    printf("input_only_ping_thread: passed\n");
    return 0;
}
