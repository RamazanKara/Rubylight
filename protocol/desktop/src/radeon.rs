//! What the Radeon panel of the in-stream Session page draws for one `GET /radeon` answer.
//! The host sends the five states of docs/native-api.md ("Radeon state and writes"); this module
//! decides, per setting and for the panel, what a client shows for each, and never offers a
//! control that would do nothing (the same rules as Nightjar Android's `RadeonPanelModel`):
//!
//! - configurable: an editable control, only while the host says `writable_now`;
//! - read only: the value;
//! - needs external setup: the value if there is one, and the host's message;
//! - unsupported: greyed, "not supported on this GPU or driver";
//! - temporarily unavailable: greyed, the host's message; the panel reads again later.
//!
//! Setting ids this client has no name for are drawn from their kind, range, options and unit.
//! No egui and no network here, so it is all tested.

use nightjar_core::http::native_api::error::ErrorCode;
use nightjar_core::http::native_api::radeon::{Choice, ControlState, Kind, Radeon, Range, Restore, Setting, Value};
use nightjar_core::http::Error;

use crate::strings::{en, tr, trf};

/// What the panel as a whole says above its settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    None,
    ReadOnly,
    Unsupported,
    Unavailable,
    AdlxUnavailable,
    NeedsExternalSetup,
    NeedsHostControl,
    NeedsSession,
}

impl Notice {
    pub fn text(self) -> Option<&'static str> {
        Some(match self {
            Notice::None => return None,
            Notice::ReadOnly => en("These settings can be seen but not changed."),
            Notice::Unsupported => en("These settings are not supported on this GPU or driver."),
            Notice::Unavailable => en("The settings are not available right now. Trying again."),
            Notice::AdlxUnavailable => en("The PC could not load the graphics driver interface."),
            Notice::NeedsExternalSetup => en("Some settings need a one-time setup in the graphics driver’s control panel on the PC."),
            Notice::NeedsHostControl => en("Turn on Host control for this device in the PC’s web UI to change these settings."),
            Notice::NeedsSession => en("You can change these settings while this device owns the session."),
        })
    }
}

/// How one setting is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Edit,
    Value,
    ExternalSetup,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub gpu: i32,
    pub id: String,
    pub group: String,
    pub field: String,
    pub kind: Kind,
    pub mode: Mode,
    pub value: Option<Value>,
    pub range: Option<Range>,
    pub options: Vec<Choice>,
    pub unit: Option<String>,
    /// The host's text for the user, if any.
    pub message: Option<String>,
    pub restore: Option<Restore>,
}

impl Row {
    /// The setting's name: ours for a known group and field, else made from the id's words.
    pub fn title(&self) -> String {
        let Some(group) = group_title(&self.group) else {
            let group = words(&self.group);
            let field = words(&self.field);
            return if plain_field(&field) {
                tr(&group).to_owned()
            } else {
                format!("{}: {}", tr(&group), tr(&field))
            };
        };
        let group = tr(group);
        if sole_setting(&self.group) {
            return group.to_owned();
        }
        if let Some(field) = field_title(&self.field) {
            return format!("{group}: {}", tr(field));
        }
        let field = words(&self.field);
        if plain_field(&field) { group.to_owned() } else { format!("{group}: {}", tr(&field)) }
    }

    /// The value as shown: On/Off, an option's label, or the number with its unit.
    pub fn value_text(&self) -> String {
        match &self.value {
            None => tr("Not available right now").to_owned(),
            Some(value) => self.show(value),
        }
    }

    fn show(&self, value: &Value) -> String {
        match value {
            Value::Bool(true) => tr("On").into(),
            Value::Bool(false) => tr("Off").into(),
            Value::Integer(n) => with_unit(&n.to_string(), self.unit.as_deref()),
            Value::Choice(c) => match self.options.iter().find(|o| &o.value == c) {
                Some(option) if !option.label.is_empty() => option.label.clone(),
                _ => c.clone(),
            },
        }
    }

