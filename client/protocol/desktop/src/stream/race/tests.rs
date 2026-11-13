use super::*;

#[test]
fn frame_numbers_only_grow_so_a_rebuilt_decoder_cannot_look_older_than_the_screen() {
    // Two decoders' frames, interleaved the way a rebuild would: the second never restarts at 1,
    // which `newer_on_screen` would read as an older picture and drop for ever.
    let first: Vec<u64> = (0..5).map(|_| next_seq()).collect();
    let second: Vec<u64> = (0..5).map(|_| next_seq()).collect();
    assert!(first.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(first.last() < second.first());
    let mut racer = Racer::<u8> { seq: *first.last().unwrap(), ..Racer::default() };
    racer.presented(1_000, &scan());
    assert!(racer.newer_on_screen(first[0]));
    assert!(!racer.newer_on_screen(second[0]));
}

/// 1080 rows, 3.2 us per row (3.46 ms a scan), the first row 800 us after a present.
fn scan() -> Scan { Scan { rows: 1080, row_ns: 3200.0, delta_us: 800 } }

fn band(slice: u8, first_row: u32, end_row: u32, ready_us: u64) -> BandReady {
    BandReady { slice, first_row, end_row, fence_value: u64::from(slice) + 1, ready_us }
}

#[test]
fn consecutive_slices_of_a_set_are_drawn_in_one_pass() {
    let bands = [band(0, 0, 270, 0), band(1, 270, 540, 0), band(2, 540, 810, 0)];
    let one = runs(bands.iter().map(|b| (*b, 3)));
    assert_eq!(one, vec![Run { first_row: 0, end_row: 810, fence_value: 3, last: 2 }]);
    // A gap in the rows, or another plane set, starts a new pass.
    let gap = runs([(bands[0], 3), (bands[2], 3)].into_iter());
    assert_eq!(gap.len(), 2);
    let sets = runs([(bands[0], 3), (bands[1], 4), (bands[2], 4)].into_iter());
    assert_eq!(sets, vec![
        Run { first_row: 0, end_row: 270, fence_value: 1, last: 0 },
        Run { first_row: 270, end_row: 810, fence_value: 3, last: 2 },
    ]);
    assert!(runs(std::iter::empty()).is_empty());
}

/// Four slices of a frame whose first slice is ready at `at` and whose last is ready `span` later.
fn frame(at: u64, span: u64) -> Vec<BandReady> {
    let rows = [(0, 270), (270, 540), (540, 810), (810, 1080)];
    rows.iter().enumerate().map(|(i, &(a, b))| band(i as u8, a, b, at + span * i as u64 / 3)).collect()
}

fn learned(span: u64) -> Predictor {
    let mut predictor = Predictor::seeded(4, 1080, 0.0, 0);
    predictor.learn(&frame(0, span));
    predictor
}

fn quiet() -> Latency { Latency::fixed(100, 500, 100) }

#[test]
fn a_scissor_is_the_rows_of_the_slice_inside_the_picture() {
    assert_eq!(scissor(0, 270, 1080), Some((0, 270)));
    assert_eq!(scissor(810, 1080, 1080), Some((810, 1080)));
    assert_eq!(scissor(810, 5000, 1080), Some((810, 1080)), "rows past the picture are cut");
    assert_eq!(scissor(1080, 1080, 1080), None);
    assert_eq!(scissor(300, 200, 1080), None, "an empty slice draws nothing");
}

#[test]
fn the_beam_reaches_a_row_after_the_start_delay_and_its_share_of_the_scan() {
    let scan = scan();
    assert_eq!(scan.reaches_us(10_000, 0), 10_800);
    assert_eq!(scan.reaches_us(10_000, 540), 10_800 + 1728);
    assert_eq!(scan.ends_us(10_000), 10_800 + 3456);
}

#[test]
fn a_scaled_or_letterboxed_picture_gets_its_share_of_the_beam() {
    let beam = Beam {
        rows: 1440, blank_rows: 60, total_rows: 1500, scan_ns: 4_166_667, refresh_hz: 240.0, lines_per_ms: 360.0,
        vblank_fraction: 0.04, scans: 20, idle_from: 1000, longest_idle_ns: 9_000_000, idle_scans: 0,
    };
    // 1080 rows shown over 1440 display rows: 4/3 of the beam's 2.78 us per row for each.
    let fit = Scan::for_picture(&beam, 800_000, 1080, 0, 1440);
    assert_eq!(fit.rows, 1080);
    assert!((fit.row_ns - 3703.7).abs() < 0.1, "{}", fit.row_ns);
    assert_eq!(fit.delta_us, 800);
    // Black bars of 90 rows above the picture delay its first row by their beam time.
    let boxed = Scan::for_picture(&beam, 800_000, 1080, 90, 1260);
    assert_eq!(boxed.delta_us, 800 + 250);
    assert!((boxed.row_ns - 3240.7).abs() < 0.1, "{}", boxed.row_ns);
    let whole = Scan::for_picture(&beam, 0, 1440, 0, 1440).ends_us(0);
    assert!(whole.abs_diff(4000) <= 1, "a picture that fills the display takes its rows' time: {whole}");
}

#[test]
fn a_fast_pace_gives_no_race() {
    // The whole frame is in 0.9 ms: nothing to gain over presenting it whole.
    let result = plan(&scan(), 50_000, &learned(900), &quiet(), 1100, 0);
    assert_eq!(result, Err(Skip::NoGain));
}

#[test]
fn a_slow_pace_starts_the_scan_ahead_of_presenting_whole() {
    // 12 ms from the first slice to the last: the scan runs 3.5 ms, the picture is shown ~6 ms earlier.
    let predictor = learned(12_000);
    let plan = plan(&scan(), 50_000, &predictor, &quiet(), 1100, 0).expect("races");
    assert!(plan.start_us >= 50_000 + 100, "not before the first slice is presented");
    assert!(plan.gain_us >= MIN_GAIN_US, "{plan:?}");
    // Every slice is presented before the beam reaches its rows.
    let scan = scan();
    for (offset, first_row) in predictor.slices() {
        let presented = 50_000 + offset as u64 + 100;
        assert!(presented + 500 + 1100 <= scan.reaches_us(plan.start_us, first_row), "slice at row {first_row} would be overtaken: {plan:?}");
    }
    // And the scan is not started earlier than that: one slice is just on the margin.
    let tight = predictor.slices().any(|(offset, first_row)| {
        let presented = 50_000 + offset as u64 + 100;
        presented + 500 + 1100 + 1000 > scan.reaches_us(plan.start_us, first_row)
    });
    assert!(tight, "the start leaves slack that gains nothing: {plan:?}");
}

#[test]
fn a_pace_in_between_gains_less_and_the_gain_is_capped_by_the_scan() {
    let slow = plan(&scan(), 0, &learned(12_000), &quiet(), 1100, 0).unwrap().gain_us;
    let middle = plan(&scan(), 0, &learned(2_000), &quiet(), 1100, 0).unwrap().gain_us;
    let slower = plan(&scan(), 0, &learned(30_000), &quiet(), 1100, 0).unwrap().gain_us;
    assert!(middle < slow, "{middle} {slow}");
    assert_eq!(slow, slower, "past the scan's own length a slower pace gains no more");
}

#[test]
fn a_scan_that_has_not_ended_holds_the_next_start() {
    let free = plan(&scan(), 50_000, &learned(12_000), &quiet(), 1100, 0).unwrap();
    let held = plan(&scan(), 50_000, &learned(12_000), &quiet(), 1100, free.start_us + 1500);
    assert!(held.map_or(true, |p| p.start_us == free.start_us + 1500 && p.gain_us < free.gain_us));
}

#[test]
fn a_bigger_margin_starts_later_and_gains_less() {
    let small = plan(&scan(), 0, &learned(12_000), &quiet(), 600, 0).unwrap();
    let big = plan(&scan(), 0, &learned(12_000), &quiet(), 1500, 0).unwrap();
    assert!(big.start_us > small.start_us && big.gain_us < small.gain_us);
}

#[test]
fn the_margin_is_the_spread_plus_a_host_batch_unless_the_setting_gives_one() {
    let latency = Latency::fixed(100, 500, 400);
    assert_eq!(latency.margin_us(0), 1400);
    assert_eq!(latency.margin_us(3), 3000);
}

#[test]
fn latency_folds_its_window_into_the_median_and_the_spread() {
    let mut latency = Latency::default();
    for i in 0..100u64 {
        // Ready at 0, issued 100 us later, the call takes 400 us; one in twenty takes 2 ms more.
        latency.record(0, 100, 500 + if i % 20 == 0 { 2000 } else { 0 }, 1_000 + i);
    }
    latency.record(0, 100, 500, 6_000_000);
    assert_eq!((latency.wake_us(), latency.call_us()), (100, 400));
    assert_eq!(latency.margin_us(0), 2000 + HOST_BATCH_US, "p99 - p50 of the whole present, plus a host batch");
}

#[test]
fn whole_pictures_teach_the_call_before_any_slice_was_presented() {
    let mut latency = Latency::default();
    assert_eq!((latency.wake_us(), latency.call_us()), (300, 1200), "guessed before anything was timed");
    for i in 0..100u64 {
        latency.record_call(100, 100 + 250 + if i % 20 == 0 { 700 } else { 0 }, 1_000 + i);
    }
    latency.record_call(100, 350, 6_000_000);
    assert_eq!(latency.call_us(), 250);
    assert_eq!(latency.wake_us(), 300, "the wake stays as guessed without slice presents");
    assert_eq!(latency.margin_us(0), 700 + HOST_BATCH_US);
}

#[test]
fn the_predictor_is_seeded_from_the_pace_and_then_learns_the_frames() {
    // 831 KB at 900 Mbps: 7.4 ms on the wire.
    let predictor = Predictor::seeded(4, 1080, 831_000.0, 900);
    let offsets: Vec<f64> = predictor.slices().map(|(offset, _)| offset).collect();
    assert_eq!(offsets[0], 0.0);
    assert!(offsets.windows(2).all(|pair| pair[1] > pair[0]));
    assert!((offsets[3] - 7_388.0 * 0.85 * 0.75).abs() < 800.0, "{offsets:?}");
    assert!(!predictor.learned());
    let mut predictor = predictor;
    for _ in 0..50 { predictor.learn(&frame(1_000, 9_000)); }
    assert!(predictor.learned());
    let last = predictor.slices().last().unwrap();
    assert!((last.0 - 9_000.0).abs() < 50.0 && last.1 == 810, "{last:?}");
}

#[test]
fn a_frame_without_slices_is_not_raced() {
    let predictor = Predictor::seeded(1, 1080, 1.0, 900);
    assert_eq!(plan(&scan(), 0, &predictor, &quiet(), 1100, 0), Err(Skip::Unsliced));
}

fn racer_with_frame(span: u64) -> (Racer<u8>, Predictor, Vec<BandReady>) {
    let bands = frame(50_000, span);
    (Racer::default(), learned(span), bands)
}

#[test]
fn slices_are_presented_at_their_times_and_the_final_draws_the_rest() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    let start = racer.due_us().expect("planned");
    assert!(start > 50_100, "the scan waits for the slices behind the beam");
    assert!(racer.take(start - 1).is_empty(), "not before the start");
    let first = racer.take(start);
    assert_eq!(first.len(), 1);
    racer.presented(start, &scan);
    assert_eq!(racer.scan_start_us(), Some(start));
    // The later slices come at their times and are drawn as they are ready.
    for (i, b) in bands.iter().enumerate().skip(1).take(2) {
        let at = b.ready_us + 100;
        racer.observe(1, *b, i as u8, at, &scan, &predictor, &latency, 1100);
        let batch = racer.take(at);
        assert_eq!(batch.len(), 1, "slice {i} is presented when it is ready");
        racer.presented(at, &scan);
    }
    let last = bands[3];
    assert_eq!(racer.finish(1, last.first_row, last.ready_us + 100, &scan, &latency), Final::Rows(810, 1080));
    let record = racer.done();
    assert_eq!((record.late, record.raced, record.presents_us.len()), (0, true, 3));
}

