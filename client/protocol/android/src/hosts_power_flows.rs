//! JNI bridge exports for `NightjarHostsJni` pairing, unpair, invite redemption, Sleep PC,
//! Restart/Shut down PC, and Host Update flows backed by `nightjar_core::hosts::{pairing, power}`.

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong, jstring};
use nightjar_core::hosts::pairing::{
    PairSecret, execute_pair_checked, execute_unpair_verified, pins_another_pem,
};
use nightjar_core::hosts::power::{
    PcPowerFailure, PcPowerHost, PowerHost, SleepFailure, UpdateHost, follow_update_cancellable,
    pc_power_confirmed_at, pc_power_refusal, pc_power_token, sleep_confirmed_at, sleep_token,
    update_target,
};
use nightjar_core::http::{
    ClientIdentity, Error, HostClient, HostUpdate, PcPower, PcPowerOffer, PowerError, PowerReply,
    Refusal, native_api::error::{ApiError, ErrorCode},
};
use nightjar_policy::hosts::{
    PC_POWER_REFUSAL_POLL_MS, PC_POWER_REFUSAL_READS, PairFailureKind, TokenStamp,
    UPDATE_MAX_WAIT_MS, UPDATE_POLL_MS, UPDATE_WAIT_MS, UnpairFailureKind, UpdateStepAction,
};
use rubylight_protocol::pairing_uri::PairingUri;
use serde_json::{Value, json};

use crate::http::{CLIENTS, new_jstring};
use crate::native_api;