    /// "Goes back to 120 fps when you end the session", while a change is pending.
    pub fn restore_hint(&self) -> Option<String> {
        let restore = self.restore.as_ref()?;
        Some(trf("Goes back to {} when you end the session", &[&self.show(&restore.original)]))
    }

    /// The note under the row: what the state means for the user, when the row cannot be edited.
    pub fn state_note(&self) -> Option<String> {
        let fallback = match self.mode {
            Mode::Unsupported => tr("Not supported on this GPU or driver"),
            Mode::Unavailable => tr("Not available right now"),
            Mode::ExternalSetup => tr("Needs a one-time setup in the driver’s control panel"),
            Mode::Edit | Mode::Value => return None,
        };
        Some(self.message.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| fallback.to_owned()))
    }

    /// The value after one step up (`+1`) or down (`-1`) for an integer, the next option for a
    /// choice, the opposite for a toggle. None when the row has no such step.
    pub fn step(&self, direction: i64) -> Option<Value> {
        match (&self.kind, self.value.as_ref()?) {
            (Kind::Toggle, Value::Bool(on)) => Some(Value::Bool(!on)),
            (Kind::Integer, Value::Integer(n)) => {
                let range = self.range.as_ref()?;
                let moved = n.checked_add(direction.signum().checked_mul(self.unit_step()?)?)?;
                Some(Value::Integer(snap(range, moved)?))
            }
            (Kind::Choice, Value::Choice(c)) => {
                if self.options.is_empty() {
                    return None;
                }
                let at = self.options.iter().position(|o| &o.value == c).unwrap_or(self.options.len() - 1);
                let len = self.options.len() as i64;
                let next = (at as i64 + direction.signum()).rem_euclid(len) as usize;
                Some(Value::Choice(self.options[next].value.clone()))
            }
            _ => None,
        }
    }

    /// How far one press of + or - moves an integer: the host's step, but never more than
    /// 20 presses across the whole range. None when the range does not fit the arithmetic.
    pub fn unit_step(&self) -> Option<i64> {
        let Some(range) = &self.range else { return Some(1) };
        let step = range.step.max(1);
        let span = range.max.checked_sub(range.min)?.max(0);
        let coarse = (span / 20 / step) * step;
        Some(coarse.max(step))
    }
}

pub fn group_title(group: &str) -> Option<&'static str> {
    Some(match group {
        "anti_lag" => en("Radeon Anti-Lag"),
        "chill" => en("Radeon Chill"),
        "boost" => en("Radeon Boost"),
        "image_sharpening" => en("Radeon Image Sharpening"),
        "enhanced_sync" => en("Enhanced Sync"),
        "frame_rate_target" => en("Frame Rate Target Control"),
        "afmf" => en("AMD Fluid Motion Frames"),
        "rsr" => en("Radeon Super Resolution"),
        "vsync" => en("Vertical sync"),
        "nvidia_low_latency" => en("Low Latency Mode"),
        "nvidia_frame_rate_limit" => en("Max frame rate"),
        "nvidia_vsync" => en("Vertical sync"),
        "nvidia_power" => en("Power management mode"),
        "nvidia_texture_filtering" => en("Texture filtering quality"),
        "nvidia_anisotropic_filtering" => en("Anisotropic filtering"),
        "nvidia_smooth_motion" => en("Smooth Motion"),
        _ => return None,
    })
}

/// A group with one setting, whose row is named by the group alone ("Max frame rate", not
/// "Max frame rate: Frame rate").
fn sole_setting(group: &str) -> bool {
    matches!(
        group,
        "nvidia_low_latency"
            | "nvidia_frame_rate_limit"
            | "nvidia_vsync"
            | "nvidia_power"
            | "nvidia_texture_filtering"
            | "nvidia_anisotropic_filtering"
            | "nvidia_smooth_motion"
    )
}

/// A field that only switches or picks the group's behaviour: "Enabled", "Mode" or none.
fn plain_field(words: &str) -> bool {
    words.is_empty() || words == "Enabled" || words == "Mode"
}

