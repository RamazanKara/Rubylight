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
    // The modes this client knows; bits a newer host adds are not its business.
    static final int ALL_MODES = 0xF;

    private HostPackedModes() {
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The modes this PC's last DESCRIBE offered, or -1 when none was seen. */
    public static int cached(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return -1;
        int stored = preferences(context).getInt(hostUuid, -1);
        int modes = stored > 0 ? stored & ALL_MODES : 0;
        return modes > 0 ? modes : -1;
    }

    /**
     * Remembers what a DESCRIBE offered. No modes (or an attribute the client could not read)
     * forgets the PC instead: that is a host with full chroma off, which serverinfo then omits too.
     * A negative value is no DESCRIBE at all (the C engine does not read it) and changes nothing.
     * Mode bits this client does not know are left out.
     */
    public static void store(Context context, String hostUuid, int describeModes) {
        if (context == null || hostUuid == null || describeModes < 0) return;
        describeModes &= ALL_MODES;
        SharedPreferences preferences = preferences(context);
        if (describeModes > 0) {
            if (preferences.getInt(hostUuid, -1) != describeModes || preferences.contains(refusedKey(hostUuid))) {
                preferences.edit().putInt(hostUuid, describeModes).remove(refusedKey(hostUuid)).apply();
            }
        } else if (preferences.contains(hostUuid)) {
            preferences.edit().remove(hostUuid).apply();
        }
    }

    private static final String REFUSED_PREFIX = "refused:";

    private static String refusedKey(String hostUuid) {
        return REFUSED_PREFIX + hostUuid;
    }

    /**
     * This PC refused a stream that asked for full chroma. A PC that does not say its modes in
     * serverinfo is then not asked again at every launch, which would launch the game, be refused
     * in the handshake and reconnect each time.
     */
    public static void markRefused(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return;
        SharedPreferences preferences = preferences(context);
        if (!preferences.getBoolean(refusedKey(hostUuid), false)) {
            preferences.edit().putBoolean(refusedKey(hostUuid), true).apply();
        }
    }

    /**
     * Whether this PC's last answer to a full chroma stream was a refusal. A stream that starts with
     * full chroma ({@link #store}) and {@link #clearRefusals} end it; serverinfo that names modes
     * outranks it at the launch.
     */
    public static boolean refused(Context context, String hostUuid) {
        return context != null && hostUuid != null && preferences(context).getBoolean(refusedKey(hostUuid), false);
    }

    /** Every PC is asked again: the user switched the setting, which is how to retry after changing a PC. */
    public static void clearRefusals(Context context) {
        if (context == null) return;
        SharedPreferences preferences = preferences(context);
        SharedPreferences.Editor editor = null;
        for (String key : preferences.getAll().keySet()) {
            if (key.startsWith(REFUSED_PREFIX)) {
                if (editor == null) editor = preferences.edit();
                editor.remove(key);
            }
        }
        if (editor != null) editor.apply();
    }

    /** What the next launch takes the PC to offer: its serverinfo when it says, else the cache, else -1. */
    public static int known(int serverInfoModes, int cachedModes) {
        return serverInfoModes >= 0 ? serverInfoModes : cachedModes;
    }
}
