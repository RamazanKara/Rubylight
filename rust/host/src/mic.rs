//! Client microphones (`butterpollo_core::mic` has the wire format). One
//! receiver serves every session on the microphone port. The client
//! speaking is decoded as its packets arrive and played into Steam
//! Streaming Microphone by a render thread that runs only while it speaks;
//! a second client is heard once the first has been quiet for a second.
use crate::state::{Session, Shared};
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    config::Config,
    mic::{self, Arrival, Playout, Sequencer},
};
use butterpollo_windows::{
    audio::OpusDecoder,
    audio_route,
    capture::{ComGuard, Priority},
    mic::Render,
};
use std::{
    net::UdpSocket,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Packets accepted for a session and refused, for tests and diagnostics.
#[derive(Default)]
pub struct Counters {
    pub accepted: AtomicU64,
    pub refused: AtomicU64,
}
/// A speaker that sends nothing for this long has stopped; its render
/// thread ends and the device is released.
const IDLE: Duration = Duration::from_secs(3);
/// Another client takes the microphone once the speaker has been quiet this long.
const HANDOVER: Duration = Duration::from_secs(1);
/// The stream warning of a device whose microphone waits for another's; `/session` tells
/// that device too ([`busy_for`]).
const MIC_BUSY_WARNING: &str = "microphone_busy";

/// Whether `client`'s microphone waits for another device's: one of its live sessions has
/// the busy warning. It clears once its microphone plays, or a few seconds after it last
/// waited.
pub fn busy_for(h: &Shared, client: &str) -> bool {
    h.sessions.lock().unwrap().active.values().any(|s| {
        !s.stopping()
            && s.launch.client.uuid == client
            && s.launch
                .warnings
                .snapshot()
                .iter()
                .any(|w| w.code == MIC_BUSY_WARNING)
    })
}
/// Wait before opening Steam Streaming Microphone again after a failure.
const RETRY: Duration = Duration::from_secs(5);
const WARNING: &str = "microphone";
const ROUTING_WARNING: &str = "microphone_default";
const FORMAT_WARNING: &str = "microphone_format";
const GAIN_WARNING: &str = "microphone_gain";

pub fn spawn(h: Shared, socket: UdpSocket) -> Result<Arc<Counters>> {
    // The shared UDP factory is nonblocking, which bypasses the receive timeout.
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    let directory = exe_directory(std::env::current_exe())?;
    let counters = Arc::new(Counters::default());
    let receiver_counters = counters.clone();
    thread::Builder::new()
        .name("microphone".into())
        .spawn(move || Receiver::new(h, receiver_counters, directory).run(&socket))?;
    Ok(counters)
}

/// The folder of the host's executable, where libopus is: a host that cannot
/// find itself has no decoder, which is an error rather than an empty folder.
fn exe_directory(exe: std::io::Result<PathBuf>) -> Result<PathBuf> {
    let exe = exe.context("the host's executable could not be found for the Opus decoder")?;
    exe.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Into::into)
        .with_context(|| format!("the host's executable {} has no folder", exe.display()))
}

