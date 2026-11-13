use super::*;

/// A synthetic display like the one measured on the RX 7900 XT: the counter runs through
/// `blank` lines of blanking (`vblank` set) and then `rows` picture lines at `lines_per_ms`,
/// from each start in `starts`; between scans it idles at 0 in the blanking (a variable
/// refresh display waiting for a present). Read every `every_ns` until `until`.
fn display(starts: &[u64], blank: u32, rows: u32, lines_per_ms: f64, every_ns: u64, until: u64) -> Vec<Sample> {
    let total = f64::from(blank + rows);
    let scan_ns = (total * 1e6 / lines_per_ms) as u64;
    let mut samples = Vec::new();
    let mut t = 0;
    while t < until {
        let scan = starts.iter().rev().find(|s| **s <= t).filter(|s| t - **s < scan_ns);
        samples.push(match scan {
            Some(s) => {
                let line = ((t - s) as f64 * lines_per_ms / 1e6) as u32;
                Sample { at_ns: t, line, vblank: line < blank }
            }
            None => Sample { at_ns: t, line: 0, vblank: true },
        });
        t += every_ns;
    }
    samples
}

/// The measured display: 1440 rows behind 202 lines of blanking, 394 lines per ms (4.17 ms).
const BLANK: u32 = 202;
const RATE: f64 = 394.0;

#[test]
fn a_240_hz_display_with_a_60_hz_stream_calibrates_to_its_scan() {
    // A scan every 16.667 ms, as a 60 fps stream on a variable refresh display makes.
    let starts: Vec<u64> = (0..60).map(|i| 3_000_000 + i * 16_667_000).collect();
    let samples = display(&starts, BLANK, 1440, RATE, 250_000, 1_000_000_000);
    let beam = calibrate(&samples, 1440).unwrap();
    assert_eq!(beam.rows, 1440);
    assert!((i64::from(beam.blank_rows) - i64::from(BLANK)).abs() <= 45, "{}", beam.blank_rows);
    assert!((beam.scan_ns as f64 - 4_170_000.0).abs() < 60_000.0, "S is the whole scan, blanking included: {}", beam.scan_ns);
    assert!((beam.refresh_hz - 240.0).abs() < 4.0, "{}", beam.refresh_hz);
    assert!((beam.lines_per_ms - RATE).abs() < 3.0, "{}", beam.lines_per_ms);
    assert!(beam.scans >= 50);
    assert!(beam.vblank_fraction > 0.7, "the display idles between scans: {}", beam.vblank_fraction);
}

#[test]
fn a_fixed_refresh_display_scanning_all_the_time_calibrates_too() {
    let starts: Vec<u64> = (0..120).map(|i| 1_000_000 + i * 4_167_000).collect();
    let samples = display(&starts, BLANK, 1440, RATE, 250_000, 450_000_000);
    let beam = calibrate(&samples, 1440).unwrap();
    assert!((beam.scan_ns as f64 - 4_170_000.0).abs() < 60_000.0, "{}", beam.scan_ns);
    assert!(beam.vblank_fraction < 0.3, "{}", beam.vblank_fraction);
}

#[test]
fn the_rows_come_from_the_readings_when_the_height_is_not_given() {
    let starts: Vec<u64> = (0..40).map(|i| 1_000_000 + i * 16_667_000).collect();
    let samples = display(&starts, 150, 1080, 394.0, 100_000, 600_000_000);
    let beam = calibrate(&samples, 0).unwrap();
    assert!((1070..=1090).contains(&beam.rows), "{}", beam.rows);
    assert_eq!(beam.total_rows, beam.blank_rows + beam.rows);
}

#[test]
fn a_display_that_never_scans_in_the_window_says_so() {
    let samples = display(&[], BLANK, 1440, RATE, 250_000, 1_000_000_000);
    assert_eq!(calibrate(&samples, 1440), Err(CalibrationError::NoScan));
    assert_eq!(calibrate(&[], 1440), Err(CalibrationError::NoScan));
}

