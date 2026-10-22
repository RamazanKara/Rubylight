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
use super::model::{Kind, Setting, State, Value};
use anyhow::{Context, Result};
use butterpollo_core::state;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

/// The journal's file name inside the profile (config) directory.
pub const JOURNAL_FILE: &str = "radeon-journal.json";

/// What `Controller` needs from the driver.
pub trait RadeonBackend {
    /// All settings of one GPU, freshly read from the driver.
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError>;
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
    /// Atomic, and synced before it returns.
    pub fn save(&self, path: &Path) -> Result<()> {
        let journal = Self {
            version: 1,
            entries: self.entries.clone(),
        };
        state::write_json(path, &journal)
    }
    /// Keeps the first original per setting: a second write only moves `written`.
    fn record(&mut self, entry: Entry) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.gpu == entry.gpu && e.id == entry.id)
        {
            Some(existing) => {
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
    /// How long restore waits before it believes "already the original". Zero by default.
    settle: Duration,
}

/// What `recover_on_startup` waits for a dying process's last write to reach the driver.
pub const STARTUP_SETTLE: Duration = Duration::from_secs(3);

fn find<'a>(settings: &'a [Setting], id: &str) -> Option<&'a Setting> {
    settings.iter().find(|s| s.id == id)
}

fn validate(settings: &[Setting], changes: &[Change]) -> Result<(), WriteError> {
    if changes.is_empty() {
        return Err(error(ErrorKind::InvalidValue, None, "no changes"));
    }
    let mut seen = HashSet::new();
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
    }
    Ok(())
}

