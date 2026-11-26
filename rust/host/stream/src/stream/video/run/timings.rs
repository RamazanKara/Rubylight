//! Periodic 5-second stream timing summaries, stutter totals, display refresh / source-rate
//! warnings, and pacing claim traces for the video session loop.

use super::super::*;

/// State for the 5-second timing/stutter window and source frame-rate warnings.
pub(super) struct TimingsWindow {
    session: Arc<Session>,
    stream_period: Duration,
    timing_due: Instant,
    pipeline_log: butterpollo_media_policy::pipeline_rates::Sampler,
    source_rate: butterpollo_media_policy::pipeline_rates::SourceRate,
    warned_virtual_display: bool,
    display_refresh_mhz: Option<u32>,
    stutter_totals: butterpollo_media_policy::stutter_report::Totals,
    claim_ages: Vec<(u64, u64)>,
    wgc_stamp_ages: Vec<f64>,
    timings_logged: bool,
}

impl TimingsWindow {
    pub(super) fn new(s: &Arc<Session>, stream_period: Duration) -> Self {
        let timing_due = Instant::now() + Duration::from_secs(5);
        let mut pipeline_log = butterpollo_media_policy::pipeline_rates::Sampler::default();
        // Baseline the rate sampler at the instant the first window starts,
        // so the first timing window is the stream's first five seconds
        // instead of a start-up grace window; a slow game then warns after
        // three complete windows (15 s).
        let _ = pipeline_log.sample(
            timing_due - Duration::from_secs(5),
            s.pipeline_counts(),
            Duration::from_secs(5),
        );
        Self {
            session: s.clone(),
            stream_period,
            timing_due,
            pipeline_log,
            source_rate: butterpollo_media_policy::pipeline_rates::SourceRate::default(),
            warned_virtual_display: false,
            display_refresh_mhz: None,
            stutter_totals: butterpollo_media_policy::stutter_report::Totals::default(),
            claim_ages: Vec::with_capacity(1024),
            wgc_stamp_ages: Vec::with_capacity(1024),
            // The 5 s timing summary is an information log, kept on by
            // default: a user's ordinary log must show where a stream
            // lost time. With logging below information, skip collecting
            // its samples and computing its percentiles.
            timings_logged: tracing::enabled!(tracing::Level::INFO),
        }
    }

    pub(super) fn flush_stutter(&mut self) {
        let pictures = self
            .session
            .stats
            .performance
            .lock()
            .unwrap()
            .advance_stutter();
        if let Some(pictures) = pictures {
            self.stutter_totals.add(&pictures, self.stream_period);
            self.session.stats.publish_stutter(&self.stutter_totals);
        }
    }

