use super::*;
use alloc::vec::Vec;

fn fnv(steps: &[u16]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for step in steps {
        for byte in step.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

fn counts(steps: &[u16]) -> Vec<usize> {
    let mut counts = alloc::vec![0usize; steps.iter().copied().max().unwrap_or(0) as usize + 1];
    for &step in steps {
        counts[usize::from(step)] += 1;
    }
    counts
}

/// Taken from the host: `butterpollo_core::pyrowave::row_steps` of rust/core/src/pyrowave.rs at
/// Rubylight 682aa119b, printed with its length, a 64 bit FNV-1a of the u16 values (little
/// endian) and the number of blocks at each step.
#[test]
fn the_steps_match_the_hosts_for_1080p_and_4k_in_420_and_444() {
    // Width, height, 4:4:4, step count, FNV-1a of the steps, blocks per step.
    type Case = (u32, u32, bool, usize, u64, &'static [usize]);
    let cases: [Case; 6] = [
        (1920, 1080, false, 3261, 0x7f17_27e1_99d6_d3cd,
         &[471, 225, 90, 297, 90, 225, 90, 333, 90, 225, 90, 297, 90, 225, 90, 333]),
        (1920, 1080, true, 6321, 0x8e1e_9992_fe5f_d76d,
         &[831, 405, 270, 477, 270, 405, 270, 513, 270, 405, 270, 477, 270, 405, 270, 513]),
        (3840, 2160, false, 12429, 0x26f1_6256_100d_042d,
         &[981, 450, 180, 585, 180, 450, 180, 657, 180, 450, 180, 585, 180, 450, 180, 657, 180, 450, 180, 585, 180, 450,
           180, 657, 180, 450, 180, 585, 180, 450, 180, 657, 180]),
        (3840, 2160, true, 24669, 0xf879_f910_4a8c_d42d,
         &[1701, 810, 540, 945, 540, 810, 540, 1017, 540, 810, 540, 945, 540, 810, 540, 1017, 540, 810, 540, 945, 540,
           810, 540, 1017, 540, 810, 540, 945, 540, 810, 540, 1017, 540]),
        (1280, 720, false, 1473, 0xd061_5c10_113e_fbf6, &[306, 150, 60, 195, 60, 150, 60, 222, 60, 150, 60]),
        (1000, 500, true, 1542, 0x74bc_cc4b_af3a_4f05, &[426, 216, 144, 252, 144, 216, 144]),
    ];
    for (width, height, chroma444, len, hash, expected) in cases {
        let steps = row_steps(width, height, chroma444);
        assert_eq!(steps.len(), len, "{width}x{height} 444={chroma444}");
        assert_eq!(counts(&steps), expected, "{width}x{height} 444={chroma444}");
        assert_eq!(fnv(&steps), hash, "{width}x{height} 444={chroma444}");
    }
}

#[test]
fn a_few_steps_are_spot_checked_by_hand() {
    // 1080p 4:2:0: 17 bands. The coarse level (48 blocks) is step 0, then level 3 (2x1 rows) ...
    let steps = row_steps(1920, 1080, false);
    assert_eq!(band_count(1080), 17);
    assert!(steps[..48].iter().all(|&s| s == 0));
    assert_eq!(*steps.iter().max().unwrap(), 15, "the last band needs no block of its own");
    // The first block of every band of a level and component: row 0 is step 0, row r of level L is
    // step r * 2^L - 1.
    let level0_luma = steps.len() - 3 * 30 * 17; // level 0 luma is the tail: 3 bands of 30x17 blocks
    let tail = &steps[level0_luma..];
    assert_eq!(tail.len(), 3 * 30 * 17);
    for band in 0..3 {
        for row in 0..17usize {
            let step = tail[band * 510 + row * 30];
            assert_eq!(usize::from(step), row.saturating_sub(1).min(16), "level 0 row {row}");
        }
    }
}

#[test]
fn slices_cover_the_bands_and_never_come_out_empty() {
    assert_eq!(slice_ends(17, 4), [5, 9, 13, 17]);
    assert_eq!(slice_ends(34, 3), [12, 23, 34]);
    assert_eq!(slice_ends(4, 6), [1, 2, 3, 4]);
    assert_eq!(slice_ends(17, 0), [17]);
    assert_eq!(band_rows(1080, 0, 5), (0, 320));
    assert_eq!(band_rows(1080, 13, 17), (832, 1080));
}

/// The records of a frame in row order (by step, index inside a step), in packets of `per_packet`.
fn row_ordered(steps: &[u16]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..steps.len() as u32).collect();
    order.sort_by_key(|&i| (steps[i as usize], i));
    order
}

fn feed(tracker: &mut RowTracker, records: &[u32], per_packet: usize) {
    tracker.begin_frame();
    for packet in records.chunks(per_packet) {
        tracker.packet();
        for &index in packet {
            tracker.record(index);
        }
    }
    tracker.finish();
}

#[test]
fn a_row_ordered_frame_passes_and_an_index_ordered_frame_fails() {
    for chroma444 in [false, true] {
        let mut tracker = RowTracker::new(1920, 1080, chroma444);
        let steps = row_steps(1920, 1080, chroma444);
        feed(&mut tracker, &row_ordered(&steps), 12);
        let report = tracker.report();
        assert_eq!((report.records as usize, report.violations), (steps.len(), 0));
        assert!(report.ordered());
        let index_order: Vec<u32> = (0..steps.len() as u32).collect();
        feed(&mut tracker, &index_order, 12);
        assert!(tracker.report().violations > 100, "{:?}", tracker.report());
        assert!(!tracker.report().ordered());
    }
}

#[test]
fn a_record_one_packet_late_passes_and_two_packets_late_is_a_violation() {
    let steps = row_steps(1920, 1080, false);
    let order = row_ordered(&steps);
    let mut tracker = RowTracker::new(1920, 1080, false);
    // Take a step-1 record and send it one packet after the first step-2 record: in place.
    let moved = |packets_late: usize| {
        let mut order = order.clone();
        let from = order.iter().position(|&i| steps[i as usize] == 1).unwrap();
        let record = order.remove(from);
        let first_two = order.iter().position(|&i| steps[i as usize] == 2).unwrap();
        // Packets of 12: land `packets_late` packets after the one that holds the first step-2 record.
        let at = ((first_two / 12) + packets_late) * 12 + 1;
        order.insert(at, record);
        order
    };
    feed(&mut tracker, &moved(1), 12);
    assert_eq!(tracker.report().violations, 0, "one packet late is the host's packing");
    feed(&mut tracker, &moved(3), 12);
    assert_eq!(tracker.report().violations, 1);
    assert!(tracker.report().ordered(), "one stray record does not make the frame unordered");
    // Two strays do.
    let mut order = moved(3);
    let from = order.iter().position(|&i| steps[i as usize] == 3).unwrap();
    let record = order.remove(from);
    order.insert(order.len() - 20, record);
    feed(&mut tracker, &order, 12);
    assert_eq!(tracker.report().violations, 2);
    assert!(!tracker.report().ordered());
}

#[test]
fn bands_complete_when_a_later_step_has_come_and_a_packet_has_passed() {
    let steps = row_steps(1920, 1080, false);
    let order = row_ordered(&steps);
    let mut tracker = RowTracker::new(1920, 1080, false);
    tracker.begin_frame();
    assert_eq!(tracker.complete_bands(), 0);
    let mut seen = Vec::new();
    for (n, packet) in order.chunks(12).enumerate() {
        tracker.packet();
        for &index in packet {
            tracker.record(index);
        }
        let complete = tracker.complete_bands();
        seen.push(complete);
        // Exactness: every record of a step below `complete` is in.
        let arrived = &order[..((n + 1) * 12).min(order.len())];
        let got = arrived.iter().filter(|&&i| u32::from(steps[i as usize]) < complete).count();
        let all = steps.iter().filter(|&&s| u32::from(s) < complete).count();
        assert_eq!(got, all, "{complete} bands complete but records of their steps are missing");
    }
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "bands never un-complete");
    assert!(seen[0] == 0 && *seen.last().unwrap() >= 14, "{seen:?}");
    assert!(*seen.last().unwrap() < 17, "the last steps wait for the frame to end: {seen:?}");
    tracker.finish();
    assert_eq!(tracker.complete_bands(), 17);
    // A new frame starts again.
    tracker.begin_frame();
    assert_eq!(tracker.complete_bands(), 0);
    assert_eq!(tracker.report(), RowReport::default());
}

#[test]
fn an_index_outside_the_picture_is_a_violation_and_a_fresh_tracker_has_no_report() {
    let mut tracker = RowTracker::new(128, 128, false);
    assert!(!tracker.report().ordered());
    tracker.begin_frame();
    tracker.packet();
    tracker.record(1_000_000);
    assert_eq!(tracker.report().violations, 1);
}

#[test]
fn records_pulled_far_forward_to_fill_packets_do_not_make_a_frame_unordered() {
    // The host's first-fit packing puts a later record into the room a packet has left, so the
    // lowest step of each packet is what follows the row order, not each record.
    let steps = row_steps(1920, 1080, false);
    let order = row_ordered(&steps);
    let (mut main, mut late): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
    for (n, &index) in order.iter().enumerate() {
        if steps[index as usize] >= 3 && n % 3 == 0 { late.push(index) } else { main.push(index) }
    }
    // Every packet of 12: ten records in order and two pulled forward from far ahead.
    let mut mixed = Vec::new();
    let mut pulled = late.iter().copied();
    for packet in main.chunks(10) {
        mixed.extend_from_slice(packet);
        mixed.extend(pulled.by_ref().take(2));
    }
    mixed.extend(pulled);
    let mut tracker = RowTracker::new(1920, 1080, false);
    feed(&mut tracker, &mixed, 12);
    let report = tracker.report();
    assert_eq!((report.records as usize, report.violations), (steps.len(), 0), "{report:?}");
    assert!(report.ordered());
}

#[test]
fn a_gap_in_the_packets_makes_the_frame_unordered() {
    let steps = row_steps(1920, 1080, false);
    let order = row_ordered(&steps);
    let mut tracker = RowTracker::new(1920, 1080, false);
    feed(&mut tracker, &order, 12);
    assert!(tracker.report().ordered());
    tracker.begin_frame();
    tracker.packet();
    tracker.gap();
    tracker.record(order[0]);
    assert_eq!(tracker.report().gaps, 1);
    assert!(!tracker.report().ordered(), "a lost packet is not row order");
    tracker.begin_frame();
    assert_eq!(tracker.report(), RowReport::default());
}
