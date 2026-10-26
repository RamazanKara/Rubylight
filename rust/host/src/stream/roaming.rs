//! Client roaming on the control port (`butterpollo_core::roaming`). A client whose
//! address changed keeps its ENet peer: once it proved the new address, the control
//! socket maps datagrams between it and the address ENet knows the peer by, and drops
//! those from the address it left.
use butterpollo_core::roaming::{
    Action, DATAGRAM_BYTES, Refusal, RoamTable, SeqMemory, is_roaming,
};
use butterpollo_windows::net::ReplySocket;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// At most one refused roaming datagram is logged in this long.
const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// A move the control loop carries out: the session follows, then `migrated` goes to
/// `new`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Commit {
    pub id: String,
    /// The address ENet knows the peer by.
    pub peer: SocketAddr,
    pub old: SocketAddr,
    pub new: SocketAddr,
    pub media_rebound: bool,
    pub migrated: [u8; DATAGRAM_BYTES],
}

#[derive(Default)]
pub(super) struct Roaming {
    /// `client_roaming`: off, roaming datagrams go to ENet, which drops them, as before.
    enabled: bool,
    table: RoamTable<String>,
    /// ENet's address of each peer in `table`, and its session.
    sessions: HashMap<SocketAddr, String>,
    /// ENet's address -> the address the client moved to.
    aliases: HashMap<SocketAddr, SocketAddr>,
    /// The address a client moved to -> ENet's address.
    moved: HashMap<SocketAddr, SocketAddr>,
    /// An address a client left -> ENet's address. Its datagrams are dropped.
    retired: HashMap<SocketAddr, SocketAddr>,
    commits: Vec<Commit>,
    refusals: Refusals,
}

