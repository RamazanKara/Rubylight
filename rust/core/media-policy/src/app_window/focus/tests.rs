//! Tests of following the focus: fake foreground windows, the delay, the
//! launch-focus case, a window on another display and the window closing.
use super::super::{
    Background, Candidate, Closed, Decision, Frame, MINIMIZED, REPLACING, Settings, Target,
    settings, wants_window,
};
use super::*;
use butterpollo_config::config::Config;

const GAME: u64 = 11;
const LAUNCHER: u64 = 10;
/// Another program's window: a store client, a browser, the task switcher.
const OTHER: u64 = 50;

fn window(id: u64, pid: u32, size: (i32, i32)) -> Candidate {
    Candidate {
        id,
        pid,
        exe: String::new(),
        title: format!("window {id}"),
        width: size.0,
        height: size.1,
        visible: true,
        minimized: false,
        owned: false,
        tool: false,
    }
}
fn focused() -> Settings {
    Settings {
        exe: None,
        title: None,
        closed: Closed::Display,
        frame: Frame::Full,
        background: Background::Display,
    }
}
fn tree() -> BTreeSet<u32> {
    [1, 2].into_iter().collect()
}
/// The game (pid 2), the launcher the app started (pid 1) and another
/// program's window (pid 99).
fn desktop() -> Vec<Candidate> {
    vec![
        window(LAUNCHER, 1, (800, 600)),
        window(GAME, 2, (1920, 1080)),
        window(OTHER, 99, (2560, 1440)),
    ]
}
fn ms(start: Instant, at: u64) -> Instant {
    start + Duration::from_millis(at)
}
fn game() -> Target {
    Target::Window { id: GAME, pid: 2 }
}
fn display() -> Target {
    Target::Display
}

/// Looks as the worker does, every 250 ms from `from` to `to` (inclusive),
/// with the same windows and foreground window, returning the last decision.
fn looks(
    tracker: &mut Tracker,
    windows: &[Candidate],
    foreground: Option<u64>,
    start: Instant,
    (from, to): (u64, u64),
) -> Decision {
    let mut at = from;
    loop {
        let look = tracker.look(windows, &tree(), &focused(), foreground, ms(start, at));
        if at >= to {
            return look.decision;
        }
        at = (at + 250).min(to);
    }
}

#[test]
fn window_focus_asks_for_the_window_and_the_display_in_the_background() {
    let config = |text: &str| settings(&Config::parse(text).unwrap());
    for value in ["window_focus", "Window_Focus", " focus "] {
        assert!(wants_window(value), "{value}");
        let read = config(&format!("capture_window = {value}\n")).unwrap();
        assert_eq!(read.background, Background::Display, "{value}");
    }
    for value in ["window", "app", "true"] {
        let read = config(&format!("capture_window = {value}\n")).unwrap();
        assert_eq!(read.background, Background::Window, "{value}");
    }
    // A typo never hides the display.
    assert_eq!(config("capture_window = window_focuss\n"), None);
    assert_eq!(config("capture_window = display\n"), None);
}

#[test]
fn only_the_apps_own_shown_window_counts_as_in_front() {
    let mut windows = desktop();
    let settings = focused();
    assert_eq!(
        in_front(&windows, &tree(), &settings, Some(GAME)),
        Some(GAME)
    );
    assert_eq!(
        in_front(&windows, &tree(), &settings, Some(LAUNCHER)),
        Some(LAUNCHER)
    );
    // Another program, the desktop (no window), or a window the scan missed.
    assert_eq!(in_front(&windows, &tree(), &settings, Some(OTHER)), None);
    assert_eq!(in_front(&windows, &tree(), &settings, None), None);
    assert_eq!(in_front(&windows, &tree(), &settings, Some(77)), None);
    // The game's own dialog is not the game's window: the display shows it.
    windows.push(Candidate {
        owned: true,
        ..window(12, 2, (400, 200))
    });
    assert_eq!(in_front(&windows, &tree(), &settings, Some(12)), None);
    // A minimised window can be active, but shows nothing.
    windows[1].visible = false;
    windows[1].minimized = true;
    assert_eq!(in_front(&windows, &tree(), &settings, Some(GAME)), None);
}

