package com.limelight.binding.video;

/**
 * Erratum 23's safety net: with Android's low-latency mode on, the decoders of
 * {@link MediaCodecHelper#corruptsInLowLatency} may keep showing a corrupted picture until the next
 * keyframe, and a stream has none of its own. So one is asked for when none came for
 * {@link #INTERVAL_MS}. Any keyframe restarts the wait; a request starts it again too, so a request
 * that goes unanswered is not repeated on every frame.
 *
 * Only the thread that submits decode units changes it; {@link #requests} may be read from another.
 */
final class KeyframeSafetyNet {
    static final long INTERVAL_MS = 10_000;

    private boolean waiting;
    private long lastMs;
    private volatile int requests;

    /** A keyframe reached the decoder. Before the first one there is nothing to refresh. */
    void onKeyframe(long nowMs) {
        waiting = true;
        lastMs = nowMs;
    }

    /** Whether to ask for a keyframe now; answering true starts the wait again. */
    boolean due(long nowMs) {
        if (!waiting || nowMs - lastMs < INTERVAL_MS) return false;
        lastMs = nowMs;
        requests++;
        return true;
    }

    /** How many keyframes this stream asked for. */
    int requests() {
        return requests;
    }
}
