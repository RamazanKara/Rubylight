package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** GET /capabilities: what the host speaks and what this device may do on it. */
public final class NativeCapabilities {
    public final int apiVersion;
    public final String hostName;
    public final String hostVersion;
    public final List<String> features;
    public final boolean list;
    public final boolean view;
    public final boolean launch;
    /** The per-device host control permission. It is off by default and Close game needs it. */
    public final boolean hostControl;

    NativeCapabilities(int apiVersion, String hostName, String hostVersion, List<String> features,
                       boolean list, boolean view, boolean launch, boolean hostControl) {
        this.apiVersion = apiVersion;
        this.hostName = hostName;
        this.hostVersion = hostVersion;
        this.features = Collections.unmodifiableList(features);
        this.list = list;
        this.view = view;
        this.launch = launch;
        this.hostControl = hostControl;
    }

    static NativeCapabilities parse(JSONObject json) throws JSONException {
        JSONObject host = json.optJSONObject("host");
        JSONObject permissions = json.getJSONObject("permissions");
        List<String> features = new ArrayList<>();
        JSONArray array = json.optJSONArray("features");
        for (int i = 0; array != null && i < array.length(); i++) {
            features.add(array.getString(i));
        }
        return new NativeCapabilities(json.optInt("api_version"), host == null ? null : host.optString("name", null),
                host == null ? null : host.optString("version", null), features,
                permissions.optBoolean("list"), permissions.optBoolean("view"), permissions.optBoolean("launch"),
                permissions.optBoolean("host_control"));
    }
}
