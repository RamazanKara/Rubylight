package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class DisplayFrameRatePolicyTest {
    @Test
    public void android15AndOlderKeepThePanelMaximumWithoutArrDetection() {
        for (int sdk : new int[] {21, 30, 31, 34, 35}) {
            assertFalse(DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, false));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, true));
            assertEquals(120, new DisplayFrameRatePolicy().cadenceVote(60, 120, 120,
                    DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, false), 0), 0);
        }
    }

    @Test
    public void android16RequiresArrSupport() {
        for (int sdk : new int[] {36, 37}) {
            assertTrue(DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, true));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, false));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveHints(sdk, false, true));
        }
    }

    @Test
    public void sustainedCadenceCanFallBelowTheNegotiatedPanelMaximum() {
        for (float fps : new float[] {60, 90, 120, 59.94f, 119.88f}) {
            DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
            for (long now = 0; now < 2000; now += 250) {
                assertEquals(120, policy.cadenceVote(fps, 120, 120, true, now), 0);
            }
            assertEquals(fps, policy.cadenceVote(fps, 120, 120, true, 2000), 0);
        }
    }

    @Test
    public void stillScenesAndInvalidSamplesRestoreMaximumAndRestartTheHold() {
        for (float unavailable : new float[] {0, 1, 20, 30, -1, Float.NaN,
                Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY}) {
            DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
            assertEquals(60, settle(policy, 60, 120, 120, 0), 0);
            for (long now = 2250; now <= 5000; now += 250) {
                assertEquals(120, policy.cadenceVote(unavailable, 120, 120, true, now), 0);
            }
            for (long now = 5250; now < 7250; now += 250) {
                assertEquals(120, policy.cadenceVote(60, 120, 120, true, now), 0);
            }
            assertEquals(60, policy.cadenceVote(60, 120, 120, true, 7250), 0);
        }
    }

    @Test
    public void increasesRestoreMaximumOnTheFirstSampleAndDecreasesWait() {
        DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
        assertEquals(60, settle(policy, 60, 120, 120, 0), 0);
        assertEquals(120, policy.cadenceVote(90, 120, 120, true, 2250), 0);
        assertEquals(90, settle(policy, 90, 120, 120, 2500), 0);
        assertEquals(120, policy.cadenceVote(120, 120, 120, true, 4750), 0);
        assertEquals(120, settle(policy, 120, 120, 120, 5000), 0);
        for (long now = 7250; now < 9250; now += 250) {
            assertEquals(120, policy.cadenceVote(60, 120, 120, true, now), 0);
        }
        assertEquals(60, policy.cadenceVote(60, 120, 120, true, 9250), 0);
    }

    @Test
    public void briefDropsAndVariableCadenceDoNotCauseRepeatedSwitches() {
        DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
        assertEquals(120, settle(policy, 120, 120, 120, 0), 0);
        for (int i = 1; i <= 40; i++) {
            float fps = i % 4 == 0 ? 120 : 60;
            assertEquals(120, policy.cadenceVote(fps, 120, 120, true, 2000 + i * 250), 0);
        }
        for (int i = 1; i <= 40; i++) {
            assertEquals(120, policy.cadenceVote(i % 2 == 0 ? 60 : 90, 120, 120, true, 12000 + i * 250), 0);
        }
    }

    @Test
    public void smallMeasurementNoiseKeepsTheConfirmedFractionalVote() {
        DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
        assertEquals(59.94f, settle(policy, 59.94f, 120, 120, 0), 0);
        for (int i = 1; i <= 40; i++) {
            float fps = i % 2 == 0 ? 60.8f : 59.2f;
            assertEquals(59.94f, policy.cadenceVote(fps, 120, 120, true, 2000 + i * 250), 0);
        }
    }

    @Test
    public void missedPollingAndClockResetCannotConfirmStaleEvidence() {
        DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
        for (long now = 0; now <= 1000; now += 250) {
            assertEquals(120, policy.cadenceVote(60, 120, 120, true, now), 0);
        }
        assertEquals(120, policy.cadenceVote(60, 120, 120, true, 5000), 0);
        assertEquals(60, settle(policy, 60, 120, 120, 5250), 0);
        assertEquals(120, policy.cadenceVote(60, 120, 120, true, 1000), 0);
    }

    @Test
    public void differentPanelCeilingsAndStreamChangesRestartLearning() {
        for (float panel : new float[] {59.94f, 60, 75, 90, 119.88f, 120, 144, 165, 240}) {
            DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
            assertEquals(Math.min(60, panel), settle(policy, 60, Math.round(panel), panel, 0), 0);
            assertEquals(panel, policy.cadenceVote(panel * 2, Math.round(panel), panel, true, 2250), 0.01);
        }
        DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
        assertEquals(60, settle(policy, 60, 120, 120, 0), 0);
        assertEquals(144, policy.cadenceVote(60, 144, 144, true, 2250), 0);
        assertEquals(60, settle(policy, 60, 144, 144, 2500), 0);
        assertEquals(144, policy.cadenceVote(59, 59, 144, true, 4750), 0);
        assertEquals(59, settle(policy, 59, 59, 144, 5000), 0);
    }

    @Test
    public void withoutArrCadenceNeverRatchetsTheVoteDownward() {
        for (float cadence : new float[] {120, 60, 30, 0, Float.NaN, 144}) {
            DisplayFrameRatePolicy policy = new DisplayFrameRatePolicy();
            assertEquals(60, settle(policy, 60, 120, 120, 0), 0);
            for (long now = 2250; now <= 5000; now += 250) {
                assertEquals(120, policy.cadenceVote(cadence, 120, 120, false, now), 0);
            }
        }
    }

    private static float settle(DisplayFrameRatePolicy policy, float fps, float streamFps, float panel, long startMs) {
        float vote = 0;
        for (long now = startMs; now <= startMs + 2000; now += 250) {
            vote = policy.cadenceVote(fps, streamFps, panel, true, now);
        }
        return vote;
    }

    @Test
    public void streamRequestRoundsPanelMaximumWithinHostLimits() {
        assertEquals(120, DisplayFrameRatePolicy.streamFrameRate(119.88f));
        assertEquals(144, DisplayFrameRatePolicy.streamFrameRate(144));
        assertEquals(1000, DisplayFrameRatePolicy.streamFrameRate(1200));
        assertEquals(1, DisplayFrameRatePolicy.streamFrameRate(0.5f));
    }
}
