package com.limelight.binding.video;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;

import java.util.ArrayDeque;

import static org.junit.Assert.*;

public class ScheduledReleaseTest {
    private static final long CONTENT_NS = 11_111_111L;
    private static final long PANEL_NS = 8_333_333L;
    private static final long DEADLINE_NS = 3_000_000L;
    private static final long HOST_BASE_US = 5_000_000L;
    private static final long MS = 1_000_000L;

    /** The codec, the clock and the display, with a clock that moves only when the test says so. */
    private final class FakePort implements ScheduledRelease.Port {
        long now = (HOST_BASE_US + 1_131_111L) * 1000;
        boolean grid = true;
        long heldPts;
        final ArrayDeque<Long> newer = new ArrayDeque<>();
        long attemptCostNs;
        int attempts;
        long lastBasis = Long.MIN_VALUE;
        // What release() was given.
        int released;
        long releasedPts;
        long releaseNs, dueNs, holdNs;
        boolean planned, late;
        int inFlightCount = -1;
        long fallbackBasisNs = Long.MIN_VALUE;
        long timingNow;
        int timingReads;

        @Override public long nanoTime() { return now; }

        @Override public void timing(ScheduledRelease.Timing out) {
            timingReads++;
            timingNow = now;
            out.grid = grid;
            // The last vsync the display has had by now.
            out.vsyncNs = now - now % PANEL_NS;
            out.intervalNs = PANEL_NS;
            out.deadlineNs = DEADLINE_NS;
        }

        @Override public long heldPtsUs() { return heldPts; }

        @Override public boolean replaceHeld(String reason, long basisNs) {
            attempts++;
            lastBasis = basisNs;
            now += attemptCostNs;
            if (newer.isEmpty()) return false;
            heldPts = newer.poll();
            return true;
        }

        @Override public long fallbackPresentNs(long codecPtsUs, long basisNs) {
            fallbackBasisNs = basisNs;
            return basisNs + 1000;
        }

        @Override public void release(long releaseNs, long dueNs, boolean planned, long holdNs, boolean late, int inFlightCount) {
            released++;
            releasedPts = heldPts;
            this.releaseNs = releaseNs;
            this.dueNs = dueNs;
            this.planned = planned;
            this.holdNs = holdNs;
            this.late = late;
            this.inFlightCount = inFlightCount;
        }
    }

    private PresentSchedulerTest.GridModel model;
    private HostFrameClock clock;
    private PresentScheduler scheduler;
    private DecoderQueuePolicy.InFlight inFlight;
    private FakePort port;
    private ScheduledRelease release;

    @Before public void setUp() {
        model = new PresentSchedulerTest.GridModel();
        clock = new HostFrameClock(model, 60_000);
        clock.start(120, true, false);
        scheduler = new PresentScheduler(clock);
        inFlight = new DecoderQueuePolicy.InFlight();
        port = new FakePort();
        release = new ScheduledRelease(scheduler, inFlight, port);
        // A steady stream, so the lateness window knows the usual delay (4 ms).
        for (int i = 0; i < 100; i++) {
            long hostUs = HOST_BASE_US + i * 11_111L;
            clock.onQueued(i + 1, hostUs);
            scheduler.isLate(i + 1, (hostUs + 3_000 + (i % 5) * 250L) * 1000, CONTENT_NS);
        }
    }

    @After public void tearDown() {
        clock.stop();
    }

    /** Declares frame pts, whose host time is such that it is ready delayUs after it. */
    private long queue(long pts, long delayUs) {
        long hostUs = port.now / 1000 - delayUs;
        clock.onQueued(pts, hostUs);
        return hostUs;
    }

