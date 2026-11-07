//! Telling a PC that is gone from one that is only quiet.
//!
//! The control stream's own timeout (10 s, or a chosen grace period) has to outlast every Wi-Fi
//! dropout, so a PC that shut down or crashed leaves a frozen picture for that long. Silence alone
//! can't say more, but once the control stream has been silent for [`PROBE_AFTER`] (its pings go
//! out several times a second, and a stalled encoder or a display switch does not stop them), two
//! connection attempts can:
//!
//! - Toward the PC's HTTPS port, which answered when the stream started. An accepted connection
//!   means the PC is there, however stalled its streaming is, and the timeout decides. A refusal
//!   comes from the PC itself: its server crashed or restarts, and the connection's own recovery
//!   takes it back.
//! - Without an answer: toward this device's own gateway, when the PC is on the same link
//!   ([`Round::SilentLinkUp`]). A gateway that answers while the PC does not means this device's
//!   network is fine. A dropout takes the gateway with it, and a gateway that keeps quiet, a VPN or
//!   a PC behind a router proves nothing: the timeout decides those.
//!
//! A server that crashed reads as no answer too (Windows drops connections to a closed port
//! without a word), so the PC itself is asked with a [`ping`] in each round: a PC that answers is
//! there and restarting, and only one that answered a ping when the stream started and has stopped
//! answering is gone. A PC that never answers pings (its firewall drops them) is left to the
//! timeout. A host that says goodbye ends the stream on purpose; the clients tell that apart
//! without any of this.
//!
//! The rounds run only during a silence, and a word from the host starts over. The Android app and
//! the desktop client both decide with [`Detector`].

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A control stream silent this long gets probed. Shorter silences are normal on Wi-Fi.
pub const PROBE_AFTER: Duration = Duration::from_secs(2);
/// A PC that merely doesn't answer, on a link that works, counts as gone after this silence.
pub const NO_ANSWER_AFTER: Duration = Duration::from_secs(4);
/// How long one round waits for its answers, the ping's included.
pub const ROUND_TIMEOUT: Duration = Duration::from_millis(750);
/// How long the ping at the start of a stream waits.
pub const FIRST_PING_TIMEOUT: Duration = Duration::from_millis(1000);
/// Rounds in a row without an answer, on a link that works, that make a PC gone: a link that
/// just came back reaches the PC within them.
const SILENT_ROUNDS: u8 = 3;
/// [`Detector::state`]'s bit for a PC that answered the first ping.
const PINGS_BIT: i32 = 1 << 8;
/// Where a gateway may answer: an accepted or refused connection both prove it is reachable.
const GATEWAY_PORTS: [u16; 3] = [53, 80, 443];

/// What one round of connection attempts found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    /// The PC accepted a connection: it is there.
    Answered,
    /// The PC refused one: it is there, its server is not.
    Refused,
    /// The PC did not answer, and this device's gateway did.
    SilentLinkUp,
    /// The PC did not answer, and nothing says this device's network works.
    SilentLinkUnknown,
}

impl Round {
    const ALL: [Round; 4] = [Round::Answered, Round::Refused, Round::SilentLinkUp, Round::SilentLinkUnknown];

    /// The round as a number, for a caller that passes it across a JNI call.
    pub fn code(self) -> i32 {
        Self::ALL.iter().position(|r| *r == self).unwrap_or(3) as i32
    }

    /// A code that is no round proves nothing.
    pub fn from_code(code: i32) -> Round {
        usize::try_from(code).ok().and_then(|i| Self::ALL.get(i).copied()).unwrap_or(Round::SilentLinkUnknown)
    }
}

/// Decides from the control stream's silence and the rounds that it brought on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Detector {
    silent: u8,
    /// The PC answered a ping when the stream started: a ping it no longer answers means something.
    pings: bool,
}

