package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.Display;

import com.limelight.binding.audio.MicGain;
import com.limelight.binding.audio.MicProcessing;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;

/**
 * Resolved stream and app preference configuration for Nightjar on Android.
 * Preference keys, defaults, value lists, and retired/renamed tables are inherited from generated {@link SettingsCatalogGen};
 * resolution math, migrations, and layered reading live in {@link PreferenceResolution}, {@link PreferenceMigrations},
 * and {@link PreferenceReader}.
 */
public class PreferenceConfiguration extends SettingsCatalogGen {
    // Re-declared directly on PreferenceConfiguration so Robolectric ReflectionHelpers.getStaticField
    // (which uses getDeclaredField without walking SettingsCatalogGen) finds them.
    public static final int PROFILE_NATIVE_VERSION = SettingsCatalogGen.PROFILE_NATIVE_VERSION;
    public static final String PREFS_VERSION_PREF_STRING = SettingsCatalogGen.PREFS_VERSION_PREF_STRING;
    public static final String RESOLUTION_PREF_STRING = SettingsCatalogGen.RESOLUTION_PREF_STRING;
    public static final String RESOLUTION_FOLLOWS_SCREEN_PREF_STRING = SettingsCatalogGen.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING;
    public static final String FPS_PREF_STRING = SettingsCatalogGen.FPS_PREF_STRING;
    public static final String ONSCREEN_CONTROLLER_PREF_STRING = SettingsCatalogGen.ONSCREEN_CONTROLLER_PREF_STRING;

    public enum FormatOption {
        AUTO,
        FORCE_AV1,
        FORCE_HEVC,
        FORCE_H264,
        FORCE_PYROWAVE,
    }

    public enum AnalogStickForScrolling {
        NONE,
        RIGHT,
        LEFT
    }

    public int width, height, fps;
    public int launchRefreshRateX100;
    public int bitrate;
    // The bitrate limit on mobile data in kbps; 0 for none.
    public int mobileBitrateLimit;
    public FormatOption videoFormat;
    public int deadzonePercentage;
    public int triggerDeadzonePercentage = DEFAULT_TRIGGER_DEADZONE;
    public int oscOpacity;
    // stretchVideo mirrors videoScaling == STRETCH for the renderers; change both with setVideoScaling.
    public boolean stretchVideo, enableSops, playHostAudio, disableWarnings;
    public int videoScaling = VideoScaling.FIT, videoAlignment = VideoScaling.CENTER;
    public int streamOrientation = StreamOrientation.AUTO;
    public boolean enableMic;
    public String micEcho = MicProcessing.Echo.AUTO.key;
    public String micEchoEngine = MicProcessing.EchoEngine.OWN.key;
    public boolean micNoiseSuppression = true, micAutoGain;
    public int micGainPercent = MicGain.DEFAULT_PERCENT;
    public boolean micButton = true;
    public boolean floatingMenuButton;
    public boolean oscFloatingSticks = true;
    public int oscStickDeadzone = DEFAULT_OSC_STICK_DEADZONE;
    public String language;
    /** Physical controllers reach the PC: Controllers is not Off. */
    public boolean sendControllers = true;
    public boolean smallIconMode, multiController, usbDriver, usbDualSense, usbDualSenseHaptics, usbSwitchPro, flipFaceButtons;
    public boolean usbForwarding;
    public boolean onscreenController;
    /** Hide the on-screen controls while an external gamepad is connected, and bring them back when it leaves. */
    public boolean oscAutoHideOnController = true;
    public boolean onlyL3R3;
    public boolean onscreenOwnPlayer;
    public boolean showGuideButton;
    /** The on-screen controller's Touchpad button: a PlayStation pad's touchpad click. */
    public boolean showTouchpadButton;
    /** The on-screen controls add a Steam Controller's trackpads, back buttons and grip touch. */
    public boolean oscSteamControls;
    public boolean enableHdr;
    /** Ask for 10-bit SDR when HDR is not streamed (see Sdr10Policy). Global only: not part of a PC's profile. */
    public boolean enableSdr10Bit;
    public boolean enablePip;
    public boolean autoReconnect;
    /** Seconds a stream waits for a silent network; 0 is off (see NetworkGrace). */
    public int networkGraceSeconds;
    /** Tell a Rubylight PC when the route to it is Wi-Fi or cellular. On unless switched off. */
    public boolean tellHostWireless = true;
    public boolean tellHostLinkSpeed = true;
    /** The video FEC asked for at launch from a PC that lets the client choose it; the PC's own by default. */
    public FecChoice fecChoice = FecChoice.HOST;
    /** Whether the bitrate is automatic for the stream's size and rate. */
    public boolean automaticBitrate;
    /** Link test before the first bitrate (Experimental). */
    public boolean linkProbe;
    public boolean quickResume;
    public com.limelight.nvstream.AdaptiveQualityController.Mode adaptiveQualityMode =
            com.limelight.nvstream.AdaptiveQualityController.Mode.OFF;
    public int qualityMinShortEdge = 720, qualityMinFps = 30;
    /** Whether device heat lowers the bitrate and resolution and drops PyroWave's forced clocks. */
    public boolean thermalProtection;
    /** Lower bitrate's Lower it sooner, as the busy controller for a PC that takes no rate changes. */
    public boolean autoBitrate;
    /** Lower bitrate's "Never: keep my bitrate". */
    public boolean keepBitrate;
    /** Automatic bitrate decided on this device and sent as 0x553C rate changes. */
    public boolean rateChangeBitrate;
    public volatile boolean enablePerfOverlay;

