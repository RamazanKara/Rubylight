//! The Moonlight WebRTC TV protocol, version 2: every message the signalling
//! WebSocket (and the discovery datagram) carries, and the error codes.
//!
//! The contract is `docs/protocol.md` of Monlight-Tizen-WebRtc-Client at the
//! pin in `docs/webrtc-design.md`. Every message holds `"version": 2` and a
//! `type`. Fields we don't know are ignored on the way in, so a client newer
//! than this host still parses; a wrong version is refused.
use super::capabilities::CapabilitiesBody;
use serde::de::{Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub const VERSION: u64 = 2;
/// A `client-log` message carries at most this many lines.
pub const MAX_CLIENT_LOG_LINES: usize = 100;
/// Longer `client-log` lines are cut.
pub const MAX_CLIENT_LOG_LINE_LENGTH: usize = 4000;
/// The sample rate `session-status` reports for the stereo Opus stream.
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// `clientId` is 16 random bytes in hex.
pub const MAX_CLIENT_ID_LENGTH: usize = 64;
/// `proof` is 32 bytes in hex.
pub const MAX_PROOF_LENGTH: usize = 128;

/// The `code` of an `error` message. The strings are the protocol's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    InvalidJson,
    UnsupportedVersion,
    InvalidMessage,
    UnsupportedMessage,
    UnsupportedPairing,
    UnsupportedSettings,
    NotAuthenticated,
    AlreadyAuthenticated,
    AuthenticationFailed,
    ClientDisabled,
    IncorrectPin,
    PairingCancelled,
    PairingExpired,
    PairingFailed,
    SunshineUnavailable,
    HostOperationActive,
    SessionActive,
    InvalidApplication,
    InternalError,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 19] = [
        Self::InvalidJson,
        Self::UnsupportedVersion,
        Self::InvalidMessage,
        Self::UnsupportedMessage,
        Self::UnsupportedPairing,
        Self::UnsupportedSettings,
        Self::NotAuthenticated,
        Self::AlreadyAuthenticated,
        Self::AuthenticationFailed,
        Self::ClientDisabled,
        Self::IncorrectPin,
        Self::PairingCancelled,
        Self::PairingExpired,
        Self::PairingFailed,
        Self::SunshineUnavailable,
        Self::HostOperationActive,
        Self::SessionActive,
        Self::InvalidApplication,
        Self::InternalError,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid-json",
            Self::UnsupportedVersion => "unsupported-version",
            Self::InvalidMessage => "invalid-message",
            Self::UnsupportedMessage => "unsupported-message",
            Self::UnsupportedPairing => "unsupported-pairing",
            Self::UnsupportedSettings => "unsupported-settings",
            Self::NotAuthenticated => "not-authenticated",
            Self::AlreadyAuthenticated => "already-authenticated",
            Self::AuthenticationFailed => "authentication-failed",
            Self::ClientDisabled => "client-disabled",
            Self::IncorrectPin => "incorrect-pin",
            Self::PairingCancelled => "pairing-cancelled",
            Self::PairingExpired => "pairing-expired",
            Self::PairingFailed => "pairing-failed",
            Self::SunshineUnavailable => "sunshine-unavailable",
            Self::HostOperationActive => "host-operation-active",
            Self::SessionActive => "session-active",
            Self::InvalidApplication => "invalid-application",
            Self::InternalError => "internal-error",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refusal with the code the client sees and a message that is safe to
/// send: it never holds a credential or a pairing secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The `error` message for a request of the given type. Errors in
    /// messages that could not be parsed use `"unknown"`, as the reference does.
    pub fn into_message(self, request_type: &str) -> ServerMessage {
        ServerMessage::error(request_type, self.code, self.message)
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

/// An integer of any sign and size. Out-of-range numbers are the client's
/// settings being unsupported (`unsupported-settings`), not a malformed
/// message, so they must get past parsing: one above `i64::MAX` saturates.
fn any_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    struct Integer;
    impl Visitor<'_> for Integer {
        type Value = i64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an integer")
        }

        fn visit_i64<E>(self, value: i64) -> Result<i64, E> {
            Ok(value)
        }

        fn visit_u64<E>(self, value: u64) -> Result<i64, E> {
            Ok(i64::try_from(value).unwrap_or(i64::MAX))
        }
    }
    deserializer.deserialize_any(Integer)
}

/// A credential that never shows in `Debug` output (and so never in a log).
/// It serializes as the plain string.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The `video` object of `start-session` and `switch-session`, and of
/// `session-status`. The codec stays a string so that an unknown one is
/// `unsupported-settings`, not a malformed message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSettings {
    #[serde(deserialize_with = "any_integer")]
    pub width: i64,
    #[serde(deserialize_with = "any_integer")]
    pub height: i64,
    #[serde(deserialize_with = "any_integer")]
    pub fps: i64,
    pub codec: String,
    #[serde(deserialize_with = "any_integer")]
    pub bitrate_kbps: i64,
    pub hdr: bool,
}

