package com.limelight.utils;

/**
 * Finds a regular rhythm in stall start times, such as the "lag spike every 2 minutes" that
 * background Wi-Fi scans cause. Pure arithmetic; callers run it only when a new stall starts.
 */
public final class PeriodicStalls {
    public static final long MIN_PERIOD_MS = 30_000;
    public static final long MAX_PERIOD_MS = 10 * 60_000;
    /** Events closer than this belong to one stall. */
    public static final long BURST_GAP_MS = 2_500;
    /** A rhythm needs this many stalls on its beat, i.e. at least three repeats. */
    public static final int MIN_HITS = 4;
    private static final long MIN_TOLERANCE_MS = 2_000;

    private PeriodicStalls() { }

    /** Tolerance for one beat: the larger of two seconds and 4% of the period. Scan timers are steady. */
    static long tolerance(long periodMs) {
        return Math.max(MIN_TOLERANCE_MS, periodMs * 4 / 100);
    }

    /**
     * @param times stall event times in ascending order (ms, any monotonic clock)
     * @param count number of valid entries at the start of {@code times}
     * @return the detected period in ms, or 0 when the stalls show no regular rhythm
     */
    public static long periodMs(long[] times, int count) {
        // Collapse runs of events into stall starts.
        long[] starts = new long[count];
        int n = 0;
        for (int i = 0; i < count; i++) {
            if (n == 0 || times[i] - times[i - 1] > BURST_GAP_MS) starts[n++] = times[i];
        }
        if (n < MIN_HITS) return 0;
        long best = 0;
        int bestHits = 0;
        // Candidate periods come from gaps between a stall and its next two successors, so one
        // unrelated stall between two beats cannot hide the rhythm.
        for (int i = 0; i < n; i++) {
            for (int j = i + 1; j < n && j <= i + 2; j++) {
                long candidate = starts[j] - starts[i];
                if (candidate < MIN_PERIOD_MS || candidate > MAX_PERIOD_MS) continue;
                long[] fit = chain(starts, n, i, candidate);
                int hits = (int) fit[0];
                if (hits > bestHits || (hits == bestHits && hits > 0 && fit[1] < best)) {
                    bestHits = hits;
                    best = fit[1];
                }
            }
        }
        return bestHits >= MIN_HITS ? best : 0;
    }

    /**
     * Follows the beat from {@code first}, tolerating one missed beat at a time and re-anchoring on
     * each hit so jitter does not accumulate. Returns {hits, refined period} or {0, 0} when the chain
     * is too sparse to be a rhythm.
     */
    private static long[] chain(long[] starts, int n, int first, long period) {
        long tolerance = tolerance(period);
        int hits = 1;
        int slots = 1;
        long last = starts[first];
        int lastIndex = first;
        int index = first + 1;
        while (index < n) {
            long hit = -1;
            int missed = 0;
            for (int skip = 1; skip <= 2 && hit < 0; skip++) {
                long expected = last + period * skip;
                for (int k = index; k < n && starts[k] <= expected + tolerance; k++) {
                    if (Math.abs(starts[k] - expected) <= tolerance) {
                        hit = starts[k];
                        index = k;
                        missed = skip - 1;
                        break;
                    }
                }
            }
            if (hit < 0) break;
            hits++;
            slots += missed + 1;
            last = hit;
            lastIndex = index;
            index++;
        }
        if (hits < MIN_HITS || hits * 4 < slots * 3) return new long[] {0, 0};
        // With many other stalls some land on a beat by luck. Spread the stalls off the beat at random:
        // a chain this long must be unlikely across all candidates tried (about two per stall).
        double span = Math.max(1, starts[n - 1] - starts[0]);
        double chance = Math.min(0.999, Math.max(1, n - hits) / span * 2 * tolerance);
        if ((hits - 1) * Math.log(chance) + Math.log(2.0 * n) > Math.log(0.001)) return new long[] {0, 0};
        // At most one unrelated stall per two beats inside the rhythm's span.
        int inSpan = lastIndex - first + 1;
        if (hits * 3 < inSpan * 2) return new long[] {0, 0};
        return new long[] {hits, (last - starts[first]) / (slots - 1)};
    }
}
