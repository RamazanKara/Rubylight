//! The background mode. Nucleus Co-op runs every background stream: there is
//! no other mode and nothing to choose. The values earlier builds wrote to
//! `background_mode` (`auto`, `child_session`, `virtual_display`) are retired
//! settings ([`retired_background`], in `config::RETIRED_SETTINGS`): the host
//! saves `nucleus` in their place and says so once. A value that is no mode
//! at all is refused, never read as another mode.
use serde::Serialize;

/// The setting earlier builds chose the background mode with.
pub const KEY: &str = "background_mode";
/// The values earlier builds wrote, each replaced by `nucleus`.
const RETIRED: [&str; 3] = ["auto", "child_session", "virtual_display"];

/// Whether `value` is a background mode an earlier build wrote (trimmed,
/// any case).
pub fn retired_background(value: &str) -> bool {
    RETIRED.contains(&value.trim().to_ascii_lowercase().as_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Nucleus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub mode: Mode,
    pub label: &'static str,
    pub reason: &'static str,
}
impl Decision {
    /// The only one.
    pub fn nucleus() -> Self {
        Self {
            mode: Mode::Nucleus,
            label: "Background streaming (Nucleus Co-op)",
            reason: "Nucleus Co-op is the background mode",
        }
    }
}

/// Check a `background_mode` value: only `nucleus` and none are modes.
pub fn parse(text: &str) -> Result<(), String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "" | "nucleus" => Ok(()),
        other => Err(format!(
            "{KEY} = {other} is not a mode: Nucleus Co-op is the only background mode (nucleus)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nucleus_is_a_mode_and_earlier_values_are_retired() {
        for text in ["", " nucleus ", "Nucleus"] {
            assert_eq!(parse(text), Ok(()), "{text}");
            assert!(!retired_background(text), "{text}");
        }
        for text in ["auto", " Child_Session ", "virtual_display"] {
            assert!(retired_background(text), "{text}");
        }
        let error = parse("rdp").unwrap_err();
        assert_eq!(
            error,
            "background_mode = rdp is not a mode: Nucleus Co-op is the only background mode (nucleus)"
        );
        assert!(parse("child").is_err());
        assert!(!retired_background("rdp"));
        let d = Decision::nucleus();
        assert_eq!(d.mode, Mode::Nucleus);
        assert_eq!(d.label, "Background streaming (Nucleus Co-op)");
        assert_eq!(d.reason, "Nucleus Co-op is the background mode");
    }
}
