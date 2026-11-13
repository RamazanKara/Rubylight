use super::*;

fn args(list: &[&str]) -> Result<Parsed, String> {
    parse(list.iter().map(OsString::from))
}

fn launch(host: &str, app: Option<&str>, quit_on_exit: bool) -> Parsed {
    Parsed::Launch(Launch {
        host: host.into(),
        app: app.map(Into::into),
        quit_on_exit,
        join: None,
        options: StreamOptions::default(),
    })
}

#[test]
fn no_arguments_open_the_launcher() {
    assert_eq!(args(&[]), Ok(Parsed::Gui));
}

#[test]
fn a_pairing_link_is_an_argument_of_its_own() {
    let link = "rubylight://pair?v=1&host=pc&port=1&otp=4821&pass=secret&fp=00";
    assert_eq!(args(&[link]), Ok(Parsed::Link(link.into())));
    // Whatever follows the scheme is the window's to refuse, never a usage error.
    for odd in ["RUBYLIGHT://pair?v=9", "rubylight:", "rubylight://other"] {
        assert_eq!(args(&[odd]), Ok(Parsed::Link(odd.into())));
    }
    let huge = format!("rubylight://pair?{}", "a".repeat(1024 * 1024));
    assert_eq!(args(&[huge.as_str()]), Ok(Parsed::Link(huge.clone())));
    // Any other positional argument stays the error it was.
    assert_eq!(args(&["ruby"]), Err("unknown argument ruby".into()));
    assert_eq!(args(&["rubylightning"]), Err("unknown argument rubylightning".into()));
}

