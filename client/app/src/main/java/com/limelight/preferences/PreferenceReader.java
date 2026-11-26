package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.preference.PreferenceManager;

import com.limelight.binding.audio.MicGain;
import com.limelight.binding.audio.MicProcessing;
import com.limelight.binding.audio.Surround714;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TrackpadArea;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;

import static com.limelight.preferences.SettingsCatalogGen.*;

/**
 * Reads global and layered (named profile, PC, game) {@link PreferenceConfiguration} snapshots from {@link SharedPreferences}.
 */
final class PreferenceReader {
    private PreferenceReader() {}

    static StreamLayers.Input layerInput(Context context, PreferenceConfiguration main, String hostUuid,
                                         String appUuid, int appId) {
        StreamLayers.Input input = new StreamLayers.Input();
        input.profiles = StreamProfiles.load(context);
        input.pcValues = HostStreamProfile.raw(context, hostUuid);
        input.gameValues = AppStreamSettings.raw(context, hostUuid, appUuid, appId);
        input.mobileLimitKbps = main.mobileBitrateLimit;
        input.mainBitrate = main.bitrate;
        return input;
    }

    static int layeredFramePacing(Context context, PreferenceConfiguration config, String layerPacing) {
        if (config.vrr || config.hostFrameTimes) return FRAME_PACING_MIN_LATENCY;
        return layerPacing != null ? PreferenceResolution.framePacingValue(layerPacing)
                : PreferenceResolution.getFramePacingValue(context);
    }

