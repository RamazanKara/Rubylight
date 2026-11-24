//! Row-order prefix reconstruction, `SlicePlan`, and beam-race state machine tests.

use super::*;

/// The wavelet level of every band of the decoder, in `bands` order (the loops of `new`).
fn band_levels(decoder: &Decoder) -> Vec<usize> {
    let mut levels = Vec::new();
    for level in (0..5).rev() {
        for component in 0..3 {
            if level == 0 && component != 0 && !decoder.chroma444 { continue; }
            for _ in 0..if level == 4 { 4 } else { 3 } { levels.push(level); }
        }
    }
    assert_eq!(levels.len(), decoder.bands.len());
    levels
}

/// The negative control's step: `row_steps` without its one band of margin. Level 4 is sent
/// first of all.
fn unmargined_step(level: usize, row: usize, bands: usize) -> usize {
    if level == 4 { 0 } else { (row << level).min(bands - 1) }
}

/// For each picture band k: decode only the blocks the row order sends before band k's cut
/// (every block whose step is at most k, the planes as they stand being the full frame's
/// blocks), and the largest difference to the full decode over band k's rows, in the decoder's
/// float units (a code value is 1/255 at 8 bits). A copy of the planes is zeroed where blocks
/// are missing, as the decoder leaves a block that did not arrive.
fn prefix_errors(decoder: &mut Decoder, lookahead: usize) -> Vec<f32> {
    let (stride, rows, full) = (decoder.stride, decoder.rows, decoder.chroma444);
    let bands = rows.div_ceil(64);
    let whole = decoder.planes.clone();
    let mut reference = whole.clone();
    let reconstruct = |planes: &mut [Vec<f32>; 3], decoder: &mut Decoder| {
        std::mem::swap(&mut decoder.planes, planes);
        decoder.reconstruct();
        std::mem::swap(&mut decoder.planes, planes);
    };
    reconstruct(&mut reference, decoder);
    let levels = band_levels(decoder);
    // The steps the host and the client share, in the decoder's block order.
    let steps = rubylight_protocol::pyrowave::rows::row_steps(stride as u32, rows as u32, full);
    assert_eq!(steps.len(), decoder.bands.iter().map(|band| band.count).sum::<usize>());
    let mut errors = Vec::new();
    for k in 0..bands {
        let mut planes = whole.clone();
        let mut index = 0;
        for (band, &level) in decoder.bands.iter().zip(&levels) {
            let across = band.width.div_ceil(32);
            for block in 0..band.count {
                let row = block / across;
                let step = if lookahead == 1 { usize::from(steps[index]) } else { unmargined_step(level, row, bands) };
                index += 1;
                if step <= k { continue; }
                let (bx, by) = (block % across * 32, row * 32);
                for y in by..(by + 32).min(band.height) {
                    let at = (band.y + y) * stride + band.x + bx;
                    planes[band.component][at..at + (bx + 32).min(band.width) - bx].fill(0.0);
                }
            }
        }
        reconstruct(&mut planes, decoder);
        let mut error = 0.0f32;
        for (component, (got, want)) in planes.iter().zip(&reference).enumerate() {
            // A 4:2:0 chroma plane has half the picture's rows.
            let scale = if component != 0 && !full { 2 } else { 1 };
            let (first, last) = (64 * k / scale, (64 * (k + 1) / scale).min(rows / scale));
            for y in first..last {
                for x in 0..stride / scale {
                    error = error.max((got[y * stride + x] - want[y * stride + x]).abs());
                }
            }
        }
        errors.push(error);
    }
    errors
}

/// Random coefficients in every block of a fresh decoder's planes.
fn random_pyramid(width: usize, height: usize, full: bool) -> Decoder {
    let mut decoder = Decoder::new(width, height, full).unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for plane in &mut decoder.planes {
        for value in plane.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *value = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
        }
    }
    decoder
}

#[test]
fn a_band_decodes_exactly_from_the_blocks_a_row_order_sends_before_its_cut() {
    // The inverse wavelet's four +-1 lifting steps reach under one block row at every level, so
    // block row r of level L is needed from band r * 2^L - 1 on: one band of lookahead.
    for (width, height, full) in [(1920, 1080, false), (1280, 720, true), (131, 97, true), (132, 98, false)] {
        let mut decoder = random_pyramid(width, height, full);
        let one = prefix_errors(&mut decoder, 1);
        println!("{width}x{height} 444={full}, one band of lookahead, max error per band: {one:?}");
        assert!(one.iter().all(|&e| e == 0.0), "{width}x{height}: {one:?}");
        // The negative control: no lookahead is wrong in every band but the last.
        let none = prefix_errors(&mut decoder, 0);
        println!("{width}x{height} 444={full}, no lookahead: {none:?}");
        if one.len() > 1 { assert!(none[..none.len() - 1].iter().any(|&e| e > 0.0), "{width}x{height}: {none:?}"); }
    }
}

