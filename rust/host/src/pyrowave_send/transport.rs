use super::*;
use butterpollo_core::network_pacing::{self, ClientPath, Pacer};
use butterpollo_windows::{
    net::{Batch, Link},
    timing::Timer,
};

pub(super) fn run(
    shared: &Slot,
    socket: Arc<ReplySocket>,
    current: Arc<Session>,
    config: Config,
    host: Shared,
    start: Instant,
) -> Result<()> {
    let _priority = butterpollo_windows::capture::Priority::new();
    let timer = Timer::new()?;
    let mut batch = Batch::default();
    let mut pacer = Pacer::new(Instant::now());
    let mut route_check = RouteCheck::new(Instant::now());
    let mut send_buffer = SendBuffer::default();
    // The route lookup runs on its own thread, not between frames here.
    let link_watch = crate::video_send::link_watch::LinkWatch::default();
    while !shared.stop.load(Ordering::Acquire)
        && !current.stopping()
        && !host.stop.load(Ordering::Acquire)
    {
        let frame = {
            let mut ready = shared.ready.lock().unwrap();
            if ready.frame.is_none() {
                ready = shared
                    .ready_changed
                    .wait_timeout(ready, Duration::from_millis(20))
                    .unwrap()
                    .0;
            }
            let frame = ready.frame.take();
            ready.sending |= frame.is_some();
            frame
        };
        let Some(mut frame) = frame else { continue };
        let report = route_check.due(Instant::now(), frame.peer);
        let route = link_watch.get(frame.peer);
        let client = client_path(&current);
        let bps = pace(&current, &config, frame.kbps, frame.wire_frame, route);
        send_buffer.fit(&socket, bps);
        if report {
            network_pacing::report_rate(
                &current.launch.warnings,
                bps,
                frame.demand,
                0, // PyroWave caps are hard limits; no encoder-bitrate floor.
            );
            if bps < frame.demand {
                tracing::warn!(
                    pacing_bps = bps,
                    wire_demand_bps = frame.demand,
                    client_pace_bps = client.pace_bps,
                    "PyroWave wire demand exceeds the available pace; lower bitrate or enable host bitrate adaptation"
                );
            }
        }
        let mut sent = 0;
        let mut sent_packets = 0;
        let mut first_sent = None;
        let packets_dropped = batch.dropped;
        let ipv6 = frame.peer.ip().to_canonical().is_ipv6();
        current
            .stats
            .video_frame
            .store(u64::from(frame.wire_frame) + 1, Ordering::Release);
        let mut handed = std::mem::take(&mut frame.blocks).into_iter();
        while let Some(packets) = handed
            .next()
            .or_else(|| frame.streamed.then(|| next_block(shared)).flatten())
        {
            let mut remaining = packets.as_slice();
            while !remaining.is_empty() {
                if shared.stop.load(Ordering::Acquire)
                    || current.stopping()
                    || host.stop.load(Ordering::Acquire)
                {
                    return Ok(());
                }
                if pacer.due() > Instant::now() {
                    timer.until_paced_or_signal(pacer.due(), &shared.stop_signal)?;
                }
                if shared.stop.load(Ordering::Acquire)
                    || current.stopping()
                    || host.stop.load(Ordering::Acquire)
                {
                    return Ok(());
                }
                let count = Batch::count(remaining, batch_limit(bps, remaining[0].len(), ipv6));
                let bytes = batch.send(&socket, &remaining[..count], frame.peer)?;
                if bytes > 0 {
                    current
                        .record_transport_sent(&remaining[..count], network_pacing::overhead(ipv6));
                }
                first_sent.get_or_insert_with(Instant::now);
                sent += bytes;
                let left = if bytes > 0 { count } else { 0 };
                sent_packets += left;
                pacer.sent(Instant::now(), bytes, left, ipv6, bps);
                current
                    .stats
                    .packets
                    .fetch_add(count as u64, Ordering::Relaxed);
                current
                    .stats
                    .bytes
                    .fetch_add(bytes as u64, Ordering::Relaxed);
                remaining = &remaining[count..];
            }
            shared.pool.give(packets);
        }
        // The next frame's blocks may leave as they are built.
        let abandoned = {
            let mut ready = shared.ready.lock().unwrap();
            ready.sending = false;
            std::mem::take(&mut ready.abandoned)
        };
        if batch.dropped != packets_dropped {
            host_dropped(&current, frame.wire_frame);
        }
        // What left for the client, which the link-rate controller weighs the
        // client's loss reports against.
        current.record_link_sent(
            frame.wire_frame,
            sent_packets,
            (sent + sent_packets * network_pacing::overhead(ipv6)) as u64,
        );
        // The packetizer dropped the rest of it (and counted it dropped): no
        // frame was sent.
        if abandoned {
            continue;
        }
        current
            .stats
            .latency_us
            .store(frame.latency, Ordering::Relaxed);
        current.stats.frames.fetch_add(1, Ordering::Relaxed);
        let sent_at = Instant::now();
        current.stats.performance.lock().unwrap().record_timing(
            sent_at,
            butterpollo_core::performance::Timing {
                period: current.stream_mode().period(),
                encode: frame.latency,
                host: frame.processing,
                age: frame.age,
                sent: micros(sent_at.saturating_duration_since(frame.claimed)),
            },
            sent as u64,
        );
        // A frame with packets dropped on send never had its last packet sent.
        if let Some(claim) = frame.claim.filter(|_| batch.dropped == packets_dropped) {
            let mut stamps = claim.sent(
                frame.polled,
                frame.packetized,
                first_sent.unwrap_or(sent_at),
                sent_at,
            );
            stamps.prepared = frame.prepared;
            current
                .stats
                .performance
                .lock()
                .unwrap()
                .record_stages(&stamps, frame.processing);
            current.record_sent(frame.wire_frame, frame.timestamp, &stamps, start);
        }
    }
    Ok(())
}

