use super::*;
use alloc::vec;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 32;

fn header(sequence: u32, count: u32) -> [u8; 8] {
    let mut bytes = [0; 8];
    write_le32(&mut bytes, 0, HEADER_BIT | (sequence << 28) | (WIDTH - 1) | ((HEIGHT - 1) << 14));
    write_le32(&mut bytes, 4, count);
    bytes
}

/// A record block of `words` words (at least 2) for record `index`, filled with `fill`.
fn record(sequence: u32, index: u32, words: u32, fill: u8) -> Vec<u8> {
    let mut bytes = vec![fill; words as usize * 4];
    write_le32(&mut bytes, 0, (sequence << 28) | (words << 16));
    write_le32(&mut bytes, 4, index << 8);
    bytes
}

fn padding(words: u32) -> Vec<u8> {
    let mut bytes = vec![0; 8 + words as usize * 4];
    write_le32(&mut bytes, 0, PADDING);
    write_le32(&mut bytes, 4, words);
    bytes
}

struct Packet {
    flags: u32,
    payload: Vec<u8>,
}

/// Splits a frame's bitstream into packets of `size` bytes, as the host sends them.
fn packets(frame_header: [u8; 8], body: &[u8], size: usize) -> Vec<Packet> {
    let mut chunks: Vec<Vec<u8>> = body.chunks(size).map(<[u8]>::to_vec).collect();
    if chunks.is_empty() {
        chunks.push(Vec::new());
    }
    let count = chunks.len();
    // The last packet's real length (FEC pads it), counting the prefix when it is also the first.
    let last = chunks.last().unwrap().len() + if count == 1 { 16 } else { 0 };
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut flags = PACKET_RECORD_START;
            let mut payload = Vec::new();
            if i == 0 {
                flags |= PACKET_FRAME_START;
                payload.extend_from_slice(&[1, 0, 0, 0, last as u8, (last >> 8) as u8, 0, 0]);
                payload.extend_from_slice(&frame_header);
            }
            if i == count - 1 {
                flags |= PACKET_FRAME_END;
            }
            payload.extend_from_slice(&chunk);
            Packet { flags, payload }
        })
        .collect()
}

