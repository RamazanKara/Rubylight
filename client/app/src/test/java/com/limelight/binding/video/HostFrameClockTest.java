package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class HostFrameClockTest {
    @Test public void decodedFramesFindTheHostTimestampTheyWereQueuedWith() {
        HostFrameClock clock = new HostFrameClock();
        assertEquals("nothing queued yet, not even codec timestamp 0", -1, clock.hostUsFor(0));
        clock.onQueued(1_000, 90_000_000L);
        clock.onQueued(1_011, 90_011_111L);
        assertEquals(90_000_000L, clock.hostUsFor(1_000));
        assertEquals(90_011_111L, clock.hostUsFor(1_011));
        assertEquals(-1, clock.hostUsFor(1_005));
    }

    @Test public void onlyTheNewestPendingFramesAreKept() {
        HostFrameClock clock = new HostFrameClock();
        for (int i = 0; i < HostFrameClock.PENDING + 10; i++) clock.onQueued(i, 1_000_000L + i);
        assertEquals("overwritten by newer frames", -1, clock.hostUsFor(5));
        assertEquals(1_000_010L, clock.hostUsFor(10));
        assertEquals(1_000_000L + HostFrameClock.PENDING + 9, clock.hostUsFor(HostFrameClock.PENDING + 9));
    }

    private static final class FakeBridge implements HostFrameClock.Bridge {
        final java.util.concurrent.CountDownLatch probes = new java.util.concurrent.CountDownLatch(2);
        volatile int starts, schedules, stops;
        @Override public void start(int minIntervalUs) { starts++; }
        @Override public void stop() { stops++; }
        @Override public void reset() { }
        @Override public boolean schedule(long hostUs, long readyNs, long[] out) {
            schedules++;
            out[0] = readyNs + 5_000_000L;
            out[1] = 5_000;
            out[2] = -1;
            return true;
        }
        @Override public boolean stats(long[] out) { return false; }
        @Override public boolean offset(long[] out) { out[0] = -3955; out[1] = 90; return true; }
        @Override public void probe() { probes.countDown(); }
        long[] lastGridCall;
        volatile int gridCalls;
        final java.util.List<Long> observed = new java.util.concurrent.CopyOnWriteArrayList<>();
        volatile int rate;
        @Override public boolean scheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
                long leadNs, long slackNs, long[] out) {
            gridCalls++;
            lastGridCall = new long[] {hostUs, readyNs, anchorNs, intervalNs, leadNs, slackNs};
            out[0] = readyNs + 2_000_000L;
            out[1] = anchorNs + 3 * intervalNs;
            out[2] = 3;
            out[3] = 3; // late and shares
            out[4] = 2_000;
            out[5] = -1;
            out[6] = 7_500;
            return true;
        }
        @Override public void observe(long hostUs) { observed.add(hostUs); }
        @Override public int contentRate() { return rate; }
    }

    @Test public void clockSyncProbesRunWithHostFrameTimesOffAndFramesAreNotScheduled() throws Exception {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 5);
        clock.start(120, false, true);
        try {
            assertTrue("probes must not wait for host frame times", bridge.probes.await(5, java.util.concurrent.TimeUnit.SECONDS));
            assertEquals(1, bridge.starts);
            clock.onQueued(1_000, 90_000_000L);
            assertEquals("ready time unchanged", 123_456_789L, clock.presentNs(1_000, 123_456_789L));
            assertEquals("the schedule is never asked", 0, bridge.schedules);
            assertNull(clock.overlayLine());
            long[] offset = new long[2];
            assertTrue(clock.clockOffset(offset));
            assertArrayEquals(new long[] {-3955, 90}, offset);
        } finally {
            clock.stop();
        }
        assertEquals(1, bridge.stops);
        assertFalse("no offset once the clock is stopped", clock.clockOffset(new long[2]));
        clock.stop();
        assertEquals("a second stop does nothing", 1, bridge.stops);
    }

    @Test public void aHostThatDidNotAnnounceClockSyncGetsNoProbes() throws Exception {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 5);
        clock.start(120, true, false);
        try {
            assertFalse("no probes", bridge.probes.await(200, java.util.concurrent.TimeUnit.MILLISECONDS));
            assertEquals(2, bridge.probes.getCount());
            clock.onQueued(1_000, 90_000_000L);
            assertEquals("scheduling still works", 5_000_000L + 10, clock.presentNs(1_000, 10));
        } finally {
            clock.stop();
        }
        // Nothing scheduled and nothing to probe: the native clock is never started.
        FakeBridge idle = new FakeBridge();
        HostFrameClock off = new HostFrameClock(idle, 5);
        off.start(120, false, false);
        off.stop();
        assertEquals(0, idle.starts);
        assertEquals(0, idle.stops);
        assertFalse(off.clockOffset(new long[2]));
    }

    @Test public void aClockThatNeverStartedAnswersNoOffsetAndStopsQuietly() {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 5);
        assertFalse(clock.clockOffset(new long[2]));
        clock.stop();
        assertEquals(0, bridge.stops);
    }

    @Test public void withHostFrameTimesOnQueuedFramesAreScheduled() {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 60_000);
        clock.start(120, true, true);
        try {
            clock.onQueued(1_000, 90_000_000L);
            assertEquals(123_456_789L + 5_000_000L, clock.presentNs(1_000, 123_456_789L));
            assertEquals(1, bridge.schedules);
            assertEquals("an unknown frame is shown when ready", 77L, clock.presentNs(1_005, 77L));
        } finally {
            clock.stop();
        }
    }

    @Test public void aFrameIsPlannedOntoTheVsyncGridWithTheArgumentsGiven() {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 60_000);
        clock.start(60, true, false);
        try {
            clock.onQueued(1_000, 90_000_000L);
            HostFrameClock.GridPlan plan = new HostFrameClock.GridPlan();
            assertTrue(clock.presentOnGrid(1_000, 5_000_000_000L, 4_990_000_000L, 16_666_667L, 4_000_000L,
                    8_000_000L, plan));
            assertArrayEquals(new long[] {90_000_000L, 5_000_000_000L, 4_990_000_000L, 16_666_667L, 4_000_000L,
                    8_000_000L}, bridge.lastGridCall);
            assertEquals(5_002_000_000L, plan.releaseNs);
            assertEquals(4_990_000_000L + 3 * 16_666_667L, plan.vsyncNs);
            assertEquals(3, plan.vsyncIndex);
            assertTrue(plan.late);
            assertTrue(plan.sharesVsync);
            assertEquals(2_000, plan.holdUs);
            assertEquals(-1, plan.latencyUs);
            assertEquals(7_500, plan.lateByUs);
        } finally {
            clock.stop();
        }
    }

    @Test public void noGridPlanWithoutSchedulingAnIntervalOrAHostTime() {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 60_000);
        HostFrameClock.GridPlan plan = new HostFrameClock.GridPlan();
        clock.start(60, false, true);
        try {
            clock.onQueued(1_000, 90_000_000L);
            assertFalse("clock sync only: frames are not scheduled",
                    clock.presentOnGrid(1_000, 1_000_000L, 0, 16_666_667L, 0, 0, plan));
        } finally {
            clock.stop();
        }
        clock.start(60, true, false);
        try {
            clock.onQueued(1_000, 90_000_000L);
            assertFalse("no interval", clock.presentOnGrid(1_000, 1_000_000L, 0, 0, 0, 0, plan));
            assertFalse("unknown frame", clock.presentOnGrid(1_005, 1_000_000L, 0, 16_666_667L, 0, 0, plan));
            assertEquals("the native side is never asked", 0, bridge.gridCalls);
        } finally {
            clock.stop();
        }
    }

    @Test public void theContentRateIsTrackedOnItsOwnAndReadFromTheNativeSide() {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 60_000);
        bridge.rate = 90_000;
        assertEquals("not running", 0, clock.contentRateMilliHz());
        clock.start(120, false, false, true);
        try {
            assertEquals("the native clock runs for the rate alone", 1, bridge.starts);
            clock.onQueued(1_000, 90_000_000L);
            clock.onQueued(1_001, 90_011_111L);
            assertEquals(java.util.Arrays.asList(90_000_000L, 90_011_111L), bridge.observed);
            assertEquals(90_000, clock.contentRateMilliHz());
            assertEquals("frames are not scheduled", 5L, clock.presentNs(1_000, 5L));
            assertEquals(0, bridge.schedules);
        } finally {
            clock.stop();
        }
        assertEquals(0, clock.contentRateMilliHz());
        assertEquals(1, bridge.stops);
    }

    @Test public void framesAreNotObservedUnlessTheRateIsTracked() {
        FakeBridge bridge = new FakeBridge();
        HostFrameClock clock = new HostFrameClock(bridge, 60_000);
        bridge.rate = 60_000;
        clock.start(120, true, false);
        try {
            clock.onQueued(1_000, 90_000_000L);
            assertTrue(bridge.observed.isEmpty());
            assertEquals(0, clock.contentRateMilliHz());
        } finally {
            clock.stop();
        }
    }
}
