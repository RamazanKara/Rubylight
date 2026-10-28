//! A whole session against an in-process host on loopback: the Rubylight host's RTSP
//! helpers answer the handshake, a rusty_enet host plays the control stream with the host's
//! own sealing and answers roaming, and UDP sockets answer the video and audio pings with
//! packetized frames and Opus packets and take the microphone. Like the Rubylight host, it
//! takes media pings only from the session's client address, moves the session only on a
//! CONFIRM of its pending challenge, and forgets the old path's media peers when it does.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use butterpollo_core::mic as host_mic;
use butterpollo_core::packet::{decrypt_control, encrypted_control};
use butterpollo_core::rtsp as host_rtsp;
use nightjar_core::control::{ptype, ControlEvent};
use nightjar_core::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};
use nightjar_core::roaming::RoamEvent;
use nightjar_core::sdp::*;
use nightjar_core::session::{
    AudioRenderer, AudioSetup, DecodeStatus, HostInfo, Interrupt, Listener, Location, Session,
    NetworkHint, SessionConfig, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::video::{Frame, FrameType};
use rubylight_protocol::roaming::{is_roaming, Datagram, Header, Payload, SDP_ATTRIBUTE};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketReceived, MTU_MAX};

const KEY: [u8; 16] = *b"session-test-key";
const PING: &[u8; 16] = b"0123456789abcdef";
const PACKET_SIZE: usize = 1024;

/// A single-block video packetizer like the host's (see `video/tests.rs`).
struct Packetizer {
    seq: u16,
    spi: u32,
    frame: u32,
}

impl Packetizer {
    fn frame(&mut self, payload: &[u8], kind: u8) -> Vec<Vec<u8>> {
        let slice = PACKET_SIZE - 16;
        let mut data = vec![1, 0, 0, kind];
        let total = payload.len() + 8;
        let last = if total % slice == 0 {
            slice
        } else {
            total % slice
        };
        data.extend_from_slice(&(last as u16).to_le_bytes());
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(payload);
        let count = total.div_ceil(slice);
        let parity = (count * 20).div_ceil(100);
        let mut shards: Vec<Vec<u8>> = (0..count + parity)
            .map(|i| {
                let mut s = vec![0u8; PACKET_SIZE + 16];
                if i < count {
                    s[16..20].copy_from_slice(&((self.spi + i as u32) << 8).to_le_bytes());
                    s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
                    s[26] = 0x10;
                    let chunk =
                        &data[(i * slice).min(data.len())..((i + 1) * slice).min(data.len())];
                    s[32..32 + chunk.len()].copy_from_slice(chunk);
                }
                s
            })
            .collect();
        let rs = ReedSolomon::new(count, parity).unwrap();
        let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
        rs.encode(&mut refs, PACKET_SIZE + 16).unwrap();
        for (i, s) in shards.iter_mut().enumerate() {
            s[0] = 0x90;
            s[2..4].copy_from_slice(&self.seq.wrapping_add(i as u16).to_be_bytes());
            s[4..8].copy_from_slice(&(self.frame * 1500).to_be_bytes());
            s[20..24].copy_from_slice(&self.frame.to_le_bytes());
            let info = ((i as u32) << 12) | ((count as u32) << 22) | (20 << 4);
            s[28..32].copy_from_slice(&info.to_le_bytes());
        }
        self.seq = self.seq.wrapping_add((count + parity) as u16);
        self.spi += count as u32;
        self.frame += 1;
        shards
    }
}

fn h264_idr() -> Vec<u8> {
    let mut f = vec![
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80,
    ];
    f.extend_from_slice(&[0, 0, 0, 1, 0x65]);
    f.extend((0..3000u32).map(|i| (i % 251) as u8 | 1));
    f
}

fn h264_p() -> Vec<u8> {
    let mut f = vec![0, 0, 0, 1, 0x41];
    f.extend((0..2000u32).map(|i| (i % 249) as u8 | 1));
    f
}

