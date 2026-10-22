package com.limelight.nativeapi;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/**
 * GET /radeon and the answers of the writes: the host's Radeon settings. Values are a Boolean for a
 * toggle, a Long for an integer and a String for a choice.
 */
public final class NativeRadeon {
    public enum ControlState {
        CONFIGURABLE, READ_ONLY, NEEDS_EXTERNAL_SETUP, UNSUPPORTED, TEMPORARILY_UNAVAILABLE, UNKNOWN;

        static ControlState of(String wire) {
            try {
                return valueOf(wire.toUpperCase(Locale.ROOT));
            } catch (IllegalArgumentException | NullPointerException e) {
                return UNKNOWN;
            }
        }
    }

    public enum Kind {
        TOGGLE, INTEGER, CHOICE, UNKNOWN;

        static Kind of(String wire) {
            try {
                return valueOf(wire.toUpperCase(Locale.ROOT));
            } catch (IllegalArgumentException | NullPointerException e) {
                return UNKNOWN;
            }
        }
    }

    public static final class Range {
        public final long min;
        public final long max;
        public final long step;

        Range(long min, long max, long step) {
            this.min = min;
            this.max = max;
            this.step = step;
        }
    }

    public static final class Choice {
        public final String value;
        public final String label;

        Choice(String value, String label) {
            this.value = value;
            this.label = label;
        }
    }

    /** What a write changed: the value before it and the value it wrote, so Restore can undo it. */
    public static final class Restore {
        public final Object original;
        public final Object written;

        Restore(Object original, Object written) {
            this.original = original;
            this.written = written;
        }
    }

    public static final class Setting {
        public final String id;
        public final String group;
        public final Kind kind;
        public final ControlState state;
        /** Null when the host does not report one. */
        public final Object value;
        public final Range range;
        public final List<Choice> options;
        public final String unit;
        public final Restore restore;
        /** Why the setting is not configurable (a machine reason), or null. */
        public final String reason;
        /** The host's text for the user, or null. */
        public final String message;

        Setting(String id, String group, Kind kind, ControlState state, Object value, Range range,
                List<Choice> options, String unit, Restore restore, String reason, String message) {
            this.id = id;
            this.group = group;
            this.kind = kind;
            this.state = state;
            this.value = value;
            this.range = range;
            this.options = Collections.unmodifiableList(options);
            this.unit = unit;
            this.restore = restore;
            this.reason = reason;
            this.message = message;
        }

        public boolean isConfigurable() {
            return state == ControlState.CONFIGURABLE;
        }
    }

    public static final class Gpu {
        public final long id;
        public final String name;
        public final String driver;
        public final List<Setting> settings;

        Gpu(long id, String name, String driver, List<Setting> settings) {
            this.id = id;
            this.name = name;
            this.driver = driver;
            this.settings = Collections.unmodifiableList(settings);
        }
    }

    public final long revision;
    public final ControlState state;
    public final String reason;
    public final String message;
    /** Whether this device has the Host control permission that writes need. */
    public final boolean hostControl;
    public final boolean writableNow;
    public final List<Gpu> gpus;

    NativeRadeon(long revision, ControlState state, String reason, String message, boolean hostControl,
                 boolean writableNow, List<Gpu> gpus) {
        this.revision = revision;
        this.state = state;
        this.reason = reason;
        this.message = message;
        this.hostControl = hostControl;
        this.writableNow = writableNow;
        this.gpus = Collections.unmodifiableList(gpus);
    }

    public static NativeRadeon parse(JSONObject json) throws JSONException {
        JSONArray array = json.optJSONArray("gpus");
        List<Gpu> gpus = new ArrayList<>();
        for (int i = 0; array != null && i < array.length(); i++) {
            JSONObject gpu = array.getJSONObject(i);
            JSONArray settings = gpu.optJSONArray("settings");
            List<Setting> parsed = new ArrayList<>();
            for (int j = 0; settings != null && j < settings.length(); j++) {
                parsed.add(parseSetting(settings.getJSONObject(j)));
            }
            gpus.add(new Gpu(gpu.getLong("id"), gpu.optString("name"), gpu.optString("driver"), parsed));
        }
        return new NativeRadeon(json.optLong("revision"), ControlState.of(json.optString("state")),
                text(json, "reason"), text(json, "message"), json.optBoolean("host_control"),
                json.optBoolean("writable_now"), gpus);
    }

    private static Setting parseSetting(JSONObject json) throws JSONException {
        JSONObject range = json.optJSONObject("range");
        JSONObject restore = json.optJSONObject("restore");
        JSONArray options = json.optJSONArray("options");
        List<Choice> choices = new ArrayList<>();
        for (int i = 0; options != null && i < options.length(); i++) {
            JSONObject choice = options.getJSONObject(i);
            choices.add(new Choice(choice.getString("value"), choice.optString("label")));
        }
        return new Setting(json.getString("id"), json.optString("group"), Kind.of(json.optString("kind")),
                ControlState.of(json.optString("state")), value(json.opt("value")),
                range == null ? null : new Range(range.optLong("min"), range.optLong("max"), range.optLong("step")),
                choices, text(json, "unit"),
                restore == null ? null : new Restore(value(restore.opt("original")), value(restore.opt("written"))),
                text(json, "reason"), text(json, "message"));
    }

    /** A setting value as Boolean, Long or String; null for anything else. */
    static Object value(Object raw) {
        if (raw instanceof Boolean || raw instanceof String) {
            return raw;
        }
        if (raw instanceof Number) {
            return ((Number) raw).longValue();
        }
        return null;
    }

    private static String text(JSONObject json, String key) {
        return json.isNull(key) ? null : json.optString(key, null);
    }

    /** One setting to write: its id and the new value (Boolean, Long/Integer or String). */
    public static final class Change {
        public final String id;
        public final Object value;

        public Change(String id, Object value) {
            this.id = id;
            this.value = value;
        }
    }

    /** The body of a Radeon write for one GPU. */
    static String writeJson(long gpu, List<Change> changes) throws JSONException {
        JSONArray array = new JSONArray();
        for (Change change : changes) {
            array.put(new JSONObject().put("id", change.id).put("value", change.value));
        }
        return new JSONObject().put("gpu", gpu).put("changes", array).toString();
    }
}
