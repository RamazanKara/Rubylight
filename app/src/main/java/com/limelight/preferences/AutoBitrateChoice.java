package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

/**
 * Whether Automatic bitrate is on for a PC. The Settings switch is the default for every PC; the
 * stream menu's toggle saves a choice for one PC (separately for conventional codecs and PyroWave,
 * which have their own controllers), and that choice wins over the default. Turning the Settings
 * switch on or off drops the PCs' own choices, so it takes effect everywhere at once.
 */
public final class AutoBitrateChoice {
    /** The Settings switch, stored with the main settings. */
    public static final String SETTINGS_KEY = "checkbox_auto_bitrate";
    public static final String CODEC_STORE = "AdaptiveBitrate";
    public static final String PYROWAVE_STORE = "PyroWaveBitrate";
    private static final String[] STORES = {CODEC_STORE, PYROWAVE_STORE};

    private AutoBitrateChoice() {
    }

    public static boolean isOn(Context context, String store, String pcUuid) {
        SharedPreferences saved = context.getSharedPreferences(store, Context.MODE_PRIVATE);
        if (pcUuid != null && saved.contains(pcUuid)) {
            return saved.getBoolean(pcUuid, false);
        }
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(SETTINGS_KEY, false);
    }

    /** The stream menu's toggle: this PC only. */
    public static void setForPc(Context context, String store, String pcUuid, boolean on) {
        context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().putBoolean(pcUuid, on).apply();
    }

    /** The Settings switch changed: every PC follows it again. */
    public static void clearPcChoices(Context context) {
        for (String store : STORES) {
            context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().apply();
        }
    }

    public static void forget(Context context, String pcUuid) {
        for (String store : STORES) {
            context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().remove(pcUuid).apply();
        }
    }
}
