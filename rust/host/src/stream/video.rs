//! The stream thread: display preparation, capture admission and pacing, encoding,
//! encoder recovery and the inline video send.
use super::*;

/// How soon the stream looks again when another thread held the running app.
const QUIT_SCAN_RETRY: Duration = Duration::from_millis(50);

/// Hand the program the stream's display shows (`pid`) to the running app, for
/// quitting it later. Only the stream of the client that launched the app:
/// another client's display shows its own programs; an app started from the
/// console has no owner. False, having waited for nothing, when another thread
/// holds the app: the stream thread never waits behind housekeeping. Whether
/// the program is a game a store client started takes the PC's process list,
/// read on a thread of its own with the app released.
pub(super) fn observe_foreground_game(h: &Shared, client: &str, program: &str, pid: u32) -> bool {
    let Some(mut current) = h.try_current_app() else {
        return false;
    };
    if let Some(app) = current.as_mut()
        && (app.owner.is_empty() || app.owner == client)
        && let Some(check) = app.observe_foreground(pid, program)
    {
        let host = h.clone();
        let started = thread::Builder::new()
            .name("foreground-game".into())
            .spawn(move || {
                let verdict = check.run();
                if let Some(app) = host.current_app.lock().unwrap().as_mut() {
                    app.finish_foreground(check, verdict);
                }
            });
        if let Err(error) = started {
            tracing::warn!(%error, pid, "the program in front was not checked; looking again");
            app.abandon_foreground();
        }
    }
    true
}

/// How often completed encoder output is collected while a frame is in flight.
pub(super) const OUTPUT_POLL: Duration = Duration::from_micros(100);

#[derive(Clone, Copy)]
enum AdmissionWait {
    Grid,
    Capture,
    FreshRate,
    Repeat,
    Encoder,
    Sender,
}

/// TRACE-only elapsed time in admission branches, including any output polling
/// and sending they perform. One aggregate per claim avoids logging every poll.
#[derive(Default)]
struct AdmissionTrace {
    elapsed: [Duration; 6],
    passes: [u32; 6],
}

impl AdmissionTrace {
    fn measure<T>(trace: &mut Option<Self>, reason: AdmissionWait, run: impl FnOnce() -> T) -> T {
        let started = trace.as_ref().map(|_| Instant::now());
        let result = run();
        if let (Some(trace), Some(started)) = (trace, started) {
            let index = reason as usize;
            trace.elapsed[index] += started.elapsed();
            trace.passes[index] = trace.passes[index].saturating_add(1);
        }
        result
    }
    fn micros(&self, reason: AdmissionWait) -> u64 {
        self.elapsed[reason as usize]
            .as_micros()
            .min(u128::from(u64::MAX)) as u64
    }
}

/// How long an encoder that fails mid-stream is recreated before the session
/// gives up: a GPU busy with a game or a driver reset costs frames, not the
/// stream. Moonlight keeps waiting with a frozen picture meanwhile.
pub(super) const ENCODER_RECOVERY: Duration = Duration::from_secs(20);

pub(super) fn encoder_progress(
    failing: &mut Option<Instant>,
    produced_frame: bool,
    now: Instant,
) -> Result<()> {
    if produced_frame {
        if let Some(since) = *failing {
            tracing::info!(
                recovery_ms = now.duration_since(since).as_millis(),
                "encoder output resumed"
            );
        }
        *failing = None;
    } else if failing.is_some_and(|since| now.duration_since(since) >= ENCODER_RECOVERY) {
        anyhow::bail!("the encoder did not produce a frame during recovery");
    }
    Ok(())
}

// Keep recovery testable without constructing a GPU or an AMF runtime.
pub(super) trait EncoderOutput {
    fn poll(&mut self) -> Result<Vec<butterpollo_windows::encoder::Encoded>>;
    fn pending(&self) -> bool;
    fn backlog(&self) -> usize;
    fn backlog_limit(&self) -> usize;
    fn log_stall(&self);
    fn device_removed(&self) -> Option<DeviceLost> {
        None
    }
}

impl EncoderOutput for Encoder {
    fn poll(&mut self) -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
        self.poll()
    }
    fn pending(&self) -> bool {
        self.pending()
    }
    fn backlog(&self) -> usize {
        self.backlog()
    }
    fn backlog_limit(&self) -> usize {
        self.backlog_limit()
    }
    fn log_stall(&self) {
        self.log_stall();
    }
    fn device_removed(&self) -> Option<DeviceLost> {
        self.device_removed()
    }
}

#[derive(Default)]
pub(super) struct EncoderRecovery {
    pub(super) failing: Option<Instant>,
    pub(super) backlog_since: Option<Instant>,
    pub(super) recreations: u32,
    pub(super) device_loss: Option<DeviceLost>,
    pub(super) progress: Option<Arc<crate::stall_watch::Progress>>,
    #[cfg(any(debug_assertions, test))]
    pub(super) stall: crate::soak_fault::EncoderStall,
}

impl EncoderRecovery {
    pub(super) fn pending(&self, encoder: &impl EncoderOutput) -> bool {
        if self.device_loss.is_some() {
            return false;
        }
        #[cfg(any(debug_assertions, test))]
        if !self.stall.held.is_empty() {
            return true;
        }
        encoder.pending()
    }
    pub(super) fn backlog(&self, encoder: &impl EncoderOutput) -> usize {
        let backlog = encoder.backlog();
        #[cfg(any(debug_assertions, test))]
        let backlog = backlog + self.stall.held.len();
        backlog
    }
    pub(super) fn output(
        &mut self,
        output: Vec<butterpollo_windows::encoder::Encoded>,
        now: impl Fn() -> Instant,
    ) -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
        #[cfg(any(debug_assertions, test))]
        let output = self.stall.output(output, now())?;
        encoder_progress(&mut self.failing, !output.is_empty(), now())?;
        Ok(output)
    }
    /// A recreated codec cannot send pictures retained from its predecessor.
    pub(super) fn recreated(&mut self, session: &Session) {
        #[cfg(any(debug_assertions, test))]
        self.stall.held.clear();
        session.request_host_keyframe();
    }
    /// Output the encoder has finished. Only failures lasting the recovery
    /// budget end the stream; the next frame recreates a dropped encoder.
    pub(super) fn collect(
        &mut self,
        encoder: &mut Option<impl EncoderOutput>,
        warnings: &butterpollo_core::session::Warnings,
        now: impl Fn() -> Instant,
    ) -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
        let Some(active) = encoder.as_mut() else {
            return Ok(vec![]);
        };
        if let Some(progress) = &self.progress {
            progress.mark(crate::stall_watch::Phase::Output);
        }
        match active.poll() {
            Ok(output) => self.output(output, now),
            Err(error) => {
                if let Some(loss) = active
                    .device_removed()
                    .or_else(|| DeviceLost::from_error(&error))
                {
                    self.device_loss = Some(loss);
                    self.failing.get_or_insert_with(&now);
                    return Ok(vec![]);
                }
                let since = *self.failing.get_or_insert_with(&now);
                if now().duration_since(since) >= ENCODER_RECOVERY {
                    return Err(error.context("the encoder kept failing"));
                }
                warnings.set("encoder_recovery", format!("Encoder output failed ({error:#}); recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats."));
                active.log_stall();
                *encoder = None;
                Ok(vec![])
            }
        }
    }
    pub(super) fn poll_full(
        &mut self,
        encoder: &mut Option<impl EncoderOutput>,
        warnings: &butterpollo_core::session::Warnings,
        now: impl Fn() -> Instant,
    ) -> Result<Option<Vec<butterpollo_windows::encoder::Encoded>>> {
        if self.failing.is_none() {
            self.recreations = 0;
        }
        let limit = butterpollo_core::stream_policy::encoder_stall_limit(self.recreations);
        let backlog_since = *self.backlog_since.get_or_insert_with(&now);
        // The host can be descheduled while the GPU finishes its work. Poll
        // before declaring a stall, or a late wake destroys healthy output
        // and unnecessarily restarts the encoder.
        let output = self.collect(encoder, warnings, &now)?;
        if !output.is_empty() {
            self.backlog_since = None;
            return Ok(Some(output));
        }
        // collect can have dropped a failed encoder or requested device
        // recovery. Let the session handle that before looking for a stall.
        if encoder.is_none() || self.device_loss.is_some() {
            self.backlog_since = None;
            return Ok(None);
        }
        if now().duration_since(backlog_since) >= limit {
            self.backlog_since = None;
            if let Some(loss) = encoder.as_ref().unwrap().device_removed() {
                self.device_loss = Some(loss);
                self.failing.get_or_insert_with(&now);
                return Ok(None);
            }
            let since = *self.failing.get_or_insert_with(&now);
            if now().duration_since(since) >= ENCODER_RECOVERY {
                anyhow::bail!("the encoder stopped returning frames");
            }
            warnings.set("encoder_recovery", format!("The encoder returned no frame for {} ms; recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats.", limit.as_millis()));
            tracing::warn!(
                wait_ms = limit.as_millis(),
                recreation = self.recreations + 1,
                recovery_ms = now().duration_since(since).as_millis(),
                "encoder stall recovery"
            );
            encoder.as_ref().unwrap().log_stall();
            self.recreations += 1;
            *encoder = None;
            return Ok(None);
        }
        Ok(Some(output))
    }
}

/// How long after an AMF reference-invalidation recovery the session sends a
/// keyframe anyway, in case the client's decoder could not follow it.
const RFI_CONFIRM_KEYFRAME: Duration = Duration::from_secs(1);

/// The interval to the frame after one captured at `at` while the client sends phase
/// lock reports: its refresh with the phase correction. `None` otherwise, and then the
/// caller keeps its original pacing.
fn phase_interval(s: &Session, at: Instant, period: Duration) -> Option<Duration> {
    let mut sync = s.phase_sync.lock().unwrap();
    let interval = sync.interval(at, period)?;
    if sync.log_due(at) {
        tracing::info!(
            client = %s.launch.client.name,
            interval_ns = interval.as_nanos() as u64,
            stream_period_ns = period.as_nanos() as u64,
            "phase lock applied"
        );
    }
    Some(interval)
}

/// A switch the session loop refused or could not make: the stream keeps `from`, and a rate
/// change the switch carried is answered with `reason`.
fn refuse_switch(
    s: &Session,
    from: butterpollo_core::reconfigure::Mode,
    reason: butterpollo_core::rate_change::Reason,
) {
    let mut reconfigure = s.reconfigure.lock().unwrap();
    reconfigure.refused(from);
    s.rate_changes.lock().unwrap().switched(from, Some(reason));
}