#[test]
fn the_wake_of_a_slice_starts_when_the_scan_was_due_not_when_it_was_ready() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(3, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    let start = racer.due_us().unwrap();
    assert!(start > bands[0].ready_us + 500, "the plan waits on purpose");
    racer.take(start + 60);
    assert_eq!(racer.presentable_us(bands[0].ready_us), start, "the wait is not a wake");
    assert_eq!(racer.presentable_us(start + 900), start + 900, "a slice ready after the start is timed from then");
}

#[test]
fn a_late_slice_holds_the_rest_and_the_frame_is_presented_whole() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(7, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    let start = racer.due_us().unwrap();
    racer.take(start);
    racer.presented(start, &scan);
    // Slice 2 turns up when the beam is long past its first row (540).
    let late = scan.reaches_us(start, 540) + 300;
    racer.observe(7, bands[1], 1, late, &scan, &predictor, &latency, 1100);
    assert!(racer.take(late).is_empty(), "a late slice is held, not presented");
    racer.observe(7, bands[2], 2, late + 10, &scan, &predictor, &latency, 1100);
    assert!(racer.take(late + 10).is_empty(), "and so are the ones after it");
    assert_eq!(racer.finish(7, 810, late + 5000, &scan, &latency), Final::Whole);
    let record = racer.done();
    assert_eq!(record.late, 1, "counted once");
    assert!(record.raced);
}

