package com.limelight.nativeapi;

import org.json.JSONException;
import org.json.JSONObject;

/**
 * Restart PC, Shut down PC (GET /power) and the host's update (GET /update), read as pc_power.rs reads them:
 * fields a host leaves out decode as false or null, so an older or newer host still parses.
 */
public final class NativePcPower {
    /** The native features that offer them. */
    public static final String FEATURE_RESTART = "power.restart";
    public static final String FEATURE_SHUTDOWN = "power.shutdown";
    public static final String FEATURE_UPDATE = "update";

    private NativePcPower() {
    }

    /** What the person asks the PC to do, with the JNI op that does it. */
    public enum Action {
        RESTART("pc_power.restart"),
        SHUT_DOWN("pc_power.shutdown");

        final String op;

        Action(String op) {
            this.op = op;
        }
    }

    /** GET /power: whether each action can run now, the host's reason when not, and the token to confirm with. */
    public static final class Offer {
        public final boolean restartAvailable;
        public final String restartReason;
        public final boolean shutdownAvailable;
        public final String shutdownReason;
        /** Null when the host handed out none. */
        public final String token;
        /** A wired wake target is ready; without it a PC that is off may not wake again. */
        public final boolean wakeFromOff;

        Offer(JSONObject json) {
            JSONObject restart = json.optJSONObject("restart");
            JSONObject shutdown = json.optJSONObject("shutdown");
            restartAvailable = restart != null && restart.optBoolean("available");
            restartReason = text(restart, "reason");
            shutdownAvailable = shutdown != null && shutdown.optBoolean("available");
            shutdownReason = text(shutdown, "reason");
            token = text(json, "token");
            wakeFromOff = json.optBoolean("wake_from_off");
        }

        /** The token to confirm {@code action} with; null when the host says it can't run now. */
        public String tokenFor(Action action) {
            return (action == Action.RESTART ? restartAvailable : shutdownAvailable) ? token : null;
        }

        /** The host's words for why {@code action} can't run, or "" when it gave none. */
        public String reason(Action action) {
            String reason = action == Action.RESTART ? restartReason : shutdownReason;
            return reason == null ? "" : reason;
        }
    }

    /** GET /update, and the answer of an accepted POST /update/install. */
    public static final class Update {
        public final String currentVersion;
        /** Null when the host knows of none. */
        public final String latestVersion;
        /** idle, waiting, downloading, ready, installing, or a newer host's phase. */
        public final String phase;
        public final boolean installSupported;

        Update(JSONObject json) {
            currentVersion = json.optString("current_version", "");
            latestVersion = text(json, "latest_version");
            phase = json.optString("phase", "");
            installSupported = json.optBoolean("install_supported");
        }

        /** The version to offer, without a leading v, when there is one to install; otherwise null. */
        public String offered() {
            if (latestVersion == null || !installSupported) {
                return null;
            }
            String latest = bare(latestVersion);
            return latest.isEmpty() || latest.equals(bare(currentVersion)) ? null : latest;
        }
    }

    /** Whether serverinfo's {@code running} version shows the update arrived: the PC is back on {@code target}. */
    public static boolean arrived(String running, String target) {
        if (running == null || target == null) {
            return false;
        }
        String version = bare(running.trim());
        return !version.isEmpty() && version.equals(bare(target));
    }

    static Offer parseOffer(JSONObject json) throws JSONException {
        if (json == null) throw new JSONException("no power object");
        return new Offer(json);
    }

    static Update parseUpdate(JSONObject json) throws JSONException {
        if (json == null) throw new JSONException("no update object");
        return new Update(json);
    }

    private static String bare(String version) {
        return version.startsWith("v") ? version.substring(1) : version;
    }

    private static String text(JSONObject json, String key) {
        if (json == null || json.isNull(key)) {
            return null;
        }
        Object value = json.opt(key);
        return value instanceof String ? (String) value : null;
    }
}
