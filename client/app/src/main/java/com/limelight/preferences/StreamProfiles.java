package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.Collections;
import java.util.EnumMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import java.util.Set;

/**
 * Named profiles: stream value sets the player names ("Home 4K120", "Hotel"), the one in use, and
 * the optional choice by network. Kept on this device only, in their own preferences file; home
 * Wi-Fi names are never sent anywhere or logged.
 */
public final class StreamProfiles {
    static final String PREFERENCES = "StreamProfiles";
    // Stored keys; never rename them (SettingsKeysTest).
    static final String ORDER = "order";
    static final String NEXT_ID = "next_id";
    static final String NAME = "name.";
    static final String VALUES = "values.";
    static final String ACTIVE = "active";
    static final String AUTO = "auto";
    static final String RULE_MOBILE = "rule.mobile";
    static final String RULE_HOME_WIFI = "rule.home_wifi";
    static final String RULE_WIFI = "rule.wifi";
    static final String HOME_WIFI = "home_wifi";
    /** A profile's screen ({@link StreamScreen#id}) and that screen's name, from 7.0; absent for every other profile. */
    static final String SCREEN = "screen.";
    static final String SCREEN_NAME = "screen_name.";
    static final int MAX_NAME = 32;
    static final int MAX_PROFILES = 50;

    public enum NameProblem { EMPTY, TOO_LONG, TAKEN }

    private StreamProfiles() {}

    static String ruleKey(StreamLayers.Network network) {
        switch (network) {
            case MOBILE: return RULE_MOBILE;
            case HOME_WIFI: return RULE_HOME_WIFI;
            case WIFI: return RULE_WIFI;
            default: return null;
        }
    }

    /** What is stored, read once; the resolver and the screens work from this. */
    public static final class Snapshot {
        static final Snapshot EMPTY = from(Collections.emptyMap());

        /** Profile names by ID, in the order the player made them. */
        public final Map<String, String> names;
        final Map<String, String> values;
        public final String activeId;
        public final boolean auto;
        final Map<StreamLayers.Network, String> rules;
        public final Set<String> homeWifi;
        /** The screen each profile is kept for, by profile ID in the profiles' order; most profiles have none. */
        final Map<String, StreamScreen> screens;

        private Snapshot(Map<String, String> names, Map<String, String> values, String activeId, boolean auto,
                         Map<StreamLayers.Network, String> rules, Set<String> homeWifi,
                         Map<String, StreamScreen> screens) {
            this.names = Collections.unmodifiableMap(names);
            this.values = values;
            this.activeId = activeId;
            this.auto = auto;
            this.rules = rules;
            this.homeWifi = Collections.unmodifiableSet(homeWifi);
            this.screens = Collections.unmodifiableMap(screens);
        }

        static Snapshot from(Map<String, ?> stored) {
            Map<String, String> names = new LinkedHashMap<>();
            Map<String, String> values = new LinkedHashMap<>();
            Object order = stored.get(ORDER);
            if (order instanceof String && !((String) order).isEmpty()) {
                for (String id : ((String) order).split(",")) {
                    Object name = stored.get(NAME + id);
                    if (!(name instanceof String) || names.containsKey(id)) continue;
                    names.put(id, (String) name);
                    Object value = stored.get(VALUES + id);
                    values.put(id, value instanceof String ? (String) value : HostStreamProfile.FORMAT);
                }
            }
            Map<StreamLayers.Network, String> rules = new EnumMap<>(StreamLayers.Network.class);
            for (StreamLayers.Network network : StreamLayers.Network.values()) {
                String key = ruleKey(network);
                Object rule = key == null ? null : stored.get(key);
                if (rule instanceof String && !((String) rule).isEmpty()) rules.put(network, (String) rule);
            }
            Object active = stored.get(ACTIVE);
            Object auto = stored.get(AUTO);
            Object home = stored.get(HOME_WIFI);
            Set<String> homeWifi = new HashSet<>();
            if (home instanceof Set) {
                for (Object name : (Set<?>) home) {
                    if (name instanceof String) homeWifi.add((String) name);
                }
            }
            Map<String, StreamScreen> screens = new LinkedHashMap<>();
            for (String id : names.keySet()) {
                Object screen = stored.get(SCREEN + id);
                Object screenName = stored.get(SCREEN_NAME + id);
                if (screen instanceof String && !((String) screen).trim().isEmpty()) {
                    screens.put(id, new StreamScreen((String) screen, screenName instanceof String ? (String) screenName : ""));
                }
            }
            return new Snapshot(names, values, active instanceof String && !((String) active).isEmpty() ?
                    (String) active : null, Boolean.TRUE.equals(auto), rules, homeWifi, screens);
        }

