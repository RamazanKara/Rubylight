//! The JNI side of `com.limelight.nvstream.jni.NativeApiBridge`: the Rubylight native API v1
//! (`HostClient::{native_capabilities, library, session, session_action, couch_menu, radeon,
//! write_radeon, restore_radeon, wake, audio, write_audio}`) for the console screens and the
//! stream menu, and Sleep PC (`power_capabilities`, `sleep`), Restart and Shut down PC
//! (`pc_power`, `power_pc`), the host's update (`host_update`, `install_host_update`), the file
//! transfer permissions (`files.capabilities`), and Nucleus Co-op (`coop`, `coop.join`).
//! Calls block on HTTPS, so Java runs them on worker threads, never on the UI thread.
//!
//! One entry point, `request`, takes the operation, the host endpoint and an optional JSON
//! argument, and always answers with one JSON object: `{"ok": <the API's JSON>}` or
//! `{"error": {...}}`. Java decodes both, so no exception crosses the JNI boundary and the
//! error keeps its API code, HTTP status and setting.

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jstring, JNI_FALSE};
use jni::JNIEnv;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

use nightjar_core::http::native_api::audio::{Audio, AudioWrite};
use nightjar_core::http::native_api::radeon::{Radeon, SettingsWrite};
use nightjar_core::http::native_api::session::{ActionId, CouchMenu, CouchMenuRequest, Session};
use nightjar_core::http::native_api::wake::Wake;
use nightjar_core::http::{
    update_arrived, valid_pairing_pin, ClientIdentity, Coop, CoopJoined, Error, HostClient,
    HostUpdate, NativeCapabilities, NativeLibrary, PairingApprove, PairingApproved, PairingRequests, PcPower, PcPowerOffer, PowerError, PowerReply, Refusal, StreamMove,
    GuestAction, SessionGuests,
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
    fn library(&self) -> Result<NativeLibrary, Error>;
    fn session(&self) -> Result<Session, Error>;
    fn session_action(&self, action: ActionId) -> Result<Session, Error>;
    fn stream_move(&self) -> Result<StreamMove, Error>;
    fn continue_here(&self) -> Result<Session, Error>;
    fn couch_menu(&self, open: bool) -> Result<CouchMenu, Error>;
    fn radeon(&self) -> Result<Radeon, Error>;
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error>;
    fn restore_radeon(&self) -> Result<Radeon, Error>;
    fn power_capabilities(&self) -> Result<PowerReply, PowerError>;
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError>;
    fn pc_power(&self) -> Result<PcPowerOffer, Error>;
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error>;
    fn host_update(&self) -> Result<HostUpdate, Error>;
    fn install_host_update(&self) -> Result<HostUpdate, Error>;
    fn pairing_requests(&self) -> Result<PairingRequests, Error>;
    fn approve_pairing(&self, id: &str, pin: &str) -> Result<PairingApproved, Error>;
    fn deny_pairing(&self, id: &str) -> Result<PairingRequests, Error>;
    fn session_guests(&self) -> Result<SessionGuests, Error>;
    fn guest_action(&self, action: &GuestAction) -> Result<SessionGuests, Error>;
    fn wake(&self) -> Result<Wake, Error>;
    fn coop(&self) -> Result<Coop, Error>;
    fn join_coop(&self, session: u32) -> Result<CoopJoined, Error>;
    fn audio(&self) -> Result<Audio, Error>;
    fn write_audio(&self, write: &AudioWrite) -> Result<Audio, Error>;
}

impl Host for HostClient {
    fn capabilities_with_files(&self) -> Result<NativeCapabilities, Error> {
        self.native_capabilities_with_files()
    }
    fn library(&self) -> Result<NativeLibrary, Error> {
        HostClient::library(self)
    }
    fn session(&self) -> Result<Session, Error> {
        HostClient::session(self)
    }
    fn session_action(&self, action: ActionId) -> Result<Session, Error> {
        HostClient::session_action(self, action)
    }
    fn stream_move(&self) -> Result<StreamMove, Error> {
        HostClient::stream_move(self)
    }
    fn continue_here(&self) -> Result<Session, Error> {
        HostClient::continue_here(self)
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
    fn pc_power(&self) -> Result<PcPowerOffer, Error> {
        HostClient::pc_power(self)
    }
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error> {
        HostClient::power_pc(self, action, token)
    }
    fn host_update(&self) -> Result<HostUpdate, Error> {
        HostClient::host_update(self)
    }
    fn install_host_update(&self) -> Result<HostUpdate, Error> {
        HostClient::install_host_update(self)
    }
    fn pairing_requests(&self) -> Result<PairingRequests, Error> {
        HostClient::pairing_requests(self)
    }
    fn approve_pairing(&self, id: &str, pin: &str) -> Result<PairingApproved, Error> {
        HostClient::approve_pairing(self, id, pin)
    }
    fn deny_pairing(&self, id: &str) -> Result<PairingRequests, Error> {
        HostClient::deny_pairing(self, id)
    }
    fn session_guests(&self) -> Result<SessionGuests, Error> {
        HostClient::session_guests(self)
    }
    fn guest_action(&self, action: &GuestAction) -> Result<SessionGuests, Error> {
        HostClient::guest_action(self, action)
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
    fn audio(&self) -> Result<Audio, Error> {
        HostClient::audio(self)
    }
    fn write_audio(&self, write: &AudioWrite) -> Result<Audio, Error> {
        HostClient::write_audio(self, write)
    }
}

/// Set in the process of a device's second screen (Dual screen mode): every client this module
/// builds marks its requests as that stream's.
static SECOND_SCREEN: AtomicBool = AtomicBool::new(false);

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
    Ok(if SECOND_SCREEN.load(Ordering::Relaxed) {
        client.with_second_screen()
    } else {
        client
    })
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
        permissions.insert("restart_shutdown".into(), json!(files.restart_shutdown));
        permissions.insert("pairing_approve".into(), json!(files.pairing_approve));
        Ok(value)
    }))
}