/// Feeds `order` (block indices) to a plan `per_packet` records at a time, as the packets of a
/// frame, and returns the slices that came due, with the packet each came due in.
fn run_plan(plan: &mut SlicePlan, order: &[u32], per_packet: usize) -> Vec<(usize, Due)> {
    plan.begin_frame();
    let mut dues = Vec::new();
    let packets = order.chunks(per_packet).count();
    for (i, packet) in order.chunks(per_packet).enumerate() {
        plan.packet();
        for &index in packet { plan.record(index); }
        if i + 1 == packets { plan.finish(); }
        if let Some(due) = plan.due() { dues.push((i, due)); }
    }
    dues
}

#[test]
fn a_row_ordered_frame_comes_due_slice_by_slice_before_it_ends() {
    let (width, height) = (1920u32, 1080u32);
    let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
    let mut ordered: Vec<u32> = (0..steps.len() as u32).collect();
    ordered.sort_by_key(|&i| (steps[i as usize], i));
    let mut plan = SlicePlan::new(width, height, false, 4);
    let packets = ordered.chunks(40).count();
    let dues = run_plan(&mut plan, &ordered, 40);
    assert!(plan.report().ordered(), "{:?}", plan.report());
    assert_eq!(dues.len(), 3, "four slices, the last being the final decode: {dues:?}");
    assert!(dues.iter().enumerate().all(|(n, (_, due))| due.slice == n), "{dues:?}");
    assert!(dues.windows(2).all(|pair| pair[0].0 < pair[1].0), "each in a later packet: {dues:?}");
    assert!(dues.iter().all(|&(packet, _)| packet + 1 < packets), "all before the frame ends");
    // The slices tile the picture from the top, and the final decode takes the rest.
    assert_eq!(dues[0].1.first_row, 0);
    assert!(dues.windows(2).all(|pair| pair[0].1.end_row == pair[1].1.first_row));
    assert_eq!(plan.rest(), (dues[2].1.end_row, height));
    assert!(dues[2].1.end_row < height);
}

/// The order of a row-ordered frame with its first record (step 0) delivered `late` packets
/// after the packet slice 0 comes due in.
fn order_with_a_late_record(width: u32, height: u32, late: usize) -> Vec<u32> {
    let ordered = row_order(width, height);
    let mut plan = SlicePlan::new(width, height, false, 4);
    let first_due = run_plan(&mut plan, &ordered, 40)[0].0;
    let mut order = ordered;
    let record = order.remove(0);
    order.insert((first_due + late) * 40, record);
    order
}

#[test]
fn one_record_two_packets_late_after_slice_zero_stops_slicing_and_is_not_exact() {
    let (width, height) = (1920u32, 1080u32);
    let order = order_with_a_late_record(width, height, 2);
    let mut plan = SlicePlan::new(width, height, false, 4);
    let dues = run_plan(&mut plan, &order, 40);
    // The frame is still ordered by the counter's tolerance (one violation) ...
    assert_eq!(plan.report().violations, 1, "{:?}", plan.report());
    assert!(plan.report().ordered(), "{:?}", plan.report());
    // ... but slice 0 came due before the record that belongs in it, and no slice follows.
    assert_eq!(dues.len(), 1, "{dues:?}");
    assert_eq!(dues[0].1.slice, 0);
}

#[test]
fn an_index_ordered_frame_is_reported_unordered_and_stops_slicing() {
    let (width, height) = (1920u32, 1080u32);
    let count = rubylight_protocol::pyrowave::rows::row_steps(width, height, false).len() as u32;
    let index_order: Vec<u32> = (0..count).collect();
    let mut plan = SlicePlan::new(width, height, false, 4);
    let dues = run_plan(&mut plan, &index_order, 40);
    assert!(!plan.report().ordered(), "{:?}", plan.report());
    assert!(plan.due().is_none(), "no slice is offered once the order is broken");
    // Whatever came due before the first violation was seen is a prefix of the picture.
    assert!(dues.len() < 3, "{dues:?}");
    // The next frame starts clean: it is the same plan, reset.
    let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
    let mut ordered: Vec<u32> = (0..count).collect();
    ordered.sort_by_key(|&i| (steps[i as usize], i));
    run_plan(&mut plan, &ordered, 40);
    assert!(plan.report().ordered());
}

