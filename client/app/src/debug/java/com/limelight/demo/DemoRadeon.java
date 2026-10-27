package com.limelight.demo;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The host's Radeon endpoints for the demo, so the stream menu's panel can be seen in every state without a
 * host. "--es radeon &lt;mode&gt;" picks one; in configurable mode writes stick (and show the restore hint), in
 * error mode the driver refuses them.
 */
final class DemoRadeon {
    /** configurable, nocontrol, readonly, external, unsupported, unavailable, adlx, error or missing. */
    static volatile String mode = "configurable";

    private static final Map<String, Object> values = new LinkedHashMap<>();
    private static final Map<String, Object> originals = new LinkedHashMap<>();
    private static int revision = 1;

    private DemoRadeon() {
    }

    static synchronized void reset(String requested) {
        mode = requested == null ? "configurable" : requested;
        values.clear();
        originals.clear();
        values.put("anti_lag.enabled", true);
        values.put("chill.enabled", false);
        values.put("chill.max_fps", 144L);
        values.put("frame_rate_target.fps", 120L);
        values.put("vsync.mode", "off");
        values.put("future.thing", 3L);
        revision = 1;
    }

    static synchronized String read() {
        switch (mode) {
            case "missing":
                return "{\"error\":{\"kind\":\"not_found\",\"message\":\"No such endpoint.\",\"http_status\":404}}";
            case "adlx":
                return ok(panel("temporarily_unavailable", true, false,
                        "\"reason\":\"adlx_unavailable\",\"message\":\"AMD's driver library is not loaded on the PC.\"", ""));
            case "unavailable":
                return ok(panel("temporarily_unavailable", true, false,
                        "\"reason\":\"driver_busy\",\"message\":\"The driver is busy. Try again in a moment.\"",
                        setting("anti_lag.enabled", "anti_lag", "toggle", "temporarily_unavailable", null, null, null, null, true)
                                + "," + setting("chill.max_fps", "chill", "integer", "temporarily_unavailable", null, "fps", "30,240,10", null, true)));
            case "unsupported":
                return ok(panel("unsupported", true, false, "",
                        setting("anti_lag.enabled", "anti_lag", "toggle", "unsupported", null, null, null, null, false)
                                + "," + setting("chill.max_fps", "chill", "integer", "unsupported", null, "fps", "30,240,10", null, false)
                                + "," + setting("rsr.enabled", "rsr", "toggle", "unsupported", null, null, null, null, false)));
            case "external":
                return ok(panel("needs_external_setup", true, false,
                        "\"reason\":\"external_setup\",\"message\":\"Open AMD Software: Adrenalin Edition on the PC and turn on the Radeon features once.\"",
                        setting("anti_lag.enabled", "anti_lag", "toggle", "needs_external_setup", null, null, null,
                                "\"message\":\"Turn on Anti-Lag once in AMD Software: Adrenalin Edition.\"", true)
                                + "," + setting("vsync.mode", "vsync", "choice", "needs_external_setup", "off", null, null, null, true)));
            case "readonly":
                return ok(panel("read_only", true, false, "",
                        setting("anti_lag.enabled", "anti_lag", "toggle", "read_only", null, null, null, null, true)
                                + "," + setting("chill.max_fps", "chill", "integer", "read_only", null, "fps", "30,240,10", null, true)
                                + "," + setting("vsync.mode", "vsync", "choice", "read_only", null, null, null, null, true)));
            case "nocontrol":
                return ok(panel("configurable", false, false, "", everything()));
            default:
                return ok(panel("configurable", true, true, "", everything()));
        }
    }

    /** One change per call is enough for the demo; the whole request is validated first, as the host does. */
    static synchronized String write(String argument) {
        try {
            JSONArray changes = new JSONObject(argument).getJSONArray("changes");
            if ("error".equals(mode)) {
                String id = changes.getJSONObject(0).getString("id");
                return "{\"error\":{\"kind\":\"api\",\"message\":\"The driver refused " + id + ".\",\"http_status\":409,"
                        + "\"code\":\"driver_rejected\",\"setting\":\"" + id + "\"}}";
            }
            if (!"configurable".equals(mode)) {
                return "{\"error\":{\"kind\":\"api\",\"message\":\"Not writable.\",\"http_status\":422,\"code\":\"not_writable\"}}";
            }
            for (int i = 0; i < changes.length(); i++) {
                JSONObject change = changes.getJSONObject(i);
                String id = change.getString("id");
                if (!values.containsKey(id)) {
                    return "{\"error\":{\"kind\":\"api\",\"message\":\"Unknown setting.\",\"http_status\":422,"
                            + "\"code\":\"invalid_value\",\"setting\":\"" + id + "\"}}";
                }
            }
            for (int i = 0; i < changes.length(); i++) {
                JSONObject change = changes.getJSONObject(i);
                String id = change.getString("id");
                Object value = change.get("value");
                if (value instanceof Integer) value = ((Integer) value).longValue();
                if (!originals.containsKey(id)) originals.put(id, values.get(id));
                values.put(id, value);
            }
            revision++;
            return ok(panel("configurable", true, true, "", everything()));
        } catch (Exception e) {
            return "{\"error\":{\"kind\":\"api\",\"message\":\"Bad request.\",\"http_status\":400,\"code\":\"invalid_request\"}}";
        }
    }

