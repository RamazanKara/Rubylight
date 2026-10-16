package com.limelight.nvstream;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public class PyroWavePaceTest {
    private static final int FPS = 120;

    /** A complete probe of 24000 packets; drop removes these sequence numbers. */
    private static PyroWavePace.Result probe(long group, long... drop) {
        PyroWavePace.Result result = new PyroWavePace.Result();
        result.expected = 24000;
        result.sent = 24000;
        result.senderMs = 2000;
        boolean[] arrived = new boolean[24000];
        Arrays.fill(arrived, true);
        for (long seq : drop) {
            arrived[(int) seq] = false;
        }
        PyroWavePace.summarize(result, arrived, FPS, group);
        return result;
    }

    @Test
    public void lossFreeProbeQualifies() {
        PyroWavePace.Result result = probe(10);
        assertEquals(240, result.frames);
        assertEquals(24000, result.received);
        assertEquals(0, result.lossPercent, 0);
        assertTrue(PyroWavePace.qualified(result));
    }

    @Test
    public void lostFrameTailsFailEvenAtLowTotalLoss() {
        // Each frame is packets [100 f, 100 f + 100); its last 1 ms group of 10 starts at 100 f + 90.
        long[] drop = new long[24];
        for (int i = 0; i < drop.length; i++) {
            drop[i] = 100L * i * 10 + 99;
        }
        PyroWavePace.Result result = probe(10, drop);
        assertEquals(24, result.missingTailFrames);
        assertTrue(result.lossPercent < 2.0);
        assertFalse(PyroWavePace.qualified(result));
    }

    @Test
    public void interiorHolesWithinLimitsStillQualify() {
        PyroWavePace.Result result = probe(10, 5, 1005, 2005);
        assertEquals(3, result.damagedFrames);
        assertEquals(0, result.missingTailFrames);
        assertTrue(PyroWavePace.qualified(result));
    }

    @Test
    public void probeTheHostCutShortIsNotMeasured() {
        PyroWavePace.Result result = probe(10);
        result.senderMs = 1500;
        assertFalse(PyroWavePace.measured(result));
        result.senderMs = 2000;
        result.sent = 23000;
        assertFalse(PyroWavePace.measured(result));
    }

    private static PyroWavePace.Probe thresholdProbe(int passAtOrBelowKbps, List<Integer> tested) {
        return pace -> {
            tested.add(pace);
            return pace <= passAtOrBelowKbps ? probe(10) : probe(10, 99, 199, 299, 399, 499, 599, 699);
        };
    }

    @Test
    public void linkThatDeliversIsConfirmedTwiceAndKept() throws Exception {
        List<Integer> tested = new ArrayList<>();
        PyroWavePace.Outcome outcome = PyroWavePace.search(2500000, 300000, thresholdProbe(Integer.MAX_VALUE, tested), () -> false);
        assertEquals(2500000, outcome.paceKbps);
        assertTrue(outcome.qualified);
        assertEquals(Arrays.asList(2500000, 2500000, 2500000), tested);
    }

    @Test
    public void searchKeepsAFivePercentMarginBelowTheFastestPass() throws Exception {
        List<Integer> tested = new ArrayList<>();
        PyroWavePace.Outcome outcome = PyroWavePace.search(2500000, 300000, thresholdProbe(2200000, tested), () -> false);
        // Bisection finds 2200 Mbps; 95% of it rounds down to the 2050 Mbps step, confirmed twice.
        assertEquals(2050000, outcome.paceKbps);
        assertTrue(outcome.qualified);
    }

    @Test
    public void nothingPassingFallsBackToTheBestMeasurementUnqualified() throws Exception {
        PyroWavePace.Outcome outcome = PyroWavePace.search(500000, 100000,
                pace -> probe(10, 99, 199, 299, 399, 499, 599, 699), () -> false);
        assertTrue(outcome.paceKbps > 0);
        assertFalse(outcome.qualified);
    }

    @Test
    public void cancellationReturnsNoPace() throws Exception {
        PyroWavePace.Outcome outcome = PyroWavePace.search(2500000, 300000, pace -> probe(10), () -> true);
        assertEquals(0, outcome.paceKbps);
    }

    @Test
    public void linkSlowerThanOneStepCannotBeCalibrated() throws Exception {
        assertEquals(0, PyroWavePace.search(40000, 10000, pace -> probe(10), () -> false).paceKbps);
    }

    @Test
    public void probeRateCountsFecHeadersAndAudio() {
        int wire = PyroWavePace.wireKbps(1000000, 120, 1392, 20, 2);
        assertTrue(wire > 1000000);
        assertTrue(wire < 1300000);
        // Even a tiny stream carries its parity shards, audio and control.
        assertEquals(PyroWavePace.wireKbps(500, 60, 1392, 20, 2), PyroWavePace.probeKbps(500, 60, 1392, 20, 2));
        assertEquals(PyroWavePace.MIN_PROBE_KBPS, PyroWavePace.probeKbps(0, 60, 1392, 20, 2));
        assertEquals(PyroWavePace.MAX_PROBE_KBPS, PyroWavePace.probeKbps(5000000, 60, 1392, 20, 2));
        assertEquals(125000, PyroWavePace.floorKbps(100000));
    }

    @Test
    public void probeSizingMatchesTheHost() {
        // kbps * 2000 ms / (8 * (packet + 134)), as Vibeshine computes `expected`.
        assertEquals(163826, PyroWavePace.expectedPackets(1000000, 1392));
        // 1 ms group at 1 Gbps of 1440-byte probe payloads with UDP, IPv4 and Ethernet bytes.
        assertEquals(1000000 / 8 / (1440 + 8 + 20 + 38), PyroWavePace.burstGroupPackets(1000000, 1392, false));
    }
}
