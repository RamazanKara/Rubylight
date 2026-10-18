#[test]
fn large_conventional_frames_report_the_wire_fec_cutoff() {
    assert_eq!(super::conventional_fec(848, 20, 0), (4, 20));
    // Past the cutoff the parity that still fits is kept.
    assert_eq!(super::conventional_fec(849, 20, 0), (4, 19));
    assert_eq!(super::conventional_fec(900, 20, 0), (4, 13));
    assert_eq!(super::conventional_fec(1000, 20, 0), (4, 2));
    assert_eq!(super::conventional_fec(1000, 20, 6), (4, 0));
    assert_eq!(super::conventional_fec(1020, 20, 0), (4, 0));
    assert_eq!(super::conventional_fec(5000, 20, 0), (4, 0));
    assert_eq!(super::conventional_fec(849, 0, 0), (4, 0));
    for shards in 849..1020 {
        let (blocks, percentage) = super::conventional_fec(shards, 20, 2);
        let per_block = shards.div_ceil(blocks);
        let parity = (per_block * percentage)
            .div_ceil(100)
            .max(if percentage > 0 { 2 } else { 0 });
        assert!(per_block + parity <= 255 || percentage == 0, "{shards}");
    }
}
use super::*;

#[test]
fn host_processing_uses_punktfunks_truncated_saturating_tenths_of_a_millisecond() {
    // Punktfunk v0.44.0 c5ed1136, gamestream/stream.rs spawn_packetizer:
    // elapsed microseconds / 100, then saturating u16 conversion.
    for (micros, expected) in [
        (0, 0),
        (49, 0),
        (50, 0),
        (99, 0),
        (100, 1),
        (199, 1),
        (1234, 12),
        (6_553_599, 65_535),
        (u64::MAX, 65_535),
    ] {
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 0,
            min_fec: 0,
            key: None,
        };
        let packets = packetizer.encode(&[42; 32], false, 9000, micros).unwrap();
        assert_eq!(
            u16::from_le_bytes(packets[0][33..35].try_into().unwrap()),
            expected,
            "{micros}"
        );
    }
}

#[test]
fn minimum_fec_uses_the_receivers_exact_parity_count() {
    for data in 1..=1023 {
        for percentage in [0, 1, 20, 100, 255] {
            for minimum in [0, 2, 24, 42, 255] {
                let (parity, effective) = block_parity(data, percentage, minimum);
                assert!(effective <= 255);
                assert_eq!(parity, (data * effective).div_ceil(100));
                if percentage > 0 {
                    assert!(parity >= crate::pyrowave::minimum_parity(data, minimum));
                }
            }
        }
    }
    // 44 parity shards cannot be advertised exactly for 211 data shards.
    // The representable 45 exceed the 255-shard limit, so split the frame.
    assert_eq!(conventional_fec(211, 20, 44), (2, 20));
    for data in 1..=1020 {
        for percentage in [1, 20, 100] {
            for minimum in [2, 24, 42, 255] {
                let (blocks, effective) = conventional_fec(data, percentage, minimum);
                let aligned = data.div_ceil(blocks);
                for block in 0..blocks {
                    let count = data.saturating_sub(block * aligned).min(aligned);
                    assert!(count + block_parity(count, effective, minimum).0 <= 255);
                }
            }
        }
    }
}