fn row_order(width: u32, height: u32) -> Vec<u32> {
    let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
    let mut ordered: Vec<u32> = (0..steps.len() as u32).collect();
    ordered.sort_by_key(|&i| (steps[i as usize], i));
    ordered
}

#[test]
fn a_lost_packet_stops_slicing_and_makes_the_frame_unordered() {
    let (width, height) = (1920u32, 1080u32);
    let ordered = row_order(width, height);
    let mut plan = SlicePlan::new(width, height, false, 4);
    plan.begin_frame();
    let mut dues = 0;
    for (i, packet) in ordered.chunks(40).enumerate() {
        if i == 1 { plan.gap(); }
        plan.packet();
        for &index in packet { plan.record(index); }
        dues += usize::from(plan.due().is_some());
    }
    assert_eq!(dues, 0, "no slice after a gap, although the records themselves are in order");
    assert!(!plan.report().ordered(), "{:?}", plan.report());
    // The next frame starts clean.
    assert_eq!(run_plan(&mut plan, &ordered, 40).len(), 3);
}

#[test]
fn the_final_decode_gate_judges_a_sliced_frame_by_its_loss_and_any_other_by_the_sdk() {
    let ready = || -> bool { panic!("a sliced frame never asks the SDK: its slice decodes cleared the flag") };
    assert_eq!(final_decode_gate(true, true, 0.0, ready), Ok(()));
    assert_eq!(final_decode_gate(true, true, SLICED_LOSS_LIMIT_PERCENT, ready), Ok(()), "the limit itself passes");
    assert!(final_decode_gate(true, true, SLICED_LOSS_LIMIT_PERCENT + 0.1, ready).unwrap_err().contains("lost too much"));
    // Slices decoded but the chunks were not the whole envelope: the SDK's gate decides.
    assert_eq!(final_decode_gate(true, false, 50.0, || true), Ok(()));
    assert!(final_decode_gate(true, false, 0.0, || false).unwrap_err().contains("no picture data"));
    // No slices: the SDK's gate, whatever the loss.
    assert_eq!(final_decode_gate(false, true, 50.0, || true), Ok(()));
    assert!(final_decode_gate(false, true, 0.0, || false).is_err());
}

#[cfg(windows)]
fn race(width: u32, height: u32) -> Race {
    Race {
        plan: SlicePlan::new(width, height, false, 4),
        header_pushed: false,
        lease: None,
        bands: Vec::new(),
        failure: None,
        no_plane: false,
        ordered_run: ORDERED_FRAMES_TO_RESUME,
        slicing: true,
        seq: 0,
        events: Vec::new(),
    }
}

/// Feeds a frame's records (40 a packet) to the race's plan, and finishes it as
/// `GpuStream::frame` does, with a slice recorded for each one that came due.
#[cfg(windows)]
fn race_frame(race: &mut Race, order: &[u32], chunks: bool) -> (RaceReport, Vec<BandReady>) {
    race.begin_frame();
    for packet in order.chunks(40) {
        race.plan.packet();
        for &index in packet { race.plan.record(index); }
        if race.slicing
            && let Some(due) = race.plan.due() {
                race.bands.push(BandReady { slice: due.slice as u8, first_row: due.first_row, end_row: due.end_row, fence_value: 1, ready_us: 10 });
            }
    }
    race.plan.finish();
    race.finish(2, 20, chunks)
}

#[cfg(windows)]
#[test]
fn an_unordered_frame_has_no_slices_and_slicing_waits_for_ordered_frames() {
    let (width, height) = (1920u32, 1080u32);
    let ordered = row_order(width, height);
    // In order for the first half, so slices come due before the break is seen, then reversed.
    let mut broken = ordered.clone();
    let tail = broken.len() / 2;
    broken[tail..].reverse();
    let mut race = race(width, height);
    let (report, bands) = race_frame(&mut race, &ordered, true);
    assert_eq!((report.ordered, report.slices), (RaceReport::ORDERED, 4));
    assert_eq!(bands.len(), 4);
    assert_eq!(bands.last().map(|b| b.end_row), Some(height));
    // Slices came due on the first half, which is in order; the frame then left the order.
    let (report, bands) = race_frame(&mut race, &broken, true);
    assert_eq!(report.ordered, RaceReport::UNORDERED);
    assert_eq!((report.slices, bands.len()), (1, 1), "the slices decoded before the break are not marked exact");
    assert_eq!((bands[0].first_row, bands[0].end_row), (0, height));
    // Quiet: ordered frames do not slice until ORDERED_FRAMES_TO_RESUME of them in a row.
    for n in 0..ORDERED_FRAMES_TO_RESUME {
        let (report, bands) = race_frame(&mut race, &ordered, true);
        assert_eq!(report.ordered, RaceReport::ORDERED);
        assert_eq!(bands.len(), 1, "ordered frame {n} after the break is still decoded whole");
    }
    let (_, bands) = race_frame(&mut race, &ordered, true);
    assert_eq!(bands.len(), 4, "slicing resumed");
    // A break in the middle of the run starts it over.
    race_frame(&mut race, &broken, true);
    race_frame(&mut race, &ordered, true);
    race_frame(&mut race, &broken, true);
    for _ in 0..ORDERED_FRAMES_TO_RESUME - 1 { race_frame(&mut race, &ordered, true); }
    assert_eq!(race_frame(&mut race, &ordered, true).1.len(), 1, "one frame short of the run");
    assert_eq!(race_frame(&mut race, &ordered, true).1.len(), 4);
}

