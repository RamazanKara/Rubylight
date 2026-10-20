// Drives the patched RtpAudioQueue. The receive and decode steps mirror AudioReceiveThreadProc
// and decodeInputData in AudioStream.c without the socket, with a direct-submit renderer.
#include "Limelight-internal.h"
#include "shim.h"

extern uint64_t dtNow;
extern int AudioPacketDuration;
extern bool AudioEncryptionEnabled;
void dtLogU8(uint8_t v);
void dtLogU32(uint32_t v);
void dtLogBytes(const void* data, size_t length);

#define MAX_PACKET_SIZE 1400

typedef struct _QUEUE_AUDIO_PACKET_HEADER {
    LINKED_BLOCKING_QUEUE_ENTRY lentry;
    int size;
} QUEUED_AUDIO_PACKET_HEADER, *PQUEUED_AUDIO_PACKET_HEADER;

typedef struct _QUEUED_AUDIO_PACKET {
    QUEUED_AUDIO_PACKET_HEADER header;
    char data[MAX_PACKET_SIZE];
} QUEUED_AUDIO_PACKET, *PQUEUED_AUDIO_PACKET;

static RTP_AUDIO_QUEUE rtpAudioQueue;
static PPLT_CRYPTO_CONTEXT audioDecryptionCtx;
static uint32_t avRiKeyId;
static uint32_t packetsToDrop;
static bool receivedDataFromPeer;
static uint8_t audioKey[16];

static void playSample(const char* data, int length) {
    dtLogU8('A');
    dtLogU32((uint32_t)length);
    if (length) dtLogBytes(data, (size_t)length);
}

static void decodeInputData(PQUEUED_AUDIO_PACKET packet) {
    if (packet->header.size == 0) {
        playSample(NULL, 0);
        return;
    }

    PRTP_PACKET rtp = (PRTP_PACKET)&packet->data[0];
    if (AudioEncryptionEnabled) {
        unsigned char decryptedOpusData[ROUND_TO_PKCS7_PADDED_LEN(MAX_PACKET_SIZE)];
        unsigned char iv[16] = { 0 };
        int dataLength = packet->header.size - sizeof(*rtp);
        uint32_t ivSeq = BE32(avRiKeyId + rtp->sequenceNumber);
        memcpy(iv, &ivSeq, sizeof(ivSeq));
        if (!PltDecryptMessage(audioDecryptionCtx, ALGORITHM_AES_CBC, CIPHER_FLAG_RESET_IV | CIPHER_FLAG_FINISH,
                               audioKey, sizeof(audioKey), iv, sizeof(iv), NULL, 0,
                               (unsigned char*)(rtp + 1), dataLength, decryptedOpusData, &dataLength)) {
            return;
        }
        playSample((char*)decryptedOpusData, dataLength);
    }
    else {
        playSample((char*)(rtp + 1), packet->header.size - sizeof(*rtp));
    }
}

void dt_audio_init(const dt_audio_config* config) {
    memcpy(AppVersionQuad, config->app_version, sizeof(AppVersionQuad));
    AudioPacketDuration = config->packet_duration;
    AudioEncryptionEnabled = config->encrypted;
    memcpy(audioKey, config->key, 16);
    memcpy(&avRiKeyId, config->iv, sizeof(avRiKeyId));
    avRiKeyId = BE32(avRiKeyId);
    packetsToDrop = 500 / AudioPacketDuration;
    receivedDataFromPeer = false;
    dtNow = 0;
    RtpaInitializeQueue(&rtpAudioQueue);
    audioDecryptionCtx = PltCreateCryptoContext();
    dt_log_clear();
}

void dt_audio_destroy(void) {
    PltDestroyCryptoContext(audioDecryptionCtx);
    RtpaCleanupQueue(&rtpAudioQueue);
}

void dt_audio_timeout(void) {
    if (receivedDataFromPeer) packetsToDrop = 0;
}

void dt_audio_receive(const uint8_t* data, int length, uint64_t now_us) {
    dtNow = now_us;
    PQUEUED_AUDIO_PACKET packet = malloc(sizeof(*packet));
    packet->header.size = length < MAX_PACKET_SIZE ? length : MAX_PACKET_SIZE;
    memcpy(packet->data, data, packet->header.size);
    if (packet->header.size < (int)sizeof(RTP_PACKET)) {
        free(packet);
        return;
    }

    PRTP_PACKET rtp = (PRTP_PACKET)&packet->data[0];
    receivedDataFromPeer = true;
    if (packetsToDrop > 0) {
        if (rtp->packetType == 97) packetsToDrop--;
        free(packet);
        return;
    }

    rtp->sequenceNumber = BE16(rtp->sequenceNumber);
    rtp->timestamp = BE32(rtp->timestamp);
    rtp->ssrc = BE32(rtp->ssrc);

    int queueStatus = RtpaAddPacket(&rtpAudioQueue, rtp, (uint16_t)packet->header.size);
    if (RTPQ_HANDLE_NOW(queueStatus)) {
        decodeInputData(packet);
    }
    else if (RTPQ_PACKET_READY(queueStatus)) {
        uint16_t qlength;
        PQUEUED_AUDIO_PACKET queuedPacket;
        while ((queuedPacket = (PQUEUED_AUDIO_PACKET)RtpaGetQueuedPacket(&rtpAudioQueue, sizeof(QUEUED_AUDIO_PACKET_HEADER), &qlength)) != NULL) {
            queuedPacket->header.size = qlength;
            decodeInputData(queuedPacket);
            free(queuedPacket);
        }
    }
    free(packet);
}

void dt_audio_stats(uint32_t out[7]) {
    out[0] = rtpAudioQueue.stats.packetCountAudio;
    out[1] = rtpAudioQueue.stats.packetCountFec;
    out[2] = rtpAudioQueue.stats.packetCountFecRecovered;
    out[3] = rtpAudioQueue.stats.packetCountFecFailed;
    out[4] = rtpAudioQueue.stats.packetCountOOS;
    out[5] = rtpAudioQueue.stats.packetCountInvalid;
    out[6] = rtpAudioQueue.stats.packetCountFecInvalid;
}