#[test]
fn a_final_decode_late_against_the_beam_is_counted_and_drawn_whole() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(2, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    let start = racer.due_us().unwrap();
    racer.take(start);
    racer.presented(start, &scan);
    for (i, b) in bands.iter().enumerate().skip(1).take(2) {
        racer.observe(2, *b, i as u8, b.ready_us + 100, &scan, &predictor, &latency, 1100);
        racer.take(b.ready_us + 100);
    }
    let after_scan = scan.ends_us(start) + 100;
    assert_eq!(racer.finish(2, 810, after_scan, &scan, &latency), Final::Whole);
    assert_eq!(racer.done().late, 1);
}

#[test]
fn a_final_before_the_scan_starts_is_drawn_whole_and_not_raced() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(3, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    assert_eq!(racer.finish(3, 810, 50_200, &scan, &latency), Final::Whole);
    let record = racer.done();
    assert!(!record.raced && record.late == 0 && record.presents_us.is_empty());
}

#[test]
fn a_fast_frame_is_not_raced_and_says_why() {
    let (scan, latency) = (scan(), quiet());
    let predictor = learned(800);
    let mut racer = Racer::<u8>::default();
    let bands = frame(50_000, 800);
    racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    assert_eq!(racer.due_us(), None);
    assert_eq!(racer.finish(1, 810, 50_900, &scan, &latency), Final::Whole);
    assert_eq!(racer.done().skip, Some(Skip::NoGain));
    assert_eq!((racer.totals.skipped, racer.totals.whole), ([1, 0, 0], 1), "counted by reason");
}

