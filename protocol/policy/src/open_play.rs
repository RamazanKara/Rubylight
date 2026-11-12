//! "When Nightjar opens": whether a start of the app goes on to a game. Both clients ask here;
//! Android through `com.limelight.console.OpenPlayModel` and [`json`].
//!
//! It never traps the player. Only a cold start from the launcher or at sign-in counts, and a guard
//! skips the next one after an automatic start that came to nothing. The guard is armed when the
//! countdown ends and stays armed unless something says the start went well (the stream outlived
//! [`SETTLED_MS`]) or was the player's to end (they cancelled or left), so a crash or a killed app
//! counts as a failure too.

use serde::Deserialize;
use serde_json::{json, Value};

/// How long the countdown names the game before it starts.
pub const COUNTDOWN_MS: u64 = 3_000;
/// A stream that lasts this long proves the automatic start works.
pub const SETTLED_MS: u64 = 10_000;

/// The setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opens {
    /// The PCs (or Home): nothing starts by itself.
    Pcs,
    /// The last game played.
    Last,
    /// The game chosen in its menu.
    Chosen,
}

impl Opens {
    /// The stored value; a value this version does not know starts nothing.
    pub fn from_key(key: &str) -> Self {
        match key {
            "last" => Self::Last,
            "chosen" => Self::Chosen,
            _ => Self::Pcs,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Pcs => "pcs",
            Self::Last => "last",
            Self::Chosen => "chosen",
        }
    }
}

/// "When Nightjar opens" as the one choice both clients show: Home, the PCs, or a countdown into
/// the last or the chosen game. It is kept in the two values older versions read: the setting
/// behind [`Opens`] (`pcs`, `last`, `chosen`) and Home over the list of PCs (desktop
/// `console_home`, Android `checkbox_console_home`). Nothing is rewritten on reading them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartScreen {
    Home,
    Pcs,
    Last,
    Chosen,
}

impl StartScreen {
    /// The choices in the order both clients list them.
    pub const ALL: [Self; 4] = [Self::Home, Self::Pcs, Self::Last, Self::Chosen];

    /// The choice the two saved values make: `last` and `chosen` whatever `console_home` holds,
    /// anything else Home or the PCs by `console_home`.
    pub fn from_saved(open: &str, console_home: bool) -> Self {
        match Opens::from_key(open) {
            Opens::Last => Self::Last,
            Opens::Chosen => Self::Chosen,
            Opens::Pcs if console_home => Self::Home,
            Opens::Pcs => Self::Pcs,
        }
    }

    /// The two values to save for this choice: the [`Opens`] key and Home over the PCs. A game's
    /// countdown keeps `console_home` as it was, so the screen behind the countdown stays the same.
    pub fn to_saved(self, console_home: bool) -> (&'static str, bool) {
        match self {
            Self::Home => (Opens::Pcs.key(), true),
            Self::Pcs => (Opens::Pcs.key(), false),
            Self::Last => (Opens::Last.key(), console_home),
            Self::Chosen => (Opens::Chosen.key(), console_home),
        }
    }

    /// What a cold start goes on to.
    pub fn opens(self) -> Opens {
        match self {
            Self::Home | Self::Pcs => Opens::Pcs,
            Self::Last => Opens::Last,
            Self::Chosen => Opens::Chosen,
        }
    }

    /// The choice's name on the JSON wire.
    pub fn key(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Pcs => "pcs",
            Self::Last => "last",
            Self::Chosen => "chosen",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|screen| screen.key() == key)
    }
}

/// How this screen came to be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// The first screen this process made: not a stream's return or a later visit.
    pub first_screen: bool,
    /// Opened by the launcher or at sign-in: not a shortcut, a link, a command line or another app.
    pub launcher: bool,
    /// The system rebuilt it from saved state (recreated, or brought back after the process died).
    pub restored: bool,
    /// Opened from Recents.
    pub from_recents: bool,
}

impl Entry {
    pub fn is_cold_start(self) -> bool {
        self.first_screen && self.launcher && !self.restored && !self.from_recents
    }
}

/// Which game a decision is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Last,
    Chosen,
}

/// What a start of the app does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Opens as usual.
    Stay,
    /// Counts down, then starts the game.
    Countdown(Game),
    /// Would start the game, but the last automatic start failed: opens as usual and says so.
    Skip(Game),
}

/// What is known at the start of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub entry: Entry,
    pub opens: Opens,
    /// A game was played before and its PC is still saved.
    pub has_last: bool,
    /// A game was chosen and its PC is still saved.
    pub has_chosen: bool,
    /// The guard is armed: the last automatic start did not end well.
    pub armed: bool,
}

/// The answer to [`decide`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    /// Clear the guard now: a cold start has used it up, so one failure skips one start, and a
    /// setting changed since then does not bring back an old failure later.
    pub disarm: bool,
}

