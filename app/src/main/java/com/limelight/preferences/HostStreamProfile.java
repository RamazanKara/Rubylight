package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nvstream.StreamConfiguration;

import java.math.BigDecimal;
import java.util.Map;

/**
 * A set of stream values: one PC's, one game's or a named profile's. Only the values chosen are
 * set; every value left null falls through to the layer below (see {@link StreamLayers}), so a
 * change there still reaches this set unless it has its own.
 */
public final class HostStreamProfile {
    private static final String PREFERENCES = "HostStreamProfiles";
    // Rubylight accepts up to 2 Gbps at stream setup; PyroWave's clean 4K60 needs about 1.6 Gbps.
    static final int MAX_BITRATE_KBPS = StreamConfiguration.MAX_BITRATE_KBPS;
    static final String FORMAT = "v2";
    private static final String NATIVE = "native";

    /** True: the size of the screen the stream starts on; false: width × height; null: all PCs. */
    public Boolean nativeResolution;
    public Integer width, height;
    public Integer refreshRateX100, bitrate, renderScale;
    /** The virtual display's Windows scale in percent; 0 is the PC's own. */
    public Integer displayScale;
    public PreferenceConfiguration.FormatOption codec;
    public Boolean virtualDisplay, hdr, fullRange, yuv444, vrr;
    public UpscalingPolicy.Mode upscalingMode;
    public Integer upscalingSharpness;
    /** A frame pacing choice as the global setting stores it, e.g. "balanced". */
    public String pacing;

    /** The values a set can hold; each one falls through on its own. */
    public enum Field {
        RESOLUTION, REFRESH, BITRATE, CODEC, UPSCALING, SHARPNESS, HDR, VRR, YUV444, FULL_RANGE, PACING,
        VIRTUAL_DISPLAY, RENDER_SCALE, DISPLAY_SCALE
    }

    static final String[] PACING_VALUES = {"latency", "balanced", "cap-fps", "smoothness"};

    public HostStreamProfile() {
    }

    /** A profile from before test.11: every value was a copy of the global settings when saved. */
    HostStreamProfile(int width, int height, int refreshRateX100, int bitrate, int renderScale,
                      PreferenceConfiguration.FormatOption codec, boolean virtualDisplay,
                      boolean hdr, boolean fullRange, boolean yuv444, Boolean vrr,
                      UpscalingPolicy.Mode upscalingMode, Integer upscalingSharpness,
                      boolean nativeResolution) {
        if ((upscalingMode == null) != (upscalingSharpness == null)) {
            throw new IllegalArgumentException("Incomplete upscaling profile");
        }
        this.nativeResolution = nativeResolution;
        this.width = requireRange(width, 64, 16384);
        this.height = requireRange(height, 64, 16384);
        this.refreshRateX100 = requireRange(refreshRateX100, 100, 100000);
        this.bitrate = requireRange(bitrate, 500, MAX_BITRATE_KBPS);
        this.renderScale = requireRange(renderScale, 50, 200);
        this.codec = codec;
        this.virtualDisplay = virtualDisplay;
        this.hdr = hdr;
        this.fullRange = fullRange;
        this.yuv444 = yuv444;
        this.vrr = vrr;
        this.upscalingMode = upscalingMode;
        this.upscalingSharpness = upscalingSharpness == null ? null : requireRange(upscalingSharpness, 0, 100);
    }

    static int requireRange(int value, int min, int max) {
        if (value < min || value > max) {
            throw new IllegalArgumentException("Value outside supported range");
        }
        return value;
    }

    static int parseRefreshRate(String text) {
        try {
            return requireRange(new BigDecimal(text.trim().replace(',', '.'))
                    .movePointRight(2).intValueExact(), 100, 100000);
        } catch (ArithmeticException e) {
            throw new IllegalArgumentException("Refresh rate needs at most two decimal places", e);
        }
    }

