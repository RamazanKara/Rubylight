package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.nvstream.http.HostCapabilities;

/**
 * The last capabilities each PC reported, keyed by host ID, so menus and per-PC screens are
 * right while the PC is offline. Only serverinfo evidence is stored: no address, MAC or name.
 */
public final class HostCapabilityCache {
    static final String PREFERENCES = "HostCapabilities";

    private HostCapabilityCache() {
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** What the PC last reported, or {@link HostCapabilities#UNKNOWN} if it never answered. */
    public static HostCapabilities load(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return HostCapabilities.UNKNOWN;
        }
        return HostCapabilities.fromJson(preferences(context).getString(hostUuid, null));
    }

    /** Remembers known capabilities; polling repeats them, so unchanged ones are not written again. */
    public static void save(Context context, String hostUuid, HostCapabilities caps) {
        if (hostUuid == null || caps == null || !caps.isKnown()) {
            return;
        }
        String json = caps.toJson();
        SharedPreferences preferences = preferences(context);
        if (!json.equals(preferences.getString(hostUuid, null))) {
            preferences.edit().putString(hostUuid, json).apply();
        }
    }

    public static void forget(Context context, String hostUuid) {
        if (hostUuid != null) {
            preferences(context).edit().remove(hostUuid).apply();
        }
    }
}
