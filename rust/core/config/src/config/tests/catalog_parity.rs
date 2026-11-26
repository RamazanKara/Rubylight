//! Parity and snapshot tests verifying generated tables and typed accessors match `settings.toml`.
use super::*;
use std::collections::BTreeSet;

#[test]
fn config_default_snapshot_covers_all_generated_typed_accessors() {
    let c = Config::default();
    assert!(c.idd_push_fence_ring());
    assert!(c.packed_444());
    assert!(c.hardware_metrics());
    assert_eq!(c.hardware_metrics_interval(), 1000);
    assert!(c.client_roaming());
    assert!(c.hdr_dynamic_metadata());
    assert!(c.audio_passthrough());
    assert!(c.bandwidth_probe());
    assert!(c.frame_warp());
    assert!(c.keyboard_follows_focus());
    assert!(!c.text_input_surrounding_text());
    assert!(c.hdr_dolby_vision());
    assert!(c.hdr_hlg());
    assert!(!c.webrtc_server());
    assert!(!c.file_transfer());
    assert!(!c.usb_forwarding());
    assert_eq!(c.usb_max_devices(), 4);
    assert!(c.couch_menu());
    assert_eq!(c.file_transfer_dir(), None);
    assert_eq!(c.file_transfer_max_bytes(), 4096 * 1024 * 1024);
    assert!(c.clipboard_image());
    assert_eq!(c.clipboard_image_max_bytes(), 16 * 1024 * 1024);
    assert!(c.webrtc_pacing());
    assert!(c.webrtc_rtx());
    assert!(!c.amd_output_prediction());
    assert!(c.amd_output_thread());
    assert!(c.nvenc_output_event());
    assert!(!c.pyrowave_encode_overlap());
    assert_eq!(c.webrtc_loss_permille(), 0);
    assert_eq!(c.webrtc_port(), WEBRTC_PORT);
    assert_eq!(c.webrtc_media_port(), WEBRTC_MEDIA_PORT);
    assert!(!c.dsu_server_lan());
    assert_eq!(c.remove_remote_monitor(), RemoveRemoteMonitor::Never);
    assert_eq!(c.bitrate_adaptation(), BitrateAdaptation::On);
    assert_eq!(c.updates(), UpdatesMode::TellMe);
    assert!(c.audio_surround_714());
    assert!(c.pen_barrel_roll());
    assert_eq!(c.touch_mode(), TouchMode::Client);
    assert_eq!(c.touch_sensitivity(), 100);
    assert!(c.touch_gestures());
    assert!(c.client_cursor());
    assert!(c.qr_pairing());
    assert_eq!(c.dd_rotation_option(), DisplayAdjust::Disabled);
    assert_eq!(c.dd_manual_rotation(), 0);
    assert_eq!(c.nucleus_path(), None);
    assert_eq!(c.nucleus_coop_players(), 4);
}