impl Media {
    pub(super) fn spawn_session(
        self: &Arc<Self>,
        h: &Shared,
        s: &Arc<Session>,
    ) -> std::io::Result<thread::JoinHandle<()>> {
        let m = self.clone();
        let worker_h = h.clone();
        let worker_s = s.clone();
        thread::Builder::new()
            .name("session".into())
            .spawn(move || {
                let h = worker_h;
                let s = worker_s;
                tracing::info!(client=%s.launch.client.name,"CLIENT CONNECTED");
                let paired_h = h.clone();
                let client_uuid = s.launch.client.uuid.clone();
                let last_seen = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if let Err(error) = thread::Builder::new().name("client last seen".into()).spawn(move || {
                    let mut paired = paired_h.paired.write().unwrap();
                    if let Some(client) = paired
                        .clients
                        .iter_mut()
                        .find(|c| c.uuid == client_uuid)
                    {
                        let last_seen = last_seen.max(client.extra.get("last_seen").and_then(serde_json::Value::as_u64).unwrap_or(0));
                        client.extra.insert(
                            "last_seen".into(),
                            serde_json::json!(last_seen),
                        );
                        if let Err(error) = paired.save(&paired_h.paired_path) {
                            tracing::warn!(%error,"client last-seen persistence failed");
                        }
                    }
                }) {
                    tracing::warn!(%error, "client last-seen worker failed");
                }
                let mut com = None;
                let mut stream_preparation = None;
                let mut latest = None;
                let mut encoder = None;
                let mut client_commands = None;
                let mut audio = None;
                let mut pyrowave_sender = None;
                let mut webrtc_sender = None;
                let mut keyframe_loss_memory = None;
                // Ends the host-side timing record's writer, which writes its footer, on every
                // way out of the stream.
                let timing_record = crate::timing_record::Detach(s.clone());
                #[cfg(test)]
                let mut fixture_preparation = None;
                // A co-op player's stream ends with its Nucleus Co-op session.
                let mut coop_watch = None;
                let result = (|| -> Result<()> {
                    coop_watch = crate::nucleus::Watch::start(&h, &s)?;
                    #[cfg(test)]
                    if let Some(fixture) = &h.reconnect_fixture {
                        fixture_preparation = s.launch.preparation.lock().unwrap().take();
                        return fixture.stream(&m, &h, &s);
                    }
                    com = Some(ComGuard::new()?);
                    let _priority = Priority::new();
                    let _streaming = butterpollo_windows::timing::StreamingScope::enter();
                    let mut config_version = h.config_version.load(Ordering::Acquire);
                    let mut c = effective_config(&h, &s.launch)?;
                    if s.config.vrr_low_latency {
                        c.values.insert("wgc_slot_aligned_publish".into(), "false".into());
                    }
                    let timer = butterpollo_windows::timing::Timer::new()?;
                    let output = s
                        .launch
                        .client
                        .extra
                        .get("output_name_override")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(c.get("output_name", ""));
                    if s.launch.role == Role::InputOnly {
                        let monitors = butterpollo_windows::input::on_input_desktop(
                            butterpollo_windows::display::monitors,
                        )?;
                        let monitor = monitors
                            .iter()
                            .find(|m| m.matches(output))
                            .or_else(|| monitors.iter().find(|m| m.primary))
                            .context("input display unavailable")?;
                        *s.output.write().unwrap() = monitor.display_name.clone();
                        client_commands = Some(crate::process::ClientCommands::start(&h, &s)?);
                        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(10));
                        }
                        return Ok(());
                    }
                    let initial = s
                        .launch
                        .preparation
                        .lock()
                        .unwrap()
                        .take();
                    // A stream request at another rate than the launch keeps an
                    // owned virtual display and raises its refresh where needed.
                    // A co-op player's window follows the stream's size instead.
                    let initial = match initial {
                        Some(p) if s.launch.nucleus.is_none() && !p.prepared().display.matches(&s.config) => Some(
                            crate::display_session::retime(p.into_prepared(), &h, &s.launch, &s.config, &c)?,
                        ),
                        Some(p) => Some(Ok(p.into_prepared())),
                        None => None,
                    };
                    let stream_preparation = stream_preparation.insert(match initial {
                        Some(Ok(prepared)) => prepared,
                        previous => {
                            if previous.is_some() {
                                tracing::info!(client = %s.launch.client.name, width = s.config.width, height = s.config.height, fps_millihz = s.config.fps_millihz(), hdr = s.config.hdr, "negotiated mode differs from launch; preparing display again");
                            }
                            drop(previous);
                            let previous = h.app_display.lock().unwrap().remove(&s.launch.client.uuid);
                            drop(previous);
                            match s.launch.nucleus {
                                Some(_) => crate::nucleus::prepare(&h, &s.launch, &s.config, &c, crate::nucleus::WINDOW_WAIT)?,
                                None => crate::display_session::prepare_stream(
                                    &h, &s.launch, &s.config, &c,
                                )?,
                            }
                        }
                    });
                    stream_preparation.report_limiter(&s.launch.warnings, &c);
                    let prepared = stream_preparation.display.clone();
                    // A game window is no display to keep for the device.
                    if s.launch.role == Role::Stream
                        && s.launch.nucleus.is_none()
                        && prepared.kept_for_reconnect()
                    {
                        let mut displays = h.app_display.lock().unwrap();
                        if !s.stopping() {
                            displays.insert(s.launch.client.uuid.clone(), (prepared.clone(), None));
                        }
                    }
                    let output = prepared.output();
                    *s.output.write().unwrap() = output.clone();
                    let (latest, capture_wake) = m.open_session_capture(&s, &c, &prepared, &mut latest)?;
                    let first = {
                        let deadline = Instant::now() + Duration::from_secs(10);
                        loop {
                            // No frame, encoder or filter is owned during startup.
                            latest.release_generation(&capture_wake);
                            if let Some(image) = latest.current()? { break image; }
                            if s.stopping() || h.stop.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            if Instant::now() >= deadline {
                                anyhow::bail!("capture produced no GPU frame on {} using {} within 10 seconds; check that the selected display is powered on, or select a virtual display for headless streaming", prepared.output(), prepared.capture());
                            }
                            if let Some(image) = latest.wait_for_frame(&timer, &capture_wake, (Instant::now() + Duration::from_millis(50)).min(deadline))? { break image; }
                        }
                    };
                    require_hdr_source(&s, || first.gpu.display_in_hdr(), false)?;
                    s.source_checked.store(true, Ordering::Release);
                    m.configure_encoder(&s, &c, &first, &prepared, &mut encoder)?;
                    drop(first);
                    let mut focus = crate::stream_focus::Focus::start(
                        &c,
                        butterpollo_core::encoder_focus::Context {
                            codec: s.config.codec,
                            ten_bit: s.config.ten_bit(),
                            pyrowave: s.config.codec == 3,
                            packed_444: s.config.chroma_packing != 0,
                            d3d12: butterpollo_windows::compute::enabled(&c),
                            preanalysis: crate::stream_focus::preanalysis(&c, s.config.ten_bit()),
                            support: h.focus_support(s.config.codec, s.config.ten_bit()),
                        },
                        &s.encoder_focus,
                        &s.encoder_focus_state,
                        &s.launch.warnings,
                    );
                    // One frame a second gets QP feedback for `qp_delta`.
                    let focus_verify = c
                        .boolean("encoder_focus_verify", false)
                        .then(|| s.config.fps.max(1));
                    let mut staging = None;
                    client_commands = Some(crate::process::ClientCommands::start(&h, &s)?);
                    audio = Some(m.spawn_audio(&h, &s)?);
                    configure_video_fec(&s, &c)?;
                    configure_adaptive_bitrate(&s, &c);
                    s.configure_link_rate(c.link_rate_adaptation(), max_bitrate_kbps(&s, &c));
                    // Packet adaptation is opt-in, or automatic for a client that
                    // reports a wireless route. Explicit pacing and host bitrate
                    // adaptation keep their own policies.
                    let keyframe_loss_pacing = crate::video_send::recovery::configure(&s, &c);
                    let automatic_pacing = s.automatic_pacing_enabled();
                    keyframe_loss_memory = Some(h.configure_keyframe_loss(&s, keyframe_loss_pacing));
                    let mut packetizer = VideoPacketizer {
                        sequence: 0,
                        iv_counter: 0,
                        frame: 1,
                        packet_size: s.config.packet_size,
                        fec_percent: c.integer("fec_percentage", 20).clamp(0, 100) as usize,
                        min_fec: s.config.min_fec,
                        key: if s.config.encryption & 2 != 0 {
                            Some(s.launch.key)
                        } else {
                            None
                        },
                    };
                    let next_wire_frame = std::cell::Cell::new(u64::from(packetizer.frame));
                    // New pictures claimed and not yet sent, for the stage timings.
                    let claims = std::cell::RefCell::new(butterpollo_core::performance::Claims::default());
                    let start = Instant::now();
                    let start_qpc_ms = butterpollo_windows::timing::qpc_ms();
                    *s.media_epoch.write().unwrap() = Some(start);
                    // Present times come from ETW only if the stamper really has a tracker.
                    let mut present_stamper = (prepared.capture() == "wgc").then(butterpollo_windows::present_timing::Stamper::default);
                    let tracks_presents = present_stamper.as_ref().is_some_and(|stamper| stamper.tracks_presents());
                    // The opt-in host-side record: off, and no thread or file, unless the
                    // environment names a directory.
                    *s.timing_record.write().unwrap() = crate::timing_record::attach(
                        crate::timing_record::directory().as_deref(),
                        &s.launch.id,
                        butterpollo_core::timing_record::Meta {
                            codec: butterpollo_core::timing_record::codec_name(s.config.codec),
                            capture: prepared.capture().to_string(),
                            present_source: butterpollo_core::timing_record::present_source(tracks_presents),
                            media_epoch_qpc_ms: start_qpc_ms,
                        },
                    );
                    webrtc_sender = if s.launch.transport == butterpollo_core::session::Transport::WebRtc {
                        Some(crate::webrtc::session::Sender::new(h.webrtc_links.take(&s.launch.id).context("the WebRTC media link is missing for this session")?))
                    } else { None };
                    pyrowave_sender = if s.config.codec == 3 { Some(crate::pyrowave_send::Sender::new(m.video.clone(),s.clone(),c.clone(),h.clone(),start,present_stamper.take())?) } else { None };
                    // The stream's size and rate. A client whose display changes can ask
                    // for others mid-stream (0x5532); the encoder is rebuilt from this copy.
                    let mut stream = s.config.clone();
                    let mut rates = butterpollo_core::reconfigure::Rates::new(&stream, c.get("minimum_fps_target", "20"));
                    // The rate pacing follows, and the period the send path reads.
                    let mut paced_rate = stream.fps_millihz();
                    let stream_period = std::cell::Cell::new(rates.period);
                    // A mode change being applied: the mode before it, and since when.
                    let mut switching: Option<(butterpollo_core::reconfigure::Mode, Instant)> = None;
                    let mut cadence = butterpollo_core::stream_policy::Cadence::new(Instant::now(), rates.period, c.boolean("wgc_pacing_smoothing", true));
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
                        || butterpollo_core::stream_policy::Pacing::from_config(&c) == butterpollo_core::stream_policy::Pacing::Arrival;
                    let source_phase_eligible = vrr || c.boolean("frame_pacing_predictive", true);
                    let new_pacer = |rates: &butterpollo_core::reconfigure::Rates| {
                        let predictive = c.boolean("frame_pacing_predictive", true);
                        butterpollo_core::stream_policy::Pacer::new(Instant::now(), rates.period)
                            .with_static_period(rates.static_period)
                            .with_prediction(predictive)
                            .with_prediction_floor(if vrr { Duration::from_millis(2) } else { Duration::ZERO })
                            // Source phase follows the opened backend, set per image below.
                            .with_source_phase(false)
                            .with_spacing(if vrr { 0.5 } else { 0.75 })
                    };
                    let mut pacer = new_pacer(&rates);
                    // The claim grid's period from before a phase lock, restored when it ends.
                    let mut unlocked_grid_period: Option<Duration> = None;
                    let due = cadence.deadline();
                    let mut last_stamp = start;
                    let mut live_at = due;
                    let mut rebuild_encoder = false;
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
                        ..Default::default()
                    };
                    // Separate encoder failures in this session, each counted once.
                    let (mut failures, mut counted_failure) = (0u32, None);
                    let mut base_config = c.clone();
                    let mut runtime_config = c.clone();
                    let mut calibration_due = s.launch.client.extra.get("hdr_profile")
                        .and_then(serde_json::Value::as_str).filter(|profile| !profile.is_empty())
                        .map(|_| Instant::now() + Duration::from_secs(30));
                    // The fullscreen game on this display, for quitting a game
                    // that a store client started outside the app's processes.
                    let mut quit_scan: Option<butterpollo_windows::foreground::Tracker> = None;
                    let mut quit_scan_due = Instant::now() + Duration::from_secs(1);
                    let _background_windows = if s.launch.role == Role::Stream
                        && butterpollo_core::background::enabled(&c)
                    {
                        background::GameWindows::start(h.clone(), s.clone())
                            .inspect_err(|error| tracing::warn!(%error, "background stream window placement unavailable"))
                            .ok()
                    } else {
                        None
                    };
                    let mut metadata_due = Instant::now() + Duration::from_secs(1);
                    // A keyframe a while after an AMF reference-invalidation recovery.
                    let mut confirm_keyframe: Option<Instant> = None;
                    let mut timing_due = Instant::now() + Duration::from_secs(5);
                    // Pipeline frame rates: the capture slot's counts this stream
                    // has taken, and the 5 s timing log's own window.
                    let mut pipeline_follow = butterpollo_core::pipeline_rates::Follow::default();
                    let mut pipeline_log = butterpollo_core::pipeline_rates::Sampler::default();
                    let mut last_image: Option<Arc<GpuImage>> = None;
                    // The couch menu's panel; one atomic load a frame while closed.
                    let mut menu = crate::couch_menu::VideoOverlay::default();
                    // The device draws the pointer (0x5540): it is left out of the video.
                    let mut pointer_left_out = false;
                    let mut pointer_warned = false;
                    let mut encoded_at = Instant::now();
                    // Keep the legacy age split beside WGC's signed raw stamp
                    // offset: a future stamp must not look like instant delivery.
                    let mut claim_ages: Vec<(u64, u64)> = Vec::with_capacity(1024);
                    let mut wgc_stamp_ages: Vec<f64> = Vec::with_capacity(1024);
                    // The 5 s timing summary is an information log, kept on by
                    // default: a user's ordinary log must show where a stream
                    // lost time. With logging below information, skip collecting
                    // its samples and computing its percentiles.
                    let mut timings_logged = tracing::enabled!(tracing::Level::INFO);
                    // The first pacing decision for the newest fresh frame, kept
                    // for the per-claim trace.
                    let mut first_seen: Option<(usize, Instant, Option<Duration>, Option<Instant>)> = None;
                    let mut video_qos = Tagged::default();
                    let mut batch = butterpollo_windows::net::Batch::default();
                    let mut packets = Vec::new();
                    let trace_send = tracing::enabled!(target: "pacing", tracing::Level::TRACE);
                    let mut admission_trace = trace_send.then(AdmissionTrace::default);
                    let mut trace_attempt = 0_u64;
                    batch.waits = trace_send.then(Default::default);
                    let mut network_pacer = butterpollo_core::network_pacing::Pacer::new(Instant::now());
                    let mut link = None;
                    // The address `link` was routed for.
                    let mut routed = None;
                    let mut link_due = Instant::now();
                    let mut reported_pacing = None;
                    // Encoder admission needs the route before its very first
                    // picture, independently of the asynchronous sender's cache.
                    let mut encode_route = None;
                    let mut encode_route_due = Instant::now();
                    let mut recovery_packetizer = packetizer.clone();
                    // The largest frame with full parity, for the FEC percentage,
                    // packet size and FEC choice it was found at.
                    let mut fec_whole = ((usize::MAX, 0, false), 0);
                    let mut fec_whole_due = Instant::now();
                    let mut fec_reported = None;
                    // A frame that holds the stream thread this long is logged
                    // with where the time went, at most every five seconds.
                    const SLOW_SEND: Duration = Duration::from_millis(100);
                    let mut slow_send_reported: Option<Instant> = None;
                    let send_outage = butterpollo_core::stream_policy::SendOutage::from_env();
                    // On a link paced near the stream bitrate, sending a frame
                    // takes a good part of a period; a sender thread lets the
                    // next picture be claimed meanwhile. On a fast link the
                    // inline send is shorter than the handoff costs.
                    let video_sender = std::cell::OnceCell::<crate::video_send::Sender>::new();
                    let send_thread = c.boolean("video_send_thread", true);
                    let mut sender_decided = false;
                    // An inline frame was paced near the stream bitrate, as after
                    // a lost keyframe: later frames move to the sender thread.
                    let mut inline_paced = false;
                    let mut send_loss = butterpollo_core::stream_policy::SendLossRecovery::default();
                    // `video_slice_delivery`: AMF returns a P-frame in slices and
                    // they are sent in FEC blocks as they arrive, here or on the
                    // sender thread. Off for the rest of the session after a
                    // frame passes its byte cap.
                    let delivery = slices::Delivery::new(usize::from(butterpollo_core::encoder_policy::slice_delivery(&c, s.config.codec)));
                    let slice_blocks = delivery.blocks();
                    let mut slice_route = slices::Route::default();
                    let mut streaming: Option<slices::Streaming> = None;
                    // The streamed frames' payload buffer, kept between frames.
                    let mut slice_spare: Vec<u8> = Vec::new();
                    let mut send_drop = butterpollo_core::stream_policy::SendDrop::from_env();
                    let batch_kb = match c.integer("video_max_batch_size_kb", 64) {
                        16 => 16,
                        32 => 32,
                        _ => 64,
                    };
                    let mut send_frames = |output: Vec<butterpollo_windows::encoder::Encoded>,
                                           peer: std::net::SocketAddr,
                                           call_latency: Duration,
                                           source: &Source,
                                           processing_started: Instant|
                     -> Result<()> {
                        progress.mark(crate::stall_watch::Phase::Send);
                        if !output.is_empty() {
                            s.launch.warnings.clear("encoder_recovery");
                            source.device_recovered(&source.warnings);
                        }
                        if let Some(sender) = &pyrowave_sender { return sender.submit(output,peer,call_latency,processing_started,&mut claims.borrow_mut()); }
                        if let Some(sender) = &webrtc_sender { return sender.submit(&s,output,call_latency,processing_started,&mut claims.borrow_mut()); }
                        // A large picture or feedback can make even a fast
                        // route block capture. Hand off before that frame sends.
                        let loss_paced = send_thread && video_sender.get().is_none() && s.keyframe_loss_active();
                        // A streamed frame stays on this path until its last unit.
                        if send_thread && video_sender.get().is_none() && !slices::open(&slice_route, &streaming) && (!sender_decided || !output.is_empty() || inline_paced || loss_paced) {
                            let mid_stream = sender_decided;
                            sender_decided = true;
                            let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
                            packetizer.fec_percent = s.fec_percent();
                            let bitrate = s.bitrate.load(Ordering::Relaxed);
                            let crate::video_send::recovery::Pace { bps, needed } =
                                crate::video_send::recovery::pace(&s, &c, &packetizer, stream_period.get(), peer, route);
                            let mut frame_packetizer = packetizer.clone();
                            let large_frame = output.iter().any(|frame| {
                                frame_packetizer.fec_percent = s.video_fec_percent(frame.idr || frame.after_invalidation, frame.bytes.len());
                                let wire = butterpollo_core::network_pacing::video_frame_wire_rate_bps(&frame_packetizer, frame.bytes.len(), stream_period.get(), peer.is_ipv6());
                                butterpollo_core::network_pacing::paced(bps, wire)
                            });
                            if std::mem::take(&mut inline_paced) || loss_paced || large_frame || automatic_pacing || c.boolean("host_adaptive_bitrate", false) || butterpollo_core::network_pacing::paced(bps, needed) {
                                // The sender continues the inline sends' frame,
                                // packet and IV numbering; nothing sends inline
                                // after this.
                                let handoff = crate::video_send::Handoff {
                                    packetizer: packetizer.clone(),
                                    present_stamper: present_stamper.take(),
                                    last_stamp,
                                    pacing_due: network_pacer.due(),
                                    send_loss: std::mem::take(&mut send_loss),
                                    slice_blocks,
                                    slices: s.config.slices as usize,
                                };
                                let _ = video_sender.set(crate::video_send::Sender::new(m.video.clone(), s.clone(), c.clone(), h.clone(), start, handoff)?);
                                tracing::info!(pacing_bps = bps, bitrate_kbps = bitrate, wire_budget_bps = needed, automatic_pacing, large_frame, mid_stream, wire_frame = packetizer.frame, "video frames are sent on their own thread for current or adaptive pacing");
                            }
                        }
                        // Units of a sliced frame: stream them here, or join them
                        // into the whole frame the other paths send.
                        let output = if output.iter().any(|frame| frame.unit != butterpollo_windows::encoder::Unit::Whole) || slice_route.streaming() {
                            let threaded = video_sender.get().is_some();
                            delivery.sync(&s.stats);
                            packetizer.fec_percent = s.frame_fec_percent(false);
                            let mut routed = Vec::with_capacity(output.len());
                            for frame in output {
                                let result = slice_route.route(frame, |first| {
                                    delivery.streams() && first.frame_cap_bytes.is_some_and(|cap| packetizer.streamable(slice_blocks, cap as usize))
                                });
                                // On the sender thread, it sees the new frame begin and drops the old one itself.
                                if result.interrupted && !threaded {
                                    // Its index is consumed if no block left, or the client sees no gap.
                                    let next = slices::drop_frame(&mut streaming, &mut packetizer, &mut slice_spare);
                                    s.launch.warnings.event("network_frame", "A streamed video frame lost its last slice; requesting a recovery frame.", butterpollo_core::session::EVENT_PERIOD);
                                    s.request_host_keyframe();
                                    next_wire_frame.set(u64::from(next));
                                }
                                routed.extend(result.frame);
                            }
                            routed
                        } else {
                            output
                        };
                        if let Some(sender) = video_sender.get() { return sender.submit_tick(output, peer, call_latency, &mut claims.borrow_mut(), processing_started); }
                        let polled = Instant::now();
                        let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
                        'frames: for frame in output {
                            if frame.unit != butterpollo_windows::encoder::Unit::Whole {
                                // One unit of a P-frame sent block by block.
                                let last = matches!(frame.unit, butterpollo_windows::encoder::Unit::Last { .. });
                                if matches!(frame.unit, butterpollo_windows::encoder::Unit::Slice { index: 0 } | butterpollo_windows::encoder::Unit::Last { index: 0 }) {
                                    let started = Instant::now();
                                    let encode = frame.latency.unwrap_or(call_latency);
                                    let claimed = polled.checked_sub(encode).unwrap_or(polled);
                                    let captured = frame.presentation.unwrap_or(claimed);
                                    let age = micros(claimed.saturating_duration_since(captured));
                                    let claim = claims.borrow_mut().take(frame.presentation);
                                    let stamp = present_stamper.as_mut().map_or(captured, |stamper| stamper.stamp(captured, &prepared.output())).max(last_stamp + Duration::from_nanos(11_112));
                                    last_stamp = stamp;
                                    let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                                    packetizer.fec_percent = s.frame_fec_percent(false);
                                    let wire_frame = packetizer.frame;
                                    let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
                                    let bitrate = s.bitrate.load(Ordering::Relaxed);
                                    let crate::video_send::recovery::Pace { bps, needed } =
                                        crate::video_send::recovery::pace(&s, &c, &packetizer, stream_period.get(), peer, route);
                                    let safe = butterpollo_core::network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
                                    let bps = s.keyframe_loss_bps(bps, safe, wire_frame, false);
                                    inline_paced |= butterpollo_core::network_pacing::paced(bps, needed);
                                    let packetize_started = Instant::now();
                                    let processing = micros(packetize_started.saturating_duration_since(processing_started));
                                    let cap = frame.frame_cap_bytes.unwrap_or(0) as usize;
                                    streaming = match packetizer.streamed_in(std::mem::take(&mut slice_spare), timestamp, processing, slice_blocks, slice_blocks.max(s.config.slices as usize), cap) {
                                        Ok(streamed) => Some(slices::Streaming {
                                            frame: streamed, wire_frame, timestamp, claim, claimed, age, started, packetize_started,
                                            first_polled: polled, last_polled: polled, payload_bytes: 0, processing, bps,
                                            sent: slices::Sent::new(batch.dropped), failed: false,
                                        }),
                                        Err(error) => {
                                            delivery.turn_off();
                                            s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame."), butterpollo_core::session::EVENT_PERIOD);
                                            s.request_host_keyframe();
                                            next_wire_frame.set(u64::from(slices::refuse_frame(&mut packetizer)));
                                            None
                                        }
                                    };
                                }
                                let Some(st) = streaming.as_mut() else { continue 'frames };
                                st.unit(polled, frame.bytes.len());
                                if !st.failed && !st.sent.abandoned {
                                    let mut stopped = false;
                                    let mut fatal = None;
                                    let slices::Streaming { frame: streamed, sent, bps, wire_frame, .. } = st;
                                    let (bps, wire_frame) = (*bps, *wire_frame);
                                    let pushed = streamed.push(&mut packetizer, &frame.bytes, last, &mut packets, |shards, block| {
                                        sent.lost.push_block(block.data, block.parity);
                                        if block.index == 0 {
                                            next_wire_frame.set(u64::from(wire_frame.wrapping_add(1)));
                                            s.stats.video_frame.store(u64::from(wire_frame) + 1, Ordering::Release);
                                        }
                                        let mut remaining = &shards[..];
                                        while !remaining.is_empty() {
                                            if s.stopping() || h.stop.load(Ordering::Acquire) {
                                                stopped = true;
                                                return Ok(());
                                            }
                                            let now = Instant::now();
                                            if network_pacer.due() > now {
                                                timer.until_paced(network_pacer.due());
                                            }
                                            let budget = butterpollo_core::network_pacing::batch_payload_limit(
                                                bps, remaining[0].len(), peer.is_ipv6(), batch_kb * 1024,
                                            );
                                            let count = butterpollo_windows::net::Batch::count(remaining, budget);
                                            let refused = batch.dropped;
                                            let result = match send_outage.filter(|outage| outage.active(start, Instant::now())) {
                                                Some(outage) if outage.in_air() => Ok(remaining[..count].iter().map(Vec::len).sum()),
                                                Some(_) => {
                                                    batch.refuse(count);
                                                    Ok(0)
                                                }
                                                None => match send_drop.as_mut() {
                                                    Some(drop) => slices::send_lossy(&mut batch, &m.video, &remaining[..count], peer, drop),
                                                    None => batch.send(&m.video, &remaining[..count], peer),
                                                },
                                            };
                                            let bytes = match result {
                                                Ok(bytes) => bytes,
                                                Err(error) => {
                                                    fatal = Some(error);
                                                    return Ok(());
                                                }
                                            };
                                            sent.first_sent.get_or_insert(Instant::now());
                                            remaining = &remaining[count..];
                                            sent.packets += count;
                                            network_pacer.sent(Instant::now(), bytes, if bytes > 0 { count } else { 0 }, peer.ip().to_canonical().is_ipv6(), bps);
                                            s.stats.packets.fetch_add(count as u64, Ordering::Relaxed);
                                            s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                                            // As for a whole frame: once the socket refused more
                                            // than FEC repairs, stop spending the link on it.
                                            if sent.lost.sent(count, (batch.dropped - refused) as usize) {
                                                if send_loss.lost(Instant::now(), stream_period.get(), sent.lost.reached_socket()) {
                                                    s.request_send_loss_recovery();
                                                }
                                                sent.abandoned = true;
                                                break;
                                            }
                                        }
                                        Ok(())
                                    });
                                    if stopped {
                                        return Ok(());
                                    }
                                    if let Some(error) = fatal {
                                        return Err(error);
                                    }
                                    if let Err(error) = pushed {
                                        st.failed = true;
                                        if let Some(overflow) = slices::note_failure(&delivery, &s.stats, &error) {
                                            slices::report_overflow(&s, st.wire_frame, overflow);
                                        } else {
                                            s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                                        }
                                        s.request_host_keyframe();
                                        next_wire_frame.set(u64::from(packetizer.frame));
                                    }
                                }
                                if !last {
                                    continue 'frames;
                                }
                                let Some(st) = streaming.take() else { continue 'frames };
                                if st.failed {
                                    slice_spare = st.frame.into_buffer();
                                    continue 'frames;
                                }
                                // The last unit is out: account for the frame as for a whole one.
                                let slices::Streaming { frame: streamed, wire_frame, timestamp, claim, claimed, age, started, packetize_started, first_polled, last_polled, payload_bytes, processing, bps, sent, .. } = st;
                                let unrecoverable = sent.lost.unrecoverable();
                                s.record_link_sent(wire_frame, sent.packets, ((sent.packets * (packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 })) + sent.packets * butterpollo_core::network_pacing::overhead(peer.is_ipv6())) as u64);
                                if unrecoverable {
                                    s.keyframe_abandoned(wire_frame);
                                    tracing::debug!(frame = wire_frame, finished = streamed.is_done(), "video frame lost on send; the rest of it was not sent");
                                } else {
                                    send_loss.delivered(false);
                                }
                                if batch.dropped != sent.dropped_at_start {
                                    s.launch.warnings.event("network_send", "Video packets were dropped by the host after transient socket send failures. You may see stutter or recovery frames; lower bitrate and check the network adapter. The log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
                                }
                                // As the whole-frame path judges it, from the frame's size, so an
                                // abandoned frame reports as well as a finished one.
                                if (streamed.fec_limited() || packetizer.fec_limited(payload_bytes)) && fec_reported.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1)) {
                                    fec_reported = Some(Instant::now());
                                    s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
                                }
                                let latency = micros(frame.latency.unwrap_or(call_latency));
                                s.stats.latency_us.store(latency, Ordering::Relaxed);
                                s.stats.frames.fetch_add(1, Ordering::Relaxed);
                                let packet_bytes = packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 };
                                let frame_bytes = (sent.packets * packet_bytes) as u64;
                                let finished = Instant::now();
                                crate::video_send::recovery::report(&s, &c, crate::video_send::recovery::RecoveryFrame {
                                    frame: wire_frame, idr: false, after_invalidation: false,
                                    payload_bytes, cap_bytes: frame.frame_cap_bytes,
                                    cap_target_bytes: frame.frame_cap_target, reencoded: frame.reencoded,
                                    fec_limited: streamed.fec_limited() || packetizer.fec_limited(payload_bytes),
                                    wire_bytes: frame_bytes.saturating_add((sent.packets * butterpollo_core::network_pacing::overhead(peer.is_ipv6())) as u64),
                                    pace_bps: bps, elapsed: finished.saturating_duration_since(started), lost: unrecoverable,
                                });
                                {
                                    let mut performance = s.stats.performance.lock().unwrap();
                                    performance.record_timing(finished,butterpollo_core::performance::Timing{period:stream_period.get(),encode:latency,host:processing,age,sent:micros(finished.saturating_duration_since(claimed))},frame_bytes);
                                    if let Some(claim) = claim.filter(|_| !unrecoverable) {
                                        let stamps = slices::stamps(&claim, (first_polled, last_polled), streamed.early_blocks(), packetize_started, sent.first_sent, finished);
                                        performance.record_stages(&stamps, processing);
                                        s.record_sent(wire_frame, timestamp, &stamps, start);
                                    }
                                }
                                slice_spare = streamed.into_buffer();
                                if Instant::now() >= link_due {
                                    link = Some(butterpollo_windows::net::routed_link(peer));
                                    link_due = Instant::now() + Duration::from_secs(2);
                                }
                                continue 'frames;
                            }
                            let frame_started = Instant::now();
                            let encode = frame.latency.unwrap_or(call_latency);
                            let latency = micros(encode);
                            s.stats.latency_us.store(latency, Ordering::Relaxed);
                            let claimed = polled.checked_sub(encode).unwrap_or(polled);
                            let captured = frame.presentation.unwrap_or(claimed);
                            let age = micros(claimed.saturating_duration_since(captured));
                            let claim = claims.borrow_mut().take(frame.presentation);
                            let stamp_started = Instant::now();
                            let stamp = present_stamper.as_mut().map_or(captured, |stamper| stamper.stamp(captured, &prepared.output())).max(last_stamp + Duration::from_nanos(11_112));
                            let stamp_time = stamp_started.elapsed();
                            last_stamp = stamp;
                            // Wrap like the previous host; a saturating cast
                            // froze the clock after 13.25 hours.
                            let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                            if frame.bytes.is_empty() {
                                continue;
                            }
                            packetizer.fec_percent = s.video_fec_percent(frame.idr || frame.after_invalidation, frame.bytes.len());
                            let wire_frame = packetizer.frame;
                            let block_layout = packetizer.block_layout(frame.bytes.len());
                            let fec_limited = packetizer.fec_limited(frame.bytes.len());
                            let packet_bytes = packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 };
                            // A client that roamed takes another route.
                            if crate::video_send::recovery::new_route(&mut routed, peer) {
                                link = None;
                            }
                            let first_link = link.is_none();
                            let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
                            if first_link {
                                crate::video_send::recovery::warn_if_client_link_below_stream(&s, route);
                            }
                            let bitrate = s.bitrate.load(Ordering::Relaxed);
                            let crate::video_send::recovery::Pace { bps, needed } =
                                crate::video_send::recovery::pace(&s, &c, &packetizer, stream_period.get(), peer, route);
                            let safe = butterpollo_core::network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
                            let bps = s.keyframe_loss_bps(bps, safe, wire_frame, frame.idr);
                            inline_paced |= butterpollo_core::network_pacing::paced(bps, needed);
                            if Instant::now() >= link_due {
                                butterpollo_core::network_pacing::report_rate(&s.launch.warnings, bps, needed, c.integer("pacing_max_bitrate_kbps", 0));
                                if reported_pacing != Some(bps) {
                                    tracing::info!(pacing_bps=bps, wire_budget_bps=needed, link_bps=route.bps, client_link_mbps=s.client_link_mbps(), client_wireless=s.config.client_wireless, configured_kbps=c.integer("pacing_max_bitrate_kbps", 0), "network pacing selected");
                                    reported_pacing = Some(bps);
                                }
                            }
                            // A frame beyond Moonlight's packet limit (very high
                            // bitrates) costs that frame and a keyframe, not the
                            // session.
                            let packetize_started = Instant::now();
                            let processing = micros(packetize_started.saturating_duration_since(processing_started));
                            let prepared_blocks = packetizer.conventional_blocks(&frame.bytes,frame.idr,frame.after_invalidation,timestamp,processing);
                            if let Err(error) = &prepared_blocks {
                                s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                                s.request_host_keyframe();
                                drop(prepared_blocks);
                                next_wire_frame.set(u64::from(packetizer.frame));
                                continue;
                            }
                            let (packet_count, mut next_block) = prepared_blocks?;
                            let mut packetize_time = packetize_started.elapsed();
                            tracing::trace!(
                                target: "pacing",
                                stream_id = %s.launch.id,
                                frame = wire_frame,
                                presentation = ?frame.presentation,
                                "packetized picture"
                            );
                            // A keyframe, as at the start of every stream, may be too
                            // large for FEC; only ordinary frames make this worth showing.
                            if !frame.idr
                                && fec_limited
                                && fec_reported.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
                            {
                                fec_reported = Some(Instant::now());
                                s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
                            }
                            let frame_bytes = (packet_count * packet_bytes) as u64;
                            let dropped = batch.dropped;
                            let waits_before = batch.waits;
                            let mut pacing_wait = Duration::ZERO;
                            let mut send_time = Duration::ZERO;
                            let mut batches = 0;
                            let mut first_send = None;
                            let mut last_send = None;
                            let mut first_sent = None;
                            let mut lost = butterpollo_core::stream_policy::SendLoss::new(block_layout);
                            let mut abandoned = 0;
                            let mut sent_packets = 0;
                            'blocks: loop {
                                let block_started = Instant::now();
                                let block_count = match next_block(&mut packets) {
                                    Ok(count) => count,
                                    Err(error) => {
                                        s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                                        s.request_host_keyframe();
                                        drop(next_block);
                                        next_wire_frame.set(u64::from(packetizer.frame));
                                        continue 'frames;
                                    }
                                };
                                packetize_time += block_started.elapsed();
                                if block_count == 0 { break; }
                                next_wire_frame.set(u64::from(wire_frame.wrapping_add(1)));
                                s.stats.video_frame.store(u64::from(wire_frame) + 1, Ordering::Release);
                                let mut remaining = &packets[..block_count];
                                while !remaining.is_empty() {
                                    if s.stopping() || h.stop.load(Ordering::Acquire) {
                                        return Ok(());
                                    }
                                    let now = Instant::now();
                                    if network_pacer.due() > now {
                                        timer.until_paced(network_pacer.due());
                                        pacing_wait += now.elapsed();
                                    }
                                    let budget = butterpollo_core::network_pacing::batch_payload_limit(
                                        bps, remaining[0].len(), peer.is_ipv6(), batch_kb * 1024,
                                    );
                                    let count =
                                        butterpollo_windows::net::Batch::count(remaining, budget);
                                    let send_started = Instant::now();
                                    let refused = batch.dropped;
                                    let bytes = match send_outage.filter(|outage| outage.active(start, Instant::now())) {
                                        // Lost after the socket: counted as sent, only the client sees it.
                                        Some(outage) if outage.in_air() => remaining[..count].iter().map(Vec::len).sum(),
                                        Some(_) => {
                                            batch.refuse(count);
                                            0
                                        }
                                        None => match send_drop.as_mut() {
                                            Some(drop) => slices::send_lossy(&mut batch, &m.video, &remaining[..count], peer, drop)?,
                                            None => batch.send(&m.video, &remaining[..count], peer)?,
                                        },
                                    };
                                    let finished = Instant::now();
                                    first_sent.get_or_insert(finished);
                                    send_time += finished.duration_since(send_started);
                                    if trace_send {
                                        first_send.get_or_insert(send_started);
                                        last_send = Some(finished);
                                        batches += 1;
                                    }
                                    remaining = &remaining[count..];
                                    sent_packets += count;
                                    network_pacer.sent(Instant::now(), bytes, if bytes > 0 { count } else { 0 }, peer.ip().to_canonical().is_ipv6(), bps);
                                    s.stats.packets.fetch_add(count as u64, Ordering::Relaxed);
                                    s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                                    // The socket refused more of this frame than FEC
                                    // repairs: the client cannot decode it. Stop
                                    // spending the link on it, and make the next
                                    // frame the keyframe the client will wait for.
                                    if lost.sent(count, (batch.dropped - refused) as usize) {
                                        abandoned = packet_count - sent_packets;
                                        if send_loss.lost(Instant::now(), stream_period.get(), lost.reached_socket()) {
                                            s.request_send_loss_recovery();
                                        }
                                        break 'blocks;
                                    }
                                }
                            }
                            drop(next_block);
                            s.record_link_sent(wire_frame, packet_count, frame_bytes.saturating_add((packet_count * butterpollo_core::network_pacing::overhead(peer.is_ipv6())) as u64));
                            if lost.unrecoverable() {
                                s.keyframe_abandoned(wire_frame);
                                tracing::debug!(frame = packetizer.frame.wrapping_sub(1), idr = frame.idr, abandoned, "video frame lost on send; the rest of it was not sent");
                            } else {
                                send_loss.delivered(frame.idr);
                            }
                            if batch.dropped != dropped {
                                s.launch.warnings.event("network_send", "Video packets were dropped by the host after transient socket send failures. You may see stutter or recovery frames; lower bitrate and check the network adapter. The log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
                            }
                            s.stats.frames.fetch_add(1, Ordering::Relaxed);
                            let sent = Instant::now();
                            crate::video_send::recovery::report(&s, &c, crate::video_send::recovery::RecoveryFrame {
                                frame: wire_frame, idr: frame.idr, after_invalidation: frame.after_invalidation,
                                payload_bytes: frame.bytes.len(), cap_bytes: frame.frame_cap_bytes,
                                cap_target_bytes: frame.frame_cap_target, reencoded: frame.reencoded, fec_limited,
                                wire_bytes: frame_bytes.saturating_add((packet_count * butterpollo_core::network_pacing::overhead(peer.is_ipv6())) as u64),
                                pace_bps: bps, elapsed: sent.saturating_duration_since(frame_started), lost: lost.unrecoverable(),
                            });
                            if let (Some(first), Some(last), Some(before), Some(after)) = (first_send, last_send, waits_before, batch.waits) {
                                tracing::trace!(target: "pacing", frame=packetizer.frame.wrapping_sub(1), packets=packet_count, batches,
                                    pacing_wait_us=micros(pacing_wait), send_us=micros(send_time),
                                    first_send_us=micros(first.saturating_duration_since(start)), last_send_us=micros(last.saturating_duration_since(start)),
                                    writable_waits=after.count-before.count, writable_wait_us=micros(after.elapsed-before.elapsed),
                                    dropped=batch.dropped-dropped, stream_id=%s.launch.id, "send");
                            }
                            {
                                let mut performance = s.stats.performance.lock().unwrap();
                                performance.record_timing(sent,butterpollo_core::performance::Timing{period:stream_period.get(),encode:latency,host:processing,age,sent:micros(sent.saturating_duration_since(claimed))},frame_bytes);
                                // A frame abandoned on send never had its last packet sent.
                                if let Some(claim) = claim.filter(|_| !lost.unrecoverable()) {
                                    let stamps = claim.sent(polled, packetize_started, first_sent.unwrap_or(sent), sent);
                                    performance.record_stages(&stamps, processing);
                                    s.record_sent(wire_frame, timestamp, &stamps, start);
                                }
                            }
                            let record_time = sent.elapsed();
                            // The interface lookup takes a moment: refresh the
                            // link speed after the frame is out, for the next one.
                            let link_started = Instant::now();
                            if Instant::now() >= link_due {
                                link = Some(butterpollo_windows::net::routed_link(peer));
                                link_due = Instant::now() + Duration::from_secs(2);
                            }
                            let link_time = link_started.elapsed();
                            let took = frame_started.elapsed();
                            if took >= SLOW_SEND && slow_send_reported.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
                                slow_send_reported = Some(Instant::now());
                                let ms = |d: Duration| (d.as_secs_f64() * 10_000.).round() / 10.;
                                tracing::warn!(
                                    frame_ms = ms(took), socket_ms = ms(send_time), pacing_ms = ms(pacing_wait),
                                    timestamp_ms = ms(stamp_time), packetize_ms = ms(packetize_time),
                                    timings_ms = ms(record_time), link_ms = ms(link_time),
                                    packets = packet_count, bytes = frame_bytes, pacing_bps = bps, idr = frame.idr,
                                    "a video frame held the stream thread while it was sent; socket_ms is time in Windows' send calls (network driver and adapter), the other parts are the host's own work"
                                );
                            }
                        }
                        Ok(())
                    };
                    (|| -> Result<()> {
                        while !s.stopping() && !h.stop.load(Ordering::Acquire) {
                            progress.mark(crate::stall_watch::Phase::Loop);
                            if let Some(sender) = video_sender.get() { sender.backlog()?; }
                            if let Some(loss) = recovery.device_loss.take() {
                                latest.latest.device_lost(loss, &latest.warnings, Instant::now())?;
                                encoder = None;
                            }
                            if Instant::now() >= live_at {
                                live_at = Instant::now() + Duration::from_millis(250);
                                let version = h.config_version.load(Ordering::Acquire);
                                if version != config_version || calibration_due.is_some_and(|due| Instant::now() >= due) {
                                    base_config = effective_config(&h, &s.launch)?;
                                    if s.config.vrr_low_latency {
                                        base_config.values.insert("wgc_slot_aligned_publish".into(), "false".into());
                                    }
                                    config_version = version;
                                    runtime_config = base_config.clone();
                                    if let Some(due) = &mut calibration_due {
                                        *due = Instant::now() + Duration::from_secs(30);
                                    }
                                }
                                *s.output.write().unwrap() = prepared.output();
                                if Instant::now() >= timing_due {
                                    timing_due = Instant::now() + Duration::from_secs(5);
                                    if timings_logged {
                                        // Sorted after the lock the sender records into is released.
                                        let window = s.stats.performance.lock().unwrap().window(Instant::now());
                                        let timing = window.snapshot();
                                        let link = s.link_rate_snapshot();
                                        let ms = |key: &str| timing[key].as_f64().unwrap_or(0.);
                                        // Pipeline frame rates since the last summary; none in the first.
                                        let rates = pipeline_log.sample(Instant::now(), s.pipeline_counts(), Duration::from_secs(5));
                                        let fps_of = |pick: fn(&butterpollo_core::pipeline_rates::PipelineRates) -> Option<u32>| {
                                            rates.as_ref().map(butterpollo_core::pipeline_rates::PipelineRates::from).as_ref().and_then(pick).map(|mhz| f64::from(mhz) / 1000.)
                                        };
                                        let split = |pick: fn(&(u64, u64)) -> u64| {
                                            let mut values: Vec<u64> = claim_ages.iter().map(pick).collect();
                                            values.sort_unstable();
                                            let mean = values.iter().sum::<u64>() as f64 / values.len().max(1) as f64 / 1000.;
                                            let p95 = values.get(values.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0) as f64 / 1000.;
                                            (mean, p95)
                                        };
                                        let (detect_mean_ms, detect_p95_ms) = split(|age| age.0);
                                        let (claim_wait_mean_ms, claim_wait_p95_ms) = split(|age| age.1);
                                        wgc_stamp_ages.sort_by(f64::total_cmp);
                                        let wgc_stamp_frames = wgc_stamp_ages.len();
                                        let wgc_stamp_future_frames = wgc_stamp_ages.iter().filter(|age| **age < 0.).count();
                                        let wgc_stamp_to_host_mean_ms = (wgc_stamp_frames > 0).then(|| wgc_stamp_ages.iter().sum::<f64>() / wgc_stamp_frames as f64);
                                        let wgc_stamp_to_host_p95_ms = wgc_stamp_ages.get(wgc_stamp_frames.saturating_sub(1) * 95 / 100).copied();
                                        // New pictures' stages over the last five seconds: mean/p95/p99 ms.
                                        let stage = |name: &str| {
                                            let ms = |key: &str| timing["stages"][name][key].as_f64().unwrap_or(0.);
                                            format!("{:.2}/{:.2}/{:.2}", ms("mean_ms"), ms("p95_ms"), ms("p99_ms"))
                                        };

                                        // How old the newest capture is when the stream takes it: p50/p95/p99 ms.
                                        let age = |name: &str| {
                                            let ms = |key: &str| timing["stages"][name][key].as_f64().unwrap_or(0.);
                                            format!("{:.2}/{:.2}/{:.2}", ms("p50_ms"), ms("p95_ms"), ms("p99_ms"))
                                        };

                                        tracing::info!(
                                            fps=ms("fps"),
                                            source_fps=fps_of(|r| r.source_mhz),
                                            capture_fps=fps_of(|r| r.capture_mhz),
                                            submit_fps=fps_of(|r| r.submit_mhz),
                                            output_fps=fps_of(|r| r.output_mhz),
                                            repeat_fps=fps_of(|r| r.repeat_mhz),
                                            host_mean_ms=ms("host_processing_mean_ms"),
                                            host_p95_ms=ms("host_processing_p95_ms"),
                                            host_p99_ms=ms("host_processing_p99_ms"),
                                            host_max_ms=ms("host_processing_max_ms"),
                                            encode_mean_ms=ms("encode_mean_ms"),
                                            encode_p95_ms=ms("encode_p95_ms"),
                                            encode_p99_ms=ms("encode_p99_ms"),
                                            frame_age_mean_ms=ms("frame_age_mean_ms"),
                                            frame_age_p95_ms=ms("frame_age_p95_ms"),
                                            present_to_send_mean_ms=ms("present_to_send_mean_ms"),
                                            present_to_send_p99_ms=ms("present_to_send_p99_ms"),
                                            present_to_sent_ms=%stage("present_to_sent"),
                                            stage_capture_ms=%stage("capture"),
                                            stage_hold_ms=%stage("hold"),
                                            stage_claim_ms=%stage("claim"),
                                            stage_convert_ms=%stage("convert"),
                                            stage_encode_ms=%stage("encode"),
                                            stage_queue_ms=%stage("queue"),
                                            stage_packetize_ms=%stage("packetize"),
                                            stage_send_queue_ms=%stage("send_queue"),
                                            stage_send_ms=%stage("send"),
                                            moonlight_host_ms=%stage("moonlight_host"),
                                            capture_age_at_claim_ms=%age("acquire_to_claim"),
                                            capture_age_at_submit_ms=%age("acquire_to_submit"),
                                            stage_frames=timing["stages"]["frames"].as_u64().unwrap_or(0),
                                            detect_mean_ms,
                                            detect_p95_ms,
                                            claim_wait_mean_ms,
                                            claim_wait_p95_ms,
                                            wgc_stamp_to_host_mean_ms,
                                            wgc_stamp_to_host_p95_ms,
                                            wgc_stamp_frames,
                                            wgc_stamp_future_frames,

                                            send_interval_p95_ms=timing["send_interval_p95_ms"].as_f64().unwrap_or(0.),
                                            send_interval_p99_ms=timing["send_interval_p99_ms"].as_f64().unwrap_or(0.),
                                            send_interval_max_ms=timing["send_interval_max_ms"].as_f64().unwrap_or(0.),
                                            // Keep reference feedback distinct from IDR recovery.
                                            idr_requests=s.stats.idr_requests.load(Ordering::Relaxed),
                                            reference_invalidations=s.stats.reference_invalidations.load(Ordering::Relaxed),
                                            send_loss_recoveries=s.stats.send_loss_recoveries.load(Ordering::Relaxed),
                                            // Inter frames past the full-FEC bound, sent as keyframes instead.
                                            reencoded_cuts=encoder.as_ref().map_or(0, |encoder| encoder.reencoded_cuts()),
                                            keyframes_over_cap=encoder.as_ref().map_or(0, |encoder| encoder.keyframes_over_cap()),
                                            keyframe_cap_scale=encoder.as_ref().map_or(1., |encoder| encoder.keyframe_cap_scale()),
                                            keyframes_past_fec=s.stats.keyframes_past_fec.load(Ordering::Relaxed),
                                            fec_reports=timing["fec_reports"].as_u64().unwrap_or(0),
                                            fec_invalid_reports=timing["fec_invalid_reports"].as_u64().unwrap_or(0),
                                            fec_duplicate_reports=timing["fec_duplicate_reports"].as_u64().unwrap_or(0),
                                            fec_recovered_frames=timing["fec_recovered_frames"].as_u64().unwrap_or(0),
                                            fec_unrecoverable_frames=timing["fec_unrecoverable_frames"].as_u64().unwrap_or(0),
                                            fec_missing_packets=timing["fec_missing_packets"].as_u64().unwrap_or(0),
                                            fec_loss_reports=timing["fec_loss_reports"].as_u64().unwrap_or(0),
                                            fec_loss_refused_reports=timing["fec_loss_refused_reports"].as_u64().unwrap_or(0),
                                            bitrate_kbps=s.bitrate.load(Ordering::Relaxed),
                                            sent_mbps=ms("bitrate_mbps"),
                                            link_state=%link["state"].as_str().unwrap_or(""),
                                            link_ceiling_kbps=link["ceiling_kbps"].as_u64().unwrap_or(0),
                                            link_current_kbps=link["current_kbps"].as_u64().unwrap_or(0),
                                            link_delivered_kbps=link["delivered_kbps"].as_u64().unwrap_or(0),
                                            link_steps_down=link["steps_down"].as_u64().unwrap_or(0),
                                            link_probes=link["probes"].as_u64().unwrap_or(0),
                                            link_failed_probes=link["failed_probes"].as_u64().unwrap_or(0),
                                            link_rtt_ms=link["rtt_ms"].as_u64().unwrap_or(0),
                                            link_rtt_floor_ms=link["rtt_floor_ms"].as_u64().unwrap_or(0),
                                            // Only with webrtc_loss_permille on: the video datagrams it has thrown away.
                                            webrtc_simulated_losses=webrtc_sender.as_ref().and_then(|sender| sender.simulated_losses()),
                                            "stream timings"
                                        );
                                    }
                                    if focus.on()
                                        && let Some(Ok(stats)) = encoder.as_ref().map(|encoder| encoder.focus_stats())
                                    {
                                        tracing::info!(
                                            mode = focus.settings(|| None).map(|settings| settings.mode.as_str()),
                                            maps = stats.maps,
                                            uniform = stats.uniform,
                                            stale = stats.stale,
                                            skipped_recovery = stats.skipped_recovery,
                                            qp_delta = stats.qp_delta,
                                            "encoder focus"
                                        );
                                    }
                                    claim_ages.clear();
                                    wgc_stamp_ages.clear();
                                    timings_logged = tracing::enabled!(tracing::Level::INFO);
                                }
                            }
                            // The client's display changed and it asked for another size or
                            // rate (0x5532). Its latest request is applied here, between
                            // frames: the encoder is rebuilt at the new size and starts it
                            // with a keyframe. The display keeps its mode, and the colour
                            // conversion scales its picture to the new size, unless the
                            // stream turned and the device's display follows it
                            // (dd_rotation_option = auto): the heartbeat reshapes or
                            // turns it then, off this loop.
                            let requested = {
                                let mut reconfigure = s.reconfigure.lock().unwrap();
                                let requested = reconfigure.take_due(Instant::now());
                                if let Some(to) = requested {
                                    s.rate_changes.lock().unwrap().taken(to);
                                }
                                requested
                            };
                            if let Some(to) = requested {
                                let from = butterpollo_core::reconfigure::Mode::of(&stream);
                                let enabled = runtime_config.boolean("stream_reconfigure", true);
                                match butterpollo_core::reconfigure::refusal(&stream, enabled) {
                                    Some(reason) => {
                                        refuse_switch(&s, from, butterpollo_core::rate_change::refusal_reason(enabled));
                                        tracing::info!(client = %s.launch.client.name, from = %from, to = %to, reason, "reconfigure refused");
                                    }
                                    None => {
                                        tracing::info!(client = %s.launch.client.name, from = %from, to = %to, "reconfigure requested");
                                        to.apply_to(&mut stream);
                                        if (to.width, to.height) != (from.width, from.height) {
                                            match s.launch.nucleus {
                                                // A co-op player's window follows the stream's size.
                                                Some(seat) => {
                                                    if let Err(error) = crate::nucleus::resize(&h, seat, (stream.width, stream.height)) {
                                                        tracing::warn!(client = %s.launch.client.name, error = %format!("{error:#}"), "co-op window resize refused");
                                                    }
                                                }
                                                None => prepared.follow_shape((stream.width, stream.height)),
                                            }
                                        }
                                        s.rebuild_link_rate(&stream, c.link_rate_adaptation(), max_bitrate_kbps(&s, &c));
                                        switching.get_or_insert((from, Instant::now()));
                                        rebuild_encoder = true;
                                    }
                                }
                            }
                            // Pacing follows the stream's rate: a new rate starts a new
                            // cadence, and a phase lock locks again at it.
                            if stream.fps_millihz() != paced_rate {
                                paced_rate = stream.fps_millihz();
                                rates = butterpollo_core::reconfigure::Rates::new(&stream, c.get("minimum_fps_target", "20"));
                                stream_period.set(rates.period);
                                progress.set_period(rates.period);
                                cadence = butterpollo_core::stream_policy::Cadence::new(Instant::now(), rates.period, c.boolean("wgc_pacing_smoothing", true));
                                pacer = new_pacer(&rates);
                                unlocked_grid_period = None;
                                {
                                    let mut grid = latest.grid.lock().unwrap();
                                    grid.period = rates.period;
                                    grid.anchor = Instant::now();
                                }
                                s.phase_sync.lock().unwrap().reset();
                            }
                            let period = stream_period.get();
                            latest.check()?;
                            // A WebRTC client has no video ping: its address is the pair ICE chose.
                            let peer = match &webrtc_sender {
                                Some(sender) => sender.peer(),
                                None => m
                                    .peers
                                    .lock()
                                    .unwrap()
                                    .get(&s.launch.id, false),
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
                            let now = Instant::now();
                            let due = cadence.deadline();
                            if !arrival_pacing && now < due {
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Grid, || -> Result<()> {
                                while Instant::now() < due {
                                    if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                                        let poll_tick = Instant::now();
                                        send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, poll_tick)?;
                                        if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                                            timer.until(
                                                (Instant::now() + OUTPUT_POLL)
                                                    .min(due),
                                            );
                                        }
                                    } else {
                                        timer.until(due);
                                    }
                                }
                                Ok(())
                                })?;
                            }
                            progress.mark(crate::stall_watch::Phase::Picture);
                            let capture_tick = Instant::now();
                            let image = AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Capture, || {
                                latest.wait_for_frame(&timer, &capture_wake, Instant::now() + period.min(Duration::from_millis(50)))
                            })?;
                            let Some(mut image) = image else {
                                // The capture is being re-created on a new device.
                                // Release everything on the old one so Windows can
                                // give the new capture a current adapter.
                                if encoder.take().is_some() {
                                    tracing::info!(client = %s.launch.client.name, "releasing the encoder while the capture recovers");
                                }
                                last_image = None;
                                staging = None;
                                pacer.reset_source_phase();
                                latest.release_generation(&capture_wake);
                                continue;
                            };
                            require_hdr_source(&s, || image.gpu.display_in_hdr(), true)?;
                            // Resolution changes or DXGI loss can recreate the capture device.
                            rebuild_encoder |= encoder
                                .as_ref()
                                .is_none_or(|encoder| !encoder.accepts_gpu_device(&image));
                            if rebuild_encoder {
                                pacer.reset_source_phase();
                            }
                            // A reopen can change the backend, e.g. to WGC after the
                            // driver's frame push failed.
                            pacer.set_source_phase(source_phase_eligible && butterpollo_core::stream_policy::source_phase(&c, latest.backend()));
                            if arrival_pacing {
                                pacer.observe_source(image.captured);
                                pacer.observe_source_rate(latest.source_rate());
                                let (start, late, extends) = latest.picture();
                                pacer.observe_picture(start, late);
                                pacer.observe_extension(extends);
                            }
                            let fresh = last_image.as_ref().is_none_or(|previous| !Arc::ptr_eq(previous, &image));
                            if arrival_pacing
                                && fresh
                                && !rebuild_encoder
                                && !s.idr.load(Ordering::Acquire)
                                && s.invalidation.lock().unwrap().is_none()
                                && let interval = latest.picture_interval()
                                && let butterpollo_core::stream_policy::Pace::WaitUntil(deadline) =
                                    pacer.decide(Instant::now(), image.captured, interval)
                            {
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::FreshRate, || -> Result<()> {
                                let key = Arc::as_ptr(&image) as usize;
                                if first_seen.is_none_or(|(seen, ..)| seen != key) {
                                    first_seen = Some((key, Instant::now(), interval, Some(deadline)));
                                }
                                // A poll can block for the encoder's 1 ms query
                                // timeout; close to the claim it would overshoot.
                                if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                                    && deadline.saturating_duration_since(Instant::now()) >= Duration::from_millis(1)
                                {
                                    send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, capture_tick)?;
                                }
                                let now = Instant::now();
                                let until = if encoder.as_ref().is_some_and(|e| recovery.pending(e))
                                    && deadline.saturating_duration_since(now) >= Duration::from_millis(1)
                                {
                                    deadline.min(now + OUTPUT_POLL)
                                } else {
                                    deadline
                                };
                                // The claim itself is met precisely; a poll for the
                                // encoder's output is not worth spinning for.
                                if until == deadline {
                                    latest.wait_if_current_precise(&timer, &capture_wake, &image, until)?;
                                } else {
                                    latest.wait_if_current(&timer, &capture_wake, &image, until)?;
                                }
                                Ok(())
                                })?;
                                continue;
                            }
                            let repeat_due = if arrival_pacing {
                                pacer.repeat_deadline(encoded_at + rates.static_period, image.captured, latest.source_interval())
                            } else {
                                encoded_at + rates.static_period
                            };
                            // A recovery request on a moving picture rides the
                            // next new frame; only a still one is encoded again
                            // at once. Grid pacing serves it at the next slot.
                            // A changed menu panel is encoded at once, like a recovery.
                            menu.poll(&s.couch_menu, [stream.width, stream.height]);
                            let recovering = menu.dirty || s.idr.load(Ordering::Acquire) || s.invalidation.lock().unwrap().is_some();
                            let reencode_at = if !recovering {
                                repeat_due
                            } else if arrival_pacing {
                                butterpollo_core::stream_policy::reencode_at(true, image.captured, period, latest.source_interval(), repeat_due)
                            } else {
                                Instant::now()
                            };
                            if !rebuild_encoder
                                && (s.config.vrr_low_latency || rates.limit_static_rate || arrival_pacing)
                                && last_image
                                    .as_ref()
                                    .is_some_and(|previous| Arc::ptr_eq(previous, &image))
                                && Instant::now() < reencode_at
                            {
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Repeat, || -> Result<()> {
                                if encoder.as_ref().is_some_and(|e| recovery.pending(e)) { send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, capture_tick)?; }
                                let wait = if encoder.as_ref().is_some_and(|e| recovery.pending(e)) { OUTPUT_POLL } else { period };
                                latest.wait_if_current(&timer, &capture_wake, &image, Instant::now() + wait.min(reencode_at.saturating_duration_since(Instant::now())))?;
                                Ok(())
                                })?;
                                continue;
                            }
                            // A picture claimed while the encoder is behind only
                            // waits in its queue: take its output first, then
                            // claim the newest picture.
                            // A frame still being sent on a paced link counts too.
                            // A frame waiting behind it is skipped on a keyframe
                            // request: the client would discard it, and the
                            // keyframe would wait for it.
                            let recovery_lead = encoder.as_ref().and_then(Encoder::recovery_lead).map(|lead| lead.min(period));
                            let sending = match video_sender.get() {
                                Some(sender) => {
                                    if s.idr.load(Ordering::Acquire) { sender.skip_pending()?; }
                                    // Claim the newest picture near the end of an IDR,
                                    // only when this encoder demonstrated a safe lead.
                                    sender.claim_backlog(recovery_lead)?
                                },
                                None => 0,
                            };
                            let encoding = encoder.as_ref().map_or(0, |e| recovery.backlog(e));
                            let encoding_limit = encoder.as_ref().map_or(encoder_backlog::DEFAULT_LIMIT, |encoder| {
                                if video_sender.get().is_some() { encoder.recovery_backlog_limit() } else { EncoderOutput::backlog_limit(encoder) }
                            });
                            let pressure = encoder_backlog::backpressure(encoding, sending, encoding_limit);
                            if !rebuild_encoder && pressure != Backpressure::None {
                                if pressure == Backpressure::Sender {
                                    AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Sender, || -> Result<()> {
                                    // Network occupancy must not start encoder stall recovery.
                                    recovery.backlog_since = None;
                                    if encoder.as_ref().is_some_and(|e| recovery.pending(e)) {
                                        send_frames(recovery.collect(&mut encoder, &s.launch.warnings, Instant::now)?, peer, Duration::ZERO, latest, capture_tick)?;
                                    } else {
                                        video_sender.get().unwrap().wait_for_capacity(encoder_backlog::total_limit(encoding_limit), period, recovery_lead)?;
                                    }
                                    Ok(())
                                    })?;
                                    continue;
                                }
                                // An encoder that returns nothing for 250 ms is
                                // recreated, as a queue that never drained was;
                                // a shorter stall on a saturated GPU only makes the
                                // stream choppy. A recreated one that is still
                                // silent gets longer each time.
                                AdmissionTrace::measure(&mut admission_trace, AdmissionWait::Encoder, || -> Result<()> {
                                    if let Some(output) = recovery.poll_full(&mut encoder, &s.launch.warnings, Instant::now)? {
                                        send_frames(output, peer, Duration::ZERO, latest, capture_tick)?;
                                    }
                                    Ok(())
                                })?;
                                continue;
                            }
                            recovery.backlog_since = None;
                            if s.launch.role == Role::Stream && Instant::now() >= quit_scan_due {
                                quit_scan_due = Instant::now() + Duration::from_secs(1);
                                // The scan runs on its own thread; this reads its last result.
                                if let Some((program, pid)) = quit_scan
                                    .get_or_insert_with(butterpollo_windows::foreground::Tracker::default)
                                    .poll_process(&[], &image.gpu.display)
                                {
                                    progress.mark(crate::stall_watch::Phase::AppTracking);
                                    if !observe_foreground_game(&h, &s.launch.client.uuid, &program, pid) {
                                        quit_scan_due = Instant::now() + QUIT_SCAN_RETRY;
                                    }
                                    progress.mark(crate::stall_watch::Phase::Picture);
                                }
                            }
                            let rebuilt = rebuild_encoder;
                            if rebuild_encoder {
                                encoder = None;
                                // Some drivers accept the compute path at creation and
                                // fail on it later, every time: after a second failure
                                // in the session, convert on the graphics queue. One
                                // failure (a game holding the GPU, a driver reset) must
                                // not cost the rest of the session the slower path,
                                // 6-9 ms a frame beside a GPU-bound game.
                                if let Some(since) = recovery.failing
                                    && counted_failure != Some(since)
                                {
                                    counted_failure = Some(since);
                                    failures += 1;
                                }
                                let mut tuning = c.clone();
                                if failures >= 2 {
                                    tuning.values.insert("gpu_compute_conversion".into(), "false".into());
                                    if butterpollo_windows::compute::enabled(&c) {
                                        s.launch.warnings.set("encoder_compute_recovery", "Compute conversion disabled after repeated encoder failures; using the graphics queue for the rest of this session. A busy game can delay frames; lower game GPU load or update the AMD driver, then reconnect to retry compute.");
                                    }
                                }
                                progress.mark(crate::stall_watch::Phase::EncoderCreate);
                                match Encoder::new_gpu_reported(
                                    &stream,
                                    &image,
                                    &tuning,
                                    &s.launch.warnings,
                                ) {
                                    Ok(mut created) => {
                                        *s.encoder.write().unwrap() = created.backend().into();
                                        created.set_hdr_target(s.hdr_target_nits());
                                        encoder = Some(created);
                                        focus.encoder_recreated();
                                    },
                                    Err(error) => {
                                        if let Some(loss) = DeviceLost::d3d11(&image.gpu.device).or_else(|| DeviceLost::from_error(&error)) {
                                            recovery.device_loss = Some(loss);
                                            recovery.failing.get_or_insert_with(Instant::now);
                                            continue;
                                        }
                                        // The encoder cannot make the size or rate the client
                                        // asked for: the stream goes back to the mode it had.
                                        if let Some((from, _)) = switching.take() {
                                            let to = butterpollo_core::reconfigure::Mode::of(&stream);
                                            from.apply_to(&mut stream);
                                            refuse_switch(&s, from, butterpollo_core::rate_change::Reason::EncoderFailed);
                                            tracing::info!(client = %s.launch.client.name, from = %from, to = %to, error = %format!("{error:#}"), "reconfigure refused: the encoder cannot make this mode; keeping the previous one");
                                            continue;
                                        }
                                        let since = *recovery.failing.get_or_insert_with(Instant::now);
                                        if since.elapsed() >= ENCODER_RECOVERY {
                                            return Err(error.context("the encoder could not be recreated"));
                                        }
                                        s.launch.warnings.set("encoder_recovery", format!("Encoder recreation failed ({error:#}); retrying the same backend while the picture freezes. Check the driver and lower game GPU load; the session will fail if frames do not resume."));
                                        timer.until(Instant::now() + Duration::from_millis(100));
                                        continue;
                                    }
                                }
                                // Encoder creation can outlast several source frames.
                                let Some(current) = latest.current()? else { continue };
                                if !encoder.as_ref().unwrap().accepts_gpu_device(&current) {
                                    continue;
                                }
                                image = current;
                                if arrival_pacing {
                                    pacer.observe_source(image.captured);
                                }
                                metadata_due = Instant::now();
                                staging = None;
                                recovery.recreated(&s);
                                rebuild_encoder = false;
                            }
                            let active = encoder
                                .as_mut()
                                .expect("a missing encoder is rebuilt above");
                            if menu.dirty || (rebuilt && menu.has_overlay()) {
                                active.set_overlay(menu.for_encoder([stream.width, stream.height]));
                            }
                            // Frames of a stream that left slice delivery return whole.
                            delivery.sync(&s.stats);
                            if delivery.joins() {
                                active.join_slices();
                            }
                            if rebuilt {
                                active.set_next_frame(match video_sender.get() {
                                    Some(sender) => sender.next_wire_frame()?,
                                    None => next_wire_frame.get(),
                                });
                            }
                            let begin = Instant::now();
                            let fresh = last_image.as_ref().is_none_or(|previous| !Arc::ptr_eq(previous, &image));
                            if let Some(admission) = admission_trace.as_mut() {
                                trace_attempt = trace_attempt.wrapping_add(1);
                                let us = |at: Instant| at.saturating_duration_since(start).as_micros() as u64;
                                let key = Arc::as_ptr(&image) as usize;
                                let (seen, interval, deadline) = match first_seen {
                                    Some((k, seen, interval, deadline)) if k == key => (seen, interval, deadline),
                                    _ => (begin, latest.picture_interval(), None),
                                };
                                let (capture_id, selected_is_latest, latest_capture_id) = latest.trace_selection(&image);
                                tracing::trace!(
                                    target: "pacing",
                                    capture_id,
                                    attempt = trace_attempt,
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
                                    choice = if fresh { "fresh" } else if recovering { "recovery_repeat" } else { "static_repeat" },
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
                                    "claim"
                                );
                                *admission = AdmissionTrace::default();
                            }
                            if timings_logged && fresh && claim_ages.len() < 4096 {
                                let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
                                claim_ages.push((
                                    micros(image.acquired.saturating_duration_since(image.captured)),
                                    micros(begin.saturating_duration_since(image.acquired)),
                                ));
                                if let Some(stamp) = image.wgc_stamp {
                                    let age = if image.acquired >= stamp {
                                        image.acquired.duration_since(stamp).as_secs_f64()
                                    } else {
                                        -stamp.duration_since(image.acquired).as_secs_f64()
                                    };
                                    wgc_stamp_ages.push(age * 1000.);
                                }
                            }
                            if Instant::now() >= metadata_due {
                                let metadata = m.set_display_hdr_metadata(&s, image.gpu.hdr_metadata());
                                active.set_hdr_metadata(metadata);
                                // A receiver's report (0x5531) arrives after the stream starts.
                                active.set_hdr_target(s.hdr_target_nits());
                                metadata_due = Instant::now() + Duration::from_secs(1);
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
                                confirm_keyframe = None;
                            }
                            let recovering = idr || invalidation.is_some();
                            // A client-chosen FEC share moves the encoder's part of the
                            // budget at a fence (an IDR or a new encoder) or after a
                            // statistical FEC report; a new rate needs no keyframe.
                            let bitrate = s.encoder_bitrate(idr || rebuilt);
                            if s.config.codec != 3 {
                                let now = Instant::now();
                                if encode_route.is_none_or(|(address, _)| address != peer.ip()) || now >= encode_route_due {
                                    encode_route = Some((peer.ip(), butterpollo_windows::net::routed_link(peer)));
                                    encode_route_due = now + Duration::from_secs(2);
                                }
                                // Budget for the parity a recovery picture uses,
                                // even while ordinary frames use adaptive FEC.
                                recovery_packetizer.fec_percent = s.recovery_fec_percent();
                                let pace = crate::video_send::recovery::preview_pace(&s, &c, &recovery_packetizer, period, peer, encode_route.unwrap().1);
                                let cap = butterpollo_core::network_pacing::recovery_frame_cap(
                                    pace.bps, bitrate, period, recovery_packetizer.fec_percent,
                                    recovery_packetizer.packet_size, peer.is_ipv6(),
                                );
                                // Moonlight takes four FEC blocks a frame: a recovery keyframe
                                // past them leaves with less parity or none, and one lost
                                // packet costs it. Without FEC the cap is the four-block packet
                                // limit, so a cut exposes as few packets as it can. Statistical
                                // FEC picks parity by frame size; its search runs once a second.
                                let statistical = s.statistical_fec();
                                let key = (recovery_packetizer.fec_percent, recovery_packetizer.packet_size, statistical);
                                if fec_whole.0 != key || (statistical && now >= fec_whole_due) {
                                    let whole = s.statistical_fec_whole_payload(now).unwrap_or_else(|| recovery_packetizer.fec_whole_payload_limit());
                                    fec_whole = (key, whole);
                                    fec_whole_due = now + Duration::from_secs(1);
                                }
                                let cap = butterpollo_core::network_pacing::fec_whole_frame_cap(cap, fec_whole.1, bitrate, period);
                                let _ = active.set_frame_cap(cap, butterpollo_core::network_pacing::two_frames(bitrate, period));
                                // AMF does not hold a scene cut's inter frame to the cap: one
                                // past this bound is encoded again as a keyframe.
                                active.set_cut_limit(
                                    butterpollo_core::network_pacing::fec_whole_frame_limit(fec_whole.1, bitrate, period)
                                        .unwrap_or(0),
                                );
                            }
                            // The device draws the pointer: only Desktop Duplication supplies
                            // it apart, so with any other capture it stays in the video and the
                            // device is told to hide its own.
                            let leave_pointer_out = s.client_cursor.active() && {
                                let ddx = latest.backend() == "ddx";
                                let pointer = image.pointer().filter(|_| ddx).map(|p| butterpollo_core::client_cursor::Pointer {
                                    width: p.width,
                                    height: p.height,
                                    hotspot: p.hotspot,
                                    logic: p.logic,
                                    pixels: p.pixels,
                                });
                                if s.client_cursor.note(pointer, &image.gpu.display.display_name) {
                                    let _ = m.control_wake.send(&[1]);
                                }
                                if !ddx && !std::mem::replace(&mut pointer_warned, true) {
                                    tracing::warn!(capture = latest.backend(), "the device-drawn cursor needs Desktop Duplication capture; the pointer stays in the video");
                                }
                                ddx
                            };
                            let pointer_toggled = leave_pointer_out != pointer_left_out;
                            pointer_left_out = leave_pointer_out;
                            // A repeat with a new panel, or with the pointer put in or left out,
                            // is a new picture.
                            active.set_repeat(!fresh && !menu.dirty && !pointer_toggled);
                            menu.dirty = false;
                            // A map that failed on a frame turned focus off in
                            // the encoder; keep it off for the stream, loudly.
                            if let Some(error) = active.take_focus_failure() {
                                focus.failed(&error, &s.encoder_focus_state, &s.launch.warnings);
                            }
                            focus.update(&s.encoder_focus, &s.encoder_focus_state, &s.launch.warnings);
                            let wanted = focus.settings(|| butterpollo_windows::encoder::focus_cursor(&image, (stream.width, stream.height)));
                            if let Err(error) = focus.apply(wanted, |settings| {
                                active.set_focus_verify(focus_verify);
                                active.set_focus(settings)
                            }) {
                                focus.failed(&error, &s.encoder_focus_state, &s.launch.warnings);
                                active.set_focus(None)?;
                            }
                            let mut presented_image = image.as_ref().clone();
                            if !fresh { presented_image.captured = Instant::now(); }
                            if leave_pointer_out { presented_image.leave_pointer_out(); }
                            progress.mark(crate::stall_watch::Phase::Encode);
                            let encode_trace_started = trace_send.then(Instant::now);
                            let encoded = (|| -> Result<Vec<butterpollo_windows::encoder::Encoded>> {
                                #[cfg(debug_assertions)]
                                crate::soak_fault::check("encoder failure")?;
                                #[cfg(debug_assertions)]
                                crate::soak_fault::check("DXGI_ERROR_DEVICE_REMOVED")?;
                                Ok(if c.boolean("wgc_direct_encoder_input", true) {                                    active.encode_gpu(&presented_image, idr, bitrate)?
                                } else {
                                    active.encode(
                                        &presented_image.readback(&mut staging)?,
                                        idr,
                                        bitrate,
                                    )?
                                })
                            })();
                            if let Some(at) = encode_trace_started {
                                tracing::trace!(
                                    target: "pacing",
                                    stream_id = %s.launch.id,
                                    attempt = trace_attempt,
                                    host_acquired = ?image.acquired,
                                    submitted_presentation = ?presented_image.captured,
                                    encode_call_us = at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
                                    submitted = active.last_submitted().filter(|submitted| *submitted >= at).map_or(0, |submitted| submitted.saturating_duration_since(start).as_micros() as u64),
                                    outputs = encoded.as_ref().map_or(0, Vec::len),
                                    ok = encoded.is_ok(),
                                    idr,
                                    fresh,
                                    "encode returned"
                                );
                            }
                            let output = match encoded {
                                Ok(output) => {
                                    recovery.output(output, Instant::now)?
                                }
                                Err(error) => {
                                    if let Some(loss) = active.device_removed()
                                        .or_else(|| DeviceLost::d3d11(&image.gpu.device))
                                        .or_else(|| DeviceLost::from_error(&error))
                                    {
                                        recovery.device_loss = Some(loss);
                                        recovery.failing.get_or_insert_with(Instant::now);
                                        continue;
                                    }
                                    // The first frame at a size the client asked for failed:
                                    // go back to the mode the stream had, as when the encoder
                                    // cannot be created at it.
                                    if let Some((from, _)) = switching.take() {
                                        let to = butterpollo_core::reconfigure::Mode::of(&stream);
                                        from.apply_to(&mut stream);
                                        s.rebuild_link_rate(&stream, c.link_rate_adaptation(), max_bitrate_kbps(&s, &c));
                                        refuse_switch(&s, from, butterpollo_core::rate_change::Reason::EncoderFailed);
                                        tracing::info!(client = %s.launch.client.name, from = %from, to = %to, error = %format!("{error:#}"), "reconfigure refused: the encoder failed at this mode; keeping the previous one");
                                        rebuild_encoder = true;
                                        continue;
                                    }
                                    // A stalled or reset GPU costs these frames and a
                                    // keyframe; the rebuild requests it. Only failures
                                    // that keep coming end the session.
                                    let since = *recovery.failing.get_or_insert_with(Instant::now);
                                    if since.elapsed() >= ENCODER_RECOVERY {
                                        return Err(error.context("the encoder kept failing"));
                                    }
                                    s.launch.warnings.set("encoder_recovery", format!("Encoding failed ({error:#}); recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats."));
                                    active.log_stall();
                                    rebuild_encoder = true;
                                    continue;
                                }
                            };
                            if rebuilt && let Some((from, started)) = switching.take() {
                                s.rate_changes.lock().unwrap().switched(butterpollo_core::reconfigure::Mode::of(&stream), None);
                                tracing::info!(
                                    client = %s.launch.client.name,
                                    from = %from,
                                    to = %butterpollo_core::reconfigure::Mode::of(&stream),
                                    switch_ms = started.elapsed().as_millis() as u64,
                                    source_width = image.width,
                                    source_height = image.height,
                                    "reconfigure"
                                );
                            }
                            let call_latency = begin.elapsed();
                            let (slot, pictures, captures) = latest.frame_counts();
                            let (pictures, captures) = pipeline_follow.advance(slot, pictures, captures);
                            s.stats.pipeline.captured(pictures, captures);
                            s.stats.pipeline.submitted(!fresh);
                            if fresh {
                                claims.borrow_mut().push(butterpollo_core::performance::Claim {
                                    key: presented_image.captured,
                                    presented: image.captured,
                                    acquired: image.acquired,
                                    published: image.published,
                                    claimed: begin,
                                    submitted: active.last_submitted().filter(|at| *at >= begin).unwrap_or(begin),
                                });
                            }
                            // Repeat deadlines start at submission, so encoder work
                            // does not extend the interval between static frames.
                            encoded_at = begin;
                            if arrival_pacing {
                                // New pictures and static repeats count toward the
                                // stream rate: a repeat that spent nothing let the
                                // game's next frame follow it at once, so VRR
                                // streams went past the rate whenever a frame came
                                // late. An unchanged picture encoded again for the
                                // client's recovery request does not, or a client
                                // losing packets left every game frame late.
                                if butterpollo_core::stream_policy::counts_toward_rate(fresh, recovering) {
                                    // Phase lock: holding frames here would only move the
                                    // wait from the client to the host, so the lock sets
                                    // the rate cap to the client's refresh instead. VRR
                                    // displays follow each frame and need no lock.
                                    match (!vrr).then(|| phase_interval(&s, begin, period)).flatten() {
                                        Some(interval) => {
                                            let mut grid = latest.grid.lock().unwrap();
                                            unlocked_grid_period.get_or_insert(grid.period);
                                            grid.period = interval;
                                            pacer.set_period(interval);
                                        }
                                        None => {
                                            if let Some(original) = unlocked_grid_period.take() {
                                                latest.grid.lock().unwrap().period = original;
                                                pacer.set_period(period);
                                            }
                                        }
                                    }
                                    pacer.claimed(begin);
                                }
                                latest.grid.lock().unwrap().anchor = pacer.allowed_at(begin);
                            } else {
                                // Phase lock: the grid follows the client's refresh, at a
                                // phase that gets frames there just before its latch.
                                // Without reports this is the original fixed-period path.
                                match phase_interval(&s, begin, period) {
                                    Some(interval) => {
                                        cadence.submitted_after(begin, interval);
                                        let mut grid = latest.grid.lock().unwrap();
                                        unlocked_grid_period.get_or_insert(grid.period);
                                        grid.anchor = cadence.deadline();
                                        grid.period = interval;
                                    }
                                    None => {
                                        cadence.submitted(begin);
                                        let mut grid = latest.grid.lock().unwrap();
                                        if let Some(original) = unlocked_grid_period.take() {
                                            grid.period = original;
                                        }
                                        grid.anchor = cadence.deadline();
                                    }
                                }
                            }
                            last_image = Some(image);
                            // An allocation can later reuse this image's address;
                            // its trace observation belongs only to this submission.
                            first_seen = None;
                            send_frames(output, peer, call_latency, latest, capture_tick)?;
                        }
                        Ok(())
                    })()
                })();
                if let Err(e) = result {
                    s.launch.warnings.set("stream_failure", format!("Stream ended: {e:#}"));
                    s.fail();
                    tracing::error!(error=%format!("{e:#}"),client=%s.launch.client.name,"session failed; check the reported encoder, capture or socket error before reconnecting");
                }
                s.stop();
                // A failed stream must not count as a healthy fast-rate probe.
                drop(keyframe_loss_memory);
                h.sessions.lock().unwrap().begin_teardown(&s);
                h.session_webhook("session.ended", &s);
                for (audio, peer) in m.peers.lock().unwrap().remove(&s.launch.id) {
                    if audio { &m.audio } else { &m.video }.forget(peer);
                }
                // Keep capture and display leases owned until the encoder has
                // terminated, even if a driver blocks its drop indefinitely.
                if let Some(audio) = audio {
                    finish_worker(audio, "audio");
                }
                drop(pyrowave_sender);
                // Frees the WebRTC media port; a link the worker never took goes with it.
                drop(webrtc_sender);
                h.webrtc_links.discard(&s.launch.id);
                drop(timing_record);
                drop(client_commands);
                drop(encoder);
                drop(latest);
                drop(stream_preparation);
                #[cfg(test)]
                drop(fixture_preparation);
                drop(com);
                if s.launch.role == Role::RemoteMonitor
                    && h.config
                        .read()
                        .unwrap()
                        .boolean("remote_monitor_disconnect_on_stream_end", false)
                {
                    crate::remote_display::disconnect(&h, Some(&s.launch.client.uuid));
                }
                h.sessions.lock().unwrap().teardown.remove(&s.launch.id);
                tracing::info!(client=%s.launch.client.name,"CLIENT DISCONNECTED");
            })
    }
}
