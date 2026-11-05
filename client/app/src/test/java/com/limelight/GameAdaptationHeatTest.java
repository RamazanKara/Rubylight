package com.limelight;

import org.junit.Test;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** Heat changes a stream only through Thermal protection and a controller that follows it. */
public class GameAdaptationHeatTest {
    @Test
    public void thermalProtectionOffChangesNothing() {
        assertFalse(GameAdaptation.heatEasesLoad(false, true, true));
        assertFalse(GameAdaptation.heatEasesLoad(false, false, false));
        assertFalse(GameAdaptation.heatHoldsBitrate(false, true));
    }

    @Test
    public void thermalProtectionNeedsAController() {
        assertFalse(GameAdaptation.heatEasesLoad(true, false, false));
        assertTrue(GameAdaptation.heatEasesLoad(true, true, false));
        assertTrue(GameAdaptation.heatEasesLoad(true, false, true));
    }

    @Test
    public void onlyABitrateControllerHasABitrateToHold() {
        assertTrue(GameAdaptation.heatHoldsBitrate(true, true));
        assertFalse(GameAdaptation.heatHoldsBitrate(true, false));
    }
}