#[test]
fn a_scan_cut_by_the_window_or_too_short_to_fit_is_not_used() {
    // Readings 1.5 ms apart see 3 per scan: too few to fit.
    let coarse = display(&[1_000_000, 20_000_000], BLANK, 1440, RATE, 1_500_000, 40_000_000);
    assert!(calibrate(&coarse, 1440).is_err());
    // A rising run whose blanking was not read is not a refresh to measure.
    let mid: Vec<Sample> = (0..20).map(|i| Sample { at_ns: i * 250_000, line: 300 + i as u32 * 60, vblank: false }).collect();
    assert!(calibrate(&mid, 1440).is_err());
}

fn measured() -> Beam {
    Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0, vblank_fraction: 0.75, scans: 50, idle_from: 2040, longest_idle_ns: 0, idle_scans: 0 }
}

#[test]
fn the_beam_row_follows_the_scan() {
    let beam = measured();
    assert_eq!(beam.row_at(0), -200, "a scan begins in the blanking");
    assert_eq!(beam.row_at(500_000), 0, "400 lines per ms: 200 lines of blanking take 0.5 ms");
    assert_eq!(beam.row_at(2_300_000), 720);
    assert_eq!(beam.row_at(4_100_000), 1440, "the last row");
    assert_eq!(beam.time_to_row(0), 500_000);
    assert_eq!(beam.time_to_row(720), 2_300_000);
    assert_eq!(beam.row_of(&Sample { at_ns: 0, line: 900, vblank: false }), Some(700));
    assert_eq!(beam.row_of(&Sample { at_ns: 0, line: 100, vblank: true }), None);
    assert_eq!(beam.row_of(&Sample { at_ns: 0, line: 5000, vblank: false }), None, "past the last row");
}

#[test]
fn a_present_mid_scan_waits_for_the_beam_to_leave_the_last_row() {
    let beam = measured();
    // 400 lines per ms: row 700 has 740 rows to go, 1.85 ms.
    assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 900, vblank: false }), Some(1_850_000));
    assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 200, vblank: false }), Some(3_600_000), "the first row: the whole picture to go");
    assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 100, vblank: true }), None, "in the blanking a flip comes before the first row");
    assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 5000, vblank: false }), None, "past the last row");
    let short = Beam { scan_ns: 2_000_000, ..beam };
    assert_eq!(short.scan_left_ns(&Sample { at_ns: 0, line: 200, vblank: false }), Some(2_000_000), "never more than a scan");
}

fn at(ns: u64, line: u32, vblank: bool) -> Sample { Sample { at_ns: ns, line, vblank } }

#[test]
fn a_present_in_the_blanking_that_restarts_the_scan_passes() {
    let before = at(0, 2400, true);
    // The first reading out of the blanking is on row 3 (line 203).
    let after = [at(10_000, 0, true), at(20_000, 130, true), at(30_000, 203, false), at(40_000, 215, false)];
    assert_eq!(restart(&before, &after, &measured(), 0), Restart::Began);
}

#[test]
fn a_present_whose_scan_starts_late_or_far_down_misses() {
    let before = at(0, 2400, true);
    let beam = measured();
    // Nothing within the window.
    assert_eq!(restart(&before, &[at(100_000, 0, true), at(400_000, 0, true)], &beam, 0), Restart::Missed);
    assert_eq!(restart(&before, &[], &beam, 0), Restart::Missed);
    // A scan already well down when first seen: it began before the present, so it is not the present's.
    assert_eq!(restart(&before, &[at(1_000, 600, false)], &beam, 0), Restart::Missed);
}

#[test]
fn the_window_covers_the_slowest_flip_measured_with_a_millisecond_to_spare() {
    // 60 presents on the 240 Hz VRR display with the host encoding on the same GPU began their
    // scans 0.2-8.8 ms after the present call (p99 8.79 ms).
    const { assert!(RESTART_WINDOW_NS >= 8_790_000 + 1_000_000) };
    let beam = measured();
    let late = [at(8_700_000, 2400, true), at(8_790_000, 203, false)];
    assert_eq!(restart(&at(0, 2400, true), &late, &beam, 0), Restart::Began);
}

