// Sends this device's microphone to the host as Opus over UDP.
//
// The stream follows ClassicOldSong/moonlight-common-c src/MicrophoneStream.c at commit
// 784fa1d0f501155ab01fea7cefe8a0e9c9628b77 (GPL-3.0, Copyright (C) ClassicOldSong and the
// moonlight-common-c contributors); the packet format is in android_microphone_packet.h.
// Differences: the Opus encoder lives here (Java hands over PCM), the socket and encoder are
// guarded by a lock so the capture thread can never send on a closed socket, and the packet is
// never sent unencrypted (hosts that take a microphone require encryption).

#include <jni.h>
#include <pthread.h>
#include <stdatomic.h>

#include <opus.h>

#include "Limelight-internal.h"
#include "android_microphone.h"

#define MIC_SAMPLE_RATE 48000
#define MIC_CHANNELS 1
#define MIC_BITRATE 32000
#define MIC_PACKET_LOSS_PERCENT 10
// Opus frame sizes at 48 kHz from 2.5 to 60 ms.
#define MIC_MAX_FRAME_SAMPLES 2880
// The largest Opus packet that still fits a datagram after both padding layers.
#define MIC_MAX_OPUS_BYTES (MAX_MIC_PACKET_SIZE - MIC_PACKET_HEADER_SIZE - 16 - 16)
// A longer gap between frames (mute, a stall) starts the encoder afresh.
#define MIC_RESET_GAP_MS 200

bool AndroidMicRequested;
uint16_t MicPortNumber;

static atomic_int micState = ATOMIC_VAR_INIT(MIC_STATE_OFF);
static pthread_mutex_t micLock = PTHREAD_MUTEX_INITIALIZER;
static SOCKET micSocket = INVALID_SOCKET;
static PPLT_CRYPTO_CONTEXT micEncryptionCtx;
static OpusEncoder* micEncoder;
static uint32_t micRiKeyId;
static uint16_t micSequenceNumber;
static uint64_t micLastFrameMs;
static uint32_t micPacketsSent;
static uint32_t micSendErrors;

void setMicrophoneState(int state) {
    atomic_store(&micState, state);
}

static void destroyMicrophoneStreamLocked(void) {
    if (micSocket != INVALID_SOCKET) {
        closeSocket(micSocket);
        micSocket = INVALID_SOCKET;
    }
    if (micEncryptionCtx != NULL) {
        PltDestroyCryptoContext(micEncryptionCtx);
        micEncryptionCtx = NULL;
    }
    if (micEncoder != NULL) {
        opus_encoder_destroy(micEncoder);
        micEncoder = NULL;
    }
    micRiKeyId = 0;
    micSequenceNumber = 0;
    micLastFrameMs = 0;
}

int initializeMicrophoneStream(void) {
    int err = 0;

    if (MicPortNumber == 0 || !(EncryptionFeaturesEnabled & SS_ENC_MICROPHONE)) {
        return -1;
    }

    pthread_mutex_lock(&micLock);
    destroyMicrophoneStreamLocked();
    micPacketsSent = 0;
    micSendErrors = 0;

    micEncryptionCtx = PltCreateCryptoContext();
    if (micEncryptionCtx == NULL) {
        err = -1;
        goto Fail;
    }

    // rikeyid is the first four bytes of the IV, big-endian (NvConnection puts it there).
    memcpy(&micRiKeyId, StreamConfig.remoteInputAesIv, sizeof(micRiKeyId));
    micRiKeyId = BE32(micRiKeyId);

    micEncoder = opus_encoder_create(MIC_SAMPLE_RATE, MIC_CHANNELS, OPUS_APPLICATION_VOIP, &err);
    if (micEncoder == NULL || err != OPUS_OK) {
        Limelog("Microphone: Opus encoder creation failed: %d\n", err);
        micEncoder = NULL;
        err = err != 0 ? err : -1;
        goto Fail;
    }
    // Speech settings the host expects: in-band FEC recovers a single lost packet from the next.
    opus_encoder_ctl(micEncoder, OPUS_SET_BITRATE(MIC_BITRATE));
    opus_encoder_ctl(micEncoder, OPUS_SET_SIGNAL(OPUS_SIGNAL_VOICE));
    opus_encoder_ctl(micEncoder, OPUS_SET_INBAND_FEC(1));
    opus_encoder_ctl(micEncoder, OPUS_SET_PACKET_LOSS_PERC(MIC_PACKET_LOSS_PERCENT));

    micSocket = bindUdpSocket(RemoteAddr.ss_family, &LocalAddr, AddrLen, 0, SOCK_QOS_TYPE_AUDIO);
    if (micSocket == INVALID_SOCKET) {
        err = LastSocketFail();
        goto Fail;
    }

    pthread_mutex_unlock(&micLock);
    setMicrophoneState(MIC_STATE_READY);
    Limelog("Microphone: ready to send to port %u (encrypted)\n", MicPortNumber);
    return 0;

Fail:
    destroyMicrophoneStreamLocked();
    pthread_mutex_unlock(&micLock);
    setMicrophoneState(MIC_STATE_UNAVAILABLE);
    Limelog("Microphone: initialization failed: %d\n", err);
    return err;
}

