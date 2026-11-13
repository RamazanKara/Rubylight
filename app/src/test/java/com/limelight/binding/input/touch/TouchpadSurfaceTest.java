package com.limelight.binding.input.touch;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class TouchpadSurfaceTest {
    @Test public void positionsGoOnTheSurfaceAndTheRangeGivesTheSize() {
        assertEquals(0.5f, TouchpadSurface.unit(600, 0, 1200), 0f);
        assertEquals(0f, TouchpadSurface.unit(-5, 0, 1200), 0f);
        assertEquals(1f, TouchpadSurface.unit(1300, 0, 1200), 0f);
        assertEquals(0f, TouchpadSurface.unit(5, 3, 3), 0f);
        // 1200 units at 12 per millimetre: a 100 mm pad.
        assertEquals(100, TouchpadSurface.lengthMm(1200, 12));
        assertEquals(0, TouchpadSurface.lengthMm(1200, 0));
        assertEquals(0, TouchpadSurface.lengthMm(0, 12));
    }
}
