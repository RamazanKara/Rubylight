//! Capture admission waits, backpressure handling, and post-encode pacing/phase-lock
//! updates for the video session loop.

use super::super::*;
use super::inline_send::{InlineSender, SendContext};

/// Result of checking arrival-pacing, repeat deferral, and encoder/sender backpressure for
/// a captured picture.
pub(super) enum Admission {
    /// Picture is admitted for enhancement and encoding.
    Proceed {
        recovering: bool,
        encoding: usize,
        sending: usize,
        encoding_limit: usize,
    },
    /// Picture was deferred or backpressure was waited on; continue the session loop.
    Wait,
}

/// Waits on the fixed grid cadence before polling capture when arrival pacing is disabled.
#[allow(clippy::too_many_arguments)]
pub(super) fn wait_grid_cadence(
    ctx: &SendContext<'_>,
    cadence: &butterpollo_media_policy::stream_policy::Cadence,
    encoder: &mut Option<Encoder>,
    recovery: &mut EncoderRecovery,
    inline_sender: &mut InlineSender,
    peer: SocketAddr,
    latest: &Source,
    admission_trace: &mut Option<AdmissionTrace>,
) -> Result<()> {
    let due = cadence.deadline();
    if Instant::now() >= due {
        return Ok(());
    }
    AdmissionTrace::measure(admission_trace, AdmissionWait::Grid, || -> Result<()> {
        while Instant::now() < due {
            // A poll can block for the encoder's 1 ms query
            // timeout; close to the grid tick it would overshoot.
            if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                && poll_fits(encoder.as_ref(), Instant::now(), due)
            {
                let poll_tick = Instant::now();
                let output = recovery.collect(encoder, &ctx.s.launch.warnings, Instant::now)?;
                inline_sender.send_frames(ctx, output, peer, Duration::ZERO, latest, poll_tick)?;
                if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                    let (until, output) = output_wait(encoder.as_ref(), Instant::now(), due);
                    ctx.timer.until_or_event(until, output)?;
                }
            } else {
                ctx.timer.until(due);
            }
        }
        Ok(())
    })
}

