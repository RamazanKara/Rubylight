package com.limelight.nativeapi;

import org.json.JSONException;
import org.json.JSONObject;

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

    public NativeLibrary library() throws NativeApiException {
        try {
            return NativeLibrary.parse(call("library", ""));
        } catch (JSONException e) {
            throw malformed("library", e);
        }
    }

    public NativeSession session() throws NativeApiException {
        return parseSession(call("session", ""));
    }

    /** Runs a session action and returns the session as it stands afterwards. Resume is the launch path, not an action. */
    public NativeSession sessionAction(NativeSession.ActionId action) throws NativeApiException {
        if (action == NativeSession.ActionId.RESUME || action == NativeSession.ActionId.UNKNOWN) {
            throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT,
                    "Not a session action: " + action.wire);
        }
        return parseSession(call("session_action", action.wire));
    }

    public NativeRadeon radeon() throws NativeApiException {
        return parseRadeon(call("radeon", ""));
    }

    /** Writes settings of one GPU; needs the Host control permission. */
    public NativeRadeon writeRadeon(long gpu, List<NativeRadeon.Change> changes) throws NativeApiException {
        try {
            return parseRadeon(call("radeon_write", NativeRadeon.writeJson(gpu, changes)));
        } catch (JSONException e) {
            throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT, "Radeon write: " + e.getMessage());
        }
    }

    /** Puts every setting this device changed back to its original value. */
    public NativeRadeon restoreRadeon() throws NativeApiException {
        return parseRadeon(call("radeon_restore", ""));
    }

    private JSONObject call(String op, String argument) throws NativeApiException {
        return decode(transport.request(host, op, argument));
    }

    private static NativeSession parseSession(JSONObject json) throws NativeApiException {
        try {
            return NativeSession.parse(json);
        } catch (JSONException e) {
            throw malformed("session", e);
        }
    }

    private static NativeRadeon parseRadeon(JSONObject json) throws NativeApiException {
        try {
            return NativeRadeon.parse(json);
        } catch (JSONException e) {
            throw malformed("radeon", e);
        }
    }

    private static NativeApiException malformed(String what, JSONException e) {
        return new NativeApiException(NativeApiException.Kind.MALFORMED, "The host's " + what + " reply: " + e.getMessage());
    }

    /** Unwraps {"ok": ...} or throws the {"error": ...} as an exception. */
    static JSONObject decode(String envelope) throws NativeApiException {
        try {
            JSONObject json = new JSONObject(envelope);
            JSONObject ok = json.optJSONObject("ok");
            if (ok != null) {
                return ok;
            }
            JSONObject error = json.optJSONObject("error");
            if (error == null) {
                throw new NativeApiException(NativeApiException.Kind.MALFORMED, "Unrecognized native API reply");
            }
            throw new NativeApiException(NativeApiException.Kind.of(error.optString("kind")),
                    error.optString("message", "Native API error"), error.optInt("http_status"),
                    error.isNull("code") ? null : error.optString("code", null),
                    error.isNull("setting") ? null : error.optString("setting", null),
                    error.optLong("retry_after_ms"));
        } catch (JSONException e) {
            throw malformed("envelope", e);
        }
    }
}
