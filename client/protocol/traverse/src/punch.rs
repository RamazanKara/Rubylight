//! UDP hole punching without IO. Both sides send punches to every address they know for the
//! other at the same time; each punch opens the sender's NAT towards that address, so the
//! other side's punches get through. The first authenticated punch or answer fixes the path at
//! the address it came from: that also finds a peer whose NAT gave it another port than its
//! record says (a symmetric NAT), as long as this side's NAT lets that port in.
//!
//! For a peer behind a symmetric NAT, [`candidates`] adds the next few ports after the one it
//! reported: many such NATs hand out ports in sequence.

use crate::{record::Nat, Millis};
use std::net::SocketAddr;

/// How long punching goes on before it gives up.
pub const TIMEOUT: Millis = 30_000;
/// Punches go out this often at first,
const FAST: Millis = 100;
/// for this long,
const FAST_FOR: Millis = 3_000;
/// then this often.
const SLOW: Millis = 500;
/// How many ports past a symmetric NAT's reported one are tried.
pub const PREDICTED_PORTS: u16 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Punching,
    Connected(SocketAddr),
    /// No punch got through before the timeout.
    Failed,
}

pub struct Puncher {
    candidates: Vec<SocketAddr>,
    started: Millis,
    next: Millis,
    state: State,
}

/// The addresses to punch for a peer's record: its endpoints, and for a symmetric NAT the
/// ports after each IPv4 one.
pub fn candidates(endpoints: &[SocketAddr], nat: Nat) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    let mut push = |address: SocketAddr| {
        if address.port() != 0 && !out.contains(&address) {
            out.push(address);
        }
    };
    for &endpoint in endpoints {
        push(endpoint);
    }
    if nat == Nat::Symmetric {
        for &endpoint in endpoints.iter().filter(|endpoint| endpoint.is_ipv4()) {
            for step in 1..=PREDICTED_PORTS {
                if let Some(port) = endpoint.port().checked_add(step) {
                    push(SocketAddr::new(endpoint.ip(), port));
                }
            }
        }
    }
    out
}

impl Puncher {
    pub fn new(candidates: Vec<SocketAddr>, now: Millis) -> Self {
        Self {
            candidates,
            started: now,
            next: now,
            state: State::Punching,
        }
    }

    /// More addresses learned while punching (the peer's record arrived later).
    pub fn add(&mut self, more: &[SocketAddr]) {
        for &address in more {
            if !self.candidates.contains(&address) {
                self.candidates.push(address);
            }
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The addresses to punch now; none once connected or failed.
    pub fn poll(&mut self, now: Millis) -> Vec<SocketAddr> {
        if self.state != State::Punching {
            return Vec::new();
        }
        if now.saturating_sub(self.started) >= TIMEOUT {
            self.state = State::Failed;
            return Vec::new();
        }
        if now < self.next {
            return Vec::new();
        }
        let interval = if now - self.started < FAST_FOR {
            FAST
        } else {
            SLOW
        };
        self.next = now + interval;
        self.candidates.clone()
    }

    pub fn next_wakeup(&self) -> Option<Millis> {
        (self.state == State::Punching).then_some(self.next.min(self.started + TIMEOUT))
    }

    /// An authenticated punch arrived from `from`: answer it there, and the path is that address.
    pub fn on_punch(&mut self, from: SocketAddr) {
        self.connect(from);
    }

    /// An authenticated answer to a punch arrived from `from`.
    pub fn on_ack(&mut self, from: SocketAddr) {
        self.connect(from);
    }

    fn connect(&mut self, from: SocketAddr) {
        // A later punch from another address (the peer roamed, or a second candidate opened)
        // moves the path there only while not yet connected.
        if !matches!(self.state, State::Connected(_)) {
            self.state = State::Connected(from);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punches_start_fast_slow_down_and_give_up() {
        let peer: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let mut puncher = Puncher::new(vec![peer], 1_000);
        assert_eq!(puncher.poll(1_000), vec![peer]);
        assert!(puncher.poll(1_050).is_empty());
        assert_eq!(puncher.poll(1_100), vec![peer]);
        let mut sent = 2;
        for now in (1_101..1_000 + TIMEOUT).step_by(10) {
            sent += puncher.poll(now).len();
        }
        // 3 s at 10 per second, then 27 s at 2 per second.
        assert!((80..=90).contains(&sent), "{sent}");
        assert_eq!(puncher.poll(1_000 + TIMEOUT), Vec::<SocketAddr>::new());
        assert_eq!(puncher.state(), State::Failed);
        assert_eq!(puncher.next_wakeup(), None);
    }

    #[test]
    fn the_first_authenticated_punch_fixes_the_path() {
        let reported: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let actual: SocketAddr = "203.0.113.5:40007".parse().unwrap();
        let mut puncher = Puncher::new(vec![reported], 0);
        puncher.on_punch(actual);
        assert_eq!(puncher.state(), State::Connected(actual));
        puncher.on_ack(reported);
        assert_eq!(puncher.state(), State::Connected(actual));
        assert!(puncher.poll(100).is_empty());
    }

    #[test]
    fn a_symmetric_peer_gets_its_next_ports_tried() {
        let v4: SocketAddr = "203.0.113.5:65530".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::5]:40000".parse().unwrap();
        assert_eq!(candidates(&[v4, v6, v4], Nat::Cone), vec![v4, v6]);
        let symmetric = candidates(&[v4, v6], Nat::Symmetric);
        // 65531..=65535: the ports that exist.
        assert_eq!(symmetric.len(), 2 + 5);
        assert!(symmetric.contains(&"203.0.113.5:65535".parse().unwrap()));
    }
}
