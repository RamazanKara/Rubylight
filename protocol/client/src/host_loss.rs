//! Telling a PC that shut down or stopped its host from a host that only ended the stream, after
//! the host said goodbye.
//!
//! A host whose control stream disconnects without a reason either ended the stream (its server
//! still listens) or is going away: its PC shuts down, or the host stops or restarts. The clients'
//! recovery would keep trying a host that went away for its whole window, and a restarted host
//! cannot resume the stream anyway. So after the goodbye of this session's host, rounds of
//! connection attempts ask:
//!
//! - Toward the PC's HTTPS port, which answered when the stream started. An accepted connection
//!   means the server is there: the old behaviour decides, once a host on its way out had
//!   [`HOST_EXIT_WINDOW`] to close it. A refusal comes from the PC itself: its server is closed.
//! - Without an answer: toward this device's own gateway, when the PC is on the same link
//!   ([`Round::SilentLinkUp`]: the PC or its closed port keeps quiet, as Windows does). A gateway
//!   that keeps quiet proves nothing ([`Round::SilentLinkUnknown`]) and gets another round; a VPN,
//!   a relay or a PC behind a router is left to the old behaviour.
//!
//! A silence without a goodbye is never decided here: a blip on either side (Wi-Fi, a mesh
//! backhaul, an adapter reset) can last as long as the connection's own timeout, so nothing
//! shorter can tell a PC that lost power from it. [`after_goodbye`] decides, for the Android app
//! and the desktop client alike.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long one round waits for its answers.
pub const ROUND_TIMEOUT: Duration = Duration::from_millis(750);
/// How long a host that said goodbye may still listen while it exits: the test host's process was
/// gone 0.23 to 0.54 s after its stop (2026-11-07). A server still open after it only ended the stream.
pub const HOST_EXIT_WINDOW: Duration = Duration::from_secs(2);
/// How long a quiet moment on this device's link waits for a round that can tell: a Retroid had
/// single such rounds, now and then, after a goodbye (2026-11-07).
pub const QUIET_LINK_WAIT: Duration = Duration::from_secs(4);
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
    /// Not yet: a host still exiting, or a quiet moment on this device's link. Another round.
    Wait,
    /// The server is closed: the PC shuts down, or its host stopped or restarts. Ends at once.
    Gone,
    /// Nothing here can tell: the old behaviour decides.
    Unknown,
}

impl Goodbye {
    const ALL: [Goodbye; 3] = [Goodbye::Wait, Goodbye::Gone, Goodbye::Unknown];

    /// The verdict as a number, for a caller that passes it across a JNI call.
    pub fn code(self) -> i32 {
        Self::ALL.iter().position(|g| *g == self).unwrap_or(2) as i32
    }
}

/// One round's verdict after the host's goodbye. `on_link`: the PC is on this device's link (a
/// gateway to compare with); `waited`: since the first round.
pub fn after_goodbye(on_link: bool, round: Round, waited: Duration) -> Goodbye {
    if !on_link {
        return Goodbye::Unknown;
    }
    match round {
        // A refusal, or a silent PC on a link that works: the server is closed.
        Round::Refused | Round::SilentLinkUp => Goodbye::Gone,
        // A host on its way out still listens for a moment after its goodbye.
        Round::Answered if waited < HOST_EXIT_WINDOW => Goodbye::Wait,
        // Still listening: only this stream ended.
        Round::Answered => Goodbye::Unknown,
        // This device's own link may be down for a moment.
        Round::SilentLinkUnknown if waited < QUIET_LINK_WAIT => Goodbye::Wait,
        Round::SilentLinkUnknown => Goodbye::Unknown,
    }
}