    /** Lower bitrate's choice (AutoBitrateChoice) as the stream acts on it. */
    public void applyLowerBitrate(String choice) {
        keepBitrate = AutoBitrateChoice.KEEP.equals(choice);
        autoBitrate = rateChangeBitrate = AutoBitrateChoice.SOONER.equals(choice);
    }

    /** The corner the performance overlay prefers: top_left, top_right, bottom_left or bottom_right. */
    public volatile String overlayPosition = "top_left";
    /** Append this device's battery percentage and local clock time to the compact performance overlay. */
    public volatile boolean compactOverlayDeviceInfo;
    public boolean enableLatencyToast;
    public boolean experimentalPacked444;
    /** Shifts each picture by the camera turn the host has not drawn yet (docs/frame-warp.md). */
    public boolean frameWarp;
    public boolean experimentalChipsetTuning;
    /** A tiny dot over the video changes colour every 100 ms (GameVideoOutput.updateCompositorHeartbeat). */
    public boolean tvDisplayCompatibility;
    public boolean bindAllUsb;
    public boolean mouseEmulation;
    public boolean mouseEmulationFixedStep;
    public int mouseEmulationSensitivity = DEFAULT_MOUSE_EMULATION_SENSITIVITY;
    public boolean disableRumble;
    /** The rumble response curve ({@code list_rumble_feel}): "standard", "strong" or "subtle". */
    public String rumbleFeel = "standard";
    public boolean backAsMeta;
    public boolean backOpensMenu;
    /** The controller quit combo ({@code list_gamepad_quit_combo}); "off" leaves it to the stream menu. */
    public String gamepadQuitCombo = "off";
    public boolean backAsGuide;
    public int guideMenuHoldMs = DEFAULT_GUIDE_MENU_HOLD_MS;
    public boolean captureSystemKeys = true;
    public boolean systemKeysFullscreenOnly;

    /** Whether system keys go to the PC now: {@code inWindow} is split screen, a freeform window or picture-in-picture. */
    public boolean sendsSystemKeys(boolean inWindow) {
        return captureSystemKeys && !(systemKeysFullscreenOnly && inWindow);
    }

