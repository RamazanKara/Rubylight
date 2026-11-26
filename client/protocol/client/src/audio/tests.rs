use super::*;
use crate::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};

/// Data packets plus two parity packets after every fourth, as GFE and Sunshine send them.
fn stream(count: u16, size: usize) -> Vec<Vec<u8>> {
    stream_of(
        (0..count)
            .map(|seq| (0..size).map(|i| (i as u16 ^ seq) as u8).collect())
            .collect(),
    )
}

/// [`stream`] of the given payloads, which must all have the same size.
fn stream_of(payloads: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
    let mut out = Vec::new();
    let mut block: Vec<Vec<u8>> = Vec::new();
    for (seq, payload) in payloads.into_iter().enumerate() {
        let seq = seq as u16;
        let size = payload.len();
        let mut p = vec![0x80, 97];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&(u32::from(seq) * 5).to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(&payload);
        out.push(p);
        block.push(payload);
        if block.len() == 4 {
            let mut shards = block.clone();
            shards.push(vec![0; size]);
            shards.push(vec![0; size]);
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, size).unwrap();
            for (index, shard) in shards[4..].iter().enumerate() {
                let mut p = vec![0x80, 127];
                p.extend_from_slice(&(seq + 1 + index as u16).to_be_bytes());
                p.extend_from_slice(&[0; 8]);
                p.push(index as u8);
                p.push(97);
                p.extend_from_slice(&(seq - 3).to_be_bytes());
                p.extend_from_slice(&(u32::from(seq - 3) * 5).to_be_bytes());
                p.extend_from_slice(&[0; 4]);
                p.extend_from_slice(shard);
                out.push(p);
            }
            block.clear();
        }
    }
    out
}

fn config() -> AudioConfig {
    AudioConfig {
        packet_duration_ms: 5,
        app_version: [7, 1, 431, -1],
        encryption: None,
    }
}

fn run(rx: &mut AudioReceiver, packets: &[Vec<u8>]) -> Vec<Option<Vec<u8>>> {
    let mut played = Vec::new();
    for packet in packets {
        rx.receive(packet, 0, &mut |out| {
            played.push(match out {
                AudioOutput::Packet(p) => Some(p.to_vec()),
                AudioOutput::Lost => None,
            })
        });
    }
    played
}

#[test]
fn drops_backlog_then_plays_in_order() {
    let packets = stream(200, 60);
    let mut rx = AudioReceiver::new(config());
    let played = run(&mut rx, &packets);
    // 100 data packets of backlog; their block's parity synchronizes the queue on seq 100.
    assert_eq!(played.len(), 100);
    assert!(played.iter().all(Option::is_some));
    assert_eq!(played[0].as_deref(), Some(&packets[150][12..]));
}

#[test]
fn dolby_chunks_recovered_by_fec_come_out_as_whole_frames() {
    use rubylight_protocol::audio_ext::{chunks_for, ChunkHeader, CHUNK_BYTES, CHUNK_PAYLOAD_BYTES};
    // Eight 640 kbit/s AC3 frames of three chunks each: six FEC blocks.
    let frames: Vec<Vec<u8>> = (0..8u8)
        .map(|index| {
            let mut frame = vec![index + 1; 2560];
            frame[..6].copy_from_slice(&[0x0B, 0x77, 0, 0, 36, 8 << 3]);
            frame
        })
        .collect();
    let mut payloads = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let count = chunks_for(frame.len());
        for (part, piece) in frame.chunks(CHUNK_BYTES).enumerate() {
            let header = ChunkHeader {
                first: part == 0,
                last: part + 1 == count,
                frame_index: index as u16,
            };
            let mut payload = header.encode().to_vec();
            payload.extend_from_slice(piece);
            payload.resize(CHUNK_PAYLOAD_BYTES, 0);
            payloads.push(payload);
        }
    }
    let mut packets = stream_of(payloads);
    // Data packets 5 and 6 (block 1, frames 1 and 2) are lost and recovered. The first block
    // only synchronizes the queue, so frame 0 and the start of frame 1 never play.
    packets.retain(|p| ![5u16, 6].contains(&u16::from_be_bytes([p[2], p[3]])) || p[1] != 97);
    let mut rx = AudioReceiver::new(AudioConfig {
        packet_duration_ms: 32,
        ..config()
    });
    rx.assemble_frames();
    rx.packets_to_drop = 0;
    let played = run(&mut rx, &packets);
    assert_eq!(rx.stats().packet_count_fec_recovered, 2);
    assert_eq!(played, frames[2..].iter().cloned().map(Some).collect::<Vec<_>>());
}