#[test]
fn a_frame_that_starts_past_its_first_slice_is_presented_whole() {
    let (scan, latency) = (scan(), quiet());
    let mut racer = Racer::<u8>::default();
    racer.observe(1, band(1, 270, 540, 50_000), 0, 50_100, &scan, &learned(12_000), &latency, 1100);
    assert_eq!(racer.due_us(), None);
    assert_eq!(racer.done().skip, Some(Skip::Missed));
}

#[test]
fn the_slices_of_an_older_frame_are_dropped() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(5, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    racer.observe(4, bands[1], 1, 50_200, &scan, &predictor, &latency, 1100);
    assert_eq!(racer.seq(), 5);
    assert_eq!(racer.slices().len(), 1, "the older frame's slice is not mixed into this one");
    assert_eq!(racer.finish(4, 810, 60_000, &scan, &latency), Final::Whole, "a final of another frame is drawn whole");
}

#[test]
fn a_newer_frame_ends_one_that_never_finished() {
    let (mut racer, predictor, bands) = racer_with_frame(12_000);
    let (scan, latency) = (scan(), quiet());
    racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    let start = racer.due_us().unwrap();
    racer.take(start);
    racer.presented(start, &scan);
    let next = frame(120_000, 12_000);
    racer.observe(2, next[0], 0, 120_100, &scan, &predictor, &latency, 1100);
    assert_eq!(racer.seq(), 2);
    assert!(racer.due_us().is_some_and(|due| due > 120_000), "the new frame is planned from its own first slice");
}

#[test]
fn the_next_scan_waits_for_the_last_one_to_end() {
    let (scan, latency) = (scan(), quiet());
    let predictor = learned(12_000);
    let mut racer = Racer::<u8>::default();
    let bands = frame(50_000, 12_000);
    racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    let start = racer.due_us().unwrap();
    racer.take(start);
    racer.presented(start, &scan);
    racer.finish(1, 810, start + 100, &scan, &latency);
    racer.done();
    // A frame whose first slice is ready in the middle of that scan.
    let early = frame(start + 500, 12_000);
    racer.observe(2, early[0], 0, start + 600, &scan, &predictor, &latency, 1100);
    if let Some(due) = racer.due_us() { assert!(due >= scan.ends_us(start), "the scan has not ended: {due}"); }
}

