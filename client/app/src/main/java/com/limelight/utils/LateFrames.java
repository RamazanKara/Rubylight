package com.limelight.utils;

/**
 * Frames that arrived at least {@link #thresholdUs} later than the host's timestamps put them after
 * the frame before, for the stream doctor's BURSTS vote (nightjar-policy's stats::LateFrames). That
 * is a network, or this device's Wi-Fi, holding frames back and handing them over in a burst: the
 * host sent them on time, so a still desktop, a game that slows down or a capture stall, where the
 * timestamps step as far as the arrivals, is never late. Fed by the decoder thread, read by the
 * health sample.
 */
public final class LateFrames {
    /** Timestamp steps longer than this start over (a new stream or a long pause). */
    private static final long MAX_HOST_STEP_US = 1_000_000;

    private long lastPtsUs = -1, lastReceiveUs;
    private int count;

    /**
     * How late a frame must be to count: three quarters of a frame interval, at least 8 ms. At that
     * delay the frame misses its refresh and the one before is shown twice.
     */
    public static long thresholdUs(int fps) {
        return fps <= 0 ? Long.MAX_VALUE : Math.max(750_000L / fps, 8_000L);
    }

    /** A frame arrived (its first packet at {@code receiveUs}) with the host timestamp {@code ptsUs}. */
    public synchronized void push(long ptsUs, long receiveUs, int fps) {
        if (lastPtsUs >= 0) {
            long hostUs = ptsUs - lastPtsUs;
            if (hostUs > 0 && hostUs <= MAX_HOST_STEP_US && receiveUs >= lastReceiveUs &&
                    receiveUs - lastReceiveUs - hostUs >= thresholdUs(fps)) {
                count++;
            }
        }
        lastPtsUs = ptsUs;
        lastReceiveUs = receiveUs;
    }

    /** The late frames since the last tick, then starts the next one. */
    public synchronized int tick() {
        int late = count;
        count = 0;
        return late;
    }
}
