package com.limelight.binding.video;

import java.util.Arrays;
import java.util.Locale;

/**
 * Erratum 23's repairs: with Android's low-latency mode on, the decoders of
 * {@link MediaCodecHelper#corruptsInLowLatency} may keep showing a corrupted picture until the next
 * keyframe, and a stream has none of its own. So one is asked for when decode latency climbs and again
 * when the climb ends, because what was damaged during it stays; on a calm picture after
 * {@link #QUIET_MS}; and whatever the picture after {@link #LIMIT_MS}. Keyframes that came for any other
 * reason restart the waits, and a request that goes unanswered is not repeated on every frame.
 *
 * Decode latency comes from the thread that takes frames out of the decoder, the rest from the thread
 * that submits decode units, and the report reads {@link Counts} from a third.
 */
final class KeyframeSafetyNet {
    enum Reason { CLIMB, CLIMB_END, QUIET, LIMIT }

    // A keyframe costs less on a calm picture than the 1-4.5 MB of one in motion, so a calm one comes soonest.
    static final long QUIET_MS = 10_000;
    // In motion a keyframe takes 46-70 ms on the wire, so at most this often when the picture never calms.
    static final long LIMIT_MS = 30_000;
    // Between keyframes, whatever asked for them, so a long climb can't become a storm. The largest
    // keyframe seen (4.5 MB) this often is 7 Mbps, about a tenth of the 63 Mbps those streams ran at.
    static final long MIN_GAP_MS = 5_000;
    // Calm is well under a stream's motion: 63 Mbps at 60 fps is about 130 KB a frame, a quarter of that here.
    static final int QUIET_FRAME_BYTES = 32 * 1024;
    // The average frame size follows about the last second at 60 fps.
    private static final float FRAME_AVERAGE_WEIGHT = 1 / 32f;

    // Medians over this long, 15 frames at 60 fps: one slow frame (a keyframe, a large frame) moves nothing.
    private static final long BUCKET_MS = 250;
    private static final int BUCKET_MIN_SAMPLES = 5;
    private static final int BUCKET_MAX_SAMPLES = 64;
    // No output for this long (the stream paused) starts the current measurement over.
    private static final long SAMPLE_GAP_MS = 2_000;
    // The reports' climbs stay up 10-15 s, a large frame's backlog drains in less than this.
    private static final long CLIMB_HOLD_MS = 1_000;
    private static final long CLIMB_END_HOLD_MS = 1_000;
    // Well above the baseline means twice it and at least 4 ms over: from the reports' average of 3 ms
    // that is 7 ms, where their 3-5-7-8-9 ramp has left the frame-to-frame noise.
    private static final float CLIMB_MIN_MARGIN_MS = 4;
    // Falling is more than a tenth down over the hold: a plateau's medians wander less than that, and a
    // backlog draining from 20 ms in a few seconds loses more.
    private static final float CLIMB_FALLING_RATIO = 0.9f;
    // The baseline is the lowest median of the last minute, made of 4 s minima. It falls at once and rises
    // only as low medians age out, and medians above the climb level never enter, so a ramp of seconds
    // can't lift it. A drift over several minutes does, and is left to the periodic refresh.
    private static final long BASELINE_SEGMENT_MS = 4_000;
    private static final int BASELINE_SEGMENTS = 15;
    // Two seconds of medians before anything can be judged: a stream's first frames are no baseline.
    private static final int WARM_UP_BUCKETS = 8;

    /** What one stream's nets and spacers did, kept across decoder restarts. Each field has one writer at a time. */
    static final class Counts {
        volatile int climbs;
        volatile int climbRepairs;
        volatile int climbEndRepairs;
        volatile int quietRepairs;
        volatile int limitRepairs;
        volatile int gapFrames;
        volatile int heldFrames;
        volatile long heldNs;

        /** How many keyframes this stream asked for. */
        int requests() {
            return climbRepairs + climbEndRepairs + quietRepairs + limitRepairs;
        }

        /** The stream's part of its report row: numbers only. */
        String describe() {
            return "climbs " + climbs + ", repairs climb " + climbRepairs + " end " + climbEndRepairs +
                    " quiet " + quietRepairs + " limit " + limitRepairs +
                    ", gap frames " + gapFrames +
                    ", held " + heldFrames + " frames " + heldNs / 1_000_000 + " ms";
        }

        private void add(Reason reason) {
            switch (reason) {
                case CLIMB: climbRepairs++; break;
                case CLIMB_END: climbEndRepairs++; break;
                case QUIET: quietRepairs++; break;
                case LIMIT: limitRepairs++; break;
            }
        }
    }

    private final Counts counts;

    private boolean started;
    private long lastKeyframeMs;
    private long lastRequestMs = Long.MIN_VALUE / 2;
    private boolean unanswered;
    private Reason pending;
    private float averageFrameBytes;

    private final float[] bucket = new float[BUCKET_MAX_SAMPLES];
    private int bucketSamples;
    private long bucketStartMs;
    private long lastSampleMs;
    private int warmUpBuckets;

    private final float[] baselineSegments = new float[BASELINE_SEGMENTS];
    private int baselineFilled;
    private int baselineNext;
    private float segmentMin = Float.MAX_VALUE;
    private long segmentStartMs;

    private long aboveSinceMs = -1;
    private float aboveFirst;

    private boolean climbing;
    private long climbStartMs;
    private float climbBaseline;
    private float climbPeak;
    private long belowSinceMs = -1;

    private String event;

    KeyframeSafetyNet(Counts counts) {
        this.counts = counts;
    }

