package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.content.res.XmlResourceParser;
import android.preference.PreferenceManager;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParser;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
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
            "checkbox_show_onscreen_controls", "checkbox_vibrate_osc", "checkbox_only_show_L3R3", "checkbox_onscreen_own_player",
            "list_stream_menu_button_y", "list_stream_menu_button_x", "list_stream_menu_button_lb", "list_stream_menu_button_rb",
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
            com.limelight.console.HomeSettings.PREF_KEY,
            // Added later; kept here so it is never renamed either.
            "list_virtual_display_dpi", "checkbox_mute_audio", "checkbox_dual_screen", "list_resolution_screen2",
            "automatic_bitrate", "checkbox_system_keys_fullscreen_only", "checkbox_display_queue_recovery",
    };
    // Rows that open a screen or a dialog and store nothing under their key.
    private static final Set<String> ACTIONS = new HashSet<>(Arrays.asList(
            "gpu_driver", "keyboard_shortcuts_editor", "stream_hotkeys", "controller_button_mapping",
            "controller_test", "gyro_mapping_profiles", "stream_profiles", "reset_osc", "check_updates_now", "help", "latency_benchmark", "report_problem", "about_app", "reset_all", "export_latency_csv",
            "pin_lock", // opens its own screen; the PIN lives in the pin_lock prefs file
            "network_tests", // opens a saved PC's connection tests
            StreamSettings.PACE_CALIBRATION_KEY, // opens a saved PC's tests, where its PyroWave frame pace is measured
            // Pickers that write older switches and store nothing under their own key.
            StreamSettings.SHARP_COLOR_KEY, StreamSettings.CONTROLLERS_KEY,
            // The merged switch writes the three decoder switches and stores nothing itself.
            StreamSettings.LOW_LATENCY_DECODING_KEY, StreamSettings.LOW_LATENCY_CUSTOMIZE_KEY,
            // Automatic bitrate shows and sets bitrate_user_set and the bitrate; it stores nothing itself.
            StreamSettings.AUTOMATIC_BITRATE_KEY,
            // Each section's More row shows and hides its other rows.
            "more_video", "more_display", "more_input", "more_audio", "more_streaming", "more_app", "more_advanced",
            "more_debug"));
    // Stored settings that a picker or a dialog writes without a row of their own.
    private static final Set<String> WRITTEN_BY_PICKERS = new HashSet<>(Arrays.asList(
            "checkbox_unlock_fps", // the frame rate picker's button
            "performance_overlay_mode", // the performance overlay picker
            com.limelight.ui.AppTheme.OLED_BLACK, // the theme picker
            "checkbox_yuv444", "experimental_packed_444", // Sharper colored text
            "checkbox_multi_controller", "checkbox_send_controllers", // Controllers
            "checkbox_real_vrr", // Variable refresh rate's On or Fastest (VrrChoice)
            com.limelight.console.HomeSettings.PREF_KEY)); // When Nightjar opens
    // Stored settings that are read outside PreferenceConfiguration.
    private static final Set<String> READ_ELSEWHERE = new HashSet<>(Arrays.asList(
            "performance_overlay_mode", // Game and the overlay
            "list_stream_menu_button_y", "list_stream_menu_button_x", "list_stream_menu_button_lb", "list_stream_menu_button_rb", // StreamMenu
            com.limelight.console.OpenPlay.SETTING, // the launch screen, before any stream
            com.limelight.ui.AppTheme.OLED_BLACK, // AppTheme, including activities opened directly
            com.limelight.ui.AppTheme.ALWAYS_DARK,
            com.limelight.ui.AppTheme.HIGH_CONTRAST, // AppTheme and the contrast overlay
            com.limelight.binding.input.GyroDiagnosticOverlay.PREF_SHOW_DIAGNOSTICS, // Game's motion readings
            com.limelight.ui.AppTheme.UI_SCALE, // NightjarActivity.attachBaseContext
            com.limelight.ui.AppTheme.UI_MODE, // NightjarActivity.attachBaseContext
            com.limelight.ui.Motion.REDUCE_MOTION, // Motion
            com.limelight.update.UpdatePrefs.AUTO_CHECK_PREF_STRING, // UpdatePrefs
            HostDisplayChoice.MAIN_PREF, // HostDisplayChoice, under each PC's own choice
            com.limelight.console.HomeSettings.PREF_KEY)); // PcView and the console Home

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @Before
    public void aScreenWithArr() {
        DisplayFrameRatePolicy.vrrOfferedOverride = true;
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        DisplayFrameRatePolicy.vrrOfferedOverride = null;
    }

    @Test public void theOverlayPositionDefaultsToTopLeftAndAnUnknownValueReadsAsIt() {
        assertEquals("top_left", PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).overlayPosition);
        for (String key : RuntimeEnvironment.getApplication().getResources().getStringArray(R.array.overlay_position_values)) {
            prefs().edit().putString("performance_overlay_position", key).commit();
            assertEquals(key, PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).overlayPosition);
        }
        prefs().edit().putString("performance_overlay_position", "middle").commit();
        assertEquals("top_left", PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).overlayPosition);
    }

    @Test public void videoDataStraightToTheDecoderIsOnUntilTurnedOff() {
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).codecDirect);
        prefs().edit().putBoolean("checkbox_codec_direct", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).codecDirect);
    }

    @Test public void tapGestureActionsPersistIndependentlyAndMatchThePickerValues() {
        String[] actions = RuntimeEnvironment.getApplication().getResources()
                .getStringArray(R.array.three_finger_tap_values);
        assertArrayEquals(new String[] {"keyboard", "middle_click", "desktop_controls", "none", "pc_keyboard"}, actions);
        assertEquals(actions.length, RuntimeEnvironment.getApplication().getResources()
                .getStringArray(R.array.three_finger_tap_names).length);
        for (String three : actions) {
            for (String four : actions) {
                prefs().edit().putString("list_three_finger_tap", three)
                        .putString("list_four_finger_tap", four).commit();
                PreferenceConfiguration read = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
                assertEquals(three, read.threeFingerTap);
                assertEquals(four, read.fourFingerTap);
            }
        }
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

    // Keys of settings that were removed. They must not come back as a setting of another meaning.
    private static final Set<String> RETIRED = new HashSet<>(Arrays.asList(
            "checkbox_gl_render")); // "Render through OpenGL", replaced by Render through Vulkan

    @Test
    public void everyKeyInPreferencesXmlIsUniqueAndNoRetiredKeyIsBack() throws Exception {
        List<String> all = new ArrayList<>();
        try (XmlResourceParser parser = RuntimeEnvironment.getApplication().getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event == XmlPullParser.START_TAG) {
                    String key = parser.getAttributeValue("http://schemas.android.com/apk/res/android", "key");
                    if (key != null) all.add(key);
                }
            }
        }
        Set<String> seen = new HashSet<>();
        for (String key : all) assertTrue("Duplicate preference key: " + key, seen.add(key));
        for (String key : RETIRED) assertFalse("Retired key is back: " + key, seen.contains(key));
    }

    @Test
    public void renderThroughOpenGlUsersMoveToVulkanOnce() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", true).commit();
        assertTrue(PreferenceConfiguration.migrateRetiredGlRender(p, 33));
        assertFalse(p.contains("checkbox_gl_render"));
        assertTrue(p.getBoolean("checkbox_vulkan_render", false));
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);

        // Turning Vulkan off afterwards sticks: the old key is gone, so nothing switches it back on.
        p.edit().putBoolean("checkbox_vulkan_render", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);
    }

    @Test
    public void openGlUsersWithAnUpscalerOrAnOldAndroidDoNotGetVulkan() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", true).putString("upscaling_mode", "fsr1").commit();
        assertFalse(PreferenceConfiguration.migrateRetiredGlRender(p, 33));
        assertFalse(p.contains("checkbox_gl_render"));
        assertFalse(p.contains("checkbox_vulkan_render"));

        p.edit().putBoolean("checkbox_gl_render", true).remove("upscaling_mode").commit();
        assertFalse(PreferenceConfiguration.migrateRetiredGlRender(p, 28));
        assertFalse(p.contains("checkbox_gl_render"));
        assertFalse(p.contains("checkbox_vulkan_render"));

        // Android 10 is the first that can run it, with no upscaler selected
        p.edit().putBoolean("checkbox_gl_render", true).putString("upscaling_mode", "off").commit();
        assertTrue(PreferenceConfiguration.migrateRetiredGlRender(p, 29));
        assertTrue(p.getBoolean("checkbox_vulkan_render", false));
    }

    @Test
    public void anExplicitVulkanChoiceBeatsTheOldOpenGlSwitch() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", true).putBoolean("checkbox_vulkan_render", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);
        assertFalse(p.contains("checkbox_gl_render"));
    }

    @Test
    public void anOffOpenGlSwitchLeavesVulkanUnsetAndIsDropped() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);
        assertFalse(p.contains("checkbox_gl_render"));
        assertFalse(p.contains("checkbox_vulkan_render"));
    }

    @Test
    public void accessibilityKeysAreSettingsWithTheirDefaults() throws Exception {
        Map<String, String> keys = keysInXml();
        for (String key : new String[] {com.limelight.ui.AppTheme.UI_SCALE, com.limelight.ui.AppTheme.UI_MODE, com.limelight.ui.AppTheme.HIGH_CONTRAST,
                com.limelight.ui.Motion.REDUCE_MOTION}) {
            assertTrue("Missing: " + key, keys.containsKey(key));
        }
        assertEquals("list_ui_scale", com.limelight.ui.AppTheme.UI_SCALE);
        assertEquals("list_ui_mode", com.limelight.ui.AppTheme.UI_MODE);
        assertEquals("checkbox_high_contrast", com.limelight.ui.AppTheme.HIGH_CONTRAST);
        assertEquals("list_reduce_motion", com.limelight.ui.Motion.REDUCE_MOTION);
    }

    @Test
    public void everyKeyFromBeforeTheRestructureIsStillASetting() throws Exception {
        Map<String, String> keys = keysInXml();
        for (String key : KEYS_BEFORE) {
            assertTrue("Missing or renamed: " + key, keys.containsKey(key) || WRITTEN_BY_PICKERS.contains(key));
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
        reads.put("list_mobile_bitrate_kbps", new Read("0", "10000", c -> c.mobileBitrateLimit));
        reads.put("video_format", new Read("auto", "neverh265", c -> c.videoFormat));
        reads.put("adaptive_quality_mode", new Read("off", "motion", c -> c.adaptiveQualityMode));
        reads.put("adaptive_quality_min_resolution", new Read("720", "360", c -> c.qualityMinShortEdge));
        reads.put("adaptive_quality_min_fps", new Read("30", "60", c -> c.qualityMinFps));
        reads.put("checkbox_thermal_protection", new Read(false, true, c -> c.thermalProtection));
        reads.put("checkbox_auto_bitrate", new Read(false, true, c -> c.autoBitrate));
        reads.put("checkbox_rate_change_bitrate", new Read(false, true, c -> c.rateChangeBitrate));
        reads.put("checkbox_enable_hdr", new Read(false, true, c -> c.enableHdr));
        reads.put("checkbox_enable_sdr_10bit", new Read(false, true, c -> c.enableSdr10Bit));
        reads.put("checkbox_vrr", new Read(false, true, c -> c.vrr));
        reads.put("checkbox_host_frame_times", new Read(false, true, c -> c.hostFrameTimes));
        // Applies with VRR, which this device doesn't offer (VrrOfferedTest checks that it applies there).
        reads.put("checkbox_real_vrr", new Read(true, false, c -> c.realVrr));
        reads.put("checkbox_content_rate_frame_rate", new Read(false, true, c -> c.contentRateFrameRate));
        reads.put("frame_pacing", new Read("latency", "balanced", c -> c.framePacing));
        reads.put("checkbox_reduce_refresh_rate", new Read(false, true, c -> c.reduceRefreshRate));
        reads.put("upscaling_mode", new Read("off", "fsr1", c -> c.upscalingMode));
        reads.put("upscaling_sharpness", new Read(50, 80, c -> c.upscalingSharpness));
        reads.put("checkbox_stretch_video", new Read(false, true, c -> c.videoScaling));
        reads.put("video_alignment", new Read("center", "top", c -> c.videoAlignment));
        reads.put("stream_orientation", new Read("auto", "rotate", c -> c.streamOrientation));
        reads.put("checkbox_yuv444", new Read(false, true, c -> c.enableYuv444));
        reads.put("checkbox_full_range", new Read(false, true, c -> c.fullRange));
        reads.put("checkbox_virtual_display", new Read(false, true, c -> c.virtualDisplay));
        reads.put("seekbar_virtual_display_scale", new Read(100, 150, c -> c.virtualDisplayScale));
        reads.put("list_virtual_display_dpi", new Read("0", "150", c -> c.virtualDisplayDpi));
        reads.put("checkbox_multi_controller", new Read(true, false, c -> c.multiController));
        reads.put("checkbox_send_controllers", new Read(true, false, c -> c.sendControllers));
        reads.put("seekbar_deadzone", new Read(7, 12, c -> c.deadzonePercentage));
        reads.put("seekbar_trigger_deadzone", new Read(2, 10, c -> c.triggerDeadzonePercentage));
        reads.put("checkbox_flip_face_buttons", new Read(false, true, c -> c.flipFaceButtons));
        reads.put("checkbox_mouse_emulation", new Read(true, false, c -> c.mouseEmulation));
        reads.put("analog_scrolling", new Read("right", "left", c -> c.analogStickForScrolling));
        reads.put("checkbox_mouse_emulation_fixed_step", new Read(false, true, c -> c.mouseEmulationFixedStep));
        reads.put("checkbox_disable_rumble", new Read(false, true, c -> c.disableRumble));
        reads.put("checkbox_back_as_meta", new Read(false, true, c -> c.backAsMeta));
        reads.put("checkbox_back_opens_menu", new Read(false, true, c -> c.backOpensMenu));
        reads.put("list_gamepad_quit_combo", new Read("off", "lb_rb_a_b", c -> c.gamepadQuitCombo));
        reads.put("checkbox_back_as_guide", new Read(false, true, c -> c.backAsGuide));
        reads.put("list_guide_menu_hold", new Read("0", "1500", c -> c.guideMenuHoldMs));
        reads.put("checkbox_capture_system_keys", new Read(true, false, c -> c.captureSystemKeys));
        reads.put("checkbox_system_keys_fullscreen_only", new Read(false, true, c -> c.systemKeysFullscreenOnly));
        reads.put("checkbox_right_alt_as_meta", new Read(false, true, c -> c.rightAltAsMeta));
        reads.put("checkbox_keyboard_follows_text_fields", new Read(true, false, c -> c.keyboardFollowsTextFields));
        reads.put("checkbox_clipboard_sync", new Read(false, true, c -> c.clipboardSync));
        reads.put("checkbox_gamepad_touchpad_as_mouse", new Read(false, true, c -> c.gamepadTouchpadAsMouse));
        reads.put("checkbox_gamepad_motion_sensors", new Read(true, false, c -> c.gamepadMotionSensors));
        reads.put("checkbox_gamepad_motion_fallback", new Read(false, true, c -> c.gamepadMotionSensorsFallbackToDevice));
        reads.put("checkbox_vibrate_fallback", new Read(false, true, c -> c.vibrateFallbackToDevice));
        reads.put("seekbar_vibrate_fallback_strength", new Read(100, 150, c -> c.vibrateFallbackToDeviceStrength));
        reads.put("checkbox_usb_driver", new Read(true, false, c -> c.usbDriver));
        reads.put("checkbox_usb_bind_all", new Read(false, true, c -> c.bindAllUsb));
        reads.put("checkbox_usb_dualsense", new Read(false, true, c -> c.usbDualSense));
        reads.put("checkbox_usb_dualsense_haptics", new Read(false, true, c -> c.usbDualSenseHaptics));
        reads.put("checkbox_usb_switch_pro", new Read(false, true, c -> c.usbSwitchPro));
        reads.put("checkbox_usb_forwarding", new Read(false, true, c -> c.usbForwarding));
        reads.put("checkbox_touchscreen_trackpad", new Read(true, false, c -> c.touchscreenTrackpad));
        reads.put("seekbar_trackpad_speed", new Read(100, 200, c -> c.trackpadSpeed));
        reads.put("seekbar_local_cursor_speed", new Read(100, 200, c -> c.localCursorSpeed));
        reads.put("checkbox_trackpad_acceleration", new Read(false, true, c -> c.trackpadAcceleration));
        reads.put("list_trackpad_acceleration_curve", new Read("medium", "strong", c -> c.trackpadAccelerationCurve));
        reads.put("checkbox_trackpad_momentum", new Read(false, true, c -> c.trackpadMomentum));
        reads.put("list_trackpad_momentum_glide", new Read("medium", "long", c -> c.trackpadMomentumGlide));
        reads.put("list_trackpad_area", new Read("full", "right_half", c -> c.trackpadArea));
        reads.put("list_three_finger_tap", new Read("keyboard", "middle_click", c -> c.threeFingerTap));
        reads.put("list_four_finger_tap", new Read("none", "desktop_controls", c -> c.fourFingerTap));
        reads.put("checkbox_multi_touch_finger_taps", new Read(false, true, c -> c.multiTouchFingerTaps));
        reads.put("list_pc_keyboard_layout", new Read("auto", "de", c -> c.pcKeyboardLayout));
        reads.put("seekbar_pc_keyboard_size", new Read(50, 60, c -> c.pcKeyboardSize));
        reads.put("seekbar_pc_keyboard_opacity", new Read(90, 60, c -> c.pcKeyboardOpacity));
        reads.put("seekbar_mouse_speed", new Read(100, 200, c -> c.mouseSpeed));
        reads.put("seekbar_trackpad_sensitivity_x", new Read(100, -100, c -> c.trackpadSensitivityX));
        reads.put("seekbar_trackpad_sensitivity_y", new Read(100, -100, c -> c.trackpadSensitivityY));
        reads.put("seekbar_trackpad_scroll_speed", new Read(50, 100, c -> c.trackpadScrollSpeed));
        reads.put("checkbox_trackpad_swap_axis", new Read(false, true, c -> c.trackpadSwapAxis));
        reads.put("checkbox_native_touchpad", new Read(false, true, c -> c.nativeTouchpad));
        reads.put("checkbox_mouse_nav_buttons", new Read(false, true, c -> c.mouseNavButtons));
        reads.put("checkbox_absolute_mouse_mode", new Read(false, true, c -> c.absoluteMouseMode));
        reads.put("checkbox_input_only_any_host", new Read(false, true, c -> c.inputOnlyAnyHost));
        reads.put("checkbox_show_onscreen_controls", new Read(false, true, c -> c.onscreenController));
        reads.put("checkbox_vibrate_osc", new Read(true, false, c -> c.vibrateOsc));
        reads.put("checkbox_only_show_L3R3", new Read(false, true, c -> c.onlyL3R3));
        reads.put("checkbox_onscreen_own_player", new Read(false, true, c -> c.onscreenOwnPlayer));
        reads.put("checkbox_show_guide_button", new Read(true, false, c -> c.showGuideButton));
        reads.put("checkbox_show_touchpad_button", new Read(false, true, c -> c.showTouchpadButton));
        reads.put("checkbox_osc_steam_controls", new Read(false, true, c -> c.oscSteamControls));
        reads.put("seekbar_osc_opacity", new Read(90, 50, c -> c.oscOpacity));
        // list_audio_config is checked by everyStoredSettingIsReadByTheStreamConfiguration only: the
        // MoonBridge audio constants it maps to are not created under the test shadow.
        reads.put("checkbox_host_audio", new Read(false, true, c -> c.playHostAudio));
        reads.put("checkbox_enable_mic", new Read(false, true, c -> c.enableMic));
        reads.put("list_mic_echo_cancel", new Read("auto", "off", c -> c.micEcho));
        reads.put("checkbox_mic_noise_suppression", new Read(true, false, c -> c.micNoiseSuppression));
        reads.put("checkbox_mic_auto_gain", new Read(false, true, c -> c.micAutoGain));
        reads.put("seekbar_mic_gain", new Read(100, 150, c -> c.micGainPercent));
        reads.put("checkbox_mic_button", new Read(true, false, c -> c.micButton));
        reads.put("checkbox_floating_menu_button", new Read(false, true, c -> c.floatingMenuButton));
        reads.put("list_osc_style", new Read("classic", "modern", c -> c.oscStyle));
        reads.put("list_osc_stick_mode", new Read("floating", "fixed", c -> c.oscFloatingSticks ? "floating" : "fixed"));
        reads.put("seekbar_osc_stick_deadzone", new Read(8, 15, c -> c.oscStickDeadzone));
        reads.put("checkbox_enable_audiofx", new Read(false, true, c -> c.enableAudioFx));
        reads.put("checkbox_audio_passthrough", new Read(false, true, c -> c.audioPassthrough));
        reads.put("checkbox_mute_audio", new Read(false, true, c -> c.muteAudio));
        reads.put("checkbox_dual_screen", new Read(false, true, c -> c.dualScreen));
        reads.put("list_resolution_screen2", new Read("native", "1920x1080", c -> c.secondScreenResolution));
        reads.put("checkbox_enable_perf_overlay", new Read(false, true, c -> c.enablePerfOverlay));
        reads.put("performance_overlay_position", new Read("top_left", "bottom_right", c -> c.overlayPosition));
        reads.put("checkbox_enable_pip", new Read(false, true, c -> c.enablePip));
        reads.put("checkbox_auto_reconnect", new Read(false, true, c -> c.autoReconnect));
        reads.put("list_network_grace", new Read("0", "30", c -> c.networkGraceSeconds));
        reads.put("checkbox_tell_host_wireless", new Read(true, false, c -> c.tellHostWireless));
        reads.put("checkbox_tell_host_link_speed", new Read(true, false, c -> c.tellHostLinkSpeed));
        reads.put("list_fec_choice", new Read("host", "20", c -> c.fecChoice));
        reads.put("checkbox_link_probe", new Read(false, true, c -> c.linkProbe));
        reads.put("checkbox_disable_warnings", new Read(false, true, c -> c.disableWarnings));
        reads.put("list_languages", new Read("default", "de", c -> c.language));
        reads.put("checkbox_small_icon_mode", new Read(false, true, c -> c.smallIconMode));
        reads.put("checkbox_quick_resume", new Read(false, true, c -> c.quickResume));
        reads.put("checkbox_enable_sops", new Read(true, false, c -> c.enableSops));
        reads.put("checkbox_codec_low_latency", new Read(true, false, c -> c.codecLowLatency));
        reads.put("checkbox_vendor_low_latency", new Read(true, false, c -> c.vendorLowLatency));
        reads.put("list_hevc_low_latency_mode", new Read("auto", "on", c -> c.hevcLowLatencyMode));
        reads.put("checkbox_phone_performance_hints", new Read(true, false, c -> c.phonePerformanceHints));
        reads.put("checkbox_codec_performance", new Read(true, false, c -> c.codecPerformance));
        reads.put("checkbox_gpu_max_clocks", new Read(false, true, c -> c.gpuMaxClocks));
        reads.put("checkbox_drop_late_frames", new Read(true, false, c -> c.dropLateFrames));
        reads.put("checkbox_display_queue_recovery", new Read(true, false, c -> c.displayQueueRecovery));
        reads.put("checkbox_texture_view", new Read(false, true, c -> c.useTextureView));
        reads.put("checkbox_vulkan_render", new Read(false, true, c -> c.vulkanRender));
        reads.put("checkbox_unlock_fps", new Read(false, true, c -> c.unlockFps));
        reads.put("checkbox_unbatched_input", new Read(true, false, c -> c.unbatchedInput));
        reads.put("checkbox_network_priority", new Read(true, false, c -> c.networkPriority));
        reads.put("checkbox_submit_hint", new Read(false, true, c -> c.submitHint));
        reads.put("checkbox_covering_decoder_size", new Read(false, true, c -> c.coveringDecoderSize));
        reads.put("checkbox_pyrowave_preacquire", new Read(false, true, c -> c.pyroWavePreacquire));
        reads.put("checkbox_codec_direct", new Read(false, true, c -> c.codecDirect));
        reads.put("checkbox_receive_decode_priority", new Read(false, true, c -> c.receiveDecodePriority));
        reads.put("checkbox_client_roaming", new Read(true, false, c -> c.clientRoaming));
        reads.put("checkbox_roaming_metered", new Read(false, true, c -> c.roamingMetered));
        reads.put("checkbox_receive_coalescing", new Read(true, false, c -> c.receiveCoalescing));
        reads.put("checkbox_internet_play", new Read(true, false, c -> c.internetPlay));
        reads.put("checkbox_enable_post_stream_toast", new Read(false, true, c -> c.enableLatencyToast));
        reads.put("checkbox_rust_engine", new Read(false, true, c -> c.rustEngine));
        reads.put("experimental_packed_444", new Read(false, true, c -> c.experimentalPacked444));
        reads.put("checkbox_frame_warp", new Read(false, true, c -> c.frameWarp));
        reads.put("checkbox_experimental_chipset_tuning", new Read(false, true, c -> c.experimentalChipsetTuning));
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
        checked.removeAll(WRITTEN_BY_PICKERS);
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
                .putBoolean("checkbox_usb_dualsense", true)
                // The content-rate vote needs the lower refresh rate switch; its own row resets it to off later.
                .putBoolean("checkbox_reduce_refresh_rate", true).commit();
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
    public void trackpadSensitivityDefaultsToNormalKeepsItsSignAndStaysInRange() {
        Application app = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertEquals(100, config.trackpadSensitivityX);
        assertEquals(100, config.trackpadSensitivityY);

        prefs().edit().putInt("seekbar_trackpad_sensitivity_x", -100).putInt("seekbar_trackpad_sensitivity_y", 150).commit();
        config = PreferenceConfiguration.readPreferences(app);
        assertEquals(-100, config.trackpadSensitivityX);
        assertEquals(150, config.trackpadSensitivityY);

        prefs().edit().putInt("seekbar_trackpad_sensitivity_x", -900).putInt("seekbar_trackpad_sensitivity_y", 900).commit();
        config = PreferenceConfiguration.readPreferences(app);
        assertEquals(-200, config.trackpadSensitivityX);
        assertEquals(200, config.trackpadSensitivityY);
    }

    @Test
    public void touchModeKeepsTheOlderSwitchesAndAddsTheNewerModes() {
        Application app = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertEquals(com.limelight.binding.input.touch.TouchMode.TRACKPAD, config.touchMode);
        assertTrue(config.touchscreenTrackpad);

        prefs().edit().putBoolean("checkbox_touchscreen_trackpad", false).putBoolean("checkbox_native_touch", true).commit();
        assertEquals(com.limelight.binding.input.touch.TouchMode.MULTI_TOUCH,
                PreferenceConfiguration.readPreferences(app).touchMode);

        for (int mode : new int[] {com.limelight.binding.input.touch.TouchMode.LOCAL_CURSOR,
                com.limelight.binding.input.touch.TouchMode.OFF, com.limelight.binding.input.touch.TouchMode.TOUCHPAD,
                com.limelight.binding.input.touch.TouchMode.DIRECT}) {
            SharedPreferences.Editor editor = prefs().edit();
            com.limelight.binding.input.touch.TouchMode.save(editor, mode);
            editor.commit();
            config = PreferenceConfiguration.readPreferences(app);
            assertEquals(mode, config.touchMode);
            // Older versions read the same switches as before.
            assertEquals(mode != com.limelight.binding.input.touch.TouchMode.DIRECT,
                    prefs().getBoolean("checkbox_touchscreen_trackpad", true));
            assertEquals(mode != com.limelight.binding.input.touch.TouchMode.DIRECT, config.touchscreenTrackpad);
            assertFalse(prefs().getBoolean("checkbox_native_touch", true));
        }
    }

    @Test
    public void videoScalingKeepsTheOlderStretchSwitchAndAddsTheNewerModes() {
        Application app = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertEquals(com.limelight.ui.VideoScaling.FIT, config.videoScaling);
        assertFalse(config.stretchVideo);
        assertEquals(com.limelight.ui.VideoScaling.CENTER, config.videoAlignment);

        // An install from before the scaling modes: the switch alone decides.
        prefs().edit().putBoolean("checkbox_stretch_video", true).commit();
        config = PreferenceConfiguration.readPreferences(app);
        assertEquals(com.limelight.ui.VideoScaling.STRETCH, config.videoScaling);
        assertTrue(config.stretchVideo);

        for (int mode = com.limelight.ui.VideoScaling.FIT; mode <= com.limelight.ui.VideoScaling.NATIVE; mode++) {
            SharedPreferences.Editor editor = prefs().edit();
            com.limelight.ui.VideoScaling.save(editor, mode);
            editor.commit();
            config = PreferenceConfiguration.readPreferences(app);
            assertEquals(mode, config.videoScaling);
            // Older versions read the same switch as before: only Stretch stretches there.
            boolean stretch = mode == com.limelight.ui.VideoScaling.STRETCH;
            assertEquals(stretch, prefs().getBoolean("checkbox_stretch_video", !stretch));
            assertEquals(stretch, config.stretchVideo);
        }

        // Fill was saved, then an older version turned Stretch on: the older choice wins.
        SharedPreferences.Editor editor = prefs().edit();
        com.limelight.ui.VideoScaling.save(editor, com.limelight.ui.VideoScaling.FILL);
        editor.putBoolean("checkbox_stretch_video", true).commit();
        assertEquals(com.limelight.ui.VideoScaling.STRETCH, PreferenceConfiguration.readPreferences(app).videoScaling);

        prefs().edit().putString("video_alignment", "top").commit();
        assertEquals(com.limelight.ui.VideoScaling.TOP, PreferenceConfiguration.readPreferences(app).videoAlignment);
    }

    @Test
    public void orientationIsAutomaticUntilChosen() {
        Application app = RuntimeEnvironment.getApplication();
        assertEquals(com.limelight.ui.StreamOrientation.AUTO, PreferenceConfiguration.readPreferences(app).streamOrientation);
        prefs().edit().putString("stream_orientation", "portrait").commit();
        assertEquals(com.limelight.ui.StreamOrientation.PORTRAIT, PreferenceConfiguration.readPreferences(app).streamOrientation);
        // A value from a newer version falls back to the long-standing behaviour.
        prefs().edit().putString("stream_orientation", "upside_down").commit();
        assertEquals(com.limelight.ui.StreamOrientation.AUTO, PreferenceConfiguration.readPreferences(app).streamOrientation);
    }

    /** Sharper colored text: one choice over the two switches it replaced; either one on reads as Automatic. */
    @Test
    public void sharperColoredTextReadsEitherOldSwitchAsAutomatic() {
        Application app = RuntimeEnvironment.getApplication();
        Boolean[][] cases = {
                // checkbox_yuv444, experimental_packed_444 (null: never set), Automatic
                {null, null, false}, {false, null, false}, {null, false, false}, {false, false, false},
                {true, null, true}, {null, true, true}, {true, false, true}, {false, true, true}, {true, true, true},
        };
        for (Boolean[] c : cases) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            if (c[0] != null) editor.putBoolean("checkbox_yuv444", c[0]);
            if (c[1] != null) editor.putBoolean("experimental_packed_444", c[1]);
            editor.commit();
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
            String name = c[0] + "/" + c[1];
            assertEquals(name, c[2], config.enableYuv444);
            assertEquals(name, c[2], config.experimentalPacked444);
            assertEquals(name, c[2], PreferenceConfiguration.sharpColor(prefs()));
            // Nothing is rewritten on load, so an older version reads its own switch as before.
            assertEquals(name, c[0], prefs().contains("checkbox_yuv444") ? prefs().getBoolean("checkbox_yuv444", false) : null);
        }
        // Choosing sets both, so older versions read the same choice.
        SharedPreferences.Editor editor = prefs().edit().clear();
        PreferenceConfiguration.saveSharpColor(editor, true);
        editor.commit();
        assertTrue(prefs().getBoolean("checkbox_yuv444", false) && prefs().getBoolean("experimental_packed_444", false));
        editor = prefs().edit();
        PreferenceConfiguration.saveSharpColor(editor, false);
        editor.commit();
        assertFalse(prefs().getBoolean("checkbox_yuv444", true) || prefs().getBoolean("experimental_packed_444", true));
        // A PC's, profile's or game's layer holds one value for both.
        PreferenceConfiguration layered = PreferenceConfiguration.readPreferences(app);
        HostStreamProfile.deserialize("v2;yuv444=true").applyTo(layered);
        assertTrue(layered.enableYuv444 && layered.experimentalPacked444);
        HostStreamProfile.deserialize("v2;yuv444=false").applyTo(layered);
        assertFalse(layered.enableYuv444 || layered.experimentalPacked444);
    }

    /** Controllers: Off, One player or A player each; an older store reads as it streamed. */
    @Test
    public void controllersReadTheOlderSwitchAsItStreamed() {
        Application app = RuntimeEnvironment.getApplication();
        assertEquals(ControllersChoice.PLAYER_EACH, ControllersChoice.read(prefs()));
        prefs().edit().putBoolean("checkbox_multi_controller", false).commit();
        assertEquals("an older off reads as One player", ControllersChoice.ONE_PLAYER, ControllersChoice.read(prefs()));
        assertTrue(PreferenceConfiguration.readPreferences(app).sendControllers);
        assertFalse(PreferenceConfiguration.readPreferences(app).multiController);

        SharedPreferences.Editor editor = prefs().edit();
        ControllersChoice.save(editor, ControllersChoice.OFF);
        editor.commit();
        assertEquals(ControllersChoice.OFF, ControllersChoice.read(prefs()));
        assertFalse(PreferenceConfiguration.readPreferences(app).sendControllers);
        assertFalse("Off keeps the player choice for when controllers come back",
                prefs().getBoolean("checkbox_multi_controller", true));

        editor = prefs().edit();
        ControllersChoice.save(editor, ControllersChoice.PLAYER_EACH);
        editor.commit();
        assertEquals(ControllersChoice.PLAYER_EACH, ControllersChoice.read(prefs()));
        assertTrue(PreferenceConfiguration.readPreferences(app).multiController);
        editor = prefs().edit();
        ControllersChoice.save(editor, ControllersChoice.ONE_PLAYER);
        editor.commit();
        assertEquals(ControllersChoice.ONE_PLAYER, ControllersChoice.read(prefs()));
    }

    /** Variable refresh rate: Off, On or Fastest, read from and written to the two stored switches. */
    @Test
    public void variableRefreshRateIsOneChoiceOverTheTwoSwitches() {
        Application app = RuntimeEnvironment.getApplication();
        // checkbox_vrr, checkbox_real_vrr (null: never stored), the choice shown
        Object[][] cases = {
                {null, null, VrrChoice.OFF}, {false, true, VrrChoice.OFF}, {false, false, VrrChoice.OFF},
                {true, true, VrrChoice.ON}, {true, false, VrrChoice.FASTEST}, {true, null, VrrChoice.ON}};
        for (Object[] c : cases) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            if (c[0] != null) editor.putBoolean("checkbox_vrr", (Boolean) c[0]);
            if (c[1] != null) editor.putBoolean("checkbox_real_vrr", (Boolean) c[1]);
            editor.commit();
            String label = c[0] + "/" + c[1];
            assertEquals(label, c[2], VrrChoice.read(prefs()));
            assertEquals(label, c[2], VrrChoice.shown(prefs(), true));
            assertEquals("where VRR isn't offered every store reads as Off", VrrChoice.OFF, VrrChoice.shown(prefs(), false));
            assertEquals(label + " stays as stored", c[0], prefs().contains("checkbox_vrr") ? prefs().getBoolean("checkbox_vrr", false) : null);
            assertEquals(label + " stays as stored", c[1], prefs().contains("checkbox_real_vrr") ? prefs().getBoolean("checkbox_real_vrr", true) : null);
        }

        prefs().edit().clear().commit();
        SharedPreferences.Editor editor = prefs().edit();
        VrrChoice.save(editor, VrrChoice.FASTEST);
        editor.commit();
        assertTrue(prefs().getBoolean("checkbox_vrr", false));
        assertFalse(prefs().getBoolean("checkbox_real_vrr", true));
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertTrue(config.vrr && !config.realVrr && !config.hostFrameTimes);

        editor = prefs().edit();
        VrrChoice.save(editor, VrrChoice.ON);
        editor.commit();
        assertTrue(prefs().getBoolean("checkbox_vrr", false));
        assertTrue(prefs().getBoolean("checkbox_real_vrr", false));
        config = PreferenceConfiguration.readPreferences(app);
        assertTrue(config.vrr && config.realVrr && config.hostFrameTimes);

        // Off leaves Real VRR as it was, both after On and after Fastest.
        for (int before : new int[] {VrrChoice.ON, VrrChoice.FASTEST}) {
            editor = prefs().edit();
            VrrChoice.save(editor, before);
            editor.commit();
            editor = prefs().edit();
            VrrChoice.save(editor, VrrChoice.OFF);
            editor.commit();
            assertFalse(prefs().getBoolean("checkbox_vrr", true));
            assertEquals(before == VrrChoice.ON, prefs().getBoolean("checkbox_real_vrr", before != VrrChoice.ON));
            assertEquals(VrrChoice.OFF, VrrChoice.read(prefs()));
            assertFalse(PreferenceConfiguration.readPreferences(app).vrr);
        }
        // A PC, game or profile layer streams with both of its values; one saved before keeps the Real VRR below.
        PreferenceConfiguration layered = PreferenceConfiguration.readPreferences(app);
        layered.realVrr = true;
        HostStreamProfile.deserialize("v2;vrr=true;real_vrr=false").applyTo(layered);
        layered.resolveHostFrameTimes();
        assertTrue(layered.vrr && !layered.realVrr && !layered.hostFrameTimes);
        PreferenceConfiguration older = PreferenceConfiguration.readPreferences(app);
        older.realVrr = false;
        HostStreamProfile.deserialize("v2;vrr=true").applyTo(older);
        assertTrue(older.vrr && !older.realVrr);

        assertEquals(java.util.Arrays.asList(VrrChoice.VALUES),
                java.util.Arrays.asList(app.getResources().getStringArray(R.array.vrr_choice_values)));
        assertEquals(VrrChoice.VALUES.length, app.getResources().getStringArray(R.array.vrr_choice_names).length);
    }

    /** When Nightjar opens: the stored pair reads as one choice, and nothing is rewritten on load. */
    @Test
    public void whenNightjarOpensReadsTheStoredPairAsOneChoice() {
        String[][] cases = {
                // list_open_on_start, checkbox_console_home (null: never set), choice
                {null, null, StartScreenChoice.HOME}, {"pcs", "true", StartScreenChoice.HOME},
                {"pcs", "false", StartScreenChoice.PCS}, {null, "false", StartScreenChoice.PCS},
                {"last", "true", StartScreenChoice.LAST}, {"last", "false", StartScreenChoice.LAST},
                {"chosen", "true", StartScreenChoice.CHOSEN}, {"chosen", "false", StartScreenChoice.CHOSEN},
                {"a newer screen", "false", StartScreenChoice.PCS}, {"a newer screen", "true", StartScreenChoice.HOME},
        };
        for (String[] c : cases) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            if (c[0] != null) editor.putString("list_open_on_start", c[0]);
            if (c[1] != null) editor.putBoolean("checkbox_console_home", Boolean.parseBoolean(c[1]));
            editor.commit();
            String name = c[0] + "/" + c[1];
            assertEquals(name, c[2], StartScreenChoice.read(prefs()));
            PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
            assertEquals(name, c[0], prefs().getString("list_open_on_start", null));
        }
        prefs().edit().clear().putBoolean("checkbox_console_home", true).commit();
        SharedPreferences.Editor editor = prefs().edit();
        StartScreenChoice.save(prefs(), editor, StartScreenChoice.LAST);
        editor.commit();
        assertEquals("last", prefs().getString("list_open_on_start", null));
        assertTrue("a game choice keeps Home as it was", prefs().getBoolean("checkbox_console_home", false));
        editor = prefs().edit();
        StartScreenChoice.save(prefs(), editor, StartScreenChoice.PCS);
        editor.commit();
        assertEquals("pcs", prefs().getString("list_open_on_start", null));
        assertFalse(prefs().getBoolean("checkbox_console_home", true));
        assertEquals(StartScreenChoice.PCS, StartScreenChoice.read(prefs()));
    }

    /** The three pickers store under keys that have no row of their own; Reset all settings clears them too. */
    @Test
    public void resetAllPutsThePickerChoicesBack() {
        prefs().edit().putBoolean("checkbox_yuv444", true).putBoolean("experimental_packed_444", true)
                .putBoolean("checkbox_send_controllers", false).putBoolean("checkbox_multi_controller", false)
                .putBoolean("checkbox_console_home", false).putString("list_open_on_start", "last").commit();
        StreamSettings.resetAllSettings(RuntimeEnvironment.getApplication());
        assertFalse(PreferenceConfiguration.sharpColor(prefs()));
        assertEquals(ControllersChoice.PLAYER_EACH, ControllersChoice.read(prefs()));
        assertEquals(StartScreenChoice.HOME, StartScreenChoice.read(prefs()));
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
        // The other latency switches remain independent Advanced controls.
        assertTrue(config.dropLateFrames && config.unbatchedInput && config.networkPriority && config.codecPerformance);
        PreferenceConfiguration.setLowLatencyDecoding(prefs, true);
        assertEquals(3, PreferenceConfiguration.lowLatencyDecodingCount(prefs));
        for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) {
            assertTrue(Arrays.asList(StreamPreset.LATENCY_SWITCHES).contains(key));
        }
    }
    @Test
    public void profileStoreKeysKeepTheirNames() {
        // Named profiles live in their own preferences file; these names are stored on devices.
        assertEquals("StreamProfiles", StreamProfiles.PREFERENCES);
        assertEquals(Arrays.asList("order", "next_id", "name.", "values.", "active", "auto", "rule.mobile",
                "rule.home_wifi", "rule.wifi", "home_wifi"), Arrays.asList(StreamProfiles.ORDER, StreamProfiles.NEXT_ID,
                StreamProfiles.NAME, StreamProfiles.VALUES, StreamProfiles.ACTIVE, StreamProfiles.AUTO,
                StreamProfiles.RULE_MOBILE, StreamProfiles.RULE_HOME_WIFI, StreamProfiles.RULE_WIFI,
                StreamProfiles.HOME_WIFI));
        assertEquals("list_mobile_bitrate_kbps", PreferenceConfiguration.MOBILE_BITRATE_PREF_STRING);
        assertEquals("pc1/uuid:game", AppStreamSettings.lookupKey("pc1", "game", 3));
        assertEquals("pc1/id:3", AppStreamSettings.lookupKey("pc1", null, 3));
    }
}
