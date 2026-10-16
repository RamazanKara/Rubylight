//! Client microphones (`butterpollo_core::mic` has the wire format). One
//! receiver serves every session on the microphone port. The client
//! speaking is decoded as its packets arrive and played into Steam
//! Streaming Microphone by a render thread that runs only while it speaks;
//! a second client is heard once the first has been quiet for a second.
use crate::state::{Session, Shared};
use anyhow::{Result, bail};
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
/// Wait before opening Steam Streaming Microphone again after a failure.
const RETRY: Duration = Duration::from_secs(5);
const WARNING: &str = "microphone";
const ROUTING_WARNING: &str = "microphone_default";

pub fn spawn(h: Shared, socket: UdpSocket) -> Result<Arc<Counters>> {
    // The shared UDP factory is nonblocking, which bypasses the receive timeout.
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    let counters = Arc::new(Counters::default());
    let receiver_counters = counters.clone();
    thread::Builder::new()
        .name("microphone".into())
        .spawn(move || Receiver::new(h, receiver_counters).run(&socket))?;
    Ok(counters)
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
}
struct Receiver {
    h: Shared,
    counters: Arc<Counters>,
    /// Off in the GPU-free reconnect tests, which only count packets.
    native: bool,
    directory: PathBuf,
    speaker: Option<Speaker>,
    decoder: Option<OpusDecoder>,
    playout: Arc<Mutex<Playout>>,
    output: Output,
    pcm: Vec<f32>,
}
impl Receiver {
    fn new(h: Shared, counters: Arc<Counters>) -> Self {
        #[cfg(test)]
        let native = h.reconnect_fixture.is_none();
        #[cfg(not(test))]
        let native = true;
        Self {
            h,
            counters,
            native,
            directory: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Into::into))
                .unwrap_or_default(),
            speaker: None,
            decoder: None,
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
            .filter(|s| s.config.mic && s.launch.peer == peer && !s.stopping())
            .cloned()
            .collect();
        let Some((session, opus)) = candidates.into_iter().find_map(|s| {
            let opus = mic::open(&s.launch.key, s.launch.key_id, &packet).ok()?;
            Some((s, opus))
        }) else {
            self.counters.refused.fetch_add(1, Ordering::Relaxed);
            return;
        };
        self.counters.accepted.fetch_add(1, Ordering::Relaxed);
        if !self
            .speaker
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(&s.session, &session))
        {
            if self
                .speaker
                .as_ref()
                .is_some_and(|s| s.heard.elapsed() < HANDOVER)
            {
                return;
            }
            self.finish();
            tracing::info!(client = %session.launch.client.name, "microphone started");
            self.speaker = Some(Speaker {
                session,
                sequencer: Sequencer::default(),
                heard: Instant::now(),
                started: Instant::now(),
                stats: Stats::default(),
            });
        }
        let speaker = self.speaker.as_mut().unwrap();
        speaker.heard = Instant::now();
        speaker.stats.packets += 1;
        let arrival = speaker.sequencer.arrive(packet.sequence);
        if arrival == Arrival::Late {
            speaker.stats.late += 1;
            return;
        }
        if !self.native {
            return;
        }
        if self.decoder.is_none() {
            match OpusDecoder::new(&self.directory) {
                Ok(decoder) => self.decoder = Some(decoder),
                Err(error) => {
                    speaker.session.launch.warnings.set(
                        WARNING,
                        format!("Microphone unavailable: the Opus decoder could not be loaded ({error:#}). Reinstall Rubylight."),
                    );
                    return;
                }
            }
        }
        let decoder = self.decoder.as_mut().unwrap();
        self.pcm.clear();
        let packet_frames =
            decode_packet(decoder, arrival, &opus, &mut self.pcm, &mut speaker.stats);
        self.playout
            .lock()
            .unwrap()
            .push_batch(&self.pcm, packet_frames);
        let session = speaker.session.clone();
        self.output.ensure(&self.h, &session, &self.playout);
    }
    fn finish(&mut self) {
        let Some(speaker) = self.speaker.take() else {
            return;
        };
        self.output.stop(&speaker.session);
        let trimmed = std::mem::take(&mut self.playout.lock().unwrap().trimmed);
        self.playout.lock().unwrap().clear();
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
    /// Set by the render thread once Steam Streaming Microphone is open.
    opened: Arc<AtomicBool>,
    retry: Option<Instant>,
}
impl Output {
    fn ensure(&mut self, h: &Shared, session: &Arc<Session>, playout: &Arc<Mutex<Playout>>) {
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
        let playout = playout.clone();
        let thread_stop = stop.clone();
        let opened = self.opened.clone();
        match thread::Builder::new()
            .name("microphone render".into())
            .spawn(move || {
                render(
                    &config,
                    &directory,
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
        if let Some((stop, thread)) = self.thread.take() {
            stop.store(true, Ordering::Release);
            Self::report(session, thread.join());
        }
        self.retry = None;
        session.launch.warnings.clear(ROUTING_WARNING);
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

fn render(
    config: &Config,
    directory: &std::path::Path,
    playout: &Mutex<Playout>,
    stop: &AtomicBool,
    opened: &AtomicBool,
    warnings: &butterpollo_core::session::Warnings,
) -> Result<()> {
    let _com = ComGuard::new()?;
    let _priority = Priority::input();
    let mut render = Render::open(&endpoint(config)?)?;
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
    while !stop.load(Ordering::Acquire) {
        render.wait(Duration::from_millis(50));
        render.fill(&mut playout.lock().unwrap())?;
    }
    Ok(())
}
/// Steam Streaming Microphone's playback endpoint, installed from Steam's
/// drivers the first time a client sends its microphone, when Steam is
/// installed and "install_steam_audio_drivers" is on.
fn endpoint(config: &Config) -> Result<String> {
    let find = || -> Result<Option<String>> {
        Ok(audio_route::steam_microphone(&audio_route::endpoints()?).map(|e| e.id.clone()))
    };
    if let Some(id) = find()? {
        return Ok(id);
    }
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if !INSTALLED.swap(true, Ordering::AcqRel)
        && let Some(saved) = audio_route::install_steam_microphone(config)?
    {
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
                tracing::info!("Steam Streaming Microphone installed");
                return Ok(id);
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
