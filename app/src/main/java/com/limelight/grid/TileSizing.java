package com.limelight.grid;

/**
 * The height of a library tile's artwork.
 *
 * <p>On a TV it is a 2:3 poster, from the poster's own width only (the column minus the tile's glow padding), so
 * a narrower (smaller) tile is always shorter than a wider one and the height does not depend on the viewport.
 * The heights used to be fixed per layout, and on a TV the "compact" 112 dp height applied to large
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
     * @param columnWidthPx the width the art is laid out in (a TV's column less its glow padding), or 0 before it has one
     * @param shortViewport a grid too short for portrait art: a phone in landscape. A TV shows posters either way
     */
    static int artHeightPx(int columnWidthPx, boolean shortViewport, boolean television, boolean small, float density) {
        if (television) {
            int width = columnWidthPx > 0 ? columnWidthPx : Math.round((small ? 112 : 168) * density);
            return Math.round(width * 3f / 2);
        }
        boolean compact = shortViewport && !small;
        if (!compact && columnWidthPx > 0) return Math.round(columnWidthPx * 4f / 3);
        return Math.round((compact ? PHONE_COMPACT_DP : small ? PHONE_SMALL_DP : PHONE_LARGE_DP) * density);
    }
}
