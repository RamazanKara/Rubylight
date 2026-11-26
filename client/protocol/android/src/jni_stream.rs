//! JNI control, telemetry, phase-lock, microphone, present-clock, and clipboard-sync entry
//! points for `com.limelight.nvstream.jni.NightjarJni`.

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JLongArray, JShortArray, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jfloat, jint, jlong};

use nightjar_core::control::ClientMessage;
use rubylight_protocol::control::{HostTimingRequest, LinkNoticeRequest, Reconfigure};

use crate::{engine, extras, jbool, platform, renderers, string};

/// Apollo's server command (0x3000): one or four bytes, Sunshine hosts only.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendServerCommand(
    env: JNIEnv,
    _: JClass,
    payload: JByteArray,
) -> jboolean {
    let Ok(bytes) = env.convert_byte_array(&payload) else {
        return JNI_FALSE;
    };
    jbool(engine::send(ClientMessage::ServerCommand(bytes)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getPendingAudioDuration(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::pending_audio_ms() as jint
}

/// The Opus concealer for the AAudio callback (`aaudio_output.c`), as a C function pointer.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_audioConcealer(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    let conceal: unsafe extern "C" fn(*mut i16, i32) -> i32 = renderers::conceal;
    conceal as usize as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getPendingVideoFrames(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::decode_queue_len() as jint
}

/// Frames dropped over a full decode queue since the stream started (`VideoStats::decoder_drops`);
/// 0 without a session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getDecoderDrops(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::decoder_drops().min(i64::MAX as u64) as jlong
}

/// Bytes of video and audio datagrams received, FEC parity included; 0 without a session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getWireBytesReceived(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(0, |s| s.wire_bytes() as jlong)
}

/// The video socket's receive buffer, asked over granted (`extras::receive_buffer_info`); -1 without a session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getVideoReceiveBufferInfo(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(-1, |s| {
        extras::receive_buffer_info(s.video_receive_buffer())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getEstimatedRttInfo(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    engine::with_session(-1, |s| extras::rtt_info(s.rtt()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_reportFrameSlack(
    _: JNIEnv,
    _: JClass,
    slack_ns: jlong,
    period_ns: jlong,
) -> jboolean {
    jbool(engine::report_frame_slack(slack_ns, period_ns))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_flushPhaseLockReport(
    _: JNIEnv,
    _: JClass,
) {
    engine::flush_phase_lock_report();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getPhaseLockLeadUs(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::phase_lock_lead_us()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hostSupportsControlMessage(
    _: JNIEnv,
    _: JClass,
    message_type: jint,
) -> jboolean {
    jbool(u16::try_from(message_type).is_ok_and(engine::host_supports))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendDisplayCaps(
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getMicrophoneState(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::microphone_state()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getHostPacked444Modes(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::host_packed444_modes()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_getAnnouncedHdrFormat(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::announced_hdr_format()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMicrophonePcm(
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
    let mut buf = [0i16; 2880];
    if env.get_short_array_region(&pcm, 0, &mut buf[..samples]).is_err() {
        return -1;
    }
    engine::send_microphone(&buf[..samples])
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_requestReconfigure(
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendHostTimingRequest(
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendLinkNoticeRequest(
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendClockSyncProbe(
    _: JNIEnv,
    _: JClass,
) -> jboolean {
    let now = platform::monotonic_us();
    jbool(engine::with_session(false, |s| {
        s.send_clock_sync_probe(now).is_ok()
    }))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockStart(
    _: JNIEnv,
    _: JClass,
    min_interval_us: jint,
    adaptive: jboolean,
) {
    engine::present_clock_start(min_interval_us, adaptive != 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockStop(
    _: JNIEnv,
    _: JClass,
) {
    engine::present_clock_stop();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockReset(
    _: JNIEnv,
    _: JClass,
) {
    engine::present_clock_reset();
}

/// out: present time (ns), hold (us), host-to-present latency (us, -1 without clock sync),
/// late (0/1), as `android_present_clock.c`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockSchedule(
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockScheduleOnGrid(
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
    if host_us < 0
        || ready_ns <= 0
        || interval_ns <= 0
        || env.get_array_length(&out).unwrap_or(0) < 7
    {
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockObserve(
    _: JNIEnv,
    _: JClass,
    host_us: jlong,
) {
    if host_us >= 0 {
        engine::present_clock_observe(extras::rtp_from_presentation_us(host_us as u64));
    }
}

/// The content rate in millihertz, 0 until it held steady for two seconds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockContentRate(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::present_clock_content_rate().min(i32::MAX as u32) as jint
}

/// out: frames scheduled, frames late, clock sync round trip (us, -1 without).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockStats(
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_presentClockOffset(
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendCursorShapeRequest(
    _: JNIEnv,
    _: JClass,
    enabled: jboolean,
) -> jboolean {
    jbool(engine::send(ClientMessage::CursorShapeRequest {
        enabled: enabled == JNI_TRUE,
    }))
}

/// Text field notices (0x553B) on or off; false without a host that announced them.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendTextInputRequest(
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendRateChange(
    _: JNIEnv,
    _: JClass,
    serial: jint,
    bitrate_kbps: jint,
    fps_millihz: jint,
) -> jboolean {
    jbool(extras::rate_change_message(serial, bitrate_kbps, fps_millihz).is_some_and(engine::send))
}

/// A Stream DVR request (0x554A): `op` 1 asks for the state, 2 saves a clip of `duration_secs`
/// (0 for the whole buffer), 3 starts and 4 stops a recording. The host answers with
/// `bridgeClDvrState` or `bridgeClDvrClip` and the same id. False for a bad op or length, or
/// without a host that announced it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendStreamDvr(
    _: JNIEnv,
    _: JClass,
    request_id: jint,
    op: jint,
    duration_secs: jint,
) -> jboolean {
    jbool(extras::stream_dvr_message(request_id, op, duration_secs).is_some_and(engine::send))
}

/// The device's keyboard layout as a BCP-47 tag (0x553E); false for a tag the wire cannot
/// carry or without a host that announced it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendKeyboardLayout(
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
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_clipboardSyncStart(
    mut env: JNIEnv,
    _: JClass,
    current: JString,
) -> jint {
    engine::clipboard_sync_start(string(&mut env, &current))
}

/// Text copied here, for clipboard sync: `MoonBridge.CLIPBOARD_COPY_*`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_clipboardSyncLocal(
    mut env: JNIEnv,
    _: JClass,
    text: JString,
) -> jint {
    string(&mut env, &text).map_or(engine::CLIPBOARD_COPY_SKIPPED, engine::clipboard_sync_local)
}

/// DualSense microphone PCM (0x5534) or USB audio endpoint status (0x5535).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendControllerAudio(
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
