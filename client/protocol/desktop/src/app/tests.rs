//! Tests for Windows swapchain colorspace selection, stream sleep pacing, and adaptive controller polling.

use super::*;

#[test]
fn windows_only_uses_scrgb_swapchain_for_hdr_10bit_sdr_and_ai_on_hdr_display() {
    let default = config::Settings::default();
    // PyroWave, 4:4:4, packed 4:4:4 and shader upscaling in 8-bit SDR stay on 8-bit BGRA.
    let sdr_shaders = config::Settings {
        codec: config::CodecPreference::PyroWave,
        prefer_yuv444: true,
        experimental_packed_444: true,
        ..default.clone()
    };
    let ai = config::Settings { upscaling: config::Upscaling::Ai, ..default.clone() };
    for display_hdr in [false, true] {
        assert!(!windows_needs_linear(&default, display_hdr), "default SDR stream uses 8-bit BGRA swapchain");
        for upscaling in [config::Upscaling::Bilinear, config::Upscaling::Fsr1, config::Upscaling::Sgsr1] {
            assert!(!windows_needs_linear(&config::Settings { upscaling, ..sdr_shaders.clone() }, display_hdr));
        }
        assert!(windows_needs_linear(&config::Settings { hdr: true, ..default.clone() }, display_hdr));
        assert!(windows_needs_linear(&config::Settings { sdr_10bit: true, ..default.clone() }, display_hdr));
    }
    // RTX Video HDR needs the linear swapchain, only where the display shows HDR.
    assert!(windows_needs_linear(&ai, true));
    assert!(!windows_needs_linear(&ai, false));
}

#[test]
fn stream_sleep_avoids_busy_spin_except_for_beam_race() {
    // Normal sub-1.5 ms pacing tail uses kernel timer SDL_DelayNS, never SDL_DelayPrecise.
    assert_eq!(stream_sleep(Duration::from_micros(800), false, None), StreamSleep::DelayNs(800_000));
    assert_eq!(stream_sleep(Duration::from_nanos(200), false, None), StreamSleep::DelayNs(1_000), "positive sub-us clamped to 1 us");
    // Beam Race keeps microsecond spin precision.
    assert_eq!(stream_sleep(Duration::from_micros(800), true, None), StreamSleep::DelayPreciseNs(800_000));
    // Longer waits use SDL_WaitEventTimeout, waking 1 ms early for frames or at exact pad cadence.
    assert_eq!(stream_sleep(Duration::from_millis(8), false, None), StreamSleep::EventTimeoutMs(7));
    assert_eq!(stream_sleep(Duration::from_millis(8), false, Some(Duration::from_millis(2))), StreamSleep::EventTimeoutMs(2));
    assert_eq!(stream_sleep(Duration::from_millis(8), false, Some(Duration::from_millis(16))), StreamSleep::EventTimeoutMs(7));
}

#[test]
fn pad_poll_relaxes_idle_controllers_and_keeps_active_gameplay_fast() {
    let start = Instant::now();
    let mut poll = PadPoll::default();
    // First frame polls immediately so already-plugged pads open right away.
    assert_eq!(poll.next_in(start, false, false, false, true), Duration::ZERO);
    poll.polled(start);
    // No pads open -> 2 s hotplug interval.
    assert_eq!(poll.next_in(start, false, false, false, true), PadPoll::ENUM_INTERVAL);
    // Launcher with a controller open -> 16 ms visible, 250 ms hidden (not SDL's 1 ms).
    assert_eq!(poll.next_in(start, true, false, false, true), PadPoll::LAUNCHER_INTERVAL);
    assert_eq!(poll.next_in(start, true, false, false, false), PadPoll::LAUNCHER_HIDDEN_INTERVAL);
    // Stream with controllers disabled -> 2 s hotplug interval.
    assert_eq!(poll.next_in(start, true, true, false, true), PadPoll::ENUM_INTERVAL);
    // Stream with idle controller -> at most 4 ms.
    assert_eq!(PadPoll::STREAM_ACTIVE_INTERVAL, Duration::from_millis(1), "1000 Hz in use");
    const { assert!(PadPoll::STREAM_IDLE_INTERVAL.as_millis() <= 4) };
    assert_eq!(poll.next_in(start, true, true, true, true), PadPoll::STREAM_IDLE_INTERVAL);
    // Controller input switches immediately to 1 ms active cadence for 1 s.
    poll.note_activity(start);
    assert_eq!(poll.next_in(start, true, true, true, true), PadPoll::STREAM_ACTIVE_INTERVAL);
    // DirectMotion filter activity also refreshes active cadence once per new timestamp.
    let later = start + Duration::from_secs(2);
    assert_eq!(poll.interval(later, true, true, true, true), PadPoll::STREAM_IDLE_INTERVAL);
    poll.sync_direct(42_000, later);
    assert_eq!(poll.interval(later, true, true, true, true), PadPoll::STREAM_ACTIVE_INTERVAL);
}

