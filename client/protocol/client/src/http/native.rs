//! The Rubylight native API v1 (`/api/native/v1`, docs/native-api.md in the host repo): the
//! host's library, session state and actions, and Radeon state. Same pinned HTTPS and client
//! certificate as the classic calls; gate them on `Feature::NativeApi`.
//!
//! Every non-2xx answer carries an `ApiError` body, kept in [`Error::Api`] together with the
//! HTTP status. Clients branch on `ErrorCode` and fall back to the status for
//! `ErrorCode::Unknown`. A 404 from a path under the prefix that has no error body means the
//! host has no native API at all (an older Rubylight, Sunshine, Apollo).

use super::coop::{Coop, CoopJoined, JoinRequest};
use super::device_approval::{PairingApprove, PairingApproved, PairingDeny, PairingRequests, FEATURE_PAIRING_APPROVE};
use super::pc_power::{HostUpdate, PcPower, PcPowerOffer, PowerRequest};
use super::native_api::audio::{Audio, AudioWrite};
use super::native_api::capabilities::Capabilities;
use super::native_api::error::{ApiError, ErrorBody};
use super::native_api::library::Library;
use super::native_api::radeon::{Radeon, SettingsWrite};
use super::native_api::session::{ActionId, CouchMenu, CouchMenuRequest, Session, SessionApp};
use super::native_api::wake::Wake;
use super::native_api::PREFIX;
use super::transport::{self, Body, Request, Timeouts};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, READ_TIMEOUT};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// The part of `GET /session` Microphone only reads.
#[derive(Deserialize)]
struct MicrophoneBusy {
    #[serde(default)]
    microphone_busy: bool,
}

/// The game another device plays, from `GET /session` (`playing_on`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayingOn {
    pub device: String,
    pub app: SessionApp,
}

/// Where this device's stream went, from `GET /session` (`moved_to`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedTo {
    pub device: String,
    pub since_unix_ms: u64,
}

/// The part of `GET /session` Continue here reads. Both are absent where the host does not say
/// them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMove {
    #[serde(default)]
    pub playing_on: Option<PlayingOn>,
    #[serde(default)]
    pub moved_to: Option<MovedTo>,
    /// The running game is this device's.
    #[serde(default)]
    pub owns_game: bool,
    /// The press that took this device's stream failed, and the game is another device's.
    #[serde(default)]
    pub press_failed: bool,
}
use std::time::Duration;

/// A library of a few hundred games is well under this.
const MAX_LIBRARY_BYTES: usize = 4 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 1024 * 1024;
/// Ending a session or a driver write waits for the host to finish; the host's own
/// `session_busy` and `temporarily_unavailable` answer sooner.
const ACTION_READ_TIMEOUT: Duration = Duration::from_secs(15);
/// The host reads an empty body as `{}`.
const EMPTY_BODY: &[u8] = b"{}";

impl Error {
    /// The native API's error body, if this is one.
    pub fn api_error(&self) -> Option<&ApiError> {
        match self {
            Error::Api { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The HTTP status of a host answer, whatever kind of error it became.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            Error::Status { code, .. } => u16::try_from(*code).ok(),
            Error::NotFound(_) => Some(404),
            _ => None,
        }
    }
}

/// Reads a 2xx body as `T`, and any other answer as an [`ApiError`].
fn decode<T: DeserializeOwned>(response: transport::Response, path: &str) -> Result<T, Error> {
    if (200..=299).contains(&response.status) {
        return serde_json::from_slice(&response.body)
            .map_err(|e| Error::Malformed(format!("native API {path}: {e}")));
    }
    Err(error_of(response, path))
}

/// A non-2xx answer as an [`Error::Api`], or a classic error when it has no error body.
pub(super) fn error_of(response: transport::Response, path: &str) -> Error {
    match serde_json::from_slice::<ErrorBody>(&response.body) {
        Ok(body) => Error::Api {
            status: response.status,
            error: body.error,
        },
        Err(_) if response.status == 404 => Error::NotFound(format!("/{path}")),
        Err(_) => Error::Status {
            code: i32::from(response.status),
            message: response.reason,
        },
    }
}

/// The file, USB and shared folder permissions, which the pinned `Permissions` does not carry
/// yet. A host without the feature leaves them out: false.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct FilePermissions {
    #[serde(default)]
    pub file_upload: bool,
    #[serde(default)]
    pub file_download: bool,
    /// The 3.0 permissions read anything but a bool as refused, so a host that gets them wrong
    /// loses only them, not the file permissions beside them.
    #[serde(default, deserialize_with = "bool_or_false")]
    pub usb: bool,
    /// The device has at least one folder grant.
    #[serde(default, deserialize_with = "bool_or_false")]
    pub shared_folders: bool,
    /// The device may restart and shut down the PC (its own grant on the host).
    #[serde(default, deserialize_with = "bool_or_false")]
    pub restart_shutdown: bool,
    /// The device may approve new devices, and nothing else (its own grant on the host).
    #[serde(default, deserialize_with = "bool_or_false")]
    pub pairing_approve: bool,
}

