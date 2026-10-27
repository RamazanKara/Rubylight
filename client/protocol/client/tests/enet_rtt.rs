//! The control stream's round trip time, as the overlay and the stats line report it, is ENet's
//! smoothed estimate. Upstream ENet (and rusty_enet) smooths with truncating integer division,
//! which stops moving once a sample is within 7 ms of the mean, so a stream on a 2 ms link read
//! 7 to 8 ms against 2 ms on the C engine, whose moonlight-common-c ENet rounds up. The patched
//! copy in `protocol/vendor/rusty_enet` rounds up too.

use std::cell::Cell;
use std::net::{SocketAddr, UdpSocket};
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use rusty_enet::{Event, Host, HostSettings, Packet, PeerID};

type Clock = Rc<Cell<u64>>;

fn host(clock: &Clock, peers: usize) -> (Host<UdpSocket>, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let clock = clock.clone();
    let host = Host::new(
        socket,
        HostSettings {
            peer_limit: peers,
            channel_limit: 1,
            time: Box::new(move || Duration::from_millis(clock.get())),
            ..Default::default()
        },
    )
    .unwrap();
    (host, address)
}

/// Services `host` until it has an event of `wanted` kind, with real time only bounding the wait
/// for loopback delivery (the ENet clock is the test's own).
fn until(host: &mut Host<UdpSocket>, other: &mut Host<UdpSocket>, wanted: impl Fn(&Event<UdpSocket>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        while let Some(event) = host.service().unwrap() {
            if wanted(&event) {
                return;
            }
        }
        // Let the other side answer (a connect handshake needs both).
        while other.service().unwrap().is_some() {}
        thread::sleep(Duration::from_millis(1));
    }
    panic!("timed out");
}

/// One reliable packet and its acknowledgement with `latency_ms` between the client's send and
/// the client reading the acknowledgement; the client's RTT sample is `latency_ms`.
fn exchange(clock: &Clock, client: &mut Host<UdpSocket>, server: &mut Host<UdpSocket>, peer: PeerID, latency_ms: u64) {
    client.peer_mut(peer).send(0, &Packet::reliable(&b"x"[..])).unwrap();
    // The send is stamped with the clock at this flush.
    while client.service().unwrap().is_some() {}
    clock.set(clock.get() + latency_ms);
    until(server, client, |event| matches!(event, Event::Receive { .. }));
    // The server answers in its next pass, still at the same ENet time; the client reads the
    // acknowledgement then (real time only gives the loopback datagrams a moment to land).
    while server.service().unwrap().is_some() {}
    thread::sleep(Duration::from_millis(3));
    while client.service().unwrap().is_some() {}
    clock.set(clock.get() + 20);
}

#[test]
fn the_smoothed_round_trip_time_converges_down_to_the_samples() {
    let clock: Clock = Rc::new(Cell::new(1_000));
    let (mut server, server_address) = host(&clock, 1);
    let (mut client, _) = host(&clock, 1);
    let peer = client.connect(server_address, 1, 0).unwrap().id();
    until(&mut client, &mut server, |e| matches!(e, Event::Connect { .. }));

    // A slow stretch (the first exchanges on a radio that has just woken up) ...
    for _ in 0..40 {
        exchange(&clock, &mut client, &mut server, peer, 40);
    }
    let slow = client.peer(peer).round_trip_time();
    assert!(slow >= Duration::from_millis(30), "slow stretch read {slow:?}");

    // ... then a 2 ms link. Truncating division left the estimate at 9 ms; it must follow.
    for _ in 0..120 {
        exchange(&clock, &mut client, &mut server, peer, 2);
    }
    let settled = client.peer(peer).round_trip_time();
    assert!(
        settled <= Duration::from_millis(3),
        "a 2 ms link reads {settled:?}, expected 2 to 3 ms"
    );
    // The variance decays by the same rounding: truncation leaves it at 3 ms after the 40 ms
    // stretch, rounding up lets it settle on a steady link.
    let variance = client.peer(peer).round_trip_time_variance();
    assert!(
        variance <= Duration::from_millis(1),
        "a steady 2 ms link leaves the variance at {variance:?}, expected 1 ms or less"
    );
}
