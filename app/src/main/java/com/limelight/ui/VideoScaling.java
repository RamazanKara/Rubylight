package com.limelight.ui;

import android.content.SharedPreferences;

/**
 * How the video fills the space it is given, and the way between a point on screen and a stream
 * pixel through that rectangle and pinch zoom. Fit and 1:1 can leave borders; Fill and 1:1 can crop.
 * Older versions store only the Stretch switch; the {@link #PREF} value adds the newer modes and is
 * trusted only while it agrees with that switch, so a choice made in an older version still wins.
 */
public final class VideoScaling {
    public static final String PREF = "video_scaling";
    public static final String STRETCH_PREF = "checkbox_stretch_video";
    public static final String ALIGNMENT_PREF = "video_alignment";

    // Positions in the video_scaling_modes array.
    public static final int FIT = 0;
    public static final int FILL = 1;
    public static final int STRETCH = 2;
    public static final int NATIVE = 3;

    // Positions in the video_alignment_values array.
    public static final int CENTER = 0;
    public static final int TOP = 1;

    private static final String[] VALUES = {"fit", "fill", "stretch", "native"};
    private static final String[] ALIGNMENTS = {"center", "top"};

    private int left, top, width, height;

    public static int resolve(String stored, boolean stretch) {
        int legacy = stretch ? STRETCH : FIT;
        int mode = indexOf(VALUES, stored);
        if (mode < 0) return legacy;
        return legacyStretch(mode) == stretch ? mode : legacy;
    }

    public static String value(int mode) {
        return VALUES[mode];
    }

    /** Stores the mode, with the switch an older version reads. */
    public static void save(SharedPreferences.Editor editor, int mode) {
        editor.putString(PREF, value(mode)).putBoolean(STRETCH_PREF, legacyStretch(mode));
    }

    /** What an older version reads: Fill and 1:1 keep the stream's shape, so they fall back to Fit there. */
    public static boolean legacyStretch(int mode) {
        return mode == STRETCH;
    }

    public static int alignment(String stored) {
        return Math.max(CENTER, indexOf(ALIGNMENTS, stored));
    }

    /** Fill and Stretch cover the whole space; the others leave borders on a screen of another shape. */
    public static boolean coversArea(int mode) {
        return mode == FILL || mode == STRETCH;
    }

    /** Picture-in-picture shows the whole stream: cropped or at 1:1, a small window would show only part of it. */
    public static int forPictureInPicture(int mode) {
        return mode == STRETCH ? STRETCH : FIT;
    }

    /** Top alignment moves a Fit or 1:1 picture up; Fill and Stretch have no border to move into. */
    public static boolean alignsTop(int mode, int alignment) {
        return alignment == TOP && (mode == FIT || mode == NATIVE);
    }

    /**
     * Places a video of videoWidth by videoHeight in an area of areaWidth by areaHeight. An unknown
     * video size fills the area. Centring rounds like FrameLayout gravity, so the result matches
     * where the view is laid out.
     */
    public void layout(int mode, int alignment, int videoWidth, int videoHeight, int areaWidth, int areaHeight) {
        int w = areaWidth, h = areaHeight;
        if (videoWidth > 0 && videoHeight > 0 && areaWidth > 0 && areaHeight > 0) {
            double aspect = (double) videoWidth / videoHeight;
            boolean areaWider = areaWidth > areaHeight * aspect;
            if (mode == FIT) {
                // Round, never truncate, and never exceed the space: a stream of the panel's exact size
                // (2184 / 1968 is not exact in floating point) must fill it rather than leave a 1 px line.
                if (areaWider) w = Math.min(areaWidth, (int) Math.round(areaHeight * aspect));
                else h = Math.min(areaHeight, (int) Math.round(areaWidth / aspect));
            } else if (mode == FILL) {
                if (areaWider) h = Math.max(areaHeight, (int) Math.round(areaWidth / aspect));
                else w = Math.max(areaWidth, (int) Math.round(areaHeight * aspect));
            } else if (mode == NATIVE) {
                w = videoWidth;
                h = videoHeight;
            }
        }
        width = w;
        height = h;
        left = (areaWidth - w) / 2;
        top = alignsTop(mode, alignment) ? 0 : (areaHeight - h) / 2;
    }

    public int left() { return left; }
    public int top() { return top; }
    public int width() { return width; }
    public int height() { return height; }

    /**
     * Screen position of a stream coordinate on one axis: through the video rectangle (start and
     * size on screen), then pinch zoom (scale and pan, relative to the rectangle).
     */
    public static float toScreen(float stream, int streamSize, float rectStart, float rectSize,
                                 float zoomScale, float zoomPan) {
        if (streamSize <= 0) return rectStart + zoomPan;
        return rectStart + zoomPan + stream / streamSize * rectSize * zoomScale;
    }

    /**
     * Stream coordinate under a screen position on one axis, the inverse of
     * {@link #toScreen}. A point on a border beside the video maps to the nearest edge.
     */
    public static float toStream(float screen, int streamSize, float rectStart, float rectSize,
                                 float zoomScale, float zoomPan) {
        if (streamSize <= 0 || !(rectSize > 0) || !(zoomScale > 0)) return 0;
        float local = (screen - rectStart - zoomPan) / zoomScale;
        local = Math.max(0, Math.min(rectSize, local));
        return local / rectSize * streamSize;
    }

    private static int indexOf(String[] values, String stored) {
        if (stored == null) return -1;
        for (int i = 0; i < values.length; i++) {
            if (values[i].equals(stored)) return i;
        }
        return -1;
    }
}