fn bool_or_false<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?.as_bool().unwrap_or(false))
}

/// `GET /capabilities` with the fields of newer hosts, read from the same body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCapabilities {
    pub capabilities: Capabilities,
    pub files: FilePermissions,
}

impl NativeCapabilities {
    /// Whether this PC can approve new devices at all, for this device or another.
    pub fn offers_pairing_approve(&self) -> bool {
        self.capabilities.features.iter().any(|f| f == FEATURE_PAIRING_APPROVE)
    }

    /// Whether this device may list and approve devices waiting to pair on this PC: with
    /// "Approve new devices" or Host control.
    pub fn may_approve_pairing(&self) -> bool {
        self.offers_pairing_approve()
            && (self.files.pairing_approve || self.capabilities.permissions.host_control)
    }
}

impl<'de> Deserialize<'de> for NativeCapabilities {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Files {
            permissions: FilePermissions,
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        let files = Files::deserialize(&value).map_err(D::Error::custom)?;
        Ok(Self {
            capabilities: Capabilities::deserialize(value).map_err(D::Error::custom)?,
            files: files.permissions,
        })
    }
}

/// `GET /library` with each game's `last_played`, read from the same body: the pinned `Game`
/// predates it. Written back, the times sit in their games again, so Android's Java and a saved
/// library read them as a host sends them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeLibrary {
    pub library: Library,
    /// When each game was last played, by install id: seconds since the epoch, `0` when the
    /// host says never played. A game the host says nothing about (an older host, a manual app)
    /// is not in it.
    pub last_played: BTreeMap<String, u64>,
    /// The collections each game is in, by install id, as the host names them. A game in none (and
    /// every game of an older host) is not in it.
    pub collections: BTreeMap<String, Vec<String>>,
}

impl From<Library> for NativeLibrary {
    fn from(library: Library) -> Self {
        Self { library, last_played: BTreeMap::new(), collections: BTreeMap::new() }
    }
}

impl<'de> Deserialize<'de> for NativeLibrary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut last_played = BTreeMap::new();
        let mut collections = BTreeMap::new();
        for game in value["games"].as_array().into_iter().flatten() {
            let Some(id) = game["install_id"].as_str() else { continue };
            // Anything but a whole number of seconds reads as unknown.
            if let Some(at) = game["last_played"].as_u64() {
                last_played.insert(id.to_owned(), at);
            }
            // Anything but names reads as none.
            let names: Vec<String> =
                game["collections"].as_array().into_iter().flatten().filter_map(|name| name.as_str().map(str::to_owned)).collect();
            if !names.is_empty() {
                collections.insert(id.to_owned(), names);
            }
        }
        Ok(Self {
            library: Library::deserialize(value).map_err(D::Error::custom)?,
            last_played,
            collections,
        })
    }
}

impl Serialize for NativeLibrary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut value = serde_json::to_value(&self.library).map_err(S::Error::custom)?;
        if let Some(games) = value["games"].as_array_mut() {
            for game in games {
                let at = game["install_id"].as_str().and_then(|id| self.last_played.get(id)).copied();
                let names = game["install_id"].as_str().and_then(|id| self.collections.get(id)).cloned();
                let Some(fields) = game.as_object_mut() else { continue };
                if let Some(at) = at {
                    fields.insert("last_played".into(), at.into());
                }
                if let Some(names) = names {
                    fields.insert("collections".into(), names.into());
                }
            }
        }
        value.serialize(serializer)
    }
}

