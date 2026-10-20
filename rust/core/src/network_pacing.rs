//! Bound packet bursts without accumulating a catch-up burst after a late send.
use std::time::{Duration, Instant};

/// The route ceiling, before startup and loss-responsive pacing. Wired/unknown
/// routes allow up to 800 Mbps. Confirmed Wi-Fi retains twice the
/// encoder bitrate, allowing at least 25% headroom above packetized demand.
/// Explicit limits retain the stream-bitrate floor, subject to the physical link.
pub fn rate_bps(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    wireless: bool,
    needed_bps: u64,
) -> u64 {
    let requested = if configured_kbps > 0 {
        (configured_kbps as u64)
            .saturating_mul(1000)
            .max(u64::from(stream_kbps) * 1100)
    } else if wireless {
        (u64::from(stream_kbps) * 2000)
            .max(needed_bps.saturating_add(needed_bps / 4))
            .clamp(1_000_000, 800_000_000)
    } else {
        800_000_000
    };
    if link_bps > 0 {
        requested.min(link_bps.saturating_mul(4) / 5).max(1)
    } else {
        requested
    }
}

/// What the client tells the host about its own path with each PyroWave
/// stream (`x-ss-video[0].pyrowaveLinkMbps` and `pyrowavePaceMbps`, as Nonary's
/// Moonlight and Nightjar send them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientPath {
    /// The client's wired link, which it reports with each PyroWave stream.
    pub link_bps: u64,
    /// The pace the client's calibration found loss-free.
    pub pace_bps: u64,
}

/// A measured client pace, either endpoint's link and an explicit cap are
/// ceilings, even when the requested stream cannot fit. A wired host alone
/// says nothing about the client's path: unmeasured and wireless paths start
/// 25% above wire demand instead of bursting at the host NIC's line rate.
pub fn pyrowave_rate_bps(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    demand_bps: u64,
    client: ClientPath,
) -> u64 {
    pyrowave_route_rate_bps(
        configured_kbps,
        stream_kbps,
        link_bps,
        false,
        demand_bps,
        client,
    )
}

pub fn pyrowave_route_rate_bps(
    configured_kbps: i64,
    stream_kbps: u32,
    link_bps: u64,
    wireless: bool,
    demand_bps: u64,
    client: ClientPath,
) -> u64 {
    let demand = {
        let needed = demand_bps.max(u64::from(stream_kbps) * 1100);
        (needed.saturating_mul(5) / 4).max(1_000_000)
    };
    let requested = if configured_kbps > 0 {
        (configured_kbps as u64).saturating_mul(1000)
    } else if client.pace_bps > 0 {
        client.pace_bps
    } else if client.link_bps > 0 && !wireless {
        client.link_bps.saturating_mul(95) / 100
    } else {
        demand
    };
    let mut rate = requested.max(1);
    for ceiling in [
        link_bps.saturating_mul(95) / 100,
        client.link_bps.saturating_mul(95) / 100,
        client.pace_bps,
    ] {
        if ceiling > 0 {
            rate = rate.min(ceiling);
        }
    }
    rate
}

/// PyroWave groups used by both calibration and streaming: at most one
/// millisecond on the wire, capped at one segmented UDP send.
pub fn pyrowave_batch_limit(bps: u64, packet: usize, ipv6: bool) -> usize {
    batch_payload_limit(bps, packet, ipv6, 65_507.min(packet.saturating_mul(64)))
}

/// Stable wire allowance for adaptation. Records include their headers and
/// critical parity in the encoder budget. Legacy containers can add one
/// four-byte length per eight-byte record, plus transport framing. Charge
/// IPv6/encryption overhead conservatively for both address families.
pub fn pyrowave_wire_rate_bps(
    kbps: u32,
    packet_size: usize,
    records: bool,
    period: Duration,
) -> u64 {
    let packet = packet_size.saturating_add(crate::pyrowave::WIRE_OVERHEAD) as f64;
    let minimum = (packet * 8. / period.as_secs_f64().max(0.000_001)).ceil() as u64;
    if records {
        return (u64::from(kbps) * 1000).max(minimum);
    }
    let payload = packet_size.saturating_sub(16).max(1) as f64;
    let bytes = (f64::from(kbps) * 125. * period.as_secs_f64()).ceil();
    // The legacy container's packet count adds four bytes before the
    // transport's eight-byte frame header. Both can cross a shard boundary.
    let packets = ((bytes * 1.5 + 12.) / payload).ceil();
    ((packets * packet * 8. / period.as_secs_f64().max(0.000_001)).ceil() as u64).max(minimum)
}

