package com.limelight.binding.video;

import java.util.Arrays;

/** Display-thread observations, published without taking the decoder's submission lock. */
final class PyroWaveDisplayClock {
    static final class Snapshot {
        static final Snapshot UNAVAILABLE = new Snapshot(0, 0);
        final long vsyncNs;
        final long intervalNs;

        Snapshot(long vsyncNs, long intervalNs) {
            this.vsyncNs = vsyncNs;
            this.intervalNs = intervalNs;
        }
    }

    private final long[] intervals = new long[7];
    private int count;
    private int position;
    private long nominalIntervalNs;
    private long previousVsyncNs;
    private volatile Snapshot snapshot = Snapshot.UNAVAILABLE;

    // Called on the Choreographer thread when the output mode changes. The advertised mode
    // only helps identify skipped callbacks; it is never published as a measured interval.
    void reset(long nominalIntervalNs) {
        this.nominalIntervalNs = nominalIntervalNs;
        previousVsyncNs = 0;
        count = position = 0;
        snapshot = Snapshot.UNAVAILABLE;
    }

    void observe(long frameTimeNs) {
        if (nominalIntervalNs <= 0 || frameTimeNs <= previousVsyncNs) return;
        long deltaNs = previousVsyncNs == 0 ? 0 : frameTimeNs - previousVsyncNs;
        previousVsyncNs = frameTimeNs;
        if (deltaNs > 0) {
            long refreshes = Math.round((double) deltaNs / nominalIntervalNs);
            if (refreshes >= 1 && refreshes <= 8) {
                long intervalNs = deltaNs / refreshes;
                // A delayed callback may skip refreshes. Irregular or very long gaps should
                // not look like a slower panel; median filtering also rejects isolated jitter.
                if (Math.abs(intervalNs - nominalIntervalNs) <= nominalIntervalNs / 10) {
                    intervals[position] = intervalNs;
                    position = (position + 1) % intervals.length;
                    count = Math.min(intervals.length, count + 1);
                }
            }
        }
        if (count >= 3) {
            long[] sorted = Arrays.copyOf(intervals, count);
            Arrays.sort(sorted);
            snapshot = new Snapshot(frameTimeNs, sorted[count / 2]);
        }
    }

    Snapshot snapshot() {
        return snapshot;
    }
}
