//! The overlay's statistics: the once-a-second window of measurements and its lines.

use super::*;

/// The overlay's early decode line: frames per second decoded ahead, and the fallbacks of the
/// last interval and in all (`now` and `last` are the early frame and fallback totals).
pub(super) fn early_line(now: (u64, u64), last: (u64, u64), secs: f64) -> String {
    trf("{} early/s, {} whole in the last second, {} fallbacks in all",
        &[&format!("{:.0}", now.0.saturating_sub(last.0) as f64 / secs), &now.1.saturating_sub(last.1), &now.1])
}

/// The overlay's line while the host holds the bitrate down for the network: whole Mbps from 10,
/// one decimal below, as Nightjar Android says it.
pub(super) fn link_notice_line(notice: &LinkNotice) -> String {
    let mbps = f64::from(notice.current_kbps) / 1000.0;
    let mbps = if mbps >= 10.0 { format!("{mbps:.0}") } else { format!("{mbps:.1}") };
    trf("Host lowered the bitrate to {} Mbps: network", &[&mbps])
}

/// The overlay's host pipeline line: the frames per second each host stage handled over the
/// host's last window (0x553D), "-" for a stage the host can't count.
pub(super) fn pipeline_rates_line(report: &PipelineRatesReport) -> String {
    let fps = |mhz: u32| if mhz == UNKNOWN { "-".to_owned() } else { format!("{:.1}", f64::from(mhz) / 1000.0) };
    trf("{} source, {} captured, {} encoded, {} sent, {} repeated fps", &[
        &fps(report.source_mhz),
        &fps(report.capture_mhz),
        &fps(report.submit_mhz),
        &fps(report.output_mhz),
        &fps(report.repeat_mhz),
    ])
}

/// The line logged when any session ends: what the decoder was given, and the frames lost
/// before the picture (over the decoder queue, or replaced by a newer one before they were due).
pub(super) fn video_summary(submitted: u64, queue_overflows: u64, replaced: u64) -> String {
    format!("nightjar: video: {submitted} frames to the decoder, {queue_overflows} dropped over the decoder queue, \
        {replaced} replaced before they were shown")
}

/// The line logged when an early decode session ends: the three counters of the decoder and
/// the chunks the queue dropped.
pub(super) fn early_summary(early_frames: u64, fallbacks: u64, chunk_us: u64, chunks_dropped: u64) -> String {
    format!("nightjar: early decode: {early_frames} frames decoded ahead, {fallbacks} fell back to the whole frame, \
        {chunks_dropped} chunks dropped, {:.1} ms of decoding done while frames arrived", chunk_us as f64 / 1000.0)
}

/// The beam race in one line (overlay and session end): frames decoded in slices as they arrived,
/// the slice decodes made, and the frames that were not in row order and decoded whole. A host
/// that never sends row order says so instead of showing zeros.
pub(super) fn race_line(frames: u64, slices: u64, unordered: u64) -> String {
    let mut line = format!("{frames} frames in {slices} slices, {unordered} not in row order");
    if frames == 0 && unordered > 0 {
        line.push_str(" (the host is not sending the row order)");
    }
    line
}

/// Whether a stream runs the display check for VRR: every VRR stream (`vrr`: the setting is on and
/// the swap chain tears) that presents with tearing and would pace otherwise without VRR
/// (`fallback`), unless `NIGHTJAR_VRR_CHECK` (`forced`) says off. The VRR setting is on by default,
/// and without the check a fixed refresh display tears.
pub(super) fn checks_display_for_vrr(vrr: bool, pacing: PacingMode, fallback: PacingMode, forced: Option<bool>) -> bool {
    vrr && pacing != PacingMode::Vsync && fallback != pacing && forced != Some(false)
}

/// Whether the stream reads the scan line around its presents, and the topic of its log lines:
/// for the beam race, for the VRR display check, and for `NIGHTJAR_VRR_SYNC=scan`, which waits on
/// the beam. None for every other stream.
pub(super) fn display_probe(beam_race: bool, vrr: &present::VrrPacing) -> Option<&'static str> {
    if beam_race {
        Some("beam race")
    } else if vrr.check || vrr.sync == present::VrrSync::Scan {
        Some("display check")
    } else {
        None
    }
}

