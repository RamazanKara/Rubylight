//! Generated fallback-console global setting catalog (`Control`, `Setting`, `GRIPS`, `GLOBAL`).
//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.

#[derive(Clone, Copy)]
pub enum Control {
    Bool,
    Choice(&'static [&'static str]),
    Text,
    Number,
    Json,
}

pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub control: Control,
}

macro_rules! console_settings {
    ($($group:literal => [$($key:literal, $label:literal, $control:expr;)*])*) => {
        &[$($(Setting { key: $key, label: $label, group: $group, control: $control },)*)*]
    };
}

use Control::*;
pub(super) use console_settings;

/// What a back grip can press (`back_grip_*`).
pub const GRIPS: &[&str] = &[
    "none",
    "a",
    "b",
    "x",
    "y",
    "lb",
    "rb",
    "lt",
    "rt",
    "l3",
    "r3",
    "back",
    "start",
    "guide",
    "dpad_up",
    "dpad_down",
    "dpad_left",
    "dpad_right",
    "touchpad",
    "misc",
];

pub const GLOBAL: &[Setting] = console_settings! {
    "App behavior" => [
        "terminate_on_pause", "Close the app when the last device disconnects", Bool;
        "exit_timeout", "App shutdown timeout (seconds; apps without their own; default 10)", Number;
        "quick_suspend", "Quick Suspend (when a device asks pauses the running game until it is resumed; off never pauses one)", Choice(&["off", "manual"]);
        "launch_curtain", "Launch curtain (a device shows the game's cover until its window is in front)", Bool;
        "guest_join", "Another device joining the running game (default off)", Choice(&["off", "ask", "allow"]);
        "guest_permission", "What a guest may do", Choice(&["view", "controller", "controller-keyboard-mouse"]);
    ]
    "Audio" => [
        "virtual_sink", "Virtual audio device", Text;
        "stream_audio", "Stream audio", Bool;
        "audio_passthrough", "PCM, Dolby Digital and Dolby Digital Plus audio", Bool;
        "audio_night_mode", "Night mode (evens out loud and quiet sound)", Bool;
        "audio_headphone_surround", "Headphone surround (surround sound rendered for headphones on a stereo device)", Bool;
        "audio_sink_capture_only", "Capture selected audio without changing defaults", Bool;
        "auto_capture_sink", "Follow audio device changes", Bool;
        "keep_sink_default", "Keep virtual speakers as the default during streaming", Bool;
        "hold_sink_while_paused", "Keep virtual speakers as the default while paused", Bool;
        "install_steam_audio_drivers", "Install Steam streaming speakers and microphone when needed", Bool;
        "stream_mic", "Use the device microphone (Steam Streaming Microphone)", Bool;
    ]
    "Displays and HDR" => [
        "virtual_display_layout", "Virtual display layout", Choice(&["exclusive", "extended", "extended_primary", "extended_isolated", "extended_primary_isolated"]);
        "dd_configuration_option", "Display activation", Choice(&["disabled", "verify_only", "ensure_active", "ensure_primary", "ensure_only_display"]);
        "dd_resolution_option", "Resolution policy", Choice(&["disabled", "auto", "manual"]);
        "dd_manual_resolution", "Manual resolution (WIDTHxHEIGHT)", Text;
        "dd_refresh_rate_option", "Refresh policy", Choice(&["disabled", "auto", "manual", "prefer_highest"]);
        "dd_manual_refresh_rate", "Manual refresh rate (supports 59.94, 119.88)", Number;
        "dd_hdr_option", "Display HDR", Choice(&["auto", "force_on", "force_off", "disabled"]);
        "hdr_peak_brightness", "Peak brightness a virtual display reports (nits)", Number;
        "hdr_dynamic_metadata", "HDR10+ and HDR Vivid dynamic metadata", Bool;
        "hdr_sdr_tone_map", "Keep HDR highlights in SDR streams", Bool;
        "hdr_dolby_vision", "Dolby Vision 8.1, 8.4 and 10.1 for devices that ask", Bool;
        "hdr_hlg", "HLG for devices that ask", Bool;
        "dd_disconnect_behavior", "When a device disconnects (restore: after the delay; keep_while_paused: keep its display while paused)", Choice(&["restore", "keep_while_paused"]);
        "dd_config_revert_delay", "Display restore delay (milliseconds)", Number;
        "dd_paused_virtual_display_timeout_secs", "Paused display timeout (seconds; 0 keeps it)", Number;
        "dd_hold_layout_while_paused", "Keep monitors off while paused", Bool;
        "dd_always_restore_from_golden", "Always restore the saved display layout", Bool;
        "dd_activate_virtual_display", "Activate virtual display", Bool;
        "dd_virtual_display_scale", "Virtual display scale (%)", Number;
        "dd_snapshot_exclude_devices", "Devices excluded from saved layout restore", Json;
        "dd_mode_remapping", "Resolution and refresh remapping", Json;
        "vulkan_hdr_layer", "Vulkan virtual HDR support", Bool;
        "virtual_display_hdr_calibration", "Windows HDR Calibration on virtual displays", Bool;
        "fallback_mode", "Default launch mode (WIDTHxHEIGHTxFPS)", Text;
        "max_stream_resolution", "Maximum stream resolution (native or WIDTHxHEIGHT)", Text;
    ]
    "Frame generation and limiting" => [
        "frame_limiter_enable", "Enable frame limiter", Bool;
        "frame_limiter_provider", "Limiter provider", Choice(&["auto", "rtss", "none"]);
        "frame_limiter_fps_limit", "Frame limit (0 uses the device rate)", Number;
        "virtual_display_refresh", "Virtual display refresh", Choice(&["legacy", "enabled", "480hz", "fixed-1000hz", "disabled"]);
        "rtss_install_path", "RTSS installation directory", Text;
        "rtss_frame_limit_type", "RTSS limiter mode", Choice(&["async", "front-edge-sync", "back-edge-sync", "nvidia-reflex"]);
        "rtss_allow_virtual_display_override", "Allow virtual display limiter overrides", Bool;
    ]
    "Video and capture" => [
        "stream_preset", "Stream preset", Choice(&["lowest_latency", "balanced", "reference"]);
        "adapter_name", "Encoding graphics adapter", Text;
        "adapter_pnp_id", "Encoding adapter device ID", Text;
        "encoder_focus", "Encoder focus (more bits for still areas such as text and HUDs)", Choice(&["off", "static", "centre"]);
        "dvr_enabled", "Instant Replay and recording (Stream DVR)", Bool;
        "dvr_ring_seconds", "Replay length in seconds (30, 60, 90 or 120)", Number;
        "dvr_quota_gb", "Space for clips (GB, 5 to 500; default 20)", Number;
        "dvr_directory", "Clips folder (leave empty for clips beside the settings)", Text;
        "hevc_mode", "HEVC support (0 auto, 1 disabled, 2 SDR, 3 HDR)", Number;
        "av1_mode", "AV1 support (0 auto, 1 disabled, 2 SDR, 3 HDR)", Number;
        "packed_444", "Full chroma for Nightjar", Bool;
        "max_bitrate", "Maximum bitrate (kbps; 0 has no configured limit)", Number;
        "limit_framerate", "Use the launch frame rate for encoding (off by default: the stream runs at the rate its request asks for)", Bool;
        "minimum_fps_target", "Still-picture resend rate (how often an unchanged desktop is sent again)", Number;
        "wgc_pacing_smoothing", "Smooth capture pacing", Bool;
        "stream_reconfigure", "Follow client size and frame rate changes", Bool;
        "frame_warp", "Let devices use Frame Warp", Bool;
        "wgc_direct_encoder_input", "Pass GPU capture directly to the encoder", Bool;
        "gpu_compute_conversion", "Copy and convert captures on a compute queue", Bool;
        "idd_push_fence_ring", "GPU fence transport for direct driver push (on by default; turn off to use the keyed-mutex ring)", Bool;
        "wgc_slot_aligned_publish", "Align capture publication to frame slots", Bool;
        "fec_percentage", "Forward error correction (%; every codec, PyroWave included)", Number;
        "pyrowave", "Enable PyroWave for compatible devices", Bool;
        "pyrowave_encode_overlap", "Prepare the next PyroWave frame while the current one encodes", Bool;
        "pyrowave_critical_fec_percentage", "PyroWave protection for required image data (%; only shown while it differs from the error correction percentage)", Number;
        "video_max_batch_size_kb", "Video send batch limit (KiB)", Number;
        "packet_pacing", "Packet pacing (automatic, the default: adaptive pacing for a device that reports a Wi-Fi or cellular route on H.264, HEVC and AV1, route pacing for the rest; always: adaptive pacing for every stream, including PyroWave; off: route pacing only, and Slow down after video loss still applies; or a fixed rate in kbps, which takes over pacing, e.g. 120000 for 120 Mbps; reconnect after saving)", Text;
        "keyframe_loss_pacing", "Slow down after video loss (on by default; after a lost keyframe or three distinct unrecoverable frames within two seconds, video is paced at twice the bitrate, with room for error correction, for the rest of the stream (a lost first keyframe alone, or loss only the host saw while sending in a stream that has not slowed down before, is lifted after 10 clean seconds; a client that reports its own losses is not slowed by asking twice for a keyframe); disabled by a fixed pacing rate or the adaptive options)", Bool;
        "bitrate_adaptation", "Adaptive bitrate (on by default: lowers the picture bitrate on loss that error correction cannot repair, then raises it again; on, and react to network delay also lowers it as delay builds, for devices that report packet arrivals such as Nightjar, reconnect after saving; off keeps the bitrate you set, so a weak link freezes or breaks up the picture; a device that sets its own bitrate takes over)", Choice(&["off", "on", "delay"]);
    ]
    "AMD encoding" => [
        "amd_usage", "AMF usage", Choice(&["auto", "transcoding", "ultralowlatency", "lowlatency", "webcam", "high_quality", "lowlatency_high_quality"]);
        "amd_quality", "AMF quality", Choice(&["auto", "speed", "balanced", "quality"]);
        "amd_rc", "AMF rate control", Choice(&["auto", "cqp", "cbr", "vbr_latency", "vbr_peak", "qvbr", "hqvbr", "hqcbr"]);
        "amd_preanalysis", "Pre-analysis", Bool;
        "amd_vbaq", "Adaptive quantization", Bool;
        "amd_enforce_hrd", "Enforce decoder buffer limits", Bool;
        "amd_coder", "H.264 entropy coder", Choice(&["auto", "cabac", "cavlc"]);
        "amd_high_motion_quality_boost", "High motion quality boost", Bool;
        "amd_av1_latency_mode", "AV1 latency mode", Choice(&["none", "power_saving", "realtime", "lowest"]);
        "amd_split_frame", "Split-frame encoding (HEVC and AV1)", Choice(&["auto", "enabled", "disabled"]);
        "amd_ltr_frames", "Long-term reference frames", Number;
    ]
    "NVIDIA encoding" => [
        "nvenc_preset", "NVENC preset (1 fastest to 7 slowest)", Number;
        "nvenc_rate_control", "NVENC rate control", Choice(&["cbr", "vbr"]);
        "nvenc_target_quality", "Variable bitrate target quality (0 lets the driver choose)", Number;
        "nvenc_peak_bitrate_ratio", "Variable bitrate peak (times the stream bitrate)", Choice(&["1", "1.25", "1.5", "2", "3"]);
        "nvenc_twopass", "Multi-pass encoding", Choice(&["disabled", "quarter_res", "full_res"]);
        "nvenc_spatial_aq", "Spatial adaptive quantization", Bool;
        "nvenc_temporal_aq", "Temporal adaptive quantization", Bool;
        "nvenc_split_encode", "Split-frame encoding (HEVC and AV1)", Choice(&["auto", "driver_decides", "enabled", "disabled", "two_strips", "three_strips", "four_strips"]);
        "nvenc_frame_budget_guard", "Frame time guard (a faster preset when frames would be late)", Bool;
        "nvenc_realtime_hags", "Realtime GPU priority under hardware-accelerated GPU scheduling", Bool;
        "nvenc_latency_over_power", "Full GPU clocks while streaming", Bool;
        "nvidia_virtual_display_vsync", "Force VSync on for virtual display streams", Bool;
        "nvenc_vbv_increase", "Frame size headroom (%)", Number;
        "nvenc_weighted_prediction", "Weighted prediction", Bool;
        "nvenc_h264_cavlc", "Use CAVLC for H.264", Bool;
        "nvenc_insert_filler_data", "Insert filler data", Bool;
        "nvenc_min_qp_h264", "Minimum QP for H.264 (0 is off)", Number;
        "nvenc_min_qp_hevc", "Minimum QP for HEVC (0 is off)", Number;
        "nvenc_min_qp_av1", "Minimum QP for AV1 (0 is off)", Number;
    ]
    "Input" => [
        "keyboard", "Keyboard input", Bool;
        "mouse", "Mouse input", Bool;
        "controller", "Controller input", Bool;
        "native_pen_touch", "Native pen and touch", Bool;
        "mouse_mode", "Mouse input method (auto: the virtual mouse when the USB/IP driver is installed)", Choice(&["auto", "virtual_mouse", "injection"]);
        "native_touchpad", "Touchpad gestures from devices", Bool;
        "gamepad", "Controller profile (auto: DualSense for PlayStation-type or motion/touchpad, Switch 2 Pro over USB/IP for a Switch 2 Pro Controller, Switch Pro for other Nintendo, Xbox Series otherwise)", Choice(&["auto", "vhf_xbox", "vhf_xbox_one", "vhf_ds4", "vhf_ds5", "usbip_ds5", "usbip_switch2", "vhf_switch", "vhf_steam", "vigem_x360", "vigem_ds4"]);
        "keybindings", "Keyboard remapping pairs", Json;
        "always_send_scancodes", "Send keyboard scan codes", Bool;
        "key_rightalt_to_key_win", "Map right Alt to Windows key", Bool;
        "key_repeat_delay", "Keyboard repeat delay (milliseconds)", Number;
        "key_repeat_frequency", "Keyboard repeat frequency (Hz)", Number;
        "high_resolution_scrolling", "High resolution scrolling", Bool;
        "forward_rumble", "Controller vibration", Bool;
        "audio_haptics", "Haptics from game audio", Bool;
        "audio_haptics_strength", "Audio haptics strength (percent, 0-200)", Number;
        "audio_haptics_curve", "Audio haptics response", Choice(&["linear", "soft", "punchy"]);
        "audio_haptics_gate", "Audio haptics noise gate (dBFS, -80 to -10)", Number;
        "dualsense_haptics_gain", "DualSense haptics strength (0 to 4 times, 1 default)", Number;
        "motion_as_ds4", "Use PlayStation profile for motion input", Bool;
        "touchpad_as_ds4", "Use PlayStation profile for touchpad input", Bool;
        "ds4_back_as_touchpad_click", "Back also clicks the touchpad (DualShock 4 on ViGEmBus)", Bool;
        "enable_dsu_server", "DSU motion server for emulators (UDP, this PC only by default)", Bool;
        "dsu_server_port", "DSU motion server port (UDP, 1024-65535)", Number;
        "dsu_server_lan", "DSU motion server on the local network", Bool;
        "back_button_timeout", "Hold Back to press Guide (milliseconds; -1 disables)", Number;
        "steam_deck_controller", "Steam Deck controller", Choice(&["auto", "steam_deck", "virtual_pad"]);
        "back_grip_l4", "Back grip L4 (upper left) presses", Choice(GRIPS);
        "back_grip_r4", "Back grip R4 (upper right) presses", Choice(GRIPS);
        "back_grip_l5", "Back grip L5 (lower left) presses", Choice(GRIPS);
        "back_grip_r5", "Back grip R5 (lower right) presses", Choice(GRIPS);
        "enable_input_only_mode", "Allow input-only connections", Bool;
    ]
    "Couch menu" => [
        "couch_menu", "Host menu drawn into the stream", Bool;
        "couch_menu_open_with", "Ways to open it (comma-separated: controller, keyboard, touch; empty turns all off)", Text;
        "couch_menu_controller_chord", "Controller chord (buttons joined with +, default l3+r3)", Text;
        "couch_menu_hold_ms", "Controller chord hold (300–3000 milliseconds; default 1000)", Number;
        "couch_menu_corner_hold_ms", "Touch corner hold (500–3000 milliseconds; default 1500)", Number;
        "couch_menu_keyboard_chord", "Keyboard chord", Choice(&["ctrl+alt+backspace", "ctrl+alt+home", "ctrl+alt+end", "ctrl+alt+insert"]);
    ]
    "Hardware metrics" => [
        "hardware_metrics", "Send hardware metrics to client", Bool;
        "hardware_metrics_interval", "Hardware metrics interval (500–10000 milliseconds; default 1000)", Number;
        "hardware_metrics_selection", "Hardware metrics selection (JSON list; empty sends all supported metrics)", Json;
    ]
    "Remote monitors" => [
        "remote_monitor_mute_audio", "Mute remote monitor audio", Bool;
        "remove_remote_monitor", "Remove remote monitor (never: keep it for resuming; stream_end: when its stream ends; disconnect: as soon as the device disconnects)", Choice(&["never", "stream_end", "disconnect"]);
        "remote_monitor_terminate_on_first_request", "Replace the current game on the first monitor request", Bool;
        "remote_monitor_confirm_app_replacement", "Require confirmation to replace a game with a monitor", Bool;
    ]
    "Network, administration and commands" => [
        "locale", "Language", Choice(&["en", "en_GB", "en_US", "bg", "cs", "de", "es", "fr", "hu", "it", "ja", "ko", "pl", "pt", "pt_BR", "ru", "sv", "tr", "uk", "vi", "zh", "zh_TW"]);
        "port", "Base Moonlight port", Number;
        "bind_address", "Listen address (leave empty for all network interfaces)", Text;
        "address_family", "IP support", Choice(&["ipv4", "both"]);
        "enable_discovery", "Moonlight discovery on the local network", Bool;
        "enable_pairing", "Allow new device pairings", Bool;
        "upnp", "Automatic router port forwarding (UPnP, PCP, NAT-PMP)", Bool;
        "internet_play", "Play over the internet (STUN, DHT rendezvous, direct paths)", Bool;
        "internet_play_stun_servers", "STUN servers (host:port, comma separated; empty for the defaults)", Text;
        "away_wake_relay", "Away wake relay (another device on this network wakes the PC from away)", Bool;
        "webrtc_server", "WebRTC server (unencrypted ws:// on the local network)", Bool;
        "webrtc_port", "WebRTC signalling port (TCP; discovery stays on UDP 8000)", Number;
        "webrtc_media_port", "WebRTC media port (UDP)", Number;
        "file_transfer", "File transfer from paired devices", Bool;
        "file_transfer_dir", "File transfer folder (leave empty for Downloads\\Rubylight)", Text;
        "file_transfer_max_mb", "Largest file to receive (MiB; default 4096)", Number;
        "file_transfer_stream_mbps", "Transfer speed while streaming (Mbit/s; 0 for full speed; default 50)", Number;
        "shared_folders", "Shared folders (JSON list of id, name and path)", Text;
        "clipboard_image", "Copy images with clipboard permission", Bool;
        "clipboard_image_max_mb", "Largest clipboard image (MiB, 1 to 64; default 16)", Number;
        "lan_encryption_mode", "Local network encryption (0 never, 1 optional, 2 required)", Number;
        "wan_encryption_mode", "Internet encryption (0 never, 1 optional, 2 required)", Number;
        "ping_timeout", "Device timeout (milliseconds)", Number;
        "dscp_tagging", "Prioritize stream traffic with DSCP marks (local network)", Bool;
        "origin_web_ui_allowed", "Administration access", Choice(&["pc", "lan", "wan"]);
        "csrf_allowed_origins", "Additional trusted administration origins", Json;
        "session_token_ttl_seconds", "Sign-in session lifetime (seconds)", Number;
        "remember_me_refresh_token_ttl_seconds", "Remembered browser lifetime (seconds)", Number;
        "lock_on_last_disconnect", "Lock Windows when the last device leaves", Bool;
        "system_tray", "Show system tray icon", Bool;
        "hide_tray_controls", "Hide tray shutdown controls", Bool;
        "min_log_level", "Log level (0 verbose, 1 debug, 2 info, 3 warning, 4 error, 6 off)", Number;
        "log_path", "Log file path", Text;
        "updates", "Updates (tell me checks and lets you know; install automatically also installs a new release when idle; off stops the checks)", Choice(&["off", "tell_me", "install"]);
        "notify_pre_releases", "Include prerelease updates", Bool;
        "update_check_interval", "Check for updates every (seconds, at least 3600; a daily time replaces it while set)", Number;
        "update_check_time", "Check for updates daily at (HH:MM, 24-hour clock; replaces the interval while set; empty uses the interval)", Text;
        "legacy_ordering", "Use legacy app ordering", Bool;
        "envvar_compatibility_mode", "Whole-number frame rate in SUNSHINE_CLIENT_FPS for commands", Bool;
        "global_prep_cmd", "Global preparation and cleanup commands", Json;
        "global_state_cmd", "Global pause and resume commands", Json;
        "server_cmd", "Host commands available to devices", Json;
    ]
};
