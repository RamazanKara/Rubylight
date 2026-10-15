package com.limelight.binding.input.touch;

import android.content.SharedPreferences;

/**
 * The finger touch mode. Older versions store it as two switches (trackpad, native touch); the
 * {@link #PREF} value adds the newer modes and is trusted only while it agrees with them, so a mode
 * picked in an older version still wins after an upgrade.
 */
public final class TouchMode {
    public static final String PREF = "touch_mode";
    public static final String TRACKPAD_PREF = "checkbox_touchscreen_trackpad";
    public static final String NATIVE_TOUCH_PREF = "checkbox_native_touch";

    // Positions in the stream_touch_modes array.
    public static final int TRACKPAD = 0;
    public static final int DIRECT = 1;
    public static final int MULTI_TOUCH = 2;
    public static final int LOCAL_CURSOR = 3;
    public static final int OFF = 4;

    private static final String[] VALUES = {"trackpad", "direct", "multi_touch", "local_cursor", "off"};

    private TouchMode() { }

    public static int resolve(String stored, boolean trackpad, boolean nativeTouch) {
        int legacy = trackpad ? TRACKPAD : nativeTouch ? MULTI_TOUCH : DIRECT;
        int mode = indexOf(stored);
        if (mode < 0) return legacy;
        // The native touch switch only counts while the trackpad switch is off, as in older versions.
        boolean agrees = legacyTrackpad(mode) == trackpad && (trackpad || legacyNativeTouch(mode) == nativeTouch);
        return agrees ? mode : legacy;
    }

    public static String value(int mode) {
        return VALUES[mode];
    }

    /** Stores the mode, with the switches an older version reads. */
    public static void save(SharedPreferences.Editor editor, int mode) {
        editor.putString(PREF, value(mode))
                .putBoolean(TRACKPAD_PREF, legacyTrackpad(mode))
                .putBoolean(NATIVE_TOUCH_PREF, legacyNativeTouch(mode));
    }

    /** What an older version reads: the newer modes fall back to Trackpad there. */
    public static boolean legacyTrackpad(int mode) {
        return mode == TRACKPAD || mode == LOCAL_CURSOR || mode == OFF;
    }

    public static boolean legacyNativeTouch(int mode) {
        return mode == MULTI_TOUCH;
    }

    /** Finger movement is relative to where the finger lands. */
    public static boolean isRelative(int mode) {
        return mode == TRACKPAD || mode == LOCAL_CURSOR;
    }

    private static int indexOf(String stored) {
        if (stored == null) return -1;
        for (int i = 0; i < VALUES.length; i++) {
            if (VALUES[i].equals(stored)) return i;
        }
        return -1;
    }
}
