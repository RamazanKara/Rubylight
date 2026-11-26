//! JNI bridge for `NightjarPolicyJni`: decoder queue policy (`DecoderQueuePolicy`), display
//! queue watch (`DisplayQueueWatch`), present scheduler lateness & counters
//! (`PresentSchedulerState`), recovery backoff (`DecoderRecoveryBackoff`), and pure decoder
//! hang/catch-up helpers (`MediaCodecDecoderRenderer`).

use jni::JNIEnv;
use jni::objects::{JClass, JLongArray, JString};
use jni::sys::{JNI_FALSE, jboolean, jdouble, jint, jlong, jstring};
use nightjar_policy::video_queue::{
    self, CompositorWatch, Debt, DecoderRecoveryBackoff, DisplayGate, DisplayQueueWatch,
    HungDecoderAction, InFlight, PresentSchedulerState, RecentLevel, Residency,
};

use crate::policy::jbool;
use crate::policy_recovery::new_jstring;
use crate::slab::MutexSlab;
use crate::string;

static RESIDENCY_SLAB: MutexSlab<Residency> = MutexSlab::new();
static DISPLAY_GATE_SLAB: MutexSlab<DisplayGate> = MutexSlab::new();
static RECENT_LEVEL_SLAB: MutexSlab<RecentLevel> = MutexSlab::new();
static DEBT_SLAB: MutexSlab<Debt> = MutexSlab::new();
static IN_FLIGHT_SLAB: MutexSlab<InFlight> = MutexSlab::new();
static DISPLAY_WATCH_SLAB: MutexSlab<DisplayQueueWatch> = MutexSlab::new();
static PRESENT_SCHEDULER_SLAB: MutexSlab<PresentSchedulerState> = MutexSlab::new();
static RECOVERY_BACKOFF_SLAB: MutexSlab<DecoderRecoveryBackoff> = MutexSlab::new();
static COMPOSITOR_WATCH_SLAB: MutexSlab<CompositorWatch> = MutexSlab::new();

fn opt_jstring(env: &JNIEnv, text: Option<String>) -> jstring {
    match text {
        Some(s) => new_jstring(env, &s),
        None => std::ptr::null_mut(),
    }
}

