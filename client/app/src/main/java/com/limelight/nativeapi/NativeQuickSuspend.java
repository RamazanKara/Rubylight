package com.limelight.nativeapi;

import com.limelight.nvstream.jni.Json;

/**
 * GET /session/suspend, POST /session/suspend and POST /session/resume: Quick Suspend's state for this device's game as
 * the host reports it. The host decides; a refusal is an answer, the {@link #REFUSED} state with the host's stable
 * {@link #reason}, which the stream menu words in the player's language.
 */
public final class NativeQuickSuspend {
    public static final String UNAVAILABLE = "unavailable";
    public static final String READY = "ready";
    public static final String SUSPENDED = "suspended";
    public static final String RESUMING = "resuming";
    public static final String REFUSED = "refused";

    /** One of the constants above, or a newer host's state, which offers nothing. */
    public final String state;
    /** The host's stable reason name ({@code none}, {@code anti_cheat_detected}, ...). */
    public final String reason;
    /** The host's English words for a refusal; null without one. */
    public final String message;
    public final String appTitle;
    public final int suspendedCount;
    /** When the game was paused, Unix ms; 0 unless it is. */
    public final long suspendedSinceUnixMs;
    /** The device or console that paused it; null unless it is paused. */
    public final String suspendedBy;

    NativeQuickSuspend(String state, String reason, String message, String appTitle, int suspendedCount,
                       long suspendedSinceUnixMs, String suspendedBy) {
        this.state = state;
        this.reason = reason;
        this.message = message;
        this.appTitle = appTitle;
        this.suspendedCount = suspendedCount;
        this.suspendedSinceUnixMs = suspendedSinceUnixMs;
        this.suspendedBy = suspendedBy;
    }

    /** The game is paused or resuming: the stream menu offers Resume game. */
    public boolean paused() {
        return SUSPENDED.equals(state) || RESUMING.equals(state);
    }

    /** The menu offers Suspend game or Resume game; neither while Quick Suspend is off for this game or nothing runs. */
    public boolean offersTile() {
        return READY.equals(state) || paused();
    }

    public boolean refused() {
        return REFUSED.equals(state);
    }

    static NativeQuickSuspend parse(Json.Obj json) {
        return new NativeQuickSuspend(json.getString("state"), json.optString("reason", "none"),
                nullable(json, "message"), nullable(json, "app_title"), json.optInt("suspended_count", 0),
                json.isNull("suspended_since_unix_ms") ? 0L : json.optLong("suspended_since_unix_ms"),
                nullable(json, "suspended_by"));
    }

    private static String nullable(Json.Obj json, String key) {
        return json.isNull(key) ? null : json.optString(key, null);
    }
}
