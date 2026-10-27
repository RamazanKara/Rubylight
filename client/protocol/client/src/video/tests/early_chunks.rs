//! Early chunks: a PyroWave records frame handed to the sink packet by packet while it is
//! still arriving. The frames and signals must not change with the option on, and the chunks
//! since the last `first` must add up to the frame's envelope, with losses, FEC recovery and
//! reordering in the way.

use super::*;

/// A multi-block records host: every block is its own FEC block of full-size packets.
pub(crate) struct Blocks {
    seq: u16,
    spi: u32,
    frame: u32,
}

impl Blocks {
    pub(crate) fn new() -> Self {
        Self {
            seq: 500,
            spi: 0,
            frame: 1,
        }
    }

    /// One frame as `counts.len()` FEC blocks of that many data packets with 25% parity;
    /// returns the datagrams of each block, parity last.
    pub(crate) fn frame(&mut self, counts: &[usize], salt: u8) -> Vec<Vec<Vec<u8>>> {
        let last_block = counts.len() - 1;
        let mut blocks = Vec::new();
        for (block, &count) in counts.iter().enumerate() {
            let parity = (count * 25).div_ceil(100);
            let mut shards: Vec<Vec<u8>> = (0..count + parity)
                .map(|i| {
                    let mut s = vec![0u8; PACKET_SIZE + 16];
                    if i < count {
                        s[16..20].copy_from_slice(&((self.spi + i as u32) << 8).to_le_bytes());
                        s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
                        s[26] = 0x10;
                        // A record starts in every third packet.
                        if i % 3 == 0 {
                            s[25] = 0x80;
                        }
                        for (n, b) in s[32..].iter_mut().enumerate() {
                            *b = (n as u8)
                                .wrapping_mul(31)
                                .wrapping_add(i as u8)
                                .wrapping_add(salt)
                                | 1;
                        }
                    }
                    s
                })
                .collect();
            let rs = ReedSolomon::new(count, parity).unwrap();
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, PACKET_SIZE + 16).unwrap();
            // Like the host: RTP and FEC fields are written after parity.
            for (i, s) in shards.iter_mut().enumerate() {
                s[0] = 0x90;
                s[2..4].copy_from_slice(&self.seq.wrapping_add(i as u16).to_be_bytes());
                s[4..8].copy_from_slice(&(self.frame * 1500).to_be_bytes());
                s[20..24].copy_from_slice(&self.frame.to_le_bytes());
                s[27] = ((block as u8) << 4) | ((last_block as u8) << 6);
                let info = ((i as u32) << 12) | ((count as u32) << 22) | (25 << 4);
                s[28..32].copy_from_slice(&info.to_le_bytes());
            }
            self.seq = self.seq.wrapping_add((count + parity) as u16);
            self.spi += count as u32;
            blocks.push(shards);
        }
        self.frame += 1;
        blocks
    }
}

pub(crate) fn records(early: bool) -> VideoReceiver {
    let mut cfg = config(0x01_0000);
    cfg.pyrowave_records = true;
    cfg.early_chunks = early;
    VideoReceiver::new(cfg)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn chunk_count(events: &[VideoEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, VideoEvent::Chunk { .. }))
        .count()
}

/// The frames and signals as text, the chunks left out.
fn without_chunks(events: &[VideoEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            VideoEvent::Frame(f) => Some(format!(
                "frame {} {:?} {} {} {} {} {:?}",
                f.number,
                f.frame_type,
                f.receive_time_us,
                f.presentation_time_us,
                f.rtp_timestamp,
                f.host_processing_latency,
                f.data()
            )),
            VideoEvent::Signal(s) => Some(format!("{s:?}")),
            VideoEvent::Chunk { .. } => None,
        })
        .collect()
}

/// Checks that the chunks before each frame add up to its envelope.
fn assert_chunks_make_the_frames(events: &[VideoEvent]) {
    let mut envelope: Vec<u8> = Vec::new();
    let mut started = false;
    for event in events {
        match event {
            VideoEvent::Chunk {
                first,
                flags,
                payload,
            } => {
                if *first {
                    envelope = vec![0, 0, 0, 0];
                    started = true;
                }
                assert!(started, "a chunk before any first");
                envelope.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                envelope.extend_from_slice(&flags.to_le_bytes());
                envelope.extend_from_slice(payload);
            }
            VideoEvent::Frame(frame) => {
                assert_eq!(envelope, frame.data(), "frame {}", frame.number);
                started = false;
            }
            VideoEvent::Signal(_) => {}
        }
    }
}

#[test]
fn chunks_are_the_frame_and_the_option_changes_nothing() {
    let mut host = Blocks::new();
    let mut plain = records(false);
    let mut early = records(true);
    let mut plain_events = Vec::new();
    let mut early_events = Vec::new();
    for n in 0..4u8 {
        for packet in host.frame(&[20, 14, 9], n).iter().flatten() {
            plain.receive(packet, 7, &mut plain_events);
            early.receive(packet, 7, &mut early_events);
        }
    }
    // Each frame completes at its last block.
    assert_eq!(frames(&plain_events).len(), 4);
    assert_eq!(without_chunks(&early_events), without_chunks(&plain_events));
    assert_eq!(chunk_count(&plain_events), 0);
    assert_chunks_make_the_frames(&early_events);
    let firsts = early_events
        .iter()
        .filter(|e| matches!(e, VideoEvent::Chunk { first: true, .. }))
        .count();
    assert_eq!(firsts, 4);
}

#[test]
fn a_chunk_leaves_with_its_packet_not_with_its_block() {
    let mut host = Blocks::new();
    let mut rx = records(true);
    let mut events = Vec::new();
    let blocks = host.frame(&[20, 14], 1);
    for (i, packet) in blocks[0].iter().take(5).enumerate() {
        rx.receive(packet, 7, &mut events);
        assert_eq!(chunk_count(&events), i + 1);
    }
    assert!(frames(&events).is_empty());
}

