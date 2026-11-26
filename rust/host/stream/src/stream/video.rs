//! The stream thread: display preparation, capture admission and pacing, encoding,
//! encoder recovery and the inline video send.
mod run;
mod setup;
mod teardown;
use super::*;
use butterpollo_session::session::EncoderFailure;

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

/// How often completed encoder output is collected while a frame is in flight
/// for polling-only encoders (those without an `output_event()`).
pub(super) const OUTPUT_POLL: Duration = Duration::from_micros(100);
/// Stall-watchdog bound for an encoder with an `output_event()` when waiting
/// for queue capacity: the event wakes the wait immediately on completion, and
/// this bound still lets `poll_full` detect a stalled encoder well within its
/// 250 ms limit without waking every 100 µs.
pub(super) const OUTPUT_EVENT_WATCHDOG: Duration = Duration::from_millis(5);

/// The next output poll while frames are in flight: one output poll from
/// now, or later when the encoder knows its oldest frame cannot be done
/// sooner (`amd_output_prediction`).
pub(super) fn next_output_poll(encoder: Option<&impl EncoderOutput>, now: Instant) -> Instant {
    let poll = now + OUTPUT_POLL;
    encoder
        .and_then(|encoder| encoder.output_due())
        .map_or(poll, |due| due.max(poll))
}

/// The wait deadline and wake event for a pending encoder before `deadline`:
/// an encoder with an `output_event()` waits directly until `deadline` (waking
/// as soon as the event signals); a polling-only encoder bounds the wait to
/// [`next_output_poll`]. Calls `output_event()` at most once (NVENC and
/// PyroWave arm the event there).
pub(super) fn output_wait<E: EncoderOutput>(
    encoder: Option<&E>,
    now: Instant,
    deadline: Instant,
) -> (Instant, Option<&butterpollo_win_graphics::timing::Signal>) {
    if let Some(event) = encoder.and_then(EncoderOutput::output_event) {
        (deadline, Some(event))
    } else {
        (next_output_poll(encoder, now).min(deadline), None)
    }
}

/// Return a finished streamed frame's unit buffers to the frame pool and keep
/// the emptied outer vector for the next frame (`VideoPacketizer::streamed_in`).
pub(super) fn recycle_streamed_pieces(
    streamed: butterpollo_media_policy::packet::StreamedFrame,
) -> Vec<Vec<u8>> {
    let mut pieces = streamed.take_pieces();
    for buf in pieces.drain(..) {
        butterpollo_win_graphics::frame_pool::give(buf);
    }
    pieces
}

/// Whether an output poll before `deadline` cannot overshoot it: only a poll
/// that can block (AMF's QueryOutput, for its 1 ms timeout) must leave room.
pub(super) fn poll_fits(
    encoder: Option<&impl EncoderOutput>,
    now: Instant,
    deadline: Instant,
) -> bool {
    encoder.is_none_or(|encoder| !encoder.poll_blocks())
        || encoder_backlog::query_fits(now, deadline)
}

/// After a poll that returned nothing while the encoder is full: an encoder
/// that predicts its output sleeps until then, one with an output event
/// (AMF's output thread, NVENC's D3D12 fence, PyroWave) until it with a 5 ms
/// stall-watchdog bound. One whose poll waits in the driver (AMF's QueryOutput)
/// polls again at once; one whose poll returns at once (NVENC without the fence)
/// sleeps one output poll, as the waits with room in the encoder do, where
/// polling again at once spun a core at the stream thread's priority for as
/// long as the encoder stayed full. Nothing waits once no frame is pending,
/// such as after a device loss.
pub(super) fn wait_for_output(
    timer: &butterpollo_win_graphics::timing::Timer,
    recovery: &EncoderRecovery,
    encoder: Option<&impl EncoderOutput>,
) -> Result<()> {
    let Some(encoder) = encoder.filter(|encoder| recovery.pending(*encoder)) else {
        return Ok(());
    };
    if let Some(due) = encoder.output_due().filter(|due| *due > Instant::now()) {
        timer.until(due);
    } else if let Some(event) = encoder.output_event() {
        timer.until_or_event(Instant::now() + OUTPUT_EVENT_WATCHDOG, Some(event))?;
    } else if !encoder.poll_blocks() {
        timer.until(Instant::now() + OUTPUT_POLL);
    }
    Ok(())
}

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

