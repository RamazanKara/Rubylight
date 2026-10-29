#include "../../main/jni/moonlight-core/aaudio_output.c"
#include <assert.h>
#include <stdio.h>

static int hardwareFrames = 384;
static jint stats[13];
static int concealCalls, concealResult = -1;

static int32_t burst(AAudioStream* stream) { (void)stream; return 192; }
static int32_t capacity(AAudioStream* stream) { (void)stream; return 384; }
static int32_t size(AAudioStream* stream) { (void)stream; return hardwareFrames; }
static int32_t xruns(AAudioStream* stream) { (void)stream; return 0; }
static int32_t deviceId(AAudioStream* stream) { (void)stream; return 42; }
static int32_t setSize(AAudioStream* stream, int32_t frames) {
    (void)stream;
    hardwareFrames = frames < 384 ? frames : 384;
    return hardwareFrames;
}
static jintArray newStats(JNIEnv* env, jsize length) {
    (void)env;
    assert(length <= 13);
    return stats;
}
// Opus concealment stand-in: 2.5 ms units, each sample 1000, or concealResult frames when that is set.
static int fakeConceal(int16_t* pcm, int frames) {
    concealCalls++;
    assert(frames > 0 && frames % 120 == 0 && frames <= CONCEAL_MAX_FRAMES);
    int count = concealResult >= 0 ? concealResult : frames;
    for (int i = 0; i < count * 2; i++) pcm[i] = 1000;
    return count;
}

static bool allEqual(const int16_t* pcm, uint32_t samples, int16_t value) {
    for (uint32_t i = 0; i < samples; i++) if (pcm[i] != value) return false;
    return true;
}

// TCL C6K TV: 864-frame callbacks, a 1728-frame reserve, 5 ms packets, Wi-Fi going quiet for 130 ms.
static void stallsAreConcealedAndTheirPacketsSkipped(void) {
    static AudioOutput output;
    static int16_t ring[16384 * 2], input[48000 * 2], pcm[864 * 2];
    memset(&output, 0, sizeof(output));
    for (int i = 0; i < 48000 * 2; i++) input[i] = 2000;
    audioQueueInit(&output.queue, ring, 16384, 1728);
    output.rate = 48000;
    output.conceal = fakeConceal;
    concealCalls = 0;
    concealResult = -1;

    // Nothing has played yet: the start waits for the reserve in silence, no concealment, nothing late.
    audioFill(&output, pcm, 864);
    assert(allEqual(pcm, 864 * 2, 0) && concealCalls == 0 && atomic_load(&output.lateFrames) == 0);

    assert(audioQueueWrite(&output.queue, input, 1728) == 1728);
    audioFill(&output, pcm, 864);
    audioFill(&output, pcm, 864);
    assert(allEqual(pcm, 864 * 2, 2000) && concealCalls == 0);

    // The stall: Opus fills each callback the ring cannot, and the gap counts as late frames.
    for (int i = 0; i < 7; i++) {
        audioFill(&output, pcm, 864);
        assert(allEqual(pcm, 864 * 2, 1000));
    }
    assert(atomic_load(&output.concealedFrames) == 7 * 864);
    assert(atomic_load(&output.lateFrames) == 7 * 864);
    assert(atomic_load(&output.queue.underruns) == 1);

    // The stalled packets come at once: the drained reserve's and the concealed time's, 33 packets in the
    // network queue. Of those whose time was concealed the oldest are skipped, the first one too, though it
    // came after the gap: the packets behind it are a burst. The newest refill the reserve, and only a
    // packet's rest waits beyond it. Counting the ring alone kept the whole network queue on top of the
    // reserve (about 75 ms here, the late bound's 36 + 40), which the late bound then dropped a packet at a time.
    uint64_t now = 5000000000ull;
    int packets = (1728 + 7 * 864 + 239) / 240, skipped = 0, kept = 0;
    uint32_t refilled = 0;
    int calls = concealCalls;
    output.concealCount = 0;
    for (int i = 0; i < packets; i++) {
        if (audioSkipLate(&output, now, 240, (uint32_t)(packets - 1 - i) * 240)) {
            assert(kept == 0);
            skipped++;
        } else {
            refilled += audioQueueWrite(&output.queue, input, 240);
            if (++kept == 1) {
                // Refilling: the ring is not empty, so the decoder is left to the stalled packets; silence until primed.
                audioFill(&output, pcm, 864);
                assert(concealCalls == calls && allEqual(pcm, 864 * 2, 0));
            }
        }
        now += 200000;
    }
    assert(skipped == 25 && kept == 8 && refilled == 1728);
    assert(atomic_load(&output.skippedFrames) == (uint32_t)skipped * 240);
    assert(atomic_load(&output.skippedFrames) <= atomic_load(&output.concealedFrames));
    // A packet at its own pace is never skipped, even with time still late and the ring full.
    assert(atomic_load(&output.lateFrames) >= 240);
    now += 5000000;
    assert(!audioSkipLate(&output, now, 240, 0));

    // Primed again: the ring plays, faded in from silence over 5 ms, and late frames left are dropped.
    audioFill(&output, pcm, 864);
    assert(pcm[0] == 0 && pcm[2 * 120] == 1000 && pcm[2 * 240] == 2000 && pcm[2 * 863] == 2000);
    assert(!output.concealing && atomic_load(&output.lateFrames) == 0);
    audioFill(&output, pcm, 864);
    assert(allEqual(pcm, 864 * 2, 0) == false);

    // A long outage: concealment stops at 200 ms, then silence; late frames stop there too.
    audioFill(&output, pcm, 864);
    calls = concealCalls;
    for (int i = 0; i < 20; i++) audioFill(&output, pcm, 864);
    assert(allEqual(pcm, 864 * 2, 0));
    assert(atomic_load(&output.lateFrames) == 9600);
    assert(atomic_load(&output.concealedFrames) <= 7 * 864 + 9600 + 864);
    assert(concealCalls > calls);

    // A concealer that refuses (its decoder busy or gone) leaves silence, still counted as late.
    static AudioOutput refused;
    memset(&refused, 0, sizeof(refused));
    audioQueueInit(&refused.queue, ring, 16384, 1728);
    refused.rate = 48000;
    refused.conceal = fakeConceal;
    concealResult = 0;
    assert(audioQueueWrite(&refused.queue, input, 1728) == 1728);
    audioFill(&refused, pcm, 864);
    audioFill(&refused, pcm, 864);
    audioFill(&refused, pcm, 864);
    assert(allEqual(pcm, 864 * 2, 0) && atomic_load(&refused.concealedFrames) == 0);
    assert(atomic_load(&refused.lateFrames) == 864);
    concealResult = -1;
}

