//! Quick Suspend's words and its stream menu tile, the same on both clients: the host pauses the
//! running game's processes and resumes them later (`docs/native-api.md`, "Quick Suspend";
//! control message 0x554B). The host decides; this only says which tile a state offers and words
//! its refusals in the player's language. Each [`Words`] has a `key` naming Android's
//! `quick_suspend_<key>` string and the English desktop Nightjar translates with `tr`.

/// A line both clients show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    pub key: &'static str,
    pub en: &'static str,
}

const fn w(key: &'static str, en: &'static str) -> Words {
    Words { key, en }
}

/// The stream menu tile that pauses the game ([`crate::stream_menu::TILES`]).
pub const SUSPEND_TILE: &str = "suspend_game";
/// The tile that resumes it while it is paused.
pub const RESUME_TILE: &str = "resume_game";

/// What the stream shows while the game is paused, until it resumes.
pub const PAUSED: Words = w(
    "paused",
    "The game is paused on the PC. Open the stream menu to resume it.",
);
/// What it says once the game runs again.
pub const RESUMED: Words = w("resumed", "Game resumed.");

/// Every refusal [`refusal`] gives, for the clients' tests.
pub const REFUSALS: [Words; 7] = [
    w(
        "refused_anti_cheat",
        "This game runs anti-cheat, so the PC won’t pause it.",
    ),
    w(
        "refused_protected",
        "A protected or system program runs with this game, so the PC won’t pause it.",
    ),
    w(
        "refused_others",
        "Others are playing or watching, so the PC won’t pause the game.",
    ),
    w(
        "refused_not_owner",
        "Only the device that started the game, or one with host control, can pause it.",
    ),
    w(
        "refused_busy",
        "The game is starting or stopping. Try again in a moment.",
    ),
    w(
        "refused_nothing",
        "There is no game the PC can pause right now.",
    ),
    w(
        "refused_failed",
        "The PC couldn’t pause the game, so nothing stays paused.",
    ),
];

/// Why the PC did not pause the game, from the host's stable reason name (0x554B's reason or the
/// native API's `reason`). A reason from a newer host reads as a failure.
pub fn refusal(reason: &str) -> Words {
    let index = match reason {
        "anti_cheat_detected" => 0,
        "protected_process" | "denylisted_process" => 1,
        "guests_active" | "coop_session" => 2,
        "not_owner" => 3,
        "session_busy" => 4,
        "no_session" | "no_owned_process" | "disabled_for_app" => 5,
        _ => 6,
    };
    REFUSALS[index]
}

/// The tile a state offers, from the host's state name (`ready`, `suspended`, ...): Suspend game
/// while the game can be paused, Resume game while it is paused or resuming, none otherwise (Quick
/// Suspend off for this game, another device's session, nothing running).
pub fn tile(state: &str) -> Option<&'static str> {
    match state {
        "ready" => Some(SUSPEND_TILE),
        "suspended" | "resuming" => Some(RESUME_TILE),
        _ => None,
    }
}

/// Whether the host's state name means the game is paused.
pub fn paused(state: &str) -> bool {
    tile(state) == Some(RESUME_TILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_the_host_names_has_words_and_an_unknown_one_reads_as_a_failure() {
        let reasons = [
            "disabled_for_app",
            "no_owned_process",
            "anti_cheat_detected",
            "protected_process",
            "denylisted_process",
            "guests_active",
            "coop_session",
            "not_owner",
            "session_busy",
            "no_session",
            "io_or_os_error",
        ];
        let mut used = std::collections::BTreeSet::new();
        for reason in reasons {
            used.insert(refusal(reason).key);
        }
        assert_eq!(used.len(), REFUSALS.len(), "every refusal is reachable");
        assert_eq!(refusal("hibernate_failed"), REFUSALS[6]);
        for words in REFUSALS.iter().chain([&PAUSED, &RESUMED]) {
            assert!(words.en.ends_with('.'), "{:?}", words.en);
            assert!(words
                .key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }

    #[test]
    fn the_tile_follows_the_hosts_state() {
        assert_eq!(tile("ready"), Some(SUSPEND_TILE));
        assert_eq!(tile("suspended"), Some(RESUME_TILE));
        assert_eq!(tile("resuming"), Some(RESUME_TILE));
        for state in ["unavailable", "refused", "unknown", ""] {
            assert_eq!(tile(state), None, "{state}");
        }
        assert!(paused("suspended") && !paused("ready"));
    }

    /// Android words them as the policy does: each `quick_suspend_<key>` in English.
    #[test]
    fn androids_english_strings_match() {
        let xml = include_str!("../../../app/src/main/res/values/quick_suspend.xml");
        for words in REFUSALS.iter().chain([&PAUSED, &RESUMED]) {
            let open = format!("<string name=\"quick_suspend_{}\">", words.key);
            let text = xml
                .split(&open)
                .nth(1)
                .and_then(|rest| rest.split("</string>").next())
                .unwrap_or_else(|| panic!("Android has no quick_suspend_{}", words.key));
            assert_eq!(text.replace("\'", "'"), words.en, "{}", words.key);
        }
    }
}
