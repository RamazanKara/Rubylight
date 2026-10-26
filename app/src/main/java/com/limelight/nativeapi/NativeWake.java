package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * GET /wake: the PC's wake targets, one per physical adapter, with the subnet broadcasts that reach it. Only what
 * waking needs is kept; the readiness checks and the sleep facts are the host console's to show.
 */
public final class NativeWake {
    public static final class Target {
        public final String mac;
        /** {@code ethernet} or {@code wifi}; any other value as the host sent it. */
        public final String kind;
        public final boolean connected;
        /** The adapter the host answered this device through. */
        public final boolean route;
        /** IPv4 literals such as {@code 192.168.4.255}. */
        public final List<String> broadcasts;

        Target(String mac, String kind, boolean connected, boolean route, List<String> broadcasts) {
            this.mac = mac;
            this.kind = kind;
            this.connected = connected;
            this.route = route;
            this.broadcasts = Collections.unmodifiableList(broadcasts);
        }
    }

    public final int revision;
    /** The MAC the host reports in serverinfo, or null. */
    public final String primary;
    public final List<Target> targets;

    NativeWake(int revision, String primary, List<Target> targets) {
        this.revision = revision;
        this.primary = primary;
        this.targets = Collections.unmodifiableList(targets);
    }

    static NativeWake parse(JSONObject json) throws JSONException {
        List<Target> targets = new ArrayList<>();
        JSONArray array = json.getJSONArray("targets");
        for (int i = 0; i < array.length(); i++) {
            // A target this client cannot read is one it does not send to.
            JSONObject target = array.optJSONObject(i);
            if (target == null || !(target.opt("mac") instanceof String)) continue;
            List<String> broadcasts = new ArrayList<>();
            JSONArray addresses = target.optJSONArray("broadcasts");
            for (int j = 0; addresses != null && j < addresses.length(); j++) {
                Object address = addresses.opt(j);
                if (address instanceof String) broadcasts.add((String) address);
            }
            targets.add(new Target(target.getString("mac"), target.optString("kind", null),
                    target.optBoolean("connected"), target.optBoolean("route"), broadcasts));
        }
        return new NativeWake(json.optInt("revision"), json.isNull("primary") ? null : json.optString("primary", null),
                targets);
    }
}