/// Two encoder failures this close together, while compute was converting,
/// move the session's conversion to the graphics queue.
pub(super) const COMPUTE_FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// A session converting on the graphics queue after failures tries compute
/// again once this long has passed without an encoder failure.
pub(super) const COMPUTE_RETRY_QUIET: Duration = Duration::from_secs(5 * 60);

/// Whether encoder failures have moved a session's colour conversion from the
/// compute queue to the graphics queue. Some drivers accept the compute path
/// at creation and fail on it later, every time; a single failure (a game
/// holding the GPU, a driver reset) must not cost the session the slower
/// path, 6-9 ms a frame beside a GPU-bound game. Only failures while compute
/// was converting count, two within [`COMPUTE_FAILURE_WINDOW`] move it, and
/// [`COMPUTE_RETRY_QUIET`] without a failure moves it back. NVENC's D3D12 path
/// is the unverified one: a creation fallback keeps the graphics queue for the
/// session, and a running D3D12 failure demotes stickily on its second failure
/// in the window rather than stalling twice every retry window.
#[derive(Debug, Default)]
pub(super) struct ComputeDemotion {
    /// The last failure that counted against compute and has no pair yet.
    last_failure: Option<Instant>,
    /// While on the graphics queue: the move or the last failure since.
    quiet_since: Option<Instant>,
    /// No retry for the rest of this session.
    sticky: bool,
}

impl ComputeDemotion {
    pub(super) fn demoted(&self) -> bool {
        self.quiet_since.is_some()
    }
    /// Whether the demotion lasts for the rest of the session.
    pub(super) fn sticky(&self) -> bool {
        self.sticky
    }
    /// Keep the graphics queue for the rest of the session without another
    /// failure: used when NVENC's D3D12 attempt already fell back at creation.
    pub(super) fn demote_sticky(&mut self, at: Instant) {
        self.last_failure = None;
        self.quiet_since = Some(at);
        self.sticky = true;
    }
    /// An encoder failure at `at`, `on_compute` when the failing encoder
    /// converted on a compute queue. `sticky` keeps the graphics queue for the
    /// session. True when it moves the session to the graphics queue.
    pub(super) fn failed(&mut self, at: Instant, on_compute: bool, sticky: bool) -> bool {
        if let Some(quiet) = self.quiet_since.as_mut() {
            *quiet = (*quiet).max(at);
            return false;
        }
        if !on_compute {
            return false;
        }
        match self.last_failure.replace(at) {
            Some(previous) if at.saturating_duration_since(previous) <= COMPUTE_FAILURE_WINDOW => {
                self.last_failure = None;
                self.quiet_since = Some(at);
                self.sticky = sticky;
                true
            }
            _ => false,
        }
    }
    /// True once, when a session on the graphics queue has gone
    /// [`COMPUTE_RETRY_QUIET`] without a failure and may convert on compute
    /// again.
    pub(super) fn retry_due(&mut self, now: Instant) -> bool {
        if self.sticky {
            return false;
        }
        match self.quiet_since {
            Some(since) if now.saturating_duration_since(since) >= COMPUTE_RETRY_QUIET => {
                self.quiet_since = None;
                true
            }
            _ => false,
        }
    }
}

/// Encoder failure events already counted towards [`ComputeDemotion`], by
/// [`EncoderRecovery::failure_epoch`], and the events that were GPU resets or
/// device losses. A rebuild can run more than once for one failure; only a new
/// event counts.
#[derive(Debug, Default)]
pub(super) struct CountedFailures {
    counted: Option<u64>,
    device_loss: Option<u64>,
}

impl CountedFailures {
    /// `epoch` was a GPU reset or device loss, which is not the compute path's
    /// fault and never counts.
    pub(super) fn device_loss(&mut self, epoch: u64) {
        self.device_loss = Some(epoch);
    }

    /// True once per failure event, unless it was a device loss.
    pub(super) fn counts(&mut self, epoch: u64) -> bool {
        if self.counted == Some(epoch) || self.device_loss == Some(epoch) {
            return false;
        }
        self.counted = Some(epoch);
        true
    }
}