/// After the host's goodbye: rounds until a verdict other than [`Goodbye::Wait`], at most about
/// [`QUIET_LINK_WAIT`] and a round. `gateway` is this device's gateway toward `host` while it is
/// on its link, asked each round; `stopped` ends it early with [`Goodbye::Unknown`]; `log` gets
/// each round.
pub fn watch_goodbye(
    host: SocketAddr,
    gateway: &dyn Fn(IpAddr) -> Option<IpAddr>,
    stopped: &dyn Fn() -> bool,
    log: &dyn Fn(&str),
) -> Goodbye {
    let started = Instant::now();
    while !stopped() {
        let Some(toward) = gateway(host.ip()) else {
            log("Host check after the goodbye: the PC is not on this device's link");
            return after_goodbye(false, Round::SilentLinkUnknown, started.elapsed());
        };
        let round = probe_round(host, Some(toward));
        let verdict = after_goodbye(true, round, started.elapsed());
        log(&format!(
            "Host check after the goodbye, {} ms in: {round:?}: {verdict:?}",
            started.elapsed().as_millis()
        ));
        if verdict != Goodbye::Wait {
            return verdict;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    const NOW: Duration = Duration::ZERO;

    #[test]
    fn a_closed_server_after_the_goodbye_ends_at_once() {
        // A refusal from the PC, or a PC (or its closed port) that keeps quiet on a working link.
        assert_eq!(after_goodbye(true, Round::Refused, NOW), Goodbye::Gone);
        assert_eq!(after_goodbye(true, Round::SilentLinkUp, NOW), Goodbye::Gone);
        assert_eq!(after_goodbye(true, Round::SilentLinkUp, Duration::from_secs(3)), Goodbye::Gone);
    }

    #[test]
    fn a_server_still_open_after_the_exit_window_only_ended_the_stream() {
        assert_eq!(after_goodbye(true, Round::Answered, NOW), Goodbye::Wait);
        assert_eq!(after_goodbye(true, Round::Answered, HOST_EXIT_WINDOW - Duration::from_millis(1)), Goodbye::Wait);
        assert_eq!(after_goodbye(true, Round::Answered, HOST_EXIT_WINDOW), Goodbye::Unknown);
    }

    #[test]
    fn a_quiet_link_gets_more_rounds_then_the_old_behaviour() {
        assert_eq!(after_goodbye(true, Round::SilentLinkUnknown, NOW), Goodbye::Wait);
        assert_eq!(after_goodbye(true, Round::SilentLinkUnknown, QUIET_LINK_WAIT - Duration::from_millis(1)), Goodbye::Wait);
        assert_eq!(after_goodbye(true, Round::SilentLinkUnknown, QUIET_LINK_WAIT), Goodbye::Unknown);
    }

    #[test]
    fn off_the_link_the_old_behaviour_decides() {
        // A relay, a VPN, a PC behind a router: no gateway to compare a silence with.
        for round in Round::ALL {
            assert_eq!(after_goodbye(false, round, NOW), Goodbye::Unknown);
        }
    }

    #[test]
    fn a_goodbye_codes_are_distinct() {
        let codes: Vec<i32> = Goodbye::ALL.iter().map(|g| g.code()).collect();
        assert_eq!(codes, [0, 1, 2]);
    }

    fn on_link(_: IpAddr) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
    }

    #[test]
    fn the_watch_after_a_goodbye_ends_with_a_server_that_still_listens_or_a_pc_off_the_link() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        let never = || false;
        let started = Instant::now();
        assert_eq!(watch_goodbye(host, &on_link, &never, &|_| {}), Goodbye::Unknown);
        assert!(started.elapsed() >= HOST_EXIT_WINDOW, "a host still exiting gets its window");
        let started = Instant::now();
        assert_eq!(watch_goodbye(host, &|_| None, &never, &|_| {}), Goodbye::Unknown);
        assert!(started.elapsed() < ROUND_TIMEOUT, "off the link nothing is asked");
    }

    // Windows answers a connection to a closed loopback port only after its retries, past a round.
    #[cfg(not(windows))]
    #[test]
    fn the_watch_after_a_goodbye_ends_once_the_exiting_host_closes_its_server() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        // The host still listens right after its goodbye, then exits.
        let exit = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(listener);
        });
        let rounds = std::sync::Mutex::new(Vec::new());
        let verdict = watch_goodbye(host, &on_link, &|| false, &|line| rounds.lock().unwrap().push(line.to_owned()));
        exit.join().unwrap();
        assert_eq!(verdict, Goodbye::Gone, "{:?}", rounds.lock().unwrap());
    }

    #[test]
    fn the_watch_after_a_goodbye_stops_when_asked() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let host = listener.local_addr().unwrap();
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let stopped = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1;
        let started = Instant::now();
        assert_eq!(watch_goodbye(host, &on_link, &stopped, &|_| {}), Goodbye::Unknown);
        assert!(started.elapsed() < HOST_EXIT_WINDOW);
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
