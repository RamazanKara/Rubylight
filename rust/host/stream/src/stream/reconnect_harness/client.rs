//! Loopback ENet/UDP test client and lossy socket for the reconnect and interop harness.
use super::{WAIT, address, udp, until};
use crate::state::Launch;
use anyhow::Result;
use butterpollo_config::config::Ports;
use rusty_enet::{Event, Host, HostSettings, Packet, PacketKind};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct LossySocket {
    pub socket: UdpSocket,
    pub blocked: Arc<AtomicBool>,
    /// A socket that replaces `socket` from the next send or receive (`Client::move_to`).
    pub moved: Arc<Mutex<Option<UdpSocket>>>,
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
            Err(error) if butterpollo_win_io::net::datagram_lost(&error) => None,
            result => result?,
        };
        if self.blocked.load(Ordering::Acquire) {
            Ok(None)
        } else {
            Ok(packet)
        }
    }
}

pub type Notices = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;
/// Frame Warp control messages the client got: message type and payload.
pub type FrameWarpMessages = Arc<Mutex<Vec<(u16, Vec<u8>)>>>;

pub struct Client {
    pub control_down: Arc<AtomicBool>,
    pub video_down: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub frames: Arc<AtomicUsize>,
    /// Rounds the worker finished: one that began before a switch was flipped ends by the next.
    pub rounds: Arc<AtomicUsize>,
    pub reason: Arc<AtomicU32>,
    /// Kinds of every control message the host sent, in arrival order.
    pub kinds: Arc<Mutex<Vec<u16>>>,
    /// Link notices (0x5538) the host sent, with their arrival times.
    pub notices: Notices,
    /// HDR mode messages (0x010e) the host sent.
    pub hdr_modes: Notices,
    /// Text input notices (0x553B) the host sent.
    pub text_notices: Notices,
    /// Rate change answers (0x553C) the host sent.
    pub rate_answers: Notices,
    /// Clipboard sync frames (0x5508) the host sent.
    pub clipboard: Notices,
    /// Frame Warp messages (0x5544 to 0x5546) the host sent, with their types.
    pub frame_warp: FrameWarpMessages,
    /// Plain control messages for the worker to send, reliable.
    pub outbox: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Sockets ENet and the video pings move to (`move_to`).
    pub control_move: Arc<Mutex<Option<UdpSocket>>>,
    pub video_move: Arc<Mutex<Option<UdpSocket>>>,
    /// Whether the control heartbeat is an IDR request (0x0302) rather than a plain
    /// ping (0x0200), which leaves `idr_requests` to the host's own keyframes.
    pub keyframe_heartbeat: Arc<AtomicBool>,
    /// The address ENet first sent from.
    pub control_address: SocketAddr,
    /// Whether ENet connected.
    pub connected: Arc<AtomicBool>,
    /// Whether the host disconnected ENet; the worker acknowledges it.
    pub disconnected: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}
impl Client {
    pub fn new(ports: Ports, launch: Launch) -> Result<Self> {
        Self::sending_from(ports, launch, udp())
    }
    /// A client whose ENet sends from `control`.
    pub fn sending_from(ports: Ports, launch: Launch, control: UdpSocket) -> Result<Self> {
        let control_down = Arc::new(AtomicBool::new(false));
        let video_down = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicUsize::new(0));
        let rounds = Arc::new(AtomicUsize::new(0));
        let reason = Arc::new(AtomicU32::new(0));
        let kinds = Arc::new(Mutex::new(Vec::new()));
        let notices = Arc::new(Mutex::new(Vec::new()));
        let hdr_modes = Notices::default();
        let text_notices = Notices::default();
        let rate_answers = Notices::default();
        let clipboard = Notices::default();
        let frame_warp = FrameWarpMessages::default();
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
        let worker_kinds = kinds.clone();
        let worker_hdr_modes = hdr_modes.clone();
        let worker_text_notices = text_notices.clone();
        let worker_rate_answers = rate_answers.clone();
        let worker_clipboard = clipboard.clone();
        let worker_frame_warp = frame_warp.clone();
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
                        let (_, kind, payload) = butterpollo_media_policy::packet::decrypt_control(
                            &launch.key,
                            packet.data(),
                            false,
                        )?;
                        worker_kinds.lock().unwrap().push(kind);
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
                        if kind == butterpollo_input_policy::text_input::TEXT_INPUT_MESSAGE_TYPE {
                            worker_text_notices
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if kind == butterpollo_media_policy::rate_change::RATE_CHANGE_MESSAGE_TYPE {
                            worker_rate_answers
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if kind == rubylight_protocol::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE {
                            worker_clipboard
                                .lock()
                                .unwrap()
                                .push((Instant::now(), payload.clone()));
                        }
                        if matches!(
                            kind,
                            butterpollo_media_policy::frame_warp::FRAME_WARP_MODEL_MESSAGE_TYPE
                                | butterpollo_media_policy::frame_warp::FRAME_WARP_TAGS_MESSAGE_TYPE
                                | butterpollo_media_policy::frame_warp::FRAME_WARP_MASK_MESSAGE_TYPE
                                | butterpollo_media_policy::frame_warp::FRAME_WARP_V2_META_MESSAGE_TYPE
                        ) {
                            worker_frame_warp
                                .lock()
                                .unwrap()
                                .push((kind, payload.clone()));
                        }
                        if kind == butterpollo_media_policy::link_notice::LINK_NOTICE_MESSAGE_TYPE {
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
            kinds,
            notices,
            hdr_modes,
            text_notices,
            rate_answers,
            clipboard,
            frame_warp,
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
    pub async fn quiet_heartbeat(&self) {
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
    pub fn send_control(&self, kind: u16, payload: &[u8]) {
        let mut message = kind.to_le_bytes().to_vec();
        message.extend_from_slice(payload);
        self.outbox.lock().unwrap().push(message);
    }
    /// Sends a control message of `kind` encrypted with the launch's `key`, as these
    /// streams' version 1 control encryption has it; `seq` counts up from 0 per stream.
    pub fn send_encrypted(&self, key: &[u8; 16], seq: u32, kind: u16, payload: &[u8]) {
        let message =
            butterpollo_media_policy::packet::encrypted_control(key, seq, kind, payload, false)
                .unwrap();
        self.outbox.lock().unwrap().push(message);
    }
    /// Stops the video pings and returns once no round that could still send one is running.
    pub async fn stop_video_pings(&self) {
        self.video_down.store(true, Ordering::Release);
        let round = self.rounds.load(Ordering::Acquire);
        until("the client's last video ping sent", WAIT, || {
            self.rounds.load(Ordering::Acquire) > round
        })
        .await;
    }
    pub fn network(&self, down: bool) {
        self.control_down.store(down, Ordering::Release);
        self.video_down.store(down, Ordering::Release);
    }
    /// Moves the client to other sockets, as when its address changes: ENet and the
    /// video pings use `control` and `video` from the worker's next round.
    pub fn move_to(&self, control: UdpSocket, video: UdpSocket) {
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
