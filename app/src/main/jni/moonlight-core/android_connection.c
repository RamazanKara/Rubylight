#include "Limelight-internal.h"
#include "android_microphone.h"

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

// Keep the core's queue initialization/teardown for control-channel callbacks, but never
// start media decoders, receive threads or missing-video timers for an input-only session.
#define startVideoStream(ctx, flags) (AndroidInputOnly ? 0 : startVideoStream(ctx, flags))
#define startAudioStream(ctx, flags) (AndroidInputOnly ? 0 : startAudioStream(ctx, flags))
#define stopVideoStream() do { if (!AndroidInputOnly) stopVideoStream(); } while (0)
#define stopAudioStream() do { if (!AndroidInputOnly) stopAudioStream(); } while (0)
#define startInputStream() androidStartInputStream()
#define stopInputStream() androidStopInputStream()
#define LiSendMouseMoveEvent(x, y) (AndroidRemoteMonitor ? 0 : LiSendMouseMoveEvent(x, y))
#include "moonlight-common-c/src/Connection.c"
