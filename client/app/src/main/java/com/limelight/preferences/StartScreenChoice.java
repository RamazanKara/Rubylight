package com.limelight.preferences;

import android.content.SharedPreferences;

import com.limelight.console.HomeSettings;
import com.limelight.console.OpenPlay;

/**
 * "When Nightjar opens" as one choice: Home, Show my PCs, Continue my last game or A chosen game. It is
 * stored as before, in {@code list_open_on_start} ({@code pcs}, {@code last}, {@code chosen}) and
 * {@code checkbox_console_home}, so older values and older versions keep working; nothing is rewritten on
 * load. The mapping is nightjar-policy's {@code open_play::StartScreen}, the one desktop Nightjar uses:
 * OpenPlayTest holds this class to protocol/android/src/open_play_contract.json.
 */
public final class StartScreenChoice {
    public static final String HOME = "home";
    public static final String PCS = "pcs";
    public static final String LAST = "last";
    public static final String CHOSEN = "chosen";
    /** The entries in the picker's order, {@code start_screen_names}. */
    static final String[] SCREENS = {HOME, PCS, LAST, CHOSEN};

    private StartScreenChoice() { }

    /** The choice the stored pair reads as: {@code last} and {@code chosen} whatever Home holds, else Home or the PCs. */
    public static String fromSaved(String open, boolean consoleHome) {
        if (LAST.equals(open)) return LAST;
        if (CHOSEN.equals(open)) return CHOSEN;
        return consoleHome ? HOME : PCS;
    }

    /** {@code list_open_on_start} for {@code screen}. */
    public static String openFor(String screen) {
        return LAST.equals(screen) || CHOSEN.equals(screen) ? screen : "pcs";
    }

    /** {@code checkbox_console_home} for {@code screen}: set by Home and Show my PCs, kept by the two games. */
    public static boolean consoleHomeFor(String screen, boolean consoleHome) {
        if (HOME.equals(screen)) return true;
        if (PCS.equals(screen)) return false;
        return consoleHome;
    }

    static String read(SharedPreferences prefs) {
        return fromSaved(prefs.getString(OpenPlay.SETTING, "pcs"), prefs.getBoolean(HomeSettings.PREF_KEY, true));
    }

    static int index(String screen) {
        for (int i = 0; i < SCREENS.length; i++) if (SCREENS[i].equals(screen)) return i;
        return 0;
    }

    static void save(SharedPreferences prefs, SharedPreferences.Editor editor, String screen) {
        editor.putString(OpenPlay.SETTING, openFor(screen));
        editor.putBoolean(HomeSettings.PREF_KEY, consoleHomeFor(screen, prefs.getBoolean(HomeSettings.PREF_KEY, true)));
    }
}