#[test]
fn a_present_in_the_middle_of_a_scan_or_in_its_blanking_says_nothing() {
    // The blanking of a scan under way (a fixed refresh has nothing else) is not the display idling.
    assert_eq!(restart(&at(0, 50, true), &[at(1_000, 203, false)], &measured(), 0), Restart::NotTested);
    assert_eq!(restart(&at(0, 900, false), &[at(1_000, 910, false)], &measured(), 0), Restart::NotTested);
}

#[test]
fn a_third_of_the_tested_presents_missing_fails_the_check() {
    let mut check = RestartCheck::default();
    for _ in 0..10 { check.push(Restart::Began); }
    for _ in 0..CHECK_MISSES - 1 { check.push(Restart::Missed); }
    assert!(!check.failed() && check.running());
    check.push(Restart::Missed);
    assert!(check.failed() && !check.running() && !check.passed());
    // Later results do not change a settled check.
    let mut done = RestartCheck::default();
    for _ in 0..CHECK_TESTED { done.push(Restart::Began); }
    assert!(done.passed() && !done.running());
    for _ in 0..5 { done.push(Restart::Missed); }
    assert!(done.passed() && !done.failed(), "presents after the check do not count");
}

#[test]
fn presents_in_a_scan_are_not_tests_and_cannot_pass_alone() {
    let mut check = RestartCheck::default();
    for _ in 0..CHECK_FRAMES - 1 { check.push(Restart::NotTested); }
    assert!(check.running() && !check.passed(), "no present was tested, so nothing is proven yet");
    check.push(Restart::NotTested);
    assert!(check.starved() && !check.running(), "a display that never idles is a fixed refresh");
}

fn beam(vblank_fraction: f64) -> Beam {
    Beam { vblank_fraction, ..measured() }
}

fn evidence() -> Evidence {
    let mut check = RestartCheck::default();
    for _ in 0..CHECK_TESTED { check.push(Restart::Began); }
    Evidence { tearing: true, scanline: true, fullscreen: true, beam: Some(Ok(beam(0.75))), check, content_hz: 60.0, display_hz: 240.0, check_rates: None, check_hz: 0.0, recent_hz: 0.0 }
}

fn missed() -> RestartCheck {
    let mut check = RestartCheck::default();
    for _ in 0..CHECK_MISSES { check.push(Restart::Missed); }
    check
}

#[test]
fn everything_in_order_is_on() {
    let verdict = decide(&evidence());
    assert!(matches!(verdict, Verdict::On(_, c) if c.on()), "{verdict:?}");
    assert_eq!(verdict.code(), "on");
    assert!(verdict.line().starts_with("on, scan 4.10 ms over 1440 rows"), "{}", verdict.line());
}

#[test]
fn each_reason_names_what_is_missing() {
    let line = |e: Evidence| decide(&e).line();
    assert_eq!(line(Evidence { tearing: false, ..evidence() }), "off (no tearing)", "V-Sync presents never tear");
    assert_eq!(line(Evidence { scanline: false, ..evidence() }), "off (no scanline)");
    assert_eq!(line(Evidence { beam: Some(Err(CalibrationError::NoScan)), ..evidence() }), "off (no scan seen)");
    assert_eq!(line(Evidence { check: missed(), ..evidence() }), "off (no VRR scan restart)", "a borderless window whose presents do not restart the scan");
    assert_eq!(line(Evidence { check: missed(), fullscreen: false, ..evidence() }), "off (not fullscreen)", "windowed");
    // A window is not raced whatever the check saw, and before the check has finished.
    assert_eq!(line(Evidence { fullscreen: false, ..evidence() }), "off (not fullscreen)");
    assert_eq!(line(Evidence { fullscreen: false, check: RestartCheck::default(), ..evidence() }), "off (not fullscreen)");
    assert_eq!(line(Evidence { fullscreen: false, beam: Some(Ok(beam(0.13))), ..evidence() }), "off (not fullscreen)", "a windowed fixed refresh");
    // A fixed refresh scans all the time: little blanking at a stream rate far below the top rate.
    assert_eq!(line(Evidence { check: missed(), beam: Some(Ok(beam(0.05))), ..evidence() }), "off (no VRR)");
    // At the display's own rate the two cannot be told apart, so a miss is only a missing restart.
    assert_eq!(line(Evidence { check: missed(), beam: Some(Ok(beam(0.05))), content_hz: 240.0, ..evidence() }), "off (no VRR scan restart)");
    let mut starved = RestartCheck::default();
    for _ in 0..CHECK_FRAMES { starved.push(Restart::NotTested); }
    assert_eq!(line(Evidence { check: starved, ..evidence() }), "off (no VRR)", "the display never idled");
}