/// The next block of the streamed frame being sent, waiting while the
/// packetizer builds it; none once the frame is complete or abandoned.
pub(super) fn next_block(shared: &Slot) -> Option<PacketBlock> {
    let mut ready = shared.ready.lock().unwrap();
    loop {
        if let Some(block) = ready.blocks.pop_front() {
            return Some(block);
        }
        if !ready.open || ready.abandoned || shared.stop.load(Ordering::Acquire) {
            return None;
        }
        ready = shared
            .ready_changed
            .wait_timeout(ready, Duration::from_millis(20))
            .unwrap()
            .0;
    }
}

/// The video socket's send buffer, sized to the pace
/// ([`network_pacing::paced_send_buffer`]). Set again only when the size
/// changes, which the whole-MiB steps keep rare.
struct SendBuffer {
    /// The size last asked for; the socket starts at the floor.
    size: usize,
    /// The first resize is logged at info, a refused size once at warn.
    logged: bool,
    refused: bool,
}

impl Default for SendBuffer {
    fn default() -> Self {
        Self {
            size: network_pacing::SEND_BUFFER_FLOOR,
            logged: false,
            refused: false,
        }
    }
}

impl SendBuffer {
    /// The size the socket needs at `bps`, if it differs from the last one.
    fn wanted(&self, bps: u64) -> Option<usize> {
        let size = network_pacing::paced_send_buffer(bps);
        (size != self.size).then_some(size)
    }

    fn fit(&mut self, socket: &std::net::UdpSocket, bps: u64) {
        let Some(size) = self.wanted(bps) else {
            return;
        };
        self.size = size;
        match socket2::SockRef::from(socket).set_send_buffer_size(size) {
            Ok(()) if !self.logged => {
                self.logged = true;
                tracing::info!(bytes = size, pacing_bps = bps, "video send buffer sized");
            }
            Ok(()) => {
                tracing::debug!(bytes = size, pacing_bps = bps, "video send buffer sized");
            }
            Err(error) if !self.refused => {
                self.refused = true;
                tracing::warn!(bytes = size, %error, "video send buffer not resized");
            }
            Err(_) => {}
        }
    }
}

/// How often the sender routes its client's address again.
const ROUTE_INTERVAL: Duration = Duration::from_secs(2);

/// When the sender routes its client's address again: every [`ROUTE_INTERVAL`], and at
/// once for a new address (a roam), whose path takes another route, as H.26x video does.
struct RouteCheck {
    due: Instant,
    /// The address last routed.
    routed: Option<SocketAddr>,
}

impl RouteCheck {
    fn new(now: Instant) -> Self {
        Self {
            due: now,
            routed: None,
        }
    }

