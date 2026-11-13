use super::*;
use crate::display_policy::{letterbox, picture, stream_to_picture};

fn window(id: u64, pid: u32, exe: &str, title: &str, size: (i32, i32)) -> Candidate {
    Candidate {
        id,
        pid,
        exe: exe.into(),
        title: title.into(),
        width: size.0,
        height: size.1,
        visible: true,
        minimized: false,
        owned: false,
        tool: false,
    }
}
fn any() -> Settings {
    Settings {
        exe: None,
        title: None,
        closed: Closed::Display,
    }
}
fn tree(pids: &[u32]) -> BTreeSet<u32> {
    pids.iter().copied().collect()
}

#[test]
fn the_window_setting_is_off_unless_asked_for_and_reads_its_options() {
    assert_eq!(settings(&Config::default()), None);
    let config = |text: &str| Config::parse(text).unwrap();
    assert_eq!(settings(&config("capture_window = display\n")), None);
    // A typo never hides the display.
    assert_eq!(settings(&config("capture_window = windwo\n")), None);
    assert_eq!(settings(&config("capture_window = window\n")), Some(any()));
    assert_eq!(settings(&config("capture_window = true\n")), Some(any()));
    assert_eq!(
        settings(&config(
            "capture_window = Window\ncapture_window_exe = \"Game.exe\"\ncapture_window_title =  Main  \ncapture_window_closed = end\n"
        )),
        Some(Settings {
            exe: Some("Game.exe".into()),
            title: Some("Main".into()),
            closed: Closed::End,
        })
    );
    assert_eq!(
        settings(&config(
            "capture_window = window\ncapture_window_closed = later\n"
        ))
        .unwrap()
        .closed,
        Closed::Display
    );
}

#[test]
fn programs_match_by_file_name_in_any_case_with_or_without_folder_and_extension() {
    assert!(exe_matches("game.exe", "Game.EXE"));
    assert!(exe_matches("game.exe", "game"));
    assert!(exe_matches(r"C:\Games\Game.exe", "game.exe"));
    assert!(exe_matches("game.exe", r"D:\x\GAME.exe"));
    assert!(!exe_matches("game.exe", "gam"));
    assert!(!exe_matches("game-launcher.exe", "game.exe"));
    assert!(!exe_matches("game.exe", ""));
    assert!(title_matches("Elden Ring™ — Main", "ring™"));
    assert!(!title_matches("Launcher", "Ring"));
}

#[test]
fn the_largest_shown_window_of_the_app_tree_is_picked() {
    let windows = [
        window(10, 1, "launcher.exe", "Launcher", (400, 300)),
        window(11, 2, "game.exe", "Game", (1920, 1080)),
        window(12, 99, "other.exe", "Browser", (2560, 1440)),
        // Dialogs, overlays and tiny helper windows are never the app's.
        Candidate {
            owned: true,
            ..window(13, 2, "game.exe", "Settings", (3000, 3000))
        },
        Candidate {
            tool: true,
            ..window(14, 2, "game.exe", "Overlay", (3000, 3000))
        },
        window(15, 2, "game.exe", "", (32, 32)),
    ];
    assert_eq!(pick(&windows, &tree(&[1, 2]), &any(), None).unwrap().id, 11);
    assert!(pick(&windows, &tree(&[7]), &any(), None).is_none());
    // Equal sizes: the oldest handle, so the choice is stable.
    let twins = [
        window(21, 1, "a.exe", "", (800, 600)),
        window(20, 1, "a.exe", "", (800, 600)),
    ];
    assert_eq!(pick(&twins, &tree(&[1]), &any(), None).unwrap().id, 20);
}

#[test]
fn the_streamed_window_stays_while_it_qualifies() {
    let windows = [
        window(10, 1, "game.exe", "Game", (1280, 720)),
        window(11, 1, "game.exe", "Map", (1920, 1080)),
    ];
    let settings = any();
    assert_eq!(
        pick(&windows, &tree(&[1]), &settings, Some(10)).unwrap().id,
        10
    );
    // Gone, or no longer the app's: the best other one.
    assert_eq!(
        pick(&windows, &tree(&[1]), &settings, Some(9)).unwrap().id,
        11
    );
    let dialog = [Candidate {
        owned: true,
        ..windows[0].clone()
    }];
    assert!(pick(&dialog, &tree(&[1]), &settings, Some(10)).is_none());
}