/// The update with what the shared rules make of it, so Java decides as desktop does: `offered`
/// (the version to install, or null), `installing_now` (no Install now to offer), `in_progress`,
/// `failed` with `failure` (the updater's words, or null), and while following an install to
/// `target`, `arrived` (the PC answers on `target`) and `gave_up`.
fn update_json(result: Result<HostUpdate, Error>, target: &str) -> String {
    ok(result.and_then(|update| {
        let mut value = serde_json::to_value(&update)
            .map_err(|e| Error::Malformed(format!("native API reply: {e}")))?;
        value["offered"] = json!(update.offered());
        value["installing_now"] = json!(update.installing_now());
        value["in_progress"] = json!(update.in_progress());
        value["failed"] = json!(update.failure().is_some());
        value["failure"] = json!(update.failure().flatten());
        if !target.is_empty() {
            value["arrived"] = json!(update_arrived(&update.current_version, target));
            value["gave_up"] = json!(update.gave_up(target));
        }
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
/// (argument: the token from `power.capabilities`), `pc_power`, `pc_power.restart` and
/// `pc_power.shutdown` (argument: the token from `pc_power`; `{}` once the PC took it), `update`
/// (argument: the version an install follows, or empty) and `update.install` (both as
/// [`update_json`]), `coop` and `coop.join` (argument: the session id), `audio` and
/// `audio_write` (argument: the `AudioWrite` JSON). Gate Sleep on `Feature::Sleep`, Restart and
/// Shut down on the capabilities' `power.restart` and `power.shutdown`, the update ones on
/// `update`, the co-op ones on `Feature::Coop` and the audio ones on the capabilities'
/// `audio.night_mode` and `audio.headphone_surround`. A refused restart, shutdown or update is
/// the usual API error, its `message` in the host's words.
pub fn execute(host: &dyn Host, op: &str, argument: &str) -> String {
    match op {
        // The pinned `Capabilities` drops `permissions.usb`, so the stream menu never offered USB.
        "capabilities" | "files.capabilities" => {
            capabilities_with_files(host.capabilities_with_files())
        }
        "library" => ok(host.library()),
        "session" => ok(host.session()),
        "session_move" => ok(host.stream_move()),
        "session_continue_here" => ok(host.continue_here()),
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
        "pc_power" => ok(host.pc_power()),
        "pc_power.restart" | "pc_power.shutdown" => {
            let action = if op == "pc_power.restart" {
                PcPower::Restart
            } else {
                PcPower::ShutDown
            };
            ok(host.power_pc(action, argument).map(|()| json!({})))
        }
        "update" => update_json(host.host_update(), argument),
        "update.install" => update_json(host.install_host_update(), ""),
        "pairing.requests" => ok(host.pairing_requests()),
        "pairing.approve" => match serde_json::from_str::<PairingApprove>(argument) {
            Ok(approve) if valid_pairing_pin(&approve.pin) => {
                ok(host.approve_pairing(&approve.id, &approve.pin))
            }
            Ok(_) => argument_error("pairing approval: the PIN has four digits".to_owned()),
            Err(e) => argument_error(format!("pairing approval: {e}")),
        },
        "pairing.deny" => ok(host.deny_pairing(argument)),
        "guests" => ok(host.session_guests()),
        "guests.action" => match GuestAction::from_json(argument) {
            Ok(action) => ok(host.guest_action(&action)),
            Err(e) => argument_error(e),
        },
        "wake" => ok(host.wake()),
        "coop" => ok(host.coop()),
        "coop.join" => match argument.trim().parse::<u32>() {
            Ok(session) => ok(host.join_coop(session)),
            Err(_) => argument_error(format!("co-op session id: {argument:?}")),
        },
        "audio" => ok(host.audio()),
        "audio_write" => match serde_json::from_str::<AudioWrite>(argument) {
            Ok(write) => ok(host.write_audio(&write)),
            Err(e) => argument_error(format!("audio write: {e}")),
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
pub extern "system" fn Java_com_limelight_nvstream_jni_NativeApiBridge_setSecondScreen(
    _: JNIEnv,
    _: JClass,
    second_screen: jboolean,
) {
    SECOND_SCREEN.store(second_screen != JNI_FALSE, Ordering::Relaxed);
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
mod tests;
