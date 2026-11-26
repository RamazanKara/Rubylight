package com.limelight.nvstream.http;

import android.content.Context;

import androidx.annotation.StringRes;

import com.limelight.R;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarJni;

import org.json.JSONException;
import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.util.Collections;
import java.util.EnumMap;
import java.util.EnumSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;

/**
 * What one host can do, judged only from the fields its serverinfo reply carries. Backed by
 * {@code nightjar_policy::http::HostCapabilities} via {@link NightjarJni#capabilitiesCall}.
 */
public final class HostCapabilities {
    /** The host software, for labels and wire-format quirks. Never gates a feature. */
    public enum Family { GAMESTREAM, SUNSHINE, APOLLO, VIBEPOLLO, RUBYLIGHT, UNKNOWN }

    /** Something a host can offer. The label is 0 for protocol details the UI never names. */
    public enum Feature {
        PERMISSIONS(R.string.host_feature_permissions),
        OTP_PAIRING(R.string.host_feature_otp_pairing),
        CLIPBOARD(R.string.host_feature_clipboard),
        SERVER_COMMANDS(R.string.host_feature_server_commands),
        APP_UUIDS(0),
        LAUNCH_BY_UUID(0),
        VIRTUAL_DISPLAY(R.string.host_feature_virtual_display),
        VIRTUAL_DISPLAY_HDR(R.string.host_feature_virtual_display_hdr),
        HOST_DISPLAY_MODES(R.string.host_feature_display_modes),
        DISPLAY_SCALE(R.string.host_feature_display_scale),
        FRACTIONAL_REFRESH(R.string.host_feature_fractional_refresh),
        HOST_VRR(R.string.host_feature_vrr),
        RUNTIME_BITRATE(R.string.host_feature_runtime_bitrate),
        PYROWAVE(R.string.host_feature_pyrowave),
        PYROWAVE_RECORDS(0),
        PYROWAVE_BANDWIDTH_PROBE(R.string.host_feature_speed_test),
        HARDWARE_METRICS(R.string.host_feature_hardware_metrics),
        NETWORK_PATH(R.string.host_feature_network_path),
        FRAME_LIMITER_STATUS(R.string.host_feature_frame_limiter),
        CONTROLLER_PREPARATION(R.string.host_feature_controller_preparation),
        REMOTE_SESSION_TILES(R.string.host_feature_remote_sessions),
        HOST_CONFIRMS_REPLACEMENT(0),
        HOST_TIMING(0),
        WEB_UI(R.string.host_feature_web_ui),
        CURSOR_SHAPE(R.string.host_feature_cursor_shape),
        FRAME_WARP(R.string.host_feature_frame_warp),
        NATIVE_API(0),
        SLEEP(0),
        FILE_UPLOAD(0),
        FILE_DOWNLOAD(0),
        DOLBY_VISION(0),
        HLG(0),
        CLIENT_FEC(0),
        BANDWIDTH_PROBE(0),
        USB_FORWARDING(0),
        AUDIO_PASSTHROUGH(0),
        SURROUND_714(0),
        TEXT_INPUT(0),
        CLIENT_KEYBOARD_LAYOUT(0),
        PEN_BARREL_ROLL(0),
        CONTROLLER_IDENTITY(0),
        PIPELINE_RATES(0),
        SECOND_SCREEN(0),
        INTERNET_PLAY(0),
        IMAGE_CLIPBOARD(0),
        SHARED_FOLDERS(0),
        COOP(0),
        NEURAL_FRAME_GENERATION(0),
        MIC_ONLY(0),
        PC_RESTART(0),
        PC_SHUTDOWN(0),
        HOST_UPDATE(0),
        PAIRING_APPROVE(0),
        QUICK_SUSPEND(0);

        @StringRes public final int label;

        Feature(@StringRes int label) {
            this.label = label;
        }
    }

