//! The control stream's round trip time, as the overlay and the stats line report it, is ENet's
//! smoothed estimate. Upstream ENet (and rusty_enet) smooths with truncating integer division,
//! which stops moving once a sample is within 7 ms of the mean, so a stream on a 2 ms link read
//! 7 to 8 ms against 2 ms on the C engine, whose moonlight-common-c ENet rounds up. The patched
//! copy in `protocol/vendor/rusty_enet` rounds up too.

use std::cell::Cell;
use std::convert::Infallible;
use std::rc::Rc;
use std::time::Duration;

use rusty_enet::{Event, Host, HostSettings, Packet, PeerID, ReadWrite};

type Clock = Rc<Cell<u64>>;

type Wire = ReadWrite<(), Infallible>;

fn host(clock: &Clock) -> Host<Wire> {
    let clock = clock.clone();
    Host::new(Wire::new(), HostSettings {
        peer_limit: 1,
        channel_limit: 1,
        time: Box::new(move || Duration::from_millis(clock.get())),
        ..Default::default()
    }).unwrap()
}

/// Deliver every datagram before advancing the synthetic ENet clock. An OS
/// scheduling delay must not become an extra RTT sample in this rounding test.
fn deliver(from: &mut Host<Wire>, to: &mut Host<Wire>) {
    from.flush();
    while let Some(((), packet)) = from.socket_mut().read() {
        to.socket_mut().write((), packet);
    }
}

fn connect(client: &mut Host<Wire>, server: &mut Host<Wire>) -> PeerID {
    let peer = client.connect((), 1, 0).unwrap().id();
    for _ in 0..8 {
        deliver(client, server);
        while server.service().unwrap().is_some() {}
        deliver(server, client);
        while let Some(event) = client.service().unwrap() {
            if matches!(event, Event::Connect { .. }) { return peer; }
        }
    }
    panic!("the peers did not complete their handshake");
}

fn exchange(clock: &Clock, client: &mut Host<Wire>, server: &mut Host<Wire>, peer: PeerID, latency_ms: u64) {
    client.peer_mut(peer).send(0, &Packet::reliable(&b"x"[..])).unwrap();
    deliver(client, server);
    clock.set(clock.get() + latency_ms);
    let mut received = false;
    while let Some(event) = server.service().unwrap() {
        received |= matches!(event, Event::Receive { .. });
    }
    assert!(received, "the server received the sample");
    deliver(server, client);
    while client.service().unwrap().is_some() {}
    clock.set(clock.get() + 20);
}

#[test]
fn the_smoothed_round_trip_time_converges_down_to_the_samples() {
    let clock: Clock = Rc::new(Cell::new(1_000));
    let mut server = host(&clock);
    let mut client = host(&clock);
    let peer = connect(&mut client, &mut server);

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
