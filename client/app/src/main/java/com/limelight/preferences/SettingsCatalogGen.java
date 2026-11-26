package com.limelight.preferences;

/**
 * Generated canonical Android preference keys, scalar defaults, value lists, and retired/renamed tables.
 * Generated from {@code client/protocol/policy/settings.toml} by {@code client/scripts/generate-settings.py}; do not edit by hand.
 */
public class SettingsCatalogGen {
    SettingsCatalogGen() {}

    static final String LEGACY_RES_FPS_PREF_STRING = "list_resolution_fps";
    static final String LEGACY_ENABLE_51_SURROUND_PREF_STRING = "checkbox_51_surround";
    static final String RESOLUTION_PREF_STRING = "list_resolution";
    static final String FPS_PREF_STRING = "list_fps";
    static final String BITRATE_PREF_STRING = "seekbar_bitrate_kbps";
    static final String BITRATE_USER_SET_PREF_STRING = "bitrate_user_set";
    static final String MOBILE_BITRATE_PREF_STRING = "list_mobile_bitrate_kbps";
    static final String PREFS_VERSION_PREF_STRING = "prefs_version";
    static final int NATIVE_DEFAULTS_VERSION = 2;
    static final int FOLLOW_SCREEN_VERSION = 3;
    static final int PROFILE_NATIVE_VERSION = 4;
    static final String RESOLUTION_FOLLOWS_SCREEN_PREF_STRING = "resolution_follows_screen";
    static final String BITRATE_PREF_OLD_STRING = "seekbar_bitrate";
    static final String STRETCH_PREF_STRING = "checkbox_stretch_video";
    static final String SOPS_PREF_STRING = "checkbox_enable_sops";
    static final String DISABLE_TOASTS_PREF_STRING = "checkbox_disable_warnings";
    static final String HOST_AUDIO_PREF_STRING = "checkbox_host_audio";
    public static final String ENABLE_MIC_PREF_STRING = "checkbox_enable_mic";
    static final String MIC_ECHO_PREF_STRING = "list_mic_echo_cancel";
    public static final String MIC_ECHO_ENGINE_PREF_STRING = "debug_mic_echo_engine";
    static final String MIC_NOISE_PREF_STRING = "checkbox_mic_noise_suppression";
    static final String MIC_AUTO_GAIN_PREF_STRING = "checkbox_mic_auto_gain";
    static final String MIC_GAIN_PREF_STRING = "seekbar_mic_gain";
    public static final String MIC_BUTTON_PREF_STRING = "checkbox_mic_button";
    public static final String FLOATING_MENU_BUTTON_PREF_STRING = "checkbox_floating_menu_button";
    static final String RETIRED_OSC_STYLE_PREF_STRING = "list_osc_style";
    public static final String OSC_STICK_MODE_PREF_STRING = "list_osc_stick_mode";
    public static final String OSC_STICK_DEADZONE_PREF_STRING = "seekbar_osc_stick_deadzone";
    public static final String OSC_AUTO_HIDE_PREF_STRING = "checkbox_osc_auto_hide";
    public static final String OSC_STICK_FLOATING = "floating";
    public static final String OSC_STICK_FIXED = "fixed";
    public static final int DEFAULT_OSC_STICK_DEADZONE = 8;
    public static final int MAX_OSC_STICK_DEADZONE = 30;
    static final String DEADZONE_PREF_STRING = "seekbar_deadzone";
    static final String TRIGGER_DEADZONE_PREF_STRING = "seekbar_trigger_deadzone";
    public static final int DEFAULT_TRIGGER_DEADZONE = 2;
    public static final int MAX_TRIGGER_DEADZONE = 30;
    static final String OSC_OPACITY_PREF_STRING = "seekbar_osc_opacity";
    static final String LANGUAGE_PREF_STRING = "list_languages";
    static final String SMALL_ICONS_PREF_STRING = "checkbox_small_icon_mode";
    static final String MULTI_CONTROLLER_PREF_STRING = "checkbox_multi_controller";
    static final String SEND_CONTROLLERS_PREF_STRING = "checkbox_send_controllers";
    static final String AUDIO_CONFIG_PREF_STRING = "list_audio_config";
    static final String AUDIO_CONFIG_714 = "714";
    static final String USB_DRIVER_PREF_SRING = "checkbox_usb_driver";
    static final String USB_DUALSENSE_PREF_STRING = "checkbox_usb_dualsense";
    static final String USB_SWITCH_PRO_PREF_STRING = "checkbox_usb_switch_pro";
    static final String USB_FORWARDING_PREF_STRING = "checkbox_usb_forwarding";
    static final String VIDEO_FORMAT_PREF_STRING = "video_format";
    static final String ONSCREEN_CONTROLLER_PREF_STRING = "checkbox_show_onscreen_controls";
    static final String ONLY_L3_R3_PREF_STRING = "checkbox_only_show_L3R3";
    static final String ONSCREEN_OWN_PLAYER_PREF_STRING = "checkbox_onscreen_own_player";
    static final String SHOW_GUIDE_BUTTON_PREF_STRING = "checkbox_show_guide_button";
    static final String SHOW_TOUCHPAD_BUTTON_PREF_STRING = "checkbox_show_touchpad_button";
    public static final String OSC_STEAM_CONTROLS_PREF_STRING = "checkbox_osc_steam_controls";
    static final String LEGACY_DISABLE_FRAME_DROP_PREF_STRING = "checkbox_disable_frame_drop";
    static final String ENABLE_HDR_PREF_STRING = "checkbox_enable_hdr";
    static final String THERMAL_PROTECTION_PREF_STRING = "checkbox_thermal_protection";
    static final String ENABLE_SDR10_PREF_STRING = "checkbox_enable_sdr_10bit";
    static final String ENABLE_PIP_PREF_STRING = "checkbox_enable_pip";
    static final String AUTO_RECONNECT_PREF_STRING = "checkbox_auto_reconnect";
    static final String NETWORK_GRACE_PREF_STRING = "list_network_grace";
    static final String TELL_HOST_WIRELESS_PREF_STRING = "checkbox_tell_host_wireless";
    static final String TELL_HOST_LINK_SPEED_PREF_STRING = "checkbox_tell_host_link_speed";
    static final String FEC_CHOICE_PREF_STRING = "list_fec_choice";
    static final String LINK_PROBE_PREF_STRING = "checkbox_link_probe";
    static final String QUICK_RESUME_PREF_STRING = "checkbox_quick_resume";
    static final String ENABLE_PERF_OVERLAY_STRING = "checkbox_enable_perf_overlay";
    static final String BIND_ALL_USB_STRING = "checkbox_usb_bind_all";
    static final String MOUSE_EMULATION_STRING = "checkbox_mouse_emulation";
    static final String MOUSE_EMULATION_FIXED_STEP_STRING = "checkbox_mouse_emulation_fixed_step";
    static final String MOUSE_EMULATION_SENSITIVITY_PREF_STRING = "seekbar_mouse_emulation_sensitivity";
    static final String BACK_AS_META_PREF_STRING = "checkbox_back_as_meta";
    static final String BACK_OPENS_MENU_PREF_STRING = "checkbox_back_opens_menu";
    static final String GAMEPAD_QUIT_COMBO_PREF_STRING = "list_gamepad_quit_combo";
    static final String BACK_AS_GUIDE_PREF_STRING = "checkbox_back_as_guide";
    static final String GUIDE_MENU_HOLD_PREF_STRING = "list_guide_menu_hold";
    public static final int DEFAULT_GUIDE_MENU_HOLD_MS = 1000;
    static final String CAPTURE_SYSTEM_KEYS_PREF_STRING = "checkbox_capture_system_keys";
    static final String SYSTEM_KEYS_FULLSCREEN_ONLY_PREF_STRING = "checkbox_system_keys_fullscreen_only";
    static final String RIGHT_ALT_AS_META_PREF_STRING = "checkbox_right_alt_as_meta";
    static final String KEYBOARD_FOLLOWS_TEXT_FIELDS_PREF_STRING = "checkbox_keyboard_follows_text_fields";
    static final String CLIPBOARD_SYNC_PREF_STRING = "checkbox_clipboard_sync";
    static final String ANALOG_SCROLLING_PREF_STRING = "analog_scrolling";
    static final String MOUSE_NAV_BUTTONS_STRING = "checkbox_mouse_nav_buttons";
    static final String INPUT_ONLY_ANY_HOST_PREF_STRING = "checkbox_input_only_any_host";
    static final String UNLOCK_FPS_STRING = "checkbox_unlock_fps";
    static final String VIBRATE_OSC_PREF_STRING = "checkbox_vibrate_osc";
    static final String VIBRATE_FALLBACK_PREF_STRING = "checkbox_vibrate_fallback";
    static final String VIBRATE_FALLBACK_STRENGTH_PREF_STRING = "seekbar_vibrate_fallback_strength";
    static final String FLIP_FACE_BUTTONS_PREF_STRING = "checkbox_flip_face_buttons";
    static final String TOUCHSCREEN_TRACKPAD_PREF_STRING = "checkbox_touchscreen_trackpad";
    static final String TRACKPAD_SPEED_PREF_STRING = "seekbar_trackpad_speed";
    static final String LOCAL_CURSOR_SPEED_PREF_STRING = "seekbar_local_cursor_speed";
    static final String TRACKPAD_AREA_PREF_STRING = "list_trackpad_area";
    static final String THREE_FINGER_TAP_PREF_STRING = "list_three_finger_tap";
    static final String FOUR_FINGER_TAP_PREF_STRING = "list_four_finger_tap";
    static final String MULTI_TOUCH_FINGER_TAPS_PREF_STRING = "checkbox_multi_touch_finger_taps";
    static final String FINGER_TAP_ON_CANCEL_PREF_STRING = "checkbox_finger_tap_on_cancel";
    static final String PC_KEYBOARD_LAYOUT_PREF_STRING = "list_pc_keyboard_layout";
    static final String PC_KEYBOARD_SIZE_PREF_STRING = "seekbar_pc_keyboard_size";
    static final String PC_KEYBOARD_OPACITY_PREF_STRING = "seekbar_pc_keyboard_opacity";
    static final String MOUSE_SPEED_PREF_STRING = "seekbar_mouse_speed";
    static final String TRACKPAD_SENSITIVITY_X_PREF_STRING = "seekbar_trackpad_sensitivity_x";
    static final String TRACKPAD_SENSITIVITY_Y_PREF_STRING = "seekbar_trackpad_sensitivity_y";
    static final String TRACKPAD_SWAP_AXIS_PREF_STRING = "checkbox_trackpad_swap_axis";
    static final String TRACKPAD_SCROLL_SPEED_PREF_STRING = "seekbar_trackpad_scroll_speed";
    static final String NATIVE_TOUCHPAD_PREF_STRING = "checkbox_native_touchpad";
    static final String LATENCY_TOAST_PREF_STRING = "checkbox_enable_post_stream_toast";
    static final String RETIRED_RUST_ENGINE_PREF_STRING = "checkbox_rust_engine";
    static final String RETIRED_RUST_ENGINE_DEFAULT_ON_PREF_STRING = "rust_engine_default_on";
    static final String PACKED_444_PREF_STRING = "experimental_packed_444";
    static final String FRAME_WARP_PREF_STRING = "checkbox_frame_warp";
    static final String CHIPSET_TUNING_PREF_STRING = "checkbox_experimental_chipset_tuning";
    static final String TV_DISPLAY_COMPATIBILITY_PREF_STRING = "checkbox_tv_display_compatibility";
    static final String FRAME_PACING_PREF_STRING = "frame_pacing";
    static final String ABSOLUTE_MOUSE_MODE_PREF_STRING = "checkbox_absolute_mouse_mode";
    static final String ENABLE_AUDIO_FX_PREF_STRING = "checkbox_enable_audiofx";
    static final String AUDIO_PASSTHROUGH_PREF_STRING = "checkbox_audio_passthrough";
    static final String MUTE_AUDIO_PREF_STRING = "checkbox_mute_audio";
    static final String DUAL_SCREEN_PREF_STRING = "checkbox_dual_screen";
    static final String SECOND_SCREEN_RESOLUTION_PREF_STRING = "list_resolution_screen2";
    public static final String SECOND_SCREEN_NATIVE = "native";
    static final String REDUCE_REFRESH_RATE_PREF_STRING = "checkbox_reduce_refresh_rate";
    static final String FULL_RANGE_PREF_STRING = "checkbox_full_range";
    static final String GAMEPAD_TOUCHPAD_AS_MOUSE_PREF_STRING = "checkbox_gamepad_touchpad_as_mouse";
    static final String GAMEPAD_MOTION_SENSORS_PREF_STRING = "checkbox_gamepad_motion_sensors";
    static final String GAMEPAD_MOTION_FALLBACK_PREF_STRING = "checkbox_gamepad_motion_fallback";
    static final String CODEC_LOW_LATENCY_PREF_STRING = "checkbox_codec_low_latency";
    static final String VENDOR_LOW_LATENCY_PREF_STRING = "checkbox_vendor_low_latency";
    public static final String DISABLE_HEVC_LOW_LATENCY_PREF_STRING = "checkbox_disable_hevc_low_latency";
    public static final String HEVC_LOW_LATENCY_MODE_PREF_STRING = "list_hevc_low_latency_mode";
    public static final int HEVC_LOW_LATENCY_AUTO = 0;
    public static final int HEVC_LOW_LATENCY_ON = 1;
    public static final int HEVC_LOW_LATENCY_OFF = 2;
    static final String CODEC_PERFORMANCE_PREF_STRING = "checkbox_codec_performance";
    static final String PHONE_PERFORMANCE_HINTS_PREF_STRING = "checkbox_phone_performance_hints";
    static final String GPU_MAX_CLOCKS_PREF_STRING = "checkbox_gpu_max_clocks";
    static final boolean DEFAULT_GPU_MAX_CLOCKS = false;
    static final String DROP_LATE_FRAMES_PREF_STRING = "checkbox_drop_late_frames";
    static final String FRACTIONAL_VSYNC_PREF_STRING = "checkbox_fractional_vsync";
    static final boolean DEFAULT_FRACTIONAL_VSYNC = false;
    static final String DISPLAY_QUEUE_RECOVERY_PREF_STRING = "checkbox_display_queue_recovery";
    static final String TEXTURE_VIEW_PREF_STRING = "checkbox_texture_view";
    static final String UNBATCHED_INPUT_PREF_STRING = "checkbox_unbatched_input";
    static final String NETWORK_PRIORITY_PREF_STRING = "checkbox_network_priority";
    static final String SUBMIT_HINT_PREF_STRING = "checkbox_submit_hint";
    static final String COVERING_DECODER_SIZE_PREF_STRING = "checkbox_covering_decoder_size";
    static final String PYROWAVE_PREACQUIRE_PREF_STRING = "checkbox_pyrowave_preacquire";
    static final String CODEC_DIRECT_PREF_STRING = "checkbox_codec_direct";
    static final String RECEIVE_DECODE_PRIORITY_PREF_STRING = "checkbox_receive_decode_priority";
    static final String CLIENT_ROAMING_PREF_STRING = "checkbox_client_roaming";
    static final String ROAMING_METERED_PREF_STRING = "checkbox_roaming_metered";
    static final String RECEIVE_COALESCING_PREF_STRING = "checkbox_receive_coalescing";
    static final String JUMBO_FRAMES_PREF_STRING = "jumbo_frames";
    static final String DECODE_QUEUE_FRAMES_PREF_STRING = "list_decode_queue_frames";
    static final int DEFAULT_DECODE_QUEUE_FRAMES = 15;
    public static final String INTERNET_PLAY_PREF_STRING = "checkbox_internet_play";
    static final String VIRTUAL_DISPLAY_PREF_STRING = "checkbox_virtual_display";
    static final String VIRTUAL_DISPLAY_SCALE_PREF_STRING = "seekbar_virtual_display_scale";
    static final String VIRTUAL_DISPLAY_DPI_PREF_STRING = "list_virtual_display_dpi";
    static final String YUV444_PREF_STRING = "checkbox_yuv444";
    static final String VRR_PREF_STRING = "checkbox_vrr";
    static final String HOST_FRAME_TIMES_PREF_STRING = "checkbox_host_frame_times";
    static final String REAL_VRR_PREF_STRING = "checkbox_real_vrr";
    static final String CONTENT_RATE_FRAME_RATE_PREF_STRING = "checkbox_content_rate_frame_rate";
    static final String ARR_FIXED_TIMING_PREF_STRING = "checkbox_arr_fixed_timing";
    static final String UPSCALING_PREF_STRING = "upscaling_mode";
    static final String UPSCALING_SHARPNESS_PREF_STRING = "upscaling_sharpness";
    static final String RETIRED_GL_RENDER_PREF_STRING = "checkbox_gl_render";
    static final String VULKAN_RENDER_PREF_STRING = "checkbox_vulkan_render";
    static final String DEBUG_HDR_HLG_ONLY_PREF_STRING = "debug_hdr_hlg_only";
    static final String DEFAULT_RESOLUTION = "1280x720";
    static final String DEFAULT_FPS = "60";
    static final boolean DEFAULT_STRETCH = false;
    static final boolean DEFAULT_SOPS = true;
    static final boolean DEFAULT_DISABLE_TOASTS = false;
    static final boolean DEFAULT_HOST_AUDIO = false;
    static final boolean DEFAULT_ENABLE_MIC = false;
    static final int DEFAULT_DEADZONE = 7;
    static final int DEFAULT_OPACITY = 90;
    public static final String DEFAULT_LANGUAGE = "default";
    static final boolean DEFAULT_MULTI_CONTROLLER = true;
    static final boolean DEFAULT_USB_DRIVER = true;
    static final String DEFAULT_VIDEO_FORMAT = "auto";
    static final boolean ONSCREEN_CONTROLLER_DEFAULT = false;
    static final boolean ONLY_L3_R3_DEFAULT = false;
    static final boolean ONSCREEN_OWN_PLAYER_DEFAULT = false;
    static final boolean OSC_AUTO_HIDE_DEFAULT = true;
    static final boolean SHOW_GUIDE_BUTTON_DEFAULT = true;
    static final boolean SHOW_TOUCHPAD_BUTTON_DEFAULT = false;
    static final boolean OSC_STEAM_CONTROLS_DEFAULT = false;
    static final boolean DEFAULT_ENABLE_HDR = false;
    static final boolean DEFAULT_ENABLE_SDR10 = false;
    static final boolean DEFAULT_ENABLE_PIP = false;
    static final boolean DEFAULT_AUTO_RECONNECT = true;
    static final boolean DEFAULT_ENABLE_PERF_OVERLAY = false;
    static final boolean DEFAULT_BIND_ALL_USB = false;
    static final boolean DEFAULT_MOUSE_EMULATION = true;
    static final int DEFAULT_MOUSE_EMULATION_SENSITIVITY = 100;
    static final String DEFAULT_ANALOG_STICK_FOR_SCROLLING = "right";
    static final boolean DEFAULT_MOUSE_NAV_BUTTONS = false;
    static final boolean DEFAULT_INPUT_ONLY_ANY_HOST = false;
    static final boolean DEFAULT_UNLOCK_FPS = false;
    static final boolean DEFAULT_VIBRATE_OSC = true;
    static final boolean DEFAULT_VIBRATE_FALLBACK = false;
    static final int DEFAULT_VIBRATE_FALLBACK_STRENGTH = 100;
    static final boolean DEFAULT_FLIP_FACE_BUTTONS = false;
    static final boolean DEFAULT_TOUCHSCREEN_TRACKPAD = true;
    static final int DEFAULT_TRACKPAD_SPEED = 100;
    static final int DEFAULT_MOUSE_SPEED = 100;
    static final int DEFAULT_TRACKPAD_SENSITIVITY = 100;
    static final int DEFAULT_TRACKPAD_SCROLL_SPEED = 50;
    static final String DEFAULT_AUDIO_CONFIG = "2";
    static final boolean DEFAULT_LATENCY_TOAST = false;
    static final String DEFAULT_FRAME_PACING = "latency";
    static final boolean DEFAULT_ABSOLUTE_MOUSE_MODE = false;
    static final boolean DEFAULT_ENABLE_AUDIO_FX = false;
    static final boolean DEFAULT_REDUCE_REFRESH_RATE = false;
    static final boolean DEFAULT_FULL_RANGE = false;
    static final boolean DEFAULT_GAMEPAD_TOUCHPAD_AS_MOUSE = false;
    static final boolean DEFAULT_GAMEPAD_MOTION_SENSORS = true;
    static final boolean DEFAULT_GAMEPAD_MOTION_FALLBACK = false;
    public static final int FRAME_PACING_MIN_LATENCY = 0;
    public static final int FRAME_PACING_BALANCED = 1;
    public static final int FRAME_PACING_CAP_FPS = 2;
    public static final int FRAME_PACING_MAX_SMOOTHNESS = 3;
    public static final String RES_360P = "640x360";
    public static final String RES_480P = "854x480";
    public static final String RES_720P = "1280x720";
    public static final String RES_1080P = "1920x1080";
    public static final String RES_1440P = "2560x1440";
    public static final String RES_4K = "3840x2160";
    public static final String RES_NATIVE = "Native";
    static final String PC_PROFILES_OVERRIDES_PREF_STRING = "pc_profiles_overrides";
    static final String AUTO_RECONNECT_DEFAULT_ON_PREF_STRING = "auto_reconnect_default_on";
    static final String RECEIVE_DECODE_PRIORITY_DEFAULT_ON_PREF_STRING = "receive_decode_priority_default_on";
    static final String SUBMIT_HINT_DEFAULT_ON_PREF_STRING = "submit_hint_default_on";
    static final String CODEC_LOW_LATENCY_CORRUPTION_OFF_PREF_STRING = "codec_low_latency_corruption_off";
    static final String OVERLAY_MODE_PREF_STRING = "performance_overlay_mode";
    static final String OVERLAY_POSITION_PREF_STRING = "performance_overlay_position";
    static final String COMPACT_OVERLAY_DEVICE_INFO_PREF_STRING = "checkbox_compact_overlay_device_info";
    static final String OVERLAY_COMPACT_DEFAULT_PREF_STRING = "performance_overlay_compact_default";