use super::host_flows::pair_failure_code;
use super::hosts_manager::HOST_MANAGERS;
use super::{base64_encode, jstr};

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowPair(
    mut env: JNIEnv,
    _: JClass,
    http_handle: jlong,
    manager_handle: jlong,
    pairing_type: jint,
    pin: JString,
    passphrase: JString,
    link_uri: JString,
    has_pinned_cert: jboolean,
    in_game: jboolean,
) -> jstring {
    let fail = |kind: PairFailureKind| -> String {
        json!({"ok": false, "kind": pair_failure_code(kind)}).to_string()
    };
    let Some(client) = CLIENTS.get(http_handle) else {
        return new_jstring(&env, &fail(PairFailureKind::Network));
    };
    let mut known_otherwise = Vec::new();
    let secret = match pairing_type {
        0 => PairSecret::Pin(jstr(&mut env, &pin).unwrap_or_default()),
        1 => PairSecret::OneTime {
            pin: jstr(&mut env, &pin).unwrap_or_default(),
            passphrase: jstr(&mut env, &passphrase).unwrap_or_default(),
        },
        2 => {
            let Some(uri_text) = jstr(&mut env, &link_uri) else {
                return new_jstring(&env, &fail(PairFailureKind::Failed));
            };
            let Some(uri) = PairingUri::parse(&uri_text) else {
                return new_jstring(&env, &fail(PairFailureKind::Failed));
            };
            if let Some(bridge) = HOST_MANAGERS.get(manager_handle) {
                for snap in bridge.manager.snapshots() {
                    if pins_another_pem(
                        snap.entry.shared.server_certificate.as_deref(),
                        &uri.cert_sha256,
                    ) {
                        known_otherwise.push(snap.entry.shared.unique_id);
                    }
                }
            }
            PairSecret::Link(Box::new(uri))
        }
        _ => return new_jstring(&env, &fail(PairFailureKind::Failed)),
    };
    let res = match execute_pair_checked(
        &client,
        &secret,
        has_pinned_cert != 0,
        in_game != 0,
        &known_otherwise,
    ) {
        Ok(cert_der) => json!({"ok": true, "cert_b64": base64_encode(&cert_der)}).to_string(),
        Err(kind) => fail(kind),
    };
    new_jstring(&env, &res)
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowUnpair(
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
) -> jint {
    let Some(addr) = native_api::string(&mut env, &address) else {
        return 3;
    };
    let port = u16::try_from(http_port).unwrap_or(47989);
    let cert_der = native_api::bytes(&mut env, &client_cert_der).unwrap_or_default();
    let key_pkcs8 = native_api::bytes(&mut env, &client_key_pkcs8).unwrap_or_default();
    let uid = native_api::string(&mut env, &unique_id).unwrap_or_default();
    let client_res = if !cert_der.is_empty() && !key_pkcs8.is_empty() {
        ClientIdentity::from_der(uid, cert_der, key_pkcs8)
            .and_then(|id| HostClient::new(&addr, port, Arc::new(id)))
    } else {
        HostClient::new_unauthenticated(&addr, port, uid)
    };
    let Ok(mut client) = client_res else {
        return 3;
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
    match execute_unpair_verified(&client) {
        Ok(()) => -1,
        Err(UnpairFailureKind::NotPaired) => 0,
        Err(UnpairFailureKind::UnknownHost) => 1,
        Err(UnpairFailureKind::NotFound) => 2,
        Err(UnpairFailureKind::Failed) => 3,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowRedeemInvite(
    mut env: JNIEnv,
    _: JClass,
    link: JString,
    unique_id: JString,
    cert_pem: JString,
    key_pem: JString,
    device_name: JString,
    now_sec: jlong,
) -> jstring {
    let out = (|| -> Result<Value, String> {
        let link_text = jstr(&mut env, &link).ok_or_else(|| "missing link".to_owned())?;
        let parsed = nightjar_core::invite::parse(&link_text)?;
        let uid = jstr(&mut env, &unique_id).ok_or_else(|| "missing uniqueId".to_owned())?;
        let cert = jstr(&mut env, &cert_pem).ok_or_else(|| "missing certificate".to_owned())?;
        let key = jstr(&mut env, &key_pem).ok_or_else(|| "missing key".to_owned())?;
        let name = jstr(&mut env, &device_name).unwrap_or_else(|| "Nightjar".into());
        let id = ClientIdentity::from_pem(&uid, &cert, &key).map_err(|e| e.to_string())?;
        let redeemed =
            nightjar_core::invite::redeem(&parsed, Arc::new(id), &name, now_sec.max(0) as u64)?;
        Ok(json!({
            "uniqueId": redeemed.unique_id,
            "name": redeemed.name,
            "address": redeemed.address.ip().to_string(),
            "port": redeemed.address.port(),
            "httpsPort": redeemed.https_port,
            "path": redeemed.path.key(),
            "certificate": redeemed.server_certificate_pem,
            "reply": redeemed.reply,
        }))
    })();
    let json = match out {
        Ok(v) => json!({ "ok": v }).to_string(),
        Err(e) => json!({ "error": e }).to_string(),
    };
    new_jstring(&env, &json)
}

struct FlowBridge<'a, 'local> {
    env: RefCell<JNIEnv<'local>>,
    transport: &'a JObject<'local>,
    sleeper: Option<&'a JObject<'local>>,
    listener: Option<&'a JObject<'local>>,
    last_error: RefCell<Option<Value>>,
    aborted: Cell<bool>,
}

impl<'a, 'local> FlowBridge<'a, 'local> {
    fn new(
        env: JNIEnv<'local>,
        transport: &'a JObject<'local>,
        sleeper: Option<&'a JObject<'local>>,
        listener: Option<&'a JObject<'local>>,
    ) -> Self {
        Self {
            env: RefCell::new(env),
            transport,
            sleeper,
            listener,
            last_error: RefCell::new(None),
            aborted: Cell::new(false),
        }
    }

    fn call_raw(&self, op: &str, arg: &str) -> Result<Value, Error> {
        if self.aborted.get() {
            return Err(Error::Cancelled);
        }
        let text = {
            let mut env = self.env.borrow_mut();
            let res: Result<Result<String, Error>, jni::errors::Error> =
                env.with_local_frame(8, |env| {
                    let j_op = env.new_string(op)?;
                    let j_arg = env.new_string(arg)?;
                    let val = env.call_method(
                        self.transport,
                        "call",
                        "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                        &[JValue::Object(&j_op), JValue::Object(&j_arg)],
                    )?;
                    let obj = val.l()?;
                    if obj.is_null() {
                        return Ok(Err(Error::Malformed("null envelope".into())));
                    }
                    let j_text = JString::from(obj);
                    let text = env.get_string(&j_text)?.to_string_lossy().into_owned();
                    Ok(Ok(text))
                });
            match res {
                Ok(inner) => inner?,
                Err(jni::errors::Error::JavaException) => {
                    let _ = env.exception_clear();
                    return Err(Error::Io(std::io::Error::other("FlowTransport error")));
                }
                Err(err) => {
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_clear();
                    }
                    return Err(Error::Malformed(err.to_string()));
                }
            }
        };

        let parsed: Value =
            serde_json::from_str(&text).map_err(|e| Error::Malformed(e.to_string()))?;
        if let Some(ok) = parsed.get("ok") {
            *self.last_error.borrow_mut() = None;
            return Ok(ok.clone());
        }
        if let Some(err) = parsed.get("error") {
            *self.last_error.borrow_mut() = Some(err.clone());
            return Err(error_from_json(err));
        }
        Err(Error::Malformed("Unrecognized native API reply".into()))
    }

    fn sleep_ms(&self, ms: u64) -> bool {
        let Some(sleeper) = self.sleeper else {
            return false;
        };
        let mut env = self.env.borrow_mut();
        let res = env.call_method(sleeper, "sleep", "(J)Z", &[JValue::Long(ms as jlong)]);
        let keep_going = match res {
            Ok(v) => v.z().unwrap_or(false),
            Err(_) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
                false
            }
        };
        if !keep_going {
            self.aborted.set(true);
        }
        keep_going
    }

    fn emit_step(&self, payload: &Value) {
        let Some(listener) = self.listener else {
            return;
        };
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(4, |env| {
            let j_str = env.new_string(payload.to_string())?;
            env.call_method(
                listener,
                "onStep",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&j_str)],
            )?;
            Ok::<(), jni::errors::Error>(())
        });
        if res.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }

    fn failed_json(&self, fallback: &impl std::fmt::Display) -> Value {
        if let Some(raw) = self.last_error.borrow().clone() {
            return json!({ "outcome": "failed", "error": raw });
        }
        json!({
            "outcome": "failed",
            "error": {
                "kind": "malformed",
                "message": fallback.to_string(),
                "http_status": 0,
            }
        })
    }
}

