package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * Variable refresh rate as one choice: Off, On (each frame at the game's timing) or Fastest (each frame on
 * arrival). It is stored as before, in {@code checkbox_vrr} (off by default) and {@code checkbox_real_vrr} (Real VRR,
 * on by default), so older values and the stream configuration keep working. Off leaves Real VRR as it was, so
 * turning VRR back on returns to it. Desktop Nightjar's row writes {@code vrr} and {@code present_at_host_times}
 * the same way.
 */
final class VrrChoice {
    static final int OFF = 0;
    static final int ON = 1;
    static final int FASTEST = 2;
    /** The values of {@code vrr_choice_values}, in the order of the picker. */
    static final String[] VALUES = {"off", "on", "fastest"};

    private VrrChoice() { }

    /** The choice for a VRR switch and a Real VRR switch. */
    static int of(boolean vrr, boolean realVrr) {
        return !vrr ? OFF : realVrr ? ON : FASTEST;
    }

    /** The stored choice, offered or not. */
    static int read(SharedPreferences prefs) {
        return of(prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false),
                prefs.getBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, true));
    }

    /**
     * The choice as the screens show it: Off where VRR is not offered (SettingRules.vrrOn), while the stored
     * choice is kept for a later Android that offers it.
     */
    static int shown(SharedPreferences prefs, boolean vrrOffered) {
        return vrrOffered ? read(prefs) : OFF;
    }

    /** The choice for a value of {@link #VALUES}; an unknown one reads as Off. */
    static int ofValue(String value) {
        for (int i = 0; i < VALUES.length; i++) {
            if (VALUES[i].equals(value)) return i;
        }
        return OFF;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(PreferenceConfiguration.VRR_PREF_STRING, choice != OFF);
        if (choice != OFF) editor.putBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, choice == ON);
    }

    /**
     * Puts the choice on a PC, game or profile layer, both values together: On and Fastest set VRR and Real VRR,
     * Off sets VRR off and leaves Real VRR to the layers below, and null sets neither.
     */
    static void save(HostStreamProfile own, Integer choice) {
        own.vrr = choice == null ? null : choice != OFF;
        own.realVrr = choice == null || choice == OFF ? null : choice == ON;
    }
}
