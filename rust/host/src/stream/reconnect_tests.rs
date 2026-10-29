//! GPU-free integration matrix through the real HTTP, RTSP, ENet and session
//! workers. Only native preparation/input and capture/encoder output are fake.
//! No service, driver, environment switch or production port is used.
use super::*;
use butterpollo_core::{
    config::Ports,
    crypto::Identity,
    mic,
    session::Preparation,
    state::{App, Client as PairedClient},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicUsize},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod clipboard_sync_tests;
mod control_fuzz_tests;
mod extension_tests;
mod interop_tests;
mod rate_change_tests;
mod roaming_tests;
mod session_end_tests;
mod webrtc_tests;

const CLOSED: u32 = 0x8003_0023;
const FAILED: u32 = 0x8000_4005;
const WAIT: Duration = Duration::from_secs(5);
/// Only bounds a hung HTTP request: a launch runs prep commands, which a loaded PC starts
/// slowly.
const REQUEST_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct Fixture {
    displays: Arc<AtomicUsize>,
    audios: Arc<AtomicUsize>,
    inputs: Arc<AtomicUsize>,
    /// What a WebRTC session's input worker injects, in place of the desktop.
    pub(crate) webrtc_input: crate::webrtc::input::Recorder,
    session_disconnected: AtomicBool,
    disconnect_during_preparation: AtomicBool,
    session_checks: AtomicUsize,
    /// The app fails to start after the display was prepared, as a bad app
    /// entry or config does.
    fail_app_preparation: AtomicBool,
    apps_prepared: AtomicUsize,
    resources_retained: AtomicUsize,
    pub(super) input_releases: AtomicUsize,
    /// What the control worker did with the clients' text input requests (0x553B), in place
    /// of starting the desktop's focus watcher.
    pub(crate) text_watches: Mutex<Vec<butterpollo_core::text_input::Watch>>,
    video_after_input_release: AtomicUsize,
    hold_teardown: AtomicBool,
    in_teardown: AtomicBool,
    reset: Mutex<Option<String>>,
    reset_done: AtomicBool,
    fail_on_control_connect: AtomicBool,
    hold_rtsp_cleanup: AtomicBool,
    rtsp_cleanup_waiting: AtomicBool,
    resume_rtsp_cleanup: AtomicBool,
    /// A WebRTC worker that fails before it takes its media link.
    webrtc_fails_before_its_link: AtomicBool,
    /// The fixture's display is not in HDR. Its frames stay FP16 either way, as a Desktop
    /// Duplication or WGC pool does for an SDR output: only the colour space says.
    display_in_sdr: AtomicBool,
    /// Windows does not report the fixture display's colour space.
    display_unreadable: AtomicBool,
    /// The WebRTC worker waits before its first frame's source check, as a capture that is slow
    /// to deliver its first frame does.
    hold_first_frame: AtomicBool,
    /// How long releasing a launch's fake audio takes, as a driver that blocks does.
    audio_release_ms: Arc<AtomicU64>,
    /// The control worker does not run between these two times, as on a loaded machine.
    control_held: Mutex<Option<(Instant, Instant)>>,
    /// The next launch's app start waits for `resume_app_preparation`, as a
    /// slow prep command does; `app_preparation_waiting` says it is waiting.
    hold_app_preparation: AtomicBool,
    app_preparation_waiting: AtomicBool,
    resume_app_preparation: AtomicBool,
    /// The RTSP handshake deadline in ms, in place of the host's 10 s (0 keeps it).
    rtsp_deadline_ms: AtomicU64,
    /// DESCRIBE and ANNOUNCE don't find the folder FFmpeg loads from, as when the host's own
    /// path can't be read.
    ffmpeg_directory_unknown: AtomicBool,
    /// DESCRIBE and ANNOUNCE's FFmpeg probe panics.
    ffmpeg_probe_panics: AtomicBool,
}