/// Whether an average packetized frame occupies at least a quarter-period
/// on the wire. Use wire demand, since FEC/minimum parity can dominate small
/// frames. Such streams need the dedicated sender to keep capture moving.
pub fn paced(bps: u64, needed_bps: u64) -> bool {
    bps <= needed_bps.saturating_mul(4)
}

pub fn report_rate(
    warnings: &crate::session::Warnings,
    bps: u64,
    needed_bps: u64,
    configured_kbps: i64,
) {
    if configured_kbps > 0 && bps > (configured_kbps as u64).saturating_mul(1000) {
        warnings.set("network_pacing_floor", format!("Requested pacing cap {configured_kbps} Kbps was raised to {:.1} Mbps by the encoder-bitrate floor. Bursts can exceed the requested cap; lower the encoder bitrate or choose a pacing cap with room for audio and FEC.", bps as f64 / 1_000_000.));
    } else {
        warnings.clear("network_pacing_floor");
    }
    if bps < needed_bps {
        warnings.set("network_pacing", format!("Network pacing is limited to {:.1} Mbps by the configured cap, measured client path or routed link, below the stream's wire budget. Sending may delay or replace frames; lower client bitrate/FEC or raise the pacing cap only if the network has headroom.", bps as f64 / 1_000_000.));
    } else {
        warnings.clear("network_pacing");
    }
}

/// Ethernet framing, inter-packet gap, IP and UDP, beyond the UDP payload.
pub fn overhead(ipv6: bool) -> usize {
    if ipv6 { 86 } else { 66 }
}

/// At most one millisecond of wire traffic per conventional-video batch,
/// further bounded by the user's payload limit. Always allow one datagram.
/// Small batches protect downstream queues even when the host NIC is fast.
pub fn batch_payload_limit(bps: u64, datagram: usize, ipv6: bool, maximum: usize) -> usize {
    let wire_packet = datagram.saturating_add(overhead(ipv6)) as u64;
    let packets = (bps / 8000 / wire_packet).max(1);
    packets
        .saturating_mul(datagram as u64)
        .min(maximum as u64)
        .max(datagram as u64) as usize
}

pub fn video_wire_rate_bps(
    packetizer: &crate::packet::VideoPacketizer,
    stream_kbps: u32,
    period: Duration,
    ipv6: bool,
) -> u64 {
    let payload = (f64::from(stream_kbps) * 125. * period.as_secs_f64()).ceil() as usize;
    video_frame_wire_rate_bps(packetizer, payload, period, ipv6)
}

/// Wire demand for one encoded frame per period. Sender selection must also
/// account for a large individual frame, even when the average fits inline.
pub fn video_frame_wire_rate_bps(
    packetizer: &crate::packet::VideoPacketizer,
    payload: usize,
    period: Duration,
    ipv6: bool,
) -> u64 {
    if payload == 0 {
        return 0;
    }
    let packets: usize = packetizer
        .block_layout(payload)
        .iter()
        .map(|(data, fec)| data + fec)
        .sum();
    let datagram = packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 };
    (packets as f64 * (datagram + overhead(ipv6)) as f64 * 8. / period.as_secs_f64()).ceil() as u64
}

/// Encoded payload that fits roughly two frame periods at the current pace,
/// bounded to 2–8 nominal frames. The lower bound protects recovery quality
/// when the link cannot carry even that budget; it is not a latency guarantee.
///
/// Include parity and all packet headers, conservatively reserving encryption
/// overhead even for an unencrypted stream. Packet/FEC rounding and minimum
/// parity can add wire time, especially for tiny frames. The sender still
/// accounts for the actual packetized frame when pacing it.
pub fn recovery_frame_cap(
    pace_bps: u64,
    bitrate_kbps: u32,
    period: Duration,
    fec_percent: usize,
    packet_size: usize,
    ipv6: bool,
) -> u64 {
    let average = f64::from(bitrate_kbps) * 125. * period.as_secs_f64();
    let minimum = (average * 2.).ceil() as u64;
    let maximum = ((average * 8.).floor() as u64).max(minimum);
    // Conventional packet_size includes the video headers but excludes the
    // extra 16-byte RTP allowance and optional 32-byte encryption wrapper.
    let payload = packet_size.saturating_sub(16).max(1) as f64;
    let wire = packet_size.saturating_add(48 + overhead(ipv6)) as f64;
    let expansion = wire / payload * (1. + fec_percent.min(100) as f64 / 100.);
    let budget = pace_bps as f64 / 8. * period.as_secs_f64() * 2. / expansion;
    (budget.floor() as u64).clamp(minimum, maximum)
}

