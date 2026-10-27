package com.limelight.binding.input;

import org.junit.Test;
import static org.junit.Assert.*;

public class MouseButtonOwnershipTest {
    @Test public void desktopGameAndPhysicalHoldsAreIndependentInEveryReleaseOrder() {
        int[][] orders = {{0, 1, 2}, {0, 2, 1}, {1, 0, 2}, {1, 2, 0}, {2, 0, 1}, {2, 1, 0}};
        for (int[] order : orders) {
            MouseButtonOwnership state = new MouseButtonOwnership();
            assertEquals(1, state.update(1, true, 0));
            assertEquals(0, state.update(1, true, 1));
            assertEquals(0, state.update(1, true, 2));
            assertEquals(0, state.update(1, true, 2));
            assertEquals(0, state.update(1, false, order[0]));
            assertEquals(0, state.update(1, false, order[1]));
            assertEquals(-1, state.update(1, false, order[2]));
            assertEquals(0, state.update(1, false, order[2]));
        }
    }

    @Test public void invalidOwnerCannotReleaseAnotherSurface() {
        MouseButtonOwnership state = new MouseButtonOwnership();
        assertEquals(1, state.update(1, true, MouseButtonOwnership.DESKTOP_CONTROLS));
        assertEquals(0, state.update(1, false, -1));
        assertEquals(0, state.update(1, false, 3));
        assertEquals(-1, state.update(1, false, MouseButtonOwnership.DESKTOP_CONTROLS));
    }

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
