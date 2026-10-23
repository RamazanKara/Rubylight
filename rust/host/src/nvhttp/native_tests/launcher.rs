//! A launcher showing a prompt instead of the game (plan item B5): the windows are read while the
//! game has not shown up, and the owner's session says what the launcher wants.
use super::*;
use butterpollo_core::{
    launcher_prompt::Window,
    native_api::session::{LauncherKind, State as SessionState},
};

fn window(process: &str, title: &str, foreground: bool) -> Window {
    Window {
        process: process.into(),
        title: title.into(),
        foreground,
    }
}
fn updating() -> Vec<Window> {
    vec![window("steamwebhelper.exe", "Updating Dota 2", false)]
}

impl Setup {
    /// The first app starts a program, as a launched game does.
    fn launching_app(&self) {
        self.host().apps.write().unwrap()[0].cmd = "game.exe".into();
    }
    fn look(&self, windows: Vec<Window>) {
        crate::launcher_watch::look(self.host(), || windows);
    }
}

#[tokio::test]
async fn a_launcher_update_shows_in_the_owners_session_and_goes_with_the_game() {
    let s = Setup::new();
    s.launching_app();
    let _stream = s.streaming();
    assert!(s.get("/session").await.session().launcher.is_none());

    // One look can be a window that flashes by.
    s.look(updating());
    let first = s.get("/session").await.session();
    assert!(first.launcher.is_none());

    s.look(updating());
    let shown = s.get("/session").await.session();
    let notice = shown.launcher.expect("the update is shown");
    assert_eq!(notice.launcher, "steam");
    assert_eq!(notice.kind, LauncherKind::Update);
    assert_eq!(notice.window_title, "Updating Dota 2");
    assert!(notice.since_unix_ms > 0);
    // The stream goes on; it is a note, not a state.
    assert_eq!(shown.state, SessionState::Streaming);
    assert!(shown.revision > first.revision);

    // The game shows up: the launcher is not read again and the notice is gone.
    s.host()
        .current_app
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .mark_game_seen();
    crate::launcher_watch::look(s.host(), || {
        panic!("the windows are not read once the game is up")
    });
    let after = s.get("/session").await.session();
    assert!(after.launcher.is_none());
    assert!(after.revision > shown.revision);
}

#[tokio::test]
async fn a_sign_in_and_a_prompt_in_front_are_told_apart() {
    let s = Setup::new();
    s.launching_app();
    let _stream = s.streaming();
    let sign_in = || {
        vec![window(
            "EpicGamesLauncher.exe",
            "Sign in to Epic Games",
            false,
        )]
    };
    s.look(sign_in());
    s.look(sign_in());
    let notice = s.get("/session").await.session().launcher.unwrap();
    assert_eq!(
        (notice.launcher.as_str(), notice.kind),
        ("epic", LauncherKind::SignIn)
    );

    // A launcher in front with a title that says nothing is a prompt only after the grace,
    // which a game launched a moment ago has not passed.
    let front = || vec![window("EADesktop.exe", "EA app", true)];
    s.look(front());
    s.look(front());
    assert!(s.get("/session").await.session().launcher.is_none());
}

#[tokio::test]
async fn another_device_does_not_see_the_launcher() {
    let s = Setup::new();
    s.launching_app();
    let _stream = s.streaming();
    s.look(updating());
    s.look(updating());
    let snapshot = s.host().session_snapshot();
    assert!(snapshot.launcher.is_some());
    let mut other = s.client.clone();
    other.uuid = "someone-else".into();
    assert!(
        butterpollo_core::native::session_view(&snapshot, &other)
            .launcher
            .is_none()
    );
}

#[tokio::test]
async fn an_app_that_starts_no_program_or_a_game_that_ended_is_not_watched() {
    let s = Setup::new();
    s.host().apps.write().unwrap()[0].cmd = String::new();
    let _stream = s.streaming();
    crate::launcher_watch::look(s.host(), || panic!("a desktop app has no launcher to read"));
    assert!(s.get("/session").await.session().launcher.is_none());

    // A shown notice goes when the app is gone.
    let t = Setup::new();
    t.launching_app();
    let _stream = t.streaming();
    t.look(updating());
    t.look(updating());
    assert!(t.host().session_snapshot().launcher.is_some());
    *t.host().current_app.lock().unwrap() = None;
    crate::launcher_watch::look(t.host(), || panic!("no app, nothing to read"));
    assert!(t.host().session_snapshot().launcher.is_none());
}

#[tokio::test]
async fn an_app_launched_from_the_console_has_no_device_to_tell() {
    let s = Setup::new();
    s.launching_app();
    let _stream = s.streaming();
    s.host().current_app.lock().unwrap().as_mut().unwrap().owner = String::new();
    crate::launcher_watch::look(s.host(), || panic!("nobody to tell"));
    assert!(s.host().session_snapshot().launcher.is_none());
}
