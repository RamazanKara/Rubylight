//! Telling a PC that shut down from a host that only ended the stream or restarts, after the host
//! said goodbye.
//!
//! A host whose control stream disconnects without a reason either ended the stream (its server
//! still listens), restarts (its server is back within a few seconds), or is on a PC that shuts
//! down. The clients' recovery reconnects to the first two, and would keep trying the last for its
//! whole window. So after the goodbye of this session's host, rounds of connection attempts ask:
//!
//! - Toward the PC's HTTPS port, which answered when the stream started. An accepted connection
//!   means the server is there: the old behaviour decides, or, after it was closed, it is back.
//!   A refusal comes from the PC itself: it is up, its server is not.
//! - Without an answer: toward this device's own gateway, when the PC is on the same link
//!   ([`Round::SilentLinkUp`]). A gateway that keeps quiet, a VPN, a relay or a PC behind a router
//!   proves nothing ([`Round::SilentLinkUnknown`]), and the old behaviour decides.
//!
//! A closed server reads as no answer too (Windows drops connections to a closed port without a
//! word), so the PC itself is asked with a [`ping`] in each round: a PC that stops answering is
//! shutting down, and one that keeps answering may be restarting its host, which gets
//! [`RESTART_GRACE`] to come back. Only a PC that answered a ping when the stream started counts;
//! one whose firewall drops pings is left to the old behaviour.
//!
//! A silence without a goodbye is never decided here: a blip on either side (Wi-Fi, a mesh
//! backhaul, an adapter reset) can last as long as the connection's own timeout, so nothing
//! shorter can tell a PC that lost power from it. [`after_goodbye`] decides, for the Android app
//! and the desktop client alike.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long one round waits for its answers, the ping's included.
pub const ROUND_TIMEOUT: Duration = Duration::from_millis(750);
/// How long the ping at the start of a stream waits.
pub const FIRST_PING_TIMEOUT: Duration = Duration::from_millis(1000);
/// How long a host that said goodbye on a PC that still answers pings has to open its server again:
/// a restart of the test host opened its HTTPS port again 1.7 to 2.2 s after the stop (2026-11-07), about a third.
pub const RESTART_GRACE: Duration = Duration::from_secs(6);
/// Between two rounds after a goodbye.
pub const GOODBYE_ROUND_GAP: Duration = Duration::from_millis(250);
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

/// What a round after the host's goodbye says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goodbye {
    /// The server is closed and the PC still answers: another round.
    Wait,
    /// The PC is shutting down, or its host did not come back within [`RESTART_GRACE`].
    Gone,
    /// The server is back after it was closed: the host restarted, and the stream reconnects.
    Back,
    /// Nothing here can tell: the old behaviour decides.
    Unknown,
}

impl Goodbye {
    const ALL: [Goodbye; 4] = [Goodbye::Wait, Goodbye::Gone, Goodbye::Back, Goodbye::Unknown];

    /// The verdict as a number, for a caller that passes it across a JNI call.
    pub fn code(self) -> i32 {
        Self::ALL.iter().position(|g| *g == self).unwrap_or(3) as i32
    }
}

/// One round's verdict after the host's goodbye. `pinged_at_start`: the PC answered a ping as the
/// stream started; `on_link`: it is on this device's link (a gateway to compare with);
/// `pc_pings`: its answer to this round's ping (None: not asked, or this device can't ping);
/// `closed_before`: an earlier round found the server closed; `waited`: since the first round.
pub fn after_goodbye(
    pinged_at_start: bool,
    on_link: bool,
    round: Round,
    pc_pings: Option<bool>,
    closed_before: bool,
    waited: Duration,
) -> Goodbye {
    if !pinged_at_start || !on_link {
        return Goodbye::Unknown;
    }
    match (round, pc_pings) {
        (Round::Answered, _) if closed_before => Goodbye::Back,
        // Still listening: only this stream ended.
        (Round::Answered, _) => Goodbye::Unknown,
        // This device's own link may be down, or the ping proves nothing.
        (Round::SilentLinkUnknown, _) | (_, None) => Goodbye::Unknown,
        // No answer at all, on a link that works: a PC shutting down.
        (Round::SilentLinkUp, Some(false)) => Goodbye::Gone,
        // A refusal or a ping answered: the PC is up, its server closed. A restart comes back.
        (Round::Refused | Round::SilentLinkUp, Some(_)) if waited >= RESTART_GRACE => Goodbye::Gone,
        (Round::Refused | Round::SilentLinkUp, Some(_)) => Goodbye::Wait,
    }
}

