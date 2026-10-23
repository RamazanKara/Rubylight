// The real SDP a Sunshine-family host receives. ML_FF_HAPTICS_PCM switches a Vibepollo
// host's control loop to a 5 ms period, so it is offered only with USB DualSense haptics on.
#include "android_sdp.c"
#include <arpa/inet.h>
#include <netinet/in.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

// Connection state the SDP generator reads, owned by the connection and RTSP code.
struct sockaddr_storage RemoteAddr;
int AppVersionQuad[4];
STREAM_CONFIGURATION StreamConfig;
CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
DECODER_RENDERER_CALLBACKS VideoCallbacks;
AUDIO_RENDERER_CALLBACKS AudioCallbacks;
int NegotiatedVideoFormat;
bool HighQualitySurroundSupported;
bool HighQualitySurroundEnabled;
int AudioPacketDuration;
uint16_t VideoPortNumber;
uint16_t MicPortNumber;
uint32_t EncryptionFeaturesSupported;
uint32_t EncryptionFeaturesRequested;
uint32_t EncryptionFeaturesEnabled;
int PyroWaveRecordsEnabled;
bool AudioEncryptionEnabled;
uint16_t RtspPortNumber;
bool AndroidHapticsPcmRequested;

bool isReferenceFrameInvalidationSupportedByDecoder(void) { return false; }
bool PltSafeStrcpy(char* dest, size_t destSize, const char* src) {
    size_t length = strlen(src);
    if (length >= destSize) return false;
    memcpy(dest, src, length + 1);
    return true;
}
void addrToUrlSafeString(struct sockaddr_storage* addr, char* string, size_t stringLength) {
    snprintf(string, stringLength, "192.0.2.1");
}

static void sunshineStream(void) {
    memset(&StreamConfig, 0, sizeof(StreamConfig));
    StreamConfig.width = 1920;
    StreamConfig.height = 1080;
    StreamConfig.fps = 60;
    StreamConfig.bitrate = 20000;
    StreamConfig.packetSize = 1024;
    StreamConfig.streamingRemotely = STREAM_CFG_LOCAL;
    StreamConfig.audioConfiguration = AUDIO_CONFIGURATION_STEREO;
    ((struct sockaddr_in*)&RemoteAddr)->sin_family = AF_INET;
    VideoPortNumber = 47998;
    NegotiatedVideoFormat = VIDEO_FORMAT_H265;
    AppVersionQuad[0] = 7; AppVersionQuad[1] = 1; AppVersionQuad[2] = 431; AppVersionQuad[3] = -1;
}

// The x-ml-general.featureFlags value, or -1 when the SDP has none.
static long offeredFlags(bool hapticsPcm) {
    static const char attribute[] = "a=x-ml-general.featureFlags:";
    AndroidHapticsPcmRequested = hapticsPcm;
    EncryptionFeaturesEnabled = 0;
    int length = 0;
    char* sdp = getSdpPayloadForStreamConfig(13, &length);
    CHECK(sdp != NULL && length > 0);
    const char* line = strstr(sdp, attribute);
    long flags = line == NULL ? -1 : strtol(line + sizeof(attribute) - 1, NULL, 10);
    free(sdp);
    return flags;
}

// The x-ss-video[0].<name> value of the SDP the stream sends, or -1 when it has none.
static long pyroWaveHint(const char* name) {
    char attribute[64];
    snprintf(attribute, sizeof(attribute), "a=x-ss-video[0].%s:", name);
    int length = 0;
    char* sdp = getSdpPayloadForStreamConfig(13, &length);
    CHECK(sdp != NULL && length > 0);
    const char* line = strstr(sdp, attribute);
    long value = line == NULL ? -1 : strtol(line + strlen(attribute), NULL, 10);
    free(sdp);
    return value;
}

// The link-speed hint goes to hosts of every codec, spelled as in Nonary's moonlight-common-c,
// only when known; the calibrated pace is PyroWave's alone.
static void pacingHints(void) {
    static const int formats[] = {VIDEO_FORMAT_H264, VIDEO_FORMAT_H265, VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_PYROWAVE};
    for (int i = 0; i < 4; i++) {
        bool pyroWave = formats[i] == VIDEO_FORMAT_PYROWAVE;
        sunshineStream();
        NegotiatedVideoFormat = formats[i];
        CHECK(pyroWaveHint("pyrowaveLinkMbps") == -1);
        CHECK(pyroWaveHint("pyrowavePaceMbps") == -1);

        StreamConfig.pyrowaveLinkMbps = 2500;
        CHECK(pyroWaveHint("pyrowaveLinkMbps") == 2500);
        CHECK(pyroWaveHint("pyrowavePaceMbps") == -1);
        StreamConfig.pyrowavePaceMbps = 1800;
        CHECK(pyroWaveHint("pyrowaveLinkMbps") == 2500);
        CHECK(pyroWaveHint("pyrowavePaceMbps") == (pyroWave ? 1800 : -1));

        // Zero (Wi-Fi, cellular) and negative values are not hints.
        StreamConfig.pyrowaveLinkMbps = 0;
        CHECK(pyroWaveHint("pyrowaveLinkMbps") == -1);
        StreamConfig.pyrowaveLinkMbps = -1;
        CHECK(pyroWaveHint("pyrowaveLinkMbps") == -1);
    }
}

int main(void) {
    pacingHints();
    sunshineStream();
    CHECK(offeredFlags(false) == (ML_FF_FEC_STATUS | ML_FF_SESSION_ID_V1));
    CHECK(offeredFlags(true) == (ML_FF_FEC_STATUS | ML_FF_SESSION_ID_V1 | ML_FF_HAPTICS_PCM));

    // GFE hosts never receive Sunshine's feature flags.
    AppVersionQuad[3] = 0;
    CHECK(offeredFlags(true) == -1);
    puts("sdp feature flags: all tests passed");
    return 0;
}
