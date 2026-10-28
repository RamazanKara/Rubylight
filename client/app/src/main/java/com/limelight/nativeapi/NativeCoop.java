package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * GET /coop: the PC's Nucleus Co-op sessions, as coop.rs reads them. Values from a newer host read as UNKNOWN, never as
 * an error. Joining one seats this device; the stream is the normal launch of the session's {@code app_id}.
 */
public final class NativeCoop {
    public enum Phase {
        STARTING, RUNNING, ENDED, UNKNOWN;

        static Phase of(String wire) {
            for (Phase phase : values()) {
                if (phase.name().equalsIgnoreCase(wire)) return phase;
            }
            return UNKNOWN;
        }
    }

    public enum EndReason {
        ENDED, EMPTY, CRASHED, FAILED, UNKNOWN;

        static EndReason of(String wire) {
            for (EndReason reason : values()) {
                if (reason.name().equalsIgnoreCase(wire)) return reason;
            }
            return UNKNOWN;
        }
    }

    /** A co-op library entry: an app whose exe has a Nucleus Co-op handler. */
    public static final class Game {
        public final int appId;
        /** {@code Co-op (Nucleus): <app name>}, as the library lists it. */
        public final String title;
        public final int maxPlayers;

        Game(JSONObject json) throws JSONException {
            appId = json.getInt("app_id");
            title = json.getString("title");
            maxPlayers = json.optInt("max_players");
        }
    }

    public static final class Player {
        public final int slot;
        /** The device's name; null for a free slot. */
        public final String name;
        /** This device's own slot. */
        public final boolean you;
        /** The device's stream is gone; it keeps the slot for a while. */
        public final boolean away;

        Player(JSONObject json) throws JSONException {
            slot = json.getInt("slot");
            name = json.isNull("name") ? null : json.optString("name", null);
            you = json.optBoolean("you");
            away = json.optBoolean("away");
        }

        public boolean isFree() {
            return name == null && !you;
        }
    }

    public static final class Session {
        public final int id;
        /** The co-op entry to launch once joined. */
        public final int appId;
        public final String title;
        public final Phase phase;
        /** Null while the session has not ended. */
        public final EndReason endReason;
        /** What stopped, for crashed and failed; null otherwise. */
        public final String endDetail;
        public final List<Player> players;
        public final int freeSlots;
        /** A join by this device would succeed now. */
        public final boolean joinable;
        /** Why this device cannot join, as the host words it; null when it can. */
        public final String refusal;

        Session(JSONObject json) throws JSONException {
            id = (int) json.getLong("id");
            appId = json.getInt("app_id");
            title = json.getString("title");
            phase = Phase.of(json.getString("phase"));
            JSONObject end = json.optJSONObject("end");
            endReason = end == null ? null : EndReason.of(end.optString("reason"));
            endDetail = end == null || end.isNull("detail") ? null : end.optString("detail", null);
            players = list(json.optJSONArray("players"), Player::new);
            freeSlots = json.optInt("free_slots");
            joinable = json.optBoolean("joinable");
            refusal = json.isNull("refusal") ? null : json.optString("refusal", null);
        }

        /** This device's slot, or null when it holds none here. */
        public Player seat() {
            for (Player player : players) {
                if (player.you) return player;
            }
            return null;
        }
    }

    /** POST /coop/join: the slot is held for this device until it launches {@link #appId}. */
    public static final class Joined {
        public final int session;
        public final int slot;
        public final int appId;

        Joined(int session, int slot, int appId) {
            this.session = session;
            this.slot = slot;
            this.appId = appId;
        }

        static Joined parse(JSONObject json) throws JSONException {
            return new Joined((int) json.getLong("session"), json.getInt("slot"), json.getInt("app_id"));
        }
    }

    /** Why a co-op call failed, for the words the screen shows; the message is always the host's (or the error's). */
    public enum Problem {
        /** 403 permission_required: Join co-op is off for this device on the host. */
        PERMISSION,
        /** 409 coop_refused: full, ended, private, or this device was removed. */
        REFUSED,
        /** 409 no_session: the session is gone. */
        GONE,
        /** 409 session_busy: another session holds Nucleus Co-op. */
        BUSY,
        /** 503: the host cannot run co-op now. */
        UNAVAILABLE,
        /** No answer from the PC. */
        UNREACHABLE,
        /** Anything else, as the error reads. */
        OTHER;

        /** As coop.rs's {@code CoopProblem::of}: the code first, then the status. */
        public static Problem of(NativeApiException error) {
            if (error.isUnreachable()) return UNREACHABLE;
            String code = error.getCode();
            int status = error.getHttpStatus();
            if ("permission_required".equals(code) || status == 403) return PERMISSION;
            if ("no_session".equals(code)) return GONE;
            if ("session_busy".equals(code)) return BUSY;
            if ("temporarily_unavailable".equals(code) || status == 503) return UNAVAILABLE;
            // coop_refused is newer than the codes the bridge knows: it arrives as unknown with a 409.
            if ("coop_refused".equals(code) || status == 409) return REFUSED;
            return OTHER;
        }
    }

    public final boolean available;
    /** Why co-op is not available; null when it is. */
    public final String reason;
    public final List<Game> games;
    public final List<Session> sessions;

    private NativeCoop(JSONObject json) throws JSONException {
        available = json.getBoolean("available");
        reason = json.isNull("reason") ? null : json.optString("reason", null);
        games = list(json.optJSONArray("games"), Game::new);
        sessions = list(json.optJSONArray("sessions"), Session::new);
    }

    static NativeCoop parse(JSONObject json) throws JSONException {
        return new NativeCoop(json);
    }

    /** The library entry's title for {@code appId}, or null when the listing has none. */
    public String entryTitle(int appId) {
        for (Game game : games) {
            if (game.appId == appId) return game.title;
        }
        return null;
    }

    private interface Reader<T> {
        T read(JSONObject json) throws JSONException;
    }

    private static <T> List<T> list(JSONArray array, Reader<T> reader) throws JSONException {
        List<T> items = new ArrayList<>();
        for (int i = 0; array != null && i < array.length(); i++) {
            items.add(reader.read(array.getJSONObject(i)));
        }
        return Collections.unmodifiableList(items);
    }
}
