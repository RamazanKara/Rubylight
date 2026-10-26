package com.limelight.preferences;

import android.content.SharedPreferences;

import com.limelight.binding.video.DisplayFrameRatePolicy;

public enum StreamPreset {
    // The phone's own resolution at the panel's top refresh rate, with every latency switch on.
    NATIVE,
    // The phone's own resolution at 30 fps, with the panel slowed to match and the GPU left alone.
    BATTERY_SAVER;

    // Switches that cut latency. All are on by default, and Native turns them back on. Max GPU clocks
    // is not one of them: it is opt-in, and only Battery saver changes it (to off).
    static final String[] LATENCY_SWITCHES = {
            "checkbox_codec_low_latency",
            "checkbox_vendor_low_latency",
            "checkbox_phone_performance_hints",
            "checkbox_drop_late_frames",
            "checkbox_unbatched_input",
            "checkbox_network_priority",
    };

    int fps(float panelMaxHz) {
        return this == BATTERY_SAVER ? 30 : DisplayFrameRatePolicy.streamFrameRate(panelMaxHz);
    }

    String pacing() {
        return this == BATTERY_SAVER ? "cap-fps" : PreferenceConfiguration.DEFAULT_FRAME_PACING;
    }

    boolean matches(SharedPreferences preferences, String nativeResolution, float panelMaxHz) {
        String fps = Integer.toString(fps(panelMaxHz));
        int bitrate = PreferenceConfiguration.getDefaultBitrate(nativeResolution, fps);
        if (!nativeResolution.equals(preferences.getString(PreferenceConfiguration.RESOLUTION_PREF_STRING,
                PreferenceConfiguration.DEFAULT_RESOLUTION)) ||
                !fps.equals(preferences.getString(PreferenceConfiguration.FPS_PREF_STRING, PreferenceConfiguration.DEFAULT_FPS)) ||
                preferences.getInt(PreferenceConfiguration.BITRATE_PREF_STRING, bitrate) != bitrate ||
                preferences.getBoolean("checkbox_vrr", false) ||
                !pacing().equals(preferences.getString(PreferenceConfiguration.FRAME_PACING_PREF_STRING,
                        PreferenceConfiguration.DEFAULT_FRAME_PACING)) ||
                !"off".equals(preferences.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off")) ||
                preferences.getBoolean("checkbox_reduce_refresh_rate", false) != (this == BATTERY_SAVER) ||
                preferences.getBoolean("checkbox_codec_performance", true) != (this == NATIVE)) {
            return false;
        }
        if (this == BATTERY_SAVER) {
            return !preferences.getBoolean(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING,
                    PreferenceConfiguration.DEFAULT_GPU_MAX_CLOCKS);
        }
        for (String key : LATENCY_SWITCHES) {
            if (!preferences.getBoolean(key, true)) {
                return false;
            }
        }
        return true;
    }

    // The codec, HDR and 4:4:4 choices stay as they are.
    void apply(SharedPreferences preferences, String nativeResolution, float panelMaxHz) {
        String fps = Integer.toString(fps(panelMaxHz));
        SharedPreferences.Editor editor = preferences.edit()
                .putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, nativeResolution)
                .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true)
                .putString(PreferenceConfiguration.FPS_PREF_STRING, fps)
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING,
                        PreferenceConfiguration.getDefaultBitrate(nativeResolution, fps))
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false)
                .putBoolean("checkbox_vrr", false)
                .putString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off")
                .putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, pacing())
                .putBoolean("checkbox_reduce_refresh_rate", this == BATTERY_SAVER)
                .putBoolean("checkbox_codec_performance", this == NATIVE);
        if (this == BATTERY_SAVER) {
            editor.putBoolean(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING, false);
        } else {
            for (String key : LATENCY_SWITCHES) {
                editor.putBoolean(key, true);
            }
        }
        editor.apply();
    }
}
