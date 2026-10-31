//! The stream thread: display preparation, capture admission and pacing, encoding,
//! encoder recovery and the inline video send.
mod run;
mod setup;
mod teardown;
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

/// What every phase of a session thread reads: the media sockets, the host and the session.
struct SessionThread<'a> {
    m: &'a Arc<Media>,
    h: &'a Shared,
    s: &'a Arc<Session>,
}

/// The capture, display and encoder leases and COM, held by the session thread until its
/// teardown releases them in order. Fields are declared in the reverse of the order the
/// thread created them: a struct drops its fields in declaration order, so an unwind drops
/// them as it dropped the thread's locals, the encoder before the capture and display.
#[derive(Default)]
struct Leases {
    encoder: Option<Encoder>,
    latest: Option<Arc<Source>>,
    stream_preparation: Option<crate::display_session::StreamPreparation>,
    com: Option<ComGuard>,
}

/// The session thread's workers and records, released at teardown before its [`Leases`].
/// Fields in the reverse of the order the thread created them, as in [`Leases`].
struct Workers {
    // A co-op player's stream ends with its Nucleus Co-op session.
    coop_watch: Option<crate::nucleus::Watch>,
    #[cfg(test)]
    fixture_preparation: Option<
        Box<dyn butterpollo_core::session::Preparation<crate::display_session::StreamPreparation>>,
    >,
    // Ends the host-side timing record's writer, which writes its footer, on every
    // way out of the stream.
    timing_record: crate::timing_record::Detach,
    keyframe_loss_memory: Option<crate::state::loss_memory::Guard>,
    webrtc_sender: Option<crate::webrtc::session::Sender>,
    pyrowave_sender: Option<crate::pyrowave_send::Sender>,
    audio: Option<thread::JoinHandle<()>>,
    client_commands: Option<crate::process::ClientCommands>,
}

impl Workers {
    fn new(s: &Arc<Session>) -> Self {
        Workers {
            coop_watch: None,
            #[cfg(test)]
            fixture_preparation: None,
            timing_record: crate::timing_record::Detach(s.clone()),
            keyframe_loss_memory: None,
            webrtc_sender: None,
            pyrowave_sender: None,
            audio: None,
            client_commands: None,
        }
    }
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
                let session = SessionThread {
                    m: &m,
                    h: &h,
                    s: &s,
                };
                tracing::info!(client=%s.launch.client.name,"CLIENT CONNECTED");
                session.record_last_seen();
                let mut leases = Leases::default();
                let mut workers = Workers::new(&s);
                let result = (|| -> Result<()> {
                    workers.coop_watch = crate::nucleus::Watch::start(&h, &s)?;
                    #[cfg(test)]
                    if let Some(fixture) = &h.reconnect_fixture {
                        workers.fixture_preparation = s.launch.preparation.lock().unwrap().take();
                        return fixture.stream(&m, &h, &s);
                    }
                    leases.com = Some(ComGuard::new()?);
                    let _priority = Priority::new();
                    let _streaming = butterpollo_windows::timing::StreamingScope::enter();
                    let Some(config) = session.configure(&mut workers)? else {
                        return Ok(());
                    };
                    let display =
                        session.prepare_display(&config.c, &mut leases.stream_preparation)?;
                    let Some(capture) = session.start_capture(
                        &config.c,
                        &config.timer,
                        &display.prepared,
                        &mut leases.latest,
                        &mut leases.encoder,
                    )?
                    else {
                        return Ok(());
                    };
                    let media = session.start_media(&config.c, &display.prepared, &mut workers)?;
                    session.run(
                        &mut leases.encoder,
                        &workers,
                        config,
                        display,
                        capture,
                        media,
                    )
                })();
                session.teardown(result, workers, leases);
            })
    }
}