fn field_title(field: &str) -> Option<&'static str> {
    Some(match field {
        "min_fps" => en("Minimum FPS"),
        "max_fps" => en("Maximum FPS"),
        "fps" => en("Frame rate"),
        "min_resolution" => en("Minimum resolution"),
        "sharpness" => en("Sharpness"),
        _ => return None,
    })
}

/// "chill.max_fps" words: "Max fps".
fn words(id: &str) -> String {
    let mut text = String::new();
    for word in id.split('_').filter(|w| !w.is_empty()) {
        if text.is_empty() {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                text.extend(first.to_uppercase());
                text.push_str(&chars.as_str().to_lowercase());
            }
        } else {
            text.push(' ');
            text.push_str(&word.to_lowercase());
        }
    }
    text
}

/// "144 fps", "70%", or just the number when there is no unit.
pub fn with_unit(value: &str, unit: Option<&str>) -> String {
    match unit {
        None | Some("") => value.to_owned(),
        Some("%") => format!("{value}%"),
        Some(unit) => format!("{value} {unit}"),
    }
}

/// The value moved into the range and onto its step grid (halves round up). None when the
/// range or value is so large that the arithmetic overflows.
fn snap(range: &Range, value: i64) -> Option<i64> {
    let step = range.step.max(1);
    let offset = value.checked_sub(range.min)?;
    let (whole, rest) = (offset.div_euclid(step), offset.rem_euclid(step));
    let cells = if rest >= step - rest { whole.checked_add(1)? } else { whole };
    let snapped = range.min.checked_add(cells.checked_mul(step)?)?;
    Some(snapped.clamp(range.min, range.max.max(range.min)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    pub id: i32,
    pub name: String,
    pub driver: String,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub revision: u64,
    pub notice: Notice,
    /// The host's text for the panel notice, if any.
    pub message: Option<String>,
    pub host_control: bool,
    pub writable_now: bool,
    pub gpus: Vec<Gpu>,
}

impl Panel {
    pub fn of(radeon: &Radeon) -> Panel {
        let notice = match radeon.state {
            ControlState::Configurable if radeon.writable_now => Notice::None,
            ControlState::Configurable if radeon.host_control => Notice::NeedsSession,
            ControlState::Configurable => Notice::NeedsHostControl,
            ControlState::ReadOnly => Notice::ReadOnly,
            ControlState::NeedsExternalSetup => Notice::NeedsExternalSetup,
            ControlState::TemporarilyUnavailable if radeon.reason.as_deref() == Some("adlx_unavailable") => {
                Notice::AdlxUnavailable
            }
            ControlState::TemporarilyUnavailable => Notice::Unavailable,
            // Unsupported, and a state this client does not know: both draw as not supported.
            ControlState::Unsupported | ControlState::Unknown => Notice::Unsupported,
        };
        let editable = radeon.state == ControlState::Configurable && radeon.writable_now;
        let gpus = radeon
            .gpus
            .iter()
            .map(|gpu| Gpu {
                id: gpu.id,
                name: gpu.name.clone(),
                driver: gpu.driver.clone(),
                rows: gpu.settings.iter().map(|s| row(gpu.id, s, editable)).collect(),
            })
            .collect();
        Panel {
            revision: radeon.revision,
            notice,
            message: radeon.message.clone().filter(|m| !m.is_empty()),
            host_control: radeon.host_control,
            writable_now: radeon.writable_now,
            gpus,
        }
    }

    pub fn find(&self, id: &str) -> Option<&Row> {
        self.gpus.iter().flat_map(|g| &g.rows).find(|r| r.id == id)
    }

    /// Whether a Radeon write is possible at all right now: host control, an owned session.
    pub fn can_write(&self) -> bool {
        self.writable_now
    }
}

fn row(gpu: i32, setting: &Setting, editable: bool) -> Row {
    let mode = match setting.state {
        ControlState::Configurable if editable && controllable(setting) => Mode::Edit,
        ControlState::Configurable | ControlState::ReadOnly => Mode::Value,
        ControlState::NeedsExternalSetup => Mode::ExternalSetup,
        ControlState::TemporarilyUnavailable => Mode::Unavailable,
        ControlState::Unsupported | ControlState::Unknown => Mode::Unsupported,
    };
    let (group, field) = match setting.id.split_once('.') {
        Some((g, f)) => (g.to_owned(), f.to_owned()),
        None => (setting.group.clone(), String::new()),
    };
    Row {
        gpu,
        id: setting.id.clone(),
        group: if setting.group.is_empty() { group } else { setting.group.clone() },
        field,
        kind: setting.kind,
        mode,
        value: setting.value.clone(),
        range: setting.range,
        options: setting.options.clone(),
        unit: setting.unit.clone(),
        message: setting.message.clone().filter(|m| !m.is_empty()),
        restore: setting.restore.clone(),
    }
}

/// A control needs a value to start from, and a stepper needs a range with room to move.
fn controllable(setting: &Setting) -> bool {
    match (setting.kind, setting.value.as_ref()) {
        (_, None) | (Kind::Unknown, _) => false,
        (Kind::Integer, Some(Value::Integer(_))) => {
            // The span must fit i64, or the stepper's arithmetic could overflow.
            setting.range.is_some_and(|r| r.max > r.min && r.max.checked_sub(r.min).is_some())
        }
        (Kind::Choice, Some(Value::Choice(_))) => !setting.options.is_empty(),
        (Kind::Toggle, Some(Value::Bool(_))) => true,
        _ => false,
    }
}

/// Why a write or a read failed, as the panel shows it next to the setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// The setting the host named, or None when the failure is about the whole request.
    pub setting: Option<String>,
    /// The host's own text, shown when there is no better one.
    pub host_message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    InvalidValue,
    NotWritable,
    DriverRejected,
    HostControl,
    NoSession,
    Busy,
    Unreachable,
    Generic,
}

impl Failure {
    /// The host's code decides, not its English.
    pub fn of(error: &Error, requested: Option<&str>) -> Failure {
        let (kind, setting, host_message) = match error {
            Error::Api { status, error } => {
                let kind = match error.code {
                    ErrorCode::InvalidValue => FailureKind::InvalidValue,
                    ErrorCode::NotWritable => FailureKind::NotWritable,
                    ErrorCode::DriverRejected => FailureKind::DriverRejected,
                    ErrorCode::HostControlRequired => FailureKind::HostControl,
                    ErrorCode::NoSession | ErrorCode::NotSessionOwner => FailureKind::NoSession,
                    ErrorCode::SessionBusy | ErrorCode::TemporarilyUnavailable => FailureKind::Busy,
                    _ if *status == 503 => FailureKind::Busy,
                    _ => FailureKind::Generic,
                };
                (kind, error.setting.clone(), Some(error.message.clone()))
            }
            Error::Io(_) | Error::Tls(_) | Error::CertificateMismatch | Error::Cancelled => {
                (FailureKind::Unreachable, None, None)
            }
            other => (FailureKind::Generic, None, Some(other.to_string())),
        };
        Failure { kind, setting: setting.or_else(|| requested.map(str::to_owned)), host_message }
    }

    pub fn text(&self) -> String {
        match self.kind {
            FailureKind::InvalidValue => tr("The PC does not accept that value.").into(),
            FailureKind::NotWritable => tr("This setting cannot be changed.").into(),
            FailureKind::DriverRejected => tr("The graphics driver rejected this change.").into(),
            FailureKind::HostControl => tr("Turn on Host control for this device in the PC’s web UI.").into(),
            FailureKind::NoSession => tr("Start or resume a session to change settings.").into(),
            FailureKind::Busy => tr("The PC is busy. Try again in a moment.").into(),
            FailureKind::Unreachable => tr("Cannot reach the PC.").into(),
            FailureKind::Generic => match &self.host_message {
                Some(m) if !m.is_empty() => m.clone(),
                _ => tr("The change could not be applied.").into(),
            },
        }
    }
}

/// Whether the host has no Radeon endpoints (a host from before them answers 404).
pub fn is_missing(error: &Error) -> bool {
    matches!(error, Error::NotFound(_) | Error::Api { status: 404, .. })
}

#[cfg(test)]
mod tests;
