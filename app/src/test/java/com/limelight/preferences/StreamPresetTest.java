package com.limelight.preferences;

import android.content.SharedPreferences;
import org.junit.Test;
import java.lang.reflect.Proxy;
import java.util.HashMap;
import java.util.Map;
import static org.junit.Assert.*;

public class StreamPresetTest {
    private static final String PHONE = "2400x1080";

    @Test
    public void nativeUsesThePanelAndTurnsEveryLatencySwitchOn() {
        Map<String, Object> values = new HashMap<>();
        for (String key : StreamPreset.LATENCY_SWITCHES) values.put(key, false);
        values.put("checkbox_vrr", true);
        values.put("upscaling_mode", "fsr1");
        values.put("frame_pacing", "balanced");
        int[] applies = {0};
        StreamPreset.NATIVE.apply(preferences(values, applies), PHONE, 120);
        assertEquals(1, applies[0]);
        assertEquals(PHONE, values.get("list_resolution"));
        assertEquals("120", values.get("list_fps"));
        assertEquals(PreferenceConfiguration.getDefaultBitrate(PHONE, "120"), values.get("seekbar_bitrate_kbps"));
        assertEquals(false, values.get("checkbox_vrr"));
        assertEquals("off", values.get("upscaling_mode"));
        assertEquals("latency", values.get("frame_pacing"));
        assertEquals(false, values.get("checkbox_reduce_refresh_rate"));
        assertEquals(true, values.get("checkbox_codec_performance"));
        for (String key : StreamPreset.LATENCY_SWITCHES) assertEquals(key, true, values.get(key));
    }

    @Test
    public void batterySaverKeepsNativeResolutionAt30Fps() {
        Map<String, Object> values = new HashMap<>();
        StreamPreset.BATTERY_SAVER.apply(preferences(values, new int[1]), PHONE, 144);
        assertEquals(PHONE, values.get("list_resolution"));
        assertEquals("30", values.get("list_fps"));
        assertEquals("cap-fps", values.get("frame_pacing"));
        assertEquals(true, values.get("checkbox_reduce_refresh_rate"));
        assertEquals(false, values.get("checkbox_codec_performance"));
        assertEquals(false, values.get(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING));
    }

    @Test
    public void presetsLeaveCodecHdrAnd444Alone() {
        for (StreamPreset preset : StreamPreset.values()) {
            for (String codec : new String[] {"auto", "forceav1", "forceh265", "neverh265", "forcepyrowave"}) {
                Map<String, Object> values = new HashMap<>();
                values.put("video_format", codec);
                values.put("checkbox_enable_hdr", true);
                values.put("checkbox_yuv444", true);
                preset.apply(preferences(values, new int[1]), PHONE, 120);
                assertEquals(codec, values.get("video_format"));
                assertEquals(true, values.get("checkbox_enable_hdr"));
                assertEquals(true, values.get("checkbox_yuv444"));
            }
        }
    }

    @Test
    public void presetsFollowPanelRateWithinHostLimitExceptBatterySaver() {
        for (StreamPreset preset : StreamPreset.values()) {
            for (float rate : new float[] {59.94f, 90, 119.88f, 144, 165, 240, 1200}) {
                Map<String, Object> values = new HashMap<>();
                preset.apply(preferences(values, new int[1]), PHONE, rate);
                int expected = preset == StreamPreset.BATTERY_SAVER ? 30 : Math.min(1000, Math.round(rate));
                assertEquals(Integer.toString(expected), values.get("list_fps"));
            }
        }
    }

    @Test
    public void matchingReflectsValuesRatherThanLastSelectedPreset() {
        Map<String, Object> values = new HashMap<>();
        SharedPreferences preferences = preferences(values, new int[1]);
        for (StreamPreset selected : StreamPreset.values()) {
            selected.apply(preferences, PHONE, 120);
            for (StreamPreset candidate : StreamPreset.values()) {
                assertEquals(selected == candidate, candidate.matches(preferences, PHONE, 120));
            }
            assertFalse(selected.matches(preferences, "1920x1080", 120));
            values.put("seekbar_bitrate_kbps", (Integer) values.get("seekbar_bitrate_kbps") + 500);
            assertFalse(selected.matches(preferences, PHONE, 120));
            selected.apply(preferences, PHONE, 120);
            values.put("upscaling_mode", "sgsr1");
            assertFalse(selected.matches(preferences, PHONE, 120));
        }
    }

