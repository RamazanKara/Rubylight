//! Generated fallback-console app/device setting catalogs and choice labels (`APP`, `CLIENT`, `choice_label`).
//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.

use super::console_global_gen::{Control::*, Setting, console_settings};

pub const APP: &[Setting] = console_settings! {
    "App behavior" => [
        "elevated", "Run as administrator", Bool;
        "auto-detach", "Keep streaming after an app launcher exits", Bool;
        "wait-all", "Wait for all app processes", Bool;
        "exit-timeout", "App shutdown timeout (seconds)", Number;
        "terminate-on-pause", "Close the app when the last device disconnects", Bool;
        "allow-client-commands", "Allow per-device connection commands", Bool;
        "menu-cmd", "Stream menu commands for this app (name, command, undo when the app closes)", Json;
        "exclude-global-menu-cmd", "Hide the host-wide PC commands while this app runs", Bool;
        "exclude-global-state-cmd", "Exclude global pause and resume commands", Bool;
        "background-stream", "Stream in the background (co-op splitter, controllers only)", Bool;
        "coop-entry", "Show the game's co-op entry in the library", Bool;
        "state-cmd", "App resume and pause commands", Json;
        "output", "App log path (null discards output)", Text;
        "gamepad", "Controller profile override", Choice(&["auto", "vhf_xbox", "vhf_xbox_one", "vhf_ds4", "vhf_ds5", "usbip_ds5", "usbip_switch2", "vhf_switch", "vhf_steam", "vigem_x360", "vigem_ds4"]);
    ]
    "App display and frame generation" => [
        "display-output", "Display override (sunshine:virtual_display selects a virtual display)", Text;
        "virtual-display", "Always use a virtual display, even on a device set to stream a monitor", Bool;
        "virtual-display-mode", "Virtual display mode (with Always above: whether devices share it)", Choice(&["disabled", "per_client", "shared"]);
        "virtual-display-layout", "Virtual display layout", Choice(&["exclusive", "extended", "extended_primary", "extended_isolated", "extended_primary_isolated"]);
        "virtual-display-primary", "Make the virtual display primary", Bool;
        "dd-configuration-option", "Display activation override", Choice(&["disabled", "verify_only", "ensure_active", "ensure_primary", "ensure_only_display"]);
        "scale-factor", "Display resolution scale (%)", Number;
        "use-app-identity", "Keep one virtual display identity for this app", Bool;
        "per-client-app-identity", "Keep a display identity for each app and device", Bool;
        "capture-window", "What to stream", Choice(&["display", "window"]);
        "capture-window-exe", "Window's program (file name; empty for any of the app's)", Text;
        "capture-window-title", "Window title contains (empty for any)", Text;
        "capture-window-closed", "When the window closes", Choice(&["display", "end"]);
        "capture-window-frame", "Window's frame", Choice(&["full", "client"]);
        "prefer-10bit-sdr", "Prefer 10-bit SDR", Bool;
        "absolute-left-release-delay", "Touch and pen click delay (milliseconds, 0 to 1000; empty uses the host's 10)", Number;
        "native-pen-touch", "Native pen and touch (empty uses the host's)", Bool;
        "frame-generation-mode", "Frame generation", Choice(&["none", "game-provided"]);
        "gen1-framegen-fix", "Legacy frame generation capture fix (only while Frame generation is Use default)", Bool;
    ]
    "App sound and couch menu" => [
        "audio-night-mode", "Night mode (empty uses the host's)", Bool;
        "audio-headphone-surround", "Headphone surround (empty uses the host's)", Bool;
        "couch-menu", "Couch menu (empty uses the host's)", Bool;
        "couch-menu-open-with", "Open the couch menu with (controller, keyboard, touch; empty uses the host's)", Text;
        "couch-menu-controller-chord", "Couch menu controller chord (empty uses the host's)", Text;
        "couch-menu-hold-ms", "Couch menu controller chord hold (milliseconds, 300 to 3000)", Number;
        "couch-menu-corner-hold-ms", "Couch menu corner hold (milliseconds, 500 to 3000)", Number;
        "couch-menu-keyboard-chord", "Couch menu keyboard chord (ctrl+alt+backspace, ctrl+alt+home, ctrl+alt+end or ctrl+alt+insert)", Text;
    ]
};

