package com.limelight.nativeapi;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarConsoleJni;

import java.util.List;

/**
 * The native API of one paired host. Every method blocks on HTTPS and must not run on the UI
 * thread; failures are {@link NativeApiException}s that keep the API's error code.
 */
public final class NativeApiClient {
    private final NativeApiTransport transport;
    private final HostEndpoint host;

    public NativeApiClient(NativeApiTransport transport, HostEndpoint host) {
        this.transport = transport;
        this.host = host;
    }

    public NativeCapabilities capabilities() throws NativeApiException {
        try {
            Json.Obj json = call("capabilities", "");
            NativeCapabilities capabilities = NativeCapabilities.parse(json);
            KnownCapabilities.record(host.hostUuid, json.toString());
            return capabilities;
        } catch (Json.SyntaxException e) {
            throw malformed("capabilities", e);
        }
    }

    /**
     * The capabilities JSON with the file, USB and shared folder permissions, for
     * {@link com.limelight.nvstream.http.HostCapabilities#withNativeCapabilities}.
     */
    public String fileCapabilities() throws NativeApiException {
        return call("files.capabilities", "").toString();
    }

    /**
     * Closes the game through the native API when this device has the host control permission.
     * Returns false, having done nothing, when it does not: the caller then quits the classic way
     * through /cancel, which needs no such permission.
     */
    public boolean closeGameIfAllowed() throws NativeApiException {
        if (!capabilities().hostControl) {
            return false;
        }
        try {
            sessionAction(NativeSession.ActionId.CLOSE_GAME);
            return true;
        } catch (NativeApiException e) {
            // The permission changed since the capabilities were read.
            if ("host_control_required".equals(e.getCode())) {
                return false;
            }
            throw e;
        }
    }

    public Json.Obj library() throws NativeApiException {
        return library(false);
    }

    /**
     * The raw library JSON; with {@code includeUnadded} also the games the host found in its stores
     * but has not added. Ask for those only when the host advertises
     * {@link NativeCapabilities#LIBRARY_UNADDED}.
     */
    public Json.Obj library(boolean includeUnadded) throws NativeApiException {
        try {
            Json.Obj json = call("library", includeUnadded ? "include=unadded" : "");
            json.getArray("games");
            return json;
        } catch (Json.SyntaxException e) {
            throw malformed("library", e);
        }
    }

    /**
     * Adds a game the host found but has not added, and returns the entry the host added (parsed by
     * Rust), or null when the reply does not list it as added. Only for a host that advertises
     * {@link NativeCapabilities#LIBRARY_ADD}.
     */
    public ConsoleSnapshot.AddedGame addToLibrary(String gameId) throws NativeApiException {
        try {
            Json.Obj reply = call("library_add", Json.obj().put("game_id", gameId).toString());
            String parsedJson = NightjarConsoleJni.parseAdded(reply.toString(), gameId);
            if (parsedJson == null || parsedJson.isEmpty()) {
                return null;
            }
            Json.Obj env = Json.parseObject(parsedJson);
            Json.Obj err = env.optObject("error");
            if (err != null) {
                throw malformed("library_add", new Json.SyntaxException(err.optString("message", "invalid")));
            }
            Json.Obj added = env.optObject("ok");
            return added == null ? null : ConsoleSnapshot.AddedGame.fromJson(added);
        } catch (Json.SyntaxException e) {
            throw malformed("library_add", e);
        }
    }

    public NativeSession session() throws NativeApiException {
        return parseSession(call("session", ""));
    }

