//! The JNI side of `com.limelight.nvstream.jni.NativeApiBridge`: the Rubylight native API v1
//! (`HostClient::{native_capabilities, library, session, session_action, couch_menu, radeon,
//! write_radeon, restore_radeon, wake}`) for the console screens and the stream menu, and Sleep
//! PC (`power_capabilities`, `sleep`), the file transfer permissions (`files.capabilities`), and
//! Nucleus Co-op (`coop`, `coop.join`).
//! Calls block on HTTPS, so Java runs them on worker threads, never on the UI thread.
//!
//! One entry point, `request`, takes the operation, the host endpoint and an optional JSON
//! argument, and always answers with one JSON object: `{"ok": <the API's JSON>}` or
//! `{"error": {...}}`. Java decodes both, so no exception crosses the JNI boundary and the
//! error keeps its API code, HTTP status and setting.

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use serde::Serialize;
use serde_json::{json, Value};

use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::radeon::{Radeon, SettingsWrite};
use nightjar_core::http::native_api::session::{ActionId, CouchMenu, CouchMenuRequest, Session};
use nightjar_core::http::native_api::wake::Wake;
use nightjar_core::http::{
    ClientIdentity, Coop, CoopJoined, Error, HostClient, NativeCapabilities, PowerError,
    PowerReply, Refusal,
};

/// What the client of one host needs; Java builds it from the paired host's stored data.
pub struct Endpoint {
    pub address: String,
    pub http_port: u16,
    /// 0 when unknown: the client asks the host's serverinfo.
    pub https_port: u16,
    pub server_certificate_pem: String,
    pub unique_id: String,
    pub client_certificate_der: Vec<u8>,
    pub client_private_key_pkcs8: Vec<u8>,
}

/// The calls of `HostClient` this module makes, so the dispatch is testable without a host.
pub trait Host {
    fn capabilities_with_files(&self) -> Result<NativeCapabilities, Error>;
    fn library(&self) -> Result<Library, Error>;
    fn session(&self) -> Result<Session, Error>;
    fn session_action(&self, action: ActionId) -> Result<Session, Error>;
    fn couch_menu(&self, open: bool) -> Result<CouchMenu, Error>;
    fn radeon(&self) -> Result<Radeon, Error>;
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error>;
    fn restore_radeon(&self) -> Result<Radeon, Error>;
    fn power_capabilities(&self) -> Result<PowerReply, PowerError>;
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError>;
    fn wake(&self) -> Result<Wake, Error>;
    fn coop(&self) -> Result<Coop, Error>;
    fn join_coop(&self, session: u32) -> Result<CoopJoined, Error>;
}

impl Host for HostClient {
    fn capabilities_with_files(&self) -> Result<NativeCapabilities, Error> {
        self.native_capabilities_with_files()
    }
    fn library(&self) -> Result<Library, Error> {
        HostClient::library(self)
    }
    fn session(&self) -> Result<Session, Error> {
        HostClient::session(self)
    }
    fn session_action(&self, action: ActionId) -> Result<Session, Error> {
        HostClient::session_action(self, action)
    }
    fn couch_menu(&self, open: bool) -> Result<CouchMenu, Error> {
        HostClient::couch_menu(self, open)
    }
    fn radeon(&self) -> Result<Radeon, Error> {
        HostClient::radeon(self)
    }
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
        HostClient::write_radeon(self, write)
    }
    fn restore_radeon(&self) -> Result<Radeon, Error> {
        HostClient::restore_radeon(self)
    }
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        HostClient::power_capabilities(self)
    }
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        HostClient::sleep(self, token)
    }
    fn wake(&self) -> Result<Wake, Error> {
        HostClient::wake(self)
    }
    fn coop(&self) -> Result<Coop, Error> {
        HostClient::coop(self)
    }
    fn join_coop(&self, session: u32) -> Result<CoopJoined, Error> {
        HostClient::join_coop(self, session)
    }
}

