//! Generated host settings catalog: overrides, 3.0 list, restart keys, typed accessors, and save validation.
//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.

use super::{
    BitrateAdaptation, Config, DisplayAdjust, MAX_STREAM_RESOLUTION, RemoveRemoteMonitor,
    TouchMode, UpdatesMode, invalid, parse_boolean, parse_integer, parse_max_stream_resolution,
    unquote,
};
use anyhow::{Result, bail};
use std::path::PathBuf;

/// Hardware metrics the host may report (`hardware_metrics_selection`).
pub const HARDWARE_METRICS: &[&str] = &[
    "cpu_usage_percent",
    "gpu_usage_percent",
    "gpu_encoder_percent",
    "gpu_clock_mhz",
    "gpu_memory_clock_mhz",
    "gpu_temperature_c",
    "gpu_hotspot_temperature_c",
    "gpu_memory_temperature_c",
    "gpu_voltage_mv",
    "gpu_fan_rpm",
    "gpu_fan_percent",
    "gpu_power_w",
    "gpu_board_power_w",
    "gpu_vram_used_mb",
    "gpu_vram_usage_percent",
];

/// Keys a device or an app may override (`override_allowed`).
pub const OVERRIDE_ALLOWED: &[&str] = &[
    "controller",
    "gamepad",
    "ds4_back_as_touchpad_click",
    "motion_as_ds4",
    "touchpad_as_ds4",
    "back_button_timeout",
    "back_grip_r4",
    "back_grip_l4",
    "back_grip_r5",
    "back_grip_l5",
    "steam_deck_controller",
    "keyboard",
    "key_repeat_delay",
    "key_repeat_frequency",
    "always_send_scancodes",
    "key_rightalt_to_key_win",
    "mouse",
    "high_resolution_scrolling",
    "absolute_left_release_delay",
    "native_pen_touch",
    "mouse_mode",
    "native_touchpad",
    "keybindings",
    "couch_menu",
    "couch_menu_open_with",
    "couch_menu_controller_chord",
    "couch_menu_hold_ms",
    "couch_menu_corner_hold_ms",
    "couch_menu_keyboard_chord",
    "audio_haptics",
    "audio_haptics_strength",
    "audio_haptics_curve",
    "audio_haptics_gate",
    "dualsense_haptics_gain",
    "audio_sink",
    "audio_capture_mode",
    "audio_sink_capture_only",
    "audio_night_mode",
    "audio_headphone_surround",
    "virtual_sink",
    "stream_audio",
    "stream_mic",
    "adapter_name",
    "adapter_pnp_id",
    "dd_configuration_option",
    "dd_resolution_option",
    "dd_manual_resolution",
    "dd_refresh_rate_option",
    "dd_manual_refresh_rate",
    "dd_hdr_option",
    "dd_config_revert_delay",
    "dd_disconnect_behavior",
    "dd_hold_layout_while_paused",
    "dd_paused_virtual_display_timeout_secs",
    "dd_always_restore_from_golden",
    "dd_snapshot_exclude_devices",
    "dd_snapshot_restore_hotkey",
    "dd_snapshot_restore_hotkey_modifiers",
    "dd_activate_virtual_display",
    "dd_virtual_display_scale",
    "dd_mode_remapping",
    "max_bitrate",
    "minimum_fps_target",
    "stream_reconfigure",
    "stream_preset",
    "fec_percentage",
    "packet_pacing",
    "keyframe_loss_pacing",
    "bitrate_adaptation",
    "video_max_batch_size_kb",
    "pyrowave_critical_fec_percentage",
    "hevc_mode",
    "av1_mode",
    "packed_444",
    "capture",
    "capture_window",
    "capture_window_exe",
    "capture_window_title",
    "capture_window_closed",
    "capture_window_frame",
    "encoder",
    "playnite_focus_attempts",
    "playnite_focus_timeout_secs",
    "playnite_focus_exit_on_first",
    "frame_limiter_enable",
    "frame_limiter_provider",
    "frame_limiter_fps_limit",
    "virtual_display_refresh",
    "rtss_frame_limit_type",
    "amd_usage",
    "amd_rc",
    "amd_peak_bitrate_ratio",
    "amd_vbv_buffer_frames",
    "amd_max_frame_size",
    "amd_qvbr_quality_level",
    "amd_enforce_hrd",
    "amd_quality",
    "amd_preanalysis",
    "amd_vbaq",
    "amd_coder",
    "amd_ltr_frames",
    "amd_smart_access_video",
    "amd_split_frame",
    "amd_vpe",
    "amd_high_motion_quality_boost",
    "amd_av1_screen_content",
    "amd_av1_latency_mode",
    "nvenc_preset",
    "nvenc_twopass",
    "nvenc_rate_control",
    "nvenc_target_quality",
    "nvenc_peak_bitrate_ratio",
    "nvenc_spatial_aq",
    "nvenc_temporal_aq",
    "nvenc_vbv_increase",
    "nvenc_split_encode",
    "nvenc_weighted_prediction",
    "nvenc_min_qp_h264",
    "nvenc_min_qp_hevc",
    "nvenc_min_qp_av1",
    "nvenc_h264_cavlc",
    "nvenc_insert_filler_data",
    "nvenc_frame_budget_guard",
    "nvenc_realtime_hags",
    "encoder_focus",
    "frame_warp",
    "hdr_peak_brightness",
    "rtx_hdr",
    "rtx_hdr_sdr_brightness",
    "rtx_hdr_contrast",
    "rtx_hdr_saturation",
    "rtx_hdr_middle_gray",
    "rtx_video",
    "rtx_video_intensity",
    "hdr_dynamic_metadata",
    "hdr_sdr_tone_map",
    "hdr_dolby_vision",
    "hdr_hlg",
    "audio_passthrough",
    "audio_surround_714",
    "keyboard_follows_focus",
    "text_input_surrounding_text",
    "keyboard_layout",
    "touch_mode",
    "touch_sensitivity",
    "touch_gestures",
    "client_cursor",
    "dd_rotation_option",
    "dd_manual_rotation",
];

