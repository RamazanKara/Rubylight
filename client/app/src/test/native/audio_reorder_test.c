#include "../../main/jni/moonlight-core/moonlight-common-c/src/RtpAudioQueue.c"
#undef NDEBUG
#include <assert.h>

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
int AudioPacketDuration;
int AppVersionQuad[4];
static uint64_t currentTimeUs;

uint64_t PltGetMicroseconds(void) {
    return currentTimeUs;
}

int main(void) {
    const int durations[] = {5, 10, 20};
    for (int i = 0; i < 3; i++) {
        RTP_AUDIO_QUEUE queue = {0};
        RTPA_FEC_BLOCK first = {0}, second = {0};
        first.fecHeader.baseSequenceNumber = 100;
        first.queueTimeUs = 1000000;
        first.next = &second;
        second.prev = &first;
        queue.blockHead = &first;
        queue.blockTail = &second;
        queue.nextRtpSequenceNumber = 100;
        queue.receivedOosData = true;
        AudioPacketDuration = durations[i];

        currentTimeUs = first.queueTimeUs + AudioPacketDuration * RTPA_DATA_SHARDS * 1000 +
                RTPQ_OOS_WAIT_TIME_MS * 1000;
        handleMissingPackets(&queue);
        assert(!first.allowDiscontinuity);
        assert(queue.stats.packetCountFecFailed == 0);
        currentTimeUs++;
        handleMissingPackets(&queue);
        assert(first.allowDiscontinuity);
        assert(queue.stats.packetCountFecFailed == 1);

        first.allowDiscontinuity = false;
        queue.receivedOosData = false;
        currentTimeUs = first.queueTimeUs;
        handleMissingPackets(&queue);
        assert(first.allowDiscontinuity);
    }
    return 0;
}