#[test]
fn a_link_cannot_add_arguments_and_errors_do_not_repeat_it() {
    let link = "rubylight://pair?otp=4821&pass=secret";
    for line in [
        vec![link, "--host", "evil"],
        vec![link, "--check-pyrowave"],
        vec![link, link],
        vec!["--host", "pc", link],
        vec!["--quit-on-exit", link],
    ] {
        let error = args(&line).unwrap_err();
        assert_eq!(error, LINK_NOT_ALONE, "{line:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_link_that_is_not_text_is_refused_without_being_repeated() {
    use std::os::unix::ffi::OsStringExt;
    let parsed = parse([OsString::from_vec(b"rubylight://pair?otp=4821\xff".to_vec())]);
    assert_eq!(parsed, Ok(Parsed::Link(String::new())));
}

#[test]
fn host_app_and_quit_parse_in_any_order_and_both_forms() {
    assert_eq!(
        args(&["--host", "Gaming PC"]),
        Ok(launch("Gaming PC", None, false))
    );
    assert_eq!(
        args(&[
            "--quit-on-exit",
            "--app",
            "Steam Big Picture",
            "--host",
            "pc"
        ]),
        Ok(launch("pc", Some("Steam Big Picture"), true))
    );
    assert_eq!(
        args(&["--host=pc", "--app=Desktop"]),
        Ok(launch("pc", Some("Desktop"), false))
    );
    assert_eq!(args(&["--host", " pc "]), Ok(launch("pc", None, false)));
    assert_eq!(args(&["--help"]), Ok(Parsed::Help));
    assert_eq!(args(&["--version"]), Ok(Parsed::Version));
}

#[test]
fn wrong_command_lines_are_errors() {
    assert!(args(&["--host"]).is_err());
    assert!(args(&["--host", "--app", "x"]).is_err());
    assert!(args(&["--host="]).is_err());
    assert!(args(&["--host", "a", "--host", "b"]).is_err());
    assert!(args(&["--app", "Desktop"]).is_err());
    assert!(args(&["--quit-on-exit"]).is_err());
    assert!(args(&["--host", "pc", "--fullscreen"]).is_err());
    assert!(args(&["--quit-on-exit=yes", "--host", "pc"]).is_err());
    assert!(args(&["pc"]).is_err());
    assert!(args(&["--version=1"]).is_err());
}

fn options_of(list: &[&str]) -> StreamOptions {
    match args(list) {
        Ok(Parsed::Launch(launch)) => launch.options,
        other => panic!("{list:?}: {other:?}"),
    }
}

/// MoonDeck's command line (its `moonlightproxy.py`): options first, then `stream`.
#[test]
fn moondecks_moonlight_qt_command_line_starts_the_stream_with_its_options() {
    let parsed = args(&[
        "--audio-config", "7.1-surround", "--resolution", "2560x1440", "--fps", "120", "--hdr", "--bitrate", "60000",
        "--no-quit-after", "--performance-overlay", "--no-vsync", "--frame-pacing", "--video-codec", "HEVC",
        "stream", "Gaming PC", "Steam Big Picture",
    ]);
    let launch = match parsed {
        Ok(Parsed::Launch(launch)) => launch,
        other => panic!("{other:?}"),
    };
    // moonlight-qt closes once its command-line stream ends.
    assert_eq!((launch.host.as_str(), launch.app.as_deref(), launch.quit_on_exit), ("Gaming PC", Some("Steam Big Picture"), true));
    let wanted = StreamOptions {
        resolution: Some((2560, 1440)),
        fps: Some(120),
        bitrate_kbps: Some(60000),
        codec: Some(CodecPreference::Hevc),
        audio: Some(AudioChannels::Surround71),
        hdr: Some(true),
        vsync: Some(false),
        frame_pacing: Some(true),
        overlay: Some(true),
        quit_after: Some(false),
        ..Default::default()
    };
    assert_eq!(launch.options, wanted);
}

#[test]
fn moonlight_qt_options_follow_its_spelling_and_the_last_one_wins() {
    assert_eq!(options_of(&["stream", "pc", "app", "--hdr", "--no-hdr"]).hdr, Some(false));
    assert_eq!(options_of(&["--no-hdr", "--hdr", "--host", "pc"]).hdr, Some(true));
    assert_eq!(options_of(&["--1080", "stream", "pc", "app"]).resolution, Some((1920, 1080)));
    assert_eq!(options_of(&["--resolution=1280X800", "--4K", "stream", "pc", "app"]).resolution, Some((3840, 2160)));
    assert_eq!(options_of(&["--video-codec", "h.264", "stream", "pc", "app"]).codec, Some(CodecPreference::H264));
    assert_eq!(options_of(&["--video-codec=AV1", "stream", "pc", "app"]).codec, Some(CodecPreference::Av1));
    assert_eq!(options_of(&["--video-codec", "auto", "stream", "pc", "app"]).codec, Some(CodecPreference::Auto));
    assert_eq!(options_of(&["--video-decoder", "hardware", "stream", "pc", "app"]).decoder, Some(DecoderPreference::Auto));
    assert_eq!(options_of(&["--display-mode", "borderless", "stream", "pc", "app"]).window_mode, Some(WindowMode::Borderless));
    assert_eq!(options_of(&["--capture-system-keys", "fullscreen", "stream", "pc", "app"]).system_keys, Some((true, true)));
    // The action is found anywhere among the words, in any case, as moonlight-qt finds it.
    assert_eq!(args(&["STREAM", "pc", "Desktop"]), Ok(Parsed::Launch(launch_of("pc", "Desktop"))));
    // moonlight-qt's options Nightjar has no setting for are taken, so its command lines start.
    for unused in ["--packet-size=1392", "--keep-awake", "--no-mouse-buttons-swap", "--touchscreen-trackpad", "--mute-on-focus-loss", "--reverse-scroll-direction"] {
        assert_eq!(options_of(&[unused, "stream", "pc", "app"]), StreamOptions::default(), "{unused}");
    }
}

#[test]
fn moonlight_qts_list_names_a_host_and_takes_only_verbose() {
    // MoonDeck's call (its moonlightproxy.py `get_apps`).
    assert_eq!(args(&["list", "Gaming PC"]), Ok(Parsed::List(List { host: "Gaming PC".into(), verbose: false })));
    assert_eq!(args(&["--verbose", "LIST", "pc"]), Ok(Parsed::List(List { host: "pc".into(), verbose: true })));
    assert_eq!(args(&["list", "pc", "extra"]), Ok(Parsed::List(List { host: "pc".into(), verbose: false })));
    for wrong in [
        &["list"][..],
        &["list", " "],
        &["list", "pc", "--csv"],
        &["list", "pc", "--hdr"],
        &["--host", "pc", "list", "pc"],
        &["list", "pc", "--quit-on-exit"],
        &["--verbose", "stream", "pc", "app"],
        &["--verbose", "--host", "pc"],
        &["--verbose=1", "list", "pc"],
    ] {
        assert!(args(wrong).is_err(), "{wrong:?}");
    }
    assert!(!shows_usage_box(false, &["list", "pc", "--bogus"]));
}

#[test]
fn list_prints_one_app_name_a_line_without_host_actions() {
    let terminate = App { control: AppControl::Terminate, ..app(3, "E16C", "Terminate") };
    let apps = vec![app(1, "", "Desktop"), app(2, "5E1F-77", "Steam Big Picture"), terminate];
    assert_eq!(app_names(&apps), "Desktop\nSteam Big Picture\n");
    assert_eq!(app_names(&[]), "");
}

#[test]
fn list_errors_carry_moonlight_qts_exit_codes() {
    use nightjar_core::http::Error;
    let refused = Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
    assert_eq!(list_error(&refused, "pc"), ("Failed to connect to pc".to_owned(), LIST_NO_PC));
    assert_eq!(list_error(&Error::NotPaired, "pc").1, LIST_NO_PC);
    assert!(list_error(&Error::NotPaired, "pc").0.starts_with("Computer pc has not been paired."));
    assert_eq!(list_error(&Error::CertificateMismatch, "pc").1, LIST_FAILED);
}

fn launch_of(host: &str, app: &str) -> Launch {
    Launch { host: host.into(), app: Some(app.into()), quit_on_exit: true, join: None, options: StreamOptions::default() }
}

#[test]
fn wrong_moonlight_qt_command_lines_are_errors() {
    for wrong in [
        &["stream"][..],
        &["stream", "pc"],
        &["stream", "pc", "app", "more"],
        &["stream", " ", "app"],
        &["--host", "pc", "stream", "pc", "app"],
        &["--fps", "sixty", "stream", "pc", "app"],
        &["--fps", "0", "stream", "pc", "app"],
        &["--bitrate", "stream", "pc", "app"],
        &["--resolution", "1080p", "stream", "pc", "app"],
        &["--resolution", "0x1080", "stream", "pc", "app"],
        &["--video-codec", "VP9", "stream", "pc", "app"],
        &["--audio-config", "quad", "stream", "pc", "app"],
        &["--hdr=yes", "stream", "pc", "app"],
        &["--no-fps", "stream", "pc", "app"],
        &["--fps", "60"],
        &["--hdr"],
    ] {
        assert!(args(wrong).is_err(), "{wrong:?}");
    }
    // A link still stands alone after moonlight-qt's words or options.
    let link = "rubylight://pair?otp=4821&pass=secret";
    assert_eq!(args(&["stream", "pc", link]), Err(LINK_NOT_ALONE.into()));
    assert_eq!(args(&["--hdr", link]), Err(LINK_NOT_ALONE.into()));
}

#[test]
fn moonlight_qt_options_change_one_streams_settings_as_moonlight_qt_does() {
    let saved = Settings { width: 1920, height: 1080, fps: 60, bitrate_kbps: 40000, frame_pacing: FramePacing::Latency, ..Default::default() };
    let mut settings = saved.clone();
    StreamOptions::default().apply(&mut settings);
    assert_eq!(settings, saved);
    // A resolution or rate without a bitrate gets the bitrate for them.
    let mut settings = saved.clone();
    StreamOptions { fps: Some(120), ..Default::default() }.apply(&mut settings);
    assert_eq!((settings.fps, settings.bitrate_kbps), (120, 0));
    let mut settings = saved.clone();
    let options = StreamOptions {
        resolution: Some((2560, 1600)),
        bitrate_kbps: Some(80000),
        hdr: Some(true),
        vsync: Some(false),
        frame_pacing: Some(true),
        overlay: Some(true),
        yuv444: Some(true),
        absolute_mouse: Some(true),
        system_keys: Some((true, false)),
        ..Default::default()
    };
    options.apply(&mut settings);
    assert_eq!((settings.width, settings.height, settings.bitrate_kbps), (2560, 1600, 80000));
    assert!(settings.hdr && !settings.vsync && settings.overlay);
    assert_eq!(settings.frame_pacing, FramePacing::Smoothness);
    assert!(settings.experimental_packed_444 && settings.prefer_yuv444);
    assert_eq!(settings.mouse_mode, MouseMode::Absolute);
    assert!(settings.capture_system_keys && !settings.system_keys_fullscreen_only);
    // A pacing already chosen that waits for the refresh stays; off is the lowest latency.
    let mut settings = Settings { frame_pacing: FramePacing::CapFps, ..saved.clone() };
    StreamOptions { frame_pacing: Some(true), ..Default::default() }.apply(&mut settings);
    assert_eq!(settings.frame_pacing, FramePacing::CapFps);
    StreamOptions { frame_pacing: Some(false), ..Default::default() }.apply(&mut settings);
    assert_eq!(settings.frame_pacing, FramePacing::Latency);
}

fn entry<'a>(
    id: u64,
    unique_id: &'a str,
    name: &'a str,
    alias: Option<&'a str>,
    paired: bool,
) -> HostEntry<'a> {
    HostEntry {
        id,
        unique_id,
        name,
        alias,
        paired,
    }
}

