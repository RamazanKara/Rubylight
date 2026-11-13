//! libnightjar_core.so: the JNI side of `com.limelight.nvstream.jni.RustBridge`, which runs
//! a stream on nightjar-core instead of moonlight-common-c. `MoonBridge` routes a stream's
//! calls here when the "New streaming engine" setting is on and the stream needs nothing the
//! Rust engine lacks; see internal/RUST_CORE.md for what is covered.
//!
//! Each entry point mirrors the C native of the same name in `app/src/main/jni/moonlight-core`
//! (`simplejni.c`, `callbacks.c`, `android_control.c`, `android_microphone.c`,
//! `android_present_clock.c`), which build on moonlight-common-c (GPL-3.0, Cameron Gutman and
//! contributors).

mod bridge;
pub mod collections;
pub mod echo;
mod engine;
pub mod extras;
pub mod files;
pub mod internet;
mod frame_warp;
mod listener;
pub mod native_api;
mod opus;
mod platform;
mod renderers;
mod touchpad;
pub mod open_play;
pub mod sharp_color;
pub mod stream_menu;
mod usb;

use std::time::Duration;

use jni::objects::{JByteArray, JClass, JFloatArray, JIntArray, JLongArray, JShortArray, JString};
use jni::sys::{jboolean, jbyte, jfloat, jint, jlong, jshort, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

use nightjar_core::control::ClientMessage;
use nightjar_core::input::InputQueue;
use nightjar_core::session::{HostInfo, NetworkChange, SessionConfig};
use rubylight_protocol::control::{HostTimingRequest, LinkNoticeRequest, Reconfigure};

use crate::extras::input_result;

fn jbool(value: bool) -> jboolean {
    if value {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

/// Runs an input call on the session: 0 or the C API's error code (-2 without a stream).
fn input(f: impl FnOnce(&mut InputQueue) -> Result<(), nightjar_core::input::InputError>) -> jint {
    engine::with_session(-2, |s| input_result(s.input(f)))
}

fn key16(env: &mut JNIEnv, array: &JByteArray) -> [u8; 16] {
    let mut out = [0u8; 16];
    if let Ok(bytes) = env.convert_byte_array(array) {
        let n = bytes.len().min(16);
        out[..n].copy_from_slice(&bytes[..n]);
    }
    out
}

fn string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_init(
    mut env: JNIEnv,
    _: JClass,
    app_version: JString,
) -> jboolean {
    // Requests to the host send `User-Agent: Nightjar/<versionName>`.
    if let Some(version) = string(&mut env, &app_version) {
        nightjar_core::http::set_app_version(&version);
    }
    match bridge::init(&mut env) {
        Ok(()) => JNI_TRUE,
        Err(e) => {
            let _ = env.exception_clear();
            platform::error(&format!("RustBridge.init failed: {e}"));
            JNI_FALSE
        }
    }
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_startConnection(
    mut env: JNIEnv,
    _: JClass,
    address: JString,
    app_version: JString,
    rtsp_session_url: JString,
    server_codec_mode_support: jint,
    width: jint,
    height: jint,
    fps: jint,
    bitrate: jint,
    packet_size: jint,
    streaming_remotely: jint,
    audio_configuration: jint,
    supported_video_formats: jint,
    client_refresh_rate_x100: jint,
    ri_aes_key: JByteArray,
    ri_aes_iv: JByteArray,
    video_capabilities: jint,
    color_space: jint,
    color_range: jint,
    host_features: jint,
    input_only: jboolean,
    enable_mic: jboolean,
    haptics_pcm: jboolean,
    pyrowave_link_mbps: jint,
    pyrowave_pace_mbps: jint,
    client_wireless: jboolean,
    network_grace_ms: jint,
    packed444: jboolean,
    packed444_modes: jint,
    remote_monitor: jboolean,
    unbatched_input: jboolean,
    network_priority: jboolean,
    roaming: jboolean,
    roaming_metered: jboolean,
    audio_passthrough: jint,
    hdr_format: jint,
    video_fec_percentage: jint,
    pyrowave_early: jboolean,
    codec_direct: jboolean,
    receive_decode_priority: jboolean,
    frame_warp_requested: jboolean,
    receive_coalescing: jboolean,
) -> jint {
    let Some(bridge) = bridge::get() else {
        platform::error("RustBridge.startConnection before init");
        return -1;
    };
    let input_only = input_only == JNI_TRUE;
    let host = HostInfo {
        address: string(&mut env, &address).unwrap_or_default(),
        app_version: string(&mut env, &app_version).unwrap_or_default(),
        rtsp_session_url: string(&mut env, &rtsp_session_url),
        server_codec_mode_support: if input_only {
            1
        } else {
            server_codec_mode_support as u32
        },
    };
    let mut stream = engine::stream_config(
        width,
        height,
        fps,
        bitrate,
        packet_size,
        audio_configuration,
        supported_video_formats,
        client_refresh_rate_x100,
        key16(&mut env, &ri_aes_key),
        key16(&mut env, &ri_aes_iv),
        video_capabilities,
        color_space,
        color_range,
    );
    stream.microphone = enable_mic == JNI_TRUE;
    stream.experimental_packed_444 = packed444 == JNI_TRUE;
    stream.packed444_modes = packed444_modes as u8 & rubylight_protocol::packed444::ALL_MODES;
    stream.haptics_pcm = haptics_pcm == JNI_TRUE;
    stream.pyrowave_records_supported = host_features & 1 != 0;
    stream.pyrowave_link_mbps = pyrowave_link_mbps.max(0);
    stream.pyrowave_pace_mbps = pyrowave_pace_mbps.max(0);
    stream.client_wireless = client_wireless == JNI_TRUE;
    stream.audio_passthrough = extras::audio_passthrough(audio_passthrough);
    stream.hdr_format = hdr_format as u8 & rubylight_protocol::hdr::ALL;
    stream.video_fec = extras::video_fec(video_fec_percentage);
    let config = SessionConfig {
        stream,
        location: extras::location(streaming_remotely),
        network_grace: Duration::from_millis(network_grace_ms.max(0) as u64),
        input_only,
        remote_monitor: remote_monitor == JNI_TRUE,
        unbatched_input: unbatched_input == JNI_TRUE,
        network_priority: network_priority == JNI_TRUE,
        receive_decode_priority: receive_decode_priority == JNI_TRUE,
        receive_coalescing: receive_coalescing == JNI_TRUE,
        roaming: extras::roaming_config(roaming == JNI_TRUE, roaming_metered == JNI_TRUE),
        // Shifts pictures, so not for a stream without them or one that sends no input.
        frame_warp: (frame_warp_requested == JNI_TRUE && !input_only && remote_monitor != JNI_TRUE)
            .then(|| {
                nightjar_core::frame_warp::FrameWarp::new(
                    Some(frame_warp::CONTROLLER),
                    frame_warp::MASK_MAX_WIDTH,
                )
            }),
    };
    engine::start(
        bridge,
        engine::StartParams {
            host,
            config,
            early_records: pyrowave_early == JNI_TRUE,
            codec_direct: codec_direct == JNI_TRUE,
            // HostCapabilities.NATIVE_CONTROLLER_IDENTITY.
            controller_identity: host_features & 2 != 0,
        },
    )
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_stopConnection(
    _: JNIEnv,
    _: JClass,
) {
    engine::stop();
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_interruptConnection(
    _: JNIEnv,
    _: JClass,
) {
    engine::interrupt();
}

/// Whether the Rust engine's connection ended with this session's host's disconnect, not a
/// silence.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_hostSaidGoodbye(
    _: JNIEnv,
    _: JClass,
) -> jboolean {
    if engine::with_session(false, |s| s.host_said_goodbye()) { JNI_TRUE } else { JNI_FALSE }
}

/// An address from Java; null or one that does not parse is none.
fn java_address(env: &mut JNIEnv, text: &JString) -> Option<std::net::IpAddr> {
    if text.is_null() {
        return None;
    }
    env.get_string(text).ok().and_then(|text| String::from(text).parse().ok())
}

/// One round of [`nightjar_core::host_loss`] for `HostLossWatch`, which runs it over both engines:
/// blocks for about 750 ms. Returns [`nightjar_core::host_loss::Round::code`]. `gateway` may be
/// null; a host that does not parse proves nothing.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_hostLossRound(
    mut env: JNIEnv,
    _: JClass,
    host: JString,
    port: jint,
    gateway: JString,
) -> jint {
    use nightjar_core::host_loss::{probe_round, Round};
    let Some(host) = java_address(&mut env, &host) else {
        return Round::SilentLinkUnknown.code();
    };
    let gateway = java_address(&mut env, &gateway);
    probe_round(std::net::SocketAddr::new(host, port as u16), gateway).code()
}

/// The address the running session connected to, as text (`HostLossWatch` asks it, never DNS);
/// null without a session.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getHostAddress(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    match engine::with_session(None, |s| Some(s.remote_address().ip().to_string())).map(|text| env.new_string(text)) {
        Some(Ok(text)) => text.into_raw(),
        _ => std::ptr::null_mut(),
    }
}

/// [`nightjar_core::host_loss::after_goodbye`] for `HostLossWatch`: one round's verdict after
/// the host's goodbye, as [`nightjar_core::host_loss::Goodbye::code`].
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_hostLossAfterGoodbye(
    _: JNIEnv,
    _: JClass,
    on_link: jboolean,
    round: jint,
    waited_ms: jlong,
) -> jint {
    host_loss_after_goodbye(on_link == JNI_TRUE, round, waited_ms)
}

fn host_loss_after_goodbye(on_link: bool, round: jint, waited_ms: jlong) -> jint {
    use nightjar_core::host_loss::{after_goodbye, Round};
    let waited = std::time::Duration::from_millis(waited_ms.max(0) as u64);
    after_goodbye(on_link, Round::from_code(round), waited).code()
}

/// The default network changed while a stream runs (`NvConnection`'s network callback):
/// `MoonBridge.NETWORK_CARRIES_ON` (0), `NETWORK_ROAMED` (1) or `NETWORK_CANNOT_CONTINUE` (2).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_networkChanged(
    _: JNIEnv,
    _: JClass,
    metered: jboolean,
) -> jint {
    network_change_code(engine::network_changed(metered == JNI_TRUE))
}

fn network_change_code(change: NetworkChange) -> jint {
    match change {
        NetworkChange::CarriesOn => 0,
        NetworkChange::Roamed => 1,
        NetworkChange::CannotContinue => 2,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMouseMove(
    _: JNIEnv,
    _: JClass,
    dx: jshort,
    dy: jshort,
) {
    input(|q| q.send_mouse_move(dx, dy));
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMousePosition(
    _: JNIEnv,
    _: JClass,
    x: jshort,
    y: jshort,
    width: jshort,
    height: jshort,
) {
    input(|q| q.send_mouse_position(x, y, width, height));
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMouseMoveAsMousePosition(
    _: JNIEnv,
    _: JClass,
    dx: jshort,
    dy: jshort,
    width: jshort,
    height: jshort,
) {
    input(|q| q.send_mouse_move_as_position(dx, dy, width, height));
}

/// `BUTTON_ACTION_PRESS` (0x07) or `BUTTON_ACTION_RELEASE` (0x08).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMouseButton(
    _: JNIEnv,
    _: JClass,
    action: jbyte,
    button: jbyte,
) {
    input(|q| q.send_mouse_button(button as u8, action == 0x07));
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMultiControllerInput(
    _: JNIEnv,
    _: JClass,
    controller: jshort,
    active_mask: jshort,
    buttons: jint,
    left_trigger: jbyte,
    right_trigger: jbyte,
    left_x: jshort,
    left_y: jshort,
    right_x: jshort,
    right_y: jshort,
) {
    input(|q| {
        q.send_controller(
            controller as u16,
            active_mask as u16,
            buttons as u32,
            left_trigger as u8,
            right_trigger as u8,
            (left_x, left_y),
            (right_x, right_y),
        )
    });
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendTouchEvent(
    _: JNIEnv,
    _: JClass,
    event: jbyte,
    pointer_id: jint,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
    contact_major: jfloat,
    contact_minor: jfloat,
    rotation: jshort,
) -> jint {
    input(|q| {
        q.send_touch(
            event as u8,
            pointer_id as u32,
            x,
            y,
            pressure,
            contact_major,
            contact_minor,
            rotation as u16,
        )
    })
}

/// How the host takes a physical touchpad's fingers: `RustBridge.TOUCHPAD_NONE`, `_INDIVIDUAL`
/// or `_FRAME`; none without a session.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_touchpadTransport(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::with_session(touchpad::TRANSPORT_NONE, |s| {
        s.input(|q| touchpad::transport_code(q.feature_flags()))
    })
}

/// One motion event of a physical touchpad: the first `count` fingers still on it (ids, and
/// positions on the touchpad's surface, 0-1), its click, and its size in millimetres (0 when
/// unknown). Returns 0, or the input error code.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_touchpadFrame(
    env: JNIEnv,
    _: JClass,
    device: jint,
    ids: JIntArray,
    xs: JFloatArray,
    ys: JFloatArray,
    pressures: JFloatArray,
    count: jint,
    button: jboolean,
    width_mm: jshort,
    height_mm: jshort,
) -> jint {
    let Some(session) = engine::session_id() else { return -2 };
    let count = count.clamp(0, 16) as usize;
    let mut id = vec![0i32; count];
    let mut x = vec![0f32; count];
    let mut y = vec![0f32; count];
    let mut pressure = vec![0f32; count];
    if count > 0
        && (env.get_int_array_region(&ids, 0, &mut id).is_err()
            || env.get_float_array_region(&xs, 0, &mut x).is_err()
            || env.get_float_array_region(&ys, 0, &mut y).is_err()
            || env.get_float_array_region(&pressures, 0, &mut pressure).is_err())
    {
        return -3;
    }
    let contacts = touchpad::contacts(&id, &x, &y, &pressure);
    let outs = touchpad::frame(
        session,
        device,
        contacts,
        button != JNI_FALSE,
        width_mm.max(0) as u16,
        height_mm.max(0) as u16,
    );
    input(|q| touchpad::send(outs, q))
}

/// Lifts the touchpad's fingers on the host (capture or focus lost).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_touchpadCancel(
    _: JNIEnv,
    _: JClass,
) -> jint {
    let Some(session) = engine::session_id() else { return -2 };
    let outs = touchpad::track(session, |t| t.cancel());
    input(|q| touchpad::send(outs, q))
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendPenEvent(
    _: JNIEnv,
    _: JClass,
    event: jbyte,
    tool: jbyte,
    buttons: jbyte,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
    contact_major: jfloat,
    contact_minor: jfloat,
    rotation: jshort,
    tilt: jbyte,
) -> jint {
    input(|q| {
        q.send_pen(
            event as u8,
            tool as u8,
            buttons as u8,
            x,
            y,
            pressure,
            contact_major,
            contact_minor,
            rotation as u16,
            tilt as u8,
            // Android's MotionEvent has no barrel roll axis: pens go as the stock packet.
            rubylight_protocol::pen::ROLL_UNKNOWN,
        )
    })
}

/// Which controller `controller` is, by its USB vendor and product: send it before the arrival.
/// The queue drops it for a host without `ControllerIdentityVersion`.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendControllerIdentityEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    vendor: jint,
    product: jint,
) -> jint {
    input(|q| q.send_controller_identity(controller as u8, vendor as u16, product as u16))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendControllerArrivalEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    active_mask: jshort,
    kind: jbyte,
    supported_buttons: jint,
    capabilities: jshort,
) -> jint {
    input(|q| {
        q.send_controller_arrival(
            controller as u8,
            active_mask as u16,
            kind as u8,
            supported_buttons as u32,
            capabilities as u16,
        )
    })
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendControllerTouchEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    event: jbyte,
    pointer_id: jint,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
) -> jint {
    // LiSendControllerTouchEvent is touchpad 0 of LiSendControllerTouchEvent2.
    input(|q| {
        q.send_controller_touch(
            controller as u8,
            event as u8,
            0,
            pointer_id as u32,
            x,
            y,
            pressure,
        )
    })
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendControllerMotionEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    kind: jbyte,
    x: jfloat,
    y: jfloat,
    z: jfloat,
) -> jint {
    input(|q| q.send_controller_motion(controller as u8, kind as u8, x, y, z))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendControllerBatteryEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    state: jbyte,
    percentage: jbyte,
) -> jint {
    input(|q| q.send_controller_battery(controller as u8, state as u8, percentage as u8))
}

/// `KEY_ACTION_DOWN` (0x03) or `KEY_ACTION_UP` (0x04).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendKeyboardInput(
    _: JNIEnv,
    _: JClass,
    key: jshort,
    action: jbyte,
    modifiers: jbyte,
    flags: jbyte,
) {
    input(|q| q.send_keyboard(key as u16, action == 0x03, modifiers as u8, flags as u8));
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMouseHighResScroll(
    _: JNIEnv,
    _: JClass,
    amount: jshort,
) {
    input(|q| q.send_high_res_scroll(amount));
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMouseHighResHScroll(
    _: JNIEnv,
    _: JClass,
    amount: jshort,
) {
    input(|q| q.send_high_res_hscroll(amount));
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendUtf8Text(
    env: JNIEnv,
    _: JClass,
    text: JByteArray,
) {
    let Ok(bytes) = env.convert_byte_array(&text) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    input(|q| q.send_text(&text));
}

/// Apollo's server command (0x3000): one or four bytes, Sunshine hosts only.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendServerCommand(
    env: JNIEnv,
    _: JClass,
    payload: JByteArray,
) -> jboolean {
    let Ok(bytes) = env.convert_byte_array(&payload) else {
        return JNI_FALSE;
    };
    jbool(engine::send(ClientMessage::ServerCommand(bytes)))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getPendingAudioDuration(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::with_session(0, |s| s.pending_audio_ms() as jint)
}

/// The Opus concealer for the AAudio callback (`aaudio_output.c`), as a C function pointer.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_audioConcealer(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    let conceal: unsafe extern "C" fn(*mut i16, i32) -> i32 = renderers::conceal;
    conceal as usize as jlong
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getPendingVideoFrames(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::with_session(0, |s| s.decode_queue_len() as i32)
}

/// Frames dropped over a full decode queue since the stream started (`VideoStats::decoder_drops`);
/// 0 without a session.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getDecoderDrops(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::decoder_drops().min(i64::MAX as u64) as jlong
}

/// Bytes of video and audio datagrams received, FEC parity included; 0 without a session.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getWireBytesReceived(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(0, |s| s.wire_bytes() as jlong)
}

/// The video socket's receive buffer, asked over granted (`extras::receive_buffer_info`); -1 without a session.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getVideoReceiveBufferInfo(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(-1, |s| extras::receive_buffer_info(s.video_receive_buffer()))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getEstimatedRttInfo(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(-1, |s| extras::rtt_info(s.rtt()))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_reportFrameSlack(
    _: JNIEnv,
    _: JClass,
    slack_ns: jlong,
    period_ns: jlong,
) -> jboolean {
    jbool(engine::report_frame_slack(slack_ns, period_ns))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_flushPhaseLockReport(
    _: JNIEnv,
    _: JClass,
) {
    engine::flush_phase_lock_report();
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getPhaseLockLeadUs(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::phase_lock_lead_us()
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_hostSupportsControlMessage(
    _: JNIEnv,
    _: JClass,
    message_type: jint,
) -> jboolean {
    jbool(u16::try_from(message_type).is_ok_and(engine::host_supports))
}

/// Whether this stream draws with Frame Warp (`frame_warp::offered`), unless a renderer declined
/// it: a renderer set up again later (a decoder reset) keeps to that.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpOffered(
    _: JNIEnv,
    _: JClass,
) -> jboolean {
    jbool(frame_warp::offered() && !frame_warp::declined())
}

/// The address of the C table the native renderers call (`frame_warp_api.h`).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpApi(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    frame_warp::api_address()
}

/// The decoder queued `frame` with presentation time `pts_us`.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpNoteFrame(
    _: JNIEnv,
    _: JClass,
    pts_us: jlong,
    frame: jint,
) {
    frame_warp::note_frame(pts_us, frame as u32);
}

