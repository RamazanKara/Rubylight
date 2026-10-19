#include <jni.h>

// Keep the pinned core intact while sharing its encryption, sequence and ENet mutex.
#include "moonlight-common-c/src/ControlStream.c"

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_sendServerCommand(JNIEnv *env, jclass clazz, jbyteArray payload) {
    jsize length = (*env)->GetArrayLength(env, payload);
    uint8_t bytes[4];
    if ((length != 1 && length != 4) || !IS_SUNSHINE() || !encryptedControlStream || stopping || peer == NULL) {
        return JNI_FALSE;
    }
    (*env)->GetByteArrayRegion(env, payload, 0, length, (jbyte*)bytes);
    if ((*env)->ExceptionCheck(env)) {
        return JNI_FALSE;
    }
    // Apollo's server-control channel is 8; older hosts use the core's channel-zero fallback.
    return sendMessageAndForget(0x3000, (short)length, bytes, 0x08, ENET_PACKET_FLAG_RELIABLE, false)
        ? JNI_TRUE : JNI_FALSE;
}

// Phase lock (protocol/core/src/phase_lock.rs): every half second, tell a Rubylight host how
// early frames were ready before this display's latch, so it can time frames to land just
// before it. Only hosts that announce the message in a=x-rl-control receive it.
#include <pthread.h>
#include "rubylight_protocol.h"
#include "phase_lock.h"
#include "control_negotiation.h"

static pthread_mutex_t phaseLockMutex = PTHREAD_MUTEX_INITIALIZER;
// Only background flush/reset take this mutex. Sampling must never wait for ENet.
static pthread_mutex_t phaseDispatchMutex = PTHREAD_MUTEX_INITIALIZER;
static RpSlackWindow* phaseLockWindow;
static uint64_t phaseLockLastReportMs;
static atomic_int phaseLockLastLeadUs = ATOMIC_VAR_INIT(INT32_MIN);
static uint8_t phaseLockPendingReport[RP_PHASE_REPORT_BYTES];
static uint64_t phaseLockPendingAtMs;
static bool phaseLockReportPending;

#define PHASE_LOCK_REPORT_INTERVAL_MS 500
#define PHASE_LOCK_MIN_FRAMES 10
#define PHASE_LOCK_MAX_REPORT_AGE_MS 1000

