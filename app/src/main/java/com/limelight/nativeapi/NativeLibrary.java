package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** GET /library: what a host can launch. */
public final class NativeLibrary {
    /** One title of a host. */
    public static final class Game {
        public final String installId;
        public final int appId;
        public final String title;
        /** Identifies the title across hosts (null when the host cannot tell); equal ids merge on Home. */
        public final String gameId;
        public final String storeSource;
        /** Null when the host does not say. */
        public final Boolean installed;
        public final boolean running;
        /** The cover's path on the host, or null. */
        public final String coverPath;
        /** Changes whenever the cover does; a cache key. */
        public final String coverVersion;

        Game(String installId, int appId, String title, String gameId, String storeSource, Boolean installed,
             boolean running, String coverPath, String coverVersion) {
            this.installId = installId;
            this.appId = appId;
            this.title = title;
            this.gameId = gameId;
            this.storeSource = storeSource;
            this.installed = installed;
            this.running = running;
            this.coverPath = coverPath;
            this.coverVersion = coverVersion;
        }

        JSONObject toJson() throws JSONException {
            JSONObject json = new JSONObject().put("install_id", installId).put("app_id", appId)
                    .put("title", title).put("running", running);
            json.put("game_id", gameId == null ? JSONObject.NULL : gameId);
            json.put("installed", installed == null ? JSONObject.NULL : installed);
            json.put("store", storeSource == null ? JSONObject.NULL : new JSONObject().put("source", storeSource));
            JSONObject artwork = new JSONObject().put("version", coverVersion);
            if (coverPath != null) {
                artwork.put("cover", coverPath);
            }
            return json.put("artwork", artwork);
        }

        static Game parse(JSONObject json) throws JSONException {
            JSONObject store = json.optJSONObject("store");
            JSONObject artwork = json.optJSONObject("artwork");
            return new Game(json.getString("install_id"), json.getInt("app_id"), json.getString("title"),
                    json.isNull("game_id") ? null : json.optString("game_id", null),
                    store == null ? null : store.optString("source", null),
                    json.isNull("installed") ? null : Boolean.valueOf(json.optBoolean("installed")),
                    json.optBoolean("running"),
                    artwork == null || artwork.isNull("cover") ? null : artwork.optString("cover", null),
                    artwork == null ? "" : artwork.optString("version", ""));
        }
    }

    public final String revision;
    public final List<Game> games;

    NativeLibrary(String revision, List<Game> games) {
        this.revision = revision;
        this.games = Collections.unmodifiableList(games);
    }

    /** The JSON {@link #parse} reads back; what the library cache stores. */
    public JSONObject toJson() throws JSONException {
        JSONArray array = new JSONArray();
        for (Game game : games) {
            array.put(game.toJson());
        }
        return new JSONObject().put("revision", revision).put("games", array);
    }

    public static NativeLibrary parse(JSONObject json) throws JSONException {
        JSONArray array = json.getJSONArray("games");
        List<Game> games = new ArrayList<>(array.length());
        for (int i = 0; i < array.length(); i++) {
            games.add(Game.parse(array.getJSONObject(i)));
        }
        return new NativeLibrary(json.optString("revision", ""), games);
    }
}
