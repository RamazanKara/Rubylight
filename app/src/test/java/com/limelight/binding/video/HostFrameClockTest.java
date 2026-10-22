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
}