#[test]
fn ceil_ms_and_stream_sleep_never_truncate_sub_ms_pad_wait_to_zero() {
    assert_eq!(ceil_ms(Duration::ZERO), 0);
    assert_eq!(ceil_ms(Duration::from_nanos(1)), 1);
    assert_eq!(ceil_ms(Duration::from_micros(300)), 1);
    assert_eq!(ceil_ms(Duration::from_millis(1)), 1);
    assert_eq!(ceil_ms(Duration::from_micros(1_500)), 2);
    assert_eq!(ceil_ms(Duration::from_millis(16)), 16);
    assert_eq!(
        launcher_wait_slice_ms(Duration::from_millis(250), Duration::from_micros(200)),
        1,
        "positive sub-ms pad_wait rounds up to 1 ms instead of spinning at 0 ms"
    );
    assert_eq!(
        launcher_wait_slice_ms(Duration::from_millis(5), Duration::from_millis(16)),
        5
    );
    assert_eq!(
        stream_sleep(Duration::from_millis(8), false, Some(Duration::from_micros(200))),
        StreamSleep::EventTimeoutMs(1)
    );
    assert_eq!(
        stream_sleep(Duration::from_millis(8), false, Some(Duration::from_micros(1_500))),
        StreamSleep::EventTimeoutMs(2)
    );
}