    /** Why a feature is unsupported when a user change, pairing or host update could change it. */
    public enum Reason {
        UPDATE_HOST(R.string.host_feature_reason_update),
        PAIR(R.string.host_feature_reason_pair),
        VIEW_PERMISSION(R.string.host_feature_reason_view_permission),
        CLIPBOARD_PERMISSION(R.string.host_feature_reason_clipboard_permission),
        COMMAND_PERMISSION(R.string.host_feature_reason_command_permission),
        NO_COMMANDS(R.string.host_feature_reason_no_commands),
        VIRTUAL_DISPLAY_OFF(R.string.host_feature_reason_virtual_display_off),
        VIRTUAL_DISPLAY_DRIVER(R.string.host_feature_reason_virtual_display_driver),
        HARDWARE_METRICS_OFF(R.string.host_feature_reason_hardware_metrics_off),
        SLEEP_PERMISSION(R.string.host_feature_reason_sleep_permission),
        RESTART_SHUTDOWN_PERMISSION(R.string.host_feature_reason_restart_shutdown_permission),
        PAIRING_APPROVE_PERMISSION(R.string.host_feature_reason_pairing_approve_permission),
        FILE_PERMISSION(R.string.host_feature_reason_file_permission),
        USB_PERMISSION(R.string.host_feature_reason_usb_permission),
        FOLDER_PERMISSION(R.string.host_feature_reason_folder_permission);

        @StringRes public final int text;

        Reason(@StringRes int text) {
            this.text = text;
        }
    }

    public static final class Availability {
        public enum State { SUPPORTED, UNSUPPORTED, UNKNOWN }

        static final Availability SUPPORTED = new Availability(State.SUPPORTED, null);
        static final Availability NEVER = new Availability(State.UNSUPPORTED, null);
        static final Availability UNKNOWN = new Availability(State.UNKNOWN, null);

        static Availability unsupported(Reason reason) {
            return new Availability(State.UNSUPPORTED, reason);
        }

        public final State state;
        public final Reason reason;

        private Availability(State state, Reason reason) {
            this.state = state;
            this.reason = reason;
        }

        public boolean isSupported() { return state == State.SUPPORTED; }
        public boolean isUnknown() { return state == State.UNKNOWN; }
        public boolean isNever() { return state == State.UNSUPPORTED && reason == null; }
        @StringRes public int reasonText() { return reason == null ? 0 : reason.text; }

        @Override public boolean equals(Object other) {
            if (this == other) return true;
            if (!(other instanceof Availability)) return false;
            Availability that = (Availability) other;
            return state == that.state && reason == that.reason;
        }

        @Override public int hashCode() { return Objects.hash(state, reason); }

        @Override public String toString() {
            return reason == null ? state.name() : state.name() + ":" + reason.name();
        }
    }

    public static final HostCapabilities UNKNOWN = new HostCapabilities();
    public static final int PYROWAVE_CODEC_BITS = 0x00800000 | 0x01000000 | 0x02000000 | 0x04000000;
    public static final int NATIVE_PYROWAVE_RECORDS = 1;
    public static final int NATIVE_CONTROLLER_IDENTITY = 2;
    public static final String NVIDIA_ENHANCEMENTS_FIELD = "NvidiaEnhancements";
    public static final String MAX_VIDEO_PACKET_FIELD = "MaxVideoPacket";
    public static final int MAX_VIDEO_PACKET = 8192;
    public static final int STOCK_MAX_VIDEO_PACKET = 1400;
    public static final int RTX_HDR = 1;
    public static final int RTX_VIDEO = 2;
    public static final String RTX_HDR_PARAMETER = "rlRtxHdr";
    public static final String RTX_VIDEO_PARAMETER = "rlRtxVideo";

    private final String json;
    private final Family family;
    private final String hostVersion;
    private final int nativeApiVersion;
    private final int hdrFormats;
    private final int maxVideoPacket;
    private final int nvidiaEnhancements;
    private final int nativeFeatures;
    private final Map<Feature, Availability> availabilities;