    public boolean rightAltAsMeta;
    public boolean keyboardFollowsTextFields;
    public boolean clipboardSync;
    public AnalogStickForScrolling analogStickForScrolling;
    public boolean mouseNavButtons;
    /** Offer Input-only over a small stream on PCs whose host has no Input-only mode. */
    public boolean inputOnlyAnyHost;
    public boolean unlockFps;
    public boolean vibrateOsc;
    public boolean vibrateFallbackToDevice;
    public int vibrateFallbackToDeviceStrength;
    public boolean touchscreenTrackpad;
    public int touchMode;
    public int trackpadSpeed, mouseSpeed, localCursorSpeed;
    /** Percent per axis for a physical touchpad, -200 to 200. */
    public int trackpadSensitivityX = DEFAULT_TRACKPAD_SENSITIVITY, trackpadSensitivityY = DEFAULT_TRACKPAD_SENSITIVITY;
    /** A physical touchpad that reports its axes rotated: swap X and Y before the sensitivity applies. */
    public boolean trackpadSwapAxis;
    /** Percent for two-finger scrolling in the touchpad gestures, 10 to 300. */
    public int trackpadScrollSpeed = DEFAULT_TRACKPAD_SCROLL_SPEED;
    /** Forward physical touchpad contacts to a PC that takes them. */
    public boolean nativeTouchpad;
    public boolean trackpadAcceleration, trackpadMomentum;
    public String trackpadAccelerationCurve, trackpadMomentumGlide, trackpadArea, threeFingerTap;
    /** The on-screen PC keyboard layout id or "auto", size, and opacity. */
    public String pcKeyboardLayout;
    public int pcKeyboardSize, pcKeyboardOpacity;
    public String fourFingerTap = FingerTapGesture.NONE;
    /** Three- and four-finger taps run their actions in Multi-touch too. */
    public boolean multiTouchFingerTaps;
    public boolean fingerTapOnCancel;
    public MoonBridge.AudioConfiguration audioConfiguration;
    public int framePacing;
    public boolean absoluteMouseMode;
    public boolean enableAudioFx;
    public boolean muteAudio;
    public boolean dualScreen;
    public String secondScreenResolution = SECOND_SCREEN_NATIVE;
    public boolean audioPassthrough;
    public boolean reduceRefreshRate;
    public boolean fullRange;
    public boolean gamepadMotionSensors;
    public boolean gamepadTouchpadAsMouse;
    public boolean gamepadMotionSensorsFallbackToDevice;
    public boolean codecLowLatency, vendorLowLatency, codecPerformance;
    /** The HEVC low-latency mode is Off; reference-frame invalidation for HEVC goes with it. */
    public boolean disableHevcLowLatency;
    /** Auto or On while {@link #disableHevcLowLatency} is false; use {@link #hevcLowLatency()}. */
    public int hevcLowLatencyMode;
    public boolean phonePerformanceHints;
    public boolean gpuMaxClocks;
    public boolean dropLateFrames, useTextureView, unbatchedInput, networkPriority;
    public boolean displayQueueRecovery;
    public boolean fractionalVsync;
    public boolean submitHint, coveringDecoderSize, pyroWavePreacquire, codecDirect, receiveDecodePriority;
    public boolean clientRoaming, roamingMetered, receiveCoalescing;
    /** Jumbo video packets (Rubylight's MaxVideoPacket and x-rl-max-packet-size). Off by default. */
    public boolean jumboFrames;
    public int decodeQueueFrames;
    /** Play over the internet: a punched path to a PC that offers it, after every direct path. */
    public boolean internetPlay;
    public boolean virtualDisplay, enableYuv444, vrr;
    public boolean realVrr, gridHostFrameTimes;
    public boolean hostFrameTimes;
    public boolean contentRateFrameRate;
    public boolean arrFixedTiming;
    public int virtualDisplayScale;
    public int virtualDisplayDpi;
    /** RTX HDR and RTX Video enhancement for this PC's streams (rlRtxHdr, rlRtxVideo). */
    public Boolean rtxHdr, rtxVideo;
    public boolean resolutionFollowsScreen, fromPcProfile;
    public HostDisplayChoice hostDisplay;
    public String pcSettings;
    public StreamLayers.Result layers;
    public String layersText;
    public boolean pyroWaveUnavailable;
    public UpscalingPolicy.Mode upscalingMode = UpscalingPolicy.Mode.OFF;
    public int upscalingSharpness = 50;
    public boolean vulkanRender;
    public boolean debugHdrHlgOnly;