/// The `audio` object. `sampleRate` appears only in `session-status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    #[serde(deserialize_with = "any_integer")]
    pub channels: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

/// What the client sends. The `type` values are the protocol's. `pair-client`
/// has no variant: it only ever gets `unsupported-pairing`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ClientMessage {
    Authenticate {
        client_id: String,
        proof: String,
    },
    RequestPairing {
        pin: String,
        #[serde(default)]
        client_name: String,
    },
    GetApps,
    GetAppArtwork {
        app_id: String,
    },
    StopSession,
    StopHostSession,
    StartSession {
        app_id: String,
        video: VideoSettings,
        audio: AudioSettings,
    },
    SwitchSession {
        app_id: String,
        video: VideoSettings,
        audio: AudioSettings,
    },
    Answer {
        session_id: u64,
        sdp: String,
    },
    Candidate {
        session_id: u64,
        candidate: String,
        mid: String,
    },
    ClientLog {
        lines: Vec<String>,
    },
}

const CLIENT_TYPES: [&str; 11] = [
    "authenticate",
    "request-pairing",
    "get-apps",
    "get-app-artwork",
    "stop-session",
    "stop-host-session",
    "start-session",
    "switch-session",
    "answer",
    "candidate",
    "client-log",
];

fn invalid(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidMessage, message)
}

/// Refuse anything but protocol version 2 (an integer, not `2.0`).
fn require_version(value: &Value) -> Result<(), ProtocolError> {
    match value.get("version").and_then(Value::as_u64) {
        Some(VERSION) => Ok(()),
        _ => Err(ProtocolError::new(
            ErrorCode::UnsupportedVersion,
            "Protocol version 2 is required",
        )),
    }
}

fn parse_value(text: &str) -> Result<Value, ProtocolError> {
    serde_json::from_str(text)
        .map_err(|_| ProtocolError::new(ErrorCode::InvalidJson, "Message is not valid JSON"))
}

/// Add `"version": 2` to a serialized message.
fn versioned(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.insert("version".into(), VERSION.into());
    }
    value
}

impl ClientMessage {
    /// The `type` string, which an `error` reply names as `requestType`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Authenticate { .. } => "authenticate",
            Self::RequestPairing { .. } => "request-pairing",
            Self::GetApps => "get-apps",
            Self::GetAppArtwork { .. } => "get-app-artwork",
            Self::StopSession => "stop-session",
            Self::StopHostSession => "stop-host-session",
            Self::StartSession { .. } => "start-session",
            Self::SwitchSession { .. } => "switch-session",
            Self::Answer { .. } => "answer",
            Self::Candidate { .. } => "candidate",
            Self::ClientLog { .. } => "client-log",
        }
    }

    /// The message as the client would send it, `version` included.
    pub fn to_json(&self) -> String {
        versioned(serde_json::to_value(self).expect("client message serializes")).to_string()
    }

    fn check(&mut self) -> Result<(), ProtocolError> {
        match self {
            Self::Authenticate { client_id, proof } => {
                if client_id.is_empty()
                    || client_id.len() > MAX_CLIENT_ID_LENGTH
                    || proof.is_empty()
                    || proof.len() > MAX_PROOF_LENGTH
                {
                    return Err(invalid("clientId and proof are required"));
                }
            }
            Self::RequestPairing { pin, .. } => {
                if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid("pin must have four digits"));
                }
            }
            Self::GetAppArtwork { app_id }
            | Self::StartSession { app_id, .. }
            | Self::SwitchSession { app_id, .. } => {
                if app_id.is_empty() {
                    return Err(invalid("appId must not be empty"));
                }
            }
            Self::Answer { session_id, .. } | Self::Candidate { session_id, .. } => {
                if *session_id == 0 {
                    return Err(invalid("A positive sessionId is required"));
                }
            }
            Self::ClientLog { lines } => {
                if lines.len() > MAX_CLIENT_LOG_LINES {
                    return Err(invalid("lines must be an array of at most 100 strings"));
                }
                // A line stays one line in the host log: control characters
                // would let it pass itself off as several.
                for line in lines {
                    *line = sanitize_log_line(line);
                }
            }
            Self::GetApps | Self::StopSession | Self::StopHostSession => {}
        }
        Ok(())
    }
}

fn sanitize_log_line(line: &str) -> String {
    let mut end = line.len().min(MAX_CLIENT_LOG_LINE_LENGTH);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end]
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Parse and check one WebSocket text message from a client. The settings of
/// `start-session` and `switch-session` are checked separately, against what
/// this host offers (`HostCapabilities::validate`).
pub fn parse_client_message(text: &str) -> Result<ClientMessage, ProtocolError> {
    let value = parse_value(text)?;
    require_version(&value)?;
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return Err(invalid("Message fields are invalid"));
    };
    if kind == "pair-client" {
        // TV apps from before the PIN moved to the TV entered one shown on the PC.
        return Err(ProtocolError::new(
            ErrorCode::UnsupportedPairing,
            "Update the TV app: this host pairs TVs with a PIN shown on the TV",
        ));
    }
    if !CLIENT_TYPES.contains(&kind) {
        return Err(ProtocolError::new(
            ErrorCode::UnsupportedMessage,
            format!("Unsupported message type: {kind}"),
        ));
    }
    let mut message: ClientMessage =
        serde_json::from_value(value).map_err(|_| invalid("Message fields are invalid"))?;
    message.check()?;
    Ok(message)
}

