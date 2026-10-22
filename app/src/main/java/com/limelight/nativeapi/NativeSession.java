package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/** GET /session and the answer of a session action: what runs on the host and what this device may do about it. */
public final class NativeSession {
    public enum State {
        IDLE, PREPARING, LAUNCHING, STREAMING, SUSPENDED, STOPPING, UNKNOWN;

        static State of(String wire) {
            return parse(State.class, wire, UNKNOWN);
        }
    }

    public enum Owner {
        NONE, YOU, OTHER_DEVICE, UNKNOWN;

        static Owner of(String wire) {
            return parse(Owner.class, wire, UNKNOWN);
        }
    }

    /** The session actions. Resume is the launch path and is not callable through the action endpoint. */
    public enum ActionId {
        RESUME("resume"), DISCONNECT("disconnect"), END("end"), CLOSE_GAME("close_game"), UNKNOWN("");

        public final String wire;

        ActionId(String wire) {
            this.wire = wire;
        }

        static ActionId of(String wire) {
            for (ActionId id : values()) {
                if (id != UNKNOWN && id.wire.equals(wire)) {
                    return id;
                }
            }
            return UNKNOWN;
        }
    }

    public static final class Action {
        public final ActionId id;
        public final boolean allowed;
        /** Why it is not allowed (an API error code such as not_session_owner), or null. */
        public final String reason;

        Action(ActionId id, boolean allowed, String reason) {
            this.id = id;
            this.allowed = allowed;
            this.reason = reason;
        }
    }

    public static final class App {
        public final String installId;
        public final int appId;
        public final String title;

        App(String installId, int appId, String title) {
            this.installId = installId;
            this.appId = appId;
            this.title = title;
        }
    }

    public final long revision;
    public final State state;
    public final Owner owner;
    /** The app of the session, or null when none runs. */
    public final App app;
    public final boolean gameRunning;
    /** Milliseconds since the epoch, 0 when the host does not say. */
    public final long sinceUnixMs;
    public final long suspendedUntilUnixMs;
    public final List<Action> actions;

    NativeSession(long revision, State state, Owner owner, App app, boolean gameRunning, long sinceUnixMs,
                  long suspendedUntilUnixMs, List<Action> actions) {
        this.revision = revision;
        this.state = state;
        this.owner = owner;
        this.app = app;
        this.gameRunning = gameRunning;
        this.sinceUnixMs = sinceUnixMs;
        this.suspendedUntilUnixMs = suspendedUntilUnixMs;
        this.actions = Collections.unmodifiableList(actions);
    }

    public static NativeSession parse(JSONObject json) throws JSONException {
        JSONObject app = json.optJSONObject("app");
        JSONArray array = json.optJSONArray("actions");
        List<Action> actions = new ArrayList<>();
        for (int i = 0; array != null && i < array.length(); i++) {
            JSONObject action = array.getJSONObject(i);
            actions.add(new Action(ActionId.of(action.optString("id")), action.optBoolean("allowed"),
                    action.isNull("reason") ? null : action.optString("reason", null)));
        }
        return new NativeSession(json.optLong("revision"), State.of(json.optString("state")),
                Owner.of(json.optString("owner")),
                app == null ? null : new App(app.getString("install_id"), app.getInt("app_id"),
                        app.getString("title")),
                json.optBoolean("game_running"), json.optLong("since_unix_ms"),
                json.optLong("suspended_until_unix_ms"), actions);
    }

    /** The action as the host offers it, or null when the host does not list it. */
    public Action action(ActionId id) {
        for (Action action : actions) {
            if (action.id == id) {
                return action;
            }
        }
        return null;
    }

    /** Whether the host lets this device run the action now. */
    public boolean allows(ActionId id) {
        Action action = action(id);
        return action != null && action.allowed;
    }

    /** Whether a session holds a game the user can come back to: streaming, suspended or on its way. */
    public boolean isActive() {
        return app != null && (state == State.STREAMING || state == State.SUSPENDED
                || state == State.LAUNCHING || state == State.PREPARING);
    }

    private static <E extends Enum<E>> E parse(Class<E> type, String wire, E fallback) {
        try {
            return Enum.valueOf(type, wire.toUpperCase(Locale.ROOT));
        } catch (IllegalArgumentException | NullPointerException e) {
            return fallback;
        }
    }
}