/// Count a rebuild's encoder failure against `demotion`: only a new failure
/// event counts, so repeated rebuilds for one failure cannot demote early.
pub(super) fn count_rebuild_failure(
    recovery: &EncoderRecovery,
    counted: &mut CountedFailures,
    demotion: &mut ComputeDemotion,
    at: Instant,
    on_compute: bool,
    sticky: bool,
) -> bool {
    recovery.failing.is_some()
        && counted.counts(recovery.failure_epoch)
        && demotion.failed(at, on_compute, sticky)
}

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
        return Err(EncoderFailure("the encoder did not produce a frame during recovery").into());
    }
    Ok(())
}

// Keep recovery testable without constructing a GPU or an AMF runtime.
pub(super) trait EncoderOutput {
    fn poll(&mut self) -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>>;
    fn pending(&self) -> bool;
    fn backlog(&self) -> usize;
    fn backlog_limit(&self) -> usize;
    fn log_stall(&self);
    fn device_removed(&self) -> Option<DeviceLost> {
        None
    }
    /// The earliest a poll can find output, when the encoder predicts it.
    fn output_due(&self) -> Option<Instant> {
        None
    }
    /// An event that ends a wait for output, armed by this call.
    fn output_event(&self) -> Option<&butterpollo_win_graphics::timing::Signal> {
        None
    }
    /// A poll can wait in the driver.
    fn poll_blocks(&self) -> bool {
        true
    }
}