impl Detector {
    /// `pings`: whether the PC answered the ping at the start of the stream. Without that it is
    /// never gone here.
    pub fn new(pings: bool) -> Self {
        Self { silent: 0, pings }
    }

    /// Whether this silence calls for a round. Below it, the detector also starts over.
    pub fn wants_round(&mut self, silence: Duration) -> bool {
        if silence < PROBE_AFTER {
            self.silent = 0;
            return false;
        }
        true
    }

    /// Takes a round's result and whether the PC answered its ping (None: not asked or unknown);
    /// true when the PC is gone.
    pub fn step(&mut self, silence: Duration, round: Round, pc_pings: Option<bool>) -> bool {
        if round == Round::SilentLinkUp && self.pings && pc_pings == Some(false) {
            self.silent = self.silent.saturating_add(1);
            return self.silent >= SILENT_ROUNDS && silence >= NO_ANSWER_AFTER;
        }
        // The PC answered somehow (it is there, maybe restarting), or nothing can tell.
        self.silent = 0;
        false
    }

    /// Whether a round should ping the PC: only where an unanswered ping can decide.
    pub fn wants_ping(&self) -> bool {
        self.pings
    }

    /// The state as a number, for a caller that keeps it across a JNI call (never negative).
    pub fn state(&self) -> i32 {
        let pings = if self.pings { PINGS_BIT } else { 0 };
        pings | i32::from(self.silent)
    }

    pub fn from_state(state: i32) -> Self {
        Self {
            silent: state as u8,
            pings: state & PINGS_BIT != 0,
        }
    }
}

enum Answer {
    Host(io::Result<()>),
    Gateway,
}

fn is_answer(result: &io::Result<TcpStream>) -> bool {
    match result {
        Ok(_) => true,
        Err(error) => error.kind() == io::ErrorKind::ConnectionRefused,
    }
}

/// One round: blocks for at most [`ROUND_TIMEOUT`], a little longer if thread start is slow.
pub fn probe_round(host: SocketAddr, gateway: Option<IpAddr>) -> Round {
    let (sender, answers) = mpsc::channel();
    let to_host = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("Host check".into())
        .spawn(move || {
            let result = TcpStream::connect_timeout(&host, ROUND_TIMEOUT).map(|_| ());
            let _ = to_host.send(Answer::Host(result));
        });
    if spawned.is_err() {
        return Round::SilentLinkUnknown;
    }
    for ip in gateway.into_iter().filter(|ip| *ip != host.ip()) {
        for port in GATEWAY_PORTS {
            let to_gateway = sender.clone();
            let _ = std::thread::Builder::new()
                .name("Link check".into())
                .spawn(move || {
                    let target = SocketAddr::new(ip, port);
                    if is_answer(&TcpStream::connect_timeout(&target, ROUND_TIMEOUT)) {
                        let _ = to_gateway.send(Answer::Gateway);
                    }
                });
        }
    }
    drop(sender);
    decide(&answers, Instant::now() + ROUND_TIMEOUT)
}

fn decide(answers: &mpsc::Receiver<Answer>, deadline: Instant) -> Round {
    let mut host_silent = false;
    let mut link_up = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match answers.recv_timeout(left) {
            Ok(Answer::Host(Ok(()))) => return Round::Answered,
            Ok(Answer::Host(Err(error))) if error.kind() == io::ErrorKind::ConnectionRefused => {
                return Round::Refused;
            }
            Ok(Answer::Host(Err(_))) => host_silent = true,
            Ok(Answer::Gateway) => link_up = true,
            // The deadline, or every attempt is done.
            Err(_) => break,
        }
        if host_silent && link_up {
            break;
        }
    }
    if link_up {
        Round::SilentLinkUp
    } else {
        Round::SilentLinkUnknown
    }
}

/// After the host said goodbye: whether its server went with it (shutting down) rather than only
/// this stream ending (a host that still accepts connections, its session over or the stream
/// closed). One round.
pub fn server_left(host: SocketAddr, gateway: Option<IpAddr>) -> bool {
    probe_round(host, gateway) != Round::Answered
}