    /// Runs the 250 ms display refresh check and, when the 5 s window elapses, updates
    /// stutter totals, source-rate warnings, and emits the `stream timings` log line.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn live_tick(
        &mut self,
        h: &Shared,
        s: &Session,
        prepared: &crate::display_session::Ready,
        output: &str,
        stream: &butterpollo_media_policy::rtsp::Negotiated,
        stream_period: Duration,
        backend: &str,
        encoder: &Option<Encoder>,
        focus: &crate::stream_focus::Focus,
        checks: &mut crate::video_send::link_watch::CheckTimes,
        link_watch: &crate::video_send::link_watch::LinkWatch,
        webrtc_sender: &Option<Box<dyn crate::webrtc_bridge::WebRtcVideoSink>>,
    ) {
        // What the stream can show: its own rate, or the captured
        // display's where that is lower. A 60 Hz monitor streamed at 120 fps
        // presents 60 frames a second at best, so a source at 60 is not the
        // limit. The display's mode is read with each window (a virtual
        // display is retimed when the stream's is), and at every live tick
        // while a frame cap follows it, so a game's mode change takes the cap
        // off within 250 ms, not at the next window. A window target names
        // no display, so the display it is laid over is asked instead.
        let window_due = Instant::now() >= self.timing_due;
        let mut display_mhz = None;
        if butterpollo_win_io::limiter::display_read_due(
            window_due,
            butterpollo_win_io::limiter::following(),
        ) {
            let shown = prepared.output();
            display_mhz = butterpollo_win_display::display::mode(&shown)
                .or_else(|_| butterpollo_win_display::display::mode(output))
                .ok()
                .map(|mode| mode.dmDisplayFrequency)
                .filter(|hz| *hz > 1)
                .map(|hz| hz.saturating_mul(1000));
            // A game in exclusive fullscreen sets the display to a
            // mode of its own, which nothing else in the log
            // shows. The automatic frame limit of a virtual
            // display counts on the refresh the host gave it, and
            // stands down while the game's is too slow for it.
            if let Some(now_mhz) = display_mhz {
                if let Some(was_mhz) = self.display_refresh_mhz.filter(|was| *was != now_mhz) {
                    tracing::warn!(
                        client = %s.launch.client.name,
                        from_hz = was_mhz / 1000,
                        to_hz = now_mhz / 1000,
                        stream_fps = stream.fps_millihz() as f64 / 1000.,
                        "the stream's display changed its refresh rate; a game in exclusive fullscreen sets its own"
                    );
                }
                self.display_refresh_mhz = Some(now_mhz);
            }
            butterpollo_win_io::limiter::follow_display(
                display_mhz.map(butterpollo_media_policy::framegen::Rate),
            );
        }
        if !window_due {
            return;
        }
        let now = Instant::now();
        self.timing_due = now + Duration::from_secs(5);
        self.stream_period = stream_period;
        self.flush_stutter();
        // Pipeline frame rates since the last summary; the
        // sampler was baselined at stream start. The
        // source-rate warning is not a logging concern: it
        // feeds the console at every `min_log_level`, so it
        // samples and runs before the INFO-only timing line.
        let rates = self
            .pipeline_log
            .sample(now, s.pipeline_counts(), Duration::from_secs(5));
        let pipeline = rates
            .as_ref()
            .map(butterpollo_media_policy::pipeline_rates::PipelineRates::from);
        let raw_source_mhz = pipeline.as_ref().and_then(|r| r.source_mhz);
        let capture_mhz = pipeline.as_ref().and_then(|r| r.capture_mhz);
        // What the stream can show: its own rate, or the
        // captured display's where that is lower (read above).
        // The source rate against what the stream can show. A
        // game below it is the limit that every per-stage
        // number hides. A new warning or a re-word writes one
        // WARN line; `SourceRate` limits re-words to one per
        // 60 s (`REWORD`).
        if let Some(change) =
            self.source_rate
                .window(now, stream.fps_millihz(), display_mhz, raw_source_mhz)
        {
            use butterpollo_media_policy::pipeline_rates::{
                SourceRateWarning, below_message, virtual_display_limited,
            };
            match change {
                SourceRateWarning::Below {
                    source_mhz,
                    limit_mhz,
                } => {
                    // Blame `idd-push` only when `capture_fps` itself is low, not paired frames (< 2 ms).
                    let pushed = virtual_display_limited(
                        backend == "idd-push",
                        capture_mhz,
                        display_mhz,
                        raw_source_mhz.unwrap_or(source_mhz),
                        limit_mhz,
                    );
                    self.warned_virtual_display = pushed;
                    let reason = if pushed {
                        "the virtual display capture is below the stream's frame rate"
                    } else {
                        "the game or desktop is the frame-rate limit"
                    };
                    tracing::warn!(
                        client = %s.launch.client.name,
                        source_fps = source_mhz / 1000,
                        capture_fps = capture_mhz.map(|mhz| mhz as f64 / 1000.),
                        stream_fps = stream.fps_millihz() as f64 / 1000.,
                        display_fps = display_mhz.map(|mhz| mhz as f64 / 1000.),
                        capture = backend,
                        "{reason}"
                    );
                    let message = below_message(
                        pushed,
                        crate::stream::launch_app(h, &s.launch).is_some(),
                        source_mhz,
                        limit_mhz,
                    );
                    s.launch.warnings.set("source_frame_rate", message);
                }
                SourceRateWarning::Cleared => {
                    s.launch.warnings.clear("source_frame_rate");
                    let msg = if std::mem::take(&mut self.warned_virtual_display) {
                        "the virtual display keeps up with the stream's frame rate again"
                    } else {
                        "the game or desktop keeps up with the stream's frame rate again"
                    };
                    tracing::info!(
                        client = %s.launch.client.name,
                        capture = backend,
                        "{msg}"
                    );
                }
            }
        }
        if self.timings_logged {
            self.log_timings(
                s,
                pipeline.as_ref(),
                encoder,
                checks,
                link_watch,
                webrtc_sender,
            );
        }
        if focus.on()
            && let Some(Ok(stats)) = encoder.as_ref().map(|encoder| encoder.focus_stats())
        {
            tracing::info!(
                mode = focus
                    .settings(|| None)
                    .map(|settings| settings.mode.as_str()),
                maps = stats.maps,
                uniform = stats.uniform,
                stale = stats.stale,
                skipped_recovery = stats.skipped_recovery,
                qp_delta = stats.qp_delta,
                "encoder focus"
            );
        }
        self.claim_ages.clear();
        self.wgc_stamp_ages.clear();
        self.timings_logged = tracing::enabled!(tracing::Level::INFO);
    }

    fn log_timings(
        &mut self,
        s: &Session,
        pipeline: Option<&butterpollo_media_policy::pipeline_rates::PipelineRates>,
        encoder: &Option<Encoder>,
        checks: &mut crate::video_send::link_watch::CheckTimes,
        link_watch: &crate::video_send::link_watch::LinkWatch,
        webrtc_sender: &Option<Box<dyn crate::webrtc_bridge::WebRtcVideoSink>>,
    ) {
        // Sorted after the lock the sender records into is released.
        let (window, send_gaps) = {
            let mut performance = s.stats.performance.lock().unwrap();
            (
                performance.window(Instant::now()),
                performance.send_gaps.take(),
            )
        };
        let timing = window.snapshot();
        let link = s.link_rate_snapshot();
        // Congestion control (`congestion_control`, or its shadow run): state,
        // and the queueing delay over this window, for `congestion_ab.py`.
        let cc = s.congestion_snapshot();
        let (cc_queue_p50, cc_queue_p95, cc_samples) = s.take_congestion_window();
        let cc_u64 = |key: &str| cc[key].as_u64();
        let ms = |key: &str| timing[key].as_f64().unwrap_or(0.);
        let input = s.stats.input_latency.lock().unwrap().take();
        let fps_of =
            |pick: fn(&butterpollo_media_policy::pipeline_rates::PipelineRates) -> Option<u32>| {
                pipeline.and_then(pick).map(|mhz| f64::from(mhz) / 1000.)
            };
        let split = |pick: fn(&(u64, u64)) -> u64| {
            let mut values: Vec<u64> = self.claim_ages.iter().map(pick).collect();
            values.sort_unstable();
            let mean = values.iter().sum::<u64>() as f64 / values.len().max(1) as f64 / 1000.;
            let p95 = values
                .get(values.len().saturating_sub(1) * 95 / 100)
                .copied()
                .unwrap_or(0) as f64
                / 1000.;
            (mean, p95)
        };
        let (detect_mean_ms, detect_p95_ms) = split(|age| age.0);
        let (claim_wait_mean_ms, claim_wait_p95_ms) = split(|age| age.1);
        self.wgc_stamp_ages.sort_by(f64::total_cmp);
        let wgc_stamp_frames = self.wgc_stamp_ages.len();
        let wgc_stamp_future_frames = self.wgc_stamp_ages.iter().filter(|age| **age < 0.).count();
        let wgc_stamp_to_host_mean_ms = (wgc_stamp_frames > 0)
            .then(|| self.wgc_stamp_ages.iter().sum::<f64>() / wgc_stamp_frames as f64);
        let wgc_stamp_to_host_p95_ms = self
            .wgc_stamp_ages
            .get(wgc_stamp_frames.saturating_sub(1) * 95 / 100)
            .copied();
        // New pictures' stages over the last five seconds: mean/p95/p99 ms.
        let stage = |name: &str| {
            let ms = |key: &str| timing["stages"][name][key].as_f64().unwrap_or(0.);
            format!(
                "{:.2}/{:.2}/{:.2}",
                ms("mean_ms"),
                ms("p95_ms"),
                ms("p99_ms")
            )
        };
        // How old the newest capture is when the stream takes it: p50/p95/p99 ms.
        let age = |name: &str| {
            let ms = |key: &str| timing["stages"][name][key].as_f64().unwrap_or(0.);
            format!(
                "{:.2}/{:.2}/{:.2}",
                ms("p50_ms"),
                ms("p95_ms"),
                ms("p99_ms")
            )
        };
        tracing::info!(
            fps = ms("fps"),
            source_fps = fps_of(|r| r.source_mhz),
            capture_fps = fps_of(|r| r.capture_mhz),
            submit_fps = fps_of(|r| r.submit_mhz),
            output_fps = fps_of(|r| r.output_mhz),
            repeat_fps = fps_of(|r| r.repeat_mhz),
            host_mean_ms = ms("host_processing_mean_ms"),
            host_p95_ms = ms("host_processing_p95_ms"),
            host_p99_ms = ms("host_processing_p99_ms"),
            host_max_ms = ms("host_processing_max_ms"),
            encode_mean_ms = ms("encode_mean_ms"),
            encode_p95_ms = ms("encode_p95_ms"),
            encode_p99_ms = ms("encode_p99_ms"),
            frame_age_mean_ms = ms("frame_age_mean_ms"),
            frame_age_p95_ms = ms("frame_age_p95_ms"),
            present_to_send_mean_ms = ms("present_to_send_mean_ms"),
            present_to_send_p99_ms = ms("present_to_send_p99_ms"),
            present_to_sent_ms = %stage("present_to_sent"),
            stage_capture_ms = %stage("capture"),
            stage_hold_ms = %stage("hold"),
            stage_claim_ms = %stage("claim"),
            stage_convert_ms = %stage("convert"),
            stage_encode_ms = %stage("encode"),
            stage_queue_ms = %stage("queue"),
            stage_packetize_ms = %stage("packetize"),
            stage_send_queue_ms = %stage("send_queue"),
            stage_send_ms = %stage("send"),
            moonlight_host_ms = %stage("moonlight_host"),
            capture_age_at_claim_ms = %age("acquire_to_claim"),
            capture_age_at_submit_ms = %age("acquire_to_submit"),
            stage_frames = timing["stages"]["frames"].as_u64().unwrap_or(0),
            detect_mean_ms,
            detect_p95_ms,
            claim_wait_mean_ms,
            claim_wait_p95_ms,
            wgc_stamp_to_host_mean_ms,
            wgc_stamp_to_host_p95_ms,
            wgc_stamp_frames,
            wgc_stamp_future_frames,
            // The encode loop's remaining display and route reads a frame:
            // mean/max ms; the watcher's longest lookup, off the hot path.
            stage_checks_ms = %checks.take(),
            link_lookup_max_ms = link_watch.take_slowest().as_secs_f64() * 1000.,
            send_interval_p95_ms = timing["send_interval_p95_ms"].as_f64().unwrap_or(0.),
            send_interval_p99_ms = timing["send_interval_p99_ms"].as_f64().unwrap_or(0.),
            send_interval_max_ms = timing["send_interval_max_ms"].as_f64().unwrap_or(0.),
            // Keep reference feedback distinct from IDR recovery.
            idr_requests = s.stats.idr_requests.load(Ordering::Relaxed),
            reference_invalidations = s.stats.reference_invalidations.load(Ordering::Relaxed),
            send_loss_recoveries = s.stats.send_loss_recoveries.load(Ordering::Relaxed),
            // Inter frames past the full-FEC bound, sent as keyframes instead.
            reencoded_cuts = encoder.as_ref().map_or(0, |encoder| encoder.reencoded_cuts()),
            keyframes_over_cap = encoder.as_ref().map_or(0, |encoder| encoder.keyframes_over_cap()),
            keyframe_cap_scale = encoder.as_ref().map_or(1., |encoder| encoder.keyframe_cap_scale()),
            keyframes_past_fec = s.stats.keyframes_past_fec.load(Ordering::Relaxed),
            fec_reports = timing["fec_reports"].as_u64().unwrap_or(0),
            fec_invalid_reports = timing["fec_invalid_reports"].as_u64().unwrap_or(0),
            fec_duplicate_reports = timing["fec_duplicate_reports"].as_u64().unwrap_or(0),
            fec_recovered_frames = timing["fec_recovered_frames"].as_u64().unwrap_or(0),
            fec_unrecoverable_frames = timing["fec_unrecoverable_frames"].as_u64().unwrap_or(0),
            fec_missing_packets = timing["fec_missing_packets"].as_u64().unwrap_or(0),
            fec_loss_reports = timing["fec_loss_reports"].as_u64().unwrap_or(0),
            fec_loss_refused_reports = timing["fec_loss_refused_reports"].as_u64().unwrap_or(0),
            bitrate_kbps = s.bitrate.load(Ordering::Relaxed),
            sent_mbps = ms("bitrate_mbps"),
            link_state = %link["state"].as_str().unwrap_or(""),
            link_ceiling_kbps = link["ceiling_kbps"].as_u64().unwrap_or(0),
            link_learned_kbps = link["learned_kbps"].as_u64().unwrap_or(0),
            link_current_kbps = link["current_kbps"].as_u64().unwrap_or(0),
            link_delivered_kbps = link["delivered_kbps"].as_u64().unwrap_or(0),
            link_steps_down = link["steps_down"].as_u64().unwrap_or(0),
            link_held_cuts = link["held_cuts"].as_u64().unwrap_or(0),
            link_probes = link["probes"].as_u64().unwrap_or(0),
            link_failed_probes = link["failed_probes"].as_u64().unwrap_or(0),
            link_rtt_ms = link["rtt_ms"].as_u64().unwrap_or(0),
            link_rtt_floor_ms = link["rtt_floor_ms"].as_u64().unwrap_or(0),
            // Only with webrtc_loss_permille on: the video datagrams it has thrown away.
            webrtc_simulated_losses = webrtc_sender.as_ref().and_then(|sender| sender.simulated_losses()),
            // ENet keyboard, mouse, touch and pen passes: the control worker's
            // read of a pass's first datagram to its input injected, p50/p99/max ms.
            input_receive_to_injected_ms = %input,
            input_passes = input.passes,
            cc_state = %cc["state"].as_str().unwrap_or("off"),
            cc_shadow = cc["shadow"].as_bool().unwrap_or(false),
            cc_usage = cc["usage"].as_str(),
            cc_target_kbps = cc_u64("target_kbps"),
            cc_wanted_kbps = cc_u64("wanted_kbps"),
            cc_acked_kbps = cc_u64("acked_kbps"),
            cc_pace_kbps = cc_u64("pace_kbps"),
            cc_loss_permille = cc_u64("loss_permille"),
            cc_trend = cc["trend"].as_f64(),
            cc_threshold = cc["threshold"].as_f64(),
            cc_queue_p50_ms = cc_queue_p50.map(|v| (v * 100.).round() / 100.),
            cc_queue_p95_ms = cc_queue_p95.map(|v| (v * 100.).round() / 100.),
            cc_queue_samples = cc_samples,
            cc_overuse_cuts = cc_u64("overuse_cuts"),
            cc_overuse_held = cc_u64("overuse_held"),
            cc_sent_kbps = cc_u64("sent_kbps"),
            cc_loss_cuts = cc_u64("loss_cuts"),
            cc_reports = cc_u64("reports"),
            cc_feedback_bytes = cc_u64("feedback_bytes"),
            "stream timings"
        );
        // Each sent frame's gap against the frame period (`send_gaps`), beside the
        // microphone's and the pointer's arrival gaps: whether the host sent evenly.
        if send_gaps.frames > 0 {
            let to_ms = |us: u32| f64::from(us) / 1000.;
            tracing::info!(
                client = %s.launch.client.name,
                frames = send_gaps.frames,
                period_ms = to_ms(send_gaps.period_us),
                bunched = send_gaps.bunched,
                short = send_gaps.short,
                on_time = send_gaps.on_time,
                late = send_gaps.late,
                skipped = send_gaps.skipped,
                pauses = send_gaps.pauses,
                jitter_ms = to_ms(send_gaps.jitter_us),
                max_gap_ms = to_ms(send_gaps.max_gap_us),
                "video send gaps"
            );
        }
    }

    pub(super) fn record_claim_age(
        &mut self,
        fresh: bool,
        repeat: bool,
        image: &GpuImage,
        begin: Instant,
    ) {
        if self.timings_logged && fresh && !repeat && self.claim_ages.len() < 4096 {
            let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
            self.claim_ages.push((
                micros(image.acquired.saturating_duration_since(image.captured)),
                micros(begin.saturating_duration_since(image.acquired)),
            ));
            if let Some(stamp) = image.wgc_stamp {
                let age = if image.acquired >= stamp {
                    image.acquired.duration_since(stamp).as_secs_f64()
                } else {
                    -stamp.duration_since(image.acquired).as_secs_f64()
                };
                self.wgc_stamp_ages.push(age * 1000.);
            }
        }
    }

    pub(super) fn on_mode_switched(
        &mut self,
        s: &Session,
        from: butterpollo_media_policy::reconfigure::Mode,
        started: Instant,
        stream: &butterpollo_media_policy::rtsp::Negotiated,
        image: &GpuImage,
    ) {
        // The new size or rate has its own usual path; the old one
        // would report its first slower windows as a stutter. The
        // old mode's last window is copied out under the sender's
        // lock and added after it is released.
        let pictures = s.stats.performance.lock().unwrap().forget_stutter_usual();
        if let Some(pictures) = pictures {
            self.stutter_totals.add(&pictures, from.period());
        }
        self.stream_period = butterpollo_media_policy::reconfigure::Mode::of(stream).period();
        self.stutter_totals.forget_usual();
        s.stats.publish_stutter(&self.stutter_totals);
        s.rate_changes.switched(
            butterpollo_media_policy::reconfigure::Mode::of(stream),
            None,
        );
        tracing::info!(
            client = %s.launch.client.name,
            from = %from,
            to = %butterpollo_media_policy::reconfigure::Mode::of(stream),
            switch_ms = started.elapsed().as_millis() as u64,
            source_width = image.width,
            source_height = image.height,
            "reconfigure"
        );
    }
}