// One gap after the ring last refilled to its reserve: the callbacks drain it, the last one short, then
// `empty` callbacks find nothing, then the whole reserve comes at once. Returns the gap's dry frames.
static uint32_t gap(AudioOutput* output, int16_t* pcm, const int16_t* input, int empty) {
    while (atomic_load(&output->queue.writtenFrames) - atomic_load(&output->queue.readFrames) >= 192)
        audioFill(output, pcm, 192);
    audioFill(output, pcm, 192);
    for (int i = 0; i < empty; i++) audioFill(output, pcm, 192);
    uint32_t dry = output->dryRun, target = atomic_load(&output->queue.targetFrames);
    assert(audioQueueWrite(&output->queue, input, target) == target);
    audioFill(output, pcm, 192);
    assert(!output->concealing);
    return dry;
}

// Retroid: 192-frame callbacks, a 672-frame reserve that grows up to 1536. A gap counts as jitter, which
// grows the reserve, while the reserve can still grow to cover it: the ring dry for at most the limit less
// the reserve. A longer one is a stall, concealed. The wait for the reserve to fill again once audio is
// back is not part of the gap.
static void gapsAreJitterOrStalls(void) {
    static AudioOutput output;
    static int16_t ring[4096 * 2], input[4096 * 2], pcm[192 * 2];
    memset(&output, 0, sizeof(output));
    audioQueueInit(&output.queue, ring, 4096, 672);
    output.rate = 48000;
    atomic_store(&output.limitFrames, 1536);
    assert(audioQueueWrite(&output.queue, input, 672) == 672);
    for (int i = 0; i < 3; i++) audioFill(&output, pcm, 192);
    // A packet 2 ms late: the ring runs dry for 96 frames, then refills for three more callbacks.
    audioFill(&output, pcm, 192);
    assert(atomic_load(&output.queue.underruns) == 1 && output.dryRun == 96);
    assert(audioQueueWrite(&output.queue, input, 240) == 240);
    for (int i = 0; i < 3; i++) audioFill(&output, pcm, 192);
    assert(output.dryRun == 96);
    assert(atomic_load(&output.jitterGaps) == 0 && atomic_load(&output.stallGaps) == 0);
    assert(audioQueueWrite(&output.queue, input, 480) == 432);
    audioFill(&output, pcm, 192);
    assert(!output.concealing);
    assert(atomic_load(&output.jitterGaps) == 1 && atomic_load(&output.stallGaps) == 0);

    // Wi-Fi quiet for 44 ms: the reserve lasts 10 ms of it, then the ring is dry for 34 ms.
    for (int i = 0; i < 11; i++) audioFill(&output, pcm, 192);
    assert(output.dryRun == 96 + 8 * 192);
    assert(audioQueueWrite(&output.queue, input, 672) == 672);
    audioFill(&output, pcm, 192);
    assert(atomic_load(&output.jitterGaps) == 1 && atomic_load(&output.stallGaps) == 1);
    assert(atomic_load(&output.queue.underruns) == 2);

    // The boundary at a 672-frame reserve: 864 frames of room. Dry for 18 ms is jitter, a burst more a stall.
    assert(gap(&output, pcm, input, 4) == 96 + 4 * 192);
    assert(atomic_load(&output.jitterGaps) == 2 && atomic_load(&output.stallGaps) == 1);
    assert(gap(&output, pcm, input, 5) == 96 + 5 * 192);
    assert(atomic_load(&output.jitterGaps) == 2 && atomic_load(&output.stallGaps) == 2);
    // Grown to 1344, one burst of room is left: an empty callback is jitter, two are a stall.
    atomic_store(&output.queue.targetFrames, 1344);
    assert(audioQueueWrite(&output.queue, input, 672) == 672);
    assert(gap(&output, pcm, input, 0) == 192);
    assert(atomic_load(&output.jitterGaps) == 3 && atomic_load(&output.stallGaps) == 2);
    assert(gap(&output, pcm, input, 1) == 2 * 192);
    assert(atomic_load(&output.jitterGaps) == 3 && atomic_load(&output.stallGaps) == 3);
    // At the limit, and with no limit set, nothing is left to grow: every gap is a stall.
    atomic_store(&output.queue.targetFrames, 1536);
    assert(audioQueueWrite(&output.queue, input, 192) == 192);
    gap(&output, pcm, input, 0);
    atomic_store(&output.limitFrames, 0);
    atomic_store(&output.queue.targetFrames, 672);
    gap(&output, pcm, input, 0);
    assert(atomic_load(&output.jitterGaps) == 3 && atomic_load(&output.stallGaps) == 5);
    // The JNI setter: a negative limit is no limit.
    jlong handle = (jlong)(intptr_t)&output;
    Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserveLimit(NULL, NULL, handle, 1536);
    assert(atomic_load(&output.limitFrames) == 1536);
    Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserveLimit(NULL, NULL, handle, -1);
    assert(atomic_load(&output.limitFrames) == 0);
}