/// Is this datagram a `discover` broadcast?
pub fn is_discovery_request(datagram: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(datagram) else {
        return false;
    };
    require_version(&value).is_ok() && value.get("type").and_then(Value::as_str) == Some("discover")
}

/// Is this DataChannel text a `request-keyframe`?
pub fn is_keyframe_request(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .is_some_and(|v| v.get("type").and_then(Value::as_str) == Some("request-keyframe"))
}

/// What the host does with a text message on the `control` DataChannel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMessage {
    KeyframeRequest,
    /// A `gamepad-connected` or `gamepad-disconnected` announcement. Clients
    /// following the reference host send these here, not on `gamepad`.
    GamepadAnnouncement,
    Other,
}

/// Sorts a `control` channel text message.
pub fn classify_control(text: &str) -> ControlMessage {
    let kind = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned));
    match kind.as_deref() {
        Some("request-keyframe") => ControlMessage::KeyframeRequest,
        Some("gamepad-connected" | "gamepad-disconnected") => ControlMessage::GamepadAnnouncement,
        _ => ControlMessage::Other,
    }
}

/// One entry of the `apps` list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub id: String,
    pub title: String,
    pub artwork_available: bool,
    pub running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Idle,
    Starting,
    StartingWebrtc,
    ConnectingSunshine,
    StartingMoonlight,
    Streaming,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostSessionState {
    Stopping,
    Switching,
    Resuming,
    Starting,
    Stopped,
    Failed,
}

/// What the host sends: over the WebSocket, and `discovery` over UDP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ServerMessage {
    Discovery {
        name: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac_address: Option<String>,
    },
    AuthRequired {
        nonce: String,
        pairing: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac_address: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sunshine_available: Option<bool>,
    },
    SunshineAvailability {
        sunshine_available: bool,
    },
    Authenticated,
    Paired {
        client_id: String,
        client_secret: Secret,
    },
    GatewayStatus {
        gateway_name: String,
        sunshine_detected: bool,
        sunshine_paired: bool,
        session_active: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        running_app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac_address: Option<String>,
    },
    Capabilities(CapabilitiesBody),
    Apps {
        apps: Vec<AppEntry>,
    },
    AppArtwork {
        app_id: String,
        available: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    SessionStatus {
        state: SessionState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        video: Option<VideoSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<AudioSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    HostSessionStatus {
        state: HostSessionState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        running_app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Error {
        request_type: String,
        code: ErrorCode,
        message: String,
    },
    Offer {
        session_id: u64,
        sdp: String,
    },
    Candidate {
        session_id: u64,
        candidate: String,
        mid: String,
    },
}

impl ServerMessage {
    pub fn discovery(name: impl Into<String>, port: u16, mac_address: Option<String>) -> Self {
        Self::Discovery {
            name: name.into(),
            port,
            mac_address,
        }
    }

    /// The first message on every connection: the single-use nonce.
    pub fn auth_required(nonce_hex: impl Into<String>, mac_address: Option<String>) -> Self {
        Self::AuthRequired {
            nonce: nonce_hex.into(),
            pairing: "client-pin".into(),
            mac_address,
            sunshine_available: Some(true),
        }
    }

    pub fn paired(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::Paired {
            client_id: client_id.into(),
            client_secret: Secret(client_secret.into()),
        }
    }

    pub fn error(request_type: &str, code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            request_type: request_type.into(),
            code,
            message: message.into(),
        }
    }

    pub fn session_status(state: SessionState, session_id: Option<u64>) -> Self {
        Self::SessionStatus {
            state,
            session_id,
            video: None,
            audio: None,
            message: None,
        }
    }

    pub fn offer(session_id: u64, sdp: impl Into<String>) -> Self {
        Self::Offer {
            session_id,
            sdp: sdp.into(),
        }
    }

    pub fn candidate(
        session_id: u64,
        candidate: impl Into<String>,
        mid: impl Into<String>,
    ) -> Self {
        Self::Candidate {
            session_id,
            candidate: candidate.into(),
            mid: mid.into(),
        }
    }

    /// The JSON text to send, `version` included.
    pub fn to_json(&self) -> String {
        versioned(serde_json::to_value(self).expect("server message serializes")).to_string()
    }

    /// Parse a host message (for clients and tests).
    pub fn from_json(text: &str) -> Result<Self, ProtocolError> {
        let value = parse_value(text)?;
        require_version(&value)?;
        serde_json::from_value(value).map_err(|_| invalid("Message fields are invalid"))
    }
}

#[cfg(test)]
mod tests;