pub const CLIENT: &[Setting] = console_settings! {
    "Device preferences" => [
        "output_name_override", "Display override", Text;
        "always_use_virtual_display", "Always use a virtual display", Bool;
        "remember_display_mode", "Remember the virtual display resolution and scale chosen in Windows", Bool;
        "background_stream", "Stream in the background (co-op splitter, controllers only)", Bool;
        "virtual_display_mode", "Virtual display mode", Choice(&["disabled", "per_client", "shared"]);
        "virtual_display_layout", "Virtual display layout", Choice(&["exclusive", "extended", "extended_primary", "extended_isolated", "extended_primary_isolated"]);
        "prefer_10bit_sdr", "Prefer 10-bit SDR", Bool;
        "hdr_profile", "HDR ICC profile filename", Text;
        "allow_client_commands", "Run this device's connection commands", Bool;
        "do", "Connection commands", Json;
        "undo", "Disconnect commands", Json;
    ]
    "Sound" => [
        "stream_audio", "Stream audio (empty uses the host's)", Bool;
        "audio_passthrough", "PCM and Dolby Digital audio (empty uses the host's)", Bool;
        "audio_sink", "Audio device (empty uses the host's)", Text;
        "audio_night_mode", "Night mode (evens out loud and quiet sound)", Bool;
        "audio_headphone_surround", "Headphone surround (surround sound rendered for headphones on a stereo device)", Bool;
    ]
    "Couch menu" => [
        "couch_menu", "Host menu drawn into the stream", Bool;
        "couch_menu_open_with", "Ways to open it (comma-separated: controller, keyboard, touch; empty turns all off)", Text;
        "couch_menu_controller_chord", "Controller chord (buttons joined with +, default l3+r3)", Text;
        "couch_menu_hold_ms", "Controller chord hold (300–3000 milliseconds; default 1000)", Number;
        "couch_menu_corner_hold_ms", "Touch corner hold (500–3000 milliseconds; default 1500)", Number;
        "couch_menu_keyboard_chord", "Keyboard chord", Choice(&["ctrl+alt+backspace", "ctrl+alt+home", "ctrl+alt+end", "ctrl+alt+insert"]);
    ]
};