#[derive(Default)]
struct Stats {
    packets: u64,
    concealed: u64,
    recovered: u64,
    late: u64,
    damaged: u64,
}
struct Speaker {
    session: Arc<Session>,
    sequencer: Sequencer,
    heard: Instant,
    started: Instant,
    stats: Stats,
    /// How evenly its packets arrive, for the log.
    gaps: butterpollo_core::arrival_gaps::ArrivalGaps,
}
struct Receiver {
    h: Shared,
    counters: Arc<Counters>,
    /// Off in the GPU-free reconnect tests, which only count packets.
    native: bool,
    directory: PathBuf,
    speaker: Option<Speaker>,
    /// Shared with the render thread, which conceals dry spells with it.
    /// Lock order: the decoder before the playout, on both threads.
    decoder: Arc<Mutex<Option<OpusDecoder>>>,
    playout: Arc<Mutex<Playout>>,
    output: Output,
    pcm: Vec<f32>,
}
impl Receiver {
    fn new(h: Shared, counters: Arc<Counters>, directory: PathBuf) -> Self {
        #[cfg(test)]
        let native = h.reconnect_fixture.is_none();
        #[cfg(not(test))]
        let native = true;
        Self {
            h,
            counters,
            native,
            directory,
            speaker: None,
            decoder: Default::default(),
            playout: Default::default(),
            output: Output::default(),
            pcm: Vec::new(),
        }
    }
    fn run(mut self, socket: &UdpSocket) {
        let mut datagram = [0; 2048];
        let mut failures = 0u32;
        while !self.h.stop.load(Ordering::Acquire) {
            match socket.recv_from(&mut datagram) {
                Ok((n, peer)) => {
                    failures = 0;
                    self.receive(&datagram[..n], peer.ip().to_canonical());
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                // Windows reports an ICMP port unreachable for an earlier
                // datagram as a reset on the next receive; nothing is lost.
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                Err(e) => {
                    failures += 1;
                    if failures == 1 {
                        tracing::warn!(error = %e, "microphone receive failed");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
            if self
                .speaker
                .as_ref()
                .is_some_and(|s| s.heard.elapsed() > IDLE || s.session.stopping())
            {
                self.finish();
            }
        }
        self.finish();
    }
    fn receive(&mut self, datagram: &[u8], peer: std::net::IpAddr) {
        let Some(packet) = mic::parse(datagram) else {
            return;
        };
        let candidates: Vec<_> = self
            .h
            .sessions
            .lock()
            .unwrap()
            .active
            .values()
            .filter(|s| s.config.mic && s.client_ip() == peer && !s.stopping())
            .cloned()
            .collect();
        let Some((session, opus)) = candidates.into_iter().find_map(|s| {
            let opus = mic::open(&s.launch.key, s.launch.key_id, &packet).ok()?;
            Some((s, opus))
        }) else {
            self.counters.refused.fetch_add(1, Ordering::Relaxed);
            return;
        };
        // A guest's microphone plays on the PC only with its keyboard-and-mouse permission.
        if !self.h.device_guests.full_access(&session) {
            self.counters.refused.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if !self
            .speaker
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(&s.session, &session))
        {
            if let Some(speaker) = self
                .speaker
                .as_ref()
                .filter(|s| s.heard.elapsed() < HANDOVER)
            {
                // One microphone plays at a time; this one waits until the other goes quiet.
                self.counters.refused.fetch_add(1, Ordering::Relaxed);
                // On the waiting device's stream card and in the log, while it lasts.
                session.launch.warnings.event(
                    MIC_BUSY_WARNING,
                    format!(
                        "This device's microphone is not played while {}'s is; it plays once that one stops.",
                        speaker.session.launch.client.name
                    ),
                    HANDOVER * 5,
                );
                return;
            }
            self.finish();
            session.launch.warnings.clear(MIC_BUSY_WARNING);
            tracing::info!(client = %session.launch.client.name, "microphone started");
            self.speaker = Some(Speaker {
                session,
                sequencer: Sequencer::default(),
                heard: Instant::now(),
                started: Instant::now(),
                stats: Stats::default(),
                gaps: Default::default(),
            });
        }
        self.counters.accepted.fetch_add(1, Ordering::Relaxed);
        let speaker = self.speaker.as_mut().unwrap();
        speaker.heard = Instant::now();
        speaker.stats.packets += 1;
        if let Some(window) = speaker.gaps.note(speaker.heard) {
            tracing::info!(
                client = %speaker.session.launch.client.name,
                packets = window.arrivals,
                gaps_25_50 = window.gaps_25_50,
                gaps_50_plus = window.gaps_50_plus,
                pauses = window.pauses,
                max_gap_ms = window.max_gap_ms,
                "microphone arrivals"
            );
        }
        let arrival = speaker.sequencer.arrive(packet.sequence);
        if arrival == Arrival::Late {
            speaker.stats.late += 1;
            return;
        }
        if !self.native {
            return;
        }
        let mut decoder = self.decoder.lock().unwrap();
        if decoder.is_none() {
            match OpusDecoder::new(&self.directory) {
                Ok(opened) => *decoder = Some(opened),
                Err(error) => {
                    speaker.session.launch.warnings.set(
                        WARNING,
                        format!("Microphone unavailable: the Opus decoder could not be loaded ({error:#}). Reinstall Rubylight."),
                    );
                    return;
                }
            }
        }
        self.pcm.clear();
        let packet_frames = decode_packet(
            decoder.as_mut().unwrap(),
            arrival,
            &opus,
            &mut self.pcm,
            &mut speaker.stats,
        );
        // Still holding the decoder: no concealment runs between this
        // packet's decoding and its audio reaching the queue.
        self.playout
            .lock()
            .unwrap()
            .push_batch(&self.pcm, packet_frames);
        drop(decoder);
        let session = speaker.session.clone();
        self.output
            .ensure(&self.h, &session, &self.decoder, &self.playout);
    }
    fn finish(&mut self) {
        let Some(speaker) = self.speaker.take() else {
            return;
        };
        self.output.stop(&speaker.session);
        let (trimmed, underruns, underrun, filled, conceal_failed) = {
            let mut playout = self.playout.lock().unwrap();
            let counts = (
                playout.trimmed,
                playout.underruns,
                playout.underrun_samples,
                playout.concealed_samples,
                playout.conceal_failed,
            );
            playout.clear();
            counts
        };
        let s = &speaker.stats;
        tracing::info!(
            client = %speaker.session.launch.client.name,
            seconds = speaker.started.elapsed().as_secs(),
            packets = s.packets,
            concealed = s.concealed,
            recovered = s.recovered,
            late = s.late,
            damaged = s.damaged,
            trimmed_ms = trimmed * 1000 / mic::RATE as u64,
            underruns,
            underrun_ms = underrun * 1000 / mic::RATE as u64,
            underrun_concealed_ms = filled * 1000 / mic::RATE as u64,
            conceal_failed,
            "microphone stopped"
        );
    }
}

fn decode_packet(
    decoder: &mut OpusDecoder,
    arrival: Arrival,
    opus: &[u8],
    pcm: &mut Vec<f32>,
    stats: &mut Stats,
) -> usize {
    let conceal = |decoder: &mut OpusDecoder, pcm: &mut Vec<f32>, stats: &mut Stats| match decoder
        .decode(None, false, pcm)
    {
        Ok(frames) => {
            stats.concealed += 1;
            frames
        }
        Err(_) => 0,
    };
    match arrival {
        Arrival::Restart => decoder.reset(),
        Arrival::AfterLoss { missing } => {
            for _ in 1..missing {
                conceal(decoder, pcm, stats);
            }
            if decoder.decode(Some(opus), true, pcm).is_ok() {
                stats.recovered += 1;
            } else {
                conceal(decoder, pcm, stats);
            }
        }
        Arrival::Next => {}
        Arrival::Late => return 0,
    }
    match decoder.decode(Some(opus), false, pcm) {
        Ok(frames) => frames,
        Err(_) => {
            // The sequencer has consumed this packet. The next packet will be
            // in order, so conceal this one now to preserve the playout clock.
            stats.damaged += 1;
            conceal(decoder, pcm, stats)
        }
    }
}

/// The render thread and what it plays.
#[derive(Default)]
struct Output {
    thread: Option<(Arc<AtomicBool>, thread::JoinHandle<Result<()>>)>,
    /// A stopped render thread still installing or opening the device. The
    /// next one starts once it has gone, so they never share the device.
    stopping: Option<thread::JoinHandle<Result<()>>>,
    /// Set by the render thread once Steam Streaming Microphone is open.
    opened: Arc<AtomicBool>,
    retry: Option<Instant>,
}
impl Output {
    fn ensure(
        &mut self,
        h: &Shared,
        session: &Arc<Session>,
        decoder: &Arc<Mutex<Option<OpusDecoder>>>,
        playout: &Arc<Mutex<Playout>>,
    ) {
        if !self.stopped() {
            return;
        }
        if self.opened.swap(false, Ordering::AcqRel) {
            session.launch.warnings.clear(WARNING);
        }
        if self
            .thread
            .as_ref()
            .is_some_and(|(_, thread)| !thread.is_finished())
        {
            return;
        }
        if let Some((_, thread)) = self.thread.take() {
            Self::report(session, thread.join());
            self.retry = Some(Instant::now() + RETRY);
        }
        if self.retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let config = h.config.read().unwrap().clone();
        let directory = h.directory.clone();
        let warnings = session.launch.warnings.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let decoder = decoder.clone();
        let playout = playout.clone();
        let thread_stop = stop.clone();
        let opened = self.opened.clone();
        match thread::Builder::new()
            .name("microphone render".into())
            .spawn(move || {
                render(
                    &config,
                    &directory,
                    &decoder,
                    &playout,
                    &thread_stop,
                    &opened,
                    &warnings,
                )
            }) {
            Ok(thread) => self.thread = Some((stop, thread)),
            Err(error) => {
                tracing::warn!(%error, "microphone render thread could not start");
                self.retry = Some(Instant::now() + RETRY);
            }
        }
    }
    fn stop(&mut self, session: &Arc<Session>) {
        if let Some(result) = self.retire() {
            Self::report(session, result);
        }
        self.retry = None;
        session.launch.warnings.clear(ROUTING_WARNING);
        session.launch.warnings.clear(FORMAT_WARNING);
        session.launch.warnings.clear(GAIN_WARNING);
    }
    /// Stops the render thread without waiting for one still installing or
    /// opening the device: that would hold up the receiver, and with it every
    /// other session's microphone packets. Its result, if it has ended.
    fn retire(&mut self) -> Option<thread::Result<Result<()>>> {
        let (stop, thread) = self.thread.take()?;
        stop.store(true, Ordering::Release);
        // A late "opened" from it must not clear the next session's warning.
        self.opened = Arc::default();
        if thread.is_finished() {
            return Some(thread.join());
        }
        self.stopping = Some(thread);
        None
    }
    /// Whether no stopped render thread is still running.
    fn stopped(&mut self) -> bool {
        let Some(thread) = self.stopping.take_if(|thread| thread.is_finished()) else {
            return self.stopping.is_none();
        };
        match thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %format!("{error:#}"), "stopped microphone output failed")
            }
            Err(_) => tracing::warn!("stopped microphone render thread panicked"),
        }
        true
    }
    fn report(session: &Arc<Session>, result: thread::Result<Result<()>>) {
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => format!("{error:#}"),
            Err(_) => "the render thread panicked".into(),
        };
        tracing::warn!(%error, "microphone output failed");
        session
            .launch
            .warnings
            .set(WARNING, format!("Microphone unavailable: {error}"));
    }
}

