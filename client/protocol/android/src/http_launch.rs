//! JNI bridge for `com.limelight.nvstream.jni.NightjarHttpJni` launch, query building,
//! role disconnect, quit, and launch confirmation classification.

use std::time::Duration;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, jboolean, jbyteArray, jint, jlong, jstring};
use nightjar_core::http::{
    AppRole, DisplayLuminance, Error, HostDisplay, LaunchRequest, LaunchUnanswered,
    asks_to_confirm_replace, asks_to_confirm_terminate, is_role_completed, is_terminate_completed,
    random_ri_key,
};
use nightjar_core::sdp::AudioConfiguration;
use rubylight_protocol::fec_report::FecChoice;
use serde_json::{Value, json};

use crate::http::{CLIENTS, caps_from_arg, error_json, invalid_client, new_jstring, ok_json};
use crate::native_api;

fn decode_hex16(s: &str) -> Option<[u8; 16]> {
    let bytes = s.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = (pair[0] as char).to_digit(16)? as u8;
        let lo = (pair[1] as char).to_digit(16)? as u8;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn parse_launch_request(v: &Value) -> Result<LaunchRequest, Error> {
    let ri_key = decode_hex16(v["ri_key_hex"].as_str().unwrap_or_default())
        .ok_or_else(|| Error::InvalidArgument("bad 16-byte hex for ri_key_hex".into()))?;
    let role = match v["role"].as_str().unwrap_or("STREAM") {
        "REMOTE_MONITOR" => AppRole::RemoteMonitor,
        "INPUT_ONLY" => AppRole::InputOnly,
        _ => AppRole::Stream,
    };
    let host_display = match v["host_display"].as_str().unwrap_or("HOST_DEFAULT") {
        "EXCLUSIVE" => HostDisplay::Exclusive,
        "EXTENDED_PRIMARY" => HostDisplay::ExtendedPrimary,
        "EXTENDED" => HostDisplay::Extended,
        "EXTENDED_ISOLATED" => HostDisplay::ExtendedIsolated,
        "EXTENDED_PRIMARY_ISOLATED" => HostDisplay::ExtendedPrimaryIsolated,
        "PHYSICAL" => HostDisplay::Physical,
        _ => HostDisplay::HostDefault,
    };
    let audio = match v["audio_surround"].as_str().unwrap_or("STEREO") {
        "SURROUND_51" => AudioConfiguration::SURROUND_51,
        "SURROUND_71" => AudioConfiguration::SURROUND_71,
        "SURROUND_714" => AudioConfiguration::SURROUND_714,
        _ => AudioConfiguration::STEREO,
    };
    let fec = v["fec"].as_str().and_then(FecChoice::parse);
    let display_luminance = v["display_luminance"]
        .as_object()
        .map(|d| DisplayLuminance {
            max_nits: d["max_nits"].as_f64().unwrap_or(0.0) as f32,
            min_nits: d["min_nits"].as_f64().unwrap_or(0.0) as f32,
            max_full_frame_nits: d["max_full_frame_nits"].as_f64().unwrap_or(0.0) as f32,
            estimated_from_ratio: d["estimated_from_ratio"].as_bool().unwrap_or(false),
        });
    Ok(LaunchRequest {
        app_id: v["app_id"].as_i64().unwrap_or(0) as i32,
        app_uuid: v["app_uuid"].as_str().unwrap_or_default().to_owned(),
        role,
        small_stream_input_only: v["small_stream_input_only"].as_bool().unwrap_or(false),
        mic_only: v["mic_only"].as_bool().unwrap_or(false),
        width: v["width"].as_u64().unwrap_or(1920) as u32,
        height: v["height"].as_u64().unwrap_or(1080) as u32,
        fps: v["fps"].as_u64().unwrap_or(60) as u32,
        launch_refresh_rate_x100: v["launch_refresh_rate_x100"].as_u64().unwrap_or(0) as u32,
        client_refresh_rate_x100: v["client_refresh_rate_x100"].as_u64().unwrap_or(0) as u32,
        virtual_display: v["virtual_display"].as_bool().unwrap_or(false),
        virtual_display_scale: v["virtual_display_scale"].as_u64().unwrap_or(100) as u32,
        virtual_display_dpi: v["virtual_display_dpi"].as_u64().unwrap_or(0) as u32,
        host_display,
        vrr: v["vrr"].as_bool().unwrap_or(false),
        sops: v["sops"].as_bool().unwrap_or(true),
        play_local_audio: v["play_local_audio"].as_bool().unwrap_or(false),
        hdr: v["hdr"].as_bool().unwrap_or(false),
        ten_bit_sdr: v["ten_bit_sdr"].as_bool().unwrap_or(false),
        audio,
        ri_key,
        ri_key_id: v["ri_key_id"].as_i64().unwrap_or(0) as i32,
        prepared_controllers: v["prepared_controllers"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        gamepad_mask: v["gamepad_mask"].as_u64().unwrap_or(0) as u32,
        playstation_mask: v["playstation_mask"].as_u64().unwrap_or(0) as u32,
        persist_gamepads: v["persist_gamepads"].as_bool().unwrap_or(false),
        fec,
        rate_change: v["rate_change"].as_bool().unwrap_or(false),
        confirm_replace: v["confirm_replace"].as_bool().unwrap_or(false),
        clipboard_sync: v["clipboard_sync"].as_bool().unwrap_or(false),
        display_luminance,
        rtx_hdr: v["rtx_hdr"].as_bool(),
        rtx_video: v["rtx_video"].as_bool(),
        hdr_static_only: v["hdr_static_only"].as_bool().unwrap_or(false),
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_randomRiKey(
    env: JNIEnv,
    _: JClass,
) -> jbyteArray {
    let key = random_ri_key();
    match env.byte_array_from_slice(&key) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_buildLaunchQuery(
    mut env: JNIEnv,
    _: JClass,
    request_json: JString,
    caps_json: JString,
    nvidia: jboolean,
) -> jstring {
    let raw = native_api::string(&mut env, &request_json).unwrap_or_default();
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match serde_json::from_str::<Value>(&raw)
        .map_err(|e| Error::InvalidArgument(e.to_string()))
        .and_then(|v| parse_launch_request(&v))
    {
        Ok(req) => ok_json(req.query(&caps, nvidia != JNI_FALSE)),
        Err(e) => error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_launchApp(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    verb: JString,
    request_json: JString,
    caps_json: JString,
    nvidia: jboolean,
    read_timeout_ms: jint,
) -> jstring {
    let reply = (|| {
        let Some(client) = CLIENTS.get(handle) else {
            return invalid_client();
        };
        let verb = native_api::string(&mut env, &verb).unwrap_or_else(|| "launch".into());
        let raw_req = native_api::string(&mut env, &request_json).unwrap_or_default();
        let caps = caps_from_arg(&mut env, &caps_json);
        let req = match serde_json::from_str::<Value>(&raw_req)
            .map_err(|e| Error::InvalidArgument(e.to_string()))
            .and_then(|v| parse_launch_request(&v))
        {
            Ok(r) => r,
            Err(e) => return error_json(&e),
        };
        let timeout =
            Duration::from_millis(u64::try_from(read_timeout_ms.max(1)).unwrap_or(120_000));
        match client.launch_with_capabilities(&verb, &req, &caps, nvidia != JNI_FALSE, timeout) {
            Ok(Some(launched)) => ok_json(json!({
                "ok": true,
                "session_url": launched.rtsp_session_url,
                // Already checked and cleaned by the shared parser (launch_warning.rs).
                "warning_code": launched.warning.as_ref().map(|w| w.code.as_str()),
                "warning_message": launched.warning.as_ref().and_then(|w| w.message.as_deref()),
            })),
            Ok(None) => ok_json(json!({
                "ok": false,
            })),
            Err(Error::Status { code: 410, message }) => ok_json(json!({
                "ok": false,
                "confirming": true,
                "status": 410,
                "body": message,
            })),
            Err(Error::Io(ref io_err))
                if io_err
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<LaunchUnanswered>())
                    .is_some() =>
            {
                let seconds = io_err
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<LaunchUnanswered>())
                    .map(|u| u.seconds)
                    .unwrap_or(1);
                json!({
                    "error": {
                        "kind": "launch_timeout",
                        "seconds": seconds,
                        "message": io_err.to_string(),
                    }
                })
                .to_string()
            }
            Err(e) => error_json(&e),
        }
    })();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_disconnectRole(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    role_ordinal: jint,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let role = match role_ordinal {
                1 => AppRole::RemoteMonitor,
                2 => AppRole::InputOnly,
                _ => AppRole::Stream,
            };
            match client.disconnect_role(role) {
                Ok(v) => ok_json(v),
                Err(e) => error_json(&e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_quitApp(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.cancel() {
            Ok(v) => ok_json(v),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_launchConfirmationClassify(
    mut env: JNIEnv,
    _: JClass,
    status: jint,
    body: JString,
) -> jstring {
    let message = native_api::string(&mut env, &body).unwrap_or_default();
    let err = Error::Status {
        code: status,
        message,
    };
    let reply = ok_json(json!({
        "replace_required": asks_to_confirm_replace(&err),
        "terminate_required": asks_to_confirm_terminate(&err),
        "remote_monitor_completed": is_role_completed(AppRole::RemoteMonitor, &err),
        "input_only_completed": is_role_completed(AppRole::InputOnly, &err),
        "terminate_completed": is_terminate_completed(&err),
    }));
    new_jstring(&env, &reply)
}
