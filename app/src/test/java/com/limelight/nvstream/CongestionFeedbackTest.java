package com.limelight.nvstream;

import org.junit.Test;
import static org.junit.Assert.*;

public class CongestionFeedbackTest {
    @Test public void modestWifiJitterAllowsPyroWaveRecoveryAt120Fps() {
        PyroWaveBitrateController pyro = new PyroWaveBitrateController(100000, 50000, 120, 0);
        assertEquals(0, pyro.sample(1000, true, false, 0, 0, 2, 3));
        assertEquals(51000, pyro.sample(21000, true, false, 0, 0, 2, 3));
    }

    @Test public void sustainedJitterReducesBothCodecsWithoutLoss() {
        AdaptiveBitrateController ordinary = new AdaptiveBitrateController(20000, 20000, 120, 0);
        PyroWaveBitrateController pyro = new PyroWaveBitrateController(100000, 100000, 120, 0);
        assertEquals(0, ordinary.sample(5000, true, false, 0, 2, 8));
        assertEquals(16000, ordinary.sample(6000, true, false, 0, 2, 8));
        assertEquals(0, pyro.sample(5000, true, false, 0, 0, 2, 8));
        assertEquals(85000, pyro.sample(6000, true, false, 0, 0, 2, 8));
    }

    @Test public void missingOrBorderlineJitterBlocksRecovery() {
        for (float jitter : new float[] {-1, Float.NaN, 5}) {
            AdaptiveBitrateController ordinary = new AdaptiveBitrateController(20000, 10000, 120, 0);
            PyroWaveBitrateController pyro = new PyroWaveBitrateController(100000, 50000, 120, 0);
            assertEquals(0, ordinary.sample(1000, true, false, 0, 2, 0));
            assertEquals(0, ordinary.sample(16000, true, false, 0, 2, jitter));
            assertEquals(0, ordinary.sample(17000, true, false, 0, 2, 0));
            assertEquals(10500, ordinary.sample(32000, true, false, 0, 2, 0));
            assertEquals(0, pyro.sample(1000, true, false, 0, 0, 2, 0));
            assertEquals(0, pyro.sample(21000, true, false, 0, 0, 2, jitter));
            assertEquals(0, pyro.sample(22000, true, false, 0, 0, 2, 0));
            assertEquals(51000, pyro.sample(42000, true, false, 0, 0, 2, 0));
        }
    }

    @Test public void invalidLossQueueOrDecodeNeverLooksHealthy() {
        for (float missing : new float[] {-1, Float.NaN}) {
            AdaptiveBitrateController ordinary = new AdaptiveBitrateController(20000, 10000, 60, 0);
            assertEquals(0, ordinary.sample(1000, true, false, missing, 2, 0));
            assertEquals(0, ordinary.sample(30000, true, false, missing, 2, 0));
            for (int metric = 0; metric < 3; metric++) {
                PyroWaveBitrateController pyro = new PyroWaveBitrateController(100000, 50000, 60, 0);
                float loss = metric == 0 ? missing : 0;
                float queue = metric == 1 ? missing : 0;
                float decode = metric == 2 ? missing : 2;
                assertEquals(0, pyro.sample(1000, true, false, loss, queue, decode, 0));
                assertEquals(0, pyro.sample(30000, true, false, loss, queue, decode, 0));
            }
        }
    }

    @Test public void staleJitterDoesNotReduceAnIdleStream() {
        AdaptiveBitrateController ordinary = new AdaptiveBitrateController(20000, 20000, 120, 0);
        PyroWaveBitrateController pyro = new PyroWaveBitrateController(100000, 100000, 120, 0);
        for (int i = 1; i <= 20; i++) {
            assertEquals(0, ordinary.sample(i * 1000L, false, false, 0, 2, 100));
            assertEquals(0, pyro.sample(i * 1000L, false, false, 0, 0, 2, 100));
        }
    }
}
