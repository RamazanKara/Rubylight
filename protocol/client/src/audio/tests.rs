use super::*;
use crate::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};

/// Data packets plus two parity packets after every fourth, as GFE and Sunshine send them.
fn stream(count: u16, size: usize) -> Vec<Vec<u8>> {
    let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
    let mut out = Vec::new();
    let mut block: Vec<Vec<u8>> = Vec::new();
    for seq in 0..count {
        let payload: Vec<u8> = (0..size).map(|i| (i as u16 ^ seq) as u8).collect();
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
