use super::*;

const SLICE: usize = 1376;

fn packetizer(fec_percent: usize, min_fec: usize, key: Option<[u8; 16]>) -> VideoPacketizer {
    VideoPacketizer {
        sequence: 100,
        iv_counter: 5,
        frame: 7,
        packet_size: 1392,
        fec_percent,
        min_fec,
        key,
    }
}

fn frame_bytes(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i * 31 + i / 7 + 1) as u8).collect()
}

/// Split `payload` into `units` non-empty parts at pseudo-random places.
fn cuts(size: usize, units: usize, random: &mut impl FnMut(usize) -> usize) -> Vec<usize> {
    let units = units.min(size).max(1);
    let mut ends: Vec<usize> = Vec::new();
    while ends.len() + 1 < units {
        let end = 1 + random(size - 1);
        if !ends.contains(&end) {
            ends.push(end);
        }
    }
    ends.sort_unstable();
    ends.push(size);
    let mut previous = 0;
    ends.into_iter()
        .map(|end| {
            let part = end - previous;
            previous = end;
            part
        })
        .collect()
}

#[derive(Default)]
struct Sent {
    packets: Vec<Vec<u8>>,
    blocks: Vec<StreamedBlock>,
    /// The 1-based unit whose arrival built each block.
    unit_of: Vec<usize>,
    early: usize,
    fec_limited: bool,
    done: bool,
}

/// Feed `parts` to a streamed frame, collecting what it built.
fn run(
    p: &mut VideoPacketizer,
    payload: &[u8],
    parts: &[usize],
    blocks: usize,
    slices: usize,
    cap: usize,
) -> (Sent, Result<()>) {
    let mut frame = p.streamed(9000, 500, blocks, slices, cap).unwrap();
    let mut sent = Sent::default();
    let mut shards = PacketBlock::new();
    let mut offset = 0;
    let mut result = Ok(());
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        let unit = &payload[offset..offset + part];
        offset += part;
        let out = &mut sent;
        result = frame.push(p, unit, last, &mut shards, |packets, block| {
            out.packets.extend(packets.iter().cloned());
            out.blocks.push(block);
            out.unit_of.push(index + 1);
            Ok(())
        });
        if result.is_err() {
            break;
        }
    }
    sent.early = frame.early_blocks();
    sent.fec_limited = frame.fec_limited();
    sent.done = frame.is_done();
    (sent, result)
}

fn open(packet: &[u8], key: Option<[u8; 16]>) -> Vec<u8> {
    match key {
        Some(key) => crypto::gcm_open(&key, &packet[..12], &packet[16..32], &packet[32..]).unwrap(),
        None => packet.to_vec(),
    }
}

/// Take the packets apart as moonlight-common-c's `RtpVideoQueue` does and
/// check what it checks: one frame index and block count on every packet,
/// blocks in order, each block's shard counts read from its own packets,
/// full data shards. Returns the frame's byte stream (frame header, payload,
/// zero padding) and the (data, parity) of each block.
fn reassemble(packets: &[Vec<u8>], key: Option<[u8; 16]>) -> (Vec<u8>, Vec<(usize, usize)>) {
    let plain: Vec<_> = packets.iter().map(|p| open(p, key)).collect();
    let frame = plain[0][20..24].to_vec();
    let total_blocks = usize::from(plain[0][27] >> 6) + 1;
    let (mut stream, mut layout) = (Vec::new(), Vec::new());
    let (mut at, mut block) = (0, 0);
    while at < plain.len() {
        let info = u32::from_le_bytes(plain[at][28..32].try_into().unwrap());
        let (data, percentage) = ((info >> 22) as usize, ((info >> 4) & 0xff) as usize);
        let parity = (data * percentage).div_ceil(100);
        assert!(data >= 1 && data + parity <= 255 + 768 * usize::from(parity == 0));
        assert!(
            at + data + parity <= plain.len(),
            "block {block} is cut short"
        );
        for (index, shard) in plain[at..at + data + parity].iter().enumerate() {
            assert_eq!(shard.len(), SLICE + 32, "every packet is a full shard");
            assert_eq!(&shard[20..24], &frame[..], "one frame index");
            assert_eq!(
                usize::from(shard[27] >> 6) + 1,
                total_blocks,
                "fixed block count"
            );
            assert_eq!(usize::from((shard[27] >> 4) & 3), block, "blocks in order");
            let info = u32::from_le_bytes(shard[28..32].try_into().unwrap());
            assert_eq!(((info >> 12) & 0x3ff) as usize, index);
            assert_eq!(info >> 22, data as u32);
            assert_eq!(((info >> 4) & 0xff) as usize, percentage);
            assert_eq!(shard[0], 0x90);
            if index < data {
                assert_eq!(shard[24] & 4 != 0, index == 0);
                assert_eq!(shard[24] & 2 != 0, index == data - 1);
                stream.extend_from_slice(&shard[32..]);
            }
        }
        for pair in plain[at..at + data + parity].windows(2) {
            let seq = |p: &Vec<u8>| u16::from_be_bytes([p[2], p[3]]);
            assert_eq!(seq(&pair[1]), seq(&pair[0]).wrapping_add(1));
        }
        layout.push((data, parity));
        at += data + parity;
        block += 1;
    }
    assert_eq!(block, total_blocks, "every announced block arrived");
    (stream, layout)
}