/// After the host's goodbye: rounds until a verdict other than [`Goodbye::Wait`], at most about
/// [`RESTART_GRACE`] and a round. `gateway` is this device's gateway toward `host` while it is on
/// its link, asked each round; `stopped` ends it early with [`Goodbye::Unknown`]; `log` gets each
/// round.
pub fn watch_goodbye(
    host: SocketAddr,
    gateway: &dyn Fn(IpAddr) -> Option<IpAddr>,
    ping: &Pinger,
    pinged_at_start: bool,
    stopped: &dyn Fn() -> bool,
    log: &dyn Fn(&str),
) -> Goodbye {
    if !pinged_at_start {
        return Goodbye::Unknown;
    }
    let started = Instant::now();
    let mut closed_before = false;
    while !stopped() {
        let toward = gateway(host.ip());
        let (round, pings) = probe_with_ping(host, toward, Some(ping));
        let verdict = after_goodbye(true, toward.is_some(), round, pings, closed_before, started.elapsed());
        log(&format!(
            "Host check after the goodbye, {} ms in: {round:?}, ping {pings:?}: {verdict:?}",
            started.elapsed().as_millis()
        ));
        if verdict != Goodbye::Wait {
            return verdict;
        }
        closed_before = true;
        std::thread::sleep(GOODBYE_ROUND_GAP);
    }
    Goodbye::Unknown
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

    const UP: Option<bool> = Some(true);
    const DOWN: Option<bool> = Some(false);
    const NOW: Duration = Duration::ZERO;

    #[test]
    fn a_pc_that_stops_answering_after_the_goodbye_is_gone_at_once() {
        assert_eq!(after_goodbye(true, true, Round::SilentLinkUp, DOWN, false, NOW), Goodbye::Gone);
        assert_eq!(after_goodbye(true, true, Round::SilentLinkUp, DOWN, true, Duration::from_secs(3)), Goodbye::Gone);
    }

    #[test]
    fn a_pc_that_keeps_answering_gets_the_restart_grace() {
        for round in [Round::Refused, Round::SilentLinkUp] {
            assert_eq!(after_goodbye(true, true, round, UP, false, NOW), Goodbye::Wait);
            assert_eq!(after_goodbye(true, true, round, UP, true, RESTART_GRACE - Duration::from_millis(1)), Goodbye::Wait);
            assert_eq!(after_goodbye(true, true, round, UP, true, RESTART_GRACE), Goodbye::Gone);
        }
        // A refusal comes from the PC itself, whatever its ping says.
        assert_eq!(after_goodbye(true, true, Round::Refused, DOWN, false, NOW), Goodbye::Wait);
    }

    #[test]
    fn a_server_that_opens_again_is_a_restart_and_one_that_never_closed_only_ended_the_stream() {
        assert_eq!(after_goodbye(true, true, Round::Answered, UP, true, Duration::from_secs(4)), Goodbye::Back);
        assert_eq!(after_goodbye(true, true, Round::Answered, UP, false, NOW), Goodbye::Unknown);
        assert_eq!(after_goodbye(true, true, Round::Answered, None, false, NOW), Goodbye::Unknown);
    }

    #[test]
    fn without_a_first_ping_or_off_the_link_the_old_behaviour_decides() {
        for round in Round::ALL {
            for pings in [UP, DOWN, None] {
                assert_eq!(after_goodbye(false, true, round, pings, false, RESTART_GRACE), Goodbye::Unknown);
                // A relay, a VPN, a PC behind a router: no gateway to compare with, so no ping either.
                assert_eq!(after_goodbye(true, false, round, pings, true, RESTART_GRACE), Goodbye::Unknown);
            }
        }
    }

    #[test]
    fn a_quiet_link_or_a_ping_that_could_not_go_out_proves_nothing() {
        for pings in [UP, DOWN, None] {
            assert_eq!(after_goodbye(true, true, Round::SilentLinkUnknown, pings, true, RESTART_GRACE), Goodbye::Unknown);
        }
        assert_eq!(after_goodbye(true, true, Round::SilentLinkUp, None, false, NOW), Goodbye::Unknown);
        assert_eq!(after_goodbye(true, true, Round::Refused, None, false, NOW), Goodbye::Unknown);
    }

    #[test]
    fn a_goodbye_codes_are_distinct() {
        let codes: Vec<i32> = Goodbye::ALL.iter().map(|g| g.code()).collect();
        assert_eq!(codes, [0, 1, 2, 3]);
    }

    fn pinger(answer: Option<bool>) -> Pinger {
        Arc::new(move |_, _| answer)
    }

    #[test]
    fn the_watch_after_a_goodbye_ends_with_a_server_that_still_listens_or_a_pc_off_the_link() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        let gateway = |_: IpAddr| Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        let never = || false;
        assert_eq!(watch_goodbye(host, &gateway, &pinger(UP), true, &never, &|_| {}), Goodbye::Unknown);
        assert_eq!(watch_goodbye(host, &|_| None, &pinger(UP), true, &never, &|_| {}), Goodbye::Unknown);
        assert_eq!(watch_goodbye(host, &gateway, &pinger(UP), false, &never, &|_| {}), Goodbye::Unknown);
    }

    // Windows answers a connection to a closed loopback port only after its retries, past a round:
    // the round then proves nothing, as it should.
    #[cfg(not(windows))]
    #[test]
    fn the_watch_after_a_goodbye_sees_a_restart_come_back() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        drop(listener);
        // The server comes back on the same port a moment later.
        let reopen = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            TcpListener::bind(host).ok().inspect(|_| std::thread::sleep(Duration::from_secs(3)))
        });
        let gateway = |_: IpAddr| Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        let rounds = std::sync::Mutex::new(Vec::new());
        let verdict = watch_goodbye(host, &gateway, &pinger(UP), true, &|| false, &|line| rounds.lock().unwrap().push(line.to_owned()));
        if reopen.join().unwrap().is_some() {
            assert_eq!(verdict, Goodbye::Back, "{:?}", rounds.lock().unwrap());
        }
    }

    #[test]
    fn the_watch_after_a_goodbye_stops_when_asked() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        drop(listener);
        let gateway = |_: IpAddr| Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let stopped = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1;
        let started = Instant::now();
        assert_eq!(watch_goodbye(host, &gateway, &pinger(UP), true, &stopped, &|_| {}), Goodbye::Unknown);
        assert!(started.elapsed() < RESTART_GRACE);
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
    fn a_gateway_answer_is_only_used_for_a_pc_that_does_not_answer() {
        let (sender, answers) = mpsc::channel();
        sender.send(Answer::Gateway).unwrap();
        sender
            .send(Answer::Host(Err(io::ErrorKind::TimedOut.into())))
            .unwrap();
        assert_eq!(
            decide(&answers, Instant::now() + Duration::from_secs(5)),
            Round::SilentLinkUp
        );

        let (sender, answers) = mpsc::channel();
        sender.send(Answer::Gateway).unwrap();
        sender.send(Answer::Host(Ok(()))).unwrap();
        assert_eq!(decide(&answers, Instant::now() + Duration::from_secs(5)), Round::Answered);

        let (sender, answers) = mpsc::channel();
        sender
            .send(Answer::Host(Err(io::ErrorKind::TimedOut.into())))
            .unwrap();
        drop(sender);
        assert_eq!(
            decide(&answers, Instant::now() + Duration::from_secs(5)),
            Round::SilentLinkUnknown
        );
    }
}
