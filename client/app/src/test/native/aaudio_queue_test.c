#include "../../main/jni/moonlight-core/aaudio_queue.h"
#include <assert.h>
#include <stdio.h>

static void assertSilence(const int16_t* samples, uint32_t frames) {
    for (uint32_t i = 0; i < frames * 2; i++) assert(samples[i] == 0);
}

static void startupWaitsForTheReserve(void) {
    AudioQueue queue;
    int16_t ring[1024 * 2], input[240 * 2], output[192 * 2];
    for (int i = 0; i < 240 * 2; i++) input[i] = (int16_t)(i + 1);
    audioQueueInit(&queue, ring, 1024, 384);
    assert(audioQueueWrite(&queue, input, 240) == 240);
    audioQueueRead(&queue, output, 192);
    assertSilence(output, 192);
    assert(atomic_load(&queue.readFrames) == 0);
    assert(atomic_load(&queue.underruns) == 0);

    assert(audioQueueWrite(&queue, input, 144) == 144);
    audioQueueRead(&queue, output, 192);
    assert(memcmp(output, input, sizeof(output)) == 0);
    assert(atomic_load(&queue.readFrames) == 192);
}

static void underrunRebuildsTheReserve(void) {
    AudioQueue queue;
    int16_t ring[1024 * 2], input[384 * 2], output[192 * 2];
    for (int i = 0; i < 384 * 2; i++) input[i] = (int16_t)(i + 1);
    audioQueueInit(&queue, ring, 1024, 384);
    assert(audioQueueWrite(&queue, input, 384) == 384);
    audioQueueRead(&queue, output, 192);
    audioQueueRead(&queue, output, 192);
    audioQueueRead(&queue, output, 192);
    assert(atomic_load(&queue.underruns) == 1);
    assertSilence(output, 192);
    for (int i = 0; i < 5; i++) audioQueueRead(&queue, output, 192);
    assert(atomic_load(&queue.underruns) == 1);

    assert(audioQueueWrite(&queue, input, 240) == 240);
    audioQueueRead(&queue, output, 192);
    assertSilence(output, 192);
    assert(atomic_load(&queue.readFrames) == 384);
    assert(audioQueueWrite(&queue, input + 240 * 2, 144) == 144);
    audioQueueRead(&queue, output, 192);
    assert(memcmp(output, input, sizeof(output)) == 0);
    audioQueueRead(&queue, output, 192);
    assert(memcmp(output, input + 192 * 2, sizeof(output)) == 0);
    assert(atomic_load(&queue.underruns) == 1);
}

static void ringAndFrameCountersWrapWithoutChangingPcm(void) {
    AudioQueue queue;
    int16_t ring[16 * 2], input[12 * 2], output[12 * 2];
    for (int i = 0; i < 12 * 2; i++) input[i] = (int16_t)(i - 12);
    audioQueueInit(&queue, ring, 16, 12);
    atomic_store(&queue.readFrames, UINT32_MAX - 3);
    atomic_store(&queue.writtenFrames, UINT32_MAX - 3);
    assert(audioQueueWrite(&queue, input, 12) == 12);
    assert(audioQueueWrite(&queue, input, 1) == 0);
    audioQueueRead(&queue, output, 12);
    assert(memcmp(output, input, sizeof(input)) == 0);
    assert(atomic_load(&queue.readFrames) == 8);
    assert(atomic_load(&queue.writtenFrames) == 8);
    assert(atomic_load(&queue.underruns) == 0);
}

static void partialUnderrunWaitsForTheNewTarget(void) {
    AudioQueue queue;
    int16_t ring[16 * 2], input[8 * 2], output[6 * 2];
    for (int i = 0; i < 8 * 2; i++) input[i] = (int16_t)(100 + i);
    audioQueueInit(&queue, ring, 16, 8);
    assert(audioQueueWrite(&queue, input, 8) == 8);
    audioQueueRead(&queue, output, 6);
    audioQueueRead(&queue, output, 6);
    assert(memcmp(output, input + 6 * 2, 2 * 2 * sizeof(int16_t)) == 0);
    assertSilence(output + 2 * 2, 4);
    assert(atomic_load(&queue.underruns) == 1);

    atomic_store(&queue.targetFrames, 12);
    assert(audioQueueWrite(&queue, input, 8) == 8);
    audioQueueRead(&queue, output, 6);
    assertSilence(output, 6);
    assert(atomic_load(&queue.readFrames) == 8);
    assert(audioQueueWrite(&queue, input, 4) == 4);
    audioQueueRead(&queue, output, 6);
    assert(memcmp(output, input, sizeof(output)) == 0);
    assert(atomic_load(&queue.underruns) == 1);
}

static void differentPacketAndCallbackCadencesRemainContinuous(void) {
    AudioQueue queue;
    int16_t ring[1024 * 2], input[48000 * 2], output[192 * 2];
    for (int i = 0; i < 48000 * 2; i++) input[i] = (int16_t)(1 + i % 30000);
    audioQueueInit(&queue, ring, 1024, 384);
    uint32_t offered = 0, written = 0, played = 0;
    bool started = false;
    // A 5 ms Opus packet and a 4 ms hardware burst must not insert silence
    // between packets merely because their boundaries do not coincide.
    for (int ms = 0; ms < 1000; ms++) {
        if (ms % 5 == 0) offered += 240;
        written += audioQueueWrite(&queue, input + written * 2, offered - written);
        if (ms % 4 == 0) {
            audioQueueRead(&queue, output, 192);
            if (!started && !queue.primed) {
                assertSilence(output, 192);
            } else {
                started = true;
                assert(memcmp(output, input + played * 2, sizeof(output)) == 0);
                played += 192;
            }
        }
    }
    assert(played > 47000);
    assert(atomic_load(&queue.underruns) == 0);
}

int main(void) {
    startupWaitsForTheReserve();
    underrunRebuildsTheReserve();
    ringAndFrameCountersWrapWithoutChangingPcm();
    partialUnderrunWaitsForTheNewTarget();
    differentPacketAndCallbackCadencesRemainContinuous();
    puts("AAudio queue tests passed");
    return 0;
}
