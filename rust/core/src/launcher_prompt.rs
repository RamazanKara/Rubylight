//! A launcher (Steam, Epic Games, EA app, Ubisoft Connect) that shows a prompt or an update in
//! place of the game it was asked to start (plan item B5). The host reads the visible windows
//! while a launched game has not shown up and this module decides what they mean. It only
//! looks: nothing here, or in the host, clicks or types into a launcher.
//!
//! Evidence, strongest first:
//! * a launcher window whose title says what it wants (an update, a sign in);
//! * a launcher window in front of everything for longer than a game takes to open
//!   ([`GRACE`]), with a title that says nothing: a licence page, a settings question, the
//!   launcher's own library.
//!
//! Windows flash by: a notice needs two looks in a row, and goes when two looks in a row
//! find none, or when the game shows up.
use crate::native_api::session::{LauncherKind, LauncherNotice};
use std::time::Duration;

/// How long a launcher may sit in front before its window counts as a prompt in its own right.
pub const GRACE: Duration = Duration::from_secs(20);
/// How long after the launch the windows are still read. A game that has not shown by then is
/// not held up by a prompt, and the host stops polling the desktop for it.
pub const WATCH_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The longest title kept for the client.
const TITLE_CHARS: usize = 120;

/// A visible top-level window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The program's file name (`steamwebhelper.exe`).
    pub process: String,
    pub title: String,
    /// Whether it is the window in front.
    pub foreground: bool,
}

/// A launcher's id for a program, or none for any other program.
pub fn launcher_of(process: &str) -> Option<&'static str> {
    let name = process.rsplit(['/', '\\']).next().unwrap_or(process);
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "steam.exe" | "steamwebhelper.exe" | "steamservice.exe" => Some("steam"),
        "epicgameslauncher.exe" | "epicwebhelper.exe" => Some("epic"),
        "eadesktop.exe" | "eaconnect_microsoft.exe" | "eaclient.exe" | "origin.exe" => Some("ea"),
        "upc.exe" | "ubisoftconnect.exe" | "uplayservice.exe" | "uplaywebcore.exe" => {
            Some("ubisoft")
        }
        _ => None,
    }
}

/// Whether a command line only opens a launcher's own window (Steam Big Picture, the Steam
/// library, the launcher program with no game named) and starts no game. The launcher is then
/// the app: a prompt on screen is what the client asked for, not a game held up by one.
pub fn opens_launcher_itself(command: &str) -> bool {
    let command = command.trim().to_ascii_lowercase();
    let (target, rest) = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
        // An unquoted path may hold spaces ("C:\Program Files (x86)\Steam\steam.exe -silent"):
        // the program ends at the first ".exe".
        None => match command.find(".exe") {
            Some(end) => command.split_at(end + ".exe".len()),
            None => command
                .split_once(char::is_whitespace)
                .unwrap_or((command.as_str(), "")),
        },
    };
    // A command that names a game to start is waited for, wherever it is written.
    let starts_a_game = |text: &str| {
        ["applaunch", "rungameid", "launch", "steam://run/"]
            .iter()
            .any(|word| text.contains(word))
    };
    if target.starts_with("steam://") {
        return !starts_a_game(target);
    }
    launcher_of(target).is_some() && !starts_a_game(rest)
}

/// What a title asks for, from its words.
fn kind_of(title: &str) -> Option<LauncherKind> {
    let title = title.to_lowercase();
    const SIGN_IN: [&str; 9] = [
        "sign in",
        "sign-in",
        "log in",
        "login",
        "verification",
        "steam guard",
        "two-factor",
        "authenticator",
        "security code",
    ];
    const UPDATE: [&str; 8] = [
        "updat",
        "download",
        "installing",
        "install ",
        "patch",
        "preparing",
        "verifying",
        "required files",
    ];
    if SIGN_IN.iter().any(|word| title.contains(word)) {
        Some(LauncherKind::SignIn)
    } else if UPDATE.iter().any(|word| title.contains(word)) {
        Some(LauncherKind::Update)
    } else {
        None
    }
}

/// What the windows show, or none: the first window that names its need, else the launcher
/// window in front once `since_launch` passed [`GRACE`].
pub fn find(windows: &[Window], since_launch: Duration) -> Option<(String, LauncherKind, String)> {
    let mut front = None;
    for window in windows {
        let Some(launcher) = launcher_of(&window.process) else {
            continue;
        };
        let title = window.title.trim();
        if title.is_empty() {
            continue;
        }
        if let Some(kind) = kind_of(title) {
            return Some((launcher.to_owned(), kind, clipped(title)));
        }
        if window.foreground {
            front = Some((launcher.to_owned(), LauncherKind::Prompt, clipped(title)));
        }
    }
    front.filter(|_| since_launch >= GRACE)
}

