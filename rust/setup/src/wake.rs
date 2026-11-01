//! The optional Wake-on-LAN step. Its choice persists across setup and updates
//! like the USB/IP driver's; when chosen, the installed host turns on the wake
//! settings of the wired adapters (`butterpollo.exe --apply-wake-settings`).
//! An update neither asks again nor applies again unless `--wake-on-lan` is
//! passed (`docs/wake-and-audio-design.md`, A.3). Fast Startup is left alone:
//! only the console offers to turn it off.
use crate::net_adapter::{self, MEDIUM_WIFI, PhysicalAdapter};
use anyhow::{Context, Result, bail};
use std::{path::Path, time::Duration};

const CHOICE: &str = "wake-on-lan.json";
/// Reading the adapters and restarting the ones whose settings changed. Later
/// than the host's own bound (`SETTINGS_TIMEOUT` in butterpollo-windows,
/// 260 s), which gives up on its NIC changes before this could kill it
/// mid-restart.
const APPLY_TIMEOUT: Duration = Duration::from_secs(300);
const ADAPTERS_TIMEOUT: Duration = Duration::from_secs(30);

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

/// After an update, with `explicit` from `--wake-on-lan`/`--no-wake-on-lan`:
/// without one nothing is applied or saved; with one it is saved, and `apply`
/// runs when it is on. A failure to save it is a note.
pub fn after_update(
    profile: &Path,
    explicit: Option<bool>,
    notes: &mut Vec<String>,
    apply: impl FnOnce(&mut Vec<String>),
) {
    let Some(enabled) = explicit else {
        return;
    };
    if enabled {
        apply(notes);
    }
    if let Err(error) = remember(profile, enabled) {
        notes.push(format!(
            "The Wake-on-LAN choice could not be saved: {error:#}"
        ));
    }
}

/// The adapters the host would change, for the installer's question, as
/// "Name (description)".
pub fn adapters() -> Result<Vec<String>> {
    let adapters =
        crate::wmi::on_com_thread("listing the network adapters", ADAPTERS_TIMEOUT, || {
            net_adapter::physical_adapters(&crate::wmi::Wmi::connect(net_adapter::NAMESPACE)?)
        })?;
    Ok(changed(&adapters)
        .into_iter()
        .map(|adapter| format!("{} ({})", adapter.name, adapter.description))
        .collect())
}

/// The physical adapters the host changes: the wired ones, or the Wi-Fi ones
/// on a PC without any.
fn changed(adapters: &[PhysicalAdapter]) -> Vec<&PhysicalAdapter> {
    let wired: Vec<_> = adapters.iter().filter(|a| a.wired()).collect();
    if !wired.is_empty() {
        return wired;
    }
    adapters
        .iter()
        .filter(|a| a.medium == MEDIUM_WIFI)
        .collect()
}

/// The installer's question: which adapters, and what changes on them.
pub fn question(adapters: &[String]) -> String {
    let which = if adapters.is_empty() {
        "the wired network adapters".to_string()
    } else {
        adapters.join(", ")
    };
    format!(
        "Setup can turn on Wake-on-LAN for {which}, so a client can wake this PC from sleep. \
On each, where they are off, it turns on \"Allow this device to wake the computer\", \
\"Wake on magic packet\" and \"Only allow a magic packet to wake the computer\". \
An adapter restarts when its settings change, so the network drops for a few seconds.\n\n\
Turning off Fast Startup, which can keep a shut-down PC from waking, is offered in the Rubylight console."
    )
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
    fn an_update_applies_and_saves_only_an_explicit_choice() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let profile = temp.path();
        remember(profile, true)?;
        let mut notes = Vec::new();
        let mut applied = 0;
        // The remembered choice is neither asked again nor applied again.
        after_update(profile, None, &mut notes, |_| applied += 1);
        assert_eq!(applied, 0);
        assert!(selected(profile, None));
        after_update(profile, Some(false), &mut notes, |_| applied += 1);
        assert_eq!(applied, 0);
        assert!(!selected(profile, None));
        after_update(profile, Some(true), &mut notes, |notes| {
            applied += 1;
            notes.push("Wake-on-LAN is set up".into());
        });
        assert_eq!(applied, 1);
        assert!(selected(profile, None));
        assert_eq!(notes, ["Wake-on-LAN is set up"]);
        // A choice that can't be saved is a note, not a failed update.
        let blocked = profile.join("blocked");
        std::fs::write(&blocked, b"a file where the profile folder should be")?;
        after_update(&blocked, Some(true), &mut notes, |_| applied += 1);
        assert_eq!(applied, 2);
        assert_eq!(notes.len(), 2);
        assert!(
            notes[1].starts_with("The Wake-on-LAN choice could not be saved: "),
            "{notes:?}"
        );
        Ok(())
    }

    #[test]
    fn the_question_names_the_adapters_and_every_change() {
        let text = question(&["Ethernet (Realtek PCIe 2.5GbE Family Controller)".into()]);
        for part in [
            "for Ethernet (Realtek PCIe 2.5GbE Family Controller),",
            "Allow this device to wake the computer",
            "Wake on magic packet",
            "Only allow a magic packet to wake the computer",
            "the network drops for a few seconds",
            "Fast Startup",
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
        let two = question(&["Ethernet (A)".into(), "Ethernet 2 (B)".into()]);
        assert!(two.contains("for Ethernet (A), Ethernet 2 (B),"), "{two}");
        assert!(question(&[]).contains("for the wired network adapters,"));
    }

    #[test]
    fn the_wired_adapters_change_or_the_wifi_ones_without_any() {
        let adapter = |name: &str, medium| PhysicalAdapter {
            name: name.into(),
            medium,
            ..Default::default()
        };
        let all = [
            adapter("WLAN", MEDIUM_WIFI),
            adapter("Ethernet", 14),
            adapter("Ethernet 2", 0),
            adapter("Mobile", 8),
        ];
        let names = |adapters: &[PhysicalAdapter]| -> Vec<String> {
            changed(adapters).iter().map(|a| a.name.clone()).collect()
        };
        assert_eq!(names(&all), ["Ethernet", "Ethernet 2"]);
        assert_eq!(names(&[all[0].clone(), all[3].clone()]), ["WLAN"]);
        assert!(names(&[all[3].clone()]).is_empty());
    }

    #[test]
    fn this_pcs_adapters_are_listed_for_the_question() -> Result<()> {
        // Read-only, and without elevation.
        for adapter in adapters()? {
            let (name, description) = adapter
                .strip_suffix(')')
                .and_then(|rest| rest.split_once(" ("))
                .unwrap_or_else(|| panic!("{adapter}"));
            assert!(!name.is_empty() && !description.is_empty(), "{adapter}");
        }
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
