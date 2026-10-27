//! The host's ENet after an outage (rust/vendor/rusty_enet), on a simulated network in
//! virtual time: a host peer as the control stream sets it up and a client that, like
//! Moonlight, sends an unreliable heartbeat every 100 ms. Each case runs with the resend
//! after a silence and without it, which is stock ENet.

use std::{cell::Cell, collections::VecDeque, net::SocketAddr, rc::Rc, time::Duration};

use rusty_enet::{Event, Host, HostSettings, Packet, PeerID, ReadWrite};

type Socket = ReadWrite<SocketAddr, std::io::Error>;

/// One-way delay in each direction.
const DELAY_MS: u64 = 10;
const HEARTBEAT_MS: u64 = 100;

fn host_address() -> SocketAddr {
    "192.0.2.1:47999".parse().unwrap()
}
fn client_address() -> SocketAddr {
    "192.0.2.2:50000".parse().unwrap()
}

struct Net {
    now: Rc<Cell<u64>>,
    host: Host<Socket>,
    client: Host<Socket>,
    resend_after_silence: bool,
    host_peer: Option<PeerID>,
    client_peer: PeerID,
    client_connected: bool,
    to_host: VecDeque<(u64, Vec<u8>)>,
    to_client: VecDeque<(u64, Vec<u8>)>,
    to_host_down: bool,
    to_client_down: bool,
    /// Datagrams lost in either direction, out of 100.
    loss_percent: u64,
    random: u64,
    dropped: u64,
    /// Datagrams the host sent.
    host_sent: u64,
    /// What the client received from the host, with the time.
    received: Vec<(u64, Vec<u8>)>,
    host_disconnected_at: Option<u64>,
}

impl Net {
    fn new(resend_after_silence: bool, loss_percent: u64) -> Self {
        // ENet takes 0 for "never"; start well away from it.
        let now = Rc::new(Cell::new(1_000_000));
        let settings = || {
            let now = now.clone();
            HostSettings {
                peer_limit: 1,
                channel_limit: 1,
                time: Box::new(move || Duration::from_millis(now.get())),
                seed: Some(7),
                ..Default::default()
            }
        };
        let host = Host::new(Socket::new(), settings()).unwrap();
        let mut client = Host::new(Socket::new(), settings()).unwrap();
        let peer = client.connect(host_address(), 1, 0).unwrap();
        // Like the reconnect harness's client: never the side that gives up.
        peer.set_timeout(32, 60_000, 60_000);
        let client_peer = peer.id();
        let mut net = Self {
            now,
            host,
            client,
            resend_after_silence,
            host_peer: None,
            client_peer,
            client_connected: false,
            to_host: VecDeque::new(),
            to_client: VecDeque::new(),
            to_host_down: false,
            to_client_down: false,
            loss_percent,
            random: 0x9e37_79b9_7f4a_7c15,
            dropped: 0,
            host_sent: 0,
            received: Vec::new(),
            host_disconnected_at: None,
        };
        net.run_until(5_000, |net| net.client_connected && net.host_peer.is_some());
        // Let the round trip time settle on the link's.
        net.run(2_000);
        net
    }

    fn now(&self) -> u64 {
        self.now.get()
    }

    fn lost(&mut self, down: bool) -> bool {
        if down {
            return true;
        }
        if self.loss_percent == 0 {
            return false;
        }
        // xorshift64: the same losses in both runs of a case.
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random % 100 < self.loss_percent
    }

    /// One millisecond: deliver what is due, service both ends, put what they sent on the wire.
    fn tick(&mut self) {
        let now = self.now() + 1;
        self.now.set(now);
        while self.to_host.front().is_some_and(|(at, _)| *at <= now) {
            let (_, datagram) = self.to_host.pop_front().unwrap();
            self.host.socket_mut().write(client_address(), datagram);
        }
        while self.to_client.front().is_some_and(|(at, _)| *at <= now) {
            let (_, datagram) = self.to_client.pop_front().unwrap();
            self.client.socket_mut().write(host_address(), datagram);
        }
        while let Some(event) = self.host.service().unwrap() {
            match event {
                Event::Connect { peer, .. } => {
                    // As the control stream sets up its peers (stream.rs).
                    peer.set_timeout(32, 15_000, 30_000);
                    peer.set_resend_after_silence(self.resend_after_silence);
                    self.host_peer = Some(peer.id());
                }
                Event::Disconnect { .. } => self.host_disconnected_at = Some(now),
                Event::Receive { .. } => {}
            }
        }
        if self.client_connected && now % HEARTBEAT_MS == 0 {
            self.client
                .peer_mut(self.client_peer)
                .send(0, &Packet::always_unreliable_unsequenced(&[0, 2][..]))
                .unwrap();
        }
        while let Some(event) = self.client.service().unwrap() {
            match event {
                Event::Connect { .. } => self.client_connected = true,
                Event::Disconnect { .. } => panic!("the client's ENet gave up"),
                Event::Receive { packet, .. } => {
                    self.received.push((now, packet.data().to_vec()));
                }
            }
        }
        while let Some((address, datagram)) = self.host.socket_mut().read() {
            assert_eq!(address, client_address());
            self.host_sent += 1;
            if self.lost(self.to_client_down) {
                self.dropped += 1;
            } else {
                self.to_client.push_back((now + DELAY_MS, datagram));
            }
        }
        while let Some((address, datagram)) = self.client.socket_mut().read() {
            assert_eq!(address, host_address());
            if self.lost(self.to_host_down) {
                self.dropped += 1;
            } else {
                self.to_host.push_back((now + DELAY_MS, datagram));
            }
        }
    }

