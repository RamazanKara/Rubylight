//! `GET /radeon` and the settings writes.
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serialize};

/// Reads an optional field of a response, and takes a shape this version
/// doesn't know (a float, an array, an object) as absent instead of failing
/// the whole response. Requests don't use it: they stay strict.
fn lenient<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient<T> {
        Known(T),
        Other(IgnoredAny),
    }
    Ok(match Option::<Lenient<T>>::deserialize(d)? {
        Some(Lenient::Known(value)) => Some(value),
        Some(Lenient::Other(_)) | None => None,
    })
}

/// One of the five states of a setting, and of the whole panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlState {
    Configurable,
    ReadOnly,
    NeedsExternalSetup,
    Unsupported,
    TemporarilyUnavailable,
    /// A state from a newer host: clients treat it as `unsupported`.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Toggle,
    Integer,
    Choice,
    #[serde(other)]
    Unknown,
}

/// A setting value. Strict: a request with any other shape is refused. In
/// responses, `Setting` reads an unknown shape as an absent value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Choice(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub min: i64,
    pub max: i64,
    pub step: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

/// A pending change: what the host goes back to at session end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restore {
    pub original: Value,
    pub written: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    /// `<group>.<field>`. Clients render ids they don't know from `kind`,
    /// `range`, `options` and `unit`.
    pub id: String,
    pub group: String,
    pub kind: Kind,
    pub state: ControlState,
    /// A value of a shape this version doesn't know reads as `None`.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<Value>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub restore: Option<Restore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    /// The `id` that `/api/hardware-metrics` reports.
    pub id: u64,
    pub name: String,
    pub driver: String,
    pub settings: Vec<Setting>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Radeon {
    pub revision: u64,
    pub state: ControlState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The caller's flag.
    pub host_control: bool,
    /// True only with host control and an owned `streaming` or `suspended`
    /// session.
    pub writable_now: bool,
    pub gpus: Vec<Gpu>,
}

/// `POST /radeon/settings`. A request: unknown fields are rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsWrite {
    pub gpu: u64,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: String,
    pub value: Value,
}
