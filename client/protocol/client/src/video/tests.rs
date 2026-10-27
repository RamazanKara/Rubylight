use super::*;
use crate::fec::ReedSolomon;

const PACKET_SIZE: usize = 1024;

/// A single-block host packetizer: 8-byte frame header, then the payload in
/// `PACKET_SIZE - 16` byte shards, with parity over the whole RTP packets.
struct Host {
    seq: u16,
    spi: u32,
    frame: u32,
    fec_percent: usize,
}

impl Host {
    fn new() -> Self {
        Self {
            seq: 100,
            spi: 0,
            frame: 1,
            fec_percent: 20,
        }
    }

    fn frame(&mut self, payload: &[u8], kind: u8) -> Vec<Vec<u8>> {
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
        let parity = (count * self.fec_percent).div_ceil(100);
        let mut shards: Vec<Vec<u8>> = (0..count + parity)
            .map(|i| {
                let mut s = vec![0u8; PACKET_SIZE + 16];
                if i < count {
                    s[16..20].copy_from_slice(&((self.spi + i as u32) << 8).to_le_bytes());
                    s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
                    s[26] = 0x10;
                    let chunk =
                        &data[(i * slice).min(data.len())..((i + 1) * slice).min(data.len())];
                    s[32..32 + chunk.len()].copy_from_slice(chunk);
                }
                s
            })
            .collect();
        if parity > 0 {
            let rs = ReedSolomon::new(count, parity).unwrap();
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, PACKET_SIZE + 16).unwrap();
        }
        // Like the host: RTP and FEC fields are written after parity.
        for (i, s) in shards.iter_mut().enumerate() {
            s[0] = 0x90;
            s[2..4].copy_from_slice(&self.seq.wrapping_add(i as u16).to_be_bytes());
            s[4..8].copy_from_slice(&(self.frame * 1500).to_be_bytes());
            s[20..24].copy_from_slice(&self.frame.to_le_bytes());
            let info =
                ((i as u32) << 12) | ((count as u32) << 22) | ((self.fec_percent as u32) << 4);
            s[28..32].copy_from_slice(&info.to_le_bytes());
        }
        self.seq = self.seq.wrapping_add((count + parity) as u16);
        self.spi += count as u32;
        self.frame += 1;
        shards
    }
}

pub(crate) fn config(format: u32) -> VideoConfig {
    VideoConfig {
        format,
        packet_size: PACKET_SIZE,
        app_version: [7, 1, 431, -1],
        rfi: false,
        pyrowave_records: false,
        early_chunks: false,
        colorspace: COLORSPACE_REC_709,
        encryption_key: None,
    }
}

fn h264_idr() -> Vec<u8> {
    let mut f = vec![
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80,
    ];
    f.extend_from_slice(&[0, 0, 0, 1, 0x65]);
    f.extend((0..3000u32).map(|i| (i % 251) as u8 | 1));
    f
}

fn h264_p(len: usize) -> Vec<u8> {
    let mut f = vec![0, 0, 0, 1, 0x41];
    f.extend((0..len as u32).map(|i| (i % 249) as u8 | 1));
    f
}

fn frames(events: &[VideoEvent]) -> Vec<&Frame> {
    events
        .iter()
        .filter_map(|e| match e {
            VideoEvent::Frame(f) => Some(f),
            _ => None,
        })
        .collect()
}

#[test]
fn h264_frames_in_order() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    let idr = h264_idr();
    let p = h264_p(5000);
    for packet in host.frame(&idr, 2).into_iter().chain(host.frame(&p, 1)) {
        rx.receive(&packet, 1_000_000, &mut events);
    }
    let frames = frames(&events);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].frame_type, FrameType::Idr);
    let kinds: Vec<_> = frames[0].buffers().iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds[..3],
        [BufferType::Sps, BufferType::Pps, BufferType::PicData]
    );
    assert!(kinds[3..].iter().all(|&k| k == BufferType::PicData));
    assert_eq!(&frames[0].data()[..idr.len()], &idr[..]);
    assert_eq!(frames[1].frame_type, FrameType::P);
    // H.264 keeps the zero padding of the last shard.
    assert_eq!(&frames[1].data()[..p.len()], &p[..]);
    assert!(frames[1].data()[p.len()..].iter().all(|&b| b == 0));
}

#[test]
fn recovers_lost_data_from_parity() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1000));
    let mut events = Vec::new();
    let payload: Vec<u8> = (0..9000u32).map(|i| (i * 7) as u8).collect();
    let packets = host.frame(&payload, 2);
    for (i, packet) in packets.iter().enumerate() {
        if i != 0 && i != 3 {
            rx.receive(packet, 5, &mut events);
        }
    }
    let frames = frames(&events);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].data(), &payload[..]);
    assert_eq!(rx.stats().packets_recovered, 2);
    assert!(events
        .iter()
        .any(|e| matches!(e, VideoEvent::Signal(VideoSignal::FecStatus(_)))));
}

fn fec_blocks(events: &[VideoEvent]) -> Vec<FecBlock> {
    events
        .iter()
        .filter_map(|e| match e {
            VideoEvent::Signal(VideoSignal::FecBlock(block)) => Some(*block),
            _ => None,
        })
        .collect()
}

