//! Generated canonical Nightjar settings catalog shared by desktop and Android.
//! Generated from `client/protocol/policy/settings.toml` by `client/scripts/generate-settings.py`; do not edit by hand.

use std::ops::RangeInclusive;

/// Total Android settings rows documented in `settings.toml`.
pub const ANDROID_SETTING_COUNT: usize = 193;
/// Total Desktop `Settings` fields declared in `settings.toml`.
pub const DESKTOP_SETTING_COUNT: usize = 147;

/// Default Hold Guide to open the stream menu duration in milliseconds.
pub const DEFAULT_GUIDE_MENU_HOLD_MS: u32 = 1000;
/// The choices for holding Guide to open the stream menu, in milliseconds; 0 is off.
pub const GUIDE_HOLD_MS: [u32; 5] = [0, 500, 1000, 1500, 2000];
/// Windows virtual display scales offered in percent; 0 keeps the PC's own setting.
pub const DISPLAY_SCALES: [u32; 13] = [
    0,
    100,
    125,
    150,
    175,
    200,
    225,
    250,
    300,
    350,
    400,
    450,
    500,
];
/// Desktop launcher UI zoom steps in percent (0 is Auto).
pub const UI_ZOOM_STEPS: [u32; 7] = [85, 100, 115, 130, 150, 200, 250];
/// Fixed video FEC percentages offered in settings.
pub const ERROR_CORRECTION_PERCENTAGES: [u8; 4] = [10, 20, 30, 50];
/// Default decode unit queue bound in frames.
pub const DECODE_QUEUE_FRAMES_DEFAULT: u32 = 15;
/// Shared adaptive quality minimum short-edge resolution choices.
pub const QUALITY_MIN_SHORT_EDGES: [u32; 5] = [360, 540, 720, 1080, 1440];
/// Shared adaptive quality minimum FPS choices.
pub const QUALITY_MIN_FPS: [u32; 5] = [30, 45, 60, 90, 120];
/// Shared network dropout grace period choices in seconds.
pub const NETWORK_GRACE_CHOICES_S: [u32; 5] = [0, 15, 20, 30, 60];
/// The PC render resolution range in percent.
pub const RENDER_SCALE: RangeInclusive<u32> = 50..=200;
/// The global stick dead zone range in percent.
pub const DEADZONE: RangeInclusive<u32> = 0..=50;
/// The microphone gain range in percent.
pub const MIC_GAIN: RangeInclusive<u32> = 25..=400;
/// The microphone gain slider step in percent.
pub const MIC_GAIN_STEP: u32 = 5;
/// The desktop keys holding Sharper colored text.
pub const SHARP_COLOR_DESKTOP_KEYS: [&str; 2] = ["prefer_yuv444", "experimental_packed_444"];
/// The Android keys holding Sharper colored text.
pub const SHARP_COLOR_ANDROID_KEYS: [&str; 2] = ["checkbox_yuv444", "experimental_packed_444"];

/// A retired preference/config key that must never be reused with another meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetiredKey {
    pub key: &'static str,
    pub replacement: &'static str,
}

/// A setting present on both Android and Desktop clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedSetting {
    pub android_key: &'static str,
    pub desktop_key: &'static str,
    pub tier: &'static str,
    pub scope: &'static str,
}

/// Retired Android preference keys.
pub const ANDROID_RETIRED_KEYS: &[RetiredKey] = &[
    RetiredKey { key: "checkbox_gl_render", replacement: "checkbox_vulkan_render" },
    RetiredKey { key: "checkbox_auto_bitrate", replacement: "adaptive_bitrate" },
    RetiredKey { key: "adaptive_bitrate", replacement: "lower_bitrate" },
    RetiredKey { key: "checkbox_rate_change_bitrate", replacement: "lower_bitrate" },
    RetiredKey { key: "keep_bitrate", replacement: "lower_bitrate" },
    RetiredKey { key: "checkbox_trackpad_acceleration", replacement: "list_trackpad_acceleration_curve" },
    RetiredKey { key: "checkbox_disable_rumble", replacement: "list_rumble_feel" },
    RetiredKey { key: "checkbox_trackpad_acceleration_v2", replacement: "list_trackpad_acceleration_curve" },
    RetiredKey { key: "checkbox_trackpad_momentum", replacement: "list_trackpad_momentum_glide" },
    RetiredKey { key: "checkbox_always_dark", replacement: "theme" },
    RetiredKey { key: "checkbox_oled_black", replacement: "theme" },
    RetiredKey { key: "list_osc_style", replacement: "" },
    RetiredKey { key: "checkbox_rust_engine", replacement: "" },
    RetiredKey { key: "rust_engine_default_on", replacement: "" },
    RetiredKey { key: "checkbox_pyrowave_front_buffer", replacement: "" },
    RetiredKey { key: "list_auto_bitrate_min_kbps", replacement: "" },
    RetiredKey { key: "list_auto_bitrate_max_kbps", replacement: "" },
];

