//! Durable display restoration after an abort, crash or service termination.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Mutex, OnceLock},
};
use windows::Win32::{Foundation::*, System::Threading::*};
type Mode = (u32, u32, u32);
#[derive(Clone, Default, Serialize, Deserialize)]
struct Entry {
    output: String,
    mode: Option<(Mode, Mode)>,
    #[serde(default)]
    mode_rate: Option<(Mode, Mode)>,
    hdr: Option<(bool, bool)>,
    #[serde(default)]
    position: Option<(
        butterpollo_core::topology::Position,
        butterpollo_core::topology::Position,
    )>,
    #[serde(default)]
    profile: Option<(Option<String>, String, bool)>,
    #[serde(default)]
    external: bool,
    #[serde(default)]
    audio: bool,
    #[serde(default)]
    microphone: bool,
    #[serde(default)]
    arrangement: Option<(
        crate::display::Snapshot,
        Vec<butterpollo_core::topology::Node>,
    )>,
    #[serde(default)]
    baseline: Option<(crate::display::Snapshot, Vec<String>)>,
    #[serde(default)]
    activation: Option<(
        crate::display::Snapshot,
        Vec<butterpollo_core::topology::Node>,
    )>,
    /// A `DISPLAYCONFIG_ROTATION`, (original, applied).
    #[serde(default)]
    rotation: Option<(u32, u32)>,
    /// The Windows scale in percent, (original, applied).
    #[serde(default)]
    scale: Option<(u32, u32)>,
}
impl Entry {
    fn display_settings(&self) -> Self {
        Self {
            output: self.output.clone(),
            profile: self.profile.clone(),
            hdr: self.hdr,
            mode: self.mode,
            mode_rate: self.mode_rate,
            position: self.position,
            microphone: self.microphone,
            rotation: self.rotation,
            scale: self.scale,
            ..Default::default()
        }
    }
    fn pending(&self) -> bool {
        self.mode.is_some()
            || self.hdr.is_some()
            || self.mode_rate.is_some()
            || self.position.is_some()
            || self.profile.is_some()
            || self.external
            || self.audio
            || self.microphone
            || self.arrangement.is_some()
            || self.baseline.is_some()
            || self.activation.is_some()
            || self.rotation.is_some()
            || self.scale.is_some()
    }
}

