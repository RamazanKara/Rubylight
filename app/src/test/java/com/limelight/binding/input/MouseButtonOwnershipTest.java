package com.limelight.binding.input;

import org.junit.Test;
import static org.junit.Assert.*;

public class MouseButtonOwnershipTest {
    @Test public void physicalAndOverlayReleaseOnlyWhenBothHaveReleased() {
        for (boolean firstOverlay : new boolean[] {true, false}) {
            MouseButtonOwnership state = new MouseButtonOwnership();
            assertEquals(1, state.update(1, true, firstOverlay));
            assertEquals(0, state.update(1, true, !firstOverlay));
            assertEquals(0, state.update(1, false, firstOverlay));
            assertEquals(-1, state.update(1, false, !firstOverlay));
            assertEquals(0, state.update(1, false, firstOverlay));
        }
    }
    @Test public void repeatedDownInvalidButtonsAndIndependentButtons() {
        MouseButtonOwnership state = new MouseButtonOwnership();
        assertEquals(1, state.update(3, true, true));
        assertEquals(0, state.update(3, true, true));
        assertEquals(1, state.update(1, true, false));
        assertEquals(-1, state.update(3, false, true));
        assertEquals(0, state.update(0, true, true));
        assertEquals(0, state.update(6, true, true));
        assertEquals(-1, state.update(1, false, false));
    }
}