#[test]
fn minimum_fec_recovers_loss_using_the_wire_selected_matrix() {
    for key in [None, Some([42; 16])] {
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 42,
            key,
        };
        let payload: Vec<_> = (0..201 * 1376 - 8)
            .map(|i| (i * 29 + i / 1376) as u8)
            .collect();
        let packets: Vec<_> = packetizer
            .encode(&payload, false, 9000, 500)
            .unwrap()
            .into_iter()
            .map(|packet| match key {
                Some(key) => {
                    crypto::gcm_open(&key, &packet[..12], &packet[16..32], &packet[32..]).unwrap()
                }
                None => packet,
            })
            .collect();
        let info = u32::from_le_bytes(packets[0][28..32].try_into().unwrap());
        let data = (info >> 22) as usize;
        let percentage = ((info >> 4) & 255) as usize;
        let parity = (data * percentage).div_ceil(100);
        assert_eq!((data, percentage, parity), (201, 21, 43));
        assert_eq!(packets.len(), data + parity);
        // Recover a missing data shard with the client's matrix formula. The
        // advertised parity count selects the coefficients as well as the size.
        let missing = 17;
        for offset in [32, 40, 255, 1407] {
            let mut recovered = packets[data][offset];
            for (column, packet) in packets[..data].iter().enumerate() {
                if column != missing {
                    recovered ^= gf_mul(packet[offset], inverse((parity + column) as u8));
                }
            }
            recovered = gf_mul(recovered, (parity + missing) as u8);
            assert_eq!(recovered, packets[missing][offset]);
        }
    }
}

#[test]
fn conventional_blocks_reuse_shards_and_match_collected_packets() {
    for key in [None, Some([42; 16])] {
        let make = || VideoPacketizer {
            sequence: 0xFFFFFF,
            iv_counter: 0xFFFFFFFE,
            frame: u32::MAX,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key,
        };
        let mut expected = make();
        let mut actual = make();
        let mut buffers = Vec::new();
        let mut pointers = Vec::new();
        for size in [792_568, 10, 264_184, 792_568] {
            let payload: Vec<u8> = (0..size).map(|i| (i * 29) as u8).collect();
            let expected_packets = expected
                .encode_recovery(&payload, false, true, 9000, 500)
                .unwrap();
            let (count, mut next) = actual
                .conventional_blocks(&payload, false, true, 9000, 500)
                .unwrap();
            assert_eq!(count, expected_packets.len());
            let mut offset = 0;
            loop {
                let count = next(&mut buffers).unwrap();
                if count == 0 {
                    break;
                }
                for (buffer, pointer) in buffers.iter().zip(&pointers) {
                    assert_eq!(buffer.as_ptr(), *pointer);
                }
                pointers = buffers.iter().map(Vec::as_ptr).collect();
                assert_eq!(&buffers[..count], &expected_packets[offset..offset + count]);
                offset += count;
            }
            assert_eq!(offset, expected_packets.len());
            drop(next);
            assert_eq!(actual.frame, expected.frame);
            assert_eq!(actual.sequence, expected.sequence);
            assert_eq!(actual.iv_counter, expected.iv_counter);
        }
    }
}

#[test]
fn abandoning_conventional_blocks_keeps_frame_and_nonce_progress() {
    let payload = vec![7; 792_568];
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([42; 16]),
    };
    let mut buffers = Vec::new();
    let (_, next) = p
        .conventional_blocks(&payload, false, false, 9000, 543)
        .unwrap();
    drop(next);
    assert_eq!((p.sequence, p.iv_counter, p.frame), (0, 0, 1));
    let (total, mut next) = p
        .conventional_blocks(&payload, false, false, 9000, 543)
        .unwrap();
    let first = next(&mut buffers).unwrap();
    assert!(first < total);
    drop(next);
    assert_eq!(p.sequence, first as u32);
    assert_eq!(p.iv_counter, first as u64);
    assert_eq!(p.frame, 2);
    assert_eq!(&buffers[0][12..16], &1u32.to_le_bytes());
    let (count, mut next) = p
        .conventional_blocks(&payload, true, false, 9750, 543)
        .unwrap();
    assert_eq!(count, total);
    assert_eq!(next(&mut buffers).unwrap(), first);
    assert_eq!(&buffers[0][..8], &(first as u64).to_le_bytes());
    assert_eq!(&buffers[0][12..16], &2u32.to_le_bytes());
}

