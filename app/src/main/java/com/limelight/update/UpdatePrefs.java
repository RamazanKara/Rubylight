package com.limelight.update;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

/** The update switch in Settings, plus when the last check ran and which version was skipped. */
public final class UpdatePrefs {
    /** The Settings switch; new key, on by default. */
    public static final String AUTO_CHECK_PREF_STRING = "checkbox_check_updates";
    public static final boolean DEFAULT_AUTO_CHECK = true;

    // Kept apart from the Settings file so Reset all settings leaves the skipped version alone.
    private static final String STATE_FILE = "update_state";
    private static final String LAST_CHECK = "last_check";
    private static final String SKIPPED_VERSION = "skipped_version";

    private final SharedPreferences settings;
    private final SharedPreferences state;

    public UpdatePrefs(Context context) {
        Context app = context.getApplicationContext() != null ? context.getApplicationContext() : context;
        settings = PreferenceManager.getDefaultSharedPreferences(app);
        state = app.getSharedPreferences(STATE_FILE, Context.MODE_PRIVATE);
    }

    public boolean isAutoCheck() {
        return settings.getBoolean(AUTO_CHECK_PREF_STRING, DEFAULT_AUTO_CHECK);
    }

    public long lastCheck() {
        return state.getLong(LAST_CHECK, 0);
    }

    public void markChecked(long nowMs) {
        state.edit().putLong(LAST_CHECK, nowMs).apply();
    }

    public String skippedVersion() {
        return state.getString(SKIPPED_VERSION, "");
    }

    public void skip(String version) {
        state.edit().putString(SKIPPED_VERSION, SemVer.normalize(version)).apply();
    }
}