pub fn client(endpoint: &Endpoint) -> Result<HostClient, Error> {
    let identity = ClientIdentity::from_der(
        endpoint.unique_id.clone(),
        endpoint.client_certificate_der.clone(),
        endpoint.client_private_key_pkcs8.clone(),
    )?;
    let client = HostClient::new(
        &endpoint.address,
        endpoint.http_port,
        std::sync::Arc::new(identity),
    )?
    .with_https_port(endpoint.https_port)
    .with_server_certificate_pem(&endpoint.server_certificate_pem)?;
    Ok(client)
}

fn ok<T: Serialize>(result: Result<T, Error>) -> String {
    match result.and_then(|value| {
        serde_json::to_value(value).map_err(|e| Error::Malformed(format!("native API reply: {e}")))
    }) {
        Ok(value) => json!({ "ok": value }).to_string(),
        Err(error) => error_envelope(&error),
    }
}

/// `{"error": {"kind", "message", ...}}`; `code`, `setting` and `retry_after_ms` come from the
/// host's error body, `http_status` from any answer of the host.
pub fn error_envelope(error: &Error) -> String {
    let kind = match error {
        Error::Io(_) => "network",
        Error::Tls(_) => "tls",
        Error::CertificateMismatch => "certificate_mismatch",
        Error::Status { .. } => "status",
        Error::NotFound(_) => "not_found",
        Error::Api { .. } => "api",
        Error::Malformed(_) => "malformed",
        Error::NotPaired => "not_paired",
        Error::InvalidIdentity(_) => "identity",
        Error::InvalidArgument(_) => "invalid_argument",
        Error::Cancelled => "cancelled",
    };
    let mut body = json!({ "kind": kind, "message": error.to_string() });
    if let Some(status) = error.http_status() {
        body["http_status"] = json!(status);
    }
    if let Some(api) = error.api_error() {
        body["code"] = serde_json::to_value(api.code).unwrap_or(Value::Null);
        body["setting"] = json!(api.setting);
        body["retry_after_ms"] = json!(api.retry_after_ms);
    }
    json!({ "error": body }).to_string()
}

/// The capabilities with the permissions the pinned `Permissions` drops put back, so Java's
/// `HostCapabilities.withNativeCapabilities` reads them as the host sent them.
fn capabilities_with_files(result: Result<NativeCapabilities, Error>) -> String {
    ok(result.and_then(|native| {
        let mut value = serde_json::to_value(&native.capabilities)
            .map_err(|e| Error::Malformed(format!("native API reply: {e}")))?;
        let files = native.files;
        let Some(permissions) = value.get_mut("permissions").and_then(Value::as_object_mut) else {
            return Err(Error::Malformed("capabilities without permissions".into()));
        };
        permissions.insert("file_upload".into(), json!(files.file_upload));
        permissions.insert("file_download".into(), json!(files.file_download));
        permissions.insert("usb".into(), json!(files.usb));
        permissions.insert("shared_folders".into(), json!(files.shared_folders));
        Ok(value)
    }))
}

fn argument_error(message: String) -> String {
    error_envelope(&Error::InvalidArgument(message))
}

fn power_json(reply: &PowerReply) -> Value {
    json!({
        "status": reply.status,
        "message": reply.message,
        "version": reply.version,
        "sleep_supported": reply.sleep_supported,
        "sleep_allowed": reply.sleep_allowed,
        "sleep_available": reply.sleep_available,
        "active_session_policy": reply.active_session_policy,
        "token": reply.token,
        "token_lifetime_seconds": reply.token_lifetime_seconds,
        "accepted": reply.accepted,
    })
}

