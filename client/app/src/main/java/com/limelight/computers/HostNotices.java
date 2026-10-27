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
    public enum Notice {
        VIRTUAL_DISPLAY_UNAVAILABLE, AUTOMATIC_QUALITY_UNAVAILABLE, PYROWAVE_UNAVAILABLE,
        // HDR on this PC: the screen, Android, the decoder or the PC lacks it.
        HDR_UNAVAILABLE,
        // Experimental full chroma that cannot run in HDR on this device (the stream is 8-bit SDR).
        PACKED_HDR_UNAVAILABLE,
        // PyroWave HDR that this device cannot show although the PC offers it.
        PYROWAVE_HDR_UNAVAILABLE,
        // A codec forced in settings that this device cannot decode.
        CODEC_UNAVAILABLE,
        // GeForce Experience too old for 4K.
        HOST_4K_UNAVAILABLE,
        // Multi-touch the host does not take, so touch drives the mouse.
        NATIVE_TOUCH_UNAVAILABLE,
        // 10-bit SDR is on but this device cannot offer it.
        SDR10_UNAVAILABLE,
        // 10-bit SDR was offered and this PC streams 8-bit.
        SDR10_NOT_ACCEPTED,
        // 10-bit SDR was asked for and the PC sends HDR because its display is in HDR.
        SDR10_HOST_HDR
    }

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
