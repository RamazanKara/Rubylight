package com.limelight.binding.input;

import org.junit.Test;

import static org.junit.Assert.*;

public class TriggerDeadzoneTest {
    private static float apply(float value) {
        return ControllerHandler.applyTriggerDeadzone(value, ControllerHandler.TRIGGER_REST_ZONE);
    }

    @Test
    public void lightPressesBelowMoonlightsThirteenPercentReachTheHost() {
        assertEquals(0.05f, apply(0.05f), 0);
        assertEquals(0.1f, apply(0.1f), 0);
        assertEquals(0.12f, apply(0.12f), 0);
    }

    @Test
    public void aRestingTriggerStillReadsAsReleased() {
        assertEquals(0f, apply(0f), 0);
        assertEquals(0f, apply(0.01f), 0);
        assertEquals(0f, apply(ControllerHandler.TRIGGER_REST_ZONE), 0);
    }

    @Test
    public void travelAboveTheRestZoneIsPassedRaw() {
        assertEquals(0.5f, apply(0.5f), 0);
        assertEquals(1f, apply(1f), 0);
    }

    @Test
    public void quirkDeadzonesStillApply() {
        assertEquals(0f, ControllerHandler.applyTriggerDeadzone(0.25f, 0.30f), 0);
        assertEquals(0.31f, ControllerHandler.applyTriggerDeadzone(0.31f, 0.30f), 0);
    }
}
