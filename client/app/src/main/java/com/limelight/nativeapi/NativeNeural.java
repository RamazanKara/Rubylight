package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * GET /neural?app=: a game's Neural Rendering switches, offered while the host lists
 * {@code neural.frame_generation}. The host downloads and installs on its own after a write, so a
 * client reads again while {@link #working()}. A device never names a program on the PC.
 */
public final class NativeNeural {
    public static final String FRAME_GENERATION = "frame_generation";

    /** One switch; null when the host's answer has no frame generation switch. */
    public static final class Switch {
        /** {@code configurable}, {@code unsupported}, {@code temporarily_unavailable} or {@code needs_external_setup}. */
        public final String state;
        public final boolean on;
        /** Why it can't change now, in the host's words; null when it can. */
        public final String reason;

        Switch(String state, boolean on, String reason) {
            this.state = state;
            this.on = on;
            this.reason = reason;
        }

        public boolean configurable() {
            return "configurable".equals(state);
        }
    }

    public final String app;
    /** The one-time notice was accepted on the host; until then a write turning it on carries accept_notice. */
    public final boolean accepted;
    public final Switch frameGeneration;
    /** Rubylight 3.0's upscaling is in the game's folder: turning the switch on replaces it. */
    public final boolean replacesEarlierUpscaling;
    /** The job's phase ({@code download}, {@code place}, {@code remove}, {@code done}, {@code failed}), or null. */
    public final String phase;
    public final long done;
    public final long total;
    /** Why the last job failed, in the host's words; null otherwise. */
    public final String error;

    NativeNeural(String app, boolean accepted, Switch frameGeneration, boolean replacesEarlierUpscaling,
                 String phase, long done, long total, String error) {
        this.app = app;
        this.accepted = accepted;
        this.frameGeneration = frameGeneration;
        this.replacesEarlierUpscaling = replacesEarlierUpscaling;
        this.phase = phase;
        this.done = done;
        this.total = total;
        this.error = error;
    }

    /** The host is downloading, installing or removing: read again in a moment. */
    public boolean working() {
        return "download".equals(phase) || "place".equals(phase) || "remove".equals(phase);
    }

    public boolean failed() {
        return "failed".equals(phase);
    }

    static NativeNeural parse(JSONObject json) throws JSONException {
        Switch frameGeneration = null;
        JSONArray switches = json.optJSONArray("switches");
        for (int i = 0; switches != null && i < switches.length(); i++) {
            JSONObject s = switches.getJSONObject(i);
            if (FRAME_GENERATION.equals(s.optString("id"))) {
                frameGeneration = new Switch(s.optString("state"), s.optBoolean("on"), text(s, "reason"));
            }
        }
        JSONObject job = json.optJSONObject("job");
        return new NativeNeural(json.getString("app"), json.optBoolean("accepted"), frameGeneration,
                json.optBoolean("replaces_earlier_upscaling"),
                job == null ? null : job.optString("phase", null),
                job == null ? 0 : job.optLong("done"), job == null ? 0 : job.optLong("total"),
                job == null ? null : text(job, "error"));
    }

    /** POST /neural/settings for the frame generation switch; accept_notice only when the user just accepted it. */
    static String writeJson(String app, boolean on, boolean acceptNotice) throws JSONException {
        JSONObject write = new JSONObject().put("app", app).put("switch", FRAME_GENERATION).put("on", on);
        if (acceptNotice) write.put("accept_notice", true);
        return write.toString();
    }

    private static String text(JSONObject json, String key) {
        return json.isNull(key) ? null : json.optString(key, null);
    }
}