/// Audio data packets with two parity packets after every fourth (see `audio/tests.rs`).
fn audio_stream(count: u16) -> Vec<Vec<u8>> {
    let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
    let mut out = Vec::new();
    let mut block: Vec<Vec<u8>> = Vec::new();
    for seq in 0..count {
        let payload: Vec<u8> = (0..60).map(|i| (i as u16 ^ seq) as u8).collect();
        let mut p = vec![0x80, 97];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&(u32::from(seq) * 5).to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(&payload);
        out.push(p);
        block.push(payload);
        if block.len() == 4 {
            let mut shards = block.clone();
            shards.push(vec![0; 60]);
            shards.push(vec![0; 60]);
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, 60).unwrap();
            for (index, shard) in shards[4..].iter().enumerate() {
                let mut p = vec![0x80, 127];
                p.extend_from_slice(&(seq + 1 + index as u16).to_be_bytes());
                p.extend_from_slice(&[0; 8]);
                p.push(index as u8);
                p.push(97);
                p.extend_from_slice(&(seq - 3).to_be_bytes());
                p.extend_from_slice(&(u32::from(seq - 3) * 5).to_be_bytes());
                p.extend_from_slice(&[0; 4]);
                p.extend_from_slice(shard);
                out.push(p);
            }
            block.clear();
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Media {
    Video,
    Audio,
}

/// The fake host's media side, as the Rubylight host's: pings count only from the session's
/// client address, and a roam forgets the old path's peers until pings arrive on the new one.
#[derive(Default)]
struct MediaState {
    /// The session's client address: the RTSP client's, then where it roamed.
    client_ip: Option<IpAddr>,
    /// Where video and audio go: the sender of the last ping taken.
    peers: [Option<SocketAddr>; 2],
    /// The pings taken: stream, sender, sequence number.
    pings: Vec<(Media, SocketAddr, u32)>,
    /// Receive errors that ended a media socket's thread.
    errors: Vec<String>,
}

/// Takes the pings that reach `socket`: 20 bytes, the RTSP payload and a sequence number.
fn serve_media(socket: UdpSocket, media: Media, state: Arc<Mutex<MediaState>>) {
    thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        loop {
            let (n, from) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                // A send to a port the client has closed since (Windows reports it here).
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => {
                    state.lock().unwrap().errors.push(format!("{media:?}: {e}"));
                    return;
                }
            };
            if n != 20 || &buffer[..16] != PING {
                continue;
            }
            let mut state = state.lock().unwrap();
            if state.client_ip != Some(from.ip()) {
                continue;
            }
            state.peers[media as usize] = Some(from);
            let sequence = u32::from_be_bytes(buffer[16..20].try_into().unwrap());
            state.pings.push((media, from, sequence));
        }
    });
}

/// The host's control socket, as the Rubylight host's: roaming datagrams are set aside, and
/// once a client roamed its new address stands in for the old one its ENet peer has.
struct Roamable {
    socket: UdpSocket,
    roaming: Vec<(SocketAddr, Vec<u8>)>,
    /// (new, old): where a roamed client is now, and the address ENet knows it by.
    moved: Option<(SocketAddr, SocketAddr)>,
}

impl rusty_enet::Socket for Roamable {
    type Address = SocketAddr;
    type Error = io::Error;

    fn init(&mut self, options: rusty_enet::SocketOptions) -> io::Result<()> {
        rusty_enet::Socket::init(&mut self.socket, options)
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        let to = match self.moved {
            Some((new, old)) if address == old => new,
            _ => address,
        };
        // A full send buffer drops the datagram, as rusty_enet's own UdpSocket does.
        rusty_enet::Socket::send(&mut self.socket, to, buffer)
    }

