//! H.264 frames of several FEC blocks: each block that completes before the frame's last goes
//! to the depacketizer at once. Frames, signals and statistics must match processing every
//! block at the frame's last, with losses, reordering and recovery changes in the way.

use super::*;

/// A multi-block H.264 host: the frame's packets split as evenly as possible into blocks, each
/// with its own parity; every block starts with SOF and ends with EOF, as Sunshine sends them.
struct Blocks {
    seq: u16,
    spi: u32,
    frame: u32,
    fec_percent: usize,
}

impl Blocks {
    fn new() -> Self {
        Self {
            // Across the signed-16 midpoint; the queue rejects a first packet before 0.
            seq: 0x7FF0,
            spi: 0,
            frame: 1,
            fec_percent: 20,
        }
    }

    /// The datagrams of each block, parity last.
    fn frame(&mut self, payload: &[u8], kind: u8, blocks: usize) -> Vec<Vec<Vec<u8>>> {
        let slice = PACKET_SIZE - 16;
        let mut data = vec![1, 0, 0, kind];
        let total = payload.len() + 8;
        let last = if total % slice == 0 {
            slice
        } else {
            total % slice
        };
        data.extend_from_slice(&(last as u16).to_le_bytes());
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(payload);
        let count = total.div_ceil(slice);
        assert!(count >= blocks, "{count} packets for {blocks} blocks");
        let mut out = Vec::new();
        let mut first = 0;
        for block in 0..blocks {
            let n = count / blocks + usize::from(block < count % blocks);
            let parity = (n * self.fec_percent).div_ceil(100);
            let mut shards: Vec<Vec<u8>> = (0..n + parity)
                .map(|i| {
                    let mut s = vec![0u8; PACKET_SIZE + 16];
                    if i < n {
                        let p = first + i;
                        s[16..20].copy_from_slice(&((self.spi + p as u32) << 8).to_le_bytes());
                        s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == n - 1 { 2 } else { 0 };
                        s[26] = 0x10;
                        let chunk =
                            &data[(p * slice).min(data.len())..((p + 1) * slice).min(data.len())];
                        s[32..32 + chunk.len()].copy_from_slice(chunk);
                    }
                    s
                })
                .collect();
            if parity > 0 {
                let rs = ReedSolomon::new(n, parity).unwrap();
                let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
                rs.encode(&mut refs, PACKET_SIZE + 16).unwrap();
            }
            for (i, s) in shards.iter_mut().enumerate() {
                s[0] = 0x90;
                s[2..4].copy_from_slice(&self.seq.wrapping_add(i as u16).to_be_bytes());
                s[4..8].copy_from_slice(&(self.frame * 1500).to_be_bytes());
                s[20..24].copy_from_slice(&self.frame.to_le_bytes());
                s[27] = ((block as u8) << 4) | (((blocks - 1) as u8) << 6);
                let info =
                    ((i as u32) << 12) | ((n as u32) << 22) | ((self.fec_percent as u32) << 4);
                s[28..32].copy_from_slice(&info.to_le_bytes());
            }
            self.seq = self.seq.wrapping_add((n + parity) as u16);
            first += n;
            out.push(shards);
        }
        self.spi += count as u32;
        self.frame += 1;
        out
    }
}

fn idr(len: usize) -> Vec<u8> {
    let mut f = vec![
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80, 0, 0, 0, 1, 0x65,
    ];
    f.extend((0..len as u32).map(|i| (i % 251) as u8 | 1));
    f
}

/// What a receiver does between packets.
#[derive(Clone)]
enum Step {
    Packet(Vec<u8>),
    Decoded(FrameType),
    Refresh,
    IdrOnly,
}

/// The events and statistics of a receiver given `steps`, with blocks processed as they
/// complete (`ahead`) or all at the frame's last block.
fn run(rfi: bool, ahead: bool, steps: &[Step]) -> (Vec<VideoEvent>, VideoStats) {
    let mut rx = VideoReceiver::new(VideoConfig { rfi, ..config(0x1) });
    rx.queue.set_process_ahead(ahead);
    let mut events = Vec::new();
    for step in steps {
        match step {
            Step::Packet(packet) => rx.receive(packet, 9, &mut events),
            Step::Decoded(kind) => rx.frame_decoded(*kind),
            Step::Refresh => rx.request_decoder_refresh(&mut events),
            Step::IdrOnly => rx.recover_with_idr_only(),
        }
    }
    (events, rx.stats())
}

