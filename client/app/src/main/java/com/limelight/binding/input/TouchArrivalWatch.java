package com.limelight.binding.input;

/**
 * How evenly a touch drag reaches the app, for the event log. A drag that turns choppy looks the
 * same on the PC whether the phone's touch path, its UI thread or the network slowed down. Sample
 * times come from the device's touch path and deliveries from this process, so a line with long
 * gaps in its samples points below the app and one with long gaps only in its deliveries points at
 * the UI thread. The first busy span gives a baseline, then only changes are logged: a span with
 * long gaps, and the first even one after it.
 */
public final class TouchArrivalWatch {
    public static final long PERIOD_MS = 5000;
    /** A gap this long is a lifted or resting finger and not a late sample. */
    public static final long PAUSE_MS = 250;
    /** A baseline needs a real drag behind it. */
    static final int BASELINE_SAMPLES = 50;
    /** Long gaps in a span that make it choppy. */
    static final int CHOPPY_GAPS = 5;
    /** The event log is short; one stream's lines stay a small part of it. */
    static final int MAX_LINES = 24;

    private static final class Gaps {
        private long last = -1;
        int count, medium, longer, pauses;
        long longest;

        void note(long atMs) {
            count++;
            if (last >= 0) {
                long gap = atMs - last;
                if (gap >= PAUSE_MS) {
                    pauses++;
                } else {
                    if (gap >= 50) longer++;
                    else if (gap >= 25) medium++;
                    longest = Math.max(longest, gap);
                }
            }
            last = atMs;
        }

        boolean choppy() {
            return medium + longer >= CHOPPY_GAPS;
        }

        void reset() {
            count = medium = longer = pauses = 0;
            longest = 0;
        }

        @Override public String toString() {
            return medium + " gaps of 25-50 ms, " + longer + " longer, longest " + longest + " ms";
        }
    }

    private final Gaps samples = new Gaps();
    private final Gaps deliveries = new Gaps();
    private long startedMs = -1;
    private boolean baselineDone;
    private boolean wasChoppy;
    private int lines;

    /** A touch sample's time, in the uptime milliseconds MotionEvent reports, history included. */
    public void sample(long sampleTimeMs) {
        samples.note(sampleTimeMs);
    }

    /** One delivered event. Once a period has passed it returns the line to log, if any. */
    public String delivered(long nowMs) {
        deliveries.note(nowMs);
        if (startedMs < 0) startedMs = nowMs;
        if (nowMs - startedMs < PERIOD_MS) return null;
        String line = lineForSpan();
        startedMs = nowMs;
        samples.reset();
        deliveries.reset();
        return line;
    }

    private String lineForSpan() {
        boolean choppy = samples.choppy() || deliveries.choppy();
        boolean baseline = !baselineDone && samples.count >= BASELINE_SAMPLES;
        boolean recovered = wasChoppy && !choppy;
        if (baseline) baselineDone = true;
        wasChoppy = choppy;
        if ((!choppy && !baseline && !recovered) || lines >= MAX_LINES) return null;
        lines++;
        return "Touch arrivals: " + samples.count + " samples (" + samples + ", " + samples.pauses + " pauses), " +
                deliveries.count + " deliveries (" + deliveries + ")";
    }
}
