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

// Whether the SDP the stream sends contains the text.
static int sdpHas(const char* text) {
    int length = 0;
    char* sdp = getSdpPayloadForStreamConfig(13, &length);
    CHECK(sdp != NULL && length > 0);
    int found = strstr(sdp, text) != NULL;
    free(sdp);
    return found;
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

// With "Tell the PC the link speed" off the app passes 0: no link attribute for any codec, while
// the calibrated pace stays PyroWave's.
static void linkSpeedSettingOff(void) {
    static const int formats[] = {VIDEO_FORMAT_H264, VIDEO_FORMAT_H265, VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_PYROWAVE};
    for (int i = 0; i < 4; i++) {
        sunshineStream();
        NegotiatedVideoFormat = formats[i];
        StreamConfig.pyrowaveLinkMbps = 0;
        StreamConfig.pyrowavePaceMbps = 1800;
        CHECK(sdpHas("pyrowaveLinkMbps") == 0);
        CHECK(pyroWaveHint("pyrowavePaceMbps") == (formats[i] == VIDEO_FORMAT_PYROWAVE ? 1800 : -1));
    }
}

// The wireless hint is Rubylight's own attribute, sent as 1 for every codec only when set,
// and never as 0.
static void clientWirelessHint(void) {
    static const int formats[] = {VIDEO_FORMAT_H264, VIDEO_FORMAT_H265, VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_PYROWAVE};
    for (int i = 0; i < 4; i++) {
        sunshineStream();
        NegotiatedVideoFormat = formats[i];
        CHECK(sdpHas("a=x-rl-video[0].clientWireless:") == 0);
        StreamConfig.clientWireless = 1;
        CHECK(sdpHas("a=x-rl-video[0].clientWireless:1") == 1);
        StreamConfig.clientWireless = 0;
        CHECK(sdpHas("a=x-rl-video[0].clientWireless") == 0);
    }
}

// The whole SDP the stream sends; the caller frees it.
static char* sdpText(void) {
    int length = 0;
    char* sdp = getSdpPayloadForStreamConfig(13, &length);
    CHECK(sdp != NULL && length > 0);
    return sdp;
}

// The x-rl-video[0].hdrFormat the stream sends for this request and offer, -1 for none.
static long hdrFormatSent(int videoFormat, uint8_t asked, uint8_t offered) {
    static const char attribute[] = "a=x-rl-video[0].hdrFormat:";
    sunshineStream();
    NegotiatedVideoFormat = videoFormat;
    AndroidHdrFormatRequested = asked;
    RlHdrFormatsOffered = offered;
    char* sdp = sdpText();
    const char* line = strstr(sdp, attribute);
    long value = line == NULL ? -1 : strtol(line + sizeof(attribute) - 1, NULL, 10);
    CHECK(RlHdrFormatAnnounced == (value < 0 ? 0 : value));
    free(sdp);
    return value;
}

// The 3.0 HDR format goes out only when asked for and offered, as one bit, on a 10-bit HEVC or
// AV1 stream, Dolby Vision 8.1 and 8.4 on HEVC alone and 10.1 on AV1 alone; as the Rust client's
// hdr_format test.
static void hdrFormat(void) {
    static const int formats[] = {VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10, VIDEO_FORMAT_AV1_MAIN10,
                                  VIDEO_FORMAT_PYROWAVE};
    for (int i = 0; i < 4; i++) {
        CHECK(hdrFormatSent(formats[i], 0, RL_HDR_ALL) == -1);
        CHECK(hdrFormatSent(formats[i], RL_HDR_DV_84, 0) == -1);
        CHECK(hdrFormatSent(formats[i], RL_HDR_DV_84, RL_HDR_HLG | RL_HDR_PQ) == -1);
        CHECK(hdrFormatSent(formats[i], RL_HDR_DV_81 | RL_HDR_DV_84, RL_HDR_ALL) == -1);
    }
    CHECK(hdrFormatSent(VIDEO_FORMAT_H265_MAIN10, RL_HDR_DV_81, RL_HDR_PQ | RL_HDR_DV_81) == 4);
    CHECK(hdrFormatSent(VIDEO_FORMAT_H265_MAIN10, RL_HDR_DV_84, RL_HDR_ALL) == 8);
    CHECK(hdrFormatSent(VIDEO_FORMAT_H265_MAIN10, RL_HDR_HLG, RL_HDR_ALL) == 2);
    CHECK(hdrFormatSent(VIDEO_FORMAT_AV1_MAIN10, RL_HDR_HLG, RL_HDR_ALL) == 2);
    CHECK(hdrFormatSent(VIDEO_FORMAT_AV1_MAIN10, RL_HDR_DV_84, RL_HDR_ALL) == -1);
    CHECK(hdrFormatSent(VIDEO_FORMAT_AV1_MAIN10, RL_HDR_DV_81, RL_HDR_ALL) == -1);
    CHECK(hdrFormatSent(VIDEO_FORMAT_AV1_MAIN10, RL_HDR_DV_101, RL_HDR_PQ | RL_HDR_DV_101) == 16);
    CHECK(hdrFormatSent(VIDEO_FORMAT_H265_MAIN10, RL_HDR_DV_101, RL_HDR_ALL) == -1);
    CHECK(hdrFormatSent(VIDEO_FORMAT_AV1_MAIN8, RL_HDR_DV_101, RL_HDR_ALL) == -1);
    CHECK(hdrFormatSent(VIDEO_FORMAT_H265, RL_HDR_HLG, RL_HDR_ALL) == -1);
    CHECK(hdrFormatSent(VIDEO_FORMAT_PYROWAVE, RL_HDR_HLG, RL_HDR_ALL) == -1);
    // PyroWave's 10-bit formats are in VIDEO_FORMAT_MASK_10BIT but carry no HDR format: the host
    // refuses one in the ANNOUNCE.
    static const int pyroWave10[] = {VIDEO_FORMAT_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10_444};
    static const uint8_t bits[] = {RL_HDR_PQ, RL_HDR_HLG, RL_HDR_DV_81, RL_HDR_DV_84, RL_HDR_DV_101};
    for (int i = 0; i < 2; i++) {
        CHECK((pyroWave10[i] & VIDEO_FORMAT_MASK_10BIT) != 0);
        for (int bit = 0; bit < 5; bit++) {
            CHECK(hdrFormatSent(pyroWave10[i], bits[bit], RL_HDR_ALL) == -1);
            CHECK(rlAnnouncedHdrFormat(bits[bit], RL_HDR_ALL, pyroWave10[i]) == 0);
        }
    }

    // Only the one line differs from the plain description.
    hdrFormatSent(VIDEO_FORMAT_H265_MAIN10, 0, RL_HDR_ALL);
    char* plain = sdpText();
    hdrFormatSent(VIDEO_FORMAT_H265_MAIN10, RL_HDR_DV_81, RL_HDR_ALL);
    char* asked = sdpText();
    static const char line[] = "a=x-rl-video[0].hdrFormat:4 \r\n";
    char* at = strstr(asked, line);
    CHECK(at != NULL);
    memmove(at, at + sizeof(line) - 1, strlen(at + sizeof(line) - 1) + 1);
    CHECK(strcmp(plain, asked) == 0);
    free(plain);
    free(asked);
    AndroidHdrFormatRequested = 0;
    RlHdrFormatsOffered = 0;
}

// The DESCRIBE answer's formats, read as strictly as the Rust client reads them.
static void hdrFormatsOffer(void) {
    CHECK(rlParseHdrFormats("") == 0);
    CHECK(rlParseHdrFormats("a=x-rl-video.hdrFormats:5\r\n") == 5);
    CHECK(rlParseHdrFormats("v=0\r\na=x-rl-control:0x5531\r\na=x-rl-video.hdrFormats:15\r\n") == 15);
    CHECK(rlParseHdrFormats("a=x-rl-video.hdrFormats:31\r\n") == 31);
    CHECK(rlParseHdrFormats("a=x-rl-video.hdrFormats:255\r\n") == 31);
    CHECK(rlParseHdrFormats("a=x-rl-video.hdrFormats: 2 \r\n") == 2);
    CHECK(rlParseHdrFormats("a=x-rl-video.hdrFormats:2") == 2);
    static const char* const malformed[] = {
        "a=x-rl-video.hdrFormats:256\r\n", "a=x-rl-video.hdrFormats:-1\r\n", "a=x-rl-video.hdrFormats:0x2\r\n",
        "a=x-rl-video.hdrFormatsX:2\r\n", "b=x-rl-video.hdrFormats:2\r\n", "a=x-rl-video.hdrFormats:\r\n",
    };
    for (size_t i = 0; i < sizeof(malformed) / sizeof(malformed[0]); i++) {
        CHECK(rlParseHdrFormats(malformed[i]) == 0);
    }
}

int main(void) {
    pacingHints();
    linkSpeedSettingOff();
    clientWirelessHint();
    hdrFormat();
    hdrFormatsOffer();
    sunshineStream();
    CHECK(offeredFlags(false) == (ML_FF_FEC_STATUS | ML_FF_SESSION_ID_V1));
    CHECK(offeredFlags(true) == (ML_FF_FEC_STATUS | ML_FF_SESSION_ID_V1 | ML_FF_HAPTICS_PCM));

    // GFE hosts never receive Sunshine's feature flags.
    AppVersionQuad[3] = 0;
    CHECK(offeredFlags(true) == -1);
    puts("sdp feature flags: all tests passed");
    return 0;
}