/// Runs `steps` both ways and checks they agree; returns the events.
fn same_both_ways(rfi: bool, steps: &[Step]) -> Vec<VideoEvent> {
    let (plain, plain_stats) = run(rfi, false, steps);
    let (ahead, ahead_stats) = run(rfi, true, steps);
    assert_eq!(format!("{ahead:?}"), format!("{plain:?}"));
    assert_eq!(ahead_stats, plain_stats);
    ahead
}

fn packets(blocks: Vec<Vec<Vec<u8>>>) -> impl Iterator<Item = Step> {
    blocks.into_iter().flatten().map(Step::Packet)
}

fn signals(events: &[VideoEvent]) -> Vec<&VideoSignal> {
    events
        .iter()
        .filter_map(|e| match e {
            VideoEvent::Signal(
                s @ (VideoSignal::RequestIdr | VideoSignal::InvalidateFrames { .. }),
            ) => Some(s),
            _ => None,
        })
        .collect()
}

#[test]
fn lossless_frames_of_two_to_four_blocks_match_processing_at_the_last_block() {
    for blocks in 2..=4 {
        let mut host = Blocks::new();
        let key = idr(9000);
        let p: Vec<_> = (0..4).map(|n| h264_p(4000 + 1500 * n)).collect();
        let mut steps: Vec<Step> = packets(host.frame(&key, 2, blocks)).collect();
        steps.push(Step::Decoded(FrameType::Idr));
        for frame in &p {
            steps.extend(packets(host.frame(frame, 1, blocks)));
        }
        let events = same_both_ways(false, &steps);
        let frames = frames(&events);
        assert_eq!(frames.len(), 5, "{blocks} blocks");
        assert_eq!(frames[0].frame_type, FrameType::Idr);
        assert_eq!(&frames[0].data()[..key.len()], &key[..]);
        for (frame, payload) in frames[1..].iter().zip(&p) {
            assert_eq!(&frame.data()[..payload.len()], &payload[..]);
        }
        assert!(signals(&events).is_empty());
    }
}

/// The blocks before the last are processed before the last block arrives.
#[test]
fn a_finished_block_is_processed_before_the_frame_completes() {
    let mut host = Blocks::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    let blocks = host.frame(&idr(9000), 2, 3);
    let first = blocks[0].len();
    for packet in blocks.iter().flatten().take(first) {
        rx.receive(packet, 1, &mut events);
    }
    // Three data packets and a parity packet.
    assert_eq!(first, 4);
    assert_eq!(rx.depacketizer.ahead(), 3);
    assert!(frames(&events).is_empty());
    for packet in blocks.iter().flatten().skip(first) {
        rx.receive(packet, 1, &mut events);
    }
    assert_eq!(rx.depacketizer.ahead(), 0);
    assert_eq!(frames(&events).len(), 1);
}

/// A repeated key-frame-only request, as the decoder thread makes after every frame, keeps
/// the blocks already processed ahead; the first one, which changes recovery, takes them back.
#[test]
fn a_repeated_key_frame_only_request_keeps_the_blocks_processed_ahead() {
    for rfi in [false, true] {
        let mut host = Blocks::new();
        let mut rx = VideoReceiver::new(VideoConfig { rfi, ..config(0x1) });
        let mut events = Vec::new();
        let blocks = host.frame(&idr(9000), 2, 3);
        let first = blocks[0].len();
        for packet in blocks.iter().flatten().take(first) {
            rx.receive(packet, 1, &mut events);
        }
        assert_eq!(rx.depacketizer.ahead(), 3, "rfi {rfi}");
        rx.recover_with_idr_only();
        assert_eq!(rx.depacketizer.ahead(), if rfi { 0 } else { 3 }, "rfi {rfi}");
        rx.recover_with_idr_only();
        for packet in blocks.iter().flatten().skip(first) {
            rx.receive(packet, 1, &mut events);
        }
        assert_eq!(frames(&events).len(), 1, "rfi {rfi}");
    }
}

