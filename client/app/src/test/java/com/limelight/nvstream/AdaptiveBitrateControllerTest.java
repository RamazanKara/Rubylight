package com.limelight.nvstream;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

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
        assertEquals("device heat", controller.lastReason());
        controller.applied(10000, 10000, 15000);
        assertEquals(0, controller.sample(31000, true, false, 0, 2));
        assertEquals(0, controller.sample(32000, true, false, 0, 2));
    }

    @Test
    public void eachChangeSaysWhy() {
        // Issue 56: a TV halved its bitrate every stream and nothing said why.
        AdaptiveBitrateController loss = new AdaptiveBitrateController(20000, 20000, 60, 0);
        assertEquals(0, loss.sample(4000, true, false, 5, 2));
        assertEquals(16000, loss.sample(5000, true, false, 5, 2));
        assertEquals("5.0% frame loss", loss.lastReason());

        AdaptiveBitrateController warning = new AdaptiveBitrateController(20000, 20000, 60, 0);
        assertEquals(0, warning.sample(4000, true, true, 0, 2));
        assertEquals(16000, warning.sample(5000, true, true, 0, 2));
        assertEquals("connection warning", warning.lastReason());

        AdaptiveBitrateController decoder = new AdaptiveBitrateController(20000, 20000, 60, 0);
        assertEquals(0, decoder.sample(14000, true, false, 0, 20));
        assertEquals(16000, decoder.sample(15000, true, false, 0, 20));
        assertEquals("decode 20.0 ms", decoder.lastReason());
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

    // Sustained loss for ten minutes, applying every request like the host does; returns the lowest rate.
    private static int lowestRateUnderSustainedLoss(AdaptiveBitrateController controller, int startKbps) {
        int current = startKbps;
        int lowest = startKbps;
        for (long now = 1000; now <= 600000; now += 1000) {
            int target = controller.sample(now, true, true, 10, 2);
            if (target != 0) {
                controller.applied(target, target, now);
                current = target;
                lowest = Math.min(lowest, current);
            }
        }
        return lowest;
    }

    @Test
    public void withoutLimitsSustainedLossStillReachesTheBuiltInFloor() {
        // The built-in floor is 2 Mbps whatever the stream bitrate.
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS,
                lowestRateUnderSustainedLoss(new AdaptiveBitrateController(40000, 40000, 60, 0), 40000));
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS,
                lowestRateUnderSustainedLoss(new AdaptiveBitrateController(40000, 40000, 60, 0), 40000));
    }

    @Test
    public void hostRoundingAboveTheCeilingIsNotRequestedAgain() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(20000, 20000, 60, 0);
        // The host applied a little more than requested.
        controller.applied(20000, 20500, 0);
        for (long now = 1000; now <= 60000; now += 1000) {
            assertEquals("no repeated request at " + now, 0, controller.sample(now, true, false, 0, 2, 0, 0));
        }
    }

    // What the network does at a given moment and rate: {loss percent, jitter ms, queue ms}.
    private interface Network {
        float[] at(long nowMs, int kbps);
    }

    private static final float[] CALM = {0, 1, 0};
    private static final Network CALM_NETWORK = (now, kbps) -> CALM;
    private static final Network LOSS = (now, kbps) -> new float[] {10, 1, 0};

    // Drives a controller once a second like Game does, applying every request at once like the host.
    private static final class Run {
        final AdaptiveBitrateController controller;
        int kbps;
        long now;
        int lowest;
        // Game calls suspend() every second while the request is in flight.
        boolean suspendBeforeApply;

        Run(AdaptiveBitrateController controller, int startKbps) {
            this.controller = controller;
            kbps = lowest = startKbps;
        }

        void seconds(int count, Network network) {
            for (int i = 0; i < count; i++) {
                now += 1000;
                float[] n = network.at(now, kbps);
                int target = controller.sample(now, true, false, n[0], 2, n[1], n[2]);
                if (target != 0) {
                    if (suspendBeforeApply) {
                        controller.suspend();
                        controller.suspend();
                    }
                    controller.applied(target, target, now);
                    kbps = target;
                    lowest = Math.min(lowest, kbps);
                }
            }
        }

        // Seconds until the rate first reaches the goal, or -1.
        int secondsUntil(int goalKbps, int limit, Network network) {
            for (int i = 1; i <= limit; i++) {
                seconds(1, network);
                if (kbps >= goalKbps) return i;
            }
            return -1;
        }
    }

    @Test
    public void jitterThatTheBitrateDoesNotCauseStopsCuttingAboveTheFloor() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 120, 0), 50000);
        run.seconds(30, CALM_NETWORK);
        run.seconds(600, (now, kbps) -> new float[] {0, 8, 0});
        assertTrue("held at " + run.kbps, run.kbps >= 50000 * 80 / 100 * 80 / 100);
        int held = run.kbps;
        run.seconds(300, (now, kbps) -> new float[] {0, 8, 0});
        assertEquals(held, run.kbps);
    }

    @Test
    public void queueingTheBitrateDoesNotCauseStopsCuttingToo() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 60, 0), 50000);
        run.seconds(600, (now, kbps) -> new float[] {0, 1, 25});
        assertTrue("held at " + run.kbps, run.kbps >= 50000 * 80 / 100 * 80 / 100);
    }

    @Test
    public void jitterThatFollowsTheBitrateKeepsCuttingUntilItIsGone() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 120, 0), 50000);
        // Jitter grows with the square of the rate: 12 ms at 50 Mbps, under the 6.25 ms limit below 36 Mbps.
        Network network = (now, kbps) -> new float[] {0, 12f * kbps / 50000 * kbps / 50000, 0};
        run.seconds(300, network);
        assertTrue("cut to " + run.kbps, run.kbps < 36000);
        assertTrue(run.kbps > 20000);
    }

    @Test
    public void lossTheBitrateCausesStillCutsAfterJitterHeldTheRate() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 120, 0), 50000);
        run.seconds(30, CALM_NETWORK);
        run.seconds(120, (now, kbps) -> new float[] {0, 8, 0});
        assertTrue(run.kbps > 20000);
        // The link is now too small for the stream: loss above 20 Mbps, on top of the jitter.
        run.seconds(300, (now, kbps) -> new float[] {kbps > 20000 ? 6 : 0, 8, 0});
        assertTrue("cut to " + run.kbps, run.kbps <= 20000);
        assertTrue(run.kbps > AdaptiveBitrateController.DEFAULT_FLOOR_KBPS);
    }

    @Test
    public void sustainedLossReachesTheFloorEvenWithJitterAndQueueing() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 120, 0), 50000);
        run.seconds(600, (now, kbps) -> new float[] {10, 8, 30});
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS, run.kbps);
    }

    @Test
    public void jitterHoldEndsWhenTheSignalIsHealthyAndRecoveryStepsBack() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 120, 0), 50000);
        run.seconds(30, CALM_NETWORK);
        run.seconds(120, (now, kbps) -> new float[] {0, 8, 0});
        assertTrue(run.kbps < 50000);
        int seconds = run.secondsUntil(50000, 60, CALM_NETWORK);
        assertTrue("took " + seconds + " s", seconds > 0);
        // A new bout of jitter is judged afresh: it holds after two cuts again, not at the floor.
        run.seconds(600, (now, kbps) -> new float[] {0, 8, 0});
        assertTrue("held at " + run.kbps, run.kbps >= 50000 * 60 / 100);
    }

    @Test
    public void recoveryFromTheFloorToTheStableRateTakesUnderHalfAMinute() {
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 60, 0), 50000);
        run.seconds(20, CALM_NETWORK);
        run.seconds(120, LOSS);
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS, run.kbps);
        int seconds = run.secondsUntil(50000, 120, CALM_NETWORK);
        assertTrue("took " + seconds + " s", seconds > 0 && seconds < 35);
    }

    @Test
    public void probingAboveTheStableRateStaysSlow() {
        Run run = new Run(new AdaptiveBitrateController(80000, 50000, 60, 0), 50000);
        run.seconds(20, CALM_NETWORK);
        run.seconds(30, LOSS);
        run.secondsUntil(50000, 120, CALM_NETWORK);
        int atStable = run.kbps;
        assertEquals(50000, atStable);
        run.seconds(14, CALM_NETWORK);
        assertEquals(atStable, run.kbps);
        run.seconds(60, CALM_NETWORK);
        assertTrue(run.kbps > atStable && run.kbps <= atStable * 130 / 100);
    }

    @Test
    public void troubleBeforeAnyStableRateRecoversSlowlyAsBefore() {
        Run run = new Run(new AdaptiveBitrateController(10000, 10000, 60, 0), 10000);
        run.seconds(30, LOSS);
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS, run.kbps);
        run.seconds(30, CALM_NETWORK);
        // Only the old 5% (at least 250 kbps) every 15 seconds.
        assertTrue("recovered to " + run.kbps, run.kbps <= AdaptiveBitrateController.DEFAULT_FLOOR_KBPS + 2 * 250);
    }

    @Test
    public void failedStepBackReturnsToTheWorkingRateAndLowersTheStableRate() {
        failedStepBackReturnsToTheWorkingRate(false);
    }

    @Test
    public void failedStepBackStillReturnsWhenSuspendCallsComeBetweenRequestAndApply() {
        failedStepBackReturnsToTheWorkingRate(true);
    }

    private static void failedStepBackReturnsToTheWorkingRate(boolean suspendBeforeApply) {
        Run run = new Run(new AdaptiveBitrateController(40000, 40000, 60, 0), 40000);
        run.suspendBeforeApply = suspendBeforeApply;
        run.seconds(20, CALM_NETWORK);
        run.seconds(18, LOSS);
        int working = run.kbps;
        assertTrue("cut to " + working, working < 15000);
        // The link now carries only a little more than the working rate.
        int capacity = working * 130 / 100;
        Network network = (now, kbps) -> new float[] {kbps > capacity ? 6 : 0, 1, 0};
        List<Integer> rates = new ArrayList<>();
        int lowestAfterSettling = Integer.MAX_VALUE;
        for (int i = 0; i < 400; i++) {
            int before = run.kbps;
            run.seconds(1, network);
            if (run.kbps != before) rates.add(run.kbps);
            if (i >= 100) lowestAfterSettling = Math.min(lowestAfterSettling, run.kbps);
        }
        // The first step back half-way to the stable rate failed and is undone by going back to the
        // working rate, not by a 20% cut; the next try is a smaller step.
        assertTrue("rates " + rates, rates.get(0) > capacity && rates.get(1) == working);
        assertTrue("rates " + rates, rates.get(2) > working && rates.get(2) - working < rates.get(0) - working);
        // It then finds the link's rate instead of staying at the working one; an occasional probe
        // above the link is cut by 20% only and walked back up.
        assertTrue("lowest " + lowestAfterSettling, lowestAfterSettling >= working * 105 / 100);
    }

    @Test
    public void floorScalesWithResolutionFrameRateAndCodec() {
        AdaptiveBitrateController.Codec hevc = AdaptiveBitrateController.Codec.HEVC;
        int hevc1080p60 = AdaptiveBitrateController.builtInFloorKbps(1920, 1080, 60, hevc);
        assertTrue("1080p60 HEVC " + hevc1080p60, hevc1080p60 >= 8000 && hevc1080p60 <= 10000);
        assertEquals(2000, AdaptiveBitrateController.builtInFloorKbps(1280, 720, 30, hevc));
        assertTrue(AdaptiveBitrateController.builtInFloorKbps(1920, 1080, 120, hevc) > hevc1080p60);
        assertTrue(AdaptiveBitrateController.builtInFloorKbps(3840, 2160, 60, hevc) > hevc1080p60);
        assertTrue(AdaptiveBitrateController.builtInFloorKbps(1920, 1080, 60, AdaptiveBitrateController.Codec.H264) > hevc1080p60);
        assertTrue(AdaptiveBitrateController.builtInFloorKbps(1920, 1080, 60, AdaptiveBitrateController.Codec.AV1) <= hevc1080p60);
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS, AdaptiveBitrateController.builtInFloorKbps(0, 0, 60, hevc));
    }

    private static int lowestUnderLoss(AdaptiveBitrateController controller, int startKbps) {
        Run run = new Run(controller, startKbps);
        run.seconds(600, LOSS);
        return run.lowest;
    }

    @Test
    public void sustainedLossStopsAtTheResolutionAwareFloor() {
        AdaptiveBitrateController.Codec hevc = AdaptiveBitrateController.Codec.HEVC;
        assertEquals(AdaptiveBitrateController.builtInFloorKbps(1920, 1080, 60, hevc),
                lowestUnderLoss(new AdaptiveBitrateController(60000, 60000, 60, 0, 1920, 1080, hevc), 60000));
        assertEquals(2000,
                lowestUnderLoss(new AdaptiveBitrateController(60000, 60000, 30, 0, 1280, 720, hevc), 60000));
    }

    @Test
    public void builtInFloorStaysUnderHalfTheStartingRateSoCongestionCanStillCut() {
        AdaptiveBitrateController.Codec hevc = AdaptiveBitrateController.Codec.HEVC;
        assertEquals(10000,
                lowestUnderLoss(new AdaptiveBitrateController(20000, 20000, 120, 0, 3840, 2160, hevc), 20000));
        assertEquals(1000,
                lowestUnderLoss(new AdaptiveBitrateController(1000, 1000, 60, 0, 3840, 2160, hevc), 1000));
    }

    @Test
    public void floorFollowsAResolutionAndFrameRateChange() {
        AdaptiveBitrateController.Codec hevc = AdaptiveBitrateController.Codec.HEVC;
        AdaptiveBitrateController down = new AdaptiveBitrateController(80000, 80000, 60, 0, 3840, 2160, hevc);
        down.setStreamSize(1280, 720);
        down.setFrameRate(30);
        assertEquals(2000, lowestUnderLoss(down, 80000));
        AdaptiveBitrateController up = new AdaptiveBitrateController(80000, 80000, 30, 0, 1280, 720, hevc);
        up.setStreamSize(1920, 1080);
        up.setFrameRate(60);
        assertEquals(AdaptiveBitrateController.builtInFloorKbps(1920, 1080, 60, hevc), lowestUnderLoss(up, 80000));
    }

    @Test
    public void queueGrowthTheBitrateCausesKeepsCuttingBelowTheLinkRate() {
        // A 30 Mbps link with a 3 s buffer: the queue grows in proportion to the rate above the link.
        int capacity = 30000;
        float[] queue = {0};
        float[] peak = {0};
        Run run = new Run(new AdaptiveBitrateController(50000, 50000, 60, 0), 50000);
        run.seconds(30, CALM_NETWORK);
        run.seconds(90, (now, kbps) -> {
            queue[0] = Math.max(0, Math.min(3000, queue[0] + (kbps - capacity) * 1000f / capacity));
            peak[0] = Math.max(peak[0], queue[0]);
            return new float[] {queue[0] >= 3000 ? 5 : 0, 1, queue[0]};
        });
        assertTrue("lowest " + run.lowest, run.lowest < capacity);
        assertTrue("the buffer overflowed: peak " + peak[0] + " ms", peak[0] < 3000);
    }

    @Test
    public void aHoldEndsWhenTheQueueStartsToGrow() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(50000, 50000, 60, 0);
        Run run = new Run(controller, 50000);
        run.seconds(30, CALM_NETWORK);
        run.seconds(120, (now, kbps) -> new float[] {0, 20, 0});
        int held = run.kbps;
        assertTrue(held < 50000);
        float[] queue = {20};
        run.seconds(30, (now, kbps) -> new float[] {0, 20, queue[0] += 15});
        assertTrue("held " + held + " then " + run.kbps, run.kbps < held);
    }

    @Test
    public void stepBackSurvivesSuspendCallsBeforeTheHostAppliesIt() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(40000, 40000, 60, 0);
        for (long now = 1000; now <= 12000; now += 1000) controller.sample(now, true, false, 0, 2, 1, 0);
        controller.sample(13000, true, false, 10, 2, 1, 0);
        int cut = controller.sample(14000, true, false, 10, 2, 1, 0);
        assertEquals(32000, cut);
        controller.applied(cut, cut, 14000);
        int back = 0;
        long now = 14000;
        while (back == 0) back = controller.sample(now += 1000, true, false, 0, 2, 1, 0);
        controller.suspend();
        controller.suspend();
        controller.applied(back, back, now);
        assertEquals(0, controller.sample(now + 1000, true, false, 10, 2, 1, 0));
        assertEquals(cut, controller.sample(now + 3000, true, false, 10, 2, 1, 0));
    }

    @Test
    public void poorSamplesMinutesApartAroundAStaticScreenDoNotCut() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(40000, 40000, 60, 0);
        assertEquals(0, controller.sample(5000, true, false, 10, 2));
        assertEquals(0, controller.sample(6000, false, false, 0, 2));
        assertEquals(0, controller.sample(300000, true, false, 10, 2));
        assertEquals(32000, controller.sample(301000, true, false, 10, 2));
    }

    private static Run heldByJitter(AdaptiveBitrateController controller) {
        Run run = new Run(controller, 50000);
        run.seconds(30, CALM_NETWORK);
        run.seconds(120, (now, kbps) -> new float[] {0, 8, 0});
        int held = run.kbps;
        run.seconds(30, (now, kbps) -> new float[] {0, 8, 0});
        assertEquals("is held", held, run.kbps);
        return run;
    }

    @Test
    public void aFrameRateChangeEndsTheHold() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(50000, 50000, 120, 0);
        Run run = heldByJitter(controller);
        int held = run.kbps;
        controller.setFrameRate(120);
        run.seconds(10, (now, kbps) -> new float[] {0, 8, 0});
        assertTrue("cut again from " + held + " to " + run.kbps, run.kbps < held);
    }

    @Test
    public void aGapInVideoEndsTheHold() {
        AdaptiveBitrateController controller = new AdaptiveBitrateController(50000, 50000, 120, 0);
        Run run = heldByJitter(controller);
        int held = run.kbps;
        controller.sample(run.now += 1000, false, false, 0, 2, 0, -1);
        run.seconds(10, (now, kbps) -> new float[] {0, 8, 0});
        assertTrue("cut again from " + held + " to " + run.kbps, run.kbps < held);
    }

    @Test
    public void aFloorRaisedAboveTheCurrentRateStillLetsLossCut() {
        AdaptiveBitrateController.Codec hevc = AdaptiveBitrateController.Codec.HEVC;
        AdaptiveBitrateController controller = new AdaptiveBitrateController(80000, 80000, 30, 0, 3840, 2160, hevc);
        // Automatic quality dropped to 720p30 and congestion cut the rate there; then 1440p60 comes back.
        controller.setStreamSize(1280, 720);
        controller.applied(3000, 3000, 0);
        controller.setStreamSize(2560, 1440);
        controller.setFrameRate(60);
        assertTrue(AdaptiveBitrateController.builtInFloorKbps(2560, 1440, 60, hevc) > 3000);
        Run run = new Run(controller, 3000);
        run.seconds(30, LOSS);
        assertTrue("lowest " + run.lowest, run.lowest < 3000);
        assertEquals(AdaptiveBitrateController.DEFAULT_FLOOR_KBPS, run.lowest);
    }
}