#[test]
fn a_shown_window_wins_over_a_larger_minimised_one_which_still_counts() {
    let minimised = Candidate {
        visible: false,
        minimized: true,
        // Windows parks a minimised window's rectangle off screen and tiny.
        width: 160,
        height: 28,
        ..window(10, 1, "game.exe", "Game", (0, 0))
    };
    let shown = window(11, 1, "game.exe", "Tool", (640, 480));
    let both = [minimised.clone(), shown];
    assert_eq!(pick(&both, &tree(&[1]), &any(), None).unwrap().id, 11);
    // The streamed window minimised stays the app's window.
    assert_eq!(pick(&both, &tree(&[1]), &any(), Some(10)).unwrap().id, 10);
    let alone = [minimised];
    assert_eq!(pick(&alone, &tree(&[1]), &any(), None).unwrap().id, 10);
    // Hidden (neither shown nor minimised) is nothing to stream.
    let hidden = [Candidate {
        visible: false,
        ..window(12, 1, "game.exe", "Game", (800, 600))
    }];
    assert!(pick(&hidden, &tree(&[1]), &any(), None).is_none());
}

#[test]
fn a_named_program_or_title_narrows_the_choice() {
    let windows = [
        window(10, 1, "launcher.exe", "Game Launcher", (1600, 900)),
        window(11, 2, "game.exe", "Game", (1280, 720)),
        window(12, 2, "game.exe", "Crash reporter", (1300, 800)),
    ];
    let program = Settings {
        exe: Some("GAME".into()),
        ..any()
    };
    assert_eq!(
        pick(&windows, &tree(&[1, 2]), &program, None).unwrap().id,
        12
    );
    let titled = Settings {
        exe: Some("game.exe".into()),
        title: Some("game".into()),
        ..any()
    };
    assert_eq!(
        pick(&windows, &tree(&[1, 2]), &titled, None).unwrap().id,
        11
    );
    let title_only = Settings {
        title: Some("launcher".into()),
        ..any()
    };
    assert_eq!(
        pick(&windows, &tree(&[1, 2]), &title_only, None)
            .unwrap()
            .id,
        10
    );
    // A title alone never reaches outside the app's processes.
    assert!(pick(&windows, &tree(&[7]), &title_only, None).is_none());
}

#[test]
fn a_named_program_is_found_outside_the_tree_when_a_launcher_handed_it_over() {
    // The launcher started the game through a store client: not in its tree.
    let windows = [window(11, 50, "game.exe", "Game", (1920, 1080))];
    let program = Settings {
        exe: Some("game.exe".into()),
        ..any()
    };
    assert_eq!(pick(&windows, &tree(&[1]), &program, None).unwrap().id, 11);
    // A copy in the tree wins over a larger one the user started at the PC.
    let both = [
        window(11, 50, "game.exe", "Game", (2560, 1440)),
        window(12, 2, "game.exe", "Game", (1280, 720)),
    ];
    assert_eq!(pick(&both, &tree(&[1, 2]), &program, None).unwrap().id, 12);
}

fn step(follow: &mut Follow, found: Option<&Candidate>, closed: Closed, at: Instant) -> Decision {
    follow.step(found, closed, at)
}

#[test]
fn the_stream_waits_on_the_display_until_the_window_appears() {
    let start = Instant::now();
    let mut follow = Follow::default();
    let waiting = step(&mut follow, None, Closed::End, start);
    assert_eq!(waiting.target, Target::Display);
    assert_eq!(waiting.warning, Some(WAITING));
    // Never ended while nothing was seen, however long the game takes.
    let late = step(
        &mut follow,
        None,
        Closed::End,
        start + Duration::from_secs(600),
    );
    assert_eq!(late.end, None);
    let game = window(11, 2, "game.exe", "Game", (1280, 720));
    let shown = step(
        &mut follow,
        Some(&game),
        Closed::End,
        start + Duration::from_secs(601),
    );
    assert_eq!(shown.target, Target::Window { id: 11, pid: 2 });
    assert_eq!(shown.warning, None);
    assert_eq!(follow.current(), Some(11));
}

#[test]
fn a_minimised_window_shows_the_display_and_comes_back_when_restored() {
    let start = Instant::now();
    let mut follow = Follow::default();
    let game = window(11, 2, "game.exe", "Game", (1280, 720));
    step(&mut follow, Some(&game), Closed::End, start);
    let minimised = Candidate {
        visible: false,
        minimized: true,
        ..game.clone()
    };
    // However long it stays minimised, the stream goes on.
    for seconds in [1, 60, 600] {
        let decision = step(
            &mut follow,
            Some(&minimised),
            Closed::End,
            start + Duration::from_secs(seconds),
        );
        assert_eq!(decision.target, Target::Display);
        assert_eq!(decision.warning, Some(MINIMIZED));
        assert_eq!(decision.end, None);
    }
    let restored = step(
        &mut follow,
        Some(&game),
        Closed::End,
        start + Duration::from_secs(601),
    );
    assert_eq!(restored.target, Target::Window { id: 11, pid: 2 });
    assert_eq!(restored.warning, None);
}

