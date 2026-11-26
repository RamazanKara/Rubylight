package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.binding.video.VideoRendererChoice;

import static com.limelight.preferences.SettingsCatalogGen.*;

/**
 * One-time preference migrations, retired-key cleanups, and streaming reset helpers for {@link PreferenceConfiguration}.
 */
final class PreferenceMigrations {
    private PreferenceMigrations() {}

    static void resetStreamingSettings(Context context) {
        String crashHostUuid = context.getSharedPreferences("DecoderTombstone", Context.MODE_PRIVATE)
                .getString("CrashHostUuid", null);
        if (crashHostUuid != null) {
            HostStreamProfile.reset(context, crashHostUuid);
        }
        StreamProfiles.clearVideoOverrides(context);
        AppStreamSettings.clearVideoOverrides(context, crashHostUuid);
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit()
                .remove(BITRATE_PREF_STRING)
                .remove(BITRATE_USER_SET_PREF_STRING)
                .remove(BITRATE_PREF_OLD_STRING)
                .remove(LEGACY_RES_FPS_PREF_STRING)
                .remove(RESOLUTION_PREF_STRING)
                .remove(FPS_PREF_STRING)
                .remove(VIDEO_FORMAT_PREF_STRING)
                .remove(ENABLE_HDR_PREF_STRING)
                .remove(ENABLE_SDR10_PREF_STRING)
                .remove(UNLOCK_FPS_STRING)
                .remove(FULL_RANGE_PREF_STRING)
                .apply();
    }

    static void completeLanguagePreferenceMigration(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit().putString(LANGUAGE_PREF_STRING, DEFAULT_LANGUAGE).apply();
    }

    static void migratePcProfilesToOverrides(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        if (prefs.getBoolean(PC_PROFILES_OVERRIDES_PREF_STRING, false)) {
            return;
        }
        int changed = HostStreamProfile.migrateAllToOverrides(context, PreferenceConfiguration.readPreferences(context));
        if (changed > 0) {
            com.limelight.LimeLog.info("PC stream profiles now keep only their own values: " + changed);
        }
        prefs.edit().putBoolean(PC_PROFILES_OVERRIDES_PREF_STRING, true).apply();
    }

    // Older installs hold the XML defaults written on first launch (720p60, latency switches off).
    // Move them once to the phone's native resolution and refresh rate with every latency switch on.
    static boolean migrateToNativeDefaults(SharedPreferences prefs, String nativeResolution, float panelMaxHz) {
        int version = prefs.getInt(PREFS_VERSION_PREF_STRING, 0);
        if (nativeResolution == null || version >= FOLLOW_SCREEN_VERSION) {
            return false;
        }
        SharedPreferences.Editor editor = prefs.edit();
        if (version < NATIVE_DEFAULTS_VERSION) {
            String fps = Integer.toString(DisplayFrameRatePolicy.streamFrameRate(panelMaxHz));
            editor.putString(RESOLUTION_PREF_STRING, nativeResolution)
                    .putString(FPS_PREF_STRING, fps)
                    .putInt(BITRATE_PREF_STRING, PreferenceResolution.getDefaultBitrate(nativeResolution, fps))
                    .putBoolean(BITRATE_USER_SET_PREF_STRING, false)
                    .remove(BITRATE_PREF_OLD_STRING)
                    .putString(FRAME_PACING_PREF_STRING, DEFAULT_FRAME_PACING)
                    .putString(UPSCALING_PREF_STRING, "off");
            for (String key : StreamPreset.LATENCY_SWITCHES) {
                if (StreamPreset.setsLatencySwitch(key)) editor.putBoolean(key, true);
            }
        }
        editor.putBoolean(RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true)
                .putInt(PREFS_VERSION_PREF_STRING, FOLLOW_SCREEN_VERSION);
        editor.apply();
        return true;
    }

    static void migrateToNativeDefaults(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        migrateToNativeDefaults(prefs, PreferenceResolution.nativeResolution(context),
                PreferenceResolution.panelMaxRefreshRate(context));
        if (prefs.getInt(PREFS_VERSION_PREF_STRING, 0) == FOLLOW_SCREEN_VERSION) {
            int changed = HostStreamProfile.migrateAllToNative(context);
            if (changed > 0) {
                com.limelight.LimeLog.info("PC stream profiles switched from 1920x1080 to native: " + changed);
            }
            prefs.edit().putInt(PREFS_VERSION_PREF_STRING, PROFILE_NATIVE_VERSION).apply();
        }
    }

    static boolean migrateAutoReconnectDefault(SharedPreferences prefs) {
        if (prefs.getBoolean(AUTO_RECONNECT_DEFAULT_ON_PREF_STRING, false)) {
            return false;
        }
        boolean wasOff = !prefs.getBoolean(AUTO_RECONNECT_PREF_STRING, DEFAULT_AUTO_RECONNECT);
        SharedPreferences.Editor editor = prefs.edit().putBoolean(AUTO_RECONNECT_DEFAULT_ON_PREF_STRING, true);
        if (wasOff) {
            editor.putBoolean(AUTO_RECONNECT_PREF_STRING, true);
        }
        editor.apply();
        return wasOff;
    }

    static boolean migrateReceiveDecodePriorityDefault(SharedPreferences prefs) {
        if (prefs.getBoolean(RECEIVE_DECODE_PRIORITY_DEFAULT_ON_PREF_STRING, false)) {
            return false;
        }
        boolean wasOff = !prefs.getBoolean(RECEIVE_DECODE_PRIORITY_PREF_STRING, true);
        SharedPreferences.Editor editor = prefs.edit().putBoolean(RECEIVE_DECODE_PRIORITY_DEFAULT_ON_PREF_STRING, true);
        if (wasOff) {
            editor.putBoolean(RECEIVE_DECODE_PRIORITY_PREF_STRING, true);
        }
        editor.apply();
        return wasOff;
    }