#[test]
fn tearing_and_scanline_come_before_the_measurements() {
    // The reason does not wait for a calibration that has not finished.
    assert_eq!(decide(&Evidence { tearing: false, beam: None, ..evidence() }).line(), "off (no tearing)");
    assert_eq!(decide(&Evidence { scanline: false, beam: None, ..evidence() }).line(), "off (no scanline)");
    assert_eq!(decide(&Evidence { beam: None, ..evidence() }).line(), "checking the display");
    assert_eq!(decide(&Evidence { beam: None, ..evidence() }).code(), "");
}

#[test]
fn the_check_unfinished_is_checking_not_on() {
    let mut check = RestartCheck::default();
    for _ in 0..5 { check.push(Restart::Began); }
    assert_eq!(decide(&Evidence { check, ..evidence() }), Verdict::Checking);
}

/// A display whose counter never wraps by itself: it counts from each start in `starts` and
/// goes on, the blanking (`vblank`) before the first row and after the last, until the next
/// start. A long gap is a variable refresh display idling; a short one, a trailing blanking
/// (front porch) on scans that run straight into each other.
fn counting(starts: &[u64], blank: u32, rows: u32, lines_per_ms: f64, every_ns: u64, until: u64) -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut t = 0;
    while t < until {
        samples.push(match starts.iter().rev().find(|s| **s <= t) {
            Some(s) => {
                let line = ((t - s) as f64 * lines_per_ms / 1e6) as u32;
                Sample { at_ns: t, line, vblank: line < blank || line >= blank + rows }
            }
            None => Sample { at_ns: t, line: 0, vblank: true },
        });
        t += every_ns;
    }
    samples
}

#[test]
fn idling_is_counted_from_past_the_blanking_and_the_longest_idle_is_kept() {
    let starts: Vec<u64> = (0..60).map(|i| 3_000_000 + i * 16_667_000).collect();
    let beam = calibrate(&counting(&starts, BLANK, 1440, RATE, 250_000, 1_000_000_000), 1440).unwrap();
    assert!(beam.idle_from >= beam.total_rows + beam.blank_rows, "{beam:?}");
    assert!(beam.idle_from >= beam.total_rows + RATE as u32 - 5, "at least 1 ms of lines past the last row: {beam:?}");
    assert!((15_500_000..=17_000_000).contains(&beam.longest_idle_ns), "a scan every 16.7 ms: {}", beam.longest_idle_ns);
}

#[test]
fn a_trailing_blanking_longer_than_the_margin_is_not_idling() {
    // Scans 5.7 ms apart: 600 lines (1.5 ms) of blanking after the last row, more than the 1 ms
    // or the blanking the margin allows. A reading in it must not count as the display idling.
    let starts: Vec<u64> = (0..150).map(|i| 1_000_000 + i * 5_700_000).collect();
    let beam = calibrate(&counting(&starts, BLANK, 1440, RATE, 250_000, 800_000_000), 1440).unwrap();
    assert!(beam.idle_from > beam.total_rows + 540, "past the porch's lines: {beam:?}");
    let in_porch = Sample { at_ns: 0, line: beam.total_rows + 500, vblank: true };
    let late = [Sample { at_ns: 1_000_000, line: BLANK + 3, vblank: false }];
    assert_eq!(restart(&in_porch, &late, &beam, 0), Restart::NotTested, "the porch of a scan that ran straight through");
    let idling = Sample { at_ns: 0, line: beam.idle_from + 5, vblank: true };
    assert_eq!(restart(&idling, &late, &beam, 0), Restart::Began);
}