#[cfg(windows)]
#[test]
fn a_late_record_after_a_slice_falls_the_frame_back_to_one_whole_decode() {
    let (width, height) = (1920u32, 1080u32);
    let late = order_with_a_late_record(width, height, 2);
    let mut race = race(width, height);
    let (report, bands) = race_frame(&mut race, &late, true);
    assert_eq!(report.ordered, RaceReport::UNORDERED, "slice 0 lacks the late record");
    assert_eq!((report.slices, bands.len()), (1, 1));
    assert_eq!((bands[0].first_row, bands[0].end_row), (0, height));
    // The run of ordered frames starts over: the next ordered frame is not sliced.
    let (report, bands) = race_frame(&mut race, &row_order(width, height), true);
    assert_eq!((report.ordered, bands.len()), (RaceReport::ORDERED, 1));
}

#[cfg(windows)]
#[test]
fn a_late_record_before_any_slice_still_counts_as_ordered() {
    let (width, height) = (1920u32, 1080u32);
    // A step 0 record delivered three packets after the cursor left step 0, but before slice 0
    // is due: late, with no slice yet to lack it.
    let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
    let mut order = row_order(width, height);
    let left_step_0 = order.iter().position(|&i| steps[i as usize] > steps[order[0] as usize]).unwrap() / 40;
    let first_due = run_plan(&mut SlicePlan::new(width, height, false, 4), &order, 40)[0].0;
    assert!(left_step_0 + 3 < first_due, "{left_step_0} {first_due}");
    let record = order.remove(0);
    order.insert((left_step_0 + 3) * 40, record);
    let mut race = race(width, height);
    let (report, bands) = race_frame(&mut race, &order, true);
    assert_eq!(race.plan.report().violations, 1);
    assert_eq!(report.ordered, RaceReport::ORDERED);
    // Slicing stopped at the violation, so the one entry is the whole picture.
    assert_eq!((bands.len(), bands[0].first_row, bands[0].end_row), (1, 0, height));
    let (report, bands) = race_frame(&mut race, &row_order(width, height), true);
    assert_eq!((report.ordered, bands.len()), (RaceReport::ORDERED, 4), "the run was not reset");
}

#[cfg(windows)]
#[test]
fn a_frame_without_chunks_is_one_slice_of_the_whole_picture() {
    let (width, height) = (1920u32, 1080u32);
    let mut race = race(width, height);
    race_frame(&mut race, &row_order(width, height), true);
    // A frame that came whole never began: the plan still holds the last frame's slices, and
    // the final decode must not take its rows from them.
    assert!(race.plan.rest().0 > 0);
    let (report, bands) = race.finish(3, 30, false);
    assert_eq!(report, RaceReport::default());
    assert_eq!(bands, [BandReady { slice: 0, first_row: 0, end_row: height, fence_value: 3, ready_us: 30 }]);
}

#[test]
#[ignore = "real 1080p frames from NIGHTJAR_PYROWAVE_FRAMES"]
fn a_real_frame_decodes_its_bands_exactly_from_the_blocks_before_their_cut() {
    let Some(dir) = crate::sdl::test_input("NIGHTJAR_PYROWAVE_FRAMES", "a NIGHTJAR_PYROWAVE_DUMP directory") else { return };
    for (width, height, full, data) in super::tests::dumped_frames(std::path::Path::new(&dir)).into_iter().take(3) {
        let mut decoder = Decoder::new(width, height, full).unwrap();
        decoder.coefficients(&data).unwrap();
        for lookahead in [1, 0] {
            let errors = prefix_errors(&mut decoder, lookahead);
            println!("real {width}x{height} 444={full}, lookahead {lookahead}, max error per band: {errors:?}");
            if lookahead == 1 { assert!(errors.iter().all(|&e| e == 0.0), "{errors:?}"); }
        }
    }
}