pub fn decide(facts: Facts) -> Decision {
    if !facts.entry.is_cold_start() {
        return Decision { action: Action::Stay, disarm: false };
    }
    let game = match facts.opens {
        Opens::Last if facts.has_last => Some(Game::Last),
        Opens::Chosen if facts.has_chosen => Some(Game::Chosen),
        _ => None,
    };
    let action = match game {
        Some(game) if facts.armed => Action::Skip(game),
        Some(game) => Action::Countdown(game),
        None => Action::Stay,
    };
    Decision { action, disarm: facts.armed }
}

/// Whether the stream of an automatic start ended well enough to clear the guard: it lasted, or the
/// player left it themselves. A stream that died young keeps it armed.
pub fn stream_end_disarms(connected_ms: u64, left_by_player: bool) -> bool {
    left_by_player || connected_ms >= SETTLED_MS
}

/// One JSON request in, one JSON answer out, for Android: `{"ok": …}` or `{"error": code, "detail": …}`.
pub mod json {
    use super::*;

    #[derive(Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    enum Request {
        Decide {
            first_screen: bool,
            launcher: bool,
            restored: bool,
            from_recents: bool,
            opens: String,
            has_last: bool,
            has_chosen: bool,
            armed: bool,
        },
        StreamEnd { connected_ms: u64, left_by_player: bool },
        Timing,
        /// The choice the saved values make ([`StartScreen::from_saved`]).
        StartScreen { open: String, console_home: bool },
        /// The values to save for a choice ([`StartScreen::to_saved`]).
        SaveStartScreen { screen: String, console_home: bool },
    }

    fn game_key(game: Game) -> &'static str {
        match game {
            Game::Last => "last",
            Game::Chosen => "chosen",
        }
    }

    fn decision(decision: Decision) -> Value {
        let (action, game) = match decision.action {
            Action::Stay => ("stay", None),
            Action::Countdown(game) => ("countdown", Some(game_key(game))),
            Action::Skip(game) => ("skip", Some(game_key(game))),
        };
        json!({"action": action, "game": game, "disarm": decision.disarm})
    }

    pub fn call(request: &str) -> String {
        let answer = match serde_json::from_str::<Request>(request) {
            Err(e) => return json!({"error": "invalid", "detail": e.to_string()}).to_string(),
            Ok(Request::Decide { first_screen, launcher, restored, from_recents, opens, has_last, has_chosen, armed }) => {
                let entry = Entry { first_screen, launcher, restored, from_recents };
                decision(decide(Facts { entry, opens: Opens::from_key(&opens), has_last, has_chosen, armed }))
            }
            Ok(Request::StreamEnd { connected_ms, left_by_player }) => {
                json!({"disarm": stream_end_disarms(connected_ms, left_by_player)})
            }
            Ok(Request::Timing) => json!({"countdown_ms": COUNTDOWN_MS, "settled_ms": SETTLED_MS}),
            Ok(Request::StartScreen { open, console_home }) => {
                json!({"screen": super::StartScreen::from_saved(&open, console_home).key()})
            }
            Ok(Request::SaveStartScreen { screen, console_home }) => match super::StartScreen::from_key(&screen) {
                Some(screen) => {
                    let (open, console_home) = screen.to_saved(console_home);
                    json!({"open": open, "console_home": console_home})
                }
                None => return json!({"error": "invalid", "detail": format!("unknown screen {screen}")}).to_string(),
            },
        };
        json!({ "ok": answer }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLD: Entry = Entry { first_screen: true, launcher: true, restored: false, from_recents: false };

    fn facts(opens: Opens) -> Facts {
        Facts { entry: COLD, opens, has_last: true, has_chosen: true, armed: false }
    }

    #[test]
    fn the_default_opens_as_usual_and_a_setting_it_does_not_know_too() {
        assert_eq!(decide(facts(Opens::Pcs)).action, Action::Stay);
        assert_eq!(Opens::from_key("something newer"), Opens::Pcs);
        for opens in [Opens::Pcs, Opens::Last, Opens::Chosen] {
            assert_eq!(Opens::from_key(opens.key()), opens);
        }
    }

    #[test]
    fn a_cold_start_counts_down_to_the_set_game() {
        assert_eq!(decide(facts(Opens::Last)).action, Action::Countdown(Game::Last));
        assert_eq!(decide(facts(Opens::Chosen)).action, Action::Countdown(Game::Chosen));
    }

    #[test]
    fn only_a_cold_start_from_the_launcher_counts() {
        let not_cold = [
            ("a stream's return or a later visit", Entry { first_screen: false, ..COLD }),
            ("a shortcut, a link or a command line", Entry { launcher: false, ..COLD }),
            ("a recreated screen", Entry { restored: true, ..COLD }),
            ("Recents", Entry { from_recents: true, ..COLD }),
        ];
        for (name, entry) in not_cold {
            let decision = decide(Facts { entry, armed: true, ..facts(Opens::Last) });
            assert_eq!(decision, Decision { action: Action::Stay, disarm: false }, "{name}: the guard waits too");
        }
    }

    #[test]
    fn nothing_to_start_opens_as_usual() {
        assert_eq!(decide(Facts { has_last: false, ..facts(Opens::Last) }).action, Action::Stay);
        assert_eq!(decide(Facts { has_chosen: false, ..facts(Opens::Chosen) }).action, Action::Stay);
    }

    #[test]
    fn the_guard_skips_exactly_one_start() {
        let armed = decide(Facts { armed: true, ..facts(Opens::Last) });
        assert_eq!(armed, Decision { action: Action::Skip(Game::Last), disarm: true });
        // The caller cleared it, so the next one starts again.
        assert_eq!(decide(facts(Opens::Last)), Decision { action: Action::Countdown(Game::Last), disarm: false });
    }

    #[test]
    fn a_setting_changed_after_a_failure_does_not_bring_it_back_later() {
        assert_eq!(decide(Facts { armed: true, ..facts(Opens::Pcs) }), Decision { action: Action::Stay, disarm: true });
    }

    #[test]
    fn a_stream_clears_the_guard_only_by_lasting_or_when_the_player_leaves() {
        assert!(!stream_end_disarms(0, false));
        assert!(!stream_end_disarms(SETTLED_MS - 1, false), "died within its first seconds");
        assert!(stream_end_disarms(SETTLED_MS, false));
        assert!(stream_end_disarms(1, true), "the player left");
    }

    #[test]
    fn the_start_screen_reads_both_saved_values_and_saves_them_back() {
        // Existing pairs read as they are.
        for console_home in [true, false] {
            assert_eq!(StartScreen::from_saved("last", console_home), StartScreen::Last);
            assert_eq!(StartScreen::from_saved("chosen", console_home), StartScreen::Chosen);
        }
        assert_eq!(StartScreen::from_saved("pcs", true), StartScreen::Home, "a new install: Home");
        assert_eq!(StartScreen::from_saved("pcs", false), StartScreen::Pcs);
        assert_eq!(StartScreen::from_saved("something newer", true), StartScreen::Home);
        assert_eq!(StartScreen::from_saved("something newer", false), StartScreen::Pcs);
        // Each choice writes what reads back as it, and a game keeps the screen behind its countdown.
        for screen in StartScreen::ALL {
            for console_home in [true, false] {
                let (open, home) = screen.to_saved(console_home);
                assert_eq!(StartScreen::from_saved(open, home), screen, "{screen:?} {console_home}");
                assert_eq!(Opens::from_key(open), screen.opens());
                assert_eq!(StartScreen::from_key(screen.key()), Some(screen));
            }
        }
        assert_eq!(StartScreen::Home.to_saved(false), ("pcs", true));
        assert_eq!(StartScreen::Pcs.to_saved(true), ("pcs", false));
        assert_eq!(StartScreen::Last.to_saved(false), ("last", false));
        assert_eq!(StartScreen::Chosen.to_saved(true), ("chosen", true));
        assert_eq!(StartScreen::from_key("teleport"), None);
    }

    #[test]
    fn json_answers_the_start_screen_both_ways() {
        let ask = |request: &str| -> Value { serde_json::from_str(&json::call(request)).unwrap() };
        assert_eq!(ask(r#"{"op":"start_screen","open":"pcs","console_home":true}"#), serde_json::json!({"ok": {"screen": "home"}}));
        assert_eq!(ask(r#"{"op":"start_screen","open":"last","console_home":false}"#), serde_json::json!({"ok": {"screen": "last"}}));
        assert_eq!(ask(r#"{"op":"save_start_screen","screen":"pcs","console_home":true}"#), serde_json::json!({"ok": {"open": "pcs", "console_home": false}}));
        assert_eq!(ask(r#"{"op":"save_start_screen","screen":"chosen","console_home":false}"#), serde_json::json!({"ok": {"open": "chosen", "console_home": false}}));
        assert_eq!(ask(r#"{"op":"save_start_screen","screen":"teleport","console_home":false}"#)["error"], "invalid");
    }

    #[test]
    fn json_answers_and_names_a_bad_request() {
        let answer: Value = serde_json::from_str(&json::call(r#"{"op":"timing"}"#)).unwrap();
        assert_eq!(answer["ok"]["settled_ms"], 10_000);
        let answer: Value = serde_json::from_str(&json::call(r#"{"op":"nope"}"#)).unwrap();
        assert_eq!(answer["error"], "invalid");
    }
}