/// Settings read only when the host starts (`restart_required`).
pub const RESTART_KEYS: &[&str] = &[
    "port",
    "address_family",
    "bind_address",
    "enable_discovery",
    "sunshine_name",
    "upnp",
    "stream_mic",
    "origin_web_ui_allowed",
    "internet_play",
    "internet_play_stun_servers",
    "webrtc_server",
    "webrtc_port",
    "webrtc_media_port",
    "enable_dsu_server",
    "dsu_server_port",
    "dsu_server_lan",
    "system_tray",
    "hide_tray_controls",
    "log_path",
    "hevc_mode",
    "av1_mode",
    "pyrowave",
    "encoder",
    "adapter_name",
    "adapter_pnp_id",
    "output_name",
    "nvenc_latency_over_power",
    "file_state",
    "credentials_file",
    "file_apps",
    "vibeshine_file_state",
    "cert",
    "pkey",
];

/// Every 3.0 setting with its default, as `sunshine.conf` would spell it.
pub const SETTINGS_30: &[(&str, &str)] = &[
    ("audio_surround_714", "true"),
    ("pen_barrel_roll", "true"),
    ("touch_mode", "client"),
    ("touch_sensitivity", "100"),
    ("touch_gestures", "true"),
    ("client_cursor", "true"),
    ("webhooks", "[]"),
    ("qr_pairing", "true"),
    ("dd_rotation_option", "disabled"),
    ("dd_manual_rotation", "0"),
    ("nucleus_path", ""),
    ("nucleus_coop_players", "4"),
];

/// Validate changed settings before saving (`Config::update_with`).
pub(super) fn validate_changed(next: &Config, changed: &dyn Fn(&str) -> bool) -> Result<()> {
    if changed("sleep_after_idle_minutes")
        && !parse_integer(&next.values["sleep_after_idle_minutes"])
            .is_some_and(|n| n == 0 || (5..=1440).contains(&n))
    {
        bail!("sleep_after_idle_minutes must be 0 (off) or between 5 and 1440");
    }
    if changed("sleep_while_suspended")
        && parse_boolean(&next.values["sleep_while_suspended"]).is_none()
    {
        bail!("sleep_while_suspended must be true or false");
    }
    if changed("hardware_metrics_interval")
        && !parse_integer(&next.values["hardware_metrics_interval"])
            .is_some_and(|n| (500..=10000).contains(&n))
    {
        bail!("hardware metrics interval must be between 500 and 10000 milliseconds");
    }
    if changed("hardware_metrics_selection") {
        for metric in next.list("hardware_metrics_selection") {
            if !HARDWARE_METRICS.contains(&metric.as_str()) {
                bail!("unknown hardware metric: {metric}");
            }
        }
    }
    if changed("port")
        && !parse_integer(&next.values["port"]).is_some_and(|n| (1029..=65514).contains(&n))
    {
        bail!("port must be between 1029 and 65514");
    }
    if changed("webrtc_port")
        && !parse_integer(&next.values["webrtc_port"]).is_some_and(|n| (1024..=65535).contains(&n))
    {
        bail!("webrtc_port must be between 1024 and 65535");
    }
    if changed("webrtc_media_port")
        && !parse_integer(&next.values["webrtc_media_port"])
            .is_some_and(|n| (1024..=65535).contains(&n))
    {
        bail!("webrtc_media_port must be between 1024 and 65535");
    }
    if changed("audio_night_mode") && parse_boolean(&next.values["audio_night_mode"]).is_none() {
        bail!("audio_night_mode must be true or false");
    }
    if changed("audio_headphone_surround")
        && parse_boolean(&next.values["audio_headphone_surround"]).is_none()
    {
        bail!("audio_headphone_surround must be true or false");
    }
    if changed("dsu_server_port")
        && !parse_integer(&next.values["dsu_server_port"])
            .is_some_and(|n| (1024..=65535).contains(&n))
    {
        bail!("dsu_server_port must be between 1024 and 65535");
    }
    if changed(MAX_STREAM_RESOLUTION) {
        parse_max_stream_resolution(&next.values[MAX_STREAM_RESOLUTION])?;
    }
    Ok(())
}