/// Emits the per-claim trace line when `pacing` trace logging is enabled.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_claim_trace(
    admission: &mut AdmissionTrace,
    trace_attempt: &mut u64,
    start: Instant,
    first_seen: Option<(usize, Instant, Option<Duration>, Option<Instant>)>,
    raw_image: &Arc<GpuImage>,
    image: &GpuImage,
    begin: Instant,
    pacing_picture_interval: Option<Duration>,
    latest: &Source,
    recovery: &mut EncoderRecovery,
    s: &Session,
    fresh: bool,
    recovering: bool,
    encoding: usize,
    sending: usize,
    encoding_limit: usize,
    rebuilt: bool,
) {
    *trace_attempt = trace_attempt.wrapping_add(1);
    let us = |at: Instant| at.saturating_duration_since(start).as_micros() as u64;
    let key = Arc::as_ptr(raw_image) as usize;
    let (seen, interval, deadline) = match first_seen {
        Some((k, seen, interval, deadline)) if k == key => (seen, interval, deadline),
        _ => (begin, pacing_picture_interval, None),
    };
    let (capture_id, selected_is_latest, latest_capture_id) = latest.trace_selection(raw_image);
    let query = recovery
        .queries
        .as_ref()
        .map_or_else(QueryTrace::default, |queries| {
            QueryTrace::new(queries, image.published)
        });
    if let Some(queries) = recovery.queries.as_mut() {
        queries.clear();
    }
    tracing::trace!(
        target: "pacing",
        capture_id,
        attempt = *trace_attempt,
        host_acquired = ?image.acquired,
        presented = us(image.captured),
        acquired = us(image.acquired),
        published = us(image.published),
        seen = us(seen),
        claim = us(begin),
        interval = interval.map_or(0, |i| i.as_micros() as u64),
        deadline = deadline.map_or(0, us),
        source_id = latest.trace_source_id(),
        stream_id = %s.launch.id,
        fresh,
        repeat = raw_image.repeat,
        choice = if raw_image.repeat {
            "capture_repeat"
        } else if fresh {
            "fresh"
        } else if recovering {
            "recovery_repeat"
        } else {
            "static_repeat"
        },
        selected_is_latest,
        latest_capture_id,
        encoder_pending = encoding,
        sender_pending = sending,
        encoder_limit = encoding_limit,
        rebuilt,
        grid_us = admission.micros(AdmissionWait::Grid),
        capture_check_us = admission.micros(AdmissionWait::Capture),
        fresh_rate_us = admission.micros(AdmissionWait::FreshRate),
        repeat_us = admission.micros(AdmissionWait::Repeat),
        encoder_pressure_us = admission.micros(AdmissionWait::Encoder),
        sender_pressure_us = admission.micros(AdmissionWait::Sender),
        admission_passes = ?admission.passes,
        output_polls = query.queries,
        output_poll_us = query.query_us,
        published_in_poll = query.published_in_query,
        poll_held_us = query.held_us,
        "claim"
    );
    *admission = AdmissionTrace::default();
}

