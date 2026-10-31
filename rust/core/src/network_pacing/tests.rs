use super::*;
#[test]
fn two_frames_is_the_recovery_cap_floor() {
    // 64 FPS, a period f64 holds exactly: 857,421.875 B rounds up.
    let period = Duration::from_micros(15_625);
    assert_eq!(two_frames(219_500, period), 857_422);
    // A pace too slow for two frames still gets them.
    assert_eq!(
        recovery_frame_cap(1, 219_500, period, 20, 1392, false),
        857_422
    );
    assert_eq!(fec_whole_frame_limit(857_421, 219_500, period), None);
    assert_eq!(
        fec_whole_frame_limit(857_422, 219_500, period),
        Some(857_422)
    );
}
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
fn the_fec_whole_cap_keeps_recovery_frames_within_full_parity_above_two_frames() {
    let period = Duration::from_secs_f64(1. / 60.);
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let whole = packetizer.fec_whole_payload_limit();
    // 200 Mbps at 60 fps on an 800 Mbps route: about 2.4 MB, no parity.
    let cap = recovery_frame_cap(800_000_000, 200_000, period, 20, 1392, false);
    assert!(packetizer.fec_limited(cap as usize));
    let held = fec_whole_frame_cap(cap, whole, 200_000, period);
    assert_eq!(held, whole as u64);
    assert!(!packetizer.fec_limited(held as usize));
    // A cap already within full parity is left as it is.
    let slow = recovery_frame_cap(100_000_000, 200_000, period, 20, 1392, false);
    assert_eq!(fec_whole_frame_cap(slow, whole, 200_000, period), slow);
    // Two nominal frames already past full parity (1.67 MB at 400 Mbps): no
    // cap at the floor keeps it, so the cap stays and the frame stays sharp.
    let fast = recovery_frame_cap(800_000_000, 400_000, period, 20, 1392, false);
    assert!((400_000. * 125. * period.as_secs_f64() * 2.).ceil() as u64 > whole as u64);
    assert_eq!(fec_whole_frame_cap(fast, whole, 400_000, period), fast);
    // The re-encode bound for scene cuts is the same: full parity where the
    // cap keeps it, none where a keyframe at the cap would lose it too.
    assert_eq!(
        fec_whole_frame_limit(whole, 200_000, period),
        Some(whole as u64)
    );
    assert_eq!(fec_whole_frame_limit(whole, 400_000, period), None);
}

#[test]
fn without_fec_the_cap_is_the_four_block_packet_limit() {
    let period = Duration::from_secs_f64(1. / 60.);
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 0,
        min_fec: 2,
        key: Some([0; 16]),
    };
    // 255 data shards in each of four blocks: about 1.40 MB.
    let whole = packetizer.fec_whole_payload_limit();
    assert_eq!(whole, 4 * 255 * 1376 - 8);
    // The automatic cap, about 3.1 MB without parity, puts over 2,000
    // packets of a recovery frame at risk; held, it has 1,020.
    let cap = recovery_frame_cap(800_000_000, 200_000, period, 0, 1392, false);
    let held = fec_whole_frame_cap(cap, whole, 200_000, period);
    let packets = |payload: u64| {
        packetizer
            .block_layout(payload as usize)
            .iter()
            .map(|(data, parity)| data + parity)
            .sum::<usize>()
    };
    assert!(packets(cap) > 2000, "{}", packets(cap));
    assert_eq!(packets(held), 4 * 255);
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
                        let wire: usize = packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
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
                let needed = video_frame_wire_rate_bps(&packetizer, payload as usize, period, ipv6);
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
fn the_clients_own_link_caps_conventional_pacing_at_four_fifths() {
    const M: u64 = 1_000_000;
    let rate = |host: u64, client: u64| {
        rate_bps_for_client(0, 50_000, host * M, false, 65 * M, client * M)
    };
    // No hint (stock Moonlight): unchanged.
    assert_eq!(rate(0, 0), 800 * M);
    assert_eq!(rate(1000, 0), 800 * M);
    // A 1 GbE and a 2.5 GbE client stay at 800 Mbps; 100 Mbps gets 80.
    assert_eq!(rate(1000, 1000), 800 * M);
    assert_eq!(rate(1000, 2500), 800 * M);
    assert_eq!(rate(0, 100), 80 * M);
    assert_eq!(rate(1000, 100), 80 * M);
    // The slower of the two NICs decides, whichever side it is.
    assert_eq!(rate(100, 1000), 80 * M);
    assert_eq!(rate(100, 2500), 80 * M);
    assert_eq!(rate(0, 2500), 800 * M);
    // The plain function is the same policy with no hint.
    assert_eq!(rate_bps(0, 50_000, 100 * M, false, 65 * M), rate(100, 0));
    // An explicit cap below the client's link stays; above it, the link wins.
    assert_eq!(
        rate_bps_for_client(30_000, 50_000, 0, false, 0, 100 * M),
        55 * M
    );
    assert_eq!(
        rate_bps_for_client(800_000, 50_000, 0, false, 0, 100 * M),
        80 * M
    );
}