        /** The screen profile {@code id} is kept for; null for none. */
        public StreamScreen screen(String id) {
            return id == null ? null : screens.get(id);
        }

        public boolean exists(String id) {
            return id != null && names.containsKey(id);
        }

        public String name(String id) {
            return id == null ? null : names.get(id);
        }

        /** The profile's values as stored; may be unreadable (see {@link HostStreamProfile#deserialize}). */
        public String rawValues(String id) {
            return values.get(id);
        }

        /** The profile's values, skipping only individual unreadable entries. */
        public HostStreamProfile valuesOrEmpty(String id) {
            String raw = values.get(id);
            return raw == null ? new HostStreamProfile() : HostStreamProfile.deserializeLenient(raw);
        }

        /** The profile ID chosen for a network, or null when that network uses the profile in use. */
        public String rule(StreamLayers.Network network) {
            return rules.get(network);
        }

        public NameProblem checkName(String name, String exceptId) {
            String trimmed = name == null ? "" : name.trim();
            if (trimmed.isEmpty()) return NameProblem.EMPTY;
            if (trimmed.length() > MAX_NAME) return NameProblem.TOO_LONG;
            for (Map.Entry<String, String> entry : names.entrySet()) {
                if (!entry.getKey().equals(exceptId) &&
                        entry.getValue().trim().toLowerCase(Locale.ROOT).equals(trimmed.toLowerCase(Locale.ROOT))) {
                    return NameProblem.TAKEN;
                }
            }
            return null;
        }

        /** {@code base}, or {@code base} with the lowest free number after it, cut to fit. */
        public String freeName(String base) {
            String trimmed = base.trim();
            if (trimmed.length() > MAX_NAME) trimmed = trimmed.substring(0, MAX_NAME).trim();
            if (checkName(trimmed, null) == null) return trimmed;
            for (int number = 2; ; number++) {
                String suffix = " " + number;
                String stem = trimmed.length() + suffix.length() > MAX_NAME ?
                        trimmed.substring(0, MAX_NAME - suffix.length()).trim() : trimmed;
                if (checkName(stem + suffix, null) == null) return stem + suffix;
            }
        }
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    public static Snapshot load(Context context) {
        return Snapshot.from(prefs(context).getAll());
    }

    private static String order(Snapshot snapshot, String added) {
        StringBuilder order = new StringBuilder();
        for (String id : snapshot.names.keySet()) {
            if (order.length() > 0) order.append(',');
            order.append(id);
        }
        if (added != null) {
            if (order.length() > 0) order.append(',');
            order.append(added);
        }
        return order.toString();
    }

    /** Makes a profile with {@code values}; returns its ID, or null when there are too many. */
    public static String create(Context context, String name, HostStreamProfile values) {
        Snapshot snapshot = load(context);
        if (snapshot.names.size() >= MAX_PROFILES) return null;
        SharedPreferences prefs = prefs(context);
        int next = Math.max(1, prefs.getInt(NEXT_ID, 1));
        while (snapshot.names.containsKey(Integer.toString(next))) next++;
        String id = Integer.toString(next);
        prefs.edit().putInt(NEXT_ID, next + 1)
                .putString(NAME + id, name.trim())
                .putString(VALUES + id, values.serialize())
                .putString(ORDER, order(snapshot, id))
                .apply();
        return id;
    }

