package com.limelight.grid;

import static org.junit.Assert.*;

import com.limelight.ui.AdaptiveLayout;

import org.junit.Test;

public class TileSizingTest {
    private static int height(boolean small, boolean television, boolean shortViewport, float density) {
        int widthPx = Math.round(AdaptiveLayout.cellWidthDp(false, small, television) * density);
        return TileSizing.artHeightPx(widthPx, shortViewport, television, small, density);
    }

    /** "Smaller tiles" on: shorter and narrower than the large tile on every TV, and on a phone held upright. */
    @Test
    public void smallTilesAreShorterThanLargeOnes() {
        for (boolean television : new boolean[] {false, true}) {
            for (boolean shortViewport : new boolean[] {false, true}) {
                if (!television && shortViewport) continue; // phones in landscape: see below
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
    public void aWiderTvColumnIsNeverShorter() {
        for (boolean shortViewport : new boolean[] {false, true}) {
            int previous = 0;
            for (int width = 100; width <= 700; width += 7) {
                int height = TileSizing.artHeightPx(width, shortViewport, true, false, 2f);
                assertTrue(height >= previous);
                previous = height;
            }
        }
    }

    @Test
    public void aShortTvViewportShowsLandscapeArtAndATallOnePortrait() {
        assertEquals(200, TileSizing.artHeightPx(300, true, true, false, 2f));
        assertEquals(400, TileSizing.artHeightPx(300, false, true, false, 2f));
    }

    /** Phones are sized as in earlier releases: portrait 4:3, and a fixed compact height for large tiles in landscape. */
    @Test
    public void phonesKeepTheirEarlierArtHeights() {
        assertEquals(400, TileSizing.artHeightPx(300, false, false, false, 2f));
        assertEquals(400, TileSizing.artHeightPx(300, false, false, true, 2f));
        assertEquals(400, TileSizing.artHeightPx(300, true, false, true, 2f));
        assertEquals(224, TileSizing.artHeightPx(300, true, false, false, 2f));
        assertEquals(224, TileSizing.artHeightPx(0, false, false, false, 1f));
        assertEquals(144 * 2, TileSizing.artHeightPx(0, false, false, true, 2f));
    }
}
