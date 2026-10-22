//! Writes, the journal and restore for Radeon settings (plan A5; the rules are in
//! `docs/native-api.md`, "Radeon state and writes").
//!
//! A write request is validated whole, its originals are journaled and synced to disk, then it is
//! applied in order. If the driver refuses one change, everything the request did is undone. The
//! driver's read-back is what comes back to the caller. Restore puts a journaled setting back only
//! while it still holds the value the host wrote.
//!
//! `RadeonBackend` is the seam to ADLX: `adlx::AdlxBackend` is the real one, and the tests use a
//! fake, so all of this runs without a GPU.
use super::{
    model::{self, Kind, Setting, State, Value},
    owner::{self, Owner},
};
use anyhow::{Context, Result, bail};
use butterpollo_core::state;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    time::Instant,
};

/// The journal's file name inside the profile (config) directory.
pub const JOURNAL_FILE: &str = "radeon-journal.json";

/// What `Controller` needs from the driver.
pub trait RadeonBackend {
    /// All settings of one GPU, freshly read from the driver.
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError>;
    /// The current value of one setting, freshly read.
    fn read_value(&mut self, gpu: i32, id: &str) -> Result<Value, BackendError> {
        let settings = self.read(gpu)?;
        find(&settings, id)
            .and_then(|s| s.value.clone())
            .ok_or_else(|| BackendError(format!("the driver does not report {id}")))
    }
    /// Write one value. The caller has validated it.
    fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), BackendError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError(pub String);
impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The error codes of `POST /radeon/settings`, plus the two the host adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Unknown id, wrong type, outside the range or off the step.
    InvalidValue,
    /// The setting is not `configurable`.
    NotWritable,
    /// The driver refused a write or kept another value. Everything was undone.
    DriverRejected,
    /// ADLX could not be read.
    Unavailable,
    /// The journal could not be read or written, so nothing was changed.
    Journal,
}
impl ErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidValue => "invalid_value",
            Self::NotWritable => "not_writable",
            Self::DriverRejected => "driver_rejected",
            Self::Unavailable => "radeon_unavailable",
            Self::Journal => "journal_error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteError {
    pub kind: ErrorKind,
    /// The setting the error names.
    pub id: Option<String>,
    pub message: String,
}
impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.id {
            Some(id) => write!(f, "{} ({id}): {}", self.kind.code(), self.message),
            None => write!(f, "{}: {}", self.kind.code(), self.message),
        }
    }
}
impl std::error::Error for WriteError {}
fn error(kind: ErrorKind, id: Option<&str>, message: impl Into<String>) -> WriteError {
    WriteError {
        kind,
        id: id.map(String::from),
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Change {
    pub id: String,
    pub value: Value,
}

/// One setting the host changed, with the value to go back to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub gpu: i32,
    pub id: String,
    /// The session that last wrote it.
    pub session: String,
    /// The value before the first write of this journal's life, kept across later writes.
    pub original: Value,
    /// The value the host left the setting at; restore only acts while it still holds this.
    pub written: Value,
    /// The driver changed it as a consequence of another write (Chill turning Anti-Lag off).
    #[serde(default)]
    pub side_effect: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub version: u32,
    /// The host that last wrote the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    #[serde(default)]
    pub entries: Vec<Entry>,
}
impl Journal {
    /// A missing file is an empty journal. An unreadable one is an error: the originals in it
    /// must not be silently forgotten.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let Some(json) = state::json_bytes(&bytes) else {
                    return Ok(Self::default());
                };
                serde_json::from_slice(json)
                    .with_context(|| format!("invalid Radeon journal {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
    /// Atomic, and synced before it returns. Stamps this process as the owner.
    pub fn save(&self, path: &Path) -> Result<()> {
        let journal = Self {
            version: 1,
            owner: Some(Owner::current()?),
            entries: self.entries.clone(),
        };
        state::write_json(path, &journal)
    }
    /// Keeps the first original per setting: a second write only moves `written`. If the setting
    /// no longer holds what the host wrote (`entry.original` is its value now), someone changed
    /// it on the PC, that change wins and becomes the value to go back to.
    fn record(&mut self, entry: Entry) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.gpu == entry.gpu && e.id == entry.id)
        {
            Some(existing) => {
                if existing.written != entry.original {
                    existing.original = entry.original;
                }
                existing.written = entry.written;
                existing.session = entry.session;
                existing.side_effect &= entry.side_effect;
            }
            None => self.entries.push(entry),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    /// Every entry, for a host start after a crash.
    All,
    Session(&'a str),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RestoreResult {
    Restored,
    /// The setting already holds the original, for example because the crash came before the write.
    AlreadyOriginal,
    /// Someone changed it on the PC since: the local change wins and the entry is dropped.
    ChangedElsewhere {
        current: Value,
        written: Value,
    },
    /// Still journaled. The next restore tries again.
    Failed(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub gpu: i32,
    pub id: String,
    pub result: RestoreResult,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestoreReport {
    pub outcomes: Vec<Outcome>,
}
impl RestoreReport {
    pub fn failed(&self) -> impl Iterator<Item = &Outcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.result, RestoreResult::Failed(_)))
    }
    pub fn is_clean(&self) -> bool {
        self.failed().next().is_none()
    }
}

pub struct Controller<B> {
    backend: B,
    journal: PathBuf,
    /// Restore stops starting new work after this; what is left stays journaled.
    deadline: Option<Instant>,
}

fn find<'a>(settings: &'a [Setting], id: &str) -> Option<&'a Setting> {
    settings.iter().find(|s| s.id == id)
}

fn is_on(settings: &[Setting], id: &str) -> bool {
    matches!(
        find(settings, id).and_then(|s| s.value.as_ref()),
        Some(Value::Bool(true))
    )
}