    /// Whether to route again now, to send to `peer`.
    fn due(&mut self, now: Instant, peer: SocketAddr) -> bool {
        if !crate::video_send::recovery::new_route(&mut self.routed, peer) && now < self.due {
            return false;
        }
        self.due = now + ROUTE_INTERVAL;
        true
    }
}

/// The client's hints, until it roams away from the path they describe.
fn client_path(current: &Session) -> ClientPath {
    ClientPath {
        link_bps: u64::from(current.client_link_mbps()) * 1_000_000,
        pace_bps: u64::from(current.client_pace_mbps()) * 1_000_000,
    }
}

/// The pace for picture `wire_frame` of `kbps` on `route`.
fn pace(current: &Session, config: &Config, kbps: u32, wire_frame: u32, route: Link) -> u64 {
    let client = client_path(current);
    // Use a stable budget so different scene sizes cannot continually
    // reset the controller's feedback window. Warn on actual wire demand.
    let needed = network_pacing::pyrowave_wire_rate_bps(
        current.bitrate.load(Ordering::Acquire),
        current.config.packet_size,
        current.config.pyrowave_records,
        current.stream_mode().period(),
    );
    let base = network_pacing::pyrowave_route_rate_bps(
        config.integer("pacing_max_bitrate_kbps", 0),
        kbps,
        route.bps,
        network_pacing::route_is_wireless(
            route.wireless,
            current.client_wireless_sets_route_pacing(),
        ),
        needed,
        client,
    );
    let paced = current.video_pacing_bps(base, needed);
    let safe = network_pacing::pyrowave_route_rate_bps(
        0,
        kbps,
        route.bps,
        true,
        needed,
        ClientPath {
            pace_bps: 0,
            ..client
        },
    )
    .min(needed.saturating_add(needed / 10).max(1_000_000));
    current.keyframe_loss_bps(paced, safe, wire_frame, true)
}

