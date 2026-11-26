package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;

import com.limelight.R;
import com.limelight.binding.audio.MicProcessing;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.DisplayModeChoice;
import com.limelight.binding.video.FramePacingPolicy;
import com.limelight.binding.video.VideoRendererChoice;

/**
 * Which settings apply given the others, so the Settings screens can hide the ones that would do
 * nothing (a hidden row keeps its value and comes back in its place). The stream configuration enforces the same overrides when it reads the settings
 * (adaptive refresh and host frame times both replace frame pacing); these only keep the screens honest.
 */
final class SettingRules {
    static final String DROP_LATE_FRAMES = "checkbox_drop_late_frames";
    static final String FRACTIONAL_VSYNC = PreferenceConfiguration.FRACTIONAL_VSYNC_PREF_STRING;
    static final String FRAME_PACING = PreferenceConfiguration.FRAME_PACING_PREF_STRING;
    static final String HOST_FRAME_TIMES = PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING;
    static final String ADAPTIVE_QUALITY = "adaptive_quality_mode";
    static final String ADAPTIVE_QUALITY_MIN_RESOLUTION = "adaptive_quality_min_resolution";
    static final String ADAPTIVE_QUALITY_MIN_FPS = "adaptive_quality_min_fps";

    private SettingRules() { }

