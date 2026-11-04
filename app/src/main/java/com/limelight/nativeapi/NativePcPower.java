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
        /** Why this device's last accepted request did not go ahead after all, in the host's words; or null. */
        public final String lastRefusal;
        /**
         * {@link #lastRefusal} is no refusal: Windows did not confirm the request in time, so the PC may still go
         * down. False from an older host, which never says.
         */
        public final boolean lastRefusalUnconfirmed;

        Offer(JSONObject json) {
            JSONObject restart = json.optJSONObject("restart");
            JSONObject shutdown = json.optJSONObject("shutdown");
            restartAvailable = restart != null && restart.optBoolean("available");
            restartReason = text(restart, "reason");
            shutdownAvailable = shutdown != null && shutdown.optBoolean("available");
            shutdownReason = text(shutdown, "reason");
            token = text(json, "token");
            wakeFromOff = json.optBoolean("wake_from_off");
            String refusal = text(json, "last_refusal");
            lastRefusal = refusal == null || refusal.isEmpty() ? null : refusal;
            lastRefusalUnconfirmed = lastRefusal != null && json.optBoolean("last_refusal_unconfirmed");
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

    /**
     * GET /update, and the answer of an accepted POST /update/install, with what pc_power.rs's rules make of it
     * (native_api.rs's update_json): Java reads the verdicts and never decides them itself.
     */
    public static final class Update {
        public final String currentVersion;
        /** Null when the host knows of none. */
        public final String latestVersion;
        /** idle, waiting, downloading, ready, installing, or a newer host's phase. */
        public final String phase;
        public final boolean installSupported;
        /** Why the install waits for another device's stream, in the host's words; null when it does not. */
        public final String waitingFor;
        /** The updater's last error, null when it gave none. */
        public final String error;
        /** The version to install, without a leading v, or null. */
        private final String offered;
        /** It installs, or is queued as an Install now already: nothing to offer again. */
        public final boolean installingNow;
        /** An update is queued, downloading or installing. */
        public final boolean inProgress;
        /** The updater failed; {@link #failure} is its words, or null. */
        public final boolean failed;
        public final String failure;
        /** Read while following an install: the PC answers on the version being installed. */
        public final boolean arrived;
        /** Read while following an install: the PC is back on another version with nothing under way. */
        public final boolean gaveUp;

        Update(JSONObject json) {
            this(json.optString("current_version", ""), text(json, "latest_version"), json.optString("phase", ""),
                    json.optBoolean("install_supported"), text(json, "waiting_for"), text(json, "error"),
                    text(json, "offered"), json.optBoolean("installing_now"), json.optBoolean("in_progress"),
                    json.optBoolean("failed"), text(json, "failure"), json.optBoolean("arrived"),
                    json.optBoolean("gave_up"));
        }

        private Update(String currentVersion, String latestVersion, String phase, boolean installSupported,
                       String waitingFor, String error, String offered, boolean installingNow, boolean inProgress,
                       boolean failed, String failure, boolean arrived, boolean gaveUp) {
            this.currentVersion = currentVersion;
            this.latestVersion = latestVersion;
            this.phase = phase;
            this.installSupported = installSupported;
            this.waitingFor = waitingFor;
            this.error = error;
            this.offered = offered;
            this.installingNow = installingNow;
            this.inProgress = inProgress;
            this.failed = failed;
            this.failure = failure;
            this.arrived = arrived;
            this.gaveUp = gaveUp;
        }

        /** The version to offer, without a leading v, when there is one to install; otherwise null. */
        public String offered() {
            return offered;
        }

        /** As desktop after a tap: under way, so the menu offers no second install until the next read says. */
        Update installing() {
            return new Update(currentVersion, latestVersion, "installing", installSupported, waitingFor, error,
                    offered, true, true, failed, failure, arrived, gaveUp);
        }
    }

    static Offer parseOffer(JSONObject json) throws JSONException {
        if (json == null) throw new JSONException("no power object");
        return new Offer(json);
    }

    static Update parseUpdate(JSONObject json) throws JSONException {
        if (json == null) throw new JSONException("no update object");
        return new Update(json);
    }

    private static String text(JSONObject json, String key) {
        if (json == null || json.isNull(key)) {
            return null;
        }
        Object value = json.opt(key);
        return value instanceof String ? (String) value : null;
    }
}