#[test]
fn a_hole_holds_the_chunks_until_parity_fills_it() {
    let mut host = Blocks::new();
    let mut rx = records(true);
    let mut events = Vec::new();
    let blocks = host.frame(&[20], 3);
    let packets = &blocks[0];
    for (i, packet) in packets.iter().enumerate().take(20) {
        if i != 4 {
            rx.receive(packet, 7, &mut events);
        }
    }
    // Packets 0..=3 are in; 5..=19 wait behind the hole.
    assert_eq!(chunk_count(&events), 4);
    assert!(frames(&events).is_empty());
    rx.receive(&packets[20], 7, &mut events);
    // A parity packet recovers the hole and the block completes.
    assert_eq!(chunk_count(&events), 20);
    assert_eq!(frames(&events).len(), 1);
    assert_chunks_make_the_frames(&events);
    assert_eq!(rx.stats().packets_recovered, 1);
}

#[test]
fn losses_and_reordering_never_differ_from_the_plain_path() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..60 {
        let mut host = Blocks::new();
        let mut stream: Vec<Vec<u8>> = Vec::new();
        for n in 0..6u8 {
            let counts: Vec<usize> = (0..1 + rng.below(4))
                .map(|_| 4 + rng.below(30) as usize)
                .collect();
            stream.extend(host.frame(&counts, n).into_iter().flatten());
        }
        let loss = rng.below(12);
        let mut delivered: Vec<Vec<u8>> = Vec::new();
        for packet in stream {
            if rng.below(100) >= loss {
                delivered.push(packet);
            }
        }
        // Local reordering of neighbours.
        for i in 0..delivered.len().saturating_sub(1) {
            if rng.below(100) < 5 {
                delivered.swap(i, i + 1);
            }
        }
        let mut plain = records(false);
        let mut early = records(true);
        let mut plain_events = Vec::new();
        let mut early_events = Vec::new();
        for packet in &delivered {
            plain.receive(packet, 7, &mut plain_events);
            early.receive(packet, 7, &mut early_events);
        }
        assert_eq!(
            without_chunks(&early_events),
            without_chunks(&plain_events),
            "round {round}"
        );
        assert_eq!(early.stats(), plain.stats(), "round {round}");
        assert_chunks_make_the_frames(&early_events);
    }
}

#[test]
fn a_packet_behind_a_hole_waits_and_the_filled_hole_releases_the_run() {
    let mut host = Blocks::new();
    let mut rx = records(true);
    let mut events = Vec::new();
    let blocks = host.frame(&[20], 5);
    let packets = &blocks[0];
    // Arrival order and the chunks out after each: only the packet that extends the run from
    // the block's start moves it on, and a filled hole releases everything queued behind it.
    let order = [0, 3, 2, 1, 4, 6, 5];
    let chunks = [1, 1, 1, 4, 5, 5, 7];
    for (&index, &expected) in order.iter().zip(&chunks) {
        rx.receive(&packets[index], 7, &mut events);
        assert_eq!(chunk_count(&events), expected, "after packet {index}");
    }
    // The rest in order completes the block.
    for packet in &packets[7..20] {
        rx.receive(packet, 7, &mut events);
    }
    assert_eq!(chunk_count(&events), 20);
    assert_eq!(frames(&events).len(), 1);
    assert_chunks_make_the_frames(&events);
}

#[test]
fn duplicates_never_differ_from_the_plain_path() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for round in 0..40 {
        let mut host = Blocks::new();
        let mut delivered: Vec<Vec<u8>> = Vec::new();
        for n in 0..5u8 {
            let counts: Vec<usize> = (0..1 + rng.below(3))
                .map(|_| 4 + rng.below(25) as usize)
                .collect();
            for packet in host.frame(&counts, n).into_iter().flatten() {
                // Some lost, some repeated right away or a few packets later.
                if rng.below(100) < 4 {
                    continue;
                }
                delivered.push(packet.clone());
                if rng.below(100) < 15 {
                    delivered.push(packet);
                }
            }
        }
        for i in 0..delivered.len().saturating_sub(4) {
            if rng.below(100) < 6 {
                let copy = delivered[i].clone();
                delivered.insert(i + 1 + rng.below(3) as usize, copy);
            }
        }
        let mut plain = records(false);
        let mut early = records(true);
        let mut plain_events = Vec::new();
        let mut early_events = Vec::new();
        for packet in &delivered {
            plain.receive(packet, 7, &mut plain_events);
            early.receive(packet, 7, &mut early_events);
        }
        assert_eq!(
            without_chunks(&early_events),
            without_chunks(&plain_events),
            "round {round}"
        );
        assert_eq!(early.stats(), plain.stats(), "round {round}");
        assert_chunks_make_the_frames(&early_events);
    }
}

/// The android engine hands `host_processing_latency` to Nightjar's "Host processing" line, which
/// must be the video frame header's value in 100 us units, like Artemis and moonlight-common-c read
/// it. A host timing report (control message 0x5536) never reaches this field.
#[test]
fn the_frame_carries_the_header_host_processing_in_100_us_units() {
    let mut host = Blocks::new();
    let mut rx = records(false);
    let mut events = Vec::new();
    let mut blocks = host.frame(&[6], 3);
    // The first packet of a frame starts with [flag byte, latency low, latency high]: 0.5 ms.
    blocks[0][0][33] = 5;
    blocks[0][0][34] = 0;
    for packet in blocks.iter().flatten() {
        rx.receive(packet, 7, &mut events);
    }
    let frames = frames(&events);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].host_processing_latency, 5);
}
