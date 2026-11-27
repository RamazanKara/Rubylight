//! The host's own warnings in the console (`/api/metadata`), each until the user
//! dismisses it there. One-time notices, such as the settings an upgrade replaced,
//! are saved, so a host restarted before anyone opened the console still shows
//! them; a dismissed warning stays hidden until its text changes.
use anyhow::{Context, Result};
use butterpollo_session::session::{Warning, Warnings};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

const FILE: &str = "host-warnings.json";
/// Raised by the Windows Firewall check at start.
pub const FIREWALL: &str = "firewall_blocked";
/// The guest page's port could not be opened.
pub const GUEST_PORT: &str = "guest_port";
/// An app's cover is on a network or device path, which the host no longer opens.
pub const NETWORK_COVER: &str = "network_cover";
/// Graphics card on Automatic streams on a GPU Windows does not list first.
pub const AUTOMATIC_GPU: &str = "automatic_gpu";

#[derive(Clone, Default, Serialize, Deserialize)]
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

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

struct CachedSaved {
    path: std::path::PathBuf,
    stamp: Option<FileStamp>,
    saved: Saved,
}

static SAVED_CACHE: Mutex<Option<CachedSaved>> = Mutex::new(None);

fn file_stamp(path: &Path) -> Option<std::io::Result<Option<FileStamp>>> {
    match std::fs::metadata(path) {
        Ok(meta) => Some(Ok(Some(FileStamp {
            modified: meta.modified().ok(),
            len: meta.len(),
        }))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Ok(None)),
        Err(error) => Some(Err(error)),
    }
}

fn cache_store(path: &Path, saved: &Saved) {
    let stamp = file_stamp(path).and_then(Result::ok).flatten();
    *SAVED_CACHE.lock().unwrap() = Some(CachedSaved {
        path: path.to_path_buf(),
        stamp,
        saved: saved.clone(),
    });
}

/// Cached view of `warnings.json` for read-only metadata polling, re-reading only when the file
/// path, size, or modification timestamp changes.
fn load_cached(directory: &Path) -> Result<Saved> {
    let path = directory.join(FILE);
    let read_stamp = || match file_stamp(&path) {
        Some(Ok(stamp)) => Ok(stamp),
        Some(Err(error)) => Err(error).with_context(|| format!("{}", path.display())),
        None => Ok(None),
    };
    let stamp = read_stamp()?;
    if let Some(cached) = SAVED_CACHE.lock().unwrap().as_ref()
        && cached.path == path
        && cached.stamp == stamp
    {
        return Ok(cached.saved.clone());
    }
    let _file = FILE_LOCK.lock().unwrap();
    let stamp = read_stamp()?;
    if let Some(cached) = SAVED_CACHE.lock().unwrap().as_ref()
        && cached.path == path
        && cached.stamp == stamp
    {
        return Ok(cached.saved.clone());
    }
    let saved = load(directory)?;
    cache_store(&path, &saved);
    Ok(saved)
}

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

fn write_saved(directory: &Path, saved: &Saved) -> Result<()> {
    let path = directory.join(FILE);
    butterpollo_session::state::write_json(&path, saved)?;
    cache_store(&path, saved);
    Ok(())
}

/// Raises a one-time notice and saves it until the console dismisses it. A notice
/// that cannot be saved still shows until the host stops.
pub fn notice(directory: &Path, warnings: &Warnings, code: &str, message: &str) {
    warnings.set(code, message);
    let _file = FILE_LOCK.lock().unwrap();
    let saved = load(directory).and_then(|mut saved| {
        saved.notices.insert(code.into(), message.into());
        saved.dismissed.remove(code);
        write_saved(directory, &saved)
    });
    if let Err(error) = saved {
        tracing::warn!(code, error = %format!("{error:#}"), "the host warning could not be saved; it shows until the host stops");
    }
}