/// Asks whether a PC answers a ping: Some(answered), None where this device can't ping.
pub type Pinger = std::sync::Arc<dyn Fn(IpAddr, Duration) -> Option<bool> + Send + Sync>;

/// A round with the PC's ping beside it, both within about [`ROUND_TIMEOUT`]. The ping goes out
/// only when `ping` is given and the PC is on this device's link (a gateway to compare with).
pub fn probe_with_ping(host: SocketAddr, gateway: Option<IpAddr>, ping: Option<&Pinger>) -> (Round, Option<bool>) {
    let pinger = ping.filter(|_| gateway.is_some()).and_then(|ping| {
        let (ping, ip) = (ping.clone(), host.ip());
        std::thread::Builder::new()
            .name("Host ping".into())
            .spawn(move || ping(ip, ROUND_TIMEOUT))
            .ok()
    });
    let round = probe_round(host, gateway);
    (round, pinger.and_then(|thread| thread.join().ok().flatten()))
}

/// The pinger of this platform: ICMP datagram sockets on Linux and Android. Windows clients pass
/// their own (`IcmpSendEcho`), and elsewhere nothing can ping.
pub fn system_pinger() -> Pinger {
    std::sync::Arc::new(|ip, timeout| match ping(ip, timeout) {
        Ok(answered) => Some(answered),
        Err(_) => None,
    })
}

/// One ICMP echo to `ip`, waiting up to `timeout`: whether it answered, or an error where this
/// device may not send one (no unprivileged ICMP sockets, IPv6, another platform).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn ping(ip: IpAddr, timeout: Duration) -> io::Result<bool> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::io::Read;
    let IpAddr::V4(_) = ip else {
        return Err(io::ErrorKind::Unsupported.into());
    };
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))?;
    let sequence = next_sequence();
    let deadline = Instant::now() + timeout;
    let sent = socket
        .connect(&SocketAddr::new(ip, 0).into())
        .and_then(|()| socket.send(&echo_request(sequence)));
    if let Err(error) = sent {
        return unanswered(error);
    }
    let mut buffer = [0u8; 128];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        socket.set_read_timeout(Some(left))?;
        match (&socket).read(&mut buffer) {
            Ok(length) if is_echo_reply(&buffer[..length], sequence) => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                return Ok(false);
            }
            Err(error) => return unanswered(error),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn ping(_: IpAddr, _: Duration) -> io::Result<bool> {
    Err(io::ErrorKind::Unsupported.into())
}

/// An ICMP error for the echo (an address nobody has on the link) is no answer; anything else
/// means the ping itself failed.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn unanswered(error: io::Error) -> io::Result<bool> {
    match error.raw_os_error() {
        Some(libc::EHOSTUNREACH | libc::EHOSTDOWN | libc::ENETUNREACH | libc::ECONNREFUSED) => Ok(false),
        _ => Err(error),
    }
}

#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn next_sequence() -> u16 {
    static SEQUENCE: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(1);
    SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// An ICMP echo request: type 8, code 0, the checksum, an identifier the kernel replaces for a
/// datagram socket, `sequence`, and a short payload.
#[cfg_attr(not(any(target_os = "linux", target_os = "android", test)), allow(dead_code))]
fn echo_request(sequence: u16) -> [u8; 16] {
    let mut packet = [0u8; 16];
    packet[0] = 8;
    packet[6..8].copy_from_slice(&sequence.to_be_bytes());
    packet[8..].copy_from_slice(b"nightjar");
    let checksum = internet_checksum(&packet);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    packet
}

/// An echo reply (type 0) to `sequence`, as a datagram socket receives it: without the IP header.
#[cfg_attr(not(any(target_os = "linux", target_os = "android", test)), allow(dead_code))]
fn is_echo_reply(packet: &[u8], sequence: u16) -> bool {
    packet.len() >= 8 && packet[0] == 0 && packet[6..8] == sequence.to_be_bytes()
}

