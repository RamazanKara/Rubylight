use super::*;
fn frame(records: &[(u32, usize)]) -> Vec<u8> {
    let mut out = vec![];
    out.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for &(index, size) in records {
        out.extend_from_slice(&((size as u32 / 4) << 16).to_le_bytes());
        out.extend_from_slice(&(index << 8).to_le_bytes());
        out.resize(out.len() + size - 8, 42);
    }
    out
}
#[test]
fn minimum_parity_rounding_keeps_every_planned_block_representable() {
    for total in [230, 231, 460, 462, 600, 800] {
        for critical in [1, 2, 201] {
            for minimum in [24, 42] {
                for extra in [0, 50, 200] {
                    let blocks = plan(total, critical, 20, minimum, 50, extra).unwrap();
                    assert_eq!(blocks.iter().map(|b| b.data).sum::<usize>(), total);
                    assert!(blocks.len() <= 4);
                    for block in blocks {
                        let (parity, percentage) =
                            fec_parameters(block.data, block.percentage, minimum);
                        assert_eq!(parity, (block.data * percentage).div_ceil(100));
                        assert!(block.data <= 1023);
                        assert!(percentage == 0 || block.data + parity <= 255);
                    }
                }
            }
        }
    }
}
#[test]
fn quality_bitrates_scale_with_pixels_and_frame_rate() {
    assert_eq!(minimum_kbps(1280, 720, 60_000), 138_240);
    assert_eq!(minimum_kbps(1920, 1080, 60_000), 186_624);
    assert_eq!(minimum_kbps(3840, 2160, 60_000), 746_496);
    assert_eq!(recommended_kbps(1280, 720, 60_000), 276_480);
    assert_eq!(recommended_kbps(1920, 1080, 60_000), 398_132);
    assert_eq!(recommended_kbps(3840, 2160, 60_000), 1_592_525);
    assert_eq!(minimum_kbps(1920, 1080, 30_000), 93_312);
    assert_eq!(minimum_kbps(1280, 720, 120_000), 276_480);
    assert_eq!(recommended_kbps(1280, 720, 30_000), 138_240);
    assert_eq!(recommended_kbps(1920, 1080, 120_000), 796_263);
    assert_eq!(minimum_kbps(3840, 2160, 59_940), 745_750);
    assert_eq!(recommended_kbps(3840, 2160, 59_940), 1_590_933);
}
#[test]
fn quality_bitrates_round_up_and_saturate() {
    for calculate in [minimum_kbps, recommended_kbps] {
        assert_eq!(calculate(1, 1, 1), 1);
        assert_eq!(calculate(1920, 1080, 0), 0);
        assert_eq!(calculate(0, 720, 60_000), 0);
        assert_eq!(calculate(1920, 0, 60_000), 0);
        assert_eq!(calculate(u32::MAX, u32::MAX, u32::MAX), u32::MAX);
    }
}
#[test]
fn record_hash_ignores_only_the_sequence_bits() {
    let record: Vec<u8> = (0..37u8).collect();
    let base = record_hash(&record);
    let mut sequence = record.clone();
    sequence[3] ^= 0x70;
    assert_eq!(record_hash(&sequence), base);
    for at in [0, 3, 7, 8, 20, 36] {
        let mut changed = record.clone();
        changed[at] ^= if at == 3 { 0x01 } else { 0x80 };
        assert_ne!(record_hash(&changed), base, "byte {at}");
    }
    assert_ne!(record_hash(&record[..36]), base);
    let short = [1u8, 2, 3, 4, 5];
    let mut short_sequence = short;
    short_sequence[3] ^= 0x70;
    assert_eq!(record_hash(&short_sequence), record_hash(&short));
}
#[test]
fn compatibility_matches_clients_and_checks_slices() {
    assert_eq!(
        container(&[10, 11, 12, 13], &[(1, 2), (0, 1)]).unwrap(),
        [2, 0, 0, 0, 2, 0, 0, 0, 11, 12, 1, 0, 0, 0, 10]
    );
    assert!(container(&[1], &[(usize::MAX, 1)]).is_err());
    let cap = max_frame_bytes(1392, false);
    let raw = budget(2_000_000, Duration::from_secs(1), 1392, false, 0);
    assert!(4 + raw + (raw / 8) * 4 <= cap);
    // 833333 wire bytes at 800 Mbps and 120 fps hold 546 packets; one
    // critical block of 212 takes 43 parity.
    assert_eq!(
        budget(800000, Duration::from_secs_f64(1. / 120.), 1392, true, 20),
        692120
    );
}
#[test]
fn legacy_parity_fits_the_existing_length_framing_wire_allowance() {
    let period = Duration::from_secs_f64(1. / 60.);
    for size in [256, 1392] {
        let slice = size - 16;
        for kbps in [2000, 10_000, 80_000, 300_000, 2_000_000] {
            let original = budget(kbps, period, size, false, 0);
            let packets = (12 + original + original / 8 * 4).div_ceil(slice);
            for rate in [20, 100, 255] {
                for minimum in [2, 24, 255] {
                    let image = budget_with_minimum(kbps, period, size, false, rate, minimum);
                    assert!(image <= original);
                    if image == 0 {
                        continue;
                    }
                    let data = (12 + image + image / 8 * 4).div_ceil(slice);
                    let (blocks, fitted) =
                        crate::packet::conventional_fec(data, rate as usize, minimum);
                    let aligned = data.div_ceil(blocks);
                    let actual = data
                        + (0..blocks)
                            .map(|block| {
                                parity(
                                    data.saturating_sub(block * aligned).min(aligned),
                                    fitted,
                                    minimum,
                                )
                            })
                            .sum::<usize>();
                    assert!(
                        actual <= packets,
                        "{size}/{kbps}/{rate}/{minimum}: {actual}>{packets}"
                    );
                }
            }
        }
    }
}
#[test]
fn record_budgets_reserve_the_negotiated_representable_minimum() {
    let period = Duration::from_secs_f64(1. / 60.);
    for minimum in [2, 24, 44, 255] {
        for rate in [1, 20, 100, 255] {
            let kbps = 80_000;
            let allowance = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize;
            let packets = allowance / (1392 + WIRE_OVERHEAD);
            let image = budget_with_minimum(kbps, period, 1392, true, rate, minimum);
            let data = (image + 8).div_ceil(1376);
            for critical in 1..=data.min(255) {
                let fec = parity(critical, rate as usize, minimum);
                if critical + fec <= 255 {
                    assert!(data + fec <= packets);
                }
            }
        }
    }
}
#[test]
fn record_budget_is_the_wire_rate_less_overhead_and_parity() {
    // The GPU end-to-end test's 40 dB control for 1080p 4:4:4 needs about
    // 3.3 MB of picture. A record budget is a wire budget, so 1 Gbps
    // allows 2.08 MB per frame and leaves 4:4:4 at 42.8/37.3/36.6 dB:
    // that is the rate, not a codec fault. The control runs at 2 Gbps.
    let period = Duration::from_secs_f64(1. / 60.);
    for (kbps, least, most) in [
        (1_000_000, 1_700_000, 1_900_000),
        (2_000_000, 3_500_000, 3_900_000),
    ] {
        let image = budget(kbps, period, 1392, true, 20);
        let wire = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize;
        assert!((least..=most).contains(&image), "{kbps}: {image}");
        assert!(image < wire, "{kbps}: {image} of {wire}");
    }
}
#[test]
fn critical_parity_bound_covers_every_adaptive_percentage() {
    for minimum in [2, 24, 42, 44, 255] {
        for critical in 0..=256 {
            let mut maximum = 0;
            for percentage in 0..=255 {
                let parity = parity(critical, percentage, minimum);
                if critical + parity <= 255 {
                    maximum = maximum.max(parity);
                }
                assert_eq!(
                    critical_parity_bound(critical, percentage, minimum),
                    maximum,
                    "{critical}/{percentage}/{minimum}"
                );
            }
        }
    }
}
#[test]
fn a_lower_adaptive_percentage_cannot_exceed_the_reserved_wire_budget() {
    let packet_size = 1392;
    let allowance = 254 * (packet_size + WIRE_OVERHEAD);
    let image = image_bytes_with_minimum(allowance, packet_size, 120, 2);
    let data = (image + 8).div_ceil(packet_size - 16);
    for critical in 1..=data {
        for percentage in 0..=120 {
            let blocks = plan(data, critical, percentage, 2, 0, 0).unwrap();
            let packets: usize = blocks
                .iter()
                .map(|block| block.data + parity(block.data, block.percentage, 2))
                .sum();
            assert!(
                packets * (packet_size + WIRE_OVERHEAD) <= allowance,
                "{critical}/{percentage}: {packets} packets"
            );
        }
    }
}
#[test]
fn aligned_frame_cost_reserves_parity_restored_by_adaptation() {
    let original = frame(
        &(0..48)
            .map(|index| (index, 6100))
            .chain((48..98).map(|index| (index, 700)))
            .collect::<Vec<_>>(),
    );
    let packet_size = 1392;
    let aligned = record_frame(
        &original,
        aligned_payload(packet_size),
        max_frame_bytes(packet_size, true),
    )
    .unwrap();
    let critical = layout(&aligned, aligned_payload(packet_size))
        .unwrap()
        .critical_shards;
    assert_eq!(critical, 213);
    let data = (aligned.len() + 8).div_ceil(packet_size - 16);
    let maximum = data + critical_parity_bound(critical, 20, 2);
    let allowance = 437_500;
    assert!(data * (packet_size + WIRE_OVERHEAD) <= allowance);
    assert!(maximum * (packet_size + WIRE_OVERHEAD) > allowance);
    for percentage in 0..=20 {
        let mut packetizer = crate::packet::VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size,
            fec_percent: 0,
            min_fec: 2,
            key: Some([42; 16]),
        };
        let packets = packetizer
            .encode_pyrowave(
                &aligned,
                0,
                0,
                crate::packet::PyrowaveFec {
                    records: true,
                    critical_percentage: percentage,
                    detail_percentage: 0,
                    wire_budget: 0,
                    ipv6: true,
                },
            )
            .unwrap();
        let wire: usize = packets
            .iter()
            .map(|packet| packet.len() + crate::network_pacing::overhead(true))
            .sum();
        assert!(wire <= maximum * (packet_size + WIRE_OVERHEAD));
        if percentage == 19 {
            assert!(wire > allowance);
        }
    }
}
#[test]
fn a_viable_frame_budget_stays_viable_as_bitrate_increases() {
    for records in [false, true] {
        for rate in [1, 20, 100, 255] {
            for minimum in [2, 24, 255] {
                let mut viable = false;
                for kbps in (0..10_000).step_by(13) {
                    let image = budget_with_minimum(
                        kbps,
                        Duration::from_secs_f64(1. / 60.),
                        1392,
                        records,
                        rate,
                        minimum,
                    );
                    assert!(!viable || image >= 16, "{records}/{rate}/{minimum}/{kbps}");
                    viable |= image >= 16;
                }
            }
        }
    }
}
/// Nonary's Moonlight calibrates a total wire rate and converts it to a
/// picture allowance with Vibeshine's `pyrowave_bandwidth.h`; the host
/// must make the same conversion. Values from that header, compiled.
#[test]
fn a_wire_allowance_holds_the_picture_vibeshine_computes() {
    for (wire, percentage, image) in [
        (833_333, 20, 692_120),
        (5_000, 20, 1_368),
        (1526 * 3, 20, 1_368),
        (1526 * 300, 20, 353_624),
        (3_333_333, 20, 2_946_008),
        (100, 20, 0),
        (833_333, 0, 751_288),
        (833_333, 50, 634_328),
    ] {
        assert_eq!(
            image_bytes(wire, 1392, percentage),
            image,
            "{wire} at {percentage}%"
        );
    }
    // 1080p60 and 4K120 at the same total: each frame gets its share.
    assert_eq!(
        budget(500_000, Duration::from_secs_f64(1. / 60.), 1392, true, 20),
        image_bytes(1_041_666, 1392, 20)
    );
    assert_eq!(
        budget(
            2_000_000,
            Duration::from_secs_f64(1. / 120.),
            1392,
            true,
            20
        ),
        image_bytes(2_083_333, 1392, 20)
    );
}
#[test]
fn a_scene_cut_gets_no_more_than_one_frame_budget_so_it_needs_no_frame_cap() {
    // The AMF cap holds a conventional recovery frame to full four-block
    // FEC but never below two nominal frames. PyroWave's rate control
    // already gives every picture, a cut included, at most one frame's
    // share of the bitrate, with its parity inside the plan the
    // packetizer builds, so that cap could never bind here.
    for records in [false, true] {
        for fec in [0, 20, 50] {
            for kbps in [20_000, 250_000, 1_000_000, 4_000_000] {
                for fps in [60., 120., 240.] {
                    let period = Duration::from_secs_f64(1. / fps);
                    let nominal = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize;
                    let picture = budget_with_minimum(kbps, period, 1392, records, fec, 2);
                    assert!(picture <= nominal, "{kbps} kbps {fps} fps {fec} %");
                    assert!(picture <= max_frame_bytes(1392, records && fec > 0));
                }
            }
        }
    }
}
#[test]
fn a_new_picture_after_a_repeat_keeps_a_full_frame_budget() {
    let ms = Duration::from_millis;
    let start = Instant::now();
    let mut interval = Interval::new(ms(10), false);
    assert_eq!(interval.next(start, false), ms(10));
    // A game at 50 fps: each picture pays for its whole gap, as before.
    assert_eq!(interval.next(start + ms(20), false), ms(20));
    // A still picture repeated at 20 fps pays for the time since the last encode.
    assert_eq!(interval.next(start + ms(70), true), ms(50));
    // The game's next picture 2 ms after the repeat gets a whole period, not 2 ms.
    assert_eq!(interval.next(start + ms(72), false), ms(10));
    // One period after the repeat it gets that period.
    assert_eq!(interval.next(start + ms(122), true), ms(50));
    assert_eq!(interval.next(start + ms(134), false), ms(12));
    // Repeats close together stay small: they show nothing new.
    assert_eq!(interval.next(start + ms(136), true), ms(2));

    let mut stable = Interval::new(ms(10), true);
    stable.next(start, false);
    assert_eq!(stable.next(start + ms(50), true), ms(10));
    assert_eq!(stable.next(start + ms(53), false), ms(10));
    assert_eq!(stable.next(start + ms(58), false), ms(5));
}
#[test]
fn aligned_records_keep_coarse_first_and_restart_after_loss() {
    let original = frame(&[(24, 40), (0, 16), (1, 60), (27, 100), (26, 12)]);
    let packed = record_frame(&original, 64, 10000).unwrap();
    assert_eq!(records(&packed).unwrap().len(), 5);
    let l = layout(&packed, 64).unwrap();
    assert!(l.critical_shards >= 2);
    assert!(l.starts[0]);
    assert_eq!(aligned_payload(1390), 0);
    assert!(record_frame(&[0; 8], 64, 10000).is_err());
    let mut bad = original;
    bad[8..12].copy_from_slice(&0x10000u32.to_le_bytes());
    assert!(record_frame(&bad, 64, 10000).is_err());
}
#[test]
fn critical_and_adaptive_parity_obey_wire_and_block_limits() {
    let maximum = plan(4000, 0, 20, 2, 0, 0).unwrap();
    assert_eq!(maximum.len(), 4);
    assert_eq!(maximum.iter().map(|b| b.data).sum::<usize>(), 4000);
    assert!(maximum.iter().all(|b| b.data <= 1023 && b.percentage == 0));
    for total in [1, 100, 800, 2000, 3000] {
        let p = plan(total, 2, 20, 2, 50, 100).unwrap();
        assert_eq!(p.iter().map(|b| b.data).sum::<usize>(), total);
        assert!(p.len() <= 4);
        assert!(
            p.iter()
                .map(|b| parity(b.data, b.percentage, 2))
                .sum::<usize>()
                <= 102
        );
    }
    assert_eq!(
        plan(1000, 0, 20, 2, 0, 0)
            .unwrap()
            .iter()
            .map(|b| parity(b.data, b.percentage, 2))
            .sum::<usize>(),
        0
    );
}
#[test]
fn detail_fec_matches_pre_pipeline_decisions() {
    let mut controller = DetailFec::new(120_000);
    let start = Instant::now();
    let mut elapsed = Duration::ZERO;
    for i in 0..80u32 {
        let order = if i % 2 == 0 {
            [0, 30, 31, 32]
        } else {
            [32, 31, 0, 30]
        };
        let mut raw = frame(&order.map(|index| (index, 20)));
        raw[3] |= ((i % 8) << 4) as u8;
        for (at, index) in order.into_iter().enumerate() {
            let offset = 8 + at * 20;
            raw[offset + 3] |= ((i % 8) << 4) as u8;
            if ((20..40).contains(&i) && index == 30) || ((40..50).contains(&i) && index >= 31) {
                raw[offset + 8..offset + 20].fill(i as u8);
            }
        }
        elapsed += Duration::from_millis(match i {
            60 => 8,
            79 => 100,
            _ => 20,
        });
        // Captured with the ordered map: sequence changes and record order
        // do not alter stability, while motion and nominal cadence reset it.
        let expected = match i {
            14..40 => (50, 2_000_000),
            74 => (50, 1_947_223),
            75 => (50, 1_957_778),
            76 => (50, 1_966_223),
            77 => (50, 1_972_978),
            78 => (50, 1_978_382),
            79 => (50, 3_582_706),
            _ => (0, 0),
        };
        assert_eq!(
            controller.observe(&raw, start + elapsed, 800_000).unwrap(),
            expected,
            "frame {i}"
        );
    }
}
#[test]
fn low_cadence_static_detail_gets_parity_and_motion_disables_it() {
    let mut controller = DetailFec::new(120000);
    let start = Instant::now();
    let same = frame(&[(0, 16), (30, 20)]);
    let mut decision = (0, 0);
    for i in 0..20 {
        decision = controller
            .observe(&same, start + Duration::from_millis(i * 20), 800000)
            .unwrap();
    }
    assert_eq!(decision.0, 50);
    assert!(decision.1 <= 2_000_000);
    assert_eq!(
        controller
            .observe(
                &frame(&[(0, 20), (30, 24)]),
                start + Duration::from_millis(400),
                800000
            )
            .unwrap(),
        (0, 0)
    );
}