/// Set `previous` back when a display still has the value a stream applied:
/// one the user has changed since stays theirs.
pub(crate) fn put_back<T: PartialEq>(
    current: Result<T>,
    previous: T,
    applied: T,
    set: impl FnOnce(T) -> Result<()>,
) -> Result<()> {
    if current? == applied {
        set(previous)
    } else {
        Ok(())
    }
}
/// Keep the original value until restoration succeeds (or the operation
/// establishes that the user has since changed it).
fn restored<T>(pending: &mut Option<T>, result: Result<()>) -> Result<()> {
    result?;
    *pending = None;
    Ok(())
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Journal {
    pid: u32,
    started: u64,
    entries: BTreeMap<String, Entry>,
}
static PATH: OnceLock<PathBuf> = OnceLock::new();
static WATCH: Mutex<Option<(Child, OwnedHandle)>> = Mutex::new(None);

fn watch_done(pid: u32) -> Result<OwnedHandle> {
    let name: Vec<u16> = format!("Local\\Butterpollo.DisplayWatch.{pid}\0")
        .encode_utf16()
        .collect();
    // SAFETY: The name is NUL-terminated, and the returned handle is owned exactly once.
    unsafe {
        let event = CreateEventW(None, true, false, windows::core::PCWSTR(name.as_ptr()))?;
        Ok(OwnedHandle::from_raw_handle(event.0))
    }
}

fn finish_watch(path: &Path) -> Result<()> {
    let guard = lock(path)?;
    let journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    if !journal.entries.is_empty() {
        return Ok(());
    }
    let mut watch = WATCH.lock().unwrap();
    let Some((process, event)) = watch.as_mut() else {
        return Ok(());
    };
    // SAFETY: The owned event remains live until the watcher exits.
    unsafe { SetEvent(HANDLE(event.as_raw_handle()))? };
    drop(guard);
    // Keep WATCH locked so a new watcher cannot reset this event before the old
    // watcher observes it. Native process exit must not hold the journal lock.
    wait_for_watch(process)?;
    watch.take();
    Ok(())
}
/// How long a killed watcher may take to go. Termination is asynchronous and
/// on a busy PC took longer than 100 ms; only a stalled watcher waits this.
const KILLED_WATCH_EXIT_MS: u32 = 2000;
fn wait_for_watch(process: &mut Child) -> Result<()> {
    let handle = HANDLE(process.as_raw_handle());
    stop_watch(
        // SAFETY: Child owns the process handle for the entire bounded wait,
        // and still after requesting termination.
        |ms| unsafe { WaitForSingleObject(handle, ms) },
        || Ok(process.kill()?),
    )
}
/// [`wait_for_watch`] with the process wait and kill passed in.
fn stop_watch(
    mut wait: impl FnMut(u32) -> WAIT_EVENT,
    kill: impl FnOnce() -> Result<()>,
) -> Result<()> {
    match wait(100) {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => {
            // Its journal is empty; a stalled watcher has nothing left to restore.
            tracing::warn!("display recovery helper did not exit within 100 ms; stopping it");
            kill()?;
            // Termination is asynchronous. Keep the signaled watcher registered
            // unless it exits, so a new watcher cannot reuse its named event.
            match wait(KILLED_WATCH_EXIT_MS) {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => bail!(
                    "display recovery helper did not stop within {KILLED_WATCH_EXIT_MS} ms of being killed"
                ),
                _ => return Err(windows::core::Error::from_thread().into()),
            }
        }
        _ => return Err(windows::core::Error::from_thread().into()),
    }
    Ok(())
}
struct JournalLock(HANDLE);
impl Drop for JournalLock {
    fn drop(&mut self) {
        // SAFETY: lock() acquired this mutex on the current thread, and its local
        // guard owns the handle until this single release and close.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}
fn lock(path: &Path) -> Result<JournalLock> {
    let hash = butterpollo_core::crypto::hash(path.to_string_lossy().to_lowercase().as_bytes());
    let name: Vec<u16> = format!(
        "Local\\Butterpollo.DisplayJournal.{:x}\0",
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    )
    .encode_utf16()
    .collect();
    // SAFETY: name is terminated and lives through CreateMutexW. The returned handle
    // is either closed on failure or transferred to a guard after this thread acquires it.
    unsafe {
        let handle = CreateMutexW(None, false, windows::core::PCWSTR(name.as_ptr()))?;
        let result = WaitForSingleObject(handle, 30000);
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            let _ = CloseHandle(handle);
            bail!("display journal is busy");
        }
        Ok(JournalLock(handle))
    }
}

fn identity(pid: u32) -> Result<Option<u64>> {
    // SAFETY: OpenProcess supplies an owned query handle; the four initialized outputs
    // remain writable for GetProcessTimes, and the handle is closed exactly once afterward.
    unsafe {
        if pid == 0 {
            return Ok(None);
        }
        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let result = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        result?;
        if exited.dwLowDateTime != 0 || exited.dwHighDateTime != 0 {
            return Ok(None);
        }
        Ok(Some(
            u64::from(created.dwLowDateTime) | (u64::from(created.dwHighDateTime) << 32),
        ))
    }
}
/// Take the journal and undo what an interrupted host left behind. An
/// incomplete recovery is returned for the log rather than as an error: a
/// display that cannot take back a setting must not stop the host from
/// starting, as it did on every start while the journal stayed pending.
pub fn initialize(directory: &Path) -> Result<Option<anyhow::Error>> {
    let path = directory.join("display-recovery.json");
    let _guard = lock(&path)?;
    let mut incomplete = None;
    if path.exists() {
        let journal: Journal = serde_json::from_slice(&std::fs::read(&path)?)?;
        if identity(journal.pid)? == Some(journal.started) && journal.pid != std::process::id() {
            bail!("another host owns this display recovery journal");
        }
        incomplete = recover(&path).err();
    }
    PATH.set(path)
        .map_err(|_| anyhow::anyhow!("display recovery already initialized"))?;
    Ok(incomplete)
}
fn watch(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let mut child = WATCH.lock().unwrap();
    if let Some((process, event)) = child.as_mut()
        && process.try_wait()?.is_none()
    {
        // SAFETY: The watcher entry owns the event for this nonblocking check.
        if unsafe { WaitForSingleObject(HANDLE(event.as_raw_handle()), 0) } == WAIT_OBJECT_0 {
            bail!("display recovery helper is still stopping");
        }
        return Ok(());
    }
    let done = watch_done(std::process::id())?;
    // SAFETY: This new watcher owns the event until it exits; the previous child is already done.
    unsafe { ResetEvent(HANDLE(done.as_raw_handle()))? };
    let spawn = |flags: u32| -> Result<std::process::Child> {
        Ok(Command::new(std::env::current_exe()?)
            .arg("--display-watch")
            .arg(std::process::id().to_string())
            .arg("--config-dir")
            .arg(path.parent().context("journal directory missing")?)
            .creation_flags(flags)
            .spawn()?)
    };
    // Escape the host's job, otherwise a service stop would also kill recovery.
    // A job that forbids breaking away (a launcher, a terminal) refuses that
    // with access denied, and every frame limit and display change failed on
    // it; the helper then stays in the job and still covers a host crash.
    let started = spawn(CREATE_BREAKAWAY_FROM_JOB.0 | CREATE_NO_WINDOW.0).or_else(|error| {
        tracing::debug!(%error, "display recovery helper cannot leave the host's job");
        spawn(CREATE_NO_WINDOW.0)
    });
    *child = Some((
        started.context("cannot start the display recovery helper")?,
        done,
    ));
    Ok(())
}
fn change(id: &str, output: &str, update: impl FnOnce(&mut Entry)) -> Result<()> {
    let path = PATH.get().context("display recovery is not initialized")?;
    let guard = lock(path)?;
    let mut journal: Journal = if path.exists() {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        Journal::default()
    };
    let prior = journal.clone();
    journal.pid = std::process::id();
    journal.started = identity(journal.pid)?.context("host identity unavailable")?;
    let entry = journal.entries.entry(id.into()).or_default();
    entry.output = output.into();
    update(entry);
    journal.entries.retain(|_, entry| entry.pending());
    butterpollo_core::state::write_json(path, &journal)?;
    if journal.entries.is_empty() {
        drop(guard);
        return finish_watch(path);
    }
    if let Err(e) = watch(path) {
        butterpollo_core::state::write_json(path, &prior)?;
        return Err(e);
    }
    Ok(())
}
// A pending entry's original value wins over a newer "before": an entry kept
// for a display that was away holds the value to restore, and a later stream
// would otherwise record the changed value as the original.
pub fn mode(id: &str, output: &str, before: Mode, applied: Mode) -> Result<()> {
    change(id, output, |e| {
        e.mode = Some((e.mode.map_or(before, |(original, _)| original), applied))
    })
}
pub fn mode_rate(id: &str, output: &str, before: Mode, applied: Mode) -> Result<()> {
    change(id, output, |e| {
        e.mode_rate = Some((
            e.mode_rate.map_or(before, |(original, _)| original),
            applied,
        ))
    })
}
pub fn hdr(id: &str, output: &str, before: bool, applied: bool) -> Result<()> {
    change(id, output, |e| {
        e.hdr = Some((e.hdr.map_or(before, |(original, _)| original), applied))
    })
}
/// A display's rotation, as `DISPLAYCONFIG_ROTATION`.
pub fn rotation(id: &str, output: &str, before: u32, applied: u32) -> Result<()> {
    change(id, output, |e| {
        e.rotation = Some((e.rotation.map_or(before, |(original, _)| original), applied))
    })
}
/// A display's Windows scale, in percent.
pub fn scale(id: &str, output: &str, before: u32, applied: u32) -> Result<()> {
    change(id, output, |e| {
        e.scale = Some((e.scale.map_or(before, |(original, _)| original), applied))
    })
}
pub fn external(pending: bool) -> Result<()> {
    change("external-limiter", "", |e| e.external = pending)
}
pub fn audio(pending: bool) -> Result<()> {
    change("audio-routing", "", |e| e.audio = pending)
}
pub fn microphone(pending: bool) -> Result<()> {
    change("microphone-routing", "", |e| e.microphone = pending)
}
pub fn arrangement(
    value: Option<(
        crate::display::Snapshot,
        Vec<butterpollo_core::topology::Node>,
    )>,
) -> Result<()> {
    change("display-arrangement", "", |e| e.arrangement = value)
}
/// Adopt an unfinished arrangement before another stream journals its layout.
/// Otherwise a failed restore becomes the next stream's apparent baseline.
pub fn pending_arrangement() -> Result<Option<crate::display::Snapshot>> {
    let Some(path) = PATH.get() else {
        return Ok(None);
    };
    let _guard = lock(path)?;
    read_pending_arrangement(path)
}
fn read_pending_arrangement(path: &Path) -> Result<Option<crate::display::Snapshot>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut journal: Journal = serde_json::from_slice(&bytes)?;
    Ok(journal
        .entries
        .remove("display-arrangement")
        .and_then(|entry| entry.arrangement.map(|(before, _)| before)))
}
pub fn baseline(value: Option<(crate::display::Snapshot, Vec<String>)>) -> Result<()> {
    change("saved-display-baseline", "", |entry| entry.baseline = value)
}
pub fn activation(
    value: Option<(
        crate::display::Snapshot,
        Vec<butterpollo_core::topology::Node>,
    )>,
) -> Result<()> {
    change("display-activation", "", |entry| entry.activation = value)
}
pub fn reset() -> Result<()> {
    let path = PATH.get().context("display recovery is not initialized")?;
    let guard = lock(path)?;
    let result = recover(path);
    drop(guard);
    if path.exists() {
        finish_watch(path)?;
    }
    result
}
/// What a kept entry holds for a display, which was away when a stream
/// ended and still has what that stream applied: the original values, and
/// for the mode also the one applied.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pending {
    pub hdr: Option<bool>,
    /// (original, applied) width, height and refresh in millihertz.
    pub mode: Option<(Mode, Mode)>,
    pub profile: Option<Option<String>>,
    /// The original `DISPLAYCONFIG_ROTATION`.
    pub rotation: Option<u32>,
    /// The original Windows scale, in percent.
    pub scale: Option<u32>,
}
/// What an earlier stream left to restore on a display. A stream that takes
/// the display over restores these rather than the values it finds, or its
/// release would clear the entry and leave them changed for good. A journal
/// that cannot be read counts as nothing pending; releasing it fails alike.
pub fn pending(id: &str) -> Pending {
    let Some(path) = PATH.get().filter(|p| p.exists()) else {
        return Pending::default();
    };
    lock(path)
        .and_then(|_guard| pending_in(path, id))
        .unwrap_or_else(|error| {
            tracing::warn!(error = %format!("{error:#}"), "display recovery journal is unreadable");
            Pending::default()
        })
}
pub(crate) fn pending_in(path: &Path, id: &str) -> Result<Pending> {
    let journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(journal
        .entries
        .get(id)
        .map(|entry| Pending {
            hdr: entry.hdr.map(|(original, _)| original),
            mode: entry.mode_rate,
            profile: entry
                .profile
                .as_ref()
                .map(|(original, ..)| original.clone()),
            rotation: entry.rotation.map(|(original, _)| original),
            scale: entry.scale.map(|(original, _)| original),
        })
        .unwrap_or_default())
}
pub fn release(id: &str) -> Result<()> {
    let Some(path) = PATH.get().filter(|p| p.exists()) else {
        return Ok(());
    };
    let guard = lock(path)?;
    release_in(path, id)?;
    drop(guard);
    finish_watch(path)
}
pub(crate) fn release_in(path: &Path, id: &str) -> Result<()> {
    let mut journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    if let Some(entry) = journal.entries.get_mut(id) {
        entry.mode = None;
        entry.mode_rate = None;
        entry.hdr = None;
        entry.rotation = None;
        entry.scale = None;
    }
    journal.entries.retain(|_, e| e.pending());
    butterpollo_core::state::write_json(path, &journal)
}
pub fn position(
    id: &str,
    output: &str,
    before: butterpollo_core::topology::Position,
    applied: butterpollo_core::topology::Position,
) -> Result<()> {
    change(id, output, |e| e.position = Some((before, applied)))
}
pub fn release_position(id: &str) -> Result<()> {
    let Some(path) = PATH.get().filter(|p| p.exists()) else {
        return Ok(());
    };
    let guard = lock(path)?;
    let mut journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    if let Some(entry) = journal.entries.get_mut(id) {
        entry.position = None;
    }
    journal.entries.retain(|_, e| e.pending());
    butterpollo_core::state::write_json(path, &journal)?;
    drop(guard);
    finish_watch(path)
}
pub fn profile(
    id: &str,
    output: &str,
    before: Option<String>,
    applied: &str,
    system: bool,
) -> Result<()> {
    change(id, output, |e| {
        let original = e.profile.take().map_or(before, |(original, ..)| original);
        e.profile = Some((original, applied.into(), system))
    })
}
pub fn release_profile(id: &str) -> Result<()> {
    let Some(path) = PATH.get().filter(|p| p.exists()) else {
        return Ok(());
    };
    let guard = lock(path)?;
    let mut journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    if let Some(entry) = journal.entries.get_mut(id) {
        entry.profile = None;
    }
    journal.entries.retain(|_, e| e.pending());
    butterpollo_core::state::write_json(path, &journal)?;
    drop(guard);
    finish_watch(path)
}
/// Undo what an interrupted host changed. Every step is attempted even when
/// an earlier one fails, and the failures are reported together. A display
/// that is not connected now, such as a TV that was switched off, keeps its
/// own HDR, colour profile, mode and position entries for a later recovery:
/// Windows remembers those settings per display, so they would otherwise
/// stay changed. Failed per-display changes and arrangement restoration are
/// kept for a later recovery as well.
fn recover(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    let directory = path.parent().context("journal directory missing")?;
    let mut failures = Vec::new();
    let mut step = |what: &str, result: Result<()>| {
        if let Err(error) = result {
            failures.push(format!("{what}: {error:#}"));
        }
    };
    if journal.entries.values().any(|e| e.external) {
        step("frame limiter", crate::limiter::recover(directory));
    }
    if journal.entries.values().any(|e| e.audio) {
        step("audio routing", crate::audio_route::recover(directory));
    }
    let mut pending_arrangements = BTreeMap::new();
    for (id, entry) in &journal.entries {
        if let Some((before, applied)) = &entry.arrangement {
            let result = crate::display_arrangement::restore_arrangement(before, applied);
            if result.is_err() {
                pending_arrangements.insert(id.clone(), entry.arrangement.clone());
            }
            step("display arrangement", result);
        }
    }
    let monitors = crate::display::monitors().unwrap_or_else(|error| {
        step("display list", Err(error));
        Vec::new()
    });
    let mut kept = Journal {
        entries: journal
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.display_settings()))
            .collect(),
        ..Default::default()
    };
    for (id, arrangement) in pending_arrangements {
        kept.entries.entry(id).or_default().arrangement = arrangement;
    }
    for entry in kept.entries.values_mut().filter(|entry| entry.microphone) {
        let result = crate::audio_route::recover_microphone(directory);
        // An unplugged original microphone must remain recoverable after a
        // later restart, just like a display that is temporarily unavailable.
        entry.microphone = result.is_err();
        step("microphone routing", result);
    }
    for (id, entry) in &journal.entries {
        let Some(m) = monitors.iter().find(|m| &m.device_id == id) else {
            // Each setting is restored later only if the display still has
            // the value the stream applied.
            continue;
        };
        let pending = kept.entries.get_mut(id).expect("copied display entry");
        let what = |setting: &str| format!("{} {setting}", m.display_name);
        if let Some((previous, applied, system)) = &entry.profile {
            step(
                &what("colour profile"),
                restored(
                    &mut pending.profile,
                    crate::hdr_profile::restore(m, previous.as_deref(), applied, *system),
                ),
            );
        }
        if let Some((previous, applied)) = entry.hdr {
            let result = if m.hdr_enabled == applied {
                crate::display::set_hdr(m, previous)
            } else {
                Ok(())
            };
            step(&what("HDR"), restored(&mut pending.hdr, result));
        }
        // Before the mode: the mode applied was recorded in the shape the
        // display had before it turned.
        if let Some((previous, applied)) = entry.rotation {
            step(
                &what("rotation"),
                restored(
                    &mut pending.rotation,
                    put_back(
                        crate::display::rotation(id),
                        previous,
                        applied,
                        |previous| crate::display::set_rotation(id, previous),
                    ),
                ),
            );
        }
        if let Some((previous, applied)) = entry.mode {
            step(
                &what("mode"),
                restored(
                    &mut pending.mode,
                    crate::display::mode(&m.display_name).and_then(|current| {
                        if (
                            current.dmPelsWidth,
                            current.dmPelsHeight,
                            current.dmDisplayFrequency,
                        ) == applied
                        {
                            crate::display::set_mode(
                                &m.display_name,
                                previous.0,
                                previous.1,
                                previous.2,
                            )
                        } else {
                            Ok(())
                        }
                    }),
                ),
            );
        }
        if let Some((previous, applied)) = entry.mode_rate {
            step(
                &what("mode and refresh"),
                restored(
                    &mut pending.mode_rate,
                    (|| {
                        let current = crate::display::mode(&m.display_name)?;
                        let rate = crate::display::Topology::query()?.refresh(id)?;
                        if (current.dmPelsWidth, current.dmPelsHeight, rate.0) == applied {
                            crate::display::Topology::set_mode_rate(
                                id,
                                previous.0,
                                previous.1,
                                butterpollo_core::framegen::Rate(previous.2),
                            )?;
                        }
                        Ok(())
                    })(),
                ),
            );
        }
        if let Some((previous, applied)) = entry.scale {
            step(
                &what("scale"),
                restored(
                    &mut pending.scale,
                    put_back(
                        crate::display::dpi_scale(m),
                        previous,
                        applied,
                        |previous| crate::display::set_dpi_scale(m, previous),
                    ),
                ),
            );
        }
    }
    step(
        "display positions",
        (|| {
            let mut topology = crate::display::Topology::query()?;
            let nodes = topology.nodes()?;
            let positions = journal
                .entries
                .iter()
                .filter_map(|(id, entry)| {
                    let (before, applied) = entry.position?;
                    nodes
                        .iter()
                        .find(|n| &n.device_id == id && n.desired_position == applied)
                        .map(|_| (id.clone(), before))
                })
                .collect::<BTreeMap<_, _>>();
            if !positions.is_empty() {
                topology.set_positions(&positions)?;
            }
            for (id, pending) in &mut kept.entries {
                if nodes.iter().any(|node| &node.device_id == id) {
                    pending.position = None;
                }
            }
            Ok(())
        })(),
    );
    for entry in journal.entries.values() {
        if let Some((before, applied)) = &entry.activation {
            step(
                "display activation",
                (|| {
                    if applied.is_empty() || crate::display_arrangement::unchanged(applied)? {
                        before.restore()?;
                    }
                    Ok(())
                })(),
            );
        }
        if let Some((snapshot, excluded)) = &entry.baseline {
            step("saved display layout", snapshot.restore_excluding(excluded));
        }
    }
    // Keep a valid document so interrupted reads never see partial JSON. The
    // kept entries have no owner until the next host records a change.
    kept.entries.retain(|_, entry| entry.pending());
    butterpollo_core::state::write_json(path, &kept)?;
    if !failures.is_empty() {
        bail!("display recovery incomplete: {}", failures.join("; "));
    }
    Ok(())
}
pub fn wait_and_recover(pid: u32, directory: &Path) -> Result<()> {
    let path = directory.join("display-recovery.json");
    let owner: Journal = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path)?)?
    } else {
        Journal::default()
    };
    if owner.pid != pid {
        return Ok(());
    }
    let done = watch_done(pid)?;
    // SAFETY: The successfully opened process handle has synchronization rights and
    // remains owned until the wait finishes; every exit path closes it exactly once.
    unsafe {
        match OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        ) {
            Ok(process) => {
                let identity = identity(pid);
                if identity.is_err() {
                    let _ = CloseHandle(process);
                }
                let result = if identity? == Some(owner.started) {
                    WaitForMultipleObjects(
                        &[process, HANDLE(done.as_raw_handle())],
                        false,
                        u32::MAX,
                    )
                } else {
                    WAIT_OBJECT_0
                };
                let _ = CloseHandle(process);
                if result == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                    return Ok(());
                }
                if result != WAIT_OBJECT_0 {
                    bail!("display watcher wait failed");
                }
            }
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
            }
            Err(e) => return Err(e.into()),
        }
    }
    let _guard = lock(&path)?;
    if path.exists() {
        let journal: Journal = serde_json::from_slice(&std::fs::read(&path)?)?;
        // A newly started host may already have reclaimed the journal.
        if journal.pid != pid {
            return Ok(());
        }
    }
    recover(&path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_next_arrangement_adopts_the_original_layout_after_a_failed_restore() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("display-recovery.json");
        assert!(read_pending_arrangement(&path)?.is_none());
        let before = crate::display::Snapshot::decode(&serde_json::json!({
            "topology": [["physical"]],
            "modes": {"physical": {"w": 2560, "h": 1440, "num": 120, "den": 1}},
            "primary": "physical"
        }))?;
        let mut pending = Some((before, Vec::new()));
        assert!(restored(&mut pending, Err(anyhow::anyhow!("Windows is busy"))).is_err());
        let mut journal = Journal::default();
        journal.entries.insert(
            "display-arrangement".into(),
            Entry {
                arrangement: pending.clone(),
                ..Default::default()
            },
        );
        butterpollo_core::state::write_json(&path, &journal)?;
        let original = read_pending_arrangement(&path)?.expect("unfinished layout");
        assert_eq!(original.nodes[0].device_id, "physical");
        assert!(original.nodes[0].active);
        assert_eq!(original.nodes[0].mode.width, 2560);
        restored(&mut pending, Ok(()))?;
        journal
            .entries
            .get_mut("display-arrangement")
            .unwrap()
            .arrangement = pending;
        butterpollo_core::state::write_json(&path, &journal)?;
        assert!(read_pending_arrangement(&path)?.is_none());
        // Corrupt recovery data must not be overwritten by a new baseline.
        std::fs::write(&path, b"{")?;
        assert!(read_pending_arrangement(&path).is_err());
        Ok(())
    }
    #[test]
    fn microphone_recovery_survives_journal_rewrites() -> Result<()> {
        let entry = Entry {
            microphone: true,
            ..Default::default()
        };
        let retained = entry.display_settings();
        let mut reloaded: Entry = serde_json::from_slice(&serde_json::to_vec(&retained)?)?;
        assert!(reloaded.microphone);
        assert!(reloaded.pending());
        reloaded.microphone = false;
        assert!(!reloaded.pending());
        let old: Entry = serde_json::from_str(r#"{"output":"","mode":null,"hdr":null}"#)?;
        assert!(!old.microphone);
        Ok(())
    }
    #[test]
    fn failed_display_settings_survive_the_next_recovery_journal() -> Result<()> {
        let original = Entry {
            hdr: Some((false, true)),
            mode: Some(((3840, 2160, 60), (1920, 1080, 120))),
            profile: Some((None, "stream.icm".into(), false)),
            ..Default::default()
        };
        let mut pending = original.display_settings();
        assert!(restored(&mut pending.hdr, Err(anyhow::anyhow!("busy"))).is_err());
        restored(&mut pending.mode, Ok(()))?;
        assert!(restored(&mut pending.profile, Err(anyhow::anyhow!("busy"))).is_err());
        let mut reloaded: Entry = serde_json::from_slice(&serde_json::to_vec(&pending)?)?;
        assert_eq!(reloaded.hdr, original.hdr);
        assert_eq!(reloaded.profile, original.profile);
        assert!(reloaded.mode.is_none());
        restored(&mut reloaded.hdr, Ok(()))?;
        restored(&mut reloaded.profile, Ok(()))?;
        assert!(!reloaded.pending());
        Ok(())
    }
    #[test]
    fn a_turned_or_scaled_display_gets_its_original_back_unless_the_user_changed_it() -> Result<()>
    {
        let mut set = Vec::new();
        put_back(Ok(2), 1, 2, |previous| {
            set.push(previous);
            Ok(())
        })?;
        // Turned again at the PC since: the user's rotation stays.
        put_back(Ok(4), 1, 2, |previous| {
            set.push(previous);
            Ok(())
        })?;
        assert_eq!(set, [1]);
        // A display that cannot be read, or refuses, keeps its entry for later.
        let mut pending = Some((100, 150));
        let gone = put_back(Err(anyhow::anyhow!("gone")), 100, 150, |_| {
            panic!("not read")
        });
        assert!(restored(&mut pending, gone).is_err());
        let refused = put_back(Ok(150), 100, 150, |_| bail!("busy"));
        assert!(restored(&mut pending, refused).is_err());
        assert_eq!(pending, Some((100, 150)));
        // A journal from before rotation and scale were kept reads without them.
        let old: Entry = serde_json::from_str(r#"{"output":"","mode":null,"hdr":null}"#)?;
        assert!(old.rotation.is_none() && old.scale.is_none());
        // Either one alone keeps the entry, and recovery keeps both until restored.
        let turned = Entry {
            rotation: Some((1, 2)),
            scale: Some((100, 150)),
            ..Default::default()
        };
        assert!(turned.pending());
        let kept = turned.display_settings();
        assert_eq!(
            (kept.rotation, kept.scale),
            (Some((1, 2)), Some((100, 150)))
        );
        Ok(())
    }
    #[test]
    fn stalled_watcher_exit_is_bounded() -> Result<()> {
        use std::os::windows::process::CommandExt;
        let mut child = Command::new("cmd.exe")
            .args(["/C", "set /p waiting="])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()?;
        let started = std::time::Instant::now();
        wait_for_watch(&mut child)?;
        // Bounded by the 100 ms grace and the wait after the kill, not by how
        // quickly a busy PC ends the process.
        assert!(
            started.elapsed()
                < std::time::Duration::from_millis(100 + u64::from(KILLED_WATCH_EXIT_MS) + 1000)
        );
        assert_eq!(
            // SAFETY: Child retains its process handle through this bounded exit check.
            unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), 1000) },
            WAIT_OBJECT_0
        );
        assert!(!child.wait()?.success());
        Ok(())
    }
    /// A killed watcher that takes `exit_ms` to go, as on a busy PC.
    fn stop_slow_watch(exit_ms: u32) -> (Result<()>, Vec<u32>, bool) {
        let mut waits = Vec::new();
        let killed = std::cell::Cell::new(false);
        let result = stop_watch(
            |ms| {
                waits.push(ms);
                if killed.get() && ms >= exit_ms {
                    WAIT_OBJECT_0
                } else {
                    WAIT_TIMEOUT
                }
            },
            || {
                killed.set(true);
                Ok(())
            },
        );
        (result, waits, killed.get())
    }
    #[test]
    fn a_killed_watcher_slow_to_exit_on_a_busy_pc_stops() {
        let (result, waits, killed) = stop_slow_watch(1500);
        result.unwrap();
        assert!(killed);
        assert_eq!(waits, [100, KILLED_WATCH_EXIT_MS]);
        let (result, _, _) = stop_slow_watch(KILLED_WATCH_EXIT_MS + 1);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("did not stop within 2000 ms of being killed")
        );
    }
    #[test]
    fn completed_journal_wakes_watcher_while_host_is_alive() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let pid = std::process::id();
        let journal = Journal {
            pid,
            started: identity(pid)?.unwrap(),
            ..Default::default()
        };
        butterpollo_core::state::write_json(
            &directory.path().join("display-recovery.json"),
            &journal,
        )?;
        let done = watch_done(pid)?;
        // SAFETY: `done` owns the event while the watcher opens and waits for it.
        unsafe { ResetEvent(HANDLE(done.as_raw_handle()))? };
        let path = directory.path().to_owned();
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(wait_and_recover(pid, &path));
        });
        // SAFETY: `done` still owns this handle. A signal before the wait must be retained.
        unsafe { SetEvent(HANDLE(done.as_raw_handle()))? };
        received.recv_timeout(std::time::Duration::from_secs(2))??;
        let after: Journal = serde_json::from_slice(&std::fs::read(
            directory.path().join("display-recovery.json"),
        )?)?;
        assert_eq!(after.pid, pid);
        Ok(())
    }
    #[test]
    fn a_kept_entry_gives_its_originals_until_released() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("display-recovery.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "pid": 0,
                "started": 0,
                "entries": {"tv": {
                    "output": r"\\.\DISPLAY2",
                    "mode": null,
                    "hdr": [false, true],
                    "mode_rate": [[3840, 2160, 60000], [1920, 1080, 120000]],
                    "profile": [null, "hdr.icm", false],
                    "rotation": [1, 2],
                    "scale": [100, 150],
                }},
            }))?,
        )?;
        assert_eq!(
            pending_in(&path, "tv")?,
            Pending {
                hdr: Some(false),
                mode: Some(((3840, 2160, 60000), (1920, 1080, 120000))),
                profile: Some(None),
                rotation: Some(1),
                scale: Some(100),
            }
        );
        assert_eq!(pending_in(&path, "monitor")?, Pending::default());
        // Releasing the display settings keeps the colour profile's entry.
        release_in(&path, "tv")?;
        assert_eq!(
            pending_in(&path, "tv")?,
            Pending {
                profile: Some(None),
                ..Default::default()
            }
        );
        Ok(())
    }
    #[test]
    fn exited_process_handles_do_not_keep_recovery_journals_owned() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/C", "exit", "0"])
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // Child still retains its process handle here, just like SCM/watchers.
        assert_eq!(identity(pid).unwrap(), None);
        assert!(identity(std::process::id()).unwrap().is_some());
    }
}