#[test]
fn alt_tab_away_shows_the_display_after_the_delay_and_back_shows_the_window_at_once() {
    let start = Instant::now();
    let windows = desktop();
    let mut tracker = Tracker::new(&focused());
    assert!(tracker.follows_focus());
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (0, 500)).target,
        game()
    );
    // Alt+Tab to another program: the window stays for the delay.
    let away = looks(&mut tracker, &windows, Some(OTHER), start, (750, 1500));
    assert_eq!(away.target, game(), "not yet a second behind");
    let look = tracker.look(&windows, &tree(), &focused(), Some(OTHER), ms(start, 1750));
    assert_eq!(look.decision.target, display());
    assert!(look.behind);
    assert_eq!(look.decision.warning, None, "what this value asks for");
    assert_eq!(look.decision.end, None);
    // The desktop itself in front (no window of anyone's) is the same.
    assert_eq!(
        looks(&mut tracker, &windows, None, start, (2000, 4000)).target,
        display()
    );
    // Back to the game: at once.
    let back = tracker.look(&windows, &tree(), &focused(), Some(GAME), ms(start, 4250));
    assert_eq!(back.decision.target, game());
    assert!(!back.behind);
}

#[test]
fn focus_flapping_shorter_than_the_delay_never_switches() {
    let start = Instant::now();
    let windows = desktop();
    let mut tracker = Tracker::new(&focused());
    looks(&mut tracker, &windows, Some(GAME), start, (0, 250));
    // Another program in front for 750 ms, then the game, again and again.
    let mut at = 500;
    for _ in 0..8 {
        let away = looks(&mut tracker, &windows, Some(OTHER), start, (at, at + 750));
        assert_eq!(away.target, game(), "at {at} ms");
        let back = looks(
            &mut tracker,
            &windows,
            Some(GAME),
            start,
            (at + 1000, at + 1000),
        );
        assert_eq!(back.target, game());
        at += 1250;
    }
}

#[test]
fn a_launcher_stealing_the_focus_for_a_moment_at_launch_never_switches() {
    let start = Instant::now();
    let windows = desktop();
    let mut tracker = Tracker::new(&focused());
    looks(&mut tracker, &windows, Some(GAME), start, (0, 500));
    // The store client takes the focus for 200 ms: one look sees it.
    let stolen = tracker.look(&windows, &tree(), &focused(), Some(OTHER), ms(start, 750));
    assert_eq!(stolen.decision.target, game());
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (1000, 3000)).target,
        game()
    );
    // The app's own launcher, for a moment: the stream stays on the game.
    let launcher = tracker.look(
        &windows,
        &tree(),
        &focused(),
        Some(LAUNCHER),
        ms(start, 3250),
    );
    assert_eq!(launcher.decision.target, game());
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (3500, 4000)).target,
        game()
    );
}

#[test]
fn a_game_that_opens_behind_its_store_client_is_streamed_until_launch_focus_brings_it_forward() {
    let start = Instant::now();
    let windows = desktop();
    let mut tracker = Tracker::new(&focused());
    // Never in front yet (#131): streamed as `window` streams it, for as
    // long as the store client stays in front.
    let behind = looks(&mut tracker, &windows, Some(OTHER), start, (0, 5000));
    assert_eq!(behind.target, game());
    // Launch focus brings it forward; from then on it follows the focus.
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (5250, 5500)).target,
        game()
    );
    assert_eq!(
        looks(&mut tracker, &windows, Some(OTHER), start, (5750, 7000)).target,
        display()
    );
}

#[test]
fn the_apps_other_window_takes_the_stream_once_it_stays_in_front() {
    let start = Instant::now();
    let mut windows = desktop();
    // The launcher shows first, then hands over to the game, which stays.
    let mut tracker = Tracker::new(&focused());
    let launcher = Target::Window {
        id: LAUNCHER,
        pid: 1,
    };
    let only_launcher = [windows[0].clone()];
    assert_eq!(
        looks(
            &mut tracker,
            &only_launcher,
            Some(LAUNCHER),
            start,
            (0, 500)
        )
        .target,
        launcher
    );
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (750, 1500)).target,
        launcher,
        "a second in front first"
    );
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (1750, 1750)).target,
        game()
    );
    // Back on the launcher (the user clicks it): it takes the stream again,
    // after the delay, and the game is not "in the background".
    assert_eq!(
        looks(&mut tracker, &windows, Some(LAUNCHER), start, (2000, 3000)).target,
        launcher
    );
    // The launcher closes: the game, in front again, at once.
    windows.remove(0);
    let back = tracker.look(&windows, &tree(), &focused(), Some(GAME), ms(start, 3250));
    assert_eq!(back.decision.target, game());
}

