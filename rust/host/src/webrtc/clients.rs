//! The WebRTC devices: which connections the signalling server holds, the
//! pairing requests waiting in the console, and the credentials check.
//!
//! A paired device is an ordinary `Client` with an empty certificate and
//! `extra.webrtc` (see `butterpollo_core::state::Client::webrtc`). The
//! console's PIN page approves a request through `/api/pin`: the request
//! sits in `Host::pins` under its unguessable request ID with an empty
//! certificate, which no GameStream pairing has, and `/api/pin` hands the
//! entered PIN to the connection that is waiting for it.

use crate::state::Host;
use butterpollo_core::{
    state::Client,
    webrtc::{
        auth::{
            Challenge, ConnectionState, ConnectionTable, Credentials, Dropped, MAX_PAIRED,
            MAX_UNAUTHENTICATED, PendingPin, PendingPins,
        },
        protocol::ErrorCode,
    },
};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, hash_map::Entry},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

/// Why a connection is told to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// The same device connected again.
    Replaced,
    /// More than eight connections had not authenticated; this was the oldest.
    TooManyUnauthenticated,
}

/// Connections one peer may hold without authenticating. Lower than the
/// total, so one peer cannot push another peer's waiting TV out.
pub const MAX_UNAUTHENTICATED_PER_IP: usize = 4;
/// A pairing notification for one peer comes at most this often.
const NOTIFY_EVERY: Duration = Duration::from_secs(30);

/// What the console shows of the server.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub enabled: bool,
    pub signalling: Option<SocketAddr>,
    pub discovery: Option<SocketAddr>,
    pub error: Option<String>,
}

/// A registered connection: its number, and the channel that tells it to go.
pub struct Registered {
    pub id: u64,
    pub closed: mpsc::UnboundedReceiver<Close>,
}

struct Inner {
    table: ConnectionTable,
    pins: PendingPins,
    closers: HashMap<u64, mpsc::UnboundedSender<Close>>,
    /// The peer of every connection, oldest first.
    peers: Vec<(u64, IpAddr)>,
    /// When each peer last raised a pairing notification.
    notified: HashMap<IpAddr, Instant>,
    notify_every: Duration,
    /// Notifications granted so far.
    #[cfg(test)]
    notices: usize,
    next: u64,
}

/// What counts as one peer: an IPv4 address, or an IPv6 /64. One PC rotates
/// through many privacy addresses of its /64.
fn peer_group(peer: IpAddr) -> IpAddr {
    match peer {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        v4 => v4,
    }
}