fn assert_identical(stream: &[u8], payload: &[u8]) {
    assert_eq!(stream[0], 1);
    assert_eq!(stream[3], 1, "streamed frames are P-frames");
    assert_eq!(&stream[6..8], &[0, 0]);
    assert_eq!(&stream[8..8 + payload.len()], payload);
    assert!(stream[8 + payload.len()..].iter().all(|b| *b == 0));
    assert!(stream.len() - 8 - payload.len() < SLICE);
    assert_eq!(stream.len() % SLICE, 0);
}

/// Recover one missing data shard of every block from its first parity shard
/// with the receiver's matrix, as `minimum_fec_recovers_loss...` does.
fn assert_recoverable(packets: &[Vec<u8>], key: Option<[u8; 16]>, layout: &[(usize, usize)]) {
    let plain: Vec<_> = packets.iter().map(|p| open(p, key)).collect();
    let mut at = 0;
    for &(data, parity) in layout {
        let block = &plain[at..at + data + parity];
        at += data + parity;
        if parity == 0 {
            continue;
        }
        for missing in [0, data / 2, data - 1] {
            for offset in [32, 40, 700, SLICE + 31] {
                let mut recovered = block[data][offset];
                for (column, packet) in block[..data].iter().enumerate() {
                    if column != missing {
                        recovered ^= gf_mul(packet[offset], inverse((parity + column) as u8));
                    }
                }
                recovered = gf_mul(recovered, (parity + missing) as u8);
                assert_eq!(
                    recovered, block[missing][offset],
                    "block of {data}+{parity}"
                );
            }
        }
    }
}

#[test]
fn a_streamed_frame_with_matching_block_boundaries_equals_the_conventional_one() {
    for key in [None, Some([42; 16])] {
        // 400 shards are two blocks of 200 at 20%. The first unit holds 201:
        // a shard stays back for the later block.
        let payload = frame_bytes(400 * SLICE - 8);
        let mut expected = packetizer(20, 2, key);
        let conventional = expected.encode(&payload, false, 9000, 500).unwrap();
        let mut actual = packetizer(20, 2, key);
        let parts = [201 * SLICE - 8, 199 * SLICE];
        let (sent, result) = run(&mut actual, &payload, &parts, 2, 2, payload.len() + 5000);
        result.unwrap();
        assert!(sent.done);
        assert_eq!(sent.early, 1);
        assert_eq!(sent.unit_of, [1, 2]);
        assert_eq!(sent.packets, conventional);
        assert_eq!(
            (actual.frame, actual.sequence, actual.iv_counter),
            (expected.frame, expected.sequence, expected.iv_counter)
        );
        assert_eq!(actual.block_layout(payload.len()), [(200, 40), (200, 40)]);
    }
}