#[test]
fn gamepad_activity_ignores_stick_jitter_and_passive_sensors() {
    assert!(!stream::direct_motion::is_gyro_peak_active(None), "accel ignored");
    assert!(!stream::direct_motion::is_gyro_peak_active(Some(0.012)), "resting gyro ignored");
    assert!(stream::direct_motion::is_gyro_peak_active(Some(0.035)), "2 deg/s aiming counts");
    let tilted = Some(2.0_f32.to_radians() / 3.0_f32.sqrt());
    assert!(stream::direct_motion::is_gyro_peak_active(tilted), "2 deg/s over three axes");
    assert!(!stream::direct_motion::is_gyro_peak_active(Some(f32::NAN)));
    assert!(stream::direct_motion::is_gyro_peak_active(Some(0.5)), "active gyro wakes");

    // SAFETY: zero-initialized SDL_Event is valid plain data.
    let make_event = |kind: SDL_EventType| -> SDL_Event {
        let mut ev: SDL_Event = unsafe { std::mem::zeroed() };
        ev.r#type = kind.0;
        ev
    };
    let mut activity = stream::direct_motion::PadActivity::default();
    assert!(activity.is_active(&make_event(SDL_EVENT_GAMEPAD_BUTTON_DOWN)));
    assert!(activity.is_active(&make_event(SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION)));
    assert!(!activity.is_active(&make_event(SDL_EVENT_GAMEPAD_ADDED)), "hotplug alone stays at idle cadence");
    let mut threshold_axis = make_event(SDL_EVENT_GAMEPAD_AXIS_MOTION);
    threshold_axis.gaxis.which = SDL_JoystickID(3);
    threshold_axis.gaxis.axis = 1;
    threshold_axis.gaxis.value = 1023;
    assert!(!activity.is_active(&threshold_axis));
    threshold_axis.gaxis.value = 1024;
    assert!(activity.is_active(&threshold_axis));
    for idle_kind in [
        SDL_EVENT_GAMEPAD_SENSOR_UPDATE,
        SDL_EVENT_SENSOR_UPDATE,
        SDL_EVENT_GAMEPAD_UPDATE_COMPLETE,
        SDL_EVENT_GAMEPAD_REMAPPED,
        SDL_EVENT_GAMEPAD_STEAM_HANDLE_UPDATED,
        SDL_EVENT_JOYSTICK_AXIS_MOTION,
        SDL_EVENT_JOYSTICK_BALL_MOTION,
        SDL_EVENT_JOYSTICK_HAT_MOTION,
        SDL_EVENT_JOYSTICK_BUTTON_DOWN,
        SDL_EVENT_JOYSTICK_BUTTON_UP,
        SDL_EVENT_JOYSTICK_ADDED,
        SDL_EVENT_JOYSTICK_REMOVED,
        SDL_EVENT_JOYSTICK_BATTERY_UPDATED,
        SDL_EVENT_JOYSTICK_UPDATE_COMPLETE,
    ] {
        let ev = make_event(idle_kind);
        assert!(!launcher_event_wakes_frame(&ev, false, [false; 4]), "idle launcher ignores {}", idle_kind.0);
        assert!(launcher_event_wakes_frame(&ev, true, [false; 4]), "controller test wakes on {}", idle_kind.0);
    }
    for wake_kind in [
        SDL_EVENT_GAMEPAD_ADDED,
        SDL_EVENT_GAMEPAD_REMOVED,
        SDL_EVENT_GAMEPAD_BUTTON_DOWN,
        SDL_EVENT_GAMEPAD_BUTTON_UP,
        SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN,
    ] {
        assert!(launcher_event_wakes_frame(&make_event(wake_kind), false, [false; 4]), "launcher wakes on {}", wake_kind.0);
    }

    let mut jitter = make_event(SDL_EVENT_GAMEPAD_AXIS_MOTION);
    jitter.gaxis.axis = SDL_GAMEPAD_AXIS_LEFTX.0 as u8;
    jitter.gaxis.value = 300;
    assert!(!launcher_event_wakes_frame(&jitter, false, [false; 4]));
    jitter.gaxis.value = 16000;
    assert!(!launcher_event_wakes_frame(&jitter, false, [false; 4]));
    assert!(launcher_event_wakes_frame(&jitter, false, [true, false, false, false]));
    jitter.gaxis.value = 16001;
    assert!(launcher_event_wakes_frame(&jitter, false, [false; 4]));
    jitter.gaxis.axis = SDL_GAMEPAD_AXIS_RIGHTX.0 as u8;
    assert!(!launcher_event_wakes_frame(&jitter, false, [false; 4]), "right stick does not navigate launcher");

    // Stream wait ignores joystick update-complete, passive sensor noise, and resting stick drift.
    assert!(!stream_event_wakes_wait(SDL_EVENT_JOYSTICK_UPDATE_COMPLETE, false, true));
    assert!(!stream_event_wakes_wait(SDL_EVENT_GAMEPAD_UPDATE_COMPLETE, false, true));
    assert!(!stream_event_wakes_wait(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, false, true));
    assert!(!stream_event_wakes_wait(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, true, false));
    assert!(stream_event_wakes_wait(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, true, true));
    assert!(!stream_event_wakes_wait(SDL_EVENT_GAMEPAD_AXIS_MOTION, false, true));
    assert!(stream_event_wakes_wait(SDL_EVENT_GAMEPAD_AXIS_MOTION, true, true));
    assert!(stream_event_wakes_wait(SDL_EVENT_MOUSE_MOTION, false, false));

    // PadActivity uses per-axis delta so a resting tilted stick with 1-LSB ADC jitter goes idle.
    let mut activity = stream::direct_motion::PadActivity::default();
    let mut axis = make_event(SDL_EVENT_GAMEPAD_AXIS_MOTION);
    axis.gaxis.which = SDL_JoystickID(7);
    axis.gaxis.axis = 0;
    axis.gaxis.value = 2500;
    assert!(activity.is_active(&axis), "initial deflection past threshold counts");
    axis.gaxis.value = 2505;
    assert!(!activity.is_active(&axis), "resting stick ADC jitter does not pin 1000 Hz polling");
}