impl Drop for Output {
    /// At shutdown, a stopped thread still gets to put the microphone
    /// defaults back.
    fn drop(&mut self) {
        if let Some(thread) = self.stopping.take() {
            let _ = thread.join();
        }
    }
}

fn render(
    config: &Config,
    directory: &std::path::Path,
    decoder: &Mutex<Option<OpusDecoder>>,
    playout: &Mutex<Playout>,
    stop: &AtomicBool,
    opened: &AtomicBool,
    warnings: &butterpollo_core::session::Warnings,
) -> Result<()> {
    let _com = ComGuard::new()?;
    let _priority = Priority::input();
    let endpoint = endpoint(config)?;
    // Stopped while installing: neither open the device nor move the defaults.
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    // Before opening: the driver copies samples between its sides unconverted.
    match audio_route::match_steam_microphone_format(&endpoint) {
        Ok(formats) => {
            warnings.clear(FORMAT_WARNING);
            tracing::info!(
                playback_before = %formats.before,
                playback = %formats.after,
                recording = %formats.recording,
                changed = formats.changed(),
                "Steam Streaming Microphone formats"
            );
        }
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Steam Streaming Microphone formats could not be matched; the microphone may sound distorted");
            // Output::stop has cleared the warning already when this thread was retired meanwhile.
            if !stop.load(Ordering::Acquire) {
                warnings.set(FORMAT_WARNING, format!("Microphone may sound distorted: Steam Streaming Microphone's playback format could not be matched to its recording format ({error:#}). Give both the same format in Windows' Sound settings."));
            }
        }
    }
    let mut render = Render::open(&endpoint)?;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let _route = match audio_route::MicrophoneRoute::acquire(directory) {
        Ok(route) => {
            warnings.clear(ROUTING_WARNING);
            Some(route)
        }
        Err(error) => {
            tracing::warn!(%error, "client microphone could not become the default; forwarding continues");
            warnings.set(ROUTING_WARNING, format!("Microphone is forwarding, but Windows could not select it automatically ({error:#}). Select Microphone (Steam Streaming Microphone) in your app."));
            None
        }
    };
    opened.store(true, Ordering::Release);
    // The level checks are calls into Windows' audio service, which can
    // stall: they run beside the render loop, never in it.
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        thread::Builder::new()
            .name("microphone level".into())
            .spawn_scoped(scope, || keep_gain(warnings, stop, &done))
            .map_err(|error| tracing::warn!(%error, "microphone level thread could not start"))
            .ok();
        let played = (|| -> Result<()> {
            let mut warned = None;
            while !stop.load(Ordering::Acquire) {
                render.wait(Duration::from_millis(50));
                // A dry spell plays the decoder's concealment, not silence.
                // The playout is locked for the pull only, and the decoder
                // only inside it, without waiting.
                render.fill_concealing(playout, &mut |out| {
                    conceal_frame(decoder, out, &mut warned, Instant::now())
                })?;
            }
            Ok(())
        })();
        done.store(true, Ordering::Release);
        played
    })
}
/// How often a failing concealment is logged.
const CONCEAL_WARNING: Duration = Duration::from_secs(10);
/// One concealed frame from the receiver's decoder for the render thread's
/// playout ([`mic::Conceal`]). The render thread runs at raised priority and
/// must not wait on the receiver: while the receiver holds the decoder it is
/// queueing a packet, which ends the dry spell anyway, so a busy decoder
/// gives nothing this wake-up. A failure is logged at most once every
/// `CONCEAL_WARNING` (`warned`: the last time) and counted by the playout.
fn conceal_frame(
    decoder: &Mutex<Option<OpusDecoder>>,
    out: &mut Vec<f32>,
    warned: &mut Option<Instant>,
    now: Instant,
) -> std::result::Result<usize, ()> {
    let Ok(mut decoder) = decoder.try_lock() else {
        return Ok(0);
    };
    let Some(decoder) = decoder.as_mut() else {
        return Ok(0);
    };
    decoder.decode(None, false, out).map_err(|error| {
        if warning_due(warned, now) {
            tracing::warn!(error = %format!("{error:#}"), "microphone underrun could not be concealed; playing silence");
        }
    })
}
/// Whether a concealment failure at `now` is logged; each logged one
/// silences the next `CONCEAL_WARNING`.
fn warning_due(warned: &mut Option<Instant>, now: Instant) -> bool {
    let due = warned.is_none_or(|at| now.saturating_duration_since(at) >= CONCEAL_WARNING);
    if due {
        *warned = Some(now);
    }
    due
}
/// The level thread: `GainCap` until the render thread stops.
fn keep_gain(warnings: &butterpollo_core::session::Warnings, stop: &AtomicBool, done: &AtomicBool) {
    let _com = match ComGuard::new() {
        Ok(com) => com,
        Err(error) => return gain_failed(&error, warnings, stop),
    };
    let mut gain = GainCap::open(warnings, stop);
    while !stop.load(Ordering::Acquire) && !done.load(Ordering::Acquire) {
        gain.check(Instant::now(), warnings, stop);
        thread::sleep(Duration::from_millis(50));
    }
}
/// How often the level thread checks that no app (an automatic gain, as in
/// Discord) raised Steam Streaming Microphone's recording level again.
const GAIN_CHECK: Duration = Duration::from_secs(1);
/// Keeps Steam Streaming Microphone's recording level at or below 0 dB
/// (`audio_route::SteamMicrophoneGain`): at once, then every `GAIN_CHECK`.
/// A failure is reported once and ends the checks.
struct GainCap {
    gain: Option<audio_route::SteamMicrophoneGain>,
    next: Option<Instant>,
}
impl GainCap {
    fn open(warnings: &butterpollo_core::session::Warnings, stop: &AtomicBool) -> Self {
        let gain = audio_route::SteamMicrophoneGain::open()
            .inspect_err(|error| gain_failed(error, warnings, stop))
            .ok();
        Self { gain, next: None }
    }
    /// Whether a check is due at `now`; each one schedules the next.
    fn due(&mut self, now: Instant) -> bool {
        if self.next.is_some_and(|next| now < next) {
            return false;
        }
        self.next = Some(now + GAIN_CHECK);
        true
    }
    fn check(
        &mut self,
        now: Instant,
        warnings: &butterpollo_core::session::Warnings,
        stop: &AtomicBool,
    ) {
        if self.gain.is_none() || !self.due(now) {
            return;
        }
        match self.gain.as_ref().unwrap().limit() {
            Ok(None) => warnings.clear(GAIN_WARNING),
            Ok(Some(level_db)) => {
                warnings.clear(GAIN_WARNING);
                tracing::info!(
                    level_db,
                    "Steam Streaming Microphone recording level lowered to 0 dB: above it, the client's voice is amplified into clipping"
                );
            }
            Err(error) => {
                gain_failed(&error, warnings, stop);
                self.gain = None;
            }
        }
    }
}
fn gain_failed(
    error: &anyhow::Error,
    warnings: &butterpollo_core::session::Warnings,
    stop: &AtomicBool,
) {
    tracing::warn!(error = %format!("{error:#}"), "Steam Streaming Microphone recording level could not be checked; the microphone may sound distorted");
    // Output::stop has cleared the warning already when this thread was retired meanwhile.
    if !stop.load(Ordering::Acquire) {
        warnings.set(GAIN_WARNING, format!("Microphone may sound distorted: Steam Streaming Microphone's recording level could not be checked ({error:#}). Set Microphone (Steam Streaming Microphone) to 54 or lower in Windows' Sound settings."));
    }
}
/// After an install that did not bring the device, the next one waits this
/// long: each holds the audio routes while it waits for the device.
const INSTALL_BACKOFF: Duration = Duration::from_secs(120);
/// Whether Steam Streaming Microphone may be installed now.
struct Install {
    installed: bool,
    /// When the last install that has not succeeded started.
    tried: Option<Instant>,
}
impl Install {
    /// Claims an install unless one succeeded, runs, or failed recently.
    fn begin(&mut self, now: Instant) -> bool {
        let due = !self.installed
            && self
                .tried
                .is_none_or(|at| now.saturating_duration_since(at) >= INSTALL_BACKOFF);
        if due {
            self.tried = Some(now);
        }
        due
    }
}
/// Steam Streaming Microphone's playback endpoint, installed from Steam's
/// drivers the first time a client sends its microphone, when Steam is
/// installed and "install_steam_audio_drivers" is on. A failed install is
/// tried again on a later connection.
fn endpoint(config: &Config) -> Result<String> {
    let find = || -> Result<Option<String>> {
        Ok(audio_route::steam_microphone(&audio_route::endpoints()?).map(|e| e.id.clone()))
    };
    if let Some(id) = find()? {
        return Ok(id);
    }
    static INSTALL: Mutex<Install> = Mutex::new(Install {
        installed: false,
        tried: None,
    });
    if INSTALL.lock().unwrap().begin(Instant::now()) {
        let saved = audio_route::install_steam_microphone(config);
        if matches!(saved, Ok(None)) {
            // Nothing was tried (no Steam, or the setting is off): the next
            // connection checks again, as the advice below suggests.
            INSTALL.lock().unwrap().tried = None;
        }
        if let Some(saved) = saved? {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                thread::sleep(Duration::from_millis(250));
                if let Some(id) = find()? {
                    // Windows may make the new device a default when its
                    // endpoints arrive; give it a moment, then put them back.
                    thread::sleep(Duration::from_millis(500));
                    if let Err(error) = saved.restore() {
                        tracing::warn!(error = %format!("{error:#}"), "default audio devices could not be restored after installing Steam Streaming Microphone");
                    }
                    INSTALL.lock().unwrap().installed = true;
                    tracing::info!("Steam Streaming Microphone installed");
                    return Ok(id);
                }
            }
        }
    }
    bail!(
        "Steam Streaming Microphone is not installed. It comes with Steam: install Steam on this PC and keep \"Install Steam streaming audio drivers\" on, then reconnect."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[test]
    #[ignore = "requires the packaged libopus DLL"]
    fn damaged_microphone_packets_keep_the_playout_clock_running() -> Result<()> {
        let root = PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
        );
        let mut encoder = butterpollo_windows::audio::Opus::new(&root, 2, false)?;
        let valid = encoder.encode(&vec![0.25; 960 * 2])?;
        // Code 3 requires a second framing byte. This cannot contain a valid
        // Opus frame, even though its encrypted transport can be intact.
        let damaged = [3];
        for missing in [0, 1, 3] {
            let mut decoder = OpusDecoder::new(&root)?;
            let mut sequencer = Sequencer::default();
            let mut stats = Stats::default();
            let mut pcm = Vec::new();
            decode_packet(
                &mut decoder,
                sequencer.arrive(10),
                &valid,
                &mut pcm,
                &mut stats,
            );
            assert_eq!(pcm.len(), 960);
            pcm.clear();
            decode_packet(
                &mut decoder,
                sequencer.arrive(11 + missing),
                &damaged,
                &mut pcm,
                &mut stats,
            );
            assert_eq!(pcm.len(), (usize::from(missing) + 1) * 960);
            assert_eq!(stats.damaged, 1);
            assert_eq!(stats.concealed, u64::from(missing) + 1);
            pcm.clear();
            decode_packet(
                &mut decoder,
                sequencer.arrive(12 + missing),
                &valid,
                &mut pcm,
                &mut stats,
            );
            assert_eq!(pcm.len(), 960);
            assert!(pcm.iter().all(|sample| sample.is_finite()));
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires the packaged libopus DLL"]
    fn recovered_microphone_burst_does_not_expand_playout_delay() -> Result<()> {
        let root = PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
        );
        let mut encoder = butterpollo_windows::audio::Opus::new(&root, 2, false)?;
        let valid = encoder.encode(&vec![0.25; 960 * 2])?;
        let mut decoder = OpusDecoder::new(&root)?;
        let mut sequencer = Sequencer::default();
        let mut stats = Stats::default();
        let mut pcm = Vec::new();
        decode_packet(
            &mut decoder,
            sequencer.arrive(10),
            &valid,
            &mut pcm,
            &mut stats,
        );
        pcm.clear();
        let frames = decode_packet(
            &mut decoder,
            sequencer.arrive(16),
            &valid,
            &mut pcm,
            &mut stats,
        );
        // Five lost packets plus the current packet produce 120 ms of PCM.
        // Recovery must retain only the newest sound within the normal queue
        // bound; the loss burst does not make the client's packets longer.
        assert_eq!(frames, 960);
        assert_eq!(pcm.len(), 960 * 6);
        let mut playout = Playout::default();
        playout.push_batch(&pcm, frames);
        assert_eq!(playout.len(), 960);
        assert_eq!(playout.trimmed, 960 * 5);
        let mut actual = vec![0.; 960];
        assert_eq!(playout.take(&mut actual), 960);
        assert_eq!(actual, pcm[pcm.len() - 960..]);
        Ok(())
    }

    /// A 437 Hz voice through the real encoder and decoder, 20 ms packets
    /// held back 60 ms twice a second, played by 10 ms device wake-ups with
    /// `conceal` (or silence): what the device played.
    fn played_through_opus(root: &std::path::Path, conceal: bool) -> Result<Vec<f32>> {
        let tone = |n: usize| ((n * 437 % 48_000) as f64 * std::f64::consts::TAU / 48_000.).sin();
        let mut encoder = butterpollo_windows::audio::Opus::new(root, 2, false)?;
        let decoder = Mutex::new(Some(OpusDecoder::new(root)?));
        let mut playout = Playout::default();
        let (mut played, mut out, mut pcm) = (Vec::new(), Vec::new(), Vec::new());
        let (mut padding, mut warned) = (0, None);
        let mut packets = (0..150usize).map(|k| {
            let at = k * 20 + 5;
            // Held back 60 ms at the start of each half second, then sent.
            let at = if at % 500 < 60 {
                at - at % 500 + 60
            } else {
                at
            };
            let frame: Vec<f32> = (k * 960..(k + 1) * 960)
                .flat_map(|n| [tone(n) as f32 * 0.5; 2])
                .collect();
            (at, frame)
        });
        let mut next = packets.next();
        for now in (0..3_000).step_by(10) {
            while let Some((_, frame)) = next.take_if(|(at, _)| *at < now) {
                let packet = encoder.encode(&frame)?;
                // As the receiver: the decoder is held until the audio is queued.
                let mut decoder = decoder.lock().unwrap();
                pcm.clear();
                decoder
                    .as_mut()
                    .unwrap()
                    .decode(Some(&packet), false, &mut pcm)?;
                playout.push_batch(&pcm, pcm.len());
                drop(decoder);
                next = packets.next();
            }
            padding = usize::saturating_sub(padding, 480);
            let silence = playout.pull_concealing(960, padding, 480, &mut out, &mut |out| {
                if conceal {
                    conceal_frame(&decoder, out, &mut warned, Instant::now())
                } else {
                    Ok(0)
                }
            });
            played.extend(&out);
            played.extend(std::iter::repeat_n(0., silence));
            padding += out.len() + silence;
        }
        assert_eq!(playout.conceal_failed, 0);
        Ok(played)
    }

    #[test]
    #[ignore = "requires the packaged libopus DLL"]
    fn opus_concealment_fills_microphone_underruns_without_clicks() -> Result<()> {
        let root = PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
        );
        let step = |played: &[f32]| {
            played[4_800..]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .fold(0., f32::max)
        };
        let silent_runs = |played: &[f32]| {
            played[4_800..]
                .windows(48)
                .filter(|run| run.iter().all(|sample| *sample == 0.))
                .count()
        };
        let silent = played_through_opus(&root, false)?;
        let concealed = played_through_opus(&root, true)?;
        // Silence broke the voice off with hard edges; Opus concealment
        // carries it through and crossfades back into it. The tone itself
        // moves at most 0.029 a sample.
        assert!(silent_runs(&silent) > 0);
        assert!(step(&silent) > 0.1, "{}", step(&silent));
        assert_eq!(silent_runs(&concealed), 0);
        assert!(step(&concealed) < 0.06, "{}", step(&concealed));
        // The same amount played: no added delay.
        assert_eq!(silent.len(), concealed.len());
        Ok(())
    }

    #[test]
    fn concealment_never_waits_for_the_receiver_and_failures_are_logged_rarely() {
        let decoder = Mutex::new(None);
        let (mut out, mut warned) = (Vec::new(), None);
        let now = Instant::now();
        // No decoder yet, or the receiver holding it: nothing, at once.
        assert_eq!(conceal_frame(&decoder, &mut out, &mut warned, now), Ok(0));
        let held = decoder.lock().unwrap();
        assert_eq!(conceal_frame(&decoder, &mut out, &mut warned, now), Ok(0));
        drop(held);
        assert!(out.is_empty() && warned.is_none());
        // A decoder failing every wake-up is logged once in ten seconds.
        assert!(warning_due(&mut warned, now));
        assert!(!warning_due(&mut warned, now + Duration::from_secs(9)));
        assert!(warning_due(&mut warned, now + CONCEAL_WARNING));
    }

    #[test]
    fn the_microphone_level_is_checked_at_once_and_then_every_second() {
        // An app's automatic gain can raise it mid-stream (issue 56).
        let mut cap = GainCap {
            gain: None,
            next: None,
        };
        let start = Instant::now();
        assert!(cap.due(start));
        assert!(!cap.due(start + Duration::from_millis(999)));
        assert!(cap.due(start + GAIN_CHECK));
        assert!(!cap.due(start + GAIN_CHECK + Duration::from_millis(500)));
        assert!(cap.due(start + GAIN_CHECK * 2));
    }
    #[test]
    fn a_failed_microphone_install_is_tried_again_after_a_pause() {
        let start = Instant::now();
        let mut install = Install {
            installed: false,
            tried: None,
        };
        assert!(install.begin(start));
        // Running, or failed a moment ago: other connections do not install.
        assert!(!install.begin(start + Duration::from_secs(5)));
        assert!(install.begin(start + INSTALL_BACKOFF));
        // Nothing was tried (Steam missing): the next connection checks again.
        install.tried = None;
        assert!(install.begin(start + INSTALL_BACKOFF));
        install.installed = true;
        install.tried = None;
        assert!(!install.begin(start + INSTALL_BACKOFF * 2));
    }

    #[test]
    fn stopping_a_render_thread_that_is_still_opening_does_not_wait_for_it() {
        // The thread is still in the install wait and does not see the flag.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let mut output = Output::default();
        output.thread = Some((
            stop.clone(),
            thread::spawn(move || {
                let _ = held.recv();
                Ok(())
            }),
        ));
        let opened = output.opened.clone();
        let started = Instant::now();
        assert!(output.retire().is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(stop.load(Ordering::Acquire));
        assert!(!Arc::ptr_eq(&opened, &output.opened));
        // No second render thread starts while the first one still runs.
        assert!(!output.stopped());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !output.stopped() {
            assert!(
                Instant::now() < deadline,
                "the stopped thread was not collected"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(output.stopping.is_none());
        // A thread that has already ended gives its result straight away.
        output.thread = Some((
            Arc::default(),
            thread::spawn(|| Err(anyhow::anyhow!("device gone"))),
        ));
        while !output.thread.as_ref().unwrap().1.is_finished() {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(output.retire(), Some(Ok(Err(_)))));
    }

    #[test]
    fn a_host_that_cannot_find_its_executable_fails_loudly() {
        let missing = exe_directory(Err(std::io::Error::other("no image path"))).unwrap_err();
        let message = format!("{missing:#}");
        assert!(
            message.contains("could not be found") && message.contains("no image path"),
            "{message}"
        );
        // A bare file name has no folder, which is not the working directory.
        let bare = exe_directory(Ok(PathBuf::from("butterpollo.exe"))).unwrap_err();
        assert!(format!("{bare:#}").contains("has no folder"), "{bare:#}");
        assert_eq!(
            exe_directory(Ok(PathBuf::from(
                r"C:\Program Files\Rubylight\butterpollo.exe"
            )))
            .unwrap(),
            PathBuf::from(r"C:\Program Files\Rubylight")
        );
    }

    #[test]
    fn a_microphone_only_session_plays_its_microphone() {
        use butterpollo_core::session::Role;
        let f = Fixture::new();
        let mut launch = f.launch(f.client(u32::MAX), Role::InputOnly);
        launch
            .options
            .insert(butterpollo_core::mic::ONLY_OPTION.into(), "1".into());
        assert!(launch.mic_only());
        let negotiated = butterpollo_core::rtsp::Negotiated {
            mic: true,
            ..Default::default()
        };
        let session = Session::new(launch, negotiated);
        f.host
            .sessions
            .lock()
            .unwrap()
            .active
            .insert(session.launch.id.clone(), session.clone());
        let counters = Arc::new(Counters::default());
        let mut receiver = Receiver::new(f.host.clone(), counters.clone(), PathBuf::new());
        receiver.native = false;
        let packet = mic::seal(
            &session.launch.key,
            session.launch.key_id,
            7,
            140,
            &[1, 2, 3],
        );
        receiver.receive(&packet, "127.0.0.2".parse().unwrap());
        assert_eq!(counters.refused.load(Ordering::Relaxed), 1);
        assert!(receiver.speaker.is_none());
        receiver.receive(&packet, session.client_ip());
        assert_eq!(counters.accepted.load(Ordering::Relaxed), 1);
        let speaker = receiver
            .speaker
            .as_ref()
            .expect("the session's microphone plays");
        assert!(Arc::ptr_eq(&speaker.session, &session));
        assert_eq!(speaker.stats.packets, 1);
    }

    #[test]
    fn a_second_microphone_is_refused_and_told_while_one_plays() {
        use butterpollo_core::session::Role;
        let f = Fixture::new();
        let negotiated = butterpollo_core::rtsp::Negotiated {
            mic: true,
            ..Default::default()
        };
        let session = |name: &str, uuid: &str, key: u8| {
            let mut client = f.client(u32::MAX);
            client.name = name.into();
            client.uuid = uuid.into();
            let mut launch = f.launch(client, Role::Stream);
            // Each session's own key, as launches get.
            launch.key = [key; 16];
            let session = Session::new(launch, negotiated.clone());
            f.host
                .sessions
                .lock()
                .unwrap()
                .active
                .insert(session.launch.id.clone(), session.clone());
            session
        };
        let phone = session("Phone", "phone", 1);
        let tablet = session("Tablet", "tablet", 2);
        let counters = Arc::new(Counters::default());
        let mut receiver = Receiver::new(f.host.clone(), counters.clone(), PathBuf::new());
        receiver.native = false;
        let packet =
            |s: &Arc<Session>| mic::seal(&s.launch.key, s.launch.key_id, 7, 140, &[1, 2, 3]);
        receiver.receive(&packet(&phone), phone.client_ip());
        receiver.receive(&packet(&tablet), tablet.client_ip());
        assert_eq!(counters.accepted.load(Ordering::Relaxed), 1);
        assert_eq!(counters.refused.load(Ordering::Relaxed), 1);
        assert!(Arc::ptr_eq(
            &receiver.speaker.as_ref().unwrap().session,
            &phone
        ));
        let warnings = tablet.launch.warnings.snapshot();
        assert!(
            warnings
                .iter()
                .any(|w| w.code == MIC_BUSY_WARNING && w.message.contains("Phone")),
            "{warnings:?}"
        );
        assert!(phone.launch.warnings.snapshot().is_empty());
        // `/session` tells the waiting device, and only it.
        assert!(busy_for(&f.host, "tablet") && !busy_for(&f.host, "phone"));
        // The phone goes quiet: the tablet's microphone plays and is no longer busy.
        receiver.speaker.as_mut().unwrap().heard -= HANDOVER;
        receiver.receive(&packet(&tablet), tablet.client_ip());
        assert!(Arc::ptr_eq(
            &receiver.speaker.as_ref().unwrap().session,
            &tablet
        ));
        assert!(!busy_for(&f.host, "tablet"));
    }

    #[test]
    fn idle_microphone_socket_waits_for_the_receive_timeout() -> Result<()> {
        let f = Fixture::new();
        // Inspect the worker's shared socket without competing for its packets.
        f.host.stop.store(true, Ordering::Release);
        let socket = crate::network::udp("127.0.0.1:0".parse()?)?;
        let receiver = socket.try_clone()?;
        // Clones share blocking mode, but keep their own receive timeout on Windows.
        receiver.set_read_timeout(Some(Duration::from_millis(250)))?;
        spawn(f.host.clone(), socket)?;

        let started = Instant::now();
        let error = receiver.recv_from(&mut [0; 1]).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
        assert!(started.elapsed() >= Duration::from_millis(100));
        Ok(())
    }
}
