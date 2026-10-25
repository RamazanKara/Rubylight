#include "Limelight-internal.h"
#include "android_microphone.h"
#include "android_input_only_ping.h"

bool AndroidInputOnly;
bool AndroidRemoteMonitor;
bool AndroidHapticsPcmRequested;

// The microphone comes up with the last stage, as in ClassicOldSong/moonlight-common-c, and goes
// down with it, before the other streams. A microphone that cannot start costs only itself.
static int androidStartInputStream(void) {
    int err = AndroidRemoteMonitor ? 0 : startInputStream();
    if (err == 0 && MicPortNumber != 0) {
        if (!(EncryptionFeaturesEnabled & SS_ENC_MICROPHONE)) {
            Limelog("Microphone: encryption was not negotiated; not sending\n");
            setMicrophoneState(MIC_STATE_UNAVAILABLE);
        }
        else {
            initializeMicrophoneStream();
        }
    }
    return err;
}

static int androidStopInputStream(void) {
    destroyMicrophoneStream();
    return AndroidRemoteMonitor ? 0 : stopInputStream();
}

// Native input-only sends the video ping and nothing else of the video stream: the ping thread
// below is the whole video side, so Vibepollo and Apollo hosts see the client they wait for.
static SOCKET inputOnlyPingSocket = INVALID_SOCKET;
static PLT_THREAD inputOnlyPingThread;
static bool inputOnlyPinging;

// Socket errors are ignored, as in VideoStream.c: nothing reads this socket and an ICMP port
// unreachable (the host may not have bound the port yet) must not end or log anything.
static void inputOnlyPingProc(void* context) {
    uint8_t datagram[RL_PING_MAX_SIZE];
    LC_SOCKADDR saddr;
    uint32_t sequence = 0;

    memcpy(&saddr, &RemoteAddr, sizeof(saddr));
    SET_PORT(&saddr, VideoPortNumber);
    while (!PltIsThreadInterrupted(&inputOnlyPingThread)) {
        size_t length = rlBuildVideoPing((const uint8_t*)VideoPingPayload.payload, ++sequence, datagram);
        sendto(inputOnlyPingSocket, (char*)datagram, (int)length, 0, (struct sockaddr*)&saddr, AddrLen);
        PltSleepMsInterruptible(&inputOnlyPingThread, RL_PING_INTERVAL_MS);
    }
}

static void stopInputOnlyPing(void) {
    if (inputOnlyPinging) {
        PltInterruptThread(&inputOnlyPingThread);
        PltJoinThread(&inputOnlyPingThread);
        inputOnlyPinging = false;
    }
    if (inputOnlyPingSocket != INVALID_SOCKET) {
        closeSocket(inputOnlyPingSocket);
        inputOnlyPingSocket = INVALID_SOCKET;
    }
}

// Keeps the core's queue initialization/teardown for control-channel callbacks, but never starts
// media decoders, receive threads or missing-video timers for an input-only session. A ping that
// cannot start costs only the ping.
static int androidStartVideoStream(void* context, int flags) {
    if (!AndroidInputOnly) {
        return startVideoStream(context, flags);
    }
    if (!rlInputOnlyPingsVideo(true, VideoPortNumber)) {
        return 0;
    }
    inputOnlyPingSocket = bindUdpSocket(RemoteAddr.ss_family, &LocalAddr, AddrLen, 0, SOCK_QOS_TYPE_VIDEO);
    if (inputOnlyPingSocket == INVALID_SOCKET) {
        Limelog("Input-only video ping: socket failed: %d\n", (int)LastSocketError());
        return 0;
    }
    if (PltCreateThread("VideoPing", inputOnlyPingProc, NULL, &inputOnlyPingThread) != 0) {
        Limelog("Input-only video ping: thread failed\n");
        stopInputOnlyPing();
        return 0;
    }
    inputOnlyPinging = true;
    return 0;
}

static void androidStopVideoStream(void) {
    if (AndroidInputOnly) {
        stopInputOnlyPing();
    }
    else {
        stopVideoStream();
    }
}

#define startVideoStream(ctx, flags) androidStartVideoStream(ctx, flags)
#define startAudioStream(ctx, flags) (AndroidInputOnly ? 0 : startAudioStream(ctx, flags))
#define stopVideoStream() androidStopVideoStream()
#define stopAudioStream() do { if (!AndroidInputOnly) stopAudioStream(); } while (0)
#define startInputStream() androidStartInputStream()
#define stopInputStream() androidStopInputStream()
#define LiSendMouseMoveEvent(x, y) (AndroidRemoteMonitor ? 0 : LiSendMouseMoveEvent(x, y))
#include "moonlight-common-c/src/Connection.c"
