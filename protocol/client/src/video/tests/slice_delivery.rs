//! Packets of the Rubylight host's `video_slice_delivery`: a P-frame leaves in
//! FEC blocks while the encoder is still producing it. The packets are the
//! host's own (`rust/tests/fixtures/slice-delivery-*.bin` in the Rubylight
//! repository, written by its `streamed_frames_match_the_wire_fixtures...` test),
//! and the stock receive path must reassemble them unchanged.
//!
//! To refresh the copies in `tests/fixtures`, run that host test with
//! `UPDATE_SLICE_FIXTURES=1` and copy the two files here.

use super::*;

/// One frame of a fixture: its kind (1 P, 2 IDR), payload and packets.
struct Captured {
    kind: u8,
    payload: Vec<u8>,
    packets: Vec<Vec<u8>>,
}

struct Fixture {
    packet_size: usize,
    key: Option<[u8; 16]>,
    frames: Vec<Captured>,
}

fn load(name: &str) -> Fixture {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("slice-delivery-{name}.bin"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut at = 0;
    let mut take = |n: usize| {
        let part = &bytes[at..at + n];
        at += n;
        part
    };
    let u32le = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()) as usize;
    assert_eq!(take(8), b"RLSLICE1");
    let packet_size = u32le(take(4));
    let key: [u8; 16] = take(16).try_into().unwrap();
    let count = u32le(take(4));
    let frames = (0..count)
        .map(|_| {
            let kind = take(1)[0];
            let length = u32le(take(4));
            let payload = take(length).to_vec();
            let packets = u32le(take(4));
            let packets = (0..packets)
                .map(|_| {
                    let n = u16::from_le_bytes(take(2).try_into().unwrap()) as usize;
                    take(n).to_vec()
                })
                .collect();
            Captured {
                kind,
                payload,
                packets,
            }
        })
        .collect();
    Fixture {
        packet_size,
        key: (key != [0; 16]).then_some(key),
        frames,
    }
}

fn receiver(fixture: &Fixture) -> VideoReceiver {
    VideoReceiver::new(VideoConfig {
        format: 0x1,
        packet_size: fixture.packet_size,
        app_version: [7, 1, 431, -1],
        rfi: false,
        pyrowave_records: false,
        colorspace: COLORSPACE_REC_709,
        encryption_key: fixture.key,
    })
}

/// Feed every frame of the fixture, dropping the packets `lose` picks
/// (frame number in the fixture, packet number in the frame).
fn play(
    fixture: &Fixture,
    lose: impl Fn(usize, usize) -> bool,
) -> (VideoReceiver, Vec<VideoEvent>) {
    let mut rx = receiver(fixture);
    let mut events = Vec::new();
    for (number, frame) in fixture.frames.iter().enumerate() {
        for (index, packet) in frame.packets.iter().enumerate() {
            if !lose(number, index) {
                rx.receive(packet, 1_000_000 + number as u64 * 16_000, &mut events);
            }
        }
        if frame.kind == 2 {
            rx.frame_decoded(FrameType::Idr);
        }
    }
    (rx, events)
}

fn assert_all_frames(fixture: &Fixture, events: &[VideoEvent]) {
    let received = frames(events);
    assert_eq!(received.len(), fixture.frames.len());
    for (got, sent) in received.iter().zip(&fixture.frames) {
        let expected = if sent.kind == 2 {
            FrameType::Idr
        } else {
            FrameType::P
        };
        assert_eq!(got.frame_type, expected);
        // H.264 keeps the zero padding of the last shard.
        let data = got.data();
        assert_eq!(&data[..sent.payload.len()], &sent.payload[..]);
        assert!(data[sent.payload.len()..].iter().all(|&b| b == 0));
        assert!(data.len() - sent.payload.len() < fixture.packet_size - 16);
    }
    assert!(!events
        .iter()
        .any(|e| matches!(e, VideoEvent::Signal(VideoSignal::RequestIdr))));
}

#[test]
fn streamed_frames_reassemble_byte_identical() {
    for name in ["plain", "gcm"] {
        let fixture = load(name);
        assert_eq!(fixture.key.is_some(), name == "gcm");
        assert_eq!(fixture.frames.len(), 3);
        let (rx, events) = play(&fixture, |_, _| false);
        assert_all_frames(&fixture, &events);
        assert_eq!(rx.stats().packets_recovered, 0, "{name}");
        assert_eq!(rx.stats().decrypt_failures, 0, "{name}");
    }
}

/// What the host wrote into the plain packets of a streamed frame: the frame
/// header, and a `streamPacketIndex` (bytes 16..20) that counts data shards
/// and parity alike across blocks, like the RTP sequence.
#[test]
fn streamed_packets_carry_the_header_and_stream_index_the_receiver_reads() {
    let fixture = load("plain");
    for frame in fixture.frames.iter().filter(|f| f.kind == 1) {
        let first = &frame.packets[0];
        // Frame header: 1, host latency in 100 us (u16), frame type 1, the last
        // payload length (not known when block 0 left: a full shard) and 0, 0.
        assert_eq!(&first[32..35], &[1, 5, 0]);
        assert_eq!(first[35], 1);
        assert_eq!(
            u16::from_le_bytes([first[36], first[37]]) as usize,
            fixture.packet_size - 16
        );
        assert_eq!(&first[38..40], &[0, 0]);
        let mut sequence = u16::from_be_bytes([first[2], first[3]]);
        let mut blocks_seen = std::collections::BTreeSet::new();
        for packet in &frame.packets {
            let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
            let (index, data) = ((info >> 12) & 0x3ff, info >> 22);
            let block = (packet[27] >> 4) & 3;
            blocks_seen.insert(block);
            assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), sequence);
            if (index as u32) < data {
                let stream_index = u32::from_le_bytes(packet[16..20].try_into().unwrap());
                assert_eq!(stream_index >> 8, u32::from(sequence), "block {block}");
                assert_eq!(stream_index & 0xff, 0);
            }
            sequence = sequence.wrapping_add(1);
        }
        assert!(blocks_seen.len() >= 2, "{blocks_seen:?}");
        assert_eq!(usize::from(first[27] >> 6) + 1, blocks_seen.len());
    }
}

#[test]
fn a_lost_packet_in_every_block_of_a_streamed_frame_is_recovered() {
    let fixture = load("plain");
    // The second data shard of every block of the P-frames (the first when a block
    // holds a single one, as a block sent early can).
    let lose = |number: usize, index: usize| {
        let packet = &fixture.frames[number].packets[index];
        let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
        fixture.frames[number].kind == 1 && (info >> 12) & 0x3ff == 1.min((info >> 22) - 1)
    };
    let lost: usize = fixture
        .frames
        .iter()
        .enumerate()
        .map(|(n, f)| (0..f.packets.len()).filter(|&i| lose(n, i)).count())
        .sum();
    assert!(lost >= 6, "{lost} packets lost");
    let (rx, events) = play(&fixture, lose);
    assert_all_frames(&fixture, &events);
    assert_eq!(rx.stats().packets_recovered, lost as u64);
}
