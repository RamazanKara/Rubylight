package com.limelight.preferences;

import android.content.SharedPreferences;

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

    /** Only FSR and SGSR sharpen; Off and Smooth have nothing to adjust. */
    static boolean upscalingUsesSharpness(SharedPreferences prefs) {
        String mode = prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off");
        return "fsr1".equals(mode) || "sgsr1".equals(mode);
    }

    /** The lowest resolution and frame rate only matter while automatic quality runs. */
    static boolean qualityLimitsApply(SharedPreferences prefs) {
        return !"off".equals(prefs.getString(ADAPTIVE_QUALITY, "off"));
    }
}