    /** A new stream tests the link before its first bitrate: switched on, and the bitrate is automatic. */
    public boolean probesFirstBitrate() {
        return linkProbe && automaticBitrate;
    }

    public static boolean isNativeResolution(int width, int height) {
        return PreferenceResolution.isNativeResolution(width, height);
    }

    /** Whether this stream follows the screen while it runs instead of keeping its size. */
    public boolean streamFollowsScreen(String panel, int[][] devicePanels) {
        if (resolutionFollowsScreen) return true;
        if (!isNativeResolution(width, height)) return false;
        if (panel != null && hasSize(getWidthFromResolutionString(panel), getHeightFromResolutionString(panel))) {
            return true;
        }
        if (devicePanels != null) {
            for (int[] other : devicePanels) {
                if (hasSize(other[0], other[1])) return true;
            }
        }
        return false;
    }

    private boolean hasSize(int otherWidth, int otherHeight) {
        return Math.max(width, height) == Math.max(otherWidth, otherHeight) &&
                Math.min(width, height) == Math.min(otherWidth, otherHeight);
    }

    public static int[][] panelSizes(int[][] modes) {
        return PreferenceResolution.panelSizes(modes);
    }

    public static boolean isSquarishScreen(int width, int height) {
        return PreferenceResolution.isSquarishScreen(width, height);
    }

    public static boolean isSquarishScreen(Display display) {
        return PreferenceResolution.isSquarishScreen(display);
    }

    static String convertFromLegacyResolutionString(String resString) {
        return PreferenceResolution.convertFromLegacyResolutionString(resString);
    }

    static int getWidthFromResolutionString(String resString) {
        return PreferenceResolution.getWidthFromResolutionString(resString);
    }

    static int getHeightFromResolutionString(String resString) {
        return PreferenceResolution.getHeightFromResolutionString(resString);
    }

    static String getResolutionText(int width, int height) {
        return PreferenceResolution.getResolutionText(width, height);
    }

    public static int getDefaultBitrate(String resString, String fpsString) {
        return PreferenceResolution.getDefaultBitrate(resString, fpsString);
    }

    public static boolean getDefaultSmallMode(Context context) {
        return PreferenceResolution.getDefaultSmallMode(context);
    }

    static boolean isBitrateUserSet(SharedPreferences prefs) {
        return PreferenceResolution.isBitrateUserSet(prefs);
    }

    /** The default the bitrate setting shows and stores, PyroWave's own while PyroWave is the chosen codec. */
    public static int getDefaultBitrate(Context context) {
        return getShownDefaultBitrate(PreferenceManager.getDefaultSharedPreferences(context), null, null, null);
    }

    static int getShownDefaultBitrate(SharedPreferences prefs, String res, String fps, String codec) {
        return PreferenceResolution.getShownDefaultBitrate(prefs, res, fps, codec);
    }

    static int pyroWaveMinimumAboveChosenKbps(PreferenceConfiguration config) {
        return PreferenceResolution.pyroWaveMinimumAboveChosenKbps(config);
    }

    static String pyroWaveBelowMinimumWarning(Context context, PreferenceConfiguration config) {
        return PreferenceResolution.pyroWaveBelowMinimumWarning(context, config);
    }

    static int decodeQueueFrames(String stored) {
        return PreferenceResolution.decodeQueueFrames(stored);
    }

    static int mobileBitrateLimit(String stored) {
        return PreferenceResolution.mobileBitrateLimit(stored);
    }