/// Runs arrival-pacing deferral, repeat throttling, and encoder/sender backpressure checks.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_admission(
    ctx: &SendContext<'_>,
    encoder: &mut Option<Encoder>,
    recovery: &mut EncoderRecovery,
    inline_sender: &mut InlineSender,
    pacer: &mut butterpollo_media_policy::stream_policy::Pacer,
    menu: &mut crate::couch_menu::VideoOverlay,
    latest: &Source,
    capture_wake: &capture::Consumer,
    raw_image: &Arc<GpuImage>,
    last_image: &Option<std::sync::Weak<GpuImage>>,
    stream: &butterpollo_media_policy::rtsp::Negotiated,
    rates: &butterpollo_media_policy::reconfigure::Rates,
    pacing: &capture::PacingSnapshot,
    arrival_pacing: bool,
    rebuild_encoder: bool,
    encoded_at: Instant,
    peer: SocketAddr,
    capture_tick: Instant,
    first_seen: &mut Option<(usize, Instant, Option<Duration>, Option<Instant>)>,
    admission_trace: &mut Option<AdmissionTrace>,
) -> Result<Admission> {
    let s = ctx.s;
    let period = ctx.stream_period;
    let fresh = last_image
        .as_ref()
        .is_none_or(|previous| !std::ptr::eq(previous.as_ptr(), Arc::as_ptr(raw_image)));
    let pace_candidate =
        arrival_pacing && fresh && !rebuild_encoder && !s.idr.load(Ordering::Acquire);
    // Read out first: the guard must not live through the wait
    // below, or it blocks `request_invalidation` on the control
    // thread.
    let invalidation_clear = !pace_candidate || s.invalidation.lock().unwrap().is_none();
    if pace_candidate
        && invalidation_clear
        && let interval = pacing.picture_interval
        && let butterpollo_media_policy::stream_policy::Pace::WaitUntil(deadline) =
            pacer.decide(Instant::now(), raw_image.captured, interval)
    {
        AdmissionTrace::measure(
            admission_trace,
            AdmissionWait::FreshRate,
            || -> Result<()> {
                let key = Arc::as_ptr(raw_image) as usize;
                if first_seen.is_none_or(|(seen, ..)| seen != key) {
                    *first_seen = Some((key, Instant::now(), interval, Some(deadline)));
                }
                // A poll can block for the encoder's 1 ms query
                // timeout; close to the claim it would overshoot.
                if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                    && poll_fits(encoder.as_ref(), Instant::now(), deadline)
                {
                    let output = recovery.collect(encoder, &s.launch.warnings, Instant::now)?;
                    inline_sender.send_frames(
                        ctx,
                        output,
                        peer,
                        Duration::ZERO,
                        latest,
                        capture_tick,
                    )?;
                }
                let now = Instant::now();
                let (until, output) = if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                    && poll_fits(encoder.as_ref(), now, deadline)
                {
                    output_wait(encoder.as_ref(), now, deadline)
                } else {
                    (deadline, None)
                };
                // The claim itself is met precisely; a poll for the
                // encoder's output is not worth spinning for.
                if until == deadline && output.is_none() {
                    latest.wait_if_current_precise(ctx.timer, capture_wake, raw_image, until)?;
                } else {
                    latest.wait_if_current_or(ctx.timer, capture_wake, raw_image, until, output)?;
                }
                Ok(())
            },
        )?;
        return Ok(Admission::Wait);
    }
    let repeat_due = if arrival_pacing {
        pacer.repeat_deadline(
            encoded_at + rates.static_period,
            raw_image.captured,
            pacing.source_interval,
        )
    } else {
        encoded_at + rates.static_period
    };
    // A recovery request on a moving picture rides the
    // next new frame; only a still one is encoded again
    // at once. Grid pacing serves it at the next slot.
    // A changed menu panel is encoded at once, like a recovery.
    menu.poll(&s.couch_menu, [stream.width, stream.height]);
    let recovering =
        menu.dirty || s.idr.load(Ordering::Acquire) || s.invalidation.lock().unwrap().is_some();
    // A still resend spends a claim's credit, so under arrival
    // pacing it waits as a claim would: on its slot grid a late
    // resend and an on-time one came closer than the spacing.
    let reencode_at = if !recovering {
        if arrival_pacing {
            pacer.resend_at(repeat_due, Instant::now())
        } else {
            repeat_due
        }
    } else if arrival_pacing {
        butterpollo_media_policy::stream_policy::reencode_at(
            true,
            raw_image.captured,
            period,
            pacing.source_interval,
            repeat_due,
        )
    } else {
        Instant::now()
    };
    if !rebuild_encoder
        && (s.config.vrr_low_latency || rates.limit_static_rate || arrival_pacing)
        && last_image
            .as_ref()
            .is_some_and(|previous| std::ptr::eq(previous.as_ptr(), Arc::as_ptr(raw_image)))
        && Instant::now() < reencode_at
    {
        AdmissionTrace::measure(admission_trace, AdmissionWait::Repeat, || -> Result<()> {
            if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                let output = recovery.collect(encoder, &s.launch.warnings, Instant::now)?;
                inline_sender.send_frames(
                    ctx,
                    output,
                    peer,
                    Duration::ZERO,
                    latest,
                    capture_tick,
                )?;
            }
            let now = Instant::now();
            let cap = reencode_at.min(now + period);
            let (until, output) = if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                output_wait(encoder.as_ref(), now, cap)
            } else {
                (cap, None)
            };
            latest.wait_if_current_or(ctx.timer, capture_wake, raw_image, until, output)?;
            Ok(())
        })?;
        return Ok(Admission::Wait);
    }
    // A picture claimed while the encoder is behind only
    // waits in its queue: take its output first, then
    // claim the newest picture.
    // A frame still being sent on a paced link counts too.
    // A frame waiting behind it is skipped on a keyframe
    // request: the client would discard it, and the
    // keyframe would wait for it.
    let recovery_lead = encoder
        .as_ref()
        .and_then(Encoder::recovery_lead)
        .map(|lead| lead.min(period));
    let sending = match inline_sender.video_sender.get() {
        Some(sender) => {
            if s.idr.load(Ordering::Acquire) {
                sender.skip_pending()?;
            }
            // Claim the newest picture near the end of an IDR,
            // only when this encoder demonstrated a safe lead.
            sender.claim_backlog(recovery_lead)?
        }
        None => 0,
    };
    let encoding = encoder.as_ref().map_or(0, |e| recovery.backlog(e));
    let encoding_limit = encoder
        .as_ref()
        .map_or(encoder_backlog::DEFAULT_LIMIT, |encoder| {
            if inline_sender.video_sender.get().is_some() {
                encoder.recovery_backlog_limit()
            } else {
                EncoderOutput::backlog_limit(encoder)
            }
        });
    let pressure = encoder_backlog::backpressure(encoding, sending, encoding_limit);
    if !rebuild_encoder && pressure != Backpressure::None {
        if pressure == Backpressure::Sender {
            AdmissionTrace::measure(admission_trace, AdmissionWait::Sender, || -> Result<()> {
                // Network occupancy must not start encoder stall recovery.
                recovery.backlog_since = None;
                if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                    let output = recovery.collect(encoder, &s.launch.warnings, Instant::now)?;
                    let idle = output.is_empty();
                    inline_sender.send_frames(
                        ctx,
                        output,
                        peer,
                        Duration::ZERO,
                        latest,
                        capture_tick,
                    )?;
                    if idle {
                        wait_for_output(ctx.timer, recovery, encoder.as_ref())?;
                    }
                } else {
                    inline_sender
                        .video_sender
                        .get()
                        .unwrap()
                        .wait_for_capacity(
                            encoder_backlog::total_limit(encoding_limit),
                            period,
                            recovery_lead,
                        )?;
                }
                Ok(())
            })?;
            return Ok(Admission::Wait);
        }
        // An encoder that returns nothing for 250 ms is
        // recreated, as a queue that never drained was;
        // a shorter stall on a saturated GPU only makes the
        // stream choppy. A recreated one that is still
        // silent gets longer each time.
        AdmissionTrace::measure(admission_trace, AdmissionWait::Encoder, || -> Result<()> {
            if let Some(output) = recovery.poll_full(encoder, &s.launch.warnings, Instant::now)? {
                let idle = output.is_empty();
                inline_sender.send_frames(
                    ctx,
                    output,
                    peer,
                    Duration::ZERO,
                    latest,
                    capture_tick,
                )?;
                if idle {
                    wait_for_output(ctx.timer, recovery, encoder.as_ref())?;
                }
            }
            Ok(())
        })?;
        return Ok(Admission::Wait);
    }
    recovery.backlog_since = None;
    Ok(Admission::Proceed {
        recovering,
        encoding,
        sending,
        encoding_limit,
    })
}