fn error_from_json(err: &Value) -> Error {
    let kind = err.get("kind").and_then(Value::as_str).unwrap_or("");
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Native API error")
        .to_owned();
    let http_status = err.get("http_status").and_then(Value::as_u64).unwrap_or(0) as u16;
    match kind {
        "network" | "tls" => Error::Io(std::io::Error::other(message)),
        "not_paired" => Error::NotPaired,
        "api" => {
            let part = |key: &str| err.get(key).and_then(Value::as_str).map(str::to_owned);
            let mut error = ApiError::new(ErrorCode::InvalidRequest, message);
            error.title = part("title");
            error.next_step = part("next_step");
            error.technical_detail = part("technical_detail");
            Error::Api { status: http_status, error: Box::new(error) }
        }
        "cancelled" => Error::Cancelled,
        _ => Error::Malformed(message),
    }
}

fn parse_power_reply(v: &Value) -> Result<PowerReply, Error> {
    let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    Ok(PowerReply {
        status: v.get("status").and_then(Value::as_u64).unwrap_or(200) as u16,
        message: v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        version: v.get("version").and_then(Value::as_u64).unwrap_or(1) as u32,
        sleep_supported: b("sleep_supported"),
        sleep_allowed: b("sleep_allowed"),
        sleep_available: b("sleep_available"),
        active_session_policy: v
            .get("active_session_policy")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        token: v.get("token").and_then(Value::as_str).map(ToOwned::to_owned),
        token_lifetime_seconds: v
            .get("token_lifetime_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(30) as u32,
        accepted: b("accepted"),
    })
}

fn refusal_kind(name: Option<&str>) -> Refusal {
    match name {
        Some("bad_request") => Refusal::BadRequest,
        Some("unauthorized") => Refusal::Unauthorized,
        Some("forbidden") => Refusal::Forbidden,
        Some("method_not_allowed") => Refusal::MethodNotAllowed,
        Some("conflict") => Refusal::Conflict,
        Some("not_implemented") => Refusal::NotImplemented,
        _ => Refusal::Unavailable,
    }
}

impl PowerHost for FlowBridge<'_, '_> {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        self.call_power("power.capabilities", "")
    }

    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        self.call_power("power.sleep", token)
    }
}

impl FlowBridge<'_, '_> {
    fn call_power(&self, op: &str, arg: &str) -> Result<PowerReply, PowerError> {
        match self.call_raw(op, arg) {
            Ok(ok) => parse_power_reply(&ok).map_err(PowerError::Host),
            Err(err) => {
                if let Some(raw) = self.last_error.borrow().as_ref()
                    && raw.get("kind").and_then(Value::as_str) == Some("power_refused")
                    && let Ok(reply) = parse_power_reply(&raw["power"])
                {
                    return Err(PowerError::Refused {
                        refusal: refusal_kind(raw.get("refusal").and_then(Value::as_str)),
                        reply,
                    });
                }
                Err(PowerError::Host(err))
            }
        }
    }
}