#[test]
fn a_present_while_the_display_idles_short_of_the_margin_is_not_tested() {
    let beam = measured();
    let late = [at(1_000_000, 203, false)];
    assert_eq!(restart(&at(0, beam.total_rows + 100, true), &late, &beam, 0), Restart::NotTested);
    assert_eq!(restart(&at(0, beam.idle_from, true), &late, &beam, 0), Restart::Began);
}

#[test]
fn a_scan_at_the_panels_own_refresh_period_is_not_the_presents() {
    // 400 lines per ms. A 48 Hz floor refreshes after 20.8 ms; the stream is 60 fps, so it
    // presents at 16.7 ms of idling. The scan that starts 20.8 ms after the last one began is
    // the panel's, whenever the present was, and cannot show the present restarted anything.
    let beam = measured();
    let before = at(1_000_000, 6_680, true); // 16.7 ms after the last scan began
    let forced = [at(1_000_000 + 4_600_000, 200, false)]; // its picture starts 20.8 ms after the last start
    assert_eq!(restart(&before, &forced, &beam, 20_800_000), Restart::NotTested);
    // The same scan with nothing known about the floor is taken at its word.
    assert_eq!(restart(&before, &forced, &beam, 0), Restart::Began);
    // A flip 1.5 ms after the present is 3.8 ms short of the floor: the present's.
    let flipped = [at(1_000_000 + 2_000_000, 200, false)];
    assert_eq!(restart(&before, &flipped, &beam, 20_800_000), Restart::Began);
}

#[test]
fn a_counter_whose_top_differs_from_the_monitors_height_is_refused() {
    // A scaled desktop or a rotated display: the monitor says 1440 rows, the counter walks 1080.
    let starts: Vec<u64> = (0..60).map(|i| 3_000_000 + i * 16_667_000).collect();
    let samples = counting(&starts, BLANK, 1080, RATE, 250_000, 1_000_000_000);
    assert_eq!(calibrate(&samples, 1440), Err(CalibrationError::RowsDiffer));
    assert!(calibrate(&samples, 1080).is_ok(), "the same readings fit the height they walk");
    assert!(calibrate(&samples, 1100).is_ok(), "within 3%");
    assert_eq!(decide(&Evidence { beam: Some(Err(CalibrationError::RowsDiffer)), ..evidence() }).line(), "off (scan rows differ from the display's)");
}

#[test]
fn a_stream_at_the_displays_rate_has_no_idle_to_test() {
    let mut starved = RestartCheck::default();
    for _ in 0..CHECK_FRAMES { starved.push(Restart::NotTested); }
    let line = |hz: f64| decide(&Evidence { check: starved, content_hz: hz, ..evidence() }).line();
    assert_eq!(line(240.0), "off (no idle at this frame rate)");
    assert_eq!(line(200.0), "off (no idle at this frame rate)");
    assert_eq!(line(60.0), "off (no VRR)", "a display that scans all the time under a slow stream");
}

#[test]
fn a_check_is_judged_at_the_rate_it_ran_at_not_a_later_one() {
    let mut starved = RestartCheck::default();
    for _ in 0..CHECK_FRAMES { starved.push(Restart::NotTested); }
    // Starved and failed at 200 fps on a 240 Hz VRR panel calibrated near its top rate; the
    // stream has slowed to 60 since.
    let later = Evidence { beam: Some(Ok(beam(0.12))), content_hz: 60.0, check_hz: 200.0, ..evidence() };
    assert_eq!(decide(&Evidence { check: starved, ..later }).line(), "off (no idle at this frame rate)");
    assert_eq!(decide(&Evidence { check: missed(), ..later }).line(), "off (no VRR scan restart)");
    // Run at 60 fps, they say what they did.
    let slow = Evidence { check_hz: 60.0, content_hz: 200.0, ..later };
    assert_eq!(decide(&Evidence { check: starved, ..slow }).line(), "off (no VRR)");
    assert_eq!(decide(&Evidence { check: missed(), ..slow }).line(), "off (no VRR)");
}

