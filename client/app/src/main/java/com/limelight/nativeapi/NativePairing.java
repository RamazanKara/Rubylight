package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** The devices waiting to pair with a host (GET /pairing/requests), never with their certificates. */
public final class NativePairing {
    private NativePairing() {
    }

    /** One waiting device. {@link #name} is the device's own choice; {@link #address} is what the host saw. */
    public static final class Request {
        public final String id;
        public final String name;
        public final String address;
        public final long ageSeconds;

        public Request(String id, String name, String address, long ageSeconds) {
            this.id = id;
            this.name = name;
            this.address = address;
            this.ageSeconds = ageSeconds;
        }
    }

    static List<Request> parseRequests(JSONObject json) throws JSONException {
        JSONArray requests = json.getJSONArray("requests");
        List<Request> parsed = new ArrayList<>(requests.length());
        for (int i = 0; i < requests.length(); i++) {
            JSONObject request = requests.getJSONObject(i);
            parsed.add(new Request(request.getString("id"), request.getString("name"), request.getString("address"),
                    request.getLong("age_seconds")));
        }
        return Collections.unmodifiableList(parsed);
    }
}
