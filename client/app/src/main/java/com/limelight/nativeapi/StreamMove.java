package com.limelight.nativeapi;

import org.json.JSONObject;

/** The part of GET /session Continue here reads: who plays the running game and where this device's stream went. */
public final class StreamMove {
    /** The device playing the running game, or null when that is nobody else. */
    public final String playingOn;
    public final int playingAppId;
    public final String playingAppUuid;
    public final String playingTitle;
    /** The device this device's stream moved to, within the last two minutes; otherwise null. */
    public final String movedTo;
    /** Whether the running game is this device's. */
    public final boolean ownedHere;

    StreamMove(String playingOn, int playingAppId, String playingAppUuid, String playingTitle, String movedTo,
               boolean ownedHere) {
        this.playingOn = playingOn;
        this.playingAppId = playingAppId;
        this.playingAppUuid = playingAppUuid;
        this.playingTitle = playingTitle;
        this.movedTo = movedTo;
        this.ownedHere = ownedHere;
    }

    static StreamMove parse(JSONObject json) {
        JSONObject playing = json.optJSONObject("playing_on");
        JSONObject app = playing == null ? null : playing.optJSONObject("app");
        JSONObject moved = json.optJSONObject("moved_to");
        return new StreamMove(app == null ? null : playing.optString("device", null),
                app == null ? 0 : (int) app.optLong("app_id"),
                app == null ? null : app.optString("install_id", null),
                app == null ? null : app.optString("title", null),
                moved == null ? null : moved.optString("device", null),
                json.optBoolean("owns_game"));
    }
}
