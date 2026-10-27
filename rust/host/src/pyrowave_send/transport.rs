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
    let mut route = butterpollo_windows::net::Link::default();
    let mut route_check = RouteCheck::new(Instant::now());
    while !shared.stop.load(Ordering::Acquire)
        && !current.stopping()
        && !host.stop.load(Ordering::Acquire)
    {
        let frame = {
            let mut ready = shared.ready.lock().unwrap();
            if ready.is_none() {
                ready = shared
                    .ready_changed
                    .wait_timeout(ready, Duration::from_millis(20))
                    .unwrap()
                    .0;
            }
            ready.take()
        };
        let Some(frame) = frame else { continue };
        let report = route_check.due(Instant::now(), frame.peer);
        if report {
            route = butterpollo_windows::net::routed_link(frame.peer);
        }
        let client = client_path(&current);
        let bps = pace(&current, &config, frame.kbps, frame.wire_frame, route);
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
        let mut first_sent = None;
        let packets_dropped = batch.dropped;
        current
            .stats
            .video_frame
            .store(u64::from(frame.wire_frame) + 1, Ordering::Release);
        for packets in frame.blocks {
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
                let count = Batch::count(
                    remaining,
                    batch_limit(
                        bps,
                        remaining[0].len(),
                        frame.peer.ip().to_canonical().is_ipv6(),
                    ),
                );
                let bytes = batch.send(&socket, &remaining[..count], frame.peer)?;
                first_sent.get_or_insert_with(Instant::now);
                sent += bytes;
                pacer.sent(
                    Instant::now(),
                    bytes,
                    if bytes > 0 { count } else { 0 },
                    frame.peer.ip().to_canonical().is_ipv6(),
                    bps,
                );
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
        }
        if batch.dropped != packets_dropped {
            current.request_send_loss_recovery();
            current.launch.warnings.event("network_send", "PyroWave video packets were dropped by the host after transient socket send failures. Lower bitrate and check the network adapter; the log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
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
    );
    current.keyframe_loss_bps(paced, safe, wire_frame, true)
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
}
