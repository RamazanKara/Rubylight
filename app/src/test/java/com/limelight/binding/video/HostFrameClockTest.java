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
}
