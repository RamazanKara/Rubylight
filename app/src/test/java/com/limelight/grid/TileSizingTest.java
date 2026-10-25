package com.limelight.grid;

import static org.junit.Assert.*;

import com.limelight.ui.AdaptiveLayout;

import org.junit.Test;

public class TileSizingTest {
    private static int height(boolean small, boolean television, boolean shortViewport, float density) {
        int widthPx = Math.round(AdaptiveLayout.cellWidthDp(false, small, television) * density);
        return TileSizing.artHeightPx(widthPx, shortViewport);
    }

    /** "Smaller tiles" on: shorter and narrower than the large tile, on every screen. */
    @Test
    public void smallTilesAreShorterThanLargeOnes() {
        for (boolean television : new boolean[] {false, true}) {
            for (boolean shortViewport : new boolean[] {false, true}) {
                for (float density : new float[] {1f, 2f, 2.625f, 3f}) {
                    int small = height(true, television, shortViewport, density);
                    int large = height(false, television, shortViewport, density);
                    String where = (television ? "tv" : "phone") + (shortViewport ? " short" : " tall") + " @" + density;
                    assertTrue(where + ": small " + small + " < large " + large, small < large);
                }
            }
        }
    }

    @Test
    public void aWiderColumnIsNeverShorter() {
        for (boolean shortViewport : new boolean[] {false, true}) {
            int previous = 0;
            for (int width = 100; width <= 700; width += 7) {
                int height = TileSizing.artHeightPx(width, shortViewport);
                assertTrue(height >= previous);
                previous = height;
            }
        }
    }

    @Test
    public void aShortViewportShowsLandscapeArtAndATallOnePortrait() {
        assertEquals(200, TileSizing.artHeightPx(300, true));
        assertEquals(400, TileSizing.artHeightPx(300, false));
    }
}
