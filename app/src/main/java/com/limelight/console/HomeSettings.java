package com.limelight.console;

import android.content.Context;
import android.content.Intent;
import android.preference.PreferenceManager;

import com.limelight.PcView;
import com.limelight.nativeapi.ConsoleRepository;

/** Whether the app opens on Home, and the way back to the classic list of PCs. */
public final class HomeSettings {
    public static final String PREF_KEY = "checkbox_console_home";
    /** Set on an intent for {@link PcView} that must stay on the list of PCs. */
    public static final String EXTRA_STAY = "com.limelight.console.STAY_ON_PC_LIST";

    private HomeSettings() {
    }

    /** The setting; on unless the player turned it off. */
    public static boolean enabled(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(PREF_KEY, true);
    }

    /**
     * Home is the launch screen when the setting is on and a paired host has a native library to show,
     * live or saved. With only classic hosts the app starts on the list of PCs, as it always did.
     */
    public static boolean opensOnHome(Context context) {
        return enabled(context) && ConsoleRepository.hasAnyLibrary(context);
    }

    /** The list of PCs, which Home's Hosts and back-to-hosts actions open without redirecting to Home. */
    public static Intent pcList(Context context) {
        return new Intent(context, PcView.class).putExtra(EXTRA_STAY, true);
    }
}
