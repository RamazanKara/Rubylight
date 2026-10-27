package com.limelight.binding.input;

import org.junit.Test;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class GyroLivenessTrackerTest {
    @Test
    public void aFullRunOfZerosTripsOnce() {
        GyroLivenessTracker tracker = new GyroLivenessTracker(10);
        assertFalse(tracker.onSample(0f, 0f, 0f, 1));
        assertFalse(tracker.onSample(0f, 0f, 0f, 10));
        assertTrue(tracker.onSample(0f, 0f, 0f, 11));
        assertFalse(tracker.onSample(0f, 0f, 0f, 12));
        assertFalse(tracker.onSample(0f, 0f, 0f, 100));
    }

    @Test
    public void movementAndNoiseRestartTheRun() {
        GyroLivenessTracker tracker = new GyroLivenessTracker(10);
        assertFalse(tracker.onSample(0f, 0f, 0f, 1));
        assertFalse(tracker.onSample(0.000001f, 0f, 0f, 5));
        assertFalse(tracker.onSample(0f, 0f, 0f, 6));
        assertFalse(tracker.onSample(0f, 0f, -0.2f, 15));
        assertFalse(tracker.onSample(0f, 0f, 0f, 16));
        assertFalse(tracker.onSample(0f, 0f, 0f, 25));
        assertTrue(tracker.onSample(0f, 0f, 0f, 26));
    }

    @Test
    public void aTimestampGoingBackRestartsTheRun() {
        GyroLivenessTracker tracker = new GyroLivenessTracker(10);
        assertFalse(tracker.onSample(0f, 0f, 0f, 20));
        assertFalse(tracker.onSample(0f, 0f, 0f, 15));
        assertFalse(tracker.onSample(0f, 0f, 0f, 24));
        assertTrue(tracker.onSample(0f, 0f, 0f, 25));
    }

    @Test
    public void theDefaultWaitsASecond() {
        GyroLivenessTracker tracker = new GyroLivenessTracker();
        long start = 5_000_000_000L;
        assertFalse(tracker.onSample(0f, 0f, 0f, start));
        assertFalse(tracker.onSample(0f, 0f, 0f, start + 999_999_999L));
        assertTrue(tracker.onSample(0f, 0f, 0f, start + 1_000_000_000L));
    }
}
