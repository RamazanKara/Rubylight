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
