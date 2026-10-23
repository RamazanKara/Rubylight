//! Row order (B10 step 3): the default order stays byte for byte, the row order is a permutation
//! that sends each picture band after what it needs.
use super::*;

/// FNV-1a over the bytes, to pin an output without storing it.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// Blocks per band and the bands of a frame, written out again here so the production table has
/// something to disagree with: (level, block columns, block rows) for each band in index order.
fn bands(width: u32, height: u32, chroma444: bool) -> Vec<(u32, u32, u32)> {
    let stride = width.div_ceil(32).max(4) * 32;
    let rows = height.div_ceil(32).max(4) * 32;
    let mut out = vec![];
    for level in (0..5).rev() {
        for component in 0..3 {
            if level == 0 && component != 0 && !chroma444 {
                continue;
            }
            for _band in 0..if level == 4 { 4 } else { 3 } {
                out.push((
                    level,
                    (stride >> (level + 1)).div_ceil(32),
                    (rows >> (level + 1)).div_ceil(32),
                ));
            }
        }
    }
    out
}

/// A frame holding every block of the picture, sizes from a fixed pseudo-random sequence:
/// mostly a few hundred bytes, one in `big_every` over a packet.
fn synthetic(width: u32, height: u32, chroma444: bool, seed: u64, big_every: u64) -> Vec<u8> {
    let small = if width > 2560 { 30 } else { 150 };
    let count: u32 = bands(width, height, chroma444)
        .iter()
        .map(|b| b.1 * b.2)
        .sum();
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    };
    let mut out = vec![];
    out.extend_from_slice(&(0x80000000u32 | (width - 1) | ((height - 1) << 14)).to_le_bytes());
    out.extend_from_slice(&(count | (u32::from(chroma444) << 26)).to_le_bytes());
    for index in 0..count {
        let words = if big_every > 0 && next() % big_every == 0 {
            360 + next() % 500
        } else {
            2 + next() % small
        } as u32;
        out.extend_from_slice(&(words << 16).to_le_bytes());
        out.extend_from_slice(&(index << 8).to_le_bytes());
        out.extend((0..words as usize * 4 - 8).map(|_| next() as u8));
    }
    out
}

/// (width, height, 4:4:4, seed, one big record in) for the fixtures below.
const FIXTURES: [(u32, u32, bool, u64, u64); 6] = [
    (1920, 1080, false, 1, 0),
    (1920, 1080, false, 2, 40),
    (1920, 1080, true, 3, 60),
    (128, 128, false, 4, 7),
    (1280, 720, true, 5, 0),
    (3840, 2160, false, 6, 90),
];

/// Taken from `record_frame` before row order existed: (length, FNV) per fixture and packet size.
const GOLDEN: [(usize, u64); 12] = [
    (1011956, 17246025799773005826),
    (1011840, 9883439662583209445),
    (1193896, 206574162747193321),
    (1194516, 1015539220477571806),
    (2197008, 1707875392606213186),
    (2198640, 4517381800307294656),
    (28832, 16009451025071511926),
    (28720, 9396680052049758406),
    (901748, 1451361694768643083),
    (901528, 11205461262102942000),
    (1185044, 10308366778531118838),
    (1185076, 2872509032596820030),
];

#[test]
fn default_order_is_byte_for_byte_what_it_was() {
    let mut got = vec![];
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        for packet_size in [1392, 1360] {
            let payload = aligned_payload(packet_size);
            let limit = max_frame_bytes(packet_size, true);
            let framed = record_frame(&frame, payload, limit).unwrap();
            assert_eq!(
                record_frame_with(&frame, payload, limit, false).unwrap(),
                framed
            );
            got.push((framed.len(), fnv(&framed)));
        }
    }
    assert_eq!(got, GOLDEN);
}

const PACKET: usize = 1392;

fn framed(frame: &[u8], rows: bool) -> Vec<u8> {
    record_frame_with(
        frame,
        aligned_payload(PACKET),
        max_frame_bytes(PACKET, true),
        rows,
    )
    .unwrap()
}

