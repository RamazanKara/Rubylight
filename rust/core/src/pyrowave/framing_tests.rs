//! Framing as pieces of the bitstream: the bytes on the wire are the framed
//! buffer's, byte for byte, for records clients and older-framing clients.
use super::rows_tests::{FIXTURES, synthetic};
use super::*;
use crate::packet::{BlockPool, PyrowaveFec, VideoPacketizer};

const PACKET_SIZES: [usize; 3] = [1392, 1360, 1024];

/// A pseudo-random sequence, the same on every run.
fn sequence(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    }
}

/// Every framing the encoder can choose for `frame`, with what it frames to
/// as one buffer: (payload, limit, rows, expected bytes).
fn framings(frame: &[u8]) -> Vec<(usize, usize, bool, Vec<u8>)> {
    let mut out = vec![];
    for packet_size in PACKET_SIZES {
        let limit = max_frame_bytes(packet_size, true);
        for payload in [aligned_payload(packet_size), 0] {
            for rows in [false, true] {
                let bytes = record_frame_with(frame, payload, limit, rows).unwrap();
                out.push((payload, limit, rows, bytes));
            }
        }
    }
    out
}

#[test]
fn record_framing_as_pieces_is_the_framed_buffer_byte_for_byte() {
    let mut reused = Vec::new();
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        for (payload, limit, rows, expected) in framings(&frame) {
            let framing = Framing::records(&frame, payload, limit, rows).unwrap();
            assert_eq!(framing.len(), expected.len(), "{w}x{h} {payload} {rows}");
            let view = framing.view(&frame).unwrap();
            assert_eq!(view.len(), expected.len());
            assert!(
                Payload::from(view).to_vec() == expected,
                "{w}x{h} payload {payload} rows {rows}: framed bytes differ"
            );
            assert_eq!(framing.is_whole(&frame), expected == frame);
            reused = record_frame_into(&frame, payload, limit, rows, reused).unwrap();
            assert!(reused == expected);
        }
    }
}

#[test]
fn a_frame_framed_past_the_limit_goes_out_whole_in_either_form() {
    let frame = synthetic(1920, 1080, false, 2, 40);
    let payload = aligned_payload(1392);
    let framed = record_frame_with(&frame, payload, usize::MAX, false).unwrap();
    assert!(framed.len() > frame.len());
    // Room for the bitstream, not for its padding.
    let limit = frame.len() + (framed.len() - frame.len()) / 2;
    let whole = record_frame_with(&frame, payload, limit, false).unwrap();
    assert!(whole == frame);
    let framing = Framing::records(&frame, payload, limit, false).unwrap();
    assert!(framing.is_whole(&frame));
    assert!(Payload::from(framing.view(&frame).unwrap()).to_vec() == frame);
    // Under the bitstream itself, both refuse it.
    assert!(record_frame_with(&frame, payload, frame.len() - 4, false).is_err());
    assert!(Framing::records(&frame, payload, frame.len() - 4, false).is_err());
}

#[test]
fn a_damaged_bitstream_is_refused_as_pieces_as_it_is_as_bytes() {
    let frame = synthetic(128, 128, false, 9, 0);
    let mut short = frame.clone();
    short.truncate(frame.len() - 4);
    let mut miscounted = frame.clone();
    miscounted[4] ^= 1;
    for bad in [&short[..], &miscounted[..], &frame[..4]] {
        for rows in [false, true] {
            let payload = aligned_payload(1392);
            let limit = max_frame_bytes(1392, true);
            assert!(record_frame_with(bad, payload, limit, rows).is_err());
            assert!(Framing::records(bad, payload, limit, rows).is_err());
        }
    }
}

/// Packet slices of `frame` as the codec's packetizer reports them: in order,
/// of mixed sizes, covering it.
fn slices(frame: &[u8], seed: u64) -> Vec<(usize, usize)> {
    let mut next = sequence(seed);
    let (mut out, mut at) = (vec![], 0);
    while at < frame.len() {
        let size = (1 + next() as usize % 9000).min(frame.len() - at);
        out.push((at, size));
        at += size;
    }
    out
}

#[test]
fn the_older_container_as_pieces_is_the_container_byte_for_byte() {
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        // Out of order and overlapping slices are passed through as given.
        let odd = vec![(16, 40), (0, 64), (frame.len() - 8, 8)];
        for packets in [slices(&frame, seed), vec![(0, frame.len())], odd] {
            let expected = container(&frame, &packets).unwrap();
            let framing = Framing::container(&frame, packets.iter().copied()).unwrap();
            assert_eq!(framing.len(), expected.len());
            assert!(!framing.is_whole(&frame));
            assert!(Payload::from(framing.view(&frame).unwrap()).to_vec() == expected);
        }
    }
}