impl EncoderOutput for Encoder {
    fn poll(&mut self) -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>> {
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
    fn output_due(&self) -> Option<Instant> {
        self.output_due()
    }
    fn output_event(&self) -> Option<&butterpollo_win_graphics::timing::Signal> {
        self.output_event()
    }
    fn poll_blocks(&self) -> bool {
        self.poll_blocks()
    }
}

#[derive(Default)]
pub(super) struct EncoderRecovery {
    pub(super) failing: Option<Instant>,
    /// Counts failure events, not rebuilds: a sequence that fails, rebuilds
    /// and fails again has two epochs even though `failing` never cleared.
    pub(super) failure_epoch: u64,
    pub(super) backlog_since: Option<Instant>,
    pub(super) recreations: u32,
    pub(super) device_loss: Option<DeviceLost>,
    pub(super) progress: Option<Arc<crate::stall_watch::Progress>>,
    /// TRACE only: when each output poll since the last claim began and
    /// ended ([`QueryTrace`]).
    pub(super) queries: Option<Vec<(Instant, Instant)>>,
    #[cfg(any(debug_assertions, test))]
    pub(super) stall: crate::soak_fault::EncoderStall,
}

/// TRACE only: output polls between two claims, and whether the claimed
/// picture was published while one blocked (AMF's QueryOutput waits up to
/// its timeout), with how long it then waited for the poll to end.
#[derive(Debug, Default, PartialEq)]
pub(super) struct QueryTrace {
    pub(super) queries: usize,
    pub(super) query_us: u64,
    pub(super) published_in_query: bool,
    pub(super) held_us: u64,
}

impl QueryTrace {
    pub(super) fn new(queries: &[(Instant, Instant)], published: Instant) -> Self {
        let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
        let mut trace = Self {
            queries: queries.len(),
            ..Self::default()
        };
        for &(start, end) in queries {
            trace.query_us += micros(end.saturating_duration_since(start));
            if start <= published && published < end {
                trace.published_in_query = true;
                trace.held_us = trace.held_us.max(micros(end - published));
            }
        }
        trace
    }
}

/// Polls kept for one claim's trace; a stall without claims stops here.
const TRACED_QUERIES: usize = 1024;

impl EncoderRecovery {
    /// Record one encoder failure at `now`, starting the recovery episode if
    /// this is its first failure. Returns the episode's start; the epoch
    /// increments for every failure event.
    pub(super) fn record_failure(&mut self, now: Instant) -> Instant {
        let since = *self.failing.get_or_insert(now);
        self.failure_epoch = self.failure_epoch.wrapping_add(1);
        since
    }
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
        output: Vec<butterpollo_win_graphics::encoder::Encoded>,
        now: impl Fn() -> Instant,
    ) -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>> {
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
        warnings: &butterpollo_session::session::Warnings,
        now: impl Fn() -> Instant,
    ) -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>> {
        let Some(active) = encoder.as_mut() else {
            return Ok(vec![]);
        };
        if let Some(progress) = &self.progress {
            progress.mark(crate::stall_watch::Phase::Output);
        }
        let started = self.queries.as_ref().map(|_| Instant::now());
        let polled = active.poll();
        if let (Some(queries), Some(started)) = (self.queries.as_mut(), started)
            && queries.len() < TRACED_QUERIES
        {
            queries.push((started, Instant::now()));
        }
        match polled {
            Ok(output) => self.output(output, now),
            Err(error) => {
                if let Some(loss) = active
                    .device_removed()
                    .or_else(|| DeviceLost::from_error(&error))
                {
                    self.device_loss = Some(loss);
                    self.record_failure(now());
                    return Ok(vec![]);
                }
                let since = self.record_failure(now());
                if now().duration_since(since) >= ENCODER_RECOVERY {
                    return Err(error.context(EncoderFailure("the encoder kept failing")));
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
        warnings: &butterpollo_session::session::Warnings,
        now: impl Fn() -> Instant,
    ) -> Result<Option<Vec<butterpollo_win_graphics::encoder::Encoded>>> {
        if self.failing.is_none() {
            self.recreations = 0;
        }
        let limit = butterpollo_media_policy::stream_policy::encoder_stall_limit(self.recreations);
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
                self.record_failure(now());
                return Ok(None);
            }
            let since = self.record_failure(now());
            if now().duration_since(since) >= ENCODER_RECOVERY {
                return Err(EncoderFailure("the encoder stopped returning frames").into());
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
/// caller keeps its original pacing. `picture` and `mean` are the source's median and mean
/// interval between pictures while frames are claimed as they arrive, `None` on the host's
/// own grid.
fn phase_interval(
    s: &Session,
    at: Instant,
    period: Duration,
    picture: Option<Duration>,
    mean: Option<Duration>,
) -> Option<Duration> {
    let mut sync = s.phase_sync.lock().unwrap();
    sync.set_source(picture, mean, period);
    let interval = sync.interval(at, period);
    match sync.aside_changed() {
        Some(true) => tracing::info!(
            client = %s.launch.client.name,
            picture_ns = picture.map_or(0, |picture| picture.as_nanos() as u64),
            stream_period_ns = period.as_nanos() as u64,
            "phase lock stands aside: the source presents at the stream rate, so frames are claimed as they arrive instead of held"
        ),
        Some(false) if sync.reporting(at) => {
            tracing::info!(client = %s.launch.client.name, "phase lock steers frames again")
        }
        _ => {}
    }
    let interval = interval?;
    if sync.log_due(at) {
        tracing::info!(
            client = %s.launch.client.name,
            interval_ns = interval.as_nanos() as u64,
            stream_period_ns = period.as_nanos() as u64,
            picture_ns = picture.map_or(0, |picture| picture.as_nanos() as u64),
            "phase lock applied"
        );
    }
    Some(interval)
}

/// A switch the session loop refused or could not make: the stream keeps `from`, codes
/// `applied`, and a rate change the switch carried is answered with `reason`.
fn refuse_switch(
    s: &Session,
    from: butterpollo_media_policy::reconfigure::Mode,
    applied: butterpollo_media_policy::reconfigure::Mode,
    reason: butterpollo_media_policy::rate_change::Reason,
) {
    let mut reconfigure = s.reconfigure.lock().unwrap();
    reconfigure.refused(from, applied);
    s.rate_changes.switched(from, Some(reason));
}

/// What every phase of a session thread reads: the media sockets, the host and the session.
struct SessionThread<'a> {
    m: &'a Arc<Media>,
    h: &'a Shared,
    s: &'a Arc<Session>,
}

impl SessionThread<'_> {
    /// The stream's picture takes `size`: a co-op player's window follows it, and any
    /// other stream's display does. A switch calls this, and so does a refused switch
    /// going back to the size the stream had.
    fn follow_size(&self, prepared: &crate::display_session::Ready, size: (u32, u32)) {
        match self.s.launch.nucleus {
            Some(seat) => {
                if let Err(error) = crate::nucleus::resize(self.h, seat, size) {
                    tracing::warn!(client = %self.s.launch.client.name, error = %format!("{error:#}"), "co-op window resize refused");
                }
            }
            None => prepared.follow_shape(size),
        }
    }
}

/// The capture, display and encoder leases and COM, held by the session thread until its
/// teardown releases them in order. Fields are declared in the reverse of the order the
/// thread created them: a struct drops its fields in declaration order, so an unwind drops
/// them as it dropped the thread's locals, the encoder before the capture and display.
#[derive(Default)]
struct Leases {
    encoder: Option<Encoder>,
    latest: Option<Arc<Source>>,
    // Follows the app's window (`capture_window = window`) from before the first capture.
    // It goes after the capture: its end shows the display again, which a capture still
    // open would switch to just before it closes.
    app_window: Option<app_window::AppWindow>,
    stream_preparation: Option<crate::display_session::StreamPreparation>,
    // Keep GPU scheduling priority, MMCSS and process priority elevated until the
    // encoder, capture and display leases finish tearing down; dropping them earlier
    // lowers GPU scheduling priority while D3D12/NVENC teardown is still in flight.
    streaming: Option<butterpollo_win_graphics::timing::StreamingScope>,
    // Keeps the console display on for the whole stream (issue #117), released on
    // every way out; Windows ends it too if the process exits or crashes.
    display_power: Option<butterpollo_win_io::wake::PowerRequest>,
    priority: Option<Priority>,
    com: Option<ComGuard>,
}

/// The session thread's workers and records, released at teardown before its [`Leases`].
/// Fields in the reverse of the order the thread created them, as in [`Leases`].
struct Workers {
    // A co-op player's stream ends with its Nucleus Co-op session.
    coop_watch: Option<crate::nucleus::Watch>,
    #[cfg(any(test, feature = "test-support"))]
    fixture_preparation: Option<
        Box<
            dyn butterpollo_session::session::Preparation<crate::display_session::StreamPreparation>,
        >,
    >,
    // Ends the host-side timing record's writer, which writes its footer, on every
    // way out of the stream.
    timing_record: crate::timing_record::Detach,
    keyframe_loss_memory: Option<crate::state::loss_memory::Guard>,
    webrtc_sender: Option<Box<dyn crate::webrtc_bridge::WebRtcVideoSink>>,
    pyrowave_sender: Option<crate::pyrowave_send::Sender>,
    audio: Option<thread::JoinHandle<()>>,
    client_commands: Option<crate::process::ClientCommands>,
}

impl Workers {
    fn new(s: &Arc<Session>) -> Self {
        Workers {
            coop_watch: None,
            #[cfg(any(test, feature = "test-support"))]
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
                // Release builds set `panic = "abort"` (Cargo.toml): a panic there ends the host
                // process, and the service supervisor starts it again, so this catch and the
                // poison recovery in the teardown run only in unwinding builds (tests, debug).
                // A panic ends the session like any failure: the teardown below releases its
                // workers, leases and registration, which an unwind past it would leave in
                // `h.sessions` with the control and audio threads still running.
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                    workers.coop_watch = crate::nucleus::Watch::start(&h, &s)?;
                    #[cfg(any(test, feature = "test-support"))]
                    if let Some(fixture) = &h.reconnect_fixture {
                        workers.fixture_preparation = s.launch.preparation.lock().unwrap().take();
                        return fixture.stream(&m, &h, &s);
                    }
                    if s.launch.mic_only() {
                        session.wait_mic_only();
                        return Ok(());
                    }
                    leases.com = Some(ComGuard::new()?);
                    leases.priority = Some(Priority::new());
                    leases.display_power = stream_display_power();
                    leases.streaming = Some(crate::win_bridge::enter_streaming_scope());
                    let Some(config) = session.configure(&mut workers)? else {
                        return Ok(());
                    };
                    let display =
                        session.prepare_display(&config.c, &mut leases.stream_preparation)?;
                    leases.app_window = session.start_app_window(&config.c, &display.prepared);
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
                }));
                let result = ran.unwrap_or_else(|panic| {
                    let what = panic_text(panic.as_ref());
                    tracing::error!(client=%s.launch.client.name, panic=%what, "session thread panicked; ending the session");
                    // A panic with the registry locked poisoned it for every later user,
                    // so a new session could not start.
                    h.sessions.clear_poison();
                    Err(anyhow::anyhow!("the stream thread panicked: {what}"))
                });
                session.teardown(result, workers, leases);
            })
    }
}

/// The display-required power request a video stream holds; a refusal is
/// logged and the stream goes on.
fn stream_display_power() -> Option<butterpollo_win_io::wake::PowerRequest> {
    butterpollo_win_io::wake::PowerRequest::display_required("Rubylight is streaming this PC")
        .inspect_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "the console display may switch off during the stream");
        })
        .ok()
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

#[cfg(test)]
mod wait_tests;