#[test]
fn one_unit_is_framed_exactly_as_a_whole_frame() {
    // Nothing has left before the last unit, so the block count is still free.
    for (key, size) in [
        (None, 100),
        (Some([1; 16]), 40_000),
        (None, 400 * SLICE - 8),
        (None, 1_300_000),
    ] {
        let payload = frame_bytes(size);
        let mut expected = packetizer(20, 2, key);
        let conventional = expected.encode(&payload, false, 9000, 500).unwrap();
        let mut actual = packetizer(20, 2, key);
        let (sent, result) = run(&mut actual, &payload, &[size], 4, 4, size + 1000);
        result.unwrap();
        assert_eq!(sent.early, 0);
        assert_eq!(sent.packets, conventional);
    }
    // Units that never completed a block early end the same way, with the
    // exact last payload length a whole frame announces.
    let size = 3 * SLICE + 100;
    let payload = frame_bytes(size);
    let mut expected = packetizer(20, 2, None);
    let conventional = expected.encode(&payload, false, 9000, 500).unwrap();
    let mut actual = packetizer(20, 2, None);
    let parts = [SLICE / 2, SLICE / 2, SLICE, size - 2 * SLICE];
    let (sent, result) = run(&mut actual, &payload, &parts, 4, 4, 40_000);
    result.unwrap();
    assert_eq!(sent.early, 0);
    assert_eq!(sent.packets, conventional);
}

#[test]
fn carried_tails_keep_every_packet_full_and_the_block_plan_fixed() {
    for key in [None, Some([9; 16])] {
        // Units end in the middle of shards; four blocks of 120 B shards.
        let size = 122 * SLICE + 700;
        let payload = frame_bytes(size);
        let mut p = packetizer(20, 2, key);
        let parts = [
            30 * SLICE + 11,
            30 * SLICE + 500,
            31 * SLICE + 1,
            size - (91 * SLICE + 512),
        ];
        let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 243 * SLICE);
        result.unwrap();
        // The cap of two frames' worth lets every block but the last leave early.
        assert_eq!(sent.early, 3);
        assert_eq!(sent.blocks.len(), 4);
        assert_eq!(sent.unit_of, [1, 2, 3, 4]);
        let (stream, layout) = reassemble(&sent.packets, key);
        assert_identical(&stream, &payload);
        let seen: Vec<_> = sent.blocks.iter().map(|b| (b.data, b.parity)).collect();
        assert_eq!(layout, seen);
        assert_eq!(
            seen.iter().map(|b| b.0).sum::<usize>(),
            (size + 8).div_ceil(SLICE)
        );
        // Each early block is bounded by what was complete minus the hold-back.
        assert_eq!(sent.blocks[0].data, 30 - 3);
        assert!(!sent.fec_limited);
        assert_recoverable(&sent.packets, key, &layout);
    }
}

#[test]
fn a_tiny_frame_keeps_a_shard_back_for_every_later_block() {
    // Seven shards in four units: block 0 may take one, never more.
    let size = 6 * SLICE + 700;
    let payload = frame_bytes(size);
    let mut p = packetizer(20, 2, None);
    let head = [4 * SLICE + 500 - 8, SLICE, SLICE - 200];
    let parts = [head[0], head[1], head[2], size - head.iter().sum::<usize>()];
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 100 * SLICE);
    result.unwrap();
    assert!(sent.blocks.iter().all(|b| b.data >= 1));
    assert_eq!(sent.blocks.len(), 4);
    assert_eq!(sent.blocks[0].data, 1, "{:?}", sent.blocks);
    let (stream, _) = reassemble(&sent.packets, None);
    assert_identical(&stream, &payload);
    // Too little for the later blocks when the last unit arrives: still one
    // whole frame, because nothing had left.
    let payload = frame_bytes(2 * SLICE);
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &[SLICE, SLICE], 4, 4, 100 * SLICE);
    result.unwrap();
    assert_eq!((sent.early, sent.blocks.len()), (0, 1));
    assert_identical(&reassemble(&sent.packets, None).0, &payload);
}