    fn run(&mut self, ms: u64) {
        for _ in 0..ms {
            self.tick();
        }
    }

    fn run_until(&mut self, ms: u64, done: impl Fn(&Self) -> bool) {
        for _ in 0..ms {
            if done(self) {
                return;
            }
            self.tick();
        }
        assert!(done(self), "not done after {ms} ms");
    }

    fn network(&mut self, down: bool) {
        self.to_host_down = down;
        self.to_client_down = down;
    }

    fn host_send(&mut self, message: &[u8]) {
        let peer = self.host_peer.unwrap();
        self.host
            .peer_mut(peer)
            .send(0, &Packet::reliable(message))
            .unwrap();
    }

    fn arrival(&self, message: &[u8]) -> Option<u64> {
        self.received
            .iter()
            .find(|(_, data)| data == message)
            .map(|(at, _)| *at)
    }
}

/// How long after an outage of `outage_ms` (10 s or more) ends the last of the messages the
/// host queued in its first 10 s arrives, or None when the host dropped the client first.
fn delay_after_outage(resend_after_silence: bool, outage_ms: u64) -> Option<u64> {
    let mut net = Net::new(resend_after_silence, 0);
    net.network(true);
    let messages: Vec<Vec<u8>> = (0..10_u8).map(|i| vec![0x55, i]).collect();
    for message in &messages {
        net.run(500);
        net.host_send(message);
        net.run(500);
    }
    net.run(outage_ms - 10_000);
    net.network(false);
    let restored = net.now();
    net.run_until(30_000, |net| {
        net.host_disconnected_at.is_some()
            || messages
                .iter()
                .all(|message| net.arrival(message).is_some())
    });
    if net.host_disconnected_at.is_some() {
        return None;
    }
    messages
        .iter()
        .map(|message| net.arrival(message).unwrap() - restored)
        .max()
}

#[test]
fn messages_queued_in_an_outage_arrive_once_the_client_is_heard_again() {
    // Outages that end at different points of the doubled timeouts.
    let mut stock_worst = Some(0);
    for outage_ms in (0..4).map(|k| 10_000 + k * 250) {
        let stock = delay_after_outage(false, outage_ms);
        let delay = delay_after_outage(true, outage_ms);
        eprintln!(
            "{outage_ms} ms outage, ms until the last message arrived (None: dropped): \
             stock {stock:?}, resend after silence {delay:?}"
        );
        // The client's next heartbeat, then a round trip.
        assert!(
            delay.is_some_and(|delay| delay <= HEARTBEAT_MS + 2 * DELAY_MS + 20),
            "{delay:?} ms after a {outage_ms} ms outage"
        );
        stock_worst = stock_worst
            .zip(stock)
            .map(|(worst, stock)| worst.max(stock));
    }
    assert!(
        stock_worst.is_none_or(|worst| worst >= 1_000),
        "stock ENet delivered within {stock_worst:?} ms: the cases no longer show the backoff"
    );
}

/// The host's datagrams, the client's arrivals, when the host dropped the client and the
/// datagrams lost over a run where the host sends a reliable message every 20 ms.
fn steady_traffic(
    resend_after_silence: bool,
    loss_percent: u64,
    to_client_down: bool,
    messages: u32,
) -> (u64, Vec<(u64, Vec<u8>)>, Option<u64>, u64) {
    let mut net = Net::new(resend_after_silence, loss_percent);
    net.to_client_down = to_client_down;
    let start = net.host_sent;
    for i in 0..messages {
        net.run(20);
        if net.host_disconnected_at.is_none() {
            net.host_send(&i.to_be_bytes());
        }
    }
    net.run(5_000);
    (
        net.host_sent - start,
        net.received,
        net.host_disconnected_at,
        net.dropped,
    )
}

#[test]
fn steady_loss_resends_exactly_as_stock_enet() {
    let stock = steady_traffic(false, 5, false, 3_000);
    let patched = steady_traffic(true, 5, false, 3_000);
    eprintln!(
        "5 % loss: host sent {} datagrams, {} lost",
        stock.0, stock.3
    );
    assert!(stock.3 > 0, "nothing was lost");
    assert_eq!(stock.2, None);
    assert_eq!(stock.1.len(), 3_000, "not every message arrived");
    assert_eq!(patched.0, stock.0, "datagrams the host sent");
    assert_eq!(patched.1, stock.1, "arrivals");
}

/// A dead host-to-client path while the client keeps sending: no resend storm, and ENet drops
/// the client when it would have.
#[test]
fn a_one_way_outage_resends_and_disconnects_as_stock_enet() {
    // 30 s: the host drops the client after its 15 s minimum, the client keeps its 60 s.
    let stock = steady_traffic(false, 0, true, 1_500);
    let patched = steady_traffic(true, 0, true, 1_500);
    assert!(
        stock.2.is_some(),
        "stock ENet kept a client it never heard back from"
    );
    assert_eq!(patched.0, stock.0, "datagrams the host sent");
    assert_eq!(patched.2, stock.2, "when the host dropped the client");
}
