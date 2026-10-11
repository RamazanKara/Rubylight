package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class DisplayFrameRatePolicyTest {
    @Test
    public void android15AndOlderKeepThePanelMaximumWithoutArrDetection() {
        for (int sdk : new int[] {21, 30, 31, 34, 35}) {
            assertFalse(DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, false));
            assertEquals(120, DisplayFrameRatePolicy.cadenceVote(60, 120, 120,
                    DisplayFrameRatePolicy.useAdaptiveHints(sdk, true, false)), 0);
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
    public void arrCadenceUsesPanelMaximumAndRecoversFromLowerVotes() {
        assertEquals(60, DisplayFrameRatePolicy.cadenceVote(59.94f, 60, 120, true), 0);
        assertEquals(119.88f, DisplayFrameRatePolicy.cadenceVote(119.88f, 60, 120, true), 0);
        assertEquals(120, DisplayFrameRatePolicy.cadenceVote(180, 60, 120, true), 0);
        for (float unavailable : new float[] {0, -1, Float.NaN, Float.POSITIVE_INFINITY}) {
            assertEquals(120, DisplayFrameRatePolicy.cadenceVote(unavailable, 60, 120, true), 0);
        }
    }

    @Test
    public void stillSceneDoesNotLowerTheVoteBelowNegotiatedMotionRate() {
        assertEquals(120, DisplayFrameRatePolicy.cadenceVote(20, 120, 120, true), 0);
        assertEquals(60, DisplayFrameRatePolicy.cadenceVote(20, 60, 120, true), 0);
        assertEquals(60, DisplayFrameRatePolicy.cadenceVote(20, 120, 60, true), 0);
    }

    @Test
    public void withoutArrCadenceNeverRatchetsTheVoteDownward() {
        for (float cadence : new float[] {120, 60, 30, 0, Float.NaN, 144}) {
            assertEquals(120, DisplayFrameRatePolicy.cadenceVote(cadence, 60, 120, false), 0);
        }
    }

    @Test
    public void streamRequestRoundsPanelMaximumWithinHostLimits() {
        assertEquals(120, DisplayFrameRatePolicy.streamFrameRate(119.88f));
        assertEquals(144, DisplayFrameRatePolicy.streamFrameRate(144));
        assertEquals(1000, DisplayFrameRatePolicy.streamFrameRate(1200));
        assertEquals(1, DisplayFrameRatePolicy.streamFrameRate(0.5f));
    }
}
