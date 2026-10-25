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
mod engine;
pub mod extras;
mod listener;
pub mod native_api;
mod opus;
mod platform;
mod renderers;

use std::time::Duration;

use jni::objects::{JByteArray, JClass, JLongArray, JShortArray, JString};
use jni::sys::{jboolean, jbyte, jfloat, jint, jlong, jshort, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

use nightjar_core::control::ClientMessage;
use nightjar_core::input::InputQueue;
use nightjar_core::session::{HostInfo, SessionConfig};
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
) -> jboolean {
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
    let config = SessionConfig {
        stream,
        location: extras::location(streaming_remotely),
        network_grace: Duration::from_millis(network_grace_ms.max(0) as u64),
        input_only,
        remote_monitor: remote_monitor == JNI_TRUE,
        unbatched_input: unbatched_input == JNI_TRUE,
        network_priority: network_priority == JNI_TRUE,
    };
    engine::start(bridge, engine::StartParams { host, config })
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
        )
    })
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
    engine::with_session(0, |s| {
        let duration = i32::from(s.outcome().audio_packet_duration_ms);
        s.stats().audio_queue_len as i32 * duration
    })
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getPendingVideoFrames(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::with_session(0, |s| s.stats().decode_queue_len as i32)
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_nvstream_jni_RustBridge_getEstimatedRttInfo(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(-1, |s| extras::rtt_info(s.stats().rtt))
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
) {
    engine::present_clock_start(min_interval_us);
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