pub struct Registry {
    inner: Mutex<Inner>,
    status: Mutex<Status>,
    changed: watch::Sender<u64>,
    epoch: Instant,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                table: ConnectionTable::default(),
                pins: PendingPins::new(),
                closers: HashMap::new(),
                peers: Vec::new(),
                notified: HashMap::new(),
                notify_every: NOTIFY_EVERY,
                #[cfg(test)]
                notices: 0,
                next: 1,
            }),
            status: Mutex::new(Status::default()),
            changed: watch::channel(0).0,
            epoch: Instant::now(),
        }
    }

    pub fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// The `webrtc` object of `/api/config`.
    pub fn status_json(&self, config: &butterpollo_core::config::Config) -> Value {
        let status = self.status();
        json!({
            "enabled": status.enabled,
            "running": status.signalling.is_some(),
            "discovery": status.discovery.is_some(),
            "port": config.webrtc_port(),
            "media_port": config.webrtc_media_port(),
            "error": status.error,
        })
    }

    /// Time on the clock the pairing requests use.
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    /// The paired list changed (a device was disabled, unpaired or renamed):
    /// every connection checks its device again.
    pub fn clients_changed(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// A new connection from `peer`, or None when it must be refused. A peer
    /// over its share of unauthenticated connections pushes out its own
    /// oldest, never another peer's; and when the total is full and the peer
    /// has none to push out, the new connection is refused.
    pub fn open(&self, peer: IpAddr) -> Option<Registered> {
        let peer = peer_group(peer);
        let mut inner = self.inner.lock().unwrap();
        let waiting: Vec<u64> = inner
            .peers
            .iter()
            .filter(|(id, ip)| {
                *ip == peer && inner.table.state(*id) == Some(&ConnectionState::Unauthenticated)
            })
            .map(|(id, _)| *id)
            .collect();
        let mut evict = None;
        if waiting.len() >= MAX_UNAUTHENTICATED_PER_IP {
            evict = Some(waiting[0]);
        } else if inner.table.unauthenticated() >= MAX_UNAUTHENTICATED {
            evict = Some(*waiting.first()?);
        }
        if let Some(id) = evict {
            inner.table.close(id);
            inner.peers.retain(|(other, _)| *other != id);
            inner.tell(
                vec![Dropped {
                    id,
                    was_streaming: false,
                }],
                Close::TooManyUnauthenticated,
            );
        }
        let id = inner.next;
        inner.next += 1;
        let (tx, closed) = mpsc::unbounded_channel();
        inner.closers.insert(id, tx);
        inner.peers.push((id, peer));
        let dropped = inner.table.open(id);
        inner.tell(dropped, Close::TooManyUnauthenticated);
        Some(Registered { id, closed })
    }

    /// Whether a pairing request from `peer` may raise a notification now:
    /// once per peer in 30 seconds.
    pub fn may_notify(&self, peer: IpAddr) -> bool {
        self.may_notify_at(peer, Instant::now())
    }

    /// [`Self::may_notify`] at `now`. Only a granted notice starts the window,
    /// so a peer asking every 20 seconds is told every 30, not never.
    fn may_notify_at(&self, peer: IpAddr, now: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let every = inner.notify_every;
        inner
            .notified
            .retain(|_, at| now.saturating_duration_since(*at) < every);
        let inner = &mut *inner;
        match inner.notified.entry(peer_group(peer)) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(now);
                #[cfg(test)]
                {
                    inner.notices += 1;
                }
                true
            }
        }
    }

    /// Shorten (or end) the window between notices.
    #[cfg(test)]
    pub fn set_notify_every(&self, every: Duration) {
        self.inner.lock().unwrap().notify_every = every;
    }

    /// Notifications granted so far.
    #[cfg(test)]
    pub fn notices(&self) -> usize {
        self.inner.lock().unwrap().notices
    }

    /// Connections that hold a slot.
    #[cfg(test)]
    pub fn slots(&self) -> usize {
        self.inner.lock().unwrap().peers.len()
    }

    /// The connection proved itself as `client_id`; its older connections go.
    pub fn authenticate(&self, id: u64, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = self.inner.lock().unwrap();
        let dropped = inner.table.authenticate(id, client_id)?;
        inner.tell(dropped, Close::Replaced);
        Ok(())
    }

    /// The connection ended.
    pub fn close(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.table.close(id);
        inner.closers.remove(&id);
        inner.peers.retain(|(other, _)| *other != id);
    }

    /// Connections held.
    #[cfg(test)]
    pub fn connections(&self) -> usize {
        self.inner.lock().unwrap().table.len()
    }

    /// The device's secret, if it is paired (a disabled device too: the proof
    /// is checked before its state is told).
    pub fn secret(&self, h: &Host, client_id: &str) -> Option<String> {
        h.paired
            .read()
            .unwrap()
            .webrtc_client(client_id)
            .and_then(|c| c.webrtc().map(|(_, secret)| secret.to_owned()))
    }

    /// The device if it is paired and enabled; otherwise why not.
    pub fn verify(&self, h: &Host, client_id: &str) -> Result<Client, ErrorCode> {
        let paired = h.paired.read().unwrap();
        let client = paired
            .webrtc_client(client_id)
            .ok_or(ErrorCode::AuthenticationFailed)?;
        if !client.enabled {
            return Err(ErrorCode::ClientDisabled);
        }
        Ok(client.clone())
    }

    /// A TV asked to pair. The request waits in the console's PIN list under
    /// the returned request ID until `/api/pin` sends the PIN the
    /// administrator typed, or it expires.
    pub fn begin_pairing(
        &self,
        h: &Host,
        pin: &str,
        name: &str,
    ) -> (String, oneshot::Receiver<(String, String)>) {
        let now = self.now();
        let request = PendingPin::new(&mut OsRng, pin, name, now);
        let id = request.request_id().to_owned();
        let (sender, receiver) = oneshot::channel();
        let mut inner = self.inner.lock().unwrap();
        inner.pins.prune(now);
        let mut pins = h.pins.lock().unwrap();
        if let Some(evicted) = inner.pins.add(request) {
            pins.remove(evicted.request_id());
        }
        pins.insert(
            id.clone(),
            crate::state::PendingPin {
                name: name.to_owned(),
                certificate: String::new(),
                created: Instant::now(),
                sender,
            },
        );
        (id, receiver)
    }

    /// The request is over without a pairing (the connection closed, or the
    /// TV asked again).
    pub fn cancel_pairing(&self, h: &Host, request_id: &str) {
        self.inner.lock().unwrap().pins.cancel(request_id);
        h.pins.lock().unwrap().remove(request_id);
    }

    /// The administrator entered `entered` for the request. The PIN is
    /// compared with the TV's in constant time, once; on a match the device is
    /// stored, named `name`, and its credentials returned.
    pub fn finish_pairing(
        &self,
        h: &Host,
        request_id: &str,
        entered: &str,
        name: &str,
    ) -> Result<Credentials, ErrorCode> {
        let approved = {
            let mut inner = self.inner.lock().unwrap();
            inner.pins.approve(request_id, entered, self.now())
        };
        let request = approved.map_err(|failure| failure.code())?;
        let credentials = Credentials::generate(&mut OsRng);
        let name = if name.trim().is_empty() {
            request.client_name()
        } else {
            name
        };
        let client = Client::paired_over_webrtc(
            name.to_owned(),
            &credentials.client_id,
            &credentials.secret,
        );
        let surplus = h
            .paired
            .write()
            .unwrap()
            .add_webrtc(&h.paired_path, client, MAX_PAIRED)
            .map_err(|error| {
                tracing::error!(error = %format!("{error:#}"), "a WebRTC device could not be saved");
                ErrorCode::PairingFailed
            })?;
        if !surplus.is_empty() {
            tracing::info!(
                count = surplus.len(),
                "the oldest WebRTC devices were unpaired to stay at 32"
            );
        }
        tracing::info!(client = name, "WebRTC device paired and saved");
        self.clients_changed();
        Ok(credentials)
    }

    /// A fresh challenge for a new connection.
    pub fn challenge(&self) -> Challenge {
        Challenge::new(&mut OsRng)
    }
}

