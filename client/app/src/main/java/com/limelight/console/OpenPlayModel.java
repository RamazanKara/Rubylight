package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;

import org.json.JSONException;
import org.json.JSONObject;

/**
 * "When Nightjar opens": the decision desktop Nightjar makes too, nightjar-policy's open_play (protocol/policy),
 * reached through libnightjar_core (protocol/android/src/open_play.rs). A model that cannot answer starts nothing.
 */
public final class OpenPlayModel {
    private OpenPlayModel() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": …}} or {@code {"error": code, "detail": …}}. */
    public interface Calls {
        String call(String request);
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) {
            System.loadLibrary("nightjar_core");
            calls = OpenPlayModel::call;
        }
        return calls;
    }

    /** JVM tests cannot load the library. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
    }

    private static native String call(String request);

    /** What this start of the app does. */
    static final class Decision {
        static final Decision STAY = new Decision("stay", null, false);

        /** {@code stay}, {@code countdown} or {@code skip}. */
        final String action;
        /** {@code last} or {@code chosen}; null when staying. */
        final String game;
        /** The guard's armed mark goes. */
        final boolean disarm;

        private Decision(String action, String game, boolean disarm) {
            this.action = action;
            this.game = game;
            this.disarm = disarm;
        }
    }

    /** The answer's {@code ok} object, null (and a log line) when the model refused or could not be reached. */
    private static JSONObject ask(JSONObject request) {
        try {
            String answer = calls().call(request.toString());
            JSONObject json = new JSONObject(answer);
            JSONObject ok = json.optJSONObject("ok");
            if (ok == null) LimeLog.warning("Automatic start: model answered " + answer);
            return ok;
        } catch (JSONException | LinkageError | RuntimeException e) {
            LimeLog.warning("Automatic start: model unavailable: " + e);
            return null;
        }
    }

    static Decision decide(boolean firstScreen, boolean launcher, boolean restored, boolean fromRecents, String opens,
                           boolean hasLast, boolean hasChosen, boolean armed) {
        try {
            JSONObject ok = ask(new JSONObject().put("op", "decide").put("first_screen", firstScreen)
                    .put("launcher", launcher).put("restored", restored).put("from_recents", fromRecents)
                    .put("opens", opens).put("has_last", hasLast).put("has_chosen", hasChosen).put("armed", armed));
            if (ok == null) return Decision.STAY;
            return new Decision(ok.getString("action"), ok.isNull("game") ? null : ok.getString("game"), ok.getBoolean("disarm"));
        } catch (JSONException e) {
            LimeLog.warning("Automatic start: bad answer: " + e);
            return Decision.STAY;
        }
    }

    /** Whether a stream that ended like this clears the guard. */
    static boolean streamEndDisarms(long connectedMs, boolean leftByPlayer) {
        try {
            JSONObject ok = ask(new JSONObject().put("op", "stream_end").put("connected_ms", connectedMs)
                    .put("left_by_player", leftByPlayer));
            // Without an answer the next start is skipped once and says so: the safe side.
            return ok != null && ok.getBoolean("disarm");
        } catch (JSONException e) {
            LimeLog.warning("Automatic start: bad answer: " + e);
            return false;
        }
    }

    /** The countdown and the time a stream must last, in ms; the model's, or these as a start only runs after it answered. */
    static long[] timing() {
        try {
            JSONObject ok = ask(new JSONObject().put("op", "timing"));
            if (ok != null) return new long[] {ok.getLong("countdown_ms"), ok.getLong("settled_ms")};
        } catch (JSONException e) {
            LimeLog.warning("Automatic start: bad answer: " + e);
        }
        return new long[] {3000, 10000};
    }
}