#[test]
fn a_closed_window_waits_for_a_replacement_then_shows_the_display_or_ends() {
    let start = Instant::now();
    let splash = window(10, 2, "game.exe", "Splash", (800, 450));
    let game = window(11, 2, "game.exe", "Game", (1920, 1080));
    for closed in [Closed::Display, Closed::End] {
        let mut follow = Follow::default();
        step(&mut follow, Some(&splash), closed, start);
        // The splash screen gives way to the game within the wait.
        let gap = step(&mut follow, None, closed, start + Duration::from_secs(1));
        assert_eq!(
            (gap.target.clone(), gap.warning, gap.end),
            (Target::Display, Some(REPLACING), None)
        );
        assert_eq!(follow.current(), None);
        let replaced = step(
            &mut follow,
            Some(&game),
            closed,
            start + Duration::from_secs(2),
        );
        assert_eq!(replaced.target, Target::Window { id: 11, pid: 2 });
        // The game closes for good.
        let closing = start + Duration::from_secs(10);
        assert_eq!(
            step(&mut follow, None, closed, closing).warning,
            Some(REPLACING)
        );
        let just_before = closing + REPLACEMENT_WAIT - Duration::from_millis(1);
        assert_eq!(step(&mut follow, None, closed, just_before).end, None);
        let after = step(&mut follow, None, closed, closing + REPLACEMENT_WAIT);
        match closed {
            Closed::Display => {
                assert_eq!(after.target, Target::Display);
                assert_eq!(after.warning, Some(CLOSED_DISPLAY));
                assert_eq!(after.end, None);
            }
            Closed::End => assert_eq!(after.end, Some(CLOSED_END)),
        }
        // A window that comes back later is streamed again.
        let back = step(
            &mut follow,
            Some(&game),
            closed,
            closing + Duration::from_secs(60),
        );
        assert_eq!(back.target, Target::Window { id: 11, pid: 2 });
        assert_eq!(back.end, None);
    }
}

/// The window's picture in the stream and the input mapped onto it: both use
/// the display policy's letterbox, so a click lands where the device shows it.
#[test]
fn a_window_of_another_shape_is_letterboxed_and_input_lands_on_the_same_window_point() {
    // A 4:3 window on a 16:9 stream: bars left and right.
    let (window, stream) = ((1024, 768), (1920, 1080));
    let placed = picture(window, stream).unwrap();
    assert_eq!(placed, (240, 0, 1440, 1080));
    // The picture's centre is the window's centre; its edges the window's edges.
    assert_eq!(stream_to_picture((0.5, 0.5), stream, placed), (0.5, 0.5));
    assert_eq!(
        stream_to_picture((240. / 1920., 0.), stream, placed),
        (0., 0.)
    );
    assert_eq!(
        stream_to_picture((1680. / 1920., 1.), stream, placed),
        (1., 1.)
    );
    // A point on a bar moves to the window's edge.
    assert_eq!(stream_to_picture((0.01, 0.5), stream, placed), (0., 0.5));
    // A portrait window on a landscape stream: wide bars left and right.
    let portrait = picture((720, 1280), (1920, 1080)).unwrap();
    assert_eq!(portrait, (656, 0, 608, 1080));
    // A tall phone stream of a wide window: bars above and below.
    let tall = picture((1920, 1080), (1080, 2400)).unwrap();
    assert_eq!(tall.0, 0);
    assert_eq!(tall.2, 1080);
    assert!(tall.1 > 0 && tall.3 < 2400);
    // A window of the stream's shape fills it, at any size.
    assert_eq!(picture((1280, 720), (1920, 1080)), None);
    assert_eq!(letterbox((3840, 2160), (1920, 1080)), (0, 0, 1920, 1080));
}

#[test]
fn odd_and_tiny_window_sizes_stay_within_the_stream_with_even_offsets() {
    for window in [
        (1, 1),
        (3, 1001),
        (1001, 3),
        (1366, 767),
        (65, 64),
        (7680, 64),
    ] {
        for stream in [(1280, 720), (1920, 1080), (2400, 1080), (1080, 2400)] {
            let (x, y, w, h) = letterbox(window, stream);
            assert!(
                x % 2 == 0 && y % 2 == 0 && w % 2 == 0 && h % 2 == 0,
                "{window:?} on {stream:?}"
            );
            assert!(
                x + w <= stream.0 && y + h <= stream.1,
                "{window:?} on {stream:?}"
            );
            assert!(w >= 2 && h >= 2);
        }
    }
}

#[test]
fn apps_and_devices_can_set_every_window_setting() {
    for key in KEYS {
        assert!(crate::config::override_allowed(key), "{key}");
    }
}