#[test]
fn an_invalid_older_container_is_refused_as_pieces_as_it_is_as_bytes() {
    let frame = vec![7u8; 100];
    let cases: [&[(usize, usize)]; 5] =
        [&[], &[(0, 0)], &[(0, 101)], &[(96, 8)], &[(usize::MAX, 2)]];
    for packets in cases {
        assert!(container(&frame, packets).is_err(), "{packets:?}");
        assert!(
            Framing::container(&frame, packets.iter().copied()).is_err(),
            "{packets:?}"
        );
    }
}

#[test]
fn any_range_of_the_pieces_reads_as_the_same_range_of_the_buffer() {
    let frame = synthetic(1920, 1080, true, 3, 60);
    let payload = aligned_payload(1392);
    let limit = max_frame_bytes(1392, true);
    let mut next = sequence(11);
    for (framing, expected) in [
        (
            Framing::records(&frame, payload, limit, true).unwrap(),
            record_frame_with(&frame, payload, limit, true).unwrap(),
        ),
        (
            Framing::container(&frame, slices(&frame, 4).into_iter()).unwrap(),
            container(&frame, &slices(&frame, 4)).unwrap(),
        ),
    ] {
        let framed = Payload::from(framing.view(&frame).unwrap());
        let mut out = vec![0; 3000];
        for _ in 0..2000 {
            let at = next() as usize % expected.len();
            let n = (next() as usize % out.len()).min(expected.len() - at);
            framed.copy_to(at, &mut out[..n]);
            assert!(out[..n] == expected[at..at + n], "{n} bytes at {at}");
        }
        // The packetizer's own reads: every shard slice in turn, the last one short.
        let slice = 1392 - 16;
        for at in (0..expected.len()).step_by(slice) {
            let n = slice.min(expected.len() - at);
            framed.copy_to(at, &mut out[..n]);
            assert!(out[..n] == expected[at..at + n]);
        }
        framed.copy_to(expected.len(), &mut []);
    }
}

#[test]
#[should_panic(expected = "past its end")]
fn reading_past_the_pieces_panics_as_a_slice_does() {
    let frame = synthetic(128, 128, false, 4, 7);
    let framing = Framing::records(&frame, aligned_payload(1392), usize::MAX, false).unwrap();
    let framed = framing.view(&frame).unwrap();
    framed.copy_to(framed.len() - 3, &mut [0; 4]);
}

#[test]
fn words_read_from_the_pieces_in_any_order_are_the_buffer_words() {
    let frame = synthetic(1280, 720, true, 5, 0);
    let payload = aligned_payload(1024);
    let framing = Framing::records(&frame, payload, usize::MAX, true).unwrap();
    let expected = record_frame_with(&frame, payload, usize::MAX, true).unwrap();
    let framed = Payload::from(framing.view(&frame).unwrap());
    let mut reader = framed.reader();
    assert_eq!(reader.len(), expected.len());
    // Forward over every byte offset, where words straddle pieces.
    for at in 0..expected.len() - 3 {
        assert_eq!(reader.word(at), word(&expected, at), "at {at}");
    }
    // Then back and forth.
    let mut next = sequence(5);
    for _ in 0..5000 {
        let at = next() as usize % (expected.len() - 3);
        assert_eq!(reader.word(at), word(&expected, at), "at {at}");
    }
}

#[test]
fn records_and_layout_read_from_the_pieces_match_the_buffer() {
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        for (payload, limit, rows, expected) in framings(&frame) {
            if payload == 0 {
                continue;
            }
            let framing = Framing::records(&frame, payload, limit, rows).unwrap();
            let framed = framing.view(&frame).unwrap();
            let fields = |records: Vec<Record>| -> Vec<(usize, usize, u32)> {
                records
                    .into_iter()
                    .map(|r| (r.offset, r.size, r.index))
                    .collect()
            };
            assert_eq!(
                fields(records_in(&mut Payload::from(framed).reader()).unwrap()),
                fields(records(&expected).unwrap())
            );
            let pieces = layout(framed, payload).unwrap();
            let bytes = layout(&expected, payload).unwrap();
            assert_eq!(pieces.critical_shards, bytes.critical_shards);
            assert_eq!(pieces.starts, bytes.starts);
        }
    }
}

#[test]
fn a_framing_is_refused_over_a_shorter_bitstream() {
    let frame = synthetic(128, 128, false, 4, 7);
    let framing = Framing::records(&frame, aligned_payload(1392), usize::MAX, false).unwrap();
    assert!(framing.view(&frame[..frame.len() - 4]).is_err());
    assert!(framing.view(&frame).is_ok());
    let packets = [(0, 64)];
    let framing = Framing::container(&frame, packets.iter().copied()).unwrap();
    assert!(framing.view(&frame[..64]).is_ok());
    assert!(framing.view(&frame[..63]).is_err());
}

