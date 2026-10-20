#include <aaudio/AAudio.h>
#include <jni.h>
#include <dlfcn.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "aaudio_queue.h"

#define AUDIO_FUNCTIONS(X) \
    X(AAudio_createStreamBuilder) \
    X(AAudioStreamBuilder_delete) \
    X(AAudioStreamBuilder_setDirection) \
    X(AAudioStreamBuilder_setSharingMode) \
    X(AAudioStreamBuilder_setPerformanceMode) \
    X(AAudioStreamBuilder_setFormat) \
    X(AAudioStreamBuilder_setChannelCount) \
    X(AAudioStreamBuilder_setSampleRate) \
    X(AAudioStreamBuilder_setDataCallback) \
    X(AAudioStreamBuilder_setErrorCallback) \
    X(AAudioStreamBuilder_openStream) \
    X(AAudioStream_getSampleRate) \
    X(AAudioStream_getChannelCount) \
    X(AAudioStream_getFormat) \
    X(AAudioStream_getPerformanceMode) \
    X(AAudioStream_getFramesPerBurst) \
    X(AAudioStream_getBufferCapacityInFrames) \
    X(AAudioStream_getBufferSizeInFrames) \
    X(AAudioStream_setBufferSizeInFrames) \
    X(AAudioStream_getXRunCount) \
    X(AAudioStream_requestStart) \
    X(AAudioStream_requestStop) \
    X(AAudioStream_close)

typedef struct {
    void* library;
    AAudioStream* stream;
    aaudio_result_t (*AAudio_createStreamBuilder)(AAudioStreamBuilder**);
    aaudio_result_t (*AAudioStreamBuilder_delete)(AAudioStreamBuilder*);
    void (*AAudioStreamBuilder_setDirection)(AAudioStreamBuilder*, aaudio_direction_t);
    void (*AAudioStreamBuilder_setSharingMode)(AAudioStreamBuilder*, aaudio_sharing_mode_t);
    void (*AAudioStreamBuilder_setPerformanceMode)(AAudioStreamBuilder*, aaudio_performance_mode_t);
    void (*AAudioStreamBuilder_setFormat)(AAudioStreamBuilder*, aaudio_format_t);
    void (*AAudioStreamBuilder_setChannelCount)(AAudioStreamBuilder*, int32_t);
    void (*AAudioStreamBuilder_setSampleRate)(AAudioStreamBuilder*, int32_t);
    void (*AAudioStreamBuilder_setDataCallback)(AAudioStreamBuilder*, AAudioStream_dataCallback, void*);
    void (*AAudioStreamBuilder_setErrorCallback)(AAudioStreamBuilder*, AAudioStream_errorCallback, void*);
    aaudio_result_t (*AAudioStreamBuilder_openStream)(AAudioStreamBuilder*, AAudioStream**);
    int32_t (*AAudioStream_getSampleRate)(AAudioStream*);
    int32_t (*AAudioStream_getChannelCount)(AAudioStream*);
    aaudio_format_t (*AAudioStream_getFormat)(AAudioStream*);
    aaudio_performance_mode_t (*AAudioStream_getPerformanceMode)(AAudioStream*);
    int32_t (*AAudioStream_getFramesPerBurst)(AAudioStream*);
    int32_t (*AAudioStream_getBufferCapacityInFrames)(AAudioStream*);
    int32_t (*AAudioStream_getBufferSizeInFrames)(AAudioStream*);
    aaudio_result_t (*AAudioStream_setBufferSizeInFrames)(AAudioStream*, int32_t);
    int32_t (*AAudioStream_getXRunCount)(AAudioStream*);
    aaudio_result_t (*AAudioStream_requestStart)(AAudioStream*);
    aaudio_result_t (*AAudioStream_requestStop)(AAudioStream*);
    aaudio_result_t (*AAudioStream_close)(AAudioStream*);
    AudioQueue queue;
    atomic_bool stopped;
} AudioOutput;

static aaudio_data_callback_result_t audioData(AAudioStream* stream, void* data, void* buffer, int32_t frames) {
    AudioOutput* output = data;
    if (atomic_load(&output->stopped)) return AAUDIO_CALLBACK_RESULT_STOP;
    audioQueueRead(&output->queue, buffer, (uint32_t)frames);
    return AAUDIO_CALLBACK_RESULT_CONTINUE;
}

static void audioError(AAudioStream* stream, void* data, aaudio_result_t error) {
    // Closing from the AAudio callback can deadlock. The decode thread performs fallback.
    atomic_store(&((AudioOutput*)data)->stopped, true);
}

