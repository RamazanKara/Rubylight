package com.limelight.utils;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.Locale;

/**
 * Calibrated PyroWave frame paces, one per PC. A pace belongs to the path it was measured on, so it
 * is kept with this device's wired link speed toward the host at the time (0 on Wi-Fi) and is only
 * sent while that speed is unchanged.
 */
public final class PyroWavePaceStore {
    private static final String PREFERENCES = "pyrowave_pace";

    private PyroWavePaceStore() {
    }

    public static final class Calibration {
        public final int paceMbps;
        public final int clientLinkMbps;
        public final int hostLinkMbps;
        /** Every loss limit passed twice; otherwise the best of the tested paces. */
        public final boolean qualified;

        public Calibration(int paceMbps, int clientLinkMbps, int hostLinkMbps, boolean qualified) {
            this.paceMbps = paceMbps;
            this.clientLinkMbps = clientLinkMbps;
            this.hostLinkMbps = hostLinkMbps;
            this.qualified = qualified;
        }

        String serialize() {
            return String.format(Locale.ROOT, "1,%d,%d,%d,%d", paceMbps, clientLinkMbps, hostLinkMbps, qualified ? 1 : 0);
        }

        static Calibration deserialize(String value) {
            if (value == null) return null;
            String[] fields = value.split(",");
            if (fields.length != 5 || !"1".equals(fields[0])) return null;
            try {
                int pace = Integer.parseInt(fields[1]);
                int client = Integer.parseInt(fields[2]);
                int host = Integer.parseInt(fields[3]);
                if (pace <= 0 || pace > 400000 || client < 0 || host < 0) return null;
                return new Calibration(pace, client, host, "1".equals(fields[4]));
            } catch (NumberFormatException e) {
                return null;
            }
        }
    }

    public static Calibration get(Context context, String hostUuid) {
        if (hostUuid == null) return null;
        return Calibration.deserialize(prefs(context).getString(hostUuid, null));
    }

    public static void put(Context context, String hostUuid, Calibration calibration) {
        if (hostUuid == null) return;
        prefs(context).edit().putString(hostUuid, calibration.serialize()).apply();
    }

    public static void clear(Context context, String hostUuid) {
        if (hostUuid == null) return;
        prefs(context).edit().remove(hostUuid).apply();
    }

    /** The pace to send for this stream, or 0 when none was measured on the current path. */
    public static int paceMbpsFor(Context context, String hostUuid, int clientLinkMbps) {
        return paceMbpsFor(get(context, hostUuid), clientLinkMbps);
    }

    static int paceMbpsFor(Calibration calibration, int clientLinkMbps) {
        return calibration != null && calibration.clientLinkMbps == clientLinkMbps ? calibration.paceMbps : 0;
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }
}