#[test]
fn effective_launcher_window_mode_honors_windowed_launcher() {
    assert_eq!(effective_launcher_window_mode(WindowMode::Borderless, false), WindowMode::Borderless);
    assert_eq!(effective_launcher_window_mode(WindowMode::Fullscreen, false), WindowMode::Fullscreen);
    assert_eq!(effective_launcher_window_mode(WindowMode::Borderless, true), WindowMode::Windowed);
    assert_eq!(effective_launcher_window_mode(WindowMode::Fullscreen, true), WindowMode::Windowed);
    assert_eq!(effective_launcher_window_mode(WindowMode::Windowed, true), WindowMode::Windowed);
}

#[test]
fn v4l2_incomplete_base_probe_is_one_shot_not_sticky() {
    use nightjar_core::sdp::{VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_H265};
    let wanted = VIDEO_FORMAT_H265;
    let key = (Hardware::V4l2m2m, wanted, false);
    let failed = stream::color_probe::Base {
        decodable: 0,
        reason: "HEVC: V4L2 M2M: the decoder made no picture of the fixture".into(),
        backends: Vec::new(),
    };
    assert!(!base_probe_sticky(key, &failed));
    let mut cache = Some(CachedBaseProbe { key, result: failed.clone(), sticky: false });
    let mut retried = None;
    // Peeking survives intermediate early returns (e.g. ProbeWork::Packed) on the same launch.
    assert_eq!(peek_base_probe(&cache, key), Some(failed.clone()));
    assert_eq!(peek_base_probe(&cache, key), Some(failed.clone()), "peek preserves non-sticky probe until launch commits");
    consume_non_sticky_base_probe(&mut cache, &mut retried, key);
    assert_eq!(peek_base_probe(&cache, key), None, "next launch re-probes V4L2 after commit");
    assert_eq!(retried, Some(key), "second launch remembers it already retried once");
    assert!(base_probe_sticky(key, &failed) || retried == Some(key), "a repeated V4L2 failure stays cached");

    let permanent_v4l2 = stream::color_probe::Base {
        decodable: 0,
        reason: "HEVC: V4L2 M2M: no decoder".into(),
        backends: Vec::new(),
    };
    assert!(base_probe_sticky(key, &permanent_v4l2), "permanent V4L2 failure does not re-probe every launch");

    let auto_with_other_fail = stream::color_probe::Base {
        decodable: VIDEO_FORMAT_H265,
        reason: "AV1: VA-API: no decoder".into(),
        backends: vec![(VIDEO_FORMAT_H265, Hardware::V4l2m2m)],
    };
    assert!(
        base_probe_sticky((Hardware::Auto(std::ptr::null_mut(), None), wanted | VIDEO_FORMAT_AV1_MAIN8, false), &auto_with_other_fail),
        "V4L2 decoding another codec does not make an unrelated backend failure non-sticky"
    );

    let passed = stream::color_probe::Base {
        decodable: wanted,
        reason: String::new(),
        backends: vec![(wanted, Hardware::V4l2m2m)],
    };
    assert!(base_probe_sticky(key, &passed));
    let mut sticky_cache = Some(CachedBaseProbe { key, result: passed.clone(), sticky: true });
    let mut sticky_retried = None;
    assert_eq!(peek_base_probe(&sticky_cache, key), Some(passed.clone()));
    consume_non_sticky_base_probe(&mut sticky_cache, &mut sticky_retried, key);
    assert_eq!(peek_base_probe(&sticky_cache, key), Some(passed), "full pass stays cached");
    assert_eq!(sticky_retried, None);

    let non_v4l2_key = (Hardware::Vaapi, VIDEO_FORMAT_AV1_MAIN8, false);
    let non_v4l2_fail = stream::color_probe::Base {
        decodable: 0,
        reason: "AV1: VA-API: no decoder".into(),
        backends: Vec::new(),
    };
    assert!(base_probe_sticky(non_v4l2_key, &non_v4l2_fail));
}

#[test]
fn pyrowave_probe_error_logs_once_per_distinct_reason() {
    let mut last = None;
    assert!(startup::should_log_pyrowave_probe_error(&mut last, false, "no Vulkan queue"));
    assert!(!startup::should_log_pyrowave_probe_error(&mut last, false, "no Vulkan queue"));
    assert!(startup::should_log_pyrowave_probe_error(&mut last, true, "no Vulkan queue"));
    assert!(startup::should_log_pyrowave_probe_error(&mut last, true, "shader compile failed"));
}