    static boolean migrateSubmitHintDefault(SharedPreferences prefs) {
        if (prefs.getBoolean(SUBMIT_HINT_DEFAULT_ON_PREF_STRING, false)) {
            return false;
        }
        boolean wasOff = !prefs.getBoolean(SUBMIT_HINT_PREF_STRING, true);
        SharedPreferences.Editor editor = prefs.edit().putBoolean(SUBMIT_HINT_DEFAULT_ON_PREF_STRING, true);
        if (wasOff) {
            editor.putBoolean(SUBMIT_HINT_PREF_STRING, true);
        }
        editor.apply();
        return wasOff;
    }

    static void retireEngineChoice(SharedPreferences prefs) {
        if (prefs.contains(RETIRED_RUST_ENGINE_PREF_STRING) || prefs.contains(RETIRED_RUST_ENGINE_DEFAULT_ON_PREF_STRING)) {
            prefs.edit().remove(RETIRED_RUST_ENGINE_PREF_STRING).remove(RETIRED_RUST_ENGINE_DEFAULT_ON_PREF_STRING).apply();
        }
    }

    static boolean migrateCodecLowLatencyCorruption(SharedPreferences prefs) {
        if (prefs.getBoolean(CODEC_LOW_LATENCY_CORRUPTION_OFF_PREF_STRING, false) ||
                !MediaCodecHelper.hasLowLatencyCorruptingDecoder()) {
            return false;
        }
        boolean wasOn = prefs.getBoolean(CODEC_LOW_LATENCY_PREF_STRING, true);
        prefs.edit().putBoolean(CODEC_LOW_LATENCY_CORRUPTION_OFF_PREF_STRING, true)
                .putBoolean(CODEC_LOW_LATENCY_PREF_STRING, false).apply();
        return wasOn;
    }

    static void migrateOverlayMode(SharedPreferences prefs) {
        String stored = prefs.getString(OVERLAY_MODE_PREF_STRING, null);
        String mode = PerformanceOverlay.normalizeMode(stored);
        SharedPreferences.Editor editor = null;
        if (!prefs.getBoolean(OVERLAY_COMPACT_DEFAULT_PREF_STRING, false)) {
            if (stored == null || PerformanceOverlay.MODE_STATISTICS.equals(stored)) {
                mode = PerformanceOverlay.MODE_COMPACT;
            }
            editor = prefs.edit().putBoolean(OVERLAY_COMPACT_DEFAULT_PREF_STRING, true);
        }
        if (!mode.equals(stored)) {
            editor = (editor != null ? editor : prefs.edit()).putString(OVERLAY_MODE_PREF_STRING, mode);
        }
        if (editor != null) editor.apply();
    }

    static boolean migrateRetiredGlRender(SharedPreferences prefs, int sdkInt) {
        if (!prefs.contains(RETIRED_GL_RENDER_PREF_STRING)) return false;
        boolean wasOn = prefs.getBoolean(RETIRED_GL_RENDER_PREF_STRING, false);
        boolean chosen = prefs.contains(VULKAN_RENDER_PREF_STRING);
        boolean upscaling = UpscalingPolicy.Mode.fromPreference(
                prefs.getString(UPSCALING_PREF_STRING, "off")) != UpscalingPolicy.Mode.OFF;
        boolean supported = sdkInt >= VideoRendererChoice.MIN_SDK;
        boolean switchOn = wasOn && !chosen && !upscaling && supported;
        SharedPreferences.Editor editor = prefs.edit().remove(RETIRED_GL_RENDER_PREF_STRING);
        if (switchOn) editor.putBoolean(VULKAN_RENDER_PREF_STRING, true);
        editor.apply();
        if (wasOn && !switchOn) {
            com.limelight.LimeLog.info("\"Render through OpenGL\" was on and is gone; Vulkan stays off (" +
                    (chosen ? "a Vulkan choice exists" : upscaling ? "an upscaler is selected" :
                            "Android " + sdkInt + " is too old") + ")");
        }
        return switchOn;
    }

    static void retireOscStyle(Context context, SharedPreferences prefs) {
        Object style = prefs.getAll().get(RETIRED_OSC_STYLE_PREF_STRING);
        com.limelight.binding.input.pad.ClassicLayoutMigration.retireStyle(context, "modern".equals(style),
                prefs.getBoolean(FLIP_FACE_BUTTONS_PREF_STRING, DEFAULT_FLIP_FACE_BUTTONS));
        if (style != null) prefs.edit().remove(RETIRED_OSC_STYLE_PREF_STRING).apply();
    }

    static void migrateKeyNames(SharedPreferences prefs) {
        com.limelight.ui.AppTheme.migrateTheme(prefs);
        SharedPreferences.Editor editor = null;
        for (String[] rename : RENAMED_PREFS) {
            if (!prefs.contains(rename[0])) continue;
            Object value = prefs.getAll().get(rename[0]);
            editor = editor != null ? editor : prefs.edit();
            if (value instanceof Boolean) editor.putBoolean(rename[1], (Boolean) value);
            else if (value instanceof String) editor.putString(rename[1], (String) value);
            else if (value instanceof Integer) editor.putInt(rename[1], (Integer) value);
            editor.remove(rename[0]);
        }
        if (editor != null) editor.apply();
    }

    static void dropAutoBitrateLimits(SharedPreferences prefs) {
        SharedPreferences.Editor editor = null;
        for (String key : REMOVED_AUTO_BITRATE_LIMIT_PREFS) {
            if (prefs.contains(key)) editor = (editor != null ? editor : prefs.edit()).remove(key);
        }
        if (editor != null) editor.apply();
    }
}