    private HostCapabilities() {
        this.json = null;
        this.family = Family.UNKNOWN;
        this.hostVersion = null;
        this.nativeApiVersion = -1;
        this.hdrFormats = 0;
        this.maxVideoPacket = 0;
        this.nvidiaEnhancements = 0;
        this.nativeFeatures = 0;
        EnumMap<Feature, Availability> map = new EnumMap<>(Feature.class);
        for (Feature f : Feature.values()) {
            map.put(f, Availability.UNKNOWN);
        }
        this.availabilities = Collections.unmodifiableMap(map);
    }

    private HostCapabilities(Json.Obj snap) {
        this.json = snap.isNull("json") ? null : snap.getString("json");
        this.family = Family.valueOf(snap.getString("family"));
        this.hostVersion = snap.isNull("host_version") ? null : snap.getString("host_version");
        this.nativeApiVersion = snap.getInt("native_api_version");
        this.hdrFormats = snap.getInt("hdr_formats");
        this.maxVideoPacket = snap.getInt("max_video_packet");
        this.nvidiaEnhancements = snap.getInt("nvidia_enhancements");
        this.nativeFeatures = snap.getInt("native_features");
        Json.Obj features = snap.getObject("features");
        EnumMap<Feature, Availability> map = new EnumMap<>(Feature.class);
        for (Feature feature : Feature.values()) {
            Json.Obj entry = features.optObject(feature.name().toLowerCase(java.util.Locale.ROOT));
            if (entry == null) {
                map.put(feature, Availability.UNKNOWN);
            } else {
                String state = entry.getString("state");
                if ("supported".equals(state)) {
                    map.put(feature, Availability.SUPPORTED);
                } else if ("unknown".equals(state)) {
                    map.put(feature, Availability.UNKNOWN);
                } else {
                    String reasonName = entry.isNull("reason") ? null : entry.optString("reason", null);
                    map.put(feature, reasonName == null ? Availability.NEVER
                            : new Availability(Availability.State.UNSUPPORTED, Reason.valueOf(reasonName)));
                }
            }
        }
        this.availabilities = Collections.unmodifiableMap(map);
    }

    public static HostCapabilities fromSnapshotJson(Json.Obj snapshot) {
        return new HostCapabilities(snapshot);
    }

    private static Json.Obj callCapabilities(Json.Obj req) {
        Json.Obj reply = Json.parseObject(NightjarJni.capabilitiesCall(req.toString()));
        if (reply.has("error")) {
            throw new Json.SyntaxException(reply.optString("message", "Capabilities error"));
        }
        return reply.getObject("ok");
    }

    static HostCapabilities fromServerInfo(String serverInfo) throws XmlPullParserException, IOException {
        try {
            Json.Obj req = Json.obj().put("op", "from_xml").put("xml", serverInfo == null ? "" : serverInfo);
            Json.Obj reply = Json.parseObject(NightjarJni.capabilitiesCall(req.toString()));
            if (reply.has("error")) {
                throw new XmlPullParserException(reply.optString("message", "Malformed serverinfo"));
            }
            return new HostCapabilities(reply.getObject("ok"));
        } catch (Json.SyntaxException e) {
            throw new IOException("Invalid capabilities bridge response", e);
        }
    }