/// The overlay's pacing, which says why a VRR stream waits for the refresh (or tears without
/// V-Sync) when the display check changed it.
pub(super) fn pacing_label(mode: PacingMode, fell_back: bool) -> &'static str {
    match mode {
        PacingMode::Vsync if fell_back => "V-Sync: this display does not refresh on present",
        PacingMode::Immediate if fell_back => "Immediate: this display does not refresh on present",
        mode => mode.label(),
    }
}

/// The overlay's host lines. "Host processing" is the average of the video frame header's per-frame
/// processing time, the same number Artemis and Moonlight show. It must stay that: users compare it
/// with those clients. The host timing reports' present to last packet sent includes the network
/// send and reads several times higher, so it only ever goes under its own "Host to network" label,
/// and a report never replaces or feeds the "Host processing" value.
pub(super) fn host_lines(w: &Window) -> Vec<(String, String)> {
    let avg = |sum: f64, n: u32| if n > 0 { sum / f64::from(n) } else { 0.0 };
    let mut lines = Vec::new();
    if w.host_latency_n > 0 {
        lines.push((
            "Host processing".into(),
            format!("{:.1} ms", avg(w.host_latency as f64, w.host_latency_n) / 10.0),
        ));
    }
    if w.timing_n > 0 {
        lines.push((
            "Host to network".into(),
            format!(
                "{:.1} ms present to send, {:.1} ms encode",
                avg(w.timing_host_us as f64, w.timing_n) / 1000.0,
                avg(w.timing_encode_us as f64, w.timing_n) / 1000.0
            ),
        ));
    }
    lines
}

/// What the hardware poll makes of the host's answer: a host that does not offer the readings
/// (not advertised, another schema version, 404, 403 or 409) is not asked again this stream.
pub(super) fn hardware_answer(result: &Result<HardwareMetrics, HostApiError>) -> HardwareAnswer {
    use nightjar_core::http::Error;
    match result {
        Ok(m) => HardwareAnswer::Sample { sampled_at_unix_ms: m.sampled_at_unix_ms, interval_ms: m.interval_ms },
        Err(HostApiError::Unavailable { .. } | HostApiError::UnsupportedVersion) => HardwareAnswer::Unavailable,
        Err(HostApiError::Host(Error::NotFound(_) | Error::Status { code: 403 | 404 | 409, .. } | Error::Api { status: 403 | 404 | 409, .. })) => HardwareAnswer::Unavailable,
        Err(_) => HardwareAnswer::Failed,
    }
}

impl Streaming {
    /// The overlay's value for the raced presenter: what it did, and why it did not when it did not.
    pub(super) fn raced_line(&self) -> String {
        let totals = self.racer.totals;
        match self.pacer.mode {
            PacingMode::BeamRace => {
                let mut line = format!("{} raced, {} whole, {} late slices in {} frames", totals.raced, totals.whole, totals.late_slices, totals.late_frames);
                if totals.abandoned > 0 { line.push_str(&format!(", {} never finished", totals.abandoned)); }
                if let Some(skip) = totals.last_skip { line.push_str(&format!("; last whole: {}", skip.label())); }
                line
            }
            _ if self.race_probe.as_ref().is_some_and(|probe| probe.racing().is_some()) => format!("off, pacing is {}", self.pacer.mode.label()),
            _ => "off until the display check passes".into(),
        }
    }