#[test]
fn an_unrecoverable_later_block_loses_its_frame_and_the_next_arrives_intact() {
    for rfi in [false, true] {
        for lost_block in 1..3 {
            let mut host = Blocks::new();
            let mut steps: Vec<Step> = packets(host.frame(&idr(6000), 2, 3)).collect();
            steps.push(Step::Decoded(FrameType::Idr));
            steps.extend(packets(host.frame(&h264_p(6000), 1, 3)));
            // Two data packets of one block, past its single parity packet.
            let mut lost = host.frame(&h264_p(9000), 1, 3);
            lost[lost_block].drain(..2);
            steps.extend(packets(lost));
            // With RFI the host answers the invalidation with a recovery frame, else a key
            // frame is the recovery.
            let recovery = h264_p(7000);
            if rfi {
                steps.extend(packets(host.frame(&recovery, 4, 3)));
            } else {
                steps.extend(packets(host.frame(&idr(6000), 2, 3)));
            }
            let next = h264_p(5000);
            steps.extend(packets(host.frame(&next, 1, 3)));

            let events = same_both_ways(rfi, &steps);
            let frames = frames(&events);
            let numbers: Vec<_> = frames.iter().map(|f| f.number).collect();
            assert_eq!(numbers, [1, 2, 4, 5], "rfi {rfi}, block {lost_block}");
            if rfi {
                assert_eq!(&frames[2].data()[..recovery.len()], &recovery[..]);
                assert_eq!(
                    signals(&events),
                    [&VideoSignal::InvalidateFrames { first: 3, last: 3 }]
                );
            } else {
                assert_eq!(frames[2].frame_type, FrameType::Idr);
            }
            assert_eq!(&frames[3].data()[..next.len()], &next[..]);
        }
    }
}

/// A key frame lost after its first block was processed leaves the same recovery as before.
#[test]
fn a_lost_key_frame_after_its_first_block_recovers_as_before() {
    for rfi in [false, true] {
        let mut host = Blocks::new();
        let mut steps: Vec<Step> = packets(host.frame(&idr(6000), 2, 3)).collect();
        steps.push(Step::Decoded(FrameType::Idr));
        let mut lost = host.frame(&idr(9000), 2, 3);
        lost[2].drain(..2);
        steps.extend(packets(lost));
        for _ in 0..3 {
            steps.extend(packets(host.frame(&h264_p(5000), 1, 3)));
        }
        steps.extend(packets(host.frame(&idr(6000), 2, 3)));
        let events = same_both_ways(rfi, &steps);
        assert!(!signals(&events).is_empty(), "rfi {rfi}");
        assert_eq!(frames(&events).last().unwrap().frame_type, FrameType::Idr);
    }
}

