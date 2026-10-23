package com.limelight.preferences;

import org.junit.Test;

import static org.junit.Assert.*;

public class SeekBarPreferenceTest {
    private static final int MIN = 500, MAX = 500000;

    @Test
    public void logScaleGivesTheCommonLanRangeAUsableShareOfTheSlider() {
        int from = SeekBarPreference.logValueToPosition(10000, MIN, MAX);
        int to = SeekBarPreference.logValueToPosition(80000, MIN, MAX);
        // A linear 0.5-500 Mbps slider gives 10-80 Mbps 14% of its travel
        assertTrue(to - from > SeekBarPreference.LOG_POSITIONS * 28 / 100);
        assertEquals(MIN, SeekBarPreference.positionToLogValue(0, MIN, MAX));
        assertEquals(MAX, SeekBarPreference.positionToLogValue(SeekBarPreference.LOG_POSITIONS, MIN, MAX));
    }

    @Test
    public void storedKbpsValuesRoundTripThroughTheSlider() {
        for (int kbps : new int[] {10000, 20000, 40000, 80000, 150000, 280000}) {
            int position = SeekBarPreference.logValueToPosition(kbps, MIN, MAX);
            assertEquals(kbps, SeekBarPreference.positionToLogValue(position, MIN, MAX));
        }
    }

    @Test
    public void logValuesSnapToReadableSteps() {
        for (int position = 0; position <= SeekBarPreference.LOG_POSITIONS; position++) {
            int value = SeekBarPreference.positionToLogValue(position, MIN, MAX);
            assertEquals(0, value % 500);
            if (value >= 10000 && value < 100000) {
                assertEquals("1 Mbps steps in the LAN range", 0, value % 1000);
            }
        }
    }

    @Test
    public void nudgeStepsToTheNextWholeStepAndStaysInRange() {
        assertEquals(21000, SeekBarPreference.nudge(20000, true, true, 500, MIN, MAX));
        assertEquals(19000, SeekBarPreference.nudge(20000, false, true, 500, MIN, MAX));
        assertEquals(20000, SeekBarPreference.nudge(19400, true, true, 500, MIN, MAX));
        assertEquals(9500, SeekBarPreference.nudge(10000, false, true, 500, MIN, MAX));
        assertEquals(105000, SeekBarPreference.nudge(100000, true, true, 500, MIN, MAX));
        assertEquals(MAX, SeekBarPreference.nudge(MAX, true, true, 500, MIN, MAX));
        assertEquals(MIN, SeekBarPreference.nudge(MIN, false, true, 500, MIN, MAX));
        assertEquals(55, SeekBarPreference.nudge(50, true, false, 5, 50, 200));
    }

    @Test
    public void linearRangesBelowZeroShiftThroughTheSlider() {
        assertEquals(0, SeekBarPreference.linearValueToPosition(-20, -20, 20));
        assertEquals(20, SeekBarPreference.linearValueToPosition(0, -20, 20));
        assertEquals(40, SeekBarPreference.linearValueToPosition(20, -20, 20));
        assertEquals(-20, SeekBarPreference.linearPositionToValue(0, -20, 20, 1));
        assertEquals(-7, SeekBarPreference.linearPositionToValue(13, -20, 20, 1));
        assertEquals(20, SeekBarPreference.linearPositionToValue(40, -20, 20, 1));
        for (int value = -20; value <= 20; value++) {
            int position = SeekBarPreference.linearValueToPosition(value, -20, 20);
            assertEquals(value, SeekBarPreference.linearPositionToValue(position, -20, 20, 1));
        }
    }

    @Test
    public void nudgeCrossesZeroOneStepAtATime() {
        assertEquals(0, SeekBarPreference.nudge(-1, true, false, 1, -20, 20));
        assertEquals(-1, SeekBarPreference.nudge(0, false, false, 1, -20, 20));
        assertEquals(-20, SeekBarPreference.nudge(-20, false, false, 1, -20, 20));
        assertEquals(-5, SeekBarPreference.nudge(-7, true, false, 5, -20, 20));
        assertEquals(-10, SeekBarPreference.nudge(-7, false, false, 5, -20, 20));
    }
}
