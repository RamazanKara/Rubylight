//! In-process loopback host (RTSP, ENet control with roaming, and media UDP sockets).

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::control::ptype;
use rubylight_protocol::roaming::{is_roaming, Datagram, Header, Payload};
use rubylight_wire::mic as host_mic;
use rubylight_wire::packet::{decrypt_control, encrypted_control};
use rubylight_wire::rtsp as host_rtsp;
use rusty_enet::{Event, Host, HostSettings, Packet, PacketReceived, MTU_MAX};

use crate::media::{KEY, PING};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Media {
    Video,
    Audio,
}

/// The fake host's media side, as the Rubylight host's: pings count only from the session's
/// client address, and a roam forgets the old path's peers until pings arrive on the new one.
#[derive(Default)]
pub(crate) struct MediaState {
    /// The session's client address: the RTSP client's, then where it roamed.
    pub(crate) client_ip: Option<IpAddr>,
    /// Where video and audio go: the sender of the last ping taken.
    pub(crate) peers: [Option<SocketAddr>; 2],
    /// The pings taken: stream, sender, sequence number.
    pub(crate) pings: Vec<(Media, SocketAddr, u32)>,
    /// Receive errors that ended a media socket's thread.
    pub(crate) errors: Vec<String>,
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
pub(crate) const CHALLENGE: u64 = 77;

pub(crate) struct FakeHost {
    pub(crate) rtsp_port: u16,
    pub(crate) video: UdpSocket,
    pub(crate) audio: UdpSocket,
    /// The microphone port, offered with roaming.
    pub(crate) mic: UdpSocket,
    /// Message types the control stream received, in order.
    pub(crate) control_messages: Arc<Mutex<Vec<u16>>>,
    /// When each input packet arrived.
    pub(crate) input_arrivals: Arc<Mutex<Vec<Instant>>>,
    pub(crate) control_disconnected: Arc<AtomicBool>,
    pub(crate) terminate: Arc<AtomicBool>,
    /// The roaming payloads the control port received, opened, with their senders.
    pub(crate) roaming: Arc<Mutex<Vec<(SocketAddr, Payload)>>>,
    /// The control stream stops servicing ENet while set.
    pub(crate) silent: Arc<AtomicBool>,
    pub(crate) media: Arc<Mutex<MediaState>>,
}

impl FakeHost {
    /// Waits for a ping the host takes on `media` and returns where it now sends that stream.
    pub(crate) fn peer(&self, media: Media) -> SocketAddr {
        self.peer_where(media, |_| true)
    }

    /// [`Self::peer`], once the stream's pings come from `ip`.
    pub(crate) fn peer_from(&self, media: Media, ip: IpAddr) -> SocketAddr {
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

pub(crate) fn start_host(packed_format: Option<u32>) -> FakeHost {
    start_host_with(packed_format, false)
}

/// With `roaming`, the host advertises client roaming, answers PROBE and CONFIRM, and offers
/// the microphone.
pub(crate) fn start_host_with(packed_format: Option<u32>, roaming: bool) -> FakeHost {
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
                        roaming,
                    );
                    if packed_format.is_some() {
                        describe.push_str(
                            "a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:15\r\n",
                        );
                    }
                    if roaming {
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
    let input_arrivals = Arc::new(Mutex::new(Vec::new()));
    let arrivals = input_arrivals.clone();
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
                        if kind == ptype::INPUT_DATA {
                            arrivals.lock().unwrap().push(Instant::now());
                        }
                        messages.lock().unwrap().push(kind);
                    }
                    Event::Disconnect { .. } => {
                        disconnected.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
            if let Some(id) = peer
                && terminating.swap(false, Ordering::SeqCst)
            {
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
        input_arrivals,
        control_disconnected,
        terminate,
        roaming: roamed,
        silent,
        media,
    }
}
