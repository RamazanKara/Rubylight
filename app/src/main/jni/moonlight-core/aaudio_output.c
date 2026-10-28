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

// The longest stall Opus conceals; after it, silence. Also the most late frames ever skipped.
#define MAX_CONCEAL_MS 200
// One concealment call at most: 120 ms at 48 kHz, the longest an Opus decode may be.
#define CONCEAL_MAX_FRAMES 5760
#define FADE_MAX_FRAMES 480

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
    X(AAudioStream_getDeviceId) \
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
    int32_t (*AAudioStream_getDeviceId)(AAudioStream*);
    aaudio_result_t (*AAudioStream_requestStart)(AAudioStream*);
    aaudio_result_t (*AAudioStream_requestStop)(AAudioStream*);
    aaudio_result_t (*AAudioStream_close)(AAudioStream*);
    AudioQueue queue;
    atomic_bool stopped;
    uint32_t rate;
    // The stream's Opus concealer (callbacks.c or nightjar-core), or NULL: silence fills a stall then.
    int (*conceal)(int16_t* pcm, int frames);
    // Callback thread only: concealment not played yet, and how long the current stall has run.
    int16_t concealPcm[CONCEAL_MAX_FRAMES * 2];
    uint32_t concealStart, concealCount, concealRun;
    bool played, concealing;
    // Decode thread only: when the last packet came.
    uint64_t lastPacketNs;
    // Stalled frames the concealment covered, whose packets the decode thread skips when they come.
    atomic_uint lateFrames, concealedFrames, skippedFrames;
} AudioOutput;

// Up to frames frames of Opus concealment into pcm, a multiple of 2.5 ms at a time; returns the frames
// filled. The decoder is called only while the ring is empty: once stalled audio is coming back in, a
// concealment between its packets would bend their decoding.
static uint32_t audioConceal(AudioOutput* output, int16_t* pcm, uint32_t frames) {
    uint32_t done = 0, unit = output->rate / 400;
    while (done < frames) {
        if (!output->concealCount) {
            uint32_t queued = atomic_load(&output->queue.writtenFrames) - atomic_load(&output->queue.readFrames);
            if (!output->conceal || !unit || queued) break;
            uint32_t want = (frames - done + unit - 1) / unit * unit;
            if (want > CONCEAL_MAX_FRAMES) want = CONCEAL_MAX_FRAMES / unit * unit;
            int got = output->conceal(output->concealPcm, (int)want);
            if (got <= 0) break;
            output->concealStart = 0;
            output->concealCount = (uint32_t)got < want ? (uint32_t)got : want;
        }
        uint32_t count = output->concealCount < frames - done ? output->concealCount : frames - done;
        memcpy(pcm + done * 2, output->concealPcm + output->concealStart * 2, count * 2 * sizeof(int16_t));
        output->concealStart += count;
        output->concealCount -= count;
        done += count;
    }
    return done;
}

// Plays the ring. Where it runs dry after audio has played, Opus conceals the gap for up to
// MAX_CONCEAL_MS (silence after that), and the gap becomes late frames: the stalled packets that
// cover them are skipped when they come, so latency returns to the reserve instead of growing.
// When the ring plays again, its audio fades in from the concealment over 5 ms.
static void audioFill(AudioOutput* output, int16_t* pcm, uint32_t frames) {
    uint32_t played = audioQueueRead(&output->queue, pcm, frames);
    uint32_t limit = output->rate / 1000 * MAX_CONCEAL_MS;
    if (played) {
        if (output->concealing) {
            int16_t tail[FADE_MAX_FRAMES * 2] = {0};
            uint32_t fade = output->rate / 200;
            if (fade > FADE_MAX_FRAMES) fade = FADE_MAX_FRAMES;
            if (fade > played) fade = played;
            // Only what is left of the last concealment: the decoder is busy with the stalled packets.
            uint32_t left = output->concealCount < fade ? output->concealCount : fade;
            memcpy(tail, output->concealPcm + output->concealStart * 2, left * 2 * sizeof(int16_t));
            for (uint32_t i = 0; i < fade * 2; i++) {
                int32_t step = (int32_t)(i / 2);
                pcm[i] = (int16_t)((pcm[i] * step + tail[i] * ((int32_t)fade - step)) / (int32_t)fade);
            }
            output->concealing = false;
            output->concealRun = output->concealCount = 0;
            atomic_store(&output->lateFrames, 0);
        }
        output->played = true;
    }
    if (played == frames || !output->played) return;
    uint32_t missing = frames - played;
    output->concealing = true;
    if (output->concealRun < limit) {
        uint32_t room = limit - output->concealRun;
        uint32_t concealed = audioConceal(output, pcm + played * 2, missing < room ? missing : room);
        atomic_fetch_add_explicit(&output->concealedFrames, concealed, memory_order_relaxed);
    }
    output->concealRun += missing;
    uint32_t late = atomic_load(&output->lateFrames), next;
    do {
        next = late + missing < limit ? late + missing : limit;
    } while (!atomic_compare_exchange_weak(&output->lateFrames, &late, next));
}

