package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class AdaptiveBitrateControllerTest {
    @Test
    public void frameRateChangesUpdateDecodeBudgetWithoutResettingBitrate() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 120, 0);
        controller.applied(16000, 16000, 15000);
        controller.setFrameRate(60);
        assertEquals(0, controller.sample(16000, true, false, 0, 10));
        assertEquals(0, controller.sample(30000, true, false, 0, 10));
        assertEquals(16800, controller.sample(31000, true, false, 0, 10));
        controller.setFrameRate(120);
        assertEquals(0, controller.sample(32000, true, false, 0, 10));
        assertEquals(12800, controller.sample(33000, true, false, 0, 10));
    }

    @Test
    public void frameRateChangeRechecksDecoderSensitivityAndPreservesHostCap() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 60, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 20));
        assertEquals(16000, controller.sample(15000, true, false, 0, 20));
        controller.applied(16000, 15000, 15000);
        assertEquals(0, controller.sample(30000, true, false, 0, 20));
        controller.setFrameRate(120);
        assertEquals(0, controller.sample(31000, true, false, 0, 20));
        assertEquals(12000, controller.sample(32000, true, false, 0, 20));
        controller.setFrameRate(60);
        assertEquals(0, controller.sample(33000, true, false, 0, 2));
        assertEquals(0, controller.sample(48000, true, false, 0, 2));
    }

    @Test
    public void modestWifiJitterAllowsRecoveryAt120Fps() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 8000, 120, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 2, 3));
        assertEquals(8400, controller.sample(16000, true, false, 0, 2, 3));
    }

    @Test
    public void timeSpentInMenusDoesNotCountAsHealthyRecovery() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 8000, 60, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 2));
        controller.suspend();
        assertEquals(0, controller.sample(30000, true, false, 0, 2));
        assertEquals(8400, controller.sample(45000, true, false, 0, 2));
    }

    @Test
    public void fixedDecoderLatencyStopsReducingBitrateAndAllowsRecovery() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 120, 0);
        assertEquals(0, controller.sample(5000, true, false, 0, 9));
        assertEquals(0, controller.sample(6000, true, false, 0, 9));
        assertEquals(16000, controller.sample(15000, true, false, 0, 9));
        controller.applied(16000, 16000, 15000);
        assertEquals(0, controller.sample(16000, true, false, 0, 9));
        assertEquals(0, controller.sample(30000, true, false, 0, 9));
        assertEquals(16800, controller.sample(45000, true, false, 0, 9));
        controller.applied(16800, 16800, 45000);
        assertEquals(0, controller.sample(46000, true, false, 0, 9));
        assertEquals(17640, controller.sample(61000, true, false, 0, 9));
    }

    @Test
    public void decoderPressureOnlyContinuesCuttingWhenLatencyImprovesAndStopsAtHalf() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 120, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 20));
        assertEquals(16000, controller.sample(15000, true, false, 0, 20));
        controller.applied(16000, 16000, 15000);
        assertEquals(0, controller.sample(16000, true, false, 0, 18));
        assertEquals(12800, controller.sample(30000, true, false, 0, 18));
        controller.applied(12800, 12800, 30000);
        assertEquals(0, controller.sample(31000, true, false, 0, 16));
        assertEquals(10240, controller.sample(45000, true, false, 0, 16));
        controller.applied(10240, 10240, 45000);
        assertEquals(0, controller.sample(46000, true, false, 0, 14));
        assertEquals(10000, controller.sample(60000, true, false, 0, 14));
        controller.applied(10000, 10000, 60000);
        assertEquals(0, controller.sample(61000, true, false, 0, 12));
        assertEquals(0, controller.sample(75000, true, false, 0, 12));
        assertEquals(8000, controller.sample(76000, true, true, 4, 12));
    }

    @Test
    public void fixedDecoderLatencyDoesNotHideNetworkCongestion() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 120, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 9));
        assertEquals(16000, controller.sample(15000, true, false, 0, 9));
        controller.applied(16000, 16000, 15000);
        assertEquals(0, controller.sample(30000, true, false, 0, 9));
        assertEquals(0, controller.sample(31000, true, true, 4, 9));
        assertEquals(12800, controller.sample(32000, true, true, 4, 9));
    }

    @Test
    public void fixedDecoderLatencyAllowsRecoveryAfterCongestionCutsBelowHalf() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 10000, 120, 0);
        int bitrate = 10000;
        for (long now = 3000; now <= 15000; now += 3000) {
            assertEquals(0, controller.sample(now - 1000, true, true, 5, 10));
            bitrate = bitrate * 80 / 100;
            assertEquals(bitrate, controller.sample(now, true, true, 5, 10));
            controller.applied(bitrate, bitrate, now);
        }
        assertEquals(0, controller.sample(30000, true, false, 0, 10));
        assertEquals(bitrate + 250, controller.sample(45000, true, false, 0, 10));
    }

    @Test
    public void recoveryNeedsMeasuredDecodeHeadroom() {
        for (float decode : new float[] {-1, Float.NaN, 13}) {
            AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 8000, 60, 0);
            assertEquals(0, controller.sample(1000, true, false, 0, 2));
            assertEquals(0, controller.sample(16000, true, false, 0, decode));
            assertEquals(0, controller.sample(17000, true, false, 0, 2));
            assertEquals(8400, controller.sample(32000, true, false, 0, 2));
        }
    }

    @Test
    public void runtimeRateCannotExceedClientLimit() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(3000000, 3000000, 60, 0);
        assertEquals(0, controller.sample(5000, true, true, 0, 2));
        assertEquals(1600000, controller.sample(6000, true, true, 0, 2));
    }

    @Test
    public void sustainedLossReducesBitrateWithCooldownAndFloor() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 10000, 60, 0);
        assertEquals(0, controller.sample(1000, true, true, 0, 2));
        assertEquals(0, controller.sample(2000, true, true, 0, 2));
        assertEquals(8000, controller.sample(3000, true, true, 0, 2));
        controller.applied(8000, 8000, 5000);
        assertEquals(0, controller.sample(6000, true, true, 0, 2));
        assertEquals(0, controller.sample(7000, true, true, 0, 2));
        assertEquals(6400, controller.sample(8000, true, true, 0, 2));
        controller.applied(2000, 2000, 10000);
        assertEquals(0, controller.sample(15000, true, true, 0, 2));
        assertEquals(0, controller.sample(16000, true, true, 0, 2));
    }

    @Test
    public void frameLossCanTriggerReductionWithoutNativeWarning() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 60, 0);
        assertEquals(0, controller.sample(5000, true, false, 4, 2));
        assertEquals(16000, controller.sample(6000, true, false, 4, 2));
    }

    @Test
    public void nativeCongestionCanReduceBitrateWhenFramesStopArriving() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 10000, 60, 0);
        assertEquals(0, controller.sample(5000, false, true, 0, 2));
        assertEquals(8000, controller.sample(6000, false, true, 0, 2));
        controller.applied(8000, 8000, 6000);
        assertEquals(0, controller.sample(30000, false, false, 0, 2));
    }

    @Test
    public void recoveryRequiresHealthyVideoAndNeverExceedsChosenCeiling() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 9800, 60, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 2));
        assertEquals(0, controller.sample(15000, true, false, 0, 2));
        assertEquals(10000, controller.sample(16000, true, false, 0, 2));
        controller.applied(10000, 10000, 16000);
        assertEquals(0, controller.sample(17000, true, false, 0, 2));
        assertEquals(0, controller.sample(32000, true, false, 0, 2));
    }

    @Test
    public void missingVideoAndBorderlineLossResetRecoveryWindow() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 8000, 60, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 2));
        assertEquals(0, controller.sample(15000, false, false, 0, 2));
        assertEquals(0, controller.sample(16000, true, false, 0, 2));
        assertEquals(0, controller.sample(31000, true, false, 1, 2));
        assertEquals(0, controller.sample(32000, true, false, 0, 2));
        assertEquals(8400, controller.sample(47000, true, false, 0, 2));
    }

    @Test
    public void hostCapIsRespectedEvenBelowNormalFloor() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 10000, 60, 0);
        controller.applied(8000, 1500, 5000);
        assertEquals(0, controller.sample(6000, true, false, 0, 2));
        assertEquals(0, controller.sample(21000, true, false, 0, 2));
        assertEquals(0, controller.sample(22000, true, true, 10, 2));
        assertEquals(0, controller.sample(23000, true, true, 10, 2));
    }

    @Test
    public void smallStartupBitrateIsNeverRaisedToTheFloor() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(1000, 1000, 60, 0);
        assertEquals(0, controller.sample(5000, true, true, 20, 2));
        assertEquals(0, controller.sample(6000, true, true, 20, 2));
    }

    @Test
    public void hotPhoneStepsDownSlowlyAndNotBelowHalf() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 60, 0);
        controller.setThermalLevel(2);
        assertEquals(0, controller.sample(5000, true, false, 0, 2));
        assertEquals(0, controller.sample(6000, true, false, 0, 2));
        assertEquals(16000, controller.sample(15000, true, false, 0, 2));
        controller.applied(10000, 10000, 15000);
        assertEquals(0, controller.sample(31000, true, false, 0, 2));
        assertEquals(0, controller.sample(32000, true, false, 0, 2));
    }

    @Test
    public void warmPhoneHoldsTheRate() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(30000, 20000, 60, 0);
        controller.setThermalLevel(1);
        for (long now = 1000; now <= 60000; now += 1000) {
            assertEquals(0, controller.sample(now, true, false, 0, 2));
        }
        controller.setThermalLevel(0);
        assertEquals(0, controller.sample(61000, true, false, 0, 2));
    }

    @Test
    public void risingQueueReducesBitrateBeforeAnyLoss() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 60, 0);
        assertEquals(0, controller.sample(3000, true, false, 0, 2, 0, 1));
        assertEquals(0, controller.sample(4000, true, false, 0, 2, 0, 13));
        assertEquals(16000, controller.sample(5000, true, false, 0, 2, 0, 17));
    }

    @Test
    public void standingQueueOfTwoFramesIsCongestion() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 120, 0);
        assertEquals(0, controller.sample(3000, true, false, 0, 2, 0, 21));
        assertEquals(16000, controller.sample(4000, true, false, 0, 2, 0, 21));
    }

    @Test
    public void steadySmallQueueIsNotCongestionButBlocksRaising() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 8000, 60, 0);
        for (long now = 1000; now <= 40000; now += 1000) {
            assertEquals(0, controller.sample(now, true, false, 0, 2, 0, 9));
        }
        assertEquals(0, controller.sample(41000, true, false, 0, 2, 0, 1));
        assertEquals(8400, controller.sample(56000, true, false, 0, 2, 0, 1));
    }

    @Test
    public void unknownQueueDelayKeepsTheOldBehavior() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(10000, 8000, 60, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 2, 0, -1));
        assertEquals(8400, controller.sample(16000, true, false, 0, 2, 0, -1));
    }
}