impl PcPowerHost for FlowBridge<'_, '_> {
    fn pc_power(&self) -> Result<PcPowerOffer, Error> {
        let ok = self.call_raw("pc_power", "")?;
        serde_json::from_value(ok).map_err(|e| Error::Malformed(e.to_string()))
    }

    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error> {
        let op = match action {
            PcPower::Restart => "pc_power.restart",
            PcPower::ShutDown => "pc_power.shutdown",
        };
        let _ = self.call_raw(op, token)?;
        Ok(())
    }
}

impl UpdateHost for FlowBridge<'_, '_> {
    fn host_update(&self) -> Result<HostUpdate, Error> {
        // Not used directly because `follow_update_cancellable` passes target separately; see `TargetUpdateHost`.
        unreachable!()
    }

    fn running_version(&self) -> Option<String> {
        None
    }
}

struct TargetUpdateHost<'a, 'local> {
    bridge: &'a FlowBridge<'a, 'local>,
    target: &'a str,
}

impl UpdateHost for TargetUpdateHost<'_, '_> {
    fn host_update(&self) -> Result<HostUpdate, Error> {
        let ok = self.bridge.call_raw("update", self.target)?;
        serde_json::from_value(ok).map_err(|e| Error::Malformed(e.to_string()))
    }

    fn running_version(&self) -> Option<String> {
        None
    }
}