    /** Parses Mbps with up to three decimals into kbps. */
    static int parseBitrateMbps(String text) {
        try {
            return requireRange(new BigDecimal(text.trim().replace(',', '.'))
                    .movePointRight(3).intValueExact(), 500, MAX_BITRATE_KBPS);
        } catch (ArithmeticException e) {
            throw new IllegalArgumentException("Bitrate needs at most three decimal places", e);
        }
    }

    static String formatBitrateMbps(int kbps) {
        return BigDecimal.valueOf(kbps, 3).stripTrailingZeros().toPlainString();
    }

    static String formatRefreshRate(int refreshRateX100) {
        return BigDecimal.valueOf(refreshRateX100, 2).stripTrailingZeros().toPlainString();
    }

    public boolean isEmpty() {
        return nativeResolution == null && refreshRateX100 == null && bitrate == null && renderScale == null &&
                displayScale == null && codec == null && virtualDisplay == null && hdr == null && fullRange == null &&
                yuv444 == null && vrr == null && upscalingMode == null && upscalingSharpness == null && pacing == null;
    }

    public boolean has(Field field) {
        switch (field) {
            case RESOLUTION: return nativeResolution != null;
            case REFRESH: return refreshRateX100 != null;
            case BITRATE: return bitrate != null;
            case CODEC: return codec != null;
            case UPSCALING: return upscalingMode != null;
            case SHARPNESS: return upscalingSharpness != null;
            case HDR: return hdr != null;
            case VRR: return vrr != null;
            case YUV444: return yuv444 != null;
            case FULL_RANGE: return fullRange != null;
            case PACING: return pacing != null;
            case VIRTUAL_DISPLAY: return virtualDisplay != null;
            case DISPLAY_SCALE: return displayScale != null;
            default: return renderScale != null;
        }
    }

    /** Copies one value from {@code from}, set or not. */
    void take(Field field, HostStreamProfile from) {
        switch (field) {
            case RESOLUTION:
                nativeResolution = from.nativeResolution;
                width = from.width;
                height = from.height;
                break;
            case REFRESH: refreshRateX100 = from.refreshRateX100; break;
            case BITRATE: bitrate = from.bitrate; break;
            case CODEC: codec = from.codec; break;
            case UPSCALING: upscalingMode = from.upscalingMode; break;
            case SHARPNESS: upscalingSharpness = from.upscalingSharpness; break;
            case HDR: hdr = from.hdr; break;
            case VRR: vrr = from.vrr; break;
            case YUV444: yuv444 = from.yuv444; break;
            case FULL_RANGE: fullRange = from.fullRange; break;
            case PACING: pacing = from.pacing; break;
            case VIRTUAL_DISPLAY: virtualDisplay = from.virtualDisplay; break;
            case DISPLAY_SCALE: displayScale = from.displayScale; break;
            default: renderScale = from.renderScale; break;
        }
    }

    /** How many values this set holds. */
    public int count() {
        int count = 0;
        for (Field field : Field.values()) {
            if (has(field)) count++;
        }
        return count;
    }

    static String requirePacing(String value) {
        for (String known : PACING_VALUES) {
            if (known.equals(value)) return value;
        }
        throw new IllegalArgumentException("Invalid frame pacing");
    }

    public boolean setsResolution() {
        return nativeResolution != null;
    }

    /** Puts this PC's own values over the global settings in {@code config}. */
    void applyTo(PreferenceConfiguration config) {
        if (nativeResolution != null) {
            config.resolutionFollowsScreen = nativeResolution;
            if (width != null && height != null) {
                config.width = width;
                config.height = height;
            }
        }
        if (refreshRateX100 != null) {
            config.launchRefreshRateX100 = refreshRateX100;
            config.fps = Math.round(refreshRateX100 / 100f);
        }
        if (bitrate != null) config.bitrate = bitrate;
        if (renderScale != null) config.virtualDisplayScale = renderScale;
        if (displayScale != null) config.virtualDisplayDpi = displayScale;
        if (codec != null) config.videoFormat = codec;
        if (virtualDisplay != null) config.virtualDisplay = virtualDisplay;
        if (hdr != null) config.enableHdr = hdr;
        if (fullRange != null) config.fullRange = fullRange;
        if (yuv444 != null) config.enableYuv444 = yuv444;
        if (vrr != null) config.vrr = vrr;
        if (upscalingMode != null) config.upscalingMode = upscalingMode;
        if (upscalingSharpness != null) config.upscalingSharpness = upscalingSharpness;
        if (pacing != null) config.framePacing = PreferenceConfiguration.framePacingValue(pacing);
        if (config.enableHdr || config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
            config.useTextureView = false;
        }
    }

