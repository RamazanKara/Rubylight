package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * GET /library: what a host can launch. Fields past the first API version are optional and read when present; a host that
 * leaves them out reads as one that cannot tell, and fields this client does not know are ignored.
 */
public final class NativeLibrary {
    /** One title of a host. */
    public static final class Game {
        /** Null for a game the host found but has not added (it has no launch entry yet). */
        public final String installId;
        /** 0 for a game the host has not added. */
        public final int appId;
        public final String title;
        /** Identifies the title across hosts (null when the host cannot tell); equal ids merge on Home. */
        public final String gameId;
        /** The store's key ("steam", "epic", ...), or null for an app no store owns. */
        public final String storeSource;
        /** The game's id in its store, or null. */
        public final String storeId;
        /** The store's display name as the host writes it ("Epic Games"), or null. */
        public final String storeName;
        /** Null when the host does not say. */
        public final Boolean installed;
        public final boolean running;
        /** False for a game the host found in a store but that is not in its library; true when the host does not say. */
        public final boolean added;
        /** Seconds since the epoch the game last ran, or null when the host does not say. */
        public final Long lastPlayed;
        /** Null when the host does not say. */
        public final Integer playtimeMinutes;
        /** Whether a launch can start it now; null when the host does not say. */
        public final Boolean launchable;
        /** The cover's path on the host, or null. */
        public final String coverPath;
        /** Changes whenever the cover does; a cache key. */
        public final String coverVersion;
        /** The other art the host has, or null each. */
        public final String heroPath;
        public final String logoPath;
        public final String landscapePath;

        Game(String installId, int appId, String title, String gameId, String storeSource, String storeId,
             String storeName, Boolean installed, boolean running, boolean added, Long lastPlayed,
             Integer playtimeMinutes, Boolean launchable, String coverPath, String coverVersion, String heroPath,
             String logoPath, String landscapePath) {
            this.installId = installId;
            this.appId = appId;
            this.title = title;
            this.gameId = gameId;
            this.storeSource = storeSource;
            this.storeId = storeId;
            this.storeName = storeName;
            this.installed = installed;
            this.running = running;
            this.added = added;
            this.lastPlayed = lastPlayed;
            this.playtimeMinutes = playtimeMinutes;
            this.launchable = launchable;
            this.coverPath = coverPath;
            this.coverVersion = coverVersion;
            this.heroPath = heroPath;
            this.logoPath = logoPath;
            this.landscapePath = landscapePath;
        }

        JSONObject toJson() throws JSONException {
            JSONObject json = new JSONObject().put("install_id", installId == null ? JSONObject.NULL : installId)
                    .put("app_id", appId).put("title", title).put("running", running);
            json.put("game_id", gameId == null ? JSONObject.NULL : gameId);
            json.put("installed", installed == null ? JSONObject.NULL : installed);
            if (storeSource == null && storeId == null && storeName == null) {
                json.put("store", JSONObject.NULL);
            } else {
                JSONObject store = new JSONObject().put("source", storeSource == null ? JSONObject.NULL : storeSource);
                if (storeId != null) store.put("id", storeId);
                if (storeName != null) store.put("name", storeName);
                json.put("store", store);
            }
            if (!added) json.put("added", false);
            if (lastPlayed != null) json.put("last_played", lastPlayed.longValue());
            if (playtimeMinutes != null) json.put("playtime_minutes", playtimeMinutes.intValue());
            if (launchable != null) json.put("launchable", launchable.booleanValue());
            JSONObject artwork = new JSONObject().put("version", coverVersion);
            if (coverPath != null) artwork.put("cover", coverPath);
            if (heroPath != null) artwork.put("hero", heroPath);
            if (logoPath != null) artwork.put("logo", logoPath);
            if (landscapePath != null) artwork.put("landscape", landscapePath);
            return json.put("artwork", artwork);
        }

        static Game parse(JSONObject json) throws JSONException {
            JSONObject store = json.optJSONObject("store");
            JSONObject artwork = json.optJSONObject("artwork");
            boolean added = json.isNull("added") || json.optBoolean("added", true);
            // An added game has both ids; one the host only found may have neither.
            String installId = added ? json.getString("install_id") : text(json, "install_id");
            int appId = added ? json.getInt("app_id") : json.optInt("app_id", 0);
            return new Game(installId, appId, json.getString("title"), text(json, "game_id"),
                    text(store, "source"), text(store, "id"), text(store, "name"),
                    json.isNull("installed") ? null : Boolean.valueOf(json.optBoolean("installed")),
                    json.optBoolean("running"), added, number(json, "last_played"),
                    json.has("playtime_minutes") && !json.isNull("playtime_minutes")
                            ? Integer.valueOf(json.optInt("playtime_minutes")) : null,
                    json.has("launchable") && !json.isNull("launchable")
                            ? Boolean.valueOf(json.optBoolean("launchable")) : null,
                    text(artwork, "cover"), artwork == null ? "" : artwork.optString("version", ""),
                    text(artwork, "hero"), text(artwork, "logo"), text(artwork, "landscape"));
        }

        private static String text(JSONObject json, String key) {
            if (json == null || json.isNull(key)) return null;
            String value = json.optString(key, null);
            return value == null || value.isEmpty() ? null : value;
        }

        /** A positive whole number, or null when missing, null, not a number or not positive. */
        private static Long number(JSONObject json, String key) {
            if (json.isNull(key)) return null;
            long value = json.optLong(key, 0);
            return value > 0 ? value : null;
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