    @Test
    public void turningOffALatencySwitchMakesNativeCustom() {
        for (String key : StreamPreset.LATENCY_SWITCHES) {
            Map<String, Object> values = new HashMap<>();
            SharedPreferences preferences = preferences(values, new int[1]);
            StreamPreset.NATIVE.apply(preferences, PHONE, 120);
            values.put(key, false);
            assertFalse(key, StreamPreset.NATIVE.matches(preferences, PHONE, 120));
        }
    }

    @Test
    public void nativeTurnsTheGpuClocksOnAndBatterySaverTurnsThemOff() {
        String gpu = PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING;
        for (Boolean stored : new Boolean[] {null, true, false}) {
            Map<String, Object> values = new HashMap<>();
            if (stored != null) values.put(gpu, stored);
            SharedPreferences preferences = preferences(values, new int[1]);
            StreamPreset.NATIVE.apply(preferences, PHONE, 120);
            assertEquals(true, values.get(gpu));
            assertTrue(StreamPreset.NATIVE.matches(preferences, PHONE, 120));

            StreamPreset.BATTERY_SAVER.apply(preferences, PHONE, 120);
            assertEquals(false, values.get(gpu));
            assertTrue(StreamPreset.BATTERY_SAVER.matches(preferences, PHONE, 120));
            values.put(gpu, true);
            assertFalse(StreamPreset.BATTERY_SAVER.matches(preferences, PHONE, 120));
        }
        assertTrue(java.util.Arrays.asList(StreamPreset.LATENCY_SWITCHES).contains(gpu));
    }

    @Test
    public void freshDefaultsMatchNative() {
        // Values never written fall back to their defaults, which are Native's.
        Map<String, Object> values = new HashMap<>();
        values.put("list_resolution", PHONE);
        values.put("list_fps", "120");
        assertTrue(StreamPreset.NATIVE.matches(preferences(values, new int[1]), PHONE, 120));
    }

    @Test
    public void migrationMovesOldDefaultsToNativeOnce() {
        Map<String, Object> values = new HashMap<>();
        // What PcView stored on first launch before this change.
        values.put("list_resolution", "1280x720");
        values.put("list_fps", "60");
        values.put("seekbar_bitrate_kbps", PreferenceConfiguration.getDefaultBitrate("1280x720", "60"));
        values.put("frame_pacing", "balanced");
        values.put("video_format", "forceav1");
        values.put("checkbox_enable_hdr", true);
        values.put("checkbox_vrr", true);
        for (String key : StreamPreset.LATENCY_SWITCHES) values.put(key, false);
        SharedPreferences preferences = preferences(values, new int[1]);

        assertTrue(PreferenceConfiguration.migrateToNativeDefaults(preferences, PHONE, 120));
        assertEquals(PHONE, values.get("list_resolution"));
        assertEquals("120", values.get("list_fps"));
        assertEquals(PreferenceConfiguration.getDefaultBitrate(PHONE, "120"), values.get("seekbar_bitrate_kbps"));
        // The migrated bitrate is a default, so it keeps following resolution and frame rate.
        assertEquals(false, values.get(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING));
        assertEquals("latency", values.get("frame_pacing"));
        for (String key : StreamPreset.LATENCY_SWITCHES) assertEquals(key, true, values.get(key));
        assertEquals("forceav1", values.get("video_format"));
        assertEquals(true, values.get("checkbox_enable_hdr"));
        assertEquals(true, values.get("checkbox_vrr"));
        assertEquals(PreferenceConfiguration.FOLLOW_SCREEN_VERSION, values.get("prefs_version"));
        assertEquals(true, values.get(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING));
        // Max GPU clocks and chipset tuning are latency switches too: the migration turns them on.
        assertEquals(true, values.get(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING));
        assertEquals(true, values.get(PreferenceConfiguration.CHIPSET_TUNING_PREF_STRING));

        // Later changes stick.
        values.put("list_resolution", "1920x1080");
        values.put("checkbox_unbatched_input", false);
        assertFalse(PreferenceConfiguration.migrateToNativeDefaults(preferences, PHONE, 120));
        assertEquals("1920x1080", values.get("list_resolution"));
        assertEquals(false, values.get("checkbox_unbatched_input"));
    }