    /**
     * Drops every value that equals the global one. Old profiles copied all global settings, so
     * only the values that differ were really chosen for the PC; the rest follow all PCs again.
     */
    HostStreamProfile keepOnlyDifferencesFrom(PreferenceConfiguration global) {
        if (nativeResolution != null) {
            boolean sameSize = width != null && height != null && width == global.width && height == global.height;
            if (nativeResolution ? global.resolutionFollowsScreen : sameSize) {
                nativeResolution = null;
                width = height = null;
            }
        }
        int globalRefresh = global.launchRefreshRateX100 == 0 ? global.fps * 100 : global.launchRefreshRateX100;
        if (refreshRateX100 != null && refreshRateX100 == globalRefresh) refreshRateX100 = null;
        if (bitrate != null && bitrate == global.bitrate) bitrate = null;
        if (renderScale != null && renderScale == global.virtualDisplayScale) renderScale = null;
        if (displayScale != null && displayScale == global.virtualDisplayDpi) displayScale = null;
        if (codec == global.videoFormat) codec = null;
        if (virtualDisplay != null && virtualDisplay == global.virtualDisplay) virtualDisplay = null;
        if (hdr != null && hdr == global.enableHdr) hdr = null;
        if (fullRange != null && fullRange == global.fullRange) fullRange = null;
        if (yuv444 != null && yuv444 == global.enableYuv444) yuv444 = null;
        if (vrr != null && vrr == global.vrr) vrr = null;
        if (upscalingMode == global.upscalingMode) upscalingMode = null;
        if (upscalingSharpness != null && upscalingSharpness == global.upscalingSharpness) upscalingSharpness = null;
        return this;
    }

    String serialize() {
        StringBuilder out = new StringBuilder(FORMAT);
        if (nativeResolution != null) {
            out.append(";res=").append(nativeResolution ? NATIVE : width + "x" + height);
        }
        if (refreshRateX100 != null) out.append(";refresh=").append(refreshRateX100);
        if (bitrate != null) out.append(";bitrate=").append(bitrate);
        if (renderScale != null) out.append(";scale=").append(renderScale);
        if (displayScale != null) out.append(";dpi=").append(displayScale);
        if (codec != null) out.append(";codec=").append(codec.name());
        if (virtualDisplay != null) out.append(";virtual=").append(virtualDisplay);
        if (hdr != null) out.append(";hdr=").append(hdr);
        if (fullRange != null) out.append(";full_range=").append(fullRange);
        if (yuv444 != null) out.append(";yuv444=").append(yuv444);
        if (vrr != null) out.append(";vrr=").append(vrr);
        if (upscalingMode != null) out.append(";upscaling=").append(upscalingMode.name());
        if (upscalingSharpness != null) out.append(";sharpness=").append(upscalingSharpness);
        if (pacing != null) out.append(";pacing=").append(pacing);
        return out.toString();
    }

    static boolean isLegacy(String value) {
        return !value.equals(FORMAT) && !value.startsWith(FORMAT + ";");
    }

    static HostStreamProfile deserialize(String value) {
        return isLegacy(value) ? deserializeLegacy(value) : deserializeOverrides(value);
    }