#[test]
fn hosts_match_by_uuid_then_name_or_alias() {
    let hosts = || {
        vec![
            entry(1, "AAAA-1111", "DESKTOP-1", Some("Living room"), true),
            entry(2, "BBBB-2222", "aaaa-1111", None, true),
            entry(3, "", "Office", None, true),
        ]
    };
    assert_eq!(
        find_host(hosts(), "aaaa-1111"),
        Ok(1),
        "a UUID wins over a name"
    );
    assert_eq!(find_host(hosts(), "desktop-1"), Ok(1));
    assert_eq!(find_host(hosts(), "LIVING ROOM"), Ok(1));
    assert_eq!(find_host(hosts(), "office"), Ok(3));
    assert!(
        find_host(hosts(), "").is_err(),
        "an empty UUID matches nothing"
    );
    assert!(find_host(hosts(), "Garage").unwrap_err().contains("Garage"));
}

#[test]
fn ambiguous_and_unpaired_hosts_are_errors() {
    let twins = vec![
        entry(1, "a", "PC", None, true),
        entry(2, "b", "pc", None, true),
    ];
    assert!(find_host(twins, "pc").unwrap_err().contains("UUID"));
    let unpaired = vec![entry(1, "a", "PC", None, false)];
    assert!(find_host(unpaired, "PC")
        .unwrap_err()
        .contains("not paired"));
}