    static final String[] REMOVED_AUTO_BITRATE_LIMIT_PREFS = {"list_auto_bitrate_min_kbps", "list_auto_bitrate_max_kbps"};
    static final int[] VIRTUAL_DISPLAY_DPI_VALUES = {0, 100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500};
    static final int[] QUALITY_MIN_SHORT_EDGES = {360, 540, 720, 1080, 1440};
    static final int[] QUALITY_MIN_FPS = {30, 45, 60, 90, 120};
    static final String[] LOW_LATENCY_DECODING_KEYS = {
            "checkbox_codec_low_latency", "checkbox_vendor_low_latency", "checkbox_phone_performance_hints",
    };
    static final String[][] RENAMED_PREFS = {
            {"checkbox_auto_bitrate", "adaptive_bitrate"},
    };
    static final String[] RETIRED_KEYS = {
            "checkbox_gl_render",
            "checkbox_auto_bitrate",
            "adaptive_bitrate",
            "checkbox_rate_change_bitrate",
            "keep_bitrate",
            "checkbox_trackpad_acceleration",
            "checkbox_disable_rumble",
            "checkbox_trackpad_acceleration_v2",
            "checkbox_trackpad_momentum",
            "checkbox_always_dark",
            "checkbox_oled_black",
            "list_osc_style",
            "checkbox_rust_engine",
            "rust_engine_default_on",
    };
}
