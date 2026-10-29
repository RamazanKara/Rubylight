//! Logs where a stream thread is when it stops making progress
//! (`butterpollo_core::stall_watch`), with how long a trivial query on the
//! stream's GPU has been outstanding. A stalled GPU holds both; a thread stuck
//! on the host's own work (a lock, a fence, the network) only the thread.
//! Observation only: nothing in the stream changes.
use butterpollo_core::stall_watch::{Event, Watch};
use std::{
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Longer than any normal wait in a stream thread (a 50 ms repeat, a 20 fps
/// static picture), short of a freeze anyone would report.
const LIMIT: Duration = Duration::from_millis(500);
/// Before a picture is claimed every wait is bounded by about a stream period
/// (a 50 ms capture wait, a repeat, a pacing deadline), so a stall there is
/// reported sooner: a 285 ms gap while a game launched stayed under `LIMIT`
/// and named no phase. At most two stream periods, so a very low frame rate
/// is not reported every frame.
const CLAIM_LIMIT: Duration = Duration::from_millis(150);
const TICK: Duration = Duration::from_millis(50);
const PROBE_EVERY: Duration = Duration::from_millis(100);
/// A probe query slower than this is logged even without a thread stall.
const PROBE_LATE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Loop,
    Picture,
    EncoderCreate,
    Encode,
    Output,
    Send,
    Capture,
    Reopen,
    /// Handing the game on the stream's display to the running app.
    AppTracking,
    /// Opening what the thread works with, before its first phase.
    Setup,
}
impl Phase {
    fn name(phase: u8) -> &'static str {
        [
            "stream loop",
            "waiting for a picture",
            "creating the encoder",
            "encoding",
            "collecting encoder output",
            "sending",
            "capturing",
            "reopening capture",
            "tracking the app's game",
            "setting up",
        ]
        .get(usize::from(phase))
        .copied()
        .unwrap_or("unknown")
    }
    /// How long a thread whose stream period is `period` may stay in `phase`
    /// before it is reported. The encoder, the network and capture reopening
    /// have longer waits of their own (encoder recovery at 250 ms, slow sends)
    /// and keep `LIMIT`.
    fn limit(phase: u8, period: Duration) -> Duration {
        const CLAIM_PHASES: [Phase; 3] = [Phase::Loop, Phase::Picture, Phase::AppTracking];
        if CLAIM_PHASES.iter().any(|claim| *claim as u8 == phase) {
            CLAIM_LIMIT.max(period * 2)
        } else {
            LIMIT
        }
    }
}

fn now() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