    /**
     * Whether Variable refresh rate is on: chosen, and offered on this device (vrrOffered). A choice
     * saved where it isn't offered is kept, but reads as off there.
     */
    static boolean vrrOn(Context context, SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false) &&
                DisplayFrameRatePolicy.vrrOffered(context);
    }

    /**
     * Frame pacing decides how frames are shown unless adaptive refresh takes over; the PC's frame times and
     * Balanced's newest-frame choice are among its choices (FramePacingChoice).
     */
    static boolean framePacingApplies(Context context, SharedPreferences prefs) {
        return !vrrOn(context, prefs);
    }

    /**
     * Render through Vulkan or Frame Warp, whichever puts frames through the Vulkan renderer (VideoRendererChoice),
     * as the title of its setting; 0 when neither does. Before Android 10 neither runs.
     */
    static int vulkanPathTitle(SharedPreferences prefs) {
        if (android.os.Build.VERSION.SDK_INT < VideoRendererChoice.MIN_SDK) return 0;
        if (prefs.getBoolean(PreferenceConfiguration.VULKAN_RENDER_PREF_STRING, false)) return R.string.title_vulkan_render;
        if (prefs.getBoolean(PreferenceConfiguration.FRAME_WARP_PREF_STRING, false)) {
            return R.string.title_frame_warp;
        }
        return 0;
    }

    /** FSR or SGSR enlarges the picture: not under Vulkan, HDR or PyroWave (UpscalingPolicy). */
    private static boolean upscalerOn(SharedPreferences prefs) {
        return vulkanPathTitle(prefs) == 0 && !"off".equals(prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off")) &&
                !prefs.getBoolean("checkbox_enable_hdr", false) && !pyroWaveChosen(prefs);
    }

    /** The codec's name in tags; it reads the same in every language. */
    private static final String PYROWAVE = "PyroWave";

    /** The switches whose summary says why they do nothing ({@link #unusedReason}); they have no other summary. */
    static final java.util.Set<String> TAGGED_SWITCHES = new java.util.HashSet<>(java.util.Arrays.asList(
            "checkbox_texture_view", PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING,
            "checkbox_onscreen_own_player", "checkbox_audio_passthrough", FRACTIONAL_VSYNC));

    /**
     * Why a row does nothing with the other settings as they are, or null. A setting that works only in some setups
     * says so where it does nothing (client/AGENTS.md): a switch in its summary, a list in its value line.
     */
    static String unusedReason(Context context, String key, SharedPreferences prefs) {
        switch (key) {
            case "checkbox_texture_view":
                // PreferenceConfiguration.readPreferences keeps the stream's own surface under these.
                if (prefs.getBoolean("checkbox_enable_hdr", false)) return unusedWith(context, R.string.title_enable_hdr);
                if (prefs.getBoolean("checkbox_enable_sdr_10bit", false)) return unusedWith(context, R.string.title_enable_sdr10);
                return pyroWaveChosen(prefs) ? context.getString(R.string.tag_unused_with, PYROWAVE) : null;
            case PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING: {
                // Heat acts through a controller that follows it (GameAdaptation.heatEasesLoad) or eases PyroWave's
                // GPU boost (MediaCodecDecoderRenderer). A PC's own choice in the stream menu can still turn one on.
                String lower = AutoBitrateChoice.read(prefs);
                boolean quality = !"off".equals(prefs.getString("adaptive_quality_mode", "off"));
                boolean boost = prefs.getBoolean(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING, false) && pyroWaveChosen(prefs);
                if (quality || boost) return null;
                // The frame-loss controller follows no heat; it replaces the busy one where the PC takes rate
                // changes (GameAdaptation.startStreamBitrate), and elsewhere the busy one runs instead.
                if (AutoBitrateChoice.SOONER.equals(lower)) return context.getString(R.string.tag_thermal_frames_lost);
                return context.getString(R.string.tag_unused_without_any, context.getString(R.string.title_lower_bitrate),
                        context.getString(R.string.title_adaptive_quality_setting), context.getString(R.string.title_gpu_max_clocks));
            }
            case HdrFormatChoice.PREF_KEY:
                return prefs.getBoolean("checkbox_enable_hdr", false) ? null :
                        context.getString(R.string.tag_unused_without, context.getString(R.string.title_enable_hdr));
            case "adaptive_quality_mode":
                // GameAdaptation.supportsAdaptiveQuality: PyroWave streams have no automatic quality.
                return pyroWaveChosen(prefs) && !"off".equals(prefs.getString(key, "off")) ?
                        context.getString(R.string.tag_unused_with, PYROWAVE) : null;
            case "checkbox_onscreen_own_player":
                // ControllerHandler.onscreenOwnsPlayerOne: L3 and R3 alone only join a pad.
                return prefs.getBoolean("checkbox_only_show_L3R3", false) ? unusedWith(context, R.string.title_only_l3r3) : null;
            case "checkbox_audio_passthrough":
                // NvConnection.audioPassthroughCodecs: muted audio asks for nothing to pass through.
                return prefs.getBoolean("checkbox_mute_audio", false) ? unusedWith(context, R.string.title_checkbox_mute_audio) : null;
            case FRACTIONAL_VSYNC:
                // Adaptive refresh, the PC's frame times and the pacings that place frames themselves hold
                // nothing (fractionalVsyncApplies), nor does a screen that refreshes when a frame comes
                // (MediaCodecDecoderRenderer.heldRefreshes); the row names what takes over.
                if (vrrOn(context, prefs)) return unusedWith(context, R.string.title_vrr);
                if (DisplayFrameRatePolicy.adaptiveInUse(DisplayFrameRatePolicy.arrScreen(context), false,
                        prefs.getBoolean(PreferenceConfiguration.ARR_FIXED_TIMING_PREF_STRING, false))) {
                    return context.getString(R.string.tag_unused_adaptive_screen);
                }
                // The Vulkan renderer presents each frame as it comes (MediaCodecDecoderRenderer.handOutputToVulkan),
                // and Frame Warp shows a held frame at the next refresh once the PC's model is locked
                // (GlesUpscaler.drawFrame): neither keeps the turn.
                int vulkan = vulkanPathTitle(prefs);
                if (vulkan != 0) return unusedWith(context, vulkan);
                if (!fractionalVsyncApplies(context, prefs)) {
                    return context.getString(R.string.tag_unused_with, StreamSettings.entryFor(context,
                            R.array.frame_pacing_choice_names, R.array.frame_pacing_choice_values, FramePacingChoice.read(prefs)));
                }
                return fractionalVsyncRefreshReason(context, prefs);
            default:
                return null;
        }
    }

    private static String unusedWith(Context context, int title) {
        return context.getString(R.string.tag_unused_with, context.getString(title));
    }

    /**
     * The tag for a Frame pacing choice that its renderer ignores, or null. PyroWave's presenter takes the pacing
     * under the PC's frame times, which those make Lowest latency (FramePacingPolicy.pyroWaveMode,
     * PreferenceConfiguration.readPreferences). The Vulkan renderer takes the pacing but neither the PC's frame
     * times nor Balanced's newest-frame choice (MediaCodecDecoderRenderer.handOutputToVulkan).
     */
    static String framePacingUnused(Context context, SharedPreferences prefs) {
        String choice = FramePacingChoice.read(prefs);
        if (FramePacingChoice.HOST_TIMES.equals(choice) && pyroWaveChosen(prefs)) {
            return context.getString(R.string.tag_unused_with, PYROWAVE);
        }
        int vulkan = vulkanPathTitle(prefs);
        if (vulkan == 0) return null;
        if (FramePacingChoice.HOST_TIMES.equals(choice)) {
            return context.getString(R.string.tag_unused_with, context.getString(vulkan));
        }
        if (FramePacingChoice.BALANCED_EVERY_FRAME.equals(choice)) {
            return context.getString(R.string.tag_every_frame_with, context.getString(vulkan));
        }
        return null;
    }

    /**
     * Fractional V-Sync holds Lowest latency frames for their turn. Balanced shows a stream at a half
     * to a quarter of the refresh from the vsync callback, at that cadence already, where no phase
     * report is sent (FramePacingPolicy.balancedPacesOnVsync); Smoothest and FPS-capped pacing hold
     * an early frame for its turn without it (MediaCodecDecoderRenderer.smoothDisplayNs), and the PC's
     * frame times and adaptive refresh show frames without it.
     */
    static boolean fractionalVsyncApplies(Context context, SharedPreferences prefs) {
        return framePacingApplies(context, prefs) && FramePacingChoice.LATENCY.equals(FramePacingChoice.read(prefs));
    }

    /**
     * Why fractional V-Sync holds nothing at the refresh the stream will run at, or null: it has to be
     * 2 to 4 times the frame rate (FramePacingPolicy.refreshesPerFrame). The stream switches the screen
     * to the mode DisplayModeChoice picks (GameVideoOutput.prepareDisplayForRendering), so the tag names
     * that mode's refresh. Null where the settings can't know it: before Android 6, with "Reduce refresh
     * rate" (the stream asks Android for the frame rate, or the game's content rate, and Android picks
     * the refresh: GameVideoOutput.baseSurfaceFrameRate), and where no mode reaches the frame rate (the
     * same). VRR, adaptive refresh and the pacings that take over return before this.
     */
    static String fractionalVsyncRefreshReason(Context context, SharedPreferences prefs) {
        if (prefs.getBoolean(PreferenceConfiguration.REDUCE_REFRESH_RATE_PREF_STRING, false)) return null;
        android.view.Display display = PreferenceConfiguration.defaultDisplay(context);
        if (display == null) return null;
        int fps, width, height;
        try {
            fps = Integer.parseInt(prefs.getString(PreferenceConfiguration.FPS_PREF_STRING, PreferenceConfiguration.DEFAULT_FPS));
            String resolution = PreferenceConfiguration.streamResolution(context, prefs);
            width = PreferenceConfiguration.getWidthFromResolutionString(resolution);
            height = PreferenceConfiguration.getHeightFromResolutionString(resolution);
        } catch (RuntimeException e) {
            return null;
        }
        if (fps <= 0) return null;
        android.view.Display.Mode current = display.getMode();
        android.view.Display.Mode[] supported = display.getSupportedModes();
        DisplayModeChoice.Mode[] candidates = new DisplayModeChoice.Mode[supported.length];
        for (int i = 0; i < supported.length; i++) {
            candidates[i] = new DisplayModeChoice.Mode(supported[i].getPhysicalWidth(), supported[i].getPhysicalHeight(),
                    supported[i].getRefreshRate());
        }
        int chosen = DisplayModeChoice.choose(new DisplayModeChoice.Mode(current.getPhysicalWidth(), current.getPhysicalHeight(),
                        current.getRefreshRate()), candidates, width, height, fps, false,
                PreferenceConfiguration.isNativeResolution(width, height), false);
        float refreshHz = chosen < 0 ? current.getRefreshRate() : supported[chosen].getRefreshRate();
        if (!(refreshHz >= fps)) return null;
        if (FramePacingPolicy.refreshesPerFrame(refreshHz, fps, false, true) > 1) return null;
        boolean switched = Math.abs(refreshHz - current.getRefreshRate()) > 0.01f;
        return context.getString(switched ? R.string.tag_unused_refresh_switched : R.string.tag_unused_refresh,
                Math.round(refreshHz));
    }

    /**
     * The Vulkan renderer (explicit Vulkan or Frame Warp) scales the picture itself and replaces
     * the upscaler, so upscaling rows do nothing while either path is active on Android 10+.
     */
    static boolean upscalingApplies(SharedPreferences prefs) {
        return vulkanPathTitle(prefs) == 0;
    }

    /** Only FSR and SGSR sharpen; Off and Smooth have nothing to adjust. */
    static boolean upscalingUsesSharpness(SharedPreferences prefs) {
        if (!upscalingApplies(prefs)) return false;
        String mode = prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off");
        return "fsr1".equals(mode) || "sgsr1".equals(mode);
    }

    /** The lowest resolution and frame rate only matter while automatic quality runs. */
    static boolean qualityLimitsApply(SharedPreferences prefs) {
        return !"off".equals(prefs.getString(ADAPTIVE_QUALITY, "off"));
    }

    /** Passthrough needs Android 10 or later to tell which formats the audio output plays as is. */
    static boolean audioPassthroughApplies(int sdk) {
        return sdk >= android.os.Build.VERSION_CODES.Q;
    }

    /**
     * Whether this device has mobile data, asked per device at runtime. Rows about mobile data show only there,
     * or where a value set earlier still acts, so it can be seen and undone.
     */
    static boolean hasMobileData(Context context) {
        return context.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEPHONY);
    }

    /** Bitrate on mobile data shows where there is mobile data, or while a cap set earlier still acts. */
    static boolean mobileBitrateShown(Context context, SharedPreferences prefs) {
        return hasMobileData(context)
                || !"0".equals(prefs.getString(PreferenceConfiguration.MOBILE_BITRATE_PREF_STRING, "0"));
    }

    /** The bitrate slider is the player's while Automatic bitrate is off. */
    static boolean bitrateShown(SharedPreferences prefs) {
        return PreferenceConfiguration.isBitrateUserSet(prefs);
    }

    private static boolean pyroWaveChosen(SharedPreferences prefs) {
        return "forcepyrowave".equals(prefs.getString("video_format", "auto"));
    }

    /**
     * Sharper colored text works with every codec: with PyroWave it asks for PyroWave's own 4:4:4 where the PC and
     * this device offer it, beside the native and packed 4:4:4 the other codecs use.
     */
    static boolean sharpColorApplies(SharedPreferences prefs) {
        return true;
    }

    /** Less color banding is 10-bit SDR: not with PyroWave, and HDR is 10-bit already. */
    static boolean sdr10Applies(SharedPreferences prefs) {
        return !pyroWaveChosen(prefs) && !prefs.getBoolean("checkbox_enable_hdr", false);
    }

    /** A lower refresh rate, and with it the game's rate (LowerRefreshChoice), is chosen while adaptive refresh is off. */
    static boolean reduceRefreshRateApplies(Context context, SharedPreferences prefs) {
        return !vrrOn(context, prefs);
    }

    /** "Fixed timing on adaptive screens" shows on an ARR screen while Variable refresh rate is off. */
    static boolean arrFixedTimingShown(Context context, SharedPreferences prefs) {
        return !vrrOn(context, prefs) && DisplayFrameRatePolicy.arrScreen(context);
    }

    /**
     * Display queue recovery works on a fixed refresh with Lowest latency or Balanced pacing (DisplayQueueWatch).
     * With the PC's frame times every frame is scheduled and the watch never runs (MediaCodecDecoderRenderer), as
     * frame pacing itself does not apply then.
     */
    static boolean displayQueueRecoveryApplies(Context context, SharedPreferences prefs) {
        String pacing = prefs.getString(FRAME_PACING, PreferenceConfiguration.DEFAULT_FRAME_PACING);
        return framePacingApplies(context, prefs) && !prefs.getBoolean(HOST_FRAME_TIMES, false) &&
                ("latency".equals(pacing) || "balanced".equals(pacing));
    }

    /**
     * Why the display queue take-back does nothing where it shows, or null when it works: a screen timed as
     * adaptive (DisplayFrameRatePolicy.adaptiveInUse), the Vulkan renderer, or an upscaler, none of which reports
     * when frames are shown the way the watch needs (MediaCodecDecoderRenderer.displayQueueDropDue).
     */
    static String displayQueueRecoveryUnused(Context context, SharedPreferences prefs) {
        if (DisplayFrameRatePolicy.adaptiveInUse(DisplayFrameRatePolicy.arrScreen(context), false,
                prefs.getBoolean(PreferenceConfiguration.ARR_FIXED_TIMING_PREF_STRING, false))) {
            return context.getString(R.string.tag_unused_adaptive_screen);
        }
        int vulkan = vulkanPathTitle(prefs);
        if (vulkan != 0) return context.getString(R.string.tag_unused_with, context.getString(vulkan));
        if (upscalerOn(prefs)) {
            return context.getString(R.string.tag_unused_with, context.getString(R.string.title_upscaling));
        }
        return null;
    }

    /** Video position moves a picture that leaves room: Fit, Safe fit or 1:1 pixels. */
    static boolean videoAlignmentApplies(SharedPreferences prefs) {
        int mode = com.limelight.ui.VideoScaling.resolve(prefs.getString(com.limelight.ui.VideoScaling.PREF, null),
                prefs.getBoolean(com.limelight.ui.VideoScaling.STRETCH_PREF, false));
        return !com.limelight.ui.VideoScaling.coversArea(mode);
    }

    /** The PC renders at a scale of the stream only on its virtual display: a Display arrangement that asks for one. */
    static boolean virtualDisplayScaleApplies(SharedPreferences prefs) {
        return DisplayArrangementChoice.virtualDisplay(DisplayArrangementChoice.readMain(prefs));
    }

    /** Controllers is not Off: the controller rows apply. */
    static boolean controllersOn(SharedPreferences prefs) {
        return ControllersChoice.read(prefs) != ControllersChoice.OFF;
    }

    /** The controller mouse's own rows, while controller mouse mode is on. */
    static boolean controllerMouseApplies(SharedPreferences prefs) {
        return controllersOn(prefs) && prefs.getBoolean("checkbox_mouse_emulation", true);
    }

    /** The on-screen controls' rows, while they are shown. */
    static boolean onScreenControlsOn(SharedPreferences prefs) {
        return prefs.getBoolean("checkbox_show_onscreen_controls", false);
    }

    /** The microphone's rows, while it is sent. */
    static boolean micOn(SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.ENABLE_MIC_PREF_STRING, false);
    }

    /** Where the overlay sits, while it is shown. */
    static boolean overlayOn(SharedPreferences prefs) {
        return OverlayChoice.read(prefs) != 0;
    }

    /** Rows hidden while Controllers is Off (Controls › Controllers and the stream menu's controller rows). */
    static final String[] CONTROLLER_ROWS = {"checkbox_flip_face_buttons", "list_gamepad_quit_combo", "seekbar_deadzone",
            "seekbar_trigger_deadzone", "checkbox_gamepad_motion_sensors", "gyro_mapping_profiles", "list_rumble_feel", "checkbox_mouse_emulation",
            "checkbox_gamepad_touchpad_as_mouse", "list_guide_menu_hold", "back_button",
            "list_stream_menu_button_y", "list_stream_menu_button_x", "list_stream_menu_button_lb", "list_stream_menu_button_rb"};
    /** Rows hidden while the on-screen controls are not shown. */
    static final String[] ON_SCREEN_ROWS = {PreferenceConfiguration.OSC_STICK_MODE_PREF_STRING,
            PreferenceConfiguration.OSC_STICK_DEADZONE_PREF_STRING, "checkbox_vibrate_osc", PreferenceConfiguration.OSC_AUTO_HIDE_PREF_STRING,
            "checkbox_only_show_L3R3",
            "checkbox_onscreen_own_player", "checkbox_show_guide_button", "checkbox_show_touchpad_button",
            PreferenceConfiguration.OSC_STEAM_CONTROLS_PREF_STRING, "seekbar_osc_opacity", "reset_osc"};
    /** Rows hidden while the microphone is not sent. */
    static final String[] MIC_ROWS = {"checkbox_mic_button", PreferenceConfiguration.MIC_ECHO_PREF_STRING,
            PreferenceConfiguration.MIC_NOISE_PREF_STRING, PreferenceConfiguration.MIC_AUTO_GAIN_PREF_STRING, "seekbar_mic_gain"};

    /** "Show debug settings": the Debug section is listed with it on, or while one of its settings is changed. */
    static final String SHOW_DEBUG = "checkbox_show_debug_settings";

    /** Whether the list of sections shows Debug: asked for, or one of its settings is not at its default. */
    static boolean debugShown(Context context, SharedPreferences prefs) {
        if (prefs.getBoolean(SHOW_DEBUG, false)) return true;
        for (String key : xml(context).debugKeys) {
            if (changed(context, key, prefs)) return true;
        }
        return false;
    }

    static final String TRACKPAD_SPEED = "seekbar_trackpad_speed";
    static final String LOCAL_CURSOR_SPEED = "seekbar_local_cursor_speed";
    static final String TRACKPAD_AREA = "list_trackpad_area";
    /** Acceleration strength and Glide length: shown while their switch is on, and left out with it (whenApplies). */
    static final String ACCELERATION_CURVE = "list_trackpad_acceleration_curve";
    static final String MOMENTUM_GLIDE = "list_trackpad_momentum_glide";
    static final String FINGER_TAPS = "checkbox_multi_touch_finger_taps";
    static final String FINGER_TAP_ON_CANCEL = "checkbox_finger_tap_on_cancel";
    static final String TRACKPAD_SENSITIVITY_X = "seekbar_trackpad_sensitivity_x";
    static final String TRACKPAD_SENSITIVITY_Y = "seekbar_trackpad_sensitivity_y";
    static final String TRACKPAD_SWAP_AXIS = "checkbox_trackpad_swap_axis";
    static final String NATIVE_TOUCHPAD = "checkbox_native_touchpad";
    static final String MOUSE_SPEED = "seekbar_mouse_speed";
    static final String MOUSE_NAV_BUTTONS = "checkbox_mouse_nav_buttons";
    static final String BACK_AS_META = "checkbox_back_as_meta";
    static final String COVERING_DECODER_SIZE = "checkbox_covering_decoder_size";
    static final String CHIPSET_TUNING = "checkbox_experimental_chipset_tuning";
    static final String TV_DISPLAY_COMPATIBILITY = PreferenceConfiguration.TV_DISPLAY_COMPATIBILITY_PREF_STRING;
    static final String IGDB_SECRET = "game_info_igdb_client_secret";
    static final String PYROWAVE_PREACQUIRE = "checkbox_pyrowave_preacquire";
    static final String OVERLAY_POSITION = "performance_overlay_position";
    static final String VIBRATE_STRENGTH = "seekbar_vibrate_fallback_strength";
    static final String SCREEN2_RESOLUTION = "list_resolution_screen2";

    /**
     * The rows shown only while some stream on this device can use them, decided by what no stream can change: this
     * device has what they act on, or the stored main setting they sit under is on (and no stream menu, external
     * display, second screen or PC's, game's or profile's settings can make them act without it) (whenApplies). One
     * whose stored value is not its default stays, saying why it does nothing now (whenReason); search finds the others
     * with that reason, and opening the result shows the row. Settings never reads a PC's, game's or profile's stored
     * values to decide what it shows: a row they or the stream menu can make act is in {@link #NOTE_ROWS}, and Thermal
     * protection, which a PC's or one stream's Automatic bitrate lets act, is always shown with its own tag.
     */
    static final java.util.List<String> WHEN_ROWS = java.util.Arrays.asList(
            VIBRATE_STRENGTH, MOUSE_SPEED, MOUSE_NAV_BUTTONS,
            TRACKPAD_SPEED, LOCAL_CURSOR_SPEED, ACCELERATION_CURVE,
            MOMENTUM_GLIDE, TRACKPAD_AREA, FINGER_TAPS, FINGER_TAP_ON_CANCEL, TRACKPAD_SENSITIVITY_X, TRACKPAD_SENSITIVITY_Y, TRACKPAD_SWAP_AXIS, NATIVE_TOUCHPAD,
            BACK_AS_META, OVERLAY_POSITION, SCREEN2_RESOLUTION, COVERING_DECODER_SIZE, CHIPSET_TUNING, TV_DISPLAY_COMPATIBILITY, IGDB_SECRET);

    /**
     * Rows always shown, because the stream menu or a PC's, game's or profile's settings can make them act: Automatic
     * quality's minimums (the stream menu's choice for a PC or one stream), the sharpness (their upscaler) and PyroWave's
     * early screen image (their codec). While the main settings leave them nothing to do they say why (whenReason).
     */
    static final java.util.List<String> NOTE_ROWS = java.util.Arrays.asList(
            ADAPTIVE_QUALITY_MIN_RESOLUTION, ADAPTIVE_QUALITY_MIN_FPS, PreferenceConfiguration.UPSCALING_SHARPNESS_PREF_STRING,
            PreferenceConfiguration.COMPACT_OVERLAY_DEVICE_INFO_PREF_STRING, PYROWAVE_PREACQUIRE);

    /** The IGDB client ID, kept apart from the settings (GameInfo.keyPrefs). */
    private static boolean igdbClientIdSet(Context context) {
        return !com.limelight.console.GameInfo.keyPrefs(context)
                .getString(com.limelight.console.GameInfo.KEY_IGDB_ID, "").trim().isEmpty();
    }

    /**
     * Whether a row of {@link #WHEN_ROWS} can do something now, or a row of {@link #NOTE_ROWS} with the main settings
     * alone; true for every other row.
     */
    static boolean whenApplies(Context context, String key, SharedPreferences prefs, SettingDevices devices) {
        switch (key) {
            case ADAPTIVE_QUALITY_MIN_RESOLUTION:
            case ADAPTIVE_QUALITY_MIN_FPS:
                return qualityLimitsApply(prefs);
            case PreferenceConfiguration.UPSCALING_SHARPNESS_PREF_STRING:
                return upscalingUsesSharpness(prefs);
            case VIBRATE_STRENGTH:
                return prefs.getBoolean("checkbox_vibrate_fallback", false);
            case MOUSE_SPEED:
                // A physical touchpad's pointer moves at the mouse speed too (TrackpadContext).
                return devices.mouse() || devices.touchpad();
            case MOUSE_NAV_BUTTONS:
                return devices.mouse();
            case TRACKPAD_SPEED:
            case LOCAL_CURSOR_SPEED:
            case ACCELERATION_CURVE:
            case MOMENTUM_GLIDE:
            case TRACKPAD_AREA:
            case FINGER_TAPS:
            case FINGER_TAP_ON_CANCEL:
                // Not by the stored touch mode: an external display makes the screen a trackpad whatever it is
                // (GameVideoOutput), the stream menu shows the on-screen controller (whose modern style has a pointer
                // touchpad) without saving it, and the second screen picks its touch mode for its own stream. Only a
                // device without a touch screen has nothing these act on.
                return devices.touchscreen();
            case TRACKPAD_SENSITIVITY_X:
            case TRACKPAD_SENSITIVITY_Y:
            case TRACKPAD_SWAP_AXIS:
            case NATIVE_TOUCHPAD:
                return devices.touchpad();
            case BACK_AS_META:
                return devices.keyboard();
            case OVERLAY_POSITION:
            case PreferenceConfiguration.COMPACT_OVERLAY_DEVICE_INFO_PREF_STRING:
                // The second screen's stream menu shows its overlay without saving it (DualScreen.saveSettings).
                return overlayOn(prefs) || prefs.getBoolean("checkbox_dual_screen", false);
            case SCREEN2_RESOLUTION:
                return prefs.getBoolean("checkbox_dual_screen", false);
            case COVERING_DECODER_SIZE:
                // GameVideoOutput.coveringStreamSize: a foldable's panels, or the turned size with Rotate with device.
                return devices.folds() || com.limelight.ui.StreamOrientation.fromValue(
                        prefs.getString(com.limelight.ui.StreamOrientation.PREF, null)) == com.limelight.ui.StreamOrientation.ROTATE;
            case CHIPSET_TUNING:
                return devices.tunedChip();
            case TV_DISPLAY_COMPATIBILITY:
                // For TV firmware, and listed elsewhere with Show debug settings to try it; the stream still draws
                // the dot from a stored value on any device.
                return devices.television() || prefs.getBoolean(SHOW_DEBUG, false);
            case IGDB_SECRET:
                return igdbClientIdSet(context);
            case PYROWAVE_PREACQUIRE:
                return pyroWaveChosen(prefs);
            default:
                return true;
        }
    }

    /** Why a row of {@link #WHEN_ROWS} or {@link #NOTE_ROWS} does nothing now, in one short line; null while it applies. */
    static String whenReason(Context context, String key, SharedPreferences prefs, SettingDevices devices) {
        if (whenApplies(context, key, prefs, devices)) return null;
        switch (key) {
            case ADAPTIVE_QUALITY_MIN_RESOLUTION:
            case ADAPTIVE_QUALITY_MIN_FPS:
                return context.getString(R.string.tag_unused_without, context.getString(R.string.title_adaptive_quality_setting));
            case PreferenceConfiguration.UPSCALING_SHARPNESS_PREF_STRING: {
                int vulkan = vulkanPathTitle(prefs);
                if (!upscalingApplies(prefs)) return unusedWith(context, vulkan != 0 ? vulkan : R.string.title_vulkan_render);
                String mode = prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off");
                if ("off".equals(mode)) return context.getString(R.string.tag_unused_without, context.getString(R.string.title_upscaling));
                return context.getString(R.string.tag_unused_with,
                        StreamSettings.entryFor(context, R.array.upscaling_names, R.array.upscaling_values, mode));
            }
            case VIBRATE_STRENGTH:
                return context.getString(R.string.tag_unused_without, context.getString(R.string.title_checkbox_vibrate_fallback));
            case MOUSE_SPEED:
                return context.getString(R.string.tag_unused_no_mouse_or_touchpad);
            case MOUSE_NAV_BUTTONS:
                return context.getString(R.string.tag_unused_no_mouse);
            case TRACKPAD_SPEED:
            case LOCAL_CURSOR_SPEED:
            case ACCELERATION_CURVE:
            case MOMENTUM_GLIDE:
            case TRACKPAD_AREA:
            case FINGER_TAPS:
            case FINGER_TAP_ON_CANCEL:
                return context.getString(R.string.tag_unused_no_touchscreen);
            case TRACKPAD_SENSITIVITY_X:
            case TRACKPAD_SENSITIVITY_Y:
            case TRACKPAD_SWAP_AXIS:
            case NATIVE_TOUCHPAD:
                return context.getString(R.string.tag_unused_no_touchpad);
            case BACK_AS_META:
                return context.getString(R.string.tag_unused_no_keyboard);
            case OVERLAY_POSITION:
            case PreferenceConfiguration.COMPACT_OVERLAY_DEVICE_INFO_PREF_STRING:
                return context.getString(R.string.tag_unused_without, context.getString(R.string.title_performance_overlay));
            case SCREEN2_RESOLUTION:
                return context.getString(R.string.tag_unused_without, context.getString(R.string.title_checkbox_dual_screen));
            case COVERING_DECODER_SIZE:
                return context.getString(R.string.tag_unused_no_fold);
            case CHIPSET_TUNING:
                return context.getString(R.string.tag_unused_chip);
            case TV_DISPLAY_COMPATIBILITY:
                return context.getString(R.string.tag_tv_only);
            case IGDB_SECRET:
                return context.getString(R.string.tag_unused_without, context.getString(R.string.game_info_igdb_id_title));
            case PYROWAVE_PREACQUIRE:
                return context.getString(R.string.tag_unused_without, PYROWAVE);
            default:
                return null;
        }
    }

    /**
     * Whether a setting's stored value is not its default in preferences.xml; a setting never stored is at its
     * default. The IGDB client secret, kept apart from the settings, is changed once it is set.
     */
    static boolean changed(Context context, String key, SharedPreferences prefs) {
        if (IGDB_SECRET.equals(key)) {
            return !com.limelight.console.GameInfo.keyPrefs(context)
                    .getString(com.limelight.console.GameInfo.KEY_IGDB_SECRET, "").trim().isEmpty();
        }
        Object stored = prefs.getAll().get(key);
        if (stored == null) return false;
        String standard = xml(context).defaults.get(key);
        return standard != null && !standard.equals(String.valueOf(stored));
    }

    /** What preferences.xml says: each setting's default and the keys of the Debug section's rows. */
    private static final class Xml {
        final java.util.Map<String, String> defaults = new java.util.HashMap<>();
        final java.util.List<String> debugKeys = new java.util.ArrayList<>();
    }

    private static Xml xml;

    private static synchronized Xml xml(Context context) {
        if (xml != null) return xml;
        Xml read = new Xml();
        String android = "http://schemas.android.com/apk/res/android";
        java.util.List<String> debug = java.util.Arrays.asList(StreamSettings.SECTIONS.get("debug"));
        String category = null;
        try (android.content.res.XmlResourceParser parser = context.getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != org.xmlpull.v1.XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event != org.xmlpull.v1.XmlPullParser.START_TAG) continue;
                String key = parser.getAttributeValue(android, "key");
                if (key == null) continue;
                if (parser.getName().equals("PreferenceCategory")) {
                    category = key;
                    continue;
                }
                if (debug.contains(category)) read.debugKeys.add(key);
                int resource = parser.getAttributeResourceValue(android, "defaultValue", 0);
                String standard = resource != 0 ? context.getResources().getString(resource) :
                        parser.getAttributeValue(android, "defaultValue");
                if (standard != null) read.defaults.put(key, standard);
            }
        } catch (org.xmlpull.v1.XmlPullParserException | java.io.IOException | RuntimeException e) {
            com.limelight.LimeLog.warning("Unable to read the settings' defaults: " + e);
        }
        xml = read;
        return xml;
    }

    /** The audio effect a microphone setting turns on, or null for every other setting. */
    static MicProcessing.Effect micEffect(String key) {
        if (PreferenceConfiguration.MIC_ECHO_PREF_STRING.equals(key)) return MicProcessing.Effect.ECHO_CANCELER;
        if (PreferenceConfiguration.MIC_NOISE_PREF_STRING.equals(key)) return MicProcessing.Effect.NOISE_SUPPRESSOR;
        if (PreferenceConfiguration.MIC_AUTO_GAIN_PREF_STRING.equals(key)) return MicProcessing.Effect.AUTOMATIC_GAIN;
        return null;
    }

    /**
     * Whether the setting asks for an effect this device does not have, so its row says so: echo
     * cancellation unless it is off (with neither canceller its engine asks for), the switches while
     * on. The stream says the same when the microphone opens.
     */
    static boolean micEffectMissing(String key, SharedPreferences prefs, MicProcessing.Availability device) {
        MicProcessing.Effect effect = micEffect(key);
        if (effect == null) return false;
        if (effect == MicProcessing.Effect.ECHO_CANCELER) {
            return MicProcessing.Echo.of(prefs.getString(key, null)) != MicProcessing.Echo.OFF &&
                    !MicProcessing.EchoEngine.of(prefs.getString(
                            PreferenceConfiguration.MIC_ECHO_ENGINE_PREF_STRING, null)).available(device);
        }
        if (device.has(effect)) return false;
        switch (effect) {
            case NOISE_SUPPRESSOR:
                return prefs.getBoolean(key, true);
            default:
                return prefs.getBoolean(key, false);
        }
    }
}
