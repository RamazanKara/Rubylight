use super::*;

/// The reporter's panel (Nightjar issue 111): 1080 rows behind 34 lines of blanking at 138.2 lines
/// a ms, so a scan takes 8.06 ms.
const SCAN_NS: u64 = 8_060_000;

#[test]
fn a_present_is_on_its_way_until_a_scan_begins_after_it() {
    // Issued as the beam left the last row, a whole scan after that scan began; read 0.4 ms later
    // with the display still idling: the scan seen began 8.46 ms ago, before the present.
    assert!(on_its_way(SCAN_NS + 400_000, 400_000, SCAN_NS));
    // Still nothing 5 ms on (the handheld's median flip): still on its way.
    assert!(on_its_way(SCAN_NS + 5_000_000, 5_000_000, SCAN_NS));
    // Its scan began 0.3 ms after the present and ended; the display idles again 9 ms after it.
    assert!(!on_its_way(8_700_000, 9_000_000, SCAN_NS));
    // A display that idled long before the present (a slow stream): the scan seen is far older.
    assert!(on_its_way(30_000_000, 1_000_000, SCAN_NS));
    // And long after one: the last scan is the present's own, or a refresh the display made since.
    assert!(!on_its_way(12_000_000, 20_000_000, SCAN_NS));
    assert!(!on_its_way(3_000_000, 20_000_000, SCAN_NS));
}

#[test]
fn the_readings_clock_may_be_off_by_a_good_part_of_a_scan() {
    // The present's time and the scan's start come from two clocks and a line count: 2 ms either
    // way decides nothing on an 8 ms scan.
    for off in [0u64, 1_000_000, 2_000_000] {
        assert!(on_its_way(SCAN_NS + 400_000 - off, 400_000, SCAN_NS), "pending, the scan read {off} ns younger");
        assert!(!on_its_way(600_000 + off, 900_000, SCAN_NS), "landed, the scan read {off} ns older");
    }
}

#[test]
fn a_landing_is_timed_only_for_the_presents_own_scan() {
    assert_eq!(landing_ns(8_000_000, 8_300_000), Some(300_000), "the scan began 0.3 ms after the present");
    assert_eq!(landing_ns(9_000_000, 8_300_000), None, "the scan began before the present");
    assert_eq!(landing_ns(1_000_000, 40_000_000), None, "the display refreshed by itself since");
    assert_eq!(landing_ns(0, LANDING_LIMIT_NS), Some(LANDING_LIMIT_NS));
}

#[test]
fn times_give_a_median_and_19_of_20_to_the_step_above() {
    let mut times = Times::default();
    assert_eq!(times.within_ms(0.5), 0.0, "none yet");
    for _ in 0..19 { times.push(350_000); }
    times.push(4_620_000);
    assert_eq!(times.count(), 20);
    assert!((times.within_ms(0.5) - 0.4).abs() < 1e-9);
    assert!((times.within_ms(0.95) - 0.4).abs() < 1e-9, "19 of 20");
    assert!((times.within_ms(1.0) - 4.7).abs() < 1e-9);
    times.push(90_000_000);
    assert!((times.within_ms(1.0) - 20.0).abs() < 1e-9, "past the last step counts in it");
}

#[test]
fn the_wait_rests_after_three_presents_in_a_row_started_no_scan() {
    let mut tally = Tally::default();
    assert!(tally.waits());
    assert!(!tally.waited_for(9_000_000, false));
    assert!(!tally.waited_for(9_000_000, false));
    // One that landed in between: the row starts again.
    assert!(!tally.waited_for(400_000, true));
    assert!(!tally.waited_for(9_000_000, false));
    assert!(!tally.waited_for(9_000_000, false));
    tally.found_landed();
    assert!(!tally.waited_for(9_000_000, false));
    assert!(!tally.waited_for(9_000_000, false));
    assert!(tally.waited_for(9_000_000, false), "the third in a row");
    for present in 0..REST_PRESENTS { assert!(!tally.waits(), "present {present} of the rest"); }
    assert!(tally.waits(), "tried again after the rest");
    assert_eq!((tally.gave_up, tally.rests, tally.waited.0), (7, 1, 8));
}

#[test]
fn the_log_says_what_the_presents_met() {
    let mut tally = Tally::default();
    assert!(tally.lines().is_empty(), "a stream that never synchronised says nothing");
    tally.undrawn = 7;
    assert_eq!(tally.lines(), ["VRR: the drawing of 7 pictures could not be waited for before their presents"], "no D3D11 renderer to ask");
    for _ in 0..100 { tally.drawing.push(1_250_000); }
    tally.undrawn = 2;
    for _ in 0..50 { tally.landing.push(330_000); }
    tally.behind = 12;
    tally.waited_for(600_000, true);
    tally.waited_for(9_400_000, false);
    assert_eq!(tally.lines(), [
        "VRR: a picture's drawing was finished before its present, after 1.3 ms at the median, 19 of 20 within 1.3 ms (2 not waited for)",
        "VRR: a present started its scan after 0.4 ms at the median, 19 of 20 within 0.4 ms (50 timed)",
        "VRR: 12 presents found the one before still on its way to the screen; 2 waited for it, 5.00 ms each, 1 of them for nothing (0 rests of 300 presents)",
    ]);
}

