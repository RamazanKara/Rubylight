package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.ArrayList;
import java.util.List;

/**
 * One game's own stream values, per PC: the layer above the PC's values. Stored like
 * {@link AppQuitOnLeave}: under the game's UUID and its numeric ID; a known UUID decides alone.
 */
public final class AppStreamSettings {
    private static final String PREFERENCES = "AppStreamSettings";

    private AppStreamSettings() {}

    static List<String> keys(String pcUuid, String appUuid, int appId) {
        List<String> keys = new ArrayList<>();
        if (appUuid != null && !appUuid.isEmpty()) keys.add(pcUuid + "/uuid:" + appUuid);
        if (appId > 0) keys.add(pcUuid + "/id:" + appId);
        return keys;
    }

    static String lookupKey(String pcUuid, String appUuid, int appId) {
        if (appUuid != null && !appUuid.isEmpty()) return pcUuid + "/uuid:" + appUuid;
        return appId > 0 ? pcUuid + "/id:" + appId : null;
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The stored values, unread, or null when the game has none. */
    public static String raw(Context context, String pcUuid, String appUuid, int appId) {
        if (pcUuid == null) return null;
        String key = lookupKey(pcUuid, appUuid, appId);
        return key == null ? null : prefs(context).getString(key, null);
    }

    /** The game's values; empty when it has none or they can't be read. */
    public static HostStreamProfile load(Context context, String pcUuid, String appUuid, int appId) {
        String raw = raw(context, pcUuid, appUuid, appId);
        if (raw == null) return new HostStreamProfile();
        try {
            return HostStreamProfile.deserialize(raw);
        } catch (IllegalArgumentException e) {
            return new HostStreamProfile();
        }
    }

    public static void save(Context context, String pcUuid, String appUuid, int appId, HostStreamProfile values) {
        if (pcUuid == null) return;
        SharedPreferences.Editor editor = prefs(context).edit();
        for (String key : keys(pcUuid, appUuid, appId)) {
            if (values.isEmpty()) editor.remove(key);
            else editor.putString(key, values.serialize());
        }
        editor.apply();
    }

    public static void forget(Context context, String pcUuid) {
        if (pcUuid == null) return;
        SharedPreferences prefs = prefs(context);
        SharedPreferences.Editor editor = prefs.edit();
        for (String key : prefs.getAll().keySet()) {
            if (key.startsWith(pcUuid + "/")) editor.remove(key);
        }
        editor.apply();
    }
}
