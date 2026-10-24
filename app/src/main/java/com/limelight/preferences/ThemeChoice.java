package com.limelight.preferences;

import android.content.SharedPreferences;

import com.limelight.ui.AppTheme;

/**
 * The app theme as one choice: System, Dark or OLED black. It is stored as before, as two switches
 * ({@code checkbox_always_dark} and {@code checkbox_oled_black}), so older values keep working. OLED black is a
 * darker variant of Dark, so choosing it sets both; a store with only OLED black set reads as OLED black too.
 */
final class ThemeChoice {
    static final int SYSTEM = 0;
    static final int DARK = 1;
    static final int OLED_BLACK = 2;

    private ThemeChoice() { }

    static int read(SharedPreferences prefs) {
        if (prefs.getBoolean(AppTheme.OLED_BLACK, false)) return OLED_BLACK;
        return prefs.getBoolean(AppTheme.ALWAYS_DARK, false) ? DARK : SYSTEM;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(AppTheme.ALWAYS_DARK, choice != SYSTEM);
        editor.putBoolean(AppTheme.OLED_BLACK, choice == OLED_BLACK);
    }
}
