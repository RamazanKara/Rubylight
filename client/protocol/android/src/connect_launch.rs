//! Launch, resume, quit, and 410-confirm-replace JNI orchestration for `NightjarSessionJni`,
//! bridging `nightjar_core::connect::launch` onto Android's `HostCallback`.

use std::cell::{Cell, RefCell};

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jstring};
use nightjar_core::connect::launch::{
    ConfirmChoice, ConfirmReplace, LaunchFlowError, LaunchFlowOptions, LaunchOutcome, LaunchTarget,
    LaunchTransport, execute_launch_or_resume, launch_confirming, quit_and_launch,
};
use nightjar_core::http::{self, Launched, StreamMove};
use nightjar_policy::http::{App, AppControl, AppRole, HostCapabilities, ServerInfo};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::hosts::jstr;
use crate::http::new_jstring;

use super::{build_server_info, jb, parse_envelope};

pub(crate) fn control_from_ordinal(ordinal: jint) -> AppControl {
    match ordinal {
        1 => AppControl::Resume,
        2 => AppControl::DisconnectMonitor,
        3 => AppControl::DisconnectInput,
        4 => AppControl::Terminate,
        5 => AppControl::Monitor,
        6 => AppControl::Input,
        7 => AppControl::RunningGame,
        _ => AppControl::None,
    }
}