#[test]
fn generated_tables_match_settings_toml_counts_and_uniqueness() {
    let toml = include_str!("../../../settings.toml").replace("\r\n", "\n");
    let mut toml_keys = BTreeSet::new();
    let mut toml_overrides = BTreeSet::new();
    let mut toml_restarts = BTreeSet::new();
    for block in toml.split("\n[[setting]]\n").skip(1) {
        let key = block
            .lines()
            .find_map(|line| line.strip_prefix("key = \"")?.strip_suffix('"'))
            .expect("every [[setting]] block has a key");
        assert!(
            toml_keys.insert(key),
            "duplicate key in settings.toml: {key}"
        );
        if block.lines().any(|line| line == "override_allowed = true") {
            toml_overrides.insert(key);
        }
        if block.lines().any(|line| line == "restart = true") {
            toml_restarts.insert(key);
        }
    }
    assert_eq!(toml_keys.len(), 296);

    let generated_overrides: BTreeSet<&str> = super::super::catalog_gen::OVERRIDE_ALLOWED
        .iter()
        .copied()
        .collect();
    assert_eq!(
        generated_overrides.len(),
        super::super::catalog_gen::OVERRIDE_ALLOWED.len()
    );
    assert_eq!(generated_overrides, toml_overrides);
    assert_eq!(generated_overrides.len(), 149);
    for key in &generated_overrides {
        assert!(override_allowed(key), "{key}");
        assert!(!retired_name(key), "{key} is retired yet overridable");
    }

    let generated_restarts: BTreeSet<&str> = RESTART_KEYS.iter().copied().collect();
    assert_eq!(generated_restarts.len(), RESTART_KEYS.len());
    assert_eq!(generated_restarts, toml_restarts);
    assert_eq!(generated_restarts.len(), 33);

    assert_eq!(HARDWARE_METRICS.len(), 15);
    assert_eq!(SETTINGS_30.len(), 12);
    assert_eq!(RETIRED_VALUES.len(), 2);
    assert_eq!(RENAMED_KEYS.len(), 5);
    assert_eq!(REMOVED_KEYS.len(), 20);
    assert_eq!(REMOVED_PREFIXES.len(), 2);
    assert_eq!(FOLDED_KEYS.len(), 11);
    assert_eq!(MIN_QP.len(), 3);
    assert_eq!(REMOVED_APP_KEYS.len(), 1);
    assert_eq!(RENAMED_APP_SWITCHES.len(), 1);
}

#[test]
fn generated_restart_keys_match_host_web_config() {
    let web_config = include_str!("../../../../../host/web/src/web/config.rs");
    let block = web_config
        .split("const RESTART_KEYS: &[&str] = &[")
        .nth(1)
        .and_then(|rest| rest.split("];").next())
        .expect("RESTART_KEYS in host/web/src/web/config.rs");
    let web_keys: Vec<&str> = block
        .lines()
        .map(|line| line.split("//").next().unwrap_or(line))
        .filter_map(|line| {
            let start = line.find('"')? + 1;
            let end = line[start..].find('"')? + start;
            Some(&line[start..end])
        })
        .collect();
    assert_eq!(RESTART_KEYS, web_keys.as_slice());
}

#[test]
fn generated_console_catalog_preserves_layers_and_choice_labels() {
    use super::super::console_catalog::{APP, CLIENT, GLOBAL, GRIPS, choice_label};

    assert_eq!(GRIPS.len(), 20);
    assert_eq!(GLOBAL.len(), 188);
    assert_eq!(APP.len(), 41);
    assert_eq!(CLIENT.len(), 22);

    let global_keys: BTreeSet<&str> = GLOBAL.iter().map(|s| s.key).collect();
    assert_eq!(global_keys.len(), GLOBAL.len());
    let app_keys: BTreeSet<&str> = APP.iter().map(|s| s.key).collect();
    assert_eq!(app_keys.len(), APP.len());
    let client_keys: BTreeSet<&str> = CLIENT.iter().map(|s| s.key).collect();
    assert_eq!(client_keys.len(), CLIENT.len());

    // Key-scoped overrides take precedence over generic choice labels.
    assert_eq!(
        choice_label("virtual_display_refresh", "enabled"),
        Some("Four times the stream frame rate")
    );
    assert_eq!(
        choice_label("virtual_display_refresh", "480hz"),
        Some("480 Hz")
    );
    assert_eq!(choice_label("amd_split_frame", "enabled"), Some("On"));
    assert_eq!(
        choice_label("dd_hdr_option", "auto"),
        Some("Match the stream")
    );
    assert_eq!(choice_label("gamepad", "auto"), Some("Automatic"));
    assert_eq!(choice_label("locale", "en"), None);
    assert_eq!(
        choice_label("stream_preset", "reference"),
        Some("Reference quality")
    );
    assert_eq!(
        choice_label("bitrate_adaptation", "delay"),
        Some("On, and react to network delay")
    );
}