// --- Slab Lifecycle (0=Residency, 1=DisplayGate, 2=RecentLevel, 3=Debt, 4=InFlight,
//                      5=DisplayQueueWatch, 6=PresentSchedulerState, 7=DecoderRecoveryBackoff,
//                      8=CompositorWatch) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_queueSlabCreate(
    _: JNIEnv,
    _: JClass,
    kind: jint,
) -> jlong {
    match kind {
        0 => RESIDENCY_SLAB.insert(Residency::new()),
        1 => DISPLAY_GATE_SLAB.insert(DisplayGate::new()),
        2 => RECENT_LEVEL_SLAB.insert(RecentLevel::new()),
        3 => DEBT_SLAB.insert(Debt::new()),
        4 => IN_FLIGHT_SLAB.insert(InFlight::new()),
        5 => DISPLAY_WATCH_SLAB.insert(DisplayQueueWatch::new()),
        6 => PRESENT_SCHEDULER_SLAB.insert(PresentSchedulerState::new()),
        7 => RECOVERY_BACKOFF_SLAB.insert(DecoderRecoveryBackoff::new()),
        _ => COMPOSITOR_WATCH_SLAB.insert(CompositorWatch::new()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_queueSlabDestroy(
    _: JNIEnv,
    _: JClass,
    kind: jint,
    handle: jlong,
) {
    match kind {
        0 => _ = RESIDENCY_SLAB.remove(handle),
        1 => _ = DISPLAY_GATE_SLAB.remove(handle),
        2 => _ = RECENT_LEVEL_SLAB.remove(handle),
        3 => _ = DEBT_SLAB.remove(handle),
        4 => _ = IN_FLIGHT_SLAB.remove(handle),
        5 => _ = DISPLAY_WATCH_SLAB.remove(handle),
        6 => _ = PRESENT_SCHEDULER_SLAB.remove(handle),
        7 => _ = RECOVERY_BACKOFF_SLAB.remove(handle),
        _ => _ = COMPOSITOR_WATCH_SLAB.remove(handle),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryBackoffCreate(
    env: JNIEnv,
    _: JClass,
    delays_ms: JLongArray,
) -> jlong {
    let backoff = if delays_ms.is_null() {
        DecoderRecoveryBackoff::new()
    } else {
        let len = env.get_array_length(&delays_ms).unwrap_or(0).max(0) as usize;
        let mut buf = vec![0i64; len];
        if len > 0 && env.get_long_array_region(&delays_ms, 0, &mut buf).is_err() {
            DecoderRecoveryBackoff::new()
        } else {
            DecoderRecoveryBackoff::with_delays(buf)
        }
    };
    RECOVERY_BACKOFF_SLAB.insert(backoff)
}

// --- Stateless DecoderQueuePolicy ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_decoderQueueDepth(
    _: JNIEnv,
    _: JClass,
    frames_in: jint,
    frames_out: jint,
    base: jint,
) -> jint {
    video_queue::depth(frames_in, frames_out, base)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_decoderOutputLimit(
    mut env: JNIEnv,
    _: JClass,
    decoder_name: JString,
    advertised_low_latency: jboolean,
    drop_late_frames: jboolean,
    recovered: jboolean,
) -> jint {
    let name = string(&mut env, &decoder_name).unwrap_or_default();
    video_queue::output_limit(
        &name,
        advertised_low_latency != JNI_FALSE,
        drop_late_frames != JNI_FALSE,
        recovered != JNI_FALSE,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_decoderWatchesDisplayQueue(
    mut env: JNIEnv,
    _: JClass,
    decoder_name: JString,
) -> jboolean {
    let name = string(&mut env, &decoder_name).unwrap_or_default();
    jbool(video_queue::watches_display_queue(&name))
}

// --- Residency / DisplayGate / RecentLevel / Debt / InFlight ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_residencyOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
) -> jlong {
    RESIDENCY_SLAB
        .with(handle, |r| match op {
            0 => r.on_output(a, b),
            1 => {
                r.track(a, b);
                0
            }
            2 => jlong::from(r.should_drop(a as i32)),
            3 => {
                r.on_dropped();
                0
            }
            _ => {
                r.reset();
                0
            }
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_displayGateOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
    c: jlong,
    fa: jboolean,
    fb: jboolean,
) -> jlong {
    DISPLAY_GATE_SLAB
        .with(handle, |g| match op {
            0 => {
                g.on_output(a, b, c, fa != JNI_FALSE, fb != JNI_FALSE);
                0
            }
            1 => jlong::from(g.held()),
            2 => {
                g.clear();
                0
            }
            _ => {
                g.reset();
                0
            }
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_displayGateStillHeld(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    min_buckets: jint,
    max_slope: jdouble,
) -> jboolean {
    jbool(
        DISPLAY_GATE_SLAB
            .with(handle, |g| g.still_held(min_buckets, max_slope))
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recentLevelOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
) -> jlong {
    RECENT_LEVEL_SLAB
        .with(handle, |rl| match op {
            0 => {
                rl.add(a, b);
                0
            }
            1 => rl.low_ns(),
            2 => rl.last_ns(),
            _ => {
                rl.reset();
                0
            }
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_debtOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
    c: jlong,
    d: jlong,
    e: jint,
    flag: jboolean,
) -> jlong {
    DEBT_SLAB
        .with(handle, |debt| match op {
            0 => {
                debt.watch_display(flag != JNI_FALSE);
                0
            }
            1 => jlong::from(debt.watches_display()),
            2 => debt.on_output(a, b, c, d, e),
            3 => jlong::from(debt.should_drop()),
            4 => {
                debt.on_dropped();
                0
            }
            5 => {
                debt.on_no_newer();
                0
            }
            _ => {
                debt.reset();
                0
            }
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_debtTakeEvent(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let event = DEBT_SLAB.with(handle, |debt| debt.take_event()).flatten();
    opt_jstring(&env, event)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_inFlightOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
) -> jlong {
    IN_FLIGHT_SLAB
        .with(handle, |inf| match op {
            0 => {
                inf.on_released(a, b);
                0
            }
            1 => {
                inf.on_rendered(a);
                0
            }
            2 => jlong::from(inf.count(a)),
            3 => jlong::from(inf.full(a)),
            _ => {
                inf.reset();
                0
            }
        })
        .unwrap_or(0)
}

// --- DisplayQueueWatch ---

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_displayQueueApplies(
    _: JNIEnv,
    _: JClass,
    setting_on: jboolean,
    latency_or_balanced: jboolean,
    scheduled: jboolean,
    render_callbacks: jboolean,
    vrr: jboolean,
    stream_frame_rate_milli_hz: jint,
    refresh_ns: jlong,
) -> jboolean {
    jbool(DisplayQueueWatch::applies(
        setting_on != JNI_FALSE,
        latency_or_balanced != JNI_FALSE,
        scheduled != JNI_FALSE,
        render_callbacks != JNI_FALSE,
        vrr != JNI_FALSE,
        stream_frame_rate_milli_hz,
        refresh_ns,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_displayQueueOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
    flag: jboolean,
) -> jlong {
    DISPLAY_WATCH_SLAB
        .with(handle, |w| match op {
            0 => {
                w.configure(flag != JNI_FALSE, a);
                0
            }
            1 => {
                w.on_rendered(a, b);
                0
            }
            _ => jlong::from(w.take_drop(a)),
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_displayQueueTakeEvent(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let event = DISPLAY_WATCH_SLAB
        .with(handle, |w| w.take_event())
        .flatten();
    opt_jstring(&env, event)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_compositorWatchOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    a: jlong,
    b: jlong,
    flag: jboolean,
) -> jlong {
    COMPOSITOR_WATCH_SLAB
        .with(handle, |w| match op {
            0 => {
                w.reset_window();
                0
            }
            1 => {
                w.on_released(a.max(0) as u64, flag != JNI_FALSE);
                0
            }
            2 => {
                w.on_rendered(a.max(0) as u64, b.max(0) as u64, b.max(0) as u64);
                0
            }
            3 => jlong::from(w.poll(a.max(0) as u64, flag != JNI_FALSE, b != 0) as i32),
            _ => i64::from(w.is_locked_on()),
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_compositorWatchTakeEvent(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let event = COMPOSITOR_WATCH_SLAB
        .with(handle, |w| w.take_event())
        .flatten();
    opt_jstring(&env, event)
}

// --- PresentSchedulerState ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_presentSchedulerIsLate(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    host_us: jlong,
    ready_ns: jlong,
    content_interval_ns: jlong,
) -> jboolean {
    jbool(
        PRESENT_SCHEDULER_SLAB
            .with(handle, |ps| {
                ps.is_late(host_us, ready_ns, content_interval_ns)
            })
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_presentSchedulerDropLate(
    _: JNIEnv,
    _: JClass,
    late: jboolean,
    newer_available: jboolean,
    previous_dropped: jboolean,
) -> jboolean {
    jbool(PresentSchedulerState::drop_late(
        late != JNI_FALSE,
        newer_available != JNI_FALSE,
        previous_dropped != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_presentSchedulerOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    flag: jboolean,
) -> jlong {
    PRESENT_SCHEDULER_SLAB
        .with(handle, |ps| match op {
            0 => {
                ps.reset();
                0
            }
            1 => {
                ps.on_planned(flag != JNI_FALSE);
                0
            }
            2 => {
                ps.on_late_dropped();
                0
            }
            3 => {
                ps.on_governor_dropped();
                0
            }
            4 => {
                ps.on_cap_dropped();
                0
            }
            5 => {
                ps.on_cap_held();
                0
            }
            6 => ps.late_dropped(),
            7 => ps.governor_dropped(),
            8 => ps.cap_dropped(),
            _ => ps.cap_held(),
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_presentSchedulerOverlayLine(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let line = PRESENT_SCHEDULER_SLAB
        .with(handle, |ps| ps.overlay_line())
        .flatten();
    opt_jstring(&env, line)
}

// --- DecoderRecoveryBackoff & Pure MediaCodecDecoderRenderer Helpers ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryBackoffOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    handle: jlong,
    frames_out: jint,
) -> jlong {
    RECOVERY_BACKOFF_SLAB
        .with(handle, |b| match op {
            0 => b.before_recovery(frames_out),
            _ => jlong::from(b.recoveries_without_frame()),
        })
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_decoderWantsCatchUp(
    mut env: JNIEnv,
    _: JClass,
    residency_handle: jlong,
    mode: jint,
    enqueue_ns: jlong,
    now_ns: jlong,
    fps_milli_hz: jint,
    drained: jint,
    max_output_drain: jint,
    scheduled: jboolean,
    frames_in: jint,
    frames_out: jint,
    depth_base: jint,
) -> jint {
    match RESIDENCY_SLAB.with(residency_handle, |r| {
        video_queue::wants_catch_up(
            mode,
            enqueue_ns,
            now_ns,
            fps_milli_hz,
            drained,
            max_output_drain,
            scheduled != JNI_FALSE,
            frames_in,
            frames_out,
            depth_base,
            r,
        )
    }) {
        Some(Some(r)) => r as jint,
        Some(None) => {
            let _ = env.throw_new(
                "java/lang/IllegalArgumentException",
                "Invalid stream frame rate",
            );
            0
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_decoderRecoveryOp(
    _: JNIEnv,
    _: JClass,
    op: jint,
    has_vendor_keys: jboolean,
    has_decoder: jboolean,
    already_hung_with_keys: jboolean,
    attempts: jint,
    frames_out: jint,
    frames_out_at_last_recovery: jint,
    frames_out_at_hang_reset: jint,
    uptime_ms: jlong,
    uptime_at_last_recovery_ms: jlong,
    uptime_at_hang_reset_ms: jlong,
) -> jint {
    match op {
        0 => {
            let (left, refilled) = video_queue::recovery_attempts_left(
                attempts,
                frames_out,
                frames_out_at_last_recovery,
                uptime_ms,
                uptime_at_last_recovery_ms,
            );
            jint::from(left) | (jint::from(refilled) << 1)
        }
        1 => jint::from(video_queue::ran_well_since_hang_reset(
            frames_out,
            frames_out_at_hang_reset,
            uptime_ms,
            uptime_at_hang_reset_ms,
        )),
        _ => {
            let decision = video_queue::hung_decoder_decision(
                has_vendor_keys != JNI_FALSE,
                has_decoder != JNI_FALSE,
                already_hung_with_keys != JNI_FALSE,
                attempts,
                frames_out,
                frames_out_at_last_recovery,
                frames_out_at_hang_reset,
                uptime_ms,
                uptime_at_last_recovery_ms,
                uptime_at_hang_reset_ms,
            );
            let code = match decision.action {
                HungDecoderAction::GiveUp => 0,
                HungDecoderAction::ResetWithKeys => 1,
                HungDecoderAction::RebuildWithoutKeys => 2,
            };
            code | (jint::from(decision.refilled_attempts) << 2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wants_catch_up_slab_miss_distinct_from_invalid_fps() {
        use super::{RESIDENCY_SLAB, Residency};
        use nightjar_policy::video_queue::{CatchUpReason as R, wants_catch_up as wcu};
        let f = |h, fps| RESIDENCY_SLAB.with(h, |r| wcu(3, 1, 1, fps, 0, 64, true, 1, 1, 0, r));
        let h = RESIDENCY_SLAB.insert(Residency::new());
        let got = (f(0, 60_000), f(h, 0), f(h, 60_000));
        assert_eq!(got, (None, Some(None), Some(Some(R::None))));
        assert!(RESIDENCY_SLAB.remove(h));
    }
}