pub(crate) fn app_role_from_ordinal(ordinal: jint) -> AppRole {
    match ordinal {
        1 => AppRole::RemoteMonitor,
        2 => AppRole::InputOnly,
        _ => AppRole::Stream,
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct JniTargetDto {
    app_id: i32,
    app_uuid: String,
    app_name: String,
    control: i32,
    role: i32,
    monitor_resume: bool,
    small_stream_input_only: bool,
    mic_only: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct JniOptsDto {
    join_running: bool,
    resume_only: bool,
    allow_idle_session_end: bool,
    never_replace: bool,
    hdr: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct JniServerInfoDto {
    paired: bool,
    current_game: i32,
    current_game_uuid: String,
    session_idle: bool,
    host_confirms_replacement: bool,
}

fn parse_target(text: &str) -> LaunchTarget {
    let dto: JniTargetDto = serde_json::from_str(text).unwrap_or_default();
    LaunchTarget {
        app_id: dto.app_id,
        app_uuid: dto.app_uuid,
        app_name: dto.app_name,
        control: control_from_ordinal(dto.control),
        role: app_role_from_ordinal(dto.role),
        monitor_resume: dto.monitor_resume,
        small_stream_input_only: dto.small_stream_input_only,
        mic_only: dto.mic_only,
    }
}

fn parse_opts(text: &str) -> LaunchFlowOptions {
    let dto: JniOptsDto = serde_json::from_str(text).unwrap_or_default();
    LaunchFlowOptions {
        join_running: dto.join_running,
        resume_only: dto.resume_only,
        allow_idle_session_end: dto.allow_idle_session_end,
        never_replace: dto.never_replace,
        hdr: dto.hdr,
    }
}

fn parse_server_info(text: &str) -> ServerInfo {
    let dto: JniServerInfoDto = serde_json::from_str(text).unwrap_or_default();
    build_server_info(
        dto.paired,
        dto.current_game,
        &dto.current_game_uuid,
        dto.session_idle,
        false,
        dto.host_confirms_replacement,
    )
}

struct AndroidLaunchBridge<'a, 'local> {
    env: RefCell<JNIEnv<'local>>,
    callback: &'a JObject<'local>,
    confirm_replace_used: Cell<bool>,
}

pub(crate) fn read_jstring_result(
    env: &mut JNIEnv,
    call: jni::errors::Result<jni::objects::JValueOwned>,
) -> jni::errors::Result<String> {
    let obj = match call {
        Ok(v) => match v.l() {
            Ok(obj) => obj,
            Err(e) => {
                let _ = env.exception_clear();
                return Err(e);
            }
        },
        Err(e) => {
            let _ = env.exception_clear();
            return Err(e);
        }
    };
    if obj.is_null() {
        return Ok(String::new());
    }
    let js = JString::from(obj);
    match env.get_string(&js) {
        Ok(s) => Ok(s.to_string_lossy().into_owned()),
        Err(e) => {
            let _ = env.exception_clear();
            Err(e)
        }
    }
}

impl<'a, 'local> AndroidLaunchBridge<'a, 'local> {
    fn new(env: JNIEnv<'local>, callback: &'a JObject<'local>) -> Self {
        Self {
            env: RefCell::new(env),
            callback,
            confirm_replace_used: Cell::new(false),
        }
    }

    fn read_jstring_result(
        env: &mut JNIEnv,
        call: jni::errors::Result<jni::objects::JValueOwned>,
    ) -> jni::errors::Result<String> {
        read_jstring_result(env, call)
    }
}

impl LaunchTransport for AndroidLaunchBridge<'_, '_> {
    fn server_info(&self, fast: bool) -> Result<ServerInfo, http::Error> {
        if self.callback.is_null() {
            return Err(http::Error::Cancelled);
        }
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(8, |env| {
            let call = env.call_method(
                self.callback,
                "serverInfo",
                "(Z)Ljava/lang/String;",
                &[JValue::Bool(jb(fast))],
            );
            Self::read_jstring_result(env, call)
        });
        let text =
            res.map_err(|_| http::Error::Io(std::io::Error::other("JNI serverInfo failed")))?;
        let v = parse_envelope(&text)?;
        Ok(parse_server_info(&v.to_string()))
    }

    fn app_list(&self, _caps: &HostCapabilities) -> Result<Vec<App>, http::Error> {
        Ok(Vec::new())
    }

    fn stream_move(&self) -> Result<StreamMove, http::Error> {
        Ok(StreamMove::default())
    }

    fn has_ended_session(&self) -> bool {
        if self.callback.is_null() {
            return false;
        }
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(4, |env| {
            match env.call_method(self.callback, "hasEndedSession", "()Z", &[]) {
                Ok(v) => Ok(v.z().unwrap_or(false)),
                Err(e) => {
                    let _ = env.exception_clear();
                    Err(e)
                }
            }
        });
        res.unwrap_or(false)
    }

    fn launch(
        &self,
        verb: &str,
        app_id: i32,
        hdr: bool,
        confirm_replace: bool,
    ) -> Result<Option<Launched>, http::Error> {
        if self.callback.is_null() {
            return Err(http::Error::Cancelled);
        }
        if confirm_replace {
            self.confirm_replace_used.set(true);
        }
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(8, |env| {
            let j_verb = match env.new_string(verb) {
                Ok(s) => s,
                Err(e) => {
                    let _ = env.exception_clear();
                    return Err(e);
                }
            };
            let call = env.call_method(
                self.callback,
                "launch",
                "(Ljava/lang/String;IZZ)Ljava/lang/String;",
                &[
                    JValue::Object(&j_verb.into()),
                    JValue::Int(app_id),
                    JValue::Bool(jb(hdr)),
                    JValue::Bool(jb(confirm_replace)),
                ],
            );
            Self::read_jstring_result(env, call)
        });
        let text = res.map_err(|_| http::Error::Io(std::io::Error::other("JNI launch failed")))?;
        let v = parse_envelope(&text)?;
        let started = v.get("started").and_then(Value::as_bool).unwrap_or(false);
        if started {
            Ok(Some(Launched {
                rtsp_session_url: v
                    .get("rtsp_session_url")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                // Android reads and shows the launch warning in Java (NvHTTP).
                warning: None,
            }))
        } else {
            Ok(None)
        }
    }

    fn quit_app(&self) -> Result<bool, http::Error> {
        if self.callback.is_null() {
            return Err(http::Error::Cancelled);
        }
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(8, |env| {
            let call = env.call_method(self.callback, "quitApp", "()Ljava/lang/String;", &[]);
            Self::read_jstring_result(env, call)
        });
        let text = res.map_err(|_| http::Error::Io(std::io::Error::other("JNI quitApp failed")))?;
        let v = parse_envelope(&text)?;
        Ok(v.as_bool().unwrap_or(false))
    }

    fn is_stopped(&self) -> bool {
        if self.callback.is_null() {
            return false;
        }
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(4, |env| {
            match env.call_method(self.callback, "isStopped", "()Z", &[]) {
                Ok(v) => Ok(v.z().unwrap_or(false)),
                Err(e) => {
                    let _ = env.exception_clear();
                    Err(e)
                }
            }
        });
        res.unwrap_or(false)
    }
}

impl ConfirmReplace for AndroidLaunchBridge<'_, '_> {
    fn confirm_replace(&self, terminate_only: bool) -> ConfirmChoice {
        if self.callback.is_null() {
            return ConfirmChoice::Rejected;
        }
        let mut env = self.env.borrow_mut();
        let res = env.with_local_frame(4, |env| {
            let call = env.call_method(
                self.callback,
                "confirmReplace",
                "(Z)I",
                &[JValue::Bool(jb(terminate_only))],
            );
            match call {
                Ok(v) => Ok(v.i().unwrap_or(1)),
                Err(e) => {
                    let _ = env.exception_clear();
                    Err(e)
                }
            }
        });
        match res.unwrap_or(1) {
            0 => ConfirmChoice::Confirmed,
            2 => ConfirmChoice::TimedOut,
            3 => ConfirmChoice::Cancelled,
            4 => ConfirmChoice::Interrupted,
            _ => ConfirmChoice::Rejected,
        }
    }
}

fn outcome_to_json(
    result: Result<LaunchOutcome, LaunchFlowError>,
    confirm_replace: bool,
) -> String {
    let body = match result {
        Ok(LaunchOutcome::Started {
            launched,
            used_verb,
        }) => json!({
            "kind": "started",
            "verb": used_verb,
            "rtsp_session_url": launched.rtsp_session_url,
            "confirm_replace": confirm_replace,
        }),
        Ok(LaunchOutcome::ActionCompleted(message)) => json!({
            "kind": "action_completed",
            "message": message,
            "confirm_replace": confirm_replace,
        }),
        Ok(LaunchOutcome::StoppedAutomaticStart) => json!({
            "kind": "stopped_automatic_start",
            "confirm_replace": confirm_replace,
        }),
        Err(LaunchFlowError::SessionEnded) => json!({"kind": "session_ended"}),
        Err(LaunchFlowError::MovedTo(device)) => json!({"kind": "moved_to", "message": device}),
        Err(LaunchFlowError::Local(local)) => {
            let local_kind = format!("{local:?}");
            json!({
                "kind": "local_failure",
                "code": local.code(),
                "message": local.message(),
                "local_kind": local_kind,
                "confirm_replace": confirm_replace,
            })
        }
        Err(LaunchFlowError::HostStatus { code, message }) => json!({
            "kind": "host_status",
            "code": code,
            "message": message,
            "confirm_replace": confirm_replace,
        }),
        Err(LaunchFlowError::Http(err)) => json!({
            "kind": "http_error",
            "message": err.to_string(),
            "confirm_replace": confirm_replace,
        }),
        Err(LaunchFlowError::Declined { verb }) => json!({
            "kind": "declined",
            "verb": verb,
            "confirm_replace": confirm_replace,
        }),
        Err(LaunchFlowError::QuitFailed) => json!({"kind": "quit_failed"}),
        Err(LaunchFlowError::SessionOwnerDenied) => json!({"kind": "session_owner_denied"}),
        Err(LaunchFlowError::AppMinimized) => json!({"kind": "app_minimized"}),
        Err(LaunchFlowError::InputOnlyOtherGame) => json!({"kind": "input_only_other_game"}),
    };
    body.to_string()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_launchAppConfirming(
    mut env: JNIEnv,
    _: JClass,
    target_json: JString,
    opts_json: JString,
    verb: JString,
    host_callback: JObject,
) -> jstring {
    let target = parse_target(&jstr(&mut env, &target_json).unwrap_or_default());
    let opts = parse_opts(&jstr(&mut env, &opts_json).unwrap_or_default());
    let verb_str = jstr(&mut env, &verb).unwrap_or_else(|| "launch".to_owned());
    let bridge = AndroidLaunchBridge::new(env, &host_callback);
    let res = launch_confirming(&bridge, &target, opts, &verb_str, &bridge);
    let reply = outcome_to_json(res, bridge.confirm_replace_used.get());
    let env = bridge.env.into_inner();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_quitAndLaunch(
    mut env: JNIEnv,
    _: JClass,
    target_json: JString,
    opts_json: JString,
    host_callback: JObject,
) -> jstring {
    let target = parse_target(&jstr(&mut env, &target_json).unwrap_or_default());
    let opts = parse_opts(&jstr(&mut env, &opts_json).unwrap_or_default());
    let bridge = AndroidLaunchBridge::new(env, &host_callback);
    let res = quit_and_launch(&bridge, &target, opts, &bridge);
    let reply = outcome_to_json(res, bridge.confirm_replace_used.get());
    let env = bridge.env.into_inner();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_launchOrResume(
    mut env: JNIEnv,
    _: JClass,
    server_info_json: JString,
    target_json: JString,
    opts_json: JString,
    host_callback: JObject,
) -> jstring {
    let info = parse_server_info(&jstr(&mut env, &server_info_json).unwrap_or_default());
    let target = parse_target(&jstr(&mut env, &target_json).unwrap_or_default());
    let opts = parse_opts(&jstr(&mut env, &opts_json).unwrap_or_default());
    let bridge = AndroidLaunchBridge::new(env, &host_callback);
    let res = execute_launch_or_resume(&bridge, &info, &target, opts, &bridge);
    let reply = outcome_to_json(res, bridge.confirm_replace_used.get());
    let env = bridge.env.into_inner();
    new_jstring(&env, &reply)
}
