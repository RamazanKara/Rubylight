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
//! The files are reached as the signed-in Windows user, never as SYSTEM (see
//! `butterpollo_core::game_files::Access`). Restores run on a thread of their own, so a file
//! that is slow to open never holds the session lock; one that fails stays journaled and is
//! tried again.
use crate::state::{Host, Shared};
use butterpollo_core::{
    game_files::{self, JOURNAL_FILE, Report, Scope, Unusable},
    state::App,
};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak,
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

/// The launch argument the host sets for the rate its frame limiter holds the stream's game
/// to, when it limits; never the client's. See `process::BACKGROUND_ARG`.
pub const FRAME_LIMIT_ARG: &str = "rubylightFrameLimit";
/// The first wait before a restore that failed is tried again; it doubles up to [`MAX_RETRY`].
const RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(600);
/// The wait while nobody is signed in to Windows: the files are reached as the user, so the
/// restore runs as soon as someone signs in, before a game is started at the desk.
const SIGN_IN_POLL: Duration = Duration::from_secs(5);

/// How game files are reached: as the signed-in Windows user; the tests reach theirs as the
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

#[derive(Default)]
pub struct GameFiles {
    /// Devices whose stream may have written keys in this run. Their journal entries are
    /// theirs until they leave the session; every other entry is due to go back.
    written: Mutex<BTreeSet<String>>,
    /// Held through every journal change, so a write and a restore never interleave.
    journal: Mutex<()>,
    /// The restore thread, once the host started it.
    worker: Mutex<Option<mpsc::Sender<()>>>,
    /// Restores asked for and done, so a shutdown can wait for them.
    progress: Mutex<Progress>,
    settled: Condvar,
}

#[derive(Default)]
struct Progress {
    asked: u64,
    done: u64,
}

impl GameFiles {
    /// The session machine moved: `present` makes the devices still part of the session, or
    /// `None` while it has not settled (a launch that fails gives the session back). Every
    /// device that wrote keys and is not present gets them back. `present` is not called when
    /// no device wrote anything.
    pub fn sync(&self, directory: &Path, present: impl FnOnce() -> Option<BTreeSet<String>>) {
        {
            let mut written = locked(&self.written);
            if written.is_empty() {
                return;
            }
            let Some(present) = present() else {
                return;
            };
            let before = written.len();
            written.retain(|device| present.contains(device));
            if written.len() == before {
                return;
            }
        }
        self.request(directory);
    }
    /// A device was unpaired or turned off: its keys go back even though its session may
    /// linger until the grace period ends.
    pub fn release_device(&self, directory: &Path, device: &str) {
        if locked(&self.written).remove(device) {
            self.request(directory);
        }
    }
    /// Every key the host changed goes back, whatever the sessions do (unpair all, the start
    /// after a crash).
    pub fn release_all(&self, directory: &Path) {
        locked(&self.written).clear();
        self.request(directory);
    }
    /// A clean shutdown: every key goes back, waiting at most `wait`. What is not back by then
    /// stays journaled for the next start.
    pub fn shutdown(&self, directory: &Path, wait: Duration) {
        self.release_all(directory);
        if !self.flush(wait) {
            tracing::error!(
                wait_seconds = wait.as_secs(),
                "managed game file keys were still being put back at shutdown; the next host start puts back what is left"
            );
        }
    }
    /// Wait, at most `wait`, until every restore asked for so far has run. True when they have.
    pub fn flush(&self, wait: Duration) -> bool {
        let progress = locked(&self.progress);
        let asked = progress.asked;
        let (progress, _) = self
            .settled
            .wait_timeout_while(progress, wait, |progress| progress.done < asked)
            .unwrap_or_else(PoisonError::into_inner);
        progress.done >= asked
    }

    /// Have every journaled key of a device that left put back: on the restore thread, or
    /// here without one (it could not start, or in tests).
    fn request(&self, directory: &Path) {
        let asked = {
            let mut progress = locked(&self.progress);
            progress.asked += 1;
            progress.asked
        };
        let sent = locked(&self.worker)
            .as_ref()
            .is_some_and(|worker| worker.send(()).is_ok());
        if !sent {
            self.restore_ended(directory);
            self.finished(asked);
        }
    }
    fn finished(&self, asked: u64) {
        let mut progress = locked(&self.progress);
        progress.done = progress.done.max(asked);
        self.settled.notify_all();
    }

