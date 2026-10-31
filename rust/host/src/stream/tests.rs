#[test]
fn webrtc_sessions_are_not_ended_for_lacking_an_enet_control_peer() {
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let moonlight = f.session(client.clone(), Role::Stream);
    assert!(super::expects_enet_control(&moonlight));
    let mut launch = f.launch(client, Role::Stream);
    launch.transport = butterpollo_core::session::Transport::WebRtc;
    let webrtc = Session::new(launch, Default::default());
    assert!(!super::expects_enet_control(&webrtc));
}
#[test]
fn the_control_watchdog_ends_a_moonlight_session_without_a_peer_but_not_a_webrtc_one() {
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let moonlight = f.session(client.clone(), Role::Stream);
    let mut launch = f.launch(client, Role::Stream);
    launch.transport = butterpollo_core::session::Transport::WebRtc;
    let webrtc = Session::new(launch, Default::default());
    assert_ne!(moonlight.launch.id, webrtc.launch.id);
    let timeout = Duration::from_millis(20);
    let mut unattended = HashMap::new();
    let control = ReplySocket::bind("127.0.0.1:0").unwrap();
    let sweep = |unattended: &mut HashMap<String, Instant>| {
        let mut sessions = vec![moonlight.clone(), webrtc.clone()];
        super::end_unattended_sessions(&mut sessions, |_| false, unattended, timeout, &control);
    };
    // The clock starts at the first sweep, so nothing ends yet.
    sweep(&mut unattended);
    assert!(!moonlight.stopping() && !webrtc.stopping());
    std::thread::sleep(timeout * 3);
    sweep(&mut unattended);
    assert!(
        moonlight.stopping(),
        "a GameStream session with no control peer must end"
    );
    assert!(
        !webrtc.stopping(),
        "a WebRTC session carries control on its DataChannels and has no ENet peer"
    );
    assert!(!unattended.contains_key(&webrtc.launch.id));
}

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
    launch.transport = butterpollo_core::session::Transport::WebRtc;
    let webrtc = effective_config(&f.host, &launch).unwrap();
    assert_eq!(webrtc.get("video_slice_delivery", ""), "off");
}
#[test]
fn overrides_saved_for_rtx_hdr_keep_only_the_peak_brightness() {
    let mut config = Config::parse("port = 47989\n").unwrap();
    let overrides = serde_json::json!({
        "rtx_hdr": true,
        "rtx_hdr_contrast": 14,
        "rtx_hdr_peak_brightness": 1200,
        "encoder": "amf",
    });
    let overrides = overrides.as_object().unwrap();
    assert!(super::sets_peak(overrides));
    super::apply_overrides(&mut config, overrides).unwrap();
    assert_eq!(config.integer("hdr_peak_brightness", 0), 1200);
    assert_eq!(config.get("encoder", ""), "amf");
    assert!(!config.values.contains_key("rtx_hdr"));
    assert!(!config.values.contains_key("rtx_hdr_contrast"));
    assert!(!super::sets_peak(
        serde_json::json!({"hdr_peak_brightness": ""})
            .as_object()
            .unwrap()
    ));
}
#[test]
fn waveform_haptics_require_negotiation_and_encrypted_multichannel_control() {
    let mut config = butterpollo_core::rtsp::Negotiated::from_sdp(
        b"a=x-ml-general.featureFlags:7\na=x-ss-general.encryptionEnabled:1\n",
    )
    .unwrap();
    assert!(haptics_negotiated(&config, 9));
    assert!(!haptics_negotiated(&config, 8));
    config.encryption = 0;
    assert!(!haptics_negotiated(&config, 9));
    config.encryption = 1;
    config.moonlight_features = 3;
    assert!(!haptics_negotiated(&config, 9));
    assert!(!haptics_negotiated(&Default::default(), 9));
}
#[test]
fn with_rumble_off_only_the_first_stop_after_a_buzz_goes_out() {
    // An Xbox report: main and trigger motors, as VHF reports kind 4.
    let packets = |data: [u8; 8]| feedback_packets(3, 4, &data);
    let (on, off) = (packets([0, 9, 0, 7, 1, 0, 2, 0]), packets([0; 8]));
    const NONE: [u16; 0] = [];
    let mut rumble = RumbleForwarded::default();
    let mut forward = |allowed: bool, packets: &[(u16, Vec<u8>)]| {
        packets
            .iter()
            .filter(|(kind, payload)| rumble.forward(allowed, *kind, payload))
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>()
    };
    // Nothing ran: rumble off sends no stops, however often VHF repeats
    // its report for a lightbar or trigger effect change.
    assert_eq!(forward(false, &off), NONE);
    assert_eq!(forward(false, &off), NONE);
    // A buzz while rumble is on, then the client turns rumble off.
    assert_eq!(forward(true, &on), [RUMBLE_DATA, RUMBLE_TRIGGER_DATA]);
    assert_eq!(forward(false, &on), NONE);
    // Both motors' stops go out once, then no more.
    assert_eq!(forward(false, &off), [RUMBLE_DATA, RUMBLE_TRIGGER_DATA]);
    assert_eq!(forward(false, &off), NONE);
    // Rumble on forwards everything, stops included.
    assert_eq!(forward(true, &off), [RUMBLE_DATA, RUMBLE_TRIGGER_DATA]);
    // Other controllers and kinds follow the injector alone.
    let other = feedback_packets(4, RUMBLE_DATA, &[0; 4]);
    assert_eq!(forward(false, &other), NONE);
    assert!(!rumble.forward(false, 0x5502, &[3, 0, 1, 2, 3]));
    assert!(rumble.forward(true, 0x5502, &[3, 0, 1, 2, 3]));
}
#[test]
fn the_control_pass_sends_feedback_through_the_rumble_filter() {
    // The pass the control loop runs, so rumble off still stops a buzz once.
    let report = |data: [u8; 8]| PadReport::Feedback {
        id: 3,
        kind: 4,
        data: data.to_vec(),
    };
    let mut pcm = Default::default();
    let mut arbiter = Default::default();
    let mut rumble = RumbleForwarded::default();
    let mut pass = |allowed: bool, data: [u8; 8]| {
        pad_reports(
            [report(data)],
            |_| allowed,
            true,
            &[0; 16],
            &mut pcm,
            &mut arbiter,
            &mut rumble,
        )
        .messages
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>()
    };
    let both = [RUMBLE_DATA, RUMBLE_TRIGGER_DATA];
    assert_eq!(pass(true, [0, 9, 0, 7, 1, 0, 2, 0]), both);
    assert_eq!(pass(false, [0; 8]), both);
    assert_eq!(pass(false, [0; 8]), [0u16; 0]);
}
#[test]
fn composite_feedback_preserves_partial_trigger_flags_and_rumble_layout() {
    let mut effect = vec![0; 23];
    effect[..3].copy_from_slice(&[4, 1, 0]);
    let packet = feedback_packets(2, 0x5503, &effect).remove(0);
    assert_eq!(&packet.1[..5], &[2, 0, 4, 1, 0]);
    let rumble = feedback_packets(2, RUMBLE_DATA, &[0, 9, 0, 7]).remove(0);
    assert_eq!(&rumble.1, &[0xee, 0xff, 0xc0, 0, 2, 0, 0, 9, 0, 7]);
}
#[test]
fn control_backlog_is_serviced_without_the_feedback_delay() {
    for queued in [600, 1100] {
        let mut remaining = queued;
        while remaining >= super::CONTROL_EVENT_LIMIT {
            assert_eq!(
                super::control_wait_timeout(Duration::from_millis(8), super::CONTROL_EVENT_LIMIT),
                0
            );
            remaining -= super::CONTROL_EVENT_LIMIT;
        }
        assert_eq!(
            super::control_wait_timeout(Duration::from_millis(8), remaining),
            8
        );
    }
    assert_eq!(super::control_wait_timeout(Duration::ZERO, 0), 0);
    assert_eq!(
        super::control_wait_timeout(Duration::from_micros(100), 0),
        1
    );
}
use super::*;
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
fn server_commands_require_permission_app_opt_in_and_a_valid_index_and_are_throttled() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let mut client = f.client(1 << 20);
    f.host.config.write().unwrap().values.insert("server_cmd".into(), json!([
            {"name":"First","cmd":"echo first"}, {"name":"Second","cmd":"echo second","elevated":true}
        ]).to_string());
    let mut at = None;
    assert_eq!(server_command(&f.host, &client, &[1], false, &mut at), None);
    assert!(at.is_none());
    assert_eq!(
        server_command(&f.host, &client, &[1], true, &mut at),
        Some(("echo second".into(), true))
    );
    assert!(at.is_some());
    assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
    at = Some(Instant::now() - Duration::from_secs(2));
    assert_eq!(
        server_command(&f.host, &client, &[0], true, &mut at),
        Some(("echo first".into(), false))
    );
    for payload in [vec![], vec![0, 1], vec![2]] {
        at = None;
        assert_eq!(
            server_command(&f.host, &client, &payload, true, &mut at),
            None
        );
    }
    for (permission, enabled) in [(0, true), (1 << 20, false)] {
        client.perm = permission;
        client.enabled = enabled;
        at = None;
        assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
        assert!(at.is_none());
    }
    client.perm = 1 << 20;
    client.enabled = true;
    let app = serde_json::from_value(json!({"name":"No commands","allow-client-commands":false}))
        .unwrap();
    *f.host.current_app.lock().unwrap() =
        Some(crate::process::RunningApp::with_environment(&app, Default::default()).unwrap());
    assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
    assert!(at.is_none());
}
#[test]
fn the_stream_thread_skips_its_game_look_while_another_thread_holds_the_app() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    *f.host.current_app.lock().unwrap() = Some(
        crate::process::RunningApp::with_environment(
            &butterpollo_core::state::App::desktop(),
            Default::default(),
        )
        .unwrap(),
    );
    // Housekeeping holds the app for 400 ms, as on a PC busy loading a game.
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let host = &f.host;
        scope.spawn(move || {
            let _current = host.current_app.lock().unwrap();
            held.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_millis(400));
        });
        holding.recv().unwrap();
        // A blocking look would wait the whole 400 ms and then report true.
        assert!(!observe_foreground_game(
            &f.host, "client", "game.exe", 4242
        ));
        release.send(()).unwrap();
    });
    // Free again, the look goes through.
    assert!(observe_foreground_game(&f.host, "client", "game.exe", 4242));
}
#[test]
fn a_program_another_program_started_is_checked_off_the_stream_thread() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    let mut app = crate::process::RunningApp::with_environment(
        &butterpollo_core::state::App::desktop(),
        Default::default(),
    )
    .unwrap();
    app.mark_launching();
    *f.host.current_app.lock().unwrap() = Some(app);
    // A "game" started after the launch by a program other than the
    // desktop (this test), as a store client starts one.
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let program = format!(r"{root}\System32\PING.EXE");
    /// Ends the stand-in game however the test ends.
    struct Game(std::process::Child);
    impl Drop for Game {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let game = Game(
        std::process::Command::new(&program)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let game_up = || {
        f.host
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .launch_progress()
            .map(|(up, _)| up)
    };
    // Each look hands a check off and returns, as the stream's once-a-second
    // look does: the process list is read and the result applied on the
    // check's own thread.
    let deadline = Instant::now() + Duration::from_secs(10);
    while game_up() != Some(true) {
        assert!(Instant::now() < deadline, "the check never found the game");
        assert!(observe_foreground_game(
            &f.host,
            "client",
            &program,
            game.0.id()
        ));
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn input_skips_controller_cleanup_while_another_thread_holds_the_app() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    let due = Instant::now() - Duration::from_secs(2);
    let mut last = due;
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let host = &f.host;
        scope.spawn(move || {
            let _current = host.current_app.lock().unwrap();
            held.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_secs(5));
        });
        holding.recv().unwrap();
        expire_controllers(&f.host, &mut last);
        // Skipped, not waited for: still due.
        assert_eq!(last, due);
        release.send(()).unwrap();
    });
    expire_controllers(&f.host, &mut last);
    assert!(last > due);
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
            json!({"max_bitrate":20000,"capture":"wgc","port":50000,"keyboard":false,"host_adaptive_pacing":true}),
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
            json!({"max_bitrate":30000,"capture":"","port":51000,"keyboard":true,"host_adaptive_pacing":false}),
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
        launch.client.extra["config_overrides"]["host_adaptive_pacing"],
        true
    );
    assert_eq!(
        f.host.apps.read().unwrap()[0].extra["config-overrides"]["port"],
        51000
    );
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
fn stream_presets_fill_the_host_and_a_device_or_app_can_pick_its_own() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let set = |text: &str| *f.host.config.write().unwrap() = Config::parse(text).unwrap();
    let client = f.client(u32::MAX);
    let effective = |client: &butterpollo_core::state::Client| {
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
    assert!(!config.values.contains_key("amd_quality"));
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
    assert!(!butterpollo_core::background::enabled(&plain));
    assert!(plain.boolean("keyboard", false));
    // From the app.
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("background-stream".into(), json!(true));
    let launch = f.launch(client.clone(), Role::Stream);
    let app = effective_config(&f.host, &launch).unwrap();
    assert!(butterpollo_core::background::enabled(&app));
    assert!(!app.boolean("keyboard", true));
    assert!(!app.boolean("mouse", true));
    assert_eq!(app.get("audio_capture_mode", ""), "game");
    assert_eq!(
        crate::display_session::virtual_display_layout(
            &app,
            &launch.client,
            None,
            Some(butterpollo_core::display_policy::HostDisplay::Virtual(
                butterpollo_core::display_policy::Arrangement::Exclusive
            )),
        ),
        "extended_isolated"
    );
    // From the device, for every app; only streams take it.
    f.host.apps.write().unwrap()[0]
        .extra
        .remove("background-stream");
    client.extra.insert("background_stream".into(), json!(true));
    let device = effective_config(&f.host, &f.launch(client.clone(), Role::Stream)).unwrap();
    assert!(butterpollo_core::background::enabled(&device));
    let input = effective_config(&f.host, &f.launch(client, Role::InputOnly)).unwrap();
    assert!(!butterpollo_core::background::enabled(&input));
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
        assert!(butterpollo_core::background::enabled(&config));
        assert!(!config.boolean("keyboard", true));
        assert!(!config.boolean("mouse", true));
        assert_eq!(config.get("audio_capture_mode", ""), "game");
    }
    launch.background = Some(false);
    f.host.apps.write().unwrap()[0]
        .extra
        .insert("background-stream".into(), true.into());
    let config = effective_config(&f.host, &launch).unwrap();
    assert!(!butterpollo_core::background::enabled(&config));
    assert!(config.boolean("keyboard", false));
    assert!(config.boolean("mouse", false));
}
#[test]
fn capture_waits_for_its_display_and_shows_the_primary_only_after_the_timeout() {
    let start = Instant::now();
    let mut missing = None;
    assert_eq!(
        reopen_on(true, false, &mut missing, start),
        ReopenOn::Stream
    );
    // Windows switched the virtual display off: capture waits for the
    // heartbeat to bring it back instead of opening the physical desktop.
    assert_eq!(reopen_on(false, false, &mut missing, start), ReopenOn::Wait);
    let almost = start + DISPLAY_RETURN_WAIT - Duration::from_millis(1);
    assert_eq!(
        reopen_on(false, false, &mut missing, almost),
        ReopenOn::Wait
    );
    assert_eq!(
        reopen_on(false, false, &mut missing, start + DISPLAY_RETURN_WAIT),
        ReopenOn::Primary
    );
    // It came back under another name: the wait starts over next time.
    let back = start + Duration::from_secs(20);
    assert_eq!(reopen_on(true, false, &mut missing, back), ReopenOn::Stream);
    assert_eq!(missing, None);
    assert_eq!(reopen_on(false, false, &mut missing, back), ReopenOn::Wait);
}
#[test]
fn a_game_window_target_reopens_on_itself_never_the_primary_display() {
    // A Nucleus Co-op player's window: closed, on a missing display or
    // malformed, the reopen reports it instead of showing the PC's screen.
    let start = Instant::now();
    for target in [
        r"window:12@\\.\NO_SUCH_DISPLAY",
        r"window:0@\\.\DISPLAY1",
        "window:garbage",
    ] {
        let present = butterpollo_windows::capture::display_present(target);
        let mut missing = None;
        for background in [false, true] {
            for elapsed in [Duration::ZERO, DISPLAY_RETURN_WAIT, Duration::from_secs(61)] {
                assert_eq!(
                    reopen_on(present, background, &mut missing, start + elapsed),
                    ReopenOn::Stream,
                    "{target}"
                );
            }
        }
        assert_eq!(missing, None, "{target}");
    }
}
#[test]
fn background_capture_never_recovers_onto_the_primary_display() {
    let start = Instant::now();
    let mut missing = None;
    for elapsed in [Duration::ZERO, DISPLAY_RETURN_WAIT, Duration::from_secs(60)] {
        assert_eq!(
            reopen_on(false, true, &mut missing, start + elapsed),
            ReopenOn::Wait
        );
    }
    assert_eq!(
        reopen_on(true, true, &mut missing, start + Duration::from_secs(61)),
        ReopenOn::Stream
    );
    assert_eq!(missing, None);

    let plain = Config::default();
    let mut background = plain.clone();
    butterpollo_core::background::apply(&mut background);
    for capture in ["auto", "wgc", "ddx"] {
        assert_ne!(
            CaptureKey::new(capture, "display", false, &plain, ""),
            CaptureKey::new(capture, "display", false, &background, "")
        );
    }
}
#[test]
fn pad_feedback_forwards_rumble_and_each_family_extra() {
    let rumble = (
        RUMBLE_DATA,
        vec![0xee, 0xff, 0xc0, 0, 2, 0, 255, 255, 128, 128],
    );
    // Xbox pads add trigger rumble.
    let xbox = feedback_packets(2, 4, &[255, 255, 128, 128, 1, 2, 3, 4]);
    assert_eq!(
        xbox,
        vec![
            rumble.clone(),
            (RUMBLE_TRIGGER_DATA, vec![2, 0, 1, 2, 3, 4])
        ]
    );
    // PlayStation pads add the lightbar only when the report sets it.
    let ds4 = feedback_packets(2, 5, &[255, 255, 128, 128, 12, 34, 56, 1]);
    assert_eq!(ds4, vec![rumble.clone(), (0x5502, vec![2, 0, 12, 34, 56])]);
    assert_eq!(
        feedback_packets(2, 5, &[255, 255, 128, 128, 12, 34, 56, 0]),
        vec![rumble]
    );
    // Reports of no known kind send nothing.
    assert!(feedback_packets(2, 3, &[255; 8]).is_empty());
}
#[test]
fn vhf_rumble_and_stop_keep_the_strengths_in_moonlight_packets() {
    for (kind, size) in [(4, 8), (5, 32)] {
        for strengths in [[0x00, 0x80, 0x00, 0x40], [0; 4]] {
            let mut data = vec![0; size];
            data[..4].copy_from_slice(&strengths);
            let packets = feedback_packets(0, kind, &data);
            assert_eq!(packets[0].0, RUMBLE_DATA);
            assert_eq!(RUMBLE_DATA, 0x010b);
            assert_eq!(&packets[0].1[..4], &RUMBLE_MARKER.to_le_bytes());
            assert_eq!(&packets[0].1[4..], &[&[0, 0][..], &strengths].concat());
        }
    }
}
#[test]
fn rumble_reaches_a_single_channel_moonlight_peer() {
    let settings = || HostSettings {
        peer_limit: 1,
        channel_limit: 1,
        ..Default::default()
    };
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let address = socket.local_addr().unwrap();
    let mut host = Host::new(ControlSocket::new(socket), settings()).unwrap();
    let mut client = Host::new(UdpSocket::bind("127.0.0.1:0").unwrap(), settings()).unwrap();
    client.connect(address, 1, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !host.peer(PeerID(0)).connected() || !client.peer(PeerID(0)).connected() {
        host.service().unwrap();
        client.service().unwrap();
        assert!(Instant::now() < deadline, "ENet connect timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.peer(PeerID(0)).channel_count(), 1);
    // The previous channel silently discarded every feedback message.
    assert_eq!(
        host.peer_mut(PeerID(0))
            .send(1, &Packet::new(&[][..], PacketKind::Reliable)),
        Err(rusty_enet::error::PeerSendError::InvalidChannel)
    );
    let key = [0x42; 16];
    for v2 in [false, true] {
        for strengths in [[0x00, 0x80, 0x00, 0x40], [0; 4]] {
            let mut data = vec![0; 8];
            data[..4].copy_from_slice(&strengths);
            let (kind, payload) = feedback_packets(2, 4, &data).remove(0);
            let message =
                butterpollo_core::packet::encrypted_control(&key, 7, kind, &payload, v2).unwrap();
            host.socket_mut().hold();
            host.peer_mut(PeerID(0))
                .send(
                    FEEDBACK_CHANNEL,
                    &Packet::new(message, PacketKind::Reliable),
                )
                .unwrap();
            host.flush();
            host.socket_mut().release().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let received = loop {
                host.service().unwrap();
                if let Some(Event::Receive {
                    channel_id, packet, ..
                }) = client.service().unwrap()
                {
                    assert_eq!(channel_id, 0);
                    break packet;
                }
                assert!(Instant::now() < deadline, "rumble delivery timed out");
                std::thread::sleep(Duration::from_millis(1));
            };
            let bytes = received.data();
            let mut iv = vec![0; if v2 { 12 } else { 16 }];
            iv[0] = 7;
            if v2 {
                iv[10..].copy_from_slice(b"HC");
            }
            let plain =
                butterpollo_core::crypto::gcm_open(&key, &iv, &bytes[8..24], &bytes[24..]).unwrap();
            // Moonlight's decrypted V2 header, marker, controller ID,
            // then the low- and high-frequency strengths, all LE.
            assert_eq!(
                &plain[..10],
                &[0x0b, 0x01, 10, 0, 0xee, 0xff, 0xc0, 0, 2, 0]
            );
            assert_eq!(&plain[10..], &strengths);
        }
    }
}
#[test]
fn an_override_the_host_cannot_use_is_skipped_and_the_rest_apply() {
    let mut config = Config::default();
    let overrides = serde_json::json!({"frame_limiter_fps_limit": "-1", "fec_percentage": "30"});
    apply_overrides(&mut config, overrides.as_object().unwrap()).unwrap();
    assert_eq!(config.get("fec_percentage", ""), "30");
    assert!(!config.values.contains_key("frame_limiter_fps_limit"));
}
#[test]
fn accepting_input_without_output_cannot_extend_encoder_recovery() {
    let start = Instant::now();
    let mut failing = Some(start);
    for seconds in 0..5 {
        encoder_progress(&mut failing, false, start + Duration::from_secs(seconds)).unwrap();
        assert_eq!(failing, Some(start));
    }
    assert!(encoder_progress(&mut failing, false, start + ENCODER_RECOVERY).is_err());
}
#[test]
fn completed_output_ends_recovery_and_allows_a_later_independent_failure() {
    let start = Instant::now();
    let mut failing = Some(start);
    encoder_progress(&mut failing, true, start + Duration::from_secs(4)).unwrap();
    assert_eq!(failing, None);
    encoder_progress(&mut failing, false, start + ENCODER_RECOVERY).unwrap();
    let later = start + ENCODER_RECOVERY + Duration::from_secs(1);
    failing = Some(later);
    encoder_progress(&mut failing, false, later + Duration::from_secs(1)).unwrap();
    assert!(encoder_progress(&mut failing, false, later + ENCODER_RECOVERY).is_err());
}
#[test]
fn wgc_interval_is_unrestricted_by_default_and_preserves_overrides() {
    let default = Config::default();
    assert!(!capture_config(&default).boolean("wgc_high_rate_capture", true));
    for (setting, expected) in [("true", true), ("false", false)] {
        let config = Config::parse(&format!("wgc_high_rate_capture = {setting}")).unwrap();
        let original = config.values.clone();
        assert_eq!(
            capture_config(&config).boolean("wgc_high_rate_capture", !expected),
            expected
        );
        assert_eq!(config.values, original);
    }
    assert!(!default.values.contains_key("wgc_high_rate_capture"));
}
#[test]
fn shared_capture_separates_explicit_wgc_intervals_without_splitting_ddx() {
    let key = |kind, setting: &str| {
        CaptureKey::new(
            kind,
            "display",
            false,
            &capture_config(&Config::parse(setting).unwrap()),
            "",
        )
    };
    for kind in ["wgc", "auto"] {
        assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=false"));
        assert_ne!(key(kind, ""), key(kind, "wgc_high_rate_capture=true"));
        assert_ne!(key(kind, ""), key(kind, "wgc_drain_to_newest=true"));
        assert_ne!(key(kind, ""), key(kind, "wgc_helper_streaming_scope=true"));
    }
    for kind in ["ddx", "dxgi"] {
        assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=true"));
        assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=false"));
        assert_eq!(key(kind, ""), key(kind, "wgc_drain_to_newest=true"));
        assert_eq!(key(kind, ""), key(kind, "wgc_helper_streaming_scope=true"));
    }
}
#[test]
fn shared_capture_respects_compute_overrides_and_independent_frame_phases() {
    let config = Config::default();
    let key =
        |config: &Config, phase: &str| CaptureKey::new("wgc", "display", false, config, phase);
    let default = key(&config, "first");
    assert_eq!(default, key(&config, "second"));
    let mut helper = config.clone();
    helper
        .values
        .insert("wgc_user_helper".into(), "true".into());
    assert_ne!(default, key(&helper, "first"));
    // The fence ring is the default: only turning it off changes the capture's identity.
    let mut explicit = config.clone();
    explicit
        .values
        .insert("idd_push_fence_ring".into(), "true".into());
    assert_eq!(default, key(&explicit, "first"));
    let mut keyed = config.clone();
    keyed
        .values
        .insert("idd_push_fence_ring".into(), "false".into());
    assert_ne!(default, key(&keyed, "first"));
    let mut changed = config.clone();
    changed
        .values
        .insert("wgc_compute_copy".into(), "false".into());
    assert_ne!(default, key(&changed, "first"));
    changed
        .values
        .insert("wgc_compute_copy".into(), "true".into());
    assert_eq!(default, key(&changed, "first"));
    changed
        .values
        .insert("gpu_compute_conversion".into(), "false".into());
    assert_ne!(default, key(&changed, "first"));
    changed
        .values
        .insert("wgc_slot_aligned_publish".into(), "true".into());
    assert_ne!(key(&changed, "first"), key(&changed, "second"));
}
#[test]
fn dualsense_trigger_effects_use_the_moonlight_layout() {
    let mut data = vec![0u8; 32];
    data[..4].copy_from_slice(&[1, 2, 3, 4]);
    data[7] = 2;
    data[10] = 0x21;
    for (i, byte) in data[11..21].iter_mut().enumerate() {
        *byte = i as u8 + 1;
    }
    data[21] = 0x26;
    for (i, byte) in data[22..32].iter_mut().enumerate() {
        *byte = i as u8 + 11;
    }
    let packets = feedback_packets(1, 5, &data);
    let (_, trigger) = packets.iter().find(|(kind, _)| *kind == 0x5503).unwrap();
    let mut expected = vec![1, 0, 0x0c, 0x21, 0x26];
    expected.extend(1..=20);
    assert_eq!(trigger, &expected);
    assert!(packets.iter().any(|(kind, _)| *kind == RUMBLE_DATA));
}
/// A flood of roaming datagrams ends one receive with none; the ENet datagram
/// behind it arrives on a later call.
#[test]
fn a_roaming_flood_does_not_hold_the_control_receive() {
    let mut socket = ControlSocket::new(Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap()));
    socket.socket.set_nonblocking(true).unwrap();
    socket.roaming.set_enabled(true);
    let host = socket.socket.local_addr().unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut magic = [0u8; butterpollo_core::roaming::DATAGRAM_BYTES];
    magic[..4].copy_from_slice(&[0x0F, 0xFE, 0x52, 0x4D]);
    for _ in 0..SWALLOWED_PER_RECEIVE + 10 {
        client.send_to(&magic, host).unwrap();
    }
    client.send_to(b"enet", host).unwrap();
    let mut buffer = [0; rusty_enet::MTU_MAX];
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut empty = 0;
    let received = loop {
        assert!(Instant::now() < deadline, "the ENet datagram never arrived");
        match rusty_enet::Socket::receive(&mut socket, &mut buffer).unwrap() {
            Some((from, rusty_enet::PacketReceived::Complete(size))) => break (from, size),
            Some(_) => unreachable!(),
            None => empty += 1,
        }
    };
    assert_eq!(received, (client.local_addr().unwrap(), 4));
    assert!(empty >= 1, "the flood ended a receive");
}
#[test]
fn held_control_datagrams_go_out_in_order_once_released() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = receiver.local_addr().unwrap();
    let mut socket = ControlSocket::new(Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap()));
    let mut buffer = [0u8; 64];
    socket.hold();
    // The middle one is too large for UDP: it is lost, the rest go out.
    for datagram in [&b"one"[..], &[0; 70_000], b"two"] {
        assert_eq!(
            rusty_enet::Socket::send(&mut socket, address, datagram).unwrap(),
            datagram.len()
        );
    }
    receiver.set_nonblocking(true).unwrap();
    assert!(receiver.recv_from(&mut buffer).is_err());
    socket.release().unwrap();
    receiver.set_nonblocking(false).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut received = Vec::new();
    for _ in 0..2 {
        let (length, _) = receiver.recv_from(&mut buffer).unwrap();
        received.push(buffer[..length].to_vec());
    }
    assert_eq!(received, [b"one".to_vec(), b"two".to_vec()]);
    // Released: sent at once again.
    rusty_enet::Socket::send(&mut socket, address, b"three").unwrap();
    let (length, _) = receiver.recv_from(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"three");
}
/// Issue #54: Moonlight's ENet, like rusty_enet, drops replies from any
/// address but the one it connected to. To one of its own addresses,
/// Windows replies from that address, so a client on one address of this
/// PC that reached another only connects when the host pins the reply
/// source. Skipped without two such addresses of one family.
#[test]
fn enet_control_answers_from_the_address_the_client_reached() {
    let settings = || HostSettings {
        peer_limit: 1,
        channel_limit: 1,
        ..Default::default()
    };
    let addresses = butterpollo_windows::net::unicast_addresses().unwrap();
    // A client address that reaches another address of this PC.
    let pair = addresses.iter().find_map(|&client_ip| {
        addresses.iter().find_map(|&reached| {
            let host = UdpSocket::bind((reached, 0)).ok()?;
            host.set_read_timeout(Some(Duration::from_millis(300)))
                .ok()?;
            let client = UdpSocket::bind((client_ip, 0)).ok()?;
            (client_ip != reached && client_ip.is_ipv6() == reached.is_ipv6()).then_some(())?;
            client.send_to(b"ping", host.local_addr().ok()?).ok()?;
            host.recv_from(&mut [0u8; 16]).ok()?;
            Some((client_ip, reached))
        })
    });
    let Some((client_ip, reached)) = pair else {
        println!("skipped: no two addresses of one family among {addresses:?}");
        return;
    };
    let wildcard = if reached.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = Arc::new(ReplySocket::bind(wildcard).unwrap());
    let reached = SocketAddr::new(reached, socket.local_addr().unwrap().port());
    let mut host = Host::new(ControlSocket::new(socket.clone()), settings()).unwrap();
    let mut client = Host::new(UdpSocket::bind((client_ip, 0)).unwrap(), settings()).unwrap();
    let client_address = client.socket().local_addr().unwrap();
    client.connect(reached, 1, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !host.peer(PeerID(0)).connected() || !client.peer(PeerID(0)).connected() {
        host.service().unwrap();
        client.service().unwrap();
        assert!(
            Instant::now() < deadline,
            "ENet connect from {client_address} to {reached} timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(socket.source(client_address), Some(reached.ip()));
    // Held datagrams, sent on release, leave from the same address.
    let observer = UdpSocket::bind((client_ip, 0)).unwrap();
    observer
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    observer.send_to(b"hello", reached).unwrap();
    let mut buffer = [0u8; rusty_enet::MTU_MAX];
    let mut control = ControlSocket::new(socket);
    let deadline = Instant::now() + Duration::from_secs(2);
    let peer = loop {
        match rusty_enet::Socket::receive(&mut control, &mut buffer).unwrap() {
            Some((peer, _)) if peer == observer.local_addr().unwrap() => break peer,
            _ => assert!(Instant::now() < deadline, "the datagram did not arrive"),
        }
    };
    control.hold();
    rusty_enet::Socket::send(&mut control, peer, b"held").unwrap();
    control.release().unwrap();
    let (length, from) = observer.recv_from(&mut buffer).unwrap();
    assert_eq!((&buffer[..length], from), (&b"held"[..], reached));
}
#[test]
fn refused_steam_attributes_are_warned_about_once_per_device() {
    let mut packet = [0u8; 84];
    packet[4..8].copy_from_slice(&0x5500_0008u32.to_le_bytes());
    let refused = input::decode(&packet[..80]).unwrap_err();
    let other = input::decode(&[0; 8]).unwrap_err();
    let mut warned = false;
    assert_eq!(first_steam_attributes_refusal(&mut warned, &other), None);
    assert!(!warned);
    assert_eq!(
        first_steam_attributes_refusal(&mut warned, &refused),
        Some(input::SteamAttributesError::Length { len: 80 })
    );
    assert_eq!(first_steam_attributes_refusal(&mut warned, &refused), None);
    let mut other_device = false;
    assert!(first_steam_attributes_refusal(&mut other_device, &refused).is_some());
}
#[test]
fn a_full_input_queue_is_refused_not_grown() {
    let key = |down| input::Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down,
    };
    let mut inputs = vec![];
    for i in 0..INPUT_QUEUE {
        assert!(queue_input(&mut inputs, key(i % 2 == 0)));
    }
    assert!(!queue_input(&mut inputs, key(true)));
    assert_eq!(inputs.len(), INPUT_QUEUE);
    // A move that merges into the last queued one still fits.
    inputs.pop();
    assert!(queue_input(
        &mut inputs,
        input::Input::Relative { x: 1, y: 1 }
    ));
    assert!(queue_input(
        &mut inputs,
        input::Input::Relative { x: 2, y: 3 }
    ));
    assert_eq!(inputs.len(), INPUT_QUEUE);
    assert!(matches!(
        inputs.last(),
        Some(input::Input::Relative { x: 3, y: 4 })
    ));
    assert!(INPUT_OVERFLOW.contains("overflow"));
}
#[test]
fn input_waiting_on_a_refused_controller_acquire_is_not_replayed_later() {
    let arrival = input::Input::Arrival {
        id: 1,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    };
    let mut inputs = vec![
        input::Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        },
        arrival.clone(),
        input::Input::MouseButton {
            button: 1,
            down: true,
        },
        input::Input::Relative { x: 5, y: 5 },
    ];
    drop_uninjectable_input(&mut inputs);
    assert_eq!(inputs, vec![arrival]);
}
#[test]
fn a_refused_controller_acquire_keeps_the_state_the_client_sends_once() {
    let pad = |id, active, buttons| input::Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    let steam = input::Input::SteamAttributes(Default::default());
    let mut inputs = vec![
        pad(0, 0b11, 1),
        input::Input::Haptics(false),
        pad(1, 0b11, 2),
        steam.clone(),
        input::Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        },
        // The client unplugged pad 1: only pad 0's latest state carries that.
        pad(0, 0b01, 0),
    ];
    drop_uninjectable_input(&mut inputs);
    assert_eq!(
        inputs,
        vec![
            input::Input::Haptics(false),
            pad(1, 0b11, 2),
            steam,
            pad(0, 0b01, 0),
        ]
    );
}
#[test]
fn an_arrived_pad_is_asked_for_the_sensors_it_has() {
    assert_eq!(
        motion_requests(3, 0x30),
        [
            (MOTION_EVENT_REQUEST, vec![3, 0, 200, 0, 1]),
            (MOTION_EVENT_REQUEST, vec![3, 0, 200, 0, 2])
        ]
    );
    assert_eq!(
        motion_requests(1, 0x20),
        [(MOTION_EVENT_REQUEST, vec![1, 0, 200, 0, 2])]
    );
    assert!(motion_requests(1, 0x0f).is_empty());
}