/// Checks the whole request against the settings as they are now, before anything is written.
fn validate(settings: &[Setting], changes: &[Change]) -> Result<(), WriteError> {
    if changes.is_empty() {
        return Err(error(ErrorKind::InvalidValue, None, "no changes"));
    }
    let mut seen = HashSet::new();
    // Toggles as the request leaves them, in order.
    let mut toggles: HashMap<String, bool> = HashMap::new();
    for change in changes {
        let id = change.id.as_str();
        let invalid = |message: String| error(ErrorKind::InvalidValue, Some(id), message);
        if !seen.insert(id) {
            return Err(invalid("listed twice".into()));
        }
        let Some(setting) = find(settings, id) else {
            return Err(invalid("unknown setting".into()));
        };
        if setting.state != State::Configurable || setting.kind == Kind::Action {
            return Err(error(
                ErrorKind::NotWritable,
                Some(id),
                format!("the setting is {}", setting.state.name()),
            ));
        }
        match (setting.kind, &change.value) {
            (Kind::Toggle, Value::Bool(_)) => {}
            (Kind::Integer, Value::Int(value)) => {
                let range = setting
                    .range
                    .ok_or_else(|| invalid("the driver reports no range".into()))?;
                if *value < range.min || *value > range.max {
                    return Err(invalid(format!(
                        "{value} is outside {}..{}",
                        range.min, range.max
                    )));
                }
                if (value - range.min) % range.step != 0 {
                    return Err(invalid(format!(
                        "{value} is not on the step of {} from {}",
                        range.step, range.min
                    )));
                }
            }
            (Kind::Choice, Value::Choice(name)) => {
                if !setting.options.iter().any(|o| &o.value == name) {
                    return Err(invalid(format!("{name} is not one of the options")));
                }
            }
            (kind, _) => return Err(invalid(format!("expected a {kind:?} value").to_lowercase())),
        }
        // The driver only takes some values while their feature is on.
        if let Some(toggle) = model::feature_toggle(id) {
            let on = toggles
                .get(&toggle)
                .copied()
                .unwrap_or_else(|| is_on(settings, &toggle));
            if !on {
                return Err(invalid(format!(
                    "the driver only accepts it while {toggle} is on; turn that on first or earlier in the same request"
                )));
            }
        }
        if let (Some(group), Value::Bool(on)) = (id.strip_suffix(".enabled"), &change.value) {
            toggles.insert(id.to_string(), *on);
            // Turning a feature on can switch an excluded one off. If the host can neither write
            // nor restore that one, the request would leave the PC changed for good.
            for other in model::excluded_by(group).filter(|_| *on) {
                let partner = format!("{other}.enabled");
                let Some(partner) = find(settings, &partner) else {
                    continue;
                };
                let derived = model::derived_from(partner.id)
                    .and_then(|from| find(settings, from))
                    .is_some_and(|from| from.state == State::Configurable);
                if partner.state != State::Configurable && is_on(settings, partner.id) && !derived
                {
                    return Err(invalid(format!(
                        "turning it on can switch {} off, and the host cannot write or restore that setting ({})",
                        partner.id,
                        partner.state.name()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The toggles a request is expected to make the driver switch off: each configurable
/// `<other>.enabled` that is on now and that a requested `<group>.enabled = true` excludes.
fn predicted_effects(before: &[Setting], changes: &[Change]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for change in changes {
        let (Some(group), Value::Bool(true)) = (change.id.strip_suffix(".enabled"), &change.value)
        else {
            continue;
        };
        if is_on(before, &change.id) {
            continue;
        }
        for other in model::excluded_by(group) {
            let partner = format!("{other}.enabled");
            let configurable =
                find(before, &partner).is_some_and(|s| s.state == State::Configurable);
            if configurable
                && is_on(before, &partner)
                && !changes.iter().any(|c| c.id == partner)
                && !out.contains(&partner)
            {
                out.push(partner);
            }
        }
    }
    out
}

impl<B: RadeonBackend> Controller<B> {
    pub fn new(backend: B, journal: PathBuf) -> Self {
        Self {
            backend,
            journal,
            deadline: None,
        }
    }
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }
    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn journal(&self) -> Result<Journal> {
        Journal::load(&self.journal)
    }

    /// Take the journal's mutex and load it, unless another live host owns settings in it. The
    /// lock stays held until the guard drops, so load, change and save are one step.
    fn open(&self) -> Result<(owner::JournalLock, Journal)> {
        let lock = owner::lock(&self.journal)?;
        let journal = Journal::load(&self.journal)?;
        if let Some(other) = journal.owner
            && !journal.entries.is_empty()
            && other.is_other_and_alive()
        {
            bail!(
                "another host (process {}) owns the Radeon journal and has settings changed",
                other.pid
            );
        }
        Ok((lock, journal))
    }

    /// Validate, journal, apply in order, and return the driver's read-back. Either all of
    /// `changes` took effect or none did.
    pub fn apply(
        &mut self,
        session: &str,
        gpu: i32,
        changes: &[Change],
    ) -> Result<Vec<Setting>, WriteError> {
        let unavailable = |e: BackendError| error(ErrorKind::Unavailable, None, e.0);
        let journal_error = |e: anyhow::Error| error(ErrorKind::Journal, None, format!("{e:#}"));
        let (_lock, saved) = self.open().map_err(journal_error)?;
        let before = self.backend.read(gpu).map_err(unavailable)?;
        validate(&before, changes)?;
        let mut journal = saved.clone();
        let original = |id: &str| find(&before, id).and_then(|s| s.value.clone());
        for change in changes {
            journal.record(Entry {
                gpu,
                id: change.id.clone(),
                session: session.into(),
                original: original(&change.id).ok_or_else(|| {
                    error(
                        ErrorKind::Unavailable,
                        Some(&change.id),
                        "the value cannot be read",
                    )
                })?,
                written: change.value.clone(),
                side_effect: false,
            });
        }
        // The partners the driver is expected to switch off go in too. If it does not switch one,
        // the entry is harmless: restore finds it already at its original.
        for partner in predicted_effects(&before, changes) {
            journal.record(Entry {
                gpu,
                id: partner,
                session: session.into(),
                original: Value::Bool(true),
                written: Value::Bool(false),
                side_effect: true,
            });
        }
        // The originals reach the disk before the first byte changes in the driver.
        journal.save(&self.journal).map_err(journal_error)?;

        let mut applied: Vec<&Change> = vec![];
        for change in changes {
            if let Err(e) = self.backend.write(gpu, &change.id, &change.value) {
                let failure = error(ErrorKind::DriverRejected, Some(&change.id), e.0);
                return Err(self.undo(session, gpu, &before, changes, &applied, &saved, failure));
            }
            applied.push(change);
        }
        let after = match self.backend.read(gpu) {
            Ok(after) => after,
            Err(e) => {
                let failure = error(ErrorKind::Unavailable, None, e.0);
                return Err(self.undo(session, gpu, &before, changes, &applied, &saved, failure));
            }
        };
        for change in changes {
            let now = find(&after, &change.id).and_then(|s| s.value.as_ref());
            if now != Some(&change.value) {
                let failure = error(
                    ErrorKind::DriverRejected,
                    Some(&change.id),
                    format!(
                        "wrote {} and the driver reads back {}",
                        change.value.display(),
                        now.map_or("nothing".into(), Value::display)
                    ),
                );
                return Err(self.undo(session, gpu, &before, changes, &applied, &saved, failure));
            }
        }
        // A setting the host can neither write nor restore must not have moved, except next to
        // what the request itself changed (vsync.enabled follows vsync.mode).
        let touched: HashSet<&str> = changes
            .iter()
            .filter_map(|c| find(&before, &c.id).map(|s| s.group))
            .collect();
        for setting in &before {
            if setting.state == State::Configurable || touched.contains(setting.group) {
                continue;
            }
            let now = find(&after, setting.id).and_then(|s| s.value.as_ref());
            if let (Some(was), Some(now)) = (&setting.value, now)
                && was != now
                && model::derived_from(setting.id).is_none()
            {
                let failure = error(
                    ErrorKind::DriverRejected,
                    Some(setting.id),
                    format!(
                        "the request switched {} from {} to {}, and the host cannot write or restore it",
                        setting.id,
                        was.display(),
                        now.display()
                    ),
                );
                return Err(self.undo(session, gpu, &before, changes, &applied, &saved, failure));
            }
        }
        // What the driver changed on its own is journaled too, so restore can undo it.
        let requested: HashSet<&str> = changes.iter().map(|c| c.id.as_str()).collect();
        let mut effects = false;
        for setting in &before {
            if setting.state != State::Configurable || requested.contains(setting.id) {
                continue;
            }
            let (Some(was), Some(now)) = (
                &setting.value,
                find(&after, setting.id).and_then(|s| s.value.as_ref()),
            ) else {
                continue;
            };
            if was != now {
                effects = true;
                journal.record(Entry {
                    gpu,
                    id: setting.id.into(),
                    session: session.into(),
                    original: was.clone(),
                    written: now.clone(),
                    side_effect: true,
                });
            }
        }
        if effects && let Err(e) = journal.save(&self.journal) {
            tracing::error!(
                error = format!("{e:#}"),
                "journal the driver's side effects; undoing the request"
            );
            let failure = error(
                ErrorKind::Journal,
                None,
                format!(
                    "the driver's side effects could not be journaled ({e:#}), so the request was undone"
                ),
            );
            return Err(self.undo(session, gpu, &before, changes, &applied, &saved, failure));
        }
        Ok(after)
    }

    /// Put every setting back to `before` after a failed request and say what it could not. What
    /// stays changed is journaled with the value it holds now, so a restore can still act on it.
    #[allow(clippy::too_many_arguments)]
    fn undo(
        &mut self,
        session: &str,
        gpu: i32,
        before: &[Setting],
        changes: &[Change],
        applied: &[&Change],
        saved: &Journal,
        mut failure: WriteError,
    ) -> WriteError {
        for change in applied.iter().rev() {
            if let Some(value) = find(before, &change.id).and_then(|s| s.value.as_ref()) {
                let _ = self.backend.write(gpu, &change.id, value);
            }
        }
        // The driver may have switched other settings as a consequence; those go back too.
        if let Ok(now) = self.backend.read(gpu) {
            for setting in before.iter().filter(|s| s.state == State::Configurable) {
                let current = find(&now, setting.id).and_then(|s| s.value.as_ref());
                if let (Some(was), Some(current)) = (&setting.value, current)
                    && was != current
                {
                    let _ = self.backend.write(gpu, setting.id, was);
                }
            }
        }
        match self.backend.read(gpu) {
            Ok(now) => {
                let stuck: Vec<&Setting> = before
                    .iter()
                    .filter(|s| s.state == State::Configurable)
                    .filter(|s| {
                        s.value.as_ref() != find(&now, s.id).and_then(|n| n.value.as_ref())
                    })
                    .collect();
                if stuck.is_empty() {
                    if let Err(e) = saved.save(&self.journal) {
                        tracing::error!(error = format!("{e:#}"), "reset the Radeon journal");
                    }
                    return failure;
                }
                let mut journal = saved.clone();
                for setting in &stuck {
                    let Some(was) = &setting.value else {
                        continue;
                    };
                    let requested = changes.iter().find(|c| c.id == setting.id);
                    let current = find(&now, setting.id)
                        .and_then(|n| n.value.clone())
                        .or_else(|| requested.map(|c| c.value.clone()))
                        .unwrap_or_else(|| was.clone());
                    journal.record(Entry {
                        gpu,
                        id: setting.id.into(),
                        session: session.into(),
                        original: was.clone(),
                        written: current,
                        side_effect: requested.is_none(),
                    });
                }
                failure.message = format!(
                    "{}; could not put back {}, which stays in the journal for the next restore",
                    failure.message,
                    stuck.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")
                );
                if let Err(e) = journal.save(&self.journal) {
                    tracing::error!(
                        error = format!("{e:#}"),
                        "journal the settings the undo could not put back"
                    );
                    failure.kind = ErrorKind::Journal;
                    failure.message = format!(
                        "{}; the journal could not be updated either ({e:#})",
                        failure.message
                    );
                }
            }
            Err(e) => {
                // Nothing to compare with: the journal on disk still holds every change of the
                // request, so a restore finds them.
                tracing::error!(error = %e, "Radeon undo could not read the driver back");
                failure.message = format!(
                    "{}; the driver could not be read to check the undo, so the request stays in the journal",
                    failure.message
                );
            }
        }
        failure
    }

    /// Put journaled settings back. A setting is only touched while it still holds the value the
    /// host wrote; anything else was changed on the PC and is left alone. Entries that fail stay
    /// in the journal.
    pub fn restore(&mut self, scope: Scope) -> Result<RestoreReport> {
        self.restore_entries(scope, false)
    }

    /// For a host start: restore whatever a crashed run left in the journal. A setting that reads
    /// as already original gets the original written once more, which changes nothing when the
    /// read was right and covers a write that had not become visible yet.
    pub fn recover_on_startup(&mut self) -> Result<RestoreReport> {
        self.restore_entries(Scope::All, true)
    }

    fn expired(&self) -> bool {
        self.deadline.is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn restore_entries(&mut self, scope: Scope, startup: bool) -> Result<RestoreReport> {
        let (_lock, mut journal) = self.open()?;
        let selected = |e: &Entry| match scope {
            Scope::All => true,
            Scope::Session(session) => e.session == session,
        };
        // Requested changes first, newest first; then what the driver switched as a consequence.
        let mut order: Vec<Entry> = journal
            .entries
            .iter()
            .filter(|e| selected(e))
            .cloned()
            .collect();
        order.reverse();
        order.sort_by_key(|e| e.side_effect);
        let mut results: Vec<(Entry, RestoreResult)> = vec![];
        let mut pending = order;
        for _ in 0..2 {
            let mut failed = vec![];
            for entry in pending {
                let result = if self.expired() {
                    RestoreResult::Failed("the restore time limit passed".into())
                } else {
                    self.restore_one(&mut journal, &entry, startup)
                };
                if matches!(result, RestoreResult::Failed(_)) {
                    failed.push(entry.clone());
                }
                results.retain(|(e, _)| !(e.gpu == entry.gpu && e.id == entry.id));
                results.push((entry, result));
            }
            pending = failed;
            if pending.is_empty() {
                break;
            }
        }
        let mut report = RestoreReport::default();
        let mut changed = false;
        for (entry, result) in results {
            match &result {
                RestoreResult::Failed(message) => tracing::error!(
                    setting = %entry.id,
                    original = %entry.original.display(),
                    error = %message,
                    "Radeon setting could not be restored; it stays in the journal"
                ),
                RestoreResult::ChangedElsewhere { current, written } => tracing::warn!(
                    setting = %entry.id,
                    original = %entry.original.display(),
                    current = %current.display(),
                    written = %written.display(),
                    "Radeon setting was changed on the PC since the host wrote it; leaving it"
                ),
                RestoreResult::Restored => tracing::info!(
                    setting = %entry.id,
                    value = %entry.original.display(),
                    "Radeon setting restored"
                ),
                RestoreResult::AlreadyOriginal => {}
            }
            if !matches!(result, RestoreResult::Failed(_)) {
                journal
                    .entries
                    .retain(|e| !(e.gpu == entry.gpu && e.id == entry.id));
                changed = true;
            }
            report.outcomes.push(Outcome {
                gpu: entry.gpu,
                id: entry.id,
                result,
            });
        }
        if changed {
            journal.save(&self.journal)?;
        }
        Ok(report)
    }

    fn restore_one(&mut self, journal: &mut Journal, entry: &Entry, startup: bool) -> RestoreResult {
        let now = match self.backend.read_value(entry.gpu, &entry.id) {
            Ok(now) => now,
            Err(e) => return RestoreResult::Failed(e.0),
        };
        if now == entry.original {
            if startup && let Err(e) = self.backend.write(entry.gpu, &entry.id, &entry.original) {
                // It reads as the original, and the write is only for safety.
                tracing::debug!(setting = %entry.id, error = %e, "startup write-back of the original");
            }
            return RestoreResult::AlreadyOriginal;
        }
        if now != entry.written {
            return RestoreResult::ChangedElsewhere {
                current: now,
                written: entry.written.clone(),
            };
        }
        if let Some(toggle) = model::feature_toggle(&entry.id) {
            match self.backend.read_value(entry.gpu, &toggle) {
                Ok(Value::Bool(false)) => return self.restore_through(journal, entry, &toggle),
                Ok(_) => {}
                Err(e) => return RestoreResult::Failed(e.0),
            }
        }
        match self.put_back(entry) {
            Ok(()) => RestoreResult::Restored,
            Err(message) => RestoreResult::Failed(message),
        }
    }

    /// Write the original and check the read-back.
    fn put_back(&mut self, entry: &Entry) -> Result<(), String> {
        self.backend
            .write(entry.gpu, &entry.id, &entry.original)
            .map_err(|e| e.0)?;
        let value = self
            .backend
            .read_value(entry.gpu, &entry.id)
            .map_err(|e| e.0)?;
        if value != entry.original {
            return Err(format!(
                "wrote {} and the driver reads back {}",
                entry.original.display(),
                value.display()
            ));
        }
        Ok(())
    }

    /// The driver takes the field's value only while its feature is on, and the feature is off
    /// now: switch it on (journaled first, with whatever that switches off), put the field back,
    /// then put the feature back.
    fn restore_through(
        &mut self,
        journal: &mut Journal,
        entry: &Entry,
        toggle: &str,
    ) -> RestoreResult {
        let group = toggle.strip_suffix(".enabled").unwrap_or(toggle);
        let partners: Vec<String> = model::excluded_by(group)
            .map(|other| format!("{other}.enabled"))
            .filter(|id| {
                matches!(
                    self.backend.read_value(entry.gpu, id),
                    Ok(Value::Bool(true))
                )
            })
            .collect();
        let mut temporary: Vec<String> = vec![];
        let wanted = std::iter::once((toggle.to_string(), false, true))
            .chain(partners.iter().map(|id| (id.clone(), true, false)));
        for (id, original, written) in wanted {
            if journal
                .entries
                .iter()
                .any(|e| e.gpu == entry.gpu && e.id == id)
            {
                continue;
            }
            temporary.push(id.clone());
            journal.entries.push(Entry {
                gpu: entry.gpu,
                id,
                session: entry.session.clone(),
                original: Value::Bool(original),
                written: Value::Bool(written),
                side_effect: true,
            });
        }
        let forget = |journal: &mut Journal| {
            journal
                .entries
                .retain(|e| !(e.gpu == entry.gpu && temporary.contains(&e.id)));
        };
        if let Err(e) = journal.save(&self.journal) {
            forget(journal);
            return RestoreResult::Failed(format!("journaling the temporary switch of {toggle}: {e:#}"));
        }
        if let Err(message) = self.through_toggle(entry, toggle, &partners) {
            // The temporary entries stay: the next restore puts the switches back.
            return RestoreResult::Failed(message);
        }
        forget(journal);
        if let Err(e) = journal.save(&self.journal) {
            tracing::warn!(error = format!("{e:#}"), "drop the temporary Radeon entries");
        }
        RestoreResult::Restored
    }

    fn through_toggle(
        &mut self,
        entry: &Entry,
        toggle: &str,
        partners: &[String],
    ) -> Result<(), String> {
        let gpu = entry.gpu;
        let switch = |backend: &mut B, id: &str, on: bool| {
            backend
                .write(gpu, id, &Value::Bool(on))
                .map_err(|e| format!("{id}: {e}"))
        };
        switch(&mut self.backend, toggle, true)?;
        self.put_back(entry)?;
        switch(&mut self.backend, toggle, false)?;
        for partner in partners {
            let now = self.backend.read_value(gpu, partner).map_err(|e| e.0)?;
            if now != Value::Bool(true) {
                switch(&mut self.backend, partner, true)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radeon::model::{ChoiceOption, Range};
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

    fn setting(id: &'static str, kind: Kind, value: Value) -> Setting {
        Setting {
            id,
            group: id.split('.').next().unwrap(),
            label: id,
            interface: "fake",
            kind,
            state: State::Configurable,
            supported: Some(true),
            readable: Some(true),
            writable: crate::radeon::model::Writable::Yes,
            value: Some(value),
            unit: None,
            range: None,
            options: vec![],
            depends: vec![],
            notes: vec![],
            adlx: BTreeMap::new(),
        }
    }
    fn toggle(id: &'static str, on: bool) -> Setting {
        setting(id, Kind::Toggle, Value::Bool(on))
    }
    fn integer(id: &'static str, value: i64) -> Setting {
        let mut s = setting(id, Kind::Integer, Value::Int(value));
        s.range = Some(Range {
            min: 30,
            max: 300,
            step: 10,
        });
        s
    }
    fn choice(id: &'static str, value: &str) -> Setting {
        let mut s = setting(id, Kind::Choice, Value::Choice(value.into()));
        s.options = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(raw, name)| ChoiceOption {
                value: (*name).into(),
                raw: raw as i32,
            })
            .collect();
        s
    }
    fn on(v: bool) -> Value {
        Value::Bool(v)
    }
    fn change(id: &str, value: Value) -> Change {
        Change {
            id: id.into(),
            value,
        }
    }

    #[derive(Default)]
    struct Driver {
        settings: Vec<Setting>,
        log: Vec<String>,
        /// Refuse a write of this id.
        refuse: Option<&'static str>,
        /// Ignore writes of this id (the read-back keeps the old value).
        ignore: Option<&'static str>,
        /// Writing `.0` to true also sets `.1` to false.
        excludes: Vec<(&'static str, &'static str)>,
        /// The journal file, to see what was on disk when a write arrived.
        journal: Option<PathBuf>,
        journal_at_first_write: Option<Journal>,
        unreadable: bool,
        /// The next reads show this id with this older value, as a long-lived process sees a
        /// setting another process wrote: (id, stale value, reads left).
        stale: Option<(&'static str, Value, usize)>,
        /// Writing `.0` fails while the toggle `.1` is off.
        needs_on: Vec<(&'static str, &'static str)>,
        /// Writes of this id after the first n are silently dropped.
        ignore_from: Option<(&'static str, usize)>,
        /// Writing this id turns the journal path into a directory, so the next save fails.
        break_journal_at: Option<&'static str>,
        writes_of: HashMap<String, usize>,
    }
    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Driver>>);
    impl Fake {
        fn with(settings: Vec<Setting>) -> Self {
            let fake = Self::default();
            fake.0.borrow_mut().settings = settings;
            fake
        }
        fn value(&self, id: &str) -> Value {
            find(&self.0.borrow().settings, id)
                .unwrap()
                .value
                .clone()
                .unwrap()
        }
        fn set(&self, id: &str, value: Value) {
            let mut d = self.0.borrow_mut();
            d.settings.iter_mut().find(|s| s.id == id).unwrap().value = Some(value);
        }
        fn writes(&self) -> Vec<String> {
            self.0.borrow().log.clone()
        }
    }
    impl RadeonBackend for Fake {
        fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
            let mut d = self.0.borrow_mut();
            if d.unreadable {
                return Err(BackendError("ADLX_FAIL".into()));
            }
            let mut settings = d.settings.clone();
            if let Some((id, value, left)) = d.stale.take() {
                if let Some(s) = settings.iter_mut().find(|s| s.id == id) {
                    s.value = Some(value.clone());
                }
                if left > 1 {
                    d.stale = Some((id, value, left - 1));
                }
            }
            Ok(settings)
        }
        fn write(&mut self, _gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
            let mut d = self.0.borrow_mut();
            if d.journal_at_first_write.is_none()
                && let Some(path) = d.journal.clone()
            {
                d.journal_at_first_write = Journal::load(&path).ok();
            }
            d.log.push(format!("{id}={}", value.display()));
            let count = d.writes_of.entry(id.to_string()).or_default();
            *count += 1;
            let count = *count;
            if d.break_journal_at == Some(id)
                && let Some(path) = d.journal.clone()
            {
                let _ = std::fs::remove_file(&path);
                std::fs::create_dir_all(&path).unwrap();
            }
            if d.refuse == Some(id) {
                return Err(BackendError("ADLX_NOT_SUPPORTED".into()));
            }
            if d.ignore == Some(id) {
                return Ok(());
            }
            if d.ignore_from.is_some_and(|(ignored, n)| ignored == id && count > n) {
                return Ok(());
            }
            let toggle_off = d.needs_on.iter().any(|(field, toggle)| {
                *field == id
                    && find(&d.settings, toggle).and_then(|s| s.value.as_ref())
                        == Some(&Value::Bool(false))
            });
            if toggle_off {
                return Err(BackendError("ADLX_FAIL".into()));
            }
            let effects: Vec<&str> = d
                .excludes
                .iter()
                .filter(|(a, _)| *a == id && *value == Value::Bool(true))
                .map(|(_, b)| *b)
                .collect();
            for s in &mut d.settings {
                if s.id == id {
                    s.value = Some(value.clone());
                } else if effects.contains(&s.id) {
                    s.value = Some(Value::Bool(false));
                }
            }
            Ok(())
        }
    }

    fn controller(fake: &Fake) -> (Controller<Fake>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        fake.0.borrow_mut().journal = Some(path.clone());
        (Controller::new(fake.clone(), path), dir)
    }
    fn panel() -> Fake {
        Fake::with(vec![
            toggle("chill.enabled", false),
            integer("pacing.limit", 140),
            integer("chill.max_fps", 140),
            toggle("anti_lag.enabled", true),
            choice("tessellation.mode", "a"),
            {
                let mut s = toggle("rsr.enabled", false);
                s.state = State::ReadOnly;
                s
            },
        ])
    }

    #[test]
    fn invalid_requests_change_nothing_and_leave_no_journal() {
        let fake = panel();
        let (mut c, dir) = controller(&fake);
        let cases = [
            (change("nope", on(true)), ErrorKind::InvalidValue),
            (change("rsr.enabled", on(true)), ErrorKind::NotWritable),
            (
                change("chill.enabled", Value::Int(1)),
                ErrorKind::InvalidValue,
            ),
            (
                change("pacing.limit", Value::Int(20)),
                ErrorKind::InvalidValue,
            ),
            (
                change("pacing.limit", Value::Int(305)),
                ErrorKind::InvalidValue,
            ),
            (
                change("pacing.limit", Value::Int(145)),
                ErrorKind::InvalidValue,
            ),
            (
                change("tessellation.mode", Value::Choice("z".into())),
                ErrorKind::InvalidValue,
            ),
        ];
        for (bad, kind) in cases {
            // A valid change first: the whole request must fail.
            let request = [change("chill.enabled", on(true)), bad.clone()];
            let e = c.apply("s1", 1, &request).unwrap_err();
            assert_eq!(e.kind, kind, "{bad:?}");
            assert_eq!(e.id.as_deref(), Some(bad.id.as_str()));
        }
        let twice = [
            change("chill.enabled", on(true)),
            change("chill.enabled", on(false)),
        ];
        assert_eq!(
            c.apply("s1", 1, &twice).unwrap_err().kind,
            ErrorKind::InvalidValue
        );
        assert_eq!(
            c.apply("s1", 1, &[]).unwrap_err().kind,
            ErrorKind::InvalidValue
        );
        assert!(fake.writes().is_empty());
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn the_original_is_on_disk_before_the_first_write_and_the_readback_returns() {
        let fake = panel();
        let (mut c, dir) = controller(&fake);
        let after = c
            .apply(
                "s1",
                1,
                &[
                    change("pacing.limit", Value::Int(150)),
                    change("tessellation.mode", Value::Choice("c".into())),
                ],
            )
            .unwrap();
        assert_eq!(
            find(&after, "pacing.limit").unwrap().value,
            Some(Value::Int(150))
        );
        let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
        assert_eq!(
            seen.entries.len(),
            2,
            "both originals journaled before any write"
        );
        assert_eq!(seen.entries[0].original, Value::Int(140));
        assert_eq!(seen.entries[0].written, Value::Int(150));
        let on_disk = Journal::load(&dir.path().join(JOURNAL_FILE)).unwrap();
        assert_eq!(on_disk.entries[1].original, Value::Choice("a".into()));
        assert_eq!(fake.writes(), ["pacing.limit=150", "tessellation.mode=c"]);
    }

    #[test]
    fn the_first_original_is_kept_across_writes_in_a_session() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(160))])
            .unwrap();
        let journal = c.journal().unwrap();
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].original, Value::Int(140));
        assert_eq!(journal.entries[0].written, Value::Int(160));
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::Restored);
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_refused_change_undoes_the_ones_before_it() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().refuse = Some("tessellation.mode");
        let e = c
            .apply(
                "s1",
                1,
                &[
                    change("chill.enabled", on(true)),
                    change("pacing.limit", Value::Int(150)),
                    change("tessellation.mode", Value::Choice("b".into())),
                ],
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert_eq!(e.id.as_deref(), Some("tessellation.mode"));
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
        // The driver switched Anti-Lag off with Chill; undoing the request turned it back on.
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn undo_also_reverts_what_the_driver_switched_on_its_own() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
        fake.0.borrow_mut().refuse = Some("tessellation.mode");
        let e = c
            .apply(
                "s1",
                1,
                &[
                    change("chill.enabled", on(true)),
                    change("tessellation.mode", Value::Choice("b".into())),
                ],
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("anti_lag.enabled"), on(true));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_readback_that_differs_is_rejected_and_undone() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().ignore = Some("pacing.limit");
        let e = c
            .apply(
                "s1",
                1,
                &[
                    change("chill.enabled", on(true)),
                    change("pacing.limit", Value::Int(150)),
                ],
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert_eq!(e.id.as_deref(), Some("pacing.limit"));
        assert!(e.message.contains("reads back 140"), "{e}");
        assert_eq!(fake.value("chill.enabled"), on(false));
    }

    #[test]
    fn driver_side_effects_are_journaled_and_restored() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
        let after = c
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        assert_eq!(
            find(&after, "anti_lag.enabled").unwrap().value,
            Some(on(false))
        );
        let journal = c.journal().unwrap();
        let effect = journal
            .entries
            .iter()
            .find(|e| e.id == "anti_lag.enabled")
            .unwrap();
        assert!(effect.side_effect);
        assert_eq!((&effect.original, &effect.written), (&on(true), &on(false)));
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert!(report.is_clean());
        assert_eq!(fake.value("chill.enabled"), on(false));
        // Chill went off first, then Anti-Lag came back.
        assert_eq!(fake.value("anti_lag.enabled"), on(true));
        assert_eq!(fake.writes().last().unwrap(), "anti_lag.enabled=on");
    }

    #[test]
    fn restore_leaves_a_setting_that_was_changed_on_the_pc() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply(
            "s1",
            1,
            &[
                change("pacing.limit", Value::Int(150)),
                change("chill.enabled", on(true)),
            ],
        )
        .unwrap();
        fake.set("pacing.limit", Value::Int(200));
        let report = c.restore(Scope::Session("s1")).unwrap();
        let by_id = |id: &str| &report.outcomes.iter().find(|o| o.id == id).unwrap().result;
        assert_eq!(by_id("chill.enabled"), &RestoreResult::Restored);
        assert_eq!(
            by_id("pacing.limit"),
            &RestoreResult::ChangedElsewhere {
                current: Value::Int(200),
                written: Value::Int(150)
            }
        );
        assert_eq!(fake.value("pacing.limit"), Value::Int(200));
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert!(
            c.journal().unwrap().entries.is_empty(),
            "the local change wins and the entry goes"
        );
    }

    #[test]
    fn restore_drops_an_entry_whose_write_never_happened() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        // A crash between the journal and the write leaves the original in place.
        let mut journal = Journal::default();
        journal.record(Entry {
            gpu: 1,
            id: "pacing.limit".into(),
            session: "s1".into(),
            original: Value::Int(140),
            written: Value::Int(150),
            side_effect: false,
        });
        journal.save(&c.journal).unwrap();
        let report = c.recover_on_startup().unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::AlreadyOriginal);
        // Startup writes the original once more, which changes nothing here.
        assert_eq!(fake.writes(), ["pacing.limit=140"]);
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_failed_restore_stays_journaled_and_is_retried() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        fake.0.borrow_mut().refuse = Some("pacing.limit");
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert!(!report.is_clean());
        assert_eq!(c.journal().unwrap().entries.len(), 1);
        assert_eq!(fake.value("pacing.limit"), Value::Int(150));
        fake.0.borrow_mut().refuse = None;
        let report = c.recover_on_startup().unwrap();
        assert!(report.is_clean());
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn restore_by_session_leaves_the_other_sessions() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        c.apply(
            "s2",
            1,
            &[change("tessellation.mode", Value::Choice("b".into()))],
        )
        .unwrap();
        c.restore(Scope::Session("s2")).unwrap();
        assert_eq!(fake.value("tessellation.mode"), Value::Choice("a".into()));
        assert_eq!(fake.value("pacing.limit"), Value::Int(150));
        assert_eq!(c.journal().unwrap().entries.len(), 1);
    }

    #[test]
    fn a_crash_is_recovered_by_a_new_controller_from_the_journal() {
        let fake = panel();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        {
            let mut c = Controller::new(fake.clone(), path.clone());
            c.apply(
                "s1",
                1,
                &[
                    change("chill.enabled", on(true)),
                    change("pacing.limit", Value::Int(150)),
                ],
            )
            .unwrap();
            // The process dies here: nothing restores, the controller is just gone.
        }
        assert_eq!(fake.value("pacing.limit"), Value::Int(150));
        let mut restarted = Controller::new(fake.clone(), path);
        let report = restarted.recover_on_startup().unwrap();
        assert!(report.is_clean());
        // Both requested changes, and Anti-Lag, which Chill is expected to switch off.
        assert_eq!(report.outcomes.len(), 3);
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
        assert!(restarted.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn startup_writes_the_original_back_even_when_the_read_says_it_is_there() {
        let fake = panel();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        Controller::new(fake.clone(), path.clone())
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        // The read still shows the original, though the write has landed.
        fake.0.borrow_mut().stale = Some(("chill.enabled", on(false), 1));
        let report = Controller::new(fake.clone(), path.clone())
            .recover_on_startup()
            .unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::AlreadyOriginal);
        assert_eq!(fake.value("chill.enabled"), on(false), "written back anyway");
        assert!(report.is_clean());
    }

    #[test]
    fn a_session_restore_does_not_write_what_reads_as_original() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        fake.set("pacing.limit", Value::Int(140));
        let writes = fake.writes().len();
        c.restore(Scope::Session("s1")).unwrap();
        assert_eq!(fake.writes().len(), writes);
    }

    #[test]
    fn an_unreadable_gpu_fails_a_restore_without_dropping_the_entry() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        fake.0.borrow_mut().unreadable = true;
        let report = c.recover_on_startup().unwrap();
        assert_eq!(report.failed().count(), 1);
        assert_eq!(c.journal().unwrap().entries.len(), 1);
    }

    #[test]
    fn a_corrupt_journal_is_an_error_not_an_empty_one() {
        let fake = panel();
        let (mut c, dir) = controller(&fake);
        std::fs::write(dir.path().join(JOURNAL_FILE), b"{ not json").unwrap();
        assert!(c.recover_on_startup().is_err());
        let e = c
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Journal);
        assert!(fake.writes().is_empty());
        // A blank file or a missing one holds nothing.
        std::fs::write(dir.path().join(JOURNAL_FILE), b"  \n").unwrap();
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn an_unreadable_driver_fails_the_request_before_anything_is_journaled() {
        let fake = panel();
        let (mut c, dir) = controller(&fake);
        fake.0.borrow_mut().unreadable = true;
        let e = c
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unavailable);
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    fn entry_of<'a>(journal: &'a Journal, id: &str) -> &'a Entry {
        journal.entries.iter().find(|e| e.id == id).unwrap()
    }

    #[test]
    fn a_partner_the_driver_will_switch_off_is_on_disk_before_the_first_write() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
        c.apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
        let effect = entry_of(&seen, "anti_lag.enabled");
        assert!(effect.side_effect);
        assert_eq!((&effect.original, &effect.written), (&on(true), &on(false)));
        assert_eq!(fake.value("anti_lag.enabled"), on(false));
    }

    #[test]
    fn a_partner_the_driver_leaves_alone_is_harmless_in_the_journal() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
        assert!(entry_of(&seen, "anti_lag.enabled").side_effect);
        let writes = fake.writes().len();
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert!(report.is_clean());
        assert_eq!(fake.value("anti_lag.enabled"), on(true));
        // Chill went back; Anti-Lag already was original and was not written.
        assert_eq!(fake.writes().len(), writes + 1);
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_side_effect_that_cannot_be_journaled_undoes_the_request() {
        let mut settings = panel().0.borrow().settings.clone();
        settings.push(toggle("surprise.enabled", true));
        let fake = Fake::with(settings);
        let (mut c, _dir) = controller(&fake);
        // Nothing in the dependency table predicts this one.
        fake.0.borrow_mut().excludes = vec![("chill.enabled", "surprise.enabled")];
        fake.0.borrow_mut().break_journal_at = Some("chill.enabled");
        let e = c
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Journal, "{e}");
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("surprise.enabled"), on(true));
    }

    fn stuck_request() -> [Change; 2] {
        [
            change("pacing.limit", Value::Int(150)),
            change("tessellation.mode", Value::Choice("b".into())),
        ]
    }

    #[test]
    fn a_stuck_undo_is_journaled_with_the_value_it_holds() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().refuse = Some("tessellation.mode");
        // The first write lands, the undo write of the same setting is dropped.
        fake.0.borrow_mut().ignore_from = Some(("pacing.limit", 1));
        let e = c.apply("s1", 1, &stuck_request()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert!(e.message.contains("pacing.limit"), "{e}");
        assert_eq!(fake.value("pacing.limit"), Value::Int(150));
        let journal = c.journal().unwrap();
        assert_eq!(journal.entries.len(), 1);
        let stuck = entry_of(&journal, "pacing.limit");
        assert_eq!(stuck.original, Value::Int(140));
        assert_eq!(stuck.written, Value::Int(150));
        assert_eq!(stuck.session, "s1");
        // Once the driver listens again a restore finds it.
        fake.0.borrow_mut().ignore_from = None;
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::Restored);
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    }

    #[test]
    fn a_stuck_undo_that_cannot_be_journaled_fails_loudly() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().refuse = Some("tessellation.mode");
        fake.0.borrow_mut().ignore_from = Some(("pacing.limit", 1));
        fake.0.borrow_mut().break_journal_at = Some("tessellation.mode");
        let e = c.apply("s1", 1, &stuck_request()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Journal);
        assert!(e.message.contains("pacing.limit"), "{e}");
        assert!(e.message.contains("journal could not be updated"), "{e}");
    }

    #[test]
    fn restore_switches_a_feature_on_for_its_field_and_puts_it_back() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().needs_on = vec![("chill.max_fps", "chill.enabled")];
        fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
        c.apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(true)),
                change("chill.max_fps", Value::Int(150)),
            ],
        )
        .unwrap();
        // Chill was turned off on the PC; the driver now refuses chill.max_fps.
        fake.set("chill.enabled", on(false));
        let writes = fake.writes().len();
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("anti_lag.enabled"), on(true));
        assert_eq!(
            fake.writes()[writes..3 + writes],
            ["chill.enabled=on", "chill.max_fps=140", "chill.enabled=off"]
        );
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn restore_journals_the_temporary_switch_before_it_writes() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        {
            let mut d = fake.0.borrow_mut();
            d.needs_on = vec![("chill.max_fps", "chill.enabled")];
            d.excludes = vec![("chill.enabled", "anti_lag.enabled")];
        }
        fake.set("chill.max_fps", Value::Int(150));
        let mut journal = Journal::default();
        journal.record(Entry {
            gpu: 1,
            id: "chill.max_fps".into(),
            session: "s1".into(),
            original: Value::Int(140),
            written: Value::Int(150),
            side_effect: false,
        });
        journal.save(&c.journal).unwrap();
        let report = c.restore(Scope::All).unwrap();
        assert!(report.is_clean(), "{report:?}");
        let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
        let toggle = entry_of(&seen, "chill.enabled");
        assert_eq!((&toggle.original, &toggle.written), (&on(false), &on(true)));
        let partner = entry_of(&seen, "anti_lag.enabled");
        assert_eq!((&partner.original, &partner.written), (&on(true), &on(false)));
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("anti_lag.enabled"), on(true));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_failed_restore_through_the_feature_leaves_the_temporary_entries() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.0.borrow_mut().needs_on = vec![("chill.max_fps", "chill.enabled")];
        fake.set("chill.max_fps", Value::Int(150));
        // The field write fails, so the feature stays on and the entries stay.
        fake.0.borrow_mut().refuse = Some("chill.max_fps");
        let mut journal = Journal::default();
        journal.record(Entry {
            gpu: 1,
            id: "chill.max_fps".into(),
            session: "s1".into(),
            original: Value::Int(140),
            written: Value::Int(150),
            side_effect: false,
        });
        journal.save(&c.journal).unwrap();
        let report = c.restore(Scope::All).unwrap();
        assert_eq!(report.failed().count(), 1);
        let journal = c.journal().unwrap();
        assert!(journal.entries.iter().any(|e| e.id == "chill.enabled"));
        fake.0.borrow_mut().refuse = None;
        let report = c.recover_on_startup().unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_field_that_needs_its_feature_on_is_refused_unless_it_is_on_by_then() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        let cases: [Vec<Change>; 2] = [
            vec![change("chill.max_fps", Value::Int(150))],
            vec![
                change("chill.max_fps", Value::Int(150)),
                change("chill.enabled", on(true)),
            ],
        ];
        for request in cases {
            let e = c.apply("s1", 1, &request).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidValue);
            assert_eq!(e.id.as_deref(), Some("chill.max_fps"));
            assert!(e.message.contains("chill.enabled"), "{e}");
        }
        assert!(fake.writes().is_empty());
        c.apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(true)),
                change("chill.max_fps", Value::Int(150)),
            ],
        )
        .unwrap();
        // On now, so the field alone is fine; switching it off first is not.
        c.apply("s1", 1, &[change("chill.max_fps", Value::Int(160))])
            .unwrap();
        let e = c
            .apply(
                "s1",
                1,
                &[
                    change("chill.enabled", on(false)),
                    change("chill.max_fps", Value::Int(170)),
                ],
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidValue);
    }

    fn read_only(id: &'static str, value: Value) -> Setting {
        let mut s = setting(id, Kind::Toggle, value);
        s.state = State::ReadOnly;
        s
    }

    #[test]
    fn enabling_a_group_that_excludes_a_setting_the_host_cannot_restore_is_refused() {
        let fake = Fake::with(vec![
            toggle("boost.enabled", false),
            read_only("rsr.enabled", on(true)),
        ]);
        let (mut c, dir) = controller(&fake);
        let e = c
            .apply("s1", 1, &[change("boost.enabled", on(true))])
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidValue);
        assert_eq!(e.id.as_deref(), Some("boost.enabled"));
        assert!(e.message.contains("rsr.enabled"), "{e}");
        assert!(fake.writes().is_empty());
        assert!(!dir.path().join(JOURNAL_FILE).exists());
        // Switching it off, or enabling it while RSR is off, is fine.
        fake.set("rsr.enabled", on(false));
        c.apply("s1", 1, &[change("boost.enabled", on(true))])
            .unwrap();
    }

    #[test]
    fn a_derived_read_only_partner_does_not_block_its_driving_setting() {
        let derived = |mode_state: State| {
            let mut mode = choice("vsync.mode", "a");
            mode.state = mode_state;
            Fake::with(vec![
                toggle("enhanced_sync.enabled", false),
                mode,
                read_only("vsync.enabled", on(true)),
            ])
        };
        // vsync.enabled follows vsync.mode, which the host can write and restore.
        let fake = derived(State::Configurable);
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("enhanced_sync.enabled", on(true))])
            .unwrap();
        let fake = derived(State::ReadOnly);
        let (mut c, _dir) = controller(&fake);
        let e = c
            .apply("s1", 1, &[change("enhanced_sync.enabled", on(true))])
            .unwrap_err();
        assert!(e.message.contains("vsync.enabled"), "{e}");
    }

    #[test]
    fn a_setting_the_host_cannot_write_that_moved_undoes_the_request() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        fake.set("rsr.enabled", on(true));
        // The dependency table does not know about this one.
        fake.0.borrow_mut().excludes = vec![("chill.enabled", "rsr.enabled")];
        let e = c
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert_eq!(e.id.as_deref(), Some("rsr.enabled"));
        assert!(e.message.contains("cannot write or restore"), "{e}");
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_local_change_between_writes_becomes_the_value_to_go_back_to() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        fake.set("pacing.limit", Value::Int(200));
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(160))])
            .unwrap();
        let journal = c.journal().unwrap();
        assert_eq!(entry_of(&journal, "pacing.limit").original, Value::Int(200));
        c.restore(Scope::Session("s1")).unwrap();
        assert_eq!(fake.value("pacing.limit"), Value::Int(200));
    }

    fn live_process() -> std::process::Child {
        std::process::Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .unwrap()
    }
    fn journal_owned_by(path: &Path, pid: u32, started: u64, entries: Vec<Entry>) {
        let journal = Journal {
            version: 1,
            owner: Some(Owner { pid, started }),
            entries,
        };
        state::write_json(path, &journal).unwrap();
    }
    fn some_entry() -> Entry {
        Entry {
            gpu: 1,
            id: "pacing.limit".into(),
            session: "other".into(),
            original: Value::Int(140),
            written: Value::Int(150),
            side_effect: false,
        }
    }

    #[test]
    fn a_second_host_does_not_touch_a_live_hosts_journal() {
        let fake = panel();
        fake.set("pacing.limit", Value::Int(150));
        let (mut c, _dir) = controller(&fake);
        let mut other = live_process();
        let started = owner::identity(other.id()).unwrap().unwrap();
        journal_owned_by(&c.journal, other.id(), started, vec![some_entry()]);
        assert!(c.recover_on_startup().is_err());
        assert!(c.restore(Scope::Session("other")).is_err());
        let e = c
            .apply("s2", 1, &[change("chill.enabled", on(true))])
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Journal);
        assert!(e.message.contains("another host"), "{e}");
        assert!(fake.writes().is_empty());
        assert_eq!(c.journal().unwrap().entries.len(), 1);
        other.kill().unwrap();
        let _ = other.wait();
        // Once that host is gone its entries are restored like a crash's.
        let report = c.recover_on_startup().unwrap();
        assert!(report.is_clean());
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    }

    #[test]
    fn a_live_hosts_empty_journal_does_not_block_a_second_host() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        let mut other = live_process();
        let started = owner::identity(other.id()).unwrap().unwrap();
        journal_owned_by(&c.journal, other.id(), started, vec![]);
        c.apply("s2", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        assert_eq!(c.journal().unwrap().owner, Some(Owner::current().unwrap()));
        other.kill().unwrap();
        let _ = other.wait();
    }

    #[test]
    fn restore_stops_at_its_deadline_and_keeps_what_is_left() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
            .unwrap();
        c.set_deadline(Some(
            Instant::now() - std::time::Duration::from_secs(1),
        ));
        let report = c.recover_on_startup().unwrap();
        assert_eq!(report.failed().count(), 1);
        assert_eq!(fake.value("pacing.limit"), Value::Int(150));
        c.set_deadline(None);
        assert!(c.recover_on_startup().unwrap().is_clean());
        assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    }
}
