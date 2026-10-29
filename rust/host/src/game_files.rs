//! The host's side of managed game files (issue #63; the rules are in
//! `butterpollo_core::game_files`): which session wrote keys, and when they go back.
//!
//! A stream that starts its app writes the keys the app lists for its device (the device's
//! block in `device-overrides` on top) just before the app's process starts, with the stream's
//! frame rate. They go back the way the Radeon settings do (`radeon_session`): when the device
//! is gone from the session (End session, Close game, the grace period running out, a stock
//! `/cancel`), when it is unpaired or turned off, at a clean shutdown, and at the next start
//! after a crash. A Disconnect leaves a suspended session, which keeps them. A launch from the
//! console has no stream, so it writes nothing.
//!
//! Restores run where they are decided, under the session lock: a settings file is small.
use crate::state::Shared;
use butterpollo_core::{
    game_files::{self, JOURNAL_FILE, Report, Scope, Unusable},
    state::App,
};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// The launch argument the host sets for the rate its frame limiter holds the stream's game
/// to, when it limits; never the client's. See `process::BACKGROUND_ARG`.
pub const FRAME_LIMIT_ARG: &str = "rubylightFrameLimit";

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct GameFiles {
    /// Devices that may have keys written in this run. A device leaves when its restore runs.
    written: Mutex<BTreeSet<String>>,
    /// Held through every journal change, so a write and a restore never interleave.
    journal: Mutex<()>,
}

impl GameFiles {
    /// The session machine moved: `present` makes the devices still part of the session, or
    /// `None` while it has not settled (a launch that fails gives the session back). Every
    /// device that wrote keys and is not present gets them back. `present` is not called when
    /// no device wrote anything.
    pub fn sync(&self, directory: &Path, present: impl FnOnce() -> Option<BTreeSet<String>>) {
        let ended: Vec<String> = {
            let mut written = locked(&self.written);
            if written.is_empty() {
                return;
            }
            let Some(present) = present() else {
                return;
            };
            let ended: Vec<String> = written
                .iter()
                .filter(|device| !present.contains(*device))
                .cloned()
                .collect();
            for device in &ended {
                written.remove(device);
            }
            ended
        };
        for device in ended {
            self.restore(directory, Scope::Session(&device));
        }
    }
    /// A device was unpaired or turned off: its keys go back even though its session may
    /// linger until the grace period ends.
    pub fn release_device(&self, directory: &Path, device: &str) {
        if locked(&self.written).remove(device) {
            self.restore(directory, Scope::Session(device));
        }
    }
    /// Every key the host changed goes back, whatever the sessions do (unpair all, shutdown,
    /// the start after a crash).
    pub fn release_all(&self, directory: &Path) {
        locked(&self.written).clear();
        self.restore(directory, Scope::All);
    }

    fn restore(&self, directory: &Path, scope: Scope<'_>) {
        let _journal = locked(&self.journal);
        match game_files::restore(&directory.join(JOURNAL_FILE), scope) {
            Ok(report) => log_restore(&report),
            Err(error) => tracing::error!(
                error = format!("{error:#}"),
                "the game files journal could not be read; the keys in it stay changed until it can be"
            ),
        }
    }

    /// Write `targets` for `device` and note it for the restore.
    fn apply(
        &self,
        directory: &Path,
        device: &str,
        targets: &[game_files::Target],
    ) -> Option<Report> {
        let _journal = locked(&self.journal);
        // Noted first: a write that half succeeded is still the session's to put back.
        locked(&self.written).insert(device.to_owned());
        match game_files::apply(&directory.join(JOURNAL_FILE), device, targets) {
            Ok(report) => Some(report),
            Err(error) => {
                tracing::warn!(
                    device,
                    error = format!("{error:#}"),
                    "the game files journal could not be read, so no managed game file was changed; the game starts with the values it has"
                );
                None
            }
        }
    }
}

fn log_restore(report: &Report) {
    if !report.restored.is_empty() {
        tracing::info!(keys = ?report.restored, "managed game file keys put back");
    }
    if !report.kept.is_empty() {
        tracing::info!(keys = ?report.kept, "managed game file keys changed since the host wrote them; the new values stay");
    }
}

