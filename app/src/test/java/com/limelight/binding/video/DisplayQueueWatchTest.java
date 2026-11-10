package com.limelight.binding.video;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

// Nightjar #7: a display that keeps a frame too many waiting for minutes in Lowest latency and Balanced.
public class DisplayQueueWatchTest {
    private static final long REFRESH_NS = 16_683_000L;
    private static final long MS = 1_000_000L;

    /** A 60 Hz stream on a 60 Hz display: one frame per refresh, its wait from the hand-off to its render time. */
    private static final class Display {
        final DisplayQueueWatch watch = new DisplayQueueWatch();
        final List<String> events = new ArrayList<>();
        long renderNs = 100_000 * MS;
        int drops;

        Display() {
            this(true);
        }

        Display(boolean watched) {
            watch.configure(watched, REFRESH_NS);
        }

        /** Shows frames for {@code seconds}, each waiting {@code waitNs} (or what {@code wait} says per frame). */
        void show(double seconds, java.util.function.LongUnaryOperator wait) {
            long frames = Math.round(seconds * 1e9 / REFRESH_NS);
            for (long i = 0; i < frames; i++) {
                renderNs += REFRESH_NS;
                long handoffNs = renderNs - wait.applyAsLong(i);
                if (watch.takeDrop(handoffNs)) {
                    drops++;
                    take();
                    continue;
                }
                take();
                watch.onRendered(renderNs - handoffNs, renderNs);
                take();
            }
        }

        void show(double seconds, long waitNs) {
            show(seconds, i -> waitNs);
        }

        /** Shows frames waiting {@code waitNs} until one is dropped, for at most {@code seconds}: true when one was. */
        boolean showUntilDrop(double seconds, long waitNs) {
            long frames = Math.round(seconds * 1e9 / REFRESH_NS);
            for (long i = 0; i < frames; i++) {
                int before = drops;
                show(REFRESH_NS / 1e9, waitNs);
                if (drops > before) return true;
            }
            return false;
        }

        void take() {
            String event = watch.takeEvent();
            if (event != null) events.add(event);
        }

        boolean said(String text) {
            for (String event : events) if (event.contains(text)) return true;
            return false;
        }
    }

    @Test
    public void aWaitARefreshOverTheStreamsOwnLevelDropsOneFrameAndIsCheckedAfter() {
        Display display = new Display();
        display.show(12, 30 * MS);
        assertEquals(0, display.drops);
        // A frame too many stays waiting: every frame waits a refresh longer.
        assertTrue(display.showUntilDrop(10, 48 * MS));
        assertTrue(display.events.toString(), display.said("over its lowest level"));
        // The display gave the buffer back.
        display.show(5, 30 * MS);
        assertTrue(display.events.toString(), display.said("held a buffer too many"));
        display.show(60, 30 * MS);
        assertEquals(1, display.drops);
    }

    @Test
    public void aSteadyDeepWaitOfUnknownLevelIsProbedOnceAndKeptWhenItDoesNotFall() {
        Display display = new Display();
        // From the start the wait is several refreshes and steady: is a frame too many waiting?
        assertTrue(display.showUntilDrop(15, 60 * MS));
        assertTrue(display.events.toString(), display.said("not known yet"));
        // It was not: the wait stays, and that is this display's own level from now on.
        display.show(60, 60 * MS);
        assertEquals(1, display.drops);
        assertTrue(display.events.toString(), display.said("no buffer too many"));
        assertFalse("a probe is no miss", display.said("no more drops"));
    }

    @Test
    public void aProbeThatGivesARefreshBackKeepsLookingUntilTheLevelHolds() {
        Display display = new Display();
        // Two frames too many, given back one at a time (the Shield's capture: 72, 57, then 44 ms).
        assertTrue(display.showUntilDrop(15, 74 * MS));
        assertTrue(display.showUntilDrop(15, 58 * MS));
        assertTrue(display.said("held a buffer too many"));
        assertTrue("a third probe finds the display's own level", display.showUntilDrop(15, 44 * MS));
        display.show(60, 44 * MS);
        assertEquals(3, display.drops);
        assertTrue(display.said("no buffer too many"));
    }

    @Test
    public void aDisplayThatReportsTheHandOffNeverDrops() {
        Display display = new Display();
        display.show(120, i -> (1 + i % 3) * MS);
        assertEquals(0, display.drops);
        assertTrue(display.events.isEmpty());
    }