    private static HostStreamProfile deserializeOverrides(String value) {
        HostStreamProfile profile = new HostStreamProfile();
        String[] entries = value.split(";");
        for (int i = 1; i < entries.length; i++) {
            int split = entries[i].indexOf('=');
            if (split <= 0) {
                throw new IllegalArgumentException("Invalid host profile entry");
            }
            String key = entries[i].substring(0, split);
            String field = entries[i].substring(split + 1);
            switch (key) {
                case "res":
                    if (field.equals(NATIVE)) {
                        profile.nativeResolution = true;
                    } else {
                        String[] size = field.split("x", -1);
                        if (size.length != 2) throw new IllegalArgumentException("Invalid resolution");
                        profile.nativeResolution = false;
                        profile.width = requireRange(Integer.parseInt(size[0]), 64, 16384);
                        profile.height = requireRange(Integer.parseInt(size[1]), 64, 16384);
                    }
                    break;
                case "refresh":
                    profile.refreshRateX100 = requireRange(Integer.parseInt(field), 100, 100000);
                    break;
                case "bitrate":
                    profile.bitrate = requireRange(Integer.parseInt(field), 500, MAX_BITRATE_KBPS);
                    break;
                case "scale":
                    profile.renderScale = requireRange(Integer.parseInt(field), 50, 200);
                    break;
                case "dpi":
                    // A scale Windows does not offer reads as the PC's own, as on the desktop,
                    // instead of dropping the PC's other values with it.
                    profile.displayScale = PreferenceConfiguration.virtualDisplayDpi(field);
                    break;
                case "codec":
                    profile.codec = PreferenceConfiguration.FormatOption.valueOf(field);
                    break;
                case "virtual":
                    profile.virtualDisplay = parseBoolean(field);
                    break;
                case "hdr":
                    profile.hdr = parseBoolean(field);
                    break;
                case "full_range":
                    profile.fullRange = parseBoolean(field);
                    break;
                case "yuv444":
                    profile.yuv444 = parseBoolean(field);
                    break;
                case "vrr":
                    profile.vrr = parseBoolean(field);
                    break;
                case "upscaling": {
                    String[] parts = field.split(":", -1);
                    if (parts.length > 2) throw new IllegalArgumentException("Invalid upscaling");
                    profile.upscalingMode = UpscalingPolicy.Mode.valueOf(parts[0]);
                    // Older v2 profiles stored both values in the same field.
                    if (parts.length == 2) profile.upscalingSharpness = requireRange(Integer.parseInt(parts[1]), 0, 100);
                    break;
                }
                case "sharpness":
                    profile.upscalingSharpness = requireRange(Integer.parseInt(field), 0, 100);
                    break;
                case "pacing":
                    profile.pacing = requirePacing(field);
                    break;
                default:
                    // A value a newer build added; this build leaves it to the global settings.
                    break;
            }
        }
        return profile;
    }

    private static boolean parseBoolean(String value) {
        if (value.equals("true")) return true;
        if (value.equals("false")) return false;
        throw new IllegalArgumentException("Invalid switch value");
    }

    private static HostStreamProfile deserializeLegacy(String value) {
        boolean nativeResolution = value.endsWith("," + NATIVE);
        if (nativeResolution) {
            value = value.substring(0, value.length() - NATIVE.length() - 1);
        }
        String[] fields = value.split(",", -1);
        if (fields.length != 10 && fields.length != 11 && fields.length != 13) {
            throw new IllegalArgumentException("Invalid host profile");
        }
        return new HostStreamProfile(Integer.parseInt(fields[0]), Integer.parseInt(fields[1]),
                Integer.parseInt(fields[2]), Integer.parseInt(fields[3]), Integer.parseInt(fields[4]),
                PreferenceConfiguration.FormatOption.valueOf(fields[5]),
                Boolean.parseBoolean(fields[6]), Boolean.parseBoolean(fields[7]),
                Boolean.parseBoolean(fields[8]), Boolean.parseBoolean(fields[9]),
                fields.length >= 11 && !fields[10].isEmpty() ? Boolean.parseBoolean(fields[10]) : null,
                fields.length == 13 ? UpscalingPolicy.Mode.valueOf(fields[11]) : null,
                fields.length == 13 ? Integer.parseInt(fields[12]) : null, nativeResolution);
    }

