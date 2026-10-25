package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * The packed full-chroma modes each PC offered in its last RTSP DESCRIBE, kept per PC. The launch
 * comes before DESCRIBE, so this is what a PC without {@code Packed444Modes} in its serverinfo is
 * known to offer when the next stream decides whether to ask for 10-bit.
 */
public final class HostPackedModes {
    static final String PREFERENCES = "HostPacked444Modes";
    static final int ALL_MODES = 0xF;

    private HostPackedModes() {
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The modes this PC's last DESCRIBE offered, or -1 when none was seen. */
    public static int cached(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return -1;
        int modes = preferences(context).getInt(hostUuid, -1);
        return modes > 0 && modes <= ALL_MODES ? modes : -1;
    }

    /**
     * Remembers what a DESCRIBE offered. No modes (or an attribute the client could not read)
     * forgets the PC instead: that is a host with full chroma off, which serverinfo then omits too.
     * A negative value is no DESCRIBE at all (the C engine does not read it) and changes nothing.
     */
    public static void store(Context context, String hostUuid, int describeModes) {
        if (context == null || hostUuid == null || describeModes < 0) return;
        SharedPreferences preferences = preferences(context);
        if (describeModes > 0 && describeModes <= ALL_MODES) {
            if (preferences.getInt(hostUuid, -1) != describeModes) {
                preferences.edit().putInt(hostUuid, describeModes).apply();
            }
        } else if (preferences.contains(hostUuid)) {
            preferences.edit().remove(hostUuid).apply();
        }
    }

    /** What the next launch takes the PC to offer: its serverinfo when it says, else the cache, else -1. */
    public static int known(int serverInfoModes, int cachedModes) {
        return serverInfoModes >= 0 ? serverInfoModes : cachedModes;
    }
}