#[test]
fn a_scan_under_way_is_waited_out_to_its_end() {
    let limit = SCAN_NS + SCAN_WAIT_SLACK_NS;
    // A picture that meets the scan just begun sleeps most of it, not 2 ms (Nightjar 5.4.1).
    assert_eq!(scan_step(Some(SCAN_NS), true, 0, limit), ScanStep::Sleep(SCAN_NS - 1_000_000));
    // The last millisecond and a half is spun, reading the beam.
    assert_eq!(scan_step(Some(1_400_000), true, 6_600_000, limit), ScanStep::Spin);
    assert_eq!(scan_step(Some(200_000), true, 7_800_000, limit), ScanStep::Spin);
    // The beam has left the last row: present.
    assert_eq!(scan_step(None, true, 8_000_000, limit), ScanStep::Go);
    // A sleep never runs past the limit.
    assert_eq!(scan_step(Some(SCAN_NS), true, 8_500_000, limit), ScanStep::Sleep(limit - 8_500_000));
    // A reading that keeps the beam in the rows past one scan and the slack is wrong: go, counted.
    assert_eq!(scan_step(Some(3_000_000), true, limit, limit), ScanStep::CutShort);
    // So is a scan line that stops answering.
    assert_eq!(scan_step(Some(3_000_000), false, 1_000_000, limit), ScanStep::CutShort);
}

#[test]
fn the_log_counts_presents_that_went_out_into_a_scan() {
    let tally = Tally { cut_short: 3, wrapped: 2, ..Tally::default() };
    assert_eq!(tally.lines(), vec![
        "VRR: 3 presents went out before the scan under way had ended (the wait reached its limit or the scan line could not be read)".to_string(),
        "VRR: 2 waits ended as the display began its next scan, with no reading past the last row".to_string(),
    ]);
}

/// The reporter's panel as a beam: 33 lines of blanking, 1080 rows, 138.2 lines a ms.
fn panel() -> crate::stream::scanline::Beam {
    crate::stream::scanline::Beam {
        rows: 1080, blank_rows: 33, total_rows: 1113, scan_ns: 8_053_000, refresh_hz: 124.2, lines_per_ms: 138.2,
        vblank_fraction: 0.4, scans: 30, idle_from: 1421, longest_idle_ns: 23_000_000, idle_scans: 10,
    }
}

/// Runs `wait_out` on a simulated clock: a sleep moves it by its length (plus `oversleep`), a
/// reading by 2 µs, and `line` gives the counter at a time (ns since the scan began).
fn simulate(start_ns: u64, oversleep: u64, line: impl Fn(u64) -> Option<u32>) -> (Waited, u32) {
    use std::cell::Cell;
    let beam = panel();
    let now = Cell::new(start_ns);
    let sample = |at: u64| line(at).map(|line| crate::stream::scanline::Sample { at_ns: at, line, vblank: line < beam.blank_rows });
    let first = sample(start_ns).unwrap();
    let waited = wait_out(&beam, first, || { now.set(now.get() + 2_000); sample(now.get()) },
        |ns| now.set(now.get() + ns + oversleep), || now.get() - start_ns).expect("a scan under way");
    let end = sample(now.get()).map_or(0, |s| s.line);
    (waited, end)
}

#[test]
fn a_wait_follows_the_beam_past_the_last_row() {
    let lines = |at: u64| Some((at as f64 * 138.2 / 1e6) as u32);
    // Met 0.3 ms into the scan: it waits the 7.75 ms left, not 2 ms, and goes once the beam has
    // left row 1080 (line 1113), before the slack runs out.
    let (waited, line) = simulate(300_000, 0, lines);
    assert!(!waited.cut_short && !waited.wrapped);
    assert!((7_700_000..8_100_000).contains(&waited.ns), "{waited:?}");
    assert!((1113..1125).contains(&line), "went out at line {line}");
    // A sleep 0.6 ms too long (a coarse timer) still goes, a little late, and never into the rows.
    let (waited, line) = simulate(300_000, 600_000, lines);
    assert!(!waited.cut_short && line >= 1113, "{waited:?} at line {line}");
}

#[test]
fn a_counter_that_wraps_ends_the_wait() {
    // Back to back scans: the counter goes from the last line straight to 0 and never reads past
    // the last row. The wait ends at the wrap, not at its limit.
    let (waited, line) = simulate(300_000, 0, |at| Some(((at as f64 * 138.2 / 1e6) as u32) % 1113));
    assert!(!waited.cut_short && waited.wrapped, "{waited:?}");
    assert!(waited.ns < 8_000_000 && line < 33, "{waited:?} at line {line}");
}

#[test]
fn a_reading_that_never_moves_or_fails_goes_out_counted() {
    let (waited, _) = simulate(300_000, 0, |_| Some(500));
    assert!(waited.cut_short);
    assert!(waited.ns <= 7_750_000 + SCAN_WAIT_SLACK_NS + 300_000, "{waited:?}");
    let (waited, _) = simulate(300_000, 0, |at| (at == 300_000).then_some(41));
    assert!(waited.cut_short, "no reading after the first: {waited:?}");
}