    /// Recomputes the overlay text once a second. Returns whether it changed.
    pub fn update_overlay(&mut self) -> bool {
        let started = *self.window.started.get_or_insert_with(Instant::now);
        let elapsed = started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return false;
        }
        let secs = elapsed.as_secs_f64();
        let stats = self.session.stats();
        let c = &self.shared.counters;
        let now = (
            stats.frames_submitted,
            c.decoded.load(Ordering::Relaxed),
            c.decode_us_total.load(Ordering::Relaxed),
            c.decode_errors.load(Ordering::Relaxed),
            c.audio_dropped.load(Ordering::Relaxed),
            c.audio_underruns.load(Ordering::Relaxed),
            c.audio_concealed.load(Ordering::Relaxed),
        );
        let last = self.last_counters;
        self.last_counters = now;
        let received = now.0 - last.0;
        let decoded = now.1 - last.1;
        let decode_ms = if decoded > 0 {
            (now.2 - last.2) as f64 / decoded as f64 / 1000.0
        } else {
            0.0
        };
        let video = (stats.video.frames_completed, stats.video.frame_drops);
        let (completed, drops) = (video.0 - self.last_video.0, video.1 - self.last_video.1);
        self.last_video = video;
        let loss = if completed + drops > 0 {
            drops as f64 * 100.0 / (completed + drops) as f64
        } else {
            0.0
        };
        let mut w = std::mem::take(&mut self.window);
        (w.host_latency, w.host_latency_n) = self.host_average.update(
            c.host_latency_total.load(Ordering::Relaxed), c.host_latency_frames.load(Ordering::Relaxed));
        let avg = |sum: f64, n: u32| if n > 0 { sum / f64::from(n) } else { 0.0 };
        let measured = |sum: f64, n: u32| if n > 0 { format!("{:.3}", sum / f64::from(n) / 1000.0) } else { String::new() };
        self.statistics.push_back(format!("{},{:.3},{:.3},{:.3},{loss:.3},{},{decode_ms:.3},{},{},{},{}\n",
            self.started.elapsed().as_millis(), received as f64 / secs, decoded as f64 / secs, f64::from(w.shown) / secs,
            stats.rtt.map_or_else(String::new, |(rtt, _)| format!("{:.3}", rtt.as_secs_f64() * 1000.0)),
            measured(w.render_us as f64, w.shown), measured(w.timing_host_us as f64, w.timing_n),
            measured(w.latency_us as f64, w.latency_n), self.facts.bitrate_kbps));
        if self.statistics.len() > 3600 { self.statistics.pop_front(); }
        let mut lines = Vec::new();
        let f = &self.facts;
        lines.push((
            "Stream".into(),
            format!("{}x{} {} fps, {}", f.width, f.height, f.fps, codec_name(f.format)),
        ));
        lines.push(("Decoder".into(), lock(&self.shared.decoder_name).clone()));
        if self.settings.upscaling != config::Upscaling::Bilinear {
            lines.push(("Upscaling".into(), self.upscaling_line()));
        }
        if self.quality_mode != QualityMode::Off { lines.push(("Quality".into(), self.quality_mode.label().into())); }
        lines.push((
            "Frames".into(),
            format!(
                "{:.0} in, {:.0} decoded, {:.0} shown",
                received as f64 / secs,
                decoded as f64 / secs,
                f64::from(w.shown) / secs
            ),
        ));
        lines.push((
            "Network".into(),
            match stats.rtt {
                Some((rtt, var)) => format!(
                    "{:.1} ms RTT (±{:.1}), {:.1}% frames lost",
                    rtt.as_secs_f64() * 1000.0,
                    var.as_secs_f64() * 1000.0,
                    loss
                ),
                None => format!("{loss:.1}% frames lost"),
            },
        ));
        if w.network_n > 0 && w.network_us / u64::from(w.network_n) >= 50 {
            lines.push((
                "Receive".into(),
                format!("{:.1} ms first packet to frame", avg(w.network_us as f64, w.network_n) / 1000.0),
            ));
        }
        lines.push((
            "Decode".into(),
            format!("{decode_ms:.2} ms{}", if now.3 > last.3 { ", errors" } else { "" }),
        ));
        if self.facts.early_without_records {
            lines.push(("Early decode".into(), "off, the host sent no PyroWave records".into()));
        }
        if self.facts.early_decode {
            let early = (c.early_frames.load(Ordering::Relaxed), c.early_fallbacks.load(Ordering::Relaxed));
            let last_early = std::mem::replace(&mut self.last_early, early);
            lines.push(("Early decode".into(), early_line(early, last_early, secs)));
        }
        if let Some(reason) = self.facts.beam_race_off {
            lines.push(("Beam race".into(), reason.into()));
        }
        if let (true, Some(probe)) = (self.facts.beam_race, &self.race_probe) {
            lines.push(("Beam race".into(), probe.line()));
        }
        if self.facts.beam_race {
            lines.push(("Race slices".into(), race_line(c.race_frames.load(Ordering::Relaxed), c.race_slices.load(Ordering::Relaxed),
                c.race_unordered.load(Ordering::Relaxed))));
        }
        if self.facts.beam_race {
            lines.push(("Raced presenter".into(), self.raced_line()));
        }
        lines.push((
            "Render".into(),
            format!("{:.2} ms", avg(w.render_us as f64, w.shown) / 1000.0),
        ));
        let mut pacing = pacing_label(self.pacer.mode, self.vrr.fell_back).to_owned();
        if self.vrr.sync != present::VrrSync::Off && self.pacer.mode != PacingMode::Vsync {
            pacing.push_str(&format!(", VRR sync {}", self.vrr.sync.label()));
            if self.scan_waits.0 > 0 {
                pacing.push_str(&format!(" ({} waits, {:.2} ms each)", self.scan_waits.0, self.scan_waits.1 as f64 / f64::from(self.scan_waits.0) / 1e6));
            }
        }
        if self.pacer.mode == PacingMode::HostTimes {
            if let Some(delay) = self.pacer.clock.delay_us() {
                pacing.push_str(&format!(
                    ", hold {:.1} ms, {} late",
                    avg(w.hold_us as f64, w.shown) / 1000.0,
                    w.late
                ));
                let _ = delay;
            }
        }
        if w.dropped_pacing > 0 {
            pacing.push_str(&format!(", {} replaced", w.dropped_pacing));
        }
        lines.push(("Pacing".into(), pacing));
        if self.facts.phase_lock && self.pacer.mode == PacingMode::Vsync {
            lines.push(("Phase lock".into(), self.phase.lead_us.map_or_else(|| "Waiting for display timing".into(), |lead| format!("{:.2} ms lead", f64::from(lead) / 1000.0))));
        }
        lines.extend(host_lines(&w));
        if w.latency_n > 0 {
            lines.push((
                "Latency".into(),
                format!(
                    "{:.1} ms PC frame to display",
                    avg(w.latency_us as f64, w.latency_n) / 1000.0
                ),
            ));
        }
        let channels = c.audio_channels.load(Ordering::Relaxed);
        let mut audio = match channels {
            0 => "no output device".to_owned(),
            2 => "stereo".to_owned(),
            6 => "5.1".to_owned(),
            8 => "7.1".to_owned(),
            n => format!("{n} channels"),
        };
        if channels > 0 {
            audio.push_str(&format!(
                ", {:.0} ms buffered",
                c.audio_queued_us.load(Ordering::Relaxed) as f64 / 1000.0
            ));
        }
        if now.4 > last.4 {
            audio.push_str(&format!(", {} packets dropped to stay in sync", now.4 - last.4));
        }
        if now.5 > last.5 {
            audio.push_str(&format!(", {} underruns", now.5 - last.5));
        }
        if now.6 > last.6 {
            audio.push_str(&format!(", {} packets concealed", now.6 - last.6));
        }
        lines.push(("Audio".into(), audio));
        lines.push(("Bitrate".into(), format!("{:.1} Mbps{}", f64::from(self.facts.bitrate_kbps) / 1000.0,
            if self.adaptive_bitrate || self.client_hint_active() { ", automatic" } else { "" })));
        if let Some(report) = &self.pipeline_rates {
            lines.push(("Host pipeline".into(), pipeline_rates_line(report)));
        }
        if let Some(notice) = &self.link_notice {
            lines.push(("Notice".into(), link_notice_line(notice)));
        }
        let video_bytes = c.video_bytes.load(Ordering::Relaxed);
        let wire_bytes = self.session.wire_bytes();
        let measures = overlay::Measures {
            rendered_fps: (w.shown > 0 || received > 0).then(|| (f64::from(w.shown) / secs) as f32),
            received_fps: (received > 0).then(|| (received as f64 / secs) as f32),
            video_mbps: (received > 0).then(|| overlay::video_mbps(video_bytes.saturating_sub(self.last_video_bytes), secs)).flatten(),
            wire_mbps: (wire_bytes > self.last_wire_bytes)
                .then(|| overlay::video_mbps(wire_bytes - self.last_wire_bytes, secs))
                .flatten(),
            loss_percent: (completed + drops > 0).then_some(loss as f32),
            network_ms: stats.rtt.map(|(rtt, _)| (rtt.as_secs_f64() * 1000.0) as f32),
            variance_ms: stats.rtt.map(|(_, var)| (var.as_secs_f64() * 1000.0) as f32),
            host_ms: (w.host_latency_n > 0).then(|| (avg(w.host_latency as f64, w.host_latency_n) / 10.0) as f32),
            host_to_network_ms: (w.timing_n > 0).then(|| (avg(w.timing_host_us as f64, w.timing_n) / 1000.0) as f32),
            decode_ms: (decoded > 0).then_some(decode_ms as f32),
            render_ms: (w.shown > 0).then(|| (avg(w.render_us as f64, w.shown) / 1000.0) as f32),
        };
        self.last_video_bytes = video_bytes;
        self.last_wire_bytes = wire_bytes;
        lines.extend(overlay::wire(&measures));
        let mut statistics = overlay::statistics(&lines[0].1, &measures);
        let now_ms = self.started.elapsed().as_millis() as u64;
        let wanted = self.overlay_visible && self.overlay_mode != OverlayMode::Compact && self.facts.role != AppRole::InputOnly;
        if let Some(generation) = self.hardware_poll.due(now_ms, wanted) {
            self.request_hardware(generation);
        }
        if !self.hardware_poll.fresh(now_ms) {
            self.hardware = None;
        }
        if let Some(metrics) = &self.hardware {
            statistics.extend(overlay::hardware(metrics, false));
            lines.extend(overlay::hardware(metrics, true));
        }
        self.overlay = Overlay {
            lines,
            statistics,
            compact: overlay::compact(&measures),
            notice: self.link_notice.as_ref().map(link_notice_line),
        };
        let timing = std::mem::take(&mut *lock(&self.shared.decode_window));
        let now_us = session::now_us();
        // The window since the last tick, or since the last requested mode, which reset it.
        let decode_timing = quality::decode_timing(self.decode_generation, self.decode_window_us, now_us, self.facts.fps.max(0) as u32, &timing);
        self.decode_window_us = now_us;
        let mut tick = Stats {
            now_ms: self.started.elapsed().as_millis() as u64,
            recent_video: decoded > 0 && self.last_arrival.is_some_and(|(_, at)| now_us.saturating_sub(at) < 500_000),
            poor_connection: stats.connection_status == Some(nightjar_core::control::ConnectionStatus::Poor),
            fps: self.facts.fps.max(0) as u32,
            loss_percent: (completed + drops > 0).then_some(loss as f32),
            decode_ms: (decoded > 0).then_some(decode_ms as f32),
            jitter_ms: (self.jitter_ms >= 0.0).then_some(self.jitter_ms as f32),
            queue_ms: (timing.samples > 0).then(|| (timing.queue_us as f64 / f64::from(timing.samples) / 1000.0) as f32),
            received_fps: Some((received as f64 / secs) as f32),
            released_fps: Some((decoded as f64 / secs) as f32),
            shown_fps: Some((f64::from(w.shown) / secs) as f32),
            ..Default::default()
        };
        self.doctor.timings(&mut tick);
        let client = self.host_abr.is_some().then(|| lock(&self.shared.host_client).clone()).flatten();
        if let (Some(abr), Some(client)) = (&mut self.host_abr, client) {
            abr.add(completed, drops, decoded);
            let rtt_ms = stats.rtt.map(|(rtt, _)| rtt.as_secs_f64() * 1000.0);
            if let Some(feedback) = abr.due(tick.now_ms, rtt_ms) {
                let shared = self.shared.clone();
                std::thread::spawn(move || {
                    let action = client.abr_feedback(&feedback)
                        .map_err(|e| eprintln!("nightjar: the PC's automatic bitrate report failed: {e}")).ok();
                    shared.notify(Notice::HostAbrAction(action));
                });
            }
        }
        // As on Android: no samples for input-only, or while a manual bitrate change or trial runs.
        if self.facts.role != AppRole::InputOnly && !(self.bitrate_busy && !self.adaptive_bitrate) {
            self.doctor.add(&tick);
        }
        let quality_target = if let Some(policy) = &mut self.quality_policy {
            if self.menu_visible || !self.focused || self.pending_mode.is_some() { policy.suspend(); None }
            else {
                let has_room = self.bitrate_busy || self.bitrate_policy.as_ref().is_some_and(|p| p.has_room())
                    || self.rate_change.as_ref().is_some_and(|rates| rates.hint_has_room());
                policy.sample(&tick, has_room, decode_timing.as_ref())
            }
        } else { None };
        if let Some(target) = quality_target {
            let applied = if self.request_mode(target) {
                self.quality_policy.as_mut().expect("active quality policy").applied(target, tick.now_ms)
                    .map_err(|error| eprintln!("nightjar: automatic quality: {target:?} not applied: {error}")).is_ok()
            } else { false };
            if !applied { self.quality_policy = None; self.quality_mode = QualityMode::Off; }
        }
        if !self.bitrate_busy {
            if let Some(policy) = &mut self.bitrate_policy {
                if self.menu_visible || !self.focused {
                    policy.suspend();
                } else {
                    let target = if self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                        // PyroWave also loses frames that are recovered without their records.
                        let record_loss = (timing.record_samples > 0)
                            .then(|| (timing.record_loss_milli as f64 / f64::from(timing.record_samples) / 1000.0) as f32);
                        let loss_percent = match (tick.loss_percent, record_loss) {
                            (Some(a), Some(b)) => Some(a.max(b)),
                            (a, b) => a.or(b),
                        };
                        policy.sample(&Stats { loss_percent, ..tick })
                    } else { policy.sample(&tick) };
                    if let Some(target) = target {
                        self.bitrate_kbps = target;
                        self.set_bitrate();
                    }
                }
            }
        }
        if let Some(toast) = self.rate_change.as_mut().and_then(|rates| rates.expire(self.started.elapsed().as_millis() as u64)) {
            self.show_toast(toast);
        }
        if !self.menu_visible && self.focused {
            if let Some(rates) = &mut self.rate_change {
                // PyroWave also loses records within frames, as for the PC's adaptive bitrate.
                let record_loss = (self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 && timing.record_samples > 0)
                    .then(|| (timing.record_loss_milli as f64 / f64::from(timing.record_samples) / 1000.0) as f32);
                let loss_percent = match (tick.loss_percent, record_loss) { (Some(a), Some(b)) => Some(a.max(b)), (a, b) => a.or(b) };
                let request = rates.tick(&Stats {
                    now_ms: tick.now_ms,
                    recent_video: tick.recent_video,
                    poor_connection: tick.poor_connection,
                    loss_percent,
                    queue_ms: tick.queue_ms,
                    ..Default::default()
                });
                if let Some(request) = request { self.send_rate_change(request); }
            }
        }
        self.window.started = Some(Instant::now());
        true
    }
}