    fn receive(
        &mut self,
        buffer: &mut [u8; MTU_MAX],
    ) -> io::Result<Option<(SocketAddr, PacketReceived)>> {
        loop {
            match self.socket.recv_from(buffer) {
                Ok((length, from)) if is_roaming(&buffer[..length]) => {
                    self.roaming.push((from, buffer[..length].to_vec()));
                }
                Ok((length, from)) => {
                    let from = match self.moved {
                        Some((new, old)) if from == new => old,
                        _ => from,
                    };
                    return Ok(Some((from, PacketReceived::Complete(length))));
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn roam_gcm() -> aes_gcm::Aes128Gcm {
    use aes_gcm::KeyInit;
    let (key, _) = nightjar_core::roaming::derive(&KEY);
    aes_gcm::Aes128Gcm::new(&key.into())
}

fn roam_reply(
    socket: &UdpSocket,
    to: SocketAddr,
    gcm: &aes_gcm::Aes128Gcm,
    seq: u32,
    payload: Payload,
) {
    let (_, session_tag) = nightjar_core::roaming::derive(&KEY);
    let header = Header {
        kind: payload.kind(),
        session_tag,
        seq,
    };
    let datagram = nightjar_core::roaming::seal(gcm, header, payload);
    socket.send_to(&datagram, to).unwrap();
}

/// The challenge the fake host hands out.
const CHALLENGE: u64 = 77;

struct FakeHost {
    rtsp_port: u16,
    video: UdpSocket,
    audio: UdpSocket,
    /// The microphone port, offered with roaming.
    mic: UdpSocket,
    /// Message types the control stream received, in order.
    control_messages: Arc<Mutex<Vec<u16>>>,
    control_disconnected: Arc<AtomicBool>,
    terminate: Arc<AtomicBool>,
    /// The roaming payloads the control port received, opened, with their senders.
    roaming: Arc<Mutex<Vec<(SocketAddr, Payload)>>>,
    /// The control stream stops servicing ENet while set.
    silent: Arc<AtomicBool>,
    media: Arc<Mutex<MediaState>>,
}

impl FakeHost {
    /// Waits for a ping the host takes on `media` and returns where it now sends that stream.
    fn peer(&self, media: Media) -> SocketAddr {
        self.peer_where(media, |_| true)
    }

    /// [`Self::peer`], once the stream's pings come from `ip`.
    fn peer_from(&self, media: Media, ip: IpAddr) -> SocketAddr {
        self.peer_where(media, |peer| peer.ip() == ip)
    }

    fn peer_where(&self, media: Media, wanted: impl Fn(SocketAddr) -> bool) -> SocketAddr {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            {
                let state = self.media.lock().unwrap();
                if let Some(peer) = state.peers[media as usize].filter(|&p| wanted(p)) {
                    return peer;
                }
                let last = &state.pings[state.pings.len().saturating_sub(4)..];
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for a {media:?} ping: client {:?}, peers {:?}, last \
                     pings {last:?}, errors {:?}",
                    state.client_ip,
                    state.peers,
                    state.errors
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn start_host(packed_format: Option<u32>) -> FakeHost {
    start_host_with(packed_format, false)
}

/// With `roaming`, the host advertises client roaming, answers PROBE and CONFIRM, and offers
/// the microphone.
fn start_host_with(packed_format: Option<u32>, roaming: bool) -> FakeHost {
    let video = UdpSocket::bind("127.0.0.1:0").unwrap();
    let audio = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mic = UdpSocket::bind("127.0.0.1:0").unwrap();
    let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = (
        video.local_addr().unwrap().port(),
        audio.local_addr().unwrap().port(),
        control_socket.local_addr().unwrap().port(),
        mic.local_addr().unwrap().port(),
    );

    let media = Arc::new(Mutex::new(MediaState::default()));
    serve_media(video.try_clone().unwrap(), Media::Video, media.clone());
    serve_media(audio.try_clone().unwrap(), Media::Audio, media.clone());

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let rtsp_port = listener.local_addr().unwrap().port();
    let rtsp_media = media.clone();
    thread::spawn(move || {
        for socket in listener.incoming() {
            let mut socket = socket.unwrap();
            // The session's client address is the RTSP client's.
            let client = socket.peer_addr().unwrap().ip();
            rtsp_media.lock().unwrap().client_ip.get_or_insert(client);
            let mut message = host_rtsp::MessageBuffer::default();
            let mut chunk = [0u8; 4096];
            loop {
                let n = socket.read(&mut chunk).unwrap();
                assert!(n > 0, "client closed mid-request");
                if message.append(&chunk[..n]).unwrap() {
                    break;
                }
            }
            let request = host_rtsp::Request::parse(&message.into_bytes()).unwrap();
            let mut headers: Vec<(&str, String)> = Vec::new();
            let mut body = Vec::new();
            match request.method.as_str() {
                "DESCRIBE" => {
                    // Control encryption v2 only: plain audio and video.
                    let mut describe = host_rtsp::describe(
                        0,
                        1,
                        0,
                        false,
                        packed_format.is_some(),
                        false,
                        roaming,
                    );
                    if packed_format.is_some() {
                        describe.push_str(
                            "a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:15\r\n",
                        );
                    }
                    if roaming {
                        describe.push_str(&format!("{SDP_ATTRIBUTE}\r\n"));
                        describe.push_str(&host_mic::sdp(ports.3));
                    }
                    body = describe.into_bytes();
                    headers.push(("Content-Type", "application/sdp".into()));
                }
                "SETUP" => {
                    let (port, control) = if request.target.contains("=audio") {
                        (ports.1, false)
                    } else if request.target.contains("=video") {
                        (ports.0, false)
                    } else if request.target.contains("=mic") {
                        (ports.3, false)
                    } else {
                        (ports.2, true)
                    };
                    headers.push(("Session", "DEADBEEFCAFE;timeout = 90".into()));
                    headers.push(("Transport", format!("server_port={port}")));
                    if control {
                        headers.push(("X-SS-Connect-Data", "305419896".into()));
                    } else {
                        headers.push(("X-SS-Ping-Payload", "0123456789abcdef".into()));
                    }
                }
                _ => {}
            }
            let response = host_rtsp::response(request.cseq, 200, "OK", &headers, &body);
            socket.write_all(&response).unwrap();
        }
    });

    let control_messages = Arc::new(Mutex::new(Vec::new()));
    let control_disconnected = Arc::new(AtomicBool::new(false));
    let terminate = Arc::new(AtomicBool::new(false));
    let roamed = Arc::new(Mutex::new(Vec::new()));
    let silent = Arc::new(AtomicBool::new(false));
    let (messages, disconnected, terminating, roam_log, silenced, control_media) = (
        control_messages.clone(),
        control_disconnected.clone(),
        terminate.clone(),
        roamed.clone(),
        silent.clone(),
        media.clone(),
    );
    thread::spawn(move || {
        let settings = HostSettings {
            peer_limit: 4,
            channel_limit: 255,
            ..Default::default()
        };
        let socket = Roamable {
            socket: control_socket,
            roaming: Vec::new(),
            moved: None,
        };
        let mut host = Host::new(socket, settings).unwrap();
        let mut peer = None;
        let mut sequence = 0u32;
        let gcm = roam_gcm();
        let mut roam_seq = 0u32;
        // Roaming as the host's `Roaming`: the client's control address, its last seq, the
        // challenge (address, nonce_c, challenge) pending for a new address and the one for
        // a probe from the current address, and the challenge the last move committed.
        let mut current = None;
        let mut client_seq = 0u32;
        let mut challenges = CHALLENGE..;
        let mut pending: Option<(SocketAddr, u64, u64)> = None;
        let mut here: Option<(SocketAddr, u64, u64)> = None;
        let mut committed = None;
        loop {
            if silenced.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            while let Some(event) = host.service().unwrap() {
                match event {
                    Event::Connect { peer: p, data } => {
                        assert_eq!(data, 305419896);
                        peer = Some(p.id());
                        current = p.address();
                    }
                    Event::Receive { packet, .. } => {
                        let (_, kind, _) = decrypt_control(&KEY, packet.data(), true).unwrap();
                        messages.lock().unwrap().push(kind);
                    }
                    Event::Disconnect { .. } => {
                        disconnected.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
            if let Some(id) = peer {
                if terminating.swap(false, Ordering::SeqCst) {
                    sequence += 1;
                    // 0x0109 with a four-byte reason: 0x80030023 ends gracefully.
                    let datagram = encrypted_control(
                        &KEY,
                        sequence,
                        0x0109,
                        &0x8003_0023u32.to_be_bytes(),
                        true,
                    )
                    .unwrap();
                    host.peer_mut(id)
                        .send(0, &Packet::reliable(datagram))
                        .unwrap();
                    host.flush();
                }
            }
            // A CHALLENGE for each PROBE, the same one for a repeat of a burst; a CONFIRM of
            // the challenge pending for its address moves the session, then MIGRATED. Anything
            // malformed, unauthentic, replayed or unmatched is dropped, as the host drops it.
            for (from, data) in std::mem::take(&mut host.socket_mut().roaming) {
                let Ok(datagram) = Datagram::decode(&data) else {
                    continue;
                };
                let Some(payload) = nightjar_core::roaming::open(&gcm, &datagram) else {
                    continue;
                };
                if datagram.header.seq <= client_seq {
                    continue;
                }
                client_seq = datagram.header.seq;
                roam_log.lock().unwrap().push((from, payload));
                let (Some(id), Some(at)) = (peer, current) else {
                    continue;
                };
                let reply = match payload {
                    Payload::Probe { nonce_c, .. } => {
                        let slot = if from == at { &mut here } else { &mut pending };
                        let challenge = match *slot {
                            Some((address, nonce, challenge))
                                if address == from && nonce == nonce_c =>
                            {
                                challenge
                            }
                            _ => {
                                let challenge = challenges.next().unwrap();
                                *slot = Some((from, nonce_c, challenge));
                                challenge
                            }
                        };
                        Payload::Challenge { nonce_c, challenge }
                    }
                    // From where the session is: a repeat of the last move's CONFIRM, or one
                    // answering a probe from here, hears MIGRATED and moves nothing.
                    Payload::Confirm { challenge } if from == at => {
                        if committed != Some(challenge) {
                            if !matches!(here, Some((_, _, c)) if c == challenge) {
                                continue;
                            }
                            here = None;
                        }
                        Payload::Migrated { challenge }
                    }
                    Payload::Confirm { challenge } => {
                        if !matches!(pending, Some((a, _, c)) if a == from && c == challenge) {
                            continue;
                        }
                        pending = None;
                        // ENet keeps the address it first knew the client by.
                        let old = host.peer(id).address().unwrap();
                        host.socket_mut().moved = Some((from, old));
                        current = Some(from);
                        committed = Some(challenge);
                        let mut media = control_media.lock().unwrap();
                        media.client_ip = Some(from.ip());
                        media.peers = [None; 2];
                        Payload::Migrated { challenge }
                    }
                    _ => continue,
                };
                roam_seq += 1;
                roam_reply(&host.socket().socket, from, &gcm, roam_seq, reply);
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    FakeHost {
        rtsp_port,
        video,
        audio,
        mic,
        control_messages,
        control_disconnected,
        terminate,
        roaming: roamed,
        silent,
        media,
    }
}

#[derive(Default)]
struct Record {
    setup: Option<VideoSetup>,
    frames: Vec<(u32, FrameType)>,
    audio_setup: Option<AudioSetup>,
    audio: Vec<Option<Vec<u8>>>,
    stages: Vec<Stage>,
    started: bool,
    terminated: Option<i32>,
    events: Vec<ControlEvent>,
    lifecycle: Vec<&'static str>,
    /// What the listener and the renderers saw, in order, up to the first frame.
    startup: Vec<String>,
    /// The DESCRIBE's packed modes as the listener got them, and whether the decoder was set up.
    packed_modes: Option<(u8, bool)>,
    logs: Vec<String>,
}

struct Recorder {
    record: Arc<Mutex<Record>>,
    need_idr_at: Option<u32>,
}

impl VideoDecoder for Recorder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.record.lock().unwrap().setup = Some(*setup);
        Ok(())
    }

    fn start(&mut self) {
        self.record.lock().unwrap().startup.push("video start".into());
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        self.record
            .lock()
            .unwrap()
            .frames
            .push((frame.number, frame.frame_type));
        if Some(frame.number) == self.need_idr_at {
            DecodeStatus::NeedIdr
        } else {
            DecodeStatus::Ok
        }
    }

    fn stop(&mut self) {
        self.record.lock().unwrap().lifecycle.push("video stop");
    }

    fn cleanup(&mut self) {
        self.record.lock().unwrap().lifecycle.push("video cleanup");
    }
}

impl AudioRenderer for Recorder {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        self.record.lock().unwrap().audio_setup = Some(*setup);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        self.record
            .lock()
            .unwrap()
            .audio
            .push(packet.map(<[u8]>::to_vec));
    }

    fn cleanup(&mut self) {
        self.record.lock().unwrap().lifecycle.push("audio cleanup");
    }
}

impl Listener for Recorder {
    fn stage_complete(&self, stage: Stage) {
        self.record.lock().unwrap().stages.push(stage);
    }

    fn host_control_messages(&self, messages: &[u16]) {
        self.record
            .lock()
            .unwrap()
            .startup
            .push(format!("control messages {messages:x?}"));
    }

    fn host_packed444_modes(&self, modes: u8) {
        let mut r = self.record.lock().unwrap();
        r.packed_modes = Some((modes, r.setup.is_some()));
    }

    fn log(&self, message: &str) {
        self.record.lock().unwrap().logs.push(message.into());
    }

    fn connection_started(&self) {
        self.record.lock().unwrap().started = true;
    }

    fn connection_terminated(&self, error: i32) {
        self.record.lock().unwrap().terminated = Some(error);
    }

    fn control_event(&self, event: ControlEvent) {
        self.record.lock().unwrap().events.push(event);
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn run_session(direct_submit: bool, packed_format: Option<u32>) {
    let host = start_host(packed_format);
    let record = Arc::new(Mutex::new(Record::default()));
    let recorder = |need_idr_at| {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at,
        })
    };
    let config = SessionConfig {
        stream: StreamConfig {
            width: 1280,
            height: 720,
            fps: 60,
            packet_size: PACKET_SIZE as i32,
            remote_input_aes_key: KEY,
            experimental_packed_444: packed_format.is_some(),
            packed444_modes: if packed_format.is_some() { 15 } else { 0 },
            supported_video_formats: packed_format.unwrap_or(VIDEO_FORMAT_H264),
            video_capabilities: if direct_submit {
                CAPABILITY_DIRECT_SUBMIT
            } else {
                0
            },
            ..Default::default()
        },
        location: Location::Auto,
        ..Default::default()
    };
    let session = Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        config,
        recorder(Some(3)),
        recorder(None),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap();
    {
        let r = record.lock().unwrap();
        assert!(r.started);
        assert_eq!(r.stages.len(), 11);
        // Before the decoder's setup, which names from them why 10-bit packed went 8-bit.
        assert_eq!(
            r.packed_modes,
            Some((if packed_format.is_some() { 15 } else { 0 }, false))
        );
        assert_eq!(
            r.setup,
            Some(VideoSetup {
                format: packed_format.unwrap_or(VIDEO_FORMAT_H264),
                width: 1280,
                height: if packed_format.is_some() { 1440 } else { 720 },
                fps: 60,
                chroma_packing: u8::from(packed_format.is_some()),
                logical_width: 1280,
                logical_height: 720,
                color_space: nightjar_core::sdp::COLORSPACE_REC_709,
                color_range: nightjar_core::sdp::COLOR_RANGE_LIMITED,
            })
        );
        let audio = r.audio_setup.unwrap();
        assert_eq!(
            (audio.opus.channel_count, audio.samples_per_frame),
            (2, 240)
        );
    }
    // Loopback is not private, so Auto streams as remote with 1024-byte packets.
    assert_eq!(session.outcome().config.location, StreamLocation::Remote);
    assert_eq!(session.outcome().config.packet_size, 1024);

    // Video: an IDR and P frames; the decoder needs a key frame at frame 3, so 4 and 5 are
    // dropped and the next IDR (6) recovers.
    let client = host.peer(Media::Video);
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let idr = if packed_format.is_some() {
        include_bytes!("../../../app/src/main/assets/packed444/av1-8.bin").to_vec()
    } else {
        h264_idr()
    };
    let p = if packed_format.is_some() {
        vec![0x5a; 2500]
    } else {
        h264_p()
    };
    let frames = [(&idr, 2), (&p, 1), (&p, 1), (&p, 1), (&p, 1)];
    for (payload, kind) in &frames {
        for (index, packet) in packetizer.frame(payload, *kind).into_iter().enumerate() {
            if packed_format.is_some() && index == 1 {
                continue;
            }
            host.video.send_to(&packet, client).unwrap();
        }
        thread::sleep(Duration::from_millis(20));
    }
    until("the key frame request", || {
        host.control_messages
            .lock()
            .unwrap()
            .iter()
            .filter(|&&t| t == ptype::REQUEST_IDR_FRAME)
            .count()
            >= 2
    });
    for packet in packetizer.frame(&idr, 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("the recovery frame", || {
        record.lock().unwrap().frames.len() >= 4
    });

    // Audio: 100 packets of backlog are dropped, the rest play in order.
    let client = host.peer(Media::Audio);
    for packet in audio_stream(200) {
        host.audio.send_to(&packet, client).unwrap();
        thread::sleep(Duration::from_micros(200));
    }
    until("audio", || record.lock().unwrap().audio.len() >= 90);

    let stats = session.stats();
    assert!(stats.rtt.is_some());
    assert_eq!(stats.frames_submitted, 4);
    assert_eq!(stats.frames_need_idr, 1);

    // The host ends the stream: the listener hears it once, then the app stops.
    host.terminate.store(true, Ordering::SeqCst);
    until("termination", || {
        record.lock().unwrap().terminated.is_some()
    });
    session.stop();

    let r = record.lock().unwrap();
    assert_eq!(
        r.frames,
        [
            (1, FrameType::Idr),
            (2, FrameType::P),
            (3, FrameType::P),
            (6, FrameType::Idr)
        ]
    );
    assert!(r.audio.iter().all(Option::is_some));
    // The renderers' start can already ask what the host announced (the Android engine runs the
    // clock sync for the timing CSV only for a host that announced it, and decides in `start`).
    assert_eq!(
        r.startup.first().map(String::as_str),
        Some("control messages [5530, 5531, 5532, 5533, 5534, 5535, 5536, 5537]"),
        "{:?}",
        r.startup
    );
    assert_eq!(r.startup.get(1).map(String::as_str), Some("video start"));
    assert_eq!(r.audio[0].as_deref(), Some(&audio_stream(200)[150][12..]));
    assert_eq!(r.terminated, Some(0));
    assert_eq!(
        r.lifecycle,
        ["audio cleanup", "video stop", "video cleanup"]
    );
    let messages = host.control_messages.lock().unwrap();
    assert!(messages.contains(&ptype::START_B));
    assert!(messages.contains(&ptype::PERIODIC_PING));
    assert!(messages.contains(&ptype::INPUT_DATA));
}

#[test]
fn a_session_streams_recovers_and_ends_direct_submit() {
    run_session(true, None);
}

#[test]
fn a_session_streams_recovers_and_ends_through_the_decode_queue() {
    run_session(false, None);
}

#[test]
fn packed_chroma_recovers_packet_loss_and_key_frames_with_logical_decoder_geometry() {
    run_session(false, Some(VIDEO_FORMAT_AV1_MAIN8));
}

#[test]
fn stop_disconnects_the_control_stream() {
    let host = start_host(None);
    let record = Arc::new(Mutex::new(Record::default()));
    let make = || {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        })
    };
    let session = Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        SessionConfig {
            stream: StreamConfig {
                remote_input_aes_key: KEY,
                ..Default::default()
            },
            location: Location::Local,
            input_only: true,
            ..Default::default()
        },
        make(),
        make(),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap();
    assert_eq!(session.outcome().config.packet_size, 1392);
    session
        .input(|q| q.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    let r = record.lock().unwrap();
    assert_eq!(r.terminated, None);
    assert!(r.setup.is_none() && r.audio_setup.is_none());
}

/// Vibepollo and Apollo hosts run their video thread for an input-only session too and end it
/// when no video ping arrives within ten seconds.
#[test]
fn input_only_pings_the_video_port_until_stop() {
    let host = start_host(None);
    let record = Arc::new(Mutex::new(Record::default()));
    let make = || {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        })
    };
    let session = Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        SessionConfig {
            stream: StreamConfig {
                remote_input_aes_key: KEY,
                ..Default::default()
            },
            location: Location::Local,
            input_only: true,
            ..Default::default()
        },
        make(),
        make(),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap();
    // The payload from the RTSP SETUP and a sequence number that counts up.
    let video_pings = || -> Vec<u32> {
        let media = host.media.lock().unwrap();
        let pings = media.pings.iter().filter(|(m, ..)| *m == Media::Video);
        pings.map(|&(_, _, sequence)| sequence).collect()
    };
    until("two video pings", || video_pings().len() >= 2);
    assert_eq!(video_pings()[..2], [1, 2]);
    // The session is up and decoder and renderer were never touched.
    session
        .input(|q| q.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    {
        let r = record.lock().unwrap();
        assert!(r.setup.is_none() && r.audio_setup.is_none());
    }
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    // Stop joined the ping thread: whatever was in flight is drained, then it is quiet.
    thread::sleep(Duration::from_millis(50));
    let pinged = video_pings().len();
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(video_pings().len(), pinged, "pings continued after stop");
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// Waits for a microphone packet from `ip` and returns the Opus packet inside.
fn await_mic_from(socket: &UdpSocket, ip: IpAddr) -> Vec<u8> {
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0u8; 2048];
    loop {
        let (n, from) = socket.recv_from(&mut buffer).expect("a microphone packet");
        if from.ip() == ip {
            let packet = host_mic::parse(&buffer[..n]).expect("a microphone packet");
            return host_mic::open(&KEY, 0, &packet).unwrap();
        }
    }
}

/// A 720p60 stream with the microphone, in 1024-byte packets as on loopback.
fn roaming_config() -> SessionConfig {
    SessionConfig {
        stream: StreamConfig {
            width: 1280,
            height: 720,
            fps: 60,
            packet_size: PACKET_SIZE as i32,
            remote_input_aes_key: KEY,
            microphone: true,
            ..Default::default()
        },
        location: Location::Auto,
        ..Default::default()
    }
}

fn start_session(host: &FakeHost, config: SessionConfig, record: &Arc<Mutex<Record>>) -> Session {
    let recorder = || {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        })
    };
    Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        config,
        recorder(),
        recorder(),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap()
}

fn roam_events(record: &Mutex<Record>) -> Vec<RoamEvent> {
    record
        .lock()
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match e {
            ControlEvent::Roam(event) => Some(*event),
            _ => None,
        })
        .collect()
}

fn count(messages: &Mutex<Vec<u16>>, kind: u16) -> usize {
    messages
        .lock()
        .unwrap()
        .iter()
        .filter(|&&t| t == kind)
        .count()
}

/// Two local addresses to roam between: 127.0.0.1 and 127.0.0.2.
fn loopback_addresses() -> Option<(IpAddr, IpAddr)> {
    let (old, new) = (
        IpAddr::from(Ipv4Addr::LOCALHOST),
        IpAddr::from(Ipv4Addr::new(127, 0, 0, 2)),
    );
    if UdpSocket::bind((new, 0)).is_err() {
        // Windows answers on all of 127.0.0.0/8; elsewhere it may take an alias.
        if cfg!(windows) {
            panic!("{new} cannot be bound");
        }
        eprintln!("skipped: {new} cannot be bound here");
        return None;
    }
    Some((old, new))
}

/// A roam onto another local address: PROBE, CONFIRM and MIGRATED move the control stream,
/// and video, audio and the microphone carry on to and from the new address.
#[test]
fn a_session_roams_control_and_media_to_a_new_address() {
    let Some((old, new)) = loopback_addresses() else {
        return;
    };
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    assert!(session.microphone_available());

    // Before the roam everything runs from the first address.
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let client = host.peer_from(Media::Video, old);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame", || record.lock().unwrap().frames.len() == 1);
    let audio = audio_stream(1300);
    let (before, after) = audio.split_at(300);
    let client = host.peer_from(Media::Audio, old);
    for packet in before {
        host.audio.send_to(packet, client).unwrap();
        thread::sleep(Duration::from_micros(200));
    }
    until("audio", || record.lock().unwrap().audio.len() >= 90);
    session.send_microphone(b"before").unwrap();
    assert_eq!(await_mic_from(&host.mic, old), b"before");

    session.roam_to(Some(new)).unwrap();
    until("the roam", || {
        roam_events(&record)
            .iter()
            .any(|e| matches!(e, RoamEvent::Roamed { .. }))
    });
    let roaming = host.roaming.lock().unwrap().clone();
    assert!(
        matches!(roaming[0].1, Payload::Probe { flags: 1, .. }),
        "a PROBE with the media rebound flag first: {roaming:?}"
    );
    assert!(roaming.contains(&(
        roaming[0].0,
        Payload::Confirm {
            challenge: CHALLENGE
        }
    )));
    assert!(
        roaming.iter().all(|(from, _)| from.ip() == new),
        "{roaming:?}"
    );

    // Video, audio and the microphone resume on the new address.
    let client = host.peer_from(Media::Video, new);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame after the roam", || {
        record.lock().unwrap().frames.len() == 2
    });
    let played = record.lock().unwrap().audio.len();
    let client = host.peer_from(Media::Audio, new);
    // Sent as a host streams, until enough of it plays: one burst could all
    // land before a slow (sanitizer) client reads the new socket, and the
    // wait then timed out with nothing more coming.
    let mut packets = after.iter();
    until("audio after the roam", || {
        if let Some(packet) = packets.next() {
            host.audio.send_to(packet, client).unwrap();
        }
        record.lock().unwrap().audio.len() >= played + 50
    });
    session.send_microphone(b"after").unwrap();
    assert_eq!(await_mic_from(&host.mic, new), b"after");
    // So does the control stream.
    input_arrives(&session, &host);

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    let r = record.lock().unwrap();
    assert_eq!(r.frames, [(1, FrameType::Idr), (2, FrameType::Idr)]);
    assert!(r.audio.iter().all(Option::is_some));
    assert_eq!(r.terminated, None);
}

/// The host drops media pings from the new address until MIGRATED, so on Roamed video and
/// audio ping at once rather than an interval (500 ms) later.
#[test]
fn media_ping_from_the_new_address_at_once_on_roamed() {
    let Some((old, new)) = loopback_addresses() else {
        return;
    };
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    host.peer_from(Media::Video, old);
    host.peer_from(Media::Audio, old);

    session.roam_to(Some(new)).unwrap();
    until("the roam", || {
        roam_events(&record)
            .iter()
            .any(|e| matches!(e, RoamEvent::Roamed { .. }))
    });
    let roamed = Instant::now();
    let pinged_from_new = |media: Media| {
        let state = host.media.lock().unwrap();
        state
            .pings
            .iter()
            .any(|&(m, from, _)| m == media && from.ip() == new)
    };
    until("video and audio pings from the new address", || {
        pinged_from_new(Media::Video) && pinged_from_new(Media::Audio)
    });
    let waited = roamed.elapsed();
    assert!(
        waited < Duration::from_millis(250),
        "the media pinged {} ms after Roamed",
        waited.as_millis()
    );

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// A network change that leaves the route toward the host where it was (loopback stays on
/// 127.0.0.1) is spurious: nothing moves and nothing is probed.
#[test]
fn a_network_change_on_the_same_address_does_not_roam() {
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    let client = host.peer(Media::Video);

    assert!(!session.network_changed(NetworkHint::default()));
    // The control stream carries on, the host saw no PROBE and video stays where it was.
    input_arrives(&session, &host);
    assert_eq!(*host.roaming.lock().unwrap(), []);
    assert!(roam_events(&record).is_empty());
    assert_eq!(host.peer(Media::Video), client);
    assert!(
        record
            .lock()
            .unwrap()
            .logs
            .iter()
            .any(|l| l.contains("still on 127.0.0.1; not roaming")),
        "{:?}",
        record.lock().unwrap().logs
    );

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// The host takes media pings only from the address the control stream roamed to: media left
/// behind by a control socket that moved alone follow it on Roamed, and video carries on.
#[test]
fn media_left_behind_follow_the_control_stream_on_roamed() {
    let Some((old, new)) = loopback_addresses() else {
        return;
    };
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let client = host.peer_from(Media::Video, old);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame", || record.lock().unwrap().frames.len() == 1);

    session.roam_control_only(new).unwrap();
    until("the roam", || {
        roam_events(&record)
            .iter()
            .any(|e| matches!(e, RoamEvent::Roamed { .. }))
    });
    let client = host.peer_from(Media::Video, new);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame after the roam", || {
        record.lock().unwrap().frames.len() == 2
    });
    let _ = host.peer_from(Media::Audio, new);
    session.send_microphone(b"after").unwrap();
    assert_eq!(await_mic_from(&host.mic, new), b"after");

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    let r = record.lock().unwrap();
    assert!(
        r.logs.iter().any(|l| l.contains("the media were left on")),
        "{:?}",
        r.logs
    );
    assert_eq!(r.terminated, None);
}

/// Silences the host's control stream until the client holds input back, past
/// `INPUT_CONTACT_LOST_AFTER` (which needs a network grace period), when a silence starts
/// probing with roaming on; then until the client hears the host again, which by then has
/// read whatever was sent to it meanwhile.
fn silence_host(host: &FakeHost, session: &Session) {
    host.silent.store(true, Ordering::SeqCst);
    until("the client to notice the silence", || {
        session.stats().input_contact_lost
    });
    host.silent.store(false, Ordering::SeqCst);
    until("the client to hear the host again", || {
        !session.stats().input_contact_lost
    });
}

/// Input after the roaming steps reaches the host: the control stream is up.
fn input_arrives(session: &Session, host: &FakeHost) {
    let inputs = count(&host.control_messages, ptype::INPUT_DATA);
    session
        .input(|q| q.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    until("input", || {
        count(&host.control_messages, ptype::INPUT_DATA) > inputs
    });
}

/// On a metered network without "Continue on mobile data" the client neither moves nor
/// probes, on the hint or through a silence after it: the network grace period decides,
/// and the stream rides the silence out where it was.
#[test]
fn on_a_metered_network_without_the_opt_in_the_grace_period_decides() {
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let config = SessionConfig {
        network_grace: Duration::from_secs(20),
        ..roaming_config()
    };
    let session = start_session(&host, config, &record);
    let client = host.peer(Media::Video);

    assert!(!session.network_changed(NetworkHint { metered: true }));
    until("the hold", || roam_events(&record) == [RoamEvent::Held]);
    silence_host(&host, &session);
    // The control stream carries on; the host has taken everything sent before this input.
    input_arrives(&session, &host);
    {
        let r = record.lock().unwrap();
        assert!(
            r.logs
                .iter()
                .any(|l| l.contains("the network grace period decides")),
            "{:?}",
            r.logs
        );
    }
    assert_eq!(roam_events(&record), [RoamEvent::Held]);
    assert_eq!(*host.roaming.lock().unwrap(), []);

    // The same video port carries on.
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame", || record.lock().unwrap().frames.len() == 1);
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// With client roaming off, a host that advertises it is never probed: not on a network
/// change, not on a simulated roam, not through a silence.
#[test]
fn with_roaming_off_nothing_is_probed() {
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let mut config = SessionConfig {
        network_grace: Duration::from_secs(20),
        ..roaming_config()
    };
    config.roaming.enabled = false;
    let session = start_session(&host, config, &record);
    assert!(
        record
            .lock()
            .unwrap()
            .logs
            .iter()
            .any(|l| l == "Roaming: off in the client's settings"),
        "{:?}",
        record.lock().unwrap().logs
    );

    assert!(!session.network_changed(NetworkHint::default()));
    for local in [None, Some(IpAddr::from(Ipv4Addr::LOCALHOST))] {
        let refused = session.roam_to(local).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::Unsupported, "{refused}");
    }
    silence_host(&host, &session);
    input_arrives(&session, &host);

    assert!(roam_events(&record).is_empty());
    assert_eq!(*host.roaming.lock().unwrap(), []);
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}
