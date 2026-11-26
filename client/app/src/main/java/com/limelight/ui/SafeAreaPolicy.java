package com.limelight.ui;

/**
 * Java port of nightjar-policy's safe_area (client/protocol/policy/src/safe_area.rs): Safe fit's
 * insets from a camera cutout and rounded screen corners, and where a picture the player moved by
 * hand (Picture position) lands. Integer pixels only, so both stay equal; PolicyTraceTest replays
 * tests/traces/safe_area against this class.
 * <p>
 * Rotations are Surface.ROTATION_0..ROTATION_270. A hand offset is kept in the panel's natural
 * orientation, in thousandths of the axis it moves along, so it stays on the same physical side
 * when the phone is turned over.
 */
public final class SafeAreaPolicy {
    /** The furthest a hand offset moves the picture, in thousandths of the screen's side. */
    public static final int MAX_OFFSET_PERMILLE = 150;

    private SafeAreaPolicy() {}

    /** How far a rectangle keeps from a corner of this radius to stay on the lit panel (0.293 r, rounded up). */
    public static int cornerInset(int radius) {
        if (radius <= 0) return 0;
        return (int) (((long) radius * 293 + 999) / 1000);
    }

    /** Safe fit's inset on one edge: past the cutout on that edge and clear of its two corners. */
    public static int edgeInset(int cutout, int cornerA, int cornerB) {
        return Math.max(Math.max(cutout, 0), Math.max(cornerInset(cornerA), cornerInset(cornerB)));
    }

    public static int clampOffset(int permille) {
        return Math.max(-MAX_OFFSET_PERMILLE, Math.min(MAX_OFFSET_PERMILLE, permille));
    }

    /**
     * Pixels to move a picture along one axis: permille of the screen's side, kept so the picture
     * (at start, size long) never leaves the screen. A covering picture does not move.
     */
    public static int offsetPx(int permille, int window, int start, int size) {
        if (window <= 0 || size <= 0 || size >= window) return 0;
        long raw = (long) clampOffset(permille) * window / 1000;
        long low = Math.min(0, -start);
        long high = Math.max(0, window - start - size);
        return (int) Math.max(low, Math.min(high, raw));
    }

    public static int screenOffsetX(int naturalX, int naturalY, int rotation) {
        switch (rotation) {
            case 1: return naturalY;
            case 2: return -naturalX;
            case 3: return -naturalY;
            default: return naturalX;
        }
    }

    public static int screenOffsetY(int naturalX, int naturalY, int rotation) {
        switch (rotation) {
            case 1: return -naturalX;
            case 2: return -naturalY;
            case 3: return naturalX;
            default: return naturalY;
        }
    }

    public static int naturalOffsetX(int screenX, int screenY, int rotation) {
        switch (rotation) {
            case 1: return -screenY;
            case 2: return -screenX;
            case 3: return screenY;
            default: return screenX;
        }
    }

    public static int naturalOffsetY(int screenX, int screenY, int rotation) {
        switch (rotation) {
            case 1: return screenX;
            case 2: return -screenY;
            case 3: return -screenX;
            default: return screenY;
        }
    }
}
