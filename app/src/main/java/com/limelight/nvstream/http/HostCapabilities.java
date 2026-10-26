package com.limelight.nvstream.http;

import android.content.Context;

import androidx.annotation.StringRes;

import com.limelight.R;

import org.json.JSONException;
import org.json.JSONObject;
import org.xmlpull.v1.XmlPullParser;
import org.xmlpull.v1.XmlPullParserException;
import org.xmlpull.v1.XmlPullParserFactory;

import java.io.IOException;
import java.io.StringReader;
import java.util.Collections;
import java.util.EnumMap;
import java.util.EnumSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;

/**
 * What one host can do, judged only from the fields its serverinfo reply carries. It is built in
 * {@link NvHTTP#getComputerDetails(String)} and cached per PC, so an offline PC still shows the
 * controls it supports. The family names the host for labels and protocol quirks; features alone
 * decide what Nightjar offers.
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
        /** Rubylight's /api/native/v1: library, session actions, Radeon state. The UI shows its results, not its name. */
        NATIVE_API(0),
        /** Sleep PC (/power-actions/v1, Vibepollo's wire format): suspend an idle host. */
        SLEEP(0);

        @StringRes public final int label;

        Feature(@StringRes int label) {
            this.label = label;
        }
    }

    /** What would make an unsupported feature available. Each text fits in 60 characters. */
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
        SLEEP_PERMISSION(R.string.host_feature_reason_sleep_permission);

        @StringRes public final int text;

        Reason(@StringRes int text) {
            this.text = text;
        }
    }

    /** Whether a host offers a feature; an unsupported one may carry what would change that. */
    public static final class Availability {
        public enum State { SUPPORTED, UNSUPPORTED, UNKNOWN }

        static final Availability SUPPORTED = new Availability(State.SUPPORTED, null);
        static final Availability NEVER = new Availability(State.UNSUPPORTED, null);
        static final Availability UNKNOWN = new Availability(State.UNKNOWN, null);

        public final State state;
        /** Set only when a host setting, permission, pairing or update would make it available. */
        public final Reason reason;

        private Availability(State state, Reason reason) {
            this.state = state;
            this.reason = reason;
        }

        static Availability unsupported(Reason reason) {
            return new Availability(State.UNSUPPORTED, reason);
        }

        public boolean isSupported() {
            return state == State.SUPPORTED;
        }

        public boolean isUnknown() {
            return state == State.UNKNOWN;
        }

        /** Unsupported, and nothing the user could change on the host alters that: hide the control. */
        public boolean isNever() {
            return state == State.UNSUPPORTED && reason == null;
        }

        /** Short text for a disabled control, or 0 when there is none. */
        @StringRes
        public int reasonText() {
            return reason == null ? 0 : reason.text;
        }

        @Override
        public boolean equals(Object other) {
            return other instanceof Availability && ((Availability) other).state == state &&
                    ((Availability) other).reason == reason;
        }

        @Override
        public int hashCode() {
            return Objects.hash(state, reason);
        }

        @Override
        public String toString() {
            return reason == null ? state.name() : state.name() + "(" + reason.name() + ")";
        }
    }

    /** A host never reached from this device: nothing is known, so nothing is ruled out. */
    public static final HostCapabilities UNKNOWN = new HostCapabilities(null);

    /** ServerCodecModeSupport bits of the four PyroWave profiles. */
    public static final int PYROWAVE_CODEC_BITS = 0x00800000 | 0x01000000 | 0x02000000 | 0x04000000;
    /** Bit of {@link #nativeFeatures()}; keep in step with HOST_FEATURE_PYROWAVE_RECORDS in pyrowave_protocol.h. */
    public static final int NATIVE_PYROWAVE_RECORDS = 1;

    private static final int JSON_VERSION = 1;
    // hostDisplay arrived in Rubylight 2.2.1.
    /** Pre-release tags in release order; the 3.0 hosts report "3.0.0-beta.N". */
    private static final String[] PRE_RELEASE_TAGS = {"-alpha.", "-beta.", "-rc."};
    /** number() stops at 100000, so a stage's numbers never reach into the next. */
    private static final int PRE_RELEASE_STAGE = 1000000;
    private static final int[] HOST_DISPLAY_MODES_VERSION = {2, 2, 1, Integer.MAX_VALUE};

    private final Evidence evidence;
    private final Family family;
    private final EnumMap<Feature, Availability> features = new EnumMap<>(Feature.class);

    private HostCapabilities(Evidence evidence) {
        this.evidence = evidence;
        this.family = evidence == null ? Family.UNKNOWN : evidence.family();
        for (Feature feature : Feature.values()) {
            features.put(feature, evidence == null ? Availability.UNKNOWN : evidence.availability(feature));
        }
    }

    /** Reads a serverinfo reply. Only {@link NvHTTP} builds capabilities from host replies. */
    static HostCapabilities fromServerInfo(String serverInfo) throws XmlPullParserException, IOException {
        return new HostCapabilities(Evidence.parse(serverInfo));
    }

    /** False for {@link #UNKNOWN}: this device has never read the host's serverinfo. */
    public boolean isKnown() {
        return evidence != null;
    }

    public Family family() {
        return family;
    }

    /** The Rubylight version the host reports, or null for other hosts. */
    public String hostVersion() {
        return evidence == null ? null : evidence.rustHostVersion;
    }

    /** The native API version the host reported: -1 for no native API, 0 when not usable by this client yet. */
    public int nativeApiVersion() {
        return evidence == null ? -1 : evidence.nativeApiVersion;
    }

    public boolean has(Feature feature) {
        return features.get(feature).isSupported();
    }

    public Availability availability(Feature feature) {
        return features.get(feature);
    }

    /** Supported or not known yet: for behavior that does no harm on a host without the feature. */
    public boolean mayHave(Feature feature) {
        return features.get(feature).state != Availability.State.UNSUPPORTED;
    }

    public Set<Feature> supported() {
        EnumSet<Feature> supported = EnumSet.noneOf(Feature.class);
        for (Map.Entry<Feature, Availability> entry : features.entrySet()) {
            if (entry.getValue().isSupported()) {
                supported.add(entry.getKey());
            }
        }
        return Collections.unmodifiableSet(supported);
    }

    /** Feature bits for the native stream, which has no other way to learn them. */
    public int nativeFeatures() {
        return has(Feature.PYROWAVE_RECORDS) ? NATIVE_PYROWAVE_RECORDS : 0;
    }

    /**
     * The codec support to negotiate with. Bits above Sunshine's own count as PyroWave only from a
     * host that also shows PyroWave; the stream description still has to offer it.
     */
    public int trustedCodecModeSupport(int serverCodecModeSupport) {
        return has(Feature.PYROWAVE) ? serverCodecModeSupport : serverCodecModeSupport & ~PYROWAVE_CODEC_BITS;
    }

    /** "Rubylight 2.2.21", "Apollo-compatible host" and so on, for identification only. */
    public String familyLabel(Context context) {
        switch (family) {
            case RUBYLIGHT:
                return context.getString(R.string.host_details_software_butterpollo, hostVersion());
            case VIBEPOLLO:
                return context.getString(R.string.host_details_software_vibepollo);
            case APOLLO:
                return context.getString(R.string.host_details_software_apollo);
            case SUNSHINE:
                return context.getString(R.string.host_details_software_other);
            case GAMESTREAM:
                return context.getString(R.string.host_details_software_gamestream);
            default:
                return context.getString(R.string.host_details_software_unknown);
        }
    }

    /** Versioned JSON of the serverinfo evidence: no addresses, MAC, names or tokens. Null when unknown. */
    public String toJson() {
        if (evidence == null) {
            return null;
        }
        try {
            return evidence.toJson().put("v", JSON_VERSION).toString();
        } catch (JSONException e) {
            throw new IllegalStateException(e);
        }
    }

    /** Stored capabilities, or {@link #UNKNOWN} for anything missing, damaged or in another format. */
    public static HostCapabilities fromJson(String json) {
        if (json == null) {
            return UNKNOWN;
        }
        try {
            JSONObject object = new JSONObject(json);
            if (object.optInt("v", -1) != JSON_VERSION) {
                return UNKNOWN;
            }
            return new HostCapabilities(Evidence.fromJson(object));
        } catch (JSONException e) {
            return UNKNOWN;
        }
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof HostCapabilities && Objects.equals(((HostCapabilities) other).evidence, evidence);
    }

    @Override
    public int hashCode() {
        return Objects.hashCode(evidence);
    }

    @Override
    public String toString() {
        return family + (hostVersion() == null ? "" : " " + hostVersion()) + " " + supported();
    }

    /**
     * "2.2.21", "2.0.0-rc.19" or "3.0.0-beta.1" as {major, minor, patch, pre-release}, where a release
     * sorts after its pre-releases and alpha, beta and rc sort in that order (the stage is the
     * thousands of the pre-release number); null for anything malformed. An optional "+build"
     * suffix is ignored.
     */
    static int[] parseRubylightVersion(String version) {
        if (version == null) {
            return null;
        }
        int[] parts = new int[4];
        int[] index = {0};
        for (int part = 0; part < 3; part++) {
            int value = number(version, index);
            if (value < 0 || (part < 2 && !consume(version, index, "."))) {
                return null;
            }
            parts[part] = value;
        }
        parts[3] = Integer.MAX_VALUE;
        for (int stage = 0; stage < PRE_RELEASE_TAGS.length; stage++) {
            if (consume(version, index, PRE_RELEASE_TAGS[stage])) {
                int pre = number(version, index);
                if (pre <= 0) {
                    return null;
                }
                parts[3] = (stage + 1) * PRE_RELEASE_STAGE + pre;
                break;
            }
        }
        if (consume(version, index, "+")) {
            int start = index[0];
            while (index[0] < version.length() && isBuildCharacter(version.charAt(index[0]))) {
                index[0]++;
            }
            if (index[0] == start) {
                return null;
            }
        }
        return index[0] == version.length() ? parts : null;
    }

    /** A decimal number without leading zeros at index[0], or -1. */
    private static int number(String text, int[] index) {
        int start = index[0];
        int value = 0;
        while (index[0] < text.length() && text.charAt(index[0]) >= '0' && text.charAt(index[0]) <= '9') {
            value = value * 10 + (text.charAt(index[0]++) - '0');
            if (value > 100000) {
                return -1;
            }
        }
        int length = index[0] - start;
        return length == 0 || (length > 1 && text.charAt(start) == '0') ? -1 : value;
    }

    private static boolean consume(String text, int[] index, String expected) {
        if (text.startsWith(expected, index[0])) {
            index[0] += expected.length();
            return true;
        }
        return false;
    }

    private static boolean isBuildCharacter(char c) {
        return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '.' || c == '-';
    }

    static boolean versionAtLeast(int[] version, int[] minimum) {
        if (version == null) {
            return false;
        }
        for (int i = 0; i < minimum.length; i++) {
            if (version[i] != minimum[i]) {
                return version[i] > minimum[i];
            }
        }
        return true;
    }

    /** The serverinfo fields capabilities are derived from; the only state that is stored. */
    static final class Evidence {
        boolean gameStream;
        boolean sunshineVersion;
        boolean paired;
        String rustHostVersion;
        long permission = -1;
        int serverCommands;
        Boolean virtualDisplayCapable;
        Boolean virtualDisplayDriverReady;
        Boolean virtualDisplayHdrCapable;
        Boolean frameLimiter;
        boolean pyroWaveFields;
        int pyroWaveProbeBytes;
        int pyroWaveCodecs;
        boolean encoderBitrate;
        int hardwareMetricsVersion = -1;
        int networkPathVersion = -1;
        boolean controllerPreparation;
        boolean currentGameUuid;
        int cursorShapeVersion = -1;
        /** -1 when the host has no native API, 0 when it has one but not for this request. */
        int nativeApiVersion = -1;
        /** -1 without PowerActionVersion: hosts send it only to paired clients over HTTPS. */
        int powerActionVersion = -1;
        boolean sleepSupported;
        /** The host lets this paired client put it to sleep. */
        boolean sleepAllowed;

        static Evidence parse(String serverInfo) throws XmlPullParserException, IOException {
            Evidence evidence = new Evidence();
            boolean encoderBitrate = false;
            boolean initialEncoderBitrate = false;
            XmlPullParser parser = XmlPullParserFactory.newInstance().newPullParser();
            parser.setInput(new StringReader(serverInfo));
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event != XmlPullParser.START_TAG || parser.getDepth() != 2) {
                    continue;
                }
                switch (parser.getName()) {
                    case "state":
                        // The MJOLNIR codename was used by GFE but never by any third-party server.
                        evidence.gameStream = parser.nextText().contains("MJOLNIR");
                        break;
                    case "appversion":
                        // Sunshine and its forks use -1 as the trailing build number.
                        evidence.sunshineVersion = parser.nextText().trim().endsWith(".-1");
                        break;
                    case "PairStatus":
                        evidence.paired = "1".equals(parser.nextText().trim());
                        break;
                    case "RustHostVersion":
                        evidence.rustHostVersion = parser.nextText().trim();
                        break;
                    case "Permission":
                        evidence.permission = unsigned(parser.nextText(), 0xFFFFFFFFL);
                        break;
                    case "ServerCommand":
                        evidence.serverCommands++;
                        break;
                    case "VirtualDisplayCapable":
                        evidence.virtualDisplayCapable = flag(parser.nextText());
                        break;
                    case "VirtualDisplayDriverReady":
                        evidence.virtualDisplayDriverReady = flag(parser.nextText());
                        break;
                    case "VirtualDisplayHDRCapable":
                        evidence.virtualDisplayHdrCapable = flag(parser.nextText());
                        break;
                    case "FrameLimiterSupported":
                        // Present at all marks the extensions; only "1" means a limiter exists.
                        evidence.frameLimiter = Boolean.TRUE.equals(flag(parser.nextText()));
                        break;
                    case "PyroWaveHostLinkMbps":
                        evidence.pyroWaveFields = true;
                        break;
                    case "PyroWaveBandwidthProbeBytes":
                        evidence.pyroWaveFields = true;
                        evidence.pyroWaveProbeBytes = (int) unsigned(parser.nextText(), NvHTTP.PYROWAVE_BANDWIDTH_PROBE_BYTES);
                        break;
                    case "ServerCodecModeSupport":
                        evidence.pyroWaveCodecs = (int) (unsigned(parser.nextText(), 0xFFFFFFFFL) & PYROWAVE_CODEC_BITS);
                        break;
                    case "RustHostEncoderBitrateKbps":
                        encoderBitrate = true;
                        break;
                    case "RustHostInitialEncoderBitrateKbps":
                        initialEncoderBitrate = true;
                        break;
                    case "HardwareMetricsVersion":
                        evidence.hardwareMetricsVersion = "1".equals(parser.nextText().trim()) ? 1 : 0;
                        break;
                    case "NetworkPathVersion":
                        evidence.networkPathVersion = "1".equals(parser.nextText().trim()) ? 1 : 0;
                        break;
                    case "ControllerPreparation":
                        evidence.controllerPreparation = "1".equals(parser.nextText().trim());
                        break;
                    case "CursorShapeVersion":
                        evidence.cursorShapeVersion = (int) unsigned(parser.nextText(), 0xFFFF);
                        break;
                    case "NativeApiVersion":
                        evidence.nativeApiVersion = (int) unsigned(parser.nextText(), 0xFFFF);
                        break;
                    case "PowerActionVersion":
                        evidence.powerActionVersion = (int) unsigned(parser.nextText(), 0xFFFF);
                        break;
                    case "SleepSupported":
                        evidence.sleepSupported = Boolean.TRUE.equals(flag(parser.nextText()));
                        break;
                    case "SleepAllowed":
                        evidence.sleepAllowed = Boolean.TRUE.equals(flag(parser.nextText()));
                        break;
                    case "currentgameuuid":
                        evidence.currentGameUuid = true;
                        break;
                    default:
                        break;
                }
            }
            evidence.encoderBitrate = encoderBitrate && initialEncoderBitrate;
            if (evidence.rustHostVersion != null && evidence.rustHostVersion.isEmpty()) {
                evidence.rustHostVersion = null;
            }
            return evidence;
        }

        private static long unsigned(String value, long max) {
            try {
                long parsed = Long.parseLong(value.trim());
                return parsed >= 0 && parsed <= max ? parsed : 0;
            } catch (NumberFormatException e) {
                return 0;
            }
        }

        /** "1"/"true" and "0"/"false"; null for anything else. */
        private static Boolean flag(String value) {
            value = value.trim();
            if ("1".equals(value) || "true".equalsIgnoreCase(value)) return Boolean.TRUE;
            if ("0".equals(value) || "false".equalsIgnoreCase(value)) return Boolean.FALSE;
            return null;
        }

        Family family() {
            if (gameStream) return Family.GAMESTREAM;
            if (rustHostVersion != null) return Family.RUBYLIGHT;
            if (pyroWaveFields || frameLimiter != null) return Family.VIBEPOLLO;
            if (permission != -1 || serverCommands > 0 || virtualDisplayCapable != null) return Family.APOLLO;
            if (sunshineVersion) return Family.SUNSHINE;
            return Family.UNKNOWN;
        }

        private boolean hasPermission(long mask) {
            return permission == -1 || (permission & mask) != 0;
        }

        private boolean canView() {
            return hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH);
        }

        /** Rubylight, or the extension fields that Vibepollo added and Rubylight also sends. */
        private boolean vibepolloExtensions() {
            return rustHostVersion != null || frameLimiter != null;
        }

        /**
         * No evidence for a feature. An unrecognized host rules nothing out; Apollo and Vibepollo
         * show some fields only to paired clients, so their unpaired replies don't rule those out.
         */
        private Availability absent(boolean hiddenUntilPaired) {
            Family family = family();
            if (family == Family.UNKNOWN ||
                    (hiddenUntilPaired && !paired && (family == Family.APOLLO || family == Family.VIBEPOLLO))) {
                return Availability.UNKNOWN;
            }
            return Availability.NEVER;
        }

        Availability availability(Feature feature) {
            switch (feature) {
                case PERMISSIONS:
                    return permission != -1 ? Availability.SUPPORTED : absent(false);
                case OTP_PAIRING:
                    // No field announces it; every host that reports permissions accepts it.
                    return permission != -1 || rustHostVersion != null ? Availability.SUPPORTED : absent(false);
                case CLIPBOARD:
                    if (permission == -1) return absent(false);
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    return canView() && (permission & (ComputerDetails.PERMISSION_CLIPBOARD_READ |
                            ComputerDetails.PERMISSION_CLIPBOARD_SET)) != 0 ? Availability.SUPPORTED :
                            Availability.unsupported(Reason.CLIPBOARD_PERMISSION);
                case SERVER_COMMANDS:
                    if (permission == -1) return serverCommands > 0 ? Availability.SUPPORTED : absent(false);
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    if ((permission & ComputerDetails.PERMISSION_SERVER_COMMAND) == 0) {
                        return Availability.unsupported(Reason.COMMAND_PERMISSION);
                    }
                    return serverCommands > 0 ? Availability.SUPPORTED : Availability.unsupported(Reason.NO_COMMANDS);
                case APP_UUIDS:
                    return currentGameUuid ? Availability.SUPPORTED : absent(false);
                case LAUNCH_BY_UUID:
                    // Rubylight and Vibepollo look an appuuid up before the ID. Apollo compares the ID
                    // with its running app's first, and Sunshine and GameStream read only the ID.
                    return vibepolloExtensions() ? Availability.SUPPORTED : absent(false);
                case VIRTUAL_DISPLAY:
                    return virtualDisplay();
                case VIRTUAL_DISPLAY_HDR: {
                    Availability display = virtualDisplay();
                    if (!display.isSupported()) return display;
                    if (virtualDisplayHdrCapable == null) return absent(false);
                    return virtualDisplayHdrCapable ? Availability.SUPPORTED : Availability.NEVER;
                }
                case HOST_DISPLAY_MODES:
                    if (rustHostVersion == null) return absent(false);
                    if (!versionAtLeast(parseRubylightVersion(rustHostVersion), HOST_DISPLAY_MODES_VERSION)) {
                        return Availability.unsupported(Reason.UPDATE_HOST);
                    }
                    return virtualDisplay();
                case FRACTIONAL_REFRESH:
                    // Rubylight, Apollo and Vibepollo read "59.94"; Sunshine truncates it to 59.
                    return vibepolloExtensions() || permission != -1 ? Availability.SUPPORTED : absent(false);
                case HOST_TIMING:
                    // The host names 0x5536 in its stream description, which serverinfo cannot show.
                    // Rubylight is the host that sends the reports; the stream checks the announcement.
                    return rustHostVersion != null ? Availability.SUPPORTED : absent(false);
                case HOST_VRR:
                case REMOTE_SESSION_TILES:
                case HOST_CONFIRMS_REPLACEMENT:
                    // No field of their own; they came with the extensions Vibepollo introduced.
                    return vibepolloExtensions() ? Availability.SUPPORTED : absent(false);
                case RUNTIME_BITRATE:
                    // Rubylight reports its encoder rate with it; Vibepollo serves it with its extensions.
                    if (rustHostVersion != null ? !encoderBitrate : frameLimiter == null) {
                        return rustHostVersion != null ? Availability.unsupported(Reason.UPDATE_HOST) : absent(false);
                    }
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    return canView() ? Availability.SUPPORTED : Availability.unsupported(Reason.VIEW_PERMISSION);
                case PYROWAVE:
                    if (!pyroWaveFields && rustHostVersion == null) return absent(true);
                    return pyroWaveCodecs != 0 ? Availability.SUPPORTED : Availability.NEVER;
                case PYROWAVE_RECORDS: {
                    Availability pyroWave = availability(Feature.PYROWAVE);
                    if (!pyroWave.isSupported()) return pyroWave;
                    // Rubylight frames records since 2.0.0-rc.1; Vibepollo since it offered PyroWave.
                    if (rustHostVersion == null) return Availability.SUPPORTED;
                    int[] version = parseRubylightVersion(rustHostVersion);
                    return version != null && version[0] >= 2 ? Availability.SUPPORTED : Availability.NEVER;
                }
                case PYROWAVE_BANDWIDTH_PROBE:
                    if (!pyroWaveFields) {
                        return rustHostVersion != null ? Availability.unsupported(Reason.UPDATE_HOST) : absent(true);
                    }
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    return pyroWaveProbeBytes == NvHTTP.PYROWAVE_BANDWIDTH_PROBE_BYTES ?
                            Availability.SUPPORTED : Availability.NEVER;
                case HARDWARE_METRICS:
                    if (hardwareMetricsVersion == -1) {
                        return rustHostVersion != null ? Availability.unsupported(Reason.UPDATE_HOST) : absent(false);
                    }
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    if (!canView()) return Availability.unsupported(Reason.VIEW_PERMISSION);
                    return hardwareMetricsVersion == 1 ? Availability.SUPPORTED :
                            Availability.unsupported(Reason.HARDWARE_METRICS_OFF);
                case NETWORK_PATH:
                    if (networkPathVersion == -1) {
                        return rustHostVersion != null ? Availability.unsupported(Reason.UPDATE_HOST) : absent(false);
                    }
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    return networkPathVersion == 1 ? Availability.SUPPORTED :
                            Availability.unsupported(Reason.VIEW_PERMISSION);
                case FRAME_LIMITER_STATUS:
                    return Boolean.TRUE.equals(frameLimiter) ? Availability.SUPPORTED : absent(false);
                case CONTROLLER_PREPARATION:
                    return controllerPreparation ? Availability.SUPPORTED :
                            rustHostVersion != null ? Availability.unsupported(Reason.UPDATE_HOST) : absent(false);
                case WEB_UI:
                    // Sunshine, Apollo, Vibepollo and Rubylight all serve one; GameStream has none.
                    return gameStream ? Availability.NEVER :
                            family() != Family.UNKNOWN ? Availability.SUPPORTED : Availability.UNKNOWN;
                case CURSOR_SHAPE:
                    // docs/protocol/cursor-shape.md: the highest version the host speaks; it also speaks 1.
                    return cursorShapeVersion >= 1 ? Availability.SUPPORTED : absent(false);
                case NATIVE_API:
                    // The host reports 0 over plain HTTP and to an unpaired certificate.
                    if (nativeApiVersion == -1) {
                        return rustHostVersion != null ? Availability.unsupported(Reason.UPDATE_HOST) : absent(false);
                    }
                    return !paired || nativeApiVersion < 1 ? Availability.unsupported(Reason.PAIR) :
                            Availability.SUPPORTED;
                case SLEEP:
                    // Rubylight and Vibepollo send the power fields only to a paired HTTPS client.
                    if (powerActionVersion == -1) {
                        // A paired HTTPS serverinfo always carries the power fields when the host has them.
                        if (rustHostVersion == null) return paired ? Availability.NEVER : absent(true);
                        return Availability.unsupported(paired ? Reason.UPDATE_HOST : Reason.PAIR);
                    }
                    if (!paired) return Availability.unsupported(Reason.PAIR);
                    if (powerActionVersion < 1 || !sleepSupported) return Availability.NEVER;
                    return sleepAllowed ? Availability.SUPPORTED : Availability.unsupported(Reason.SLEEP_PERMISSION);
                default:
                    throw new IllegalArgumentException(feature.name());
            }
        }

        private Availability virtualDisplay() {
            if (virtualDisplayCapable == null) return absent(true);
            if (!virtualDisplayCapable) {
                // Rubylight reports its own setting; elsewhere "false" means the platform has none.
                return rustHostVersion != null ? Availability.unsupported(Reason.VIRTUAL_DISPLAY_OFF) : Availability.NEVER;
            }
            return Boolean.FALSE.equals(virtualDisplayDriverReady) ?
                    Availability.unsupported(Reason.VIRTUAL_DISPLAY_DRIVER) : Availability.SUPPORTED;
        }

        JSONObject toJson() throws JSONException {
            JSONObject object = new JSONObject();
            object.put("gameStream", gameStream).put("sunshineVersion", sunshineVersion).put("paired", paired)
                    .put("permission", permission).put("serverCommands", serverCommands)
                    .put("pyroWaveFields", pyroWaveFields).put("pyroWaveProbeBytes", pyroWaveProbeBytes)
                    .put("pyroWaveCodecs", pyroWaveCodecs).put("encoderBitrate", encoderBitrate)
                    .put("hardwareMetricsVersion", hardwareMetricsVersion).put("networkPathVersion", networkPathVersion)
                    .put("controllerPreparation", controllerPreparation).put("currentGameUuid", currentGameUuid)
                    .put("cursorShapeVersion", cursorShapeVersion).put("nativeApiVersion", nativeApiVersion)
                    .put("powerActionVersion", powerActionVersion).put("sleepSupported", sleepSupported)
                    .put("sleepAllowed", sleepAllowed);
            if (rustHostVersion != null) object.put("rustHostVersion", rustHostVersion);
            if (frameLimiter != null) object.put("frameLimiter", frameLimiter);
            if (virtualDisplayCapable != null) object.put("virtualDisplayCapable", virtualDisplayCapable);
            if (virtualDisplayDriverReady != null) object.put("virtualDisplayDriverReady", virtualDisplayDriverReady);
            if (virtualDisplayHdrCapable != null) object.put("virtualDisplayHdrCapable", virtualDisplayHdrCapable);
            return object;
        }

        static Evidence fromJson(JSONObject object) throws JSONException {
            Evidence evidence = new Evidence();
            evidence.gameStream = object.getBoolean("gameStream");
            evidence.sunshineVersion = object.getBoolean("sunshineVersion");
            evidence.paired = object.getBoolean("paired");
            evidence.rustHostVersion = optionalString(object, "rustHostVersion");
            evidence.permission = object.getLong("permission");
            evidence.serverCommands = object.getInt("serverCommands");
            evidence.frameLimiter = optionalBoolean(object, "frameLimiter");
            evidence.virtualDisplayCapable = optionalBoolean(object, "virtualDisplayCapable");
            evidence.virtualDisplayDriverReady = optionalBoolean(object, "virtualDisplayDriverReady");
            evidence.virtualDisplayHdrCapable = optionalBoolean(object, "virtualDisplayHdrCapable");
            evidence.pyroWaveFields = object.getBoolean("pyroWaveFields");
            evidence.pyroWaveProbeBytes = object.getInt("pyroWaveProbeBytes");
            evidence.pyroWaveCodecs = object.getInt("pyroWaveCodecs") & PYROWAVE_CODEC_BITS;
            evidence.encoderBitrate = object.getBoolean("encoderBitrate");
            evidence.hardwareMetricsVersion = object.getInt("hardwareMetricsVersion");
            evidence.networkPathVersion = object.getInt("networkPathVersion");
            evidence.controllerPreparation = object.getBoolean("controllerPreparation");
            evidence.currentGameUuid = object.getBoolean("currentGameUuid");
            // Absent from capabilities stored before cursor shapes existed.
            evidence.cursorShapeVersion = object.optInt("cursorShapeVersion", -1);
            evidence.nativeApiVersion = object.optInt("nativeApiVersion", -1);
            evidence.powerActionVersion = object.optInt("powerActionVersion", -1);
            evidence.sleepSupported = object.optBoolean("sleepSupported", false);
            evidence.sleepAllowed = object.optBoolean("sleepAllowed", false);
            if (evidence.permission < -1 || evidence.permission > 0xFFFFFFFFL || evidence.serverCommands < 0 ||
                    (evidence.rustHostVersion != null && evidence.rustHostVersion.isEmpty())) {
                throw new JSONException("Invalid stored capabilities");
            }
            return evidence;
        }

        private static String optionalString(JSONObject object, String key) throws JSONException {
            return object.has(key) ? object.getString(key) : null;
        }

        private static Boolean optionalBoolean(JSONObject object, String key) throws JSONException {
            return object.has(key) ? object.getBoolean(key) : null;
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Evidence)) return false;
            Evidence that = (Evidence) other;
            return gameStream == that.gameStream && sunshineVersion == that.sunshineVersion &&
                    paired == that.paired && Objects.equals(rustHostVersion, that.rustHostVersion) &&
                    permission == that.permission && serverCommands == that.serverCommands &&
                    Objects.equals(virtualDisplayCapable, that.virtualDisplayCapable) &&
                    Objects.equals(virtualDisplayDriverReady, that.virtualDisplayDriverReady) &&
                    Objects.equals(virtualDisplayHdrCapable, that.virtualDisplayHdrCapable) &&
                    Objects.equals(frameLimiter, that.frameLimiter) && pyroWaveFields == that.pyroWaveFields &&
                    pyroWaveProbeBytes == that.pyroWaveProbeBytes && pyroWaveCodecs == that.pyroWaveCodecs &&
                    encoderBitrate == that.encoderBitrate && hardwareMetricsVersion == that.hardwareMetricsVersion &&
                    networkPathVersion == that.networkPathVersion &&
                    controllerPreparation == that.controllerPreparation && currentGameUuid == that.currentGameUuid &&
                    cursorShapeVersion == that.cursorShapeVersion && nativeApiVersion == that.nativeApiVersion &&
                    powerActionVersion == that.powerActionVersion && sleepSupported == that.sleepSupported &&
                    sleepAllowed == that.sleepAllowed;
        }

        @Override
        public int hashCode() {
            return Objects.hash(gameStream, sunshineVersion, paired, rustHostVersion, permission, serverCommands,
                    virtualDisplayCapable, virtualDisplayDriverReady, virtualDisplayHdrCapable, frameLimiter,
                    pyroWaveFields, pyroWaveProbeBytes, pyroWaveCodecs, encoderBitrate, hardwareMetricsVersion,
                    networkPathVersion, controllerPreparation, currentGameUuid, cursorShapeVersion,
                    nativeApiVersion, powerActionVersion, sleepSupported, sleepAllowed);
        }
    }
}
