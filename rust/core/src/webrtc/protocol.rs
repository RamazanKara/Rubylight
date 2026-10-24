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
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_err(text: &str) -> ErrorCode {
        parse_client_message(text).unwrap_err().code
    }

    /// A host message parses, and serializes back to the same JSON value.
    fn round_trip_server(text: &str) -> ServerMessage {
        let message = ServerMessage::from_json(text).unwrap();
        let original: Value = serde_json::from_str(text).unwrap();
        let again: Value = serde_json::from_str(&message.to_json()).unwrap();
        assert_eq!(original, again, "{text}");
        message
    }

    /// A client message parses, and serializes back to the same JSON value.
    fn round_trip_client(text: &str) -> ClientMessage {
        let message = parse_client_message(text).unwrap();
        let original: Value = serde_json::from_str(text).unwrap();
        let again: Value = serde_json::from_str(&message.to_json()).unwrap();
        assert_eq!(original, again, "{text}");
        message
    }

    #[test]
    fn debug_output_never_shows_the_client_secret() {
        let secret = "9f8e7d6c5b4a39281706f5e4d3c2b1a0";
        let message = ServerMessage::paired("client-1", secret);
        assert!(!format!("{message:?}").contains(secret));
        assert!(format!("{message:?}").contains("<redacted>"));
        // The wire form still carries it.
        assert!(message.to_json().contains(secret));
    }

    #[test]
    fn negative_and_huge_numbers_parse_so_validation_can_refuse_them() {
        let text = r#"{"version":2,"type":"start-session","appId":"0","video":{"width":-1,"height":1080,
            "fps":99999999999,"codec":"h264","bitrateKbps":-5,"hdr":false},
            "audio":{"channels":-2}}"#;
        match parse_client_message(text).unwrap() {
            ClientMessage::StartSession { video, audio, .. } => {
                assert_eq!(video.width, -1);
                assert_eq!(video.fps, 99_999_999_999);
                assert_eq!(video.bitrate_kbps, -5);
                assert_eq!(audio.channels, -2);
            }
            other => panic!("{other:?}"),
        }
        // A non-number is still an invalid message.
        let text = text.replace("\"width\":-1", "\"width\":\"wide\"");
        assert_eq!(parse_err(&text), ErrorCode::InvalidMessage);
    }

    #[test]
    fn error_codes_match_the_protocol_strings() {
        for code in ErrorCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        }
        let names: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        for expected in [
            "invalid-json",
            "unsupported-version",
            "invalid-message",
            "unsupported-message",
            "unsupported-pairing",
            "unsupported-settings",
            "not-authenticated",
            "already-authenticated",
            "authentication-failed",
            "client-disabled",
            "incorrect-pin",
            "pairing-cancelled",
            "pairing-expired",
            "pairing-failed",
            "sunshine-unavailable",
            "host-operation-active",
            "session-active",
            "invalid-application",
            "internal-error",
        ] {
            assert!(names.contains(&expected), "{expected}");
        }
        assert_eq!(names.len(), 19);
    }

    #[test]
    fn doc_discovery_request_and_reply() {
        assert!(is_discovery_request(r#"{"version":2,"type":"discover"}"#));
        assert!(!is_discovery_request(r#"{"version":1,"type":"discover"}"#));
        assert!(!is_discovery_request(
            r#"{"version":2,"type":"authenticate"}"#
        ));
        assert!(!is_discovery_request("not json"));
        let reply = round_trip_server(
            r#"{"version":2,"type":"discovery","name":"Sunshine-PC","port":8000,"macAddress":"2C:F0:5D:7B:E6:D0"}"#,
        );
        assert_eq!(
            reply,
            ServerMessage::discovery("Sunshine-PC", 8000, Some("2C:F0:5D:7B:E6:D0".into()))
        );
        // macAddress is omitted when unknown.
        let bare = ServerMessage::discovery("Rubylight", 8000, None).to_json();
        assert!(!bare.contains("macAddress"));
    }

    #[test]
    fn doc_auth_required_and_authenticate() {
        let nonce = "ab".repeat(32);
        let text = format!(
            r#"{{"version":2,"type":"auth-required","nonce":"{nonce}","pairing":"client-pin","macAddress":"2C:F0:5D:7B:E6:D0","sunshineAvailable":true}}"#
        );
        let message = round_trip_server(&text);
        assert_eq!(
            message,
            ServerMessage::auth_required(nonce, Some("2C:F0:5D:7B:E6:D0".into()))
        );
        let auth = format!(
            r#"{{"version":2,"type":"authenticate","clientId":"{}","proof":"{}"}}"#,
            "0f".repeat(16),
            "cd".repeat(32)
        );
        let ClientMessage::Authenticate { client_id, proof } = round_trip_client(&auth) else {
            panic!("not authenticate");
        };
        assert_eq!(client_id.len(), 32);
        assert_eq!(proof.len(), 64);
        round_trip_server(r#"{"version":2,"type":"authenticated"}"#);
        round_trip_server(
            r#"{"version":2,"type":"sunshine-availability","sunshineAvailable":true}"#,
        );
    }

    #[test]
    fn doc_pairing_messages() {
        let ClientMessage::RequestPairing { pin, client_name } = round_trip_client(
            r#"{"version":2,"type":"request-pairing","pin":"0421","clientName":"Samsung TV"}"#,
        ) else {
            panic!("not request-pairing");
        };
        assert_eq!((pin.as_str(), client_name.as_str()), ("0421", "Samsung TV"));
        // The name is optional.
        let ClientMessage::RequestPairing { client_name, .. } =
            parse_client_message(r#"{"version":2,"type":"request-pairing","pin":"0421"}"#).unwrap()
        else {
            panic!("not request-pairing");
        };
        assert_eq!(client_name, "");
        let text = format!(
            r#"{{"version":2,"type":"paired","clientId":"{}","clientSecret":"{}"}}"#,
            "0f".repeat(16),
            "cd".repeat(32)
        );
        assert!(matches!(
            round_trip_server(&text),
            ServerMessage::Paired { .. }
        ));
        for pin in ["042", "04211", "04a1", "٠٤٢١", ""] {
            let text = json!({"version": 2, "type": "request-pairing", "pin": pin}).to_string();
            assert_eq!(parse_err(&text), ErrorCode::InvalidMessage, "{pin}");
        }
    }

    #[test]
    fn pair_client_is_unsupported_pairing() {
        assert_eq!(
            parse_err(r#"{"version":2,"type":"pair-client","pin":"1234"}"#),
            ErrorCode::UnsupportedPairing
        );
    }

    #[test]
    fn doc_gateway_status() {
        let message = round_trip_server(
            r#"{"version":2,"type":"gateway-status","gatewayName":"Sunshine-PC","sunshineDetected":true,"sunshinePaired":true,"sessionActive":false,"runningAppId":"7","macAddress":"2C:F0:5D:7B:E6:D0"}"#,
        );
        assert!(matches!(
            message,
            ServerMessage::GatewayStatus {
                session_active: false,
                running_app_id: Some(_),
                ..
            }
        ));
        // runningAppId and macAddress are optional.
        round_trip_server(
            r#"{"version":2,"type":"gateway-status","gatewayName":"PC","sunshineDetected":true,"sunshinePaired":true,"sessionActive":true}"#,
        );
    }

    #[test]
    fn doc_apps_and_artwork() {
        assert_eq!(
            round_trip_client(r#"{"version":2,"type":"get-apps"}"#),
            ClientMessage::GetApps
        );
        let apps = round_trip_server(
            r#"{"version":2,"type":"apps","apps":[{"id":"0","title":"Desktop","artworkAvailable":false,"running":false}]}"#,
        );
        let ServerMessage::Apps { apps } = apps else {
            panic!("not apps");
        };
        assert_eq!(apps[0].title, "Desktop");
        assert_eq!(
            round_trip_client(r#"{"version":2,"type":"get-app-artwork","appId":"0"}"#),
            ClientMessage::GetAppArtwork { app_id: "0".into() }
        );
        round_trip_server(
            r#"{"version":2,"type":"app-artwork","appId":"0","available":true,"mimeType":"image/jpeg","data":"AAEC"}"#,
        );
        round_trip_server(r#"{"version":2,"type":"app-artwork","appId":"0","available":false}"#);
        assert_eq!(
            parse_err(r#"{"version":2,"type":"get-app-artwork","appId":""}"#),
            ErrorCode::InvalidMessage
        );
        assert_eq!(
            parse_err(r#"{"version":2,"type":"get-app-artwork","appId":7}"#),
            ErrorCode::InvalidMessage
        );
    }

    const START: &str = r#"{
      "version": 2,
      "type": "start-session",
      "appId": "0",
      "video": {
        "width": 3840,
        "height": 2160,
        "fps": 120,
        "codec": "hevc",
        "bitrateKbps": 50000,
        "hdr": true
      },
      "audio": {"channels": 2}
    }"#;

    const SWITCH: &str = r#"{
      "version":2,
      "type":"switch-session",
      "appId":"7",
      "video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},
      "audio":{"channels":2}
    }"#;

    #[test]
    fn doc_start_and_switch_session() {
        let ClientMessage::StartSession {
            app_id,
            video,
            audio,
        } = round_trip_client(START)
        else {
            panic!("not start-session");
        };
        assert_eq!(app_id, "0");
        assert_eq!(
            video,
            VideoSettings {
                width: 3840,
                height: 2160,
                fps: 120,
                codec: "hevc".into(),
                bitrate_kbps: 50000,
                hdr: true,
            }
        );
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.sample_rate, None);
        let ClientMessage::SwitchSession { app_id, video, .. } = round_trip_client(SWITCH) else {
            panic!("not switch-session");
        };
        assert_eq!((app_id.as_str(), video.fps, video.hdr), ("7", 60, false));
    }

    #[test]
    fn session_settings_missing_a_field_are_invalid_messages() {
        let mut value: Value = serde_json::from_str(START).unwrap();
        value["video"].as_object_mut().unwrap().remove("hdr");
        assert_eq!(parse_err(&value.to_string()), ErrorCode::InvalidMessage);
        let mut value: Value = serde_json::from_str(START).unwrap();
        value.as_object_mut().unwrap().remove("audio");
        assert_eq!(parse_err(&value.to_string()), ErrorCode::InvalidMessage);
        let mut value: Value = serde_json::from_str(START).unwrap();
        value["video"]["fps"] = json!("sixty");
        assert_eq!(parse_err(&value.to_string()), ErrorCode::InvalidMessage);
    }

    #[test]
    fn doc_session_status_and_host_session_status() {
        round_trip_server(r#"{"version":2,"type":"session-status","state":"idle"}"#);
        for state in [
            "idle",
            "starting",
            "starting-webrtc",
            "connecting-sunshine",
            "starting-moonlight",
            "streaming",
            "stopping",
            "error",
        ] {
            round_trip_server(
                &json!({"version": 2, "type": "session-status", "state": state, "sessionId": 3})
                    .to_string(),
            );
        }
        let streaming = round_trip_server(
            r#"{"version":2,"type":"session-status","state":"streaming","sessionId":3,"video":{"width":1920,"height":1080,"fps":60,"codec":"h264","bitrateKbps":20000,"hdr":false},"audio":{"channels":2,"sampleRate":48000},"message":"ok"}"#,
        );
        assert!(matches!(
            streaming,
            ServerMessage::SessionStatus {
                state: SessionState::Streaming,
                session_id: Some(3),
                ..
            }
        ));
        for state in [
            "stopping",
            "switching",
            "resuming",
            "starting",
            "stopped",
            "failed",
        ] {
            round_trip_server(
                &json!({"version": 2, "type": "host-session-status", "state": state,
                    "runningAppId": "7", "targetAppId": "8", "message": "m"})
                .to_string(),
            );
        }
        round_trip_server(r#"{"version":2,"type":"host-session-status","state":"stopped"}"#);
    }

    #[test]
    fn doc_stop_messages() {
        assert_eq!(
            round_trip_client(r#"{"version":2,"type":"stop-session"}"#),
            ClientMessage::StopSession
        );
        assert_eq!(
            round_trip_client(r#"{"version":2,"type":"stop-host-session"}"#),
            ClientMessage::StopHostSession
        );
    }

    #[test]
    fn doc_error_message() {
        let message = round_trip_server(
            r#"{"version":2,"type":"error","requestType":"start-session","code":"unsupported-settings","message":"Unsupported resolution"}"#,
        );
        assert_eq!(
            message,
            ServerMessage::error(
                "start-session",
                ErrorCode::UnsupportedSettings,
                "Unsupported resolution"
            )
        );
        let reply =
            ProtocolError::new(ErrorCode::ClientDisabled, "disabled").into_message("authenticate");
        assert!(reply.to_json().contains(r#""code":"client-disabled""#));
    }

    #[test]
    fn signalling_messages_carry_a_session_id() {
        let offer = ServerMessage::offer(4, "v=0");
        assert_eq!(round_trip_server(&offer.to_json()), offer);
        let candidate =
            ServerMessage::candidate(4, "candidate:1 1 udp 1 10.0.0.2 8001 typ host", "0");
        assert_eq!(round_trip_server(&candidate.to_json()), candidate);
        let answer =
            round_trip_client(r#"{"version":2,"type":"answer","sessionId":4,"sdp":"v=0"}"#);
        assert_eq!(answer.type_name(), "answer");
        let candidate = round_trip_client(
            r#"{"version":2,"type":"candidate","sessionId":4,"candidate":"candidate:1","mid":"0"}"#,
        );
        assert_eq!(candidate.type_name(), "candidate");
        for text in [
            r#"{"version":2,"type":"answer","sessionId":0,"sdp":"v=0"}"#,
            r#"{"version":2,"type":"answer","sessionId":-1,"sdp":"v=0"}"#,
            r#"{"version":2,"type":"answer","sdp":"v=0"}"#,
            r#"{"version":2,"type":"candidate","sessionId":0,"candidate":"c","mid":"0"}"#,
        ] {
            assert_eq!(parse_err(text), ErrorCode::InvalidMessage, "{text}");
        }
    }

    #[test]
    fn client_log_lines_are_bounded_and_single_line() {
        let text = json!({"version": 2, "type": "client-log",
            "lines": ["ok", "a\nb\u{7f}c", "x".repeat(MAX_CLIENT_LOG_LINE_LENGTH + 50), "é".repeat(MAX_CLIENT_LOG_LINE_LENGTH)]})
        .to_string();
        let ClientMessage::ClientLog { lines } = parse_client_message(&text).unwrap() else {
            panic!("not client-log");
        };
        assert_eq!(lines[0], "ok");
        assert_eq!(lines[1], "a b c");
        assert_eq!(lines[2].len(), MAX_CLIENT_LOG_LINE_LENGTH);
        assert!(lines[3].len() <= MAX_CLIENT_LOG_LINE_LENGTH);
        assert!(lines[3].chars().all(|c| c == 'é'));
        let many = json!({"version": 2, "type": "client-log", "lines": vec!["x"; 101]});
        assert_eq!(parse_err(&many.to_string()), ErrorCode::InvalidMessage);
        let not_strings = json!({"version": 2, "type": "client-log", "lines": [1]});
        assert_eq!(
            parse_err(&not_strings.to_string()),
            ErrorCode::InvalidMessage
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let text = r#"{"version":2,"type":"stop-session","future":{"a":1},"x":[]}"#;
        assert_eq!(
            parse_client_message(text).unwrap(),
            ClientMessage::StopSession
        );
        let text = START
            .replace(r#""audio""#, r#""surprise": true, "audio""#)
            .replace(r#""hdr": true"#, r#""hdr": true, "extra": 1"#);
        assert!(parse_client_message(&text).is_ok());
        let host = r#"{"version":2,"type":"authenticated","also":"ignored"}"#;
        assert_eq!(
            ServerMessage::from_json(host).unwrap(),
            ServerMessage::Authenticated
        );
    }

    #[test]
    fn version_must_be_the_integer_two() {
        for text in [
            r#"{"type":"get-apps"}"#,
            r#"{"version":1,"type":"get-apps"}"#,
            r#"{"version":3,"type":"get-apps"}"#,
            r#"{"version":"2","type":"get-apps"}"#,
            r#"{"version":2.0,"type":"get-apps"}"#,
            r#"{"version":null,"type":"get-apps"}"#,
            r#"[]"#,
        ] {
            assert_eq!(parse_err(text), ErrorCode::UnsupportedVersion, "{text}");
        }
        assert_eq!(
            ServerMessage::from_json(r#"{"version":1,"type":"authenticated"}"#)
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedVersion
        );
    }

    #[test]
    fn malformed_messages_get_the_matching_code() {
        assert_eq!(parse_err("{"), ErrorCode::InvalidJson);
        assert_eq!(parse_err(""), ErrorCode::InvalidJson);
        assert_eq!(
            parse_err(r#"{"version":2,"type":"reboot"}"#),
            ErrorCode::UnsupportedMessage
        );
        assert_eq!(parse_err(r#"{"version":2}"#), ErrorCode::InvalidMessage);
        assert_eq!(
            parse_err(r#"{"version":2,"type":7}"#),
            ErrorCode::InvalidMessage
        );
        assert_eq!(
            parse_err(r#"{"version":2,"type":"authenticate","clientId":"a"}"#),
            ErrorCode::InvalidMessage
        );
        let long = "a".repeat(MAX_CLIENT_ID_LENGTH + 1);
        let text = json!({"version": 2, "type": "authenticate", "clientId": long, "proof": "ab"});
        assert_eq!(parse_err(&text.to_string()), ErrorCode::InvalidMessage);
        let text = json!({"version": 2, "type": "authenticate", "clientId": "", "proof": "ab"});
        assert_eq!(parse_err(&text.to_string()), ErrorCode::InvalidMessage);
    }

    #[test]
    fn keyframe_request_is_recognised_only_by_type() {
        assert!(is_keyframe_request(r#"{"type":"request-keyframe"}"#));
        assert!(is_keyframe_request(
            r#"{"v":1,"type":"request-keyframe","x":2}"#
        ));
        assert!(!is_keyframe_request(r#"{"type":"rumble"}"#));
        assert!(!is_keyframe_request(r#"{"note":"request-keyframe"}"#));
        assert!(!is_keyframe_request("request-keyframe"));
    }
}
