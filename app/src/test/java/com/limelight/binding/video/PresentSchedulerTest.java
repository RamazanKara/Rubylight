package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class PresentSchedulerTest {
    /**
     * The native clock's rule, as far as the scheduler depends on it (protocol/core/src/
     * present_clock.rs): a frame's present time is its host time plus a fixed delay on the client
     * clock, or its ready time when it is late; the vsync is the first at or after the present time
     * less the slack, never before the first one the frame can make; a frame whose vsync was taken
     * by the one before it moves one vsync later once, else it shares.
     */
    static final class GridModel implements HostFrameClock.Bridge {
        long offsetNs;
        long delayNs;
        long[] lastCall;
        int plain;
        long lastVsyncNs = Long.MIN_VALUE;
        boolean lastBumped;
        boolean bumped;
        @Override public void start(int minIntervalUs) { }
        @Override public void stop() { }
        @Override public void reset() { }
        @Override public boolean schedule(long hostUs, long readyNs, long[] out) {
            plain++;
            out[0] = readyNs;
            return true;
        }
        @Override public boolean stats(long[] out) { return false; }
        @Override public boolean offset(long[] out) { return false; }
        @Override public void probe() { }
        @Override public boolean scheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
                long leadNs, long slackNs, long[] out) {
            lastCall = new long[] {hostUs, readyNs, anchorNs, intervalNs, leadNs, slackNs};
            long presentNs = hostUs * 1000 + offsetNs + delayNs;
            boolean late = readyNs > presentNs;
            if (late) presentNs = readyNs;
            long first = ceilDiv(readyNs - anchorNs, intervalNs);
            long index = Math.max(first, ceilDiv(presentNs - slackNs - anchorNs, intervalNs));
            boolean shares = false;
            bumped = false;
            if (lastVsyncNs != Long.MIN_VALUE) {
                long taken = Math.floorDiv(lastVsyncNs - anchorNs + intervalNs / 2, intervalNs);
                if (index <= taken) {
                    if (taken == first && !lastBumped) {
                        index = first + 1;
                        bumped = true;
                    } else {
                        index = taken;
                        shares = true;
                    }
                }
            }
            long vsync = anchorNs + index * intervalNs;
            lastVsyncNs = vsync;
            lastBumped = bumped;
            long release = Math.max(vsync - leadNs, readyNs);
            out[0] = release;
            out[1] = vsync;
            out[2] = index;
            out[3] = (late ? 1 : 0) | (shares ? 2 : 0);
            out[4] = (release - readyNs) / 1000;
            out[5] = -1;
            out[6] = late ? (readyNs - (hostUs * 1000 + offsetNs + delayNs)) / 1000 : 0;
            return true;
        }
        @Override public void observe(long hostUs) { }
        @Override public int contentRate() { return 0; }
    }

    private static long ceilDiv(long a, long b) {
        return Math.floorDiv(a + b - 1, b);
    }

    private static final long DEADLINE_NS = 3_000_000L;

    private static final class Run {
        final GridModel model = new GridModel();
        final HostFrameClock clock = new HostFrameClock(model, 60_000);
        final PresentScheduler scheduler = new PresentScheduler(clock);
        final PresentScheduler.Plan plan = new PresentScheduler.Plan();
        long vsyncAnchorNs = 10_000_000_000L + 1_234_567L;
        long vsyncIntervalNs;
        int frames;
        long previousVsyncNs = Long.MIN_VALUE;
        int shared;
        int bumpedFrames;
        int bumpedInARow;
        int worstExtraVsyncs;

        Run(double panelHz, double contentFps) {
            this(panelHz, contentFps, DEADLINE_NS);
        }

        Run(double panelHz, double contentFps, long deadlineNs) {
            vsyncIntervalNs = (long) (1e9 / panelHz);
            clock.start((float) panelHz, true, false);
            long contentNs = (long) (1e9 / contentFps);
            // Each frame is ready 4 ms after the game presented it, and the clock's delay is 4.5 ms.
            model.delayNs = 4_500_000L;
            long hostBaseUs = 1_000_000L;
            long arrivalNs = 4_000_000L;
            // The clocks' offset puts host time 0 at 10.0005 s on the client.
            model.offsetNs = 10_000_500_000L - hostBaseUs * 1000;
            boolean previousBumped = false;
            for (int i = 0; i < 600; i++) {
                long hostUs = hostBaseUs + i * contentNs / 1000;
                long readyNs = hostUs * 1000 + model.offsetNs + arrivalNs;
                clock.onQueued(i + 1, hostUs);
                assertTrue(scheduler.plan(i + 1, readyNs, vsyncAnchorNs, vsyncIntervalNs, deadlineNs, plan));
                long first = ceilDiv(readyNs + deadlineNs - vsyncAnchorNs, vsyncIntervalNs);
                long index = (plan.vsyncNs - vsyncAnchorNs) / vsyncIntervalNs;
                assertEquals("on the grid", 0, (plan.vsyncNs - vsyncAnchorNs) % vsyncIntervalNs);
                assertTrue("never before the first vsync it can make", index >= first);
                worstExtraVsyncs = (int) Math.max(worstExtraVsyncs, index - first);
                assertTrue("due after the vsync before and no later than its own",
                        plan.releaseNs > plan.vsyncNs - vsyncIntervalNs && plan.releaseNs <= plan.vsyncNs);
                assertTrue("never released before it is ready", plan.releaseNs >= readyNs);
                assertTrue("vsyncs do not go back", plan.vsyncNs >= previousVsyncNs);
                if (plan.sharesVsync) shared++;
                else if (previousVsyncNs != Long.MIN_VALUE) assertTrue("distinct frames, distinct vsyncs", plan.vsyncNs > previousVsyncNs);
                boolean bumpedNow = model.bumped;
                if (bumpedNow) bumpedFrames++;
                if (bumpedNow && previousBumped) bumpedInARow++;
                previousBumped = bumpedNow;
                previousVsyncNs = plan.vsyncNs;
                frames++;
            }
        }
    }

    @Test public void sixtyFpsOnA60HzPanelTakesTheFirstVsyncEveryTime() {
        Run run = new Run(60, 60);
        assertEquals(600, run.frames);
        assertEquals("no frame moved past its first vsync", 0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
    }

    // The Retroid's panel reports presentation deadlines of 16.67 ms at 60 Hz and 11.33 ms at 120 Hz (a
    // buffer queued that long before a vsync is shown at it; SurfaceFlinger's own latency log agrees).
    // The plan must aim at the first vsync that deadline allows, never the one after it.
    @Test public void sixtyFpsOnA120HzPanelWithAnElevenMsDeadlineTakesTheFirstVsyncItCanMake() {
        Run run = new Run(120, 60, 11_333_333L);
        assertEquals("no frame moved past its first vsync", 0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
    }

    @Test public void sixtyFpsOnA60HzPanelWithAFullIntervalDeadlineTakesTheFirstVsyncItCanMake() {
        Run run = new Run(60, 60, 16_666_666L);
        assertEquals("no frame moved past its first vsync", 0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
    }

    @Test public void ninetyFpsOnA90HzPanelTakesTheFirstVsyncEveryTime() {
        Run run = new Run(90, 90);
        assertEquals(0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
    }

    @Test public void oneTwentyFpsOnA120HzPanelTakesTheFirstVsyncEveryTime() {
        Run run = new Run(120, 120);
        assertEquals(0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
    }

    @Test public void ninetyFpsOnA120HzPanelPullsDownWithoutSharingOrDelay() {
        Run run = new Run(120, 90);
        // Four vsyncs for three frames: the frames fall on vsyncs 1, 2 and 1 apart, none shared.
        assertEquals("no frame needs to wait for another", 0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
        assertEquals(0, run.bumpedFrames);
    }

    @Test public void thirtyFpsOnA60HzPanelKeepsTheGamesCadence() {
        Run run = new Run(60, 30);
        assertEquals(0, run.worstExtraVsyncs);
        assertEquals(0, run.shared);
    }

    @Test public void contentFasterThanThePanelMovesAFrameOneVsyncAtMostAndNeverTwiceInARow() {
        Run run = new Run(60, 120);
        assertTrue("some frames had to move", run.bumpedFrames > 0);
        assertEquals("one extra vsync is the most", 1, run.worstExtraVsyncs);
        assertEquals(0, run.bumpedInARow);
        assertTrue("and some had to share a vsync", run.shared > 0);
    }

    @Test public void contentFasterThanThePanelAt90On60KeepsTheBound() {
        Run run = new Run(60, 90);
        assertTrue(run.worstExtraVsyncs <= 1);
        assertEquals(0, run.bumpedInARow);
    }

    @Test public void theClockIsAskedForAGridOfTheDisplaysVsyncsLeadingByAQuarterAndReadyAtTheDeadline() {
        Run run = new Run(60, 60);
        long[] call = run.model.lastCall;
        assertEquals(run.vsyncAnchorNs, call[2]);
        assertEquals(run.vsyncIntervalNs, call[3]);
        assertEquals("due a quarter of a vsync before its vsync", run.vsyncIntervalNs / 4, call[4]);
        assertEquals("the nearest vsync: half a vsync of slack", run.vsyncIntervalNs / 2, call[5]);
        long hostUs = 1_000_000L + 599 * (long) (1e9 / 60) / 1000;
        long readyNs = hostUs * 1000 + run.model.offsetNs + 4_000_000L;
        assertEquals("a buffer queued the presentation deadline before a vsync is shown at it",
                readyNs + DEADLINE_NS, call[1]);
    }

    @Test public void aNegativeDeadlineIsTreatedAsNone() {
        Run run = new Run(60, 60);
        run.clock.onQueued(1000, 2_000_000L);
        assertTrue(run.scheduler.plan(1000, 20_000_000_000L, run.vsyncAnchorNs, run.vsyncIntervalNs, -5, run.plan));
        assertEquals(20_000_000_000L, run.model.lastCall[1]);
    }

    @Test public void thereIsNoPlanWithoutAVsyncAnIntervalOrAHostTime() {
        Run run = new Run(60, 60);
        run.clock.onQueued(5000, 3_000_000L);
        int calls = 0;
        run.model.lastCall = null;
        assertFalse("no vsync sample yet", run.scheduler.plan(5000, 1_000_000_000L, 0, run.vsyncIntervalNs, 0, run.plan));
        assertFalse("no interval", run.scheduler.plan(5000, 1_000_000_000L, run.vsyncAnchorNs, 0, 0, run.plan));
        assertFalse("unknown frame", run.scheduler.plan(9999, 1_000_000_000L, run.vsyncAnchorNs, run.vsyncIntervalNs, 0, run.plan));
        assertNull("the clock is never asked", run.model.lastCall);
    }

    @Test public void aFramePlanIsNotMadeWhenFramesAreNotScheduled() {
        GridModel model = new GridModel();
        HostFrameClock clock = new HostFrameClock(model, 60_000);
        PresentScheduler scheduler = new PresentScheduler(clock);
        clock.start(60, false, true);
        try {
            clock.onQueued(1, 1_000_000L);
            assertFalse(scheduler.plan(1, 1_000_000_000L, 5_000_000_000L, 16_666_667L, 0,
                    new PresentScheduler.Plan()));
        } finally {
            clock.stop();
        }
    }

    // --- late frames ---

    private static final long CONTENT_INTERVAL_NS = 11_111_111L;

    private static PresentScheduler warmScheduler(HostFrameClock clock, int frames) {
        PresentScheduler scheduler = new PresentScheduler(clock);
        for (int i = 0; i < frames; i++) {
            long hostUs = 5_000_000L + i * 11_111L;
            clock.onQueued(i + 1, hostUs);
            // Ready 3 ms after the host time, give or take a millisecond of jitter.
            long jitterUs = (i % 5) * 250L;
            assertFalse("a steady stream is never late", scheduler.isLate(i + 1,
                    (hostUs + 3_000 + jitterUs) * 1000, CONTENT_INTERVAL_NS));
        }
        return scheduler;
    }

    private HostFrameClock startedClock() {
        HostFrameClock clock = new HostFrameClock(new GridModel(), 60_000);
        clock.start(120, true, false);
        return clock;
    }

    @Test public void aFrameMoreThanHalfAnIntervalBehindTheGamesCadenceIsLate() {
        HostFrameClock clock = startedClock();
        PresentScheduler scheduler = warmScheduler(clock, 100);
        long hostUs = 5_000_000L + 100 * 11_111L;
        clock.onQueued(1000, hostUs);
        // 3 ms is normal; the delay is the 97th percentile (4 ms here) and half a millisecond.
        long edge = 3_000 + 1_000 + 500 + CONTENT_INTERVAL_NS / 2000;
        assertFalse("just inside", scheduler.isLate(1000, (hostUs + edge - 100) * 1000, CONTENT_INTERVAL_NS));
        clock.onQueued(1001, hostUs + 11_111);
        assertTrue("just outside", scheduler.isLate(1001, (hostUs + 11_111 + edge + 100) * 1000, CONTENT_INTERVAL_NS));
        clock.onQueued(1002, hostUs + 22_222);
        assertTrue("a long stall", scheduler.isLate(1002, (hostUs + 22_222 + 60_000) * 1000, CONTENT_INTERVAL_NS));
    }

    @Test public void noFrameIsLateWhileTheWindowFillsOrInTheWarmUp() {
        HostFrameClock clock = startedClock();
        PresentScheduler scheduler = new PresentScheduler(clock);
        for (int i = 0; i < PresentScheduler.MIN_SAMPLES; i++) {
            long hostUs = 5_000_000L + i * 1_000L;
            clock.onQueued(i + 1, hostUs);
            assertFalse("frame " + i, scheduler.isLate(i + 1, (hostUs + (i == 20 ? 80_000 : 3_000)) * 1000,
                    CONTENT_INTERVAL_NS));
        }
        // Enough frames, but only a few tens of ms of host time since the first.
        long hostUs = 5_000_000L + 40_000;
        clock.onQueued(500, hostUs);
        assertFalse("warm-up", scheduler.isLate(500, (hostUs + 80_000) * 1000, CONTENT_INTERVAL_NS));
    }

    @Test public void aFrameWithoutAHostTimeIsNeverLateAndLeavesTheWindowAlone() {
        HostFrameClock clock = startedClock();
        PresentScheduler scheduler = warmScheduler(clock, 100);
        assertFalse(scheduler.isLate(424242, 99_000_000_000L, CONTENT_INTERVAL_NS));
        long hostUs = 5_000_000L + 100 * 11_111L;
        clock.onQueued(1000, hostUs);
        assertTrue("the window was not poisoned", scheduler.isLate(1000, (hostUs + 60_000) * 1000, CONTENT_INTERVAL_NS));
    }

    @Test public void aRestartedVideoClockStartsTheWindowOver() {
        HostFrameClock clock = startedClock();
        PresentScheduler scheduler = warmScheduler(clock, 100);
        long hostUs = 900_000_000L;
        clock.onQueued(2000, hostUs);
        assertFalse("a new clock: nothing to compare with", scheduler.isLate(2000, (hostUs + 60_000) * 1000, CONTENT_INTERVAL_NS));
        scheduler.reset();
        clock.onQueued(2001, hostUs + 11_111);
        assertFalse("after a reset the window fills again", scheduler.isLate(2001, (hostUs + 80_000) * 1000, CONTENT_INTERVAL_NS));
    }

    @Test public void aLateFrameIsDroppedOnlyForANewerOneAndNeverTwiceInARow() {
        assertTrue(PresentScheduler.dropLate(true, true, false));
        assertFalse("the last frame is shown", PresentScheduler.dropLate(true, false, false));
        assertFalse("never two in a row", PresentScheduler.dropLate(true, true, true));
        assertFalse("an on-time frame stays", PresentScheduler.dropLate(false, true, false));
        assertFalse(PresentScheduler.dropLate(false, false, true));
    }

    @Test public void aDroppedFrameTeachesTheClockWithoutTakingAVsync() {
        GridModel model = new GridModel();
        HostFrameClock clock = new HostFrameClock(model, 60_000);
        PresentScheduler scheduler = new PresentScheduler(clock);
        clock.start(120, true, false);
        try {
            clock.onQueued(1, 2_000_000L);
            scheduler.discard(1, 7_000_000_000L);
            assertEquals(1, model.plain);
            assertNull("the grid state is untouched", model.lastCall);
            assertEquals(Long.MIN_VALUE, model.lastVsyncNs);
        } finally {
            clock.stop();
        }
    }

    @Test public void theOverlayLineCountsSharedVsyncsAndDrops() {
        Run run = new Run(60, 120);
        run.scheduler.onLateDropped();
        run.scheduler.onLateDropped();
        run.scheduler.onGovernorDropped();
        run.scheduler.onCapDropped();
        run.scheduler.onCapHeld();
        String line = run.scheduler.overlayLine();
        assertTrue(line, line.contains("late dropped 2"));
        assertTrue(line, line.contains("queue dropped 2"));
        assertTrue(line, line.contains("compositor held 1"));
        assertEquals(2, run.scheduler.lateDropped());
        assertNull(new PresentScheduler(run.clock).overlayLine());
    }
}
