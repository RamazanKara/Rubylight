package com.limelight.binding.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class TouchArrivalWatchTest {
    private final TouchArrivalWatch watch = new TouchArrivalWatch();
    private long now = 1000;

    /** One span of a drag with evenly spaced events; returns its line, if any. */
    private String drag(long gapMs) {
        String line = null;
        long end = now + TouchArrivalWatch.PERIOD_MS;
        while (now <= end) {
            watch.sample(now);
            String got = watch.delivered(now);
            if (got != null) line = got;
            now += gapMs;
        }
        return line;
    }

    @Test
    public void theFirstBusySpanIsTheBaselineAndEvenSpansAfterItStayQuiet() {
        String baseline = drag(8);
        assertNotNull(baseline);
        assertTrue(baseline, baseline.contains("0 gaps of 25-50 ms, 0 longer, longest 8 ms"));
        assertNull(drag(8));
        assertNull(drag(8));
    }

    @Test
    public void stepsOf42MsAreLoggedAndTheRecoveryOnce() {
        drag(8);
        String choppy = drag(42);
        assertNotNull(choppy);
        assertTrue(choppy, choppy.contains("gaps of 25-50 ms, 0 longer, longest 42 ms"));
        assertNotNull("a span that stays choppy keeps being logged", drag(42));
        assertNotNull("the first even span closes the episode", drag(8));
        assertNull(drag(8));
    }

    @Test
    public void aLiftedFingerIsAPauseAndNotAGap() {
        drag(8);
        String line = null;
        for (int i = 0; i < 10 && line == null; i++) {
            watch.sample(now);
            line = watch.delivered(now);
            now += TouchArrivalWatch.PAUSE_MS + 400;
        }
        // Pauses alone are normal use: no gaps, so nothing to report.
        assertNull(line);
    }

    @Test
    public void gapsOnlyInDeliveriesStillCount() {
        drag(8);
        String line = null;
        long end = now + TouchArrivalWatch.PERIOD_MS;
        while (now <= end) {
            // A busy UI thread: samples kept their 8 ms, five at a time arrive 40 ms apart.
            for (int i = 4; i >= 0; i--) watch.sample(now - i * 8);
            String got = watch.delivered(now);
            if (got != null) line = got;
            now += 40;
        }
        assertNotNull(line);
        assertTrue(line, line.contains("samples (0 gaps of 25-50 ms, 0 longer, longest 8 ms"));
        assertTrue(line, line.contains("deliveries (") && !line.contains("deliveries (0 gaps"));
    }

    @Test
    public void onlyFewLongGapsAreNotChoppy() {
        drag(8);
        watch.sample(now);
        assertNull(watch.delivered(now));
        for (int i = 0; i < TouchArrivalWatch.CHOPPY_GAPS - 1; i++) {
            now += 40;
            watch.sample(now);
            watch.delivered(now);
        }
        assertNull(drag(8));
    }

    @Test
    public void aStreamWritesABoundedNumberOfLines() {
        int lines = 0;
        for (int i = 0; i < TouchArrivalWatch.MAX_LINES * 20; i++) {
            if (drag(42) != null) lines++;
        }
        assertEquals(TouchArrivalWatch.MAX_LINES, lines);
    }

    @Test
    public void aLongChoppyStretchLeavesLinesForALaterEpisode() {
        int minutes = 3;
        int lines = 0;
        for (long i = 0; i < minutes * 60_000 / TouchArrivalWatch.PERIOD_MS; i++) {
            if (drag(42) != null) lines++;
        }
        assertTrue("spans logged: " + lines, lines >= TouchArrivalWatch.FREE_LINES && lines < TouchArrivalWatch.MAX_LINES);
        drag(8);
        drag(8);
        now += 10 * TouchArrivalWatch.LATE_LINE_SPACING_MS;
        String first = drag(42);
        String second = drag(42);
        assertTrue("the next episode still gets a line", first != null || second != null);
    }
}
