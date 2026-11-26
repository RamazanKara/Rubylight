//! The Rubylight native API v1 (`/api/native/v1`): HostClient calls. Pure DTOs live in
//! `nightjar_policy::http`.

pub use nightjar_policy::http::{
    parse_added, AddedGame, FilePermissions, GameExtra, MovedTo, NativeCapabilities,
    NativeLibrary, PlayingOn, StreamMove,
};
use super::coop::{Coop, CoopJoined, JoinRequest};
use super::device_approval::{PairingApprove, PairingApproved, PairingDeny, PairingRequests};
use super::session_guests::{GuestAction, InviteCreate, InviteCreated, SessionGuests};
use super::pc_power::{HostUpdate, PcPower, PcPowerOffer, PowerRequest};
use super::native_api::audio::{Audio, AudioWrite};
use super::native_api::capabilities::Capabilities;
use super::native_api::error::ErrorBody;
use super::native_api::radeon::{Radeon, SettingsWrite};
use super::native_api::session::{ActionId, CouchMenu, CouchMenuRequest, Session};
use super::native_api::wake::Wake;
use super::native_api::PREFIX;
use super::neural::{Neural, NeuralWrite};
use super::transport::{self, Body, Request, Timeouts};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, READ_TIMEOUT};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::time::Duration;

/// The part of `GET /session` Microphone only reads.
#[derive(Deserialize)]
struct MicrophoneBusy {
    #[serde(default)]
    microphone_busy: bool,
}


/// A library of a few hundred games is well under this.
const MAX_LIBRARY_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_JSON_BYTES: usize = 1024 * 1024;
/// Ending a session or a driver write waits for the host to finish; the host's own
/// `session_busy` and `temporarily_unavailable` answer sooner.
pub(super) const ACTION_READ_TIMEOUT: Duration = Duration::from_secs(15);
/// The host reads an empty body as `{}`.
pub(super) const EMPTY_BODY: &[u8] = b"{}";

fn decode<T: DeserializeOwned>(response: transport::Response, path: &str) -> Result<T, Error> {
    if (200..=299).contains(&response.status) {
        return serde_json::from_slice(&response.body)
            .map_err(|e| Error::Malformed(format!("native API {path}: {e}")));
    }
    Err(error_of(response, path))
}

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