/// (index, packet it starts in) for every record, in sending order.
fn sent(frame: &[u8]) -> Vec<(u32, usize)> {
    let payload = aligned_payload(PACKET);
    records(frame)
        .unwrap()
        .iter()
        .map(|r| (r.index, (r.offset + 8) / payload))
        .collect()
}

fn padding_bytes(frame: &[u8]) -> usize {
    let (mut at, mut total) = (8, 0);
    while at < frame.len() {
        let header = word(frame, at);
        if header == PADDING {
            let size = 8 + word(frame, at + 4) as usize * 4;
            total += size;
            at += size;
        } else {
            at += ((header >> 16) & 0xfff) as usize * 4;
        }
    }
    total
}

#[test]
fn the_block_table_matches_the_coarse_count_and_the_independent_one() {
    for &(w, h, c, seed, _) in &FIXTURES {
        let table = blocks(w, h, c);
        let frame = synthetic(w, h, c, seed, 0);
        assert_eq!(
            table.len(),
            bands(w, h, c).iter().map(|b| b.1 * b.2).sum::<u32>() as usize
        );
        assert_eq!(
            table.iter().filter(|b| b.level == 4).count() as u32,
            coarse_count(&frame)
        );
        assert_eq!(table.len(), records(&frame).unwrap().len());
        let steps = row_steps(w, h, c);
        assert_eq!(steps.len(), table.len());
        let last = (h.div_ceil(32).max(4) * 32).div_ceil(64) as u16 - 1;
        assert!(steps.iter().all(|&s| s <= last));
        // The last band needs the last block row of level 0 and below, which sit one band earlier.
        assert_eq!(
            *steps.iter().max().unwrap(),
            last - 1,
            "{w}x{h}: the last step"
        );
    }
    // 1080p, the numbers in the design note: level 0 row r from band r-1, level 1 row r from 2r-1.
    let steps = row_steps(1920, 1080, false);
    let table = blocks(1920, 1080, false);
    let first = |level: u32, row: u32| {
        let at = table
            .iter()
            .position(|b| b.level == level && b.row == row)
            .unwrap();
        steps[at]
    };
    assert_eq!((first(0, 0), first(0, 1), first(0, 16)), (0, 0, 15));
    assert_eq!((first(1, 0), first(1, 1), first(1, 8)), (0, 1, 15));
    assert_eq!((first(2, 4), first(3, 2), first(4, 1)), (15, 15, 0));
}

#[test]
fn row_order_is_a_permutation_with_the_coarse_records_first() {
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        let (index, rows) = (framed(&frame, false), framed(&frame, true));
        assert_eq!(&rows[..8], &index[..8]);
        let mut a: Vec<_> = records(&index)
            .unwrap()
            .iter()
            .map(|r| (r.index, index[r.offset..r.offset + r.size].to_vec()))
            .collect();
        let mut b: Vec<_> = records(&rows)
            .unwrap()
            .iter()
            .map(|r| (r.index, rows[r.offset..r.offset + r.size].to_vec()))
            .collect();
        let order: Vec<u32> = b.iter().map(|r| r.0).collect();
        a.sort();
        b.sort();
        assert!(a == b, "{w}x{h}: the same records and bytes");
        let coarse = coarse_count(&frame);
        assert!(
            order[..coarse as usize].iter().all(|&i| i < coarse),
            "{w}x{h}: level 4 first"
        );
        assert_ne!(order, (0..order.len() as u32).collect::<Vec<_>>());
        // The critical FEC block is the same size, and the packet starts are valid.
        let payload = aligned_payload(PACKET);
        let (li, lr) = (
            layout(&index, payload).unwrap(),
            layout(&rows, payload).unwrap(),
        );
        assert!(
            lr.critical_shards > 0,
            "{w}x{h}: row order must keep the critical block"
        );
        assert_eq!(lr.critical_shards, li.critical_shards);
        assert!(lr.starts[0]);
    }
}