static void closeAudio(AudioOutput* output) {
    atomic_store(&output->stopped, true);
    if (output->stream) {
        output->AAudioStream_requestStop(output->stream);
        output->AAudioStream_close(output->stream);
    }
    free(output->queue.samples);
    if (output->library) dlclose(output->library);
    free(output);
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeOpenAudio(JNIEnv* env, jclass clazz, jint rate) {
    AudioOutput* output = calloc(1, sizeof(*output));
    if (!output) return 0;
    atomic_init(&output->stopped, false);
    // Loading at runtime keeps API 21-26 installations independent of libaaudio.
    output->library = dlopen("libaaudio.so", RTLD_NOW | RTLD_LOCAL);
    if (!output->library) goto failed;
#define AUDIO_LOAD(name) output->name = dlsym(output->library, #name); if (!output->name) goto failed;
    AUDIO_FUNCTIONS(AUDIO_LOAD)
#undef AUDIO_LOAD
    AAudioStreamBuilder* builder = NULL;
    if (output->AAudio_createStreamBuilder(&builder) != AAUDIO_OK) goto failed;
    output->AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_OUTPUT);
    output->AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
    output->AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_I16);
    output->AAudioStreamBuilder_setChannelCount(builder, 2);
    output->AAudioStreamBuilder_setSampleRate(builder, rate);
    void (*setUsage)(AAudioStreamBuilder*, aaudio_usage_t) = dlsym(output->library, "AAudioStreamBuilder_setUsage");
    if (setUsage) setUsage(builder, AAUDIO_USAGE_GAME);
    output->AAudioStreamBuilder_setDataCallback(builder, audioData, output);
    output->AAudioStreamBuilder_setErrorCallback(builder, audioError, output);
    for (int attempt = 0; attempt < 2; attempt++) {
        output->AAudioStreamBuilder_setSharingMode(builder, attempt == 0 ? AAUDIO_SHARING_MODE_EXCLUSIVE : AAUDIO_SHARING_MODE_SHARED);
        if (output->AAudioStreamBuilder_openStream(builder, &output->stream) != AAUDIO_OK) {
            output->stream = NULL;
            continue;
        }
        if (output->AAudioStream_getSampleRate(output->stream) == rate &&
                output->AAudioStream_getChannelCount(output->stream) == 2 &&
                output->AAudioStream_getFormat(output->stream) == AAUDIO_FORMAT_PCM_I16 &&
                output->AAudioStream_getPerformanceMode(output->stream) == AAUDIO_PERFORMANCE_MODE_LOW_LATENCY) break;
        output->AAudioStream_close(output->stream);
        output->stream = NULL;
    }
    output->AAudioStreamBuilder_delete(builder);
    if (!output->stream) goto failed;
    int burst = output->AAudioStream_getFramesPerBurst(output->stream);
    int capacity = output->AAudioStream_getBufferCapacityInFrames(output->stream);
    if (burst <= 0 || burst > rate / 10 || capacity < burst) goto failed;
    // A power-of-two ring also preserves the index when the 32-bit counters wrap.
    uint32_t ringCapacity = 1;
    while (ringCapacity < (uint32_t)burst * 16) ringCapacity *= 2;
    output->queue.samples = calloc(ringCapacity * 2, sizeof(int16_t));
    if (!output->queue.samples) goto failed;
    int target = output->AAudioStream_setBufferSizeInFrames(output->stream, burst * 2);
    if (target <= 0) goto failed;
    audioQueueInit(&output->queue, output->queue.samples, ringCapacity, (uint32_t)burst * 2);
    if (output->AAudioStream_requestStart(output->stream) != AAUDIO_OK) goto failed;
    return (jlong)(intptr_t)output;
failed:
    closeAudio(output);
    return 0;
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeWriteAudio(JNIEnv* env, jclass clazz,
        jlong handle, jshortArray samples, jint length) {
    AudioOutput* output = (AudioOutput*)(intptr_t)handle;
    if (length < 0 || length > (*env)->GetArrayLength(env, samples) || length % 2) return JNI_FALSE;
    jshort* pcm = (*env)->GetShortArrayElements(env, samples, NULL);
    if (!pcm) return JNI_FALSE;
    uint32_t remaining = (uint32_t)length / 2;
    uint32_t position = 0;
    int waits = 0;
    while (remaining && !atomic_load(&output->stopped)) {
        uint32_t count = audioQueueWrite(&output->queue, pcm + position * 2, remaining);
        if (!count) {
            if (++waits >= 100) break;
            struct timespec pause = {0, 1000000};
            nanosleep(&pause, NULL);
            continue;
        }
        remaining -= count;
        position += count;
    }
    (*env)->ReleaseShortArrayElements(env, samples, pcm, JNI_ABORT);
    return remaining == 0 && !atomic_load(&output->stopped);
}

JNIEXPORT jintArray JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioStats(JNIEnv* env, jclass clazz, jlong handle) {
    AudioOutput* output = (AudioOutput*)(intptr_t)handle;
    int xruns = output->AAudioStream_getXRunCount(output->stream);
    int underruns = (int)atomic_load(&output->queue.underruns);
    // The software starvation count is still valid if hardware xruns are unsupported.
    jint stats[] = {output->AAudioStream_getFramesPerBurst(output->stream),
            (jint)output->queue.capacity,
            (jint)atomic_load(&output->queue.targetFrames),
            (xruns < 0 ? 0 : xruns) + underruns,
            output->AAudioStream_getBufferSizeInFrames(output->stream), xruns, underruns};
    jintArray result = (*env)->NewIntArray(env, 7);
    if (result) (*env)->SetIntArrayRegion(env, result, 0, 7, stats);
    return result;
}

JNIEXPORT jint JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioBuffer(JNIEnv* env, jclass clazz, jlong handle, jint frames) {
    AudioOutput* output = (AudioOutput*)(intptr_t)handle;
    output->AAudioStream_setBufferSizeInFrames(output->stream, frames);
    // Hardware capacity bounds the device queue, not the reserve for network jitter.
    uint32_t target = (uint32_t)frames < output->queue.capacity ? (uint32_t)frames : output->queue.capacity;
    atomic_store(&output->queue.targetFrames, target);
    return (jint)target;
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeCloseAudio(JNIEnv* env, jclass clazz, jlong handle) {
    closeAudio((AudioOutput*)(intptr_t)handle);
}