    static boolean isVirtualDisplayDpi(int percent) {
        return PreferenceResolution.isVirtualDisplayDpi(percent);
    }

    static int virtualDisplayDpi(String stored) {
        return PreferenceResolution.virtualDisplayDpi(stored);
    }

    static int layeredFramePacing(Context context, PreferenceConfiguration config, String layerPacing) {
        return PreferenceReader.layeredFramePacing(context, config, layerPacing);
    }

    /** Real VRR with adaptive refresh, else the switch for a fixed refresh; again after a layer sets VRR. */
    void resolveHostFrameTimes() {
        hostFrameTimes = vrr ? realVrr : gridHostFrameTimes;
    }

    /** The bitrate for this configuration's size, frame rate, and codec when nobody chose one. */
    int automaticBitrateKbps() {
        if (videoFormat == FormatOption.FORCE_PYROWAVE) {
            return com.limelight.nvstream.PyroWaveBitrateFloor.defaultKbps(width, height, fps, 0);
        }
        return getDefaultBitrate(getResolutionText(width, height), Integer.toString(fps));
    }

    static int framePacingValue(String str) {
        return PreferenceResolution.framePacingValue(str);
    }

    public static void resetStreamingSettings(Context context) {
        PreferenceMigrations.resetStreamingSettings(context);
    }

    public static void completeLanguagePreferenceMigration(Context context) {
        PreferenceMigrations.completeLanguagePreferenceMigration(context);
    }

    public void setVideoScaling(int mode) {
        videoScaling = mode;
        stretchVideo = mode == VideoScaling.STRETCH;
    }

    public static boolean isShieldAtvFirmwareWithBrokenHdr() {
        return Build.MANUFACTURER.equalsIgnoreCase("NVIDIA") &&
                Build.FINGERPRINT.contains("PPR1.180610.011/4079208_2235.1395");
    }

    /** The microphone processing these settings ask for. */
    public MicProcessing micProcessing() {
        return new MicProcessing(MicProcessing.Echo.of(micEcho), micNoiseSuppression, micAutoGain, micGainPercent,
                MicProcessing.EchoEngine.of(micEchoEngine));
    }

    /** The stored Hold Guide to open the stream menu time in ms; the default for anything else. */
    public static int guideMenuHoldMs(String stored) {
        return PreferenceResolution.guideMenuHoldMs(stored);
    }

    static int listChoice(String stored, int[] choices, int fallback) {
        return PreferenceResolution.listChoice(stored, choices, fallback);
    }

    static boolean sharpColor(SharedPreferences prefs) {
        return prefs.getBoolean(YUV444_PREF_STRING, false) || prefs.getBoolean(PACKED_444_PREF_STRING, false);
    }

    /** Writes Sharper colored text: On sets both switches, Off clears both. */
    static void saveSharpColor(SharedPreferences.Editor editor, boolean automatic) {
        editor.putBoolean(YUV444_PREF_STRING, automatic).putBoolean(PACKED_444_PREF_STRING, automatic);
    }

    public static PreferenceConfiguration readPreferences(Context context, String hostUuid) {
        return readPreferences(context, hostUuid, HostCapabilityCache.load(context, hostUuid));
    }

    /** The settings for a stream from this PC, given what the PC last showed it can do. */
    public static PreferenceConfiguration readPreferences(Context context, String hostUuid, HostCapabilities caps) {
        return readPreferences(context, hostUuid, caps, null, 0, StreamLayers.Network.OTHER);
    }

    static StreamLayers.Input layerInput(Context context, PreferenceConfiguration main, String hostUuid,
                                         String appUuid, int appId) {
        return PreferenceReader.layerInput(context, main, hostUuid, appUuid, appId);
    }

    /** The settings for a stream of a game from this PC with all layers resolved. */
    public static PreferenceConfiguration readPreferences(Context context, String hostUuid, HostCapabilities caps,
                                                          String appUuid, int appId, StreamLayers.Network network) {
        return readPreferences(context, hostUuid, caps, appUuid, appId, network, null);
    }

