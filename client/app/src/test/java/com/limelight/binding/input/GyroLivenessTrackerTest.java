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
    public void oneMovingSampleMeansTheGyroscopeIsRealForGood() {
        // Noise before the run is complete, then zeros for far longer than the run: no trip.
        GyroLivenessTracker tracker = new GyroLivenessTracker(10);
        assertFalse(tracker.onSample(0f, 0f, 0f, 1));
        assertFalse(tracker.onSample(0.000001f, 0f, 0f, 5));
        for (long t = 6; t <= 1000; t++) {
            assertFalse(tracker.onSample(0f, 0f, 0f, t));
        }
        // Motion first, then a controller at rest that the driver rounds to zero: no trip.
        GyroLivenessTracker resting = new GyroLivenessTracker(10);
        assertFalse(resting.onSample(0f, 0f, -0.2f, 1));
        for (long t = 2; t <= 1000; t++) {
            assertFalse(resting.onSample(0f, 0f, 0f, t));
        }
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
