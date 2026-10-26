//! The optional Wake-on-LAN step. Its choice persists across setup and updates
//! like the USB/IP driver's; when chosen, the installed host turns on the wake
//! settings of the wired adapters (`butterpollo.exe --apply-wake-settings`).
//! Fast Startup is left alone: only the console offers to turn it off.
use anyhow::{Context, Result, bail};
use std::{path::Path, time::Duration};

const CHOICE: &str = "wake-on-lan.json";
/// Reading the adapters and restarting the ones whose settings changed.
const APPLY_TIMEOUT: Duration = Duration::from_secs(180);

pub fn selected(profile: &Path, explicit: Option<bool>) -> bool {
    explicit.unwrap_or_else(|| {
        std::fs::read(profile.join(CHOICE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|choice| choice["enabled"] == true)
    })
}

pub fn remember(profile: &Path, enabled: bool) -> Result<()> {
    std::fs::create_dir_all(profile)?;
    std::fs::write(
        profile.join(CHOICE),
        if enabled {
            br#"{"enabled":true}"#.as_slice()
        } else {
            br#"{"enabled":false}"#.as_slice()
        },
    )
    .context("saving the Wake-on-LAN installer choice")
}

/// Apply the wake settings with the host in `install`; what it changed, in the
/// words the console shows.
pub fn apply(install: &Path) -> Result<Vec<String>> {
    let host = install.join("butterpollo.exe");
    let (code, output) = crate::system::run(
        &host.display().to_string(),
        &["--apply-wake-settings"],
        APPLY_TIMEOUT,
    )?;
    outcome(code, &output)
}

/// The host prints `{"changes":[..],"restarted":[..],"errors":[..]}` and exits
/// 1 when anything failed.
fn outcome(code: i32, output: &str) -> Result<Vec<String>> {
    let applied = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .filter(serde_json::Value::is_object);
    let Some(applied) = applied else {
        bail!("butterpollo.exe --apply-wake-settings exited with code {code}: {output}");
    };
    let strings = |key: &str| -> Vec<String> {
        applied[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let errors = strings("errors");
    if code != 0 || !errors.is_empty() {
        bail!(
            "{}",
            if errors.is_empty() {
                format!("butterpollo.exe --apply-wake-settings exited with code {code}")
            } else {
                errors.join("; ")
            }
        );
    }
    Ok(strings("changes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_on_lan_stays_optional_and_updates_keep_the_explicit_choice() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let profile = temp.path();
        assert!(!selected(profile, None));
        assert!(selected(profile, Some(true)));
        remember(profile, true)?;
        assert!(selected(profile, None));
        assert!(!selected(profile, Some(false)));
        remember(profile, false)?;
        assert!(!selected(profile, None));
        std::fs::write(profile.join(CHOICE), b"interrupted write")?;
        assert!(!selected(profile, None));
        Ok(())
    }

    #[test]
    fn the_hosts_report_is_the_changes_or_a_loud_error() {
        let done =
            r#"{"changes":["Ethernet: wake on magic packets only"],"restarted":[],"errors":[]}"#;
        assert_eq!(
            outcome(0, done).unwrap(),
            ["Ethernet: wake on magic packets only"]
        );
        assert!(
            outcome(0, r#"{"changes":[],"restarted":[],"errors":[]}"#)
                .unwrap()
                .is_empty()
        );
        let failed = r#"{"changes":["x"],"restarted":[],"errors":["Ethernet: access denied"]}"#;
        let error = outcome(1, failed).unwrap_err().to_string();
        assert_eq!(error, "Ethernet: access denied");
        // A failure without a report, or a report with a failing exit code, is not success.
        assert!(outcome(1, "thread main panicked").is_err());
        assert!(outcome(0, "").is_err());
        assert!(outcome(1, r#"{"changes":[],"restarted":[],"errors":[]}"#).is_err());
    }
}
