package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * The performance overlay as one choice: Off, or one of its modes. It is stored as before, the on/off switch under
 * {@code checkbox_enable_perf_overlay} and the mode under {@code performance_overlay_mode}, so older values and the
 * stream menu keep working. Choosing Off leaves the mode as it was, so turning the overlay on again restores it.
 */
final class OverlayChoice {
    static final String ENABLED_KEY = "checkbox_enable_perf_overlay";
    static final String MODE_KEY = PreferenceConfiguration.OVERLAY_MODE_PREF_STRING;
    /** The modes in the order of the picker after Off; the values stored under {@code performance_overlay_mode}. */
    static final String[] MODES = {"compact", "statistics", "debug"};

    private OverlayChoice() { }

    /** 0 for Off, else 1 + the index in {@link #MODES}; an unknown stored mode counts as the default, Compact. */
    static int read(SharedPreferences prefs) {
        if (!prefs.getBoolean(ENABLED_KEY, false)) return 0;
        String mode = prefs.getString(MODE_KEY, MODES[0]);
        for (int i = 0; i < MODES.length; i++) {
            if (MODES[i].equals(mode)) return i + 1;
        }
        return 1;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(ENABLED_KEY, choice != 0);
        if (choice != 0) editor.putString(MODE_KEY, MODES[choice - 1]);
    }
}