#[test]
fn a_fixed_refresh_is_told_a_second_after_its_calibration_without_waiting_for_the_check() {
    let presents = |results: &[Restart]| {
        let mut check = RestartCheck::default();
        for &result in results { check.push(result); }
        check
    };
    let quiet = |n: u32| presents(&vec![Restart::NotTested; n as usize]);
    // The laptop's 120 Hz panel under a 60 fps stream: 1% of the readings in the blanking, and
    // a second of presents none of which started a scan.
    let laptop = Evidence { check: quiet(FIXED_FRAMES), beam: Some(Ok(beam(0.01))), content_hz: 60.0, display_hz: 119.5, ..evidence() };
    assert_eq!(decide(&laptop).line(), "off (no VRR)");
    let mut missing = vec![Restart::NotTested; FIXED_FRAMES as usize];
    missing[10] = Restart::Missed;
    assert_eq!(decide(&Evidence { check: presents(&missing), ..laptop }).line(), "off (no VRR)", "a present in the blanking that did not restart it");
    // Not yet: the calibration alone decides nothing.
    assert_eq!(decide(&Evidence { check: quiet(FIXED_FRAMES - 1), ..laptop }), Verdict::Checking);
    // The VRR panel calibrated while it still scanned at its top rate (12% in the blanking):
    // a present that started a scan leaves it to the check.
    let mut began = vec![Restart::NotTested; FIXED_FRAMES as usize];
    began[5] = Restart::Began;
    let busy_vrr = Evidence { check: presents(&began), beam: Some(Ok(beam(0.12))), ..evidence() };
    assert_eq!(decide(&busy_vrr), Verdict::Checking);
    // This PC's 240 Hz VRR panel at 60 fps idles between scans: the check decides.
    let vrr = Evidence { check: quiet(FIXED_FRAMES), beam: Some(Ok(beam(0.67))), ..evidence() };
    assert_eq!(decide(&vrr), Verdict::Checking);
    // At the display's own rate a fixed refresh and a busy VRR display look alike.
    assert_eq!(decide(&Evidence { content_hz: 119.5, ..laptop }), Verdict::Checking);
    // A check that passed is not overruled by the blanking share.
    assert!(matches!(decide(&Evidence { beam: Some(Ok(beam(0.12))), ..evidence() }), Verdict::On(..)));
}

/// The 120 Hz 4K VRR monitor of nightjar#72 calibrated on a resume: scan 8.31 ms over 2160
/// rows, 23% of the readings in the blanking, longest idle 17.0 ms, 115 scans, while the 120 fps
/// stream presented 64-66 times a second.
fn resumed() -> Evidence {
    let beam = Beam { rows: 2160, blank_rows: 90, total_rows: 2250, scan_ns: 8_310_000, refresh_hz: 120.3, lines_per_ms: 270.8,
        vblank_fraction: 0.23, scans: 115, idle_from: 2521, longest_idle_ns: 17_000_000, idle_scans: 30 };
    let mut check = RestartCheck::default();
    for _ in 0..FIXED_FRAMES { check.push(Restart::NotTested); }
    Evidence { beam: Some(Ok(beam)), check, content_hz: 66.0, display_hz: 120.3, check_rates: Some((64.0, 66.0)), ..evidence() }
}

fn starved() -> RestartCheck {
    let mut check = RestartCheck::default();
    for _ in 0..CHECK_FRAMES { check.push(Restart::NotTested); }
    check
}

#[test]
fn a_resumed_stream_on_a_display_seen_waiting_for_presents_is_not_called_no_vrr_untested() {
    let resumed = resumed();
    assert_eq!(decide(&resumed), Verdict::Checking, "was off (no VRR) half a second after the calibration");
    // The whole check, starved: the display idles, the presents just never came while it did.
    let verdict = decide(&Evidence { check: starved(), ..resumed });
    assert_eq!(verdict.line(), "off (no idle at this frame rate)");
    assert!(!verdict.fixed_refresh());
    assert!(!verdict.refreshes_on_present(), "no present restarted a scan: nothing to return to tearing on");
    // Presents tested while it idled that did not restart it: it tears, V-Sync it is.
    assert!(decide(&Evidence { check: missed(), ..resumed }).fixed_refresh());
    // At the stream's rate the check passes as on the fresh launch.
    assert!(decide(&Evidence { check: evidence().check, ..resumed }).refreshes_on_present());
    // A single reading past a scan (a mode switch during the calibration) proves no waiting.
    let once = match resumed.beam { Some(Ok(beam)) => Beam { idle_scans: 1, ..beam }, _ => unreachable!() };
    assert!(decide(&Evidence { beam: Some(Ok(once)), ..resumed }).fixed_refresh());
}