/// `{"ok": <reply>}`; a documented refusal is `{"error": {"kind": "power_refused", "message",
/// "http_status", "refusal", "power": <reply>}}`, anything else the usual error.
fn power(result: Result<PowerReply, PowerError>) -> String {
    match result {
        Ok(reply) => json!({ "ok": power_json(&reply) }).to_string(),
        Err(PowerError::Refused { refusal, reply }) => {
            let refusal = match refusal {
                Refusal::BadRequest => "bad_request",
                Refusal::Unauthorized => "unauthorized",
                Refusal::Forbidden => "forbidden",
                Refusal::MethodNotAllowed => "method_not_allowed",
                Refusal::Conflict => "conflict",
                Refusal::NotImplemented => "not_implemented",
                Refusal::Unavailable => "unavailable",
            };
            json!({ "error": {
                "kind": "power_refused",
                "message": reply.message,
                "http_status": reply.status,
                "refusal": refusal,
                "power": power_json(&reply),
            } })
            .to_string()
        }
        Err(PowerError::Host(error)) => error_envelope(&error),
    }
}

/// Runs one operation: `capabilities` and `files.capabilities` (both with the file, USB and
/// shared folder permissions: Java's `NativeCapabilities` reads `usb` from either), `library`,
/// `session`, `session_action` (argument `disconnect`, `end` or `close_game`), `couch_menu`
/// (argument: the `CouchMenuRequest` JSON), `radeon`, `radeon_write` (argument: the
/// `SettingsWrite` JSON), `radeon_restore`, `wake`, `power.capabilities` and `power.sleep`
/// (argument: the token from `power.capabilities`), `coop` and `coop.join` (argument: the
/// session id). Gate the power ones on `Feature::Sleep` and the co-op ones on `Feature::Coop`.
pub fn execute(host: &dyn Host, op: &str, argument: &str) -> String {
    match op {
        // The pinned `Capabilities` drops `permissions.usb`, so the stream menu never offered USB.
        "capabilities" | "files.capabilities" => {
            capabilities_with_files(host.capabilities_with_files())
        }
        "library" => ok(host.library()),
        "session" => ok(host.session()),
        "session_action" => {
            let action = match argument {
                "disconnect" => ActionId::Disconnect,
                "end" => ActionId::End,
                "close_game" => ActionId::CloseGame,
                other => return argument_error(format!("unknown session action: {other:?}")),
            };
            ok(host.session_action(action))
        }
        "couch_menu" => match serde_json::from_str::<CouchMenuRequest>(argument) {
            Ok(request) => ok(host.couch_menu(request.open)),
            Err(e) => argument_error(format!("couch menu: {e}")),
        },
        "radeon" => ok(host.radeon()),
        "radeon_write" => match serde_json::from_str::<SettingsWrite>(argument) {
            Ok(write) => ok(host.write_radeon(&write)),
            Err(e) => argument_error(format!("Radeon write: {e}")),
        },
        "radeon_restore" => ok(host.restore_radeon()),
        "power.capabilities" => power(host.power_capabilities()),
        "power.sleep" => power(host.sleep(argument)),
        "wake" => ok(host.wake()),
        "coop" => ok(host.coop()),
        "coop.join" => match argument.trim().parse::<u32>() {
            Ok(session) => ok(host.join_coop(session)),
            Err(_) => argument_error(format!("co-op session id: {argument:?}")),
        },
        other => argument_error(format!("unknown native API operation: {other:?}")),
    }
}

/// Builds the client and runs the operation.
pub fn request(endpoint: &Endpoint, op: &str, argument: &str) -> String {
    match client(endpoint) {
        Ok(client) => execute(&client, op, argument),
        Err(error) => error_envelope(&error),
    }
}

pub(crate) fn string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

pub(crate) fn bytes(env: &mut JNIEnv, value: &JByteArray) -> Option<Vec<u8>> {
    if value.is_null() {
        return None;
    }
    env.convert_byte_array(value).ok()
}

