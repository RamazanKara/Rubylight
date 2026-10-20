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
    sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicUsize},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CLOSED: u32 = 0x8003_0023;
const FAILED: u32 = 0x8000_4005;
const WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct Fixture {
    displays: Arc<AtomicUsize>,
    audios: Arc<AtomicUsize>,
    inputs: Arc<AtomicUsize>,
    session_disconnected: AtomicBool,
    disconnect_during_preparation: AtomicBool,
    session_checks: AtomicUsize,
    apps_prepared: AtomicUsize,
    resources_retained: AtomicUsize,
    pub(super) input_releases: AtomicUsize,
    video_after_input_release: AtomicUsize,
    hold_teardown: AtomicBool,
    in_teardown: AtomicBool,
    reset: Mutex<Option<String>>,
    reset_done: AtomicBool,
    fail_on_control_connect: AtomicBool,
    hold_rtsp_cleanup: AtomicBool,
    rtsp_cleanup_waiting: AtomicBool,
    resume_rtsp_cleanup: AtomicBool,
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
        *launch.audio_preparation.lock().unwrap() = Some(Box::new(Audio {
            _slot: Slot::new(&self.audios),
        }));
        if launch.role == Role::Stream && h.current_app.lock().unwrap().is_none() {
            self.apps_prepared.fetch_add(1, Ordering::AcqRel);
            return Ok(Some(crate::process::RunningApp::with_environment(
                app.context("fixture application missing")?,
                Default::default(),
            )?));
        }
        Ok(None)
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
    pub(super) fn stream(&self, media: &Media, h: &Shared, s: &Session) -> Result<()> {
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
                // packetizer and video socket. No decoder or GPU is involved.
                for packet in
                    packetizer.encode_recovery(&[0, 0, 0, 1, 0x65, 0x80], true, false, 0, 0)?
                {
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

async fn until(message: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {message}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn address(port: u16) -> SocketAddr {
    assert!(!(47984..=48010).contains(&port) && !(48518..=48544).contains(&port));
    ([127, 0, 0, 1], port).into()
}
fn udp() -> UdpSocket {
    static NEXT: AtomicU16 = AtomicU16::new(52000);
    loop {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(port < 64000, "fixture UDP port range exhausted");
        if let Ok(socket) = UdpSocket::bind(address(port)) {
            socket.set_nonblocking(true).unwrap();
            return socket;
        }
    }
}
fn free_ports() -> Ports {
    static NEXT: AtomicU16 = AtomicU16::new(22000);
    loop {
        let base = NEXT.fetch_add(32, Ordering::Relaxed);
        assert!(base < 40000, "fixture host port range exhausted");
        let ports = Ports::from_base(base);
        if reserve(ports).is_ok() {
            return ports;
        }
    }
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
    directory: PathBuf,
    started: Instant,
}
impl Harness {
    async fn new(timeout_ms: u32, clients: usize) -> Result<Self> {
        let started = Instant::now();
        let directory =
            std::env::temp_dir().join(format!("butterpollo-reconnect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let ports = free_ports();
        std::fs::write(
            directory.join("sunshine.conf"),
            format!(
                "port={}\nbind_address=127.0.0.1\nping_timeout={timeout_ms}\nstream_audio=false\nenable_discovery=false\nupnp=false\n",
                ports.http
            ),
        )?;
        let mut h = crate::state::Host::load(directory.clone(), directory.clone(), None)?;
        let fixture = Arc::new(Fixture::default());
        Arc::get_mut(&mut h).unwrap().reconnect_fixture = Some(fixture.clone());
        h.codecs.store(1, Ordering::Release);
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
                    .timeout(Duration::from_secs(8))
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
                .timeout(WAIT)
                .build()?,
            ports,
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
        let endpoint = if resume { "resume" } else { "launch" };
        let key = hex::encode(crypto::random::<16>());
        let app = self.h.apps.read().unwrap()[0].id();
        Ok(self.clients[client]
            .get(format!("https://127.0.0.1:{}/{endpoint}", self.ports.https))
            .query(&[
                ("appid", app.to_string()),
                ("rikey", key),
                ("rikeyid", "1".into()),
                ("corever", "1".into()),
                ("mode", "640x480x60".into()),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }
    async fn launch(&self, client: usize, resume: bool) -> Result<Launch> {
        let response = self.launch_response(client, resume).await?;
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
        let response = tokio::time::timeout(WAIT, async {
            let mut socket = tokio::net::TcpStream::connect(address(self.ports.rtsp)).await?;
            socket.write_all(&wire).await?;
            let mut response = vec![];
            socket.read_to_end(&mut response).await?;
            Ok::<_, anyhow::Error>(response)
        })
        .await??;
        assert!(
            response.len() >= 24,
            "RTSP {method} closed without a response"
        );
        let length = u32::from_be_bytes(response[..4].try_into()?) & 0x7fffffff;
        assert_eq!(length as usize, response.len() - 24);
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
    async fn play(&self, launch: &Launch) -> Result<(Arc<Session>, Client)> {
        self.play_encrypted(launch, 0).await
    }
    async fn play_encrypted(
        &self,
        launch: &Launch,
        encryption: u32,
    ) -> Result<(Arc<Session>, Client)> {
        self.rtsp(launch, 1, "ANNOUNCE", &format!("a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\na=x-ss-general.encryptionEnabled:{encryption}\r\n")).await?;
        self.rtsp(launch, 2, "PLAY", "").await?;
        let session = self.h.sessions.lock().unwrap().active[&launch.id].clone();
        let client = Client::new(self.ports, launch.clone())?;
        until("control and video established", WAIT, || {
            client.frames.load(Ordering::Acquire) > 2
                && session.stats.idr_requests.load(Ordering::Acquire) > 0
        })
        .await;
        assert!(!session.stopping());
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
        until("normal termination delivered", WAIT, || {
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
        let (session, client) = self.play(&launch).await?;
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
}
impl rusty_enet::Socket for LossySocket {
    type Address = SocketAddr;
    type Error = std::io::Error;
    fn init(&mut self, options: rusty_enet::SocketOptions) -> std::io::Result<()> {
        rusty_enet::Socket::init(&mut self.socket, options)
    }
    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> std::io::Result<usize> {
        if self.blocked.load(Ordering::Acquire) {
            return Ok(buffer.len());
        }
        rusty_enet::Socket::send(&mut self.socket, address, buffer)
    }
    fn receive(
        &mut self,
        buffer: &mut [u8; rusty_enet::MTU_MAX],
    ) -> std::io::Result<Option<(SocketAddr, rusty_enet::PacketReceived)>> {
        let packet = rusty_enet::Socket::receive(&mut self.socket, buffer)?;
        if self.blocked.load(Ordering::Acquire) {
            Ok(None)
        } else {
            Ok(packet)
        }
    }
}
struct Client {
    control_down: Arc<AtomicBool>,
    video_down: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicUsize>,
    reason: Arc<AtomicU32>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}
impl Client {
    fn new(ports: Ports, launch: Launch) -> Result<Self> {
        let control_down = Arc::new(AtomicBool::new(false));
        let video_down = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicUsize::new(0));
        let reason = Arc::new(AtomicU32::new(0));
        let mut host = Host::new(
            LossySocket {
                socket: udp(),
                blocked: control_down.clone(),
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
        let video = udp();
        let (worker_stop, worker_video, worker_frames, worker_reason) = (
            stop.clone(),
            video_down.clone(),
            frames.clone(),
            reason.clone(),
        );
        let worker = thread::spawn(move || -> Result<()> {
            let mut next_ping = Instant::now();
            let mut buffer = [0; 2048];
            while !worker_stop.load(Ordering::Acquire) {
                while let Some(event) = host.service()? {
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
                    }
                }
                if Instant::now() >= next_ping {
                    if host.peer(peer_id).connected() {
                        // An IDR request is also a valid control heartbeat, and
                        // its counter proves the host parsed it after recovery.
                        host.peer_mut(peer_id)
                            .send(0, &Packet::new(&[2, 3][..], PacketKind::Reliable))?;
                        // And an unreliable copy: a reliable one queued in an
                        // outage is resent on rusty_enet's doubling timeout, up
                        // to about the outage's length after the network returns
                        // (a 10 s drop failed once on CI). Moonlight's ENet backs
                        // off linearly and is heard again within half a second.
                        host.peer_mut(peer_id).send(
                            0,
                            &Packet::new(
                                &[2, 3][..],
                                PacketKind::AlwaysUnreliable { sequenced: false },
                            ),
                        )?;
                    }
                    if !worker_video.load(Ordering::Acquire) {
                        video.send_to(launch.ping.as_bytes(), address(ports.video))?;
                    }
                    next_ping = Instant::now() + Duration::from_millis(50);
                }
                while let Ok((n, source)) = video.recv_from(&mut buffer) {
                    assert_eq!(source, address(ports.video));
                    assert!(n > 16, "video packet has no RTP payload");
                    if !worker_video.load(Ordering::Acquire) {
                        worker_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
                host.flush();
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
            reason,
            worker: Some(worker),
        })
    }
    fn network(&self, down: bool) {
        self.control_down.store(down, Ordering::Release);
        self.video_down.store(down, Ordering::Release);
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
    client.video_down.store(true, Ordering::Release);
    tokio::time::sleep(Duration::from_millis(300)).await;
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
    let (old, client) = h.play(&launch).await?;
    drop(client);
    until("vanished client times out", WAIT, || old.stopping()).await;
    assert_eq!(old.termination_reason(), FAILED);
    h.empty().await?;
    let resumed = h.launch(0, true).await?;
    let (session, client) = h.play(&resumed).await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_resume_preserves_the_owner_and_rejects_mode_changes() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
    }
    let generation = {
        let mut current = h.h.current_app.lock().unwrap();
        let app = current.as_mut().unwrap();
        app.owner = "0".into();
        app.background = true;
        app.generation.clone()
    };
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    let rejected = h.launch_response(1, true).await?;
    assert!(rejected.contains("status_code=\"409\""), "{rejected}");
    assert!(rejected.contains("another device"), "{rejected}");
    assert!(!old.stopping());
    h.counts(1, 0).await?;

    h.h.paired.write().unwrap().clients[0]
        .extra
        .remove("background_stream");
    let rejected = h.launch_response(0, true).await?;
    assert!(rejected.contains("status_code=\"409\""), "{rejected}");
    assert!(rejected.contains("changing background"), "{rejected}");
    assert!(!old.stopping());
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("background_stream".into(), true.into());

    drop(client);
    let resumed = h.launch(0, true).await?;
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    let (session, client) = h.play(&resumed).await?;
    h.end(&session, client).await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_lost_while_video_continues_times_out() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
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
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_enet_peer_reset_mid_stream_times_out_then_resumes() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    *h.fixture.reset.lock().unwrap() = Some(launch.id.clone());
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
    let (session, client) = h.play(&launch).await?;
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
    let send = |datagram: Vec<u8>| socket.send_to(&datagram, address(h.ports.mic));
    for sequence in 0..5 {
        send(mic::seal(
            &launch.key,
            launch.key_id,
            sequence,
            0,
            &[0xf8, 0xff, 0xfe],
        ))?;
    }
    let mut damaged = mic::seal(&launch.key, launch.key_id, 5, 0, &[0xf8, 0xff, 0xfe]);
    // CBC has no authentication tag: a wrong key can randomly produce valid
    // padding. Keep the microphone header but truncate the ciphertext so this
    // rejection is deterministic for every randomly generated launch key.
    damaged.pop();
    send(damaged)?;
    until("microphone packets accepted and refused", WAIT, || {
        counters.accepted.load(Ordering::Acquire) == 5
            && counters.refused.load(Ordering::Acquire) == 1
    })
    .await;
    h.end(&session, client).await?;
    // Without a microphone SETUP, the next session's packets are refused.
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(!session.config.mic);
    send(mic::seal(
        &launch.key,
        launch.key_id,
        0,
        0,
        &[0xf8, 0xff, 0xfe],
    ))?;
    until(
        "microphone packet of a session without one refused",
        WAIT,
        || counters.refused.load(Ordering::Acquire) == 2,
    )
    .await;
    assert_eq!(counters.accepted.load(Ordering::Acquire), 5);
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
    // the overload limit; what the host reports sent must still arrive.
    assert!(field("sent") > field("expected") / 2, "{body}");
    assert!(field("sent") <= field("expected"), "{body}");
    assert_eq!(counted as u32, field("sent"));
    // A bad query is refused before anything is sent.
    let bad = h.clients[0].get(url(2)).send().await?;
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
    h.finish().await
}