    /** This PC's stored values, unread, or null when it has none. */
    static String raw(Context context, String hostUuid) {
        return hostUuid == null ? null : context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .getString(hostUuid, null);
    }

    static HostStreamProfile load(Context context, String hostUuid) {
        if (hostUuid == null) {
            return null;
        }
        String value = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .getString(hostUuid, null);
        if (value == null) {
            return null;
        }
        try {
            HostStreamProfile profile = deserialize(value);
            return profile.isEmpty() ? null : profile;
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    void save(Context context, String hostUuid) {
        if (isEmpty()) {
            reset(context, hostUuid);
            return;
        }
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .edit().putString(hostUuid, serialize()).apply();
    }

    public static void reset(Context context, String hostUuid) {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .edit().remove(hostUuid).apply();
    }

    /**
     * Converts a stored profile from before test.11 into one that keeps only the values that
     * differ from {@code global}. Returns the new value, "" when nothing is left, or null when the
     * value is already converted.
     */
    static String migrateToOverrides(String value, PreferenceConfiguration global) {
        if (!isLegacy(value)) {
            return null;
        }
        HostStreamProfile profile = deserializeLegacy(value).keepOnlyDifferencesFrom(global);
        // VRR turns off frame timing on a Rubylight PC; the old screen saved a copy of the global
        // switch with every profile, so it follows the main setting again.
        profile.vrr = null;
        return profile.isEmpty() ? "" : profile.serialize();
    }

    // Applies migrateToOverrides to every saved profile; returns how many changed.
    static int migrateAllToOverrides(Context context, PreferenceConfiguration global) {
        SharedPreferences prefs = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
        SharedPreferences.Editor editor = prefs.edit();
        int changed = 0;
        for (Map.Entry<String, ?> entry : prefs.getAll().entrySet()) {
            if (!(entry.getValue() instanceof String)) continue;
            try {
                String migrated = migrateToOverrides((String) entry.getValue(), global);
                if (migrated == null) continue;
                if (migrated.isEmpty()) {
                    editor.remove(entry.getKey());
                } else {
                    editor.putString(entry.getKey(), migrated);
                }
                changed++;
            } catch (IllegalArgumentException ignored) {
                // An unreadable profile is left as it is; load() ignores it too.
            }
        }
        if (changed > 0) editor.apply();
        return changed;
    }

    // Test builds 0.4.0 test.6 to test.9 left PC profiles at 1920x1080 on phones whose screen is
    // not 1080p; such a profile becomes native once. Returns the new value, or null to keep it.
    static String migrateToNative(String value) {
        if (!isLegacy(value)) {
            return null;
        }
        HostStreamProfile profile = deserializeLegacy(value);
        if (profile.nativeResolution || profile.width != 1920 || profile.height != 1080) {
            return null;
        }
        return value + "," + NATIVE;
    }

    // Applies migrateToNative to every saved profile; returns how many changed.
    static int migrateAllToNative(Context context) {
        SharedPreferences prefs = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
        SharedPreferences.Editor editor = prefs.edit();
        int changed = 0;
        for (Map.Entry<String, ?> entry : prefs.getAll().entrySet()) {
            if (!(entry.getValue() instanceof String)) continue;
            try {
                String migrated = migrateToNative((String) entry.getValue());
                if (migrated != null) {
                    editor.putString(entry.getKey(), migrated);
                    changed++;
                }
            } catch (IllegalArgumentException ignored) {
                // An unreadable profile is left as it is; load() ignores it too.
            }
        }
        if (changed > 0) editor.apply();
        return changed;
    }

    /** A copy, for the settings screen's draft. */
    HostStreamProfile copy() {
        return deserializeOverrides(serialize());
    }
}