    /** As above; a stream that restarts names the screen it started on with its network. */
    public static PreferenceConfiguration readPreferences(Context context, String hostUuid, HostCapabilities caps,
                                                          String appUuid, int appId, StreamLayers.Network network,
                                                          StreamScreen screen) {
        return PreferenceReader.readLayered(context, hostUuid, caps, appUuid, appId, network, screen);
    }

    /** The codec for all PCs, or the device's own choice when that is PyroWave too. */
    public static FormatOption codecWithoutPyroWave(FormatOption allPcsFormat) {
        return allPcsFormat == FormatOption.FORCE_PYROWAVE ? FormatOption.AUTO : allPcsFormat;
    }

    static void migratePcProfilesToOverrides(Context context) {
        PreferenceMigrations.migratePcProfilesToOverrides(context);
    }

    public static String nativeResolution(Context context) {
        return PreferenceResolution.nativeResolution(context);
    }

    static String nativeResolution(Display display) {
        return PreferenceResolution.nativeResolution(display);
    }

    /** Streams at a fixed {@code resolution} such as 1920x1080; false, with the size unchanged, for anything else. */
    public boolean useResolution(String resolution) {
        if (resolution == null || !resolution.matches("\\d{2,5}x\\d{2,5}")) {
            return false;
        }
        int fixedWidth = getWidthFromResolutionString(resolution);
        int fixedHeight = getHeightFromResolutionString(resolution);
        if (fixedWidth < 64 || fixedHeight < 64 || fixedWidth > 16384 || fixedHeight > 16384) {
            return false;
        }
        width = fixedWidth;
        height = fixedHeight;
        resolutionFollowsScreen = false;
        return true;
    }

    /** Streams at the full size of {@code display}, whatever the saved resolution says. */
    public boolean followScreen(Display display) {
        String resolution = nativeResolution(display);
        if (resolution == null) {
            return false;
        }
        width = getWidthFromResolutionString(resolution);
        height = getHeightFromResolutionString(resolution);
        resolutionFollowsScreen = true;
        return true;
    }

    static int[] panelSize(int realWidth, int realHeight, int[][] modes) {
        return PreferenceResolution.panelSize(realWidth, realHeight, modes);
    }

    public static boolean hasPanelShape(Context context, int width, int height, double tolerance) {
        return PreferenceResolution.hasPanelShape(context, width, height, tolerance);
    }

    static boolean sameShape(int width, int height, int otherWidth, int otherHeight, double tolerance) {
        return PreferenceResolution.sameShape(width, height, otherWidth, otherHeight, tolerance);
    }

    public static float panelMaxRefreshRate(Context context) {
        return PreferenceResolution.panelMaxRefreshRate(context);
    }

    public static Display defaultDisplay(Context context) {
        return PreferenceResolution.defaultDisplay(context);
    }

    static boolean migrateToNativeDefaults(SharedPreferences prefs, String nativeResolution, float panelMaxHz) {
        return PreferenceMigrations.migrateToNativeDefaults(prefs, nativeResolution, panelMaxHz);
    }

    static String streamResolution(Context context, SharedPreferences prefs) {
        return PreferenceResolution.streamResolution(context, prefs);
    }

    static String effectiveResolution(SharedPreferences prefs, String saved, String nativeResolution) {
        return PreferenceResolution.effectiveResolution(prefs, saved, nativeResolution);
    }

    public static void migrateToNativeDefaults(Context context) {
        PreferenceMigrations.migrateToNativeDefaults(context);
    }

