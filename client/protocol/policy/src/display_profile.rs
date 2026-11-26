//! Per-display profile recall (nightjar#163, #137): the saved stream profile a screen calls up.
//!
//! A profile the player keeps "for this screen" carries a [`DisplayMatcher`]; when a stream
//! starts on a screen that matches it, that profile is picked ahead of the network choice and the
//! profile in use, so a laptop or phone moved between its own screen and an external monitor gets
//! each screen's resolution, refresh rate and HDR back. A profile without a matcher, or a screen
//! no profile is kept for, picks exactly as before. Both apps call [`pick`]: desktop directly,
//! Android through `protocol/android/src/display_profile.rs` with the JSON of [`json::call`].

use serde::{Deserialize, Serialize};

/// The screen a profile is kept for, by an identity that outlasts reconnects and restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayMatcher {
    /// What identifies the screen. Desktop: the name the system reads from the monitor's EDID
    /// (`DELL U2720Q`, `Built-in Retina Display`), the same for that monitor on any port; Android:
    /// [`DisplayMatcher::BUILT_IN`] for the device's own screen, or `external:` and the name of
    /// the connected display.
    pub id: String,
    /// The screen's name as the editor shows it.
    pub name: String,
}

impl DisplayMatcher {
    /// Android's identity for the device's own screen (both panels of a foldable).
    pub const BUILT_IN: &'static str = "builtin";
    /// Android's prefix for a connected display's identity.
    pub const EXTERNAL: &'static str = "external:";

    /// A screen by its identity and name; None when the identity is blank, as an unnamed screen
    /// cannot be told apart from the next one.
    pub fn new(id: &str, name: &str) -> Option<Self> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let name = name.trim();
        Some(Self { id: id.to_owned(), name: if name.is_empty() { id } else { name }.to_owned() })
    }

    /// True when `screen` is the screen this matcher was saved for. Names compare without
    /// regard to case or surrounding spaces, as systems differ in how they report them.
    pub fn matches(&self, screen: &DisplayMatcher) -> bool {
        let (mine, theirs) = (self.id.trim(), screen.id.trim());
        !mine.is_empty() && mine.to_lowercase() == theirs.to_lowercase()
    }
}

/// The profile kept for `screen`: the first of `rules` (a profile and its matcher, if any) whose
/// matcher matches. None when the screen is unknown or no profile is kept for it, so the network
/// choice or the profile in use decides as before.
pub fn pick<'a, T>(rules: impl IntoIterator<Item = (T, Option<&'a DisplayMatcher>)>, screen: Option<&DisplayMatcher>) -> Option<T> {
    let screen = screen?;
    rules.into_iter().find_map(|(id, matcher)| matcher.filter(|m| m.matches(screen)).map(|_| id))
}

/// One JSON request in, one JSON answer out, for Android's `StreamLayers`.
pub mod json {
    use super::{pick, DisplayMatcher};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Rule {
        profile: String,
        display: Option<DisplayMatcher>,
    }

    #[derive(Deserialize)]
    struct Request {
        #[serde(default)]
        rules: Vec<Rule>,
        screen: Option<DisplayMatcher>,
    }

    /// `{"rules":[{"profile":"3","display":{"id":"builtin","name":"…"}}],"screen":{"id":"builtin"}}`
    /// answers `{"profile":"3"}`, or `{"profile":null}` when no rule matches; a request that can't
    /// be read answers `{"error":"invalid","detail":…}`.
    pub fn call(request: &str) -> String {
        match serde_json::from_str::<Request>(request) {
            Ok(request) => {
                let picked = pick(request.rules.iter().map(|r| (r.profile.as_str(), r.display.as_ref())), request.screen.as_ref());
                serde_json::json!({ "profile": picked }).to_string()
            }
            Err(e) => serde_json::json!({ "error": "invalid", "detail": e.to_string() }).to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: &str) -> DisplayMatcher {
        DisplayMatcher::new(id, "").unwrap()
    }

    #[test]
    fn a_screen_calls_up_the_profile_kept_for_it_and_nothing_else() {
        let (retina, dell) = (screen("Built-in Retina Display"), screen("DELL U2720Q"));
        let rules = [(1u64, None), (2, Some(&retina)), (3, Some(&dell))];
        assert_eq!(pick(rules, Some(&screen("built-in retina display "))), Some(2));
        assert_eq!(pick(rules, Some(&dell)), Some(3));
        assert_eq!(pick(rules, Some(&screen("LG HDR 4K"))), None);
        assert_eq!(pick(rules, None), None);
        // No profile kept for any screen: nothing is picked, as before 7.0.
        assert_eq!(pick([(1u64, None), (2, None)], Some(&retina)), None);
    }

    #[test]
    fn a_blank_identity_matches_nothing() {
        assert!(DisplayMatcher::new("  ", "Screen").is_none());
        let blank = DisplayMatcher::default();
        assert!(!blank.matches(&blank));
        assert_eq!(pick([(1u64, Some(&blank))], Some(&blank)), None);
        assert_eq!(DisplayMatcher::new(" DELL ", " ").unwrap().name, "DELL");
    }

    #[test]
    fn answers_android_requests_in_json() {
        let request = r#"{"rules":[{"profile":"4","display":null},{"profile":"7","display":{"id":"external:HDMI Screen","name":"HDMI Screen"}}],"screen":{"id":"external:hdmi screen"}}"#;
        assert_eq!(json::call(request), r#"{"profile":"7"}"#);
        assert_eq!(json::call(r#"{"rules":[],"screen":{"id":"builtin"}}"#), r#"{"profile":null}"#);
        assert_eq!(json::call(r#"{"rules":[{"profile":"1","display":{"id":"builtin"}}]}"#), r#"{"profile":null}"#);
        assert!(json::call("not json").contains("\"error\":\"invalid\""));
    }
}
