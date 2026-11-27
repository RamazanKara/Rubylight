//! Durable display restoration after an abort, crash or service termination.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(test)]
use std::{os::windows::io::AsRawHandle, process::Command};
#[cfg(test)]
use windows::Win32::{Foundation::*, System::Threading::*};

mod dpi;
mod dpi_settle;
mod hooks;
mod restore;
mod watcher;

pub use dpi::{DpiBaseline, settle_scale, while_leasing};
pub use hooks::{RecoveryHooks, initialize_with, set_hooks, wait_and_recover_with};
use restore::recover;
#[cfg(test)]
use restore::{restore_mode_needed, restore_scale_needed, restored, restored_all};
pub use watcher::wait_and_recover;
#[cfg(test)]
use watcher::{KILLED_WATCH_EXIT_MS, stop_watch, wait_for_watch, watch_done};
use watcher::{finish_watch, identity, lock, watch};

type Mode = (u32, u32, u32);
#[derive(Clone, Default, Serialize, Deserialize)]
struct Entry {
    output: String,
    mode: Option<(Mode, Mode)>,
    #[serde(default)]
    mode_rate: Option<(Mode, Mode)>,
    /// The recorded `mode_rate` applied value is the mode requested before the
    /// apply, not the one the display actually took. Recovery restores the
    /// original regardless for such an entry: after a crash it cannot tell
    /// which mode actually landed.
    #[serde(default)]
    mode_rate_requested: bool,
    hdr: Option<(bool, bool)>,
    #[serde(default)]
    position: Option<(
        butterpollo_media_policy::topology::Position,
        butterpollo_media_policy::topology::Position,
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
        Vec<butterpollo_media_policy::topology::Node>,
    )>,
    #[serde(default)]
    baseline: Option<(crate::display::Snapshot, Vec<String>)>,
    #[serde(default)]
    activation: Option<(
        crate::display::Snapshot,
        Vec<butterpollo_media_policy::topology::Node>,
    )>,
    /// A `DISPLAYCONFIG_ROTATION`, (original, applied).
    #[serde(default)]
    rotation: Option<(u32, u32)>,
    /// The Windows scale in percent, (original, applied).
    #[serde(default)]
    scale: Option<(u32, u32)>,
    /// The recorded `scale` applied value is the one requested before the
    /// apply: after a crash, recovery puts the original back whenever the
    /// display shows anything else, as it cannot tell what landed.
    #[serde(default)]
    scale_requested: bool,
    /// Every physical display's Windows scale (device id to percent) from
    /// before a virtual display joined the desktop ([`DpiBaseline`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dpi_baseline: Option<BTreeMap<String, u32>>,
    /// Inherited from a prior crashed host while this process already started streaming.
    #[serde(default, skip_serializing_if = "is_false")]
    stale: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Entry {
    /// What recovery keeps until each restore succeeds. The arrangement is
    /// restored before this copy is taken and re-added only if it failed.
    fn display_settings(&self) -> Self {
        Self {
            output: self.output.clone(),
            profile: self.profile.clone(),
            hdr: self.hdr,
            mode: self.mode,
            mode_rate: self.mode_rate,
            mode_rate_requested: self.mode_rate_requested,
            position: self.position,
            external: self.external,
            audio: self.audio,
            microphone: self.microphone,
            baseline: self.baseline.clone(),
            activation: self.activation.clone(),
            rotation: self.rotation,
            scale: self.scale,
            scale_requested: self.scale_requested,
            dpi_baseline: self.dpi_baseline.clone(),
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
            || self.dpi_baseline.as_ref().is_some_and(|b| !b.is_empty())
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

#[derive(Clone, Default, Serialize, Deserialize)]
struct Journal {
    pid: u32,
    started: u64,
    entries: BTreeMap<String, Entry>,
}

static PATH: OnceLock<PathBuf> = OnceLock::new();
static PENDING_STARTUP: AtomicBool = AtomicBool::new(false);

fn quarantine_corrupt(path: &Path) -> PathBuf {
    let quarantine = path.with_extension("corrupt.json");
    let _ = std::fs::remove_file(&quarantine);
    if std::fs::rename(path, &quarantine).is_err() {
        let _ = std::fs::remove_file(path);
    }
    quarantine
}

fn read_journal(path: &Path) -> Result<Journal> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Take the journal and undo what an interrupted host left behind. An
/// incomplete recovery or an unreadable journal (quarantined to
/// `display-recovery.corrupt.json`) is returned for logging rather than stopping
/// host startup.
pub fn initialize(directory: &Path) -> Result<Option<anyhow::Error>> {
    let path = directory.join("display-recovery.json");
    let incomplete = initialize_in(&path, recover)?;
    PATH.set(path)
        .map_err(|_| anyhow::anyhow!("display recovery already initialized"))?;
    Ok(incomplete)
}

fn initialize_in(
    path: &Path,
    recover_fn: impl FnOnce(&Path) -> Result<()>,
) -> Result<Option<anyhow::Error>> {
    let _guard = lock(path)?;
    let mut incomplete = None;
    if path.exists() {
        match read_journal(path) {
            Ok(journal) => {
                if identity(journal.pid)? == Some(journal.started)
                    && journal.pid != std::process::id()
                {
                    bail!("another host owns this display recovery journal");
                }
                incomplete = recover_fn(path).err();
            }
            Err(error) => {
                let quarantine = quarantine_corrupt(path);
                tracing::warn!(
                    path = %path.display(),
                    quarantined = %quarantine.display(),
                    error = %format!("{error:#}"),
                    "display recovery journal was unreadable; quarantined so the host can start"
                );
                incomplete = Some(anyhow::anyhow!(
                    "quarantined unreadable display recovery journal ({}): {error:#}",
                    quarantine.display()
                ));
            }
        }
    }
    PENDING_STARTUP.store(incomplete.is_some() && path.exists(), Ordering::Release);
    Ok(incomplete)
}

/// Whether startup recovery left unfinished display restoration from an earlier
/// interrupted session.
pub fn has_pending() -> bool {
    PENDING_STARTUP.load(Ordering::Acquire)
}

/// Retry unfinished startup display recovery, if any entries remain from a
/// previous host. Returns `Ok(true)` when leftover entries were restored,
/// `Ok(false)` when nothing remains or only live entries of this host exist.
pub fn recover_pending() -> Result<bool> {
    if !has_pending() {
        return Ok(false);
    }
    let Some(path) = PATH.get() else {
        PENDING_STARTUP.store(false, Ordering::Release);
        return Ok(false);
    };
    let outcome = recover_pending_at(path, recover);
    if outcome.is_ok() {
        PENDING_STARTUP.store(false, Ordering::Release);
    }
    outcome
}

fn recover_pending_at(path: &Path, recover_fn: impl FnOnce(&Path) -> Result<()>) -> Result<bool> {
    let _guard = lock(path)?;
    if !path.exists() {
        return Ok(false);
    }
    let journal = match read_journal(path) {
        Ok(journal) => journal,
        Err(error) => {
            let quarantine = quarantine_corrupt(path);
            tracing::warn!(
                quarantined = %quarantine.display(),
                error = %format!("{error:#}"),
                "quarantined unreadable display recovery journal during pending retry"
            );
            return Ok(false);
        }
    };
    let current = std::process::id();
    let has_stale = journal.entries.values().any(|entry| entry.stale);
    if journal.pid == current && !has_stale {
        // Every entry belongs to a live stream or monitor lease in this host.
        return Ok(false);
    }
    if journal.pid != 0 && journal.pid != current && identity(journal.pid)? == Some(journal.started)
    {
        anyhow::bail!("another host owns this display recovery journal");
    }
    recover_fn(path)?;
    Ok(true)
}

fn change(id: &str, output: &str, update: impl FnOnce(&mut Entry)) -> Result<()> {
    change_journal(|journal| {
        let entry = journal.entries.entry(id.into()).or_default();
        entry.output = output.into();
        entry.stale = false;
        update(entry);
    })
}

/// [`change`] for an update that spans entries.
fn change_journal(update: impl FnOnce(&mut Journal)) -> Result<()> {
    let path = PATH.get().context("display recovery is not initialized")?;
    let guard = lock(path)?;
    let current_pid = std::process::id();
    let mut journal: Journal = if path.exists() {
        match read_journal(path) {
            Ok(mut existing) => {
                // Keep un-recovered entries from a prior host marked stale so `recover_pending`
                // still restores them after this host writes its own PID.
                if existing.pid != current_pid && !existing.entries.is_empty() {
                    for leftover in existing.entries.values_mut() {
                        leftover.stale = true;
                    }
                }
                existing
            }
            Err(error) => {
                let quarantine = quarantine_corrupt(path);
                tracing::warn!(
                    quarantined = %quarantine.display(),
                    error = %format!("{error:#}"),
                    "quarantined unreadable display recovery journal before recording change"
                );
                Journal::default()
            }
        }
    } else {
        Journal::default()
    };
    let prior = journal.clone();
    journal.pid = current_pid;
    journal.started = identity(journal.pid)?.context("host identity unavailable")?;
    update(&mut journal);
    journal.entries.retain(|_, entry| entry.pending());
    butterpollo_session::state::write_json(path, &journal)?;
    if journal.entries.is_empty() {
        drop(guard);
        return finish_watch(path);
    }
    if let Err(e) = watch(path) {
        butterpollo_session::state::write_json(path, &prior)?;
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
        ));
        e.mode_rate_requested = false;
    })
}

/// Journal the mode a stream is about to apply, before the apply. A crash
/// after this but before the mode the display actually took is recorded
/// leaves this requested value behind; recovery restores the original then
/// rather than requiring an exact match, which it cannot.
pub fn mode_rate_requested(id: &str, output: &str, before: Mode, requested: Mode) -> Result<()> {
    change(id, output, |e| {
        e.mode_rate = Some((
            e.mode_rate.map_or(before, |(original, _)| original),
            requested,
        ));
        e.mode_rate_requested = true;
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

/// A display's Windows scale, in percent, as it landed.
pub fn scale(id: &str, output: &str, before: u32, applied: u32) -> Result<()> {
    scale_entry(id, output, before, applied, false)
}

/// Journal the scale a stream is about to apply, before the apply; [`scale`]
/// records what landed afterwards.
pub fn scale_requested(id: &str, output: &str, before: u32, requested: u32) -> Result<()> {
    scale_entry(id, output, before, requested, true)
}

/// A stream that leases a display's scale restores it itself, so the display
/// leaves the physical baseline and is not set twice.
fn scale_entry(id: &str, output: &str, before: u32, applied: u32, requested: bool) -> Result<()> {
    change_journal(|journal| {
        dpi::forget(journal, id);
        let e = journal.entries.entry(id.into()).or_default();
        e.output = output.into();
        e.stale = false;
        e.scale = Some((e.scale.map_or(before, |(original, _)| original), applied));
        e.scale_requested = requested;
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
        Vec<butterpollo_media_policy::topology::Node>,
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
        Vec<butterpollo_media_policy::topology::Node>,
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
        entry.mode_rate_requested = false;
        entry.hdr = None;
        entry.rotation = None;
        entry.scale = None;
        entry.scale_requested = false;
    }
    journal.entries.retain(|_, e| e.pending());
    butterpollo_session::state::write_json(path, &journal)
}

pub fn position(
    id: &str,
    output: &str,
    before: butterpollo_media_policy::topology::Position,
    applied: butterpollo_media_policy::topology::Position,
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
    butterpollo_session::state::write_json(path, &journal)?;
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
    butterpollo_session::state::write_json(path, &journal)?;
    drop(guard);
    finish_watch(path)
}

#[cfg(test)]
mod tests;