fn app(id: i32, uuid: &str, name: &str) -> App {
    App {
        name: name.into(),
        id,
        hdr_supported: false,
        uuid: uuid.into(),
        host_index: None,
        art_version: String::new(),
        control: AppControl::None,
        monitor_resume: false,
    }
}

fn found(apps: &[App], wanted: &str) -> Result<i32, String> {
    find_app(apps, wanted).map(|a| a.id)
}

#[test]
fn apps_match_by_uuid_then_name() {
    let apps = vec![
        app(1, "", "Desktop"),
        app(2, "5E1F-77", "Steam Big Picture"),
        app(3, "x", "Steam Big Picture"),
    ];
    assert_eq!(found(&apps, "desktop"), Ok(1));
    assert_eq!(found(&apps, "5e1f-77"), Ok(2));
    assert!(found(&apps, "steam big picture")
        .unwrap_err()
        .contains("UUID"));
    assert!(found(&apps, "Cyberpunk").unwrap_err().contains("Cyberpunk"));
}

#[test]
fn host_actions_are_no_apps_to_stream() {
    let terminate = App {
        control: AppControl::Terminate,
        ..app(9, "E16C", "Terminate")
    };
    let apps = vec![app(1, "", "Desktop"), terminate];
    assert!(found(&apps, "terminate")
        .unwrap_err()
        .contains("not an app to stream"));
    assert!(found(&apps, "e16c").is_err());
}

fn cli(app: Option<&str>, quit_on_exit: bool) -> CliLaunch {
    CliLaunch::resolved(
        Launch {
            host: "pc".into(),
            app: app.map(Into::into),
            quit_on_exit,
            join: None,
            options: StreamOptions::default(),
        },
        7,
    )
}

fn seen(answered: bool, apps: AppsSeen) -> Seen {
    Seen {
        identity: true,
        host: Some(HostSeen {
            answered,
            paired: true,
            offline: None,
            running: None,
            confirms_replacement: false,
            apps,
        }),
        message: None,
        busy: false,
        confirming: false,
    }
}

/// The PC as `seen` with the apps loaded and `running` running.
fn running(apps: Vec<App>, running: Option<(i32, Option<&str>)>, confirms: bool) -> Seen {
    let mut seen = seen(true, AppsSeen::Loaded(apps));
    let host = seen.host.as_mut().unwrap();
    host.running = running.map(|(id, uuid)| (id, uuid.map(Into::into)));
    host.confirms_replacement = confirms;
    seen
}

#[test]
fn an_app_is_loaded_and_launched_once() {
    let mut c = cli(Some("desktop"), false);
    assert_eq!(
        c.next(&Seen {
            identity: false,
            ..seen(false, AppsSeen::NotLoaded)
        }),
        Step::Wait
    );
    assert_eq!(c.next(&seen(false, AppsSeen::NotLoaded)), Step::Wait);
    assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
    assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
    assert_eq!(c.next(&seen(true, AppsSeen::Loading)), Step::Wait);
    let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam"), app(5, "", "Desktop")]);
    assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 5, false));
    let busy = |s: Seen| Seen { busy: true, ..s };
    assert_eq!(c.next(&busy(seen(true, apps()))), Step::Wait);
}