fn packetizer(key: Option<[u8; 16]>, fec_percent: usize) -> VideoPacketizer {
    VideoPacketizer {
        sequence: 77,
        iv_counter: 5,
        frame: 3,
        packet_size: 1392,
        fec_percent,
        min_fec: 2,
        key,
    }
}

fn fec(records: bool, critical_percentage: usize, wire_budget: usize) -> PyrowaveFec {
    PyrowaveFec {
        records,
        critical_percentage,
        detail_percentage: 0,
        wire_budget,
        ipv6: false,
    }
}

/// The wire packets of two frames in a row, from `payload` as one buffer and
/// as pieces, through both of the packetizer's PyroWave entry points.
fn assert_same_packets(
    bytes: &[u8],
    framed: Framed,
    records: bool,
    critical: usize,
    budget: usize,
    key: Option<[u8; 16]>,
    fec_percent: usize,
) {
    let (mut a, mut b) = (packetizer(key, fec_percent), packetizer(key, fec_percent));
    for frame in 0..2 {
        let one = a
            .encode_pyrowave(bytes, 9000 + frame, 543, fec(records, critical, budget))
            .unwrap();
        let other = b
            .encode_pyrowave(framed, 9000 + frame, 543, fec(records, critical, budget))
            .unwrap();
        assert!(
            one == other,
            "records {records} critical {critical} budget {budget} key {} fec {fec_percent}",
            key.is_some()
        );
    }
    let pool = BlockPool::default();
    let (count, limited, blocks) = a
        .pyrowave_blocks_in(
            bytes,
            9100,
            543,
            fec(records, critical, budget),
            Some(&pool),
        )
        .unwrap();
    let one = blocks.collect::<Result<Vec<_>>>().unwrap();
    let (other_count, other_limited, blocks) = b
        .pyrowave_blocks_in(
            framed,
            9100,
            543,
            fec(records, critical, budget),
            Some(&pool),
        )
        .unwrap();
    let other = blocks.collect::<Result<Vec<_>>>().unwrap();
    assert_eq!((count, limited), (other_count, other_limited));
    assert!(one == other);
    assert_eq!(
        (a.sequence, a.iv_counter, a.frame),
        (b.sequence, b.iv_counter, b.frame)
    );
}

#[test]
fn records_clients_get_the_same_packets_from_the_pieces() {
    let key = Some(*b"0123456789abcdef");
    for &(w, h, c, seed, big) in &FIXTURES[..4] {
        let frame = synthetic(w, h, c, seed, big);
        for rows in [false, true] {
            let payload = aligned_payload(1392);
            let limit = max_frame_bytes(1392, true);
            let bytes = record_frame_with(&frame, payload, limit, rows).unwrap();
            let framing = Framing::records(&frame, payload, limit, rows).unwrap();
            let framed = framing.view(&frame).unwrap();
            for (critical, budget) in [(0, 0), (20, 0), (50, 3_000_000), (100, 0)] {
                for key in [None, key] {
                    assert_same_packets(&bytes, framed, true, critical, budget, key, 20);
                }
            }
        }
    }
}

#[test]
fn older_framing_clients_get_the_same_packets_from_the_pieces() {
    let key = Some(*b"fedcba9876543210");
    for &(w, h, c, seed, big) in &FIXTURES[..4] {
        let frame = synthetic(w, h, c, seed, big);
        let packets = slices(&frame, seed);
        let bytes = container(&frame, &packets).unwrap();
        let framing = Framing::container(&frame, packets.iter().copied()).unwrap();
        let framed = framing.view(&frame).unwrap();
        for (critical, fec_percent) in [(0, 0), (0, 20), (20, 20), (0, 100)] {
            for key in [None, key] {
                assert_same_packets(&bytes, framed, false, critical, 0, key, fec_percent);
            }
        }
    }
}

#[test]
fn a_framing_dropped_and_built_again_frames_the_same() {
    let frame = synthetic(1920, 1080, false, 2, 40);
    let payload = aligned_payload(1392);
    let first = Framing::records(&frame, payload, usize::MAX, true).unwrap();
    let expected = Payload::from(first.view(&frame).unwrap()).to_vec();
    drop(first);
    // Its piece list is reused, cleared.
    for _ in 0..6 {
        let again = Framing::records(&frame, payload, usize::MAX, true).unwrap();
        assert!(Payload::from(again.view(&frame).unwrap()).to_vec() == expected);
    }
}