    public boolean isKnown() { return json != null; }
    public Family family() { return family; }
    public boolean isRubylight() { return family == Family.RUBYLIGHT; }
    public String hostVersion() { return hostVersion; }
    public int nativeApiVersion() { return nativeApiVersion; }
    public boolean has(Feature feature) { return availability(feature).isSupported(); }
    public int maxVideoPacket() { return maxVideoPacket; }
    public int nvidiaEnhancements() { return nvidiaEnhancements; }
    public boolean offersEnhancement(int bit) { return bit != 0 && (nvidiaEnhancements & bit) == bit; }
    public String enhancementParameters(Boolean rtxHdr, Boolean rtxVideo) {
        if (json == null || nvidiaEnhancements == 0) return "";
        try {
            Json.Obj req = Json.obj().put("op", "enhancement_parameters").put("caps", json);
            if (rtxHdr != null) req.put("rtx_hdr", rtxHdr.booleanValue());
            if (rtxVideo != null) req.put("rtx_video", rtxVideo.booleanValue());
            return Json.parseObject(NightjarJni.capabilitiesCall(req.toString())).optString("ok", "");
        } catch (Json.SyntaxException e) {
            return "";
        }
    }
    public Availability availability(Feature feature) { return availabilities.getOrDefault(feature, Availability.UNKNOWN); }
    public boolean mayHave(Feature feature) {
        Availability a = availability(feature);
        return a.isSupported() || a.isUnknown();
    }
    public Set<Feature> supported() {
        EnumSet<Feature> set = EnumSet.noneOf(Feature.class);
        for (Feature f : Feature.values()) if (has(f)) set.add(f);
        return set;
    }
    public int hdrFormats() { return hdrFormats; }
    public int nativeFeatures() { return nativeFeatures; }
    public int trustedCodecModeSupport(int serverCodecModeSupport) {
        return has(Feature.PYROWAVE) ? serverCodecModeSupport : serverCodecModeSupport & ~PYROWAVE_CODEC_BITS;
    }
    public String familyLabel(Context context) {
        switch (family) {
            case RUBYLIGHT: return context.getString(R.string.host_details_software_butterpollo, hostVersion());
            case VIBEPOLLO: return context.getString(R.string.host_details_software_vibepollo);
            case APOLLO: return context.getString(R.string.host_details_software_apollo);
            case SUNSHINE: return context.getString(R.string.host_details_software_other);
            case GAMESTREAM: return context.getString(R.string.host_details_software_gamestream);
            default: return context.getString(R.string.host_details_software_unknown);
        }
    }
    public String toJson() { return json; }
    public HostCapabilities withNativeCapabilities(String nativeJson) throws JSONException {
        if (nativeJson == null) throw new JSONException("Null capabilities");
        try {
            Json.Obj req = Json.obj().put("op", "with_native").put("native", nativeJson);
            req.put("caps", json);
            Json.Obj snapshot = callCapabilities(req);
            return isKnown() ? new HostCapabilities(snapshot) : this;
        } catch (Json.SyntaxException e) {
            throw new JSONException(e.getMessage());
        }
    }
    public static HostCapabilities fromJson(String json) {
        if (json == null) return UNKNOWN;
        try {
            Json.Obj req = Json.obj().put("op", "from_json").put("json", json);
            Json.Obj reply = Json.parseObject(NightjarJni.capabilitiesCall(req.toString()));
            if (reply.has("error")) return UNKNOWN;
            return new HostCapabilities(reply.getObject("ok"));
        } catch (Json.SyntaxException e) {
            return UNKNOWN;
        }
    }
    @Override public boolean equals(Object other) {
        if (this == other) return true;
        if (!(other instanceof HostCapabilities)) return false;
        HostCapabilities that = (HostCapabilities) other;
        return Objects.equals(json, that.json) && availabilities.equals(that.availabilities);
    }
    @Override public int hashCode() { return Objects.hash(json, availabilities); }
    @Override public String toString() { return family + (hostVersion == null ? "" : " " + hostVersion) + " " + supported(); }

    static long[] parseRubylightVersion(String version) {
        if (version == null) return null;
        try {
            Json.Obj req = Json.obj().put("op", "parse_rubylight_version").put("version", version);
            Json.Obj reply = Json.parseObject(NightjarJni.capabilitiesCall(req.toString()));
            if (reply.isNull("ok")) return null;
            Json.Arr arr = reply.getArray("ok");
            return new long[] {arr.getLong(0), arr.getLong(1), arr.getLong(2), arr.getLong(3)};
        } catch (Json.SyntaxException e) {
            return null;
        }
    }

    static boolean versionAtLeast(long[] version, long[] minimum) {
        if (version == null) return false;
        for (int i = 0; i < minimum.length; i++) {
            if (version[i] != minimum[i]) return version[i] > minimum[i];
        }
        return true;
    }
}