/// Retired Desktop JSON setting keys.
pub const DESKTOP_RETIRED_KEYS: &[RetiredKey] = &[
    RetiredKey { key: "adaptive_bitrate", replacement: "lower_bitrate" },
    RetiredKey { key: "client_hint_bitrate", replacement: "lower_bitrate" },
    RetiredKey { key: "keep_bitrate", replacement: "lower_bitrate" },
];

/// Renamed Android preference keys `(old, new)`.
pub const ANDROID_RENAMED_KEYS: &[(&str, &str)] = &[
    ("checkbox_auto_bitrate", "adaptive_bitrate"),
];

/// Cross-platform settings shared between Android and Desktop.
pub const SHARED_SETTINGS: &[SharedSetting] = &[
    SharedSetting {
        android_key: "simple_smoothness",
        desktop_key: "simple_smoothness",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "simple_picture",
        desktop_key: "simple_picture",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "simple_hdr",
        desktop_key: "simple_hdr",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_console_home",
        desktop_key: "console_home",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_resolution",
        desktop_key: "width",
        tier: "main",
        scope: "layer",
    },
    SharedSetting { android_key: "list_fps", desktop_key: "fps", tier: "main", scope: "layer" },
    SharedSetting {
        android_key: "seekbar_bitrate_kbps",
        desktop_key: "bitrate_kbps",
        tier: "main",
        scope: "layer",
    },
    SharedSetting {
        android_key: "lower_bitrate",
        desktop_key: "lower_bitrate",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "video_format",
        desktop_key: "codec",
        tier: "main",
        scope: "layer",
    },
    SharedSetting {
        android_key: "checkbox_enable_hdr",
        desktop_key: "hdr",
        tier: "main",
        scope: "layer",
    },
    SharedSetting {
        android_key: "sharp_color",
        desktop_key: "prefer_yuv444",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "checkbox_enable_sdr_10bit",
        desktop_key: "sdr_10bit",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_frame_warp",
        desktop_key: "frame_warp",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "adaptive_quality_mode",
        desktop_key: "adaptive_quality",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "adaptive_quality_min_resolution",
        desktop_key: "quality_min_short_edge",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "adaptive_quality_min_fps",
        desktop_key: "quality_min_fps",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "video_scaling",
        desktop_key: "video_scaling",
        tier: "main",
        scope: "global",
    },
    SharedSetting { android_key: "checkbox_vrr", desktop_key: "vrr", tier: "main", scope: "layer" },
    SharedSetting {
        android_key: "frame_pacing",
        desktop_key: "frame_pacing",
        tier: "main",
        scope: "layer",
    },
    SharedSetting {
        android_key: "checkbox_fractional_vsync",
        desktop_key: "fractional_vsync",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "upscaling_mode",
        desktop_key: "upscaling",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "upscaling_sharpness",
        desktop_key: "upscale_sharpness",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "video_alignment",
        desktop_key: "align_video_top",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_full_range",
        desktop_key: "full_range",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "host_display_location",
        desktop_key: "host_display",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "seekbar_virtual_display_scale",
        desktop_key: "virtual_display_scale",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "list_virtual_display_dpi",
        desktop_key: "virtual_display_dpi",
        tier: "more",
        scope: "layer",
    },
    SharedSetting {
        android_key: "checkbox_absolute_mouse_mode",
        desktop_key: "mouse_mode",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "controllers",
        desktop_key: "controllers",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_keyboard_follows_text_fields",
        desktop_key: "keyboard_follows_text_fields",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_clipboard_sync",
        desktop_key: "clipboard_sync",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_flip_face_buttons",
        desktop_key: "swap_face_buttons",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_gamepad_quit_combo",
        desktop_key: "gamepad_quit_combo",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "seekbar_deadzone",
        desktop_key: "deadzone",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "seekbar_trigger_deadzone",
        desktop_key: "trigger_deadzone",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_gamepad_motion_sensors",
        desktop_key: "controller_motion",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_rumble_feel",
        desktop_key: "rumble_feel",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_mouse_emulation",
        desktop_key: "controller_mouse",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "analog_scrolling",
        desktop_key: "analog_scrolling",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_mouse_emulation_fixed_step",
        desktop_key: "mouse_fixed_step",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_gamepad_touchpad_as_mouse",
        desktop_key: "gamepad_touchpad_as_mouse",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_guide_menu_hold",
        desktop_key: "guide_hold_ms",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "back_button",
        desktop_key: "back_opens_menu",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "seekbar_mouse_speed",
        desktop_key: "mouse_speed",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_mouse_nav_buttons",
        desktop_key: "mouse_nav_buttons",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_native_touchpad",
        desktop_key: "native_touchpad",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_capture_system_keys",
        desktop_key: "capture_system_keys",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_right_alt_as_meta",
        desktop_key: "right_alt_as_meta",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_usb_dualsense_haptics",
        desktop_key: "controller_audio",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_usb_forwarding",
        desktop_key: "usb_forwarding",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_audio_config",
        desktop_key: "audio",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_host_audio",
        desktop_key: "play_audio_on_host",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_enable_mic",
        desktop_key: "microphone",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_mic_echo_cancel",
        desktop_key: "mic_echo_cancel",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_mic_noise_suppression",
        desktop_key: "mic_noise_suppression",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_mic_auto_gain",
        desktop_key: "mic_auto_gain",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "seekbar_mic_gain",
        desktop_key: "mic_gain",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_mute_audio",
        desktop_key: "mute_audio",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_audio_passthrough",
        desktop_key: "audio_passthrough",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_enable_perf_overlay",
        desktop_key: "overlay",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "performance_overlay_position",
        desktop_key: "overlay_position",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_auto_reconnect",
        desktop_key: "auto_reconnect",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_client_roaming",
        desktop_key: "client_roaming",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_network_grace",
        desktop_key: "network_grace_ms",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_receive_coalescing",
        desktop_key: "receive_coalescing",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "jumbo_frames",
        desktop_key: "jumbo_frames",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_internet_play",
        desktop_key: "internet_play",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_disable_warnings",
        desktop_key: "hide_connection_warnings",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_dual_screen",
        desktop_key: "dual_screen",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_resolution_screen2",
        desktop_key: "second_screen_width",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_input_only_any_host",
        desktop_key: "input_only_any_host",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_compact_overlay_device_info",
        desktop_key: "compact_overlay_device_info",
        tier: "more",
        scope: "global",
    },
    SharedSetting { android_key: "theme", desktop_key: "theme", tier: "main", scope: "global" },
    SharedSetting {
        android_key: "list_languages",
        desktop_key: "language",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_open_on_start",
        desktop_key: "open_on_start",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_quick_resume",
        desktop_key: "quick_resume",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_check_updates",
        desktop_key: "check_updates",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_small_icon_mode",
        desktop_key: "home_card_size",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_show_debug_settings",
        desktop_key: "show_debug_settings",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_ui_scale",
        desktop_key: "ui_zoom",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_high_contrast",
        desktop_key: "high_contrast",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_reduce_motion",
        desktop_key: "reduce_motion",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_enable_sops",
        desktop_key: "sops",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_decode_queue_frames",
        desktop_key: "decode_queue_frames",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "list_fec_choice",
        desktop_key: "error_correction",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_tell_host_wireless",
        desktop_key: "tell_host_wireless",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_network_priority",
        desktop_key: "network_priority",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_unbatched_input",
        desktop_key: "unbatched_input",
        tier: "main",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_link_probe",
        desktop_key: "link_probe",
        tier: "more",
        scope: "global",
    },
    SharedSetting {
        android_key: "checkbox_enable_post_stream_toast",
        desktop_key: "post_stream_decode_time",
        tier: "more",
        scope: "global",
    },
];

/// Returns whether `key` is a retired Android preference key.
pub fn is_android_retired(key: &str) -> bool {
    ANDROID_RETIRED_KEYS.iter().any(|r| r.key == key)
}

/// Returns whether `key` is a retired Desktop JSON setting key.
pub fn is_desktop_retired(key: &str) -> bool {
    DESKTOP_RETIRED_KEYS.iter().any(|r| r.key == key)
}