    /** Who plays the running game and where this device's stream went. Only when the host lists session.continue_here. */
    public StreamMove streamMove() throws NativeApiException {
        try {
            return StreamMove.parse(call("session_move", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("session move", e);
        }
    }

    /** Ends the other devices' streams of the running game, which keeps running; the normal resume follows. */
    public NativeSession continueHere() throws NativeApiException {
        return parseSession(call("session_continue_here", ""));
    }

    /** Runs a session action and returns the session as it stands afterwards. Resume is the launch path, not an action. */
    public NativeSession sessionAction(NativeSession.ActionId action) throws NativeApiException {
        if (action == NativeSession.ActionId.RESUME || action == NativeSession.ActionId.UNKNOWN) {
            throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT,
                    "Not a session action: " + action.wire);
        }
        return parseSession(call("session_action", action.wire));
    }

    /**
     * Opens the menu the host draws into this device's stream and returns whether it is open now. Only for a host that
     * advertises {@link NativeCapabilities#COUCH_MENU}; the host refuses it without a stream this device owns.
     */
    public boolean openCouchMenu() throws NativeApiException {
        try {
            return call("couch_menu", Json.obj().put("open", true).toString()).getBoolean("open");
        } catch (Json.SyntaxException e) {
            throw malformed("couch menu", e);
        }
    }

    /** The PC's wake targets. Only for a host that advertises {@link NativeCapabilities#WAKE}. */
    public NativeWake wake() throws NativeApiException {
        try {
            return NativeWake.parse(call("wake", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("wake", e);
        }
    }

    /**
     * Night mode and headphone surround of this device's stream. Only for a host that advertises
     * {@link NativeCapabilities#NIGHT_MODE} or {@link NativeCapabilities#HEADPHONE_SURROUND}.
     */
    public NativeAudio audio() throws NativeApiException {
        try {
            return NativeAudio.parse(call("audio", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("audio", e);
        }
    }

    /** Turns one of them on or off for this device's own stream and returns the host's new state. */
    public NativeAudio writeAudio(String id, boolean on) throws NativeApiException {
        String body;
        try {
            body = NativeAudio.writeJson(id, on);
        } catch (Json.SyntaxException e) {
            throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT, "Audio write: " + e.getMessage());
        }
        try {
            return NativeAudio.parse(call("audio_write", body));
        } catch (Json.SyntaxException e) {
            throw malformed("audio", e);
        }
    }

    /** Quick Suspend's state for this device's game. Only for a host that advertises {@link NativeCapabilities#QUICK_SUSPEND}. */
    public NativeQuickSuspend quickSuspend() throws NativeApiException {
        return quickSuspend("quick_suspend");
    }

    /** Pauses the game on the PC; a refusal is the {@code refused} state with the host's reason, not an exception. */
    public NativeQuickSuspend suspendGame() throws NativeApiException {
        return quickSuspend("quick_suspend.suspend");
    }

    /** Resumes the paused game. */
    public NativeQuickSuspend resumeGame() throws NativeApiException {
        return quickSuspend("quick_suspend.resume");
    }

    private NativeQuickSuspend quickSuspend(String op) throws NativeApiException {
        try {
            return NativeQuickSuspend.parse(call(op, ""));
        } catch (Json.SyntaxException e) {
            throw malformed("quick suspend", e);
        }
    }

    /** A game's Neural Rendering switches; {@code app} is the library's app id. */
    public NativeNeural neural(String app) throws NativeApiException {
        try {
            return NativeNeural.parse(call("neural", app));
        } catch (Json.SyntaxException e) {
            throw malformed("Neural Rendering", e);
        }
    }

    /** Turns the game's frame generation on or off; needs View streams and Host control. */
    public NativeNeural writeNeural(String app, boolean on, boolean acceptNotice) throws NativeApiException {
        String body;
        try {
            body = NativeNeural.writeJson(app, on, acceptNotice);
        } catch (Json.SyntaxException e) {
            throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT, "Neural write: " + e.getMessage());
        }
        try {
            return NativeNeural.parse(call("neural.write", body));
        } catch (Json.SyntaxException e) {
            throw malformed("Neural Rendering", e);
        }
    }

    public NativeRadeon radeon() throws NativeApiException {
        return parseRadeon(call("radeon", ""));
    }

    /** Writes settings of one GPU; needs the Host control permission. */
    public NativeRadeon writeRadeon(int gpu, List<NativeRadeon.Change> changes) throws NativeApiException {
        try {
            return parseRadeon(call("radeon_write", NativeRadeon.writeJson(gpu, changes)));
        } catch (Json.SyntaxException e) {
            throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT, "Radeon write: " + e.getMessage());
        }
    }

    /** Puts every setting this device changed back to its original value. */
    public NativeRadeon restoreRadeon() throws NativeApiException {
        return parseRadeon(call("radeon_restore", ""));
    }

    /**
     * Sleep PC's offer (GET /power-actions/v1): a single-use token while the host is idle and lets this device
     * put it to sleep, otherwise none. Gate it on {@code Feature.SLEEP}.
     */
    public PowerReply powerCapabilities() throws NativeApiException {
        return parsePower(call("power.capabilities", ""));
    }

    /**
     * Spends a token from {@link #powerCapabilities} (POST /power-actions/v1/sleep). The host's refusals are
     * {@link NativeApiException.Kind#POWER_REFUSED} with its status_message as sent.
     */
    public PowerReply sleep(String token) throws NativeApiException {
        return parsePower(call("power.sleep", token));
    }

    /**
     * Restart and Shut down (GET /power): whether each can run now, the host's reason when not, and a single-use
     * token for the next 30 s. Only for a host whose capabilities list power.restart or power.shutdown.
     */
    public NativePcPower.Offer pcPower() throws NativeApiException {
        try {
            return NativePcPower.parseOffer(call("pc_power", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("power", e);
        }
    }

    /**
     * Restarts or shuts down the PC with a token from {@link #pcPower}; returning means the PC took it (202). A
     * refusal is {@link NativeApiException.Kind#API} with the host's words as its message.
     */
    public void powerPc(NativePcPower.Action action, String token) throws NativeApiException {
        call(action.op, token);
    }

    /** The host's version, its pending update and the updater's phase (GET /update). Only with feature update. */
    public NativePcPower.Update hostUpdate() throws NativeApiException {
        return hostUpdate("");
    }

    /**
     * {@link #hostUpdate()} while following an install to {@code target}: the answer also says whether setup gave up
     * ({@link NativePcPower.Update#gaveUp}).
     */
    public NativePcPower.Update hostUpdate(String target) throws NativeApiException {
        try {
            NativePcPower.Update update = NativePcPower.parseUpdate(call("update", target));
            KnownUpdates.record(host.hostUuid, update);
            return update;
        } catch (Json.SyntaxException e) {
            throw malformed("update", e);
        }
    }

    /**
     * Installs the host's pending update (POST /update/install), ending this device's own stream. Needs Host
     * control; refused, in the host's words, while another device streams.
     */
    public NativePcPower.Update installHostUpdate() throws NativeApiException {
        try {
            NativePcPower.Update update = NativePcPower.parseUpdate(call("update.install", ""));
            KnownUpdates.installing(host.hostUuid, update);
            return update;
        } catch (Json.SyntaxException e) {
            throw malformed("update", e);
        }
    }

    /**
     * The PC's Nucleus Co-op sessions (GET /coop). Only for a host that advertises {@link NativeCapabilities#COOP};
     * {@link NativeCoop.Problem#of} says what a failure means.
     */
    public NativeCoop coop() throws NativeApiException {
        try {
            return NativeCoop.parse(call("coop", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("co-op", e);
        }
    }

    /**
     * Seats this device in a co-op session, or back in its own slot (POST /coop/join). The host holds the slot for two
     * minutes, until this device launches {@link NativeCoop.Joined#appId} the normal way, which streams that slot.
     */
    public NativeCoop.Joined joinCoop(int session) throws NativeApiException {
        try {
            return NativeCoop.Joined.parse(call("coop.join", Integer.toUnsignedString(session)));
        } catch (Json.SyntaxException e) {
            throw malformed("co-op join", e);
        }
    }

    /** One answer of Sleep PC's routes, as power.rs parses it. */
    public static final class PowerReply {
        /** The HTTP status: 200 for an offer, 202 for an accepted sleep. */
        public final int status;
        public final String message;
        public final boolean sleepSupported;
        public final boolean sleepAllowed;
        /** Supported, allowed and idle. */
        public final boolean sleepAvailable;
        /** Null when the host handed out no token. */
        public final String token;
        public final int tokenLifetimeSeconds;
        public final boolean accepted;

        PowerReply(Json.Obj json) {
            status = json.getInt("status");
            message = json.getString("message");
            sleepSupported = json.getBoolean("sleep_supported");
            sleepAllowed = json.getBoolean("sleep_allowed");
            sleepAvailable = json.getBoolean("sleep_available");
            token = json.isNull("token") ? null : json.getString("token");
            tokenLifetimeSeconds = json.getInt("token_lifetime_seconds");
            accepted = json.getBoolean("accepted");
        }

        public static PowerReply unavailable(boolean sleepSupported, boolean sleepAllowed) {
            return new PowerReply(Json.obj()
                    .put("status", 200)
                    .put("message", "")
                    .put("sleep_supported", sleepSupported)
                    .put("sleep_allowed", sleepAllowed)
                    .put("sleep_available", false)
                    .put("token", (String) null)
                    .put("token_lifetime_seconds", 0)
                    .put("accepted", false));
        }
    }

    /** The devices waiting to pair (GET /pairing/requests). Only with feature pairing.approve and Host control. */
    public List<NativePairing.Request> pairingRequests() throws NativeApiException {
        try {
            return NativePairing.parseRequests(call("pairing.requests", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("pairing requests", e);
        }
    }

    /**
     * Sends the PIN a waiting device shows (POST /pairing/approve) and answers that device's name once it has paired.
     * Blocks while the host finishes the handshake; a refusal is in the host's words.
     */
    public String approvePairing(String id, String pin) throws NativeApiException {
        try {
            Json.Obj request = Json.obj().put("id", id).put("pin", pin);
            return call("pairing.approve", request.toString()).getString("name");
        } catch (Json.SyntaxException e) {
            throw malformed("pairing approval", e);
        }
    }

    /** The running game's guests and the joins waiting (GET /session/guests). Only with feature session.guests. */
    public NativeGuests guests() throws NativeApiException {
        try {
            return NativeGuests.parse(call("guests", ""));
        } catch (Json.SyntaxException e) {
            throw malformed("guests", e);
        }
    }

    /**
     * Answers a join or manages a guest (POST /session/guests/...), as the device playing. {@code action} is
     * approve, deny, kick, revoke, permission or keyboard_mouse; {@code id} is -1 for keyboard_mouse back to this
     * device, and {@code permission} may be null except for permission. Answers the list as it is afterwards.
     */
    public NativeGuests guestAction(String action, int id, String permission) throws NativeApiException {
        try {
            Json.Obj request = Json.obj()
                    .put("action", action)
                    .put("id", id < 0 ? null : id);
            if (permission != null) request.put("permission", permission);
            return NativeGuests.parse(call("guests.action", request.toString()));
        } catch (Json.SyntaxException e) {
            throw malformed("guests", e);
        }
    }

    /**
     * Makes an invite to play over the internet (POST /session/guests/invites), with the PC's default permission
     * and lifetime. Only with feature session.guest-invites; the link is a secret, shown once and never logged.
     */
    public NativeGuests.Invite createInvite() throws NativeApiException {
        try {
            Json.Obj made = call("guests.invite", "");
            return new NativeGuests.Invite(made.getString("link"), made.getLong("expires"));
        } catch (Json.SyntaxException e) {
            throw malformed("invite", e);
        }
    }

    /** Ends a waiting device's request (POST /pairing/deny) and answers the devices still waiting. */
    public List<NativePairing.Request> denyPairing(String id) throws NativeApiException {
        try {
            return NativePairing.parseRequests(call("pairing.deny", id));
        } catch (Json.SyntaxException e) {
            throw malformed("pairing requests", e);
        }
    }

    /** Raw JSON envelope request used by Rust host-flow loops (`SleepFlow`, `PcPowerFlow`, `HostUpdateFlow`). */
    public String requestRaw(String op, String argument) {
        String envelope;
        try {
            envelope = transport.request(host, op, argument);
        } catch (NativeApiException e) {
            Json.Obj err = Json.obj()
                    .put("kind", e.getKind().name().toLowerCase(java.util.Locale.ROOT))
                    .put("message", e.getMessage() == null ? "Native API error" : e.getMessage())
                    .put("http_status", e.getHttpStatus())
                    .put("retry_after_ms", e.getRetryAfterMs());
            if (e.getCode() != null) err.put("code", e.getCode());
            if (e.getSetting() != null) err.put("setting", e.getSetting());
            return Json.obj().put("error", err).toString();
        }
        if ("update".equals(op)) {
            try {
                KnownUpdates.record(host.hostUuid, NativePcPower.parseUpdate(decode(envelope)));
            } catch (NativeApiException | Json.SyntaxException ignored) {
            }
        } else if ("update.install".equals(op)) {
            try {
                KnownUpdates.installing(host.hostUuid, NativePcPower.parseUpdate(decode(envelope)));
            } catch (NativeApiException | Json.SyntaxException ignored) {
            }
        }
        return envelope;
    }

    private Json.Obj call(String op, String argument) throws NativeApiException {
        return decode(transport.request(host, op, argument));
    }

    private static NativeSession parseSession(Json.Obj json) throws NativeApiException {
        try {
            return NativeSession.parse(json);
        } catch (Json.SyntaxException e) {
            throw malformed("session", e);
        }
    }

    private static NativeRadeon parseRadeon(Json.Obj json) throws NativeApiException {
        try {
            return NativeRadeon.parse(json);
        } catch (Json.SyntaxException e) {
            throw malformed("radeon", e);
        }
    }

    private static PowerReply parsePower(Json.Obj json) throws NativeApiException {
        try {
            return new PowerReply(json);
        } catch (Json.SyntaxException e) {
            throw malformed("power action", e);
        }
    }

    private static NativeApiException malformed(String what, Json.SyntaxException e) {
        return new NativeApiException(NativeApiException.Kind.MALFORMED, "The host's " + what + " reply: " + e.getMessage());
    }

    /** Unwraps {"ok": ...} or throws the {"error": ...} as an exception. */
    static Json.Obj decode(String envelope) throws NativeApiException {
        try {
            Json.Obj json = Json.parseObject(envelope);
            Json.Obj ok = json.optObject("ok");
            if (ok != null) {
                return ok;
            }
            Json.Obj error = json.optObject("error");
            if (error == null) {
                throw new NativeApiException(NativeApiException.Kind.MALFORMED, "Unrecognized native API reply");
            }
            throw new NativeApiException(NativeApiException.Kind.of(error.optString("kind")),
                    error.optString("message", "Native API error"), error.optInt("http_status", 0),
                    error.isNull("code") ? null : error.optString("code", null),
                    error.isNull("setting") ? null : error.optString("setting", null),
                    error.optLong("retry_after_ms"));
        } catch (Json.SyntaxException e) {
            throw malformed("envelope", e);
        }
    }
}