#[test]
fn amd_decode_note_formats_enabled_and_disabled_for_logfile() {
    let enabled = startup::amd_decode_note(None, Some("mem"), None, Some("dcc"));
    assert!(enabled.contains("AMD_DEBUG=") && enabled.contains("RADV_PERFTEST="));
    assert!(enabled.contains("lowlatencydec"));
    let disabled = startup::amd_decode_note(Some("0"), Some("mem"), None, Some("dcc"));
    assert!(disabled.contains("NIGHTJAR_AMD_LOW_LATENCY_DECODE=0"));
}

#[test]
fn reconnect_skips_launcher_window_restore_and_quit_on_exit_waits_for_pending_end() {
    assert!(!session::restores_launcher_window_mode_on_exit(true));
    assert!(session::restores_launcher_window_mode_on_exit(false));

    assert!(!launch::cli_stream_settled(false, false, false, true), "pending_end keeps --quit-on-exit waiting for host reason");
    assert!(launch::cli_stream_settled(false, false, false, false));
}

#[test]
fn stale_quit_app_target_is_detected_by_recheck_running() {
    use nightjar_policy::console::{recheck_running, Running};
    assert_eq!(recheck_running(Some(10), Some(10)), Running::Same);
    assert_eq!(recheck_running(Some(10), Some(20)), Running::Changed);
    assert_eq!(recheck_running(Some(10), None), Running::Gone);
}

#[test]
fn pending_bitrate_slider_flushes_immediately_when_taken_on_menu_close() {
    use nightjar_desktop_ui::stream_overlay_widgets::{bitrate_slider, take_pending_bitrate_slider};
    let ctx = egui::Context::default();
    let mut value = 20_000;
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let mut out = ctx.run_ui(raw, |ui| {
        let mut s = value;
        ui.horizontal(|ui| {
            // Focus slider or simulate scrubbing by storing pending state; stepping slider directly:
            let _ = bitrate_slider(ui, &mut s, true);
        });
        value = s;
    });
    out.textures_delta.clear();
    // Simulate active scrub committed only after menu closes (before 450 ms).
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::unique("bitrate-slider-scrub"),
            nightjar_desktop_ui::stream_overlay_widgets::Scrub { changed_at: Some(Instant::now()) },
        );
    });
    assert!(take_pending_bitrate_slider(&ctx));
    assert!(!take_pending_bitrate_slider(&ctx), "taking settles it once");
}

#[test]
fn unreadable_list_names_removed_pc_or_unready_key() {
    use crate::hosts::Hosts;
    use std::sync::Arc;
    let hosts = Hosts::without_discovery(vec![nightjar_desktop_config::config::HostRecord::default()], Arc::new(|| {}));
    let id = hosts.list[0].id;
    assert_eq!(
        launch::unreadable_list(&hosts, id),
        crate::strings::tr("This device’s pairing key is not ready yet. Try again in a moment.")
    );
    assert_eq!(
        launch::unreadable_list(&hosts, id + 99),
        crate::strings::tr("This PC is no longer in your list.")
    );
}

