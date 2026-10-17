use super::*;
use butterpollo_core::network_pacing::{self, ClientPath, Pacer};
use butterpollo_windows::{net::Batch, timing::Timer};

pub(super) fn run(
    shared: &Slot,
    socket: Arc<UdpSocket>,
    current: Arc<Session>,
    config: Config,
    host: Shared,
) -> Result<()> {
    let _priority = butterpollo_windows::capture::Priority::new();
    let timer = Timer::new()?;
    let mut batch = Batch::default();
    let mut pacer = Pacer::new(Instant::now());
    let mut link = 0;
    let mut link_due = Instant::now();
    let client = ClientPath {
        link_bps: u64::from(current.config.pyrowave_client_link_mbps) * 1_000_000,
        pace_bps: u64::from(current.config.pyrowave_client_pace_mbps) * 1_000_000,
    };
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
        let bps = network_pacing::pyrowave_rate_bps(
            config.integer("pacing_max_bitrate_kbps", 0),
            frame.kbps,
            link,
            frame.demand,
            client,
        );
        if Instant::now() >= link_due {
            network_pacing::report_rate(
                &current.launch.warnings,
                bps,
                frame.demand,
                config.integer("pacing_max_bitrate_kbps", 0),
            );
        }
        let mut sent = 0;
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
                    timer.until_precise(pacer.due());
                }
                let count = Batch::count(remaining, batch_limit(bps, remaining[0].len()));
                let bytes = batch.send(&socket, &remaining[..count], frame.peer)?;
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
        if Instant::now() >= link_due {
            link = butterpollo_windows::net::routed_link_bps(frame.peer);
            link_due = Instant::now() + Duration::from_secs(2);
        }
    }
    Ok(())
}