/// The shift for the picture queued with `pts_us`, drawn now: `out` gets uv x, uv y, px x, px y
/// and the lens terms k.x, k.y, n.x, n.y (`frame_warp::WARP_FLOATS` floats), and the return is the
/// HUD mask generation, -1 without a mask or -2 to draw it as it is (also when `out` is shorter).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpForPts(
    env: JNIEnv,
    _: JClass,
    pts_us: jlong,
    out: JFloatArray,
) -> jint {
    let (code, values) = frame_warp::warp_for(frame_warp::frame_for_pts(pts_us));
    if env.get_array_length(&out).unwrap_or(0) < values.len() as i32
        || env.set_float_array_region(&out, 0, &values).is_err()
    {
        return frame_warp::WARP_INACTIVE;
    }
    code
}

/// As `frameWarpForPts`, without counting it in the statistics: whether the GLES presenter
/// should draw the picture on screen again (`frame_warp::peek_for`).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpPeekForPts(
    env: JNIEnv,
    _: JClass,
    pts_us: jlong,
    out: JFloatArray,
) -> jint {
    let (code, values) = frame_warp::peek_for(frame_warp::frame_for_pts(pts_us));
    if env.get_array_length(&out).unwrap_or(0) < values.len() as i32
        || env.set_float_array_region(&out, 0, &values).is_err()
    {
        return frame_warp::WARP_INACTIVE;
    }
    code
}