impl Roaming {
    /// Roaming whose launches' seqs live in `memory`, beyond this socket.
    pub(super) fn remembering(memory: SeqMemory) -> Self {
        Self {
            table: RoamTable::new().with_memory(memory),
            ..Default::default()
        }
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether `data` is a roaming datagram for [`Self::on_datagram`] rather than ENet's.
    pub(super) fn intercepts(&self, data: &[u8]) -> bool {
        self.enabled && is_roaming(data)
    }

    /// Accepts roaming for session `id`, whose ENet peer is at `peer`.
    pub(super) fn insert(&mut self, id: &str, rikey: &[u8; 16], peer: SocketAddr) {
        self.table.insert(id.to_owned(), rikey, peer);
        self.sessions.insert(peer, id.to_owned());
    }

    pub(super) fn set_stopping(&mut self, id: &str) {
        self.table.set_stopping(&id.to_owned());
    }

    /// Handles a roaming datagram from `from`, which reached `local`, and answers it.
    pub(super) fn on_datagram(
        &mut self,
        socket: &ReplySocket,
        now: Instant,
        from: SocketAddr,
        local: Option<IpAddr>,
        data: &[u8],
    ) {
        match self.table.on_datagram(now, from, data) {
            Ok(Action::Reply { to, datagram }) => {
                socket.answer_from(to, local);
                send(socket, &datagram, to);
            }
            Ok(Action::Commit {
                id,
                peer,
                old,
                new,
                media_rebound,
                datagram,
            }) => {
                socket.answer_from(new, local);
                self.aliases.remove(&peer);
                self.moved.remove(&old);
                if new != peer {
                    self.aliases.insert(peer, new);
                    self.moved.insert(new, peer);
                }
                self.retired.remove(&new);
                self.retired.insert(old, peer);
                self.commits.push(Commit {
                    id,
                    peer,
                    old,
                    new,
                    media_rebound,
                    migrated: datagram,
                });
            }
            Err(refusal) => self.refusals.note(now, refusal, from),
        }
    }

    /// The moves since the last call, in order.
    pub(super) fn take_commits(&mut self) -> Vec<Commit> {
        std::mem::take(&mut self.commits)
    }

    /// The address ENet knows a datagram from `from` by; `None` for an address a client
    /// left, whose datagrams are dropped.
    pub(super) fn peer_of(&self, from: SocketAddr) -> Option<SocketAddr> {
        if self.moved.is_empty() && self.retired.is_empty() {
            return Some(from);
        }
        if let Some(peer) = self.moved.get(&from) {
            return Some(*peer);
        }
        (!self.retired.contains_key(&from)).then_some(from)
    }

    /// Where a datagram ENet sends to `peer` goes.
    pub(super) fn wire_of(&self, peer: SocketAddr) -> SocketAddr {
        if self.aliases.is_empty() {
            return peer;
        }
        self.aliases.get(&peer).copied().unwrap_or(peer)
    }

    /// Forgets the peers `live` no longer holds, with their addresses. A session's
    /// roaming ends with its last peer; its seqs stay (`RoamTable::remove`).
    pub(super) fn retain_peers(&mut self, socket: &ReplySocket, live: impl Fn(SocketAddr) -> bool) {
        let gone: Vec<SocketAddr> = self
            .sessions
            .keys()
            .copied()
            .filter(|peer| !live(*peer))
            .collect();
        for peer in gone {
            let Some(id) = self.sessions.remove(&peer) else {
                continue;
            };
            if let Some(wire) = self.aliases.remove(&peer) {
                socket.forget(wire);
            }
            for addresses in [&mut self.moved, &mut self.retired] {
                addresses.retain(|wire, of| {
                    if *of == peer {
                        socket.forget(*wire);
                    }
                    *of != peer
                });
            }
            if !self.sessions.values().any(|other| *other == id) {
                self.table.remove(&id);
            }
        }
    }

    /// Whether any peer is known, so [`Self::retain_peers`] has work.
    pub(super) fn has_peers(&self) -> bool {
        !self.sessions.is_empty()
    }
}

pub(super) fn send(socket: &ReplySocket, datagram: &[u8], to: SocketAddr) {
    match socket.send_to(datagram, to) {
        Ok(_) => {}
        Err(error) if butterpollo_windows::net::datagram_lost(&error) => {
            tracing::debug!(%error, %to, "roaming reply dropped");
        }
        Err(error) => tracing::warn!(%error, %to, "roaming reply not sent"),
    }
}

/// Logs refused datagrams, one in [`REFUSAL_LOG_INTERVAL`]: a flood must not flood the log.
#[derive(Default)]
struct Refusals {
    logged: Option<Instant>,
    suppressed: u32,
}

impl Refusals {
    fn note(&mut self, now: Instant, refusal: Refusal, from: SocketAddr) {
        if self
            .logged
            .is_some_and(|at| now.saturating_duration_since(at) < REFUSAL_LOG_INTERVAL)
        {
            self.suppressed = self.suppressed.saturating_add(1);
            return;
        }
        tracing::info!(
            ?refusal,
            %from,
            suppressed = self.suppressed,
            "roaming datagram refused"
        );
        self.logged = Some(now);
        self.suppressed = 0;
    }
}

/// `address` with an IPv4-mapped IPv6 address as plain IPv4, for logs.
pub(super) fn canonical(address: SocketAddr) -> SocketAddr {
    SocketAddr::new(address.ip().to_canonical(), address.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::roaming::{Datagram, Header, Payload, RoamKeys, open, seal};

    const RIKEY: [u8; 16] = [7; 16];
    const CHALLENGE: u64 = 0x1234;

    fn client(seq: u32, payload: Payload) -> [u8; DATAGRAM_BYTES] {
        let keys = RoamKeys::derive(&RIKEY);
        let header = Header {
            kind: payload.kind(),
            session_tag: keys.session_tag,
            seq,
        };
        seal(&keys.key, header, payload)
    }

    /// A move between IPv4-mapped addresses, as a dual-stack control socket reports
    /// them: the aliases keep that form for ENet, and the log line has plain IPv4.
    #[test]
    fn a_move_maps_the_new_address_drops_the_old_and_ends_with_the_peer() {
        let socket = ReplySocket::bind("127.0.0.1:0").unwrap();
        let mut roaming = Roaming {
            enabled: true,
            table: RoamTable::with_challenges(|| CHALLENGE),
            ..Default::default()
        };
        let peer: SocketAddr = "[::ffff:192.168.1.20]:50000".parse().unwrap();
        let new: SocketAddr = "[::ffff:10.0.0.7]:41000".parse().unwrap();
        roaming.insert("s", &RIKEY, peer);
        let probe = client(
            1,
            Payload::Probe {
                nonce_c: 1,
                flags: 0,
            },
        );
        assert!(roaming.intercepts(&probe));
        assert!(!roaming.intercepts(&[0; DATAGRAM_BYTES]));
        let now = Instant::now();
        roaming.on_datagram(&socket, now, new, None, &probe);
        assert!(roaming.take_commits().is_empty(), "a probe moves nothing");
        assert_eq!(roaming.peer_of(new), Some(new));
        assert_eq!(roaming.wire_of(peer), peer);
        let confirm = client(
            2,
            Payload::Confirm {
                challenge: CHALLENGE,
            },
        );
        roaming.on_datagram(&socket, now, new, None, &confirm);
        let commits = roaming.take_commits();
        assert_eq!(commits.len(), 1);
        let commit = &commits[0];
        assert_eq!(
            (commit.id.as_str(), commit.peer, commit.old, commit.new),
            ("s", peer, peer, new)
        );
        assert_eq!(
            canonical(commit.new),
            "10.0.0.7:41000".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(roaming.peer_of(new), Some(peer));
        assert_eq!(roaming.peer_of(peer), None, "the address the client left");
        assert_eq!(roaming.wire_of(peer), new);
        let other: SocketAddr = "[::ffff:192.168.1.30]:50000".parse().unwrap();
        assert_eq!(roaming.peer_of(other), Some(other));
        assert_eq!(roaming.wire_of(other), other);

        roaming.retain_peers(&socket, |live| live == other);
        assert!(!roaming.has_peers());
        assert_eq!(roaming.peer_of(new), Some(new));
        assert_eq!(roaming.peer_of(peer), Some(peer));
        assert_eq!(roaming.wire_of(peer), peer);
    }

    /// A restarted control worker's socket shares the seqs: the host's CHALLENGE seq goes
    /// on under the same launch key, so no nonce repeats.
    #[test]
    fn a_new_control_socket_goes_on_with_the_launch_seqs() {
        let socket = ReplySocket::bind("127.0.0.1:0").unwrap();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let from = udp.local_addr().unwrap();
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let memory = SeqMemory::default();
        let keys = RoamKeys::derive(&RIKEY);
        let mut seqs = Vec::new();
        for seq in [1, 2] {
            let mut roaming = Roaming::remembering(memory.clone());
            roaming.set_enabled(true);
            roaming.insert("s", &RIKEY, peer);
            let probe = client(
                seq,
                Payload::Probe {
                    nonce_c: 1,
                    flags: 0,
                },
            );
            roaming.on_datagram(&socket, Instant::now(), from, None, &probe);
            let mut buffer = [0; 64];
            let (n, _) = udp.recv_from(&mut buffer).unwrap();
            let datagram = Datagram::decode(&buffer[..n]).unwrap();
            assert!(open(&keys.key, &datagram).is_some());
            seqs.push(datagram.header.seq);
        }
        assert_eq!(seqs, [1, 2]);
    }

    /// Off, roaming datagrams are ENet's, which drops them.
    #[test]
    fn turned_off_nothing_is_intercepted() {
        let roaming = Roaming::default();
        let probe = client(
            1,
            Payload::Probe {
                nonce_c: 1,
                flags: 0,
            },
        );
        assert!(!roaming.intercepts(&probe));
    }

    #[test]
    fn refusals_are_logged_once_in_ten_seconds() {
        let mut refusals = Refusals::default();
        let from: SocketAddr = "10.0.0.7:41000".parse().unwrap();
        let now = Instant::now();
        refusals.note(now, Refusal::UnknownSession, from);
        refusals.note(now + Duration::from_secs(1), Refusal::UnknownSession, from);
        refusals.note(now + Duration::from_secs(9), Refusal::UnknownSession, from);
        assert_eq!(refusals.suppressed, 2);
        refusals.note(now + REFUSAL_LOG_INTERVAL, Refusal::UnknownSession, from);
        assert_eq!(refusals.suppressed, 0);
        assert_eq!(refusals.logged, Some(now + REFUSAL_LOG_INTERVAL));
    }
}
