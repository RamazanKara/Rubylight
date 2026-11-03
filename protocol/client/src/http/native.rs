//! The Rubylight native API v1 (`/api/native/v1`, docs/native-api.md in the host repo): the
//! host's library, session state and actions, and Radeon state. Same pinned HTTPS and client
//! certificate as the classic calls; gate them on `Feature::NativeApi`.
//!
//! Every non-2xx answer carries an `ApiError` body, kept in [`Error::Api`] together with the
//! HTTP status. Clients branch on `ErrorCode` and fall back to the status for
//! `ErrorCode::Unknown`. A 404 from a path under the prefix that has no error body means the
//! host has no native API at all (an older Rubylight, Sunshine, Apollo).

use super::coop::{Coop, CoopJoined, JoinRequest};
use super::native_api::audio::{Audio, AudioWrite};
use super::native_api::capabilities::Capabilities;
use super::native_api::error::{ApiError, ErrorBody};
use super::native_api::library::Library;
use super::native_api::radeon::{Radeon, SettingsWrite};
use super::native_api::session::{ActionId, CouchMenu, CouchMenuRequest, Session};
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
}

impl From<Library> for NativeLibrary {
    fn from(library: Library) -> Self {
        Self { library, last_played: BTreeMap::new() }
    }
}

impl<'de> Deserialize<'de> for NativeLibrary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut last_played = BTreeMap::new();
        for game in value["games"].as_array().into_iter().flatten() {
            // Anything but a whole number of seconds reads as unknown.
            if let (Some(id), Some(at)) = (game["install_id"].as_str(), game["last_played"].as_u64()) {
                last_played.insert(id.to_owned(), at);
            }
        }
        Ok(Self {
            library: Library::deserialize(value).map_err(D::Error::custom)?,
            last_played,
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
                if let (Some(at), Some(fields)) = (at, game.as_object_mut()) {
                    fields.insert("last_played".into(), at.into());
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
            }
        );
        let read = decode::<NativeCapabilities>(
            response(200, &body(r#","usb":true,"shared_folders":true"#)),
            path,
        )
        .unwrap();
        assert!(read.files.usb && read.files.shared_folders);
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
