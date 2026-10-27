package com.limelight.utils;

import org.junit.Test;
import static org.junit.Assert.*;

public class ConnectionTestStatsTest {
    @Test
    public void computesMeanLatencyAndAdjacentVariation() {
        ConnectionTestStats stats = new ConnectionTestStats();
        stats.add(10);
        stats.add(20);
        stats.add(15);
        assertEquals(15, stats.latencyMs(), 0.001);
        assertEquals(7.5, stats.jitterMs(), 0.001);
        assertEquals(0, stats.lossPercent(), 0);
    }

    @Test
    public void failedProbesCountAsLossAndBreakTheJitterSequence() {
        ConnectionTestStats stats = new ConnectionTestStats();
        stats.add(10);
        stats.add(Double.NaN);
        stats.add(100);
        stats.add(110);
        assertEquals(25, stats.lossPercent(), 0);
        assertEquals(10, stats.jitterMs(), 0);
        assertEquals(220.0 / 3, stats.latencyMs(), 0.001);
    }

    @Test
    public void noSuccessfulSamplesAreUnavailableNotZeroLatency() {
        ConnectionTestStats stats = new ConnectionTestStats();
        assertTrue(Double.isNaN(stats.latencyMs()));
        assertTrue(Double.isNaN(stats.jitterMs()));
        assertTrue(Double.isNaN(stats.lossPercent()));
        stats.add(-1);
        stats.add(Double.POSITIVE_INFINITY);
        assertEquals(100, stats.lossPercent(), 0);
        assertTrue(Double.isNaN(stats.latencyMs()));
        stats.add(0);
        assertEquals(0, stats.latencyMs(), 0);
        assertTrue(Double.isNaN(stats.jitterMs()));
    }
}