#[test]
fn a_client_link_below_the_bitrate_does_not_pace_the_stream_under_it() {
    const M: u64 = 1_000_000;
    // 100 Mbps client, 150 Mbps stream: 4/5 of the port is 80, the floor
    // is 165.
    let rate = |client: u64| rate_bps_for_client(0, 150_000, 1000 * M, false, 180 * M, client * M);
    assert_eq!(rate(100), 165 * M);
    assert!(client_link_below_stream(150_000, 1000 * M, 100 * M));
    // A hint that fits the stream is capped at 4/5 as before.
    assert_eq!(rate(1000), 800 * M);
    assert_eq!(
        rate_bps_for_client(0, 50_000, 0, false, 65 * M, 100 * M),
        80 * M
    );
    assert!(!client_link_below_stream(50_000, 0, 100 * M));
    // No hint, or the host's own link limiting: no floor.
    assert!(!client_link_below_stream(150_000, 100 * M, 0));
    assert!(!client_link_below_stream(150_000, 100 * M, 1000 * M));
    assert_eq!(
        rate_bps_for_client(0, 150_000, 100 * M, false, 180 * M, 0),
        80 * M
    );
}

#[test]
fn a_client_that_reports_wifi_gets_wifi_pacing_from_the_first_frame() {
    const M: u64 = 1_000_000;
    const GBPS: u64 = 1_000_000_000;
    // Wired 1 GbE host, HEVC 4K60 at 63 Mbps, client link unknown (Wi-Fi).
    let (kbps, needed) = (63_000, 75 * M);
    let start = |host_wireless, client_wireless| {
        rate_bps_for_client(
            0,
            kbps,
            GBPS,
            route_is_wireless(host_wireless, client_wireless),
            needed,
            0,
        )
    };
    assert_eq!(start(false, false), 800 * M);
    assert_eq!(start(false, true), 126 * M);
    assert_eq!(start(false, true), rate_bps(0, kbps, GBPS, true, needed));
    // A wireless host route still decides alone; the hint adds, never removes.
    assert_eq!(start(true, false), 126 * M);
    assert_eq!(start(true, true), 126 * M);
    // The same clamp as the host's own Wi-Fi route: demand above 2x wins.
    assert_eq!(
        rate_bps_for_client(0, 63_000, GBPS, route_is_wireless(false, true), 200 * M, 0),
        250 * M
    );
    // PyroWave without a client pace or wired link hint starts at demand,
    // not at the host link; the wireless flag also drops a wired link hint.
    let wired_hint = ClientPath {
        link_bps: GBPS,
        pace_bps: 0,
    };
    let pyrowave = |wireless| {
        pyrowave_route_rate_bps(
            0,
            300_000,
            2_500 * M,
            route_is_wireless(false, wireless),
            100 * M,
            wired_hint,
        )
    };
    assert_eq!(pyrowave(false), 950 * M);
    assert_eq!(pyrowave(true), 412_500_000);
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
        let wire = packets * (pyrowave::aligned_payload(packet_size) + 32 + 32 + overhead(false));
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
