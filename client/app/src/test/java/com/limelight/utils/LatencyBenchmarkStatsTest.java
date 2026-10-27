package com.limelight.utils;

import org.junit.Test;
import static org.junit.Assert.*;

public class LatencyBenchmarkStatsTest {
    @Test public void emptyMetricsAreUnavailableRatherThanZeroLatency() {
        assertArrayEquals(new double[] {0, -1, -1, -1},
                new LatencyBenchmarkStats().summary(LatencyBenchmarkStats.INPUT_AGE), 0);
    }

    @Test public void callbacksMeasureIntervalsAndDeliveryDelaySeparately() {
        LatencyBenchmarkStats stats = new LatencyBenchmarkStats();
        stats.frame(1_000_000, 3_000_000);
        stats.frame(11_000_000, 14_000_000);
        stats.frame(21_000_000, 25_000_000);
        assertArrayEquals(new double[] {2, 10, 10, 10}, stats.summary(0), 0);
        assertArrayEquals(new double[] {3, 3, 4, 4}, stats.summary(1), 0);
    }

    @Test public void duplicatesAndMixedClockSamplesCannotCorruptCadence() {
        LatencyBenchmarkStats stats = new LatencyBenchmarkStats();
        stats.frame(1_000_000, 2_000_000);
        stats.frame(1_000_000, 3_000_000);
        stats.frame(500_000, 3_000_000);
        stats.frame(10_000_000, 9_000_000);
        stats.frame(11_000_000, 12_000_000);
        stats.add(2, -1);
        assertArrayEquals(new double[] {1, 10, 10, 10}, stats.summary(0), 0);
        assertEquals(2, stats.summary(1)[0], 0);
        assertEquals(0, stats.summary(2)[0], 0);
    }

    @Test public void percentilesAndWindowEvictionUseOnlyActualRecentSamples() {
        LatencyBenchmarkStats stats = new LatencyBenchmarkStats();
        for (int i = 1; i <= 100; i++) stats.add(2, i * 1_000_000L);
        assertArrayEquals(new double[] {100, 50.5, 95, 99}, stats.summary(2), 0);
        for (int i = 0; i < LatencyBenchmarkStats.WINDOW_SIZE; i++) stats.add(2, 2_000_000);
        assertArrayEquals(new double[] {LatencyBenchmarkStats.WINDOW_SIZE, 2, 2, 2}, stats.summary(2), 0);
    }
}
