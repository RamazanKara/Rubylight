package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class DisplayFrameRatePolicyTest {
    @Test
    public void android15AndOlderCannotEnableArr() {
        for (int sdk : new int[] {21, 30, 31, 34, 35}) {
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, false));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, true));
        }
    }

    @Test
    public void android16RequiresArrSupport() {
        for (int sdk : new int[] {36, 37}) {
            assertTrue(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, true));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, true, false));
            assertFalse(DisplayFrameRatePolicy.useAdaptiveRefreshRate(sdk, false, true));
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
