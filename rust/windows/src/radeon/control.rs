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
    /// The AMD GPUs ADLX lists, with their driver. The identity of a GPU does not go stale like
    /// the settings view does, so the real backends list them in this process.
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        super::adlx::gpus().map_err(|e| BackendError(format!("{e:#}")))
    }
}

/// One AMD GPU as ADLX names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuInfo {
    /// The ADLX unique id, which `/api/hardware-metrics` reports as well.
    pub id: i32,
    pub name: String,
    /// The Adrenalin release when ADLX names it, else the driver version.
    pub driver: String,
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
    /// The SID of the user whose settings these are. Radeon settings are per user: another
    /// account's values are not this account's to read, compare or put back, so a restore leaves
    /// entries of other users alone. Empty (a journal from before this field) matches nobody.
    #[serde(default)]
    pub user: String,
}
impl Entry {
    /// The entry for this setting of this user's.
    fn is(&self, user: &str, gpu: i32, id: &str) -> bool {
        self.user == user && self.gpu == gpu && self.id == id
    }
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
            .find(|e| e.is(&entry.user, entry.gpu, &entry.id))
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
    /// Only these settings of one session, and the switches the driver made as a consequence of
    /// them. The session's other entries (a client's own writes) stay journaled.
    Settings(&'a str, &'a [String]),
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
    /// The SID of the user whose settings the helpers read and write: the signed-in user, `None`
    /// while nobody is.
    user: Box<dyn Fn() -> Option<String> + Send>,
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
                if partner.state != State::Configurable && is_on(settings, partner.id) && !derived {
                    return Err(invalid(format!(
                        "turning it on can switch {} off, and the host cannot write or restore that setting ({})",
                        partner.id,
                        partner.state.name()
                    )));
                }
            }
            // Not measured yet: whether Enhanced Sync switches Wait for Vertical Refresh off (and
            // what that does to vsync.mode, which has a setter and is journaled, but whose value
            // after the flip is unknown). Refuse until a hardware run says what happens.
            if group == "enhanced_sync"
                && *on
                && !is_on(settings, id)
                && is_on(settings, "vsync.enabled")
            {
                return Err(invalid(
                    "vsync is on, and what Enhanced Sync does to it is not measured yet; set vsync.mode to always_off first".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Whether turning on one of the `<group>.enabled` toggles in `ids` can make the driver switch
/// `partner` (another `<group>.enabled`) off.
fn switched_off_by(ids: &[String], partner: &str) -> bool {
    ids.iter()
        .filter_map(|id| id.strip_suffix(".enabled"))
        .any(|group| model::excluded_by(group).any(|other| format!("{other}.enabled") == partner))
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
            user: Box::new(crate::process::user_sid),
        }
    }
    /// Name the user another way (the tests).
    #[cfg(test)]
    pub(super) fn with_user(mut self, user: impl Fn() -> Option<String> + Send + 'static) -> Self {
        self.user = Box::new(user);
        self
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
    /// What would still be put back for the signed-in user: the journal's entries of their
    /// settings. Another user's entries are not this user's to show or restore.
    pub fn pending(&self) -> Result<Vec<Entry>> {
        let user = (self.user)();
        Ok(self
            .journal()?
            .entries
            .into_iter()
            .filter(|e| user.as_deref() == Some(e.user.as_str()))
            .collect())
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
        let Some(user) = (self.user)() else {
            return Err(error(
                ErrorKind::Unavailable,
                None,
                "nobody is signed in, so there is no user whose settings these would be",
            ));
        };
        let before = self.backend.read(gpu).map_err(unavailable)?;
        validate(&before, changes)?;
        let mut journal = saved.clone();
        let original = |id: &str| find(&before, id).and_then(|s| s.value.clone());
        for change in changes {
            journal.record(Entry {
                user: user.clone(),
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
        let predicted = predicted_effects(&before, changes);
        for partner in predicted.iter().cloned() {
            journal.record(Entry {
                user: user.clone(),
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
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
            }
            applied.push(change);
        }
        let after = match self.backend.read(gpu) {
            Ok(after) => after,
            Err(e) => {
                let failure = error(ErrorKind::Unavailable, None, e.0);
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
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
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
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
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
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
                    user: user.clone(),
                    gpu,
                    id: setting.id.into(),
                    session: session.into(),
                    original: was.clone(),
                    written: now.clone(),
                    side_effect: true,
                });
            }
        }
        // A predicted partner the driver did not switch (as far as this read-back shows) stays
        // journaled too. The driver may apply the switch after the read, and a restore finds the
        // partner at its original, which costs nothing.
        let saved_again = if effects {
            journal.save(&self.journal)
        } else {
            Ok(())
        };
        if let Err(e) = saved_again {
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
            return Err(self.undo(
                session, &user, gpu, &before, changes, &applied, &saved, failure,
            ));
        }
        Ok(after)
    }

    /// Put every setting back to `before` after a failed request and say what it could not. What
    /// stays changed is journaled with the value it holds now, so a restore can still act on it.
    #[allow(clippy::too_many_arguments)]
    fn undo(
        &mut self,
        session: &str,
        user: &str,
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
                    .filter(|s| s.value.as_ref() != find(&now, s.id).and_then(|n| n.value.as_ref()))
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
                        user: user.into(),
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
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn restore_entries(&mut self, scope: Scope, startup: bool) -> Result<RestoreReport> {
        let (_lock, mut journal) = self.open()?;
        let selected = |e: &Entry| match scope {
            Scope::All => true,
            Scope::Session(session) => e.session == session,
            Scope::Settings(session, ids) => {
                e.session == session
                    && (ids.contains(&e.id) || (e.side_effect && switched_off_by(ids, &e.id)))
            }
        };
        // Only the signed-in user's entries: another user's values (or the defaults of a view
        // that is not theirs) must never be compared with or written into this user's settings.
        let user = (self.user)();
        let (mine, others): (Vec<&Entry>, Vec<&Entry>) = journal
            .entries
            .iter()
            .filter(|e| selected(e))
            .partition(|e| user.as_deref() == Some(e.user.as_str()));
        if !others.is_empty() {
            tracing::error!(
                settings = %others.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "),
                owners = %others.iter().map(|e| e.user.as_str()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", "),
                signed_in = user.as_deref().unwrap_or("nobody"),
                "Radeon journal entries belong to another user; left in place until that user is signed in"
            );
        }
        // Requested changes first, newest first; then what the driver switched as a consequence.
        let mut order: Vec<Entry> = mine.into_iter().cloned().collect();
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
                results.retain(|(e, _)| !e.is(&entry.user, entry.gpu, &entry.id));
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
                    .retain(|e| !e.is(&entry.user, entry.gpu, &entry.id));
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

    fn restore_one(
        &mut self,
        journal: &mut Journal,
        entry: &Entry,
        startup: bool,
    ) -> RestoreResult {
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
                .any(|e| e.is(&entry.user, entry.gpu, &id))
            {
                continue;
            }
            temporary.push(id.clone());
            journal.entries.push(Entry {
                user: entry.user.clone(),
                gpu: entry.gpu,
                id,
                session: entry.session.clone(),
                original: Value::Bool(original),
                written: Value::Bool(written),
                side_effect: true,
            });
        }
        let forget = |journal: &mut Journal| {
            journal.entries.retain(|e| {
                !(e.user == entry.user && e.gpu == entry.gpu && temporary.contains(&e.id))
            });
        };
        if let Err(e) = journal.save(&self.journal) {
            forget(journal);
            return RestoreResult::Failed(format!(
                "journaling the temporary switch of {toggle}: {e:#}"
            ));
        }
        if let Err(message) = self.through_toggle(entry, toggle, &partners) {
            // The temporary entries stay: the next restore puts the switches back.
            return RestoreResult::Failed(message);
        }
        forget(journal);
        if let Err(e) = journal.save(&self.journal) {
            tracing::warn!(
                error = format!("{e:#}"),
                "drop the temporary Radeon entries"
            );
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
        let put = self.put_back(entry);
        // Back to how the feature was whether or not the field went back: a failed put-back must
        // not leave the feature on.
        let tail = (|| {
            switch(&mut self.backend, toggle, false)?;
            for partner in partners {
                let now = self.backend.read_value(gpu, partner).map_err(|e| e.0)?;
                if now != Value::Bool(true) {
                    switch(&mut self.backend, partner, true)?;
                }
            }
            Ok(())
        })();
        match (put, tail) {
            (Ok(()), tail) => tail,
            (Err(put), Ok(())) => Err(format!("{put}; {toggle} was switched back")),
            (Err(put), Err(tail)) => Err(format!(
                "{put}; switching {toggle} back also failed ({tail}), the temporary entries stay journaled"
            )),
        }
    }
}

#[cfg(test)]
mod tests;