    @Test
    public void resolutionSavedOnAnotherPanelFollowsTheScreenAfterUpdating() {
        // A 0.4.0 test.6 to test.8 install: native defaults already applied, at a size from another panel.
        Map<String, Object> values = new HashMap<>();
        values.put("prefs_version", PreferenceConfiguration.NATIVE_DEFAULTS_VERSION);
        values.put("list_resolution", "1920x1080");
        values.put("checkbox_unbatched_input", false);
        SharedPreferences preferences = preferences(values, new int[1]);
        assertTrue(PreferenceConfiguration.migrateToNativeDefaults(preferences, PHONE, 120));
        assertEquals(PreferenceConfiguration.FOLLOW_SCREEN_VERSION, values.get("prefs_version"));
        // Only the resolution changes meaning; other choices made since stay.
        assertEquals(false, values.get("checkbox_unbatched_input"));
        assertEquals("1920x1080", values.get("list_resolution"));
        assertEquals(PHONE, PreferenceConfiguration.effectiveResolution(preferences, "1920x1080", PHONE));
        // The inner screen of a foldable gets its own size at the next stream.
        assertEquals("2184x1968", PreferenceConfiguration.effectiveResolution(preferences, "1920x1080", "2184x1968"));
        assertFalse(PreferenceConfiguration.migrateToNativeDefaults(preferences, PHONE, 120));
    }

    @Test
    public void aFixedResolutionStaysFixed() {
        Map<String, Object> values = new HashMap<>();
        values.put(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false);
        SharedPreferences preferences = preferences(values, new int[1]);
        assertEquals("1920x1080", PreferenceConfiguration.effectiveResolution(preferences, "1920x1080", PHONE));
        // An unreadable screen keeps the saved size.
        values.put(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true);
        assertEquals("1920x1080", PreferenceConfiguration.effectiveResolution(preferences, "1920x1080", null));
    }

    @Test
    public void presetsFollowTheScreen() {
        for (StreamPreset preset : StreamPreset.values()) {
            Map<String, Object> values = new HashMap<>();
            values.put(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false);
            preset.apply(preferences(values, new int[1]), PHONE, 120);
            assertEquals(true, values.get(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING));
        }
    }

    @Test
    public void migrationWaitsWhenTheScreenIsUnknown() {
        Map<String, Object> values = new HashMap<>();
        values.put("list_resolution", "1280x720");
        SharedPreferences preferences = preferences(values, new int[1]);
        assertFalse(PreferenceConfiguration.migrateToNativeDefaults(preferences, null, 60));
        assertEquals("1280x720", values.get("list_resolution"));
        assertFalse(values.containsKey("prefs_version"));
    }

    private SharedPreferences preferences(Map<String, Object> values, int[] applies) {
        SharedPreferences.Editor editor = (SharedPreferences.Editor) Proxy.newProxyInstance(
                getClass().getClassLoader(), new Class<?>[] {SharedPreferences.Editor.class}, (proxy, method, args) -> {
                    if (method.getName().startsWith("put")) {
                        values.put((String) args[0], args[1]);
                        return proxy;
                    }
                    if (method.getName().equals("remove")) {
                        values.remove((String) args[0]);
                        return proxy;
                    }
                    if (method.getName().equals("apply")) {
                        applies[0]++;
                    }
                    return null;
                });
        return (SharedPreferences) Proxy.newProxyInstance(
                getClass().getClassLoader(), new Class<?>[] {SharedPreferences.class}, (proxy, method, args) -> {
                    if (method.getName().startsWith("get")) {
                        return values.getOrDefault(args[0], args[1]);
                    }
                    if (method.getName().equals("contains")) {
                        return values.containsKey(args[0]);
                    }
                    return editor;
                });
    }
}
