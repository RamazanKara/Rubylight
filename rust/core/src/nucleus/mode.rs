//! The background mode. Nucleus Co-op runs every background stream: there is
//! no other mode and nothing to choose. `background_mode` stays readable:
//! the values earlier builds wrote (`auto`, `child_session`,
//! `virtual_display`) read as `nucleus`, which the host logs once
//! ([`legacy`]). A value that is no mode at all runs none and says why, never
//! another mode.
use serde::Serialize;

/// The setting earlier builds chose the background mode with.
pub const KEY: &str = "background_mode";
/// The values earlier builds wrote, each read as `nucleus`.
const LEGACY: [&str; 3] = ["auto", "child_session", "virtual_display"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Nucleus,
    /// `background_mode` holds no mode: background streams are refused with
    /// the reason.
    Unavailable,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nucleus => "Background streaming (Nucleus Co-op)",
            Self::Unavailable => "Background streaming: not available",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub mode: Mode,
    pub label: &'static str,
    pub reason: String,
}
impl Decision {
    fn new(mode: Mode, reason: impl Into<String>) -> Self {
        Self {
            mode,
            label: mode.label(),
            reason: reason.into(),
        }
    }
}

/// Check a `background_mode` value; true for one an earlier build wrote.
pub fn parse(text: &str) -> Result<bool, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "" | "nucleus" => Ok(false),
        old if LEGACY.contains(&old) => Ok(true),
        other => Err(format!(
            "{KEY} = {other} is not a mode: Nucleus Co-op is the only background mode, so remove the line or set it to nucleus"
        )),
    }
}

/// The line the host logs once for a value an earlier build wrote, or None.
pub fn legacy(text: &str) -> Option<String> {
    matches!(parse(text), Ok(true)).then(|| {
        format!(
            "{KEY} = {} is from an earlier build and reads as nucleus: Nucleus Co-op is the only background mode",
            text.trim()
        )
    })
}

pub fn decide(text: &str) -> Decision {
    match parse(text) {
        Ok(false) => Decision::new(Mode::Nucleus, "Nucleus Co-op runs background streams"),
        Ok(true) => Decision::new(
            Mode::Nucleus,
            format!(
                "{KEY} = {} from an earlier build reads as nucleus",
                text.trim()
            ),
        ),
        Err(reason) => Decision::new(Mode::Unavailable, reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_an_earlier_build_wrote_runs_nucleus_and_says_so() {
        for text in ["", " nucleus ", "Nucleus"] {
            let d = decide(text);
            assert_eq!(d.mode, Mode::Nucleus, "{text}");
            assert_eq!(d.label, "Background streaming (Nucleus Co-op)");
            assert_eq!(legacy(text), None, "{text}");
        }
        // `auto` used to pick a child session on Pro and higher: never now.
        for text in ["auto", " Child_Session ", "virtual_display"] {
            let d = decide(text);
            assert_eq!(d.mode, Mode::Nucleus, "{text}");
            assert!(d.reason.contains("reads as nucleus"), "{}", d.reason);
            let line = legacy(text).unwrap();
            assert!(line.contains(text.trim()), "{line}");
            assert!(line.contains("only background mode"), "{line}");
        }
    }

    #[test]
    fn a_value_that_is_no_mode_runs_none_and_says_why() {
        let d = decide("rdp");
        assert_eq!(d.mode, Mode::Unavailable);
        assert_eq!(d.label, "Background streaming: not available");
        assert!(d.reason.contains("background_mode = rdp"), "{}", d.reason);
        assert!(d.reason.contains("set it to nucleus"), "{}", d.reason);
        assert!(parse("child").is_err());
        assert_eq!(legacy("rdp"), None);
    }
}
