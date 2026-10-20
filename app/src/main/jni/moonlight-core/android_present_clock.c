// Timestamp presentation (protocol/core/src/present_clock.rs): the renderer asks, per decoded
// frame, when to show it so the screen keeps the host's frame times; clock sync replies (0x5537)
// arrive on the control thread and add the host-to-screen latency. All times are
// CLOCK_MONOTONIC, the clock of System.nanoTime().
#include <jni.h>
#include <pthread.h>
#include <time.h>
#include "rubylight_protocol.h"
#include "clock_sync.h"

static pthread_mutex_t presentClockMutex = PTHREAD_MUTEX_INITIALIZER;
static RpPresentClock* presentClock;

static uint64_t monotonicMicros(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000000 + (uint64_t)now.tv_nsec / 1000;
}

void BridgeClClockSync(const uint8_t* payload, int length) {
    uint64_t receivedUs = monotonicMicros();
    pthread_mutex_lock(&presentClockMutex);
    if (presentClock) rp_present_clock_sync(presentClock, payload, (size_t)length, receivedUs);
    pthread_mutex_unlock(&presentClockMutex);
}

// Starts a clock for a new stream; minIntervalUs is the display's shortest refresh interval.
JNIEXPORT void JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cPresentClockStart(JNIEnv* env, jclass clazz, jint minIntervalUs) {
    (void)env; (void)clazz;
    RpPresentSettings settings;
    rp_present_settings_default(&settings);
    settings.min_interval_us = minIntervalUs > 0 ? minIntervalUs : 0;
    RpPresentClock* created = rp_present_clock_new(&settings);
    pthread_mutex_lock(&presentClockMutex);
    RpPresentClock* old = presentClock;
    presentClock = created;
    pthread_mutex_unlock(&presentClockMutex);
    if (old) rp_present_clock_free(old);
}

JNIEXPORT void JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cPresentClockStop(JNIEnv* env, jclass clazz) {
    (void)env; (void)clazz;
    pthread_mutex_lock(&presentClockMutex);
    RpPresentClock* old = presentClock;
    presentClock = NULL;
    pthread_mutex_unlock(&presentClockMutex);
    if (old) rp_present_clock_free(old);
}

// Forgets the frames seen (a decoder restart or a new stream size); clock sync stays.
JNIEXPORT void JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cPresentClockReset(JNIEnv* env, jclass clazz) {
    (void)env; (void)clazz;
    pthread_mutex_lock(&presentClockMutex);
    if (presentClock) rp_present_clock_reset(presentClock);
    pthread_mutex_unlock(&presentClockMutex);
}

// Schedules the frame whose host timestamp is hostUs (moonlight-common-c's presentationTimeUs,
// the RTP timestamp in microseconds) and that was ready at readyNs. Writes present time (ns),
// hold (us), host-to-present latency (us, -1 without clock sync) and late (0/1) to out[0..3].
JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cPresentClockSchedule(JNIEnv* env, jclass clazz, jlong hostUs,
                                                               jlong readyNs, jlongArray out) {
    (void)clazz;
    if (hostUs < 0 || readyNs <= 0 || (*env)->GetArrayLength(env, out) < 4) return JNI_FALSE;
    // presentationTimeUs = floor(rtp * 1000 / 90), so the RTP timestamp is its ceiling back.
    uint32_t rtp = (uint32_t)(((uint64_t)hostUs * 9 + 99) / 100);
    pthread_mutex_lock(&presentClockMutex);
    if (!presentClock) {
        pthread_mutex_unlock(&presentClockMutex);
        return JNI_FALSE;
    }
    RpPresentSchedule s = rp_present_clock_schedule(presentClock, rtp, (uint64_t)readyNs / 1000);
    pthread_mutex_unlock(&presentClockMutex);
    // Keep the ready time's sub-microsecond part so a frame that is on time is not shown early.
    jlong presentNs = s.present_us == (uint64_t)readyNs / 1000 ? readyNs : (jlong)s.present_us * 1000;
    jlong values[4] = { presentNs, s.hold_us, s.latency_us, s.late ? 1 : 0 };
    (*env)->SetLongArrayRegion(env, out, 0, 4, values);
    return JNI_TRUE;
}

// Frames scheduled, frames late and the clock sync round trip (us, -1 without) to out[0..2].
JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_cPresentClockStats(JNIEnv* env, jclass clazz, jlongArray out) {
    (void)clazz;
    if ((*env)->GetArrayLength(env, out) < 3) return JNI_FALSE;
    uint64_t frames = 0, late = 0;
    int64_t rtt = -1;
    pthread_mutex_lock(&presentClockMutex);
    bool running = presentClock != NULL;
    if (running) rp_present_clock_stats(presentClock, &frames, &late, &rtt);
    pthread_mutex_unlock(&presentClockMutex);
    if (!running) return JNI_FALSE;
    jlong values[3] = { (jlong)frames, (jlong)late, rtt };
    (*env)->SetLongArrayRegion(env, out, 0, 3, values);
    return JNI_TRUE;
}