    /** A keyframe reached the decoder. Before the first one there is nothing to refresh. */
    synchronized void onKeyframe(long nowMs) {
        started = true;
        lastKeyframeMs = nowMs;
        unanswered = false;
        pending = null;
    }

    /**
     * The keyframe now arriving answers this net's request and came after this gap in the frame numbers:
     * the frames dropped while waiting for it are not network loss. Only one gap per request.
     */
    synchronized boolean takeAnsweredGap(int missingFrames) {
        if (!unanswered) return false;
        unanswered = false;
        counts.gapFrames += missingFrames;
        return true;
    }

    /** The size of a frame that is not a keyframe. */
    synchronized void onFrame(int bytes) {
        averageFrameBytes += (bytes - averageFrameBytes) * FRAME_AVERAGE_WEIGHT;
    }

    /** Whether to ask for a keyframe now; answering true starts the waits again. */
    synchronized boolean due(long nowMs) {
        if (!started) return false;
        long sinceRefresh = nowMs - Math.max(lastKeyframeMs, lastRequestMs);
        if (sinceRefresh < MIN_GAP_MS) return false;
        Reason reason = pending;
        if (reason == null) {
            if (sinceRefresh >= LIMIT_MS) reason = Reason.LIMIT;
            else if (sinceRefresh >= QUIET_MS && averageFrameBytes <= QUIET_FRAME_BYTES) reason = Reason.QUIET;
            else return false;
        }
        pending = null;
        lastRequestMs = nowMs;
        unanswered = true;
        counts.add(reason);
        return true;
    }

    /** One frame's time from decoder input to output. Unknown times (0 or less) don't count. */
    synchronized void onDecodeLatency(long nowMs, float latencyMs) {
        if (latencyMs <= 0) return;
        if (bucketSamples > 0 && nowMs - lastSampleMs >= SAMPLE_GAP_MS) {
            bucketSamples = 0;
            aboveSinceMs = -1;
            belowSinceMs = -1;
        }
        lastSampleMs = nowMs;
        if (bucketSamples == 0) bucketStartMs = nowMs;
        bucket[bucketSamples++] = latencyMs;
        if (bucketSamples == BUCKET_MAX_SAMPLES ||
                (bucketSamples >= BUCKET_MIN_SAMPLES && nowMs - bucketStartMs >= BUCKET_MS)) {
            Arrays.sort(bucket, 0, bucketSamples);
            float median = bucket[bucketSamples / 2];
            bucketSamples = 0;
            if (climbing) whileClimbing(median, nowMs);
            else judge(median, nowMs);
        }
    }

    private void judge(float median, long nowMs) {
        if (warmUpBuckets < WARM_UP_BUCKETS) {
            warmUpBuckets++;
            addToBaseline(median, nowMs);
            return;
        }
        float baseline = baseline();
        if (median < baseline + Math.max(CLIMB_MIN_MARGIN_MS, baseline)) {
            aboveSinceMs = -1;
            addToBaseline(median, nowMs);
            return;
        }
        if (aboveSinceMs < 0) {
            aboveSinceMs = nowMs;
            aboveFirst = median;
        } else if (nowMs - aboveSinceMs >= CLIMB_HOLD_MS) {
            if (median >= aboveFirst * CLIMB_FALLING_RATIO) {
                startClimb(baseline, median, nowMs);
            } else {
                // Falling: a backlog draining rather than a climb. A level that then holds still counts.
                aboveSinceMs = nowMs;
                aboveFirst = median;
            }
        }
    }

    private void startClimb(float baseline, float median, long nowMs) {
        climbing = true;
        climbStartMs = nowMs;
        climbBaseline = baseline;
        climbPeak = median;
        belowSinceMs = -1;
        aboveSinceMs = -1;
        counts.climbs++;
        pending = Reason.CLIMB;
        event = String.format(Locale.ROOT, "Decoder latency climb: %.1f ms over a %.1f ms baseline", median, baseline);
    }

    private void whileClimbing(float median, long nowMs) {
        climbPeak = Math.max(climbPeak, median);
        // Past the way back down: for the reports' 20 ms from 3 ms that is under 11.5 ms, well inside a frame.
        if (median > climbBaseline + (climbPeak - climbBaseline) / 2) {
            belowSinceMs = -1;
        } else if (belowSinceMs < 0) {
            belowSinceMs = nowMs;
        } else if (nowMs - belowSinceMs >= CLIMB_END_HOLD_MS) {
            climbing = false;
            pending = Reason.CLIMB_END;
            event = String.format(Locale.ROOT, "Decoder latency climb ended after %.0f s, peak %.1f ms",
                    (nowMs - climbStartMs) / 1000f, climbPeak);
        }
    }

    private float baseline() {
        float lowest = segmentMin;
        for (int i = 0; i < baselineFilled; i++) lowest = Math.min(lowest, baselineSegments[i]);
        return lowest;
    }

    private void addToBaseline(float median, long nowMs) {
        if (segmentMin == Float.MAX_VALUE) segmentStartMs = nowMs;
        segmentMin = Math.min(segmentMin, median);
        if (nowMs - segmentStartMs >= BASELINE_SEGMENT_MS) {
            baselineSegments[baselineNext] = segmentMin;
            baselineNext = (baselineNext + 1) % BASELINE_SEGMENTS;
            baselineFilled = Math.min(BASELINE_SEGMENTS, baselineFilled + 1);
            segmentMin = Float.MAX_VALUE;
        }
    }

    /** A line for the report about the climb that just started or ended, once; null when there is none. */
    synchronized String takeEvent() {
        String taken = event;
        event = null;
        return taken;
    }
}