/// What the GLES presenter checks each refresh before asking for a shift
/// (`frame_warp::input_state`): the count of inputs noted in the low 32 bits, bit 32 while the
/// tracked right stick is deflected.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpInputState(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    frame_warp::input_state()
}

/// The HUD mask when its generation is not `known` (-1 for none): `size` gets width and height,
/// and `cells`, when it has room, one byte per cell (0 or 255, rows from the top). Returns the
/// generation, -1 unchanged, -2 cleared, or -3 when `cells` is too small.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpMask(
    env: JNIEnv,
    _: JClass,
    known: jint,
    size: JIntArray,
    cells: JByteArray,
) -> jint {
    let warp = frame_warp::current();
    let mask = match frame_warp::mask_update(warp.as_deref(), known) {
        frame_warp::MaskUpdate::Unchanged => return frame_warp::MASK_UNCHANGED,
        frame_warp::MaskUpdate::Cleared => return frame_warp::MASK_CLEARED,
        frame_warp::MaskUpdate::New(mask) => mask,
    };
    if env.get_array_length(&size).unwrap_or(0) < 2
        || env
            .set_int_array_region(&size, 0, &[i32::from(mask.width), i32::from(mask.height)])
            .is_err()
    {
        return frame_warp::MASK_TOO_SMALL;
    }
    let count = usize::from(mask.width) * usize::from(mask.height);
    if (env.get_array_length(&cells).unwrap_or(0) as usize) < count {
        return frame_warp::MASK_TOO_SMALL;
    }
    let mut bytes = vec![0u8; count];
    if !frame_warp::pack_r8(&mask, &mut bytes)
        || env.set_byte_array_region(&cells, 0, bridge::jbytes(&bytes)).is_err()
    {
        return frame_warp::MASK_TOO_SMALL;
    }
    i32::from(mask.generation)
}

