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
            Notice::ReadOnly => "These settings can be seen but not changed.",
            Notice::Unsupported => "These settings are not supported on this GPU or driver.",
            Notice::Unavailable => "The settings are not available right now. Trying again.",
            Notice::AdlxUnavailable => "The host could not load the AMD driver interface.",
            Notice::NeedsExternalSetup => "Some settings need a one-time setup in AMD Software on the host.",
            Notice::NeedsHostControl => "Turn on Host control for this device on the host to change these settings.",
            Notice::NeedsSession => "You can change these settings while this device owns the session.",
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
            return if field.is_empty() || field == "Enabled" { group } else { format!("{group}: {field}") };
        };
        if let Some(field) = field_title(&self.field) {
            return format!("{group}: {field}");
        }
        let field = words(&self.field);
        if field.is_empty() || field == "Enabled" || field == "Mode" { group.to_owned() } else { format!("{group}: {field}") }
    }

    /// The value as shown: On/Off, an option's label, or the number with its unit.
    pub fn value_text(&self) -> String {
        match &self.value {
            None => "Not available right now".to_owned(),
            Some(value) => self.show(value),
        }
    }

    fn show(&self, value: &Value) -> String {
        match value {
            Value::Bool(true) => "On".into(),
            Value::Bool(false) => "Off".into(),
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
        Some(format!("Goes back to {} when you end the session", self.show(&restore.original)))
    }

    /// The note under the row: what the state means for the user, when the row cannot be edited.
    pub fn state_note(&self) -> Option<String> {
        let fallback = match self.mode {
            Mode::Unsupported => "Not supported on this GPU or driver",
            Mode::Unavailable => "Not available right now",
            Mode::ExternalSetup => "Needs a one-time setup in AMD Software",
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
        "anti_lag" => "Radeon Anti-Lag",
        "chill" => "Radeon Chill",
        "boost" => "Radeon Boost",
        "image_sharpening" => "Radeon Image Sharpening",
        "enhanced_sync" => "Enhanced Sync",
        "frame_rate_target" => "Frame Rate Target Control",
        "afmf" => "AMD Fluid Motion Frames",
        "rsr" => "Radeon Super Resolution",
        "vsync" => "Vertical sync",
        _ => return None,
    })
}

fn field_title(field: &str) -> Option<&'static str> {
    Some(match field {
        "min_fps" => "Minimum FPS",
        "max_fps" => "Maximum FPS",
        "fps" => "Frame rate",
        "min_resolution" => "Minimum resolution",
        "sharpness" => "Sharpness",
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
            FailureKind::InvalidValue => "The host does not accept that value.".into(),
            FailureKind::NotWritable => "This setting cannot be changed.".into(),
            FailureKind::DriverRejected => "The AMD driver rejected this change.".into(),
            FailureKind::HostControl => "Turn on Host control for this device on the host.".into(),
            FailureKind::NoSession => "Start or resume a session to change settings.".into(),
            FailureKind::Busy => "The host is busy. Try again in a moment.".into(),
            FailureKind::Unreachable => "Cannot reach the host.".into(),
            FailureKind::Generic => match &self.host_message {
                Some(m) if !m.is_empty() => m.clone(),
                _ => "The change could not be applied.".into(),
            },
        }
    }
}