    private static String ok(String body) {
        return "{\"ok\":" + body + "}";
    }

    private static String panel(String state, boolean hostControl, boolean writable, String extra, String settings) {
        return "{\"revision\":" + revision + ",\"state\":\"" + state + "\",\"host_control\":" + hostControl
                + ",\"writable_now\":" + writable + (extra.isEmpty() ? "" : "," + extra) + ",\"gpus\":["
                + (settings.isEmpty() ? "" : "{\"id\":11520,\"name\":\"AMD Radeon RX 7900 XT\",\"driver\":\"25.9.1\",\"settings\":["
                + settings + "]}") + "]}";
    }

    /** Mixed per-setting states: what a real host shows once the probe has covered some settings and not others. */
    private static String everything() {
        return setting("anti_lag.enabled", "anti_lag", "toggle", "configurable", null, null, null, null, true)
                + "," + setting("chill.enabled", "chill", "toggle", "configurable", null, null, null, null, true)
                + "," + setting("chill.max_fps", "chill", "integer", "configurable", null, "fps", "30,240,10", null, true)
                + "," + setting("frame_rate_target.fps", "frame_rate_target", "integer", "configurable", null, "fps", "30,240,1", null, true)
                + "," + setting("vsync.mode", "vsync", "choice", "configurable", null, null, null, null, true)
                + "," + setting("image_sharpening.sharpness", "image_sharpening", "integer", "read_only", 60L, "%", "0,100,1", null, true)
                + "," + setting("afmf.enabled", "afmf", "toggle", "needs_external_setup", false, null, null,
                "\"reason\":\"external_setup\",\"message\":\"Turn on AMD Fluid Motion Frames once in AMD Software: Adrenalin Edition.\"", true)
                + "," + setting("rsr.enabled", "rsr", "toggle", "unsupported", null, null, null, null, false)
                + "," + setting("boost.enabled", "boost", "toggle", "temporarily_unavailable", null, null, null,
                "\"reason\":\"driver_busy\",\"message\":\"The driver is busy. Try again in a moment.\"", false)
                + "," + setting("future.thing", "future", "integer", "configurable", null, "ms", "1,10,1", null, true);
    }

    /**
     * @param fixed a value to show instead of the stored one; null reads the stored value, if the setting has one
     * @param range "min,max,step"
     * @param tail extra JSON members (reason, message)
     * @param hasValue whether the state carries a value
     */
    private static String setting(String id, String group, String kind, String state, Object fixed, String unit,
                                  String range, String tail, boolean hasValue) {
        StringBuilder json = new StringBuilder("{\"id\":\"").append(id).append("\",\"group\":\"").append(group)
                .append("\",\"kind\":\"").append(kind).append("\",\"state\":\"").append(state).append('"');
        Object value = fixed != null ? fixed : values.get(id);
        if (hasValue && value != null) {
            json.append(",\"value\":").append(literal(value));
        }
        if (unit != null) json.append(",\"unit\":\"").append(unit).append('"');
        if (range != null) {
            String[] r = range.split(",");
            json.append(",\"range\":{\"min\":").append(r[0]).append(",\"max\":").append(r[1]).append(",\"step\":").append(r[2]).append('}');
        }
        if ("choice".equals(kind)) {
            json.append(",\"options\":[{\"value\":\"off\",\"label\":\"Off\"},{\"value\":\"always\",\"label\":\"Always on\"},"
                    + "{\"value\":\"app\",\"label\":\"Let the game decide\"}]");
        }
        Object original = originals.get(id);
        if (original != null && "configurable".equals(state) && !original.equals(values.get(id))) {
            json.append(",\"restore\":{\"original\":").append(literal(original)).append(",\"written\":")
                    .append(literal(values.get(id))).append('}');
        }
        if (tail != null) json.append(',').append(tail);
        return json.append('}').toString();
    }

    private static String literal(Object value) {
        return value instanceof String ? JSONObject.quote((String) value) : String.valueOf(value);
    }
}