/// Put back what a run that ended without its restores left changed. At host start, before
/// any stream.
pub fn start(h: &Shared) {
    h.game_files.release_all(&h.directory);
}

/// Where `app`'s files are: the folder a relative path starts at (the working directory, else
/// the folder of the program), and every folder a file may be in, resolved: those two and the
/// Steam install folder. `$(...)` in the paths must be expanded already.
pub fn folders(app: &App) -> (Option<PathBuf>, Vec<PathBuf>) {
    let program = {
        let target = butterpollo_windows::process::command_target(&app.cmd);
        let path = Path::new(target);
        (!target.contains("://") && path.is_absolute())
            .then(|| path.parent().map(Path::to_path_buf))
            .flatten()
    };
    let working = Some(app.working_dir.trim())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| program.clone());
    let steam = app
        .extra
        .get("steam-install-dir")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let base = working.clone().or_else(|| steam.clone());
    let folders = game_files::folders(working.into_iter().chain(program).chain(steam));
    (base, folders)
}

/// Check the files `app` and its devices' blocks name before it is saved: beyond the entries'
/// own checks (`device_overrides::check`), a file that exists must be inside the app's folders.
/// A file that does not exist yet is taken; the launch checks again. Paths with `$(...)` are
/// checked at launch only.
pub fn check(app: &App) -> anyhow::Result<()> {
    let mut entries = game_files::entries(&app.extra)?;
    for block in app
        .extra
        .get(butterpollo_core::device_overrides::SETTING)
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|blocks| blocks.values())
    {
        if let Some(list) = block.get(game_files::SETTING) {
            entries.extend(game_files::parse(list)?);
        }
    }
    if entries.is_empty() || app.cmd.contains("$(") || app.working_dir.contains("$(") {
        return Ok(());
    }
    let (base, folders) = folders(app);
    if folders.is_empty() {
        return Ok(());
    }
    for entry in &entries {
        if let Err(Unusable::Refused(why)) =
            game_files::resolve(&entry.path, base.as_deref(), &folders)
        {
            anyhow::bail!("{}: {why}", game_files::SETTING);
        }
    }
    Ok(())
}

