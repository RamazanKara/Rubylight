package com.limelight.binding.input.pad;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class PadControlViewRedrawTest {
    @Test
    public void aStickMoveRedrawsTheBaseOnlyWhileItsArcIsOrWasDrawn() {
        // Still at rest and no button changed: the base has nothing new to show.
        assertFalse(PadControlView.baseNeedsRedraw(false, false, false));
        // The arc starts or stops with the move, so the base must redraw either way.
        assertTrue(PadControlView.baseNeedsRedraw(false, false, true));
        assertTrue(PadControlView.baseNeedsRedraw(false, true, false));
        // A button shown on the base changed.
        assertTrue(PadControlView.baseNeedsRedraw(true, false, false));
    }
}