/// The last packet any record of step <= k starts in, for each k.
fn last_packet_by_step(steps: &[u16], placed: &[(u32, usize)]) -> Vec<usize> {
    let mut last = vec![0; *steps.iter().max().unwrap() as usize + 1];
    for &(index, packet) in placed {
        let k = steps[index as usize] as usize;
        last[k] = last[k].max(packet);
    }
    for k in 1..last.len() {
        last[k] = last[k].max(last[k - 1]);
    }
    last
}

#[test]
fn a_band_is_complete_no_later_than_one_packet_after_a_plain_sequential_packing() {
    let payload = aligned_payload(PACKET);
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        let steps = row_steps(w, h, c);
        let coarse = coarse_count(&frame);
        let out = framed(&frame, true);
        let coarse_end = records(&out)
            .unwrap()
            .iter()
            .filter(|r| r.index < coarse)
            .map(|r| r.offset + r.size)
            .max()
            .unwrap();
        // The row order laid out with no first-fit at all: sorted, a record that does not fit
        // opens the next packet, a record over a packet runs on.
        let mut fine: Vec<_> = records(&frame)
            .unwrap()
            .into_iter()
            .filter(|r| r.index >= coarse)
            .collect();
        fine.sort_by_key(|r| (steps[r.index as usize], r.index));
        let mut at = coarse_end + 8;
        let mut plain = vec![];
        for r in &fine {
            if r.size <= payload - 8 && at % payload + r.size > payload {
                at = at.next_multiple_of(payload);
            }
            plain.push((r.index, at / payload));
            at += r.size;
        }
        let fine_sent: Vec<_> = sent(&out).into_iter().filter(|r| r.0 >= coarse).collect();
        let (got, want) = (
            last_packet_by_step(&steps, &fine_sent),
            last_packet_by_step(&steps, &plain),
        );
        let worst = got
            .iter()
            .zip(&want)
            .map(|(g, w)| *g as i64 - *w as i64)
            .max()
            .unwrap();
        eprintln!(
            "{w}x{h} big 1/{big}: a band completes at most {worst} packets after the plain packing"
        );
        assert!(worst <= 1, "{w}x{h}: {got:?} against {want:?}");
    }
}

#[test]
fn a_lost_tail_takes_the_last_rows_whole_in_row_order() {
    let payload = aligned_payload(PACKET);
    for &(w, h, c, seed, big) in &FIXTURES[..3] {
        let frame = synthetic(w, h, c, seed, big);
        let (steps, table) = (row_steps(w, h, c), blocks(w, h, c));
        let last = *steps.iter().max().unwrap();
        for lost_percent in [5usize, 20] {
            let mut report = vec![];
            for rows in [false, true] {
                let out = framed(&frame, rows);
                let cut = (out.len() + 8).div_ceil(payload) * (100 - lost_percent) / 100;
                let all = records(&out).unwrap();
                let lost = |r: &Record| (r.offset + r.size - 1 + 8) / payload >= cut;
                // The bands whose records all arrived, counting from the top.
                let first_lost = all
                    .iter()
                    .filter(|r| lost(r))
                    .map(|r| steps[r.index as usize])
                    .min()
                    .unwrap();
                let level = |keep: fn(u32) -> bool| {
                    all.iter()
                        .filter(|r| lost(r) && keep(table[r.index as usize].level))
                        .count()
                };
                report.push((
                    first_lost,
                    level(|l| l >= 1),
                    all.iter().filter(|r| lost(r)).count(),
                ));
                if rows {
                    assert!(
                        u32::from(first_lost) * 100 + 10 * u32::from(last)
                            >= u32::from(last) * (100 - lost_percent as u32),
                        "{w}x{h}: {first_lost} of {last} at {lost_percent}%"
                    );
                    assert!(
                        level(|l| l == 4) == 0,
                        "{w}x{h}: the critical records are in the first packets"
                    );
                } else {
                    // Index order loses the finest level first, from the bottom, and nothing above it
                    // at a small loss.
                    assert!(lost_percent > 5 || level(|l| l >= 1) == 0, "{w}x{h}");
                }
            }
            eprintln!(
                "{w}x{h} lost tail {lost_percent}%: (first band hit, records above level 0, records) index {:?}, rows {:?}",
                report[0], report[1]
            );
        }
    }
}