// Whether the decode thread skips this packet: the concealment already played its time, and it came
// right behind the one before (under half its duration), as a stall's packets do once the network
// is back. Audio that comes at its own pace, as after the host paused, is never skipped.
static bool audioSkipLate(AudioOutput* output, uint64_t nowNs, uint32_t frames) {
    uint64_t gap = nowNs - output->lastPacketNs;
    output->lastPacketNs = nowNs;
    if (!frames || !output->rate || gap >= (uint64_t)frames * 500000000u / output->rate) return false;
    uint32_t late = atomic_load(&output->lateFrames);
    while (late >= frames) {
        if (atomic_compare_exchange_weak(&output->lateFrames, &late, late - frames)) {
            atomic_fetch_add_explicit(&output->skippedFrames, frames, memory_order_relaxed);
            return true;
        }
    }
    return false;
}

static aaudio_data_callback_result_t audioData(AAudioStream* stream, void* data, void* buffer, int32_t frames) {
    AudioOutput* output = data;
    if (atomic_load(&output->stopped)) return AAUDIO_CALLBACK_RESULT_STOP;
    audioFill(output, buffer, (uint32_t)frames);
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
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeOpenAudio(JNIEnv* env, jclass clazz, jint rate,
        jlong concealer) {
    AudioOutput* output = calloc(1, sizeof(*output));
    if (!output) return 0;
    atomic_init(&output->stopped, false);
    atomic_init(&output->lateFrames, 0);
    atomic_init(&output->concealedFrames, 0);
    atomic_init(&output->skippedFrames, 0);
    output->rate = (uint32_t)rate;
    output->conceal = (int (*)(int16_t*, int))(intptr_t)concealer;
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
            output->AAudioStream_getBufferSizeInFrames(output->stream), xruns, underruns,
            output->AAudioStream_getDeviceId(output->stream),
            output->AAudioStream_getBufferCapacityInFrames(output->stream),
            (jint)atomic_load(&output->concealedFrames), (jint)atomic_load(&output->skippedFrames)};
    jintArray result = (*env)->NewIntArray(env, 11);
    if (result) (*env)->SetIntArrayRegion(env, result, 0, 11, stats);
    return result;
}

// The device buffer, sized for hardware xruns only: the callback keeps it full, with silence if it must,
// so a larger one adds latency without covering a network stall.
JNIEXPORT jint JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioBuffer(JNIEnv* env, jclass clazz, jlong handle, jint frames) {
    AudioOutput* output = (AudioOutput*)(intptr_t)handle;
    int32_t granted = output->AAudioStream_setBufferSizeInFrames(output->stream, frames);
    return granted > 0 ? granted : output->AAudioStream_getBufferSizeInFrames(output->stream);
}

// The jitter reserve in the ring, sized for its own starvation. Hardware capacity does not bound it.
JNIEXPORT jint JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioReserve(JNIEnv* env, jclass clazz, jlong handle, jint frames) {
    AudioOutput* output = (AudioOutput*)(intptr_t)handle;
    if (frames <= 0) return (jint)atomic_load(&output->queue.targetFrames);
    uint32_t target = (uint32_t)frames < output->queue.capacity ? (uint32_t)frames : output->queue.capacity;
    atomic_store(&output->queue.targetFrames, target);
    return (jint)target;
}

// Frames in the ring that the callback has not taken yet; the decode thread's per-packet drop check.
JNIEXPORT jint JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioQueued(JNIEnv* env, jclass clazz, jlong handle) {
    AudioOutput* output = (AudioOutput*)(intptr_t)handle;
    return (jint)(atomic_load(&output->queue.writtenFrames) - atomic_load(&output->queue.readFrames));
}

// The decode thread, before writing a packet of frames frames: true to skip it as late (audioSkipLate).
JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeAudioSkipLate(JNIEnv* env, jclass clazz, jlong handle,
        jint frames) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return audioSkipLate((AudioOutput*)(intptr_t)handle, (uint64_t)now.tv_sec * 1000000000u + (uint64_t)now.tv_nsec,
                         frames > 0 ? (uint32_t)frames : 0) ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_audio_AndroidAudioRenderer_nativeCloseAudio(JNIEnv* env, jclass clazz, jlong handle) {
    closeAudio((AudioOutput*)(intptr_t)handle);
}
