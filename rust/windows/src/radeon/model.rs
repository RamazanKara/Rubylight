//! Raw ADLX answers and the mapping from them to the five Radeon states.
//!
//! The FFI layer in `adlx.rs` only copies what ADLX said into `GroupRaw` values. Everything that
//! decides what a setting is (supported, readable, writable, state, dependencies) lives here, so
//! it runs on fakes without a GPU.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The five states of plan decision 5, as named in `docs/native-api.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Configurable,
    ReadOnly,
    NeedsExternalSetup,
    Unsupported,
    TemporarilyUnavailable,
}
impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Configurable => "configurable",
            Self::ReadOnly => "read_only",
            Self::NeedsExternalSetup => "needs_external_setup",
            Self::Unsupported => "unsupported",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Toggle,
    Integer,
    Choice,
    Action,
}

/// Whether the setting can be written, decided without writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Writable {
    /// ADLX reports the feature supported and, for integers, a usable range. The driver's
    /// acceptance of an actual write is not tested here.
    Yes,
    /// ADLX itself says the value cannot change (unsupported, or an empty range).
    No,
    /// ADLX has no query that tells; only a write would show.
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Choice(String),
}
impl Value {
    pub fn display(&self) -> String {
        match self {
            Self::Bool(true) => "on".into(),
            Self::Bool(false) => "off".into(),
            Self::Int(v) => v.to_string(),
            Self::Choice(v) => v.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Range {
    pub min: i64,
    pub max: i64,
    pub step: i64,
}
impl Range {
    /// A range a client can edit: non-empty, with a positive step.
    pub fn usable(self) -> bool {
        self.min < self.max && self.step > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChoiceOption {
    pub value: String,
    pub raw: i32,
}

/// An ADLX call that did not succeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The runtime does not expose the versioned interface at all.
    Missing,
    /// `ADLX_RESULT` returned by the call.
    Code(i32),
}
impl Fault {
    pub fn describe(self) -> String {
        match self {
            Self::Missing => "interface not exposed by this ADLX runtime".into(),
            Self::Code(code) => result_name(code),
        }
    }
    /// The feature is not there on this GPU or driver (UNKNOWN_INTERFACE, NOT_FOUND,
    /// NOT_SUPPORTED), as opposed to a failure that may pass.
    fn is_unsupported(self) -> bool {
        matches!(self, Self::Missing | Self::Code(6 | 9 | 12))
    }
    /// ADLX says the feature must be reset to factory settings before it works.
    fn needs_reset(self) -> bool {
        self == Self::Code(18)
    }
}
pub type Query<T> = Result<T, Fault>;

/// `ADLX_RESULT` from ADLXDefines.h, SDK 2.0.0.125.
pub fn result_name(code: i32) -> String {
    const NAMES: [&str; 19] = [
        "ADLX_OK",
        "ADLX_ALREADY_ENABLED",
        "ADLX_ALREADY_INITIALIZED",
        "ADLX_FAIL",
        "ADLX_INVALID_ARGS",
        "ADLX_BAD_VER",
        "ADLX_UNKNOWN_INTERFACE",
        "ADLX_TERMINATED",
        "ADLX_ADL_INIT_ERROR",
        "ADLX_NOT_FOUND",
        "ADLX_INVALID_OBJECT",
        "ADLX_ORPHAN_OBJECTS",
        "ADLX_NOT_SUPPORTED",
        "ADLX_PENDING_OPERATION",
        "ADLX_GPU_INACTIVE",
        "ADLX_GPU_IN_USE",
        "ADLX_TIMEOUT_OPERATION",
        "ADLX_NOT_ACTIVE",
        "ADLX_RESET_NEEDED",
    ];
    usize::try_from(code)
        .ok()
        .and_then(|i| NAMES.get(i))
        .map_or_else(|| format!("ADLX_RESULT({code})"), |name| (*name).into())
}

/// Names of the C enums in ADLXDefines.h (SDK 2.0.0.125).
pub type EnumItems = &'static [(i32, &'static str)];
pub const ANTILAG_STATE: EnumItems = &[(0, "anti_lag"), (1, "anti_lag_next")];
pub const WAIT_FOR_VERTICAL_REFRESH: EnumItems = &[
    (0, "always_off"),
    (1, "off_unless_app_specifies"),
    (2, "on_unless_app_specifies"),
    (3, "always_on"),
];
pub const AA_MODE: EnumItems = &[
    (0, "use_app_settings"),
    (1, "enhance_app_settings"),
    (2, "override_app_settings"),
];
pub const AA_LEVEL: EnumItems = &[
    (0, "invalid"),
    (2, "2x"),
    (3, "2x_eq"),
    (4, "4x"),
    (5, "4x_eq"),
    (8, "8x"),
    (9, "8x_eq"),
];
pub const AA_METHOD: EnumItems = &[
    (0, "multisampling"),
    (1, "adaptive_multisampling"),
    (2, "supersampling"),
];
pub const AF_LEVEL: EnumItems = &[(0, "invalid"), (2, "2x"), (4, "4x"), (8, "8x"), (16, "16x")];
pub const TESSELLATION_MODE: EnumItems = &[
    (0, "amd_optimized"),
    (1, "use_app_settings"),
    (2, "override_app_settings"),
];
pub const TESSELLATION_LEVEL: EnumItems = &[
    (1, "off"),
    (2, "2x"),
    (4, "4x"),
    (6, "6x"),
    (8, "8x"),
    (16, "16x"),
    (32, "32x"),
    (64, "64x"),
];
pub const AFMF_ALGORITHM: EnumItems = &[(0, "auto"), (1, "enhanced"), (2, "standard")];
pub const AFMF_SEARCH_MODE: EnumItems = &[(0, "auto"), (1, "standard"), (2, "high")];
pub const AFMF_PERFORMANCE_MODE: EnumItems = &[(0, "auto"), (1, "quality"), (2, "performance")];
pub const AFMF_FAST_MOTION: EnumItems = &[(0, "repeat_frames"), (1, "blended_frames")];
pub const FFX_FRAME_GEN_RATIO: EnumItems = &[(1, "2x")];

/// The enum behind a choice setting, to turn an option name back into the raw ADLX value.
pub fn enum_items(id: &str) -> Option<EnumItems> {
    Some(match id {
        "anti_lag.level" => ANTILAG_STATE,
        "vsync.mode" => WAIT_FOR_VERTICAL_REFRESH,
        "afmf.algorithm" => AFMF_ALGORITHM,
        "afmf.search_mode" => AFMF_SEARCH_MODE,
        "afmf.performance_mode" => AFMF_PERFORMANCE_MODE,
        "afmf.fast_motion_response" => AFMF_FAST_MOTION,
        "anti_aliasing.mode" => AA_MODE,
        "anti_aliasing.level" => AA_LEVEL,
        "anti_aliasing.method" => AA_METHOD,
        "anisotropic_filtering.level" => AF_LEVEL,
        "tessellation.mode" => TESSELLATION_MODE,
        "tessellation.level" => TESSELLATION_LEVEL,
        "fidelityfx_frame_gen.ratio" => FFX_FRAME_GEN_RATIO,
        _ => return None,
    })
}
pub fn enum_raw(items: EnumItems, name: &str) -> Option<i32> {
    items
        .iter()
        .find(|(_, item)| *item == name)
        .map(|(raw, _)| *raw)
}
pub fn enum_name(items: EnumItems, raw: i32) -> String {
    items
        .iter()
        .find(|(value, _)| *value == raw)
        .map_or_else(|| format!("unknown({raw})"), |(_, name)| (*name).into())
}
pub fn enum_options(items: EnumItems) -> Vec<ChoiceOption> {
    items
        .iter()
        .filter(|(_, name)| *name != "invalid")
        .map(|(raw, name)| ChoiceOption {
            value: (*name).into(),
            raw: *raw,
        })
        .collect()
}

/// One value the FFI layer read from an ADLX getter.
pub struct FieldRaw {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub unit: Option<&'static str>,
    /// `None` for actions, which have nothing to read.
    pub value: Option<Query<Value>>,
    /// `Some` only when ADLX has a range query for this field.
    pub range: Option<Query<Range>>,
    /// The values the header enum or an ADLX option list names. ADLX has no per-GPU query for the
    /// enum-backed ones, so these are what the SDK defines, not what the driver accepts.
    pub options: Vec<ChoiceOption>,
    /// Whether ADLX can list the accepted options (true only for the FidelityFX ratio list).
    pub options_queried: bool,
    pub note: Option<String>,
}

/// One ADLX interface (`IADLX3DChill`, ...) and the fields read through it.
pub struct GroupRaw {
    pub group: &'static str,
    pub interface: &'static str,
    /// The `Get...` call on the services object (or the QueryInterface for a versioned one).
    pub obtained: Query<()>,
    pub supported: Option<Query<bool>>,
    pub fields: Vec<FieldRaw>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Cannot be on at the same time as the other.
    Excludes,
    /// Needs the other to be on.
    Requires,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dependency {
    pub group: &'static str,
    pub relation: Relation,
    pub note: &'static str,
    /// Whether the other feature is on right now, when the probe could tell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// From the "cannot be simultaneously enabled with" and "requires" sentences in I3DSettings*.h.
/// `gpu_scaling` is a display setting outside the 3D services and is not probed.
const DEPENDENCIES: &[(&str, Relation, &str, &str)] = &[
    ("anti_lag", Relation::Excludes, "chill", ""),
    ("chill", Relation::Excludes, "anti_lag", ""),
    ("chill", Relation::Excludes, "boost", ""),
    ("boost", Relation::Excludes, "chill", ""),
    ("boost", Relation::Excludes, "rsr", "on some GPUs"),
    (
        "image_sharpening",
        Relation::Excludes,
        "rsr",
        "on some GPUs",
    ),
    ("rsr", Relation::Excludes, "boost", "on some GPUs"),
    (
        "rsr",
        Relation::Excludes,
        "image_sharpening",
        "on some GPUs",
    ),
    (
        "rsr",
        Relation::Requires,
        "gpu_scaling",
        "enabling RSR turns GPU scaling on",
    ),
    ("enhanced_sync", Relation::Excludes, "vsync", ""),
    ("vsync", Relation::Excludes, "enhanced_sync", ""),
    (
        "image_sharpen_desktop",
        Relation::Requires,
        "image_sharpening",
        "",
    ),
];

/// Settings the host reads but never writes, and why.
const NEVER_WRITTEN: &[(&str, &str)] = &[
    (
        "vsync.enabled",
        "derived from vsync.mode; ADLX has no setter for it",
    ),
    (
        "rsr.enabled",
        "not written: ADLX turns GPU scaling on with RSR, and GPU scaling is a display setting the host can neither read nor restore",
    ),
    (
        "anti_aliasing.level",
        "not written: while the mode leaves AA to the application the level reads 'invalid', and ADLX refuses to write that back, so a change could not be restored",
    ),
    (
        "anisotropic_filtering.level",
        "not written: while filtering is left to the application the level reads 'invalid', and ADLX refuses to write that back, so a change could not be restored",
    ),
];

/// Choice settings whose write and restore round-trip was proven on hardware
/// (`radeon::hardware`, results in `docs/hardware-matrix.md`). ADLX cannot say which enum values a
/// driver accepts, so the rest of the choices stay read-only until a run proves them.
pub const PROVEN_CHOICES: &[&str] = &[
    "vsync.mode",
    "afmf.search_mode",
    "afmf.performance_mode",
    "afmf.fast_motion_response",
    "anti_aliasing.mode",
    "anti_aliasing.method",
    "tessellation.mode",
    "tessellation.level",
];

/// Settings the driver only accepts while their feature is on (proven on hardware: ADLX_FAIL or
/// ADLX_NOT_ACTIVE otherwise). A request that changes one lists `<group>.enabled` first.
const NEEDS_FEATURE_ON: &[&str] = &[
    "chill.min_fps",
    "chill.max_fps",
    "boost.min_resolution",
    "image_sharpening.sharpness",
    "frame_rate_target.fps",
    "afmf.search_mode",
    "afmf.performance_mode",
    "afmf.fast_motion_response",
];

/// The classified result for one field.
#[derive(Clone, Debug, Serialize)]
pub struct Setting {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub interface: &'static str,
    pub kind: Kind,
    pub state: State,
    /// `None` when ADLX could not answer.
    pub supported: Option<bool>,
    /// `None` for actions.
    pub readable: Option<bool>,
    pub writable: Writable,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ChoiceOption>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<Dependency>,
    pub notes: Vec<String>,
    /// What ADLX answered to each call, for the record.
    pub adlx: BTreeMap<&'static str, String>,
}

fn answer<T>(query: &Query<T>) -> String {
    match query {
        Ok(_) => "ADLX_OK".into(),
        Err(fault) => fault.describe(),
    }
}

/// Classify every field of one interface.
pub fn classify_group(group: &GroupRaw) -> Vec<Setting> {
    group
        .fields
        .iter()
        .map(|field| classify(group, field))
        .collect()
}

fn classify(group: &GroupRaw, field: &FieldRaw) -> Setting {
    let mut setting = Setting {
        id: field.id,
        group: group.group,
        label: field.label,
        interface: group.interface,
        kind: field.kind,
        state: State::Unsupported,
        supported: None,
        readable: (field.kind != Kind::Action).then_some(false),
        writable: Writable::No,
        value: None,
        unit: field.unit,
        range: None,
        options: field.options.clone(),
        depends: vec![],
        notes: field.note.iter().cloned().collect(),
        adlx: BTreeMap::new(),
    };
    setting
        .adlx
        .insert("get_interface", answer(&group.obtained));
    if let Some(supported) = &group.supported {
        setting.adlx.insert("is_supported", answer(supported));
    }
    let unavailable = |setting: &mut Setting, fault: Fault, what: &str| {
        setting.state = if fault.is_unsupported() {
            State::Unsupported
        } else if fault.needs_reset() {
            State::NeedsExternalSetup
        } else {
            State::TemporarilyUnavailable
        };
        setting.supported = fault.is_unsupported().then_some(false);
        setting.notes.push(format!("{what}: {}", fault.describe()));
    };
    if let Err(fault) = group.obtained {
        unavailable(&mut setting, fault, "interface");
        return setting;
    }
    match &group.supported {
        Some(Ok(true)) => setting.supported = Some(true),
        Some(Ok(false)) => {
            setting.supported = Some(false);
            setting.notes.push("ADLX IsSupported is false".into());
            return setting;
        }
        Some(Err(fault)) => {
            unavailable(&mut setting, *fault, "IsSupported");
            return setting;
        }
        None => setting.supported = None,
    }
    if let Some(value) = &field.value {
        setting.adlx.insert("read", answer(value));
        match value {
            Ok(value) => {
                setting.readable = Some(true);
                setting.value = Some(value.clone());
            }
            Err(fault) => {
                unavailable(&mut setting, *fault, "read");
                return setting;
            }
        }
    }
    if let Some(range) = &field.range {
        setting.adlx.insert("range", answer(range));
        match range {
            Ok(range) => setting.range = Some(*range),
            Err(fault) => {
                // No bounds means no valid write; the value is still shown.
                setting.state = State::ReadOnly;
                setting.writable = Writable::Unverified;
                setting.notes.push(format!(
                    "range query failed ({}); writes cannot be bounded",
                    fault.describe()
                ));
                return setting;
            }
        }
    }
    if let Some((_, reason)) = NEVER_WRITTEN.iter().find(|(id, _)| *id == field.id) {
        setting.notes.push((*reason).into());
        setting.writable = Writable::No;
        setting.state = State::ReadOnly;
        return setting;
    }
    setting.writable = match (field.kind, setting.range) {
        (Kind::Integer, Some(range)) if !range.usable() => {
            setting.notes.push(format!(
                "ADLX range {}..{} step {} leaves nothing to change",
                range.min, range.max, range.step
            ));
            Writable::No
        }
        (Kind::Integer, None) => Writable::Unverified,
        (Kind::Choice, _) if field.options_queried || PROVEN_CHOICES.contains(&field.id) => {
            Writable::Yes
        }
        (Kind::Choice, _) => {
            setting.notes.push(
                "ADLX has no query for the accepted options; the list is the SDK enum, so a write may still be refused for some of them".into(),
            );
            Writable::Unverified
        }
        _ => Writable::Yes,
    };
    if NEEDS_FEATURE_ON.contains(&field.id) {
        setting.notes.push(format!(
            "the driver refuses writes while {}.enabled is off; list {0}.enabled first in the same request",
            group.group
        ));
    }
    if let (Some(Value::Int(value)), Some(range)) = (&setting.value, setting.range)
        && (*value < range.min || *value > range.max)
    {
        setting
            .notes
            .push(format!("current value {value} is outside the ADLX range"));
    }
    setting.state = match setting.writable {
        Writable::Yes => State::Configurable,
        _ => State::ReadOnly,
    };
    setting
}

/// Whether a feature is on, for the dependency check. `None` when the probe has no answer.
fn group_active(settings: &[Setting], group: &str) -> Option<bool> {
    let find = |id: &str| settings.iter().find(|s| s.id == id);
    if group == "vsync" {
        return match find("vsync.mode")?.value.as_ref()? {
            Value::Choice(mode) => Some(matches!(
                mode.as_str(),
                "on_unless_app_specifies" | "always_on"
            )),
            _ => None,
        };
    }
    match find(&format!("{group}.enabled"))?.value.as_ref()? {
        Value::Bool(on) => Some(*on),
        _ => None,
    }
}

/// Attach the documented dependencies, with the current state of the other side.
pub fn apply_dependencies(settings: &mut [Setting]) {
    let active: Vec<_> = DEPENDENCIES
        .iter()
        .map(|(_, _, other, _)| group_active(settings, other))
        .collect();
    for setting in settings.iter_mut() {
        for ((group, relation, other, note), active) in DEPENDENCIES.iter().zip(&active) {
            if setting.group == *group {
                setting.depends.push(Dependency {
                    group: other,
                    relation: *relation,
                    note,
                    active: *active,
                });
            }
        }
    }
}

/// Markdown rows for `docs/hardware-matrix.md`.
pub fn markdown_table(settings: &[Setting]) -> String {
    let mut out = String::from(
        "| Setting | ADLX interface | Supported | Readable | Writable | State | Current value | Range or options | Notes |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for s in settings {
        let yes_no = |v: Option<bool>| match v {
            Some(true) => "yes",
            Some(false) => "no",
            None => "n/a",
        };
        let value = s.value.as_ref().map_or("-".into(), Value::display);
        let value = match (&s.value, s.unit) {
            (Some(Value::Int(_)), Some(unit)) => format!("{value} {unit}"),
            _ => value,
        };
        let range = match (&s.range, s.options.is_empty()) {
            (Some(r), _) => format!("{}..{} step {}", r.min, r.max, r.step),
            (None, false) => s
                .options
                .iter()
                .map(|o| o.value.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            (None, true) => "-".into(),
        };
        let mut notes: Vec<String> = s.notes.clone();
        for d in &s.depends {
            let verb = match d.relation {
                Relation::Excludes => "excludes",
                Relation::Requires => "requires",
            };
            let live = match d.active {
                Some(true) => " (on now)",
                Some(false) => " (off now)",
                None => "",
            };
            notes.push(format!("{verb} {}{live}", d.group));
        }
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} | {} | {} | {} |\n",
            s.id,
            s.interface,
            yes_no(s.supported),
            yes_no(s.readable),
            serde_json::to_value(s.writable)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            s.state.name(),
            value,
            range,
            notes.join("; ").replace('|', "/"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggle(id: &'static str, on: Query<bool>) -> FieldRaw {
        FieldRaw {
            id,
            label: id,
            kind: Kind::Toggle,
            unit: None,
            value: Some(on.map(Value::Bool)),
            range: None,
            options: vec![],
            options_queried: false,
            note: None,
        }
    }
    fn integer(id: &'static str, value: i64, range: Query<Range>) -> FieldRaw {
        FieldRaw {
            id,
            label: id,
            kind: Kind::Integer,
            unit: Some("fps"),
            value: Some(Ok(Value::Int(value))),
            range: Some(range),
            options: vec![],
            options_queried: false,
            note: None,
        }
    }
    fn choice(id: &'static str, items: EnumItems, raw: i32) -> FieldRaw {
        FieldRaw {
            id,
            label: id,
            kind: Kind::Choice,
            unit: None,
            value: Some(Ok(Value::Choice(enum_name(items, raw)))),
            range: None,
            options: enum_options(items),
            options_queried: false,
            note: None,
        }
    }
    fn group(
        group: &'static str,
        supported: Option<Query<bool>>,
        fields: Vec<FieldRaw>,
    ) -> GroupRaw {
        GroupRaw {
            group,
            interface: "IADLX3DFake",
            obtained: Ok(()),
            supported,
            fields,
        }
    }
    const RANGE: Range = Range {
        min: 30,
        max: 300,
        step: 1,
    };

    #[test]
    fn supported_toggle_is_configurable_with_its_value() {
        let s = &classify_group(&group(
            "anti_lag",
            Some(Ok(true)),
            vec![toggle("anti_lag.enabled", Ok(true))],
        ))[0];
        assert_eq!(s.state, State::Configurable);
        assert_eq!(s.supported, Some(true));
        assert_eq!(s.readable, Some(true));
        assert_eq!(s.writable, Writable::Yes);
        assert_eq!(s.value, Some(Value::Bool(true)));
    }

    #[test]
    fn unsupported_feature_has_no_value_and_is_not_writable() {
        let s = &classify_group(&group(
            "chill",
            Some(Ok(false)),
            vec![toggle("chill.enabled", Ok(true))],
        ))[0];
        assert_eq!(s.state, State::Unsupported);
        assert_eq!(s.supported, Some(false));
        assert_eq!(s.readable, Some(false));
        assert_eq!(s.writable, Writable::No);
        assert!(s.value.is_none());
    }

    #[test]
    fn missing_interface_and_not_supported_codes_are_unsupported() {
        for fault in [
            Fault::Missing,
            Fault::Code(6),
            Fault::Code(9),
            Fault::Code(12),
        ] {
            let mut g = group(
                "afmf",
                Some(Ok(true)),
                vec![toggle("afmf.enabled", Ok(true))],
            );
            g.obtained = Err(fault);
            assert_eq!(classify_group(&g)[0].state, State::Unsupported, "{fault:?}");
        }
        let g = group(
            "afmf",
            Some(Err(Fault::Code(12))),
            vec![toggle("afmf.enabled", Ok(true))],
        );
        assert_eq!(classify_group(&g)[0].state, State::Unsupported);
    }

    #[test]
    fn other_failures_are_temporarily_unavailable_and_loud() {
        let mut g = group(
            "afmf",
            Some(Ok(true)),
            vec![toggle("afmf.enabled", Ok(true))],
        );
        g.obtained = Err(Fault::Code(3));
        let s = &classify_group(&g)[0];
        assert_eq!(s.state, State::TemporarilyUnavailable);
        assert_eq!(s.supported, None);
        assert!(s.notes.iter().any(|n| n.contains("ADLX_FAIL")));
        let g = group(
            "afmf",
            Some(Ok(true)),
            vec![toggle("afmf.enabled", Err(Fault::Code(17)))],
        );
        let s = &classify_group(&g)[0];
        assert_eq!(s.state, State::TemporarilyUnavailable);
        assert_eq!(s.readable, Some(false));
        assert_eq!(s.adlx["read"], "ADLX_NOT_ACTIVE");
    }

    #[test]
    fn integer_with_a_usable_range_is_configurable() {
        let s = &classify_group(&group(
            "chill",
            Some(Ok(true)),
            vec![integer("chill.max_fps", 144, Ok(RANGE))],
        ))[0];
        assert_eq!(s.state, State::Configurable);
        assert_eq!(s.range, Some(RANGE));
        assert_eq!(s.writable, Writable::Yes);
    }

    #[test]
    fn empty_or_stepless_range_is_read_only_and_not_writable() {
        for range in [
            Range {
                min: 60,
                max: 60,
                step: 1,
            },
            Range {
                min: 30,
                max: 300,
                step: 0,
            },
            Range {
                min: 300,
                max: 30,
                step: 1,
            },
        ] {
            let s = &classify_group(&group(
                "chill",
                Some(Ok(true)),
                vec![integer("chill.max_fps", 60, Ok(range))],
            ))[0];
            assert_eq!(s.state, State::ReadOnly, "{range:?}");
            assert_eq!(s.writable, Writable::No);
            assert_eq!(s.readable, Some(true));
        }
    }

    #[test]
    fn failed_range_query_leaves_writability_unverified() {
        let s = &classify_group(&group(
            "chill",
            Some(Ok(true)),
            vec![integer("chill.max_fps", 144, Err(Fault::Code(3)))],
        ))[0];
        assert_eq!(s.state, State::ReadOnly);
        assert_eq!(s.writable, Writable::Unverified);
        assert_eq!(s.value, Some(Value::Int(144)));
        assert!(s.range.is_none());
    }

    #[test]
    fn enum_backed_choice_is_unverified_and_read_only_until_a_run_proves_it() {
        let s = &classify_group(&group(
            "sdk",
            Some(Ok(true)),
            vec![choice("sdk.unproven_mode", WAIT_FOR_VERTICAL_REFRESH, 1)],
        ))[0];
        assert_eq!(s.state, State::ReadOnly);
        assert_eq!(s.writable, Writable::Unverified);
        assert_eq!(
            s.value,
            Some(Value::Choice("off_unless_app_specifies".into()))
        );
        assert_eq!(s.options.len(), 4);
        assert!(s.notes.iter().any(|n| n.contains("no query")));
    }

    #[test]
    fn a_choice_proven_on_hardware_is_configurable() {
        let s = &classify_group(&group(
            "vsync",
            Some(Ok(true)),
            vec![choice("vsync.mode", WAIT_FOR_VERTICAL_REFRESH, 1)],
        ))[0];
        assert_eq!(s.state, State::Configurable);
        assert_eq!(s.writable, Writable::Yes);
        assert_eq!(s.options.len(), 4);
    }

    #[test]
    fn afmf_modes_say_they_need_afmf_on() {
        let s = &classify_group(&group(
            "afmf",
            Some(Ok(true)),
            vec![choice("afmf.search_mode", AFMF_SEARCH_MODE, 0)],
        ))[0];
        assert_eq!(s.state, State::Configurable);
        assert!(s.notes.iter().any(|n| n.contains("afmf.enabled")));
    }

    #[test]
    fn levels_that_cannot_be_written_back_are_never_configurable() {
        for (id, items) in [
            ("anti_aliasing.level", AA_LEVEL),
            ("anisotropic_filtering.level", AF_LEVEL),
        ] {
            let s = &classify_group(&group("level", Some(Ok(true)), vec![choice(id, items, 0)]))[0];
            assert_eq!(s.state, State::ReadOnly, "{id}");
            assert_eq!(s.writable, Writable::No, "{id}");
            assert!(s.notes.iter().any(|n| n.contains("could not be restored")));
        }
    }

    #[test]
    fn queried_option_list_makes_a_choice_writable() {
        let mut field = choice("fidelityfx_frame_gen.ratio", FFX_FRAME_GEN_RATIO, 1);
        field.options_queried = true;
        let s = &classify_group(&group("fidelityfx_frame_gen", Some(Ok(true)), vec![field]))[0];
        assert_eq!(s.writable, Writable::Yes);
        assert_eq!(s.state, State::Configurable);
    }

    #[test]
    fn action_has_no_value_and_follows_supported() {
        let action = FieldRaw {
            id: "reset_shader_cache.run",
            label: "reset",
            kind: Kind::Action,
            unit: None,
            value: None,
            range: None,
            options: vec![],
            options_queried: false,
            note: None,
        };
        let s = &classify_group(&group("reset_shader_cache", Some(Ok(true)), vec![action]))[0];
        assert_eq!(s.readable, None);
        assert_eq!(s.state, State::Configurable);
        assert!(s.value.is_none());
    }

    #[test]
    fn out_of_range_value_is_noted_not_hidden() {
        let s = &classify_group(&group(
            "chill",
            Some(Ok(true)),
            vec![integer("chill.max_fps", 400, Ok(RANGE))],
        ))[0];
        assert_eq!(s.value, Some(Value::Int(400)));
        assert!(s.notes.iter().any(|n| n.contains("outside")));
    }

    #[test]
    fn enum_names_cover_known_and_unknown_values() {
        assert_eq!(enum_name(WAIT_FOR_VERTICAL_REFRESH, 3), "always_on");
        assert_eq!(enum_name(ANTILAG_STATE, 1), "anti_lag_next");
        assert_eq!(enum_name(AF_LEVEL, 16), "16x");
        assert_eq!(enum_name(AA_LEVEL, 0), "invalid");
        assert_eq!(enum_name(AA_LEVEL, 99), "unknown(99)");
        assert_eq!(enum_name(TESSELLATION_LEVEL, 99), "unknown(99)");
        assert_eq!(result_name(12), "ADLX_NOT_SUPPORTED");
        assert_eq!(result_name(77), "ADLX_RESULT(77)");
    }

    #[test]
    fn dependencies_report_whether_the_other_side_is_on() {
        let mut settings = vec![];
        settings.extend(classify_group(&group(
            "anti_lag",
            Some(Ok(true)),
            vec![toggle("anti_lag.enabled", Ok(false))],
        )));
        settings.extend(classify_group(&group(
            "chill",
            Some(Ok(true)),
            vec![toggle("chill.enabled", Ok(true))],
        )));
        settings.extend(classify_group(&group(
            "enhanced_sync",
            Some(Ok(true)),
            vec![toggle("enhanced_sync.enabled", Ok(false))],
        )));
        settings.extend(classify_group(&group(
            "vsync",
            Some(Ok(true)),
            vec![choice("vsync.mode", WAIT_FOR_VERTICAL_REFRESH, 3)],
        )));
        apply_dependencies(&mut settings);
        let by_id = |id: &str| settings.iter().find(|s| s.id == id).unwrap();
        let anti_lag = by_id("anti_lag.enabled");
        assert_eq!(anti_lag.depends.len(), 1);
        assert_eq!(anti_lag.depends[0].group, "chill");
        assert_eq!(anti_lag.depends[0].active, Some(true));
        let chill = by_id("chill.enabled");
        let others: Vec<_> = chill.depends.iter().map(|d| (d.group, d.active)).collect();
        assert_eq!(others, vec![("anti_lag", Some(false)), ("boost", None)]);
        assert_eq!(by_id("enhanced_sync.enabled").depends[0].active, Some(true));
        assert_eq!(by_id("vsync.mode").depends[0].active, Some(false));
    }

    #[test]
    fn json_uses_snake_case_states_and_omits_absent_fields() {
        let s = classify_group(&group(
            "chill",
            Some(Ok(false)),
            vec![toggle("chill.enabled", Ok(true))],
        ));
        let json = serde_json::to_value(&s[0]).unwrap();
        assert_eq!(json["state"], "unsupported");
        assert_eq!(json["writable"], "no");
        assert!(json.get("value").is_none());
        assert_eq!(json["adlx"]["is_supported"], "ADLX_OK");
    }

    #[test]
    fn markdown_rows_carry_state_value_range_and_notes() {
        let mut settings = classify_group(&group(
            "chill",
            Some(Ok(true)),
            vec![integer("chill.max_fps", 144, Ok(RANGE))],
        ));
        apply_dependencies(&mut settings);
        let table = markdown_table(&settings);
        let row = table.lines().last().unwrap();
        assert!(row.starts_with("| `chill.max_fps` |"), "{row}");
        assert!(
            row.contains("| yes | yes | yes | configurable | 144 fps | 30..300 step 1 |"),
            "{row}"
        );
        assert!(row.contains("excludes anti_lag"), "{row}");
    }
}