pub struct Progress {
    thread: &'static str,
    stream: String,
    phase: AtomicU8,
    at: AtomicU64,
    /// The stream's frame period in nanoseconds; 0 until set.
    period: AtomicU64,
    gpu: Option<Arc<GpuProbe>>,
}
impl Progress {
    pub fn mark(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
        self.at.store(now(), Ordering::Release);
    }
    /// The stream's frame period, which a claim may take twice before it is
    /// reported.
    pub fn set_period(&self, period: Duration) {
        self.period
            .store(period.as_nanos() as u64, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Watched {
    streams: Mutex<Vec<(Weak<Progress>, Watch)>>,
    wake: Condvar,
}

/// Watched until the returned handle is dropped.
/// The thread starts in `Phase::Setup`.
pub fn register(thread: &'static str, stream: &str, gpu: Option<Arc<GpuProbe>>) -> Arc<Progress> {
    static WATCHED: OnceLock<Arc<Watched>> = OnceLock::new();
    let progress = Arc::new(Progress {
        thread,
        stream: stream.to_owned(),
        phase: AtomicU8::new(Phase::Setup as u8),
        at: AtomicU64::new(now()),
        period: AtomicU64::new(0),
        gpu,
    });
    let watched = WATCHED.get_or_init(|| {
        let watched = Arc::new(Watched::default());
        let watcher = watched.clone();
        if let Err(error) = thread::Builder::new()
            .name("stall watch".into())
            .spawn(move || watch(&watcher))
        {
            tracing::warn!(%error, "stream stalls are not reported");
        }
        watched
    });
    watched
        .streams
        .lock()
        .unwrap()
        .push((Arc::downgrade(&progress), Watch::default()));
    watched.wake.notify_one();
    progress
}

fn watch(watched: &Watched) {
    loop {
        thread::sleep(TICK);
        let mut streams = watched.streams.lock().unwrap();
        streams.retain(|(progress, _)| progress.strong_count() > 0);
        while streams.is_empty() {
            streams = watched.wake.wait(streams).unwrap();
            streams.retain(|(progress, _)| progress.strong_count() > 0);
        }
        for (progress, watch) in streams.iter_mut() {
            let Some(progress) = progress.upgrade() else {
                continue;
            };
            let at = progress.at.load(Ordering::Acquire);
            let phase = progress.phase.load(Ordering::Relaxed);
            let period = Duration::from_nanos(progress.period.load(Ordering::Relaxed));
            let limit = Phase::limit(phase, period).as_nanos() as u64;
            let event = watch.check(at, phase, false, now(), limit);
            let (gpu_query_outstanding_ms, gpu_query_last_ms) = progress
                .gpu
                .as_ref()
                .map_or((None, None), |gpu| gpu.state());
            match event {
                Some(Event::Stalled { phase, ms }) => tracing::warn!(
                    thread = progress.thread,
                    stream_id = %progress.stream,
                    phase = Phase::name(phase),
                    stalled_ms = ms,
                    gpu_query_outstanding_ms,
                    gpu_query_last_ms,
                    "stream thread stopped making progress"
                ),
                Some(Event::Resumed { phase, ms }) => tracing::warn!(
                    thread = progress.thread,
                    stream_id = %progress.stream,
                    phase = Phase::name(phase),
                    stalled_ms = ms,
                    gpu_query_outstanding_ms,
                    gpu_query_last_ms,
                    "stream thread resumed after a stall"
                ),
                None => {}
            }
        }
    }
}

/// Times an event query on the stream's GPU every 100 ms, on a thread and
/// device of its own, until the last handle is dropped.
pub struct GpuProbe {
    /// When the outstanding query was issued (`now()`); 0 when none is.
    issued: AtomicU64,
    last_us: AtomicU64,
}
impl GpuProbe {
    pub fn start(display: &str, stream: &str) -> Arc<Self> {
        let probe = Arc::new(Self {
            issued: AtomicU64::new(0),
            last_us: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&probe);
        let (display, stream) = (display.to_owned(), stream.to_owned());
        let started = thread::Builder::new().name("gpu probe".into()).spawn(move || {
            let query = match butterpollo_windows::gpu_probe::Probe::new(&display) {
                Ok(query) => query,
                Err(error) => {
                    let output = &display;
                    tracing::info!(error = %format!("{error:#}"), display = %output, "GPU stall probe unavailable");
                    return;
                }
            };
            let output = &display;
            tracing::info!(display = %output, stream_id = %stream, "GPU stall probe opened");
            loop {
                thread::sleep(PROBE_EVERY);
                let Some(probe) = weak.upgrade() else { return };
                let issued = now();
                probe.issued.store(issued, Ordering::Release);
                // The probe holds no handle while it waits: the stream can end.
                drop(probe);
                query.issue();
                let mut reported = false;
                let done = loop {
                    match query.done() {
                        Ok(true) => break true,
                        Ok(false) => {}
                        Err(error) => {
                            tracing::info!(error = %format!("{error:#}"), %stream, "GPU stall probe stopped");
                            break false;
                        }
                    }
                    let waited = Duration::from_nanos(now() - issued);
                    if !reported && waited >= LIMIT {
                        reported = true;
                        tracing::warn!(stream_id = %stream, waited_ms = waited.as_millis(), "the GPU has not finished a trivial query");
                    }
                    if weak.strong_count() == 0 {
                        return;
                    }
                    thread::sleep(Duration::from_millis(1));
                };
                let took = Duration::from_nanos(now() - issued);
                let Some(probe) = weak.upgrade() else { return };
                probe.issued.store(0, Ordering::Release);
                probe.last_us.store(took.as_micros() as u64, Ordering::Relaxed);
                if !done {
                    return;
                }
                if took >= PROBE_LATE {
                    tracing::warn!(stream_id = %stream, gpu_query_ms = took.as_millis(), "a trivial GPU query finished late");
                }
            }
        });
        if let Err(error) = started {
            tracing::warn!(%error, "GPU stall probe not started");
        }
        probe
    }
    /// Milliseconds the outstanding query has waited, and the last one took.
    fn state(&self) -> (Option<u64>, Option<u64>) {
        let issued = self.issued.load(Ordering::Acquire);
        let last = self.last_us.load(Ordering::Relaxed);
        (
            (issued != 0).then(|| now().saturating_sub(issued) / 1_000_000),
            (last != 0).then_some(last / 1000),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_before_the_claim_is_reported_sooner_than_one_in_the_encoder_or_capture() {
        let ms = |ms: u64| ms * 1_000_000;
        // 160 ms in the steps before a claim, the app's game look among them.
        for phase in [Phase::Loop, Phase::Picture, Phase::AppTracking] {
            let limit = Phase::limit(phase as u8, Duration::from_millis(8)).as_nanos() as u64;
            assert_eq!(
                Watch::default().check(ms(100), phase as u8, false, ms(260), limit),
                Some(Event::Stalled {
                    phase: phase as u8,
                    ms: 160
                }),
                "{}",
                Phase::name(phase as u8)
            );
        }
        // Encoder creation, capture reopening and a thread still setting up
        // have longer waits of their own.
        for phase in [
            Phase::Setup,
            Phase::EncoderCreate,
            Phase::Encode,
            Phase::Output,
            Phase::Send,
            Phase::Capture,
            Phase::Reopen,
        ] {
            let limit = Phase::limit(phase as u8, Duration::from_millis(8)).as_nanos() as u64;
            assert_eq!(
                Watch::default().check(ms(100), phase as u8, false, ms(260), limit),
                None,
                "{}",
                Phase::name(phase as u8)
            );
        }
        // A stream at 5 fps waits 200 ms for a picture as a matter of course.
        let slow = Duration::from_millis(200);
        assert_eq!(
            Phase::limit(Phase::Picture as u8, slow),
            Duration::from_millis(400)
        );
        assert_eq!(Phase::limit(Phase::Encode as u8, slow), LIMIT);
        assert_eq!(
            Phase::name(Phase::AppTracking as u8),
            "tracking the app's game"
        );
        assert_eq!(Phase::name(Phase::Setup as u8), "setting up");
    }
}
