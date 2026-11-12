package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;

import com.limelight.binding.audio.MicProcessing;
import com.limelight.binding.video.DisplayFrameRatePolicy;

/**
 * Which settings apply given the others, so the Settings screens can hide the ones that would do
 * nothing (a hidden row keeps its value and comes back in its place). The stream configuration enforces the same overrides when it reads the settings
 * (adaptive refresh and host frame times both replace frame pacing); these only keep the screens honest.
 */
final class SettingRules {
    static final String DROP_LATE_FRAMES = "checkbox_drop_late_frames";
    static final String FRAME_PACING = PreferenceConfiguration.FRAME_PACING_PREF_STRING;
    static final String HOST_FRAME_TIMES = PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING;
    static final String ADAPTIVE_QUALITY = "adaptive_quality_mode";
    static final String ADAPTIVE_QUALITY_MIN_RESOLUTION = "adaptive_quality_min_resolution";
    static final String ADAPTIVE_QUALITY_MIN_FPS = "adaptive_quality_min_fps";

    private SettingRules() { }

    /**
     * Whether adaptive refresh is on: chosen, and offered on this device. A choice saved where no
     * screen has ARR is kept for a later Android that adds it, but reads as off until then.
     */
    static boolean vrrOn(Context context, SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false) &&
                DisplayFrameRatePolicy.vrrOffered(context);
    }

    /** Frame pacing decides how frames are shown unless adaptive refresh or host frame times take over. */
    static boolean framePacingApplies(Context context, SharedPreferences prefs) {
        return !vrrOn(context, prefs) && !prefs.getBoolean(HOST_FRAME_TIMES, false);
    }

    /** Dropping late frames is part of Balanced frame pacing; the other modes ignore the choice. */
    static boolean dropLateFramesApplies(Context context, SharedPreferences prefs) {
        return framePacingApplies(context, prefs) &&
                "balanced".equals(prefs.getString(FRAME_PACING, PreferenceConfiguration.DEFAULT_FRAME_PACING));
    }

    /**
     * The Vulkan renderer scales the picture itself and replaces the upscaler, so the choice does
     * nothing while it is on. It does not apply on Android 9 and older, where the setting is ignored.
     */
    static boolean upscalingApplies(SharedPreferences prefs) {
        return !prefs.getBoolean(PreferenceConfiguration.VULKAN_RENDER_PREF_STRING, false) ||
                android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.Q;
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

    /** Only the new streaming engine roams; the C engine ignores the choice. */
    static boolean clientRoamingApplies(SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.RUST_ENGINE_PREF_STRING, true);
    }

    /**
     * Passthrough needs the new streaming engine, and Android 10 or later to tell which formats the
     * audio output plays as is.
     */
    static boolean audioPassthroughApplies(SharedPreferences prefs, int sdk) {
        return sdk >= android.os.Build.VERSION_CODES.Q &&
                prefs.getBoolean(PreferenceConfiguration.RUST_ENGINE_PREF_STRING, true);
    }

    /** Only the new streaming engine forwards USB devices. */
    static boolean usbForwardingApplies(SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.RUST_ENGINE_PREF_STRING, true);
    }

    /** Only the new streaming engine syncs the clipboard. */
    static boolean clipboardSyncApplies(SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.RUST_ENGINE_PREF_STRING, true);
    }

    /** Only the new streaming engine knows the shift each picture still needs. */
    static boolean frameWarpApplies(SharedPreferences prefs) {
        return prefs.getBoolean(PreferenceConfiguration.RUST_ENGINE_PREF_STRING, true);
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

    /** Continuing on mobile data is a roaming choice, so it shows only while roaming is on. */
    static boolean roamingMeteredApplies(SharedPreferences prefs) {
        return clientRoamingApplies(prefs) &&
                prefs.getBoolean(PreferenceConfiguration.CLIENT_ROAMING_PREF_STRING, true);
    }

    /** The stick settings belong to the modern controller only. */
    static boolean modernControllerApplies(SharedPreferences prefs) {
        return PreferenceConfiguration.OSC_STYLE_MODERN.equals(
                PreferenceConfiguration.oscStyle(prefs.getString(PreferenceConfiguration.OSC_STYLE_PREF_STRING, null)));
    }

    /** The bitrate slider is the player's while Automatic bitrate is off. */
    static boolean bitrateShown(SharedPreferences prefs) {
        return PreferenceConfiguration.isBitrateUserSet(prefs);
    }

    private static boolean pyroWaveChosen(SharedPreferences prefs) {
        return "forcepyrowave".equals(prefs.getString("video_format", "auto"));
    }

    /** Sharper colored text: PyroWave's 4:4:4 is its own codec choice. */
    static boolean sharpColorApplies(SharedPreferences prefs) {
        return !pyroWaveChosen(prefs);
    }

    /** Less color banding is 10-bit SDR: not with PyroWave, and HDR is 10-bit already. */
    static boolean sdr10Applies(SharedPreferences prefs) {
        return !pyroWaveChosen(prefs) && !prefs.getBoolean("checkbox_enable_hdr", false);
    }

    /** A lower refresh rate is chosen while adaptive refresh is off. */
    static boolean reduceRefreshRateApplies(Context context, SharedPreferences prefs) {
        return !vrrOn(context, prefs);
    }

    /** Matching the game's frame rate needs the screen to be allowed below the stream's rate. */
    static boolean contentRateApplies(Context context, SharedPreferences prefs) {
        return reduceRefreshRateApplies(context, prefs) && prefs.getBoolean("checkbox_reduce_refresh_rate", false);
    }

    /** Display queue recovery works on a fixed refresh with Lowest latency or Balanced pacing (DisplayQueueWatch). */
    static boolean displayQueueRecoveryApplies(Context context, SharedPreferences prefs) {
        String pacing = prefs.getString(FRAME_PACING, PreferenceConfiguration.DEFAULT_FRAME_PACING);
        return !vrrOn(context, prefs) && ("latency".equals(pacing) || "balanced".equals(pacing));
    }

    /** Video position moves a picture that leaves room: Fit or 1:1 pixels. */
    static boolean videoAlignmentApplies(SharedPreferences prefs) {
        int mode = com.limelight.ui.VideoScaling.resolve(prefs.getString(com.limelight.ui.VideoScaling.PREF, null),
                prefs.getBoolean(com.limelight.ui.VideoScaling.STRETCH_PREF, false));
        return mode == com.limelight.ui.VideoScaling.FIT || mode == com.limelight.ui.VideoScaling.NATIVE;
    }

    /** The PC renders at a scale of the stream only on its virtual display. */
    static boolean virtualDisplayScaleApplies(SharedPreferences prefs) {
        return prefs.getBoolean("checkbox_virtual_display", false);
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
            "seekbar_trigger_deadzone", "checkbox_gamepad_motion_sensors", "checkbox_disable_rumble", "checkbox_mouse_emulation",
            "checkbox_gamepad_touchpad_as_mouse", "list_guide_menu_hold", "checkbox_back_opens_menu", "checkbox_back_as_guide",
            "list_stream_menu_button_y", "list_stream_menu_button_x", "list_stream_menu_button_lb", "list_stream_menu_button_rb"};
    /** Rows hidden while the on-screen controls are not shown. */
    static final String[] ON_SCREEN_ROWS = {"list_osc_style", "checkbox_vibrate_osc", "checkbox_only_show_L3R3",
            "checkbox_onscreen_own_player", "checkbox_show_guide_button", "checkbox_show_touchpad_button", "seekbar_osc_opacity",
            "reset_osc"};
    /** Rows hidden while the microphone is not sent. */
    static final String[] MIC_ROWS = {"checkbox_mic_button", PreferenceConfiguration.MIC_ECHO_PREF_STRING,
            PreferenceConfiguration.MIC_NOISE_PREF_STRING, PreferenceConfiguration.MIC_AUTO_GAIN_PREF_STRING, "seekbar_mic_gain"};

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