    /// Put back every journaled key of a device that is not streaming. True when one could not
    /// be, and stays journaled to be tried again.
    fn restore_ended(&self, directory: &Path) -> bool {
        let _journal = locked(&self.journal);
        // Read under the journal lock: a device that started a new stream since its restore was
        // asked for owns its entries again, and they go back when that stream ends.
        let running = locked(&self.written).clone();
        match game_files::restore(
            &directory.join(JOURNAL_FILE),
            Scope::Except(&running),
            ACCESS,
        ) {
            Ok(report) => {
                log_restore(&report);
                !report.failed.is_empty()
            }
            Err(error) => {
                tracing::error!(
                    error = format!("{error:#}"),
                    "the game files journal could not be read or saved; the keys in it stay changed, and it is tried again"
                );
                true
            }
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
        match game_files::apply(&directory.join(JOURNAL_FILE), device, targets, ACCESS) {
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

/// Start the restore thread and put back what a run that ended without its restores left
/// changed. At host start, before any stream.
pub fn start(h: &Shared) {
    let (sender, receiver) = mpsc::channel();
    let host = Arc::downgrade(h);
    match std::thread::Builder::new()
        .name("game-files".into())
        .spawn(move || restorer(&host, &receiver))
    {
        Ok(_) => *locked(&h.game_files.worker) = Some(sender),
        Err(error) => {
            tracing::error!(%error, "no thread for managed game file restores; they run where they are decided");
        }
    }
    h.game_files.release_all(&h.directory);
}

/// The wait before a restore that failed is tried again, the last wait having been `last`:
/// [`SIGN_IN_POLL`] while nobody is signed in, otherwise [`RETRY`], doubling up to
/// [`MAX_RETRY`].
fn next_wait(last: Option<Duration>, nobody_signed_in: bool) -> Duration {
    if nobody_signed_in {
        return SIGN_IN_POLL;
    }
    match last {
        Some(last) if last >= RETRY => (last * 2).min(MAX_RETRY),
        _ => RETRY,
    }
}

/// Whether the host runs as the service at the sign-in screen, with no user to act as.
fn nobody_signed_in() -> bool {
    use butterpollo_windows::process;
    process::is_system() && !process::user_signed_in()
}

/// What the restore thread does once it wakes.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The host is gone.
    Stop,
    Restore,
    /// A retry is due at the sign-in screen, where it would only fail again (and log and save
    /// the journal again): it looks for a sign-in again after [`SIGN_IN_POLL`].
    AwaitSignIn,
}

fn next(woke: Result<(), RecvTimeoutError>, nobody_signed_in: impl FnOnce() -> bool) -> Next {
    match woke {
        Err(RecvTimeoutError::Disconnected) => Next::Stop,
        Err(RecvTimeoutError::Timeout) if nobody_signed_in() => Next::AwaitSignIn,
        _ => Next::Restore,
    }
}

/// The restore thread: a restore for every request (those that queue up meanwhile run as
/// one), and again later while one fails. It ends with the host.
fn restorer(host: &Weak<Host>, receiver: &Receiver<()>) {
    let mut retry: Option<(Instant, Duration)> = None;
    loop {
        let woke = match retry {
            Some((at, _)) => receiver.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match next(woke, nobody_signed_in) {
            Next::Stop => return,
            Next::AwaitSignIn => {
                retry = Some((Instant::now() + SIGN_IN_POLL, SIGN_IN_POLL));
                continue;
            }
            Next::Restore => {}
        }
        while receiver.try_recv().is_ok() {}
        let Some(h) = host.upgrade() else {
            return;
        };
        let files = &h.game_files;
        let asked = locked(&files.progress).asked;
        let failed = files.restore_ended(&h.directory);
        files.finished(asked);
        retry = failed.then(|| {
            let wait = next_wait(retry.map(|(_, wait)| wait), nobody_signed_in());
            (Instant::now() + wait, wait)
        });
    }
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
    let folders = game_files::folders(working.into_iter().chain(program).chain(steam), ACCESS);
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
            game_files::resolve(&entry.path, base.as_deref(), &folders, ACCESS)
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
    let targets = game_files::targets(
        &entries,
        fps,
        base.as_deref(),
        &folders,
        ACCESS,
        &mut report,
    );
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
    fn restores_run_on_their_own_thread_and_a_shutdown_waits_for_them() {
        let f = Fixture::new();
        let h = &f.host;
        let (app, file) = game(&f);
        start(h);
        assert!(locked(&h.game_files.worker).is_some());
        launching(h, &app, &args("phone", "1920x1080x60"));
        assert!(std::fs::read_to_string(&file).unwrap().contains("=60"));
        h.game_files.sync(&h.directory, || Some(BTreeSet::new()));
        assert!(h.game_files.flush(Duration::from_secs(10)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
        launching(h, &app, &args("tv", "1920x1080x90"));
        assert!(std::fs::read_to_string(&file).unwrap().contains("=90"));
        h.game_files.shutdown(&h.directory, Duration::from_secs(10));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), INI);
        assert!(locked(&h.game_files.written).is_empty());
    }

    #[test]
    fn a_journal_that_cannot_be_read_is_tried_again() {
        let f = Fixture::new();
        let h = &f.host;
        std::fs::create_dir_all(h.directory.join(JOURNAL_FILE)).unwrap();
        assert!(h.game_files.restore_ended(&h.directory));
    }

    #[test]
    fn a_failed_restore_waits_for_a_sign_in_then_less_and_less_often() {
        // At the sign-in screen it looks again every few seconds, so a reboot's restore runs
        // right after the sign-in, before a game starts at the desk.
        assert_eq!(next_wait(None, true), SIGN_IN_POLL);
        assert_eq!(next_wait(Some(SIGN_IN_POLL), true), SIGN_IN_POLL);
        assert_eq!(next_wait(Some(SIGN_IN_POLL), false), RETRY);
        assert_eq!(next_wait(None, false), RETRY);
        assert_eq!(next_wait(Some(RETRY), false), RETRY * 2);
        assert_eq!(next_wait(Some(MAX_RETRY), false), MAX_RETRY);
        assert_eq!(next_wait(Some(MAX_RETRY), true), SIGN_IN_POLL);
        // While it waits, a due retry only looks for a sign-in; one asked for always runs.
        let timeout = Err(RecvTimeoutError::Timeout);
        assert_eq!(next(timeout, || true), Next::AwaitSignIn);
        assert_eq!(next(timeout, || false), Next::Restore);
        assert_eq!(next(Ok(()), || true), Next::Restore);
        assert_eq!(
            next(Err(RecvTimeoutError::Disconnected), || false),
            Next::Stop
        );
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
