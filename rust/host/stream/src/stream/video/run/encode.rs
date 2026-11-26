//! Per-frame encoder preparation (luminance, overlay, HDR metadata, invalidation, client
//! cursor, focus), submission, and encode-failure handling for the video session loop.

use super::super::*;
use super::inline_send::InlineSender;
use super::rebuild::{apply_frame_cap, rollback_mode_switch};
use super::timings::{TimingsWindow, emit_claim_trace};

/// Result of submitting one captured picture to the active encoder.
pub(super) enum EncodeStep {
    Encoded {
        output: Vec<butterpollo_win_graphics::encoder::Encoded>,
        begin: Instant,
        fresh: bool,
        recovering: bool,
        presented_captured: Instant,
        last_submitted: Option<Instant>,
    },
    /// Recoverable error handled (device loss, mode rollback, or rebuild scheduled).
    Continue,
}

/// Configures per-frame encoder state and encodes `image`.
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_frame(
    thread: &SessionThread<'_>,
    c: &Config,
    runtime_config: &Config,
    prepared: &Arc<crate::display_session::Ready>,
    progress: &crate::stall_watch::Progress,
    latest: &Source,
    active: &mut Encoder,
    mut readback: impl FnMut(&GpuImage) -> Result<butterpollo_win_graphics::capture::Image>,
    stream: &mut butterpollo_media_policy::rtsp::Negotiated,
    resolution_cap: Option<(u32, u32)>,
    switching: &mut Option<(butterpollo_media_policy::reconfigure::Mode, Instant)>,
    rebuild_encoder: &mut bool,
    applied_luminance: &mut Option<(f32, f32)>,
    metadata_due: &mut Instant,
    confirm_keyframe: &mut Option<Instant>,
    menu: &mut crate::couch_menu::VideoOverlay,
    enhancer: &crate::nvidia_enhance::Enhancer,
    focus: &mut crate::stream_focus::Focus,
    focus_verify: Option<u32>,
    pointer_left_out: &mut bool,
    pointer_warned: &mut bool,
    backend: &str,
    inline_sender: &InlineSender,
    recovery: &mut EncoderRecovery,
    counted_failures: &mut CountedFailures,
    recovery_packetizer: &mut VideoPacketizer,
    fec_whole: &mut ((usize, usize, bool), usize),
    fec_whole_due: &mut Instant,
    checks: &mut crate::video_send::link_watch::CheckTimes,
    mut frame_checks: Duration,
    timings: &mut TimingsWindow,
    admission_trace: &mut Option<AdmissionTrace>,
    trace_attempt: &mut u64,
    trace_send: bool,
    start: Instant,
    first_seen: Option<(usize, Instant, Option<Duration>, Option<Instant>)>,
    raw_image: &Arc<GpuImage>,
    image: &Arc<GpuImage>,
    last_image: &Option<std::sync::Weak<GpuImage>>,
    pacing: &capture::PacingSnapshot,
    admission_recovering: bool,
    encoding: usize,
    sending: usize,
    encoding_limit: usize,
    rebuilt: bool,
    peer: SocketAddr,
    period: Duration,
    direct_encoder_input: bool,
) -> Result<EncodeStep> {
    let SessionThread { m, s, .. } = *thread;
    // RTX HDR uses its own SDR white and scale; otherwise follow Windows' SDR brightness for HDR
    // and an HDR desktop (FP16, or PQ that an SDR stream tone-maps).
    let wanted_luminance = enhancer.luminance().or_else(|| {
        (stream.hdr || image.pixel != butterpollo_win_graphics::capture::Pixel::Bgra8)
            .then(|| (image.gpu.sdr_white_nits(), 1.0))
    });
    if let Some((white, scale)) = wanted_luminance
        && (rebuilt || *applied_luminance != Some((white, scale)))
    {
        active.set_luminance(white, scale);
        *applied_luminance = Some((white, scale));
    }
    if menu.dirty || (rebuilt && menu.has_overlay()) {
        active.set_overlay(menu.for_encoder([stream.width, stream.height]));
    }
    // Frames of a stream that left slice delivery return whole.
    inline_sender.delivery.sync(&s.stats);
    if inline_sender.delivery.joins() {
        active.join_slices();
    }
    if rebuilt {
        active.set_next_frame(match inline_sender.video_sender.get() {
            Some(sender) => sender.next_wire_frame()?,
            None => inline_sender.next_wire_frame.get(),
        });
    }
    let begin = Instant::now();
    let fresh = last_image
        .as_ref()
        .is_none_or(|previous| !std::ptr::eq(previous.as_ptr(), Arc::as_ptr(raw_image)));
    if let Some(admission) = admission_trace.as_mut() {
        emit_claim_trace(
            admission,
            trace_attempt,
            start,
            first_seen,
            raw_image,
            image,
            begin,
            pacing.picture_interval,
            latest,
            recovery,
            s,
            fresh,
            admission_recovering,
            encoding,
            sending,
            encoding_limit,
            rebuilt,
        );
    }
    timings.record_claim_age(fresh, raw_image.repeat, image, begin);
    if Instant::now() >= *metadata_due {
        let checks_started = Instant::now();
        let mut display_metadata = image.gpu.hdr_metadata();
        if let Some(peak) = enhancer.hdr_peak(runtime_config) {
            // RTX HDR generated this picture at its own peak; the
            // SDR monitor's peak does not describe it.
            display_metadata = display_metadata.generated(peak);
        }
        frame_checks += checks_started.elapsed();
        let metadata = m.set_display_hdr_metadata(s, display_metadata);
        active.set_hdr_metadata(metadata);
        // A receiver's report (0x5531) arrives after the stream starts.
        active.set_hdr_target(s.hdr_target_nits());
        *metadata_due = Instant::now() + Duration::from_secs(1);
    }
    let invalidation = s.invalidation.lock().unwrap().take();
    if let Some((first, last)) = invalidation {
        if !active.invalidate_ref_frames(first, last) {
            s.request_recovery_keyframe();
        } else if active.needs_recovery_confirmation() {
            // A decoder that cannot follow the long-term
            // reference stays frozen, and Moonlight does not
            // ask again: a keyframe a second later bounds that.
            confirm_keyframe.get_or_insert(Instant::now() + RFI_CONFIRM_KEYFRAME);
        }
    }
    if confirm_keyframe.is_some_and(|at| Instant::now() >= at) {
        s.request_confirming_keyframe();
    }
    let idr = s.idr.swap(false, Ordering::AcqRel);
    if idr {
        *confirm_keyframe = None;
    }
    let recovering = idr || invalidation.is_some();
    // A client-chosen FEC share moves the encoder's part of the
    // budget at a fence (an IDR or a new encoder) or after a
    // statistical FEC report; a new rate needs no keyframe.
    let bitrate = s.encoder_bitrate(idr || rebuilt);
    apply_frame_cap(
        s,
        c,
        active,
        peer,
        period,
        bitrate,
        &inline_sender.link_watch,
        recovery_packetizer,
        fec_whole,
        fec_whole_due,
        &mut frame_checks,
    );
    checks.record(frame_checks);
    // The device draws the pointer: only Desktop Duplication supplies
    // it apart, so with any other capture it stays in the video and the
    // device is told to hide its own.
    let leave_pointer_out = s.client_cursor.active()
        && {
            let ddx = backend == "ddx";
            let pointer = image.pointer().filter(|_| ddx).map(|p| {
                butterpollo_input_policy::client_cursor::Pointer {
                    width: p.width,
                    height: p.height,
                    hotspot: p.hotspot,
                    logic: p.logic,
                    pixels: p.pixels,
                }
            });
            if s.client_cursor
                .note(pointer, &image.gpu.display.display_name)
            {
                let _ = m.control_wake.send(&[1]);
            }
            if !ddx && !std::mem::replace(pointer_warned, true) {
                tracing::warn!(
                    capture = backend,
                    "the device-drawn cursor needs Desktop Duplication capture; the pointer stays in the video"
                );
            }
            ddx
        };
    let pointer_toggled = leave_pointer_out != *pointer_left_out;
    *pointer_left_out = leave_pointer_out;
    // A repeat with a new panel, or with the pointer put in or left out,
    // is a new picture.
    active.set_repeat((!fresh || raw_image.repeat) && !menu.dirty && !pointer_toggled);
    menu.dirty = false;
    // A map that failed on a frame turned focus off in
    // the encoder; keep it off for the stream, loudly.
    if let Some(error) = active.take_focus_failure() {
        focus.failed(&error, &s.encoder_focus_state, &s.launch.warnings);
    }
    focus.update(&s.encoder_focus, &s.encoder_focus_state, &s.launch.warnings);
    let wanted = focus.settings(|| {
        butterpollo_win_graphics::encoder::focus_cursor(image, (stream.width, stream.height))
    });
    if let Err(error) = focus.apply(wanted, |settings| {
        active.set_focus_verify(focus_verify);
        active.set_focus(settings)
    }) {
        focus.failed(&error, &s.encoder_focus_state, &s.launch.warnings);
        active.set_focus(None)?;
    }
    let mut modified_image = None;
    let presented_image = if !fresh || leave_pointer_out {
        let copy = modified_image.insert(image.as_ref().clone());
        if !fresh {
            copy.captured = Instant::now();
        }
        if leave_pointer_out {
            copy.leave_pointer_out();
        }
        &*copy
    } else {
        image.as_ref()
    };
    progress.mark(crate::stall_watch::Phase::Encode);
    let encode_trace_started = trace_send.then(Instant::now);
    let encoded = (|| -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>> {
        #[cfg(debug_assertions)]
        crate::soak_fault::check("encoder failure")?;
        #[cfg(debug_assertions)]
        crate::soak_fault::check("DXGI_ERROR_DEVICE_REMOVED")?;
        Ok(if direct_encoder_input {
            active.encode_gpu(presented_image, idr, bitrate)?
        } else {
            active.encode(&readback(presented_image)?, idr, bitrate)?
        })
    })();
    if let Some(at) = encode_trace_started {
        tracing::trace!(
            target: "pacing",
            stream_id = %s.launch.id,
            attempt = *trace_attempt,
            host_acquired = ?image.acquired,
            submitted_presentation = ?presented_image.captured,
            encode_call_us = at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
            submitted = active
                .last_submitted()
                .filter(|submitted| *submitted >= at)
                .map_or(0, |submitted| submitted.saturating_duration_since(start).as_micros() as u64),
            outputs = encoded.as_ref().map_or(0, Vec::len),
            ok = encoded.is_ok(),
            idr,
            fresh,
            "encode returned"
        );
    }
    match encoded {
        Ok(output) => Ok(EncodeStep::Encoded {
            output: recovery.output(output, Instant::now)?,
            begin,
            fresh,
            recovering,
            presented_captured: presented_image.captured,
            last_submitted: active.last_submitted(),
        }),
        Err(error) => {
            if let Some(loss) = active
                .device_removed()
                .or_else(|| DeviceLost::d3d11(&image.gpu.device))
                .or_else(|| DeviceLost::from_error(&error))
            {
                recovery.device_loss = Some(loss);
                recovery.record_failure(Instant::now());
                counted_failures.device_loss(recovery.failure_epoch);
                return Ok(EncodeStep::Continue);
            }
            // The first frame at a size the client asked for failed:
            // go back to the mode the stream had, as when the encoder
            // cannot be created at it.
            if let Some((from, _)) = switching.take() {
                rollback_mode_switch(
                    thread,
                    c,
                    prepared,
                    stream,
                    resolution_cap,
                    from,
                    &error,
                    "reconfigure refused: the encoder failed at this mode; keeping the previous one",
                );
                *rebuild_encoder = true;
                return Ok(EncodeStep::Continue);
            }
            // A stalled or reset GPU costs these frames and a
            // keyframe; the rebuild requests it. Only failures
            // that keep coming end the session.
            let since = recovery.record_failure(Instant::now());
            if since.elapsed() >= ENCODER_RECOVERY {
                return Err(error.context(EncoderFailure("the encoder kept failing")));
            }
            s.launch.warnings.set("encoder_recovery", format!("Encoding failed ({error:#}); recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats."));
            active.log_stall();
            *rebuild_encoder = true;
            Ok(EncodeStep::Continue)
        }
    }
}