#[test]
fn only_what_the_cap_allows_leaves_early() {
    // 4K60 at 80 Mbps: about 122 shards, the cap twice that.
    let payload = frame_bytes(122 * SLICE);
    let parts = [31 * SLICE, 31 * SLICE, 30 * SLICE, 30 * SLICE];
    let parts_total: usize = parts.iter().sum();
    assert_eq!(parts_total, 122 * SLICE);
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 243 * SLICE);
    result.unwrap();
    assert_eq!(sent.early, 3);
    assert_identical(&reassemble(&sent.packets, None).0, &payload);

    // 500 Mbps HEVC: a cap of about 1515 shards. The third block would leave
    // a remainder the last block could not hold at 0%, so it waits.
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 1515 * SLICE);
    result.unwrap();
    assert_eq!(sent.early, 2);
    assert_eq!(sent.blocks.len(), 4);
    assert_eq!(sent.unit_of[..2], [1, 2]);
    assert_eq!(sent.unit_of[2..], [4, 4]);
    assert!(!sent.fec_limited);
    let (stream, layout) = reassemble(&sent.packets, None);
    assert_identical(&stream, &payload);
    assert_recoverable(&sent.packets, None, &layout);

    // A cap so large that even the first block could leave a remainder no
    // later block can hold: the frame goes whole.
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 4000 * SLICE);
    result.unwrap();
    assert_eq!((sent.early, sent.blocks.len()), (0, 1));
    assert_identical(&reassemble(&sent.packets, None).0, &payload);

    // At 3000 shards the first block leaves, the second would not fit.
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 3000 * SLICE);
    result.unwrap();
    assert_eq!(sent.early, 1);
    assert_eq!(sent.blocks.len(), 4);
    assert_identical(&reassemble(&sent.packets, None).0, &payload);

    // What cannot be carried in the blocks at all is not streamed.
    let p = packetizer(20, 2, None);
    assert!(p.streamable(4, 4000 * SLICE));
    assert!(!p.streamable(4, 4092 * SLICE));
    assert!(!p.streamable(2, 2046 * SLICE));
    assert!(p.streamable(2, 2000 * SLICE));
    assert!(!p.streamable(1, SLICE));
    assert!(!p.streamable(5, SLICE));
    assert!(p.streamed(0, 0, 2, 2, 2046 * SLICE).is_err());
}

#[test]
fn the_last_unit_lowers_parity_only_as_far_as_the_remaining_blocks_need() {
    // A big frame at a big cap: the last blocks hold what is left, at the
    // frame's percentage if that fits 255 shards a block, else less.
    let size = 900 * SLICE;
    let payload = frame_bytes(size);
    let parts = [230 * SLICE, 230 * SLICE, 230 * SLICE, size - 690 * SLICE];
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 1000 * SLICE);
    result.unwrap();
    assert_eq!(sent.blocks.len(), 4);
    assert!(
        sent.blocks
            .iter()
            .all(|b| b.data + b.parity <= 255 || b.parity == 0)
    );
    let (stream, layout) = reassemble(&sent.packets, None);
    assert_identical(&stream, &payload);
    assert_recoverable(&sent.packets, None, &layout);

    // Remaining blocks that cannot fit with parity go without it, as whole
    // frames do, and the frame says so.
    let size = 3000 * SLICE;
    let payload = frame_bytes(size);
    let parts = [900 * SLICE, 900 * SLICE, 600 * SLICE, 600 * SLICE];
    let mut p = packetizer(20, 2, None);
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 3200 * SLICE);
    result.unwrap();
    assert!(sent.fec_limited);
    assert_identical(&reassemble(&sent.packets, None).0, &payload);
}

#[test]
fn streamed_frames_of_every_shape_reassemble_to_the_identical_frame() {
    let mut seed = 12345u64;
    let mut random = move |range: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize % range
    };
    let mut early_seen = 0;
    for case in 0..300 {
        let key = (case % 3 == 0).then_some([case as u8; 16]);
        let blocks = [2, 3, 4][random(3)];
        let fec = [0, 10, 20, 20, 50][random(5)];
        let min_fec = [0, 2, 6][random(3)];
        let size = match random(4) {
            0 => 1 + random(5000),
            1 => 1 + random(60_000),
            2 => 100_000 + random(120_000),
            _ => 300_000 + random(900_000),
        };
        let slices = blocks * (1 + random(2));
        let units = 1 + random(slices + 2);
        let payload = frame_bytes(size);
        let parts = cuts(size, units, &mut random);
        let cap = size + random(120_000);
        let mut p = packetizer(fec, min_fec, key);
        if !p.streamable(blocks, cap) {
            continue;
        }
        let first_index = p.frame;
        let (sent, result) = run(&mut p, &payload, &parts, blocks, slices, cap);
        result.unwrap();
        assert!(sent.done);
        early_seen += sent.early;
        let (stream, layout) = reassemble(&sent.packets, key);
        assert_identical(&stream, &payload);
        let seen: Vec<_> = sent.blocks.iter().map(|b| (b.data, b.parity)).collect();
        assert_eq!(layout, seen, "case {case}");
        assert_recoverable(&sent.packets, key, &layout);
        assert!(sent.blocks.len() <= 4);
        // One frame index, taken once.
        assert_eq!(p.frame, first_index.wrapping_add(1));
        assert_eq!(p.sequence as usize, 100 + sent.packets.len());
        if key.is_some() {
            assert_eq!(p.iv_counter as usize, 5 + sent.packets.len());
        }
        // A block leaves only once its unit has arrived.
        for (k, unit) in sent.unit_of.iter().enumerate().take(sent.early) {
            assert!(*unit >= ((k + 1) * slices).div_ceil(blocks), "case {case}");
        }
        // With the frame bounded by its cap, no early block starves a later one.
        assert!(sent.blocks.iter().all(|b| b.data >= 1));
        assert!(
            sent.packets
                .iter()
                .all(|packet| packet.len() == SLICE + 32 + if key.is_some() { 32 } else { 0 })
        );
    }
    assert!(early_seen > 100, "{early_seen} early blocks");
}

