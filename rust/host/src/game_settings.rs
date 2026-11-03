//! The host's side of game settings files per device (issues #72 and #69; the rules are in
//! `butterpollo_core::game_settings`): when a swap begins and ends, its retries and the console.
//!
//! A stream's launch swaps the files in just before the app's process starts; a launch the
//! files cannot be swapped for fails with the reason, so the game never starts with the wrong
//! device's settings. The swap ends when the app stops ([`Swap`] goes with the
//! `RunningApp`, after its processes are closed): End session, Close game, the game exiting,
//! another app replacing it. A Disconnect keeps the app, so it keeps the swap. A launch from the
//! console has no device, so it swaps nothing.
//!
//! Only one app runs at a time. A second device joining the running game plays with the
//! launching device's settings; a device starting another app ends the first app, and with it
//! its swap, before its own begins.
//!
//! What a crash or a failure left is finished by a thread of its own: at host start, then every
//! 30 seconds while something is left, as the Windows user once someone is signed in. While
//! anything is left, the console shows a warning with the reason.
use crate::state::{Host, Shared};
use butterpollo_core::{
    game_files,
    game_settings::{self, Report, Store},
    state::App,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

/// The console warning while a swap is left unfinished.
const WARNING: &str = "game_settings";
/// The console warning for a launch the files could not be swapped for.
const LAUNCH_WARNING: &str = "game_settings_launch";
/// How often what is left is tried again.
const RETRY: Duration = Duration::from_secs(30);

/// How the files are reached: as the signed-in Windows user; the tests reach theirs as the
/// test process.
const ACCESS: game_files::Access = if cfg!(test) {
    game_files::as_host
} else {
    as_user
};

#[cfg_attr(test, allow(dead_code))]
fn as_user(work: &mut dyn FnMut()) -> std::io::Result<()> {
    butterpollo_windows::user_folder::as_user(work)
        .map_err(|error| std::io::Error::other(error.to_string()))
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The swaps of apps running in this run: the retries leave them alone.
static LIVE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Wakes the retry thread early, after a swap failed to finish.
static NUDGE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn nudge() {
    *locked(&NUDGE.0) = true;
    NUDGE.1.notify_all();
}

fn store(directory: &Path) -> Store {
    Store::new(directory, ACCESS)
}

/// A running app's swap: finished when it drops.
pub struct Swap {
    id: String,
    app: String,
    directory: PathBuf,
    host: Weak<Host>,
}

impl Drop for Swap {
    fn drop(&mut self) {
        let result = store(&self.directory).finish(&self.id);
        locked(&LIVE).remove(&self.id);
        match result {
            Ok(report) => log_finished(&self.app, &report),
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(app = %self.app, %error, "the game settings files could not all be saved and put back; they stay as they are and the host tries again");
                if let Some(h) = self.host.upgrade() {
                    raise(&h, &error);
                }
                nudge();
            }
        }
    }
}

fn log_finished(app: &str, report: &Report) {
    for warning in &report.warnings {
        tracing::error!(%app, "{warning}");
    }
    tracing::info!(%app, saved = ?report.saved, restored = ?report.restored, "game settings files saved for the device and the PC's own put back");
}

fn raise(h: &Host, error: &str) {
    h.warnings.set(
        WARNING,
        format!("Game settings files are not back yet: {error}. The host tries again every 30 seconds; until then the files may hold a device's settings."),
    );
}

/// The stream's app (expanded) is about to start for the device in `args` (`clientUuid`,
/// `clientName`): swap its game settings files in. `environment` is the Windows user's, for
/// `%VAR%`. None when the app lists no files or the launch has no device.
pub fn launching(
    h: &Shared,
    app: &App,
    args: &HashMap<String, String>,
    environment: &BTreeMap<String, String>,
) -> anyhow::Result<Option<Swap>> {
    let written = game_settings::paths(&app.extra)?;
    if written.is_empty() {
        return Ok(None);
    }
    let Some(device) = args.get("clientUuid").filter(|device| !device.is_empty()) else {
        tracing::info!(app = %app.name, "game settings files are swapped per device; this launch has no device, so the files stay as they are");
        return Ok(None);
    };
    let device_name = args
        .get("clientName")
        .filter(|name| !name.is_empty())
        .unwrap_or(device);
    let result = (|| {
        let files = written
            .iter()
            .map(|path| {
                let expanded = game_settings::expand(path, |name| {
                    environment
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.clone())
                })?;
                Ok((path.clone(), expanded))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let live = locked(&LIVE).clone();
        store(&h.directory).begin(
            &game_settings::app_key(app),
            &app.name,
            device,
            device_name,
            &files,
            &live,
        )
    })();
    match result {
        Ok(begun) => {
            locked(&LIVE).insert(begun.id.clone());
            h.warnings.clear(LAUNCH_WARNING);
            tracing::info!(app = %app.name, device = %device_name, placed = ?begun.report.placed, first = ?begun.report.first, "game settings files swapped in for the device (first: kept as they are, saved for it when the game ends)");
            Ok(Some(Swap {
                id: begun.id,
                app: app.name.clone(),
                directory: h.directory.clone(),
                host: std::sync::Arc::downgrade(h),
            }))
        }
        Err(error) => {
            let error = error.context(format!(
                "{} did not start: its game settings files could not be swapped for {device_name}",
                app.name
            ));
            tracing::error!(error = format!("{error:#}"), "game settings files");
            h.warnings.set(LAUNCH_WARNING, format!("{error:#}."));
            Err(error)
        }
    }
}

/// Whether the host runs as the service at the sign-in screen, with no user to act as.
fn nobody_signed_in() -> bool {
    use butterpollo_windows::process;
    process::is_system() && !process::user_signed_in()
}

/// Finish what earlier runs or failed finishes left, and show what is still left. True while
/// something is.
fn retry(h: &Host) -> bool {
    let store = store(&h.directory);
    let live = locked(&LIVE).clone();
    let results = match store.recover(&live) {
        Ok(results) => results,
        Err(error) => {
            let error = format!("{error:#}");
            tracing::error!(%error, "the game settings journal could not be read; nothing is swapped until it can be");
            raise(h, &error);
            return true;
        }
    };
    let mut left = Vec::new();
    for (swap, result) in results {
        match result {
            Ok(report) => log_finished(&swap.app_name, &report),
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(%error, "game settings files left by an earlier session could not be put back yet");
                left.push(error);
            }
        }
    }
    if left.is_empty() {
        h.warnings.clear(WARNING);
        false
    } else {
        raise(h, &left.join("; "));
        true
    }
}

/// Start the retry thread; its first pass finishes what a crashed run left. At host start,
/// before any stream.
pub fn start(h: &Shared) {
    let host = std::sync::Arc::downgrade(h);
    if let Err(error) = std::thread::Builder::new()
        .name("game-settings".into())
        .spawn(move || retrier(&host))
    {
        tracing::error!(%error, "no thread for game settings files; what a crash left is put back at the next launch of the game");
    }
}

fn retrier(host: &Weak<Host>) {
    loop {
        let Some(h) = host.upgrade() else {
            return;
        };
        let journal = h
            .directory
            .join(game_settings::FOLDER)
            .join(game_settings::JOURNAL_FILE);
        if journal.exists() && !nobody_signed_in() {
            retry(&h);
        }
        drop(h);
        let nudged = locked(&NUDGE.0);
        let (mut nudged, _) = NUDGE
            .1
            .wait_timeout_while(nudged, RETRY, |nudged| !*nudged)
            .unwrap_or_else(PoisonError::into_inner);
        *nudged = false;
    }
}

/// The console's view of an app's game settings files (`GET /api/game-settings?uuid=`): each
/// paired device with the files it has a saved copy of, and the swaps still in place.
pub fn status(h: &Host, uuid: &str) -> anyhow::Result<Value> {
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no app {uuid}"))?;
    let written = game_settings::paths(&app.extra)?;
    let key = game_settings::app_key(&app);
    let store = store(&h.directory);
    let devices: Vec<Value> = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .map(|client| {
            let saved: Vec<&String> = written
                .iter()
                .filter(|path| store.saved(&key, &client.uuid, path))
                .collect();
            json!({
                "uuid": client.uuid,
                "name": client.name,
                "saved": saved,
            })
        })
        .collect();
    let live = locked(&LIVE).clone();
    let pending: Vec<Value> = store
        .journal()?
        .swaps
        .into_iter()
        .filter(|swap| swap.app == key)
        .map(|swap| {
            json!({
                "device": swap.device,
                "deviceName": swap.device_name,
                "running": live.contains(&swap.id),
                "error": swap.error,
            })
        })
        .collect();
    Ok(json!({
        "files": written,
        "devices": devices,
        "pending": pending,
        "folder": h.directory.join(game_settings::FOLDER),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    /// The app, in the host's library, and its settings file in a folder of its own.
    fn game(f: &Fixture) -> (App, PathBuf, BTreeMap<String, String>) {
        let folder = f.host.directory.join("game");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("Settings.ini");
        std::fs::write(&file, "pc").unwrap();
        let app: App = serde_json::from_value(json!({
            "name": "Game",
            "uuid": "game-1",
            "game-settings-files": ["%GameDir%\\Settings.ini"],
        }))
        .unwrap();
        f.host.apps.write().unwrap().push(app.clone());
        // Windows names are compared without regard to case.
        let environment =
            BTreeMap::from([("GAMEDIR".to_owned(), folder.to_string_lossy().into_owned())]);
        (app, file, environment)
    }

    fn args(device: &str) -> HashMap<String, String> {
        HashMap::from([
            ("clientUuid".to_owned(), device.to_owned()),
            ("clientName".to_owned(), format!("{device} name")),
        ])
    }

    fn read(file: &Path) -> String {
        std::fs::read_to_string(file).unwrap()
    }

    #[test]
    fn a_launch_swaps_the_devices_copy_in_and_the_app_stopping_puts_the_pcs_back() {
        let f = Fixture::new();
        let (app, file, environment) = game(&f);
        let swap = launching(&f.host, &app, &args("tv"), &environment)
            .unwrap()
            .expect("the app lists a file");
        std::fs::write(&file, "tv").unwrap();
        let shown = status(&f.host, "game-1").unwrap();
        assert_eq!(shown["pending"][0]["running"], true);
        assert_eq!(shown["pending"][0]["deviceName"], "tv name");
        drop(swap);
        assert_eq!(read(&file), "pc");
        let swap = launching(&f.host, &app, &args("tv"), &environment)
            .unwrap()
            .unwrap();
        assert_eq!(read(&file), "tv", "the TV's copy is in place");
        drop(swap);
        assert_eq!(read(&file), "pc");
        assert!(
            status(&f.host, "game-1").unwrap()["pending"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_launch_without_a_device_or_files_swaps_nothing_and_a_missing_file_fails_it() {
        let f = Fixture::new();
        let (app, file, environment) = game(&f);
        let plain: App = serde_json::from_value(json!({"name": "Plain"})).unwrap();
        assert!(
            launching(&f.host, &plain, &args("tv"), &environment)
                .unwrap()
                .is_none()
        );
        assert!(
            launching(&f.host, &app, &HashMap::new(), &environment)
                .unwrap()
                .is_none(),
            "a console launch has no device"
        );
        std::fs::remove_file(&file).unwrap();
        let error = launching(&f.host, &app, &args("tv"), &environment)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
        assert!(format!("{error:#}").contains("Game did not start"), "{error:#}");
        let unset = launching(&f.host, &app, &args("tv"), &BTreeMap::new())
            .err()
            .unwrap();
        assert!(format!("{unset:#}").contains("not set"), "{unset:#}");
    }

    #[test]
    fn what_a_crash_left_is_put_back_by_the_retry() {
        let f = Fixture::new();
        let (app, file, environment) = game(&f);
        let swap = launching(&f.host, &app, &args("tv"), &environment)
            .unwrap()
            .unwrap();
        std::fs::write(&file, "tv").unwrap();
        // A crash: the swap never finishes, and the next run has none live.
        locked(&LIVE).remove(&swap.id);
        std::mem::forget(swap);
        assert!(!retry(&f.host));
        assert_eq!(read(&file), "pc");
        let key = game_settings::app_key(&app);
        assert!(store(&f.host.directory).saved(&key, "tv", "%GameDir%\\Settings.ini"));
    }
}
