//! Which way a background stream runs: in a Windows child session where
//! Windows offers one and the setting allows, else through Nucleus Co-op.
//! The decision always carries its reason, which the log and the console
//! show; a mode that was chosen but cannot run is an error, never another
//! mode.
use crate::config::BackgroundMode;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    ChildSession,
    Nucleus,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ChildSession => "Background streaming (child session)",
            Self::Nucleus => "Background streaming (Nucleus Co-op)",
        }
    }
}

/// Whether this PC can run a child session, and if not, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildSupport {
    Available,
    Unavailable(String),
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

pub fn decide(setting: BackgroundMode, child: &ChildSupport) -> Result<Decision, String> {
    match (setting, child) {
        (BackgroundMode::Auto, ChildSupport::Available) => Ok(Decision::new(
            Mode::ChildSession,
            "Automatic: Windows offers a child session here",
        )),
        (BackgroundMode::Auto, ChildSupport::Unavailable(why)) => Ok(Decision::new(
            Mode::Nucleus,
            format!("Automatic: no child session ({why})"),
        )),
        (BackgroundMode::ChildSession, ChildSupport::Available) => {
            Ok(Decision::new(Mode::ChildSession, "chosen in the settings"))
        }
        (BackgroundMode::ChildSession, ChildSupport::Unavailable(why)) => Err(format!(
            "Background streaming is set to a child session, which this PC cannot run ({why}): choose Automatic or Nucleus Co-op in the settings"
        )),
        (BackgroundMode::Nucleus, _) => Ok(Decision::new(Mode::Nucleus, "chosen in the settings")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> ChildSupport {
        ChildSupport::Unavailable("Windows Home has no child sessions".into())
    }

    #[test]
    fn automatic_takes_a_child_session_where_there_is_one() {
        let d = decide(BackgroundMode::Auto, &ChildSupport::Available).unwrap();
        assert_eq!(d.mode, Mode::ChildSession);
        assert_eq!(d.label, "Background streaming (child session)");
        let d = decide(BackgroundMode::Auto, &home()).unwrap();
        assert_eq!(d.mode, Mode::Nucleus);
        assert_eq!(d.label, "Background streaming (Nucleus Co-op)");
        assert_eq!(
            d.reason,
            "Automatic: no child session (Windows Home has no child sessions)"
        );
    }

    #[test]
    fn a_chosen_mode_runs_or_fails_but_never_becomes_the_other() {
        let error = decide(BackgroundMode::ChildSession, &home()).unwrap_err();
        assert!(
            error.contains("Windows Home has no child sessions"),
            "{error}"
        );
        assert!(
            error.contains("choose Automatic or Nucleus Co-op"),
            "{error}"
        );
        assert_eq!(
            decide(BackgroundMode::ChildSession, &ChildSupport::Available)
                .unwrap()
                .mode,
            Mode::ChildSession
        );
        for child in [ChildSupport::Available, home()] {
            let d = decide(BackgroundMode::Nucleus, &child).unwrap();
            assert_eq!(
                (d.mode, d.reason.as_str()),
                (Mode::Nucleus, "chosen in the settings")
            );
        }
    }
}