#[cfg_attr(not(any(target_os = "linux", target_os = "android", test)), allow(dead_code))]
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;

    const LONG: Duration = Duration::from_secs(5);
    const GONE: Option<bool> = Some(false);

    fn pinging() -> Detector {
        Detector::new(true)
    }

    #[test]
    fn a_short_silence_never_probes_and_starts_over() {
        let mut d = pinging();
        assert!(!d.wants_round(Duration::from_millis(1999)));
        assert!(d.wants_round(PROBE_AFTER));
        d.step(LONG, Round::SilentLinkUp, GONE);
        d.step(LONG, Round::SilentLinkUp, GONE);
        assert!(!d.wants_round(Duration::ZERO), "the host was heard");
        assert_eq!(d, pinging(), "starting over keeps the first ping's answer");
    }

    #[test]
    fn a_pc_that_stops_answering_pings_on_a_working_link_is_gone() {
        let mut d = pinging();
        assert!(!d.step(LONG, Round::SilentLinkUp, GONE));
        assert!(!d.step(LONG, Round::SilentLinkUp, GONE));
        assert!(d.step(LONG, Round::SilentLinkUp, GONE));

        let mut d = pinging();
        for _ in 0..10 {
            assert!(!d.step(NO_ANSWER_AFTER - Duration::from_millis(1), Round::SilentLinkUp, GONE));
        }
        assert!(d.step(NO_ANSWER_AFTER, Round::SilentLinkUp, GONE));
    }

    #[test]
    fn a_pc_that_still_answers_pings_is_restarting_not_gone() {
        // A crashed or restarting server: the port is closed (silent on Windows, refused
        // elsewhere), the machine answers. The connection's own recovery takes it back.
        let mut d = pinging();
        for round in [Round::SilentLinkUp, Round::Refused] {
            for _ in 0..100 {
                assert!(!d.step(Duration::from_secs(60), round, Some(true)));
            }
        }
        d.step(LONG, Round::SilentLinkUp, GONE);
        d.step(LONG, Round::SilentLinkUp, GONE);
        assert!(!d.step(LONG, Round::SilentLinkUp, Some(true)));
        assert!(!d.step(LONG, Round::SilentLinkUp, GONE), "the answer started it over");
    }

    #[test]
    fn a_refusal_is_never_gone() {
        let mut d = pinging();
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::Refused, GONE));
        }
    }

    #[test]
    fn a_pc_that_never_answered_a_ping_is_left_to_the_timeout() {
        let mut d = Detector::new(false);
        assert!(!d.wants_ping());
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::SilentLinkUp, GONE));
            assert!(!d.step(Duration::from_secs(60), Round::SilentLinkUp, None));
        }
        assert_eq!(Detector::default(), Detector::new(false));
    }

    #[test]
    fn a_ping_that_could_not_go_out_proves_nothing() {
        let mut d = pinging();
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::SilentLinkUp, None));
        }
    }

    #[test]
    fn a_stalled_host_that_answers_is_never_gone() {
        let mut d = pinging();
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::Answered, GONE));
        }
    }

    #[test]
    fn a_dropout_or_a_pc_off_the_link_is_never_gone() {
        // A dropout takes the gateway with it; a relay, VPN or routed PC has none to compare.
        let mut d = pinging();
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::SilentLinkUnknown, GONE));
        }
        d.step(LONG, Round::SilentLinkUp, GONE);
        d.step(LONG, Round::SilentLinkUp, GONE);
        assert!(!d.step(LONG, Round::SilentLinkUnknown, GONE));
        assert!(!d.step(LONG, Round::SilentLinkUp, GONE), "the quiet link started it over");
    }

    #[test]
    fn the_state_survives_a_round_trip() {
        for pings in [false, true] {
            let mut d = Detector::new(pings);
            d.step(LONG, Round::SilentLinkUp, GONE);
            let state = d.state();
            assert!(state >= 0);
            assert_eq!(Detector::from_state(state), d);
        }
        assert_eq!(Detector::from_state(0), Detector::new(false));
    }

    #[test]
    fn a_round_survives_its_code() {
        for round in Round::ALL {
            assert_eq!(Round::from_code(round.code()), round);
        }
        assert_eq!(Round::from_code(-1), Round::SilentLinkUnknown);
        assert_eq!(Round::from_code(4), Round::SilentLinkUnknown);
    }

    #[test]
    fn a_round_pings_only_toward_a_pc_on_the_link() {
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        let pinger: Pinger = Arc::new(move |_, _| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(true)
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        assert_eq!(probe_with_ping(host, None, Some(&pinger)), (Round::Answered, None));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0, "no gateway, no ping");
        let gateway = Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        assert_eq!(probe_with_ping(host, gateway, Some(&pinger)), (Round::Answered, Some(true)));
        assert_eq!(probe_with_ping(host, gateway, None), (Round::Answered, None));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn an_echo_request_carries_its_sequence_and_a_valid_checksum() {
        let packet = echo_request(0x1234);
        assert_eq!((packet[0], packet[1]), (8, 0));
        assert_eq!(packet[6..8], [0x12, 0x34]);
        assert_eq!(internet_checksum(&packet), 0, "a packet with its checksum sums to zero");
        let mut reply = packet;
        reply[0] = 0;
        assert!(is_echo_reply(&reply, 0x1234));
        assert!(!is_echo_reply(&reply, 0x1235));
        assert!(!is_echo_reply(&packet, 0x1234), "a request is no reply");
        assert!(!is_echo_reply(&reply[..7], 0x1234));
    }

    #[test]
    fn the_checksum_matches_rfc_1071() {
        // RFC 1071's example words sum to 0xddf2 with the carries folded in.
        assert_eq!(internet_checksum(&[0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7]), !0xddf2);
        assert_eq!(internet_checksum(&[0xff]), !0xff00);
    }

    // Where this device may ping (Linux with ping_group_range, Android), loopback answers.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn loopback_answers_a_ping_where_pings_are_allowed() {
        match ping(IpAddr::V4(Ipv4Addr::LOCALHOST), Duration::from_secs(1)) {
            Ok(answered) => assert!(answered),
            Err(error) => eprintln!("pings not allowed here: {error}"),
        }
    }

    #[test]
    fn a_listening_port_answers() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        assert_eq!(
            probe_round(listener.local_addr().unwrap(), None),
            Round::Answered
        );
    }

    #[test]
    fn a_goodbye_from_a_server_that_still_listens_is_only_the_stream_ending() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        assert!(!server_left(listener.local_addr().unwrap(), None));
        let closed = listener.local_addr().unwrap();
        drop(listener);
        assert!(server_left(closed, None));
    }

    #[test]
    fn a_gateway_answer_is_only_used_for_a_pc_that_does_not_answer() {
        let (sender, answers) = mpsc::channel();
        sender.send(Answer::Gateway).unwrap();
        sender
            .send(Answer::Host(Err(io::ErrorKind::TimedOut.into())))
            .unwrap();
        assert_eq!(
            decide(&answers, Instant::now() + LONG),
            Round::SilentLinkUp
        );

        let (sender, answers) = mpsc::channel();
        sender.send(Answer::Gateway).unwrap();
        sender.send(Answer::Host(Ok(()))).unwrap();
        assert_eq!(decide(&answers, Instant::now() + LONG), Round::Answered);

        let (sender, answers) = mpsc::channel();
        sender
            .send(Answer::Host(Err(io::ErrorKind::TimedOut.into())))
            .unwrap();
        drop(sender);
        assert_eq!(
            decide(&answers, Instant::now() + LONG),
            Round::SilentLinkUnknown
        );
    }
}