#[test]
fn conventional_blocks_match_pre_pipeline_fingerprints() {
    let cases = [
        (
            32,
            false,
            "bc399e222e5a4ae7e26c450a17255b398230f83b22887ecb1a154fe184a32b76",
        ),
        (
            32,
            true,
            "8ba8584ce4c91aaf819f46ddecca4ca225afbcc57df358cce0d083f05fc1eef2",
        ),
        (
            192,
            false,
            "ef623c7f9e369b6f484c8b25674b640051689bddf0d143c179a0f22e778425a2",
        ),
        (
            192,
            true,
            "fd2014942f4fe1b55b4656ece29610ace6e30c4c5ef9342be2c3e04a7be27da5",
        ),
        (
            576,
            false,
            "6651ed521955b8d3e198bea9bad990362efa386d686968acada77b0d56e8a8ad",
        ),
        (
            576,
            true,
            "f42a64f087e69d821b868d5dbbc5310f43b320d1cb6f139d1583af19fc2b187e",
        ),
    ];
    for (shards, encrypted, fingerprint) in cases {
        let payload: Vec<u8> = (0..shards * 1376 - 8)
            .map(|i| ((i * 73 + (i >> 3)) & 255) as u8)
            .collect();
        let mut p = VideoPacketizer {
            sequence: 65000,
            iv_counter: 65534,
            frame: 7,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: encrypted.then(|| std::array::from_fn(|i| i as u8)),
        };
        let (_, mut next) = p
            .conventional_blocks(&payload, true, false, 9000, 500)
            .unwrap();
        let mut buffers = Vec::new();
        let mut wire = Vec::new();
        loop {
            let count = next(&mut buffers).unwrap();
            if count == 0 {
                break;
            }
            for packet in &buffers[..count] {
                wire.extend_from_slice(packet);
            }
        }
        assert_eq!(hex::encode(crypto::hash(&wire)), fingerprint);
    }
}

#[test]
fn abandoning_pyrowave_blocks_keeps_frame_and_nonce_progress() {
    let payload = vec![7; 400_000];
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 0,
        min_fec: 2,
        key: Some([42; 16]),
    };
    let fec = || PyrowaveFec {
        records: false,
        critical_percentage: 20,
        detail_percentage: 0,
        wire_budget: 0,
        ipv6: false,
    };
    let (total, _, mut blocks) = p.pyrowave_blocks(&payload, 9000, 543, fec()).unwrap();
    let first = blocks.next().unwrap().unwrap();
    assert!(first.len() < total);
    drop(blocks);
    assert_eq!(p.sequence, first.len() as u32);
    assert_eq!(p.iv_counter, first.len() as u64);
    assert_eq!(p.frame, 2);
    assert_eq!(&first[0][12..16], &1u32.to_le_bytes());

    let next = p.encode_pyrowave(&payload, 9750, 543, fec()).unwrap();
    assert_eq!(next.len(), total);
    assert_eq!(&next[0][..8], &(first.len() as u64).to_le_bytes());
    assert_eq!(&next[0][12..16], &2u32.to_le_bytes());
    assert_eq!(p.frame, 3);
    assert_eq!(p.sequence, (first.len() + total) as u32);
    assert_eq!(p.iv_counter, (first.len() + total) as u64);
}

#[test]
fn legacy_pyrowave_uses_the_conventional_receivers_fec_bytes() {
    for key in [None, Some([42; 16])] {
        for (shards, minimum) in [(1, 2), (201, 42), (600, 2), (849, 2), (1020, 2), (1200, 2)] {
            let make = || VideoPacketizer {
                sequence: 65534,
                iv_counter: 65534,
                frame: 7,
                packet_size: 1392,
                fec_percent: 20,
                min_fec: minimum,
                key,
            };
            let payload: Vec<_> = (0..shards * 1376 - 8).map(|i| (i * 29) as u8).collect();
            let expected = make().encode(&payload, true, 9000, 543).unwrap();
            let mut pyro = make();
            let (count, limited, blocks) = pyro
                .pyrowave_blocks(
                    &payload,
                    9000,
                    543,
                    PyrowaveFec {
                        records: false,
                        critical_percentage: 20,
                        detail_percentage: 50,
                        wire_budget: usize::MAX,
                        ipv6: true,
                    },
                )
                .unwrap();
            let actual: Vec<_> = blocks.flat_map(|block| block.unwrap()).collect();
            assert_eq!(actual.len(), count);
            // Legacy framing changes neither parity bytes nor the ordinary
            // receiver's matrix selection, including minimum-parity rounding.
            assert_eq!(actual, expected, "{shards} shards, minimum {minimum}");
            assert_eq!(limited, conventional_fec(shards, 20, minimum).1 < 20);
            assert_eq!(count > shards, shards < 1020);
        }
    }
}