/// The client's container for the packets that arrived; `lost` packets are left out and the
/// packet after a gap loses its contiguous flag.
fn container(packets: &[Packet], lost: &[usize]) -> Vec<u8> {
    let mut out = vec![0; 4];
    let mut previous_kept = None;
    for (i, packet) in packets.iter().enumerate() {
        if lost.contains(&i) {
            continue;
        }
        let mut flags = packet.flags;
        if previous_kept == Some(i.wrapping_sub(1)) {
            flags |= PACKET_CONTIGUOUS;
        }
        out.extend_from_slice(&(packet.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&packet.payload);
        previous_kept = Some(i);
    }
    out
}

fn decode(records: &mut Records, data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut pushed = Vec::new();
    records.push_frame(data, |packet| {
        pushed.push(packet.to_vec());
        true
    }).then_some(pushed)
}

fn count_of(header: &[u8]) -> u32 {
    le32(header, 4) & 0xff_ffff
}

#[test]
fn complete_frame_pushes_header_then_records_in_index_order() {
    let body = [record(1, 2, 4, 0xa2), record(1, 0, 3, 0xa0), record(1, 1, 2, 0xa1)].concat();
    let data = container(&packets(header(1, 3), &body, 1400), &[]);
    let mut records = Records::with_geometry(WIDTH, HEIGHT, false);
    let pushed = decode(&mut records, &data).unwrap();
    assert_eq!(pushed.len(), 4);
    assert_eq!(pushed[0], header(1, 3));
    assert_eq!(pushed[1], record(1, 0, 3, 0xa0));
    assert_eq!(pushed[2], record(1, 1, 2, 0xa1));
    assert_eq!(pushed[3], record(1, 2, 4, 0xa2));
    assert_eq!(records.loss_percent, 0.0);
}

#[test]
fn records_spanning_packets_and_padding_reassemble() {
    let body = [record(2, 0, 10, 1), padding(3), record(2, 1, 12, 2), record(2, 2, 7, 3)].concat();
    let data = container(&packets(header(2, 3), &body, 24), &[]);
    let pushed = decode(&mut Records::default(), &data).unwrap();
    assert_eq!(pushed.len(), 4);
    assert_eq!(pushed[2], record(2, 1, 12, 2));
}

#[test]
fn lost_record_is_replayed_from_the_previous_frame_with_the_new_sequence() {
    let mut records = Records::with_geometry(WIDTH, HEIGHT, false);
    let first = [record(1, 0, 6, 1), record(1, 1, 6, 2), record(1, 2, 6, 3)].concat();
    assert!(decode(&mut records, &container(&packets(header(1, 3), &first, 24), &[])).is_some());

    // 24-byte packets carry one record each; packet 1 (record 1) is lost.
    let second = [record(2, 0, 6, 4), record(2, 1, 6, 5), record(2, 2, 6, 6)].concat();
    let pushed = decode(&mut records, &container(&packets(header(2, 3), &second, 24), &[1])).unwrap();
    assert_eq!(count_of(&pushed[0]), 3);
    assert_eq!(pushed[1], record(2, 0, 6, 4));
    assert_eq!(pushed[2], record(2, 2, 6, 6));
    assert_eq!(pushed[3], record(2, 1, 6, 2), "record 1 comes from frame 1, renumbered to sequence 2");
    assert!((records.loss_percent - 100.0 / 3.0).abs() < 0.01);
}

#[test]
fn lost_header_packet_uses_the_negotiated_geometry() {
    let body = [record(3, 0, 8, 1), record(3, 1, 8, 2)].concat();
    let all = packets(header(3, 2), &body, 32);
    let data = container(&all, &[0]);
    let mut records = Records::with_geometry(WIDTH, HEIGHT, true);
    let pushed = decode(&mut records, &data).unwrap();
    assert_eq!(le32(&pushed[0], 0), HEADER_BIT | (3 << 28) | (WIDTH - 1) | ((HEIGHT - 1) << 14));
    assert_eq!(le32(&pushed[0], 4) >> 26 & 1, 1, "4:4:4 from the negotiated format");
    assert_eq!(records.loss_percent, -1.0);

    // Without negotiated geometry the first frame can't be rebuilt.
    assert!(decode(&mut Records::default(), &data).is_none());
}

#[test]
fn lost_header_does_not_report_cached_record_count_as_current_loss() {
    let mut records = Records::with_geometry(WIDTH, HEIGHT, false);
    let first = [record(1, 0, 8, 1), record(1, 1, 8, 2)].concat();
    assert!(decode(&mut records, &container(&packets(header(1, 2), &first, 32), &[])).is_some());

    let second = [record(2, 0, 8, 3), record(2, 1, 8, 4), record(2, 2, 8, 5)].concat();
    let pushed = decode(&mut records, &container(&packets(header(2, 3), &second, 32), &[0])).unwrap();
    assert_eq!(count_of(&pushed[0]), 3);
    assert_eq!(records.loss_percent, -1.0);
}

#[test]
fn format_change_drops_the_cached_records() {
    let mut records = Records::with_geometry(WIDTH, HEIGHT, false);
    let first = [record(1, 0, 6, 1), record(1, 1, 6, 2)].concat();
    assert!(decode(&mut records, &container(&packets(header(1, 2), &first, 1400), &[])).is_some());
    let mut other = header(2, 2);
    other[7] |= 0x04; // A different format byte.
    let second = [record(2, 0, 6, 3), record(2, 1, 6, 4)].concat();
    let pushed = decode(&mut records, &container(&packets(other, &second, 24), &[1])).unwrap();
    assert_eq!(count_of(&pushed[0]), 1, "nothing replayed across a format change");
}

#[test]
fn malformed_frames_are_rejected() {
    let mut records = Records::with_geometry(WIDTH, HEIGHT, false);
    let ok = container(&packets(header(1, 1), &record(1, 0, 4, 9), 1400), &[]);
    assert!(decode(&mut records.clone(), &ok).is_some());

    // Ordinary framing, truncation, oversized packets and unknown flags.
    let mut ordinary = ok.clone();
    ordinary[0] = 1;
    assert!(decode(&mut records.clone(), &ordinary).is_none());
    assert!(decode(&mut records.clone(), &ok[..ok.len() - 1]).is_none());
    let mut flags = ok.clone();
    flags[8] |= 0x10;
    assert!(decode(&mut records.clone(), &flags).is_none());

    // Duplicate record index, and records from two sequences in one frame.
    let duplicate = [record(1, 0, 4, 1), record(1, 0, 4, 2)].concat();
    assert!(decode(&mut records.clone(), &container(&packets(header(1, 2), &duplicate, 1400), &[])).is_none());
    let mixed = [record(1, 0, 4, 1), record(2, 1, 4, 2)].concat();
    assert!(decode(&mut records.clone(), &container(&packets(header(1, 2), &mixed, 1400), &[])).is_none());

    // More records than the header announces, or fewer in a complete frame.
    let extra = [record(1, 0, 4, 1), record(1, 1, 4, 2)].concat();
    assert!(decode(&mut records.clone(), &container(&packets(header(1, 1), &extra, 1400), &[])).is_none());
    assert!(decode(&mut records.clone(), &container(&packets(header(1, 3), &extra, 1400), &[])).is_none());
    assert_eq!(records.loss_percent, 0.0, "clones above leave the original untouched");
    assert!(decode(&mut records, &ordinary).is_none());
    assert_eq!(records.loss_percent, 100.0);
}

#[test]
fn a_refused_push_fails_the_frame() {
    let data = container(&packets(header(1, 1), &record(1, 0, 4, 9), 1400), &[]);
    assert!(!Records::default().push_frame(&data, |_| false));
}

#[test]
fn consecutive_losses_retain_the_latest_record_and_callback_order() {
    let mut records = Records::default();
    // Sparse indices also exercise recovery without allocating an index-sized table.
    let first = [record(1, 0xff_ffff, 6, 1), record(1, 7, 6, 2), record(1, 200, 6, 3)].concat();
    decode(&mut records, &container(&packets(header(1, 3), &first, 24), &[])).unwrap();

    let second = [record(2, 0xff_ffff, 6, 4), record(2, 7, 6, 5), record(2, 200, 6, 6)].concat();
    let pushed = decode(&mut records, &container(&packets(header(2, 3), &second, 24), &[1])).unwrap();
    assert_eq!(pushed, vec![header(2, 3).to_vec(), record(2, 200, 6, 6),
        record(2, 0xff_ffff, 6, 4), record(2, 7, 6, 2)]);

    // Lose the header and the last packet: new record 7, then cached 200 and maximum index.
    let third = [record(3, 0xff_ffff, 6, 7), record(3, 7, 6, 8), record(3, 200, 6, 9)].concat();
    let pushed = decode(&mut records, &container(&packets(header(3, 3), &third, 24), &[0, 2])).unwrap();
    assert_eq!(pushed, vec![header(3, 3).to_vec(), record(3, 7, 6, 8),
        record(3, 200, 6, 6), record(3, 0xff_ffff, 6, 4)]);
    assert_eq!(records.loss_percent, -1.0);
}

#[test]
fn truncated_record_at_a_gap_is_recovered_without_joining_unrelated_runs() {
    let mut records = Records::default();
    let first = [record(1, 0, 3, 1), record(1, 1, 6, 2), record(1, 2, 3, 3)].concat();
    decode(&mut records, &container(&packets(header(1, 3), &first, 48), &[])).unwrap();
    let second = [record(2, 0, 3, 4), record(2, 1, 6, 5), record(2, 2, 3, 6)].concat();
    let mut all = packets(header(2, 3), &second, 12);
    all[2].flags &= !PACKET_RECORD_START; // This payload continues record 1.
    let pushed = decode(&mut records, &container(&all, &[2])).unwrap();
    assert_eq!(pushed, vec![header(2, 3).to_vec(), record(2, 0, 3, 4),
        record(2, 2, 3, 6), record(2, 1, 6, 2)]);
}

#[test]
fn duplicate_indices_across_gaps_and_malformed_tails_do_not_change_the_cache() {
    let first = [record(1, 0, 6, 1), record(1, 1, 6, 2), record(1, 2, 6, 3)].concat();
    let mut original = Records::default();
    decode(&mut original, &container(&packets(header(1, 3), &first, 24), &[])).unwrap();
    let duplicate = [record(2, 0, 6, 4), record(2, 1, 6, 5), record(2, 0, 6, 6)].concat();
    let mut malformed = container(&packets(header(2, 1), &record(2, 0, 6, 9), 24), &[]);
    malformed.push(0); // A valid record must not be emitted before discovering this tail.
    for rejected in [container(&packets(header(2, 3), &duplicate, 24), &[1]), malformed] {
        let mut records = original.clone();
        let mut calls = 0;
        assert!(!records.push_frame(&rejected, |_| { calls += 1; true }));
        assert_eq!(calls, 0);
        assert_eq!(records.loss_percent, 100.0);
        let next = [record(3, 0, 6, 7), record(3, 1, 6, 8), record(3, 2, 6, 9)].concat();
        let pushed = decode(&mut records, &container(&packets(header(3, 3), &next, 24), &[0, 2])).unwrap();
        assert_eq!(pushed, vec![header(3, 3).to_vec(), record(3, 1, 6, 8),
            record(3, 0, 6, 1), record(3, 2, 6, 3)]);
    }
}

#[test]
fn refusal_at_header_received_or_recovered_record_keeps_the_previous_cache() {
    let first = [record(1, 0, 6, 1), record(1, 1, 6, 2), record(1, 2, 6, 3)].concat();
    let mut original = Records::default();
    decode(&mut original, &container(&packets(header(1, 3), &first, 24), &[])).unwrap();
    let second = [record(2, 0, 6, 4), record(2, 1, 6, 5), record(2, 2, 6, 6)].concat();
    let rejected = container(&packets(header(2, 3), &second, 24), &[1]);
    for refuse_at in 0..4 {
        let mut records = original.clone();
        let mut calls = 0;
        assert!(!records.push_frame(&rejected, |_| {
            let accept = calls != refuse_at;
            calls += 1;
            accept
        }));
        assert_eq!(calls, refuse_at + 1);
        assert_eq!(records.loss_percent, 100.0);

        // A distinct record makes every original record missing in this next partial frame.
        let next = [record(3, 8, 6, 7), record(3, 9, 6, 8)].concat();
        let pushed = decode(&mut records, &container(&packets(header(3, 2), &next, 24), &[1])).unwrap();
        assert_eq!(pushed, vec![header(3, 4).to_vec(), record(3, 8, 6, 7),
            record(3, 0, 6, 1), record(3, 1, 6, 2), record(3, 2, 6, 3)]);
    }
}

#[test]
fn empty_complete_frame_retires_cached_records() {
    let mut records = Records::default();
    decode(&mut records, &container(&packets(header(1, 1), &record(1, 5, 6, 1), 24), &[])).unwrap();
    assert_eq!(decode(&mut records, &container(&packets(header(2, 0), &[], 24), &[])).unwrap(),
        vec![header(2, 0).to_vec()]);
    let next = [record(3, 0, 6, 2), record(3, 1, 6, 3)].concat();
    assert_eq!(decode(&mut records, &container(&packets(header(3, 2), &next, 24), &[1])).unwrap(),
        vec![header(3, 1).to_vec(), record(3, 0, 6, 2)]);
}

#[test]
fn container_validates_everything_before_pushing() {
    let mut packet_a = header(1, 2).to_vec();
    packet_a.extend(record(1, 0, 2, 0));
    let packet_b = record(1, 1, 3, 0);
    let mut data = 2u32.to_le_bytes().to_vec();
    for packet in [&packet_a, &packet_b] {
        data.extend_from_slice(&(packet.len() as u32).to_le_bytes());
        data.extend_from_slice(packet);
    }
    let mut pushed = Vec::new();
    assert!(push_container(&data, |p| {
        pushed.push(p.to_vec());
        true
    }));
    assert_eq!(pushed, vec![packet_a.clone(), packet_b.clone()]);

    // A bad last packet must not let the first one through.
    let mut bad = data.clone();
    let tail = bad.len() - packet_b.len();
    bad[tail + 3] = 0x20; // Sequence 2 in the last block.
    let mut calls = 0;
    assert!(!push_container(&bad, |_| {
        calls += 1;
        true
    }));
    assert_eq!(calls, 0);

    assert!(!push_container(&[], |_| true));
    assert!(!push_container(&0u32.to_le_bytes(), |_| true));
    let mut trailing = data.clone();
    trailing.push(0);
    assert!(!push_container(&trailing, |_| true));
}
