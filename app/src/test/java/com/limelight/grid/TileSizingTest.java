package com.limelight.grid;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class TileSizingTest {
    /** Covers and no-art tiles share one 2:3 slot, on every form factor and in a short viewport too. */
    @Test
    public void aTileIsAlwaysTwoToThree() {
        for (int width : new int[] {100, 216, 300, 451}) {
            for (boolean small : new boolean[] {false, true}) {
                assertEquals(Math.round(width * 3f / 2), TileSizing.artHeightPx(width, small, 2f));
            }
        }
    }

    @Test
    public void beforeItHasAWidthATileAssumesTheColumnItWillGet() {
        assertEquals(Math.round(168 * 2f * 3 / 2), TileSizing.artHeightPx(0, false, 2f));
        assertEquals(Math.round(112 * 2f * 3 / 2), TileSizing.artHeightPx(0, true, 2f));
    }
}