#[test]
fn away_wake_outcome_awake_waits_for_online_reach_without_disarming_open_play() {
    use crate::hosts::{Apps, Host, Hosts, Reach};
    use crate::open_play::{AppsSeen, HostSeen, Run, Seen, SessionSeen, Step, Target};
    use nightjar_desktop_config::config::{HostRecord, OpenGame};

    let record = HostRecord {
        unique_id: "uid-awake".into(),
        name: "Living Room".into(),
        address: "198.51.100.10".into(),
        mac: Some("aa:bb:cc:dd:ee:ff".into()),
        ..HostRecord::default()
    };
    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    let id = hosts.list[0].id;
    hosts.list[0].reach = Reach::Offline;

    let mut run_state = Run::new(Target {
        host: id,
        pc: "Living Room".into(),
        game: OpenGame {
            host: "uid-awake".into(),
            app_id: 7,
            install_id: String::new(),
            title: "Hades".into(),
        },
    });
    let t0 = Instant::now();
    let seen_of = |reach: Reach, waking: bool, countdown: bool, has_dialog: bool| Seen {
        pin_screen: false,
        no_dialog: !has_dialog && !countdown,
        countdown,
        message: has_dialog,
        identity: true,
        host: Some(HostSeen {
            reach,
            can_wake: true,
            paired: true,
            apps: AppsSeen::NotLoaded,
            session: SessionSeen::Free,
            reads: None,
        }),
        waking,
        network: true,
        busy: false,
        streaming: false,
        left_by_player: false,
    };

    assert_eq!(run_state.next(&seen_of(Reach::Offline, false, false, false), t0), Step::Count(3));
    let t1 = t0 + Duration::from_secs(4);
    assert_eq!(run_state.next(&seen_of(Reach::Offline, false, true, false), t1), Step::Begin);
    assert_eq!(run_state.next(&seen_of(Reach::Offline, false, true, false), t1), Step::Wake);

    let mut waking = wol::Wakes::default();
    assert!(waking.start(id, t1));
    let run_num = waking.current().unwrap().run;
    let mut away = launch::AwayWakeUi::default();
    away.begin("Contacting your wake relay…".into());
    let mut launcher = ui::Launcher::new(
        Default::default(),
        Default::default(),
        Default::default(),
        nightjar_desktop_ui::stream_types::Display {
            width: 1920,
            height: 1080,
            refresh_hz: 60.0,
            refresh_reported: true,
        },
    );

    // Relay reports Outcome::Awake while poll_one hasn't landed Reach::Online yet.
    let mut ctx = launch::WakeContext {
        hosts: &mut hosts,
        waking: &mut waking,
        away: &mut away,
        launcher: &mut launcher,
    };
    launch::on_wake_relay_done_at(
        &mut ctx,
        id,
        run_num,
        "Living Room",
        hosts::wake_relay::Outcome::Awake,
        &["Relay NUC".into()],
        t1,
    );
    assert!(waking.host().is_some(), "waking must stay active until host poll confirms Online");
    assert_eq!(
        run_state.next(&seen_of(Reach::Offline, waking.host().is_some(), false, false), t1),
        Step::Wait,
        "open_play must wait rather than disarming on Outcome::Awake before Reach::Online"
    );

    // Once poll_one sets Reach::Online, wake_step clears waking and OpenPlay advances cleanly.
    hosts.list[0].reach = Reach::Online;
    hosts.list[0].apps = Apps::NotLoaded;
    let mut ctx = launch::WakeContext {
        hosts: &mut hosts,
        waking: &mut waking,
        away: &mut away,
        launcher: &mut launcher,
    };
    launch::wake_step_at(&mut ctx, t1 + Duration::from_millis(250));
    assert!(waking.host().is_none());
    assert!(!away.active);
    let next_step = run_state.next(&seen_of(Reach::Online, false, false, false), t1 + Duration::from_millis(250));
    assert!(!matches!(next_step, Step::Over { .. }), "must advance rather than disarm: {next_step:?}");

    // If poll stays offline past WAKE_RELAY_POLL_TIMEOUT, wake_step surfaces Dialog::WakeFailed.
    let mut waking2 = wol::Wakes::default();
    assert!(waking2.start(id, t1));
    let run2 = waking2.current().unwrap().run;
    hosts.list[0].reach = Reach::Offline;
    let mut away2 = launch::AwayWakeUi::default();
    let mut ctx2 = launch::WakeContext {
        hosts: &mut hosts,
        waking: &mut waking2,
        away: &mut away2,
        launcher: &mut launcher,
    };
    launch::on_wake_relay_done_at(
        &mut ctx2,
        id,
        run2,
        "Living Room",
        hosts::wake_relay::Outcome::Awake,
        &["Relay NUC".into()],
        t1,
    );
    launch::wake_step_at(&mut ctx2, t1 + launch::WAKE_RELAY_POLL_TIMEOUT);
    assert!(waking2.host().is_none());
    assert!(matches!(launcher.dialog, ui::Dialog::WakeFailed { host, .. } if host == id));
    let _ = Host::ui_view;
}
