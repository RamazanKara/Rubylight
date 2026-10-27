package com.limelight.binding.video;

/**
 * The intervals between the last presents, for the overlay's frame pacing lines: how many vsyncs
 * each frame stayed on screen, as a timeline and a histogram. Ported from Moonlight V+'s
 * JitterMonitor (qiin2333/moonlight-vplus, GPL-3.0): a ring of 240 present intervals, counts from
 * 0 to 8 vsyncs (8 also holds longer ones), judder as the share of frames off the most common
 * count, jitter as the mean absolute deviation of the interval, and intervals over 12 vsyncs (a
 * stall or a pause, not pacing) left out.
 *
 * Unlike V+, the vsync count is taken when the overlay reads the window, so a refresh rate change
 * counts the window again instead of needing a reset, and the histogram covers the window, not
 * the whole stream. Not thread-safe: {@link FrameLatencyStats} calls it under its own lock. The
 * ring is allocated once; while disabled, {@link #record} returns on its first check.
 */
final class PresentJitter {
    static final int CAPACITY = 240;
    static final int MAX_UNITS = 8;
    // V+ drops intervals longer than this many vsyncs.
    private static final int MAX_GAP_UNITS = MAX_UNITS + 4;
    // A second without a present is a pause; the vsync is not known when recording.
    private static final long MAX_RECORDED_NS = 1_000_000_000L;
    static final int TIMELINE_COLUMNS = 60;
    private static final int SAMPLES_PER_COLUMN = CAPACITY / TIMELINE_COLUMNS;
    // Indexed by vsync count; 0 (two presents within one vsync) is a dot so the column still shows.
    private static final char[] BLOCKS = {'.', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'};

    private final long[] intervalsNs = new long[CAPACITY];
    private int head;
    private int size;
    private long lastPresentNs;
    private boolean enabled;

    /** Starts or stops recording; either way the window starts empty. */
    void setEnabled(boolean on) {
        if (on == enabled) return;
        enabled = on;
        clear();
    }

    boolean isEnabled() {
        return enabled;
    }

    /** Forgets the window and the last present, after a decoder reset or while disabled. */
    void clear() {
        head = 0;
        size = 0;
        lastPresentNs = 0;
    }

    int size() {
        return size;
    }

    /** One present, in the System.nanoTime() domain. */
    void record(long presentNs) {
        if (!enabled) return;
        long previous = lastPresentNs;
        if (presentNs <= previous) return; // out of order: keep the newest present
        lastPresentNs = presentNs;
        if (previous == 0) return;
        long deltaNs = presentNs - previous;
        if (deltaNs > MAX_RECORDED_NS) return;
        intervalsNs[head] = deltaNs;
        head = (head + 1) % CAPACITY;
        if (size < CAPACITY) size++;
    }

    /** The window counted in vsyncs of vsyncNs, or null while disabled, without a vsync or without samples. */
    Snapshot snapshot(long vsyncNs) {
        if (!enabled || vsyncNs <= 0 || size == 0) return null;
        Snapshot out = new Snapshot();
        long maxNs = vsyncNs * MAX_GAP_UNITS;
        int index = (head - size + CAPACITY) % CAPACITY;
        long sumNs = 0;
        for (int i = 0; i < size; i++) {
            long ns = intervalsNs[index];
            index = (index + 1) % CAPACITY;
            if (ns > maxNs) continue;
            int units = (int) Math.min(MAX_UNITS, (ns + vsyncNs / 2) / vsyncNs);
            out.intervalsNs[out.count] = ns;
            out.units[out.count] = (byte) units;
            out.histogram[units]++;
            out.count++;
            sumNs += ns;
        }
        if (out.count == 0) return null;
        double meanNs = (double) sumNs / out.count;
        double deviationNs = 0;
        for (int i = 0; i < out.count; i++) deviationNs += Math.abs(out.intervalsNs[i] - meanNs);
        out.averageMs = (float) (meanNs / 1e6);
        out.jitterMs = (float) (deviationNs / out.count / 1e6);
        int mode = 0;
        for (int u = 1; u <= MAX_UNITS; u++) {
            if (out.histogram[u] > out.histogram[mode]) mode = u;
        }
        out.modeUnits = mode;
        out.judderPercent = 100f * (out.count - out.histogram[mode]) / out.count;
        return out;
    }

    /** The window oldest to newest, counted against one vsync. */
    static final class Snapshot {
        final long[] intervalsNs = new long[CAPACITY];
        final byte[] units = new byte[CAPACITY];
        final int[] histogram = new int[MAX_UNITS + 1];
        int count;
        int modeUnits;
        float averageMs;
        float jitterMs;
        float judderPercent;

        float percent(int units) {
            return count == 0 ? 0 : 100f * histogram[units] / count;
        }

        /**
         * The window as at most {@link #TIMELINE_COLUMNS} block characters, oldest first, each the
         * longest of {@link #SAMPLES_PER_COLUMN} frames so a single long frame still shows.
         */
        String timeline() {
            StringBuilder line = new StringBuilder(TIMELINE_COLUMNS);
            for (int start = 0; start < count; start += SAMPLES_PER_COLUMN) {
                int longest = 0;
                for (int i = start; i < Math.min(count, start + SAMPLES_PER_COLUMN); i++) {
                    longest = Math.max(longest, units[i]);
                }
                line.append(BLOCKS[longest]);
            }
            return line.toString();
        }
    }
}
