package com.limelight.preferences;

import java.util.ArrayList;
import java.util.EnumMap;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/**
 * The stream values a stream starts with. Layers, highest first: this game, this PC, a named
 * profile (the one in use, or the one chosen for the network), then the main settings. Each value
 * comes from the highest layer that sets it. On mobile data, the optional bitrate limit then caps
 * the bitrate. A layer that is missing or can't be read is skipped and reported, never silently.
 */
public final class StreamLayers {
    /** Where a value comes from. */
    public enum Source { MAIN, PROFILE, PC, GAME, MOBILE_LIMIT }

    /** The network a stream starts on, as far as profiles care. OTHER is Ethernet, none or unknown. */
    public enum Network { MOBILE, HOME_WIFI, WIFI, OTHER }

    /** Why a profile was picked. */
    public enum Reason { NONE, IN_USE, NETWORK }

    public enum Problem {
        /** The profile picked (by hand or for the network) no longer exists; the main settings apply. */
        PROFILE_MISSING,
        /** The profile picked holds a value this build can't read; the main settings apply. */
        PROFILE_INVALID,
        /** This PC's own values can't be read; the layers below apply. */
        PC_INVALID,
        /** This game's own values can't be read; the layers below apply. */
        GAME_INVALID,
        /** A home Wi-Fi choice is set, but the Wi-Fi name can't be read; the Wi-Fi counts as other Wi-Fi. */
        HOME_WIFI_UNKNOWN,
    }

    public static final class Input {
        public StreamProfiles.Snapshot profiles = StreamProfiles.Snapshot.EMPTY;
        public Network network = Network.OTHER;
        /** False on Wi-Fi whose name could not be read (no location access, or location off). */
        public boolean wifiNameKnown = true;
        /** Stored values of this PC and this game, or null for none. */
        public String pcValues, gameValues;
        /** Bitrate limit on mobile data in kbps; 0 for none. */
        public int mobileLimitKbps;
        /** The bitrate of the main settings, for the mobile data limit. */
        public int mainBitrate;
    }

    public static final class Result {
        /** The values above the main settings; unset ones follow the main settings. */
        public final HostStreamProfile values = new HostStreamProfile();
        final EnumMap<HostStreamProfile.Field, Source> sources = new EnumMap<>(HostStreamProfile.Field.class);
        public final List<Problem> problems = new ArrayList<>();
        public Network network = Network.OTHER;
        /** The profile applied, or null. */
        public String profileId, profileName;
        /** Why {@link #profileId} was applied; for {@link Problem#PROFILE_MISSING} or INVALID, why it was picked. */
        public Reason reason = Reason.NONE;
        /** The name of a profile that was picked but can't be read. */
        public String invalidProfileName;
        public boolean pcApplied, gameApplied, bitrateLimited;
        /** A profile, this PC or this game set the bitrate (the mobile data limit does not count). */
        public boolean bitrateLayered;

        public Source source(HostStreamProfile.Field field) {
            Source source = sources.get(field);
            return source == null ? Source.MAIN : source;
        }

        /** True when any layer above the main settings set a value. */
        public boolean layered() {
            return !sources.isEmpty();
        }
    }

    private StreamLayers() {}

    /**
     * The network kind. A metered network (mobile data, or Wi-Fi that Android marks as metered,
     * such as a phone's hotspot) counts as mobile data. {@code wifiName} is null when unknown.
     */
    public static Network classify(boolean cellular, boolean wifi, boolean metered, String wifiName, Set<String> home) {
        if (cellular || (wifi && metered)) return Network.MOBILE;
        if (!wifi) return Network.OTHER;
        return wifiName != null && home.contains(wifiName) ? Network.HOME_WIFI : Network.WIFI;
    }

    /** A Wi-Fi name as Android reports it: without quotes, or null when hidden or unknown. */
    public static String cleanWifiName(String reported) {
        if (reported == null) return null;
        String name = reported;
        if (name.length() >= 2 && name.startsWith("\"") && name.endsWith("\"")) {
            name = name.substring(1, name.length() - 1);
        }
        if (name.isEmpty() || name.equalsIgnoreCase("<unknown ssid>") || name.equals("0x")) return null;
        return name;
    }

    public static Result resolve(Input in) {
        Result result = new Result();
        result.network = in.network;
        StreamProfiles.Snapshot profiles = in.profiles;
        String picked = null;
        if (profiles.auto) {
            if (in.network == Network.WIFI && !in.wifiNameKnown && !profiles.homeWifi.isEmpty() &&
                    profiles.rule(Network.HOME_WIFI) != null) {
                result.problems.add(Problem.HOME_WIFI_UNKNOWN);
            }
            picked = profiles.rule(in.network);
            if (picked != null) result.reason = Reason.NETWORK;
        }
        if (picked == null && profiles.activeId != null) {
            picked = profiles.activeId;
            result.reason = Reason.IN_USE;
        }

        HostStreamProfile profile = null;
        if (picked != null) {
            if (!profiles.exists(picked)) {
                result.problems.add(Problem.PROFILE_MISSING);
            } else {
                try {
                    profile = HostStreamProfile.deserialize(profiles.rawValues(picked));
                    result.profileId = picked;
                    result.profileName = profiles.name(picked);
                } catch (IllegalArgumentException e) {
                    result.problems.add(Problem.PROFILE_INVALID);
                    result.invalidProfileName = profiles.name(picked);
                }
            }
        }
        HostStreamProfile pc = read(in.pcValues, Problem.PC_INVALID, result);
        HostStreamProfile game = read(in.gameValues, Problem.GAME_INVALID, result);

        for (HostStreamProfile.Field field : HostStreamProfile.Field.values()) {
            if (game != null && game.has(field)) {
                result.values.take(field, game);
                result.sources.put(field, Source.GAME);
            } else if (pc != null && pc.has(field)) {
                result.values.take(field, pc);
                result.sources.put(field, Source.PC);
            } else if (profile != null && profile.has(field)) {
                result.values.take(field, profile);
                result.sources.put(field, Source.PROFILE);
            }
        }
        result.pcApplied = result.sources.containsValue(Source.PC);
        result.gameApplied = result.sources.containsValue(Source.GAME);
        result.bitrateLayered = result.values.bitrate != null;

        if (in.network == Network.MOBILE && in.mobileLimitKbps > 0) {
            int bitrate = result.values.bitrate != null ? result.values.bitrate : in.mainBitrate;
            if (bitrate > in.mobileLimitKbps) {
                result.values.bitrate = in.mobileLimitKbps;
                result.sources.put(HostStreamProfile.Field.BITRATE, Source.MOBILE_LIMIT);
                result.bitrateLimited = true;
            }
        }
        return result;
    }

    private static HostStreamProfile read(String stored, Problem problem, Result result) {
        if (stored == null) return null;
        try {
            return HostStreamProfile.deserialize(stored);
        } catch (IllegalArgumentException e) {
            result.problems.add(problem);
            return null;
        }
    }

    /** For the stream log: the layers in force, without names. */
    public static String describe(Result result) {
        StringBuilder out = new StringBuilder();
        out.append("network ").append(result.network.name().toLowerCase(Locale.ROOT));
        if (result.profileId != null) {
            out.append(", profile #").append(result.profileId).append(" (")
                    .append(result.reason == Reason.NETWORK ? "for network" : "in use").append(')');
        }
        if (result.pcApplied) out.append(", PC values");
        if (result.gameApplied) out.append(", game values");
        if (result.bitrateLimited) out.append(", mobile data bitrate limit");
        for (Problem problem : result.problems) out.append(", ").append(problem.name());
        return out.toString();
    }
}