fn clipped(title: &str) -> String {
    title.chars().take(TITLE_CHARS).collect()
}

/// Follows the windows from look to look. One per launched game.
#[derive(Debug, Default)]
pub struct Watch {
    shown: Option<LauncherNotice>,
    candidate: Option<(String, LauncherKind)>,
    streak: u8,
    absent: u8,
}

impl Watch {
    /// One look. `game_up` says the game's own window or process has shown up, which ends any
    /// notice. Returns the notice to show now.
    pub fn look(
        &mut self,
        now_unix_ms: u64,
        since_launch: Duration,
        game_up: bool,
        windows: &[Window],
    ) -> Option<LauncherNotice> {
        let found = if game_up {
            None
        } else {
            find(windows, since_launch)
        };
        match found {
            None => {
                self.candidate = None;
                self.streak = 0;
                self.absent = self.absent.saturating_add(1);
                if game_up || self.absent >= 2 {
                    self.shown = None;
                }
            }
            Some((launcher, kind, title)) => {
                self.absent = 0;
                let same = self.candidate.as_ref() == Some(&(launcher.clone(), kind));
                self.candidate = Some((launcher.clone(), kind));
                self.streak = if same {
                    self.streak.saturating_add(1)
                } else {
                    1
                };
                if let Some(shown) = &mut self.shown
                    && shown.launcher == launcher
                    && shown.kind == kind
                {
                    // The same prompt; its title may move (a percentage).
                    shown.window_title = title;
                } else if self.streak >= 2 {
                    self.shown = Some(LauncherNotice {
                        launcher,
                        kind,
                        window_title: title,
                        since_unix_ms: now_unix_ms,
                    });
                }
            }
        }
        self.shown.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(process: &str, title: &str, foreground: bool) -> Window {
        Window {
            process: process.into(),
            title: title.into(),
            foreground,
        }
    }
    const SOON: Duration = Duration::from_secs(3);
    const LATE: Duration = Duration::from_secs(40);

    #[test]
    fn the_four_launchers_are_known_by_their_programs() {
        for (program, id) in [
            ("C:\\Program Files (x86)\\Steam\\steam.exe", "steam"),
            ("steamwebhelper.exe", "steam"),
            ("EpicGamesLauncher.exe", "epic"),
            ("EADesktop.exe", "ea"),
            ("upc.exe", "ubisoft"),
            ("UbisoftConnect.exe", "ubisoft"),
        ] {
            assert_eq!(launcher_of(program), Some(id), "{program}");
        }
        for program in ["game.exe", "explorer.exe", "chrome.exe", ""] {
            assert_eq!(launcher_of(program), None, "{program}");
        }
    }

    #[test]
    fn a_command_that_only_opens_a_launcher_is_not_a_game_waiting_on_one() {
        for command in [
            "steam://open/bigpicture",
            " steam://open/games ",
            r#""C:\Program Files (x86)\Steam\steam.exe""#,
            r#""C:\Program Files (x86)\Steam\steam.exe" -bigpicture"#,
            r"C:\Epic\EpicGamesLauncher.exe",
            r"C:\Program Files (x86)\Steam\steam.exe",
            r"C:\Program Files (x86)\Steam\steam.exe -bigpicture",
        ] {
            assert!(opens_launcher_itself(command), "{command}");
        }
        for command in [
            r"C:\Program Files (x86)\Steam\steam.exe -applaunch 570",
            "steam://rungameid/570",
            "steam://run/570",
            r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 570"#,
            "com.epicgames.launcher://apps/Fortnite?action=launch&silent=true",
            r"C:\Games\game.exe",
            "game.exe",
            "",
        ] {
            assert!(!opens_launcher_itself(command), "{command}");
        }
    }

    #[test]
    fn a_title_that_names_its_need_is_reported_at_once() {
        let steam = |title| vec![window("steamwebhelper.exe", title, false)];
        let (launcher, kind, _) = find(&steam("Updating Dota 2"), SOON).unwrap();
        assert_eq!((launcher.as_str(), kind), ("steam", LauncherKind::Update));
        assert_eq!(
            find(
                &steam("Steam Guard - Computer Authorization Required"),
                SOON
            )
            .unwrap()
            .1,
            LauncherKind::SignIn
        );
        let epic = vec![window(
            "EpicGamesLauncher.exe",
            "Sign in to Epic Games",
            false,
        )];
        assert_eq!(find(&epic, SOON).unwrap().1, LauncherKind::SignIn);
        let ea = vec![window("EADesktop.exe", "Downloading update", false)];
        assert_eq!(find(&ea, SOON).unwrap().1, LauncherKind::Update);
        let ubisoft = vec![window("upc.exe", "Ubisoft Connect - Login", false)];
        assert_eq!(find(&ubisoft, SOON).unwrap().1, LauncherKind::SignIn);
    }

    #[test]
    fn a_sign_in_wins_over_an_update_in_one_title() {
        let both = vec![window("steam.exe", "Sign in to update", false)];
        assert_eq!(find(&both, SOON).unwrap().1, LauncherKind::SignIn);
    }

    #[test]
    fn a_launcher_in_front_counts_only_after_the_grace() {
        let front = vec![window("steamwebhelper.exe", "Steam", true)];
        assert_eq!(find(&front, SOON), None);
        let (_, kind, title) = find(&front, LATE).unwrap();
        assert_eq!((kind, title.as_str()), (LauncherKind::Prompt, "Steam"));
        // Behind another window it is just a launcher running.
        let behind = vec![window("steamwebhelper.exe", "Steam", false)];
        assert_eq!(find(&behind, LATE), None);
    }

    #[test]
    fn other_programs_and_untitled_windows_are_not_launcher_prompts() {
        let other = vec![
            window("chrome.exe", "Updating Chrome", true),
            window("steam.exe", "", true),
            window("steam.exe", "   ", true),
        ];
        assert_eq!(find(&other, LATE), None);
    }

    #[test]
    fn a_long_title_is_clipped_for_the_client() {
        let long = "Updating ".to_owned() + &"x".repeat(500);
        let found = find(&[window("steam.exe", &long, false)], SOON).unwrap();
        assert_eq!(found.2.chars().count(), TITLE_CHARS);
    }

    #[test]
    fn a_notice_needs_two_looks_and_keeps_its_first_time() {
        let mut watch = Watch::default();
        let update = |title| vec![window("steamwebhelper.exe", title, false)];
        assert_eq!(watch.look(1000, SOON, false, &update("Updating A")), None);
        let shown = watch
            .look(2000, SOON, false, &update("Updating A 40%"))
            .unwrap();
        assert_eq!(
            (shown.launcher.as_str(), shown.kind, shown.since_unix_ms),
            ("steam", LauncherKind::Update, 2000)
        );
        // The title moves, the time of the first sight stays.
        let later = watch
            .look(9000, SOON, false, &update("Updating A 90%"))
            .unwrap();
        assert_eq!(later.since_unix_ms, 2000);
        assert_eq!(later.window_title, "Updating A 90%");
    }

    #[test]
    fn a_flash_of_one_look_shows_nothing_and_a_gap_of_one_look_keeps_the_notice() {
        let mut watch = Watch::default();
        let update = vec![window("steam.exe", "Updating", false)];
        assert_eq!(watch.look(1, SOON, false, &update), None);
        assert_eq!(watch.look(2, SOON, false, &[]), None);
        assert_eq!(watch.look(3, SOON, false, &update), None);
        assert!(watch.look(4, SOON, false, &update).is_some());
        assert!(
            watch.look(5, SOON, false, &[]).is_some(),
            "one look without it"
        );
        assert!(
            watch.look(6, SOON, false, &[]).is_none(),
            "two looks without it"
        );
    }

    #[test]
    fn the_game_showing_up_ends_the_notice_at_once() {
        let mut watch = Watch::default();
        let update = vec![window("steam.exe", "Updating", false)];
        watch.look(1, SOON, false, &update);
        assert!(watch.look(2, SOON, false, &update).is_some());
        assert!(watch.look(3, SOON, true, &update).is_none());
        // And it does not come back on the next look while the game is up.
        assert!(watch.look(4, SOON, true, &update).is_none());
    }

    #[test]
    fn a_different_prompt_replaces_the_shown_one_after_two_looks() {
        let mut watch = Watch::default();
        let update = vec![window("steam.exe", "Updating", false)];
        let sign_in = vec![window("steam.exe", "Sign in", false)];
        watch.look(1, SOON, false, &update);
        watch.look(2, SOON, false, &update);
        assert_eq!(
            watch.look(3, SOON, false, &sign_in).unwrap().kind,
            LauncherKind::Update
        );
        let now = watch.look(4, SOON, false, &sign_in).unwrap();
        assert_eq!((now.kind, now.since_unix_ms), (LauncherKind::SignIn, 4));
    }
}