#[cfg(test)]
mod host_line_tests {
    use super::*;

    #[test]
    fn host_processing_averages_received_frames_over_two_windows_like_artemis() {
        let mut average = HostAverage::default();
        // First second: 60 frames of 0.5 ms (5 units). Nothing earlier to add.
        assert_eq!(average.update(300, 60), (300, 60));
        // Second second: 60 frames of 1.0 ms. The overlay sums both seconds.
        assert_eq!(average.update(900, 120), (900, 120));
        // Third second: only the last two seconds count, the first has dropped out.
        assert_eq!(average.update(1200, 180), (900, 120));
        // A window without frames carrying a header value leaves the previous one standing.
        assert_eq!(average.update(1200, 180), (300, 60));
        assert_eq!(average.update(1200, 180), (0, 0));
        let w = Window { host_latency: 900, host_latency_n: 120, ..Default::default() };
        assert_eq!(host_lines(&w), vec![("Host processing".to_owned(), "0.8 ms".to_owned())]);
    }

    #[test]
    fn host_processing_is_the_header_average_and_a_host_report_never_changes_it() {
        // Header values are in 100 us units: 3 frames of 5, 5 and 5 are 0.5 ms.
        let header = Window { host_latency: 15, host_latency_n: 3, ..Default::default() };
        let lines = host_lines(&header);
        assert_eq!(lines, vec![("Host processing".to_owned(), "0.5 ms".to_owned())]);
        // Reports say 5.0 ms present to send: a separate line, the processing line is unchanged.
        let both = Window { timing_host_us: 15_000, timing_encode_us: 1_200, timing_n: 3, ..header };
        let lines = host_lines(&both);
        assert_eq!(lines[0], ("Host processing".to_owned(), "0.5 ms".to_owned()));
        assert_eq!(lines[1].0, "Host to network");
        assert_eq!(lines[1].1, "5.0 ms present to send, 0.4 ms encode");
        // Reports alone never produce a "Host processing" line.
        let reports = Window { timing_host_us: 15_000, timing_encode_us: 1_200, timing_n: 3, ..Default::default() };
        assert!(host_lines(&reports).iter().all(|(label, _)| label != "Host processing"));
    }
}

#[cfg(test)]
mod early_tests {
    use super::*;

    #[test]
    fn the_overlay_line_and_the_session_summary_carry_the_counters() {
        assert_eq!(early_line((130, 3), (70, 2), 2.0), "30 early/s, 1 whole in the last second, 3 fallbacks in all");
        let summary = early_summary(1200, 4, 8_500, 7);
        for part in ["1200 frames decoded ahead", "4 fell back", "7 chunks dropped", "8.5 ms"] {
            assert!(summary.contains(part), "{summary}");
        }
        let video = video_summary(3600, 12, 5);
        for part in ["3600 frames to the decoder", "12 dropped over the decoder queue", "5 replaced"] {
            assert!(video.contains(part), "{video}");
        }
    }
}