/// The renderer cannot draw Frame Warp for this stream: the host stops its work for it.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpDecline(
    _: JNIEnv,
    _: JClass,
) {
    frame_warp::decline();
}

/// The overlay's Frame Warp line: returns `RustBridge.FRAME_WARP_*` and writes gain, lag ms,
/// shift px and peak px to `out`.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_frameWarpStatus(
    env: JNIEnv,
    _: JClass,
    out: JFloatArray,
) -> jint {
    let mut numbers = [0.0f32; frame_warp::STATUS_NUMBERS];
    let state = frame_warp::status(&mut numbers);
    if env.get_array_length(&out).unwrap_or(0) >= numbers.len() as i32 {
        let _ = env.set_float_array_region(&out, 0, &numbers);
    }
    state
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendDisplayCaps(
    _: JNIEnv,
    _: JClass,
    hdr: jboolean,
    max_nits: jfloat,
    max_average_nits: jfloat,
    min_nits: jfloat,
) -> jboolean {
    let caps = extras::display_caps(hdr == JNI_TRUE, max_nits, max_average_nits, min_nits);
    jbool(engine::send(ClientMessage::DisplayCaps(caps)))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getMicrophoneState(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::microphone_state()
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getHostPacked444Modes(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::host_packed444_modes()
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getAnnouncedHdrFormat(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::announced_hdr_format()
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendMicrophonePcm(
    env: JNIEnv,
    _: JClass,
    pcm: JShortArray,
    samples: jint,
) -> jint {
    let samples = samples.max(0) as usize;
    if pcm.is_null()
        || !extras::valid_microphone_frame(samples)
        || env.get_array_length(&pcm).unwrap_or(0) < samples as i32
    {
        return -1;
    }
    let mut frame = vec![0i16; samples];
    if env.get_short_array_region(&pcm, 0, &mut frame).is_err() {
        return -1;
    }
    engine::send_microphone(&frame)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_requestReconfigure(
    _: JNIEnv,
    _: JClass,
    width: jint,
    height: jint,
    fps_millihz: jint,
) -> jboolean {
    let (Ok(width), Ok(height)) = (u16::try_from(width), u16::try_from(height)) else {
        return JNI_FALSE;
    };
    if fps_millihz <= 0 {
        return JNI_FALSE;
    }
    jbool(engine::send(ClientMessage::Reconfigure(Reconfigure {
        width,
        height,
        fps_millihz: fps_millihz as u32,
    })))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendHostTimingRequest(
    _: JNIEnv,
    _: JClass,
    enabled: jboolean,
) -> jboolean {
    jbool(engine::send(ClientMessage::HostTimingRequest(
        HostTimingRequest {
            enabled: enabled == JNI_TRUE,
        },
    )))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendLinkNoticeRequest(
    _: JNIEnv,
    _: JClass,
    enabled: jboolean,
) -> jboolean {
    jbool(engine::send(ClientMessage::LinkNoticeRequest(
        LinkNoticeRequest {
            enabled: enabled == JNI_TRUE,
        },
    )))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendClockSyncProbe(
    _: JNIEnv,
    _: JClass,
) -> jboolean {
    let now = platform::monotonic_us();
    jbool(engine::with_session(false, |s| {
        s.send_clock_sync_probe(now).is_ok()
    }))
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockStart(
    _: JNIEnv,
    _: JClass,
    min_interval_us: jint,
    adaptive: jboolean,
) {
    engine::present_clock_start(min_interval_us, adaptive != 0);
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockStop(
    _: JNIEnv,
    _: JClass,
) {
    engine::present_clock_stop();
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockReset(
    _: JNIEnv,
    _: JClass,
) {
    engine::present_clock_reset();
}

/// out: present time (ns), hold (us), host-to-present latency (us, -1 without clock sync),
/// late (0/1), as `android_present_clock.c`.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockSchedule(
    env: JNIEnv,
    _: JClass,
    host_us: jlong,
    ready_ns: jlong,
    out: JLongArray,
) -> jboolean {
    if host_us < 0 || ready_ns <= 0 || env.get_array_length(&out).unwrap_or(0) < 4 {
        return JNI_FALSE;
    }
    let ready_us = ready_ns as u64 / 1000;
    let Some(s) =
        engine::present_clock_schedule(extras::rtp_from_presentation_us(host_us as u64), ready_us)
    else {
        return JNI_FALSE;
    };
    // Keep the ready time's sub-microsecond part so a frame on time is not shown early.
    let present_ns = if s.present_us == ready_us {
        ready_ns
    } else {
        s.present_us as i64 * 1000
    };
    let values = [
        present_ns,
        s.hold_us,
        s.latency_us.unwrap_or(-1),
        i64::from(s.late),
    ];
    jbool(env.set_long_array_region(&out, 0, &values).is_ok())
}

/// out: release time (ns), vsync (ns), vsync index, flags (1 late, 2 shares the previous frame's
/// vsync), hold (us), host-to-vsync latency (us, -1 without clock sync), how late (us); see
/// `extras::grid_schedule_to_ns`.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockScheduleOnGrid(
    env: JNIEnv,
    _: JClass,
    host_us: jlong,
    ready_ns: jlong,
    anchor_ns: jlong,
    interval_ns: jlong,
    lead_ns: jlong,
    slack_ns: jlong,
    out: JLongArray,
) -> jboolean {
    if host_us < 0 || ready_ns <= 0 || interval_ns <= 0 || env.get_array_length(&out).unwrap_or(0) < 7 {
        return JNI_FALSE;
    }
    let grid = extras::grid_from_ns(anchor_ns, interval_ns, lead_ns, slack_ns);
    let Some(s) = engine::present_clock_schedule_on_grid(
        extras::rtp_from_presentation_us(host_us as u64),
        ready_ns as u64 / 1000,
        grid,
    ) else {
        return JNI_FALSE;
    };
    let values = extras::grid_schedule_to_ns(&s, ready_ns, anchor_ns, interval_ns, lead_ns);
    jbool(env.set_long_array_region(&out, 0, &values).is_ok())
}

/// A frame's host time (us) for the content rate.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockObserve(
    _: JNIEnv,
    _: JClass,
    host_us: jlong,
) {
    if host_us >= 0 {
        engine::present_clock_observe(extras::rtp_from_presentation_us(host_us as u64));
    }
}

/// The content rate in millihertz, 0 until it held steady for two seconds.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockContentRate(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::present_clock_content_rate().min(i32::MAX as u32) as jint
}

/// out: frames scheduled, frames late, clock sync round trip (us, -1 without).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockStats(
    env: JNIEnv,
    _: JClass,
    out: JLongArray,
) -> jboolean {
    if env.get_array_length(&out).unwrap_or(0) < 3 {
        return JNI_FALSE;
    }
    let Some((frames, late, rtt)) = engine::present_clock_stats() else {
        return JNI_FALSE;
    };
    jbool(
        env.set_long_array_region(&out, 0, &[frames as i64, late as i64, rtt])
            .is_ok(),
    )
}

/// out: host-to-client clock offset (us, `client = host + offset`) now and the round trip (us) of
/// the exchange it comes from, as `android_present_clock.c`. False before the first exchange.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_presentClockOffset(
    env: JNIEnv,
    _: JClass,
    out: JLongArray,
) -> jboolean {
    if env.get_array_length(&out).unwrap_or(0) < 2 {
        return JNI_FALSE;
    }
    let Some((offset, rtt)) = engine::present_clock_offset(platform::monotonic_us()) else {
        return JNI_FALSE;
    };
    jbool(env.set_long_array_region(&out, 0, &[offset, rtt]).is_ok())
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendCursorShapeRequest(
    _: JNIEnv,
    _: JClass,
    enabled: jboolean,
) -> jboolean {
    jbool(engine::send(ClientMessage::CursorShapeRequest {
        enabled: enabled == JNI_TRUE,
    }))
}

/// Text field notices (0x553B) on or off; false without a host that announced them.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendTextInputRequest(
    _: JNIEnv,
    _: JClass,
    enabled: jboolean,
) -> jboolean {
    jbool(engine::send(extras::text_input_request(
        enabled == JNI_TRUE,
    )))
}

/// A bitrate in kbps, a rate in mHz or both (0x553C), 0 keeping that value; the host answers
/// with `bridgeClRateChange` and the same serial. False for a negative or invalid value, or
/// without a host that announced it.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendRateChange(
    _: JNIEnv,
    _: JClass,
    serial: jint,
    bitrate_kbps: jint,
    fps_millihz: jint,
) -> jboolean {
    jbool(extras::rate_change_message(serial, bitrate_kbps, fps_millihz).is_some_and(engine::send))
}

/// The device's keyboard layout as a BCP-47 tag (0x553E); false for a tag the wire cannot
/// carry or without a host that announced it.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendKeyboardLayout(
    mut env: JNIEnv,
    _: JClass,
    tag: JString,
) -> jboolean {
    let message = string(&mut env, &tag)
        .as_deref()
        .and_then(extras::keyboard_layout_message);
    jbool(message.is_some_and(engine::send))
}

/// Starts clipboard sync (0x5508) with a host that offered it: `MoonBridge.CLIPBOARD_SYNC_*`.
/// `current` is the clipboard here as it is, null when it could not be read; it stays here.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_clipboardSyncStart(
    mut env: JNIEnv,
    _: JClass,
    current: JString,
) -> jint {
    engine::clipboard_sync_start(string(&mut env, &current))
}

/// Text copied here, for clipboard sync: `MoonBridge.CLIPBOARD_COPY_*`.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_clipboardSyncLocal(
    mut env: JNIEnv,
    _: JClass,
    text: JString,
) -> jint {
    match string(&mut env, &text) {
        Some(text) => engine::clipboard_sync_local(text),
        None => engine::CLIPBOARD_COPY_SKIPPED,
    }
}

/// DualSense microphone PCM (0x5534) or USB audio endpoint status (0x5535).
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_sendControllerAudio(
    env: JNIEnv,
    _: JClass,
    kind: jint,
    payload: JByteArray,
) -> jboolean {
    if payload.is_null() {
        return JNI_FALSE;
    }
    let Ok(bytes) = env.convert_byte_array(&payload) else {
        return JNI_FALSE;
    };
    match extras::controller_audio_message(kind, &bytes) {
        Some(message) => jbool(engine::send(message)),
        None => JNI_FALSE,
    }
}

/// Attaches the device Java's `UsbDeviceConnection` has open (`fd`, its node `path`) to the
/// stream's host: the request id, or a `USB_*` code. Before it returns, Rust has its own copy
/// of `fd` (a dup of the same usbfs file), which it closes when the attachment ends or fails.
/// Java keeps its connection open until it hears the attachment ended and closes it then; the
/// kernel lets go of the device once both are closed.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_usbAttach(
    mut env: JNIEnv,
    _: JClass,
    fd: jint,
    path: JString,
    name: JString,
    address: JString,
    http_port: jint,
    https_port: jint,
    server_certificate_pem: JString,
    unique_id: JString,
    client_certificate_der: JByteArray,
    client_private_key_pkcs8: JByteArray,
) -> jint {
    let bytes = |array: &JByteArray| {
        (!array.is_null())
            .then(|| env.convert_byte_array(array).ok())
            .flatten()
    };
    let (certificate, key) = (
        bytes(&client_certificate_der),
        bytes(&client_private_key_pkcs8),
    );
    let endpoint = (|| {
        Some(native_api::Endpoint {
            address: string(&mut env, &address)?,
            http_port: u16::try_from(http_port).ok()?,
            https_port: u16::try_from(https_port).unwrap_or(0),
            server_certificate_pem: string(&mut env, &server_certificate_pem)?,
            unique_id: string(&mut env, &unique_id)?,
            client_certificate_der: certificate?,
            client_private_key_pkcs8: key?,
        })
    })();
    let (Some(endpoint), Some(path), Some(name)) =
        (endpoint, string(&mut env, &path), string(&mut env, &name))
    else {
        platform::error("USB: usbAttach without its host or device");
        return usb::INVALID;
    };
    let device = match usb::open(fd, &path) {
        Ok(device) => device,
        Err(e) => {
            platform::error(&format!("USB: {name} at {path} did not open: {e}"));
            return usb::DEVICE_FAILED;
        }
    };
    let make = move || {
        let host = native_api::client(&endpoint).map_err(|e| e.to_string())?;
        let send = |message| {
            engine::with_session(Err(nightjar_core::control::SendError::Stopped), |s| {
                s.send(message)
            })
        };
        let changed = || {
            if let Some(bridge) = bridge::get() {
                bridge.post(bridge.m.usb_changed, &[]);
            }
        };
        Ok(nightjar_core::usb::Forwarder::for_host(
            std::sync::Arc::new(host),
            send,
            changed,
        ))
    };
    usb::attach(make, device, &name)
}

/// Detaches an attachment `usbAttach` made; false when it is not attached.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_usbDetach(
    _: JNIEnv,
    _: JClass,
    request_id: jint,
) -> jboolean {
    jbool(u32::try_from(request_id).is_ok_and(usb::detach))
}

