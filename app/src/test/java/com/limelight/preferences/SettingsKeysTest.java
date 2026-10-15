package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.content.res.XmlResourceParser;
import android.preference.PreferenceManager;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParser;

import java.util.Arrays;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.function.Function;

import static org.junit.Assert.*;

/** Settings were moved, retitled and reworded, but what users stored must keep working. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsKeysTest {
    // Every row of the settings screens before the restructure, actions included.
    private static final String[] KEYS_BEFORE = {
            "list_resolution", "list_fps", "seekbar_bitrate_kbps", "video_format", "adaptive_quality_mode",
            "adaptive_quality_min_resolution", "adaptive_quality_min_fps", "checkbox_enable_hdr", "checkbox_vrr",
            "checkbox_yuv444", "gpu_driver", "upscaling_mode", "upscaling_sharpness", "checkbox_stretch_video",
            "host_display_location", "checkbox_virtual_display", "seekbar_virtual_display_scale",
            "keyboard_shortcuts_editor", "seekbar_deadzone", "checkbox_multi_controller", "controller_button_mapping",
            "checkbox_flip_face_buttons", "checkbox_gamepad_motion_sensors", "checkbox_gamepad_motion_fallback",
            "checkbox_vibrate_fallback", "seekbar_vibrate_fallback_strength", "checkbox_gamepad_touchpad_as_mouse",
            "checkbox_mouse_emulation", "analog_scrolling", "checkbox_touchscreen_trackpad", "seekbar_trackpad_speed",
            "seekbar_mouse_speed", "checkbox_mouse_nav_buttons", "checkbox_absolute_mouse_mode",
            "checkbox_show_onscreen_controls", "checkbox_vibrate_osc", "checkbox_only_show_L3R3",
            "checkbox_show_guide_button", "seekbar_osc_opacity", "reset_osc", "checkbox_enable_perf_overlay",
            "performance_overlay_mode", "list_audio_config", "checkbox_host_audio", "checkbox_enable_mic",
            "checkbox_enable_audiofx", "checkbox_enable_post_stream_toast", "checkbox_auto_reconnect",
            "checkbox_enable_pip", "list_languages", "checkbox_small_icon_mode", "help", "latency_benchmark",
            "report_problem", "about_app", "reset_all", "frame_pacing", "checkbox_codec_low_latency",
            "checkbox_vendor_low_latency", "checkbox_codec_performance", "checkbox_phone_performance_hints",
            "checkbox_gpu_max_clocks", "checkbox_drop_late_frames", "checkbox_unbatched_input",
            "checkbox_network_priority", "checkbox_texture_view", "checkbox_unlock_fps",
            "checkbox_reduce_refresh_rate", "checkbox_full_range", "checkbox_usb_driver", "checkbox_usb_dualsense",
            "checkbox_usb_dualsense_haptics", "checkbox_usb_bind_all", "checkbox_enable_sops",
            "checkbox_disable_warnings", "export_latency_csv", "checkbox_quick_resume",
    };
    // Rows that open a screen or a dialog and store nothing under their key.
    private static final Set<String> ACTIONS = new HashSet<>(Arrays.asList(
            "gpu_driver", "host_display_location", "keyboard_shortcuts_editor", "controller_button_mapping",
            "controller_test", "reset_osc", "help", "latency_benchmark", "report_problem", "about_app", "reset_all", "export_latency_csv",
            // The merged switch writes the three decoder switches and stores nothing itself.
            StreamSettings.LOW_LATENCY_DECODING_KEY));
    // Stored settings that are read outside PreferenceConfiguration.
    private static final Set<String> READ_ELSEWHERE = new HashSet<>(Arrays.asList(
            "performance_overlay_mode", // Game and the overlay
            StreamSettings.DEVELOPER_OPTIONS_PREF_STRING)); // StreamSettings

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    private static Map<String, String> keysInXml() throws Exception {
        Map<String, String> keys = new LinkedHashMap<>();
        try (XmlResourceParser parser = RuntimeEnvironment.getApplication().getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event == XmlPullParser.START_TAG && !"PreferenceCategory".equals(parser.getName())) {
                    String key = parser.getAttributeValue("http://schemas.android.com/apk/res/android", "key");
                    if (key != null) keys.put(key, parser.getName());
                }
            }
        }
        return keys;
    }

    @Test
    public void everyKeyFromBeforeTheRestructureIsStillASetting() throws Exception {
        Map<String, String> keys = keysInXml();
        for (String key : KEYS_BEFORE) {
            assertTrue("Missing or renamed: " + key, keys.containsKey(key));
        }
        assertEquals(KEYS_BEFORE.length, new HashSet<>(Arrays.asList(KEYS_BEFORE)).size());
    }

    /** A stored value, one other value, and what the stream configuration reads from it. */
    private static final class Read {
        final Object a, b;
        final Function<PreferenceConfiguration, Object> field;

        Read(Object a, Object b, Function<PreferenceConfiguration, Object> field) {
            this.a = a;
            this.b = b;
            this.field = field;
        }
    }

    private static Map<String, Read> reads() {
        Map<String, Read> reads = new LinkedHashMap<>();
        reads.put("list_resolution", new Read("1280x720", "1920x1080", c -> c.width));
        reads.put("list_fps", new Read("30", "60", c -> c.fps));
        reads.put("seekbar_bitrate_kbps", new Read(10000, 45500, c -> c.bitrate));
        reads.put("video_format", new Read("auto", "neverh265", c -> c.videoFormat));
        reads.put("adaptive_quality_mode", new Read("off", "motion", c -> c.adaptiveQualityMode));
        reads.put("adaptive_quality_min_resolution", new Read("720", "360", c -> c.qualityMinShortEdge));
        reads.put("adaptive_quality_min_fps", new Read("30", "60", c -> c.qualityMinFps));
        reads.put("checkbox_enable_hdr", new Read(false, true, c -> c.enableHdr));
        reads.put("checkbox_vrr", new Read(false, true, c -> c.vrr));
        reads.put("frame_pacing", new Read("latency", "balanced", c -> c.framePacing));
        reads.put("checkbox_reduce_refresh_rate", new Read(false, true, c -> c.reduceRefreshRate));
        reads.put("upscaling_mode", new Read("off", "fsr1", c -> c.upscalingMode));
        reads.put("upscaling_sharpness", new Read(50, 80, c -> c.upscalingSharpness));
        reads.put("checkbox_stretch_video", new Read(false, true, c -> c.stretchVideo));
        reads.put("checkbox_yuv444", new Read(false, true, c -> c.enableYuv444));
        reads.put("checkbox_full_range", new Read(false, true, c -> c.fullRange));
        reads.put("checkbox_virtual_display", new Read(false, true, c -> c.virtualDisplay));
        reads.put("seekbar_virtual_display_scale", new Read(100, 150, c -> c.virtualDisplayScale));
        reads.put("checkbox_multi_controller", new Read(true, false, c -> c.multiController));
        reads.put("seekbar_deadzone", new Read(7, 12, c -> c.deadzonePercentage));
        reads.put("checkbox_flip_face_buttons", new Read(false, true, c -> c.flipFaceButtons));
        reads.put("checkbox_mouse_emulation", new Read(true, false, c -> c.mouseEmulation));
        reads.put("analog_scrolling", new Read("right", "left", c -> c.analogStickForScrolling));
        reads.put("checkbox_mouse_emulation_fixed_step", new Read(false, true, c -> c.mouseEmulationFixedStep));
        reads.put("checkbox_disable_rumble", new Read(false, true, c -> c.disableRumble));
        reads.put("checkbox_back_as_meta", new Read(false, true, c -> c.backAsMeta));
        reads.put("checkbox_right_alt_as_meta", new Read(false, true, c -> c.rightAltAsMeta));
        reads.put("checkbox_gamepad_touchpad_as_mouse", new Read(false, true, c -> c.gamepadTouchpadAsMouse));
        reads.put("checkbox_gamepad_motion_sensors", new Read(true, false, c -> c.gamepadMotionSensors));
        reads.put("checkbox_gamepad_motion_fallback", new Read(false, true, c -> c.gamepadMotionSensorsFallbackToDevice));
        reads.put("checkbox_vibrate_fallback", new Read(false, true, c -> c.vibrateFallbackToDevice));
        reads.put("seekbar_vibrate_fallback_strength", new Read(100, 150, c -> c.vibrateFallbackToDeviceStrength));
        reads.put("checkbox_usb_driver", new Read(true, false, c -> c.usbDriver));
        reads.put("checkbox_usb_bind_all", new Read(false, true, c -> c.bindAllUsb));
        reads.put("checkbox_usb_dualsense", new Read(false, true, c -> c.usbDualSense));
        reads.put("checkbox_usb_dualsense_haptics", new Read(false, true, c -> c.usbDualSenseHaptics));
        reads.put("checkbox_touchscreen_trackpad", new Read(true, false, c -> c.touchscreenTrackpad));
        reads.put("seekbar_trackpad_speed", new Read(100, 200, c -> c.trackpadSpeed));
        reads.put("seekbar_mouse_speed", new Read(100, 200, c -> c.mouseSpeed));
        reads.put("checkbox_mouse_nav_buttons", new Read(false, true, c -> c.mouseNavButtons));
        reads.put("checkbox_absolute_mouse_mode", new Read(false, true, c -> c.absoluteMouseMode));
        reads.put("checkbox_show_onscreen_controls", new Read(false, true, c -> c.onscreenController));
        reads.put("checkbox_vibrate_osc", new Read(true, false, c -> c.vibrateOsc));
        reads.put("checkbox_only_show_L3R3", new Read(false, true, c -> c.onlyL3R3));
        reads.put("checkbox_show_guide_button", new Read(true, false, c -> c.showGuideButton));
        reads.put("seekbar_osc_opacity", new Read(90, 50, c -> c.oscOpacity));
        // list_audio_config is checked by everyStoredSettingIsReadByTheStreamConfiguration only: the
        // MoonBridge audio constants it maps to are not created under the test shadow.
        reads.put("checkbox_host_audio", new Read(false, true, c -> c.playHostAudio));
        reads.put("checkbox_enable_mic", new Read(false, true, c -> c.enableMic));
        reads.put("checkbox_enable_audiofx", new Read(false, true, c -> c.enableAudioFx));
        reads.put("checkbox_enable_perf_overlay", new Read(false, true, c -> c.enablePerfOverlay));
        reads.put("checkbox_enable_pip", new Read(false, true, c -> c.enablePip));
        reads.put("checkbox_auto_reconnect", new Read(false, true, c -> c.autoReconnect));
        reads.put("checkbox_disable_warnings", new Read(false, true, c -> c.disableWarnings));
        reads.put("list_languages", new Read("default", "de", c -> c.language));
        reads.put("checkbox_small_icon_mode", new Read(false, true, c -> c.smallIconMode));
        reads.put("checkbox_quick_resume", new Read(false, true, c -> c.quickResume));
        reads.put("checkbox_enable_sops", new Read(true, false, c -> c.enableSops));
        reads.put("checkbox_codec_low_latency", new Read(true, false, c -> c.codecLowLatency));
        reads.put("checkbox_vendor_low_latency", new Read(true, false, c -> c.vendorLowLatency));
        reads.put("checkbox_phone_performance_hints", new Read(true, false, c -> c.phonePerformanceHints));
        reads.put("checkbox_codec_performance", new Read(true, false, c -> c.codecPerformance));
        reads.put("checkbox_gpu_max_clocks", new Read(false, true, c -> c.gpuMaxClocks));
        reads.put("checkbox_drop_late_frames", new Read(true, false, c -> c.dropLateFrames));
        reads.put("checkbox_texture_view", new Read(false, true, c -> c.useTextureView));
        reads.put("checkbox_unlock_fps", new Read(false, true, c -> c.unlockFps));
        reads.put("checkbox_unbatched_input", new Read(true, false, c -> c.unbatchedInput));
        reads.put("checkbox_network_priority", new Read(true, false, c -> c.networkPriority));
        reads.put("checkbox_enable_post_stream_toast", new Read(false, true, c -> c.enableLatencyToast));
        return reads;
    }

    private static void put(SharedPreferences.Editor editor, String key, Object value) {
        if (value instanceof Boolean) editor.putBoolean(key, (Boolean) value);
        else if (value instanceof Integer) editor.putInt(key, (Integer) value);
        else editor.putString(key, (String) value);
    }

    @Test
    public void everyStoredSettingIsReadByTheStreamConfiguration() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        // The dual-sense haptics switch is read only while the USB driver for it is on.
        prefs().edit().putBoolean("checkbox_usb_dualsense", true).commit();
        Set<String> read = new HashSet<>();
        SharedPreferences backing = prefs();
        SharedPreferences recording = (SharedPreferences) java.lang.reflect.Proxy.newProxyInstance(
                getClass().getClassLoader(), new Class<?>[] {SharedPreferences.class}, (proxy, method, args) -> {
                    if (method.getName().startsWith("get") && args != null && args.length > 0 && args[0] instanceof String) {
                        read.add((String) args[0]);
                    }
                    return method.invoke(backing, args);
                });
        android.content.ContextWrapper context = new android.content.ContextWrapper(app) {
            @Override public SharedPreferences getSharedPreferences(String name, int mode) {
                return name.equals(app.getPackageName() + "_preferences") ? recording : super.getSharedPreferences(name, mode);
            }
        };
        PreferenceConfiguration.readPreferences(context);

        Set<String> stored = new HashSet<>(keysInXml().keySet());
        stored.removeAll(ACTIONS);
        stored.removeAll(READ_ELSEWHERE);
        Set<String> unread = new HashSet<>(stored);
        unread.removeAll(read);
        assertTrue("Stored settings PreferenceConfiguration no longer reads: " + unread, unread.isEmpty());
        Set<String> checked = new HashSet<>(reads().keySet());
        checked.add(PreferenceConfiguration.AUDIO_CONFIG_PREF_STRING);
        assertEquals("Each stored setting needs a value check below", stored, checked);
    }

    @Test
    public void everyStoredSettingStillReachesTheStreamConfiguration() {
        Application app = RuntimeEnvironment.getApplication();
        Map<String, Read> reads = reads();

        // Run the one-time migrations first, then keep the values they would otherwise replace.
        PreferenceConfiguration.readPreferences(app);
        prefs().edit().putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true)
                .putBoolean("checkbox_usb_dualsense", true).commit();
        for (Map.Entry<String, Read> entry : reads.entrySet()) {
            String key = entry.getKey();
            Read read = entry.getValue();
            SharedPreferences.Editor editor = prefs().edit();
            put(editor, key, read.a);
            editor.commit();
            Object first = read.field.apply(PreferenceConfiguration.readPreferences(app));
            editor = prefs().edit();
            put(editor, key, read.b);
            editor.commit();
            Object second = read.field.apply(PreferenceConfiguration.readPreferences(app));
            assertNotEquals(key + " is no longer read", first, second);
            // Leave the switch the dual-sense haptics row depends on as it was set above.
            if (!"checkbox_usb_dualsense".equals(key)) {
                editor = prefs().edit();
                put(editor, key, read.a);
                editor.commit();
            } else {
                prefs().edit().putBoolean(key, true).commit();
            }
        }
    }

    @Test
    public void lowLatencyDecodingMovesItsThreeSwitchesTogether() {
        SharedPreferences prefs = prefs();
        // Run the one-time native-defaults migration first; it turns the latency switches on.
        PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertEquals(3, PreferenceConfiguration.lowLatencyDecodingCount(prefs));
        PreferenceConfiguration.setLowLatencyDecoding(prefs, false);
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertFalse(config.codecLowLatency);
        assertFalse(config.vendorLowLatency);
        assertFalse(config.phonePerformanceHints);
        // The other latency switches are separate developer options.
        assertTrue(config.dropLateFrames && config.unbatchedInput && config.networkPriority && config.codecPerformance);
        PreferenceConfiguration.setLowLatencyDecoding(prefs, true);
        assertEquals(3, PreferenceConfiguration.lowLatencyDecodingCount(prefs));
        for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) {
            assertTrue(Arrays.asList(StreamPreset.LATENCY_SWITCHES).contains(key));
        }
    }
}