#[test]
fn a_minimised_window_shows_the_display_without_a_warning_and_comes_back_on_restore() {
    let start = Instant::now();
    let mut windows = desktop();
    let mut tracker = Tracker::new(&focused());
    looks(&mut tracker, &windows, Some(GAME), start, (0, 250));
    windows[1].visible = false;
    windows[1].minimized = true;
    let minimised = tracker.look(&windows, &tree(), &focused(), Some(OTHER), ms(start, 500));
    assert_eq!(minimised.decision.target, display());
    assert_eq!(minimised.decision.warning, None);
    windows[1].visible = true;
    windows[1].minimized = false;
    let restored = tracker.look(&windows, &tree(), &focused(), Some(GAME), ms(start, 3000));
    assert_eq!(restored.decision.target, game());
    // `window` keeps its warning: nothing changes for it.
    let mut window_only = Tracker::new(&Settings {
        background: Background::Window,
        ..focused()
    });
    windows[1].visible = false;
    windows[1].minimized = true;
    // The game alone: a fresh look prefers a shown window, the launcher.
    let only_game = [windows[1].clone()];
    let look = window_only.look(&only_game, &tree(), &focused(), None, start);
    assert_eq!(look.decision.warning, Some(MINIMIZED));
}

#[test]
fn a_window_that_closes_waits_for_its_replacement_as_window_does() {
    let start = Instant::now();
    let mut windows = desktop();
    let mut tracker = Tracker::new(&focused());
    looks(&mut tracker, &windows, Some(GAME), start, (0, 250));
    // The game's window closes; the launcher is gone too.
    windows.retain(|w| w.id == OTHER);
    let closed = tracker.look(&windows, &tree(), &focused(), Some(OTHER), ms(start, 500));
    assert_eq!(closed.decision.target, display());
    assert_eq!(closed.decision.warning, Some(REPLACING));
    assert!(!closed.behind);
    // Its replacement, opened in front, is streamed at once.
    windows.push(window(13, 2, (1920, 1080)));
    let replaced = tracker.look(&windows, &tree(), &focused(), Some(13), ms(start, 1000));
    assert_eq!(replaced.decision.target, Target::Window { id: 13, pid: 2 });
}

#[test]
fn a_window_on_another_display_follows_the_focus_and_its_background_is_the_streams_display() {
    // Which display a window is on is the capture target's business
    // (`app_window_target` names it); the decision is the same wherever it
    // is, and the background is always the stream's own display, whose mode,
    // colour space and input mapping the stream was set up for.
    let start = Instant::now();
    let windows = vec![window(GAME, 2, (3840, 2160)), window(OTHER, 99, (800, 600))];
    let mut tracker = Tracker::new(&focused());
    assert_eq!(
        looks(&mut tracker, &windows, Some(GAME), start, (0, 250)).target,
        game()
    );
    assert_eq!(
        looks(&mut tracker, &windows, Some(OTHER), start, (500, 1750)).target,
        Target::Display
    );
}

#[test]
fn window_never_reads_the_focus_and_keeps_the_window_in_the_background() {
    let start = Instant::now();
    let windows = desktop();
    let settings = Settings {
        background: Background::Window,
        ..focused()
    };
    let mut tracker = Tracker::new(&settings);
    assert!(!tracker.follows_focus());
    for at in (0..5000).step_by(250) {
        let look = tracker.look(&windows, &tree(), &settings, Some(OTHER), ms(start, at));
        assert_eq!(look.decision.target, game());
        assert!(!look.behind);
    }
    // The streamed window stays while it qualifies, wherever the focus is.
    let look = tracker.look(
        &windows,
        &tree(),
        &settings,
        Some(LAUNCHER),
        ms(start, 6000),
    );
    assert_eq!(look.decision.target, game());
}
