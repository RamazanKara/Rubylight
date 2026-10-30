// socket-dscp.patch: bindUdpSocket marks the audio and microphone sockets DSCP Expedited
// Forwarding and the video socket AF41, keeps their SO_PRIORITY, and leaves best-effort
// sockets unmarked. Platform.c
// and PlatformSockets.c are the real ones.
#include <stdarg.h>
#include <stdio.h>
#include "Limelight-internal.h"
// After Platform.h, which defines NDEBUG without LC_DEBUG.
#undef NDEBUG
#include <assert.h>

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4];
struct sockaddr_storage RemoteAddr;
struct sockaddr_storage LocalAddr;
SOCKADDR_LEN AddrLen;
uint16_t RtspPortNumber;

static void logMessage(const char* format, ...) {
    va_list args;
    va_start(args, format);
    vfprintf(stderr, format, args);
    va_end(args);
}

// Platform.c starts ENet with the platform; this test never uses it.
int enet_initialize(void) { return 0; }
void enet_deinitialize(void) {}

static int readOption(SOCKET s, int level, int name) {
    int value = -1;
    socklen_t len = sizeof(value);
    assert(getsockopt(s, level, name, &value, &len) == 0);
    return value;
}

static SOCKET bindLoopback(int family, int qos) {
    struct sockaddr_storage local = { 0 };
    SOCKADDR_LEN len;
    if (family == AF_INET6) {
        struct sockaddr_in6* a = (struct sockaddr_in6*)&local;
        a->sin6_family = AF_INET6;
        a->sin6_addr = in6addr_loopback;
        len = sizeof(*a);
    }
    else {
        struct sockaddr_in* a = (struct sockaddr_in*)&local;
        a->sin_family = AF_INET;
        a->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        len = sizeof(*a);
    }
    return bindUdpSocket(family, &local, len, 0, qos);
}

int main(void) {
    ListenerCallbacks.logMessage = logMessage;
    SOCKET audio = bindLoopback(AF_INET, SOCK_QOS_TYPE_AUDIO);
    SOCKET video = bindLoopback(AF_INET, SOCK_QOS_TYPE_VIDEO);
    SOCKET plain = bindLoopback(AF_INET, SOCK_QOS_TYPE_BEST_EFFORT);
    assert(audio != INVALID_SOCKET && video != INVALID_SOCKET && plain != INVALID_SOCKET);
    assert(readOption(audio, IPPROTO_IP, IP_TOS) == 0xB8);
    assert(readOption(video, IPPROTO_IP, IP_TOS) == 0x88);
    assert(readOption(plain, IPPROTO_IP, IP_TOS) == 0);
    // The marking does not reset the priority (IP_TOS does on Linux).
    assert(readOption(audio, SOL_SOCKET, SO_PRIORITY) == 6);
    assert(readOption(video, SOL_SOCKET, SO_PRIORITY) == 5);
    closeSocket(audio);
    closeSocket(video);
    closeSocket(plain);

    // A machine without IPv6 loopback has nothing more to test.
    SOCKET audio6 = bindLoopback(AF_INET6, SOCK_QOS_TYPE_AUDIO);
    if (audio6 != INVALID_SOCKET) {
        assert(readOption(audio6, IPPROTO_IPV6, IPV6_TCLASS) == 0xB8);
        closeSocket(audio6);
    }
    return 0;
}
