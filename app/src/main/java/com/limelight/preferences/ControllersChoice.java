package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * Controllers as one choice: Off, One player (every controller as player 1) or A player each. Stored in two
 * switches: {@code checkbox_send_controllers} (new, on by default) and {@code checkbox_multi_controller}, the
 * older "each controller its own player" switch, so a store from before reads as it streamed: its switch off
 * reads as One player. Off leaves the player switch as it was, so turning controllers back on returns to it.
 * Desktop Nightjar's row writes {@code controllers} and {@code multi_controller} the same way.
 */
final class ControllersChoice {
    static final int OFF = 0;
    static final int ONE_PLAYER = 1;
    static final int PLAYER_EACH = 2;

    private ControllersChoice() { }

    static int read(SharedPreferences prefs) {
        if (!prefs.getBoolean(PreferenceConfiguration.SEND_CONTROLLERS_PREF_STRING, true)) return OFF;
        return prefs.getBoolean(PreferenceConfiguration.MULTI_CONTROLLER_PREF_STRING, true) ? PLAYER_EACH : ONE_PLAYER;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(PreferenceConfiguration.SEND_CONTROLLERS_PREF_STRING, choice != OFF);
        if (choice != OFF) editor.putBoolean(PreferenceConfiguration.MULTI_CONTROLLER_PREF_STRING, choice == PLAYER_EACH);
    }
}