void resetPhaseLockReports(void) {
    // Wait for an old stream's flush before clearing its mailbox. The renderer's
    // latency handler is joined before control teardown and a new stream starts.
    pthread_mutex_lock(&phaseDispatchMutex);
    pthread_mutex_lock(&phaseLockMutex);
    if (phaseLockWindow) rp_slack_window_free(phaseLockWindow);
    phaseLockWindow = NULL;
    phaseLockLastReportMs = 0;
    phaseLockReportPending = false;
    phaseLockPendingAtMs = 0;
    atomic_store(&phaseLockLastLeadUs, INT32_MIN);
    pthread_mutex_unlock(&phaseLockMutex);
    pthread_mutex_unlock(&phaseDispatchMutex);
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_reportFrameSlack(JNIEnv *env, jclass clazz, jlong slackNs, jlong periodNs) {
    (void)env; (void)clazz;
    if (!hostSupportsControlMessage(RP_PHASE_REPORT_MESSAGE_TYPE)) return JNI_FALSE;
    bool ready = false;
    pthread_mutex_lock(&phaseLockMutex);
    if (!phaseLockWindow) phaseLockWindow = rp_slack_window_new();
    if (phaseLockWindow) {
        uint64_t now = PltGetMillis();
        if (phaseLockLastReportMs == 0) phaseLockLastReportMs = now;
        rp_slack_window_push(phaseLockWindow, slackNs, periodNs);
        if (now - phaseLockLastReportMs >= PHASE_LOCK_REPORT_INTERVAL_MS) {
            phaseLockLastReportMs = now;
            ready = rp_slack_window_report(phaseLockWindow, PHASE_LOCK_MIN_FRAMES, phaseLockPendingReport);
            if (ready) {
                phaseLockPendingAtMs = now;
                phaseLockReportPending = true;
                int32_t leadNs = (int32_t)((uint32_t)phaseLockPendingReport[8] |
                        ((uint32_t)phaseLockPendingReport[9] << 8) |
                        ((uint32_t)phaseLockPendingReport[10] << 16) |
                        ((uint32_t)phaseLockPendingReport[11] << 24));
                atomic_store(&phaseLockLastLeadUs, leadNs / 1000);
            }
        }
    }
    pthread_mutex_unlock(&phaseLockMutex);
    return ready ? JNI_TRUE : JNI_FALSE;
}

// Called only on the renderer's background handler. Keep one latest report, not a
// queue of old timing feedback, and never hold the sampling mutex while sending.
JNIEXPORT void JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_flushPhaseLockReport(JNIEnv *env, jclass clazz) {
    (void)env; (void)clazz;
    uint8_t report[RP_PHASE_REPORT_BYTES];
    bool send = false;
    pthread_mutex_lock(&phaseDispatchMutex);
    pthread_mutex_lock(&phaseLockMutex);
    uint64_t now = PltGetMillis();
    if (phaseLockReportPending && now >= phaseLockPendingAtMs &&
            now - phaseLockPendingAtMs <= PHASE_LOCK_MAX_REPORT_AGE_MS) {
        memcpy(report, phaseLockPendingReport, sizeof(report));
        send = true;
    }
    phaseLockReportPending = false;
    pthread_mutex_unlock(&phaseLockMutex);
    if (send && hostSupportsControlMessage(RP_PHASE_REPORT_MESSAGE_TYPE) &&
            encryptedControlStream && !stopping && peer != NULL) {
        // A lost report is replaced half a second later. The shared ENet mutex
        // can still wait for another control operation, away from video output.
        sendMessageAndForget(RP_PHASE_REPORT_MESSAGE_TYPE, (short)sizeof(report), report,
                             CTRL_CHANNEL_GENERIC, 0, false);
    }
    pthread_mutex_unlock(&phaseDispatchMutex);
}

// The early edge of the last reported window, in microseconds before the latch (negative when
// late), or Integer.MIN_VALUE before the first report.
JNIEXPORT jint JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_getPhaseLockLeadUs(JNIEnv *env, jclass clazz) {
    (void)env; (void)clazz;
    return atomic_load(&phaseLockLastLeadUs);
}

// Control messages the host named in its DESCRIBE answer (a=x-rl-control, Rubylight 2.2.0+).
// Written on the connection thread before the control stream starts, read from Java threads.
#include "control_negotiation.h"

static uint16_t hostControlMessages[RL_MAX_CONTROL_MESSAGES];
static atomic_int hostControlMessageCount;

void resetHostControlMessages(void) {
    atomic_store(&hostControlMessageCount, 0);
}

void setHostControlMessages(const uint16_t* ids, int count) {
    atomic_store(&hostControlMessageCount, 0);
    if (count > RL_MAX_CONTROL_MESSAGES) count = RL_MAX_CONTROL_MESSAGES;
    if (count < 0) count = 0;
    memcpy(hostControlMessages, ids, (size_t)count * sizeof(ids[0]));
    atomic_store(&hostControlMessageCount, count);
}

bool hostSupportsControlMessage(uint16_t messageType) {
    int count = atomic_load(&hostControlMessageCount);
    for (int i = 0; i < count; i++) {
        if (hostControlMessages[i] == messageType) return true;
    }
    return false;
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_hostSupportsControlMessage(JNIEnv *env, jclass clazz, jint messageType) {
    (void)env; (void)clazz;
    if (messageType < 0 || messageType > 0xFFFF) return JNI_FALSE;
    return hostSupportsControlMessage((uint16_t)messageType) ? JNI_TRUE : JNI_FALSE;
}

// Display HDR luminance (protocol/core/src/control.rs, DisplayCaps): sent at stream start and
// when the display changes, only to hosts that announced 0x5531. The Java side sends it again
// only when a value changes, so it goes reliably.
JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_sendDisplayCaps(JNIEnv *env, jclass clazz, jboolean hdr,
                                                          jfloat maxNits, jfloat maxAverageNits, jfloat minNits) {
    (void)env; (void)clazz;
    if (!hostSupportsControlMessage(RP_DISPLAY_CAPS_MESSAGE_TYPE) ||
        !encryptedControlStream || stopping || peer == NULL) {
        return JNI_FALSE;
    }
    uint8_t message[RP_DISPLAY_CAPS_BYTES];
    rp_display_caps_encode(hdr == JNI_TRUE, maxNits, maxAverageNits, minNits, message);
    return sendMessageAndForget(RP_DISPLAY_CAPS_MESSAGE_TYPE, (short)sizeof(message), message,
                                CTRL_CHANNEL_GENERIC, ENET_PACKET_FLAG_RELIABLE, false)
        ? JNI_TRUE : JNI_FALSE;
}

// Mid-stream reconfigure (protocol/core/src/control.rs): ask a Rubylight 2.2 host for a new
// resolution or frame rate without reconnecting; it answers with an IDR at the new size. Sent
// reliably, since a lost request would leave the stream at the old size. The caller checks that
// the host advertised the message and checks again here; the host validates the values too.
JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_requestReconfigure(JNIEnv *env, jclass clazz,
                                                             jint width, jint height, jint fpsMillihz) {
    (void)env; (void)clazz;
    if (width <= 0 || width > UINT16_MAX || height <= 0 || height > UINT16_MAX || fpsMillihz <= 0) {
        return JNI_FALSE;
    }
    if (!hostSupportsControlMessage(RP_RECONFIGURE_MESSAGE_TYPE) ||
        !encryptedControlStream || stopping || peer == NULL) {
        return JNI_FALSE;
    }
    uint8_t request[RP_RECONFIGURE_BYTES];
    rp_reconfigure_encode((uint16_t)width, (uint16_t)height, (uint32_t)fpsMillihz, request);
    return sendMessageAndForget(RP_RECONFIGURE_MESSAGE_TYPE, (short)sizeof(request), request,
                                CTRL_CHANNEL_GENERIC, ENET_PACKET_FLAG_RELIABLE, false)
        ? JNI_TRUE : JNI_FALSE;
}

// Speaker/microphone control uses the authenticated stream and existing ENet sequencing.
JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_sendControllerAudio(JNIEnv* env, jclass clazz, jint kind, jbyteArray payload) {
    (void)clazz;
    if ((kind != 0x5534 && kind != 0x5535) || !hostSupportsControlMessage((uint16_t)kind) ||
        !encryptedControlStream || stopping || peer == NULL || !payload) return JNI_FALSE;
    // A microphone packet is at most ML_HAPTICS_MAX_PAYLOAD (972) bytes; reject anything larger
    // before it reaches the stack buffer or the control stream.
    jsize length = (*env)->GetArrayLength(env, payload);
    if (length < 8 || length > ML_HAPTICS_MAX_PAYLOAD) return JNI_FALSE;
    uint8_t bytes[ML_HAPTICS_MAX_PAYLOAD];
    (*env)->GetByteArrayRegion(env, payload, 0, length, (jbyte*)bytes);
    if ((*env)->ExceptionCheck(env)) return JNI_FALSE;
    if (kind == 0x5534 && !MlHapticsValidate(bytes, length)) return JNI_FALSE;
    if (kind == 0x5535 && (length != 8 || bytes[0] != 1 || bytes[1] != 1 || MlHapticsRead16(bytes+2) >= 16 || (bytes[4] & ~15) || bytes[5] || bytes[6] || bytes[7])) return JNI_FALSE;
    return sendMessageAndForget((short)kind, (short)length, bytes, 8,
        kind == 0x5535 ? ENET_PACKET_FLAG_RELIABLE : ENET_PACKET_FLAG_UNSEQUENCED, false) ? JNI_TRUE : JNI_FALSE;
}

// Host timing (protocol/core/src/control.rs, HostTimingRequest): asks a host that announced 0x5536
// to report, per new frame, how long it held the picture from present to its last packet sent.
// Sent once per connection (and again to switch it off); reliable, since a lost request would
// leave the stream without reports.
#define HOST_TIMING_MESSAGE_TYPE 0x5536
#define HOST_TIMING_REQUEST_BYTES 4

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_sendHostTimingRequest(JNIEnv *env, jclass clazz, jboolean enabled) {
    (void)env; (void)clazz;
    if (!hostSupportsControlMessage(HOST_TIMING_MESSAGE_TYPE) ||
        !encryptedControlStream || stopping || peer == NULL) {
        return JNI_FALSE;
    }
    uint8_t request[HOST_TIMING_REQUEST_BYTES] = { 1, enabled == JNI_TRUE ? 1 : 0, 0, 0 };
    return sendMessageAndForget(HOST_TIMING_MESSAGE_TYPE, (short)sizeof(request), request,
                                CTRL_CHANNEL_GENERIC, ENET_PACKET_FLAG_RELIABLE, false)
        ? JNI_TRUE : JNI_FALSE;
}

// Clock sync probe (protocol/core/src/control.rs, ClockSyncRequest): the host answers at once
// with its video RTP clock (see android_present_clock.c). Unsequenced: a lost probe is one sample.
#include <time.h>
#include "clock_sync.h"

static uint32_t clockSyncSequence;

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_sendClockSyncProbe(JNIEnv *env, jclass clazz) {
    (void)env; (void)clazz;
    if (!hostSupportsControlMessage(RL_CLOCK_SYNC_MESSAGE_TYPE) ||
        !encryptedControlStream || stopping || peer == NULL) {
        return JNI_FALSE;
    }
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    uint8_t probe[RP_CLOCK_SYNC_REQUEST_BYTES];
    rp_clock_sync_request_encode(++clockSyncSequence,
                                 (uint64_t)now.tv_sec * 1000000 + (uint64_t)now.tv_nsec / 1000, probe);
    return sendMessageAndForget(RL_CLOCK_SYNC_MESSAGE_TYPE, (short)sizeof(probe), probe,
                                CTRL_CHANNEL_GENERIC, ENET_PACKET_FLAG_UNSEQUENCED, false)
        ? JNI_TRUE : JNI_FALSE;
}

// Host cursor shapes (docs/protocol/cursor-shape.md): asks a host that named 0x5540 in its
// DESCRIBE answer to send its cursor image as 0x5541 and leave it out of the video, or to stop.
#include "cursor_shape.h"

static atomic_bool cursorShapesOn;

void resetCursorShapes(void) {
    atomic_store(&cursorShapesOn, false);
}

bool cursorShapesRequested(void) {
    return atomic_load(&cursorShapesOn);
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_sendCursorShapeRequest(JNIEnv *env, jclass clazz, jboolean enabled) {
    (void)env; (void)clazz;
    if (!hostSupportsControlMessage(RL_CURSOR_SHAPE_REQUEST_TYPE) ||
        !encryptedControlStream || stopping || peer == NULL) {
        return JNI_FALSE;
    }
    uint8_t request[RL_CURSOR_SHAPE_REQUEST_BYTES] = {
        RL_CURSOR_SHAPE_VERSION, enabled ? 1 : 0,
        (uint8_t)(RL_CURSOR_SHAPE_MAX_EDGE & 0xFF), (uint8_t)(RL_CURSOR_SHAPE_MAX_EDGE >> 8)
    };
    // Shapes are accepted from just before an enable goes out, so the host's first answer is not
    // dropped, and refused from just before a disable, so none arrives after Java let go.
    atomic_store(&cursorShapesOn, enabled ? true : false);
    bool sent = sendMessageAndForget(RL_CURSOR_SHAPE_REQUEST_TYPE, (short)sizeof(request), request,
                                     CTRL_CHANNEL_GENERIC, ENET_PACKET_FLAG_RELIABLE, false);
    if (!sent) atomic_store(&cursorShapesOn, false);
    return sent ? JNI_TRUE : JNI_FALSE;
}