impl<B: RadeonBackend> Controller<B> {
    pub fn new(backend: B, journal: PathBuf) -> Self {
        Self {
            backend,
            journal,
            settle: Duration::ZERO,
        }
    }
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }
    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn journal(&self) -> Result<Journal> {
        Journal::load(&self.journal)
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
        let before = self.backend.read(gpu).map_err(unavailable)?;
        validate(&before, changes)?;
        let journal_error = |e: anyhow::Error| error(ErrorKind::Journal, None, format!("{e:#}"));
        let saved = Journal::load(&self.journal).map_err(journal_error)?;
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
        // The originals reach the disk before the first byte changes in the driver.
        journal.save(&self.journal).map_err(journal_error)?;

        let mut applied: Vec<&Change> = vec![];
        for change in changes {
            if let Err(e) = self.backend.write(gpu, &change.id, &change.value) {
                let failure = error(ErrorKind::DriverRejected, Some(&change.id), e.0);
                return Err(self.undo(gpu, &before, &applied, &saved, failure));
            }
            applied.push(change);
        }
        let after = match self.backend.read(gpu) {
            Ok(after) => after,
            Err(e) => {
                let failure = error(ErrorKind::Unavailable, None, e.0);
                return Err(self.undo(gpu, &before, &applied, &saved, failure));
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
                return Err(self.undo(gpu, &before, &applied, &saved, failure));
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
                "journal the driver's side effects"
            );
        }
        Ok(after)
    }

    /// Put every setting back to `before` after a failed request and say what it could not.
    fn undo(
        &mut self,
        gpu: i32,
        before: &[Setting],
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
        let stuck: Vec<String> = match self.backend.read(gpu) {
            Ok(now) => before
                .iter()
                .filter(|s| s.state == State::Configurable)
                .filter(|s| s.value.as_ref() != find(&now, s.id).and_then(|n| n.value.as_ref()))
                .map(|s| s.id.to_string())
                .collect(),
            Err(_) => applied.iter().map(|c| c.id.clone()).collect(),
        };
        if stuck.is_empty() {
            if let Err(e) = saved.save(&self.journal) {
                tracing::error!(error = format!("{e:#}"), "reset the Radeon journal");
            }
        } else {
            failure.message = format!(
                "{}; could not put back {}, which stays in the journal for the next restore",
                failure.message,
                stuck.join(", ")
            );
        }
        failure
    }

    /// Put journaled settings back. A setting is only touched while it still holds the value the
    /// host wrote; anything else was changed on the PC and is left alone. Entries that fail stay
    /// in the journal.
    pub fn restore(&mut self, scope: Scope) -> Result<RestoreReport> {
        let mut journal = Journal::load(&self.journal)?;
        let selected = |e: &Entry| match scope {
            Scope::All => true,
            Scope::Session(session) => e.session == session,
        };
        // Requested changes first, newest first; then what the driver switched as a consequence.
        let mut order: Vec<usize> = (0..journal.entries.len())
            .filter(|&i| selected(&journal.entries[i]))
            .collect();
        order.reverse();
        order.sort_by_key(|&i| journal.entries[i].side_effect);
        let mut results: Vec<(usize, RestoreResult)> = vec![];
        let mut pending = order;
        for _ in 0..2 {
            let mut failed = vec![];
            for &index in &pending {
                let result = self.restore_one(&journal.entries[index]);
                if matches!(result, RestoreResult::Failed(_)) {
                    failed.push(index);
                }
                results.retain(|(i, _)| *i != index);
                results.push((index, result));
            }
            pending = failed;
            if pending.is_empty() {
                break;
            }
        }
        // A write from a process that just died can still be on its way to the driver (another
        // process read it ~2 s late on the RX 7900 XT), so "already the original" is checked again
        // after the settle time before the entry is dropped.
        let settled: Vec<usize> = results
            .iter()
            .filter(|(_, result)| *result == RestoreResult::AlreadyOriginal)
            .map(|(index, _)| *index)
            .collect();
        if !settled.is_empty() && !self.settle.is_zero() {
            std::thread::sleep(self.settle);
            for index in settled {
                let again = self.restore_one(&journal.entries[index]);
                if again != RestoreResult::AlreadyOriginal
                    && let Some(slot) = results.iter_mut().find(|(i, _)| *i == index)
                {
                    slot.1 = again;
                }
            }
        }
        let mut report = RestoreReport::default();
        let mut keep = vec![true; journal.entries.len()];
        for (index, result) in results {
            let entry = &journal.entries[index];
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
            keep[index] = matches!(result, RestoreResult::Failed(_));
            report.outcomes.push(Outcome {
                gpu: entry.gpu,
                id: entry.id.clone(),
                result,
            });
        }
        let mut index = 0;
        let before = journal.entries.len();
        journal.entries.retain(|_| {
            index += 1;
            keep[index - 1]
        });
        if journal.entries.len() != before {
            journal.save(&self.journal)?;
        }
        Ok(report)
    }

    /// For a host start: restore whatever a crashed run left in the journal.
    pub fn recover_on_startup(&mut self) -> Result<RestoreReport> {
        self.restore(Scope::All)
    }

    fn restore_one(&mut self, entry: &Entry) -> RestoreResult {
        let current = |backend: &mut B| -> Result<Value, String> {
            let settings = backend.read(entry.gpu).map_err(|e| e.0)?;
            find(&settings, &entry.id)
                .and_then(|s| s.value.clone())
                .ok_or_else(|| "the driver no longer reports the setting".to_string())
        };
        let now = match current(&mut self.backend) {
            Ok(now) => now,
            Err(message) => return RestoreResult::Failed(message),
        };
        if now == entry.original {
            return RestoreResult::AlreadyOriginal;
        }
        if now != entry.written {
            return RestoreResult::ChangedElsewhere {
                current: now,
                written: entry.written.clone(),
            };
        }
        if let Err(e) = self.backend.write(entry.gpu, &entry.id, &entry.original) {
            return RestoreResult::Failed(e.0);
        }
        match current(&mut self.backend) {
            Ok(value) if value == entry.original => RestoreResult::Restored,
            Ok(value) => RestoreResult::Failed(format!(
                "wrote {} and the driver reads back {}",
                entry.original.display(),
                value.display()
            )),
            Err(message) => RestoreResult::Failed(message),
        }
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
        /// The next reads show this id with this older value, as another process saw a dying
        /// process's write ~2 s late: (id, stale value, reads left).
        stale: Option<(&'static str, Value, usize)>,
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
            if d.refuse == Some(id) {
                return Err(BackendError("ADLX_NOT_SUPPORTED".into()));
            }
            if d.ignore == Some(id) {
                return Ok(());
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
                change("chill.max_fps", Value::Int(20)),
                ErrorKind::InvalidValue,
            ),
            (
                change("chill.max_fps", Value::Int(305)),
                ErrorKind::InvalidValue,
            ),
            (
                change("chill.max_fps", Value::Int(145)),
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
                    change("chill.max_fps", Value::Int(150)),
                    change("tessellation.mode", Value::Choice("c".into())),
                ],
            )
            .unwrap();
        assert_eq!(
            find(&after, "chill.max_fps").unwrap().value,
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
        assert_eq!(fake.writes(), ["chill.max_fps=150", "tessellation.mode=c"]);
    }

    #[test]
    fn the_first_original_is_kept_across_writes_in_a_session() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("chill.max_fps", Value::Int(150))])
            .unwrap();
        c.apply("s1", 1, &[change("chill.max_fps", Value::Int(160))])
            .unwrap();
        let journal = c.journal().unwrap();
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].original, Value::Int(140));
        assert_eq!(journal.entries[0].written, Value::Int(160));
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::Restored);
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
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
                    change("chill.max_fps", Value::Int(150)),
                    change("tessellation.mode", Value::Choice("b".into())),
                ],
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert_eq!(e.id.as_deref(), Some("tessellation.mode"));
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
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
        fake.0.borrow_mut().ignore = Some("chill.max_fps");
        let e = c
            .apply(
                "s1",
                1,
                &[
                    change("chill.enabled", on(true)),
                    change("chill.max_fps", Value::Int(150)),
                ],
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::DriverRejected);
        assert_eq!(e.id.as_deref(), Some("chill.max_fps"));
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
                change("chill.max_fps", Value::Int(150)),
                change("chill.enabled", on(true)),
            ],
        )
        .unwrap();
        fake.set("chill.max_fps", Value::Int(200));
        let report = c.restore(Scope::Session("s1")).unwrap();
        let by_id = |id: &str| &report.outcomes.iter().find(|o| o.id == id).unwrap().result;
        assert_eq!(by_id("chill.enabled"), &RestoreResult::Restored);
        assert_eq!(
            by_id("chill.max_fps"),
            &RestoreResult::ChangedElsewhere {
                current: Value::Int(200),
                written: Value::Int(150)
            }
        );
        assert_eq!(fake.value("chill.max_fps"), Value::Int(200));
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
            id: "chill.max_fps".into(),
            session: "s1".into(),
            original: Value::Int(140),
            written: Value::Int(150),
            side_effect: false,
        });
        journal.save(&c.journal).unwrap();
        let report = c.recover_on_startup().unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::AlreadyOriginal);
        assert!(fake.writes().is_empty());
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_failed_restore_stays_journaled_and_is_retried() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("chill.max_fps", Value::Int(150))])
            .unwrap();
        fake.0.borrow_mut().refuse = Some("chill.max_fps");
        let report = c.restore(Scope::Session("s1")).unwrap();
        assert!(!report.is_clean());
        assert_eq!(c.journal().unwrap().entries.len(), 1);
        assert_eq!(fake.value("chill.max_fps"), Value::Int(150));
        fake.0.borrow_mut().refuse = None;
        let report = c.recover_on_startup().unwrap();
        assert!(report.is_clean());
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
        assert!(c.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn restore_by_session_leaves_the_other_sessions() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("chill.max_fps", Value::Int(150))])
            .unwrap();
        c.apply(
            "s2",
            1,
            &[change("tessellation.mode", Value::Choice("b".into()))],
        )
        .unwrap();
        c.restore(Scope::Session("s2")).unwrap();
        assert_eq!(fake.value("tessellation.mode"), Value::Choice("a".into()));
        assert_eq!(fake.value("chill.max_fps"), Value::Int(150));
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
                    change("chill.max_fps", Value::Int(150)),
                ],
            )
            .unwrap();
            // The process dies here: nothing restores, the controller is just gone.
        }
        assert_eq!(fake.value("chill.max_fps"), Value::Int(150));
        let mut restarted = Controller::new(fake.clone(), path);
        let report = restarted.recover_on_startup().unwrap();
        assert!(report.is_clean());
        assert_eq!(report.outcomes.len(), 2);
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
        assert!(restarted.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn a_late_visible_write_is_restored_after_the_settle_time() {
        let fake = panel();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JOURNAL_FILE);
        Controller::new(fake.clone(), path.clone())
            .apply("s1", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        // The first read still shows the original, though the write has landed.
        fake.0.borrow_mut().stale = Some(("chill.enabled", on(false), 1));

        let mut hasty = Controller::new(fake.clone(), path.clone());
        let report = hasty.restore(Scope::All).unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::AlreadyOriginal);
        assert_eq!(
            fake.value("chill.enabled"),
            on(true),
            "the stale read fooled it"
        );

        // With the settle time the entry is checked again and the real value put back.
        let fake = panel();
        let path = dir.path().join("second.json");
        Controller::new(fake.clone(), path.clone())
            .apply("s2", 1, &[change("chill.enabled", on(true))])
            .unwrap();
        fake.0.borrow_mut().stale = Some(("chill.enabled", on(false), 1));
        let mut patient = Controller::new(fake.clone(), path).with_settle(Duration::from_millis(5));
        let report = patient.recover_on_startup().unwrap();
        assert_eq!(report.outcomes[0].result, RestoreResult::Restored);
        assert_eq!(fake.value("chill.enabled"), on(false));
        assert!(patient.journal().unwrap().entries.is_empty());
    }

    #[test]
    fn an_unreadable_gpu_fails_a_restore_without_dropping_the_entry() {
        let fake = panel();
        let (mut c, _dir) = controller(&fake);
        c.apply("s1", 1, &[change("chill.max_fps", Value::Int(150))])
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
}