#[test]
fn legacy_pyrowave_rejects_frames_past_the_legacy_shard_cap() {
    let mut packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 256,
        fec_percent: 0,
        min_fec: 2,
        key: None,
    };
    for rate in [0, 20] {
        assert!(
            packetizer
                .pyrowave_blocks(
                    &vec![0; 4000 * 240],
                    0,
                    0,
                    PyrowaveFec {
                        records: false,
                        critical_percentage: rate,
                        detail_percentage: 0,
                        wire_budget: 0,
                        ipv6: false,
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn pyrowave_blocks_match_pre_pipeline_bytes() {
    use crate::pyrowave;
    // Captured before incremental sending, including parity, record restart
    // flags, encryption, and two frames crossing the sequence/frame wrap.
    let cases = [
        (
            1392,
            true,
            800,
            0,
            2,
            619,
            "b2b826c26f16471691fe0ca4d2207a6ffef47cd0c51ce2a5222856f5e7f9acec",
            [
                "169af59cd64cc956d735e679ebeaa8442224acc11f2c7f341cf1ac4e276b3539",
                "bb105d41f7fd6c13a6c16bd31213c8364ba4c514c8375d900a64e0a996b4f992",
            ],
        ),
        (
            1392,
            true,
            800,
            50,
            2,
            805,
            "b2b826c26f16471691fe0ca4d2207a6ffef47cd0c51ce2a5222856f5e7f9acec",
            [
                "06de60def4c061f8e9835050aa6233f71d8e6b8cae6734b2f5cea1713dd6e50d",
                "bada91533e981e5c142d2860ad726c9f2943b7cb40625b8dc759b4284d50ae5f",
            ],
        ),
        (
            1390,
            true,
            800,
            50,
            2,
            590,
            "4d6e3b3dc7f7f37d09442e2a532b7562e222d38a0541e14cd87190d33e5b8421",
            [
                "22573bace717c714caa86542103f4b494181c9f448b3225118f618fd2da5d071",
                "64900b28c9b4e171d0dad8898e15f6e243e21eb99b1bbed2b4e651e3340a4a80",
            ],
        ),
        (
            1400,
            false,
            800,
            0,
            2,
            585,
            "abb0c4381be111b03f15691e8d7a056e135f851358a2f8e46d4452a86c2740c5",
            [
                "e89b3bfb00e3087a807b5cb18df98b46645c866ea48199506123ca9944617979",
                "de8233c384c800bbc88f9f34395562a81ddcaff97e2a906f6ec9e93fbbcd5a6e",
            ],
        ),
        (
            256,
            true,
            24,
            50,
            4,
            102,
            "3e05f81bb5698a8d8b86b0613fcda7bf1c9ae99cde33ce1d9168e8ffea446e70",
            [
                "08962468200244aa2219af92e5058848da77016a1d08e55b87d6dcb330b63a66",
                "199475562fd3b81146a67a55f11307cab5c95106e63c4d19f341cae9cc354f89",
            ],
        ),
        (
            1392,
            true,
            24,
            0,
            2,
            20,
            "be5a30a5632247d5eff24f07c3bdc1de4f53237a02f6cd2b023b50d901b4aea4",
            [
                "1f5230361b737209b8fae71640a49c831527599629e69446884d839dd37e4252",
                "5514b5aefaf96cb6d6779a3fb57dfd1e8c0c08d1ce69df47a12d399cacba04fd",
            ],
        ),
    ];
    for (size, records, count, detail, minimum, packet_count, framed_hash, wire_hashes) in cases {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
        raw.extend_from_slice(&(count as u32).to_le_bytes());
        for i in 0..count {
            let bytes = 8 + (i * 73 % 504) * 4;
            raw.extend_from_slice(&((bytes as u32 / 4) << 16).to_le_bytes());
            raw.extend_from_slice(&((((i * 37) % count) as u32) << 8).to_le_bytes());
            raw.extend((0..bytes - 8).map(|j| (i * 31 + j * 17) as u8));
        }
        let framed = if records {
            pyrowave::record_frame(
                &raw,
                pyrowave::aligned_payload(size),
                pyrowave::max_frame_bytes(size, true),
            )
            .unwrap()
        } else {
            pyrowave::container(&raw, &[(0, 8), (8, raw.len() - 8)]).unwrap()
        };
        assert_eq!(hex::encode(crypto::hash(&framed)), framed_hash);
        for (encrypted, expected) in [false, true].into_iter().zip(wire_hashes) {
            let mut p = VideoPacketizer {
                sequence: u32::MAX - 5,
                iv_counter: 65534,
                frame: u32::MAX,
                packet_size: size,
                fec_percent: 0,
                min_fec: minimum,
                key: encrypted.then(|| std::array::from_fn(|i| i as u8)),
            };
            let mut all = Vec::new();
            for _ in 0..2 {
                let (total, fec_limited, blocks) = p
                    .pyrowave_blocks(
                        &framed,
                        9000,
                        543,
                        PyrowaveFec {
                            records,
                            critical_percentage: if records { 20 } else { 0 },
                            detail_percentage: detail,
                            wire_budget: 2_000_000,
                            ipv6: true,
                        },
                    )
                    .unwrap();
                assert_eq!(total, packet_count);
                // The unaligned 1390-byte fixture cannot carry critical FEC.
                assert_eq!(fec_limited, records && size == 1390);
                let mut emitted = 0;
                for (block_index, block) in blocks.enumerate() {
                    let block = block.unwrap();
                    emitted += block.len();
                    for packet in block {
                        let plain = if encrypted {
                            crypto::gcm_open(
                                &std::array::from_fn(|i| i as u8),
                                &packet[..12],
                                &packet[16..32],
                                &packet[32..],
                            )
                            .unwrap()
                        } else {
                            packet.clone()
                        };
                        assert_eq!((plain[27] >> 4) & 3, block_index as u8);
                        all.extend(packet);
                    }
                }
                assert_eq!(emitted, total);
            }
            assert_eq!(hex::encode(crypto::hash(&all)), expected);
            assert_eq!(p.frame, 1);
            assert_eq!(
                p.sequence,
                (u32::MAX - 5).wrapping_add(2 * packet_count as u32)
            );
            assert_eq!(
                p.iv_counter,
                65534
                    + if encrypted {
                        2 * packet_count as u64
                    } else {
                        0
                    }
            );
        }
    }
}
#[test]
fn fec_excludes_the_reserved_envelope_and_preserves_all_row_tails() {
    for (data, parity, size) in [
        (1, 2, 0),
        (8, 4, 31),
        (28, 7, 1392),
        (192, 39, 1408),
        (254, 1, 1416),
    ] {
        let mut expected: Vec<Vec<u8>> = (0..data + parity)
            .map(|row| (0..size).map(|i| (row * 73 + i * 29) as u8).collect())
            .collect();
        let mut actual: Vec<Vec<u8>> = expected
            .iter()
            .map(|bytes| {
                let mut packet = vec![0xAB; 32];
                packet.extend_from_slice(bytes);
                packet
            })
            .collect();
        cauchy_encode(&mut expected, data, parity).unwrap();
        cauchy_encode_offset::<32>(&mut actual, data, parity).unwrap();
        for (packet, bytes) in actual.iter().zip(expected) {
            assert_eq!(&packet[..32], &[0xAB; 32]);
            assert_eq!(&packet[32..], bytes);
        }
    }
    assert!(cauchy_encode_offset::<32>(&mut [vec![0; 31], vec![0; 31]], 1, 1).is_err());
}
#[test]
fn replay_window_accepts_reordering_but_rejects_duplicates_and_wraps() {
    let mut window = ReplayWindow::default();
    for n in [100, 102, 101, 103, 99] {
        assert!(window.accept(n));
    }
    for n in [100, 99, 102] {
        assert!(!window.accept(n));
    }
    // A keyframe request resent behind hundreds of motion reports.
    for n in 104..=900 {
        assert!(window.accept(n));
    }
    assert!(window.accept(1));
    assert!(!window.accept(1));
    // Older than the window, or reusing a slot a newer sequence cleared.
    assert!(window.accept(10_000));
    assert!(!window.accept(10_000 - 4096));
    assert!(window.accept(10_000 - 4095));
    assert!(!window.accept(10_000 - 4095));
    assert!(window.accept(u32::MAX));
    assert!(!window.accept(0));
}
#[test]
fn forged_legacy_input_cannot_change_the_next_iv() {
    let key = [3; 16];
    let mut input = LegacyInput::new(0x12345678);
    let original = input.iv;
    let raw = vec![7; 40];
    let (tag, encrypted) = crypto::gcm_seal(&key, &original, &raw).unwrap();
    let mut packet = ((tag.len() + encrypted.len()) as u32)
        .to_be_bytes()
        .to_vec();
    packet.extend_from_slice(&tag);
    packet.extend_from_slice(&encrypted);
    let mut bad = packet.clone();
    bad[5] ^= 1;
    assert!(input.open(&key, &bad).is_err());
    assert_eq!(input.iv, original);
    assert_eq!(input.open(&key, &packet).unwrap(), raw);
    assert_eq!(input.iv, packet[packet.len() - 16..]);
}
#[test]
fn simd_cauchy_matches_scalar_for_every_coefficient_and_tail() {
    for coefficient in 0..=255 {
        for len in [1, 15, 16, 17, 31, 33, 1023] {
            let source: Vec<u8> = (0..len).map(|i| (i * 71 + 13) as u8).collect();
            let mut out = vec![0x55; len];
            axpy(&mut out, &source, coefficient);
            let expected: Vec<u8> = source
                .iter()
                .map(|v| 0x55 ^ gf_mul(*v, coefficient))
                .collect();
            assert_eq!(out, expected);
        }
    }
}
#[test]
fn batched_cauchy_matches_independent_scalar_for_row_groups_and_short_tails() {
    for (data, parity) in [
        (1, 254),
        (254, 1),
        (4, 2),
        (4, 3),
        (4, 4),
        (4, 5),
        (4, 6),
        (4, 7),
        (32, 7),
        (96, 20),
        (192, 39),
    ] {
        for length in [0, 1, 15, 31, 32, 33, 63, 65, 1416] {
            let mut shards: Vec<Vec<u8>> = (0..data + parity)
                .map(|i| (0..length).map(|j| (i * 29 + j * 71 + 13) as u8).collect())
                .collect();
            let mut expected = shards.clone();
            for (row, out) in expected[data..].iter_mut().enumerate() {
                out.fill(0);
                for (column, input) in shards[..data].iter().enumerate() {
                    let coefficient = inverse((parity + column) as u8 ^ row as u8);
                    for (out, input) in out.iter_mut().zip(input) {
                        *out ^= gf_mul(*input, coefficient);
                    }
                }
            }
            cauchy_encode(&mut shards, data, parity).unwrap();
            assert_eq!(shards, expected, "matrix {data}+{parity}, {length} bytes");
        }
    }
}
#[test]
fn packet_insert_crosses_sources_at_every_boundary() {
    for h in 0..10 {
        for s in 0..16 {
            for a in 0..25 {
                for b in 0..25 {
                    let aa = vec![1; a];
                    let bb = vec![2; b];
                    let joined = [aa.clone(), bb.clone()].concat();
                    let expected: Vec<u8> = if s == 0 {
                        vec![]
                    } else {
                        joined
                            .chunks(s)
                            .flat_map(|p| [vec![0; h], p.to_vec()].concat())
                            .collect()
                    };
                    assert_eq!(concat_and_insert(h, s, &aa, &bb).unwrap(), expected);
                }
            }
        }
    }
}
#[test]
fn video_headers_follow_moonlight_layout() {
    let mut p = VideoPacketizer {
        sequence: 65535,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 0,
        min_fec: 0,
        key: None,
    };
    let packets = p.encode(&vec![3; 1100], true, 900, 200).unwrap();
    assert_eq!(packets.len(), 2);
    assert_eq!(&packets[0][..4], &[0x90, 0, 0xff, 0xff]);
    assert_eq!(packets[0][24], 5);
    assert_eq!(packets[1][24], 3);
    assert_eq!(&packets[0][32..40], &[1, 2, 0, 2, 100, 0, 0, 0]);
    assert_eq!(&packets[1][2..4], &[0, 0]);
    let stream_index = |packet: &[u8]| u32::from_le_bytes(packet[16..20].try_into().unwrap()) >> 8;
    assert_eq!(stream_index(&packets[0]), 65535);
    assert_eq!(stream_index(&packets[1]), 65536);
    let recovered = p.encode_recovery(&[3; 16], false, true, 1000, 300).unwrap();
    assert_eq!(recovered[0][35], 5); // Moonlight recovery marker
    assert_eq!(stream_index(&recovered[0]), 65537);
    let keyframe = p.encode_recovery(&[3; 16], true, true, 1100, 300).unwrap();
    assert_eq!(keyframe[0][35], 2); // IDR takes precedence
    p.sequence = 0xFFFFFF;
    let wrapped = p.encode(&vec![3; 1100], true, 1200, 300).unwrap();
    assert_eq!(stream_index(&wrapped[0]), 0xFFFFFF);
    assert_eq!(stream_index(&wrapped[1]), 0);
    assert_eq!(&wrapped[0][2..4], &[0xff, 0xff]);
    assert_eq!(&wrapped[1][2..4], &[0, 0]);
}
#[test]
fn encrypted_video_nonce_is_unique_and_authenticated() {
    let k = [5; 16];
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 20,
        min_fec: 1,
        key: Some(k),
    };
    let b = p.encode(&vec![1; 1100], false, 123, 0).unwrap();
    assert_ne!(&b[0][..12], &b[1][..12]);
    let plain = crypto::gcm_open(&k, &b[0][..12], &b[0][16..32], &b[0][32..]).unwrap();
    assert_eq!(plain[0], 0x90);
    assert_eq!(plain[24], 5);
    p.iv_counter = u64::MAX;
    assert!(p.encode(&[1], true, 124, 0).is_err());
    assert_eq!(p.iv_counter, u64::MAX);
}
#[test]
fn encrypted_video_matches_independent_per_packet_sealing_across_fec_and_wraps() {
    let key = std::array::from_fn(|i| i as u8);
    for packet_size in [256, 1392] {
        for fec_percent in [0, 20] {
            let make = |key| VideoPacketizer {
                sequence: 0xFFFFFF,
                iv_counter: 0xFFFFFFFE,
                frame: u32::MAX,
                packet_size,
                fec_percent,
                min_fec: 1,
                key,
            };
            let mut plain = make(None);
            let mut encrypted = make(Some(key));
            let slice = packet_size - 16;
            for size in [1, slice - 7, 192 * slice - 8, 576 * slice + 13] {
                let payload: Vec<_> = (0..size).map(|i| (i * 29) as u8).collect();
                let frame = plain.frame;
                let nonce = encrypted.iv_counter;
                let expected = plain.encode(&payload, true, 9000, 500).unwrap();
                let actual = encrypted.encode(&payload, true, 9000, 500).unwrap();
                assert_eq!(actual.len(), expected.len());
                for (i, (packet, shard)) in actual.iter().zip(expected).enumerate() {
                    let mut iv = [0; 12];
                    iv[..8].copy_from_slice(&(nonce + i as u64).to_le_bytes());
                    iv[11] = b'V';
                    let (tag, bytes) = crypto::gcm_seal(&key, &iv, &shard).unwrap();
                    assert_eq!(&packet[..12], &iv);
                    assert_eq!(&packet[12..16], &frame.to_le_bytes());
                    assert_eq!(&packet[16..32], &tag);
                    assert_eq!(&packet[32..], bytes);
                }
                assert_eq!(plain.frame, encrypted.frame);
                assert_eq!(plain.sequence, encrypted.sequence);
                assert_eq!(encrypted.iv_counter, nonce + actual.len() as u64);
            }
        }
    }
}
fn frame_index(packet: &[u8]) -> u32 {
    u32::from_le_bytes(packet[20..24].try_into().unwrap())
}
#[test]
fn a_frame_beyond_the_packet_limit_still_takes_its_frame_index() {
    // Moonlight sees a lost frame only from a gap in the frame index.
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: None,
    };
    let first = p.encode(&[1; 5000], true, 0, 0).unwrap();
    assert_eq!(frame_index(&first[0]), 1);
    assert!(p.encode(&vec![2; 1376 * 4092], false, 1500, 0).is_err());
    let third = p.encode(&[3; 5000], false, 3000, 0).unwrap();
    assert_eq!(frame_index(&third[0]), 3);
}
#[test]
fn a_large_minimum_parity_fits_the_fec_percentage_field() {
    for min_fec in [2, 3, 10, 255] {
        let mut p = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec,
            key: None,
        };
        let packets = p.encode(&[1; 10], false, 0, 0).unwrap();
        for (index, packet) in packets.iter().enumerate() {
            let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
            let (shard, data, percentage) = ((info >> 12) & 0x3ff, info >> 22, (info >> 4) & 0xff);
            assert_eq!(shard as usize, index, "min_fec {min_fec}");
            // moonlight-common-c derives the parity count from the percentage.
            assert_eq!(
                ((data * percentage).div_ceil(100) + data) as usize,
                packets.len()
            );
        }
    }
}
#[test]
fn block_layout_matches_the_packets_each_frame_is_sent_as() {
    for (packet_size, fec_percent, min_fec) in
        [(1392, 20, 2), (1024, 0, 0), (1392, 50, 6), (512, 10, 1)]
    {
        let mut p = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size,
            fec_percent,
            min_fec,
            key: None,
        };
        for size in [
            1, 100, 1376, 1377, 40_000, 290_000, 1_180_000, 1_400_000, 3_000_000,
        ] {
            let Ok(packets) = p.encode(&vec![7; size], false, 0, 0) else {
                assert!(size > (packet_size - 16) * 4092 - 8);
                continue;
            };
            let mut sent: Vec<(usize, usize)> = Vec::new();
            for packet in &packets {
                let block = usize::from((packet[27] >> 4) & 3);
                let data = (u32::from_le_bytes(packet[28..32].try_into().unwrap()) >> 22) as usize;
                if sent.len() == block {
                    sent.push((data, 0));
                }
                sent[block].1 += 1;
            }
            let sent: Vec<_> = sent
                .into_iter()
                .map(|(data, packets)| (data, packets - data))
                .collect();
            assert_eq!(
                p.block_layout(size),
                sent,
                "size {size} packet {packet_size} fec {fec_percent}"
            );
            assert_eq!(
                sent.iter().map(|(d, f)| d + f).sum::<usize>(),
                packets.len()
            );
        }
    }
}
