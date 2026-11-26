//! Radeon control backend trait, journal schema, restore report types and request validation.

use super::*;

/// The journal's file name inside the profile (config) directory.
pub const JOURNAL_FILE: &str = "radeon-journal.json";

/// Raw driver `(key, optional value)` pairs captured before applying a setting.
pub type RawSettingKeys = Vec<(u32, Option<u32>)>;

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
        super::super::adlx::gpus().map_err(|e| BackendError(format!("{e:#}")))
    }
    /// Hardware identities (PnP / PCI IDs) corresponding to `gpus()`, when available.
    fn gpu_identities(&mut self) -> Vec<GpuIdentity> {
        Vec::new()
    }
    /// Snapshot raw driver key state before applying `id`, so restore can restore absent
    /// keys (via deletion), companion keys, or raw out-of-UI values faithfully.
    fn snapshot_keys(
        &mut self,
        _gpu: i32,
        _id: &str,
    ) -> Result<Option<RawSettingKeys>, BackendError> {
        Ok(None)
    }
    /// Restore using journaled raw driver state when supported. Returns `Ok(true)` if
    /// restored and verified directly, or `Ok(false)` to use `write` + `read_value`.
    fn restore_raw(&mut self, _entry: &Entry) -> Result<bool, BackendError> {
        Ok(false)
    }
}

/// Hardware identity for a GPU returned by [`RadeonBackend::gpus`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuIdentity {
    pub id: i32,
    pub pnp_id: Option<String>,
    pub device_id: Option<u32>,
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
pub(super) fn error(kind: ErrorKind, id: Option<&str>, message: impl Into<String>) -> WriteError {
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
    /// Optional raw driver keys `(key_id, original_value)` captured before mutation.
    /// `None` in an item means the key was not stored in the profile and must be deleted on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<(u32, Option<u32>)>>,
}
impl Entry {
    pub(super) fn new(
        user: String,
        gpu: i32,
        id: String,
        session: String,
        original: Value,
        written: Value,
        side_effect: bool,
    ) -> Self {
        Self {
            gpu,
            id,
            session,
            original,
            written,
            side_effect,
            user,
            raw: None,
        }
    }
    /// The entry for this setting of this user's.
    pub(super) fn is(&self, user: &str, gpu: i32, id: &str) -> bool {
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
    pub(super) fn record(&mut self, entry: Entry) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.is(&entry.user, entry.gpu, &entry.id))
        {
            Some(existing) => {
                if existing.written != entry.original {
                    existing.original = entry.original;
                    existing.raw = entry.raw;
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
    /// Journal entries belonging to another Windows user (or when nobody is signed in)
    /// that were left in place until that user signs in.
    pub deferred_other_users: usize,
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

pub(super) fn find<'a>(settings: &'a [Setting], id: &str) -> Option<&'a Setting> {
    settings.iter().find(|s| s.id == id)
}

pub(super) fn is_on(settings: &[Setting], id: &str) -> bool {
    matches!(
        find(settings, id).and_then(|s| s.value.as_ref()),
        Some(Value::Bool(true))
    )
}

/// Checks the whole request against the settings as they are now, before anything is written.
pub(super) fn validate(settings: &[Setting], changes: &[Change]) -> Result<(), WriteError> {
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
pub(super) fn switched_off_by(ids: &[String], partner: &str) -> bool {
    ids.iter()
        .filter_map(|id| id.strip_suffix(".enabled"))
        .any(|group| model::excluded_by(group).any(|other| format!("{other}.enabled") == partner))
}

/// The toggles a request is expected to make the driver switch off: each configurable
/// `<other>.enabled` that is on now and that a requested `<group>.enabled = true` excludes.
pub(super) fn predicted_effects(before: &[Setting], changes: &[Change]) -> Vec<String> {
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
