package com.limelight.binding.input;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;

import static org.junit.Assert.*;

public class TriggerDeadzoneTest {
    private static final float DEFAULT = PreferenceConfiguration.DEFAULT_TRIGGER_DEADZONE / 100f;

    private static float apply(float value) {
        return ControllerHandler.applyTriggerDeadzone(value, DEFAULT);
    }

    @Test
    public void lightPressesBelowMoonlightsThirteenPercentReachTheHost() {
        assertEquals(0.05f, apply(0.05f), 0);
        assertEquals(0.1f, apply(0.1f), 0);
        assertEquals(0.12f, apply(0.12f), 0);
    }

    @Test
    public void aRestingTriggerStillReadsAsReleasedByDefault() {
        assertEquals(0f, apply(0f), 0);
        assertEquals(0f, apply(0.01f), 0);
        assertEquals(0f, apply(DEFAULT), 0);
    }

    @Test
    public void travelAboveTheDeadzoneIsPassedRaw() {
        assertEquals(0.5f, apply(0.5f), 0);
        assertEquals(1f, apply(1f), 0);
    }

    @Test
    public void zeroPassesAnyPressAndDropsNegativeNoise() {
        assertEquals(0.004f, ControllerHandler.applyTriggerDeadzone(0.004f, 0), 0);
        assertEquals(0f, ControllerHandler.applyTriggerDeadzone(-0.01f, 0), 0);
    }

    @Test
    public void aLargerSettingDropsMoreTravel() {
        assertEquals(0f, ControllerHandler.applyTriggerDeadzone(0.15f, 0.2f), 0);
        assertEquals(0.25f, ControllerHandler.applyTriggerDeadzone(0.25f, 0.2f), 0);
    }

    @Test
    public void quirkPadsNeverGoBelowThirtyPercent() {
        assertEquals(0.30f, ControllerHandler.QUIRK_TRIGGER_DEADZONE, 0);
        float quirk = Math.max(0f, ControllerHandler.QUIRK_TRIGGER_DEADZONE);
        assertEquals(0f, ControllerHandler.applyTriggerDeadzone(0.25f, quirk), 0);
        assertEquals(0.31f, ControllerHandler.applyTriggerDeadzone(0.31f, quirk), 0);
    }
}