/// The second screen of a first that joined another device's game joins it by its id: the PC
/// reports no running game to this device, and the name may name no listed app.
#[test]
fn a_join_resumes_the_game_by_its_id() {
    let mut c = cli(Some("Running game"), true);
    c.launch.join = Some(9);
    let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam"), app(5, "", "Desktop")]);
    assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 9, true));
}

#[test]
fn a_list_that_drops_back_to_unloaded_is_asked_for_again() {
    let mut c = cli(Some("desktop"), false);
    assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
    assert_eq!(c.next(&seen(true, AppsSeen::Loading)), Step::Wait);
    assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
    assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
}

#[test]
fn the_running_app_is_resumed_and_another_one_is_an_error() {
    let apps = || vec![app(1, "", "Steam"), app(5, "D-5", "Desktop")];
    let mut c = cli(Some("desktop"), false);
    assert_eq!(
        c.next(&running(apps(), Some((5, None)), false)),
        Step::Launch(7, 5, true)
    );
    // A UUID decides when both sides have one.
    let mut c = cli(Some("desktop"), false);
    assert_eq!(
        c.next(&running(apps(), Some((1, Some("D-5"))), false)),
        Step::Launch(7, 5, true)
    );
    let mut c = cli(Some("desktop"), false);
    assert_eq!(
        c.next(&running(apps(), Some((1, None)), false)),
        Step::Fail("the PC is running Steam; quit it first or start that app instead".into())
    );
    // A PC that asks its user before replacing the running app gets a plain launch.
    let mut c = cli(Some("desktop"), false);
    assert_eq!(
        c.next(&running(apps(), Some((1, None)), true)),
        Step::Launch(7, 5, false)
    );
    // Resume and the other special tiles are never resumed themselves.
    let resume_tile = App {
        control: AppControl::Resume,
        ..app(8, "", "Resume")
    };
    let mut c = cli(Some("resume"), false);
    assert_eq!(
        c.next(&running(vec![resume_tile], Some((1, None)), false)),
        Step::Launch(7, 8, false)
    );
}

#[test]
fn a_launch_the_launcher_drops_ends_the_start() {
    let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam")]);
    let launch =
        |c: &mut CliLaunch| assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 1, false));
    // Dropped without a message (the app vanished, no identity): an error.
    let mut c = cli(Some("Steam"), false);
    launch(&mut c);
    assert_eq!(
        c.next(&seen(true, apps())),
        Step::Fail("the stream did not start".into())
    );
    // Cancelled by the user: closed without an error.
    let mut c = cli(Some("Steam"), false);
    launch(&mut c);
    c.cancel();
    assert_eq!(c.next(&seen(true, apps())), Step::Close);
    // Still connecting after the backstop.
    let mut c = cli(Some("Steam"), false);
    launch(&mut c);
    c.started = Instant::now() - LAUNCH_LIMIT - Duration::from_secs(1);
    let busy = Seen {
        busy: true,
        ..seen(true, apps())
    };
    assert_eq!(
        c.next(&busy),
        Step::Fail("the stream did not start in time".into())
    );
}

#[test]
fn the_start_waits_for_the_replace_prompt_and_its_cancel_closes() {
    let apps = || vec![app(1, "", "Steam"), app(5, "", "Desktop")];
    let refused = || running(apps(), Some((1, None)), true);
    let confirming = || Seen { confirming: true, ..refused() };
    for confirm in [true, false] {
        let mut c = cli(Some("desktop"), false);
        assert_eq!(c.next(&refused()), Step::Launch(7, 5, false));
        // The PC refused the launch and the launcher asks, however long the user takes.
        c.started = Instant::now() - LAUNCH_LIMIT - Duration::from_secs(1);
        assert_eq!(c.next(&confirming()), Step::Wait);
        if confirm {
            // The launch with the answer gets the full limit.
            assert_eq!(c.next(&Seen { busy: true, ..refused() }), Step::Wait);
            c.streaming_began();
            assert_eq!(c.next(&refused()), Step::Wait);
        } else {
            // Cancelling the prompt cancels the start, without an error.
            c.cancel();
            assert_eq!(c.next(&refused()), Step::Close);
            assert!(c.failed.is_none());
        }
    }
}

