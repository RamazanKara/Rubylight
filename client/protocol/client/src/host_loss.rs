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
//!   means the PC's server stopped, or the network would have carried nothing back.
//! - Without an answer: toward this device's own gateway, when the PC is on the same link
//!   ([`Round::SilentLinkUp`]). A gateway that answers while the PC does not means this device's
//!   network is fine and the PC is off it. A dropout takes the gateway with it, and a gateway that
//!   keeps quiet, a VPN or a PC behind a router proves nothing: the timeout decides those.
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
/// How long one round waits for its answers.
pub const ROUND_TIMEOUT: Duration = Duration::from_millis(750);
/// Refusals in a row that make a PC gone.
const REFUSED_ROUNDS: u8 = 2;
/// Rounds in a row without an answer, on a link that works, that make a PC gone: a link that
/// just came back reaches the PC within them.
const SILENT_ROUNDS: u8 = 3;
/// Where a gateway may answer: an accepted or refused connection both prove it is reachable.
const GATEWAY_PORTS: [u16; 3] = [53, 80, 443];

/// What one round of connection attempts found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    /// The PC accepted a connection: it is there.
    Answered,
    /// The PC refused one: its server is gone.
    Refused,
    /// The PC did not answer, and this device's gateway did.
    SilentLinkUp,
    /// The PC did not answer, and nothing says this device's network works.
    SilentLinkUnknown,
}

/// Decides from the control stream's silence and the rounds that it brought on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Detector {
    refused: u8,
    silent: u8,
}

impl Detector {
    /// Whether this silence calls for a round. Below it, the detector also starts over.
    pub fn wants_round(&mut self, silence: Duration) -> bool {
        if silence < PROBE_AFTER {
            *self = Self::default();
            return false;
        }
        true
    }

    /// Takes a round's result; true when the PC is gone.
    pub fn step(&mut self, silence: Duration, round: Round) -> bool {
        match round {
            Round::Refused => {
                self.refused += 1;
                self.silent = 0;
                self.refused >= REFUSED_ROUNDS
            }
            Round::SilentLinkUp => {
                self.refused = 0;
                self.silent = self.silent.saturating_add(1);
                self.silent >= SILENT_ROUNDS && silence >= NO_ANSWER_AFTER
            }
            Round::Answered | Round::SilentLinkUnknown => {
                *self = Self::default();
                false
            }
        }
    }

    /// The state as a number, for a caller that keeps it across a JNI call (never negative).
    pub fn state(&self) -> i32 {
        i32::from(self.refused) << 8 | i32::from(self.silent)
    }

    pub fn from_state(state: i32) -> Self {
        Self {
            refused: (state >> 8) as u8,
            silent: state as u8,
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

/// After the control stream ended without a reason: whether the PC is gone rather than the
/// connection only lost. A host that closes on its way down says goodbye first, and no silence
/// follows. At most four rounds; a round that proves nothing ends it.
pub fn confirm_gone(host: SocketAddr, gateway: impl Fn() -> Option<IpAddr>) -> bool {
    let mut detector = Detector::default();
    for _ in 0..=SILENT_ROUNDS {
        if detector.step(NO_ANSWER_AFTER, probe_round(host, gateway())) {
            return true;
        }
        if detector == Detector::default() {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    const LONG: Duration = Duration::from_secs(5);

    #[test]
    fn a_short_silence_never_probes_and_starts_over() {
        let mut d = Detector::default();
        assert!(!d.wants_round(Duration::from_millis(1999)));
        assert!(d.wants_round(PROBE_AFTER));
        d.step(LONG, Round::Refused);
        assert!(!d.wants_round(Duration::ZERO), "the host was heard");
        assert_eq!(d, Detector::default());
    }

    #[test]
    fn two_refusals_in_a_row_are_a_gone_pc() {
        let mut d = Detector::default();
        assert!(!d.step(PROBE_AFTER, Round::Refused));
        assert!(d.step(PROBE_AFTER, Round::Refused));
    }

    #[test]
    fn a_stalled_host_that_answers_is_never_gone() {
        let mut d = Detector::default();
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::Answered));
        }
        d.step(LONG, Round::Refused);
        assert!(!d.step(LONG, Round::Answered));
        assert!(!d.step(LONG, Round::Refused), "the answer started it over");
    }

    #[test]
    fn a_dropout_that_takes_the_link_with_it_is_never_gone() {
        let mut d = Detector::default();
        for _ in 0..100 {
            assert!(!d.step(Duration::from_secs(60), Round::SilentLinkUnknown));
        }
    }

    #[test]
    fn no_answer_on_a_working_link_needs_rounds_and_time() {
        let mut d = Detector::default();
        assert!(!d.step(LONG, Round::SilentLinkUp));
        assert!(!d.step(LONG, Round::SilentLinkUp));
        assert!(d.step(LONG, Round::SilentLinkUp));

        let mut d = Detector::default();
        for _ in 0..10 {
            assert!(!d.step(NO_ANSWER_AFTER - Duration::from_millis(1), Round::SilentLinkUp));
        }
        assert!(d.step(NO_ANSWER_AFTER, Round::SilentLinkUp));
    }

    #[test]
    fn a_link_that_goes_quiet_starts_the_rounds_over() {
        let mut d = Detector::default();
        d.step(LONG, Round::SilentLinkUp);
        d.step(LONG, Round::SilentLinkUp);
        assert!(!d.step(LONG, Round::SilentLinkUnknown));
        assert!(!d.step(LONG, Round::SilentLinkUp));
    }

    #[test]
    fn the_state_survives_a_round_trip() {
        let mut d = Detector::default();
        d.step(LONG, Round::SilentLinkUp);
        let state = d.state();
        assert!(state >= 0);
        assert_eq!(Detector::from_state(state), d);
        d.step(LONG, Round::Refused);
        assert_eq!(Detector::from_state(d.state()), d);
    }

    fn free_port() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap()
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

    #[test]
    fn confirming_stops_at_a_pc_that_answers() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        assert!(!confirm_gone(listener.local_addr().unwrap(), || None));
    }

    // Windows retries a refused connection for about two seconds, past a round's timeout: there it
    // reads as no answer, which the gateway decides. Elsewhere a refusal comes back at once.
    #[cfg(not(windows))]
    #[test]
    fn a_closed_port_is_a_gone_pc() {
        assert!(confirm_gone(free_port(), || None));
    }

    #[cfg(windows)]
    #[test]
    fn a_closed_port_without_a_link_proves_nothing_on_windows() {
        assert!(!confirm_gone(free_port(), || None));
    }
}