#[test]
fn reordering_across_a_block_boundary_matches() {
    let mut host = Blocks::new();
    let mut steps: Vec<Step> = packets(host.frame(&idr(6000), 2, 2)).collect();
    steps.push(Step::Decoded(FrameType::Idr));
    // Block 0's parity after block 1 began: block 0 completed from its data already.
    let mut late_parity = host.frame(&h264_p(9000), 1, 3);
    let parity = late_parity[0].pop().unwrap();
    late_parity[1].insert(1, parity);
    late_parity[1].swap(2, 3);
    steps.extend(packets(late_parity));
    // A data packet of block 0 recovered from parity, the block's packets reversed.
    let mut recovered = host.frame(&h264_p(9000), 1, 3);
    recovered[0].remove(1);
    recovered[0].reverse();
    steps.extend(packets(recovered));
    // Block 0's last data packet and its parity after block 1 began: the frame is lost.
    let mut late_data = host.frame(&h264_p(9000), 1, 2);
    let n = late_data[0].len() - 2;
    let tail: Vec<_> = late_data[0].drain(n..).collect();
    late_data[1].splice(1..1, tail);
    steps.extend(packets(late_data));
    let next = h264_p(9000);
    steps.extend(packets(host.frame(&next, 1, 3)));

    let events = same_both_ways(true, &steps);
    let numbers: Vec<_> = frames(&events).iter().map(|f| f.number).collect();
    assert_eq!(numbers[..3], [1, 2, 3]);
    assert!(signals(&events)
        .iter()
        .any(|s| matches!(s, VideoSignal::InvalidateFrames { .. })));
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

#[test]
fn losses_reordering_and_recovery_changes_never_differ() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for round in 0..200 {
        let rfi = rng.below(2) == 0;
        let mut host = Blocks::new();
        host.fec_percent = [0, 10, 20, 50][rng.below(4) as usize];
        let mut stream: Vec<Vec<u8>> = Vec::new();
        for n in 0..10 {
            let blocks = 1 + rng.below(4) as usize;
            let len = blocks * 1100 + rng.below(9000) as usize;
            let frame = match rng.below(6) {
                0 => host.frame(&idr(len), 2, blocks),
                1 => host.frame(&h264_p(len), 4, blocks),
                _ => host.frame(&h264_p(len + n), 1, blocks),
            };
            stream.extend(frame.into_iter().flatten());
        }
        let loss = rng.below(10);
        let mut delivered: Vec<Vec<u8>> = stream
            .into_iter()
            .filter(|_| rng.below(100) >= loss)
            .collect();
        for i in 0..delivered.len().saturating_sub(1) {
            if rng.below(100) < 4 {
                delivered.swap(i, i + 1);
            }
        }
        let mut steps = Vec::new();
        for packet in delivered {
            match rng.below(200) {
                0 => steps.push(Step::Refresh),
                1 => steps.push(Step::IdrOnly),
                2..=5 => steps.push(Step::Decoded(FrameType::Idr)),
                _ => {}
            }
            steps.push(Step::Packet(packet));
        }
        let (plain, plain_stats) = run(rfi, false, &steps);
        let (ahead, ahead_stats) = run(rfi, true, &steps);
        assert_eq!(format!("{ahead:?}"), format!("{plain:?}"), "round {round}");
        assert_eq!(ahead_stats, plain_stats, "round {round}");
    }
}

/// Late parity of a finished frame is dropped before a packet buffer is taken: the output and
/// statistics match the stream without it, bar the datagrams counted as received, and the pool
/// is left as it was.
#[test]
fn late_parity_of_a_finished_frame_takes_no_pool_buffer() {
    let mut host = Blocks::new();
    let key = idr(9000);
    let p = h264_p(4000);
    let first = host.frame(&key, 2, 2);
    let second = host.frame(&p, 1, 2);
    // Each block's last datagram is its parity.
    let late: Vec<Vec<u8>> = first
        .iter()
        .map(|block| block.last().unwrap().clone())
        .collect();

    let feed = |with_late: bool| {
        let mut rx = VideoReceiver::new(config(0x1));
        let mut events = Vec::new();
        for datagram in first.iter().flatten() {
            rx.receive(datagram, 9, &mut events);
        }
        rx.frame_decoded(FrameType::Idr);
        for datagram in second.iter().flatten() {
            rx.receive(datagram, 9, &mut events);
        }
        let pool_before = rx.pools.packets.clone();
        if with_late {
            for datagram in &late {
                rx.receive(datagram, 9, &mut events);
            }
        }
        (events, rx.stats(), pool_before, rx.pools.packets.clone())
    };

    let (plain, plain_stats, _, _) = feed(false);
    let (with_late, mut late_stats, pool_before, pool_after) = feed(true);
    assert!(
        !pool_before.is_empty(),
        "the pool needs buffers for this check to mean anything"
    );
    assert_eq!(pool_after, pool_before, "late parity touched a pool buffer");
    assert_eq!(format!("{with_late:?}"), format!("{plain:?}"));
    late_stats.packets_received -= late.len() as u64;
    assert_eq!(late_stats, plain_stats);
}
