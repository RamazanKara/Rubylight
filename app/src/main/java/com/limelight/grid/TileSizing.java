package com.limelight.grid;

/**
 * The height of a library tile's artwork.
 *
 * <p>On a TV it follows the tile's width only, so a narrower (smaller) tile is always shorter than a wider
 * one. The heights used to be fixed per layout, and on a TV the "compact" 112 dp height applied to large
 * tiles alone: the smaller tiles kept a 4:3 portrait cover and ended up taller than the large ones.
 *
 * <p>Phones keep what earlier releases did: a 4:3 portrait cover from the column width, and only the large
 * tiles of a short viewport (a phone in landscape) use the fixed compact height.
 */
final class TileSizing {
    private static final int PHONE_COMPACT_DP = 112;
    private static final int PHONE_SMALL_DP = 144;
    private static final int PHONE_LARGE_DP = 224;

    private TileSizing() {}

    /**
     * @param columnWidthPx the grid's stretched column width, or 0 before it has one
     * @param shortViewport a grid too short for portrait art: a 1080p TV below its header, or a phone in landscape
     */
    static int artHeightPx(int columnWidthPx, boolean shortViewport, boolean television, boolean small, float density) {
        if (television) {
            int width = columnWidthPx > 0 ? columnWidthPx : Math.round((small ? 104 : 160) * density);
            // A TV in a short viewport shows landscape art so that a whole row, its title included, stays on screen.
            return Math.round(width * (shortViewport ? 2f / 3 : 4f / 3));
        }
        boolean compact = shortViewport && !small;
        if (!compact && columnWidthPx > 0) return Math.round(columnWidthPx * 4f / 3);
        return Math.round((compact ? PHONE_COMPACT_DP : small ? PHONE_SMALL_DP : PHONE_LARGE_DP) * density);
    }
}