/// Human-readable label for a fallback console select choice, if catalogued.
pub fn choice_label(key: &str, choice: &str) -> Option<&'static str> {
    match (key, choice) {
        ("virtual_display_refresh", "enabled") => Some("Four times the stream frame rate"),
        ("dd_hdr_option", "auto") => Some("Match the stream"),
        ("dd_hdr_option", "disabled") => Some("Don't change HDR"),
        ("amd_av1_latency_mode", "none") => Some("Driver default"),
        ("encoder_focus", "off") => Some("Off"),
        ("bitrate_adaptation", "off") => Some("Off"),
        ("bitrate_adaptation", "on") => Some("On"),
        ("updates", "off") => Some("Off"),
        ("updates", "tell_me") => Some("Tell me"),
        ("updates", "install") => Some("Install automatically"),
        ("encoder_focus", "static") => Some("Still areas"),
        ("encoder_focus", "centre") => Some("Still areas and centre"),
        ("stream_preset", "lowest_latency") => Some("Lowest latency"),
        ("stream_preset", "reference") => Some("Reference quality"),
        ("bitrate_adaptation", "delay") => Some("On, and react to network delay"),
        ("dd_disconnect_behavior", "restore") => Some("Restore after the delay"),
        ("remove_remote_monitor", "never") => Some("Never"),
        ("remove_remote_monitor", "stream_end") => Some("When its stream ends"),
        ("remove_remote_monitor", "disconnect") => Some("As soon as the device disconnects"),
        (_, "true") => Some("On"),
        (_, "enabled") => Some("On"),
        (_, "false") => Some("Off"),
        (_, "disabled") => Some("Off"),
        (_, "auto") => Some("Automatic"),
        (_, "driver_decides") => Some("Driver decides (implicit only)"),
        (_, "none") => Some("None"),
        (_, "exclusive") => Some("Turn off other monitors"),
        (_, "extended") => Some("Extend — keep PC display primary"),
        (_, "extended_primary") => Some("Extend — make virtual display primary"),
        (_, "extended_isolated") => Some("Isolate — keep PC display primary"),
        (_, "extended_primary_isolated") => Some("Isolate — make virtual display primary"),
        (_, "verify_only") => Some("Only check that it is on"),
        (_, "ensure_active") => Some("Turn it on"),
        (_, "ensure_primary") => Some("Turn it on and make it primary"),
        (_, "ensure_only_display") => Some("Turn it on and other displays off"),
        (_, "manual") => Some("Use a fixed value"),
        (_, "prefer_highest") => Some("Highest available"),
        (_, "force_on") => Some("Always on"),
        (_, "force_off") => Some("Always off"),
        (_, "rtss") => Some("RivaTuner Statistics Server (RTSS)"),
        (_, "legacy") => Some("Twice the stream frame rate"),
        (_, "480hz") => Some("480 Hz"),
        (_, "fixed-1000hz") => Some("1000 Hz"),
        (_, "async") => Some("Asynchronous"),
        (_, "front-edge-sync") => Some("Front edge sync"),
        (_, "back-edge-sync") => Some("Back edge sync"),
        (_, "nvidia-reflex") => Some("NVIDIA Reflex"),
        (_, "transcoding") => Some("Transcoding (convert video formats)"),
        (_, "ultralowlatency") => Some("Ultra low latency"),
        (_, "lowlatency") => Some("Low latency"),
        (_, "webcam") => Some("Webcam"),
        (_, "high_quality") => Some("High quality"),
        (_, "lowlatency_high_quality") => Some("Low latency, high quality"),
        (_, "speed") => Some("Speed"),
        (_, "balanced") => Some("Balanced"),
        (_, "quality") => Some("Quality"),
        (_, "cqp") => Some("Constant quantization (fixed compression level)"),
        (_, "cbr") => Some("Constant bitrate"),
        (_, "vbr") => Some("Variable bitrate"),
        (_, "quarter_res") => Some("Quarter resolution"),
        (_, "full_res") => Some("Full resolution"),
        (_, "two_strips") => Some("Two strips"),
        (_, "three_strips") => Some("Three strips"),
        (_, "four_strips") => Some("Four strips"),
        (_, "vbr_latency") => Some("Variable bitrate, limited latency"),
        (_, "vbr_peak") => Some("Variable bitrate, limited peak rate"),
        (_, "qvbr") => Some("Variable bitrate, target quality"),
        (_, "hqvbr") => Some("High-quality variable bitrate"),
        (_, "hqcbr") => Some("High-quality constant bitrate"),
        (_, "cabac") => Some("CABAC (better compression)"),
        (_, "cavlc") => Some("CAVLC (simpler decoding)"),
        (_, "power_saving") => Some("Power saving"),
        (_, "realtime") => Some("Real time"),
        (_, "lowest") => Some("Lowest latency"),
        (_, "vhf_xbox") => Some("Xbox Series"),
        (_, "vhf_xbox_one") => Some("Xbox One"),
        (_, "vhf_ds4") => Some("DualShock 4"),
        (_, "vhf_ds5") => Some("DualSense"),
        (_, "usbip_ds5") => Some("DualSense with waveform haptics (USB/IP)"),
        (_, "usbip_switch2") => Some("Switch 2 Pro (USB/IP)"),
        (_, "vhf_switch") => Some("Switch Pro"),
        (_, "vhf_steam") => Some("Steam Controller"),
        (_, "vigem_x360") => Some("Xbox 360 (ViGEmBus)"),
        (_, "vigem_ds4") => Some("DualShock 4 (ViGEmBus)"),
        (_, "steam_deck") => Some("Steam Deck"),
        (_, "virtual_pad") => Some("Virtual controller"),
        (_, "a") => Some("A (Cross)"),
        (_, "b") => Some("B (Circle)"),
        (_, "x") => Some("X (Square)"),
        (_, "y") => Some("Y (Triangle)"),
        (_, "lb") => Some("Left bumper (L1)"),
        (_, "rb") => Some("Right bumper (R1)"),
        (_, "lt") => Some("Left trigger (L2)"),
        (_, "rt") => Some("Right trigger (R2)"),
        (_, "l3") => Some("Left stick click (L3)"),
        (_, "r3") => Some("Right stick click (R3)"),
        (_, "back") => Some("Back (Select)"),
        (_, "start") => Some("Start (Options)"),
        (_, "guide") => Some("Guide (Home)"),
        (_, "dpad_up") => Some("D-pad up"),
        (_, "dpad_down") => Some("D-pad down"),
        (_, "dpad_left") => Some("D-pad left"),
        (_, "dpad_right") => Some("D-pad right"),
        (_, "touchpad") => Some("Touchpad click"),
        (_, "misc") => Some("Share (Xbox) or mute (DualSense)"),
        (_, "ipv4") => Some("IPv4 only"),
        (_, "both") => Some("IPv4 and IPv6"),
        (_, "pc") => Some("This PC only"),
        (_, "lan") => Some("Local network"),
        (_, "wan") => Some("Any network"),
        (_, "per_client") => Some("One for each device"),
        (_, "shared") => Some("One shared by all devices"),
        (_, "game-provided") => Some("Provided by the game"),
        (_, "keep_while_paused") => Some("Keep while paused"),
        _ => None,
    }
}