#[test]
fn a_picture_of_an_older_frame_does_not_step_the_image_back_or_close_the_newer_frame() {
    let (scan, latency) = (scan(), quiet());
    let predictor = learned(12_000);
    let mut racer = Racer::<u8>::default();
    let (older, newer) = (frame(50_000, 12_000), frame(66_000, 12_000));
    racer.observe(1, older[0], 0, 50_100, &scan, &predictor, &latency, 1100);
    // The slices of frame 2 arrive and are presented before the final decode of frame 1 is shown.
    racer.observe(2, newer[0], 1, 66_100, &scan, &predictor, &latency, 1100);
    assert!(!racer.newer_on_screen(1), "queued is not on the screen yet");
    let start = racer.due_us().unwrap();
    assert_eq!(racer.take(start).len(), 1);
    racer.presented(start, &scan);
    assert!(racer.newer_on_screen(1), "slices of frame 2 are on the screen: frame 1's picture is dropped");
    assert!(!racer.newer_on_screen(2) && !racer.newer_on_screen(3), "the frame itself and newer ones are shown");
    assert!(!racer.newer_on_screen(0), "a picture that was not sliced says nothing of its age");
    // Were it shown anyway, it must not close frame 2.
    assert_eq!(racer.close(1, start + 50, &scan), None);
    assert_eq!(racer.finish(1, 810, start + 60, &scan, &latency), Final::Whole);
    racer.observe(2, newer[1], 2, start + 200, &scan, &predictor, &latency, 1100);
    assert_eq!(racer.take(start + 200).len(), 1, "frame 2 goes on");
    let record = racer.close(2, start + 900, &scan).expect("frame 2's picture closes it");
    assert!(record.raced);
    assert!(racer.newer_on_screen(1), "and frame 1 stays behind it");
    assert_eq!(racer.totals.raced, 1);
}

#[test]
fn resetting_the_racer_lets_go_of_the_slices_it_holds() {
    use std::rc::Rc;
    let (scan, latency) = (scan(), quiet());
    let predictor = learned(12_000);
    let mut racer = Racer::<Rc<()>>::default();
    let held = Rc::new(());
    let bands = frame(50_000, 12_000);
    racer.observe(1, bands[0], held.clone(), 50_100, &scan, &predictor, &latency, 1100);
    assert_eq!(Rc::strong_count(&held), 2, "the racer holds the slice");
    racer.reset();
    assert_eq!(Rc::strong_count(&held), 1, "reset drops it");
    assert_eq!((racer.due_us(), racer.totals.abandoned), (None, 1));
    // Slices of the frame it dropped stay dropped, and the next frame starts afresh.
    racer.observe(1, bands[1], held.clone(), 51_000, &scan, &predictor, &latency, 1100);
    assert_eq!(Rc::strong_count(&held), 1);
    racer.observe(2, bands[0], held.clone(), 70_000, &scan, &predictor, &latency, 1100);
    assert!(racer.due_us().is_some());
}

#[test]
fn mid_row_age_is_measured_the_same_way_for_both_modes() {
    let scan = scan();
    let bands = frame(50_000, 12_000);
    // Step 2: one present on the final decode. The beam is at row 540 at present + 800 + 1728 us.
    let last_ready = bands[3].ready_us;
    let whole = mid_row_age_ns(&scan, last_ready + 100, &bands).unwrap();
    assert_eq!(whole, ((last_ready + 100 + 800 + 1728) as i64 - bands[2].ready_us as i64) * 1000, "the middle row is in slice 2");
    // Raced: the scan starts well before.
    let raced = mid_row_age_ns(&scan, 54_000, &bands).unwrap();
    assert!(raced < whole, "{raced} {whole}");
    assert_eq!(whole - raced, (last_ready + 100 - 54_000) as i64 * 1000);
    // An unsliced frame has one band for the whole picture: its own ready time.
    let one = [band(0, 0, 1080, 60_000)];
    assert_eq!(mid_row_age_ns(&scan, 60_100, &one), Some((60_100 + 800 + 1728 - 60_000) as i64 * 1000));
    assert_eq!(mid_row_age_ns(&scan, 60_100, &[]), None);
}