pub(crate) struct Slot(Arc<AtomicUsize>);
impl Slot {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Display {
    _slot: Slot,
    fixture: Arc<Fixture>,
}
struct Audio {
    _slot: Slot,
    release_ms: Arc<AtomicU64>,
}
impl Drop for Audio {
    fn drop(&mut self) {
        let ms = self.release_ms.load(Ordering::Acquire);
        if ms > 0 {
            thread::sleep(Duration::from_millis(ms));
        }
    }
}
impl butterpollo_core::session::AudioPreparation<butterpollo_windows::audio_route::Route>
    for Audio
{
    fn route(&self) -> Arc<butterpollo_windows::audio_route::Route> {
        panic!("the fake audio must never enter native audio preparation")
    }
}
impl Preparation<crate::display_session::StreamPreparation> for Display {
    fn prepared(&self) -> &crate::display_session::StreamPreparation {
        panic!("the fake display must never enter native display preparation")
    }
    fn into_prepared(self: Box<Self>) -> crate::display_session::StreamPreparation {
        panic!("the fake display must never enter native display preparation")
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        if self.fixture.hold_teardown.load(Ordering::Acquire) {
            self.fixture.in_teardown.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.fixture.hold_teardown.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

impl Fixture {
    pub(crate) fn check_streaming_session(&self) -> Result<()> {
        self.session_checks.fetch_add(1, Ordering::AcqRel);
        anyhow::ensure!(
            !self.session_disconnected.load(Ordering::Acquire),
            "The host Windows session is disconnected or changing. Retry in a moment."
        );
        Ok(())
    }
    pub(crate) fn prepare_app(
        &self,
        h: &Shared,
        app: Option<&App>,
        launch: &Launch,
    ) -> Result<Option<crate::process::RunningApp>> {
        anyhow::ensure!(
            !self.fail_app_preparation.load(Ordering::Acquire),
            "fixture application failed to start"
        );
        if self.hold_app_preparation.swap(false, Ordering::AcqRel) {
            // Launches prepare on a blocking thread, so waiting here holds
            // only this launch, still pending and preparing.
            self.app_preparation_waiting.store(true, Ordering::Release);
            let deadline = Instant::now() + WAIT;
            while !self.resume_app_preparation.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "app preparation was not resumed");
                thread::sleep(Duration::from_millis(1));
            }
            self.app_preparation_waiting.store(false, Ordering::Release);
        }
        *launch.audio_preparation.lock().unwrap() = Some(Box::new(Audio {
            _slot: Slot::new(&self.audios),
            release_ms: self.audio_release_ms.clone(),
        }));
        if launch.role == Role::Stream && h.current_app.lock().unwrap().is_none() {
            self.apps_prepared.fetch_add(1, Ordering::AcqRel);
            return Ok(Some(crate::process::RunningApp::with_environment(
                app.context("fixture application missing")?,
                // As `process::launch` records the launching client, owner of the game.
                [("SUNSHINE_CLIENT_UUID".into(), launch.client.uuid.clone())].into(),
            )?));
        }
        Ok(None)
    }
    pub(crate) fn rtsp_deadline(&self) -> Option<Duration> {
        let ms = self.rtsp_deadline_ms.load(Ordering::Acquire);
        (ms != 0).then(|| Duration::from_millis(ms))
    }
    pub(crate) fn ffmpeg_directory_unknown(&self) -> bool {
        self.ffmpeg_directory_unknown.load(Ordering::Acquire)
    }
    pub(crate) fn ffmpeg_probe_panics(&self) -> bool {
        self.ffmpeg_probe_panics.load(Ordering::Acquire)
    }
    pub(crate) fn retain_resources(&self) {
        self.resources_retained.fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) async fn before_rtsp_cleanup(&self) {
        if !self.hold_rtsp_cleanup.swap(false, Ordering::AcqRel) {
            return;
        }
        self.rtsp_cleanup_waiting.store(true, Ordering::Release);
        let deadline = Instant::now() + WAIT;
        while !self.resume_rtsp_cleanup.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "RTSP cleanup was not resumed");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        self.rtsp_cleanup_waiting.store(false, Ordering::Release);
    }
    pub(super) fn control_connected(&self, h: &Shared, id: &str) {
        if !self.fail_on_control_connect.swap(false, Ordering::AcqRel) {
            return;
        }
        let session = h.sessions.lock().unwrap().active[id].clone();
        session.fail();
        // Model the session worker finishing between the ENet Connect event
        // and the control worker's first sweep. This can happen when capture
        // or encoder startup fails before the first input packet arrives.
        let deadline = Instant::now() + WAIT;
        while h.sessions.lock().unwrap().active.contains_key(id) {
            assert!(
                Instant::now() < deadline,
                "failed session did not tear down"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    pub(crate) fn prepare(self: &Arc<Self>, launch: &Launch) {
        // The normal pending-launch slot and session worker own this lease.
        // Counting it verifies custody and teardown without creating a monitor.
        *launch.preparation.lock().unwrap() = Some(Box::new(Display {
            _slot: Slot::new(&self.displays),
            fixture: self.clone(),
        }));
        if self.disconnect_during_preparation.load(Ordering::Acquire) {
            self.session_disconnected.store(true, Ordering::Release);
        }
    }
    pub(crate) fn input(&self) -> Slot {
        Slot::new(&self.inputs)
    }
    /// Holds the control worker once it is due, before its next pass.
    pub(super) fn hold_control(&self) {
        let mut held = self.control_held.lock().unwrap();
        if let Some((from, until)) = *held
            && Instant::now() >= from
        {
            held.take();
            drop(held);
            thread::sleep(until.saturating_duration_since(Instant::now()));
        }
    }
    pub(super) fn reset_peer(
        &self,
        host: &mut Host<ControlSocket>,
        peers: &HashMap<PeerID, ControlPeer>,
    ) {
        let mut reset = self.reset.lock().unwrap();
        if let Some(id) = reset.as_ref()
            && let Some((peer, _)) = peers.iter().find(|(_, p)| &p.id == id)
        {
            // Reset ENet itself without fabricating a Disconnect event or
            // deleting the host's bookkeeping: cleanup is the worker's job.
            host.peer_mut(*peer).reset();
            reset.take();
            self.reset_done.store(true, Ordering::Release);
        }
    }
    /// The capture's reading of the display's colour space: HDR10, not, or unreadable.
    fn display_in_hdr(&self) -> Option<bool> {
        (!self.display_unreadable.load(Ordering::Acquire))
            .then(|| !self.display_in_sdr.load(Ordering::Acquire))
    }
    /// A WebRTC session's stream worker: the production `Sender` and audio sink, with synthetic
    /// access units and Opus packets. The keyframe flag is honoured as the encoder does, and the
    /// display check is the production one.
    fn stream_webrtc(&self, h: &Shared, s: &Session) -> Result<()> {
        let _audio = s.launch.audio_preparation.lock().unwrap().take();
        anyhow::ensure!(
            !self.webrtc_fails_before_its_link.load(Ordering::Acquire),
            "the fixture's WebRTC worker failed before taking its link"
        );
        while self.hold_first_frame.load(Ordering::Acquire)
            && !h.stop.load(Ordering::Acquire)
            && !s.stopping()
        {
            thread::sleep(Duration::from_millis(5));
        }
        require_hdr_source(s, || self.display_in_hdr(), false)?;
        s.source_checked.store(true, Ordering::Release);
        // As `spawn_audio`: the audio end first, then the video worker takes the link.
        let mut audio = super::audio_sink::WebRtcAudio(
            h.webrtc_links
                .audio_sender(&s.launch.id)
                .context("the WebRTC audio link is missing")?,
        );
        // The layout the production audio thread would open its encoder with.
        let layout = super::audio_layout(s, Some(audio.0.layout()))?;
        let sender = crate::webrtc::session::Sender::new(
            h.webrtc_links
                .take(&s.launch.id)
                .context("the WebRTC link is missing")?,
        );
        *s.encoder.write().unwrap() = "reconnect fixture".into();
        let mut claims = butterpollo_core::performance::Claims::default();
        let codec = s.config.codec;
        let mut first = true;
        let mut position = 0u64;
        let result = (|| -> Result<()> {
            while !h.stop.load(Ordering::Acquire) && !s.stopping() {
                require_hdr_source(s, || self.display_in_hdr(), true)?;
                // Two 5 ms packets per tick; the payload names the layout the encoder would use.
                let payload = webrtc_tests::audio_payload(&layout);
                for _ in 0..2 {
                    super::audio_sink::AudioSink::send(
                        &mut audio,
                        &payload,
                        Instant::now(),
                        position,
                    )?;
                    position += 240;
                }
                let idr = s.idr.swap(false, Ordering::AcqRel) || first;
                first = false;
                let frame = webrtc_tests::access_unit(codec, idr);
                let started = Instant::now();
                sender.submit(
                    s,
                    vec![butterpollo_windows::encoder::Encoded {
                        bytes: frame,
                        unit: butterpollo_windows::encoder::Unit::Whole,
                        idr,
                        after_invalidation: false,
                        frame_cap_bytes: None,
                        reencoded: false,
                        frame_cap_target: None,
                        latency: Some(Duration::from_millis(2)),
                        presentation: Some(started),
                    }],
                    Duration::from_millis(2),
                    started,
                    &mut claims,
                )?;
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        })();
        // The production worker marks the session failed while it still holds its sender, so the
        // client hears an error and not the idle that dropping the link would cause.
        if result.is_err() {
            s.fail();
        }
        drop(sender);
        result
    }
    pub(super) fn stream(&self, media: &Media, h: &Shared, s: &Session) -> Result<()> {
        if s.launch.transport == butterpollo_core::session::Transport::WebRtc {
            return self.stream_webrtc(h, s);
        }
        let _audio = s.launch.audio_preparation.lock().unwrap().take();
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1024,
            fec_percent: 0,
            min_fec: 0,
            key: None,
        };
        *s.encoder.write().unwrap() = "reconnect fixture".into();
        // The video FEC as the video worker starts it; the fixture's own frames
        // keep sending without parity.
        super::configure_video_fec(s, &super::effective_config(h, &s.launch)?)?;
        // As the encoder does once it has the display's metadata: the control
        // worker then sends the HDR mode message (0x010e).
        s.set_display_hdr_metadata(butterpollo_core::hdr::Metadata::default());
        // The video clock starts with the stream, as in the video worker: clock sync
        // probes are answered from here on.
        *s.media_epoch.write().unwrap() = Some(Instant::now());
        while !h.stop.load(Ordering::Acquire) && !s.stopping() {
            let peer = media
                .peers
                .lock()
                .unwrap()
                .get(&(s.launch.id.clone(), false))
                .copied();
            if let Some(peer) = peer {
                let released = self.input_releases.load(Ordering::Acquire);
                // A synthetic encoded access unit still uses the production
                // packetizer and video socket. No decoder or GPU is involved. It opens with
                // an SPS and PPS like an encoder's IDR: Moonlight's depacketizer, and
                // Nightjar's (interop_tests), drop a key frame without them.
                for packet in packetizer.encode_recovery(SYNTHETIC_IDR, true, false, 0, 0)? {
                    media.video.send_to(&packet, peer)?;
                    s.stats.packets.fetch_add(1, Ordering::Relaxed);
                }
                s.stats.frames.fetch_add(1, Ordering::Relaxed);
                // Record progress after the real control worker's loss
                // checkpoint, while the session is still live. The test can
                // inspect it even if its async task wakes after timeout.
                if !s.stopping() {
                    self.video_after_input_release
                        .store(released, Ordering::Release);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

/// The fixture's H.264 key frame: SPS, PPS and an IDR slice, each a stub.
const SYNTHETIC_IDR: &[u8] = &[
    0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x80,
];

async fn until(message: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {message}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Waits for video and the host's first IDR request; false if the session stopped first.
async fn established(session: &Session, client: &Client) -> bool {
    let deadline = Instant::now() + WAIT;
    while client.frames.load(Ordering::Acquire) <= 2
        || session.stats.idr_requests.load(Ordering::Acquire) == 0
    {
        if session.stopping() {
            return false;
        }
        assert!(
            Instant::now() < deadline,
            "{}",
            setup_state("timed out: control and video established", session, client)
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    !session.stopping()
}

fn setup_state(what: &str, session: &Session, client: &Client) -> String {
    format!(
        "{what} (ENet connected: {}, video packets: {}, IDR requests: {}, stopping: {}, reason: {:#x}, warnings: {:?})",
        client.connected.load(Ordering::Acquire),
        client.frames.load(Ordering::Acquire),
        session.stats.idr_requests.load(Ordering::Acquire),
        session.stopping(),
        session.termination_reason(),
        session.launch.warnings.snapshot()
    )
}

/// The host's default ports and the release harness's, which no fixture takes.
fn reserved(port: u16) -> bool {
    (47984..=48010).contains(&port) || (48518..=48544).contains(&port)
}
fn address(port: u16) -> SocketAddr {
    assert!(!reserved(port));
    ([127, 0, 0, 1], port).into()
}
const CLIENT_PORTS: u16 = 64000 - 52000;

/// A client socket. Each test process starts at its own port, as `free_ports` does.
fn udp() -> UdpSocket {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = (std::process::id() % u32::from(CLIENT_PORTS)) as u16;
    loop {
        let index = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(index < CLIENT_PORTS, "fixture UDP port range exhausted");
        if let Ok(socket) = UdpSocket::bind(address(52000 + (start + index) % CLIENT_PORTS)) {
            socket.set_nonblocking(true).unwrap();
            return socket;
        }
    }
}
const PORT_SLOTS: u16 = (40000 - 22000) / 32;

/// A slot's spare port, which no slot's `Ports` use (they span `base - 5` to `base + 21`).
const CLAIM: u16 = 22;

// Each test process starts at its own slot, so two `cargo test` runs at once
// rarely try the same ports.
fn port_base(start: u16, index: u16) -> u16 {
    assert!(index < PORT_SLOTS, "fixture host port range exhausted");
    22000 + (start.wrapping_add(index) % PORT_SLOTS) * 32
}
/// The slot at `base` if it is free, with a socket on its spare port that claims it while it
/// is held. The host binds its ports a while after `reserve` lets go of them (after loading
/// its identity, and the listeners in their own tasks); another run of this binary that
/// checked the same slot meanwhile took them, and one of the two failed with "address in
/// use" (os error 10048). A run skips a slot another holds the claim of.
fn claim_slot(base: u16) -> Option<(Ports, UdpSocket)> {
    let claim = UdpSocket::bind(address(base + CLAIM)).ok()?;
    let ports = Ports::from_base(base);
    reserve(ports).ok()?;
    Some((ports, claim))
}
fn free_ports() -> (Ports, UdpSocket) {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = (std::process::id() % u32::from(PORT_SLOTS)) as u16;
    loop {
        if let Some(slot) = claim_slot(port_base(start, NEXT.fetch_add(1, Ordering::Relaxed))) {
            return slot;
        }
    }
}

#[test]
fn port_bases_start_per_process_and_stay_in_range() {
    assert_eq!(port_base(0, 0), 22000);
    assert_eq!(port_base(7, 0), 22000 + 7 * 32);
    assert_eq!(port_base(PORT_SLOTS - 1, 1), 22000);
    let bases: std::collections::HashSet<_> = (0..PORT_SLOTS).map(|i| port_base(300, i)).collect();
    assert_eq!(bases.len(), usize::from(PORT_SLOTS));
    assert!(bases.iter().all(|b| (22000..40000 - 31).contains(b)));
}

/// While one harness holds a slot's claim, another run checking that slot skips it, even
/// before the host has bound any of the slot's ports.
#[test]
fn a_claimed_slot_is_skipped_until_its_claim_is_dropped() {
    let (ports, claim) = (0..PORT_SLOTS)
        .find_map(|i| claim_slot(port_base(150, i)))
        .expect("a free slot");
    let used = |p: Ports| {
        [
            p.http, p.https, p.web, p.rtsp, p.video, p.control, p.audio, p.mic,
        ]
    };
    let base = ports.http;
    assert!(!used(ports).contains(&(base + CLAIM)));
    assert!(!used(Ports::from_base(base + 32)).contains(&(base + CLAIM)));
    assert!(
        reserve(ports).is_ok(),
        "the claim holds none of the host's ports"
    );
    assert!(claim_slot(base).is_none());
    drop(claim);
    assert!(claim_slot(base).is_some());
}
fn reserve(ports: Ports) -> Result<(Vec<std::net::TcpListener>, Vec<UdpSocket>)> {
    let tcp = [ports.http, ports.https, ports.web, ports.rtsp]
        .into_iter()
        .map(|p| std::net::TcpListener::bind(address(p)))
        .collect::<std::io::Result<_>>()?;
    let udp = [ports.video, ports.control, ports.audio, ports.mic]
        .into_iter()
        .map(|p| UdpSocket::bind(address(p)))
        .collect::<std::io::Result<_>>()?;
    Ok((tcp, udp))
}

struct Harness {
    h: Shared,
    fixture: Arc<Fixture>,
    media: Option<Arc<Media>>,
    listeners: Vec<tokio::task::JoinHandle<Result<()>>>,
    http: reqwest::Client,
    clients: Vec<reqwest::Client>,
    ports: Ports,
    /// Keeps other runs of this binary off `ports` (`claim_slot`).
    _claim: UdpSocket,
    /// A `ping_timeout` a loaded PC's stream setup can outlast (`play_short_timeout`).
    short_timeout: bool,
    directory: PathBuf,
    started: Instant,
}
impl Harness {
    async fn new(timeout_ms: u32, clients: usize) -> Result<Self> {
        Self::configured(timeout_ms, clients, "").await
    }
    /// A harness whose configuration also has the lines `extra`.
    async fn configured(timeout_ms: u32, clients: usize, extra: &str) -> Result<Self> {
        let started = Instant::now();
        // A failed test prints what its host logged (the test writer is captured per test).
        crate::test_log::install();
        let directory =
            std::env::temp_dir().join(format!("butterpollo-reconnect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let (ports, claim) = free_ports();
        std::fs::write(
            directory.join("sunshine.conf"),
            format!(
                "port={}\nbind_address=127.0.0.1\nping_timeout={timeout_ms}\nstream_audio=false\nenable_discovery=false\nupnp=false\n{extra}",
                ports.http
            ),
        )?;
        let mut h = crate::state::Host::load(directory.clone(), directory.clone(), None)?;
        let fixture = Arc::new(Fixture::default());
        Arc::get_mut(&mut h).unwrap().reconnect_fixture = Some(fixture.clone());
        h.codecs.store(1 | 0x200, Ordering::Release);
        h.startup_codecs.send_replace(true);
        let app = App::desktop();
        *h.apps.write().unwrap() = vec![app.clone()];
        *h.current_app.lock().unwrap() = Some(crate::process::RunningApp::with_environment(
            &app,
            Default::default(),
        )?);
        let mut http_clients = vec![];
        for index in 0..clients {
            let extra_identity;
            let identity = if index == 0 {
                &h.identity
            } else {
                extra_identity = Identity::generate()?;
                &extra_identity
            };
            h.paired.write().unwrap().clients.push(PairedClient {
                name: format!("reconnect-{index}"),
                uuid: index.to_string(),
                cert: identity.certificate.clone(),
                perm: u32::MAX,
                enabled: true,
                extra: Default::default(),
            });
            let pem = format!("{}{}", identity.certificate, identity.private_pem);
            http_clients.push(
                reqwest::Client::builder()
                    .no_proxy()
                    .danger_accept_invalid_certs(true)
                    .identity(reqwest::Identity::from_pem(pem.as_bytes())?)
                    .pool_max_idle_per_host(0)
                    .timeout(REQUEST_WAIT)
                    .build()?,
            );
        }
        let media = Media::new(h.clone(), address(ports.video).ip())?;
        let acceptor = crate::tls::acceptor(&h.identity, true)?;
        let listeners = vec![
            tokio::spawn(crate::tls::serve(
                address(ports.http),
                crate::nvhttp::router(h.clone(), false),
                None,
            )),
            tokio::spawn(crate::tls::serve(
                address(ports.https),
                crate::nvhttp::router(h.clone(), true),
                Some(acceptor),
            )),
            tokio::spawn(crate::rtsp_server::serve(
                address(ports.rtsp),
                h.clone(),
                media.clone(),
            )),
        ];
        let harness = Self {
            h,
            fixture,
            media: Some(media),
            listeners,
            clients: http_clients,
            http: reqwest::Client::builder()
                .no_proxy()
                .pool_max_idle_per_host(0)
                .timeout(REQUEST_WAIT)
                .build()?,
            ports,
            _claim: claim,
            short_timeout: timeout_ms <= 2000,
            directory,
            started,
        };
        for port in [ports.http, ports.https, ports.rtsp] {
            let deadline = Instant::now() + WAIT;
            loop {
                if tokio::net::TcpStream::connect(address(port)).await.is_ok() {
                    break;
                }
                assert!(Instant::now() < deadline, "listener {port} did not start");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        harness.counts(0, 0).await?;
        Ok(harness)
    }
    async fn launch_response(&self, client: usize, resume: bool) -> Result<String> {
        self.launch_response_hdr(client, resume, None).await
    }
    /// A launch or resume with the given `hdrMode` query (absent for `None`).
    async fn launch_response_hdr(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
    ) -> Result<String> {
        self.launch_response_with(client, resume, hdr_mode, &[])
            .await
    }
    /// A launch or resume with the given `hdrMode` and the `extra` query parameters.
    async fn launch_response_with(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
        extra: &[(&'static str, &str)],
    ) -> Result<String> {
        let endpoint = if resume { "resume" } else { "launch" };
        let key = hex::encode(crypto::random::<16>());
        let app = self.h.apps.read().unwrap()[0].id();
        let mut query = vec![
            ("appid", app.to_string()),
            ("rikey", key),
            ("rikeyid", "1".into()),
            ("corever", "1".into()),
            ("mode", "640x480x60".into()),
        ];
        query.extend(hdr_mode.map(|mode| ("hdrMode", mode.to_string())));
        query.extend(extra.iter().map(|(name, value)| (*name, value.to_string())));
        Ok(self.clients[client]
            .get(format!("https://127.0.0.1:{}/{endpoint}", self.ports.https))
            .query(&query)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }
    async fn launch(&self, client: usize, resume: bool) -> Result<Launch> {
        self.launch_hdr(client, resume, None).await
    }
    async fn launch_hdr(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
    ) -> Result<Launch> {
        self.launch_with(client, resume, hdr_mode, &[]).await
    }
    async fn launch_with(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
        extra: &[(&'static str, &str)],
    ) -> Result<Launch> {
        let response = self
            .launch_response_with(client, resume, hdr_mode, extra)
            .await?;
        assert!(
            response.contains("status_code=\"200\""),
            "resume={resume}: {response}"
        );
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "1"
        );
        let launch = self
            .h
            .sessions
            .lock()
            .unwrap()
            .pending
            .values()
            .find(|l| l.client.uuid == client.to_string())
            .cloned()
            .expect("launch must be pending");
        Ok(launch)
    }
    async fn rtsp(
        &self,
        launch: &Launch,
        sequence: u32,
        method: &str,
        body: &str,
    ) -> Result<String> {
        self.rtsp_at(launch, sequence, method, "rtsp://127.0.0.1/stream", body)
            .await
    }
    async fn rtsp_at(
        &self,
        launch: &Launch,
        sequence: u32,
        method: &str,
        target: &str,
        body: &str,
    ) -> Result<String> {
        let wire = sealed_rtsp(launch, sequence, method, target, body)?;
        let response = tokio::time::timeout(WAIT, async {
            let mut socket = tokio::net::TcpStream::connect(address(self.ports.rtsp)).await?;
            socket.write_all(&wire).await?;
            let mut response = vec![];
            // A host that closes without answering may reset the connection: Windows reports
            // that as aborted rather than as the end of the stream.
            match socket.read_to_end(&mut response).await {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                    ) => {}
                result => {
                    result?;
                }
            }
            Ok::<_, anyhow::Error>(response)
        })
        .await??;
        opened_rtsp(launch, method, &response)
    }
    async fn play(&self, launch: &Launch) -> Result<(Arc<Session>, Client)> {
        self.play_encrypted(launch, 0).await
    }
    async fn play_encrypted(
        &self,
        launch: &Launch,
        encryption: u32,
    ) -> Result<(Arc<Session>, Client)> {
        self.play_announcing(launch, encryption, "").await
    }
    /// Plays after an ANNOUNCE that carries the extra SDP lines `sdp`.
    async fn play_announcing(
        &self,
        launch: &Launch,
        encryption: u32,
        sdp: &str,
    ) -> Result<(Arc<Session>, Client)> {
        let (session, client) = self.connect_announcing(launch, encryption, sdp).await?;
        assert!(
            established(&session, &client).await,
            "{}",
            setup_state(
                "stopped before control and video established",
                &session,
                &client
            )
        );
        Ok((session, client))
    }
    /// `play` for a host whose `ping_timeout` (1 or 2 s) is shorter than a loaded PC can take
    /// from PLAY to the client's first ENet packet: a session that ended before ENet connected
    /// is resumed and played again, as Moonlight would reconnect. One that ended after it
    /// connected fails the test. The tests that use it check what happens once the stream runs.
    async fn play_short_timeout(&self, launch: &Launch) -> Result<(Arc<Session>, Client)> {
        assert!(self.short_timeout, "a long ping_timeout outlasts any setup");
        let mut launch = launch.clone();
        for attempt in 1.. {
            let (session, client) = self.connect_announcing(&launch, 0, "").await?;
            if established(&session, &client).await {
                return Ok((session, client));
            }
            let state = setup_state("stopped during setup", &session, &client);
            assert!(attempt < 3, "{state}");
            assert_eq!(session.termination_reason(), FAILED, "{state}");
            assert!(!client.connected.load(Ordering::Acquire), "{state}");
            eprintln!("attempt {attempt}: {state}; resuming");
            drop(client);
            self.empty().await?;
            launch = self.launch(0, true).await?;
        }
        unreachable!()
    }
    /// ANNOUNCE with the extra SDP lines `sdp`, PLAY and the control connection,
    /// without waiting for video or the first IDR request.
    async fn connect_announcing(
        &self,
        launch: &Launch,
        encryption: u32,
        sdp: &str,
    ) -> Result<(Arc<Session>, Client)> {
        self.rtsp(launch, 1, "ANNOUNCE", &format!("a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\na=x-ss-general.encryptionEnabled:{encryption}\r\n{sdp}")).await?;
        self.rtsp(launch, 2, "PLAY", "").await?;
        let session = self.h.sessions.lock().unwrap().active[&launch.id].clone();
        let client = Client::new(self.ports, launch.clone())?;
        Ok((session, client))
    }
    async fn counts(&self, active: usize, pending: usize) -> Result<()> {
        let response = self
            .http
            .get(format!("http://127.0.0.1:{}/serverinfo", self.ports.http))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        assert_eq!(
            xml(&response, "RustHostSessionCount"),
            active.to_string(),
            "{response}"
        );
        assert_eq!(
            xml(&response, "RustHostPendingSessionCount"),
            pending.to_string(),
            "{response}"
        );
        Ok(())
    }
    async fn empty(&self) -> Result<()> {
        until(
            "sessions, display leases and input slots released",
            WAIT,
            || {
                self.h.sessions.lock().unwrap().idle()
                    && self.fixture.displays.load(Ordering::Acquire) == 0
                    && self.fixture.audios.load(Ordering::Acquire) == 0
                    && self.fixture.inputs.load(Ordering::Acquire) == 0
            },
        )
        .await;
        self.counts(0, 0).await?;
        assert!(self.h.sessions.lock().unwrap().teardown.is_empty());
        assert!(self.h.app_display.lock().unwrap().is_empty());
        assert!(self.h.monitors.lock().unwrap().is_empty());
        assert!(
            self.media
                .as_ref()
                .unwrap()
                .peers
                .lock()
                .unwrap()
                .is_empty()
        );
        Ok(())
    }
    async fn end(&self, session: &Session, client: Client) -> Result<()> {
        self.rtsp(&session.launch, 3, "TEARDOWN", "").await?;
        // After an outage the host's ENet resends on a timeout that doubled while the client
        // was away, seconds after the network returns.
        until("normal termination delivered", WAIT * 3, || {
            client.reason.load(Ordering::Acquire) != 0
        })
        .await;
        assert_eq!(session.termination_reason(), CLOSED);
        assert_eq!(client.reason.load(Ordering::Acquire), CLOSED);
        drop(client);
        self.empty().await
    }
    async fn finish(mut self) -> Result<()> {
        self.empty().await?;
        // Reuse the same listeners/UDP ports for a fresh launch before checking
        // that stopping this isolated host releases the sockets themselves.
        let launch = self.launch(0, false).await?;
        let (session, client) = if self.short_timeout {
            self.play_short_timeout(&launch).await?
        } else {
            self.play(&launch).await?
        };
        self.counts(1, 0).await?;
        self.end(&session, client).await?;
        self.h.stop.store(true, Ordering::Release);
        for listener in self.listeners.drain(..) {
            listener.abort();
            assert!(listener.await.unwrap_err().is_cancelled());
        }
        let weak = Arc::downgrade(self.media.as_ref().unwrap());
        self.media.take();
        // The microphone's blocking receive can outlive Media until its timeout.
        until("media workers and sockets stopped", WAIT, || {
            weak.upgrade().is_none() && reserve(self.ports).is_ok()
        })
        .await;
        eprintln!(
            "reconnect matrix case completed in {:.3}s",
            self.started.elapsed().as_secs_f64()
        );
        Ok(())
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.fixture.hold_teardown.store(false, Ordering::Release);
        self.h.stop.store(true, Ordering::Release);
        let canceled = self.h.sessions.lock().unwrap().request_stop(None);
        drop(canceled);
        for listener in &self.listeners {
            listener.abort();
        }
        let weak = self.media.as_ref().map(Arc::downgrade);
        self.media.take();
        let deadline = Instant::now() + WAIT;
        while weak.as_ref().is_some_and(|w| w.upgrade().is_some()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
/// An encrypted RTSP request from `launch`'s client, as Moonlight sends it.
fn sealed_rtsp(
    launch: &Launch,
    sequence: u32,
    method: &str,
    target: &str,
    body: &str,
) -> Result<Vec<u8>> {
    let request = format!(
        "{method} {target} RTSP/1.0\r\nCSeq: {sequence}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut iv = [0; 12];
    iv[..4].copy_from_slice(&sequence.to_le_bytes());
    iv[10..].copy_from_slice(b"CR");
    let (tag, ciphertext) = crypto::gcm_seal(&launch.key, &iv, request.as_bytes())?;
    let mut wire = ((ciphertext.len() as u32) | 0x80000000)
        .to_be_bytes()
        .to_vec();
    wire.extend_from_slice(&sequence.to_be_bytes());
    wire.extend_from_slice(&tag);
    wire.extend_from_slice(&ciphertext);
    Ok(wire)
}
/// The host's decrypted answer to `method`; an error unless it is a 200, or
/// when the host closed the connection without one.
fn opened_rtsp(launch: &Launch, method: &str, response: &[u8]) -> Result<String> {
    anyhow::ensure!(
        response.len() >= 24,
        "RTSP {method} closed without a response"
    );
    let length = u32::from_be_bytes(response[..4].try_into()?) & 0x7fffffff;
    assert_eq!(length as usize, response.len() - 24);
    let mut iv = [0; 12];
    iv[..4].copy_from_slice(&u32::from_be_bytes(response[4..8].try_into()?).to_le_bytes());
    iv[10..].copy_from_slice(b"HR");
    let response = String::from_utf8(crypto::gcm_open(
        &launch.key,
        &iv,
        &response[8..24],
        &response[24..],
    )?)?;
    anyhow::ensure!(response.starts_with("RTSP/1.0 200"), "{response}");
    Ok(response)
}
fn xml<'a>(text: &'a str, tag: &str) -> &'a str {
    text.split_once(&format!("<{tag}>"))
        .unwrap()
        .1
        .split_once(&format!("</{tag}>"))
        .unwrap()
        .0
}

struct LossySocket {
    socket: UdpSocket,
    blocked: Arc<AtomicBool>,
    /// A socket that replaces `socket` from the next send or receive (`Client::move_to`).
    moved: Arc<Mutex<Option<UdpSocket>>>,
}
impl LossySocket {
    fn take_move(&mut self) {
        if let Some(socket) = self.moved.lock().unwrap().take() {
            self.socket = socket;
        }
    }
}
impl rusty_enet::Socket for LossySocket {
    type Address = SocketAddr;
    type Error = std::io::Error;
    fn init(&mut self, options: rusty_enet::SocketOptions) -> std::io::Result<()> {
        rusty_enet::Socket::init(&mut self.socket, options)
    }
    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> std::io::Result<usize> {
        self.take_move();
        if self.blocked.load(Ordering::Acquire) {
            return Ok(buffer.len());
        }
        rusty_enet::Socket::send(&mut self.socket, address, buffer)
    }
    fn receive(
        &mut self,
        buffer: &mut [u8; rusty_enet::MTU_MAX],
    ) -> std::io::Result<Option<(SocketAddr, rusty_enet::PacketReceived)>> {
        // As the host's control socket: a datagram the client sent before the host bound its
        // port comes back as an ICMP port unreachable, and Windows reports that as
        // WSAECONNRESET on this receive. rusty_enet would end the client's worker on it.
        self.take_move();
        let packet = match rusty_enet::Socket::receive(&mut self.socket, buffer) {
            Err(error) if butterpollo_windows::net::datagram_lost(&error) => None,
            result => result?,
        };
        if self.blocked.load(Ordering::Acquire) {
            Ok(None)
        } else {
            Ok(packet)
        }
    }
}
/// A client can send its ENet connect before the host binds the control port. The port
/// unreachable that comes back loses that datagram; the client's worker goes on and connects on
/// ENet's retry.
#[test]
fn the_client_socket_outlives_a_datagram_to_a_closed_port() -> Result<()> {
    let closed = udp().local_addr()?;
    let mut socket = LossySocket {
        socket: udp(),
        blocked: Arc::default(),
        moved: Arc::default(),
    };
    rusty_enet::Socket::send(&mut socket, closed, b"connect")?;
    let mut buffer = [0; rusty_enet::MTU_MAX];
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        assert!(rusty_enet::Socket::receive(&mut socket, &mut buffer)?.is_none());
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
/// Link notice payloads with the time each arrived.
type Notices = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;

struct Client {
    control_down: Arc<AtomicBool>,
    video_down: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicUsize>,
    /// Rounds the worker finished: one that began before a switch was flipped ends by the next.
    rounds: Arc<AtomicUsize>,
    reason: Arc<AtomicU32>,
    /// Link notices (0x5538) the host sent, with their arrival times.
    notices: Notices,
    /// HDR mode messages (0x010e) the host sent.
    hdr_modes: Notices,
    /// Text input notices (0x553B) the host sent.
    text_notices: Notices,
    /// Rate change answers (0x553C) the host sent.
    rate_answers: Notices,
    /// Clipboard sync frames (0x5508) the host sent.
    clipboard: Notices,
    /// Plain control messages for the worker to send, reliable.
    outbox: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Sockets ENet and the video pings move to (`move_to`).
    control_move: Arc<Mutex<Option<UdpSocket>>>,
    video_move: Arc<Mutex<Option<UdpSocket>>>,
    /// Whether the control heartbeat is an IDR request (0x0302) rather than a plain
    /// ping (0x0200), which leaves `idr_requests` to the host's own keyframes.
    keyframe_heartbeat: Arc<AtomicBool>,
    /// The address ENet first sent from.
    control_address: SocketAddr,
    /// Whether ENet connected.
    connected: Arc<AtomicBool>,
    /// Whether the host disconnected ENet; the worker acknowledges it.
    disconnected: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}
impl Client {
    fn new(ports: Ports, launch: Launch) -> Result<Self> {
        Self::sending_from(ports, launch, udp())
    }
    /// A client whose ENet sends from `control`.
    fn sending_from(ports: Ports, launch: Launch, control: UdpSocket) -> Result<Self> {
        let control_down = Arc::new(AtomicBool::new(false));
        let video_down = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicUsize::new(0));
        let rounds = Arc::new(AtomicUsize::new(0));
        let reason = Arc::new(AtomicU32::new(0));
        let notices = Arc::new(Mutex::new(Vec::new()));
        let hdr_modes = Notices::default();
        let text_notices = Notices::default();
        let rate_answers = Notices::default();
        let clipboard = Notices::default();
        let outbox = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let control_move = Arc::<Mutex<Option<UdpSocket>>>::default();
        let video_move = Arc::<Mutex<Option<UdpSocket>>>::default();
        let keyframe_heartbeat = Arc::new(AtomicBool::new(true));
        let control_address = control.local_addr()?;
        let connected = Arc::new(AtomicBool::new(false));
        let worker_connected = connected.clone();
        let disconnected = Arc::new(AtomicBool::new(false));
        let worker_disconnected = disconnected.clone();
        let mut host = Host::new(
            LossySocket {
                socket: control,
                blocked: control_down.clone(),
                moved: control_move.clone(),
            },
            HostSettings {
                peer_limit: 1,
                channel_limit: 48,
                ..Default::default()
            },
        )?;
        let peer = host.connect(address(ports.control), 48, launch.connect_data)?;
        peer.set_timeout(32, 60_000, 60_000);
        let peer_id = peer.id();
        let mut video = udp();
        let worker_video_move = video_move.clone();
        let (worker_stop, worker_video, worker_frames, worker_reason) = (
            stop.clone(),
            video_down.clone(),
            frames.clone(),
            reason.clone(),
        );
        let (worker_notices, worker_outbox) = (notices.clone(), outbox.clone());
        let worker_hdr_modes = hdr_modes.clone();
        let worker_text_notices = text_notices.clone();
        let worker_rate_answers = rate_answers.clone();
        let worker_clipboard = clipboard.clone();
        let worker_rounds = rounds.clone();
        let worker_keyframe_heartbeat = keyframe_heartbeat.clone();
        let worker = thread::spawn(move || -> Result<()> {
            let mut next_ping = Instant::now();
            let mut buffer = [0; 2048];
            while !worker_stop.load(Ordering::Acquire) {
                while let Some(event) = host.service()? {
                    if let Event::Connect { .. } = event {
                        worker_connected.store(true, Ordering::Release);
                    }
                    if let Event::Disconnect { .. } = event {
                        worker_disconnected.store(true, Ordering::Release);
                    }
                    if let Event::Receive { packet, .. } = event {
                        let (_, kind, payload) = butterpollo_core::packet::decrypt_control(
                            &launch.key,
                            packet.data(),
                            false,
                        )?;
                        if kind == 0x0109 {
                            worker_reason.store(
                                u32::from_be_bytes(payload[..4].try_into()?),
                                Ordering::Release,
                            );
                        }
                        if kind == 0x010e {
                            worker_hdr_modes
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if kind == butterpollo_core::text_input::TEXT_INPUT_MESSAGE_TYPE {
                            worker_text_notices
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if kind == butterpollo_core::rate_change::RATE_CHANGE_MESSAGE_TYPE {
                            worker_rate_answers
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if kind == butterpollo_core::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE {
                            worker_clipboard
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if kind == butterpollo_core::link_notice::LINK_NOTICE_MESSAGE_TYPE {
                            worker_notices
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload));
                        }
                    }
                }
                for message in std::mem::take(&mut *worker_outbox.lock().unwrap()) {
                    host.peer_mut(peer_id)
                        .send(0, &Packet::new(message, PacketKind::Reliable))?;
                }
                if Instant::now() >= next_ping {
                    if host.peer(peer_id).connected() {
                        // An IDR request is also a valid control heartbeat, and
                        // its counter proves the host parsed it after recovery.
                        let heartbeat: &[u8] = if worker_keyframe_heartbeat.load(Ordering::Acquire)
                        {
                            &[2, 3]
                        } else {
                            &[0, 2]
                        };
                        host.peer_mut(peer_id)
                            .send(0, &Packet::new(heartbeat, PacketKind::Reliable))?;
                        // And an unreliable copy: a reliable one queued in an
                        // outage is resent on rusty_enet's doubling timeout, up
                        // to about the outage's length after the network returns
                        // (a 10 s drop failed once on CI). Moonlight's ENet backs
                        // off linearly and is heard again within half a second.
                        host.peer_mut(peer_id).send(
                            0,
                            &Packet::new(
                                heartbeat,
                                PacketKind::AlwaysUnreliable { sequenced: false },
                            ),
                        )?;
                    }
                    if let Some(socket) = worker_video_move.lock().unwrap().take() {
                        video = socket;
                    }
                    if !worker_video.load(Ordering::Acquire) {
                        video.send_to(launch.ping.as_bytes(), address(ports.video))?;
                    }
                    next_ping = Instant::now() + Duration::from_millis(50);
                }
                while let Ok((n, source)) = video.recv_from(&mut buffer) {
                    // Another run of this binary may still stream to a port it freed and this
                    // client took: only the host's own video counts.
                    if source != address(ports.video) {
                        continue;
                    }
                    assert!(n > 16, "video packet has no RTP payload");
                    if !worker_video.load(Ordering::Acquire) {
                        worker_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
                host.flush();
                worker_rounds.fetch_add(1, Ordering::AcqRel);
                thread::sleep(Duration::from_millis(1));
            }
            // Dropping ENet sends no Disconnect, just like a vanished client.
            Ok(())
        });
        Ok(Self {
            control_down,
            video_down,
            stop,
            frames,
            rounds,
            reason,
            notices,
            hdr_modes,
            text_notices,
            rate_answers,
            clipboard,
            outbox,
            control_move,
            video_move,
            keyframe_heartbeat,
            control_address,
            connected,
            disconnected,
            worker: Some(worker),
        })
    }
    /// Sends plain pings as the control heartbeat from now on, and returns once no IDR
    /// request the heartbeat sent can still reach the host.
    async fn quiet_heartbeat(&self) {
        self.keyframe_heartbeat.store(false, Ordering::Release);
        let round = self.rounds.load(Ordering::Acquire);
        until("the client's last IDR heartbeat sent", WAIT, || {
            self.rounds.load(Ordering::Acquire) > round
        })
        .await;
        // The reliable copy is acknowledged within a loopback round trip.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    /// Sends a plain control message of `kind` (the host takes unencrypted ones in
    /// these streams).
    fn send_control(&self, kind: u16, payload: &[u8]) {
        let mut message = kind.to_le_bytes().to_vec();
        message.extend_from_slice(payload);
        self.outbox.lock().unwrap().push(message);
    }
    /// Sends a control message of `kind` encrypted with the launch's `key`, as these
    /// streams' version 1 control encryption has it; `seq` counts up from 0 per stream.
    fn send_encrypted(&self, key: &[u8; 16], seq: u32, kind: u16, payload: &[u8]) {
        let message =
            butterpollo_core::packet::encrypted_control(key, seq, kind, payload, false).unwrap();
        self.outbox.lock().unwrap().push(message);
    }
    /// Stops the video pings and returns once no round that could still send one is running.
    async fn stop_video_pings(&self) {
        self.video_down.store(true, Ordering::Release);
        let round = self.rounds.load(Ordering::Acquire);
        until("the client's last video ping sent", WAIT, || {
            self.rounds.load(Ordering::Acquire) > round
        })
        .await;
    }
    fn network(&self, down: bool) {
        self.control_down.store(down, Ordering::Release);
        self.video_down.store(down, Ordering::Release);
    }
    /// Moves the client to other sockets, as when its address changes: ENet and the
    /// video pings use `control` and `video` from the worker's next round.
    fn move_to(&self, control: UdpSocket, video: UdpSocket) {
        *self.control_move.lock().unwrap() = Some(control);
        *self.video_move.lock().unwrap() = Some(video);
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap().unwrap();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_change_during_launch_or_resume_releases_unaccepted_preparation() -> Result<()> {
    for resume in [false, true] {
        let h = Harness::new(10_000, 1).await?;
        if !resume {
            let previous = h.h.current_app.lock().unwrap().take();
            drop(previous);
        }
        let undo_marker = h.directory.join("rejected-launch-prep.txt");
        if !resume {
            // with_environment runs the real prep/undo lifecycle. These are
            // hidden cmd.exe built-ins writing only inside this test's TMP
            // directory; no app window, input or user process is involved.
            h.h.apps.write().unwrap()[0]
                .prep
                .push(butterpollo_core::state::PrepCommand {
                    r#do: format!("echo prepared>\"{}\"", undo_marker.display()),
                    undo: format!("echo undone>>\"{}\"", undo_marker.display()),
                    // CI administrators may have no linked non-elevated
                    // token. Use the existing runner context, as the process
                    // lifecycle fixtures do; never request UAC elevation.
                    elevated: butterpollo_windows::process::user_environment().is_err(),
                    extra: Default::default(),
                });
        }
        let previous_generation =
            h.h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .map(|app| app.generation.clone());
        h.fixture
            .disconnect_during_preparation
            .store(true, Ordering::Release);
        let response = h.launch_response(0, resume).await?;
        assert!(response.contains("status_code=\"503\""), "{response}");
        assert!(response.contains("disconnected or changing"), "{response}");
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "0"
        );
        assert!(!response.contains("sessionUrl0"));
        assert_eq!(h.fixture.session_checks.load(Ordering::Acquire), 2);
        assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
        assert_eq!(
            h.fixture.apps_prepared.load(Ordering::Acquire),
            usize::from(!resume)
        );
        assert_eq!(
            h.h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .map(|app| app.generation.clone()),
            previous_generation
        );
        assert!(h.h.app_audio.lock().unwrap().is_none());
        if !resume {
            assert_eq!(
                std::fs::read_to_string(&undo_marker)?
                    .lines()
                    .collect::<Vec<_>>(),
                ["prepared", "undone"],
                "rejecting a newly prepared app must execute its real RAII undo"
            );
        }
        h.empty().await?;

        h.fixture
            .disconnect_during_preparation
            .store(false, Ordering::Release);
        h.fixture
            .session_disconnected
            .store(false, Ordering::Release);
        let retry = h.launch(0, resume).await?;
        let (session, client) = h.play(&retry).await?;
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_change_before_play_rejects_only_the_pending_attempt() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let live_launch = h.launch(0, false).await?;
    let (session, client) = h.play(&live_launch).await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let pending = h.launch(1, false).await?;
    h.rtsp(
        &pending,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.fixture
        .session_disconnected
        .store(true, Ordering::Release);
    let error = h.rtsp(&pending, 2, "PLAY", "").await.unwrap_err();
    assert!(
        error.to_string().contains("disconnected or changing"),
        "{error:#}"
    );
    // This test keeps an extra launch identity clone; real RTSP snapshots
    // deliberately do not own its preparation. Release the test's clone too.
    drop(pending);
    h.counts(1, 0).await?;
    assert!(!session.stopping());
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    assert_eq!(h.fixture.audios.load(Ordering::Acquire), 1);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    // HTTP already accepted this app; a later PLAY rejection must preserve
    // its normal resumable lifecycle and must not stop another client's stream.
    h.fixture
        .session_disconnected
        .store(false, Ordering::Release);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_rtsp_socket_releases_preparation_before_replacement() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    drop(launch);
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let abandoned = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until("abandoned RTSP connection authorized", WAIT, || {
        h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;

    let replacement = h.launch(0, true).await?;
    assert_eq!(
        h.fixture.displays.load(Ordering::Acquire),
        1,
        "canceled preparation still owned while the replacement prepares its display"
    );
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    let (session, client) = h.play(&replacement).await?;
    drop(abandoned);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_rtsp_cleanup_preserves_a_new_launch_announcement() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let older_launch = h.launch(0, false).await?;
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let older_connection = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until("older RTSP connection reached cleanup", WAIT, || {
        h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;

    let launch = h.launch(1, false).await?;
    h.rtsp(
        &launch,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    until("older RTSP cleanup resumed", WAIT, || {
        !h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;
    // The old connection can now reach its read only after pruning. Its EOF
    // response establishes that cleanup completed before this PLAY.
    let mut older_connection = older_connection;
    older_connection.shutdown().await?;
    let mut response = Vec::new();
    older_connection.read_to_end(&mut response).await?;

    h.rtsp(&launch, 2, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch)?;
    until("newly announced session is streaming", WAIT, || {
        client.frames.load(Ordering::Acquire) > 2
            && session.stats.idr_requests.load(Ordering::Acquire) > 0
    })
    .await;
    let canceled =
        h.h.sessions
            .lock()
            .unwrap()
            .request_stop(Some(&older_launch.id));
    drop((canceled, older_launch));
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_during_control_connect_preserves_the_host_termination_reason() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.rtsp(
        &launch,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.rtsp(&launch, 2, "PLAY", "").await?;
    h.fixture
        .fail_on_control_connect
        .store(true, Ordering::Release);
    let client = Client::new(h.ports, launch)?;
    until("early failure reason delivered", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), FAILED);
    drop(client);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_media_pings_wake_idle_sockets() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    // A ping the client's worker sent after this one would take the video peer back.
    client.stop_video_pings().await;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    for port in [h.ports.video, h.ports.audio] {
        socket.send_to(b"PING", address(port))?;
    }
    let peer = socket.local_addr()?;
    until("legacy audio and video pings registered", WAIT, || {
        let peers = h.media.as_ref().unwrap().peers.lock().unwrap();
        [false, true]
            .into_iter()
            .all(|audio| peers.get(&(launch.id.clone(), audio)) == Some(&peer))
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_rtsp_requests_receive_an_authenticated_error_response() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    for (sequence, method, target, body, message) in [
        (
            1,
            "PLAY",
            "streamid=video",
            "",
            "ANNOUNCE required before PLAY",
        ),
        (
            2,
            "SETUP",
            "streamid=unknown",
            "",
            "unknown stream setup target",
        ),
        (
            3,
            "ANNOUNCE",
            "streamid=video",
            "a=x-nv-video[0].clientViewportWd:1\r\n",
            "",
        ),
    ] {
        let response = h
            .rtsp_at(&launch, sequence, method, target, body)
            .await
            .unwrap_err()
            .to_string();
        assert!(response.starts_with("RTSP/1.0 400"), "{response}");
        assert!(response.contains(message), "{response}");
    }
    let canceled = h.h.sessions.lock().unwrap().request_stop(Some(&launch.id));
    drop(canceled);
    drop(launch);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vanishes_then_resumes_within_ping_timeout() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    drop(client);
    assert!(!old.stopping());
    let resumed = h.launch(0, true).await?;
    assert_ne!(resumed.id, old.launch.id);
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    h.counts(0, 1).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    let (session, client) = h.play(&resumed).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vanishes_then_resumes_after_ping_timeout() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play_short_timeout(&launch).await?;
    drop(client);
    until("vanished client times out", WAIT, || old.stopping()).await;
    assert_eq!(old.termination_reason(), FAILED);
    h.empty().await?;
    let resumed = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&resumed).await?;
    h.end(&session, client).await?;
    h.finish().await
}

async fn network_drop(seconds: u64, timeout_ms: u32, survives: bool) -> Result<()> {
    let h = Harness::new(timeout_ms, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let lost_at = Instant::now();
    client.network(true);
    if !survives {
        until(
            "outage ends the session within ping_timeout plus worker scheduling allowance",
            Duration::from_millis(u64::from(timeout_ms)) + WAIT,
            || session.stopping(),
        )
        .await;
        assert_eq!(session.termination_reason(), FAILED);
        h.empty().await?;
    }
    tokio::time::sleep(Duration::from_secs(seconds).saturating_sub(lost_at.elapsed())).await;
    assert!(h.fixture.input_releases.load(Ordering::Acquire) > 0);
    if survives {
        assert!(
            !session.stopping(),
            "{seconds}s outage ended the session within ping_timeout"
        );
        let before = session.stats.idr_requests.load(Ordering::Acquire);
        let frames = client.frames.load(Ordering::Acquire);
        client.network(false);
        until("control and video resume after packet loss", WAIT, || {
            session.stats.idr_requests.load(Ordering::Acquire) > before
                && client.frames.load(Ordering::Acquire) > frames
        })
        .await;
        h.counts(1, 0).await?;
        h.end(&session, client).await?;
    } else {
        assert!(
            session.stopping(),
            "{seconds}s outage exceeded ping_timeout without ending"
        );
        assert_eq!(session.termination_reason(), FAILED);
        drop(client);
        h.empty().await?;
        let resumed = h.launch(0, true).await?;
        let (session, client) = h.play(&resumed).await?;
        h.end(&session, client).await?;
    }
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_2s_survives_default_timeout() -> Result<()> {
    network_drop(2, 10_000, true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_10s_survives_15s_timeout() -> Result<()> {
    network_drop(10, 15_000, true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_40s_times_out_then_resumes() -> Result<()> {
    network_drop(40, 10_000, false).await
}

/// A client hears of a lowered bitrate (0x5538) only after it asks, at most once a
/// second, newest first, and hears the return to the requested rate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_notices_reach_only_a_client_that_asks() -> Result<()> {
    use butterpollo_core::link_notice::{
        LINK_NOTICE_MESSAGE_TYPE, LinkNotice, LinkNoticeRequest, LinkReason, LinkState,
    };
    let limited = |current_kbps| LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 100_000,
        current_kbps,
    };
    let heard = |client: &Client| -> Vec<(Instant, LinkNotice)> {
        client
            .notices
            .lock()
            .unwrap()
            .iter()
            .map(|(at, payload)| (*at, LinkNotice::decode(payload).expect("a valid notice")))
            .collect()
    };
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;

    // Nobody asked: a lowered bitrate is the host's business.
    session.link_notices.lock().unwrap().note(limited(75_000));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        heard(&client).is_empty(),
        "a notice went to a client that did not ask"
    );

    // Asking brings the standing limit at once.
    let asked = Instant::now();
    client.send_control(
        LINK_NOTICE_MESSAGE_TYPE,
        &LinkNoticeRequest { enabled: true }.encode(),
    );
    until("the standing limit arrives", WAIT, || {
        heard(&client).len() == 1
    })
    .await;
    assert_eq!(heard(&client)[0].1, limited(75_000));

    // Three changes in a burst: the newest alone follows, a second after the first. The first
    // left the host after the request did; a busy client may hear it late, so measure from
    // the request.
    for kbps in [60_000, 50_000, 40_000] {
        session.link_notices.lock().unwrap().note(limited(kbps));
    }
    until("the newest arrives", WAIT * 2, || heard(&client).len() == 2).await;
    let notices = heard(&client);
    assert_eq!(notices[1].1, limited(40_000));
    assert!(
        notices[1].0.duration_since(asked) >= Duration::from_millis(900),
        "{:?} from the request to the second notice",
        notices[1].0.duration_since(asked)
    );

    // The return to the requested rate clears it, and nothing else follows.
    session.link_notices.lock().unwrap().note_full(100_000);
    until("the clear arrives", WAIT * 2, || heard(&client).len() == 3).await;
    let clear = heard(&client)[2].1;
    assert_eq!(clear.state, LinkState::Full);
    assert_eq!(clear.reason, LinkReason::None);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(heard(&client).len(), 3);
    h.end(&session, client).await?;
    h.finish().await
}

/// The text field a focus watcher would see.
fn text_field(
    input_type: butterpollo_core::text_input::InputType,
) -> butterpollo_core::text_input::Focused {
    use butterpollo_core::text_input::{Field, Focused, Surrounding};
    Focused {
        field: Field {
            input_type,
            sensitive: false,
            no_autocorrect: false,
            no_autocapitalize: false,
            enter_action: false,
        },
        element: 1,
        surrounding: Some(Surrounding {
            text: "hello world".into(),
            selection: Some((5, 5)),
        }),
    }
}

/// The text input notices (0x553B) `client` heard.
fn text_notices(client: &Client) -> Vec<butterpollo_core::text_input::TextInputNotice> {
    client
        .text_notices
        .lock()
        .unwrap()
        .iter()
        .map(|(_, payload)| {
            butterpollo_core::text_input::TextInputNotice::decode(payload).expect("a valid notice")
        })
        .collect()
}

/// Keyboard follows focus (0x553B): a client that never asks hears nothing, one that asks
/// hears the field that has the focus, a lower once it left text fields, and nothing after
/// it stops asking. Over plain control the text around the cursor stays on the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_input_notices_reach_only_a_client_that_asks() -> Result<()> {
    use butterpollo_core::text_input::{
        InputType, NoticeKind, TEXT_INPUT_MESSAGE_TYPE, TextInputRequest, Watch,
    };
    let h = Harness::configured(10_000, 1, "text_input_surrounding_text = enabled\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;

    // Stock Moonlight never asks: the focus stays the host's business.
    session
        .text_input
        .lock()
        .unwrap()
        .note(Some(text_field(InputType::Text)), Instant::now());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(text_notices(&client).is_empty());
    assert!(h.fixture.text_watches.lock().unwrap().is_empty());

    let ask = |notices: bool| TextInputRequest {
        notices,
        surrounding_text: true,
        element_ids: false,
        max_text: 64,
    };
    client.send_control(TEXT_INPUT_MESSAGE_TYPE, &ask(true).encode());
    until("the watcher starts", WAIT, || {
        !h.fixture.text_watches.lock().unwrap().is_empty()
    })
    .await;
    // Plain control: no text, though the client asked and the host allows it.
    assert_eq!(
        *h.fixture.text_watches.lock().unwrap(),
        [Watch::On { max_text: 0 }]
    );
    session
        .text_input
        .lock()
        .unwrap()
        .note(Some(text_field(InputType::Email)), Instant::now());
    until("the raise arrives", WAIT, || {
        text_notices(&client).len() == 1
    })
    .await;
    let raise = &text_notices(&client)[0];
    assert_eq!(
        (raise.kind, raise.input_type),
        (NoticeKind::Raise, InputType::Email)
    );
    assert_eq!(raise.text, "");
    assert_eq!(raise.element, 0, "the client did not ask for element ids");

    session
        .text_input
        .lock()
        .unwrap()
        .note(None, Instant::now());
    until("the lower arrives", WAIT, || {
        text_notices(&client).len() == 2
    })
    .await;
    let lower = &text_notices(&client)[1];
    assert_eq!(lower.kind, NoticeKind::Lower);
    assert!(lower.serial > raise.serial);

    client.send_control(TEXT_INPUT_MESSAGE_TYPE, &ask(false).encode());
    until("the watcher stops", WAIT, || {
        h.fixture.text_watches.lock().unwrap().len() == 2
    })
    .await;
    assert_eq!(h.fixture.text_watches.lock().unwrap()[1], Watch::Off);
    session
        .text_input
        .lock()
        .unwrap()
        .note(Some(text_field(InputType::Text)), Instant::now());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(text_notices(&client).len(), 2);
    h.end(&session, client).await?;
    h.finish().await
}

/// Over encrypted control, with `text_input_surrounding_text` on, the notice carries the text
/// around the cursor; never for a password field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_input_sends_surrounding_text_only_over_encrypted_control() -> Result<()> {
    use butterpollo_core::text_input::{
        InputType, TEXT_INPUT_MESSAGE_TYPE, TextInputRequest, Watch,
    };
    let h = Harness::configured(10_000, 1, "text_input_surrounding_text = enabled\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let request = TextInputRequest {
        notices: true,
        surrounding_text: true,
        element_ids: true,
        max_text: 64,
    };
    client
        .outbox
        .lock()
        .unwrap()
        .push(butterpollo_core::packet::encrypted_control(
            &launch.key,
            1,
            TEXT_INPUT_MESSAGE_TYPE,
            &request.encode(),
            false,
        )?);
    until("the watcher starts", WAIT, || {
        !h.fixture.text_watches.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        *h.fixture.text_watches.lock().unwrap(),
        [Watch::On { max_text: 64 }]
    );
    session
        .text_input
        .lock()
        .unwrap()
        .note(Some(text_field(InputType::Text)), Instant::now());
    until("the raise arrives", WAIT, || {
        text_notices(&client).len() == 1
    })
    .await;
    let raise = &text_notices(&client)[0];
    assert_eq!(raise.text, "hello world");
    assert_eq!((raise.selection_start, raise.selection_end), (5, 5));
    assert_eq!(raise.element, 1, "the client asked for element ids");

    session
        .text_input
        .lock()
        .unwrap()
        .note(Some(text_field(InputType::Password)), Instant::now());
    until("the update arrives", WAIT, || {
        text_notices(&client).len() == 2
    })
    .await;
    let update = &text_notices(&client)[1];
    assert_eq!(update.input_type, InputType::Password);
    assert_eq!(update.text, "", "a password field's text left the host");
    h.end(&session, client).await?;
    h.finish().await
}

/// A device without the keyboard permission, or a host with `keyboard_follows_focus` off,
/// starts no watcher and hears nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_input_needs_the_keyboard_permission_and_the_setting() -> Result<()> {
    use butterpollo_core::text_input::{
        InputType, TEXT_INPUT_MESSAGE_TYPE, TextInputRequest, Watch,
    };
    for (config, perm) in [
        ("", !butterpollo_core::input::KEYBOARD_PERMISSION),
        ("keyboard_follows_focus = disabled\n", u32::MAX),
    ] {
        let h = Harness::configured(10_000, 1, config).await?;
        h.h.paired.write().unwrap().clients[0].perm = perm;
        let launch = h.launch(0, false).await?;
        let (session, client) = h.play(&launch).await?;
        let request = TextInputRequest {
            notices: true,
            surrounding_text: false,
            element_ids: false,
            max_text: 0,
        };
        client.send_control(TEXT_INPUT_MESSAGE_TYPE, &request.encode());
        until("the request is handled", WAIT, || {
            !h.fixture.text_watches.lock().unwrap().is_empty()
        })
        .await;
        assert_eq!(
            *h.fixture.text_watches.lock().unwrap(),
            [Watch::Off],
            "{config}"
        );
        session
            .text_input
            .lock()
            .unwrap()
            .note(Some(text_field(InputType::Text)), Instant::now());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(text_notices(&client).is_empty(), "{config}");
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

/// The device's keyboard layout report (0x553E) is kept, the newest winning, while the host
/// leaves the layout to the device; with `keyboard_layout = us` it is not taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_layout_reports_keep_the_newest() -> Result<()> {
    use butterpollo_core::keyboard_layout::{
        KEYBOARD_LAYOUT_MESSAGE_TYPE, KeyboardLayout, LayoutSource,
    };
    let report = |klid: u32, tag: &str| KeyboardLayout {
        source: LayoutSource::Klid,
        klid,
        tag: tag.into(),
    };
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    for layout in [report(0x0407, "de-DE"), report(0x040C, "fr-FR")] {
        client.send_control(KEYBOARD_LAYOUT_MESSAGE_TYPE, &layout.encode().unwrap());
    }
    // Kept as the layout id it resolved to on arrival.
    until("the newest report is kept", WAIT, || {
        *session.keyboard_layout.lock().unwrap() == Some(0x040C)
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await?;

    let h = Harness::configured(10_000, 1, "keyboard_layout = us\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    client.send_control(
        KEYBOARD_LAYOUT_MESSAGE_TYPE,
        &report(0x0407, "de-DE").encode().unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(*session.keyboard_layout.lock().unwrap(), None);
    h.end(&session, client).await?;
    h.finish().await
}

/// A device without the keyboard permission does not pick the layout keys go with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_layout_reports_need_the_keyboard_permission() -> Result<()> {
    use butterpollo_core::keyboard_layout::{
        KEYBOARD_LAYOUT_MESSAGE_TYPE, KeyboardLayout, LayoutSource,
    };
    use butterpollo_core::text_input::{TEXT_INPUT_MESSAGE_TYPE, TextInputRequest};
    let h = Harness::new(10_000, 1).await?;
    h.h.paired.write().unwrap().clients[0].perm = !butterpollo_core::input::KEYBOARD_PERMISSION;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let german = KeyboardLayout {
        source: LayoutSource::Klid,
        klid: 0x0407,
        tag: "de-DE".into(),
    };
    client.send_control(KEYBOARD_LAYOUT_MESSAGE_TYPE, &german.encode().unwrap());
    // Control messages are handled in order: once the request after it is, so is the report.
    let request = TextInputRequest {
        notices: true,
        surrounding_text: false,
        element_ids: false,
        max_text: 0,
    };
    client.send_control(TEXT_INPUT_MESSAGE_TYPE, &request.encode());
    until("the request after the report is handled", WAIT, || {
        !h.fixture.text_watches.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(*session.keyboard_layout.lock().unwrap(), None);
    h.end(&session, client).await?;
    h.finish().await
}

/// Reliable control messages the host sent during a 10 s outage reach the client right after
/// it: ENet resends them once the client is heard again, not when the timeouts that doubled
/// in the outage run out (rust/vendor/rusty_enet).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reliable_messages_sent_in_an_outage_arrive_when_it_ends() -> Result<()> {
    use butterpollo_core::link_notice::{
        LINK_NOTICE_MESSAGE_TYPE, LinkNotice, LinkNoticeRequest, LinkReason, LinkState,
    };
    let limited = |current_kbps| LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 100_000,
        current_kbps,
    };
    let arrival = |client: &Client, notice: LinkNotice| -> Option<Instant> {
        client
            .notices
            .lock()
            .unwrap()
            .iter()
            .find(|(_, payload)| LinkNotice::decode(payload).expect("a valid notice") == notice)
            .map(|(at, _)| *at)
    };
    let h = Harness::new(15_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    session.link_notices.lock().unwrap().note(limited(75_000));
    client.send_control(
        LINK_NOTICE_MESSAGE_TYPE,
        &LinkNoticeRequest { enabled: true }.encode(),
    );
    until("the standing limit arrives", WAIT, || {
        arrival(&client, limited(75_000)).is_some()
    })
    .await;

    let lost_at = Instant::now();
    client.network(true);
    // Over a second apart, so the host sends both in the outage.
    for (at_ms, kbps) in [(1_200, 60_000), (2_400, 50_000)] {
        tokio::time::sleep(Duration::from_millis(at_ms).saturating_sub(lost_at.elapsed())).await;
        session.link_notices.lock().unwrap().note(limited(kbps));
    }
    tokio::time::sleep(Duration::from_secs(10).saturating_sub(lost_at.elapsed())).await;
    assert!(
        arrival(&client, limited(60_000)).is_none(),
        "a notice crossed the outage"
    );
    let restored_at = Instant::now();
    client.network(false);
    until(
        "the notices sent in the outage arrive",
        Duration::from_secs(20),
        || arrival(&client, limited(50_000)).is_some(),
    )
    .await;
    let delay = arrival(&client, limited(50_000))
        .unwrap()
        .duration_since(restored_at);
    eprintln!("notice delay after the outage: {delay:?}");
    assert!(
        delay <= Duration::from_millis(500),
        "{delay:?} after the outage ended"
    );
    assert!(!session.stopping());
    h.end(&session, client).await?;
    h.finish().await
}

/// A device's `background_stream` launch of an app runs in Nucleus Co-op, a
/// private session of one player: the host's own app is neither replaced,
/// started nor kept, the stream is a Nucleus player's, its owner returns to
/// it and another device is refused, by the app and by its co-op entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_launches_run_in_nucleus_and_only_their_owner_returns() -> Result<()> {
    let (h, pc, entry) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
    }
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let first = launch_app(&h, 0, portal).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    assert_eq!(seat.slot, 0);
    assert_eq!((launch.background, launch.app_id), (Some(true), portal));
    let listed = h.h.nucleus.sessions();
    assert_eq!(
        listed[0].session.kind,
        butterpollo_core::nucleus::coop::Kind::Background
    );
    assert_eq!(
        (listed[0].session.slots.len(), listed[0].app_id),
        (1, portal)
    );
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    let config = crate::stream::effective_config(&h.h, &launch)?;
    assert!(crate::nucleus::player(&config));
    assert!(!butterpollo_core::background::enabled(&config));
    let refused = launch_app(&h, 1, portal).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains("Portal 2 can&apos;t stream in the background")
            || refused.contains("Portal 2 can't stream in the background"),
        "{refused}"
    );
    assert!(refused.contains("for a background stream"), "{refused}");
    assert!(pending_of(&h, 1).is_none());
    let by_entry = launch_app(&h, 1, entry).await?;
    assert!(by_entry.contains("status_code=\"409\""), "{by_entry}");
    // The owner's next launch returns to its own slot of the same session.
    let again = launch_app(&h, 0, portal).await?;
    assert!(again.contains("status_code=\"200\""), "{again}");
    let relaunch = pending_of(&h, 0).unwrap();
    assert_eq!(relaunch.nucleus, Some(seat));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    drop(launch);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&relaunch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &relaunch.id);
    drop(relaunch);
    // The harness's last launch is a normal one.
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
    }
    h.finish().await
}

/// A one-player game without a hub handler streams in the background with
/// Rubylight's own handler, written once at the launch: the stream is a
/// Nucleus player's like any other, with nothing added on its way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_without_a_hub_handler_runs_with_ours() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    let mut ice = h.h.apps.read().unwrap().last().unwrap().clone();
    ice.name = "Ice".into();
    ice.cmd = r"C:\NoSuchGames\Ice\Ice.exe".into();
    ice.extra.insert("background-stream".into(), true.into());
    let id = ice.id();
    h.h.apps.write().unwrap().push(ice);
    let started = launch_app(&h, 0, id).await?;
    assert!(started.contains("status_code=\"200\""), "{started}");
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].session.game, own[0].guid());
    assert_eq!((listed[0].session.slots.len(), listed[0].app_id), (1, id));
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    let config = crate::stream::effective_config(&h.h, &launch)?;
    assert!(crate::nucleus::player(&config));
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &launch.id);
    drop(launch);
    h.finish().await
}

/// A background launch Nucleus Co-op can't run is refused with the reason
/// and streams nothing: no display, no app, nothing pending. An app's own
/// `background-stream` asks for one as the device's setting does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_nucleus_cannot_run_streams_nothing() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    // A launcher's link names no game for Nucleus Co-op to start.
    let mut store = h.h.apps.read().unwrap().last().unwrap().clone();
    store.name = "Store Game".into();
    store.cmd = "com.epicgames.launcher://apps/x?action=launch".into();
    store.extra.insert("background-stream".into(), true.into());
    let id = store.id();
    h.h.apps.write().unwrap().push(store);
    let missing = launch_app(&h, 0, id).await?;
    assert!(missing.contains("status_code=\"404\""), "{missing}");
    assert!(
        missing.contains("Store Game does not name one"),
        "{missing}"
    );
    assert!(missing.contains("stream in the background"), "{missing}");
    assert!(pending_of(&h, 0).is_none());
    // Where Nucleus Co-op can't run at all, 503 and why.
    h.h.nucleus
        .use_pc(Arc::new(crate::nucleus::fake::Pc::unavailable()));
    let unavailable = launch_app(&h, 0, id).await?;
    assert!(unavailable.contains("status_code=\"503\""), "{unavailable}");
    assert!(unavailable.contains("not on this test PC"), "{unavailable}");
    h.counts(0, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert!(pc.started.lock().unwrap().is_empty());
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_race_to_resume_the_same_app() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let (a, b) = tokio::join!(h.launch(0, true), h.launch(1, true));
    let (a, b) = (a?, b?);
    assert_ne!(a.id, b.id);
    h.counts(0, 2).await?;
    let (a, b) = tokio::join!(h.play(&a), h.play(&b));
    let ((a, ac), (b, bc)) = (a?, b?);
    h.counts(2, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 2);
    assert_eq!(h.fixture.inputs.load(Ordering::Acquire), 2);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    h.rtsp(&a.launch, 3, "TEARDOWN", "").await?;
    until("first client's resources released", WAIT, || {
        h.fixture.displays.load(Ordering::Acquire) == 1
            && h.fixture.inputs.load(Ordering::Acquire) == 1
    })
    .await;
    assert_eq!(a.termination_reason(), CLOSED);
    assert!(!b.stopping());
    let before = bc.frames.load(Ordering::Acquire);
    until(
        "second client's video survives first client's teardown",
        WAIT,
        || bc.frames.load(Ordering::Acquire) > before,
    )
    .await;
    h.counts(1, 0).await?;
    drop(ac);
    h.end(&b, bc).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtsp_reconnect_waits_for_previous_teardown() -> Result<()> {
    for (client, resume) in [(0, false), (0, true), (1, false), (1, true)] {
        reconnect_waits_for_previous_teardown(client, resume).await?;
    }
    Ok(())
}

async fn reconnect_waits_for_previous_teardown(client_index: usize, resume: bool) -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    h.fixture.hold_teardown.store(true, Ordering::Release);
    h.rtsp(&launch, 3, "TEARDOWN", "").await?;
    until("previous session enters resource teardown", WAIT, || {
        h.fixture.in_teardown.load(Ordering::Acquire)
    })
    .await;
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    h.counts(0, 0).await?;
    assert!(
        h.h.sessions
            .lock()
            .unwrap()
            .teardown
            .contains_key(&launch.id)
    );
    let (session, resumed_client, overlapped) = {
        let resume = h.launch(client_index, resume);
        tokio::pin!(resume);
        let launch = match tokio::time::timeout(Duration::from_millis(500), &mut resume).await {
            Ok(result) => result?,
            Err(_) => {
                h.counts(0, 0).await?;
                assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
                h.fixture.hold_teardown.store(false, Ordering::Release);
                resume.await?
            }
        };
        let (session, resumed_client) = h.play(&launch).await?;
        let overlapped = h.fixture.displays.load(Ordering::Acquire) > 1;
        h.fixture.hold_teardown.store(false, Ordering::Release);
        (session, resumed_client, overlapped)
    };
    drop(client);
    h.end(&session, resumed_client).await?;
    h.finish().await?;
    assert!(
        !overlapped,
        "RTSP PLAY started a replacement while the previous session still owned display resources in teardown"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtsp_reconnect_reports_busy_when_previous_teardown_times_out() -> Result<()> {
    for (client_index, resume) in [(0, false), (0, true), (1, false), (1, true)] {
        let h = Harness::new(10_000, 2).await?;
        let launch = h.launch(0, false).await?;
        let (old, client) = h.play(&launch).await?;
        h.fixture.hold_teardown.store(true, Ordering::Release);
        h.rtsp(&launch, 3, "TEARDOWN", "").await?;
        until("previous session enters resource teardown", WAIT, || {
            h.fixture.in_teardown.load(Ordering::Acquire)
        })
        .await;

        let started = Instant::now();
        let response = h.launch_response(client_index, resume).await?;
        assert!(started.elapsed() >= Duration::from_secs(5));
        assert!(response.contains("status_code=\"503\""), "{response}");
        assert!(
            response.contains("status_message=\"Another stream operation is still running\""),
            "{response}"
        );
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "0"
        );
        assert!(!response.contains("sessionUrl0"), "{response}");
        h.counts(0, 0).await?;
        assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
        assert!(
            h.h.sessions
                .lock()
                .unwrap()
                .teardown
                .contains_key(&launch.id)
        );
        assert_eq!(old.termination_reason(), CLOSED);

        h.fixture.hold_teardown.store(false, Ordering::Release);
        drop(client);
        h.empty().await?;
        let launch = h.launch(client_index, resume).await?;
        let (session, client) = h.play(&launch).await?;
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

/// A control worker that runs late finds ENet's own timeout for a lost client before its ping
/// check. The stream ended as closed by the host (0x80030023); it was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_control_stream_found_late_by_enet_still_fails() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    let lost = Instant::now();
    // From before the ping check is due, once ENet has resent its unanswered ping often enough,
    // to after ENet's own timeout.
    *h.fixture.control_held.lock().unwrap() = Some((
        lost + Duration::from_millis(1800),
        lost + Duration::from_millis(4000),
    ));
    client.control_down.store(true, Ordering::Release);
    until("the lost client's stream ends", WAIT, || session.stopping()).await;
    h.empty().await?;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.finish().await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_lost_while_video_continues_times_out() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    let before = client.frames.load(Ordering::Acquire);
    let released = h.fixture.input_releases.load(Ordering::Acquire);
    client.control_down.store(true, Ordering::Release);
    assert!(!session.stopping());
    until(
        "video traffic cannot keep a lost control stream alive",
        WAIT,
        || session.stopping(),
    )
    .await;
    h.empty().await?;
    let released_after = h.fixture.input_releases.load(Ordering::Acquire);
    let video_after_release = h.fixture.video_after_input_release.load(Ordering::Acquire);
    let frames_after = client.frames.load(Ordering::Acquire);
    assert!(
        released_after > released,
        "control loss must release input: {released} -> {released_after}"
    );
    assert!(
        video_after_release > released,
        "video must continue after control loss and before timeout: input {released} -> {released_after}, video checkpoint {video_after_release}, frames {before} -> {frames_after}"
    );
    assert!(
        frames_after > before,
        "client must still receive video while control is blocked: {before} -> {frames_after}"
    );
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    let launch = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_enet_peer_reset_mid_stream_times_out_then_resumes() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    *h.fixture.reset.lock().unwrap() = Some(session.launch.id.clone());
    until("host ENet peer was reset", WAIT, || {
        h.fixture.reset_done.load(Ordering::Acquire)
    })
    .await;
    until("reset peer's session times out", WAIT, || {
        session.stopping()
    })
    .await;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.empty().await?;
    let launch = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A microphone client as Apollo's: offered in DESCRIBE, set up and played
/// as `streamid=mic`, its encrypted packets taken for its own session only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn microphone_packets_reach_only_the_session_that_set_them_up() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(
        describe.contains(&format!(
            "m=audio {} RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\n",
            h.ports.mic
        )),
        "{describe}"
    );
    // Loopback is a LAN client: encryption optional, the microphone requested.
    assert!(
        describe.contains("encryptionSupported:13\r\n"),
        "{describe}"
    );
    assert!(describe.contains("encryptionRequested:9\r\n"), "{describe}");
    let setup = h
        .rtsp_at(&launch, 11, "SETUP", "streamid=mic/0/0", "")
        .await?;
    assert!(
        setup.contains(&format!("Transport: server_port={}\r\n", h.ports.mic)),
        "{setup}"
    );
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(session.config.mic);
    // A client that plays each stream on its own plays the microphone last.
    h.rtsp_at(&launch, 12, "PLAY", "streamid=mic", "").await?;
    let counters = h
        .media
        .as_ref()
        .unwrap()
        .mic
        .clone()
        .expect("microphone port bound");
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let send = |datagram: Vec<u8>| {
        socket
            .send_to(&datagram, address(h.ports.mic))
            .expect("microphone packet sent");
    };
    // A loaded PC loses the odd loopback datagram: what has not arrived by the next
    // attempt is sent again, under a new sequence number.
    let attempt = Duration::from_millis(200);
    let mut sequence = 0;
    let mut sent = Instant::now() - attempt;
    until("microphone packets accepted", WAIT, || {
        let accepted = counters.accepted.load(Ordering::Acquire);
        if accepted < 5 && sent.elapsed() >= attempt {
            for _ in accepted..5 {
                send(mic::seal(
                    &launch.key,
                    launch.key_id,
                    sequence,
                    0,
                    &[0xf8, 0xff, 0xfe],
                ));
                sequence += 1;
            }
            sent = Instant::now();
        }
        accepted >= 5
    })
    .await;
    assert_eq!(counters.refused.load(Ordering::Acquire), 0);
    let mut damaged = mic::seal(&launch.key, launch.key_id, sequence, 0, &[0xf8, 0xff, 0xfe]);
    // CBC has no authentication tag: a wrong key can randomly produce valid
    // padding. Keep the microphone header but truncate the ciphertext so this
    // rejection is deterministic for every randomly generated launch key.
    damaged.pop();
    let mut sent = Instant::now() - attempt;
    until("a damaged microphone packet refused", WAIT, || {
        if sent.elapsed() >= attempt {
            send(damaged.clone());
            sent = Instant::now();
        }
        counters.refused.load(Ordering::Acquire) >= 1
    })
    .await;
    h.end(&session, client).await?;
    // No session takes microphone packets from here on.
    let accepted = counters.accepted.load(Ordering::Acquire);
    // Without a microphone SETUP, the next session's packets are refused.
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(!session.config.mic);
    let refused = counters.refused.load(Ordering::Acquire);
    let mut sent = Instant::now() - attempt;
    until(
        "microphone packet of a session without one refused",
        WAIT,
        || {
            if sent.elapsed() >= attempt {
                send(mic::seal(
                    &launch.key,
                    launch.key_id,
                    0,
                    0,
                    &[0xf8, 0xff, 0xfe],
                ));
                sent = Instant::now();
            }
            counters.refused.load(Ordering::Acquire) > refused
        },
    )
    .await;
    assert_eq!(counters.accepted.load(Ordering::Acquire), accepted);
    h.end(&session, client).await?;
    // A client that would send it in the clear is not taken.
    let launch = h.launch(0, false).await?;
    h.rtsp_at(&launch, 11, "SETUP", "streamid=mic", "").await?;
    let (session, client) = h.play_encrypted(&launch, 0).await?;
    assert!(!session.config.mic);
    assert!(
        session
            .launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == "microphone"),
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// Nonary's calibration and Nightjar read the host's UDP port from the
/// headers before the probe's result, so the host must send them at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_client_runs_the_pyrowave_udp_probe_with_a_handshake() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let token = "0123456789abcdef0123456789abcdef";
    let client = std::net::UdpSocket::bind("127.0.0.1:0")?;
    // Buffer the complete two-second probe if parallel tests deschedule this receiver.
    socket2::SockRef::from(&client).set_recv_buffer_size(8 * 1024 * 1024)?;
    let named = client.local_addr()?.port();
    let url = |handshake: u8| {
        format!(
            "https://127.0.0.1:{}/pyrowave-udp-probe?kbps=20000&port={named}&packetsize=1024&token={token}&handshake={handshake}",
            h.ports.https
        )
    };
    // Not over plain HTTP, where no client is identified.
    let plain = format!("http://127.0.0.1:{}/pyrowave-udp-probe", h.ports.http);
    assert_eq!(
        h.http.get(plain).send().await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let response = h.clients[0].get(url(1)).send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let port: u16 = response.headers()["x-pyrowave-udp-port"]
        .to_str()?
        .parse()?;
    let counted = tokio::task::spawn_blocking(move || -> Result<usize> {
        client.send_to(token.as_bytes(), ("127.0.0.1", port))?;
        client.set_read_timeout(Some(Duration::from_millis(400)))?;
        let mut seen = std::collections::HashSet::new();
        let mut buffer = [0u8; 2048];
        while let Ok((bytes, _)) = client.recv_from(&mut buffer) {
            if bytes == 1024 + 48 && &buffer[..32] == token.as_bytes() {
                seen.insert(u32::from_be_bytes(buffer[32..36].try_into()?));
            }
        }
        Ok(seen.len())
    });
    let body = response.text().await?;
    let counted = counted.await??;
    let field = |name: &str| -> u32 {
        body.split(&format!("<{name}>"))
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    assert!(body.contains("status_code=\"200\""), "{body}");
    assert_eq!(
        field("expected"),
        butterpollo_core::pyrowave_probe::expected(20_000, 1024)
    );
    // A busy machine (a CI runner) can fall behind the schedule and stop at
    // the overload limit (2128 of 4317 on a windows-2022 runner); what the
    // host reports sent must still arrive.
    assert!(field("sent") > field("expected") / 4, "{body}");
    assert!(field("sent") <= field("expected"), "{body}");
    assert_eq!(counted as u32, field("sent"));
    // A bad query is refused before anything is sent.
    let bad = h.clients[0].get(url(2)).send().await?;
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_state_follows_a_launch_a_resume_and_the_end_of_a_stream() -> Result<()> {
    use butterpollo_core::native_api::session::State;
    let h = Harness::new(10_000, 1).await?;
    let journal = h.directory.join(crate::session_control::JOURNAL);
    // A game runs from before and nobody streams.
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert!(!journal.exists());

    // /launch replies and waits for the client.
    let launch = h.launch(0, false).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Launching);
    assert_eq!(snapshot.owner.as_deref(), Some("0"));
    h.h.session_flush();
    assert!(journal.exists());
    let (first, first_client) = h.play(&launch).await?;
    assert_eq!(h.h.session_snapshot().state, State::Streaming);

    // The same device's /resume supersedes its running stream.
    let resume = h.launch(0, true).await?;
    assert!(first.stopping());
    assert_eq!(h.h.session_snapshot().state, State::Launching);
    drop(first_client);
    until("the superseded stream ended", WAIT, || {
        !h.h.sessions
            .lock()
            .unwrap()
            .active
            .contains_key(&first.launch.id)
    })
    .await;
    let (second, client) = h.play(&resume).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Streaming);
    assert_eq!(snapshot.owner.as_deref(), Some("0"));

    // The client ends the stream: no display is kept, so the session is
    // idle and the game keeps running.
    h.end(&second, client).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    h.h.session_flush();
    assert!(!journal.exists());
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_while_a_launch_waits_for_its_client_ends_the_session() -> Result<()> {
    use butterpollo_core::native_api::session::State;
    let h = Harness::new(10_000, 1).await?;
    let journal = h.directory.join(crate::session_control::JOURNAL);
    // The copy holds the launch's resources: keep only the id.
    let id = h.launch(0, false).await?.id;
    assert_eq!(h.h.session_snapshot().state, State::Launching);
    h.h.session_flush();
    assert!(journal.exists());

    // Stock /cancel, from the launching device, stops the pending launch and
    // the game.
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(!h.h.sessions.lock().unwrap().pending.contains_key(&id));
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    h.h.session_flush();
    assert!(!journal.exists());
    h.finish().await
}

/// A launch whose app fails to start after its display was prepared answers
/// with an error, so the client never connects. Its pending slot and display
/// lease go at once rather than at the 30 s expiry: /serverinfo must not keep
/// counting a client that is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_whose_app_fails_to_start_is_not_left_pending() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture
        .fail_app_preparation
        .store(true, Ordering::Release);
    let response = h.launch_response(0, false).await?;
    assert!(!response.contains("status_code=\"200\""), "{response}");
    assert_eq!(xml(&response, "gamesession"), "0", "{response}");
    assert!(h.h.sessions.lock().unwrap().pending.is_empty());
    h.counts(0, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 0);
    h.fixture
        .fail_app_preparation
        .store(false, Ordering::Release);
    h.finish().await
}

/// A client that negotiates a ten-bit format (`dynamicRangeMode:1`) after a
/// launch or resume: it is ten-bit SDR only when the query said `hdrMode=0`,
/// and HDR otherwise (no `hdrMode`, or `hdrMode=1`). SDR keeps `hdr` off, the
/// part the prepared display compares, so the display prepared for the launch
/// is kept, and the client is told HDR is off. The three launches run first,
/// each ended by TEARDOWN, which leaves the app for the three resumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ten_bit_format_streams_sdr_only_with_hdr_mode_zero() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let sdp = "a=x-nv-vqos[0].bitStreamFormat:1\r\na=x-nv-video[0].dynamicRangeMode:1\r\n";
    // (resume, hdrMode, streams ten-bit SDR rather than HDR)
    for (resume, hdr_mode, sdr) in [
        (false, None, false),
        (false, Some("0"), true),
        (false, Some("1"), false),
        (true, None, false),
        (true, Some("0"), true),
        (true, Some("1"), false),
    ] {
        let case = format!("resume={resume} hdrMode={hdr_mode:?}");
        let launch = h.launch_hdr(0, resume, hdr_mode).await?;
        let (session, client) = h.connect_announcing(&launch, 0, sdp).await?;
        let config = &session.config;
        assert!(config.ten_bit(), "{case}");
        assert_eq!((config.hdr, config.sdr_10bit), (!sdr, sdr), "{case}");
        // The message needs the control connection only.
        until("HDR mode message", WAIT, || {
            !client.hdr_modes.lock().unwrap().is_empty()
        })
        .await;
        assert_eq!(
            client.hdr_modes.lock().unwrap()[0].1[0],
            u8::from(!sdr),
            "{case}"
        );
        h.end(&session, client).await?;
    }
    h.finish().await
}

const VIEWPORT: &str =
    "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n";

/// Moves `launch`'s reply `age` into the past, as a client that took that long would see it.
fn age_launch(h: &Harness, launch: &Launch, age: Duration) {
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .get_mut(&launch.id)
        .expect("launch pending")
        .created = Instant::now() - age;
}

/// A client may not use its launch before the host has prepared it and
/// replied: an RTSP request then gets no answer, and the launch, once
/// prepared, streams as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_still_being_prepared_takes_no_rtsp_request() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture
        .hold_app_preparation
        .store(true, Ordering::Release);
    let early = async {
        until("launch preparing its app", WAIT, || {
            h.fixture.app_preparation_waiting.load(Ordering::Acquire)
        })
        .await;
        let early =
            h.h.sessions
                .lock()
                .unwrap()
                .pending
                .values()
                .next()
                .cloned();
        let early = early.expect("a launch is pending while it prepares");
        let options = h.rtsp(&early, 1, "OPTIONS", "").await;
        let announce = h.rtsp(&early, 2, "ANNOUNCE", VIEWPORT).await;
        h.fixture
            .resume_app_preparation
            .store(true, Ordering::Release);
        (early, options, announce)
    };
    let (launch, (early, options, announce)) = tokio::join!(h.launch(0, false), early);
    let launch = launch?;
    assert_eq!(launch.id, early.id);
    for refused in [options, announce] {
        let refused = refused.unwrap_err().to_string();
        assert!(refused.contains("closed without a response"), "{refused}");
    }
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A handshake under way when the launch's 30 s pass keeps it for its lease;
/// a launch nobody claimed in its 30 s is refused and pruned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claimed_launch_outlives_its_30_s_and_an_unclaimed_one_expires() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.rtsp(&launch, 1, "ANNOUNCE", VIEWPORT).await?;
    age_launch(&h, &launch, Duration::from_secs(31));
    h.rtsp(&launch, 2, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "claimed launch did not stream"
    );
    h.end(&session, client).await?;

    let stale = h.launch(0, false).await?;
    age_launch(&h, &stale, Duration::from_secs(31));
    let refused = h
        .rtsp(&stale, 1, "OPTIONS", "")
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("closed without a response"), "{refused}");
    h.counts(0, 0).await?;
    // The pruned launch holds its display until the last copy goes.
    drop(stale);
    h.finish().await
}

/// The device launches again while an RTSP connection still holds its older
/// launch: that connection's request is refused, and only the new launch streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_racing_a_new_launch_streams_only_the_new_one() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let first = h.launch(0, false).await?;
    h.rtsp(&first, 1, "OPTIONS", "").await?;
    // An RTSP connection authorized for the first launch, before it reads.
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let mut held = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until(
        "RTSP connection authorized for the first launch",
        WAIT,
        || h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire),
    )
    .await;
    let second = h.launch(0, false).await?;
    assert_ne!(first.id, second.id);
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    held.write_all(&sealed_rtsp(
        &first,
        2,
        "ANNOUNCE",
        "rtsp://127.0.0.1/stream",
        VIEWPORT,
    )?)
    .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, held.read_to_end(&mut response)).await??;
    let refused = opened_rtsp(&first, "ANNOUNCE", &response)
        .unwrap_err()
        .to_string();
    assert!(refused.starts_with("RTSP/1.0 400"), "{refused}");
    assert!(refused.contains("replaced"), "{refused}");
    let refused = h.rtsp(&first, 3, "PLAY", "").await.unwrap_err().to_string();
    assert!(refused.contains("closed without a response"), "{refused}");
    // The replaced launch holds its display until the last copy goes.
    drop(first);
    h.counts(0, 1).await?;
    let (session, client) = h.play(&second).await?;
    assert_eq!(
        h.h.sessions
            .lock()
            .unwrap()
            .active
            .keys()
            .collect::<Vec<_>>(),
        [&second.id]
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// Two devices launch at once. Two PLAYs of one launch start one session,
/// and the other device's launch stays its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_launching_at_once_each_keep_their_launch_and_play_once() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let (other, launch) = tokio::join!(h.launch(0, false), h.launch(1, false));
    let (other, launch) = (other?, launch?);
    h.counts(0, 2).await?;
    h.rtsp(&launch, 1, "ANNOUNCE", VIEWPORT).await?;
    let (a, b) = tokio::join!(
        h.rtsp(&launch, 2, "PLAY", ""),
        // Not 3: that is the TEARDOWN's.
        h.rtsp(&launch, 4, "PLAY", "")
    );
    assert_eq!(
        [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "{a:?} {b:?}"
    );
    let session = {
        let sessions = h.h.sessions.lock().unwrap();
        assert_eq!(sessions.active.len(), 1);
        assert!(sessions.pending.contains_key(&other.id));
        sessions.active[&launch.id].clone()
    };
    h.counts(1, 1).await?;
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "the one session did not stream"
    );
    let canceled = h.h.sessions.lock().unwrap().request_stop(Some(&other.id));
    drop((canceled, other));
    h.end(&session, client).await?;
    h.finish().await
}

/// A client that connects and stalls is cut off at the handshake deadline,
/// without costing its launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_rtsp_connection_is_closed_at_its_deadline() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture.rtsp_deadline_ms.store(300, Ordering::Release);
    let launch = h.launch(0, false).await?;
    let started = Instant::now();
    let mut stalled = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    // Half of an encrypted request's header, then nothing.
    stalled
        .write_all(&sealed_rtsp(&launch, 1, "OPTIONS", "rtsp://127.0.0.1/stream", "")?[..12])
        .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stalled.read_to_end(&mut response)).await??;
    assert!(response.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(300));
    h.counts(0, 1).await?;
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A launch of app `id` from `client`, with the query a Moonlight launch sends.
async fn launch_app(h: &Harness, client: usize, id: u32) -> Result<String> {
    Ok(h.clients[client]
        .get(format!("https://127.0.0.1:{}/launch", h.ports.https))
        .query(&[
            ("appid", id.to_string()),
            ("rikey", hex::encode(crypto::random::<16>())),
            ("rikeyid", "1".into()),
            ("corever", "1".into()),
            ("mode", "1281x721x60".into()),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}
/// A harness whose host has Nucleus Co-op (the fake) and Portal 2, which it
/// has a handler for; client 1 may join co-op and client 0 has it turned off.
/// Returns the co-op entry's id.
async fn coop_harness() -> Result<(Harness, Arc<crate::nucleus::fake::Pc>, u32)> {
    let h = Harness::new(10_000, 2).await?;
    let pc = Arc::new(crate::nucleus::fake::Pc::installed());
    h.h.nucleus.use_pc(pc.clone());
    h.h.apps.write().unwrap().push(App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", crate::nucleus::fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    });
    h.h.paired.write().unwrap().clients[1]
        .extra
        .insert("coop_join".into(), true.into());
    // Never set, a device that launches apps may join: client 0 has it off.
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), false.into());
    let apps = h.h.apps.read().unwrap().clone();
    let entries = h.h.nucleus.entries(&apps, "");
    assert_eq!(entries.len(), 1);
    let id = entries[0].id();
    Ok((h, pc, id))
}
/// A copy of the launch `client` has pending; it holds the launch's display lease.
fn pending_of(h: &Harness, client: usize) -> Option<Launch> {
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .values()
        .find(|l| l.client.uuid == client.to_string())
        .cloned()
}

/// Two devices launching the co-op entry share one Nucleus session, each in
/// its own slot. Neither replaces, starts or keeps the host's app: Nucleus
/// started the game. The pending launches carry the seats their streams use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_launches_share_one_session_beside_the_running_app() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let first = launch_app(&h, 0, id).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    assert_eq!(xml(&first, "gamesession"), "1");
    let second = launch_app(&h, 1, id).await?;
    assert!(second.contains("status_code=\"200\""), "{second}");
    h.counts(0, 2).await?;
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    let session = a.nucleus.unwrap().session;
    assert_eq!(
        a.nucleus,
        Some(butterpollo_core::session::Seat { session, slot: 0 })
    );
    assert_eq!(
        b.nucleus,
        Some(butterpollo_core::session::Seat { session, slot: 1 })
    );
    assert_eq!((a.app_id, b.app_id), (id, id));
    assert_eq!(a.background, Some(false));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // The first window was parked at the stream's even size.
    assert_eq!(pc.last().sizes.lock().unwrap().first(), Some(&(1280, 720)));
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 2);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, true]);
    assert_eq!(listed[0].app_id, id);
    // The launches' last copies give their slots back.
    for launch in [a, b] {
        let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop(canceled);
    }
    assert_eq!(h.h.nucleus.sessions()[0].held, [false, false]);
    h.finish().await
}

/// A co-op launch Nucleus refuses answers with the refusal's status and text
/// and leaves nothing pending: a device without Join co-op (409), a start
/// that fails (503) and a game whose handler went away (404).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_co_op_launches_answer_their_status_and_leave_nothing_pending() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    *pc.start_error.lock().unwrap() = Some("Nucleus Co-op did not start".into());
    let failed = launch_app(&h, 0, id).await?;
    assert!(failed.contains("status_code=\"503\""), "{failed}");
    assert!(failed.contains("Nucleus Co-op did not start"), "{failed}");
    assert_eq!(xml(&failed, "gamesession"), "0");
    *pc.start_error.lock().unwrap() = None;
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let seated = pending_of(&h, 1).unwrap();
    // Device 0 may not join the running session.
    let refused = launch_app(&h, 0, id).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(refused.contains("Can join co-op games"), "{refused}");
    assert!(pending_of(&h, 0).is_none());
    // The handler is gone by the time the next device launches.
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), true.into());
    pc.catalog.lock().unwrap().handlers.clear();
    let missing = launch_app(&h, 0, id).await?;
    assert!(missing.contains("status_code=\"404\""), "{missing}");
    assert!(missing.contains("portal2.exe"), "{missing}");
    assert!(pending_of(&h, 0).is_none());
    h.counts(0, 1).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&seated.id);
    drop(canceled);
    // The copy holds the launch's display lease and its slot.
    drop(seated);
    h.finish().await
}

/// A co-op launch that fails after Nucleus seated it gives the slot back: the
/// next launch of that device is not held off by a slot nobody streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_co_op_launch_that_fails_later_gives_its_slot_back() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    h.fixture
        .disconnect_during_preparation
        .store(true, Ordering::Release);
    let failed = launch_app(&h, 0, id).await?;
    assert!(failed.contains("status_code=\"503\""), "{failed}");
    assert!(failed.contains("disconnected or changing"), "{failed}");
    assert!(pending_of(&h, 0).is_none());
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    h.empty().await?;
    h.fixture
        .disconnect_during_preparation
        .store(false, Ordering::Release);
    h.fixture
        .session_disconnected
        .store(false, Ordering::Release);
    // The device gets its own slot back in the same session.
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let launch = pending_of(&h, 0).unwrap();
    assert_eq!(
        launch.nucleus,
        Some(butterpollo_core::session::Seat {
            session: listed[0].session.id,
            slot: 0
        })
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    drop(launch);
    h.finish().await
}

/// A WebRTC launch of the co-op entry takes a seat as a Moonlight launch
/// does. Nucleus's refusals become start-session errors with their text:
/// busy or refused is SessionActive, anything else a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webrtc_co_op_launches_take_seats_and_answer_refusals() -> Result<()> {
    use crate::nvhttp::{WebRtcLaunch, WebRtcLaunchError};
    let (h, pc, id) = coop_harness().await?;
    let webrtc = |client: usize| {
        let request = WebRtcLaunch {
            client: h.h.paired.read().unwrap().clients[client].clone(),
            peer: std::net::Ipv4Addr::LOCALHOST.into(),
            app_id: id,
            width: 1280,
            height: 720,
            fps: 60,
            hdr: false,
        };
        let host = h.h.clone();
        tokio::task::spawn_blocking(move || crate::nvhttp::launch_webrtc(&host, request))
    };
    *pc.start_error.lock().unwrap() = Some("Nucleus Co-op did not start".into());
    match webrtc(0).await?.err() {
        Some(WebRtcLaunchError::Failed(message)) => {
            assert!(message.contains("did not start"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    *pc.start_error.lock().unwrap() = None;
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let seat = pending_of(&h, 1).unwrap().nucleus.unwrap();
    match webrtc(0).await?.err() {
        Some(WebRtcLaunchError::SessionActive(message)) => {
            assert!(message.contains("Can join co-op games"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), true.into());
    let Ok(launch) = webrtc(0).await? else {
        panic!("the co-op WebRTC launch failed");
    };
    assert_eq!(
        launch.transport,
        butterpollo_core::session::Transport::WebRtc
    );
    assert_eq!(
        launch.nucleus,
        Some(butterpollo_core::session::Seat {
            session: seat.session,
            slot: 1
        })
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    drop(launch);
    // A co-op player's WebRTC launch leaves the other player's Moonlight launch alone.
    let other = pending_of(&h, 1).expect("the Moonlight launch stays").id;
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&other);
    drop(canceled);
    h.finish().await
}

/// A co-op launch never takes the host's session from its owner, and a co-op
/// player's stock /cancel or native End session ends only its own launch: the
/// other player's launch and the host's app stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_co_op_player_neither_owns_the_session_nor_cancels_the_others() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    let before = h.h.session_snapshot();
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let after = h.h.session_snapshot();
    assert_eq!((after.state, after.owner), (before.state, before.owner));
    let response = h.clients[1]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(pending_of(&h, 1).is_none());
    assert!(pending_of(&h, 0).is_some());
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    // Nightjar's End session from the player that is left ends its own launch
    // too, though it owns no host session; the host's app stays.
    let host = h.h.clone();
    let client = h.h.paired.read().unwrap().clients[0].clone();
    tokio::task::spawn_blocking(move || {
        crate::native_api::session_action(&host, &client, crate::native_api::Action::End)
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(pending_of(&h, 0).is_none());
    assert_eq!(h.h.nucleus.sessions()[0].held, [false, false]);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    h.finish().await
}

/// A co-op launch replaced by the device's next launch gives its slot back:
/// the player shows as away and keeps the slot for `coop::GRACE`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_co_op_launch_gives_its_slot_back() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, false]);
    assert!(!listed[0].session.players()[0].away);
    let other = h.launch(0, false).await?;
    assert!(other.nucleus.is_none());
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&other.id);
    drop(canceled);
    drop(other);
    h.finish().await
}

/// Moonlight's multi-controller packet: controller `id` of the `active` ones.
fn pad_packet(id: u16, active: u16, buttons: u16) -> Vec<u8> {
    let mut b = vec![0u8; 34];
    b[..4].copy_from_slice(&30u32.to_be_bytes());
    b[4..8].copy_from_slice(&12u32.to_le_bytes());
    b[10..12].copy_from_slice(&id.to_le_bytes());
    b[12..14].copy_from_slice(&active.to_le_bytes());
    b[16..18].copy_from_slice(&buttons.to_le_bytes());
    b
}
/// A key press of A.
fn key_packet() -> Vec<u8> {
    let mut b = vec![0u8; 14];
    b[..4].copy_from_slice(&10u32.to_be_bytes());
    b[4..8].copy_from_slice(&3u32.to_le_bytes());
    b[9..11].copy_from_slice(&0x8041u16.to_le_bytes());
    b
}

/// Two players' streams drive their own slots' pads and never the desktop:
/// a keyboard is dropped and said once. When the game crashes both streams
/// end with the reason, the session is stopped and nothing stays active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_streams_drive_their_slots_and_end_with_the_session() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    let (sb, cb) = h.play(&b).await?;
    h.counts(2, 0).await?;
    ca.send_encrypted(&a.key, 0, 0x0206, &pad_packet(0, 1, 0x1000));
    ca.send_encrypted(&a.key, 1, 0x0206, &key_packet());
    ca.send_encrypted(&a.key, 2, 0x0206, &key_packet());
    cb.send_encrypted(&b.key, 0, 0x0206, &pad_packet(0, 1, 0x2000));
    until("both players' pads moved", WAIT, || {
        pc.last().inputs.lock().unwrap().len() == 2
    })
    .await;
    let mut inputs = pc.last().inputs.lock().unwrap().clone();
    inputs.sort_by_key(|(slot, _)| *slot);
    assert!(matches!(
        inputs[..],
        [
            (
                0,
                butterpollo_core::input::Input::Controller {
                    buttons: 0x1000,
                    ..
                }
            ),
            (
                1,
                butterpollo_core::input::Input::Controller {
                    buttons: 0x2000,
                    ..
                }
            )
        ]
    ));
    let codes = |s: &Session| -> Vec<String> {
        s.launch
            .warnings
            .snapshot()
            .into_iter()
            .map(|w| w.code)
            .filter(|code| code.starts_with("coop_"))
            .collect()
    };
    until("the keyboard was said to be dropped", WAIT, || {
        codes(&sa) == ["coop_keyboard_mouse"]
    })
    .await;
    assert!(codes(&sb).is_empty());
    pc.last()
        .session
        .lock()
        .unwrap()
        .end(butterpollo_core::nucleus::coop::End::Crashed(
            "portal2.exe exited".into(),
        ));
    until("both streams ended", WAIT, || {
        sa.stopping() && sb.stopping()
    })
    .await;
    for s in [&sa, &sb] {
        let ended = s
            .launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == "coop_ended")
            .unwrap();
        assert!(
            ended.message.contains("portal2.exe exited"),
            "{}",
            ended.message
        );
    }
    drop((ca, cb, a, b, sa, sb));
    h.empty().await?;
    // The streams' last copies of their launches gave the slots back.
    until("the session stopped", WAIT, || pc.last().stopped()).await;
    h.finish().await
}

/// The console's End and every player gone (`End::Empty`, which Nucleus
/// Co-op's session reaches after `coop::GRACE`) close each player's stream
/// quietly with the reason, never as a failure, and stop Nucleus Co-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_streams_close_quietly_when_the_session_is_ended_or_empty() -> Result<()> {
    use butterpollo_core::nucleus::coop::End;
    let (h, pc, id) = coop_harness().await?;
    let closed_with = |s: &Session, text: &str| {
        let ended = s
            .launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == "coop_ended")
            .unwrap();
        assert!(ended.message.contains(text), "{}", ended.message);
        assert_eq!(s.termination_reason(), CLOSED);
    };
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    let session = a.nucleus.unwrap().session;
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    let (sb, cb) = h.play(&b).await?;
    h.counts(2, 0).await?;
    assert!(h.h.nucleus.end(session));
    until("both streams ended", WAIT, || {
        sa.stopping() && sb.stopping()
    })
    .await;
    closed_with(&sa, "The Portal 2 co-op session was ended on the PC");
    closed_with(&sb, "The Portal 2 co-op session was ended on the PC");
    assert!(pc.last().stopped());
    drop((ca, cb, a, b, sa, sb));
    h.empty().await?;
    // The next launch starts a new session; everybody leaves it.
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let a = pending_of(&h, 0).unwrap();
    assert_ne!(a.nucleus.unwrap().session, session);
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    pc.last().session.lock().unwrap().end(End::Empty);
    until("the stream ended", WAIT, || sa.stopping()).await;
    closed_with(&sa, "every player left");
    drop((ca, a, sa));
    h.empty().await?;
    until("the session stopped", WAIT, || pc.last().stopped()).await;
    h.finish().await
}

/// A co-op launch whose device never connects gives its slot back when it
/// expires, once the expired launch is dropped outside the sessions lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_co_op_launch_gives_its_slot_back() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let expired = {
        let mut sessions = h.h.sessions.lock().unwrap();
        let long_ago = Instant::now()
            .checked_sub(butterpollo_core::session::LAUNCH_WAIT + Duration::from_secs(1))
            .unwrap();
        for launch in sessions.pending.values_mut() {
            launch.created = long_ago;
        }
        sessions.expire()
    };
    assert_eq!(expired.len(), 1);
    // The expired launch still holds the slot until it is dropped.
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    drop(expired);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    h.finish().await
}