    // Auto, On or Off for the HEVC low-latency flags, as Moonlight V+'s "HEVC low-latency decoding"
    // (qiin2333/moonlight-vplus, GPL-3.0). Off stays stored in the switch's key above, so saved
    // values, profiles and the benchmark key keep their meaning; this key tells Auto from On.
    /** The HEVC low-latency mode. */
    public static int readHevcLowLatencyMode(SharedPreferences prefs) {
        if (prefs.getBoolean(DISABLE_HEVC_LOW_LATENCY_PREF_STRING, false)) return HEVC_LOW_LATENCY_OFF;
        return "on".equals(prefs.getString(HEVC_LOW_LATENCY_MODE_PREF_STRING, "auto")) ?
                HEVC_LOW_LATENCY_ON : HEVC_LOW_LATENCY_AUTO;
    }

    /** The HEVC low-latency mode the decoder uses: Off whenever {@link #disableHevcLowLatency} says so. */
    public int hevcLowLatency() {
        return disableHevcLowLatency ? HEVC_LOW_LATENCY_OFF : hevcLowLatencyMode;
    }

    static void writeHevcLowLatencyMode(SharedPreferences.Editor editor, int mode) {
        editor.putBoolean(DISABLE_HEVC_LOW_LATENCY_PREF_STRING, mode == HEVC_LOW_LATENCY_OFF)
                .putString(HEVC_LOW_LATENCY_MODE_PREF_STRING, hevcLowLatencyModeValue(mode));
    }

    static String hevcLowLatencyModeValue(int mode) {
        return mode == HEVC_LOW_LATENCY_ON ? "on" : mode == HEVC_LOW_LATENCY_OFF ? "off" : "auto";
    }

    static int lowLatencyDecodingCount(SharedPreferences prefs) {
        int on = 0;
        for (String key : LOW_LATENCY_DECODING_KEYS) {
            if (prefs.getBoolean(key, true)) on++;
        }
        return on;
    }

    static void setLowLatencyDecoding(SharedPreferences prefs, boolean enabled) {
        SharedPreferences.Editor editor = prefs.edit();
        for (String key : LOW_LATENCY_DECODING_KEYS) {
            editor.putBoolean(key, enabled);
        }
        editor.apply();
    }

    static boolean migrateAutoReconnectDefault(SharedPreferences prefs) {
        return PreferenceMigrations.migrateAutoReconnectDefault(prefs);
    }

    static boolean migrateReceiveDecodePriorityDefault(SharedPreferences prefs) {
        return PreferenceMigrations.migrateReceiveDecodePriorityDefault(prefs);
    }

    static void retireEngineChoice(SharedPreferences prefs) {
        PreferenceMigrations.retireEngineChoice(prefs);
    }

    static boolean migrateCodecLowLatencyCorruption(SharedPreferences prefs) {
        return PreferenceMigrations.migrateCodecLowLatencyCorruption(prefs);
    }

    public String resolutionSource() {
        return resolutionFollowsScreen ? (fromPcProfile ? "native, PC profile" : "native") :
                (fromPcProfile ? "fixed, PC profile" : "fixed in settings");
    }

    static void migrateOverlayMode(SharedPreferences prefs) {
        PreferenceMigrations.migrateOverlayMode(prefs);
    }

    static boolean migrateRetiredGlRender(SharedPreferences prefs, int sdkInt) {
        return PreferenceMigrations.migrateRetiredGlRender(prefs, sdkInt);
    }

    static void retireOscStyle(Context context, SharedPreferences prefs) {
        PreferenceMigrations.retireOscStyle(context, prefs);
    }

    static void migrateKeyNames(SharedPreferences prefs) {
        PreferenceMigrations.migrateKeyNames(prefs);
    }

    static void dropAutoBitrateLimits(SharedPreferences prefs) {
        PreferenceMigrations.dropAutoBitrateLimits(prefs);
    }

    /** The saved "Jumbo video packets" setting alone. */
    public static boolean jumboFrames(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context)
                .getBoolean(JUMBO_FRAMES_PREF_STRING, false);
    }

    public static PreferenceConfiguration readPreferences(Context context) {
        return PreferenceReader.readGlobal(context);
    }
}