/// Updates resend anchor, arrival-pacing or fixed-cadence phase lock, and the capture claim
/// grid after a frame is submitted to the encoder.
#[allow(clippy::too_many_arguments)]
pub(super) fn advance_pacing(
    s: &Session,
    latest: &Source,
    capture_wake: &Arc<capture::Consumer>,
    pacer: &mut butterpollo_media_policy::stream_policy::Pacer,
    cadence: &mut butterpollo_media_policy::stream_policy::Cadence,
    phase_locked: &mut bool,
    grid_period: &mut Duration,
    encoded_at: &mut Instant,
    begin: Instant,
    period: Duration,
    static_period: Duration,
    arrival_pacing: bool,
    vrr: bool,
    fresh: bool,
    recovering: bool,
    pacing: &capture::PacingSnapshot,
) {
    // Repeat deadlines start at submission, so encoder work
    // does not extend the interval between static frames.
    // A static resend stays on its grid with either pacing, or
    // the wake-up latency of every resend adds up to a slower
    // stream; a new picture or a recovery re-anchors here.
    *encoded_at = butterpollo_media_policy::stream_policy::resend_anchor(
        *encoded_at,
        begin,
        static_period,
        fresh,
        recovering,
    );
    if arrival_pacing {
        // New pictures and static repeats count toward the
        // stream rate: a repeat that spent nothing let the
        // game's next frame follow it at once, so VRR
        // streams went past the rate whenever a frame came
        // late. An unchanged picture encoded again for the
        // client's recovery request does not, or a client
        // losing packets left every game frame late.
        if butterpollo_media_policy::stream_policy::counts_toward_rate(fresh, recovering) {
            // Bank and spend credit at the period that was in force up to `begin`
            // before applying any Phase Lock period step for the next interval.
            pacer.claimed(begin);
            // Phase lock: holding frames here would only move the
            // wait from the client to the host, so the lock sets
            // the rate cap to the client's refresh instead. VRR
            // displays follow each frame and need no lock.
            match (!vrr)
                .then(|| {
                    phase_interval(
                        s,
                        begin,
                        period,
                        pacing.picture_interval,
                        pacing.picture_mean,
                    )
                })
                .flatten()
            {
                Some(interval) => {
                    *phase_locked = true;
                    *grid_period = interval;
                    pacer.set_period(begin, interval);
                }
                None => {
                    if std::mem::take(phase_locked) {
                        *grid_period = period;
                        pacer.set_period(begin, period);
                    }
                }
            }
        }
        latest.update_claim_grid(capture_wake, *grid_period, pacer.allowed_at(begin));
    } else {
        // Phase lock: the grid follows the client's refresh, at a
        // phase that gets frames there just before its latch.
        // Without reports this is the original fixed-period path.
        match phase_interval(s, begin, period, None, None) {
            Some(interval) => {
                cadence.submitted_after(begin, interval);
                *phase_locked = true;
                *grid_period = interval;
            }
            None => {
                cadence.submitted(begin);
                if std::mem::take(phase_locked) {
                    *grid_period = period;
                }
            }
        }
        latest.update_claim_grid(capture_wake, *grid_period, cadence.deadline());
    }
}