#[test]
fn finished_blocks_are_counted_for_the_fec_report() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1000));
    let mut events = Vec::new();
    let payload: Vec<u8> = (0..9000u32).map(|i| (i * 7) as u8).collect();
    // Nine data shards and two parity: two lost and recovered.
    for (i, packet) in host.frame(&payload, 2).iter().enumerate() {
        if i != 0 && i != 3 {
            rx.receive(packet, 5, &mut events);
        }
    }
    let recovered = FecBlock {
        frame: 1,
        block_index: 0,
        block_count: 1,
        data_shards: 9,
        parity_shards: 2,
        arrived_shards: 9,
        missing_shards: 2,
        unrecovered: false,
    };
    assert_eq!(fec_blocks(&events), [recovered]);

    // Three lost are past the parity: the block ends unrecovered when the next one begins.
    events.clear();
    for (i, packet) in host.frame(&payload, 1).iter().enumerate() {
        if ![1, 4, 6].contains(&i) {
            rx.receive(packet, 6, &mut events);
        }
    }
    assert!(fec_blocks(&events).is_empty(), "still waiting for shards");
    for packet in host.frame(&payload, 1) {
        rx.receive(&packet, 7, &mut events);
    }
    let lost = FecBlock {
        frame: 2,
        arrived_shards: 8,
        missing_shards: 3,
        unrecovered: true,
        ..recovered
    };
    assert_eq!(fec_blocks(&events)[0], lost);

    // Complete with parity still on the way: only the shards up to the last arrival count.
    events.clear();
    let packets = host.frame(&payload, 1);
    for packet in &packets[..9] {
        rx.receive(packet, 8, &mut events);
    }
    let early = FecBlock {
        frame: 4,
        arrived_shards: 9,
        missing_shards: 0,
        unrecovered: false,
        ..recovered
    };
    assert_eq!(fec_blocks(&events), [early]);
}

#[test]
fn lost_frame_requests_idr_without_rfi() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    let _lost = host.frame(&h264_p(100), 1);
    for packet in host.frame(&h264_p(100), 1) {
        rx.receive(&packet, 2, &mut events);
    }
    assert_eq!(frames(&events).len(), 1);
    assert!(events
        .iter()
        .any(|e| matches!(e, VideoEvent::Signal(VideoSignal::RequestIdr))));
}

#[test]
fn encrypted_packets_decrypt() {
    use aes_gcm::aead::generic_array::GenericArray;
    use aes_gcm::aead::{AeadInPlace, KeyInit};
    let key = [7u8; 16];
    let cipher = aes_gcm::Aes128Gcm::new(GenericArray::from_slice(&key));
    let mut host = Host::new();
    let mut cfg = config(0x1000);
    cfg.encryption_key = Some(key);
    let mut rx = VideoReceiver::new(cfg);
    let mut events = Vec::new();
    let payload = vec![0x5a; 2000];
    for (n, mut packet) in host.frame(&payload, 2).into_iter().enumerate() {
        let mut iv = [0u8; 12];
        iv[..8].copy_from_slice(&(n as u64).to_le_bytes());
        let tag = cipher
            .encrypt_in_place_detached(GenericArray::from_slice(&iv), &[], &mut packet)
            .unwrap();
        let mut datagram = iv.to_vec();
        datagram.extend_from_slice(&1u32.to_le_bytes());
        datagram.extend_from_slice(&tag);
        datagram.extend_from_slice(&packet);
        if n == 1 {
            let mut forged = datagram.clone();
            forged[40] ^= 1;
            rx.receive(&forged, 1, &mut events);
        }
        rx.receive(&datagram, 1, &mut events);
    }
    assert_eq!(frames(&events)[0].data(), &payload[..]);
    assert_eq!(rx.stats().decrypt_failures, 1);
}

#[test]
fn pyrowave_records_build_an_envelope() {
    let mut host = Host::new();
    let mut cfg = config(0x01_0000);
    cfg.pyrowave_records = true;
    let mut rx = VideoReceiver::new(cfg);
    let mut events = Vec::new();
    let payload = vec![3u8; 2500];
    let packets = host.frame(&payload, 2);
    let count = packets.len();
    for packet in &packets {
        rx.receive(packet, 9, &mut events);
    }
    let frames = frames(&events);
    assert_eq!(frames.len(), 1);
    let data = frames[0].data();
    assert_eq!(&data[..4], &[0, 0, 0, 0]);
    let first_len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let first_flags = u32::from_le_bytes(data[8..12].try_into().unwrap());
    assert_eq!(first_len, PACKET_SIZE - 16);
    assert_eq!(first_flags, depacketizer::PYRO_PACKET_FRAME_START);
    assert!(count >= 3);
}

#[test]
fn receive_buffer_size() {
    assert_eq!(recv_buffer_size(20_000, 1392), 2048 * 1408);
    assert_eq!(recv_buffer_size(2_000_000, 1392), 15_000_000);
    assert_eq!(recv_buffer_size(u32::MAX, 1392), 64 * 1024 * 1024);
}

#[test]
fn fec_status_wire_format() {
    let status = FecStatus {
        frame_index: 0x01020304,
        highest_received_sequence: 0x0506,
        block_count: 3,
        ..FecStatus::default()
    };
    let bytes = status.to_bytes();
    assert_eq!(&bytes[..6], &[1, 2, 3, 4, 5, 6]);
    assert_eq!(bytes[20], 3);
}

pub(crate) mod early_chunks;
mod sei;
mod slice_delivery;