void destroyMicrophoneStream(void) {
    bool wasOpen;
    uint32_t sent, errors;

    pthread_mutex_lock(&micLock);
    wasOpen = micSocket != INVALID_SOCKET;
    sent = micPacketsSent;
    errors = micSendErrors;
    destroyMicrophoneStreamLocked();
    pthread_mutex_unlock(&micLock);

    if (wasOpen) {
        Limelog("Microphone: stopped after %u packets sent (%u send errors)\n", sent, errors);
    }
}

// Encodes one frame and sends it. Returns the datagram length, or a negative value on failure.
static int sendMicrophoneFrameLocked(const opus_int16* pcm, int samples) {
    unsigned char opus[MIC_MAX_OPUS_BYTES];
    uint8_t packet[MAX_MIC_PACKET_SIZE];
    LC_SOCKADDR saddr;
    uint64_t now;
    int opusLength;
    int packetLength;
    int err;

    if (micSocket == INVALID_SOCKET || micEncoder == NULL || micEncryptionCtx == NULL) {
        return -1;
    }

    now = PltGetMillis();
    if (micLastFrameMs != 0 && now - micLastFrameMs > MIC_RESET_GAP_MS) {
        opus_encoder_ctl(micEncoder, OPUS_RESET_STATE);
    }
    micLastFrameMs = now;

    opusLength = opus_encode(micEncoder, pcm, samples, opus, sizeof(opus));
    if (opusLength <= 0) {
        micSendErrors++;
        return opusLength < 0 ? opusLength : -1;
    }

    packetLength = micBuildPacket(micEncryptionCtx, (const uint8_t*)StreamConfig.remoteInputAesKey, micRiKeyId,
                                  micSequenceNumber, (uint32_t)now, opus, opusLength, packet);
    if (packetLength < 0) {
        Limelog("Microphone: could not encrypt a %d-byte packet\n", opusLength);
        micSendErrors++;
        return -1;
    }
    micSequenceNumber++;

    memcpy(&saddr, &RemoteAddr, sizeof(saddr));
    SET_PORT(&saddr, MicPortNumber);
    err = (int)sendto(micSocket, (const char*)packet, packetLength, 0, (struct sockaddr*)&saddr, AddrLen);
    if (err < 0) {
        err = LastSocketFail();
        // Log the first failure and then every 500th, so a dead route cannot flood the log.
        if (micSendErrors++ % 500 == 0) {
            Limelog("Microphone: send failed: %d\n", err);
        }
        return err > 0 ? -err : err;
    }

    if (micPacketsSent++ == 0) {
        Limelog("Microphone: first packet sent (%d bytes of Opus)\n", opusLength);
    }
    return err;
}

JNIEXPORT jint JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cSendMicrophonePcm(JNIEnv* env, jclass clazz, jshortArray pcm, jint samples) {
    opus_int16 frame[MIC_MAX_FRAME_SAMPLES];
    int ret;
    (void)clazz;

    // Opus takes 2.5, 5, 10, 20, 40 or 60 ms frames.
    if (pcm == NULL || (samples != 120 && samples != 240 && samples != 480 &&
                        samples != 960 && samples != 1920 && samples != 2880) ||
            (*env)->GetArrayLength(env, pcm) < samples) {
        return -1;
    }
    (*env)->GetShortArrayRegion(env, pcm, 0, samples, frame);
    if ((*env)->ExceptionCheck(env)) {
        return -1;
    }

    pthread_mutex_lock(&micLock);
    ret = sendMicrophoneFrameLocked(frame, samples);
    pthread_mutex_unlock(&micLock);
    return ret;
}

JNIEXPORT jint JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cGetMicrophoneState(JNIEnv* env, jclass clazz) {
    (void)env; (void)clazz;
    return atomic_load(&micState);
}
