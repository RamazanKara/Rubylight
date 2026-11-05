package com.limelight.binding.video;

import java.util.Arrays;
import java.util.Locale;

/**
 * Erratum 23's repairs: with Android's low-latency mode on, the decoders of
 * {@link MediaCodecHelper#corruptsInLowLatency} may keep showing a corrupted picture until the next
 * keyframe, and a stream has none of its own. So one is asked for when decode latency climbs and again
 * when the climb ends, in case what was damaged during it stays: one test's blocks came with such a climb,
 * and whether a climb causes them is not known. Also on a calm picture after {@link #QUIET_MS}, and
 * whatever the picture after {@link #LIMIT_MS}. Keyframes that came for any other reason restart the
 * waits, and a request that goes unanswered is not repeated on every frame.
 *
 * Should that theory be wrong these keyframes only cost, so a climb has to reach well into the frame
 * interval, repeated climbs' repairs back off, and a level that stays up becomes the new baseline.
 *
 * Decode latency comes from the thread that takes frames out of the decoder, the rest from the thread
 * that submits decode units, and the report reads {@link Counts} from a third.
 */
final class KeyframeSafetyNet {
    enum Reason { CLIMB, CLIMB_END, QUIET, LIMIT }

    // A keyframe should cost less on a calm picture than the 1-4.5 MB of one in motion, though that is not
    // measured on these decoders, so a calm one comes soonest.
    static final long QUIET_MS = 10_000;
    // In motion a keyframe takes 46-70 ms on the wire, so at most this often when the picture never calms.
    static final long LIMIT_MS = 30_000;
    // Between keyframes, whatever asked for them, so a long climb can't become a storm. The largest
    // keyframe seen (4.5 MB) this often is 7 Mbps, about a tenth of the 63 Mbps those streams ran at.
    static final long MIN_GAP_MS = 5_000;
    // Calm is well under a stream's motion: a quarter of what a frame may take at its bitrate and frame rate,
    // so 32 KB at 63 Mbps and 60 fps, where motion is about 130 KB.
    private static final int QUIET_FRAME_SHARE = 4;
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
    // Well above the baseline means twice it and at least 4 ms over, and past half a frame interval: until
    // there a decode has time to spare, whatever its baseline. From the reports' average of 3 ms at 60 fps
    // that is 8.3 ms, which their 3-5-7-8-9 ramp passes on its way to about 20.
    private static final float CLIMB_MIN_MARGIN_MS = 4;
    private static final float CLIMB_FLOOR_FRAME_SHARE = 0.5f;
    // After each climb repair the next one waits this long, the last gap repeating, so a latency that keeps
    // moving can't turn into a keyframe every MIN_GAP_MS for good.
    private static final long[] CLIMB_REPAIR_GAPS_MS = {5_000, 10_000, 20_000, 30_000};
    // This long without a climb or a climb repair starts those gaps over. More than the longest of them, so
    // a latency that keeps moving never gets there.
    private static final long CLIMB_CALM_MS = 60_000;
    // A climb up this long is the stream's new level, not a climb: a heavier game, heat, another bitrate, a
    // resize. The reports' climbs last 10-15 s.
    private static final long CLIMB_MAX_MS = 60_000;
    // The peak is what the medians held for a second, as long as a climb's end has to hold: a spike's height
    // would put the way back down above the plateau it came from.
    private static final int CLIMB_PEAK_BUCKETS = (int) (CLIMB_END_HOLD_MS / BUCKET_MS);
    // Falling is more than a tenth down over the hold: a plateau's medians wander less than that, and a
    // backlog draining from 20 ms in a few seconds loses more.
    private static final float CLIMB_FALLING_RATIO = 0.9f;
    // The baseline is the lowest median of the last minute, made of 4 s minima. It falls at once and rises
    // only as low medians age out, and medians at or above the climb level never enter, so a ramp of seconds
    // can't lift it. A drift over several minutes does, and is left to the periodic refresh.
    private static final long BASELINE_SEGMENT_MS = 4_000;
    private static final int BASELINE_SEGMENTS = 15;
    // Two seconds of medians before anything can be judged: a stream's first frames are no baseline.
    private static final int WARM_UP_BUCKETS = 8;

    /** What one stream's nets and spacers did, kept across decoder restarts. Each field has one writer at a time. */
    static final class Counts {
        volatile int climbs;
        volatile int settledClimbs;
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
            return "climbs " + climbs + ", settled " + settledClimbs +
                    ", repairs climb " + climbRepairs + " end " + climbEndRepairs +
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
    private final float[] climbRecent = new float[CLIMB_PEAK_BUCKETS];
    private int climbRecentNext;
    private long belowSinceMs = -1;