/// The endpoint from the arguments every bridge call carries; `None` when one is missing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn endpoint(
    env: &mut JNIEnv,
    address: &JString,
    http_port: jint,
    https_port: jint,
    server_certificate_pem: &JString,
    unique_id: &JString,
    client_certificate_der: &JByteArray,
    client_private_key_pkcs8: &JByteArray,
) -> Option<Endpoint> {
    Some(Endpoint {
        address: string(env, address)?,
        http_port: u16::try_from(http_port).ok()?,
        https_port: u16::try_from(https_port).unwrap_or(0),
        server_certificate_pem: string(env, server_certificate_pem)?,
        unique_id: string(env, unique_id)?,
        client_certificate_der: bytes(env, client_certificate_der)?,
        client_private_key_pkcs8: bytes(env, client_private_key_pkcs8)?,
    })
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NativeApiBridge_request(
    mut env: JNIEnv,
    _: JClass,
    op: JString,
    address: JString,
    http_port: jint,
    https_port: jint,
    server_certificate_pem: JString,
    unique_id: JString,
    client_certificate_der: JByteArray,
    client_private_key_pkcs8: JByteArray,
    argument: JString,
) -> jstring {
    let reply = (|| {
        let op = string(&mut env, &op)?;
        let endpoint = endpoint(
            &mut env,
            &address,
            http_port,
            https_port,
            &server_certificate_pem,
            &unique_id,
            &client_certificate_der,
            &client_private_key_pkcs8,
        )?;
        let argument = string(&mut env, &argument).unwrap_or_default();
        Some(request(&endpoint, &op, &argument))
    })()
    .unwrap_or_else(|| {
        error_envelope(&Error::InvalidArgument(
            "missing or invalid native API request".into(),
        ))
    });
    match env.new_string(reply) {
        Ok(reply) => reply.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::error::{ApiError, ErrorCode};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        fail: Option<ErrorCode>,
        /// The host's GET /capabilities body, when not the default one.
        capabilities: Option<&'static str>,
    }

    impl Fake {
        fn note(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_string());
        }

        fn session(&self) -> Result<Session, Error> {
            if let Some(code) = self.fail {
                return Err(Error::Api {
                    status: code.status(),
                    error: ApiError {
                        code,
                        message: "Turn on Host control for this device.".into(),
                        setting: Some("chill.enabled".into()),
                        retry_after_ms: Some(250),
                    },
                });
            }
            Ok(serde_json::from_str(
                r#"{"revision":4,"state":"suspended","owner":"you","app":null,"game_running":true,"actions":[{"id":"end","allowed":true}]}"#,
            )
            .unwrap())
        }
    }

    impl Host for Fake {
        fn capabilities_with_files(&self) -> Result<NativeCapabilities, Error> {
            self.note("capabilities");
            Ok(serde_json::from_str(self.capabilities.unwrap_or(
                r#"{"api_version":1,"host":{"name":"PC","version":"3.0.0","unique_id":"U"},"features":["files.upload","files.download"],"permissions":{"list":true,"view":true,"launch":true,"host_control":true,"file_upload":true,"file_download":false,"usb":"yes","shared_folders":true}}"#,
            ))
            .unwrap())
        }
        fn library(&self) -> Result<Library, Error> {
            self.note("library");
            Err(Error::NotFound("/api/native/v1/library".into()))
        }
        fn session(&self) -> Result<Session, Error> {
            self.note("session");
            Fake::session(self)
        }
        fn session_action(&self, action: ActionId) -> Result<Session, Error> {
            self.note(&format!("action {action:?}"));
            Fake::session(self)
        }
        fn radeon(&self) -> Result<Radeon, Error> {
            self.note("radeon");
            Err(Error::Cancelled)
        }
        fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
            self.note(&format!("write {} {}", write.gpu, write.changes.len()));
            Err(Error::Malformed("no".into()))
        }
        fn restore_radeon(&self) -> Result<Radeon, Error> {
            self.note("restore");
            Err(Error::NotPaired)
        }
        fn couch_menu(&self, open: bool) -> Result<CouchMenu, Error> {
            self.note(&format!("couch menu {open}"));
            if let Some(code) = self.fail {
                return Err(Error::Api {
                    status: code.status(),
                    error: ApiError {
                        code,
                        message: "Start a stream first.".into(),
                        setting: None,
                        retry_after_ms: None,
                    },
                });
            }
            Ok(CouchMenu { open })
        }
        fn wake(&self) -> Result<Wake, Error> {
            self.note("wake");
            Ok(serde_json::from_str(
                r#"{"revision":1,"primary":"02:7c:16:b4:13:4f","targets":[{"mac":"02:7c:16:b4:13:4f","kind":"ethernet","connected":true,"route":true,"broadcasts":["192.168.4.255"],"state":"ready","checks":[]}],"system":{"sleep":"s3","modern_standby":false,"hibernate":false,"fast_startup":"inactive","shutdown_wake":"firmware_unknown"},"idle_sleep":{"state":"off","minutes":0,"reason":null}}"#,
            )
            .unwrap())
        }
        fn coop(&self) -> Result<Coop, Error> {
            self.note("coop");
            Ok(serde_json::from_str(
                r#"{"available":true,"reason":null,"games":[{"app_id":7,"title":"Portal 2 (Co-op)","game":"g","max_players":2}],"sessions":[{"id":3,"app_id":7,"title":"Portal 2","game":"g","phase":"running","end":null,"players":[{"slot":0,"name":"Fold7","you":false,"away":false},{"slot":1,"name":null,"you":false,"away":false}],"free_slots":1,"joinable":false,"refusal":"This device may not join co-op sessions"}]}"#,
            )
            .unwrap())
        }
        fn join_coop(&self, session: u32) -> Result<CoopJoined, Error> {
            self.note(&format!("join {session}"));
            if let Some(code) = self.fail {
                return Err(Error::Api {
                    status: code.status(),
                    error: ApiError {
                        code,
                        message: "Turn on “Can join co-op games” for this device in the Rubylight console."
                            .into(),
                        setting: None,
                        retry_after_ms: None,
                    },
                });
            }
            Ok(CoopJoined {
                session,
                slot: 1,
                app_id: 7,
            })
        }
        fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
            self.note("power");
            Ok(power_reply(200, "", Some(TOKEN)))
        }
        fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
            self.note(&format!("sleep {token}"));
            match token {
                TOKEN => Ok(power_reply(202, "Sleep request accepted", None)),
                "" => Err(PowerError::Host(Error::InvalidArgument("no token".into()))),
                _ => Err(PowerError::Refused {
                    refusal: Refusal::Conflict,
                    reply: power_reply(409, "Sleep token expired or already used", None),
                }),
            }
        }
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn power_reply(status: u16, message: &str, token: Option<&str>) -> PowerReply {
        PowerReply {
            status,
            message: message.into(),
            version: 1,
            sleep_supported: true,
            sleep_allowed: true,
            sleep_available: token.is_some(),
            active_session_policy: "idle-only".into(),
            token: token.map(str::to_owned),
            token_lifetime_seconds: 30,
            accepted: status == 202,
        }
    }

    fn parse(reply: &str) -> Value {
        serde_json::from_str(reply).unwrap()
    }

    #[test]
    fn answers_carry_the_apis_json_under_ok() {
        let fake = Fake::default();
        let reply = parse(&execute(&fake, "capabilities", ""));
        assert_eq!(reply["ok"]["api_version"], 1);
        assert_eq!(reply["ok"]["permissions"]["host_control"], true);
        let reply = parse(&execute(&fake, "session", ""));
        assert_eq!(reply["ok"]["state"], "suspended");
        assert_eq!(reply["ok"]["actions"][0]["id"], "end");
        assert!(reply.get("error").is_none());
    }

    #[test]
    fn file_capabilities_keep_the_permissions_the_pinned_type_drops() {
        let fake = Fake::default();
        let reply = parse(&execute(&fake, "files.capabilities", ""));
        let permissions = &reply["ok"]["permissions"];
        assert_eq!(permissions["file_upload"], true);
        assert_eq!(permissions["file_download"], false);
        // Anything but a bool reads as refused, as the Rust client reads it.
        assert_eq!(permissions["usb"], false);
        assert_eq!(permissions["shared_folders"], true);
        assert_eq!(permissions["host_control"], true);
        assert_eq!(reply["ok"]["features"][0], "files.upload");
        assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "capabilities");
        // The plain capabilities answer the same.
        let plain = parse(&execute(&fake, "capabilities", ""));
        assert_eq!(plain, reply);
    }

    /// The stream menu's USB row reads `permissions.usb` from the plain `capabilities`: the
    /// pinned `Capabilities` dropped it, so a host that offered USB never got the row.
    #[test]
    fn plain_capabilities_keep_the_usb_permission() {
        let granted = Fake {
            capabilities: Some(
                r#"{"api_version":1,"host":{"name":"PC","version":"3.0.0","unique_id":"U"},"features":["library","usb"],"permissions":{"list":true,"view":true,"launch":true,"host_control":false,"usb":true}}"#,
            ),
            ..Fake::default()
        };
        let reply = parse(&execute(&granted, "capabilities", ""));
        assert_eq!(reply["ok"]["permissions"]["usb"], true);
        assert_eq!(reply["ok"]["features"][1], "usb");
        // A host without the permission, or an older one without the field, refuses it.
        let refused = Fake {
            capabilities: Some(
                r#"{"api_version":1,"host":{"name":"PC","version":"3.0.0","unique_id":"U"},"features":["usb"],"permissions":{"list":true,"view":true,"launch":true,"host_control":false}}"#,
            ),
            ..Fake::default()
        };
        let reply = parse(&execute(&refused, "capabilities", ""));
        assert_eq!(reply["ok"]["permissions"]["usb"], false);
    }

    #[test]
    fn coop_lists_and_joins_by_session_id_and_keeps_the_refusal() {
        let fake = Fake::default();
        let reply = parse(&execute(&fake, "coop", ""));
        assert_eq!(reply["ok"]["sessions"][0]["id"], 3);
        assert_eq!(reply["ok"]["sessions"][0]["joinable"], false);
        assert_eq!(
            reply["ok"]["sessions"][0]["refusal"],
            "This device may not join co-op sessions"
        );
        assert_eq!(
            reply["ok"]["sessions"][0]["players"][1]["name"],
            Value::Null
        );
        let reply = parse(&execute(&fake, "coop.join", "3"));
        assert_eq!(reply["ok"], json!({"session": 3, "slot": 1, "app_id": 7}));
        assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "join 3");
        let calls = fake.calls.lock().unwrap().len();
        for argument in ["", "-1", "three", "4294967296"] {
            let reply = parse(&execute(&fake, "coop.join", argument));
            assert_eq!(reply["error"]["kind"], "invalid_argument", "{argument}");
        }
        assert_eq!(
            fake.calls.lock().unwrap().len(),
            calls,
            "a bad id never reaches the host"
        );
        let refused = Fake {
            fail: Some(ErrorCode::PermissionRequired),
            ..Fake::default()
        };
        let reply = parse(&execute(&refused, "coop.join", "3"));
        assert_eq!(reply["error"]["http_status"], 403);
        assert_eq!(reply["error"]["code"], "permission_required");
        assert_eq!(
            reply["error"]["message"],
            "Turn on “Can join co-op games” for this device in the Rubylight console."
        );
    }

    #[test]
    fn session_actions_map_by_name_and_resume_is_refused() {
        let fake = Fake::default();
        for (argument, called) in [
            ("disconnect", "action Disconnect"),
            ("end", "action End"),
            ("close_game", "action CloseGame"),
        ] {
            let reply = parse(&execute(&fake, "session_action", argument));
            assert_eq!(reply["ok"]["revision"], 4, "{argument}");
            assert_eq!(fake.calls.lock().unwrap().last().unwrap(), called);
        }
        let calls = fake.calls.lock().unwrap().len();
        for argument in ["resume", "", "Close_Game"] {
            let reply = parse(&execute(&fake, "session_action", argument));
            assert_eq!(reply["error"]["kind"], "invalid_argument", "{argument}");
        }
        // Nothing reached the host for the refused ones.
        assert_eq!(fake.calls.lock().unwrap().len(), calls);
    }

    #[test]
    fn api_errors_keep_their_code_status_and_setting() {
        let fake = Fake {
            fail: Some(ErrorCode::HostControlRequired),
            ..Fake::default()
        };
        let reply = parse(&execute(&fake, "session_action", "close_game"));
        let error = &reply["error"];
        assert_eq!(error["kind"], "api");
        assert_eq!(error["code"], "host_control_required");
        assert_eq!(error["http_status"], 403);
        assert_eq!(error["setting"], "chill.enabled");
        assert_eq!(error["retry_after_ms"], 250);
        assert_eq!(error["message"], "Turn on Host control for this device.");
        assert!(reply.get("ok").is_none());
    }

    #[test]
    fn other_errors_have_a_kind_and_the_status_when_the_host_answered() {
        let fake = Fake::default();
        let missing = parse(&execute(&fake, "library", ""));
        assert_eq!(missing["error"]["kind"], "not_found");
        assert_eq!(missing["error"]["http_status"], 404);
        assert!(missing["error"].get("code").is_none());
        assert_eq!(
            parse(&execute(&fake, "radeon", ""))["error"]["kind"],
            "cancelled"
        );
        assert_eq!(
            parse(&execute(&fake, "radeon_restore", ""))["error"]["kind"],
            "not_paired"
        );
        let status = parse(&error_envelope(&Error::Status {
            code: 502,
            message: "Bad Gateway".into(),
        }));
        assert_eq!(
            (
                status["error"]["kind"].as_str(),
                status["error"]["http_status"].as_u64()
            ),
            (Some("status"), Some(502))
        );
        let io = parse(&error_envelope(&Error::Io(std::io::Error::other(
            "refused",
        ))));
        assert_eq!(io["error"]["kind"], "network");
        assert!(io["error"].get("http_status").is_none());
    }

    #[test]
    fn radeon_writes_parse_strictly_before_they_reach_the_host() {
        let fake = Fake::default();
        let write = r#"{"gpu":11520,"changes":[{"id":"chill.enabled","value":true},{"id":"chill.max_fps","value":144}]}"#;
        let reply = parse(&execute(&fake, "radeon_write", write));
        assert_eq!(reply["error"]["kind"], "malformed");
        assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "write 11520 2");

        let calls = fake.calls.lock().unwrap().len();
        for bad in ["", "{}", r#"{"gpu":1,"changes":[],"extra":1}"#] {
            let reply = parse(&execute(&fake, "radeon_write", bad));
            assert_eq!(reply["error"]["kind"], "invalid_argument", "{bad}");
        }
        assert_eq!(fake.calls.lock().unwrap().len(), calls);
    }

    #[test]
    fn power_ops_carry_the_reply_and_refusals_keep_it() {
        let fake = Fake::default();
        let offer = parse(&execute(&fake, "power.capabilities", ""));
        assert_eq!(offer["ok"]["token"], TOKEN);
        assert_eq!(
            (
                offer["ok"]["status"].as_u64(),
                offer["ok"]["sleep_available"].as_bool()
            ),
            (Some(200), Some(true))
        );
        let accepted = parse(&execute(&fake, "power.sleep", TOKEN));
        assert_eq!(
            (
                accepted["ok"]["accepted"].as_bool(),
                &accepted["ok"]["token"]
            ),
            (Some(true), &Value::Null)
        );
        assert_eq!(
            fake.calls.lock().unwrap().last().unwrap(),
            &format!("sleep {TOKEN}")
        );

        let refused = parse(&execute(&fake, "power.sleep", "spent"));
        let error = &refused["error"];
        assert_eq!(
            (error["kind"].as_str(), error["refusal"].as_str()),
            (Some("power_refused"), Some("conflict"))
        );
        assert_eq!(
            (error["http_status"].as_u64(), error["message"].as_str()),
            (Some(409), Some("Sleep token expired or already used"))
        );
        assert_eq!(error["power"]["accepted"], false);
        assert!(refused.get("ok").is_none());
        let invalid = parse(&execute(&fake, "power.sleep", ""));
        assert_eq!(invalid["error"]["kind"], "invalid_argument");
    }

    #[test]
    fn the_couch_menu_parses_strictly_and_keeps_api_errors() {
        let fake = Fake::default();
        let reply = parse(&execute(&fake, "couch_menu", r#"{"open":true}"#));
        assert_eq!(reply["ok"]["open"], true);
        assert_eq!(
            fake.calls.lock().unwrap().last().unwrap(),
            "couch menu true"
        );

        let calls = fake.calls.lock().unwrap().len();
        for bad in ["", "{}", "true", r#"{"open":true,"page":"radeon"}"#] {
            let reply = parse(&execute(&fake, "couch_menu", bad));
            assert_eq!(reply["error"]["kind"], "invalid_argument", "{bad}");
        }
        assert_eq!(fake.calls.lock().unwrap().len(), calls);

        let fake = Fake {
            fail: Some(ErrorCode::NoSession),
            ..Fake::default()
        };
        let reply = parse(&execute(&fake, "couch_menu", r#"{"open":true}"#));
        assert_eq!(reply["error"]["kind"], "api");
        assert_eq!(reply["error"]["code"], "no_session");
        assert_eq!(reply["error"]["http_status"], ErrorCode::NoSession.status());
        assert_eq!(reply["error"]["message"], "Start a stream first.");
    }

    #[test]
    fn wake_answers_with_the_targets() {
        let fake = Fake::default();
        let reply = parse(&execute(&fake, "wake", ""));
        assert_eq!(reply["ok"]["primary"], "02:7c:16:b4:13:4f");
        assert_eq!(reply["ok"]["targets"][0]["broadcasts"][0], "192.168.4.255");
        assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "wake");
    }

    #[test]
    fn an_unknown_operation_is_an_argument_error() {
        let reply = parse(&execute(&Fake::default(), "reboot", ""));
        assert_eq!(reply["error"]["kind"], "invalid_argument");
    }

    #[test]
    fn an_unusable_identity_or_certificate_is_reported_not_thrown() {
        let endpoint = Endpoint {
            address: "192.0.2.1".into(),
            http_port: 47989,
            https_port: 47984,
            server_certificate_pem: String::new(),
            unique_id: "0123456789ABCDEF".into(),
            client_certificate_der: vec![1, 2, 3],
            client_private_key_pkcs8: vec![4, 5, 6],
        };
        // Bytes that are no certificate come back as an error envelope, not a panic.
        let reply = parse(&request(&endpoint, "session", ""));
        assert!(reply.get("ok").is_none());
        assert!(["malformed", "identity"].contains(&reply["error"]["kind"].as_str().unwrap()));
        let reply = parse(&request(
            &Endpoint {
                unique_id: "not valid!".into(),
                ..endpoint
            },
            "session",
            "",
        ));
        assert_eq!(reply["error"]["kind"], "identity");
    }

    #[test]
    fn a_generated_identity_reaches_the_certificate_check() {
        let identity = ClientIdentity::generate().unwrap();
        let endpoint = Endpoint {
            address: "192.0.2.1".into(),
            http_port: 47989,
            https_port: 47984,
            server_certificate_pem: "not a certificate".into(),
            unique_id: identity.unique_id().to_string(),
            client_certificate_der: identity.certificate_der().to_vec(),
            client_private_key_pkcs8: identity.private_key_pkcs8().to_vec(),
        };
        let reply = parse(&request(&endpoint, "library", ""));
        assert_ne!(reply["error"]["kind"], Value::Null);
        assert!(reply.get("ok").is_none());
    }
}