    static PreferenceConfiguration readLayered(Context context, String hostUuid, HostCapabilities caps,
                                               String appUuid, int appId, StreamLayers.Network network,
                                               StreamScreen screen) {
        PreferenceConfiguration config = readGlobal(context);
        if (hostUuid != null) config.applyLowerBitrate(AutoBitrateChoice.forPc(context, hostUuid));
        PreferenceConfiguration.FormatOption allPcsFormat = config.videoFormat;
        PreferenceMigrations.migratePcProfilesToOverrides(context);
        config.hostDisplay = hostUuid == null ? null : HostDisplayChoice.load(context, hostUuid);
        StreamLayers.Input input = layerInput(context, config, hostUuid, appUuid, appId);
        if (network == null) {
            StreamNetwork.fill(context, input);
        } else {
            // A restart or a second stream picks as its stream did when it started.
            input.network = network;
            input.screen = screen;
        }
        StreamLayers.Result layers = StreamLayers.resolve(input);
        config.layers = layers;
        config.automaticBitrate &= !layers.bitrateLayered;
        if (layers.layered()) {
            HostStreamProfile profile = layers.values;
            boolean followedScreen = config.resolutionFollowsScreen;
            profile.applyTo(config);
            config.vrr &= com.limelight.binding.video.DisplayFrameRatePolicy.vrrOffered(context);
            config.resolveHostFrameTimes();
            StreamLayers.Source resolution = layers.source(HostStreamProfile.Field.RESOLUTION);
            config.fromPcProfile = resolution == StreamLayers.Source.PC || resolution == StreamLayers.Source.GAME;
            if (config.resolutionFollowsScreen && !followedScreen) {
                String nativeResolution = PreferenceResolution.nativeResolution(context);
                if (nativeResolution != null) {
                    config.width = PreferenceResolution.getWidthFromResolutionString(nativeResolution);
                    config.height = PreferenceResolution.getHeightFromResolutionString(nativeResolution);
                }
            }
            if (config.automaticBitrate) {
                int automatic = config.automaticBitrateKbps();
                boolean limited = input.network == StreamLayers.Network.MOBILE && input.mobileLimitKbps > 0 &&
                        automatic > input.mobileLimitKbps;
                config.bitrate = limited ? input.mobileLimitKbps : automatic;
                layers.bitrateLimited = limited;
            }
            config.pcSettings = layers.pcApplied ? input.pcValues : null;
            SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
            config.framePacing = layeredFramePacing(context, config, profile.pacing);
            config.reduceRefreshRate = !config.vrr && prefs.getBoolean(REDUCE_REFRESH_RATE_PREF_STRING, DEFAULT_REDUCE_REFRESH_RATE);
            config.enableHdr &= !PreferenceConfiguration.isShieldAtvFirmwareWithBrokenHdr();
            config.useTextureView = prefs.getBoolean(TEXTURE_VIEW_PREF_STRING, false) && !config.enableHdr && !config.enableSdr10Bit &&
                    config.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        }
        if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE
                && !caps.mayHave(HostCapabilities.Feature.PYROWAVE)) {
            config.pyroWaveUnavailable = true;
            config.videoFormat = PreferenceConfiguration.codecWithoutPyroWave(allPcsFormat);
            config.useTextureView = PreferenceManager.getDefaultSharedPreferences(context)
                    .getBoolean(TEXTURE_VIEW_PREF_STRING, false) && !config.enableHdr && !config.enableSdr10Bit;
            if (config.automaticBitrate) {
                int automatic = config.automaticBitrateKbps();
                boolean limited = input.network == StreamLayers.Network.MOBILE && input.mobileLimitKbps > 0 &&
                        automatic > input.mobileLimitKbps;
                config.bitrate = limited ? input.mobileLimitKbps : automatic;
                layers.bitrateLimited = limited;
            }
        }
        config.contentRateFrameRate = com.limelight.binding.video.DisplayFrameRatePolicy.tracksContentRate(
                PreferenceManager.getDefaultSharedPreferences(context)
                        .getBoolean(CONTENT_RATE_FRAME_RATE_PREF_STRING, false), config.reduceRefreshRate,
                config.vrr, config.useTextureView);
        return config;
    }

    static PreferenceConfiguration readGlobal(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        PreferenceMigrations.migrateToNativeDefaults(context);
        if (PreferenceMigrations.migrateAutoReconnectDefault(prefs)) {
            com.limelight.LimeLog.info("Stream recovery switched to its new default: on");
        }
        if (PreferenceMigrations.migrateReceiveDecodePriorityDefault(prefs)) {
            com.limelight.LimeLog.info("Decoder priority for receiving switched to its new default: on");
        }
        FoldedChoice.migrate(prefs);
        PreferenceConfiguration config = new PreferenceConfiguration();

        PreferenceMigrations.migrateKeyNames(prefs);
        PreferenceMigrations.migrateOverlayMode(prefs);
        if (PreferenceMigrations.migrateRetiredGlRender(prefs, Build.VERSION.SDK_INT)) {
            com.limelight.LimeLog.info("\"Render through OpenGL\" was on; it is now \"Render through Vulkan\"");
        }
        PreferenceMigrations.retireOscStyle(context, prefs);
        PreferenceMigrations.retireEngineChoice(prefs);
        if (PreferenceMigrations.migrateCodecLowLatencyCorruption(prefs)) {
            com.limelight.LimeLog.info("Low latency: Android's mode switched off once, this TV's decoder can corrupt the picture with it");
        }
        if (prefs.contains(LEGACY_ENABLE_51_SURROUND_PREF_STRING)) {
            if (prefs.getBoolean(LEGACY_ENABLE_51_SURROUND_PREF_STRING, false)) {
                prefs.edit()
                        .remove(LEGACY_ENABLE_51_SURROUND_PREF_STRING)
                        .putString(AUDIO_CONFIG_PREF_STRING, "51")
                        .apply();
            }
        }

        readResolutionAndFps(context, prefs, config);
        SharedPreferences.Editor missingDefaults = null;
        if (!prefs.contains(SMALL_ICONS_PREF_STRING)) {
            missingDefaults = prefs.edit().putBoolean(SMALL_ICONS_PREF_STRING, PreferenceResolution.getDefaultSmallMode(context));
        }
        if (!prefs.contains(GAMEPAD_MOTION_SENSORS_PREF_STRING) && Build.VERSION.SDK_INT == Build.VERSION_CODES.S) {
            if (missingDefaults == null) missingDefaults = prefs.edit();
            missingDefaults.putBoolean(GAMEPAD_MOTION_SENSORS_PREF_STRING, false);
        }
        if (missingDefaults != null) {
            missingDefaults.apply();
        }

        config.videoFormat = PreferenceResolution.getVideoFormatValue(context);
        config.bitrate = prefs.getInt(BITRATE_PREF_STRING, prefs.getInt(BITRATE_PREF_OLD_STRING, 0) * 1000);
        if (config.bitrate == 0) {
            config.bitrate = config.automaticBitrateKbps();
        }
        config.automaticBitrate = !PreferenceResolution.isBitrateUserSet(prefs);
        if (config.automaticBitrate) {
            config.bitrate = config.automaticBitrateKbps();
        }

        readAudioConfig(context, prefs, config);
        config.mobileBitrateLimit = PreferenceResolution.mobileBitrateLimit(prefs.getString(MOBILE_BITRATE_PREF_STRING, "0"));
        config.framePacing = PreferenceResolution.getFramePacingValue(context);
        config.analogStickForScrolling = PreferenceResolution.getAnalogStickForScrollingValue(context);
        config.deadzonePercentage = prefs.getInt(DEADZONE_PREF_STRING, DEFAULT_DEADZONE);
        config.triggerDeadzonePercentage = Math.max(0, Math.min(MAX_TRIGGER_DEADZONE,
                prefs.getInt(TRIGGER_DEADZONE_PREF_STRING, DEFAULT_TRIGGER_DEADZONE)));
        config.oscOpacity = prefs.getInt(OSC_OPACITY_PREF_STRING, DEFAULT_OPACITY);
        config.language = prefs.getString(LANGUAGE_PREF_STRING, DEFAULT_LANGUAGE);

        readStreamAndControlPrefs(context, prefs, config);
        readAdvancedAndDisplayPrefs(context, prefs, config);
        return config;
    }

    private static void readResolutionAndFps(Context context, SharedPreferences prefs, PreferenceConfiguration config) {
        String str = prefs.getString(LEGACY_RES_FPS_PREF_STRING, null);
        if (str != null) {
            if (str.equals("360p30")) { config.width = 640; config.height = 360; config.fps = 30; }
            else if (str.equals("360p60")) { config.width = 640; config.height = 360; config.fps = 60; }
            else if (str.equals("720p30")) { config.width = 1280; config.height = 720; config.fps = 30; }
            else if (str.equals("720p60")) { config.width = 1280; config.height = 720; config.fps = 60; }
            else if (str.equals("1080p30")) { config.width = 1920; config.height = 1080; config.fps = 30; }
            else if (str.equals("1080p60")) { config.width = 1920; config.height = 1080; config.fps = 60; }
            else if (str.equals("4K30")) { config.width = 3840; config.height = 2160; config.fps = 30; }
            else if (str.equals("4K60")) { config.width = 3840; config.height = 2160; config.fps = 60; }
            else { config.width = 1280; config.height = 720; config.fps = 60; }
            prefs.edit()
                    .remove(LEGACY_RES_FPS_PREF_STRING)
                    .putString(RESOLUTION_PREF_STRING, PreferenceResolution.getResolutionString(config.width, config.height))
                    .putString(FPS_PREF_STRING, "" + config.fps)
                    .apply();
            return;
        }
        String resStr = prefs.getString(RESOLUTION_PREF_STRING, DEFAULT_RESOLUTION);
        if (!resStr.contains("x")) {
            resStr = PreferenceResolution.convertFromLegacyResolutionString(resStr);
            prefs.edit().putString(RESOLUTION_PREF_STRING, resStr).apply();
        }
        String savedResolution = resStr;
        resStr = PreferenceResolution.effectiveResolution(prefs, resStr, PreferenceResolution.nativeResolution(context));
        config.resolutionFollowsScreen = !resStr.equals(savedResolution) ||
                prefs.getBoolean(RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false);
        config.width = PreferenceResolution.getWidthFromResolutionString(resStr);
        config.height = PreferenceResolution.getHeightFromResolutionString(resStr);
        config.fps = Integer.parseInt(prefs.getString(FPS_PREF_STRING, DEFAULT_FPS));
    }

    private static void readAudioConfig(Context context, SharedPreferences prefs, PreferenceConfiguration config) {
        String audioConfig = prefs.getString(AUDIO_CONFIG_PREF_STRING, DEFAULT_AUDIO_CONFIG);
        if (audioConfig.equals(AUDIO_CONFIG_714)) {
            // Moonlight V+'s 7.1.4 where this device plays it (the route may have changed since it
            // was picked), and 7.1 otherwise.
            if (Surround714.playable(context)) {
                config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_714_SURROUND;
            } else {
                com.limelight.LimeLog.info("7.1.4 isn't playable on this output; asking for 7.1");
                config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_71_SURROUND;
            }
        }
        else if (audioConfig.equals("71")) {
            config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_71_SURROUND;
        }
        else if (audioConfig.equals("51")) {
            config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_51_SURROUND;
        }
        else {
            config.audioConfiguration = MoonBridge.AUDIO_CONFIGURATION_STEREO;
        }
    }

    private static void readStreamAndControlPrefs(Context context, SharedPreferences prefs, PreferenceConfiguration config) {
        config.disableWarnings = prefs.getBoolean(DISABLE_TOASTS_PREF_STRING, DEFAULT_DISABLE_TOASTS);
        config.enableSops = prefs.getBoolean(SOPS_PREF_STRING, DEFAULT_SOPS);
        config.setVideoScaling(VideoScaling.resolve(prefs.getString(VideoScaling.PREF, null),
                prefs.getBoolean(STRETCH_PREF_STRING, DEFAULT_STRETCH)));
        config.videoAlignment = VideoScaling.alignment(prefs.getString(VideoScaling.ALIGNMENT_PREF, null));
        config.streamOrientation = StreamOrientation.fromValue(prefs.getString(StreamOrientation.PREF, null));
        config.playHostAudio = prefs.getBoolean(HOST_AUDIO_PREF_STRING, DEFAULT_HOST_AUDIO);
        config.enableMic = prefs.getBoolean(ENABLE_MIC_PREF_STRING, DEFAULT_ENABLE_MIC);
        config.micEcho = MicProcessing.Echo.of(prefs.getString(MIC_ECHO_PREF_STRING, null)).key;
        config.micEchoEngine = MicProcessing.EchoEngine.of(prefs.getString(MIC_ECHO_ENGINE_PREF_STRING, null)).key;
        config.micNoiseSuppression = prefs.getBoolean(MIC_NOISE_PREF_STRING, true);
        config.micAutoGain = prefs.getBoolean(MIC_AUTO_GAIN_PREF_STRING, false);
        config.micGainPercent = MicGain.clampPercent(prefs.getInt(MIC_GAIN_PREF_STRING, MicGain.DEFAULT_PERCENT));
        config.micButton = prefs.getBoolean(MIC_BUTTON_PREF_STRING, true);
        config.floatingMenuButton = prefs.getBoolean(FLOATING_MENU_BUTTON_PREF_STRING, false);
        config.oscFloatingSticks = !OSC_STICK_FIXED.equals(prefs.getString(OSC_STICK_MODE_PREF_STRING, null));
        config.oscStickDeadzone = Math.max(0, Math.min(MAX_OSC_STICK_DEADZONE,
                prefs.getInt(OSC_STICK_DEADZONE_PREF_STRING, DEFAULT_OSC_STICK_DEADZONE)));
        config.smallIconMode = prefs.getBoolean(SMALL_ICONS_PREF_STRING, PreferenceResolution.getDefaultSmallMode(context));
        config.multiController = prefs.getBoolean(MULTI_CONTROLLER_PREF_STRING, DEFAULT_MULTI_CONTROLLER);
        config.sendControllers = prefs.getBoolean(SEND_CONTROLLERS_PREF_STRING, true);
        config.usbDriver = prefs.getBoolean(USB_DRIVER_PREF_SRING, DEFAULT_USB_DRIVER);
        config.usbDualSense = prefs.getBoolean(USB_DUALSENSE_PREF_STRING, false);
        config.usbDualSenseHaptics = config.usbDualSense && prefs.getBoolean("checkbox_usb_dualsense_haptics", false);
        config.usbSwitchPro = prefs.getBoolean(USB_SWITCH_PRO_PREF_STRING, false);
        config.usbForwarding = prefs.getBoolean(USB_FORWARDING_PREF_STRING, false);
        config.onscreenController = prefs.getBoolean(ONSCREEN_CONTROLLER_PREF_STRING, ONSCREEN_CONTROLLER_DEFAULT);
        config.oscAutoHideOnController = prefs.getBoolean(OSC_AUTO_HIDE_PREF_STRING, OSC_AUTO_HIDE_DEFAULT);
        config.onlyL3R3 = prefs.getBoolean(ONLY_L3_R3_PREF_STRING, ONLY_L3_R3_DEFAULT);
        config.onscreenOwnPlayer = prefs.getBoolean(ONSCREEN_OWN_PLAYER_PREF_STRING, ONSCREEN_OWN_PLAYER_DEFAULT);
        config.showGuideButton = prefs.getBoolean(SHOW_GUIDE_BUTTON_PREF_STRING, SHOW_GUIDE_BUTTON_DEFAULT);
        config.showTouchpadButton = prefs.getBoolean(SHOW_TOUCHPAD_BUTTON_PREF_STRING, SHOW_TOUCHPAD_BUTTON_DEFAULT);
        config.oscSteamControls = prefs.getBoolean(OSC_STEAM_CONTROLS_PREF_STRING, OSC_STEAM_CONTROLS_DEFAULT);
        config.enableHdr = prefs.getBoolean(ENABLE_HDR_PREF_STRING, DEFAULT_ENABLE_HDR)
                && !PreferenceConfiguration.isShieldAtvFirmwareWithBrokenHdr();
        config.enableSdr10Bit = prefs.getBoolean(ENABLE_SDR10_PREF_STRING, DEFAULT_ENABLE_SDR10);
        config.enablePip = prefs.getBoolean(ENABLE_PIP_PREF_STRING, DEFAULT_ENABLE_PIP);
        config.autoReconnect = prefs.getBoolean(AUTO_RECONNECT_PREF_STRING, DEFAULT_AUTO_RECONNECT);
        config.networkGraceSeconds = com.limelight.utils.NetworkGrace.parseSeconds(
                prefs.getString(NETWORK_GRACE_PREF_STRING, "0"));
        config.tellHostWireless = config.tellHostLinkSpeed = prefs.getBoolean(TELL_HOST_WIRELESS_PREF_STRING, true)
                && prefs.getBoolean(TELL_HOST_LINK_SPEED_PREF_STRING, true);
        config.fecChoice = FecChoice.fromPreference(prefs.getString(FEC_CHOICE_PREF_STRING, FecChoice.HOST.wireValue));
        config.linkProbe = prefs.getBoolean(LINK_PROBE_PREF_STRING, false);
        config.quickResume = prefs.getBoolean(QUICK_RESUME_PREF_STRING, false);
        config.adaptiveQualityMode = com.limelight.nvstream.AdaptiveQualityController.Mode.parse(
                prefs.getString("adaptive_quality_mode", "off"));
        config.qualityMinShortEdge = PreferenceResolution.listChoice(
                prefs.getString("adaptive_quality_min_resolution", "720"), QUALITY_MIN_SHORT_EDGES, 720);
        config.qualityMinFps = PreferenceResolution.listChoice(
                prefs.getString("adaptive_quality_min_fps", "30"), QUALITY_MIN_FPS, 30);
        config.thermalProtection = prefs.getBoolean(THERMAL_PROTECTION_PREF_STRING, false);
        PreferenceMigrations.dropAutoBitrateLimits(prefs);
        AutoBitrateChoice.migrate(context);
        config.applyLowerBitrate(AutoBitrateChoice.read(prefs));
        config.enablePerfOverlay = prefs.getBoolean(ENABLE_PERF_OVERLAY_STRING, DEFAULT_ENABLE_PERF_OVERLAY);
        config.overlayPosition = com.limelight.ui.OverlayPlacement.Corner.parse(
                prefs.getString(OVERLAY_POSITION_PREF_STRING, null)).key;
        config.compactOverlayDeviceInfo = prefs.getBoolean(COMPACT_OVERLAY_DEVICE_INFO_PREF_STRING, false);
        config.bindAllUsb = prefs.getBoolean(BIND_ALL_USB_STRING, DEFAULT_BIND_ALL_USB);
        config.mouseEmulation = prefs.getBoolean(MOUSE_EMULATION_STRING, DEFAULT_MOUSE_EMULATION);
        config.mouseEmulationFixedStep = prefs.getBoolean(MOUSE_EMULATION_FIXED_STEP_STRING, false);
        config.mouseEmulationSensitivity = Math.max(25, Math.min(200,
                prefs.getInt(MOUSE_EMULATION_SENSITIVITY_PREF_STRING, DEFAULT_MOUSE_EMULATION_SENSITIVITY)));
        config.disableRumble = FoldedChoice.VIBRATION.isOff(prefs);
        config.rumbleFeel = FoldedChoice.VIBRATION.valueWhileOn(prefs);
        config.backAsMeta = prefs.getBoolean(BACK_AS_META_PREF_STRING, false);
        config.backOpensMenu = prefs.getBoolean(BACK_OPENS_MENU_PREF_STRING, false);
        config.gamepadQuitCombo = prefs.getString(GAMEPAD_QUIT_COMBO_PREF_STRING, "off");
        config.backAsGuide = prefs.getBoolean(BACK_AS_GUIDE_PREF_STRING, false);
        config.guideMenuHoldMs = PreferenceResolution.guideMenuHoldMs(prefs.getString(GUIDE_MENU_HOLD_PREF_STRING, null));
        config.captureSystemKeys = prefs.getBoolean(CAPTURE_SYSTEM_KEYS_PREF_STRING, true);
        config.systemKeysFullscreenOnly = prefs.getBoolean(SYSTEM_KEYS_FULLSCREEN_ONLY_PREF_STRING, false);
        config.rightAltAsMeta = prefs.getBoolean(RIGHT_ALT_AS_META_PREF_STRING, false);
        config.keyboardFollowsTextFields = prefs.getBoolean(KEYBOARD_FOLLOWS_TEXT_FIELDS_PREF_STRING, true);
        config.clipboardSync = prefs.getBoolean(CLIPBOARD_SYNC_PREF_STRING, false);
        config.mouseNavButtons = prefs.getBoolean(MOUSE_NAV_BUTTONS_STRING, DEFAULT_MOUSE_NAV_BUTTONS);
        config.inputOnlyAnyHost = prefs.getBoolean(INPUT_ONLY_ANY_HOST_PREF_STRING, DEFAULT_INPUT_ONLY_ANY_HOST);
        config.unlockFps = prefs.getBoolean(UNLOCK_FPS_STRING, DEFAULT_UNLOCK_FPS);
        config.vibrateOsc = prefs.getBoolean(VIBRATE_OSC_PREF_STRING, DEFAULT_VIBRATE_OSC);
        config.vibrateFallbackToDevice = prefs.getBoolean(VIBRATE_FALLBACK_PREF_STRING, DEFAULT_VIBRATE_FALLBACK);
        config.vibrateFallbackToDeviceStrength = prefs.getInt(VIBRATE_FALLBACK_STRENGTH_PREF_STRING, DEFAULT_VIBRATE_FALLBACK_STRENGTH);
        config.flipFaceButtons = prefs.getBoolean(FLIP_FACE_BUTTONS_PREF_STRING, DEFAULT_FLIP_FACE_BUTTONS);
        config.touchMode = TouchMode.resolve(
                prefs.getString(TouchMode.PREF, null),
                prefs.getBoolean(TOUCHSCREEN_TRACKPAD_PREF_STRING, DEFAULT_TOUCHSCREEN_TRACKPAD),
                prefs.getBoolean(TouchMode.NATIVE_TOUCH_PREF, false));
        config.touchscreenTrackpad = TouchMode.legacyTrackpad(config.touchMode);
        config.trackpadSpeed = Math.max(25, Math.min(400, prefs.getInt(TRACKPAD_SPEED_PREF_STRING, DEFAULT_TRACKPAD_SPEED)));
        config.localCursorSpeed = Math.max(25, Math.min(400, prefs.getInt(LOCAL_CURSOR_SPEED_PREF_STRING, DEFAULT_TRACKPAD_SPEED)));
        config.trackpadAcceleration = !FoldedChoice.ACCELERATION.isOff(prefs);
        config.trackpadAccelerationCurve = FoldedChoice.ACCELERATION.valueWhileOn(prefs);
        config.trackpadMomentum = !FoldedChoice.MOMENTUM.isOff(prefs);
        config.trackpadMomentumGlide = FoldedChoice.MOMENTUM.valueWhileOn(prefs);
        config.trackpadArea = prefs.getString(TRACKPAD_AREA_PREF_STRING, TrackpadArea.FULL);
        config.threeFingerTap = prefs.getString(THREE_FINGER_TAP_PREF_STRING, FingerTapGesture.KEYBOARD);
        config.fourFingerTap = prefs.getString(FOUR_FINGER_TAP_PREF_STRING, FingerTapGesture.NONE);
        config.multiTouchFingerTaps = prefs.getBoolean(MULTI_TOUCH_FINGER_TAPS_PREF_STRING, false);
        config.fingerTapOnCancel = prefs.getBoolean(FINGER_TAP_ON_CANCEL_PREF_STRING, false);
        config.pcKeyboardLayout = prefs.getString(PC_KEYBOARD_LAYOUT_PREF_STRING, "auto");
        config.pcKeyboardSize = Math.max(30, Math.min(70, prefs.getInt(PC_KEYBOARD_SIZE_PREF_STRING, 50)));
        config.pcKeyboardOpacity = Math.max(30, Math.min(100, prefs.getInt(PC_KEYBOARD_OPACITY_PREF_STRING, 90)));
        config.mouseSpeed = Math.max(25, Math.min(400, prefs.getInt(MOUSE_SPEED_PREF_STRING, DEFAULT_MOUSE_SPEED)));
        config.trackpadSensitivityX = Math.max(-200, Math.min(200, prefs.getInt(TRACKPAD_SENSITIVITY_X_PREF_STRING, DEFAULT_TRACKPAD_SENSITIVITY)));
        config.trackpadSensitivityY = Math.max(-200, Math.min(200, prefs.getInt(TRACKPAD_SENSITIVITY_Y_PREF_STRING, DEFAULT_TRACKPAD_SENSITIVITY)));
        config.trackpadSwapAxis = prefs.getBoolean(TRACKPAD_SWAP_AXIS_PREF_STRING, false);
        config.nativeTouchpad = prefs.getBoolean(NATIVE_TOUCHPAD_PREF_STRING, false);
        config.trackpadScrollSpeed = Math.max(10, Math.min(300, prefs.getInt(TRACKPAD_SCROLL_SPEED_PREF_STRING, DEFAULT_TRACKPAD_SCROLL_SPEED)));
    }

    private static void readAdvancedAndDisplayPrefs(Context context, SharedPreferences prefs, PreferenceConfiguration config) {
        config.enableLatencyToast = prefs.getBoolean(LATENCY_TOAST_PREF_STRING, DEFAULT_LATENCY_TOAST);
        config.experimentalPacked444 = PreferenceConfiguration.sharpColor(prefs);
        config.frameWarp = prefs.getBoolean(FRAME_WARP_PREF_STRING, false);
        config.experimentalChipsetTuning = prefs.getBoolean(CHIPSET_TUNING_PREF_STRING, false);
        config.tvDisplayCompatibility = prefs.getBoolean(TV_DISPLAY_COMPATIBILITY_PREF_STRING,
                context.getResources().getBoolean(com.limelight.R.bool.default_tv_display_compatibility));
        config.absoluteMouseMode = prefs.getBoolean(ABSOLUTE_MOUSE_MODE_PREF_STRING, DEFAULT_ABSOLUTE_MOUSE_MODE);
        config.enableAudioFx = prefs.getBoolean(ENABLE_AUDIO_FX_PREF_STRING, DEFAULT_ENABLE_AUDIO_FX);
        config.audioPassthrough = prefs.getBoolean(AUDIO_PASSTHROUGH_PREF_STRING, false);
        config.muteAudio = prefs.getBoolean(MUTE_AUDIO_PREF_STRING, false);
        config.dualScreen = prefs.getBoolean(DUAL_SCREEN_PREF_STRING, false);
        config.secondScreenResolution = prefs.getString(SECOND_SCREEN_RESOLUTION_PREF_STRING, SECOND_SCREEN_NATIVE);
        config.reduceRefreshRate = prefs.getBoolean(REDUCE_REFRESH_RATE_PREF_STRING, DEFAULT_REDUCE_REFRESH_RATE);
        config.fullRange = prefs.getBoolean(FULL_RANGE_PREF_STRING, DEFAULT_FULL_RANGE);
        config.gamepadTouchpadAsMouse = prefs.getBoolean(GAMEPAD_TOUCHPAD_AS_MOUSE_PREF_STRING, DEFAULT_GAMEPAD_TOUCHPAD_AS_MOUSE);
        config.gamepadMotionSensors = prefs.getBoolean(GAMEPAD_MOTION_SENSORS_PREF_STRING, DEFAULT_GAMEPAD_MOTION_SENSORS);
        config.gamepadMotionSensorsFallbackToDevice = prefs.getBoolean(GAMEPAD_MOTION_FALLBACK_PREF_STRING, DEFAULT_GAMEPAD_MOTION_FALLBACK);
        config.codecLowLatency = prefs.getBoolean(CODEC_LOW_LATENCY_PREF_STRING, true);
        config.vendorLowLatency = prefs.getBoolean(VENDOR_LOW_LATENCY_PREF_STRING, true);
        config.hevcLowLatencyMode = PreferenceConfiguration.readHevcLowLatencyMode(prefs);
        config.disableHevcLowLatency = config.hevcLowLatencyMode == HEVC_LOW_LATENCY_OFF;
        config.codecPerformance = prefs.getBoolean(CODEC_PERFORMANCE_PREF_STRING, true);
        config.phonePerformanceHints = prefs.getBoolean(PHONE_PERFORMANCE_HINTS_PREF_STRING, true);
        config.gpuMaxClocks = prefs.getBoolean(GPU_MAX_CLOCKS_PREF_STRING, DEFAULT_GPU_MAX_CLOCKS);
        if (prefs.contains("checkbox_pyrowave_front_buffer")) {
            prefs.edit().remove("checkbox_pyrowave_front_buffer").apply();
        }
        config.dropLateFrames = prefs.getBoolean(DROP_LATE_FRAMES_PREF_STRING, true);
        config.displayQueueRecovery = prefs.getBoolean(DISPLAY_QUEUE_RECOVERY_PREF_STRING, true);
        config.fractionalVsync = prefs.getBoolean(FRACTIONAL_VSYNC_PREF_STRING, DEFAULT_FRACTIONAL_VSYNC);
        config.useTextureView = prefs.getBoolean(TEXTURE_VIEW_PREF_STRING, false) && !config.enableHdr && !config.enableSdr10Bit &&
                config.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        config.unbatchedInput = prefs.getBoolean(UNBATCHED_INPUT_PREF_STRING, true);
        config.networkPriority = prefs.getBoolean(NETWORK_PRIORITY_PREF_STRING, true);
        config.submitHint = prefs.getBoolean(SUBMIT_HINT_PREF_STRING, true);
        config.coveringDecoderSize = prefs.getBoolean(COVERING_DECODER_SIZE_PREF_STRING, false);
        config.pyroWavePreacquire = prefs.getBoolean(PYROWAVE_PREACQUIRE_PREF_STRING, false);
        config.receiveDecodePriority = prefs.getBoolean(RECEIVE_DECODE_PRIORITY_PREF_STRING, true);
        config.codecDirect = prefs.getBoolean(CODEC_DIRECT_PREF_STRING,
                context.getResources().getBoolean(com.limelight.R.bool.default_codec_direct));
        config.clientRoaming = prefs.getBoolean(CLIENT_ROAMING_PREF_STRING, true);
        config.roamingMetered = prefs.getBoolean(ROAMING_METERED_PREF_STRING, false);
        config.receiveCoalescing = prefs.getBoolean(RECEIVE_COALESCING_PREF_STRING, true);
        config.jumboFrames = prefs.getBoolean(JUMBO_FRAMES_PREF_STRING, false);
        config.decodeQueueFrames = PreferenceResolution.decodeQueueFrames(
                prefs.getString(DECODE_QUEUE_FRAMES_PREF_STRING, String.valueOf(DEFAULT_DECODE_QUEUE_FRAMES)));
        config.internetPlay = prefs.getBoolean(INTERNET_PLAY_PREF_STRING, true);
        config.virtualDisplay = prefs.getBoolean(VIRTUAL_DISPLAY_PREF_STRING, false);
        config.virtualDisplayScale = Math.max(50, Math.min(200, prefs.getInt(VIRTUAL_DISPLAY_SCALE_PREF_STRING, 100)));
        config.virtualDisplayDpi = PreferenceResolution.virtualDisplayDpi(prefs.getString(VIRTUAL_DISPLAY_DPI_PREF_STRING, "0"));
        config.enableYuv444 = config.experimentalPacked444;
        config.vrr = prefs.getBoolean(VRR_PREF_STRING, false)
                && com.limelight.binding.video.DisplayFrameRatePolicy.vrrOffered(context);
        config.upscalingMode = UpscalingPolicy.Mode.fromPreference(prefs.getString(UPSCALING_PREF_STRING, "off"));
        config.upscalingSharpness = Math.max(0, Math.min(100, prefs.getInt(UPSCALING_SHARPNESS_PREF_STRING, 50)));
        config.vulkanRender = prefs.getBoolean(VULKAN_RENDER_PREF_STRING, false);
        config.debugHdrHlgOnly = prefs.getBoolean(DEBUG_HDR_HLG_ONLY_PREF_STRING, false);
        if (config.vrr) {
            config.framePacing = FRAME_PACING_MIN_LATENCY;
            config.reduceRefreshRate = false;
        }
        config.realVrr = prefs.getBoolean(REAL_VRR_PREF_STRING, true);
        config.gridHostFrameTimes = prefs.getBoolean(HOST_FRAME_TIMES_PREF_STRING, false);
        config.arrFixedTiming = prefs.getBoolean(ARR_FIXED_TIMING_PREF_STRING, false);
        config.resolveHostFrameTimes();
        config.contentRateFrameRate = com.limelight.binding.video.DisplayFrameRatePolicy.tracksContentRate(
                prefs.getBoolean(CONTENT_RATE_FRAME_RATE_PREF_STRING, false), config.reduceRefreshRate,
                config.vrr, config.useTextureView);
        if (config.hostFrameTimes) {
            config.framePacing = FRAME_PACING_MIN_LATENCY;
        }
    }
}