#[test]
fn a_message_or_an_unreachable_pc_fails_the_start_at_once() {
    let mut c = cli(Some("Steam"), false);
    let message = Seen {
        message: Some("Nightjar's identity: could not be read".into()),
        ..seen(false, AppsSeen::NotLoaded)
    };
    assert_eq!(
        c.next(&message),
        Step::Fail("Nightjar's identity: could not be read".into())
    );
    let mut c = cli(Some("Steam"), false);
    let mut offline = seen(false, AppsSeen::NotLoaded);
    offline.host.as_mut().unwrap().offline = Some("connection refused".into());
    assert_eq!(
        c.next(&offline),
        Step::Fail("the PC pc cannot be reached: connection refused".into())
    );
}

#[test]
fn only_a_start_without_a_frontend_shows_an_error_box() {
    assert!(shows_error_box(None));
    let launch = Launch {
        host: "pc".into(),
        app: None,
        quit_on_exit: true,
        join: None,
        options: StreamOptions::default(),
    };
    assert!(!shows_error_box(Some(&launch)));
}

#[test]
fn the_usage_is_boxed_only_without_a_console_and_never_for_a_frontend_start() {
    // A console prints it; a box on top would only be in the way.
    assert!(!shows_usage_box(true, &["--help"]));
    assert!(!shows_usage_box(true, &["--bogus"]));
    // Without one the box is the only place it shows.
    assert!(shows_usage_box(false, &["--help"]));
    assert!(shows_usage_box(false, &["--app", "Desktop"]));
    // A frontend's wrong command line ends at once, in stderr.
    assert!(!shows_usage_box(false, &["--host", "pc", "--bogus"]));
    assert!(!shows_usage_box(false, &["--bogus", "--host=pc"]));
    assert!(shows_usage_box(false, &["--hostname"]));
    assert!(!shows_usage_box(false, &["--bogus", "stream", "pc", "app"]));
}

#[test]
fn without_an_app_the_app_list_opens_once() {
    let mut c = cli(None, false);
    assert_eq!(c.next(&seen(false, AppsSeen::NotLoaded)), Step::OpenApps(7));
    assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
}

#[test]
fn a_missing_app_a_failed_list_a_silent_pc_or_a_failed_start_are_errors() {
    let mut c = cli(Some("Cyberpunk"), false);
    assert!(matches!(
        c.next(&seen(true, AppsSeen::Loaded(vec![app(1, "", "Steam")]))),
        Step::Fail(_)
    ));
    let mut c = cli(Some("Steam"), false);
    assert!(matches!(
        c.next(&seen(true, AppsSeen::Failed("timeout".into()))),
        Step::Fail(_)
    ));
    let mut c = cli(Some("Steam"), false);
    assert!(matches!(
        c.next(&Seen {
            host: None,
            ..seen(true, AppsSeen::NotLoaded)
        }),
        Step::Fail(_)
    ));
    let mut c = cli(Some("Steam"), false);
    c.started = Instant::now() - START_LIMIT - Duration::from_secs(1);
    assert!(matches!(
        c.next(&seen(false, AppsSeen::NotLoaded)),
        Step::Fail(_)
    ));
    let mut c = cli(Some("Steam"), false);
    assert_eq!(
        c.next(&seen(true, AppsSeen::Loaded(vec![app(1, "", "Steam")]))),
        Step::Launch(7, 1, false)
    );
    let failed = Seen {
        message: Some("The stream could not start: no video".into()),
        ..seen(true, AppsSeen::NotLoaded)
    };
    assert_eq!(
        c.next(&failed),
        Step::Fail("The stream could not start: no video".into())
    );
}

#[test]
fn quit_on_exit_closes_after_the_stream_and_keeps_its_error() {
    let mut c = cli(Some("Steam"), true);
    assert!(!c.stream_ended(None), "no stream ran yet");
    c.streaming_began();
    assert!(c.stream_ended(None));
    assert_eq!(c.failed, None);
    let mut c = cli(Some("Steam"), true);
    c.streaming_began();
    assert!(c.stream_ended(Some("The stream ended: host error".into())));
    assert_eq!(c.failed.as_deref(), Some("The stream ended: host error"));
    let mut c = cli(Some("Steam"), false);
    c.streaming_began();
    assert!(
        !c.stream_ended(None),
        "without --quit-on-exit the launcher stays"
    );
}