impl Inner {
    fn tell(&mut self, dropped: Vec<Dropped>, reason: Close) {
        for Dropped { id, .. } in dropped {
            if let Some(closer) = self.closers.remove(&id) {
                let _ = closer.send(reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, last])
    }

    #[test]
    fn a_peer_over_its_share_pushes_out_only_its_own_oldest() {
        let registry = Registry::new();
        let mut a: Vec<_> = (0..4).map(|_| registry.open(ip(1)).unwrap()).collect();
        let mut b: Vec<_> = (0..4).map(|_| registry.open(ip(2)).unwrap()).collect();
        // The total (8) is full. Peer A asks again: its own oldest goes.
        let a5 = registry.open(ip(1)).unwrap();
        assert_eq!(a[0].closed.try_recv(), Ok(Close::TooManyUnauthenticated));
        for other in a[1..].iter_mut().chain(b.iter_mut()) {
            assert!(other.closed.try_recv().is_err(), "left alone");
        }
        assert_eq!(registry.connections(), 8);
        drop(a5);
    }

    #[test]
    fn a_new_peer_is_refused_when_the_total_is_full_and_nobody_is_pushed_out() {
        let registry = Registry::new();
        let mut held: Vec<_> = [1, 1, 1, 1, 2, 2, 2, 2]
            .into_iter()
            .map(|n| registry.open(ip(n)).unwrap())
            .collect();
        assert!(registry.open(ip(3)).is_none());
        for connection in &mut held {
            assert!(connection.closed.try_recv().is_err());
        }
        assert_eq!(registry.connections(), 8);
        // A connection that ends makes room.
        registry.close(held[0].id);
        assert!(registry.open(ip(3)).is_some());
    }

    #[test]
    fn authenticated_connections_do_not_count_against_a_peers_share() {
        let registry = Registry::new();
        let first = registry.open(ip(1)).unwrap();
        registry.authenticate(first.id, "client").unwrap();
        let mut others: Vec<_> = (0..4).map(|_| registry.open(ip(1)).unwrap()).collect();
        for other in &mut others {
            assert!(other.closed.try_recv().is_err());
        }
        assert_eq!(registry.connections(), 5);
    }

    #[test]
    fn a_peer_raises_a_pairing_notice_once_in_thirty_seconds() {
        let registry = Registry::new();
        assert!(registry.may_notify(ip(1)));
        assert!(!registry.may_notify(ip(1)));
        assert!(registry.may_notify(ip(2)));
    }

    #[test]
    fn asking_more_often_than_the_window_does_not_slide_it() {
        let registry = Registry::new();
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        assert!(registry.may_notify_at(ip(1), at(0)));
        // A TV asking every 20 seconds is refused at 20 ...
        assert!(!registry.may_notify_at(ip(1), at(20)));
        assert!(!registry.may_notify_at(ip(1), at(29)));
        // ... and told again 30 seconds after the last notice, not the last ask.
        assert!(registry.may_notify_at(ip(1), at(40)));
        assert!(!registry.may_notify_at(ip(1), at(60)));
        assert!(registry.may_notify_at(ip(1), at(70)));
    }

    fn v6(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn ipv6_privacy_addresses_of_one_prefix_count_as_one_peer() {
        let registry = Registry::new();
        let mut one_pc: Vec<_> = (1..=4)
            .map(|n| {
                registry
                    .open(v6(&format!("2001:db8:1:2:{n}:{n}:{n}:{n}")))
                    .unwrap()
            })
            .collect();
        // A fifth address of the same /64 pushes out the oldest of the PC.
        let _fifth = registry.open(v6("2001:db8:1:2::99")).unwrap();
        assert_eq!(
            one_pc[0].closed.try_recv(),
            Ok(Close::TooManyUnauthenticated)
        );
        // Another /64 has its own share.
        let other: Vec<_> = (1..=4)
            .map(|n| registry.open(v6(&format!("2001:db8:1:3::{n}"))).unwrap())
            .collect();
        for connection in one_pc[1..].iter_mut() {
            assert!(connection.closed.try_recv().is_err());
        }
        assert_eq!(other.len(), 4);
        // One notice per prefix.
        assert!(registry.may_notify(v6("2001:db8:1:2::7")));
        assert!(!registry.may_notify(v6("2001:db8:1:2:a:b:c:d")));
        assert!(registry.may_notify(v6("2001:db8:1:3::7")));
    }
}
