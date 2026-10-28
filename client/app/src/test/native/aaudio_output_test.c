#include "../../main/jni/moonlight-core/aaudio_output.c"
#include <assert.h>
#include <stdio.h>

static int hardwareFrames = 384;
static jint stats[9];

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
    assert(length <= 9);
    return stats;
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
    puts("AAudio output tests passed");
    return 0;
}