impl Config {
    fn check_range(
        &self,
        key: &str,
        value: i64,
        default: i64,
        range: std::ops::RangeInclusive<i64>,
    ) -> i64 {
        if range.contains(&value) {
            value
        } else {
            invalid(key, self.get(key, ""));
            default
        }
    }
    /// Updates: `off`, `tell_me` (default) or `install`.
    pub fn updates(&self) -> UpdatesMode {
        let value = self.get("updates", "tell_me");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "off" => UpdatesMode::Off,
            "tell_me" => UpdatesMode::TellMe,
            "install" => UpdatesMode::Install,
            _ => {
                invalid("updates", value);
                UpdatesMode::TellMe
            }
        }
    }
    /// Whether paired clients may read hardware metrics during a stream (`hardware_metrics`, on unless turned off).
    pub fn hardware_metrics(&self) -> bool {
        self.boolean("hardware_metrics", true)
    }
    /// Shortest interval between hardware metrics readings in ms (clamped to 500..=10000).
    pub fn hardware_metrics_interval(&self) -> u64 {
        self.integer("hardware_metrics_interval", 1000)
            .clamp(500, 10000) as u64
    }
    /// USB forwarding from devices with `allow_usb` (off by default).
    pub fn usb_forwarding(&self) -> bool {
        self.boolean("usb_forwarding", false)
    }
    /// How many forwarded USB devices the host takes at once (`usb_max_devices`, 1 to 16, 4 by default).
    pub fn usb_max_devices(&self) -> u8 {
        self.check_range(
            "usb_max_devices",
            self.integer("usb_max_devices", 4),
            4,
            1..=16,
        ) as u8
    }
    /// The WebRTC server for TV and browser clients (off by default).
    pub fn webrtc_server(&self) -> bool {
        self.boolean("webrtc_server", false)
    }
    /// The WebRTC signalling port (TCP; discovery always listens on UDP 8000).
    pub fn webrtc_port(&self) -> u16 {
        self.check_range(
            "webrtc_port",
            self.integer("webrtc_port", 8000),
            8000,
            1024..=65535,
        ) as u16
    }
    /// The WebRTC media port (UDP).
    pub fn webrtc_media_port(&self) -> u16 {
        self.check_range(
            "webrtc_media_port",
            self.integer("webrtc_media_port", 8001),
            8001,
            1024..=65535,
        ) as u16
    }
    /// File transfer from paired devices (off by default).
    pub fn file_transfer(&self) -> bool {
        self.boolean("file_transfer", false)
    }
    /// The folder transfers use; `None` (empty) is the signed-in user's `Downloads\Rubylight`.
    pub fn file_transfer_dir(&self) -> Option<PathBuf> {
        let value = unquote(self.get("file_transfer_dir", "").trim()).trim();
        (!value.is_empty()).then(|| PathBuf::from(value))
    }
    /// The largest upload in bytes (`file_transfer_max_mb`, MiB, 4096 by default).
    pub fn file_transfer_max_bytes(&self) -> u64 {
        const MIB: i64 = 1024 * 1024;
        let mib = self.check_range(
            "file_transfer_max_mb",
            self.integer("file_transfer_max_mb", 4096),
            4096,
            1..=i64::MAX / MIB,
        );
        (mib * MIB) as u64
    }
    /// Images on the shared clipboard, besides text (on by default).
    pub fn clipboard_image(&self) -> bool {
        self.boolean("clipboard_image", true)
    }
    /// The largest clipboard image in bytes (`clipboard_image_max_mb`, MiB, 1 to 64, 16 by default).
    pub fn clipboard_image_max_bytes(&self) -> u64 {
        const MIB: i64 = 1024 * 1024;
        let mib = self.check_range(
            "clipboard_image_max_mb",
            self.integer("clipboard_image_max_mb", 16),
            16,
            1..=64,
        );
        (mib * MIB) as u64
    }
    /// Client roaming (`client_roaming`, on by default).
    pub fn client_roaming(&self) -> bool {
        self.boolean("client_roaming", true)
    }
    /// `GET /bandwidth-probe` for paired devices (`bandwidth_probe`, on by default).
    pub fn bandwidth_probe(&self) -> bool {
        self.boolean("bandwidth_probe", true)
    }
    /// PCM, Dolby Digital (AC3) and Dolby Digital Plus (E-AC3) audio for clients that pick one (`audio_passthrough`, on by default).
    pub fn audio_passthrough(&self) -> bool {
        self.boolean("audio_passthrough", true)
    }
    /// Show the device's keyboard when a text field on the host gets the focus (`keyboard_follows_focus`, on by default).
    pub fn keyboard_follows_focus(&self) -> bool {
        self.boolean("keyboard_follows_focus", true)
    }
    /// Send the text around the cursor to the device's keyboard (`text_input_surrounding_text`, off by default).
    pub fn text_input_surrounding_text(&self) -> bool {
        self.boolean("text_input_surrounding_text", false)
    }
    /// Whether the DSU motion server listens where other devices on the network can reach it (`dsu_server_lan`, off by default).
    pub fn dsu_server_lan(&self) -> bool {
        self.boolean("dsu_server_lan", false)
    }
    /// The host-drawn couch menu (on by default).
    pub fn couch_menu(&self) -> bool {
        self.boolean("couch_menu", true)
    }
    /// Whether direct driver push (`idd-push`) uses the GPU fence ring (default true; false selects keyed-mutex ring).
    pub fn idd_push_fence_ring(&self) -> bool {
        self.boolean("idd_push_fence_ring", true)
    }
    /// Whether Nightjar clients are offered packed full chroma (`packed_444`, on unless turned off).
    pub fn packed_444(&self) -> bool {
        self.boolean("packed_444", true)
    }
    /// Whether PyroWave converts the next picture while the GPU still encodes the last one (`pyrowave_encode_overlap`, off by default).
    pub fn pyrowave_encode_overlap(&self) -> bool {
        self.boolean("pyrowave_encode_overlap", false)
    }
    /// HDR10+ and HDR Vivid dynamic metadata in HDR streams (`hdr_dynamic_metadata`, on by default).
    pub fn hdr_dynamic_metadata(&self) -> bool {
        self.boolean("hdr_dynamic_metadata", true)
    }
    /// BT.2446 Method B for SDR streams of an HDR desktop (`hdr_sdr_tone_map`, off by default).
    pub fn hdr_sdr_tone_map(&self) -> bool {
        self.boolean("hdr_sdr_tone_map", false)
    }
    /// The Dolby Vision offer on HEVC and AV1 HDR streams (`hdr_dolby_vision`, on by default).
    pub fn hdr_dolby_vision(&self) -> bool {
        self.boolean("hdr_dolby_vision", true)
    }
    /// The HLG offer on HEVC and AV1 HDR streams (`hdr_hlg`, on by default).
    pub fn hdr_hlg(&self) -> bool {
        self.boolean("hdr_hlg", true)
    }
    /// Let devices use Frame Warp (`frame_warp`, on by default).
    pub fn frame_warp(&self) -> bool {
        self.boolean("frame_warp", true)
    }
    /// Adaptive bitrate: `off`, `on` (default) or `delay`.
    pub fn bitrate_adaptation(&self) -> BitrateAdaptation {
        let value = self.get("bitrate_adaptation", "on");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "off" => BitrateAdaptation::Off,
            "on" => BitrateAdaptation::On,
            "delay" => BitrateAdaptation::Delay,
            _ => {
                invalid("bitrate_adaptation", value);
                BitrateAdaptation::On
            }
        }
    }
    /// When a remote monitor is removed from Windows: `never` (default), `stream_end` or `disconnect`.
    pub fn remove_remote_monitor(&self) -> RemoveRemoteMonitor {
        let value = self.get("remove_remote_monitor", "never");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "never" => RemoveRemoteMonitor::Never,
            "stream_end" => RemoveRemoteMonitor::StreamEnd,
            "disconnect" => RemoveRemoteMonitor::Disconnect,
            _ => {
                invalid("remove_remote_monitor", value);
                RemoveRemoteMonitor::Never
            }
        }
    }
    /// 7.1.4 surround sound (on by default).
    pub fn audio_surround_714(&self) -> bool {
        self.boolean("audio_surround_714", true)
    }
    /// Stylus barrel roll, 0x5500000B (on by default).
    pub fn pen_barrel_roll(&self) -> bool {
        self.boolean("pen_barrel_roll", true)
    }
    /// `touch_mode`: `client` (the default), `mouse` or `trackpad`.
    pub fn touch_mode(&self) -> TouchMode {
        let value = unquote(self.get("touch_mode", "client").trim()).trim();
        match value {
            "client" => TouchMode::Client,
            "mouse" => TouchMode::Mouse,
            "trackpad" => TouchMode::Trackpad,
            _ => {
                invalid("touch_mode", value);
                TouchMode::Client
            }
        }
    }
    /// Pointer movement for a swipe in mouse or trackpad mode, in percent (25 to 400, 100 by default).
    pub fn touch_sensitivity(&self) -> u16 {
        self.check_range(
            "touch_sensitivity",
            self.integer("touch_sensitivity", 100),
            100,
            25..=400,
        ) as u16
    }
    /// Multi-finger touch gestures as scrolling and clicks (on by default).
    pub fn touch_gestures(&self) -> bool {
        self.boolean("touch_gestures", true)
    }
    /// Let a device draw the cursor itself (on by default).
    pub fn client_cursor(&self) -> bool {
        self.boolean("client_cursor", true)
    }
    /// Pairing by QR code instead of a PIN (on by default).
    pub fn qr_pairing(&self) -> bool {
        self.boolean("qr_pairing", true)
    }
    /// `dd_rotation_option`: `disabled` (the default), `auto` or `manual`.
    pub fn dd_rotation_option(&self) -> DisplayAdjust {
        let value = unquote(self.get("dd_rotation_option", "disabled").trim()).trim();
        match value {
            "disabled" => DisplayAdjust::Disabled,
            "auto" => DisplayAdjust::Auto,
            "manual" => DisplayAdjust::Manual,
            _ => {
                invalid("dd_rotation_option", value);
                DisplayAdjust::Disabled
            }
        }
    }
    /// `dd_manual_rotation`: 0 (the default), 90, 180 or 270 degrees clockwise.
    pub fn dd_manual_rotation(&self) -> u16 {
        let value = self.integer("dd_manual_rotation", 0);
        if matches!(value, 0 | 90 | 180 | 270) {
            value as u16
        } else {
            invalid("dd_manual_rotation", self.get("dd_manual_rotation", ""));
            0
        }
    }
    /// The Nucleus Co-op folder: `nucleus_path`, or None for an earlier Rubylight's install, if any.
    pub fn nucleus_path(&self) -> Option<PathBuf> {
        let value = unquote(self.get("nucleus_path", "").trim()).trim();
        (!value.is_empty()).then(|| PathBuf::from(value))
    }
    /// The most players a co-op session gets (1 to 4, 4 by default).
    pub fn nucleus_coop_players(&self) -> u32 {
        self.check_range(
            "nucleus_coop_players",
            self.integer("nucleus_coop_players", 4),
            4,
            1..=4,
        ) as u32
    }
    /// Whether the WebRTC media thread paces video (the default).
    pub fn webrtc_pacing(&self) -> bool {
        self.boolean("webrtc_pacing", true)
    }
    /// Whether the WebRTC offer carries NACK and RTX (`webrtc_rtx`, on by default).
    pub fn webrtc_rtx(&self) -> bool {
        self.boolean("webrtc_rtx", true)
    }
    /// Video datagrams the WebRTC media thread drops per thousand (`webrtc_loss_permille`, 0 to 1000, 0 by default).
    pub fn webrtc_loss_permille(&self) -> u32 {
        self.integer("webrtc_loss_permille", 0).clamp(0, 1000) as u32
    }
    /// Whether the AMF encoder skips an output query while its oldest frame cannot be finished yet (`amd_output_prediction`, off by default).
    pub fn amd_output_prediction(&self) -> bool {
        self.boolean("amd_output_prediction", false)
    }
    /// Whether a thread of its own waits in AMF's output query (`amd_output_thread`, on by default).
    pub fn amd_output_thread(&self) -> bool {
        self.boolean("amd_output_thread", true)
    }
    /// Whether the stream thread also wakes on NVENC's D3D12 output fence (`nvenc_output_event`, on by default).
    pub fn nvenc_output_event(&self) -> bool {
        self.boolean("nvenc_output_event", true)
    }
}
