package com.limelight.binding.input.touch;

/** The part of the screen where a trackpad gesture may start, as fractions of the screen. */
public final class TrackpadArea {
    public static final String FULL = "full";
    public static final String RIGHT_HALF = "right_half";
    public static final String LEFT_HALF = "left_half";
    public static final String BOTTOM_HALF = "bottom_half";
    public static final String TOP_HALF = "top_half";

    private final float left, top, right, bottom;

    private TrackpadArea(float left, float top, float right, float bottom) {
        this.left = left;
        this.top = top;
        this.right = right;
        this.bottom = bottom;
    }

    public static TrackpadArea forPreset(String preset) {
        if (RIGHT_HALF.equals(preset)) return new TrackpadArea(.5f, 0, 1, 1);
        if (LEFT_HALF.equals(preset)) return new TrackpadArea(0, 0, .5f, 1);
        if (BOTTOM_HALF.equals(preset)) return new TrackpadArea(0, .5f, 1, 1);
        if (TOP_HALF.equals(preset)) return new TrackpadArea(0, 0, 1, .5f);
        return new TrackpadArea(0, 0, 1, 1);
    }

    public boolean isFull() {
        return left == 0 && top == 0 && right == 1 && bottom == 1;
    }

    /** Whether (x, y) inside a width by height screen lies in the area; edges belong to it. */
    public boolean contains(float x, float y, float width, float height) {
        if (isFull()) return true;
        if (!(width > 0 && height > 0)) return true;
        float fx = x / width, fy = y / height;
        return fx >= left && fx <= right && fy >= top && fy <= bottom;
    }
}