/// Whether the host has no Radeon endpoints (a host from before them answers 404).
pub fn is_missing(error: &Error) -> bool {
    matches!(error, Error::NotFound(_) | Error::Api { status: 404, .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::error::ApiError;

    fn radeon(json: &str) -> Radeon {
        serde_json::from_str(json).expect("valid Radeon json")
    }

    /// The doc's example answer: all five states and a pending restore.
    fn fixture(writable: bool, host_control: bool) -> Radeon {
        radeon(&format!(
            r#"{{
              "revision": 7, "state": "configurable", "host_control": {host_control}, "writable_now": {writable},
              "gpus": [{{ "id": -11520, "name": "AMD Radeon RX 7900 XT", "driver": "25.9.1", "settings": [
                {{ "id": "anti_lag.enabled", "group": "anti_lag", "kind": "toggle", "state": "configurable", "value": true }},
                {{ "id": "chill.max_fps", "group": "chill", "kind": "integer", "unit": "fps", "state": "configurable",
                   "value": 144, "range": {{ "min": 30, "max": 300, "step": 1 }}, "restore": {{ "original": 120, "written": 144 }} }},
                {{ "id": "afmf.enabled", "group": "afmf", "kind": "toggle", "state": "needs_external_setup", "value": false,
                   "reason": "external_setup", "message": "Turn on AMD Fluid Motion Frames once in AMD Software." }},
                {{ "id": "rsr.enabled", "group": "rsr", "kind": "toggle", "state": "unsupported" }},
                {{ "id": "image_sharpening.sharpness", "group": "image_sharpening", "kind": "integer", "unit": "%",
                   "state": "temporarily_unavailable", "reason": "driver_busy", "message": "The driver is busy." }},
                {{ "id": "enhanced_sync.enabled", "group": "enhanced_sync", "kind": "toggle", "state": "read_only", "value": true }},
                {{ "id": "vsync.mode", "group": "vsync", "kind": "choice", "state": "configurable", "value": "off",
                   "options": [{{ "value": "off", "label": "Off" }}, {{ "value": "on", "label": "Always on" }}] }},
                {{ "id": "future_thing.gain", "group": "future_thing", "kind": "integer", "unit": "x",
                   "state": "configurable", "value": 3, "range": {{ "min": 1, "max": 9, "step": 2 }} }}
              ] }}] }}"#
        ))
    }

    fn mode_of(panel: &Panel, id: &str) -> Mode {
        panel.find(id).expect(id).mode
    }

    #[test]
    fn each_of_the_five_states_draws_its_own_way() {
        let panel = Panel::of(&fixture(true, true));
        assert_eq!(panel.notice, Notice::None);
        assert_eq!(mode_of(&panel, "anti_lag.enabled"), Mode::Edit);
        assert_eq!(mode_of(&panel, "chill.max_fps"), Mode::Edit);
        assert_eq!(mode_of(&panel, "enhanced_sync.enabled"), Mode::Value);
        assert_eq!(mode_of(&panel, "afmf.enabled"), Mode::ExternalSetup);
        assert_eq!(mode_of(&panel, "rsr.enabled"), Mode::Unsupported);
        assert_eq!(mode_of(&panel, "image_sharpening.sharpness"), Mode::Unavailable);
        // The notes under the rows that cannot be edited: the host's message, else ours.
        assert_eq!(panel.find("afmf.enabled").unwrap().state_note().unwrap(), "Turn on AMD Fluid Motion Frames once in AMD Software.");
        assert_eq!(panel.find("rsr.enabled").unwrap().state_note().unwrap(), "Not supported on this GPU or driver");
        assert_eq!(panel.find("image_sharpening.sharpness").unwrap().state_note().unwrap(), "The driver is busy.");
        assert_eq!(panel.find("anti_lag.enabled").unwrap().state_note(), None);
        // Values: needs-external-setup shows its value, unsupported and unavailable have none.
        assert_eq!(panel.find("afmf.enabled").unwrap().value_text(), "Off");
        assert_eq!(panel.find("rsr.enabled").unwrap().value_text(), "Not available right now");
    }

    #[test]
    fn nothing_is_editable_unless_the_host_says_writable_now() {
        // Host control but no owned session: configurable settings show as values, with the reason.
        let panel = Panel::of(&fixture(false, true));
        assert_eq!(panel.notice, Notice::NeedsSession);
        assert!(!panel.can_write());
        assert!(panel.gpus[0].rows.iter().all(|r| r.mode != Mode::Edit));
        assert_eq!(mode_of(&panel, "anti_lag.enabled"), Mode::Value);
        // Not even host control: the notice says to turn it on.
        let panel = Panel::of(&fixture(false, false));
        assert_eq!(panel.notice, Notice::NeedsHostControl);
        assert!(panel.gpus[0].rows.iter().all(|r| r.mode != Mode::Edit));
    }

    #[test]
    fn a_whole_panel_state_overrides_the_rows() {
        let mut read_only = fixture(true, true);
        read_only.state = ControlState::ReadOnly;
        let panel = Panel::of(&read_only);
        assert_eq!(panel.notice, Notice::ReadOnly);
        assert!(panel.gpus[0].rows.iter().all(|r| r.mode != Mode::Edit), "read only panel offers no control");

        let mut adlx = fixture(false, true);
        adlx.state = ControlState::TemporarilyUnavailable;
        adlx.reason = Some("adlx_unavailable".into());
        adlx.gpus.clear();
        let panel = Panel::of(&adlx);
        assert_eq!(panel.notice, Notice::AdlxUnavailable);
        assert!(panel.gpus.is_empty());
        adlx.reason = Some("driver_busy".into());
        assert_eq!(Panel::of(&adlx).notice, Notice::Unavailable);

        let mut external = fixture(true, true);
        external.state = ControlState::NeedsExternalSetup;
        assert_eq!(Panel::of(&external).notice, Notice::NeedsExternalSetup);
        let mut unsupported = fixture(true, true);
        unsupported.state = ControlState::Unsupported;
        assert_eq!(Panel::of(&unsupported).notice, Notice::Unsupported);
        // A state from a newer host reads as unsupported.
        unsupported.state = ControlState::Unknown;
        assert_eq!(Panel::of(&unsupported).notice, Notice::Unsupported);
        assert!(Panel::of(&unsupported).gpus[0].rows.iter().all(|r| r.mode != Mode::Edit));
    }

    #[test]
    fn a_control_needs_a_value_and_room_to_move() {
        let mut data = fixture(true, true);
        let settings = &mut data.gpus[0].settings;
        settings[0].value = None; // a toggle with no value
        settings[1].range = Some(Range { min: 5, max: 5, step: 1 }); // nothing to slide
        settings[6].options.clear(); // a choice with no options
        let panel = Panel::of(&data);
        assert_eq!(mode_of(&panel, "anti_lag.enabled"), Mode::Value);
        assert_eq!(mode_of(&panel, "chill.max_fps"), Mode::Value);
        assert_eq!(mode_of(&panel, "vsync.mode"), Mode::Value);
        // A kind from a newer host is shown, never edited.
        let mut data = fixture(true, true);
        data.gpus[0].settings[0].kind = Kind::Unknown;
        assert_eq!(mode_of(&Panel::of(&data), "anti_lag.enabled"), Mode::Value);
    }

    #[test]
    fn names_come_from_known_ids_and_from_the_words_of_unknown_ones() {
        let panel = Panel::of(&fixture(true, true));
        assert_eq!(panel.find("anti_lag.enabled").unwrap().title(), "Radeon Anti-Lag");
        assert_eq!(panel.find("chill.max_fps").unwrap().title(), "Radeon Chill: Maximum FPS");
        assert_eq!(panel.find("vsync.mode").unwrap().title(), "Vertical sync");
        assert_eq!(panel.find("future_thing.gain").unwrap().title(), "Future thing: Gain");
        let mut data = fixture(true, true);
        data.gpus[0].settings[7].id = "future_thing.enabled".into();
        assert_eq!(Panel::of(&data).find("future_thing.enabled").unwrap().title(), "Future thing");
    }

    #[test]
    fn an_unknown_id_draws_from_its_kind_range_and_unit() {
        let panel = Panel::of(&fixture(true, true));
        let row = panel.find("future_thing.gain").unwrap();
        assert_eq!(row.mode, Mode::Edit);
        assert_eq!(row.value_text(), "3 x");
        // The host's step of 2 is kept on the way up and clamped to the range at the end.
        assert_eq!(row.step(1), Some(Value::Integer(5)));
        let mut top = row.clone();
        top.value = Some(Value::Integer(9));
        assert_eq!(top.step(1), Some(Value::Integer(9)));
    }

    #[test]
    fn steps_stay_in_range_and_cycle_choices() {
        let panel = Panel::of(&fixture(true, true));
        let fps = panel.find("chill.max_fps").unwrap();
        // 30..300 has 270 values: 20 presses cross it, so one press is 13.
        assert_eq!(fps.unit_step(), Some(13));
        assert_eq!(fps.step(1), Some(Value::Integer(157)));
        assert_eq!(fps.step(-1), Some(Value::Integer(131)));
        let mut low = fps.clone();
        low.value = Some(Value::Integer(31));
        assert_eq!(low.step(-1), Some(Value::Integer(30)));
        assert_eq!(panel.find("anti_lag.enabled").unwrap().step(1), Some(Value::Bool(false)));
        let sync = panel.find("vsync.mode").unwrap();
        assert_eq!(sync.step(1), Some(Value::Choice("on".into())));
        let mut last = sync.clone();
        last.value = Some(Value::Choice("on".into()));
        assert_eq!(last.step(1), Some(Value::Choice("off".into())), "a choice wraps around");
        assert_eq!(sync.value_text(), "Off");
        assert_eq!(last.value_text(), "Always on");
        // A row with no value has no step.
        assert_eq!(panel.find("rsr.enabled").unwrap().step(1), None);
    }

    #[test]
    fn a_range_that_overflows_is_not_editable_and_never_panics() {
        let mut data = fixture(true, true);
        data.gpus[0].settings[1].range = Some(Range { min: i64::MIN, max: i64::MAX, step: 1 });
        let panel = Panel::of(&data);
        let row = panel.find("chill.max_fps").unwrap();
        assert_eq!(row.mode, Mode::Value, "a span that does not fit i64 is shown, not edited");
        assert_eq!(row.unit_step(), None);
        assert_eq!(row.step(1), None);
        // Arithmetic at the edges of a range that is allowed gives a value or None, never a wrap.
        let mut edge = Panel::of(&fixture(true, true)).find("chill.max_fps").unwrap().clone();
        edge.range = Some(Range { min: i64::MAX - 100, max: i64::MAX, step: 7 });
        edge.value = Some(Value::Integer(i64::MAX));
        assert_eq!(edge.step(1), None, "one step past i64::MAX overflows");
        assert_eq!(edge.step(-1), Some(Value::Integer(i64::MAX - 100 + 13 * 7)));
        edge.value = Some(Value::Integer(i64::MIN));
        edge.range = Some(Range { min: 0, max: 100, step: 1 });
        assert_eq!(edge.step(-1), None);
    }

    #[test]
    fn a_pending_change_says_what_the_session_end_restores() {
        let panel = Panel::of(&fixture(true, true));
        assert_eq!(panel.find("chill.max_fps").unwrap().restore_hint().unwrap(), "Goes back to 120 fps when you end the session");
        assert_eq!(panel.find("anti_lag.enabled").unwrap().restore_hint(), None);
        let mut data = fixture(true, true);
        data.gpus[0].settings[0].restore = Some(Restore { original: Value::Bool(false), written: Value::Bool(true) });
        data.gpus[0].settings[6].restore =
            Some(Restore { original: Value::Choice("on".into()), written: Value::Choice("off".into()) });
        let panel = Panel::of(&data);
        assert_eq!(panel.find("anti_lag.enabled").unwrap().restore_hint().unwrap(), "Goes back to Off when you end the session");
        assert_eq!(panel.find("vsync.mode").unwrap().restore_hint().unwrap(), "Goes back to Always on when you end the session");
    }

    fn api_error(status: u16, code: ErrorCode, setting: Option<&str>) -> Error {
        Error::Api {
            status,
            error: ApiError { code, message: "host words".into(), setting: setting.map(str::to_owned), retry_after_ms: None },
        }
    }

    #[test]
    fn failures_follow_the_hosts_code_and_name_the_setting() {
        let failure = Failure::of(&api_error(422, ErrorCode::InvalidValue, Some("chill.max_fps")), Some("anti_lag.enabled"));
        assert_eq!(failure.kind, FailureKind::InvalidValue);
        assert_eq!(failure.setting.as_deref(), Some("chill.max_fps"), "the host's setting wins");
        assert_eq!(failure.text(), "The host does not accept that value.");
        let failure = Failure::of(&api_error(409, ErrorCode::DriverRejected, None), Some("chill.max_fps"));
        assert_eq!(failure.setting.as_deref(), Some("chill.max_fps"), "else the one that was written");
        assert_eq!(failure.text(), "The AMD driver rejected this change.");
        assert_eq!(Failure::of(&api_error(403, ErrorCode::HostControlRequired, None), None).kind, FailureKind::HostControl);
        assert_eq!(Failure::of(&api_error(409, ErrorCode::NotSessionOwner, None), None).kind, FailureKind::NoSession);
        assert_eq!(Failure::of(&api_error(409, ErrorCode::SessionBusy, None), None).kind, FailureKind::Busy);
        assert_eq!(Failure::of(&api_error(503, ErrorCode::Unknown, None), None).kind, FailureKind::Busy);
        let generic = Failure::of(&api_error(500, ErrorCode::Internal, None), None);
        assert_eq!(generic.text(), "host words", "an error we have no text for shows the host's");
        let unreachable = Failure::of(&Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)), None);
        assert_eq!(unreachable.text(), "Cannot reach the host.");
    }

    #[test]
    fn a_host_without_radeon_endpoints_is_told_apart() {
        assert!(is_missing(&Error::NotFound("radeon".into())));
        assert!(is_missing(&api_error(404, ErrorCode::NotFound, None)));
        assert!(!is_missing(&api_error(409, ErrorCode::NoSession, None)));
        assert!(!is_missing(&Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut))));
    }
}