    public static void rename(Context context, String id, String name) {
        if (load(context).exists(id)) prefs(context).edit().putString(NAME + id, name.trim()).apply();
    }

    public static void saveValues(Context context, String id, HostStreamProfile values) {
        if (load(context).exists(id)) prefs(context).edit().putString(VALUES + id, values.serialize()).apply();
    }

    /** Removes the profile and every choice that names it, so nothing points at a missing profile. */
    public static void delete(Context context, String id) {
        Snapshot snapshot = load(context);
        if (!snapshot.exists(id)) return;
        Map<String, String> names = new LinkedHashMap<>(snapshot.names);
        names.remove(id);
        StringBuilder order = new StringBuilder();
        for (String kept : names.keySet()) {
            if (order.length() > 0) order.append(',');
            order.append(kept);
        }
        SharedPreferences.Editor editor = prefs(context).edit()
                .remove(NAME + id).remove(VALUES + id).remove(SCREEN + id).remove(SCREEN_NAME + id)
                .putString(ORDER, order.toString());
        if (id.equals(snapshot.activeId)) editor.remove(ACTIVE);
        for (StreamLayers.Network network : StreamLayers.Network.values()) {
            if (id.equals(snapshot.rule(network))) editor.remove(ruleKey(network));
        }
        editor.apply();
    }

    /** The profile in use when no network choice applies; null for none. */
    public static void setActive(Context context, String id) {
        SharedPreferences.Editor editor = prefs(context).edit();
        if (id == null) editor.remove(ACTIVE);
        else editor.putString(ACTIVE, id);
        editor.apply();
    }

    public static void setAuto(Context context, boolean auto) {
        prefs(context).edit().putBoolean(AUTO, auto).apply();
    }

    /** The profile for a network; null to use the profile in use there. */
    public static void setRule(Context context, StreamLayers.Network network, String id) {
        String key = ruleKey(network);
        if (key == null) return;
        SharedPreferences.Editor editor = prefs(context).edit();
        if (id == null) editor.remove(key);
        else editor.putString(key, id);
        editor.apply();
    }

    /**
     * Keeps profile {@code id} for {@code screen}, so a stream started there uses it; null keeps no profile for it. A
     * screen calls up one profile, so the profile kept for it before lets it go.
     */
    public static void keepForScreen(Context context, StreamScreen screen, String id) {
        Snapshot snapshot = load(context);
        SharedPreferences.Editor editor = prefs(context).edit();
        String before = StreamLayers.screenProfile(snapshot, screen);
        if (before != null) editor.remove(SCREEN + before).remove(SCREEN_NAME + before);
        if (id != null && snapshot.exists(id)) editor.putString(SCREEN + id, screen.id).putString(SCREEN_NAME + id, screen.name);
        editor.apply();
    }

    /** Lets profile {@code id} go of the screen it was kept for. */
    public static void forgetScreen(Context context, String id) {
        prefs(context).edit().remove(SCREEN + id).remove(SCREEN_NAME + id).apply();
    }

    public static void setHomeWifi(Context context, Set<String> names) {
        SharedPreferences.Editor editor = prefs(context).edit();
        if (names.isEmpty()) editor.remove(HOME_WIFI);
        else editor.putStringSet(HOME_WIFI, new HashSet<>(names));
        editor.apply();
    }

    /** Removes decoder-crash video overrides from every saved named profile. */
    static void clearVideoOverrides(Context context) {
        Snapshot snapshot = load(context);
        for (String id : snapshot.names.keySet()) {
            HostStreamProfile profile = snapshot.valuesOrEmpty(id);
            if (profile.clearVideoCrashFields()) {
                saveValues(context, id, profile);
            }
        }
    }
}
