package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;

import com.limelight.binding.audio.MicProcessing;

/**
 * Which settings apply given the others, so the Settings screens can disable or hide the ones that
 * would do nothing. The stream configuration enforces the same overrides when it reads the settings
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

    /** Frame pacing decides how frames are shown unless adaptive refresh or host frame times take over. */
    static boolean framePacingApplies(SharedPreferences prefs) {
        return !prefs.getBoolean("checkbox_vrr", false) && !prefs.getBoolean(HOST_FRAME_TIMES, false);
    }

    /** Dropping late frames is part of Balanced frame pacing; the other modes ignore the choice. */
    static boolean dropLateFramesApplies(SharedPreferences prefs) {
        return framePacingApplies(prefs) &&
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
