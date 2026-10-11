package com.limelight.binding.input.driver;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class DualSenseControllerTest {
    @Test
    public void motionRateKeepsFractionalReportIntervals() {
        long last = 1_000_000_000L;
        int reports = 0;
        for (long now = last + 4_000_000L; now <= 2_000_000_000L; now += 4_000_000L) {
            if (now - last >= 5_000_000L) {
                last = DualSenseController.advanceMotionTimestamp(last, now, 5_000_000L);
                reports++;
            }
        }
        assertEquals(200, reports);
        assertEquals(2_000_000_000L, last);
    }
}