impl HostClient {
    fn native<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&[u8]>,
        limit: usize,
        read: Duration,
    ) -> Result<T, Error> {
        let path = format!("{}/{path}", PREFIX.trim_start_matches('/'));
        let target = self.url_target(&path, None);
        let request = Request {
            host: &self.host,
            port: self.https_port(true)?,
            target: &target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(read),
            },
            body_limit: limit,
        };
        let response = transport::send(&request, &self.cancel, body.map(Body::json))?;
        decode(response, &path)
    }

    /// What the host's native API offers this device. 404 means the host has none.
    pub fn native_capabilities(&self) -> Result<Capabilities, Error> {
        self.native("capabilities", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// [`HostClient::native_capabilities`] with the file permissions; feed it to
    /// `HostCapabilities::with_native` for `Feature::FileUpload` and `Feature::FileDownload`.
    pub fn native_capabilities_with_files(&self) -> Result<NativeCapabilities, Error> {
        self.native("capabilities", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// The host's games with their cover versions. Fetch covers with
    /// [`HostClient::box_art`] when an artwork `version` changes.
    pub fn library(&self) -> Result<NativeLibrary, Error> {
        self.native("library", None, MAX_LIBRARY_BYTES, READ_TIMEOUT)
    }

    /// The session and the actions this device may run.
    pub fn session(&self) -> Result<Session, Error> {
        self.native("session", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Whether this device's microphone reaches the host while another device's plays: the
    /// session's `microphone_busy`, false where the host does not say it. Only when the
    /// capabilities list `session.mic_only`.
    pub fn microphone_busy(&self) -> Result<bool, Error> {
        let session: MicrophoneBusy = self.native("session", None, MAX_JSON_BYTES, READ_TIMEOUT)?;
        Ok(session.microphone_busy)
    }

    /// Who plays the running game and where this device's stream went. Only when the
    /// capabilities list `session.continue_here`.
    pub fn stream_move(&self) -> Result<StreamMove, Error> {
        self.native("session", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Continue here: the other devices' streams of the running game end and the game stays. The
    /// caller then starts its stream with the normal resume. Only when the capabilities list
    /// `session.continue_here`.
    pub fn continue_here(&self) -> Result<Session, Error> {
        self.native(
            "session/continue-here",
            Some(EMPTY_BODY),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
    }

    /// Runs `Disconnect`, `End` or `CloseGame` and returns the session afterwards. `Resume`
    /// is [`HostClient::resume`]: it starts the stream, which this API never carries.
    pub fn session_action(&self, action: ActionId) -> Result<Session, Error> {
        let path = match action {
            ActionId::Disconnect => "session/disconnect",
            ActionId::End => "session/end",
            ActionId::CloseGame => "session/close-game",
            ActionId::Resume => {
                return Err(Error::InvalidArgument(
                    "resume is not a native API action; use resume()".into(),
                ))
            }
            ActionId::Unknown => {
                return Err(Error::InvalidArgument("unknown session action".into()))
            }
        };
        self.native(path, Some(EMPTY_BODY), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// Opens or closes the menu the host draws into this device's stream. Only when the
    /// capabilities list `session.couch_menu`; the answer says whether it is open now.
    pub fn couch_menu(&self, open: bool) -> Result<CouchMenu, Error> {
        let body = serde_json::to_vec(&CouchMenuRequest { open })
            .map_err(|e| Error::InvalidArgument(format!("couch menu: {e}")))?;
        self.native(
            "session/couch-menu",
            Some(&body),
            MAX_JSON_BYTES,
            READ_TIMEOUT,
        )
    }

    /// The PC's wake targets: every physical adapter's MAC and subnet broadcasts. Only when
    /// the capabilities list `wake`.
    pub fn wake(&self) -> Result<Wake, Error> {
        self.native("wake", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Restart and Shut down: whether each can run now, and the token to confirm with. Only
    /// when the capabilities list `power.restart` or `power.shutdown`.
    pub fn pc_power(&self) -> Result<PcPowerOffer, Error> {
        self.native("power", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Restarts or shuts down the PC with a token from [`HostClient::pc_power`]. Ok means the
    /// PC took the request (202); [`host_words`](super::host_words) gives a refusal's reason.
    pub fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error> {
        let body = serde_json::to_vec(&PowerRequest { token })
            .map_err(|e| Error::InvalidArgument(format!("power request: {e}")))?;
        self.native::<serde_json::Value>(action.path(), Some(&body), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
            .map(drop)
    }

    /// The host's version, its pending update and the updater's phase. Only when the
    /// capabilities list `update`.
    pub fn host_update(&self) -> Result<HostUpdate, Error> {
        self.native("update", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// The devices waiting to pair. Only where [`NativeCapabilities::may_approve_pairing`] says so.
    pub fn pairing_requests(&self) -> Result<PairingRequests, Error> {
        self.native("pairing/requests", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Sends the PIN a waiting device shows. Answers once that device has paired;
    /// a refusal's message says why, in the host's words.
    pub fn approve_pairing(&self, id: &str, pin: &str) -> Result<PairingApproved, Error> {
        let request = PairingApprove { id: id.to_owned(), pin: pin.to_owned() };
        let body = serde_json::to_vec(&request)
            .map_err(|e| Error::InvalidArgument(format!("pairing approval: {e}")))?;
        self.native("pairing/approve", Some(&body), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// Ends a waiting device's request, and answers the devices still waiting.
    pub fn deny_pairing(&self, id: &str) -> Result<PairingRequests, Error> {
        let body = serde_json::to_vec(&PairingDeny { id: id.to_owned() })
            .map_err(|e| Error::InvalidArgument(format!("pairing denial: {e}")))?;
        self.native("pairing/deny", Some(&body), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// Installs the host's pending update, ending this device's own stream. Needs Host control;
    /// refused while another device streams.
    pub fn install_host_update(&self) -> Result<HostUpdate, Error> {
        self.native("update/install", Some(EMPTY_BODY), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// The Nucleus Co-op sessions this device could join. Only when the capabilities list
    /// `coop`; without it the host answers 503 `temporarily_unavailable` with the reason.
    pub fn coop(&self) -> Result<Coop, Error> {
        self.native("coop", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Takes a free slot in `session`, or this device's own again, and holds it until the
    /// device launches the answer's `app_id`. [`CoopProblem::of`](super::CoopProblem::of) words a refusal.
    pub fn join_coop(&self, session: u32) -> Result<CoopJoined, Error> {
        let body = serde_json::to_vec(&JoinRequest { session })
            .map_err(|e| Error::InvalidArgument(format!("co-op join: {e}")))?;
        self.native("coop/join", Some(&body), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// The encode GPU's Radeon settings.
    pub fn radeon(&self) -> Result<Radeon, Error> {
        self.native("radeon", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Writes settings during an owned session. Needs Host control on the host; without it the
    /// answer is `ErrorCode::HostControlRequired`.
    pub fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
        let body = serde_json::to_vec(write)
            .map_err(|e| Error::InvalidArgument(format!("Radeon write: {e}")))?;
        self.native(
            "radeon/settings",
            Some(&body),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
    }

    /// Puts back what this device changed. Needs Host control too.
    pub fn restore_radeon(&self) -> Result<Radeon, Error> {
        self.native(
            "radeon/restore",
            Some(EMPTY_BODY),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
    }

    /// Night mode and headphone surround of this device's stream. Only when the capabilities
    /// list `audio.night_mode` or `audio.headphone_surround`; without a stream both settings are
    /// `temporarily_unavailable`.
    pub fn audio(&self) -> Result<Audio, Error> {
        self.native("audio", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Changes them for this device's own stream from its next audio packet; nothing is saved.
    /// Needs View or Launch, not Host control.
    pub fn write_audio(&self, write: &AudioWrite) -> Result<Audio, Error> {
        let body = serde_json::to_vec(write)
            .map_err(|e| Error::InvalidArgument(format!("audio write: {e}")))?;
        self.native("audio/settings", Some(&body), MAX_JSON_BYTES, READ_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::native_api::error::ErrorCode;

    fn response(status: u16, body: &str) -> transport::Response {
        transport::Response {
            status,
            reason: "Reason".into(),
            content_type: Some("application/json; charset=utf-8".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn a_library_keeps_collections_through_a_write_back() {
        let text = r#"{"revision":"r","games":[
            {"install_id":"a","app_id":1,"title":"A","game_id":"steam:1","store":null,"installed":true,"running":false,"artwork":{"version":"1"},"collections":["VR","Couch"]},
            {"install_id":"b","app_id":2,"title":"B","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"2"}},
            {"install_id":"c","app_id":3,"title":"C","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"3"},"collections":"VR"}]}"#;
        let library: NativeLibrary = serde_json::from_str(text).unwrap();
        assert_eq!(library.collections.len(), 1, "an older host's game and a changed field read as none");
        assert_eq!(library.collections["a"], ["VR", "Couch"]);
        let written = serde_json::to_value(&library).unwrap();
        assert_eq!(written["games"][0]["collections"], serde_json::json!(["VR", "Couch"]));
        assert!(written["games"][1].get("collections").is_none());
    }

    #[test]
    fn a_library_keeps_last_played_through_a_write_back() {
        let body = r#"{"revision":"r","games":[
            {"install_id":"a","app_id":1,"title":"A","game_id":"steam:1","store":null,"installed":true,"running":false,"artwork":{"version":"1"},"last_played":1788025796},
            {"install_id":"b","app_id":2,"title":"B","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"2"},"last_played":0},
            {"install_id":"c","app_id":3,"title":"C","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"3"}},
            {"install_id":"d","app_id":4,"title":"D","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"4"},"last_played":"soon"}]}"#;
        let library: NativeLibrary = decode(response(200, body), "api/native/v1/library").unwrap();
        assert_eq!(library.library.games.len(), 4);
        assert_eq!(
            library.last_played,
            BTreeMap::from([("a".to_owned(), 1788025796), ("b".to_owned(), 0)]),
            "0 is never played; absent and malformed are unknown"
        );
        let written = serde_json::to_value(&library).unwrap();
        assert_eq!(written["games"][0]["last_played"], 1788025796);
        assert_eq!(written["games"][1]["last_played"], 0);
        assert!(written["games"][2].get("last_played").is_none());
        assert_eq!(serde_json::from_value::<NativeLibrary>(written).unwrap(), library);
        let old: NativeLibrary = serde_json::from_str(r#"{"revision":"r","games":[]}"#).unwrap();
        assert!(old.last_played.is_empty());
    }

    #[test]
    fn errors_keep_the_body_and_the_status() {
        let error = decode::<Session>(
            response(403, r#"{"error":{"code":"host_control_required","message":"Turn it on.","setting":null,"retry_after_ms":null}}"#),
            "api/native/v1/session/close-game",
        )
        .unwrap_err();
        assert_eq!(error.http_status(), Some(403));
        let api = error.api_error().unwrap();
        assert_eq!(
            (api.code, api.message.as_str()),
            (ErrorCode::HostControlRequired, "Turn it on.")
        );
        assert_eq!(error.to_string(), "Turn it on.");

        // A code from a newer host decodes as Unknown, and the status stays usable.
        let error = decode::<Session>(
            response(
                409,
                r#"{"error":{"code":"quota_exceeded","message":"No."}}"#,
            ),
            "api/native/v1/session",
        )
        .unwrap_err();
        assert_eq!(error.api_error().unwrap().code, ErrorCode::Unknown);
        assert_eq!(error.http_status(), Some(409));
    }

    #[test]
    fn answers_without_an_error_body_become_classic_errors() {
        let path = "api/native/v1/library";
        assert!(
            matches!(decode::<Library>(response(404, "<html>no</html>"), path),
            Err(Error::NotFound(url)) if url == "/api/native/v1/library")
        );
        let error = decode::<Library>(response(502, ""), path).unwrap_err();
        assert!(matches!(&error, Error::Status { code: 502, .. }));
        assert_eq!(error.http_status(), Some(502));
        assert!(error.api_error().is_none());
        // A 2xx that is not the type is malformed, not a panic.
        assert!(matches!(
            decode::<Library>(response(200, "{}"), path),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn approving_a_device_takes_the_feature_and_either_grant() {
        let read = |features: &str, permissions: &str| {
            decode::<NativeCapabilities>(
                response(
                    200,
                    &format!(
                        r#"{{"api_version":1,"host":{{"name":"PC","version":"3.7.0","unique_id":"U"}},
                        "features":[{features}],"permissions":{{"list":true,"view":true,"launch":false,
                        {permissions}}}}}"#
                    ),
                ),
                "api/native/v1/capabilities",
            )
            .unwrap()
        };
        let feature = r#""pairing.approve""#;
        let neither = read(feature, r#""host_control":false"#);
        assert!(neither.offers_pairing_approve() && !neither.may_approve_pairing());
        assert!(read(feature, r#""host_control":true"#).may_approve_pairing());
        assert!(read(feature, r#""host_control":false,"pairing_approve":true"#).may_approve_pairing());
        // A value that is not a bool grants nothing.
        assert!(!read(feature, r#""host_control":false,"pairing_approve":"true""#).may_approve_pairing());
        // An older host has no such feature, whatever the device holds.
        let older = read("", r#""host_control":true,"pairing_approve":true"#);
        assert!(!older.offers_pairing_approve() && !older.may_approve_pairing());
    }

    #[test]
    fn the_file_permissions_come_from_the_same_body() {
        let body = |permissions: &str| {
            format!(
                r#"{{"api_version":1,"host":{{"name":"PC","version":"3.1.0","unique_id":"U"}},
                "features":["library","files.upload"],"permissions":{{"list":true,"view":true,
                "launch":false,"host_control":false{permissions}}}}}"#
            )
        };
        let path = "api/native/v1/capabilities";
        let read = decode::<NativeCapabilities>(
            response(200, &body(r#","file_upload":true,"file_download":false"#)),
            path,
        )
        .unwrap();
        assert_eq!(
            read.files,
            FilePermissions {
                file_upload: true,
                file_download: false,
                usb: false,
                shared_folders: false,
                restart_shutdown: false,
                pairing_approve: false,
            }
        );
        let read = decode::<NativeCapabilities>(
            response(200, &body(r#","usb":true,"shared_folders":true,"restart_shutdown":true"#)),
            path,
        )
        .unwrap();
        assert!(read.files.usb && read.files.shared_folders && read.files.restart_shutdown);
        assert!(!read.files.file_upload && !read.files.file_download);
        assert_eq!(read.capabilities.features, ["library", "files.upload"]);
        assert_eq!(
            read.capabilities,
            decode::<Capabilities>(response(200, &body("")), path).unwrap()
        );
        // An older host has neither.
        let read = decode::<NativeCapabilities>(response(200, &body("")), path).unwrap();
        assert_eq!(read.files, FilePermissions::default());
        // A 3.0 permission that is not a bool is refused; the rest of the body still counts.
        for odd in [r#""true""#, "1", "null", "{}"] {
            let read = decode::<NativeCapabilities>(
                response(
                    200,
                    &body(&format!(r#","file_upload":true,"usb":{odd},"shared_folders":{odd}"#)),
                ),
                path,
            )
            .unwrap();
            assert_eq!(
                read.files,
                FilePermissions {
                    file_upload: true,
                    ..Default::default()
                },
                "{odd}"
            );
        }
        // An older permission that is not a bool, or no Capabilities at all, is malformed.
        for broken in [body(r#","file_upload":1"#), r#"{"permissions":{}}"#.into()] {
            assert!(matches!(
                decode::<NativeCapabilities>(response(200, &broken), path),
                Err(Error::Malformed(_))
            ));
        }
    }

    #[test]
    fn only_the_native_actions_are_posted() {
        let identity = std::sync::Arc::new(super::super::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity).unwrap();
        for action in [ActionId::Resume, ActionId::Unknown] {
            assert!(matches!(
                client.session_action(action),
                Err(Error::InvalidArgument(_))
            ));
        }
    }
}