    private int climbRepairsInARow;
    private long lastClimbRepairMs = Long.MIN_VALUE / 2;
    private long lastClimbActivityMs;

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
    synchronized boolean due(long nowMs, int bitrateKbps, int fpsMilliHz) {
        if (!started) return false;
        long sinceRefresh = nowMs - Math.max(lastKeyframeMs, lastRequestMs);
        if (sinceRefresh < MIN_GAP_MS) return false;
        // A climb repair still waiting for its gap: the waits below go on, and any keyframe answers it too.
        Reason reason = pending != null && nowMs - lastClimbRepairMs >= climbRepairGapMs() ? pending : null;
        if (reason != null) {
            climbRepairsInARow++;
            lastClimbRepairMs = nowMs;
            lastClimbActivityMs = nowMs;
        } else if (sinceRefresh >= LIMIT_MS) {
            reason = Reason.LIMIT;
        } else if (sinceRefresh >= QUIET_MS && averageFrameBytes < quietFrameBytes(bitrateKbps, fpsMilliHz)) {
            reason = Reason.QUIET;
        } else {
            return false;
        }
        pending = null;
        lastRequestMs = nowMs;
        unanswered = true;
        counts.add(reason);
        return true;
    }

    private long climbRepairGapMs() {
        if (climbRepairsInARow == 0) return 0;
        return CLIMB_REPAIR_GAPS_MS[Math.min(climbRepairsInARow, CLIMB_REPAIR_GAPS_MS.length) - 1];
    }

    /** The size under which a frame is calm at this bitrate and frame rate; 0, so none is, when either is unknown. */
    static long quietFrameBytes(int bitrateKbps, int fpsMilliHz) {
        if (bitrateKbps <= 0 || fpsMilliHz < 1000) return 0;
        return bitrateKbps * 125_000L / fpsMilliHz / QUIET_FRAME_SHARE;
    }

    /** One frame's time from decoder input to output. Unknown times (0 or less) don't count. */
    synchronized void onDecodeLatency(long nowMs, float latencyMs, int fpsMilliHz) {
        if (latencyMs <= 0) return;
        long sinceLastSampleMs = nowMs - lastSampleMs;
        if (sinceLastSampleMs >= SAMPLE_GAP_MS) {
            bucketSamples = 0;
            aboveSinceMs = -1;
            belowSinceMs = -1;
            // A paused stream's time is no time at this level.
            climbStartMs += sinceLastSampleMs;
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
            else judge(median, nowMs, fpsMilliHz);
        }
    }

    // An unknown frame rate has no budget to measure against, so nothing climbs.
    private static float climbFloorMs(int fpsMilliHz) {
        return fpsMilliHz < 1000 ? Float.MAX_VALUE :
                FramePacingPolicy.frameIntervalNs(fpsMilliHz) / 1_000_000f * CLIMB_FLOOR_FRAME_SHARE;
    }

    private void judge(float median, long nowMs, int fpsMilliHz) {
        if (warmUpBuckets < WARM_UP_BUCKETS) {
            warmUpBuckets++;
            addToBaseline(median, nowMs);
            return;
        }
        float baseline = baseline();
        if (median < Math.max(baseline + Math.max(CLIMB_MIN_MARGIN_MS, baseline), climbFloorMs(fpsMilliHz))) {
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
        Arrays.fill(climbRecent, median);
        belowSinceMs = -1;
        aboveSinceMs = -1;
        if (nowMs - lastClimbActivityMs >= CLIMB_CALM_MS) climbRepairsInARow = 0;
        lastClimbActivityMs = nowMs;
        counts.climbs++;
        pending = Reason.CLIMB;
        event = String.format(Locale.ROOT, "Decoder latency climb: %.1f ms over a %.1f ms baseline", median, baseline);
    }

    private void whileClimbing(float median, long nowMs) {
        if (nowMs - climbStartMs >= CLIMB_MAX_MS) {
            settle(nowMs);
            return;
        }
        climbRecent[climbRecentNext] = median;
        climbRecentNext = (climbRecentNext + 1) % CLIMB_PEAK_BUCKETS;
        float held = median;
        for (float recent : climbRecent) held = Math.min(held, recent);
        climbPeak = Math.max(climbPeak, held);
        // Past the way back down: for the reports' 20 ms from 3 ms that is under 11.5 ms, well inside a frame.
        if (median > climbBaseline + (climbPeak - climbBaseline) / 2) {
            belowSinceMs = -1;
        } else if (belowSinceMs < 0) {
            belowSinceMs = nowMs;
        } else if (nowMs - belowSinceMs >= CLIMB_END_HOLD_MS) {
            climbing = false;
            lastClimbActivityMs = nowMs;
            pending = Reason.CLIMB_END;
            event = String.format(Locale.ROOT, "Decoder latency climb ended after %.0f s, peak %.1f ms",
                    (nowMs - climbStartMs) / 1000f, climbPeak);
        }
    }

    // The level that stayed is the new normal and no repair: the next medians warm a baseline up from it.
    private void settle(long nowMs) {
        climbing = false;
        lastClimbActivityMs = nowMs;
        baselineFilled = 0;
        baselineNext = 0;
        segmentMin = Float.MAX_VALUE;
        warmUpBuckets = 0;
        counts.settledClimbs++;
        event = String.format(Locale.ROOT, "Decoder latency stayed up for %.0f s, peak %.1f ms: the new baseline",
                (nowMs - climbStartMs) / 1000f, climbPeak);
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