/// This stream's attachments as JSON, oldest first: `[]` without any.
#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_usbList(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    match env.new_string(usb::list_json()) {
        Ok(list) => list.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod host_loss_tests {
    use super::host_loss_after_goodbye;
    use nightjar_core::host_loss::{Goodbye, Round, HOST_EXIT_WINDOW, QUIET_LINK_WAIT};

    const EXIT_MS: i64 = HOST_EXIT_WINDOW.as_millis() as i64;
    const QUIET_MS: i64 = QUIET_LINK_WAIT.as_millis() as i64;

    #[test]
    fn the_codes_from_java_reach_the_shared_verdict() {
        assert_eq!(host_loss_after_goodbye(true, Round::SilentLinkUp.code(), 0), Goodbye::Gone.code());
        assert_eq!(host_loss_after_goodbye(true, Round::Refused.code(), 0), Goodbye::Gone.code());
        assert_eq!(host_loss_after_goodbye(true, Round::Answered.code(), 0), Goodbye::Wait.code());
        assert_eq!(host_loss_after_goodbye(true, Round::Answered.code(), EXIT_MS), Goodbye::Unknown.code());
    }

    #[test]
    fn what_java_cannot_tell_is_the_old_behaviour() {
        // A PC off the link.
        assert_eq!(host_loss_after_goodbye(false, Round::SilentLinkUp.code(), 0), Goodbye::Unknown.code());
        // An unknown round code reads as a quiet link: another round, then the old behaviour.
        assert_eq!(host_loss_after_goodbye(true, 99, 0), Goodbye::Wait.code());
        assert_eq!(host_loss_after_goodbye(true, 99, QUIET_MS), Goodbye::Unknown.code());
        // A negative wait from a clock step is no wait.
        assert_eq!(host_loss_after_goodbye(true, Round::Answered.code(), -5), Goodbye::Wait.code());
    }
}