/// Eight 640 kbit/s AC3 frames (2560 bytes each, the longest AC3 frame), distinct per frame.
fn ac3_640_frames() -> Vec<Vec<u8>> {
    (0..8u8)
        .map(|index| {
            let mut frame: Vec<u8> = (0..2560u32).map(|i| (i % 251) as u8 ^ index).collect();
            frame[..6].copy_from_slice(&[0x0B, 0x77, 0, 0, 36, 8 << 3]);
            frame
        })
        .collect()
}

#[test]
fn unchunked_dolby_frames_over_1400_bytes_arrive_whole_and_fec_repairs_them() {
    const KEY: [u8; 16] = *b"unchunked-dolby!";
    let iv = [0, 0, 0x12, 0x34, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9];
    let frames = ac3_640_frames();
    for encrypted in [false, true] {
        // One frame per packet, as such a host sends them: encrypted with the key id plus the
        // sequence number as IV, then FEC over what goes on the wire.
        let payloads = frames
            .iter()
            .enumerate()
            .map(|(seq, frame)| {
                if !encrypted {
                    return frame.clone();
                }
                let mut packet_iv = [0u8; 16];
                packet_iv[..4].copy_from_slice(&(0x1234 + seq as u32).to_be_bytes());
                crate::crypto::cbc_encrypt(&KEY, &packet_iv, frame, crate::crypto::CbcPadding::Pkcs7)
                    .unwrap()
            })
            .collect();
        let mut packets = stream_of(payloads);
        let longest = if encrypted { 12 + 2576 } else { 12 + 2560 };
        assert_eq!(packets[0].len(), longest);
        assert!(packets.iter().all(|p| p.len() > MAX_PACKET_SIZE));
        // Data packets 5 and 6 (block 1) are lost; their 2576-byte parity shards bring them back.
        packets.retain(|p| ![5u16, 6].contains(&u16::from_be_bytes([p[2], p[3]])) || p[1] != 97);
        let mut rx = AudioReceiver::new(AudioConfig {
            packet_duration_ms: 32,
            encryption: encrypted.then_some(AudioEncryption { key: KEY, iv }),
            ..config()
        });
        rx.unchunked_frames();
        assert_eq!(rx.max_packet_size(), MAX_UNCHUNKED_PACKET_SIZE);
        rx.packets_to_drop = 0;
        let played = run(&mut rx, &packets);
        let stats = rx.stats();
        assert_eq!((stats.packet_count_fec_recovered, stats.decrypt_failures), (2, 0));
        assert_eq!(stats.packet_count_invalid, 0);
        // The first block only synchronizes the queue.
        assert_eq!(played, frames[4..].iter().cloned().map(Some).collect::<Vec<_>>(), "{encrypted}");
    }
}

#[test]
fn unchunked_packets_that_are_not_frames_are_lost_and_counted() {
    let mut payloads = ac3_640_frames();
    payloads[5] = vec![0x55; 2560];
    let mut rx = AudioReceiver::new(AudioConfig {
        packet_duration_ms: 32,
        ..config()
    });
    rx.unchunked_frames();
    rx.packets_to_drop = 0;
    let played = run(&mut rx, &stream_of(payloads.clone()));
    assert_eq!(played.len(), 4);
    assert_eq!(played[1], None);
    assert_eq!(played[2].as_deref(), Some(&payloads[6][..]));
    assert_eq!(rx.stats().packet_count_invalid, 1);
}

#[test]
fn opus_and_chunked_streams_keep_the_1400_byte_read() {
    // A longer datagram is cut, as in the C core, and the socket reads no more.
    let mut opus = AudioReceiver::new(config());
    let mut chunked = AudioReceiver::new(config());
    chunked.assemble_frames();
    for rx in [&mut opus, &mut chunked] {
        assert_eq!(rx.max_packet_size(), MAX_PACKET_SIZE);
    }
    opus.packets_to_drop = 0;
    let packets = stream(8, 2000);
    let played = run(&mut opus, &packets);
    assert_eq!(played.len(), 4);
    assert_eq!(played[0].as_deref(), Some(&packets[6][12..MAX_PACKET_SIZE]));
}

