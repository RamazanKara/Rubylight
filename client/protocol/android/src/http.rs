//! JNI bridge for `com.limelight.nvstream.jni.NightjarHttpJni` (HTTP, TLS, pairing, launch, probes).
//!
//! Stores `Arc<HostClient>` and `Arc<TransferCancel>` in lock-free-outer [`Slab`] tables so
//! `cancelPendingRequests` never blocks behind an in-flight call. Every reply crosses JNI as a
//! canonical `{"ok": ...}` / `{"error": {"kind", "message", ...}}` JSON envelope.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jint, jlong, jstring};
use nightjar_core::http::{
    App, AppControl, ClientIdentity, Error, Evidence, HostCapabilities, HostClient, InfoPath,
    PairFailure, PairResult, ServerInfo, TransferCancel, parse_app_list,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::native_api;
use crate::slab::Slab;

pub(crate) static CLIENTS: Slab<HostClient> = Slab::new();
pub(crate) static CANCELS: Slab<TransferCancel> = Slab::new();

pub(crate) fn jbool(v: bool) -> jboolean {
    if v { JNI_TRUE } else { JNI_FALSE }
}

pub(crate) fn new_jstring(env: &JNIEnv, text: &str) -> jstring {
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

pub(crate) fn ok_json<T: Serialize>(value: T) -> String {
    json!({"ok": value}).to_string()
}

pub(crate) fn error_json(error: &Error) -> String {
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
    let mut body = json!({
        "kind": kind,
        "message": error.to_string(),
    });
    if let Some(status) = error.http_status() {
        body["http_status"] = json!(status);
    }
    if let Error::Status { code, message } = error {
        body["error_code"] = json!(code);
        body["error_message"] = json!(message);
    }
    if let Some(api) = error.api_error() {
        body["code"] = serde_json::to_value(api.code).unwrap_or(Value::Null);
        body["setting"] = json!(api.setting);
        body["retry_after_ms"] = json!(api.retry_after_ms);
        put_explanation(&mut body, api);
    }
    json!({"error": body}).to_string()
}

/// The host's `title`, `next_step` and `technical_detail`, each only when it sent it, so Java's
/// error text is the message alone for a host without them.
pub(crate) fn put_explanation(body: &mut Value, api: &nightjar_core::http::native_api::error::ApiError) {
    for (key, part) in [("title", &api.title), ("next_step", &api.next_step), ("technical_detail", &api.technical_detail)] {
        if let Some(part) = part {
            body[key] = json!(part);
        }
    }
}

pub(crate) fn server_info_json(info: &ServerInfo) -> Value {
    json!({
        "raw": info.raw,
        "hostname": info.hostname,
        "unique_id": info.unique_id,
        "app_version": info.app_version,
        "gfe_version": info.gfe_version,
        "gpu_type": info.gpu_type,
        "https_port": info.https_port,
        "external_port": info.external_port,
        "mac_address": info.mac_address,
        "local_ip": info.local_ip,
        "external_ip": info.external_ip,
        "paired": info.paired,
        "state": info.state,
        "session_idle": info.session_idle,
        "current_game": info.current_game,
        "current_game_uuid": info.current_game_uuid,
        "server_codec_mode_support": info.server_codec_mode_support,
        "max_luma_pixels_h264": info.max_luma_pixels_h264,
        "max_luma_pixels_hevc": info.max_luma_pixels_hevc,
        "permission": info.permission,
        "server_commands": info.server_commands,
        "encoder_bitrate_supported": info.encoder_bitrate_supported,
        "encoder_bitrate_kbps": info.encoder_bitrate_kbps,
        "initial_encoder_bitrate_kbps": info.initial_encoder_bitrate_kbps,
        "frame_limiter_enabled": info.frame_limiter_enabled,
        "virtual_display_frame_limiter_enabled": info.virtual_display_frame_limiter_enabled,
        "frame_limiter_fps_limit_millihz": info.frame_limiter_fps_limit_millihz,
        "pyrowave_host_link_mbps": info.pyrowave_host_link_mbps,
        "pyrowave_bandwidth_probe_bytes": info.pyrowave_bandwidth_probe_bytes,
        "pyrowave_burst_probe": info.pyrowave_burst_probe,
        "pyrowave_critical_fec_percentage": info.pyrowave_critical_fec_percentage,
        "pyrowave_min_parity_shards": info.pyrowave_min_parity_shards,
        "packed444_modes": info.packed444_modes,
        "last_failure": info.last_failure,
        "capabilities": crate::capabilities::snapshot(&info.capabilities),
        "nvidia_server": info.nvidia_server(),
        "supports_4k": info.supports_4k(),
    })
}

pub(crate) fn caps_from_arg(env: &mut JNIEnv, caps_json: &JString) -> HostCapabilities {
    native_api::string(env, caps_json)
        .and_then(|s| Evidence::from_versioned_json(&s).map(HostCapabilities::from_evidence))
        .unwrap_or(HostCapabilities::UNKNOWN)
}

pub(crate) fn invalid_client() -> String {
    error_json(&Error::InvalidArgument("unknown HTTP client handle".into()))
}

// --- Client & Cancel lifecycle ---

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientCreate(
    mut env: JNIEnv,
    _: JClass,
    address: JString,
    http_port: jint,
    https_port: jint,
    unique_id: JString,
    client_cert_der: JByteArray,
    client_key_pkcs8: JByteArray,
    server_cert_der: JByteArray,
    device_name: JString,
) -> jlong {
    let Some(address) = native_api::string(&mut env, &address) else {
        return 0;
    };
    let port = u16::try_from(http_port).unwrap_or(47989);
    let cert_der = native_api::bytes(&mut env, &client_cert_der).unwrap_or_default();
    let key_pkcs8 = native_api::bytes(&mut env, &client_key_pkcs8).unwrap_or_default();
    let uid = native_api::string(&mut env, &unique_id).unwrap_or_default();
    let client_res = if !cert_der.is_empty() && !key_pkcs8.is_empty() {
        ClientIdentity::from_der(uid, cert_der, key_pkcs8)
            .and_then(|id| HostClient::new(&address, port, Arc::new(id)))
    } else {
        HostClient::new_unauthenticated(&address, port, uid)
    };
    let Ok(mut client) = client_res else {
        return 0;
    };
    if let Some(name) = native_api::string(&mut env, &device_name)
        && !name.is_empty()
    {
        client = client.with_device_name(name);
    }
    if let Ok(hp) = u16::try_from(https_port)
        && hp > 0
    {
        client = client.with_https_port(hp);
    }
    if let Some(der) = native_api::bytes(&mut env, &server_cert_der)
        && !der.is_empty()
    {
        client.pin_server_certificate_der(Some(der));
    }
    if native_api::SECOND_SCREEN.load(Ordering::Relaxed) {
        client = client.with_second_screen();
    }
    CLIENTS.insert(client)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_setAppVersion(
    mut env: JNIEnv,
    _: JClass,
    version: JString,
) {
    if let Some(v) = native_api::string(&mut env, &version) {
        nightjar_core::http::set_app_version(&v);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    CLIENTS.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientCancelPendingRequests(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(client) = CLIENTS.get(handle) {
        client.cancel_pending_requests();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientSetHttpsPort(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    port: jint,
) {
    if let (Some(client), Ok(port)) = (CLIENTS.get(handle), u16::try_from(port)) {
        client.set_https_port(port);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientGetHttpsPort(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jint {
    CLIENTS
        .get(handle)
        .map(|c| i32::from(c.https_port_cached()))
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientSetServerCertDer(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    cert_der: JByteArray,
) {
    let Some(client) = CLIENTS.get(handle) else {
        return;
    };
    let der = native_api::bytes(&mut env, &cert_der).filter(|d| !d.is_empty());
    client.pin_server_certificate_der(der);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientGetServerCertDer(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jbyteArray {
    let Some(der) = CLIENTS.get(handle).and_then(|c| c.server_certificate_der()) else {
        return std::ptr::null_mut();
    };
    match env.byte_array_from_slice(&der) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_setSecondScreen(
    _: JNIEnv,
    _: JClass,
    second_screen: jboolean,
) {
    native_api::SECOND_SCREEN.store(second_screen != JNI_FALSE, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    CANCELS.insert(TransferCancel::new())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    CANCELS.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelCancel(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(c) = CANCELS.get(handle) {
        c.cancel();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelIsCancelled(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jboolean {
    jbool(CANCELS.get(handle).is_some_and(|c| c.is_cancelled()))
}

// --- ServerInfo, Pairing, AppList ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getServerInfo(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    likely_online: jboolean,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.server_info_with_cert_check(likely_online != JNI_FALSE) {
            Ok((info, path)) => {
                let path_name = match path {
                    InfoPath::Pinned => "pinned",
                    InfoPath::Refused => "refused",
                    InfoPath::CertificateMismatch => "cert_mismatch",
                    InfoPath::Plain => "plain",
                };
                ok_json(json!({
                    "path": path_name,
                    "info": server_info_json(&info),
                }))
            }
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probePairedComputer(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.server_info_https_strict(Duration::from_secs(3), 65_536) {
            Ok(info) => ok_json(server_info_json(&info)),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_parseServerInfoXml(
    mut env: JNIEnv,
    _: JClass,
    xml: JString,
    http_port: jint,
    strict: jboolean,
) -> jstring {
    let raw = native_api::string(&mut env, &xml).unwrap_or_default();
    let port = u16::try_from(http_port).unwrap_or(47989);
    let parsed = if strict != JNI_FALSE {
        ServerInfo::parse(&raw, port)
    } else {
        ServerInfo::parse_partial(&raw, port)
    };
    let reply = match parsed {
        Ok(info) => ok_json(server_info_json(&info)),
        Err(e) => error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_pair(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    server_info_xml: JString,
    pin: JString,
    passphrase: JString,
    expected_cert_sha256: JByteArray,
) -> jstring {
    let reply = (|| {
        let Some(client) = CLIENTS.get(handle) else {
            return invalid_client();
        };
        let raw_info = native_api::string(&mut env, &server_info_xml).unwrap_or_default();
        let info = match ServerInfo::parse(&raw_info, 47989) {
            Ok(i) => i,
            Err(e) => return error_json(&e),
        };
        let pin = native_api::string(&mut env, &pin).unwrap_or_default();
        let passphrase = native_api::string(&mut env, &passphrase);
        let expected_fp = native_api::bytes(&mut env, &expected_cert_sha256).and_then(|b| {
            let arr: [u8; 32] = b.try_into().ok()?;
            Some(arr)
        });
        let result = client.pair_checked(&info, &pin, passphrase.as_deref(), expected_fp);
        match result {
            Ok(PairResult::Paired {
                server_certificate_pem,
            }) => ok_json(json!({
                "state": "PAIRED",
                "server_certificate_pem": server_certificate_pem,
                "server_certificate_der": client.server_certificate_der(),
            })),
            Ok(PairResult::PinWrong) => ok_json(json!({"state": "PIN_WRONG"})),
            Ok(PairResult::AlreadyInProgress) => ok_json(json!({"state": "ALREADY_IN_PROGRESS"})),
            Ok(PairResult::Failed(PairFailure::ServerSignature)) => {
                ok_json(json!({"state": "SERVER_UNVERIFIED"}))
            }
            Ok(PairResult::Failed(PairFailure::Declined(_))) => ok_json(json!({"state": "FAILED"})),
            Err(Error::CertificateMismatch) => ok_json(json!({"state": "CERT_MISMATCH"})),
            Err(e) => error_json(&e),
        }
    })();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_unpair(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.unpair() {
            Ok(()) => ok_json(true),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

fn app_json(app: &App) -> Value {
    let control = match app.control {
        AppControl::None => "NONE",
        AppControl::Resume => "RESUME",
        AppControl::DisconnectMonitor => "DISCONNECT_MONITOR",
        AppControl::DisconnectInput => "DISCONNECT_INPUT",
        AppControl::Terminate => "TERMINATE",
        AppControl::Monitor => "MONITOR",
        AppControl::Input => "INPUT",
        AppControl::RunningGame => "RUNNING_GAME",
    };
    json!({
        "name": app.name,
        "id": app.id,
        "hdr_supported": app.hdr_supported,
        "uuid": app.uuid,
        "host_index": app.host_index,
        "art_version": app.art_version,
        "control": control,
        "monitor_resume": app.monitor_resume,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getAppListRaw(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.app_list_raw() {
            Ok(raw) => ok_json(raw),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_parseAppListXml(
    mut env: JNIEnv,
    _: JClass,
    xml: JString,
    caps_json: JString,
) -> jstring {
    let raw = native_api::string(&mut env, &xml).unwrap_or_default();
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match parse_app_list(&raw, &caps) {
        Ok(apps) => ok_json(apps.iter().map(app_json).collect::<Vec<_>>()),
        Err(e) => error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getBoxArt(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    app_id: jint,
) -> jbyteArray {
    let Some(client) = CLIENTS.get(handle) else {
        let _ = env.throw_new("java/io/IOException", "Unknown HTTP client");
        return std::ptr::null_mut();
    };
    match client.box_art(app_id) {
        Ok(bytes) => match env.byte_array_from_slice(&bytes) {
            Ok(arr) => arr.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        Err(Error::NotFound(_)) => std::ptr::null_mut(),
        Err(e) => {
            let _ = env.throw_new("java/io/IOException", e.to_string());
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getBoxArtUrl(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    app_id: jint,
) -> jstring {
    let Some(client) = CLIENTS.get(handle) else {
        let _ = env.throw_new("java/io/IOException", "Unknown HTTP client");
        return std::ptr::null_mut();
    };
    match client.box_art_url(app_id) {
        Ok(url) => new_jstring(&env, &url),
        Err(e) => {
            let _ = env.throw_new("java/io/IOException", e.to_string());
            std::ptr::null_mut()
        }
    }
}
