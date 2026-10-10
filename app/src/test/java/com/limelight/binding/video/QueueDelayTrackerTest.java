package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class QueueDelayTrackerTest {
    @Test
    public void delayIsMeasuredAboveTheClockOffset() {
        QueueDelayTracker tracker = new QueueDelayTracker();
        assertEquals(-1, tracker.takeWindowMs(), 0);
        // Host clock about 5 s ahead; the third frame waits 6 ms longer than the first two.
        tracker.onFrame(1_000, 5_000_000 - 1_000);
        tracker.onFrame(17_000, 5_016_000 - 1_000);
        tracker.onFrame(41_000, 5_033_000);
        assertEquals(2, tracker.takeWindowMs(), 0.001);
    }

    @Test
    public void minimumCarriesAcrossWindowsSoAStandingQueueShows() {
        QueueDelayTracker tracker = new QueueDelayTracker();
        tracker.onFrame(1_000, 1_000);
        tracker.takeWindowMs();
        tracker.onFrame(2_020_000, 2_000_000);
        tracker.onFrame(2_040_000, 2_020_000);
        assertEquals(20, tracker.takeWindowMs(), 0.001);
    }

    @Test
    public void oldMinimumsAgeOutAfterTenWindows() {
        QueueDelayTracker tracker = new QueueDelayTracker();
        tracker.onFrame(1_000, 1_000);
        tracker.takeWindowMs();
        for (int i = 0; i < 10; i++) {
            tracker.onFrame(10_000_000 + i, 10_000_000 + i - 5_000);
            tracker.takeWindowMs();
        }
        tracker.onFrame(20_000_000, 20_000_000 - 5_000);
        assertEquals(0, tracker.takeWindowMs(), 0.001);
    }

    @Test
    public void missingTimestampsAreIgnored() {
        QueueDelayTracker tracker = new QueueDelayTracker();
        tracker.onFrame(0, 1_000);
        tracker.onFrame(1_000, 0);
        assertEquals(-1, tracker.takeWindowMs(), 0);
    }
}