/// The FEC failures counted after each data packet of `seqs`, given at its time in ms.
fn fec_failures_at(rx: &mut AudioReceiver, seqs: &[(u16, u64)]) -> Vec<u32> {
    let packets = stream(32, 60);
    let data = |seq: u16| {
        packets
            .iter()
            .find(|p| p[1] == 97 && u16::from_be_bytes([p[2], p[3]]) == seq)
            .unwrap()
    };
    seqs.iter()
        .map(|&(seq, ms)| {
            rx.receive(data(seq), ms * 1000, &mut |_| {});
            rx.stats().packet_count_fec_failed
        })
        .collect()
}

#[test]
fn a_chunked_stream_gives_up_a_block_from_when_the_next_started_arriving() {
    // Seq 0 synchronizes on block 1, which plays; old seq 1 then counts as out of sequence, so
    // the queue waits for block 2's lost seq 10 (its parity is lost too) once block 3 starts.
    let mut seqs = vec![(0, 0), (4, 1), (5, 1), (6, 1), (7, 1), (1, 1)];
    seqs.extend([(8, 10), (9, 10), (11, 10), (12, 42), (13, 50), (14, 53)]);
    let failed = |rx: &mut AudioReceiver| fec_failures_at(rx, &seqs)[6..].to_vec();
    let dolby = || {
        let mut rx = AudioReceiver::new(AudioConfig {
            packet_duration_ms: 32,
            ..config()
        });
        rx.packets_to_drop = 0;
        rx
    };
    // AC3 frames: block 3 started 32 ms after block 2, so 10 ms after that block 2 is lost,
    // not after four 32 ms frames and 10 ms (138 ms).
    let mut rx = dolby();
    rx.assemble_frames();
    assert_eq!(failed(&mut rx), [0, 0, 0, 0, 0, 1]);
    let mut unchunked = dolby();
    assert_eq!(failed(&mut unchunked), [0; 6]);
    // Opus keeps four packet durations and 10 ms: 30 ms after block 2 started.
    let mut opus = AudioReceiver::new(config());
    opus.packets_to_drop = 0;
    assert_eq!(failed(&mut opus), [0, 0, 0, 1, 1, 1]);
}

#[test]
fn recovers_two_lost_packets_per_block() {
    let packets = stream(200, 60);
    let mut rx = AudioReceiver::new(config());
    rx.receive_timeout();
    let mut skip = packets.clone();
    // Data packets 8 and 10 (block 2) are lost.
    skip.retain(|p| ![8u16, 10].contains(&u16::from_be_bytes([p[2], p[3]])) || p[1] != 97);
    let mut rx2 = AudioReceiver::new(config());
    rx2.packets_to_drop = 0;
    let played = run(&mut rx2, &skip);
    assert!(played.iter().all(Option::is_some));
    assert_eq!(rx2.stats().packet_count_fec_recovered, 2);
    assert_eq!(played[4].as_deref(), Some(&packets[(8 / 4) * 6][12..]));
}

#[test]
fn reordered_packets_drain_buffered_contiguous_packets_immediately() {
    let packets = stream(16, 40);
    let data = |seq: u16| {
        packets
            .iter()
            .find(|p| p[1] == 97 && u16::from_be_bytes([p[2], p[3]]) == seq)
            .unwrap()
    };
    let mut rx = AudioReceiver::new(config());
    rx.packets_to_drop = 0;

    // Synchronize on block 1 (seq 4..=7).
    let _ = run(&mut rx, &[data(0).clone()]);

    // Within a block: [5, 4] arrives -> 5 queues, then 4 triggers HandleNowAndDrain for [4, 5].
    assert!(run(&mut rx, &[data(5).clone()]).is_empty());
    let played = run(&mut rx, &[data(4).clone()]);
    assert_eq!(
        played,
        vec![Some(data(4)[12..].to_vec()), Some(data(5)[12..].to_vec())]
    );

    // Complete block 1 (4..=7) so the queue leaves synchronization, then mark the link as
    // reordering with a late packet from block 0 and test cross-block draining [12, 11] after 8..=10.
    assert_eq!(run(&mut rx, &[data(6).clone()]).len(), 1);
    assert_eq!(run(&mut rx, &[data(7).clone()]).len(), 1);
    assert!(run(&mut rx, &[data(3).clone()]).is_empty());
    assert_eq!(run(&mut rx, &[data(8).clone(), data(9).clone(), data(10).clone()]).len(), 3);
    assert!(run(&mut rx, &[data(12).clone()]).is_empty());
    let played = run(&mut rx, &[data(11).clone()]);
    assert_eq!(
        played,
        vec![Some(data(11)[12..].to_vec()), Some(data(12)[12..].to_vec())]
    );
}

