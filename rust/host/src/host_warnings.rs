//! The host's own warnings in the console (`/api/metadata`), each until the user
//! dismisses it there. One-time notices, such as the settings an upgrade replaced,
//! are saved, so a host restarted before anyone opened the console still shows
//! them; a dismissed warning stays hidden until its text changes.
use anyhow::{Context, Result};
use butterpollo_core::session::{Warning, Warnings};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

const FILE: &str = "host-warnings.json";
/// Raised by the Windows Firewall check at start.
pub const FIREWALL: &str = "firewall_blocked";

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    /// One-time notices by code, shown again at every start until dismissed.
    #[serde(default)]
    notices: BTreeMap<String, String>,
    /// The text of each dismissed warning by code.
    #[serde(default)]
    dismissed: BTreeMap<String, String>,
}

/// Held across each load, change and save of the file, so concurrent dismisses
/// and notices do not lose one another's change.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// The saved notices and dismissals. A file that is not valid JSON counts as
/// empty, so it is replaced by the next save instead of blocking every dismiss.
fn load(directory: &Path) -> Result<Saved> {
    let path = directory.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "the host's saved warnings are damaged; starting from none");
            Saved::default()
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Saved::default()),
        Err(error) => Err(error).with_context(|| format!("{}", path.display())),
    }
}

/// Raises the saved one-time notices again.
pub fn restore(directory: &Path, warnings: &Warnings) {
    match load(directory) {
        Ok(saved) => {
            for (code, message) in &saved.notices {
                warnings.set(code, message);
            }
        }
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "the host's saved warnings could not be read")
        }
    }
}

/// Raises a one-time notice and saves it until the console dismisses it. A notice
/// that cannot be saved still shows until the host stops.
pub fn notice(directory: &Path, warnings: &Warnings, code: &str, message: &str) {
    warnings.set(code, message);
    let _file = FILE_LOCK.lock().unwrap();
    let saved = load(directory).and_then(|mut saved| {
        saved.notices.insert(code.into(), message.into());
        saved.dismissed.remove(code);
        butterpollo_core::state::write_json(&directory.join(FILE), &saved)
    });
    if let Err(error) = saved {
        tracing::warn!(code, error = %format!("{error:#}"), "the host warning could not be saved; it shows until the host stops");
    }
}

/// The current warnings without those the console dismissed at this text.
pub fn visible(directory: &Path, warnings: &Warnings) -> Vec<Warning> {
    let dismissed = load(directory)
        .map(|saved| saved.dismissed)
        .unwrap_or_default();
    warnings
        .snapshot()
        .into_iter()
        .filter(|warning| dismissed.get(&warning.code) != Some(&warning.message))
        .collect()
}

/// Hides the current warning `code`, and forgets it if it was a one-time notice.
pub fn dismiss(directory: &Path, warnings: &Warnings, code: &str) -> Result<()> {
    let Some(current) = warnings.snapshot().into_iter().find(|w| w.code == code) else {
        anyhow::bail!("there is no host warning {code:?} to dismiss");
    };
    let _file = FILE_LOCK.lock().unwrap();
    let mut saved = load(directory)?;
    if saved.notices.remove(code).is_some() {
        warnings.clear(code);
    } else {
        saved.dismissed.insert(current.code, current.message);
    }
    butterpollo_core::state::write_json(&directory.join(FILE), &saved)
}