#[test]
fn the_quick_rule_waits_for_a_settled_rate_not_for_the_streams() {
    // The same monitor scanning at its own 120 Hz: it never idles.
    let fixed = match resumed().beam { Some(Ok(beam)) => Beam { vblank_fraction: 0.04, longest_idle_ns: beam.scan_ns, idle_scans: 0, ..beam }, _ => unreachable!() };
    let steady = Evidence { beam: Some(Ok(fixed)), ..resumed() };
    assert!(decide(&steady).fixed_refresh(), "a steady 66 of 120: a game at its own rate on a fixed display");
    assert!(decide(&Evidence { check_rates: Some((20.0, 20.0)), content_hz: 20.0, ..steady }).fixed_refresh(), "a still desktop");
    assert!(decide(&Evidence { check_rates: None, ..steady }).fixed_refresh(), "no second measured yet");
    // Still settling (66 to 120 presents a second while the check ran): the whole check decides.
    let settling = Evidence { check_rates: Some((66.0, 120.0)), ..steady };
    assert_eq!(decide(&settling), Verdict::Checking);
    assert!(decide(&Evidence { check: starved(), ..settling }).fixed_refresh(), "and still tells it");
    // The last presents ran busy and no whole second has said so yet: the next presents or the
    // whole check decide.
    let rising = Evidence { recent_hz: 120.0, ..steady };
    assert_eq!(decide(&rising), Verdict::Checking);
    assert!(decide(&Evidence { recent_hz: 70.0, ..rising }).fixed_refresh(), "a burst that passed");
    assert!(decide(&Evidence { check: starved(), ..rising }).fixed_refresh(), "and still tells it");
}

#[test]
fn a_calibration_tells_a_display_that_waited_from_one_that_scans_at_its_own_rate() {
    // 4K at 120 Hz: 2250 lines in 8.31 ms. Scans straight through, the counter wrapping each time.
    const LINES: f64 = 270.8;
    let straight: Vec<u64> = (0..150).map(|i| 1_000_000 + i * 8_310_000).collect();
    let calibrated = |starts: &[u64]| calibrate(&counting(starts, 90, 2160, LINES, 250_000, 1_200_000_000), 2160).unwrap();
    let fixed = calibrated(&straight);
    assert!(!fixed.idled() && fixed.idle_scans == 0, "{fixed:?}");
    // Waiting once, 17 ms: a mode switch could do that, so it is not enough.
    let mut once = straight.clone();
    for start in once.iter_mut().skip(40) { *start += 8_700_000; }
    let switched = calibrated(&once);
    assert!(switched.idle_scans == 1 && !switched.idled(), "{switched:?}");
    // Waiting for presents now and then.
    let mut waited = once.clone();
    for start in waited.iter_mut().skip(80) { *start += 8_700_000; }
    let variable = calibrated(&waited);
    assert!(variable.idle_scans == 2 && variable.idled(), "{variable:?}");
}

/// Manual: `cargo test -p nightjar-desktop scanline_dump -- --ignored --nocapture` while a
/// stream (or anything that presents) runs on the primary monitor. Prints raw readings.
#[cfg(windows)]
#[test]
#[ignore = "reads the primary monitor's real scan line"]
fn scanline_dump() {
    use std::time::Duration;
    use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
    // SAFETY: the desktop window handle is always valid.
    let scan = Scanline::open(unsafe { GetDesktopWindow() }, std::time::Instant::now()).expect("scanline on the primary monitor");
    println!("monitor {}", scan.device_name());
    let samples = collect(&scan, Duration::from_millis(1000), Duration::from_micros(250));
    for s in samples.iter().take(400) { println!("{} {} {}", s.at_ns, s.line, u8::from(s.vblank)); }
    println!("{:?}", calibrate(&samples, 0));
}