#[test]
fn stray_malformed_fec_packets_do_not_latch_incompatible_server() {
    let packets = stream(20, 60);
    let fec = |seq_base: u16| {
        let mut p = vec![0x80, 127];
        p.extend_from_slice(&0x100u16.to_be_bytes());
        p.extend_from_slice(&[0; 8]);
        p.push(0);
        p.push(97);
        p.extend_from_slice(&seq_base.to_be_bytes());
        p.extend_from_slice(&[0; 8]);
        p.extend_from_slice(&[0; 60]);
        p
    };
    let mut rx = AudioReceiver::new(config());
    rx.packets_to_drop = 0;

    // A single malformed base_seq does not turn off FEC; a valid packet resets the streak.
    let _ = run(&mut rx, &[packets[0].clone()]);
    for _ in 0..(queue::INCOMPATIBLE_STREAK - 1) {
        let _ = run(&mut rx, &[fec(5)]);
    }
    // Block 1 (`seq 4..=7`) starts at `packets[6]` and resets the streak before block 2 (`seq 8..=11`).
    let _ = run(&mut rx, &packets[6..12]);
    // Another short burst stays below the streak threshold, so FEC still recovers block 2 (seq 8 & 10 lost).
    for _ in 0..(queue::INCOMPATIBLE_STREAK - 1) {
        let _ = run(&mut rx, &[fec(5)]);
    }
    let mut rest = packets[12..].to_vec();
    rest.retain(|p| ![8u16, 10].contains(&u16::from_be_bytes([p[2], p[3]])) || p[1] != 97);
    let played = run(&mut rx, &rest);
    assert!(played.iter().all(Option::is_some));
    assert_eq!(rx.stats().packet_count_fec_recovered, 2);

    // Reaching INCOMPATIBLE_STREAK in a row latches incompatible_server so audio passes straight through.
    let mut rx2 = AudioReceiver::new(config());
    rx2.packets_to_drop = 0;
    let _ = run(&mut rx2, &[packets[0].clone()]);
    for _ in 0..queue::INCOMPATIBLE_STREAK {
        let _ = run(&mut rx2, &[fec(5)]);
    }
    let single = run(&mut rx2, &[packets[7].clone()]);
    assert_eq!(single, vec![Some(packets[7][12..].to_vec())]);
}

#[test]
fn fec_blocks_stay_bounded_under_future_sequence_flood() {
    let packets = stream(20, 40);
    let mut rx = AudioReceiver::new(config());
    rx.packets_to_drop = 0;
    // Complete block 1 (`seq 4..=7`) so `synchronizing` clears, then send a late OOS packet (`seq 5`)
    // so `received_oos_data` waits for the reordered head instead of advancing past block 2 immediately.
    let _ = run(&mut rx, &packets[..1]);
    let _ = run(&mut rx, &packets[6..12]);
    let _ = run(&mut rx, &[packets[7].clone()]);

    let mut block2: Vec<_> = packets
        .iter()
        .filter(|p| p[1] == 97 && (8..=11).contains(&u16::from_be_bytes([p[2], p[3]])))
        .cloned()
        .collect();
    // Arrive out of order (`seq 9` before `seq 8`), keeping block 2 active at head.
    let _ = run(&mut rx, &[block2[1].clone()]);
    // Flood distinct future bases beyond MAX_FEC_BLOCKS within `wait_us`.
    for i in 1..(queue::MAX_FEC_BLOCKS + 32) {
        let seq = 12 + (i as u16) * 4;
        let mut p = vec![0x80, 97];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&(u32::from(seq) * 5).to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(&[7; 40]);
        let _ = run(&mut rx, &[p]);
    }
    // Active block 2 remained intact and drains in order once `seq 8, 10, 11` arrive.
    let mut rest_packets = vec![block2.remove(0)];
    rest_packets.extend(block2.into_iter().skip(1));
    let rest = run(&mut rx, &rest_packets);
    assert_eq!(rest.len(), 4);
    assert!(rest.iter().all(Option::is_some));
}
