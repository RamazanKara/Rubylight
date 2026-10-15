package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.HashSet;
import java.util.Set;

/**
 * The games, per PC, whose stream quits them on the PC when the player leaves it. A game is kept
 * under its UUID and its numeric ID: hosts that report UUIDs are matched by UUID, others by ID.
 */
public final class AppQuitOnLeave {
    private static final String PREFERENCES = "AppQuitOnLeave";

    private AppQuitOnLeave() {}

    static Set<String> keys(String appUuid, int appId) {
        Set<String> keys = new HashSet<>();
        if (appUuid != null && !appUuid.isEmpty()) keys.add("uuid:" + appUuid);
        if (appId > 0) keys.add("id:" + appId);
        return keys;
    }

    static boolean contains(Set<String> stored, String appUuid, int appId) {
        if (stored == null) return false;
        // An ID can be reused for another game on hosts with UUIDs, so a known UUID decides alone.
        if (appUuid != null && !appUuid.isEmpty()) return stored.contains("uuid:" + appUuid);
        return appId > 0 && stored.contains("id:" + appId);
    }

    static Set<String> with(Set<String> stored, String appUuid, int appId, boolean enabled) {
        Set<String> next = stored == null ? new HashSet<>() : new HashSet<>(stored);
        if (enabled) next.addAll(keys(appUuid, appId));
        else next.removeAll(keys(appUuid, appId));
        return next;
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    public static boolean isEnabled(Context context, String pcUuid, String appUuid, int appId) {
        return pcUuid != null && contains(prefs(context).getStringSet(pcUuid, null), appUuid, appId);
    }

    public static void setEnabled(Context context, String pcUuid, String appUuid, int appId, boolean enabled) {
        if (pcUuid == null) return;
        Set<String> next = with(prefs(context).getStringSet(pcUuid, null), appUuid, appId, enabled);
        SharedPreferences.Editor editor = prefs(context).edit();
        if (next.isEmpty()) editor.remove(pcUuid);
        else editor.putStringSet(pcUuid, next);
        editor.apply();
    }

    public static void forget(Context context, String pcUuid) {
        if (pcUuid != null) prefs(context).edit().remove(pcUuid).apply();
    }
}
