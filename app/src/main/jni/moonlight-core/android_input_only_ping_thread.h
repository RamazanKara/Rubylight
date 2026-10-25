#pragma once
#include "android_input_only_ping.h"

// The video ping thread of a native input-only session. Include after Limelight-internal.h: it
// uses the connection's RemoteAddr, LocalAddr, AddrLen, VideoPortNumber and VideoPingPayload.

typedef struct _RL_INPUT_ONLY_PING {
    SOCKET socket;
    PLT_THREAD thread;
    bool running;
} RL_INPUT_ONLY_PING;

#define RL_INPUT_ONLY_PING_INIT { INVALID_SOCKET }

// Socket errors are ignored, as in VideoStream.c: nothing reads this socket and an ICMP port
// unreachable (the host may not have bound the port yet) must not end or log anything.
static void rlInputOnlyPingProc(void* context) {
    RL_INPUT_ONLY_PING* ping = (RL_INPUT_ONLY_PING*)context;
    uint8_t datagram[RL_PING_MAX_SIZE];
    LC_SOCKADDR saddr;
    uint32_t sequence = 0;

    memcpy(&saddr, &RemoteAddr, sizeof(saddr));
    SET_PORT(&saddr, VideoPortNumber);
    while (!PltIsThreadInterrupted(&ping->thread)) {
        size_t length = rlBuildVideoPing((const uint8_t*)VideoPingPayload.payload, ++sequence, datagram);
        sendto(ping->socket, (char*)datagram, (int)length, 0, (struct sockaddr*)&saddr, AddrLen);
        PltSleepMsInterruptible(&ping->thread, RL_PING_INTERVAL_MS);
    }
}

// Safe on a ping that never started or already stopped; the thread is joined before the socket
// it sends on is closed.
static void rlInputOnlyPingStop(RL_INPUT_ONLY_PING* ping) {
    if (ping->running) {
        PltInterruptThread(&ping->thread);
        PltJoinThread(&ping->thread);
        ping->running = false;
    }
    if (ping->socket != INVALID_SOCKET) {
        closeSocket(ping->socket);
        ping->socket = INVALID_SOCKET;
    }
}

// Starts pinging the negotiated video port. A ping that cannot start costs only the ping: the
// failure is logged, nothing is left open, and the session carries on.
static void rlInputOnlyPingStart(RL_INPUT_ONLY_PING* ping) {
    if (!rlInputOnlyPingsVideo(true, VideoPortNumber)) {
        return;
    }
    ping->socket = bindUdpSocket(RemoteAddr.ss_family, &LocalAddr, AddrLen, 0, SOCK_QOS_TYPE_VIDEO);
    if (ping->socket == INVALID_SOCKET) {
        Limelog("Input-only video ping: socket failed: %d\n", (int)LastSocketError());
        return;
    }
    if (PltCreateThread("VideoPing", rlInputOnlyPingProc, ping, &ping->thread) != 0) {
        Limelog("Input-only video ping: thread failed\n");
        rlInputOnlyPingStop(ping);
        return;
    }
    ping->running = true;
}
