//! The session loop: pacing and admission of captured pictures, encoder recovery and
//! mode switches, the 5 s stream timings, and the video send (inline, or handed to the
//! sender thread).

mod admission;
mod encode;
mod inline_send;
mod rebuild;
mod timings;

use super::setup::{Captured, Configured, Started, StreamDisplay};
use super::*;
use admission::{Admission, advance_pacing, check_admission, wait_grid_cadence};
use encode::{EncodeStep, encode_frame, sync_stream_output};
use inline_send::{InlineSender, SendContext};
use rebuild::{RebuildOutcome, apply_pending_reconfigure, rebuild_encoder_now};
use timings::TimingsWindow;

// The moved code still borrows `m`, `h`, `s` and the other values it now receives by
// reference, as it did when it owned them.
#[allow(clippy::needless_borrow)]
impl SessionThread<'_> {
    /// Streams until the session stops or fails, continuing the setup phases' values.
    /// They are bound in the order the phases created them, so they drop in reverse.
    pub(super) fn run(
        &self,
        encoder: &mut Option<Encoder>,
        workers: &Workers,
        config: Configured,
        display: StreamDisplay,
        capture: Captured<'_>,
        media: Started,
    ) -> Result<()> {
        let SessionThread { m, h, s } = *self;
        let Workers {
            pyrowave_sender,
            webrtc_sender,
            ..
        } = workers;
        let Configured {
            mut config_version,
            c,
            timer,
        } = config;
        let StreamDisplay {
            prepared,
            mut output,
        } = display;
        let Captured {
            latest,
            capture_wake,
            mut focus,
            focus_verify,
            gpu_adapter,
        } = capture;
        let mut staging = None;
        let Started {
            automatic_pacing,
            packetizer,
            next_wire_frame,
            claims,
            start,
            present_stamper,
        } = media;
        let mut recovery_packetizer = packetizer.clone();
        let trace_send = tracing::enabled!(target: "pacing", tracing::Level::TRACE);
        let mut inline_sender = InlineSender::new(
            &s,
            &c,
            encoder,
            workers,
            automatic_pacing,
            packetizer,
            next_wire_frame,
            start,
            present_stamper,
            trace_send,
        );
        // The stream's size and rate. A client whose display changes can ask
        // for others mid-stream (0x5532); the encoder is rebuilt from this copy.
        // The host's `max_stream_resolution` cap shrinks only the coded picture.
        let mut stream = s.effective_config();
        let resolution_cap = c.max_stream_resolution();
        let mut rates = butterpollo_media_policy::reconfigure::Rates::new(
            &stream,
            config::minimum_fps_target(&c),
        );
        // A constant-rate capture publishes for its fastest stream: this one's
        // stream rate, shared with any other stream on the same capture.
        capture_wake.set_period(rates.period);
        // The rate pacing follows, and the period the send path reads.
        let mut paced_rate = stream.fps_millihz();
        let mut stream_period = rates.period;
        // A mode change being applied: the mode before it, and since when.
        let mut switching: Option<(butterpollo_media_policy::reconfigure::Mode, Instant)> = None;
        let mut cadence = butterpollo_media_policy::stream_policy::Cadence::new(
            Instant::now(),
            rates.period,
            config::wgc_pacing_smoothing(&c),
        );
        // VRR claims each frame as it arrives, but no faster than the
        // stream rate. The encoder budgets every frame from that rate,
        // and the VRR virtual display runs at 1000 Hz: uncapped, a game
        // or desktop faster than the stream starved every frame of
        // bits (a blurry picture, fringed text) and flooded the link.
        // Predictive waits apply to a source whose frames are at least
        // 2 ms apart (a 120 or 240 Hz game on the 1000 Hz display
        // is claimed as it arrives, not a quarter period late); a
        // source at the display's own 1000 Hz is claimed on arrival.
        // A stable surplus of WGC compositions can also be aligned
        // with the game's updates.
        let vrr = s.config.vrr_low_latency;
        let arrival_pacing = vrr
            || butterpollo_media_policy::stream_policy::Pacing::from_config(&c)
                == butterpollo_media_policy::stream_policy::Pacing::Arrival;
        let source_phase_eligible = vrr || config::frame_pacing_predictive(&c);
        // Read once: the setting is the session's, and a lookup a frame costs.
        let direct_encoder_input = config::wgc_direct_encoder_input(&c);
        let new_pacer = |rates: &butterpollo_media_policy::reconfigure::Rates| {
            let predictive = config::frame_pacing_predictive(&c);
            butterpollo_media_policy::stream_policy::Pacer::new(Instant::now(), rates.period)
                .with_static_period(rates.static_period)
                .with_prediction(predictive)
                .with_prediction_floor(if vrr {
                    Duration::from_millis(2)
                } else {
                    Duration::ZERO
                })
                // Source phase follows the opened backend, read again after
                // a capture recovery.
                .with_source_phase(false)
                .with_spacing(if vrr { 0.5 } else { 0.75 })
        };
        let mut pacer = new_pacer(&rates);
        // The claim grid's period from before a phase lock, restored when it ends.
        let mut unlocked_grid_period: Option<Duration> = None;
        let mut live_at = cadence.deadline();
        let mut rebuild_encoder = false;
        let mut applied_luminance: Option<(f32, f32)> = None;
        let mut hdr_fallback = butterpollo_media_policy::hdr::HdrFallback::default();
        if stream.hdr && encoder.is_some() {
            hdr_fallback.note_hdr_active();
        }
        // Where this thread is, should it stop making progress.
        let progress = crate::stall_watch::register(
            "session",
            &s.launch.id,
            Some(crate::stall_watch::GpuProbe::start(&output, &s.launch.id)),
        );
        progress.set_period(rates.period);
        // Since when encoding has failed without a frame getting through.
        let mut recovery = EncoderRecovery {
            progress: Some(progress.clone()),
            queries: trace_send.then(Vec::new),
            ..Default::default()
        };
        // Encoder failures in this session, each counted once, and whether
        // they moved conversion off the compute queue.
        let mut compute_demotion = ComputeDemotion::default();
        if encoder
            .as_ref()
            .is_some_and(|encoder| encoder.compute_fell_back())
        {
            compute_demotion.demote_sticky(Instant::now());
        }
        // The encoder failure events already counted towards
        // [`ComputeDemotion`], so a rebuild that does not follow a new failure
        // does not count the same one again.
        let mut counted_failures = CountedFailures::default();
        // Whether the last encoder made converted on a compute queue: a
        // failing one is dropped before its failure is counted.
        let mut converting_on_compute = encoder
            .as_ref()
            .is_some_and(|encoder| encoder.converts_on_compute());
        // Whether that compute conversion was NVENC's D3D12 session, the
        // unverified path that a failed session leaves for good.
        let mut nvenc_on_compute = encoder
            .as_ref()
            .is_some_and(|encoder| encoder.converts_on_compute() && encoder.backend() == "nvenc");
        // AMF's video converter refused a long run of frames for a lasting reason: the rest of
        // the session encodes without it, converting on the compute queue.
        let mut vpe_gave_way = false;
        let mut base_config = c.clone();
        let mut runtime_config = c.clone();
        // The runtime configuration's revision, raised whenever it is
        // replaced: the enhancement choice below is parsed again only then,
        // not for every picture.
        let mut runtime_revision = 0_u64;
        // The capture backend, read once and again only after a recovery:
        // every backend change goes through a capture reset, and the worker
        // writes the backend it opened before the capture's first new
        // picture. Whether claims wait for the capture's learned phase
        // follows the backend, and its setting is the session's.
        let mut backend = latest.backend();
        let mut backend_due = false;
        let mut source_phase = source_phase_eligible
            && butterpollo_media_policy::stream_policy::source_phase(&c, backend);
        let mut calibration_due = s
            .launch
            .client
            .extra
            .get("hdr_profile")
            .and_then(serde_json::Value::as_str)
            .filter(|profile| !profile.is_empty())
            .map(|_| Instant::now() + Duration::from_secs(30));
        // The fullscreen game on this display, for quitting a game
        // that a store client started outside the app's processes.
        let mut quit_scan: Option<butterpollo_win_graphics::foreground::Tracker> = None;
        let mut quit_scan_due = Instant::now() + Duration::from_secs(1);
        let _background_windows =
            if s.launch.role == Role::Stream && butterpollo_session::background::enabled(&c) {
                background::GameWindows::start(h.clone(), s.clone())
                    .inspect_err(|error| {
                        tracing::warn!(%error, "background stream window placement unavailable")
                    })
                    .ok()
            } else {
                None
            };
        // The first picture updates HDR metadata after the enhancer has chosen
        // its conversion: RTX HDR's peak comes from that selection, not only
        // from the configuration the encoder was made with.
        let mut metadata_due = Instant::now();
        // A keyframe a while after an AMF reference-invalidation recovery.
        let mut confirm_keyframe: Option<Instant> = None;
        // Pipeline frame rates: the capture slot's counts this stream
        // has taken, and the 5 s timing log's own window.
        let mut pipeline_follow = butterpollo_media_policy::pipeline_rates::Follow::default();
        let mut timings = TimingsWindow::new(&s, stream_period, gpu_adapter);
        // Identity only: a weak reference releases the previous capture's
        // texture and ring slot once it is encoded, while the live `Weak`
        // still pins the allocation so its address can't be reused by a new
        // `GpuImage` and compare equal.
        let mut last_image: Option<std::sync::Weak<GpuImage>> = None;
        // RTX HDR or RTX Video enhancement; a no-op without the NVIDIA
        // component or when neither runs.
        let mut enhancer = crate::nvidia_enhance::Enhancer::default();
        // The couch menu's panel; one atomic load a frame while closed.
        let mut menu = crate::couch_menu::VideoOverlay::default();
        // Frame Warp's passes over captures, for a client that asked (0x5543).
        let mut frame_warp = crate::stream::frame_warp::VideoFrameWarp::default();
        // The device draws the pointer (0x5540): it is left out of the video.
        let mut pointer_left_out = false;
        let mut pointer_warned = false;
        let mut encoded_at = Instant::now();
        // The first pacing decision for the newest fresh frame, kept
        // for the per-claim trace.
        let mut first_seen: Option<(usize, Instant, Option<Duration>, Option<Instant>)> = None;
        let mut video_qos = crate::stream::qos::Tagged::default();
        let mut admission_trace = trace_send.then(AdmissionTrace::default);
        let mut trace_attempt = 0_u64;
        // The checks the encode loop still makes a frame (display
        // colour and route reads), for the timing log.
        let mut checks = crate::video_send::link_watch::CheckTimes::default();
        // The largest frame with full parity, for the FEC percentage,
        // packet size and FEC choice it was found at.
        let mut fec_whole = ((usize::MAX, 0, false), 0);
        let mut fec_whole_due = Instant::now();

        let send_ctx = |stream_period| SendContext {
            m,
            h,
            s,
            c: &c,
            timer: &timer,
            progress: &progress,
            prepared: &prepared,
            claims: &claims,
            pyrowave_sender,
            webrtc_sender,
            stream_period,
        };

        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
            progress.mark(crate::stall_watch::Phase::Loop);
            if let Some(sender) = inline_sender.video_sender.get() {
                sender.backlog()?;
            }
            if let Some(loss) = recovery.device_loss.take() {
                // A GPU reset or device loss is not the compute path's
                // fault, so its failure event never counts against it.
                counted_failures.device_loss(recovery.failure_epoch);
                latest
                    .latest
                    .device_lost(loss, &latest.warnings, Instant::now())?;
                *encoder = None;
            }
            if Instant::now() >= live_at {
                live_at = Instant::now() + Duration::from_millis(250);
                let version = h.config_version.load(Ordering::Acquire);
                if version != config_version
                    || calibration_due.is_some_and(|due| Instant::now() >= due)
                {
                    base_config = effective_config(&h, &s.launch)?;
                    if s.config.vrr_low_latency {
                        base_config
                            .values
                            .insert("wgc_slot_aligned_publish".into(), "false".into());
                    }
                    config_version = version;
                    runtime_config = base_config.clone();
                    runtime_revision = runtime_revision.wrapping_add(1);
                    if let Some(due) = &mut calibration_due {
                        *due = Instant::now() + Duration::from_secs(30);
                    }
                }
                sync_stream_output(s, &prepared, latest, &mut output);
                timings.live_tick(
                    h,
                    s,
                    &prepared,
                    &output,
                    &stream,
                    stream_period,
                    backend,
                    encoder,
                    &focus,
                    &mut checks,
                    &inline_sender.link_watch,
                    webrtc_sender,
                );
            }
            // The client's display changed and it asked for another size or
            // rate (0x5532). Its latest request is applied here, between
            // frames: the encoder is rebuilt at the new size and starts it
            // with a keyframe. The display keeps its mode, and the colour
            // conversion scales its picture to the new size, unless the
            // stream turned and the device's display follows it
            // (dd_rotation_option = auto): the heartbeat reshapes or
            // turns it then, off this loop.
            apply_pending_reconfigure(
                self,
                &c,
                &runtime_config,
                &prepared,
                &mut stream,
                resolution_cap,
                &mut switching,
                &mut rebuild_encoder,
            );
            // Pacing follows the stream's rate: a new rate starts a new
            // cadence, and a phase lock locks again at it.
            if stream.fps_millihz() != paced_rate {
                paced_rate = stream.fps_millihz();
                rates = butterpollo_media_policy::reconfigure::Rates::new(
                    &stream,
                    config::minimum_fps_target(&c),
                );
                capture_wake.set_period(rates.period);
                stream_period = rates.period;
                progress.set_period(rates.period);
                cadence = butterpollo_media_policy::stream_policy::Cadence::new(
                    Instant::now(),
                    rates.period,
                    config::wgc_pacing_smoothing(&c),
                );
                pacer = new_pacer(&rates);
                unlocked_grid_period = None;
                {
                    let mut grid = latest.grid.lock().unwrap();
                    grid.period = rates.period;
                    grid.anchor = Instant::now();
                }
                s.phase_sync.lock().unwrap().reset();
            }
            let period = stream_period;
            latest.check()?;
            if latest.needs_release(&capture_wake) {
                release_capture_resources(
                    encoder,
                    &mut last_image,
                    &mut staging,
                    &mut enhancer,
                    &mut frame_warp,
                    &mut pacer,
                    s,
                    latest,
                    &capture_wake,
                    &mut backend_due,
                );
                continue;
            }
            // A WebRTC client has no video ping: its address is the pair ICE chose.
            let peer = match webrtc_sender {
                Some(sender) => sender.peer(),
                None => m.peers.lock().unwrap().get(&s.launch.id, false),
            };
            let Some(peer) = peer else {
                // A roam drops the old path's address until the new one pings.
                if s.media_wait_start(start).elapsed() > crate::network::ping_timeout(&c) {
                    anyhow::bail!("client video ping timed out");
                }
                timer.until(Instant::now() + Duration::from_millis(1));
                continue;
            };
            if s.config.video_qos {
                video_qos.follow(&m.video, peer, false);
            }
            let ctx = send_ctx(period);
            if !arrival_pacing {
                wait_grid_cadence(
                    &ctx,
                    &cadence,
                    encoder,
                    &mut recovery,
                    &mut inline_sender,
                    peer,
                    latest,
                    &mut admission_trace,
                )?;
            }
            progress.mark(crate::stall_watch::Phase::Picture);
            let capture_tick = Instant::now();
            let image =
                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Capture, || {
                    latest.wait_for_frame(
                        &timer,
                        &capture_wake,
                        Instant::now() + period.min(Duration::from_millis(50)),
                    )
                })?;
            let Some(mut raw_image) = image else {
                release_capture_resources(
                    encoder,
                    &mut last_image,
                    &mut staging,
                    &mut enhancer,
                    &mut frame_warp,
                    &mut pacer,
                    s,
                    latest,
                    &capture_wake,
                    &mut backend_due,
                );
                continue;
            };
            if backend_due {
                // The capture reopened: the worker has named the
                // backend it opened before this, its first new picture.
                backend = latest.backend();
                backend_due = false;
                sync_stream_output(s, &prepared, latest, &mut output);
                source_phase = source_phase_eligible
                    && butterpollo_media_policy::stream_policy::source_phase(&c, backend);
            }
            // Whether this capture only extends the focus window's
            // picture, read before an enhancement replaces the image and
            // a later capture can take its place (`None` then).
            let extends_capture = latest.extends_capture(&raw_image);
            let checks_started = Instant::now();
            require_hdr_source(&s, || raw_image.gpu.display_in_hdr(), true)?;
            let frame_checks = checks_started.elapsed();
            // Check whether the enhancement choice or device changed before
            // pacing, repeat deferral and backpressure, without running GPU
            // inference on frames that will be deferred or dropped.
            rebuild_encoder |=
                enhancer.will_change(&s, &runtime_config, runtime_revision, &raw_image);
            // Resolution changes or DXGI loss can recreate the capture device.
            rebuild_encoder |= encoder
                .as_ref()
                .is_none_or(|encoder| !encoder.accepts_gpu_device(&raw_image));
            if !vpe_gave_way
                && encoder
                    .as_ref()
                    .is_some_and(|encoder| encoder.converter_gave_way())
            {
                vpe_gave_way = true;
                rebuild_encoder = true;
                tracing::info!(
                    client = %s.launch.client.name,
                    "amd_vpe: the converter refused every frame for a lasting reason; rebuilding the encoder without it for the rest of the session"
                );
            }
            if compute_demotion.demoted() && compute_demotion.retry_due(Instant::now()) {
                rebuild_encoder = true;
                s.launch.warnings.clear("encoder_compute_recovery");
                tracing::info!(
                    client = %s.launch.client.name,
                    quiet_secs = COMPUTE_RETRY_QUIET.as_secs(),
                    "encoder conversion returns to the compute queue: no encoder failure since it moved to the graphics queue"
                );
            }
            if rebuild_encoder {
                pacer.reset_source_phase();
            }
            // A reopen can change the backend, e.g. to WGC after the
            // driver's frame push failed; the cached value is read again
            // once the capture recovers.
            pacer.set_source_phase(source_phase);
            // One read of the capture's pacing fields for every decision
            // this frame makes from them.
            let pacing = latest.pacing_snapshot();
            if arrival_pacing {
                // Another window's update next to the focus window's
                // picture is no update of the source: learned as one, a
                // window composed at 240 Hz beside a 60 Hz game could
                // move the phase onto its compositions, and each game
                // frame then waited for the next of them. A capture
                // replaced before it was read is left out.
                if extends_capture == Some(false) {
                    pacer.observe_source(raw_image.captured);
                }
                pacer.observe_source_rate(pacing.source_rate);
                pacer.observe_picture(pacing.picture_start, pacing.late_picture_interval);
                pacer.observe_extension(pacing.extends_picture);
            }
            let (admission_recovering, encoding, sending, encoding_limit) = match check_admission(
                &ctx,
                encoder,
                &mut recovery,
                &mut inline_sender,
                &mut pacer,
                &mut menu,
                latest,
                &capture_wake,
                &raw_image,
                &last_image,
                &stream,
                &rates,
                &pacing,
                arrival_pacing,
                rebuild_encoder,
                encoded_at,
                peer,
                capture_tick,
                &mut first_seen,
                &mut admission_trace,
            )? {
                Admission::Proceed {
                    recovering,
                    encoding,
                    sending,
                    encoding_limit,
                } => (recovering, encoding, sending, encoding_limit),
                Admission::Wait => continue,
            };
            if s.launch.role == Role::Stream && Instant::now() >= quit_scan_due {
                quit_scan_due = Instant::now() + Duration::from_secs(1);
                // The scan runs on its own thread; this reads its last result.
                if let Some((program, pid)) = quit_scan
                    .get_or_insert_with(butterpollo_win_graphics::foreground::Tracker::default)
                    .poll_process(&[], &raw_image.gpu.display)
                {
                    progress.mark(crate::stall_watch::Phase::AppTracking);
                    if !observe_foreground_game(&h, &s.launch.client.uuid, &program, pid) {
                        quit_scan_due = Instant::now() + QUIT_SCAN_RETRY;
                    }
                    progress.mark(crate::stall_watch::Phase::Picture);
                }
            }
            // The picture the encoder receives: enhanced where the
            // stream runs an enhancement, deferred past pacing and
            // backpressure so dropped or deferred frames run no NGX work.
            if let Some(tuned) = enhancer.tune(&h, &raw_image, &base_config) {
                runtime_config = tuned;
                runtime_revision = runtime_revision.wrapping_add(1);
            }
            let (mut image, enhancement_changed) =
                enhancer.apply(&s, &runtime_config, runtime_revision, raw_image.clone());
            if m.sync_hdr_state(
                &s,
                &mut stream,
                &raw_image,
                &prepared,
                enhancer.hdr_peak(&runtime_config).is_some(),
                encoder.is_none(),
                &mut hdr_fallback,
                Instant::now(),
            ) {
                rebuild_encoder = true;
                pacer.reset_source_phase();
                runtime_revision = runtime_revision.wrapping_add(1);
                applied_luminance = None;
            }
            if enhancement_changed
                || (!rebuild_encoder
                    && encoder
                        .as_ref()
                        .is_none_or(|encoder| !encoder.accepts_gpu_device(&image)))
            {
                rebuild_encoder = true;
                pacer.reset_source_phase();
            }
            let rebuilt = rebuild_encoder;
            if rebuild_encoder {
                match rebuild_encoder_now(
                    self,
                    encoder,
                    &c,
                    &runtime_config,
                    runtime_revision,
                    &prepared,
                    &timer,
                    &progress,
                    latest,
                    &mut stream,
                    resolution_cap,
                    &mut switching,
                    &mut recovery,
                    &mut counted_failures,
                    &mut compute_demotion,
                    &mut converting_on_compute,
                    &mut nvenc_on_compute,
                    vpe_gave_way,
                    &inline_sender,
                    &mut focus,
                    &mut enhancer,
                    &mut pacer,
                    arrival_pacing,
                    &image,
                )? {
                    RebuildOutcome::Ready {
                        raw_image: fresh_raw,
                        image: fresh_img,
                    } => {
                        raw_image = fresh_raw;
                        image = fresh_img;
                        metadata_due = Instant::now();
                        staging = None;
                        rebuild_encoder = false;
                    }
                    RebuildOutcome::Continue => continue,
                }
            }
            let active = encoder
                .as_mut()
                .expect("a missing encoder is rebuilt above");
            let (output, begin, fresh, recovering, presented_captured, last_submitted) =
                match encode_frame(
                    self,
                    &c,
                    &runtime_config,
                    &prepared,
                    &progress,
                    latest,
                    active,
                    |img| img.readback(&mut staging),
                    &mut stream,
                    resolution_cap,
                    &mut switching,
                    &mut rebuild_encoder,
                    &mut applied_luminance,
                    &mut metadata_due,
                    &mut confirm_keyframe,
                    &mut menu,
                    &enhancer,
                    &mut focus,
                    focus_verify,
                    &mut pointer_left_out,
                    &mut pointer_warned,
                    backend,
                    &inline_sender,
                    &mut recovery,
                    &mut counted_failures,
                    &mut recovery_packetizer,
                    &mut fec_whole,
                    &mut fec_whole_due,
                    &mut checks,
                    frame_checks,
                    &mut timings,
                    &mut admission_trace,
                    &mut trace_attempt,
                    trace_send,
                    start,
                    first_seen,
                    &raw_image,
                    &image,
                    &last_image,
                    &pacing,
                    admission_recovering,
                    encoding,
                    sending,
                    encoding_limit,
                    rebuilt,
                    peer,
                    period,
                    direct_encoder_input,
                )? {
                    EncodeStep::Encoded {
                        output,
                        begin,
                        fresh,
                        recovering,
                        presented_captured,
                        last_submitted,
                    } => (
                        output,
                        begin,
                        fresh,
                        recovering,
                        presented_captured,
                        last_submitted,
                    ),
                    EncodeStep::Continue => continue,
                };
            if rebuilt && let Some((from, started)) = switching.take() {
                timings.on_mode_switched(s, from, started, &stream, &image);
            }
            let call_latency = begin.elapsed();
            let (slot, pictures, captures) = latest.frame_counts();
            let (pictures, captures) = pipeline_follow.advance(slot, pictures, captures);
            s.stats.pipeline.captured(pictures, captures);
            // A constant-rate repeat has no new content, so the pipeline
            // reads it as the repeat it is (`capture = wgcc`).
            s.stats.pipeline.submitted(!fresh || raw_image.repeat);
            if fresh && !raw_image.repeat {
                claims
                    .borrow_mut()
                    .push(butterpollo_media_policy::performance::Claim {
                        key: presented_captured,
                        presented: image.captured,
                        acquired: image.acquired,
                        published: image.published,
                        claimed: begin,
                        submitted: last_submitted.filter(|at| *at >= begin).unwrap_or(begin),
                    });
            }
            advance_pacing(
                s,
                latest,
                &mut pacer,
                &mut cadence,
                &mut unlocked_grid_period,
                &mut encoded_at,
                begin,
                period,
                rates.static_period,
                arrival_pacing,
                vrr,
                fresh,
                recovering,
                &pacing,
            );
            last_image = Some(Arc::downgrade(&raw_image));
            // An allocation can later reuse this image's address;
            // its trace observation belongs only to this submission.
            first_seen = None;
            inline_sender.send_frames(&ctx, output, peer, call_latency, latest, capture_tick)?;
            // After the frame went out, off its path: Frame Warp's passes.
            // A constant-rate repeat carries no new content to warp.
            frame_warp.frame(
                h,
                s,
                &image,
                fresh && !raw_image.repeat,
                (stream.width, stream.height),
            );
        }
        Ok(())
    }
}

/// Releases the encoder, held GPU frames/passes, and capture generation so Windows can recreate
/// the capture device cleanly even while waiting on a video peer.
#[allow(clippy::too_many_arguments)]
fn release_capture_resources<S>(
    encoder: &mut Option<Encoder>,
    last_image: &mut Option<std::sync::Weak<GpuImage>>,
    staging: &mut Option<S>,
    enhancer: &mut crate::nvidia_enhance::Enhancer,
    frame_warp: &mut crate::stream::frame_warp::VideoFrameWarp,
    pacer: &mut butterpollo_media_policy::stream_policy::Pacer,
    s: &Session,
    latest: &Arc<Source>,
    capture_wake: &Arc<capture::Consumer>,
    backend_due: &mut bool,
) {
    if let Some(released) = encoder.take() {
        tracing::info!(
            client = %s.launch.client.name,
            in_flight = released.backlog(),
            "releasing the encoder while the capture recovers"
        );
        drop(released);
    }
    *last_image = None;
    *staging = None;
    enhancer.reset();
    frame_warp.reset();
    pacer.reset_source_phase();
    latest.release_generation(capture_wake);
    *backend_due = true;
}