/// Time spent in the send call or waking late that still counts toward the
/// batch's interval. It covers a typical 64 KB send (about 0.2 ms) plus timer
/// lateness; a longer stall or idle gap earns no more than this.
pub const SEND_SLACK: Duration = Duration::from_micros(500);

pub struct Pacer {
    due: Instant,
}
impl Pacer {
    pub fn new(now: Instant) -> Self {
        Self { due: now }
    }
    pub fn due(&self) -> Instant {
        self.due
    }
    /// Called immediately after a batch is sent. Up to `SEND_SLACK` of send
    /// time and timer overshoot consumes the interval; beyond that, a stall or
    /// idle gap earns no catch-up credit, so the next batch waits at least the
    /// interval less `SEND_SLACK`.
    /// Account for bytes on the wire, including one header per datagram.
    pub fn sent(
        &mut self,
        completed: Instant,
        payload: usize,
        packets: usize,
        ipv6: bool,
        bps: u64,
    ) {
        let bytes = payload.saturating_add(packets.saturating_mul(overhead(ipv6)));
        let interval = Duration::from_secs_f64(bytes as f64 * 8. / bps.max(1) as f64);
        self.due = (self.due + interval).max(completed + interval.saturating_sub(SEND_SLACK));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unmeasured_and_wireless_pyrowave_routes_do_not_burst_at_the_host_link() {
        let demand = 200_000_000;
        let client = ClientPath {
            link_bps: 10_000_000_000,
            pace_bps: 0,
        };
        assert_eq!(
            pyrowave_route_rate_bps(
                0,
                180_000,
                10_000_000_000,
                false,
                demand,
                ClientPath::default()
            ),
            250_000_000
        );
        assert_eq!(
            pyrowave_route_rate_bps(0, 180_000, 10_000_000_000, true, demand, client),
            250_000_000
        );
        for cap in [0, 50_000, 900_000] {
            for host in [0, 100_000_000, 10_000_000_000] {
                let measured = ClientPath {
                    pace_bps: 80_000_000,
                    ..client
                };
                let rate = pyrowave_route_rate_bps(cap, 180_000, host, false, demand, measured);
                assert!(rate <= measured.pace_bps);
                if cap > 0 {
                    assert!(rate <= cap as u64 * 1000);
                }
                let warnings = crate::session::Warnings::default();
                report_rate(&warnings, rate, demand, cap);
                assert!(
                    warnings
                        .snapshot()
                        .iter()
                        .any(|w| w.code == "network_pacing")
                );
            }
        }
    }

    #[test]
    fn pyrowave_batches_fit_wire_time_and_segment_limits_for_both_ip_families() {
        for ipv6 in [false, true] {
            for rate in [1, 10_000_000, 100_000_000, 1_000_000_000, 10_000_000_000] {
                for packet in [272, 1072, 1440] {
                    let payload = pyrowave_batch_limit(rate, packet, ipv6);
                    let count = payload / packet;
                    assert!(count >= 1);
                    assert!(count <= 64);
                    assert!(payload <= 64 * 1024);
                    if count > 1 {
                        assert!(count as u64 * (packet + overhead(ipv6)) as u64 * 8000 <= rate);
                    }
                }
            }
        }
    }

    #[test]
    fn recovery_cap_matches_slow_route_and_relaxes_on_fast_routes() {
        let period = Duration::from_secs_f64(1. / 119.88);
        let average = 54_000. * 125. * period.as_secs_f64();
        let cap = |pace| recovery_frame_cap(pace, 54_000, period, 10, 1392, false);
        let slow = cap(70_770_000) as f64 / average;
        assert!((2.1..2.3).contains(&slow), "slow route: {slow} frames");
        assert_eq!(cap(800_000_000), (8. * average).floor() as u64);
        let wireless = cap(108_000_000) as f64 / average;
        assert!((3.2..3.4).contains(&wireless));
        assert!(cap(800_000_000) * 8 < 800_000_000 / 100);
    }

    #[test]
    fn recovery_cap_accounts_for_parity_headers_and_two_frame_bounds() {
        for bitrate in [1, 2_000, 54_000, 200_000, u32::MAX] {
            for fps in [24., 59.94, 119.88, 240.] {
                let period = Duration::from_secs_f64(1. / fps);
                let average = f64::from(bitrate) * 125. * period.as_secs_f64();
                let min = (2. * average).ceil() as u64;
                let max = (8. * average).floor() as u64;
                for pace in [0, 70_000_000, 800_000_000, u64::MAX] {
                    let plain = recovery_frame_cap(pace, bitrate, period, 0, 1392, false);
                    let parity = recovery_frame_cap(pace, bitrate, period, 20, 1392, false);
                    let ipv6 = recovery_frame_cap(pace, bitrate, period, 20, 1392, true);
                    let smaller = recovery_frame_cap(pace, bitrate, period, 20, 512, true);
                    assert!((min..=max).contains(&plain));
                    assert!((min..=plain).contains(&parity));
                    assert!((min..=parity).contains(&ipv6));
                    assert!((min..=ipv6).contains(&smaller));
                }
            }
        }
        for packet in [0, 16, usize::MAX] {
            assert_eq!(
                recovery_frame_cap(u64::MAX, 0, Duration::ZERO, 100, packet, true),
                0
            );
        }
    }
    #[test]
    fn conventional_wire_budget_includes_framing_encryption_and_fec_rounding() {
        for ipv6 in [false, true] {
            for key in [None, Some([0; 16])] {
                let mut packetizer = crate::packet::VideoPacketizer {
                    sequence: 0,
                    iv_counter: 0,
                    frame: 1,
                    packet_size: 1392,
                    fec_percent: 20,
                    min_fec: 2,
                    key,
                };
                let period = Duration::from_secs_f64(1. / 60.);
                let payload = (63_000. * 125. * period.as_secs_f64()).ceil() as usize;
                let needed = video_wire_rate_bps(&packetizer, 63_000, period, ipv6);
                let packets = packetizer.encode(&vec![0; payload], false, 0, 0).unwrap();
                let wire: usize = packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
                assert_eq!(
                    needed,
                    (wire as f64 * 8. / period.as_secs_f64()).ceil() as u64
                );
                assert!(needed > 78_000_000);
                let warnings = crate::session::Warnings::default();
                report_rate(&warnings, 78_000_000, needed, 78_000);
                assert_eq!(warnings.snapshot()[0].code, "network_pacing");
            }
        }
    }

    #[test]
    fn legacy_pyrowave_wire_budget_counts_container_headers_at_shard_boundaries() {
        for (fps, boundary_kbps, boundary_packets) in [(125, 912, 2), (50, 4768, 14)] {
            let period = Duration::from_secs_f64(1. / f64::from(fps));
            for kbps in boundary_kbps - 1..=boundary_kbps + 1 {
                for fec in [0, 20] {
                    // Eight-byte codec packets maximize the length container's
                    // overhead. Use the real container and transport encoder.
                    let image_budget = crate::pyrowave::budget(kbps, period, 1392, false, fec);
                    if image_budget < 16 {
                        // Only the low-rate parity case emits no frame;
                        // neither unprotected shard-boundary case is skipped.
                        assert_eq!((fps, fec, image_budget), (125, 20, 0));
                        continue;
                    }
                    let raw = vec![0; image_budget & !7];
                    let records: Vec<_> = (0..raw.len()).step_by(8).map(|at| (at, 8)).collect();
                    let frame = crate::pyrowave::container(&raw, &records).unwrap();
                    let needed = pyrowave_wire_rate_bps(kbps, 1392, false, period);
                    for ipv6 in [false, true] {
                        for key in [None, Some([0; 16])] {
                            let mut packetizer = crate::packet::VideoPacketizer {
                                sequence: 0,
                                iv_counter: 0,
                                frame: 1,
                                packet_size: 1392,
                                fec_percent: 0,
                                min_fec: 2,
                                key,
                            };
                            let packets = packetizer
                                .encode_pyrowave(
                                    &frame,
                                    0,
                                    0,
                                    crate::packet::PyrowaveFec {
                                        records: false,
                                        critical_percentage: fec as usize,
                                        detail_percentage: 0,
                                        wire_budget: 0,
                                        ipv6,
                                    },
                                )
                                .unwrap();
                            let wire: usize =
                                packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
                            let actual = (wire as f64 * 8. / period.as_secs_f64()).ceil() as u64;
                            assert!(needed >= actual, "{fps}/{kbps}/{fec}: {needed} < {actual}");
                            if kbps == boundary_kbps && fec == 0 {
                                assert_eq!(packets.len(), boundary_packets);
                                if ipv6 && key.is_some() {
                                    assert_eq!(needed, actual);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn only_a_pacing_shortfall_warns_and_a_faster_link_clears_it() {
        let warnings = crate::session::Warnings::default();
        report_rate(
            &warnings,
            rate_bps(0, 100_000, 100_000_000, false, 120_000_000),
            120_000_000,
            0,
        );
        assert!(warnings.snapshot()[0].message.contains("80.0 Mbps"));
        report_rate(
            &warnings,
            rate_bps(0, 100_000, 1_000_000_000, false, 120_000_000),
            120_000_000,
            0,
        );
        assert!(warnings.snapshot().is_empty());
        report_rate(
            &warnings,
            rate_bps(50_000, 100_000, 0, false, 120_000_000),
            100_000_000,
            50_000,
        );
        assert_eq!(warnings.snapshot()[0].code, "network_pacing_floor");
        report_rate(
            &warnings,
            rate_bps(200_000, 100_000, 0, false, 120_000_000),
            100_000_000,
            200_000,
        );
        assert!(warnings.snapshot().is_empty());
    }

    #[test]
    fn sender_selection_includes_fec_and_minimum_parity_in_frame_send_time() {
        // Dynamic automatic pacing reserves the sender independently of this
        // near-budget check; a healthy wired stream still starts fast.
        assert!(!paced(
            rate_bps(0, 80_000, 0, false, 100_000_000),
            100_000_000
        ));
        assert!(!paced(
            rate_bps(0, 80_000, 2_500_000_000, false, 100_000_000),
            100_000_000
        ));
        assert!(paced(
            rate_bps(160_000, 80_000, 0, false, 100_000_000),
            100_000_000
        ));
        assert!(paced(
            rate_bps(0, 50_000, 100_000_000, false, 65_000_000),
            65_000_000
        ));
        assert!(!paced(
            rate_bps(800_000, 80_000, 0, false, 100_000_000),
            100_000_000
        ));
        // Parity dominates a low-bitrate, high-refresh stream's wire demand.
        assert!(paced(rate_bps(0, 2_000, 0, true, 15_000_000), 15_000_000));
        assert!(paced(400_000_000, 100_000_000));
        assert!(!paced(400_000_001, 100_000_000));
        assert!(!paced(1, 0));
    }

    #[test]
    fn large_frames_need_the_sender_even_when_average_traffic_fits_inline() {
        let period = Duration::from_secs_f64(1. / 90.);
        for bitrate in [80_000, 100_000] {
            for ipv6 in [false, true] {
                for key in [None, Some([7; 16])] {
                    let mut packetizer = crate::packet::VideoPacketizer {
                        sequence: 0,
                        iv_counter: 0,
                        frame: 1,
                        packet_size: 1392,
                        fec_percent: 20,
                        min_fec: 2,
                        key,
                    };
                    let average = video_wire_rate_bps(&packetizer, bitrate, period, ipv6);
                    let bps = rate_bps(0, bitrate, 1_000_000_000, false, average);
                    assert!(!paced(bps, average));
                    let payload = recovery_frame_cap(bps, bitrate, period, 20, 1392, ipv6);
                    let needed =
                        video_frame_wire_rate_bps(&packetizer, payload as usize, period, ipv6);
                    assert!(paced(bps, needed));
                    let packets = packetizer
                        .encode(&vec![0; payload as usize], true, 0, 0)
                        .unwrap();
                    let bytes: usize = packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
                    assert_eq!(
                        needed,
                        (bytes as f64 * 8. / period.as_secs_f64()).ceil() as u64
                    );
                    let wire_time = Duration::from_secs_f64(bytes as f64 * 8. / bps as f64);
                    assert!(wire_time > period);
                    assert!(!paced(
                        bps,
                        video_frame_wire_rate_bps(&packetizer, 0, period, ipv6)
                    ));
                }
            }
        }
    }

    #[test]
    fn pyrowave_demand_cannot_override_an_explicit_cap_or_link_limit() {
        let demand = 512_000 * 60 * 8;
        assert_eq!(
            pyrowave_rate_bps(30_000, 30_000, 100_000_000, demand, ClientPath::default()),
            30_000_000
        );
        assert_eq!(
            pyrowave_rate_bps(100_000, 100_000, 100_000_000, demand, ClientPath::default()),
            95_000_000
        );
        assert_eq!(
            pyrowave_rate_bps(0, 30_000, 1_000_000_000, demand, ClientPath::default()),
            demand * 5 / 4
        );
        assert_eq!(
            pyrowave_rate_bps(0, 30_000, 0, demand, ClientPath::default()),
            demand * 5 / 4
        );
    }

    #[test]
    fn pyrowave_paces_a_slower_client_port_at_its_own_rate() {
        const G: u64 = 1_000_000_000;
        let demand = 300_000_000;
        let link = |bps| ClientPath {
            link_bps: bps,
            ..Default::default()
        };
        // Issue #31: a 2.5 GbE client on a 10 GbE host, and a 1 GbE client on
        // a 2.5 GbE host, as the client reports its link.
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 10 * G, demand, link(5 * G / 2)),
            2_375_000_000
        );
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 5 * G / 2, demand, link(G)),
            950_000_000
        );
        // A client as fast as the host keeps the link rate: no added send time.
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 10 * G, demand, link(10 * G)),
            9_500_000_000
        );
        // Calibration remains a hard ceiling, even below the stream's demand.
        let pace = |bps| ClientPath {
            pace_bps: bps,
            link_bps: 10 * G,
        };
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 10 * G, demand, pace(2 * G)),
            2 * G
        );
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 10 * G, demand, pace(100_000_000)),
            100_000_000
        );
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, G, demand, pace(2 * G)),
            950_000_000
        );
        // An overloaded stream must not overrun a slower client link.
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 10 * G, demand, link(100_000_000)),
            95_000_000
        );
        // Explicit caps and client evidence also apply without a host speed.
        assert_eq!(
            pyrowave_rate_bps(100_000, 100_000, 10 * G, demand, link(G)),
            100_000_000
        );
        assert_eq!(
            pyrowave_rate_bps(0, 270_000, 0, demand, link(G)),
            950_000_000
        );
    }

    #[test]
    fn host_link_speed_is_a_ceiling_not_a_measurement_of_the_client_path() {
        assert_eq!(rate_bps(0, 50_000, 0, false, 65_000_000), 800_000_000);
        assert_eq!(
            rate_bps(0, 50_000, 2_500_000_000, false, 65_000_000),
            800_000_000
        );
        assert_eq!(
            rate_bps(0, 50_000, 100_000_000, false, 65_000_000),
            80_000_000
        );
        for needed in [0, 65_000_000, 150_000_000] {
            assert_eq!(rate_bps(30_000, 50_000, 0, false, needed), 55_000_000);
            assert_eq!(rate_bps(800_000, 50_000, 0, false, needed), 800_000_000);
            assert_eq!(
                rate_bps(100_000, 100_000, 100_000_000, false, needed),
                80_000_000
            );
        }
        assert_eq!(rate_bps(0, 1, 0, true, 0), 1_000_000);
        assert_eq!(rate_bps(0, 1, 0, false, 0), 800_000_000);
        assert_eq!(rate_bps(0, u32::MAX, 0, true, u64::MAX), 800_000_000);
        assert!(rate_bps(i64::MAX, u32::MAX, u64::MAX, false, u64::MAX) > 0);
    }

    #[test]
    fn only_wireless_starting_rates_follow_bitrate_without_capping_pyrowave_demand() {
        for kbps in [9_300, 19_000, 60_000, 149_000] {
            for link in [0, 1_000_000_000, 2_500_000_000] {
                assert_eq!(
                    rate_bps(0, kbps, link, true, u64::from(kbps) * 1300),
                    u64::from(kbps) * 2000
                );
                assert_eq!(
                    rate_bps(0, kbps, link, false, u64::from(kbps) * 1300),
                    800_000_000
                );
            }
        }
        for kbps in [200_000, 800_000, 2_000_000] {
            let demand = u64::from(kbps) * 1400;
            assert_eq!(
                pyrowave_rate_bps(0, kbps, 0, demand, ClientPath::default()),
                demand * 5 / 4
            );
            assert_eq!(
                pyrowave_rate_bps(0, kbps, 1_000_000_000, demand, ClientPath::default()),
                (demand * 5 / 4).min(950_000_000)
            );
        }
    }

    #[test]
    fn automatic_pacing_leaves_room_for_parity_small_packets_and_high_refresh() {
        for kbps in [1, 2_000, 40_000, 200_000] {
            for fps in [30, 60, 120, 240] {
                for fec in [0, 20, 100] {
                    for packet_size in [512, 1392] {
                        for ipv6 in [false, true] {
                            let packetizer = crate::packet::VideoPacketizer {
                                sequence: 0,
                                iv_counter: 0,
                                frame: 1,
                                packet_size,
                                fec_percent: fec,
                                min_fec: 4,
                                key: Some([0; 16]),
                            };
                            let period = Duration::from_secs_f64(1. / f64::from(fps));
                            let needed = video_wire_rate_bps(&packetizer, kbps, period, ipv6);
                            let bps = rate_bps(0, kbps, 1_000_000_000, true, needed);
                            assert!(
                                bps >= needed + needed / 4,
                                "{kbps} Kbps, {fps} fps, {fec}% FEC, {packet_size} bytes: {needed} > {bps}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn batches_fit_one_millisecond_on_the_wire_or_one_datagram() {
        for ipv6 in [false, true] {
            for datagram in [528, 1408, 1440] {
                for bps in [1, 1_000_000, 80_000_000, 800_000_000, u64::MAX] {
                    for maximum in [16 * 1024, 32 * 1024, 64 * 1024] {
                        let limit = batch_payload_limit(bps, datagram, ipv6, maximum);
                        let count = limit / datagram;
                        assert!(count >= 1 && limit <= maximum);
                        if count > 1 {
                            assert!((count * (datagram + overhead(ipv6))) as u64 * 8000 <= bps);
                        }
                    }
                }
            }
        }
        assert_eq!(batch_payload_limit(1, 1408, false, 1024), 1408);
    }

    /// Model a 1 Gbps host feeding a 100 Mbps egress queue of just 16 KiB.
    /// It is an explicit test topology, not a claim about any client's NIC.
    fn downstream_losses(previous_policy: bool, ipv6: bool, keyframes: bool) -> usize {
        let packetizer = crate::packet::VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: Some([0; 16]),
        };
        let period = Duration::from_secs_f64(1. / 60.);
        let needed = video_wire_rate_bps(&packetizer, 40_000, period, ipv6);
        let bps = if previous_policy {
            800_000_000
        } else {
            rate_bps(80_000, 40_000, 1_000_000_000, false, needed)
        };
        let datagram = packetizer.packet_size + 16 + 32;
        let wire_packet = datagram + overhead(ipv6);
        let limit = if previous_policy {
            64 * 1024
        } else {
            batch_payload_limit(bps, datagram, ipv6, 64 * 1024)
        };
        let per_batch = (limit.min(65507) / datagram).clamp(1, 64);
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let mut now = start;
        let mut drained_at = start;
        let mut lost = 0;
        let wire_time = Duration::from_secs_f64(wire_packet as f64 * 8. / 100_000_000.);
        let host_time = Duration::from_secs_f64(wire_packet as f64 * 8. / 1_000_000_000.);
        let capacity = Duration::from_secs_f64(16. * 1024. * 8. / 100_000_000.);
        for frame in 0..600 {
            now = now.max(start + period * frame);
            let payload = (40_000. * 125. * period.as_secs_f64()).ceil() as usize
                * if keyframes && frame % 300 == 0 { 8 } else { 1 };
            for (data, fec) in packetizer.block_layout(payload) {
                let mut remaining = data + fec;
                while remaining > 0 {
                    now = now.max(pacer.due());
                    let count = remaining.min(per_batch);
                    for _ in 0..count {
                        now += host_time;
                        let done = drained_at.max(now) + wire_time;
                        if done.saturating_duration_since(now) <= capacity {
                            drained_at = done;
                        } else {
                            lost += 1;
                        }
                    }
                    pacer.sent(now, count * datagram, count, ipv6, bps);
                    remaining -= count;
                }
            }
        }
        lost
    }

    #[test]
    fn an_explicit_eighty_mbps_cap_can_protect_a_slower_downstream_queue() {
        for ipv6 in [false, true] {
            for keyframes in [false, true] {
                assert!(downstream_losses(true, ipv6, keyframes) > 10_000);
                assert_eq!(downstream_losses(false, ipv6, keyframes), 0);
            }
        }
    }

    /// A record frame of the encoder's full budget, as the host frames it.
    fn budget_frame(width: u32, height: u32, budget: usize) -> Vec<u8> {
        let size = |i: usize| 8 + (i * 73 % 504) * 4;
        let (mut count, mut total) = (0, 8);
        while total + size(count) <= budget {
            total += size(count);
            count += 1;
        }
        let mut raw = Vec::with_capacity(total);
        raw.extend_from_slice(&(0x80000000u32 | (width - 1) | ((height - 1) << 14)).to_le_bytes());
        raw.extend_from_slice(&(count as u32).to_le_bytes());
        for i in 0..count {
            raw.extend_from_slice(&((size(i) as u32 / 4) << 16).to_le_bytes());
            raw.extend_from_slice(&((((i * 37) % count) as u32) << 8).to_le_bytes());
            raw.extend((0..size(i) - 8).map(|j| (i * 31 + j * 17) as u8));
        }
        raw
    }

    #[test]
    fn pyrowave_pacing_leaves_headroom_for_a_budget_sized_frame() {
        use crate::{
            packet::{PyrowaveFec, VideoPacketizer},
            pyrowave,
        };
        // Moonlight's 1392-byte packets less the encryption header.
        let packet_size = 1360;
        let period = Duration::from_secs_f64(1. / 60.);
        for (width, height, kbps) in [
            (1920, 1080, 400_000),
            (2560, 1440, 708_000),
            (3840, 2160, 800_000),
        ] {
            let budget = pyrowave::budget(kbps, period, packet_size, true, 20);
            let framed = pyrowave::record_frame(
                &budget_frame(width, height, budget),
                pyrowave::aligned_payload(packet_size),
                pyrowave::max_frame_bytes(packet_size, true),
            )
            .unwrap();
            let mut packetizer = VideoPacketizer {
                sequence: 0,
                iv_counter: 0,
                frame: 1,
                packet_size,
                fec_percent: 0,
                min_fec: 2,
                key: None,
            };
            let fec = PyrowaveFec {
                records: true,
                critical_percentage: 20,
                detail_percentage: 0,
                wire_budget: 0,
                ipv6: false,
            };
            let (packets, fec_limited, _) = packetizer.pyrowave_blocks(&framed, 0, 0, fec).unwrap();
            assert!(!fec_limited);
            // Payload, RTP and NV headers, encryption, and Ethernet/IP/UDP.
            let wire =
                packets * (pyrowave::aligned_payload(packet_size) + 32 + 32 + overhead(false));
            // As the PyroWave sender computes its demand.
            let demand = (packets * (packet_size + 16 + 32 + overhead(false))) as u64 * 60 * 8;
            let bps = pyrowave_rate_bps(0, kbps, 0, demand, ClientPath::default());
            let paced = bps as f64 * period.as_secs_f64() / 8.;
            assert!(
                paced >= 1.2 * wire as f64,
                "{width}x{height} at {kbps} Kbps: {packets} packets, {wire} wire bytes, {paced:.0} paced bytes per frame"
            );
        }
    }

    #[test]
    fn socket_time_and_short_wake_delays_do_not_accumulate() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        for batch in 1..=20 {
            let woke = pacer.due() + Duration::from_micros(50);
            let completed = woke + Duration::from_micros(200);
            pacer.sent(completed, 934, 1, false, 8_000_000);
            assert_eq!(pacer.due(), start + Duration::from_millis(batch));
        }
        // Sends slower than the slack fall behind without building debt.
        for _ in 0..5 {
            let completed = pacer.due() + Duration::from_micros(700);
            pacer.sent(completed, 934, 1, false, 8_000_000);
            assert_eq!(
                pacer.due(),
                completed + Duration::from_millis(1) - SEND_SLACK
            );
        }
    }

    #[test]
    fn socket_stalls_and_late_wakeups_cannot_create_catch_up_credit() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        // One millisecond of IPv4 traffic at 8 Mbps.
        let interval = Duration::from_millis(1);
        pacer.sent(start, 934, 1, false, 8_000_000);
        assert_eq!(pacer.due(), start + interval);
        // A 5 ms stall earns only the bounded slack, not a catch-up burst.
        let after_stall = start + Duration::from_millis(6);
        pacer.sent(after_stall, 934, 1, false, 8_000_000);
        assert_eq!(pacer.due(), after_stall + interval - SEND_SLACK);
        let on_time = pacer.due();
        pacer.sent(on_time, 934, 1, false, 8_000_000);
        assert_eq!(pacer.due(), on_time + interval);
        // Starting the next frame after a long idle period creates no debt.
        let next_frame = start + Duration::from_secs(1);
        pacer.sent(next_frame, 934, 1, false, 8_000_000);
        assert_eq!(pacer.due(), next_frame + interval - SEND_SLACK);
    }

    #[test]
    fn each_ipv6_datagram_has_wire_overhead_and_empty_batches_have_no_debt() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        pacer.sent(start, 2 * 914, 2, true, 8_000_000);
        assert_eq!(pacer.due(), start + Duration::from_millis(2));
        let completed = start + Duration::from_millis(3);
        pacer.sent(completed, 0, 0, false, 8_000_000);
        assert_eq!(pacer.due(), completed);
    }
}