/// Says which apps' covers are on a network or device path, which the host does not open
/// ([`butterpollo_session::network_path`]): they show the default cover until the image is copied
/// to this PC. Cleared when no app has one.
pub fn network_covers(warnings: &Warnings, apps: &[butterpollo_session::state::App]) {
    let names = butterpollo_session::catalog::network_covers(apps);
    if names.is_empty() {
        warnings.clear(NETWORK_COVER);
        return;
    }
    let shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let more = match names.len() {
        0..=3 => String::new(),
        n => format!(" and {} more", n - 3),
    };
    let message = format!(
        r"The cover of {shown}{more} is on a network path (\\server\share) or a device path, which the host does not open: it would sign in to that computer with this PC's account. The app shows the default cover; copy the image to this PC and choose it again in the app's settings."
    );
    tracing::warn!(apps = names.len(), "{message}");
    warnings.set(NETWORK_COVER, message);
}

/// Says which GPU Graphics card on Automatic streams on while Windows lists
/// another first ([`butterpollo_win_graphics::capture::automatic_switch`]),
/// where streams rendered before; gone as soon as `config` chooses a GPU. A
/// plain warning: dismissed, it stays hidden until it names another GPU.
pub fn automatic_gpu(warnings: &Warnings, config: &butterpollo_config::config::Config) {
    show_automatic_gpu(
        warnings,
        config,
        butterpollo_win_graphics::capture::automatic_switch(),
    );
}

fn show_automatic_gpu(
    warnings: &Warnings,
    config: &butterpollo_config::config::Config,
    switched: Option<String>,
) {
    let chosen =
        !config.get("adapter_name", "").is_empty() || !config.get("adapter_pnp_id", "").is_empty();
    match switched.filter(|_| !chosen) {
        Some(gpu) => warnings.set(
            AUTOMATIC_GPU,
            format!(
                "Graphics card is on Automatic, so Rubylight streams from {gpu} rather than the GPU Windows lists first, unless the streamed display is connected to another AMD or NVIDIA GPU. To stream on another GPU, choose it under Graphics card in Settings."
            ),
        ),
        None => warnings.clear(AUTOMATIC_GPU),
    }
}

/// The current warnings without those the console dismissed at this text.
pub fn visible(directory: &Path, warnings: &Warnings) -> Vec<Warning> {
    let dismissed = load_cached(directory)
        .map(|saved| saved.dismissed)
        .unwrap_or_default();
    let mut entries = warnings.snapshot();
    if let Some(message) = crate::dsu::runtime_warning()
        && !entries.iter().any(|w| w.code == crate::dsu::WARNING)
    {
        entries.push(Warning {
            code: crate::dsu::WARNING.into(),
            message,
        });
    }
    entries
        .into_iter()
        .filter(|warning| dismissed.get(&warning.code) != Some(&warning.message))
        .collect()
}

/// Hides the current warning `code`, and forgets it if it was a one-time notice.
pub fn dismiss(directory: &Path, warnings: &Warnings, code: &str) -> Result<()> {
    let current = warnings
        .snapshot()
        .into_iter()
        .find(|w| w.code == code)
        .or_else(|| {
            (code == crate::dsu::WARNING)
                .then(crate::dsu::runtime_warning)
                .flatten()
                .map(|message| Warning {
                    code: crate::dsu::WARNING.into(),
                    message,
                })
        });
    let Some(current) = current else {
        anyhow::bail!("there is no host warning {code:?} to dismiss");
    };
    let _file = FILE_LOCK.lock().unwrap();
    let mut saved = load(directory)?;
    if saved.notices.remove(code).is_some() {
        warnings.clear(code);
    } else {
        saved.dismissed.insert(current.code, current.message);
    }
    write_saved(directory, &saved)
}

/// Takes back the notice `code` once what it reported is over: it is cleared, and neither
/// comes back at the next start nor stays remembered as dismissed.
pub fn withdraw(directory: &Path, warnings: &Warnings, code: &str) {
    warnings.clear(code);
    let _file = FILE_LOCK.lock().unwrap();
    let saved = load(directory).and_then(|mut saved| {
        let notice = saved.notices.remove(code).is_some();
        if !notice && saved.dismissed.remove(code).is_none() {
            return Ok(());
        }
        write_saved(directory, &saved)
    });
    if let Err(error) = saved {
        tracing::warn!(code, error = %format!("{error:#}"), "the host warning could not be taken back; it may show again at the next start");
    }
}

