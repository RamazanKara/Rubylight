//! Unit tests for host/device/app config overrides, stream presets, and background isolation.

use super::super::*;

#[test]
fn webrtc_launches_take_whole_frames_whatever_the_host_asks_for() {
    let f = crate::state::test_support::Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("video_slice_delivery".into(), "on".into());
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client, Role::Stream);
    let moonlight = effective_config(&f.host, &launch).unwrap();
    assert_eq!(moonlight.get("video_slice_delivery", ""), "on");
    launch.transport = butterpollo_session::session::Transport::WebRtc;
    let webrtc = effective_config(&f.host, &launch).unwrap();
    assert_eq!(webrtc.get("video_slice_delivery", ""), "off");
}
// Issue #73: a decoder that freezes on long-term references takes 0 for itself, whatever the
// client's budget, and the host's other devices keep theirs.
#[test]
fn a_devices_long_term_references_reach_its_stream_and_no_other_device() {
    use butterpollo_media_policy::encoder_policy::{amf_ltr_frames, amf_offers_invalidation};
    let f = crate::state::test_support::Fixture::new();
    let others = f.client(u32::MAX);
    let mut cube = others.clone();
    cube.extra.insert(
        "config_overrides".into(),
        serde_json::json!({"amd_ltr_frames": 0}),
    );
    let hevc = butterpollo_media_policy::rtsp::Negotiated {
        codec: 1,
        references: 0,
        ..Default::default()
    };
    let config = effective_config(&f.host, &f.launch(cube, Role::Stream)).unwrap();
    assert_eq!(amf_ltr_frames(&config, &hevc), 0);
    assert!(!amf_offers_invalidation(&config));
    let config = effective_config(&f.host, &f.launch(others, Role::Stream)).unwrap();
    assert_eq!(amf_ltr_frames(&config, &hevc), 4);
    assert!(amf_offers_invalidation(&config));
}
#[test]
fn overrides_saved_for_rtx_hdr_are_chosen_and_the_peak_renamed() {
    let mut config = Config::parse("port = 47989\n").unwrap();
    let overrides = serde_json::json!({
        "rtx_hdr": true,
        "rtx_hdr_contrast": 14,
        "rtx_hdr_peak_brightness": 1200,
        "encoder": "amf",
    });
    let overrides = overrides.as_object().unwrap();
    assert!(super::super::sets_peak(overrides));
    super::super::apply_overrides(&mut config, overrides).unwrap();
    assert_eq!(config.integer("hdr_peak_brightness", 0), 1200);
    assert_eq!(config.get("encoder", ""), "amf");
    // RTX HDR reads them as the device's or app's choice.
    assert!(butterpollo_media_policy::nvidia_enhance::rtx_hdr_enabled(
        &config
    ));
    assert_eq!(config.integer("rtx_hdr_contrast", 0), 14);
    let cleared = serde_json::json!({"rtx_hdr": null});
    super::super::apply_overrides(&mut config, cleared.as_object().unwrap()).unwrap();
    assert!(!butterpollo_media_policy::nvidia_enhance::rtx_hdr_enabled(
        &config
    ));
    assert!(!super::super::sets_peak(
        serde_json::json!({"hdr_peak_brightness": ""})
            .as_object()
            .unwrap()
    ));
}
#[test]
fn ignored_config_values_and_host_wide_overrides_are_logged() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    let path = f.host.directory.join("warnings.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    crate::test_log::capture_during(subscriber, || {
        let mut config = Config::parse("parity_number=not-a-number").unwrap();
        assert_eq!(config.integer("parity_number", 7), 7);
        apply_overrides(
            &mut config,
            serde_json::json!({"port":50000}).as_object().unwrap(),
        )
        .unwrap();
        assert!(!config.values.contains_key("port"));
    });
    let log = std::fs::read_to_string(path).unwrap();
    assert!(log.contains("parity_number"), "{log}");
    assert!(log.contains("not-a-number"), "{log}");
    assert!(
        log.contains("port") && log.contains("ignoring an override"),
        "{log}"
    );
}
#[test]
fn app_overrides_win_over_device_and_host_without_changing_saved_configuration() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    *f.host.config.write().unwrap() =
        Config::parse("max_bitrate=10000\ncapture=ddx\nport=47989\nkeyboard=true").unwrap();
    let original = f.host.config.read().unwrap().values.clone();
    let mut client = f.client(u32::MAX);
    client.extra.insert(
            "config_overrides".into(),
            json!({"max_bitrate":20000,"capture":"wgc","port":50000,"keyboard":false,"packet_pacing":"always"}),
        );
    let launch = f.launch(client, Role::Stream);
    let device = effective_config(&f.host, &launch).unwrap();
    assert_eq!(device.integer("max_bitrate", 0), 20000);
    assert_eq!(device.get("capture", ""), "wgc");
    assert!(!device.boolean("keyboard", true));
    assert!(device.adaptive_packet_pacing(0, false));
    assert!(
        !f.host
            .config
            .read()
            .unwrap()
            .adaptive_packet_pacing(0, false)
    );
    assert_eq!(device.integer("port", 0), 47989);
    f.host.apps.write().unwrap()[0].extra.insert(
            "config-overrides".into(),
            json!({"max_bitrate":30000,"capture":"","port":51000,"keyboard":true,"packet_pacing":"off"}),
        );
    let app = effective_config(&f.host, &launch).unwrap();
    assert_eq!(app.integer("max_bitrate", 0), 30000);
    assert_eq!(app.get("capture", ""), "wgc");
    assert!(app.boolean("keyboard", false));
    assert!(!app.adaptive_packet_pacing(0, false));
    assert_eq!(app.integer("port", 0), 47989);
    assert_eq!(f.host.config.read().unwrap().values, original);
    assert_eq!(
        launch.client.extra["config_overrides"]["max_bitrate"],
        20000
    );
    assert_eq!(
        launch.client.extra["config_overrides"]["packet_pacing"],
        "always"
    );
    assert_eq!(
        f.host.apps.read().unwrap()[0].extra["config-overrides"]["port"],
        51000
    );
}
#[test]
fn an_apps_window_frame_choice_reaches_the_stream() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let launch = f.launch(f.client(u32::MAX), Role::Stream);
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("capture-window-frame".into(), json!("client"));
    let config = effective_config(&f.host, &launch).unwrap();
    assert_eq!(
        config.get(butterpollo_media_policy::app_window::FRAME, ""),
        "client"
    );
    // A co-op player's stream is its window already and has no window settings.
    let mut coop = f.launch(f.client(u32::MAX), Role::Stream);
    coop.nucleus = Some(butterpollo_session::session::Seat {
        session: 1,
        slot: 0,
    });
    let config = effective_config(&f.host, &coop).unwrap();
    assert_eq!(
        config.get(butterpollo_media_policy::app_window::FRAME, ""),
        ""
    );
}
/// Every capture method that captures whole displays, AMD Direct Capture among them, gives way
/// to WGC for a stream of the app's window, with a warning, instead of failing to open.
#[test]
fn an_app_window_stream_captures_with_wgc_whatever_whole_display_method_is_set() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("capture-window".into(), json!("window"));
    for (method, name) in [
        ("ddx", "Desktop Duplication"),
        ("dxgi", "Desktop Duplication"),
        ("idd-push", "direct driver push"),
        ("amd", "AMD Direct Capture"),
    ] {
        *f.host.config.write().unwrap() = Config::parse(&format!("capture = {method}")).unwrap();
        let launch = f.launch(f.client(u32::MAX), Role::Stream);
        let config = effective_config(&f.host, &launch).unwrap();
        assert_eq!(config.get("capture", ""), "wgc", "{method}");
        let warnings = launch.warnings.snapshot();
        let warning = warnings
            .iter()
            .find(|w| w.code == "capture_app_window_method")
            .unwrap_or_else(|| panic!("no warning for {method}"));
        assert!(warning.message.contains(name), "{}", warning.message);
    }
}
#[test]
fn an_apps_block_for_the_device_wins_over_the_app_the_device_and_the_host() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    *f.host.config.write().unwrap() =
        Config::parse("max_bitrate=10000\nframe_limiter_fps_limit=0").unwrap();
    let mut client = f.client(u32::MAX);
    client.extra.insert(
        "config_overrides".into(),
        json!({"max_bitrate":20000,"stream_preset":"lowest_latency"}),
    );
    let launch = f.launch(client, Role::Stream);
    let device = launch.client.uuid.clone();
    let set = |overrides: serde_json::Value| {
        let mut apps = f.host.apps.write().unwrap();
        apps[0].extra.insert(
            "config-overrides".into(),
            json!({"max_bitrate":30000,"frame_limiter_fps_limit":60}),
        );
        apps[0].extra.insert("device-overrides".into(), overrides);
    };
    // Another device's block, or one for a device that is gone, changes nothing.
    set(json!({"another-device": {"config-overrides": {"max_bitrate": 50000}}}));
    let app = effective_config(&f.host, &launch).unwrap();
    assert_eq!(app.integer("max_bitrate", 0), 30000);
    assert_eq!(app.get("frame_limiter_fps_limit", ""), "60");
    assert_eq!(app.get("amd_max_frame_size", ""), "1");
    // The device's block wins key by key: an empty value keeps the app's.
    set(json!({
        device.as_str(): {"config-overrides": {
            "max_bitrate": 40000, "frame_limiter_fps_limit": "", "stream_preset": "reference"
        }},
        "another-device": {"config-overrides": {"max_bitrate": 50000}},
    }));
    let both = effective_config(&f.host, &launch).unwrap();
    assert_eq!(both.integer("max_bitrate", 0), 40000);
    assert_eq!(both.get("frame_limiter_fps_limit", ""), "60");
    assert_eq!(both.get("amd_max_frame_size", ""), "0");
    // The saved app keeps its block.
    assert!(
        f.host.apps.read().unwrap()[0]
            .extra
            .contains_key("device-overrides")
    );
}
#[test]
fn a_devices_controller_type_wins_over_the_hosts_and_an_apps_over_the_devices() {
    // Issue #68: a Switch running Moonlight reports no controller type, so it needs Switch Pro
    // of its own while every other device keeps the host's type.
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    *f.host.config.write().unwrap() = Config::parse("gamepad=vhf_xbox").unwrap();
    let other = f.client(u32::MAX);
    let mut switch = other.clone();
    switch.uuid = "switch".into();
    switch
        .extra
        .insert("config_overrides".into(), json!({"gamepad": "vhf_switch"}));
    let gamepad = |client: &butterpollo_session::state::Client| {
        effective_config(&f.host, &f.launch(client.clone(), Role::Stream))
            .unwrap()
            .get("gamepad", "")
            .to_owned()
    };
    assert_eq!(gamepad(&switch), "vhf_switch");
    assert_eq!(gamepad(&other), "vhf_xbox");
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("gamepad".into(), json!("vhf_ds5"));
    assert_eq!(gamepad(&switch), "vhf_ds5");
}
#[test]
fn stream_presets_fill_the_host_and_a_device_or_app_can_pick_its_own() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let set = |text: &str| *f.host.config.write().unwrap() = Config::parse(text).unwrap();
    let client = f.client(u32::MAX);
    let effective = |client: &butterpollo_session::state::Client| {
        effective_config(&f.host, &f.launch(client.clone(), Role::Stream)).unwrap()
    };
    // No preset is Balanced: nothing is added.
    set("max_bitrate=10000");
    let plain = effective(&client);
    assert!(!plain.values.contains_key("amd_max_frame_size"));
    assert!(!plain.values.contains_key("amd_quality"));
    set("stream_preset=balanced\nmax_bitrate=10000");
    assert_eq!(effective(&client).values.len(), plain.values.len() + 1);
    // The host's preset fills what it left unset; its own value wins.
    set("stream_preset=lowest_latency");
    assert_eq!(effective(&client).get("amd_max_frame_size", ""), "1");
    set("stream_preset=reference\namd_max_frame_size=4");
    let reference = effective(&client);
    assert_eq!(reference.get("amd_max_frame_size", ""), "4");
    // No preset changes the rate control.
    assert!(!reference.values.contains_key("amd_rc"));
    // A device's preset is the one that fills what no layer set: the
    // host's explicit values still win over it.
    set("stream_preset=reference\namd_quality=quality");
    let mut device = client.clone();
    device.extra.insert(
        "config_overrides".into(),
        json!({"stream_preset":"lowest_latency"}),
    );
    let config = effective(&device);
    assert_eq!(config.get("stream_preset", ""), "lowest_latency");
    assert_eq!(config.get("amd_max_frame_size", ""), "1");
    assert_eq!(config.get("amd_quality", ""), "quality");
    assert_eq!(config.get("amd_preanalysis", ""), "false");
    // The NVENC counterparts follow the same layers.
    assert_eq!(config.get("nvenc_vbv_increase", ""), "0");
    assert_eq!(config.get("nvenc_preset", ""), "1");
    assert_eq!(config.get("nvenc_temporal_aq", ""), "false");
    set("stream_preset=reference\nnvenc_preset=4");
    assert_eq!(effective(&device).get("nvenc_preset", ""), "4");
    let mut nvenc = client.clone();
    nvenc.extra.insert(
        "config_overrides".into(),
        json!({"stream_preset":"reference","nvenc_vbv_increase":"50"}),
    );
    assert_eq!(effective(&nvenc).get("nvenc_vbv_increase", ""), "50");
    set("stream_preset=reference\namd_max_frame_size=4");
    let config = effective(&device);
    assert_eq!(config.get("amd_max_frame_size", ""), "4");
    assert_eq!(config.get("amd_quality", ""), "speed");
    // The device's own values win too.
    set("stream_preset=reference");
    device.extra.insert(
        "config_overrides".into(),
        json!({"stream_preset":"lowest_latency","amd_max_frame_size":"3"}),
    );
    let config = effective(&device);
    assert_eq!(config.get("amd_max_frame_size", ""), "3");
    assert_eq!(config.get("amd_quality", ""), "speed");
    // A null or empty preset is no preset of its own: the host's stays.
    set("stream_preset=lowest_latency");
    for none in [json!(null), json!("")] {
        let mut device = client.clone();
        device
            .extra
            .insert("config_overrides".into(), json!({ "stream_preset": none }));
        let config = effective(&device);
        assert_eq!(config.get("stream_preset", ""), "lowest_latency");
        assert_eq!(config.get("amd_max_frame_size", ""), "1");
    }
    // An app's preset beats the device's for what is unset, and the
    // device's explicit value still wins over it.
    set("");
    f.host.apps.write().unwrap()[0].extra.insert(
        "config-overrides".into(),
        json!({"stream_preset":"reference"}),
    );
    let config = effective(&client);
    assert_eq!(config.get("amd_max_frame_size", ""), "0");
    assert_eq!(config.get("nvenc_vbv_increase", ""), "400");
    assert!(!config.values.contains_key("amd_quality"));
    assert!(!config.values.contains_key("nvenc_preset"));
    let config = effective(&device);
    assert_eq!(config.get("stream_preset", ""), "reference");
    assert_eq!(config.get("amd_max_frame_size", ""), "3");
    // Reference names no quality preset, and the device's own preset gave way to the app's.
    assert!(!config.values.contains_key("amd_quality"));
    // An app's explicit value wins over its own preset and the device's.
    f.host.apps.write().unwrap()[0].extra.insert(
        "config-overrides".into(),
        json!({"stream_preset":"lowest_latency","amd_max_frame_size":"auto"}),
    );
    let config = effective(&client);
    assert_eq!(config.get("amd_max_frame_size", ""), "auto");
    assert_eq!(config.get("amd_quality", ""), "speed");
    // An unknown name is ignored: the layer below keeps its preset.
    f.host.apps.write().unwrap()[0].extra.insert(
        "config-overrides".into(),
        json!({"stream_preset":"fastest"}),
    );
    set("stream_preset=reference\namd_quality=quality");
    let config = effective(&client);
    assert_eq!(config.get("stream_preset", ""), "reference");
    assert_eq!(config.get("amd_max_frame_size", ""), "0");
    assert_eq!(config.get("amd_quality", ""), "quality");
}
#[test]
fn a_background_stream_keeps_keyboard_and_mouse_off_whatever_overrides_say() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let mut client = f.client(u32::MAX);
    client.extra.insert(
        "config_overrides".into(),
        json!({"keyboard":true,"mouse":true,"audio_capture_mode":"system"}),
    );
    let plain = effective_config(&f.host, &f.launch(client.clone(), Role::Stream)).unwrap();
    assert!(!butterpollo_session::background::enabled(&plain));
    assert!(plain.boolean("keyboard", false));
    // From the app.
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("background-stream".into(), json!(true));
    let launch = f.launch(client.clone(), Role::Stream);
    let app = effective_config(&f.host, &launch).unwrap();
    assert!(butterpollo_session::background::enabled(&app));
    assert!(!app.boolean("keyboard", true));
    assert!(!app.boolean("mouse", true));
    assert_eq!(app.get("audio_capture_mode", ""), "game");
    assert_eq!(
        crate::display_session::virtual_display_layout(
            &app,
            &launch.client,
            None,
            Some(
                butterpollo_media_policy::display_policy::HostDisplay::Virtual(
                    butterpollo_media_policy::display_policy::Arrangement::Exclusive
                )
            ),
        ),
        "extended_isolated"
    );
    // From the device, for every app; only streams take it.
    f.host.apps.write().unwrap()[0]
        .extra
        .remove("background-stream");
    client.extra.insert("background_stream".into(), json!(true));
    let device = effective_config(&f.host, &f.launch(client.clone(), Role::Stream)).unwrap();
    assert!(butterpollo_session::background::enabled(&device));
    let input = effective_config(&f.host, &f.launch(client, Role::InputOnly)).unwrap();
    assert!(!butterpollo_session::background::enabled(&input));
    // A saved effective-config key must enforce the same isolation.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("background_stream".into(), "true".into());
    let configured =
        effective_config(&f.host, &f.launch(f.client(u32::MAX), Role::Stream)).unwrap();
    assert!(!configured.boolean("keyboard", true));
    assert!(!configured.boolean("mouse", true));
    assert_eq!(configured.get("audio_capture_mode", ""), "game");
}
#[test]
fn a_launch_keeps_its_background_mode_when_settings_change() {
    let f = crate::state::test_support::Fixture::new();
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    launch.background = Some(true);
    for audio in ["system", "game"] {
        f.host.apps.write().unwrap()[0].extra.insert(
            "config-overrides".into(),
            serde_json::json!({"keyboard":true, "mouse":true, "audio_capture_mode":audio}),
        );
        let config = effective_config(&f.host, &launch.clone()).unwrap();
        assert!(butterpollo_session::background::enabled(&config));
        assert!(!config.boolean("keyboard", true));
        assert!(!config.boolean("mouse", true));
        assert_eq!(config.get("audio_capture_mode", ""), "game");
    }
    launch.background = Some(false);
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("background-stream".into(), true.into());
    let config = effective_config(&f.host, &launch).unwrap();
    assert!(!butterpollo_session::background::enabled(&config));
    assert!(config.boolean("keyboard", false));
    assert!(config.boolean("mouse", false));
}
#[test]
fn an_override_the_host_cannot_use_is_skipped_and_the_rest_apply() {
    let mut config = Config::default();
    let overrides = serde_json::json!({"frame_limiter_fps_limit": "-1", "fec_percentage": "30"});
    apply_overrides(&mut config, overrides.as_object().unwrap()).unwrap();
    assert_eq!(config.get("fec_percentage", ""), "30");
    assert!(!config.values.contains_key("frame_limiter_fps_limit"));
}
/// An app's or device's retired HDR request without a Display HDR of its own
/// is kept as saved and folds over the layers below when applied, as it did
/// beside them.
#[test]
fn an_hdr_request_alone_in_overrides_folds_over_the_layers_below() {
    let apply = |base: &str, layer: serde_json::Value| {
        let mut config = Config::parse(base).unwrap();
        super::super::apply_overrides(&mut config, layer.as_object().unwrap()).unwrap();
        config.get("dd_hdr_option", "auto").to_owned()
    };
    let request = |value: serde_json::Value| serde_json::json!({"dd_hdr_request_override": value});
    // Don't change HDR below still wins over it.
    assert_eq!(
        apply("dd_hdr_option = disabled\n", request("force_on".into())),
        "disabled"
    );
    assert_eq!(apply("", request("force_on".into())), "force_on");
    assert_eq!(
        apply("dd_hdr_option = force_on\n", request("force_off".into())),
        "force_off"
    );
    // Follow the device ends the host's Always on, as it ended its request.
    assert_eq!(
        apply(
            "dd_hdr_request_override = force_on\n",
            request("auto".into())
        ),
        "auto"
    );
    assert_eq!(
        apply(
            "dd_hdr_option = force_off\n",
            request(serde_json::Value::Null)
        ),
        "auto"
    );
    assert_eq!(
        apply("dd_hdr_option = disabled\n", request("auto".into())),
        "disabled"
    );
    // Beside the layer's own Display HDR it folds in that layer.
    assert_eq!(
        apply(
            "dd_hdr_option = disabled\n",
            serde_json::json!({"dd_hdr_option": "auto", "dd_hdr_request_override": "force_off"})
        ),
        "force_off"
    );
}
#[test]
fn a_launch_without_hdr_dynamic_metadata_wins_over_app_and_device_overrides() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let mut client = f.client(u32::MAX);
    client.extra.insert(
        "config_overrides".into(),
        json!({"hdr_dynamic_metadata": true}),
    );
    let mut launch = f.launch(client, Role::Stream);
    f.host.apps.write().unwrap()[0].extra.insert(
        "config-overrides".into(),
        json!({"hdr_dynamic_metadata": true}),
    );
    let dynamic = |launch: &_| {
        effective_config(&f.host, launch)
            .unwrap()
            .boolean("hdr_dynamic_metadata", false)
    };
    assert!(dynamic(&launch));
    launch.options.insert("rlHdrDynamic".into(), "0".into());
    assert!(!dynamic(&launch));
}
