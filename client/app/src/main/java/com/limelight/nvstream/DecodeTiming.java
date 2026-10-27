package com.limelight.nvstream;

/** Immutable observations from one bounded, non-overlapping decoder sampling interval. */
public final class DecodeTiming {
    /** Input-to-output latency includes pipeline depth, not just work on a single frame. */
    public static double decodeBudgetMs(int fps) {
        return 2000d / Math.max(1, fps);
    }

    public final long generation, windowStartNs, windowEndNs, firstSampleNs, lastSampleNs;
    public final int fps, samples, lateFrames;

    public DecodeTiming(long generation, long windowStartNs, long windowEndNs,
                        long firstSampleNs, long lastSampleNs, int fps, int samples, int lateFrames) {
        this.generation = generation;
        this.windowStartNs = windowStartNs;
        this.windowEndNs = windowEndNs;
        this.firstSampleNs = firstSampleNs;
        this.lastSampleNs = lastSampleNs;
        this.fps = fps;
        this.samples = samples;
        this.lateFrames = lateFrames;
    }
}
