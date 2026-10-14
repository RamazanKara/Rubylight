package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Messages about something a PC lacks for good, shown once per PC instead of on every stream.
 * A notice is shown again only after the PC gained the feature and then lost it.
 */
public final class HostNotices {
    public enum Notice { VIRTUAL_DISPLAY_UNAVAILABLE, AUTOMATIC_QUALITY_UNAVAILABLE, PYROWAVE_UNAVAILABLE }

    static final String PREFERENCES = "HostNotices";

    private HostNotices() {
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    private static List<String> shown(SharedPreferences preferences, String hostUuid) {
        String stored = preferences.getString(hostUuid, "");
        return stored.isEmpty() ? new ArrayList<>() : new ArrayList<>(Arrays.asList(stored.split(",")));
    }

    /** True the first time for this PC, and remembered; false while it stays shown. */
    public static synchronized boolean claim(Context context, String hostUuid, Notice notice) {
        if (context == null || hostUuid == null) {
            return true;
        }
        SharedPreferences preferences = preferences(context);
        List<String> shown = shown(preferences, hostUuid);
        if (shown.contains(notice.name())) {
            return false;
        }
        shown.add(notice.name());
        preferences.edit().putString(hostUuid, android.text.TextUtils.join(",", shown)).apply();
        return true;
    }

    /** The PC has the feature now; if it loses it again, say so again. */
    public static synchronized void reset(Context context, String hostUuid, Notice notice) {
        if (context == null || hostUuid == null) {
            return;
        }
        SharedPreferences preferences = preferences(context);
        List<String> shown = shown(preferences, hostUuid);
        if (shown.remove(notice.name())) {
            if (shown.isEmpty()) {
                preferences.edit().remove(hostUuid).apply();
            } else {
                preferences.edit().putString(hostUuid, android.text.TextUtils.join(",", shown)).apply();
            }
        }
    }

    public static synchronized void forget(Context context, String hostUuid) {
        if (hostUuid != null) {
            preferences(context).edit().remove(hostUuid).apply();
        }
    }
}