    @Test public void anOnTimeFrameIsPlannedOnTheGridAndReleasedWithoutAnAttemptToReplaceIt() {
        queue(1000, 3_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        assertEquals(1000, release.run(false, CONTENT_NS, true));
        assertEquals(0, port.attempts);
        assertEquals(1, port.released);
        assertTrue(port.planned);
        assertFalse(port.late);
        assertEquals(0, port.dueNs % PANEL_NS);
        assertTrue(port.releaseNs >= port.now);
        assertEquals(1, port.newer.size());
    }

    @Test public void aLateFrameGivesWayToANewerOutput() {
        queue(1000, 60_000);
        queue(2000, 3_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        assertEquals(2000, release.run(false, CONTENT_NS, true));
        assertEquals(1, port.attempts);
        assertEquals(1, scheduler.lateDropped());
        assertEquals(2000, port.releasedPts);
        assertEquals(1, port.released);
    }

    @Test public void noSecondDropAfterAFrameWasDroppedAlready() {
        queue(1000, 60_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        assertEquals(1000, release.run(true, CONTENT_NS, true));
        assertEquals("not even asked", 0, port.attempts);
        assertEquals(0, scheduler.lateDropped());
        assertTrue(port.late);
        assertEquals(1, port.newer.size());
    }

    @Test public void twoLateFramesInARunAreNotBothDropped() {
        queue(1000, 60_000);
        queue(2000, 60_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        port.newer.add(3000L);
        assertEquals("the second one is shown", 2000, release.run(false, CONTENT_NS, true));
        assertEquals(1, scheduler.lateDropped());
        assertEquals(1, port.newer.size());
    }

    @Test public void theLastFrameIsNeverDroppedHoweverLate() {
        queue(1000, 80_000);
        port.heldPts = 1000;
        assertEquals(1000, release.run(false, CONTENT_NS, true));
        assertEquals(1, port.attempts);
        assertEquals(0, scheduler.lateDropped());
        assertEquals(1, port.released);
        assertTrue(port.late);
    }

    @Test public void aFullCompositorQueueReleasesAtOnceWhenNothingIsNewer() {
        queue(1000, 3_000);
        port.heldPts = 1000;
        inFlight.onReleased(10, port.now + 10 * MS);
        inFlight.onReleased(11, port.now + 18 * MS);
        long before = port.now;
        assertEquals(1000, release.run(false, CONTENT_NS, true));
        assertEquals("never waits", before, port.now);
        assertEquals(1, scheduler.capHeld());
        assertEquals(0, scheduler.capDropped());
        assertEquals(1, port.released);
        assertEquals(2, port.inFlightCount);
        assertTrue(port.releaseNs >= port.now);
    }

    @Test public void aFullCompositorQueueGivesWayToANewerOutputOnce() {
        queue(1000, 3_000);
        queue(2000, 3_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        port.newer.add(3000L);
        inFlight.onReleased(10, port.now + 10 * MS);
        inFlight.onReleased(11, port.now + 18 * MS);
        assertEquals(2000, release.run(false, CONTENT_NS, true));
        assertEquals(1, scheduler.capDropped());
        assertEquals("never two in a row", 1, port.newer.size());
    }

    @Test public void aFullQueueIsNotDroppedForAfterADrop() {
        queue(1000, 3_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        inFlight.onReleased(10, port.now + 10 * MS);
        inFlight.onReleased(11, port.now + 18 * MS);
        assertEquals(1000, release.run(true, CONTENT_NS, true));
        assertEquals(0, port.attempts);
        assertEquals(1, scheduler.capHeld());
    }

    @Test public void theCapExpiresByTimeWhenNoRenderCallbacksArrive() {
        queue(1000, 3_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        inFlight.onReleased(10, port.now + 10 * MS);
        inFlight.onReleased(11, port.now + 18 * MS);
        port.now += 30 * MS;
        queue(1000, 3_000);
        assertEquals(1000, release.run(false, CONTENT_NS, true));
        assertEquals("their vsyncs have passed", 0, scheduler.capHeld());
        assertEquals(0, port.attempts);
        assertEquals(0, port.inFlightCount);
    }

    @Test public void theCapIsOffWithoutRenderCallbacks() {
        queue(1000, 3_000);
        port.heldPts = 1000;
        port.newer.add(2000L);
        inFlight.onReleased(10, port.now + 10 * MS);
        inFlight.onReleased(11, port.now + 18 * MS);
        assertEquals(1000, release.run(false, CONTENT_NS, false));
        assertEquals(0, scheduler.capHeld());
        assertEquals(0, port.attempts);
    }

    @Test public void theFrameIsPlannedFromTheTimeAndVsyncAtItsReleaseNotFromBeforeTheAttempt() {
        queue(1000, 60_000);
        port.heldPts = 1000;
        // Asking for a newer output takes 5 ms (and there is none).
        port.attemptCostNs = 5 * MS;
        long start = port.now;
        assertEquals(1000, release.run(false, CONTENT_NS, true));
        assertEquals(start + 5 * MS, port.now);
        assertEquals("the clock was asked at the later time", port.now + DEADLINE_NS, model.lastCall[1]);
        assertEquals("with the later vsync", port.now - port.now % PANEL_NS, model.lastCall[2]);
        assertTrue("never in the past", port.releaseNs >= port.now);
        assertTrue(port.dueNs >= port.now);
        assertEquals("the hold is from the time it is released", port.releaseNs - port.now, port.holdNs);
        assertEquals("the snapshot was read again", port.now, port.timingNow);
    }

    @Test public void theFallbackKeepsTheGridBasisSoEveryFrameIsJudgedTheSameWay() {
        port.heldPts = 424242;   // no host time: the clock cannot plan it
        assertEquals(424242, release.run(false, CONTENT_NS, true));
        assertFalse(port.planned);
        assertEquals(port.now + DEADLINE_NS, port.fallbackBasisNs);
        assertEquals(DEADLINE_NS + 1000, port.holdNs);
    }

    @Test public void withoutAGridTheFallbackUsesTheReadyTime() {
        queue(1000, 3_000);
        port.grid = false;
        port.heldPts = 1000;
        assertEquals(1000, release.run(false, CONTENT_NS, true));
        assertFalse(port.planned);
        assertEquals(port.now, port.fallbackBasisNs);
        assertEquals(0, port.dueNs - port.releaseNs);
    }
}