impl HostClient {
    pub(super) fn native<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&[u8]>,
        limit: usize,
        read: Duration,
    ) -> Result<T, Error> {
        // A query of the call's own goes before the identity's: one `?` in all.
        let (path, query) = split_query(path);
        let path = format!("{}/{path}", PREFIX.trim_start_matches('/'));
        let target = self.url_target(&path, query);
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
        self.library_with_unadded(false)
    }

    /// The host's library, optionally including unadded store games when `include_unadded` is true
    /// (`GET /api/native/v1/library?include=unadded`).
    pub fn library_with_unadded(&self, include_unadded: bool) -> Result<NativeLibrary, Error> {
        let path = if include_unadded {
            "library?include=unadded"
        } else {
            "library"
        };
        self.native(path, None, MAX_LIBRARY_BYTES, READ_TIMEOUT)
    }

    /// Adds an unadded store game (`game_id`, e.g. `"gog:c"`) to the host's library
    /// (`POST /api/native/v1/library/add`).
    pub fn add_to_library(&self, game_id: &str) -> Result<Option<AddedGame>, Error> {
        let body = serde_json::to_vec(&serde_json::json!({ "game_id": game_id }))
            .map_err(|e| Error::InvalidArgument(format!("add_to_library: {e}")))?;
        let value: serde_json::Value =
            self.native("library/add", Some(&body), MAX_LIBRARY_BYTES, ACTION_READ_TIMEOUT)?;
        parse_added(&value, game_id).map_err(Error::Malformed)
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

    /// The running game's guests and the joins waiting. Only when the capabilities list
    /// `session.guests`.
    pub fn session_guests(&self) -> Result<SessionGuests, Error> {
        self.native("session/guests", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// Answers a join or manages a guest, as the device playing (or one with Host control).
    /// Answers the list as it is afterwards; a refusal's message says why, in the host's words.
    pub fn guest_action(&self, action: &GuestAction) -> Result<SessionGuests, Error> {
        let (path, body) = action.request();
        self.native(path, Some(&body), MAX_JSON_BYTES, ACTION_READ_TIMEOUT)
    }

    /// Makes an invite to play over the internet ([`NativeCapabilities::may_invite`]): a link that
    /// pairs one friend's Nightjar as a guest. The link is answered once; keep it only to share it.
    pub fn create_invite(&self, invite: &InviteCreate) -> Result<InviteCreated, Error> {
        let body = serde_json::to_vec(invite)
            .map_err(|e| Error::InvalidArgument(format!("invite: {e}")))?;
        self.native(
            "session/guests/invites",
            Some(&body),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
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

    /// A game's Neural Rendering switches. Only when the capabilities list
    /// `neural.frame_generation`; reading needs View streams.
    pub fn neural(&self, app: &str) -> Result<Neural, Error> {
        let app: String = url_component(app);
        self.native(
            &format!("neural?app={app}"),
            None,
            MAX_JSON_BYTES,
            READ_TIMEOUT,
        )
    }

    /// Turns a game's switch on or off; the host then downloads, installs or removes on its own.
    /// Needs View streams and Host control, and no stream running on the PC.
    pub fn write_neural(&self, write: &NeuralWrite) -> Result<Neural, Error> {
        let body = serde_json::to_vec(write)
            .map_err(|e| Error::InvalidArgument(format!("Neural Rendering write: {e}")))?;
        self.native(
            "neural/settings",
            Some(&body),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
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

/// `path?query` as the path and its query, for [`HostClient::url_target`] to join with the
/// identity's parameters.
fn split_query(path: &str) -> (&str, Option<&str>) {
    match path.split_once('?') {
        Some((path, query)) => (path, Some(query).filter(|q| !q.is_empty())),
        None => (path, None),
    }
}

/// `text` percent-encoded for a query value: unreserved characters as they are.
fn url_component(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::native_api::error::ErrorCode;
    use crate::http::native_api::library::Library;
    use std::collections::BTreeMap;

    fn response(status: u16, body: &str) -> transport::Response {
        transport::Response {
            status,
            reason: "Reason".into(),
            content_type: Some("application/json; charset=utf-8".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    /// A native call's own query joins the identity's parameters with one `?`: `neural?app=`
    /// once reached the host as `app=<id>?uniqueid=...`, a game it didn't have.
    #[test]
    fn a_native_query_and_the_identity_share_one_question_mark() {
        let identity = std::sync::Arc::new(super::super::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 47989, identity).unwrap();
        let app = url_component("1F2E3D4C-0000-4000-8000-00000000F011");
        let call = format!("neural?app={app}");
        let (path, query) = split_query(&call);
        let target = client.url_target(&format!("api/v1/{path}"), query);
        assert!(target.starts_with("api/v1/neural?app=1F2E3D4C-0000-4000-8000-00000000F011&uniqueid="), "{target}");
        assert_eq!(target.matches('?').count(), 1, "{target}");
        assert_eq!(split_query("library"), ("library", None));
        assert_eq!(url_component("a b&c"), "a%20b%26c");
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
    fn a_library_keeps_playtime_through_a_write_back() {
        let body = r#"{"revision":"r","games":[
            {"install_id":"a","app_id":1,"title":"A","game_id":"steam:1","store":null,"installed":true,"running":false,"artwork":{"version":"1"},"playtime_minutes":5412},
            {"install_id":"b","app_id":2,"title":"B","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"2"},"playtime_minutes":0},
            {"install_id":"c","app_id":3,"title":"C","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"3"}},
            {"install_id":"d","app_id":4,"title":"D","game_id":null,"store":null,"installed":null,"running":false,"artwork":{"version":"4"},"playtime_minutes":-3}]}"#;
        let library: NativeLibrary = decode(response(200, body), "api/native/v1/library").unwrap();
        assert_eq!(
            library.playtime_minutes,
            BTreeMap::from([("a".to_owned(), 5412), ("b".to_owned(), 0)]),
            "0 is never played; absent and malformed are unknown"
        );
        let written = serde_json::to_value(&library).unwrap();
        assert_eq!(written["games"][0]["playtime_minutes"], 5412);
        assert_eq!(written["games"][1]["playtime_minutes"], 0);
        assert!(written["games"][2].get("playtime_minutes").is_none());
        assert_eq!(serde_json::from_value::<NativeLibrary>(written).unwrap(), library);
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
        let body_with_3_0 = body(r#","usb":true,"shared_folders":true,"restart_shutdown":true"#);
        let read = decode::<NativeCapabilities>(
            response(200, &body_with_3_0),
            path,
        )
        .unwrap();
        assert!(read.files.usb && read.files.shared_folders && read.files.restart_shutdown);
        assert!(!read.files.file_upload && !read.files.file_download);
        assert_eq!(read.capabilities.features, ["library", "files.upload"]);
        assert_eq!(
            read.capabilities,
            decode::<Capabilities>(response(200, &body_with_3_0), path).unwrap()
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
    fn native_library_keeps_mixed_collections_consistent() {
        let body = r#"{
            "revision": "r1",
            "games": [
                {
                    "install_id": "mixed",
                    "app_id": 1,
                    "title": "Mixed",
                    "running": false,
                    "artwork": {"cover": "/c", "version": "v"},
                    "collections": ["Action", 123, null, "RPG"]
                },
                {
                    "install_id": "bad-arr",
                    "app_id": 2,
                    "title": "BadArr",
                    "running": false,
                    "artwork": {"cover": "/c", "version": "v"},
                    "collections": [123, null]
                },
                {
                    "install_id": "not-arr",
                    "app_id": 3,
                    "title": "NotArr",
                    "running": false,
                    "artwork": {"cover": "/c", "version": "v"},
                    "collections": "Nope"
                }
            ]
        }"#;
        let parsed: NativeLibrary = serde_json::from_str(body).unwrap();
        assert_eq!(
            parsed.collections.get("mixed").map(Vec::as_slice),
            Some(&["Action".to_owned(), "RPG".to_owned()][..])
        );
        assert_eq!(
            parsed.library.games[0].collections,
            vec!["Action".to_owned(), "RPG".to_owned()]
        );
        assert!(!parsed.collections.contains_key("bad-arr"));
        assert!(parsed.library.games[1].collections.is_empty());
        assert!(!parsed.collections.contains_key("not-arr"));
        assert!(parsed.library.games[2].collections.is_empty());
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
