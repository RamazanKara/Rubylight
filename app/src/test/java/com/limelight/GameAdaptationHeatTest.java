package com.limelight;

import org.junit.Test;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** Heat changes a stream only through Thermal protection and a controller that follows it. */
public class GameAdaptationHeatTest {
    @Test
    public void thermalProtectionOffChangesNothing() {
        assertFalse(GameAdaptation.heatChangesStream(false, true, true));
        assertFalse(GameAdaptation.heatChangesStream(false, false, false));
    }

    @Test
    public void thermalProtectionNeedsAController() {
        assertFalse(GameAdaptation.heatChangesStream(true, false, false));
        assertTrue(GameAdaptation.heatChangesStream(true, true, false));
        assertTrue(GameAdaptation.heatChangesStream(true, false, true));
    }
}
