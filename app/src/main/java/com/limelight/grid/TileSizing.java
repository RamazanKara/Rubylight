package com.limelight.grid;

/**
 * The height of a library tile's artwork. It follows the tile's width only, so a narrower (smaller)
 * tile is always shorter than a wider one. The heights used to be fixed per layout, and on a TV the
 * "compact" 112 dp height applied to large tiles alone: the smaller tiles kept a 4:3 portrait cover and
 * ended up taller than the large ones.
 */
final class TileSizing {
    private TileSizing() {}

    /**
     * Portrait cover on a roomy screen. A short viewport (a phone in landscape, a 1080p TV) shows
     * landscape art so that a whole row, its title included, stays on screen.
     */
    static int artHeightPx(int columnWidthPx, boolean shortViewport) {
        return Math.round(columnWidthPx * (shortViewport ? 2f / 3 : 4f / 3));
    }
}
