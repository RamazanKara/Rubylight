package com.limelight.grid;

/** The height of a library tile's artwork: a 2:3 poster from the art's own width, on every form factor, so covers and no-art tiles share a slot. */
final class TileSizing {
    private static final int SMALL_DP = 112;
    private static final int LARGE_DP = 168;

    private TileSizing() {}

    /** @param columnWidthPx the width the art is laid out in, or 0 before it has one */
    static int artHeightPx(int columnWidthPx, boolean small, float density) {
        int width = columnWidthPx > 0 ? columnWidthPx : Math.round((small ? SMALL_DP : LARGE_DP) * density);
        return Math.round(width * 3f / 2);
    }
}
