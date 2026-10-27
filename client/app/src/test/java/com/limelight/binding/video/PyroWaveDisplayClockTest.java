package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

public class PyroWaveDisplayClockTest {
    private static final long START_NS = 1_000_000_000L;

    @Test public void waitsForMeasuredIntervalsInsteadOfPublishingAdvertisedRate() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        clock.reset(8_333_333L);
        long measuredInterval = 8_341_675L; // 119.88 Hz, advertised as 120 Hz.
        for (int frame = 0; frame < 3; frame++) {
            clock.observe(START_NS + frame * measuredInterval);
            assertSame(PyroWaveDisplayClock.Snapshot.UNAVAILABLE, clock.snapshot());
        }
        clock.observe(START_NS + 3 * measuredInterval);
        assertEquals(measuredInterval, clock.snapshot().intervalNs);
        assertEquals(START_NS + 3 * measuredInterval, clock.snapshot().vsyncNs);
    }

    @Test public void skippedCallbacksMeasureOneRefreshInsteadOfSlowingTheClock() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        long interval = 16_683_350L;
        clock.reset(16_666_666L);
        clock.observe(START_NS);
        clock.observe(START_NS + 2 * interval);
        clock.observe(START_NS + 5 * interval);
        clock.observe(START_NS + 13 * interval);
        assertEquals(interval, clock.snapshot().intervalNs);
        assertEquals(START_NS + 13 * interval, clock.snapshot().vsyncNs);
    }

    @Test public void medianRejectsIsolatedJitterAndLongPauseKeepsMeasuredPeriod() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        long interval = 10_000_000L;
        clock.reset(interval);
        long time = START_NS;
        clock.observe(time);
        for (int frame = 0; frame < 3; frame++) clock.observe(time += interval);
        clock.observe(time += interval + 300_000);
        clock.observe(time += interval - 300_000);
        clock.observe(time += interval + 4_000_000); // Not a plausible refresh multiple.
        assertEquals(interval, clock.snapshot().intervalNs);
        clock.observe(time += 1_000_000_000L);
        assertEquals(interval, clock.snapshot().intervalNs);
        assertEquals(time, clock.snapshot().vsyncNs);
    }

    @Test public void sourceTimestampWinsOverDuplicateOrOutOfOrderCallbacks() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        clock.reset(10_000_000L);
        for (int frame = 0; frame < 4; frame++) clock.observe(START_NS + frame * 10_000_000L);
        PyroWaveDisplayClock.Snapshot before = clock.snapshot();
        clock.observe(before.vsyncNs);
        clock.observe(before.vsyncNs - 10_000_000L);
        assertSame(before, clock.snapshot());
        clock.observe(before.vsyncNs + 10_000_000L);
        assertEquals(before.vsyncNs + 10_000_000L, clock.snapshot().vsyncNs);
        assertEquals(10_000_000L, clock.snapshot().intervalNs);
        // A submission which already copied the snapshot cannot observe a partial update.
        assertEquals(START_NS + 30_000_000L, before.vsyncNs);
    }

    @Test public void displayModeChangeDiscardsTheOldPhaseAndSampleHistory() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        clock.reset(10_000_000L);
        for (int frame = 0; frame < 8; frame++) clock.observe(START_NS + frame * 10_000_000L);
        clock.reset(16_666_666L);
        assertSame(PyroWaveDisplayClock.Snapshot.UNAVAILABLE, clock.snapshot());
        for (int frame = 0; frame < 3; frame++) {
            clock.observe(2 * START_NS + frame * 16_683_350L);
            assertSame(PyroWaveDisplayClock.Snapshot.UNAVAILABLE, clock.snapshot());
        }
        clock.observe(2 * START_NS + 3 * 16_683_350L);
        assertEquals(16_683_350L, clock.snapshot().intervalNs);
    }

    @Test public void absentOrInvalidDisplayModeDoesNotProduceAClock() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        for (long interval : new long[] {0, -1}) {
            clock.reset(interval);
            for (int frame = 0; frame < 8; frame++) clock.observe(START_NS + frame * 10_000_000L);
            assertSame(PyroWaveDisplayClock.Snapshot.UNAVAILABLE, clock.snapshot());
        }
    }
}