impl Drop for TimingsWindow {
    fn drop(&mut self) {
        self.flush_stutter();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_media_policy::performance::Claim;
    use butterpollo_session::session::Role;

    #[test]
    fn dropping_timings_window_flushes_final_partial_stutter_window() {
        let f = Fixture::new();
        let s = f.session(f.client(u32::MAX), Role::Stream);
        let period = Duration::from_micros(16_667);
        let base = Instant::now();
        let at = |us: u64| base + Duration::from_micros(us);
        let stamps = Claim {
            key: at(0),
            presented: at(0),
            acquired: at(100),
            published: at(150),
            claimed: at(200),
            submitted: at(700),
        }
        .sent(at(1000), at(1100), at(1500), at(2000));
        {
            let _timings = TimingsWindow::new(&s, period);
            let mut perf = s.stats.performance.lock().unwrap();
            let count = butterpollo_media_policy::stutter_report::MIN_PICTURES;
            for _ in 0..count {
                perf.record_stages(&stamps, 1_000);
            }
            assert_eq!(perf.uncounted_stutter().count(), count);
            drop(perf);
            assert_eq!(s.stats.stutter_window().report(period).windows(), 1);
        }
        assert_eq!(
            s.stats
                .performance
                .lock()
                .unwrap()
                .uncounted_stutter()
                .count(),
            0,
            "dropping TimingsWindow must flush the final partial stutter window"
        );
        let report = s.stats.stutter_window().report(period);
        assert_eq!(report.windows(), 1);
        assert_eq!(
            report.pictures(),
            butterpollo_media_policy::stutter_report::MIN_PICTURES as u64
        );
    }
}