fn sleep_result_json(bridge: &FlowBridge<'_, '_>, res: Result<Option<String>, SleepFailure>) -> Value {
    match res {
        Ok(Some(token)) => json!({"outcome": "token", "token": token}),
        Ok(None) => json!({"outcome": "accepted"}),
        Err(SleepFailure::Unavailable(reason)) => json!({
            "outcome": "unavailable",
            "reason": match reason {
                nightjar_policy::hosts::SleepUnavailableReason::Unsupported => 0,
                nightjar_policy::hosts::SleepUnavailableReason::NotAllowed => 1,
                nightjar_policy::hosts::SleepUnavailableReason::Busy => 2,
            }
        }),
        Err(SleepFailure::Refused { status, message }) => {
            json!({"outcome": "refused", "status": status, "message": message})
        }
        Err(SleepFailure::Transport(err)) => bridge.failed_json(&err),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowSleepOffer(
    env: JNIEnv,
    _: JClass,
    transport: JObject,
) -> jstring {
    let bridge = FlowBridge::new(env, &transport, None, None);
    let out = sleep_result_json(&bridge, sleep_token(&bridge).map(|(t, _)| Some(t)));
    let env = bridge.env.borrow();
    new_jstring(&env, &out.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowSleepSpend(
    mut env: JNIEnv,
    _: JClass,
    transport: JObject,
    token: JString,
    mono_start_ms: jlong,
    wall_start_ms: jlong,
    mono_now_ms: jlong,
    wall_now_ms: jlong,
) -> jstring {
    let tok = jstr(&mut env, &token).unwrap_or_default();
    let bridge = FlowBridge::new(env, &transport, None, None);
    let stamp = TokenStamp {
        mono_ms: mono_start_ms.max(0) as u64,
        wall_ms: wall_start_ms.max(0) as u64,
    };
    let out = sleep_result_json(
        &bridge,
        sleep_confirmed_at(
            &bridge,
            tok,
            stamp,
            mono_now_ms.max(0) as u64,
            wall_now_ms.max(0) as u64,
        )
        .map(|()| None),
    );
    let env = bridge.env.borrow();
    new_jstring(&env, &out.to_string())
}

fn pc_power_err_json(bridge: &FlowBridge<'_, '_>, err: PcPowerFailure) -> Value {
    match err {
        PcPowerFailure::Unavailable(reason) => {
            if bridge.last_error.borrow().is_some() {
                json!({"outcome": "refused", "message": reason})
            } else {
                json!({"outcome": "unavailable", "reason": reason})
            }
        }
        PcPowerFailure::Transport(e) => bridge.failed_json(&e),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowPcPowerOffer(
    env: JNIEnv,
    _: JClass,
    transport: JObject,
    is_restart: jboolean,
) -> jstring {
    let bridge = FlowBridge::new(env, &transport, None, None);
    let action = if is_restart != 0 {
        PcPower::Restart
    } else {
        PcPower::ShutDown
    };
    let out = match pc_power_token(&bridge, action) {
        Ok((token, _, wake_from_off)) => {
            json!({"outcome": "token", "token": token, "wake_from_off": wake_from_off})
        }
        Err(err) => pc_power_err_json(&bridge, err),
    };
    let env = bridge.env.borrow();
    new_jstring(&env, &out.to_string())
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowPcPowerSpend(
    mut env: JNIEnv,
    _: JClass,
    transport: JObject,
    is_restart: jboolean,
    token: JString,
    mono_start_ms: jlong,
    wall_start_ms: jlong,
    mono_now_ms: jlong,
    wall_now_ms: jlong,
    sleeper: JObject,
    listener: JObject,
) {
    let tok = jstr(&mut env, &token).unwrap_or_default();
    let bridge = FlowBridge::new(env, &transport, Some(&sleeper), Some(&listener));
    let action = if is_restart != 0 {
        PcPower::Restart
    } else {
        PcPower::ShutDown
    };
    let stamp = TokenStamp {
        mono_ms: mono_start_ms.max(0) as u64,
        wall_ms: wall_start_ms.max(0) as u64,
    };
    match pc_power_confirmed_at(
        &bridge,
        action,
        tok,
        stamp,
        mono_now_ms.max(0) as u64,
        wall_now_ms.max(0) as u64,
    ) {
        Ok(_) => {
            bridge.emit_step(&json!({"outcome": "accepted"}));
            if let Some(refusal) = pc_power_refusal(&bridge, PC_POWER_REFUSAL_READS as u32, || {
                bridge.sleep_ms(PC_POWER_REFUSAL_POLL_MS);
            }) {
                let step = match refusal {
                    nightjar_policy::http::LateRefusal::Refused(msg) => {
                        json!({"outcome": "refused_later", "refusal": msg, "unconfirmed": false})
                    }
                    nightjar_policy::http::LateRefusal::Unconfirmed(msg) => {
                        json!({"outcome": "refused_later", "refusal": msg, "unconfirmed": true})
                    }
                };
                bridge.emit_step(&step);
            }
        }
        Err(err) => {
            bridge.emit_step(&pc_power_err_json(&bridge, err));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostFlowUpdateRun(
    env: JNIEnv,
    _: JClass,
    transport: JObject,
    sleeper: JObject,
    listener: JObject,
) {
    let bridge = FlowBridge::new(env, &transport, Some(&sleeper), Some(&listener));
    let update_raw = match bridge.call_raw("update.install", "") {
        Ok(raw) => raw,
        Err(Error::Api { ref error, .. }) if !error.message.is_empty() => {
            bridge.emit_step(&json!({"outcome": "refused", "message": error.message}));
            return;
        }
        Err(err) => {
            bridge.emit_step(&bridge.failed_json(&err));
            return;
        }
    };
    let explicit_no_offer = update_raw.get("offered").is_some_and(Value::is_null);
    let update: HostUpdate = match serde_json::from_value(update_raw) {
        Ok(u) => u,
        Err(err) => {
            bridge.emit_step(&bridge.failed_json(&Error::Malformed(err.to_string())));
            return;
        }
    };
    let Ok(target) = (!explicit_no_offer).then(|| update_target(&update)).transpose().ok().flatten().ok_or(()) else {
        bridge.emit_step(&json!({"outcome": "nothing_to_install"}));
        return;
    };
    bridge.emit_step(&json!({"outcome": "started", "version": target}));

    let polls = (UPDATE_WAIT_MS / UPDATE_POLL_MS) as u32;
    let most = (UPDATE_MAX_WAIT_MS / UPDATE_POLL_MS) as u32;
    let host = TargetUpdateHost {
        bridge: &bridge,
        target: &target,
    };
    let Some(terminal) = follow_update_cancellable(
        &host,
        &target,
        polls,
        most,
        || bridge.sleep_ms(UPDATE_POLL_MS),
        || bridge.aborted.get(),
        |reason| bridge.emit_step(&json!({"outcome": "waiting", "reason": reason})),
    ) else {
        return;
    };
    let step = match terminal {
        UpdateStepAction::Back { version } => {
            json!({"outcome": "back", "version": version})
        }
        UpdateStepAction::UpdateFailed { error } => {
            json!({"outcome": "update_failed", "error": error})
        }
        UpdateStepAction::CameBackOn {
            current_version,
            error,
        } => {
            json!({"outcome": "came_back_on", "current_version": current_version, "error": error})
        }
        UpdateStepAction::TimedOut { target } => {
            json!({"outcome": "timed_out", "version": target})
        }
        UpdateStepAction::Continue | UpdateStepAction::Waiting { .. } => return,
    };
    bridge.emit_step(&step);
}