#[test]
fn row_order_costs_no_bytes_to_speak_of() {
    let payload = aligned_payload(PACKET);
    let mut report = String::new();
    for &(w, h, c, seed, big) in &FIXTURES {
        let frame = synthetic(w, h, c, seed, big);
        let (index, rows) = (framed(&frame, false), framed(&frame, true));
        let (pi, pr) = (
            (index.len() + 8).div_ceil(payload),
            (rows.len() + 8).div_ceil(payload),
        );
        report += &format!(
            "{w}x{h} {} big 1/{big}: index {} B in {pi} packets, padding {} B | rows {} B in {pr} packets, padding {} B\n",
            if c { "444" } else { "420" },
            index.len(),
            padding_bytes(&index),
            rows.len(),
            padding_bytes(&rows)
        );
        assert!(
            pr * 100 <= pi * 101 + 200 && pi * 100 <= pr * 101 + 200,
            "{report}"
        );
        assert!(
            rows.len().abs_diff(index.len()) <= index.len() / 100 + 2 * PACKET,
            "{report}"
        );
    }
    eprintln!("{report}");
}

#[test]
fn a_block_index_outside_the_picture_is_refused_in_row_order_only() {
    let frame = synthetic(128, 128, false, 9, 0);
    let mut bad = frame.clone();
    let at = records(&frame).unwrap().last().unwrap().offset;
    bad[at + 4..at + 8].copy_from_slice(&(1_000_000u32 << 8).to_le_bytes());
    let payload = aligned_payload(PACKET);
    assert!(record_frame_with(&bad, payload, max_frame_bytes(PACKET, true), true).is_err());
    assert!(record_frame_with(&bad, payload, max_frame_bytes(PACKET, true), false).is_ok());
}

#[test]
fn the_packetizer_takes_a_row_ordered_frame_with_the_same_fec_blocks() {
    use crate::packet::{PyrowaveFec, VideoPacketizer};
    for &(w, h, c, seed, big) in &FIXTURES[..3] {
        let frame = synthetic(w, h, c, seed, big);
        let mut counts = vec![];
        for rows in [false, true] {
            let out = framed(&frame, rows);
            let mut p = VideoPacketizer {
                sequence: 0,
                iv_counter: 0,
                frame: 1,
                packet_size: PACKET,
                fec_percent: 0,
                min_fec: 2,
                key: None,
            };
            let fec = PyrowaveFec {
                records: true,
                critical_percentage: 20,
                detail_percentage: 0,
                wire_budget: 0,
                ipv6: false,
            };
            let (total, _, blocks) = p.pyrowave_blocks(&out, 0, 0, fec).unwrap();
            let blocks = blocks.collect::<Result<Vec<_>>>().unwrap();
            assert_eq!(blocks.iter().map(|b| b.len()).sum::<usize>(), total);
            counts.push((blocks.len(), total));
        }
        assert_eq!(counts[0].0, counts[1].0, "{w}x{h}: FEC blocks");
        assert!(
            counts[0].1.abs_diff(counts[1].1) <= counts[0].1 / 100 + 3,
            "{w}x{h}: {counts:?}"
        );
    }
}

#[test]
fn the_cached_steps_follow_the_frame_geometry() {
    for _ in 0..2 {
        for &(w, h, c, seed, big) in &FIXTURES {
            let frame = synthetic(w, h, c, seed, big);
            assert_eq!(*cached_row_steps(&frame), row_steps(w, h, c), "{w}x{h}");
        }
    }
}
