package com.limelight.binding.video;

/**
 * Network queueing delay from frame arrival against the host's capture timestamp.
 * The two clocks differ by an unknown offset, so the delay is the one-way delay
 * above its minimum over the last ten one-second windows. A queue building at
 * the access point raises it before packets are lost.
 */
public final class QueueDelayTracker {
    private static final int WINDOWS = 10;

    private final long[] windowMinUs = new long[WINDOWS];
    private int windows;
    private int next;
    private long currentMinUs = Long.MAX_VALUE;
    private long excessTotalUs;
    private int samples;

    /** One frame: client arrival of its first packet and the host's presentation time, both in microseconds. */
    public void onFrame(long receiveTimeUs, long presentationTimeUs) {
        if (receiveTimeUs <= 0 || presentationTimeUs <= 0) {
            return;
        }
        long delayUs = receiveTimeUs - presentationTimeUs;
        currentMinUs = Math.min(currentMinUs, delayUs);
        long baseUs = currentMinUs;
        for (int i = 0; i < windows; i++) {
            baseUs = Math.min(baseUs, windowMinUs[i]);
        }
        excessTotalUs += delayUs - baseUs;
        samples++;
    }

    /** Ends a window; returns its mean queueing delay in milliseconds, or -1 without frames. */
    public float takeWindowMs() {
        float result = samples == 0 ? -1 : excessTotalUs / 1000.0f / samples;
        if (samples != 0) {
            windowMinUs[next] = currentMinUs;
            next = (next + 1) % WINDOWS;
            windows = Math.min(WINDOWS, windows + 1);
        }
        currentMinUs = Long.MAX_VALUE;
        excessTotalUs = 0;
        samples = 0;
        return result;
    }

    /** A new stream or a host clock reset (keyframe after reconnect) starts over. */
    public void reset() {
        windows = next = samples = 0;
        currentMinUs = Long.MAX_VALUE;
        excessTotalUs = 0;
    }
}
