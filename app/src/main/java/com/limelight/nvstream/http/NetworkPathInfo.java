package com.limelight.nvstream.http;

import org.json.JSONException;
import org.json.JSONObject;
import java.io.IOException;

/** Only the current peer's route category; no remote peer list or addresses. */
public final class NetworkPathInfo {
    public enum Kind { DIRECT, RELAY, PEER_RELAY, UNKNOWN }
    public final Kind kind;
    private NetworkPathInfo(Kind kind) { this.kind = kind; }
    public static NetworkPathInfo parse(String response) throws IOException {
        if (response == null || response.length() > 512) throw new IOException("Invalid route response size");
        try {
            JSONObject json = new JSONObject(response);
            Object version = json.get("version");
            if (!(version instanceof Number) || ((Number) version).doubleValue() != 1) throw new JSONException("Unknown version");
            String kind = json.getString("kind");
            return new NetworkPathInfo("direct".equals(kind) ? Kind.DIRECT : "relay".equals(kind) ? Kind.RELAY :
                    "peer_relay".equals(kind) ? Kind.PEER_RELAY : Kind.UNKNOWN);
        } catch (JSONException e) { throw new IOException("Invalid route response", e); }
    }
}
