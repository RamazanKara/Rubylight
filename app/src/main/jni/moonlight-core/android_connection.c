#include "Limelight-internal.h"
#include "android_microphone.h"
#include "android_input_only_ping_thread.h"

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
// is the whole video side, so Vibepollo and Apollo hosts see the client they wait for.
static RL_INPUT_ONLY_PING inputOnlyPing = RL_INPUT_ONLY_PING_INIT;

// Keeps the core's queue initialization/teardown for control-channel callbacks, but never starts
// media decoders, receive threads or missing-video timers for an input-only session.
static int androidStartVideoStream(void* context, int flags) {
    if (!AndroidInputOnly) {
        return startVideoStream(context, flags);
    }
    rlInputOnlyPingStart(&inputOnlyPing);
    return 0;
}

static void androidStopVideoStream(void) {
    if (AndroidInputOnly) {
        rlInputOnlyPingStop(&inputOnlyPing);
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