static void copyStats(JNIEnv* env, jintArray array, jsize start, jsize length, const jint* values) {
    (void)env;
    memcpy((jint*)array + start, values, length * sizeof(jint));
}

int main(void) {
    AudioOutput output = {0};
    int16_t ring[4096 * 2], input[48000 * 2], pcm[192 * 2];
    for (int i = 0; i < 48000 * 2; i++) input[i] = (int16_t)(1 + i % 30000);
    audioQueueInit(&output.queue, ring, 4096, 384);
    output.AAudioStream_getFramesPerBurst = burst;
    output.AAudioStream_getBufferCapacityInFrames = capacity;
    output.AAudioStream_getBufferSizeInFrames = size;
    output.AAudioStream_getXRunCount = xruns;
    output.AAudioStream_getDeviceId = deviceId;
    output.AAudioStream_setBufferSizeInFrames = setSize;
    jlong handle = (jlong)(intptr_t)&output;
    const struct JNINativeInterface functions = {
        .NewIntArray = newStats,
        .SetIntArrayRegion = copyStats,
    };
    JNIEnv env = &functions;

    Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioStats(&env, NULL, handle);
    assert(stats[1] == 4096);
    assert(stats[7] == 42);
    assert(stats[8] == 384);
    assert(stats[9] == 0 && stats[10] == 0);
    assert(stats[11] == 0 && stats[12] == 0);
    // The device buffer and the jitter reserve are sized apart: a larger device buffer only adds latency.
    hardwareFrames = 192;
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioBuffer(NULL, NULL, handle, 768) == 384);
    assert(hardwareFrames == 384);
    assert(atomic_load(&output.queue.targetFrames) == 384);
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserve(NULL, NULL, handle, 768) == 768);
    assert(hardwareFrames == 384);
    assert(atomic_load(&output.queue.targetFrames) == 768);
    // The ring bounds the reserve, and a refused size keeps the current one.
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserve(NULL, NULL, handle, 9000) == 4096);
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserve(NULL, NULL, handle, 0) == 4096);
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserve(NULL, NULL, handle, 768) == 768);
    Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioStats(&env, NULL, handle);
    assert(stats[2] == 768);
    assert(stats[4] == 384);
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioQueued(NULL, NULL, handle) == 0);
    assert(audioQueueWrite(&output.queue, input, 500) == 500);
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioQueued(NULL, NULL, handle) == 500);
    audioQueueRead(&output.queue, pcm, 192);
    assert(Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioQueued(NULL, NULL, handle) == 500);
    audioQueueRead(&output.queue, pcm, 192);
    audioQueueInit(&output.queue, ring, 4096, 768);

    uint32_t offered = 0, written = 0, played = 0;
    bool started = false;
    // Two 5 ms packets arrive together every 10 ms, with alternating 6 ms jitter.
    // The device's 8 ms capacity must not cap the independent network reserve.
    for (int ms = 0; ms < 1000; ms++) {
        if (ms % 20 == 2 || ms % 20 == 18) offered += 480;
        written += audioQueueWrite(&output.queue, input + written * 2, offered - written);
        if (ms % 4 == 0) {
            audioQueueRead(&output.queue, pcm, 192);
            if (started || output.queue.primed) {
                started = true;
                assert(memcmp(pcm, input + played * 2, sizeof(pcm)) == 0);
                played += 192;
            }
        }
    }
    assert(played > 47000);
    assert(atomic_load(&output.queue.underruns) == 0);
    stallsAreConcealedAndTheirPacketsSkipped();
    gapsAreJitterOrStalls();
    puts("AAudio output tests passed");
    return 0;
}