/// The warning for the Windows Firewall problems the host found, if any.
pub fn firewall(problems: &[String]) -> Option<String> {
    let found = problems.join("; ");
    let mut chars = found.chars();
    let first = chars.next()?;
    Some(format!(
        "{}{}. Devices on the network may not find or reach this PC. In Windows Defender Firewall, open Advanced settings, Inbound Rules, and allow butterpollo.exe or delete the rule that blocks it.",
        first.to_uppercase(),
        chars.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(std::path::PathBuf);
    impl Temp {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> Result<Temp> {
        let path =
            std::env::temp_dir().join(format!("rubylight-warnings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Temp(path))
    }

    #[test]
    fn a_notice_shows_after_a_restart_until_it_is_dismissed() -> Result<()> {
        let directory = tempdir()?;
        let warnings = Warnings::default();
        notice(
            directory.path(),
            &warnings,
            "settings_retired",
            "Replaced old settings",
        );
        // A restart: a new host with no warnings of its own.
        let restarted = Warnings::default();
        restore(directory.path(), &restarted);
        let shown = visible(directory.path(), &restarted);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].message, "Replaced old settings");
        dismiss(directory.path(), &restarted, "settings_retired")?;
        assert!(visible(directory.path(), &restarted).is_empty());
        let again = Warnings::default();
        restore(directory.path(), &again);
        assert!(visible(directory.path(), &again).is_empty());
        Ok(())
    }

    #[test]
    fn a_dismissed_condition_stays_hidden_until_its_text_changes() -> Result<()> {
        let directory = tempdir()?;
        let warnings = Warnings::default();
        warnings.set("codec_support", "AV1 is missing");
        warnings.set("other", "Something else");
        dismiss(directory.path(), &warnings, "codec_support")?;
        let codes = |w: &Warnings| {
            visible(directory.path(), w)
                .into_iter()
                .map(|w| w.code)
                .collect::<Vec<_>>()
        };
        assert_eq!(codes(&warnings), ["other"]);
        // The condition is checked again (the codec probe sets it each pass).
        warnings.set("codec_support", "AV1 is missing");
        assert_eq!(codes(&warnings), ["other"]);
        warnings.set("codec_support", "AV1 and HEVC are missing");
        assert_eq!(codes(&warnings), ["codec_support", "other"]);
        Ok(())
    }

    #[test]
    fn dismisses_at_the_same_time_all_stick() -> Result<()> {
        let directory = tempdir()?;
        let warnings = Warnings::default();
        let codes: Vec<String> = (0..16).map(|i| format!("code_{i}")).collect();
        for code in &codes {
            warnings.set(code, "shown");
        }
        std::thread::scope(|scope| {
            for code in &codes {
                let (directory, warnings) = (directory.path(), &warnings);
                scope.spawn(move || dismiss(directory, warnings, code).unwrap());
            }
        });
        assert!(visible(directory.path(), &warnings).is_empty());
        Ok(())
    }

    #[test]
    fn a_damaged_file_is_replaced_instead_of_blocking_dismiss() -> Result<()> {
        let directory = tempdir()?;
        std::fs::write(directory.path().join(FILE), b"{\"notices\": [trunc")?;
        let warnings = Warnings::default();
        warnings.set("codec_support", "AV1 is missing");
        assert_eq!(visible(directory.path(), &warnings).len(), 1);
        dismiss(directory.path(), &warnings, "codec_support")?;
        assert!(visible(directory.path(), &warnings).is_empty());
        Ok(())
    }

    #[test]
    fn dismissing_a_warning_that_is_not_shown_fails() {
        let directory = tempdir().unwrap();
        let error =
            dismiss(directory.path(), &Warnings::default(), "settings_retired").unwrap_err();
        assert!(error.to_string().contains("settings_retired"), "{error}");
    }

    #[test]
    fn a_firewall_block_rule_becomes_a_warning_with_the_fix() {
        assert_eq!(firewall(&[]), None);
        let warning = firewall(&[
            "Windows Firewall rule \"butterpollo.exe\" blocks incoming connections to the host"
                .into(),
        ])
        .unwrap();
        assert!(warning.starts_with(
            "Windows Firewall rule \"butterpollo.exe\" blocks incoming connections to the host."
        ));
        assert!(warning.contains("Inbound Rules"));
        let none = firewall(&[
            "no Windows Firewall rule allows incoming connections to the host on this network"
                .into(),
        ])
        .unwrap();
        assert!(
            none.starts_with("No Windows Firewall rule allows"),
            "{none}"
        );
    }
}