#[test]
fn a_unit_past_the_cap_fails_the_frame_and_keeps_its_progress() {
    let payload = frame_bytes(100 * SLICE);
    // After block 0 left: the index and packets stay consumed.
    let mut p = packetizer(20, 2, Some([3; 16]));
    let parts = [26 * SLICE, 30 * SLICE, 44 * SLICE];
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 70 * SLICE);
    let error = result.unwrap_err();
    assert_eq!(
        error.downcast_ref::<SliceOverflow>(),
        Some(&SliceOverflow {
            cap: 70 * SLICE,
            bytes: 100 * SLICE
        })
    );
    assert!(!sent.done);
    assert!(!sent.blocks.is_empty());
    assert_eq!(p.frame, 8, "block 0 took the frame index");
    assert_eq!(p.sequence as usize, 100 + sent.packets.len());
    assert_eq!(p.iv_counter as usize, 5 + sent.packets.len());
    // Before any block left: the index is still consumed, nothing was built.
    let mut p = packetizer(20, 2, Some([3; 16]));
    let parts = [3 * SLICE, 97 * SLICE];
    let (sent, result) = run(&mut p, &payload, &parts, 4, 4, 50 * SLICE);
    assert!(
        result
            .unwrap_err()
            .downcast_ref::<SliceOverflow>()
            .is_some()
    );
    assert!(sent.packets.is_empty());
    assert_eq!((p.frame, p.sequence, p.iv_counter), (8, 100, 5));
    // The next frame carries on from there.
    let mut next = p.streamed(9100, 500, 2, 2, 100 * SLICE).unwrap();
    let mut shards = PacketBlock::new();
    let mut got = Vec::new();
    next.push(
        &mut p,
        &payload[..60 * SLICE],
        false,
        &mut shards,
        |packets, _| {
            got.extend(packets.iter().cloned());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(&got[0][12..16], &8u32.to_le_bytes());
    assert_eq!(p.frame, 9);
}

#[test]
fn an_abandoned_streamed_frame_builds_nothing_more() {
    let payload = frame_bytes(100 * SLICE);
    let mut p = packetizer(20, 2, None);
    let mut frame = p.streamed(9000, 500, 4, 4, 200 * SLICE).unwrap();
    let mut shards = PacketBlock::new();
    // The sender fails on the first block.
    let error = frame
        .push(
            &mut p,
            &payload[..60 * SLICE],
            false,
            &mut shards,
            |_, _| bail!("socket gone"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("socket gone"));
    let sequence = p.sequence;
    assert!(
        frame
            .push(
                &mut p,
                &payload[60 * SLICE..],
                true,
                &mut shards,
                |_, _| Ok(())
            )
            .is_err()
    );
    assert_eq!((p.sequence, p.frame), (sequence, 8));
    // Another frame packetized in between is an error, not a corrupt stream.
    let mut frame = p.streamed(9000, 500, 4, 4, 200 * SLICE).unwrap();
    p.encode(&[1; 100], false, 0, 0).unwrap();
    assert!(
        frame
            .push(&mut p, &payload, true, &mut shards, |_, _| Ok(()))
            .is_err()
    );
    // A finished frame takes no more units.
    let mut frame = p.streamed(9000, 500, 4, 4, 200 * SLICE).unwrap();
    frame
        .push(&mut p, &payload, true, &mut shards, |_, _| Ok(()))
        .unwrap();
    assert!(frame.is_done());
    assert!(
        frame
            .push(&mut p, &payload, true, &mut shards, |_, _| Ok(()))
            .is_err()
    );
}
