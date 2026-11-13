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
mod tests;