/// The stream's app (expanded, with its device's block on top) is about to start: write the
/// keys it lists. A key that cannot be written is skipped with a warning and the game starts.
/// `args` are the launch's: the device (`clientUuid`), the mode with the stream's rate and the
/// frame limiter's rate ([`FRAME_LIMIT_ARG`]). A launch without a device writes nothing.
pub fn launching(h: &Shared, app: &App, args: &HashMap<String, String>) {
    let entries = match game_files::entries(&app.extra) {
        Ok(entries) if entries.is_empty() => return,
        Ok(entries) => entries,
        Err(error) => {
            // Saved apps are checked; this is a hand-edited apps file.
            tracing::warn!(app = %app.name, error = format!("{error:#}"), "the app's managed game files are not usable; the game starts without them");
            return;
        }
    };
    let Some(device) = args.get("clientUuid").filter(|device| !device.is_empty()) else {
        tracing::info!(app = %app.name, "managed game files apply to streams; this launch has no device, so none is changed");
        return;
    };
    let fps = args
        .get("mode")
        .and_then(|mode| crate::radeon_profile::stream_fps(mode));
    let (base, folders) = folders(app);
    let mut report = Report::default();
    let targets = game_files::targets(&entries, fps, base.as_deref(), &folders, &mut report);
    if let Some(limit) = args
        .get(FRAME_LIMIT_ARG)
        .and_then(|limit| limit.parse::<u32>().ok())
    {
        for target in game_files::limit_conflicts(&targets, limit) {
            tracing::warn!(
                key = %target,
                value = %target.value,
                frame_limiter = limit,
                "a managed game file sets a frame limit other than the host's frame limiter; the lower one wins"
            );
        }
    }
    if targets.is_empty() {
        return;
    }
    if let Some(report) = h.game_files.apply(&h.directory, device, &targets) {
        if !report.written.is_empty() {
            tracing::info!(app = %app.name, keys = ?report.written, "managed game file keys set for the stream");
        }
        log_restore(&report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use serde_json::json;

    const INI: &str = "[Framerate]\r\nFramerateLimit=59\r\n";

    fn game(f: &Fixture) -> (App, PathBuf) {
        let folder = f.host.directory.join("Games").join("RDR2");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("OptiScaler.ini");
        std::fs::write(&file, INI).unwrap();
        let mut app: App = serde_json::from_value(json!({
            "name": "RDR2",
            "cmd": format!("\"{}\"", folder.join("RDR2.exe").display()),
            "game-files": [{"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": "{stream_fps}"}],
        }))
        .unwrap();
        app.working_dir = String::new();
        (app, file)
    }
    fn args(device: &str, mode: &str) -> HashMap<String, String> {
        HashMap::from([
            ("clientUuid".to_owned(), device.to_owned()),
            ("mode".to_owned(), mode.to_owned()),
        ])
    }

    #[test]
    fn a_stream_writes_its_rate_and_the_device_leaving_the_session_puts_it_back() {
        let f = Fixture::new();
        let h = &f.host;
        let (app, file) = game(&f);
        launching(h, &app, &args("phone", "1920x1080x119.88"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[Framerate]\r\nFramerateLimit=120\r\n"
        );
        // Not settled (a launch runs): nothing goes back.
        h.game_files.sync(&h.directory, || None);
        assert!(std::fs::read_to_string(&file).unwrap().contains("=120"));
        // Suspended: the owner is still present.
        h.game_files
            .sync(&h.directory, || Some(BTreeSet::from(["phone".to_owned()])));
        assert!(std::fs::read_to_string(&file).unwrap().contains("=120"));
        h.game_files.sync(&h.directory, || Some(BTreeSet::new()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
    }

    #[test]
    fn a_launch_without_a_device_or_a_key_still_goes_on() {
        let f = Fixture::new();
        let h = &f.host;
        let (mut app, file) = game(&f);
        // From the console: no device, nothing written.
        launching(h, &app, &HashMap::new());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
        // A key the file does not have is skipped, and so is nothing else.
        app.extra.insert(
            "game-files".into(),
            json!([{"path": "OptiScaler.ini", "section": "Framerate", "key": "Missing", "value": 1}]),
        );
        launching(h, &app, &args("phone", "1920x1080x60"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
    }

    #[test]
    fn unpairing_and_the_next_start_put_the_keys_back() {
        let f = Fixture::new();
        let h = &f.host;
        let (app, file) = game(&f);
        launching(h, &app, &args("phone", "1920x1080x60"));
        assert!(std::fs::read_to_string(&file).unwrap().contains("=60"));
        h.game_files.release_device(&h.directory, "tv");
        assert!(std::fs::read_to_string(&file).unwrap().contains("=60"));
        h.game_files.release_device(&h.directory, "phone");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
        // A run that stopped without its restore: the next start puts it back.
        launching(h, &app, &args("phone", "1920x1080x90"));
        let restarted = GameFiles::default();
        restarted.release_all(&h.directory);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
    }

    #[test]
    fn saving_refuses_a_file_outside_the_apps_folders() {
        let f = Fixture::new();
        let (mut app, _) = game(&f);
        check(&app).unwrap();
        let outside = f.host.directory.join("outside.ini");
        std::fs::write(&outside, INI).unwrap();
        app.extra.insert(
            "device-overrides".into(),
            json!({"tv": {"game-files": [{"path": outside.display().to_string(), "section": "Framerate", "key": "FramerateLimit", "value": 60}]}}),
        );
        assert!(check(&app).is_err());
        // A file that does not exist yet is taken.
        app.extra.insert(
            "device-overrides".into(),
            json!({"tv": {"game-files": [{"path": "later.ini", "section": "A", "key": "B", "value": 60}]}}),
        );
        check(&app).unwrap();
    }
}