/// The warning for the Windows Firewall problems the host found, if any.
pub fn firewall(problems: &[String]) -> Option<String> {
    // Rules that share a name read the same: say each once.
    let mut unique: Vec<&str> = Vec::new();
    for problem in problems {
        if !unique.contains(&problem.as_str()) {
            unique.push(problem);
        }
    }
    let found = unique.join("; ");
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
    fn a_cover_on_a_network_path_is_a_warning_until_no_app_has_one() {
        let warnings = Warnings::default();
        let mut app = butterpollo_session::state::App::desktop();
        app.name = "Shared Game".into();
        app.extra.insert(
            "image-path".into(),
            serde_json::json!(r"\\nas\covers\game.png"),
        );
        network_covers(&warnings, std::slice::from_ref(&app));
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, NETWORK_COVER);
        assert!(shown[0].message.contains("Shared Game"));
        assert!(shown[0].message.contains(r"\\server\share"));
        app.extra.insert(
            "image-path".into(),
            serde_json::json!(r"Z:\covers\game.png"),
        );
        network_covers(&warnings, &[app]);
        assert!(warnings.snapshot().is_empty());
    }

    #[test]
    fn the_gpu_automatic_picks_over_the_first_listed_shows_until_dismissed() -> Result<()> {
        let directory = tempdir()?;
        let warnings = Warnings::default();
        let automatic = butterpollo_config::config::Config::parse("")?;
        let show = |gpu: &str| show_automatic_gpu(&warnings, &automatic, Some(gpu.into()));
        show("AMD Radeon RX 7900 XT");
        let shown = visible(directory.path(), &warnings);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, AUTOMATIC_GPU);
        assert!(shown[0].message.contains("AMD Radeon RX 7900 XT"));
        assert!(shown[0].message.contains("under Graphics card"));
        // Every later stream on that GPU: still the one warning, and once
        // dismissed it stays hidden.
        show("AMD Radeon RX 7900 XT");
        assert_eq!(warnings.snapshot().len(), 1);
        dismiss(directory.path(), &warnings, AUTOMATIC_GPU)?;
        show("AMD Radeon RX 7900 XT");
        assert!(visible(directory.path(), &warnings).is_empty());
        // Another GPU is news.
        show("AMD Radeon RX 9070 XT");
        assert_eq!(visible(directory.path(), &warnings).len(), 1);
        // A GPU chosen under Graphics card ends it at once, before any stream.
        let chosen =
            butterpollo_config::config::Config::parse("adapter_name = AMD Radeon RX 9070 XT\n")?;
        show_automatic_gpu(&warnings, &chosen, Some("AMD Radeon RX 9070 XT".into()));
        assert!(warnings.snapshot().is_empty());
        show("AMD Radeon RX 9070 XT");
        show_automatic_gpu(&warnings, &automatic, None);
        assert!(warnings.snapshot().is_empty());
        Ok(())
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
    fn rules_with_the_same_name_are_named_once() {
        let rule =
            "Windows Firewall rule \"butterpollo.exe\" blocks incoming connections to the host"
                .to_string();
        let other = "Windows Firewall rule \"Rubylight\" blocks incoming connections to the host"
            .to_string();
        let warning = firewall(&[rule.clone(), rule.clone(), other.clone(), rule]).unwrap();
        assert_eq!(
            warning.matches("\"butterpollo.exe\" blocks").count(),
            1,
            "{warning}"
        );
        assert_eq!(
            warning.matches("\"Rubylight\" blocks").count(),
            1,
            "{warning}"
        );
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

    #[test]
    fn visible_uses_cached_dismissals_until_file_changes() -> Result<()> {
        let directory = tempdir()?;
        let warnings = Warnings::default();
        warnings.set("codec_support", "AV1 missing");
        dismiss(directory.path(), &warnings, "codec_support")?;
        assert!(visible(directory.path(), &warnings).is_empty());

        // Rewrite `warnings.json` directly on disk to prove visible() refreshes when the stamp changes.
        std::fs::write(directory.path().join(FILE), b"{}")?;
        assert_eq!(visible(directory.path(), &warnings).len(), 1);
        Ok(())
    }
}