/// The socket refused packets of this frame. That is the host's loss, not
/// the network's: the client's report of the frame is not counted against the
/// link, and the link-rate controller counts it as a send loss (E3).
fn host_dropped(current: &Session, wire_frame: u32) {
    current.keyframe_abandoned(wire_frame);
    current.request_send_loss_recovery();
    current.launch.warnings.event("network_send", "PyroWave video packets were dropped by the host after transient socket send failures. Lower bitrate and check the network adapter; the log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::session::Role;

    #[test]
    fn a_roam_routes_the_new_path_at_once() {
        let now = Instant::now();
        let (old, new): (SocketAddr, SocketAddr) = (
            "192.0.2.1:9".parse().unwrap(),
            "198.51.100.4:9".parse().unwrap(),
        );
        let mut check = RouteCheck::new(now);
        assert!(check.due(now, old), "the first picture");
        assert!(!check.due(now + Duration::from_millis(10), old));
        assert!(check.due(now + Duration::from_millis(20), new), "a roam");
        assert!(!check.due(now + Duration::from_millis(30), new));
        assert!(check.due(now + Duration::from_millis(20) + ROUTE_INTERVAL, new));
    }

    /// The video socket's buffer follows the pace in whole MiB steps, and is
    /// set only when the step changes; a socket keeps a size it was given.
    #[test]
    fn the_send_buffer_follows_the_pace_and_is_set_only_on_a_change() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut buffer = SendBuffer::default();
        assert_eq!(buffer.wanted(400_000_000), None, "H.26x rates: the floor");
        assert_eq!(buffer.wanted(1_750_000_000), Some(3 << 20));
        buffer.fit(&socket, 1_750_000_000);
        assert_eq!(buffer.size, 3 << 20);
        let kernel = socket2::SockRef::from(&socket).send_buffer_size().unwrap();
        assert!(kernel >= 3 << 20, "{kernel}");
        assert_eq!(buffer.wanted(1_800_000_000), None, "the same step");
        assert_eq!(buffer.wanted(2_600_000_000), Some(4 << 20));
        // The pace falls back (a send loss): the buffer does too.
        buffer.fit(&socket, 500_000_000);
        assert_eq!(buffer.size, network_pacing::SEND_BUFFER_FLOOR);
    }

    /// A client's link and pace hints pace a PyroWave stream until the client roams:
    /// the new path is paced as for a client that sent none.
    #[test]
    fn a_roamed_clients_old_link_hints_no_longer_pace_pyrowave() {
        let fixture = crate::state::test_support::Fixture::new();
        let stream = |link, pace| {
            let session = Session::new(
                fixture.launch(fixture.client(0), Role::Stream),
                butterpollo_core::rtsp::Negotiated {
                    codec: 3,
                    bitrate_kbps: 40_000,
                    pyrowave_client_link_mbps: link,
                    pyrowave_client_pace_mbps: pace,
                    ..Default::default()
                },
            );
            session.bitrate.store(40_000, Ordering::Release);
            session.configure_keyframe_loss(true);
            session
        };
        let config = Config::default();
        let route = Link {
            bps: 1_000_000_000,
            ..Default::default()
        };
        let unhinted = pace(&stream(0, 0), &config, 40_000, 1, route);
        let session = stream(50, 45);
        assert_eq!(pace(&session, &config, 40_000, 1, route), 45_000_000);
        assert_ne!(unhinted, 45_000_000);
        session.roamed("198.51.100.4".parse().unwrap(), Instant::now());
        session.new_network_path();
        assert_eq!(
            (
                client_path(&session).link_bps,
                client_path(&session).pace_bps
            ),
            (0, 0)
        );
        assert_eq!(pace(&session, &config, 40_000, 2, route), unhinted);
    }

    /// A latched `KeyframeLoss` must slow an unhinted PyroWave stream: before
    /// the fix, `safe` equalled `base` and a socket-refused keyframe changed
    /// neither the rate nor the packet burst size.
    #[test]
    fn a_latched_keyframe_loss_paces_an_unhinted_pyrowave_stream_slower() {
        let fixture = crate::state::test_support::Fixture::new();
        let session = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_core::rtsp::Negotiated {
                codec: 3,
                bitrate_kbps: 40_000,
                ..Default::default()
            },
        );
        session.bitrate.store(40_000, Ordering::Release);
        session.configure_keyframe_loss(true);
        let config = Config::default();
        let route = Link {
            bps: 1_000_000_000,
            ..Default::default()
        };
        let needed = network_pacing::pyrowave_wire_rate_bps(
            40_000,
            session.config.packet_size,
            session.config.pyrowave_records,
            session.stream_mode().period(),
        );
        // Frame 1 registers the keyframe, then the socket refuses it.
        assert_eq!(pace(&session, &config, 40_000, 1, route), needed * 5 / 4);
        session.keyframe_abandoned(1);
        assert!(session.keyframe_loss_active());
        assert_eq!(
            pace(&session, &config, 40_000, 2, route),
            needed + needed / 10
        );
    }

    /// Review 2026-10-29: packets the socket refused were counted as the
    /// network's loss when the client reported the frame, and never as the
    /// host's.
    #[test]
    fn a_frame_the_socket_refused_is_the_hosts_loss() {
        let fixture = crate::state::test_support::Fixture::new();
        let session = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_core::rtsp::Negotiated {
                codec: 3,
                pyrowave_records: true,
                bitrate_kbps: 400_000,
                ..Default::default()
            },
        );
        host_dropped(&session, 7);
        assert_eq!(session.stats.frames_lost_on_send.load(Ordering::Relaxed), 1);
        assert_eq!(
            session.stats.send_loss_recoveries.load(Ordering::Relaxed),
            1
        );
        assert!(session.keyframe_loss.lock().unwrap().is_abandoned(7));
        assert!(!session.keyframe_loss.lock().unwrap().is_abandoned(8));
        // Review 2026-10-29: and the link-rate controller counts it as a send
        // loss (E3): three such frames lower the bitrate.
        session.configure_link_rate(true, 0);
        for frame in 1..=60 {
            session.record_link_sent(frame, 300, 420_000);
        }
        session.stats.video_frame.store(70, Ordering::Release);
        for frame in 50..=52 {
            host_dropped(&session, frame);
        }
        let (from, to) = session
            .tick_link_rate(
                Instant::now() + Duration::from_millis(200),
                Duration::from_millis(10),
            )
            .expect("three frames the socket refused lower the bitrate");
        assert!(to < from, "{from} -> {to}");
        assert_eq!(session.info()["link_rate"]["state"], "limited");
    }
}
