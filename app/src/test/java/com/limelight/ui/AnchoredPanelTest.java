package com.limelight.ui;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** A panel opened from the top row hangs under its button, as Home's PC sheet hangs under the capsule. */
public class AnchoredPanelTest {
    @Test
    public void thePanelEndsOnItsAnchorsEndBelowIt() {
        float density = 2f;
        int width = 720;
        int edge = 24;
        // A button at the end of the top row: the panel's end meets the button's, 8 dp under it.
        int[] at = ActionSheet.PanelFrame.underAnchor(new int[] {1800, 60}, 96, 96, new int[] {0, 0}, 1920, width, edge, density, false);
        assertEquals(1800 + 96 - width, at[0]);
        assertEquals(60 + 96 + 16, at[1]);
        // Near the start edge it stays inside the window; in RTL its start meets the button's.
        assertEquals(edge, ActionSheet.PanelFrame.underAnchor(new int[] {100, 60}, 96, 96, new int[] {0, 0}, 1920, width, edge, density, false)[0]);
        assertEquals(100, ActionSheet.PanelFrame.underAnchor(new int[] {100, 60}, 96, 96, new int[] {0, 0}, 1920, width, edge, density, true)[0]);
    }
}