    @Test
    public void theWaitThatClockDriftAddsWithinOneRefreshNeverDrops() {
        // Lowest latency with the host 45 ppm faster than the display: the wait climbs through one
        // refresh over about six minutes, then the display replaces a frame and it starts over.
        Display display = new Display();
        for (int cycle = 0; cycle < 2; cycle++) {
            final long base = 40 * MS;
            final long[] frame = {0};
            display.show(370, i -> base + Math.min(REFRESH_NS - MS / 2, frame[0]++ * 45 * REFRESH_NS / 1_000_000));
        }
        // Only the one probe of a deep wait whose level is not known yet; it finds none too many.
        assertEquals(1, display.drops);
        assertTrue(display.said("no buffer too many"));
        assertFalse(display.said("over its lowest level"));
    }

    @Test
    public void dropsThatDoNotLowerTheWaitStopTheWatchForTheStream() {
        Display display = new Display();
        display.show(12, 30 * MS);
        // The wait rises for good (a longer composition path, not a queue): a drop does not change it.
        assertTrue(display.showUntilDrop(10, 50 * MS));
        display.show(4, 50 * MS);
        assertTrue(display.said("no buffer too many"));
        // The new level is the display's own; a second rise is tried once more, then the watch stops.
        assertTrue(display.showUntilDrop(12, 69 * MS));
        display.show(4, 69 * MS);
        assertEquals(2, display.drops);
        assertTrue(display.events.toString(), display.said("no more drops"));
        display.show(10, 90 * MS);
        display.show(60, 69 * MS);
        assertEquals(2, display.drops);
    }

    @Test
    public void aFrameReportedBeforeThePreviousOneIsLoggedAsReplacedAndLeftOut() {
        Display display = new Display();
        display.show(5, 30 * MS);
        long previous = display.renderNs;
        display.watch.onRendered(8 * MS, previous - 47 * MS);
        display.take();
        assertTrue(display.events.toString(), display.said("replaced in the display's queue"));
        display.show(30, 30 * MS);
        assertEquals(0, display.drops);
    }

    @Test
    public void appliesOnlyWhereTheDisplaysQueueIsNightjarsToWatch() {
        int fps60 = 60_000;
        assertTrue(DisplayQueueWatch.applies(true, true, false, true, false, fps60, REFRESH_NS));
        // Smoothest and the scheduled modes keep their own queue.
        assertFalse(DisplayQueueWatch.applies(true, false, false, true, false, fps60, REFRESH_NS));
        assertFalse(DisplayQueueWatch.applies(true, true, true, true, false, fps60, REFRESH_NS));
        // No render times (the upscaler, Vulkan), a variable refresh, or no refresh known.
        assertFalse(DisplayQueueWatch.applies(true, true, false, false, false, fps60, REFRESH_NS));
        assertFalse(DisplayQueueWatch.applies(true, true, false, true, true, fps60, REFRESH_NS));
        assertFalse(DisplayQueueWatch.applies(true, true, false, true, false, fps60, 0));
        // A stream slower than the display lets its queue run dry every refresh.
        assertFalse(DisplayQueueWatch.applies(true, true, false, true, false, fps60, REFRESH_NS / 2));
        assertTrue(DisplayQueueWatch.applies(true, true, false, true, false, 120_000, REFRESH_NS));
        assertTrue(DisplayQueueWatch.applies(true, true, false, true, false, 59_940, REFRESH_NS));
        assertFalse(DisplayQueueWatch.applies(true, true, false, true, false, 0, REFRESH_NS));
    }

    @Test
    public void withItsSettingOffTheWatchNeverRunsOrDrops() {
        // Take back display queue delay off: nothing else matters.
        assertFalse(DisplayQueueWatch.applies(false, true, false, true, false, 60_000, REFRESH_NS));
        // The decoder configures the watch with that answer: a frame too many waiting for minutes
        // and a frame replaced in the queue are neither dropped nor logged.
        Display display = new Display(false);
        display.show(12, 30 * MS);
        display.show(120, 48 * MS);
        display.show(60, 74 * MS);
        display.watch.onRendered(8 * MS, display.renderNs - 47 * MS);
        display.take();
        assertEquals(0, display.drops);
        assertTrue(display.events.toString(), display.events.isEmpty());
    }

    @Test
    public void aRefreshChangeOrAnotherModeStartsOver() {
        Display display = new Display();
        display.show(12, 30 * MS);
        display.watch.configure(true, REFRESH_NS / 2);
        display.take();
        assertTrue(display.said("starting over"));
        display.watch.configure(true, REFRESH_NS);
        // The level 30 ms was forgotten: 62 ms is a new start's level, at most probed, never a rise over 30.
        display.show(12, 62 * MS);
        assertFalse(display.events.toString(), display.said("over its lowest level"));
        assertTrue(display.drops <= 1);
        display.watch.configure(false, REFRESH_NS);
        assertFalse(display.watch.takeDrop(display.renderNs));
    }
}
