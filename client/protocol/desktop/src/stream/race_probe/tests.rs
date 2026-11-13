use super::super::scanline::{Restart, CHECK_FRAMES, CHECK_MISSES, FIXED_FRAMES};
use super::*;

fn beam() -> Beam {
    Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0, vblank_fraction: 0.75, scans: 50, idle_from: 1840, longest_idle_ns: 4_100_000, idle_scans: 0 }
}

fn around() -> Surroundings { Surroundings { tearing: true, fullscreen: true, focused: true, content_hz: 60.0 } }

#[test]
fn a_calibration_for_another_monitor_is_dropped() {
    let slot = Calibration::default();
    *slot.lock().unwrap() = Some((1, Ok(beam())));
    assert!(take_current(&slot, 2).is_none(), "measured the monitor the window has left");
    assert!(slot.lock().unwrap().is_none(), "and gone, not kept for later");
    *slot.lock().unwrap() = Some((2, Ok(beam())));
    assert!(matches!(take_current(&slot, 2), Some(Ok(_))));
}

#[test]
fn a_probe_ignores_the_stale_calibration_and_takes_the_new_one() {
    let mut probe = RaceProbe::new(true, "beam race");
    *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
    assert!(probe.apply_follow(Follow::Moved), "the new display is measured again");
    probe.decide(around());
    assert!(probe.beam.is_none(), "the old display's beam is not the new one's");
    *probe.calibration.lock().unwrap() = Some((1, Ok(Beam { rows: 1080, ..beam() })));
    probe.decide(around());
    assert!(matches!(probe.beam, Some(Ok(b)) if b.rows == 1080));
}

#[test]
fn a_monitor_that_cannot_be_opened_says_no_scanline() {
    let mut probe = RaceProbe::new(true, "beam race");
    *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
    probe.decide(around());
    assert!(probe.beam.is_some());
    assert!(!probe.apply_follow(Follow::Lost), "there is nothing to measure");
    probe.decide(around());
    assert_eq!(probe.line(), "off (no scanline)");
    assert!(probe.beam.is_none() && probe.delays.is_empty(), "nothing of the old display is kept");
}

#[test]
fn staying_on_the_monitor_keeps_the_measurements() {
    let mut probe = RaceProbe::new(true, "beam race");
    *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
    probe.decide(around());
    assert!(!probe.apply_follow(Follow::Same));
    assert!(probe.beam.is_some());
    assert_eq!(probe.longest_idle_ns, 4_100_000);
}

#[test]
fn the_rate_is_the_presents_measured_never_above_the_streams() {
    let start = Instant::now();
    let mut rate = PresentRate::default();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    for i in 0..=20 { rate.note(at(i * 50)); }
    assert_eq!(rate.content_hz(120.0), 20.0, "a still desktop the host repeats at 20 fps, on a 120 fps stream");
    assert_eq!(rate.content_hz(0.0), 20.0, "with the stream's rate unknown");
    let mut fast = PresentRate::default();
    for i in 0..=250 { fast.note(at(i * 4)); }
    assert_eq!(fast.content_hz(120.0), 120.0, "never above the stream's");
    assert_eq!(PresentRate::default().content_hz(60.0), 60.0, "the stream's until a second is measured");
}

#[test]
fn the_last_presents_tell_ahead_of_the_second_and_leave_the_measured_rate_alone() {
    let start = Instant::now();
    let at = |ms: f64| start + std::time::Duration::from_secs_f64(ms / 1000.0);
    let mut rate = PresentRate::default();
    assert_eq!(rate.recent_hz(120.0), 0.0, "before there were enough");
    // A first second of 77 presents.
    let mut ms = 0.0;
    for _ in 0..=77 { rate.note(at(ms)); ms += 1000.0 / 77.0; }
    assert!((rate.content_hz(120.0) - 77.0).abs() < 2.0, "{:?}", rate.hz);
    assert!((rate.recent_hz(120.0) - 77.0).abs() < 2.0, "{:?}", rate.recent);
    // A quarter of a second at 120 a second tells, long before the next second ends.
    for _ in 0..60 { rate.note(at(ms)); ms += 1000.0 / 120.0; }
    assert!((rate.recent_hz(120.0) - 120.0).abs() < 2.0, "{:?}", rate.recent);
    assert_eq!(rate.recent_hz(60.0), 60.0, "never above the stream's");
    assert!((rate.content_hz(120.0) - 77.0).abs() < 2.0, "the second still measures: {:?}", rate.hz);
    // It is no latch: the next presents read lower at once.
    for _ in 0..50 { rate.note(at(ms)); ms += 1000.0 / 60.0; }
    assert!((rate.recent_hz(120.0) - 60.0).abs() < 2.0, "{:?}", rate.recent);
}

/// Presents `n` pictures `1 / hz` apart that start no scan, each one timed as `after_present` does.
fn present_at(probe: &mut RaceProbe, around: Surroundings, at: &mut Instant, hz: f64, n: u32) {
    for _ in 0..n {
        *at += std::time::Duration::from_secs_f64(1.0 / hz);
        probe.rate.note(*at);
        present(probe, around, 1);
    }
}

/// A 120 Hz display calibrated while it scanned at its top rate: nightjar#75's monitor on a
/// resume, 4% of the readings in the blanking and no idle, and a fixed 120 Hz one alike.
fn at_top_rate() -> Beam {
    Beam { rows: 2160, blank_rows: 87, total_rows: 2247, scan_ns: 8_320_000, refresh_hz: 120.2, lines_per_ms: 270.0,
        vblank_fraction: 0.04, scans: 144, idle_from: 3229, longest_idle_ns: 8_300_000, idle_scans: 0 }
}

#[test]
fn a_resume_is_judged_at_the_rate_its_check_ran_not_at_its_first_second() {
    use super::super::present::{after_display_check, PacingMode};
    let stream = Surroundings { content_hz: 120.0, ..around() };
    // nightjar#75's three resumes: 77, 52 and 77 presents in the first second, 120 a second after.
    for first_second in [77.0, 52.0] {
        let mut at = Instant::now();
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(stream);
        present_at(&mut probe, stream, &mut at, first_second, first_second as u32 + 2);
        *probe.calibration.lock().unwrap() = Some((0, Ok(at_top_rate())));
        present_at(&mut probe, stream, &mut at, 120.0, FIXED_FRAMES + 2);
        assert_eq!(*probe.verdict(), Verdict::Checking, "was off (no VRR) here, half a second after the calibration");
        present_at(&mut probe, stream, &mut at, 120.0, CHECK_FRAMES);
        assert_eq!(probe.line(), "off (no idle at this frame rate)", "as on a fresh launch");
        assert_eq!(after_display_check(PacingMode::HostTimes, probe.verdict(), PacingMode::Vsync), PacingMode::HostTimes);
        assert_eq!(after_display_check(PacingMode::Immediate, probe.verdict(), PacingMode::Vsync), PacingMode::Immediate);
    }
}

#[test]
fn a_fixed_display_under_a_slower_stream_still_turns_to_v_sync_on_a_resume() {
    use super::super::present::{after_display_check, PacingMode};
    let stream = Surroundings { content_hz: 120.0, ..around() };
    // A game held at 60 on a fixed 120 Hz panel: the rate its check ran at is the first second's.
    for (first_second, rate) in [(60.0, 60.0), (52.0, 60.0), (77.0, 77.0)] {
        let mut at = Instant::now();
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(stream);
        present_at(&mut probe, stream, &mut at, first_second, first_second as u32 + 2);
        *probe.calibration.lock().unwrap() = Some((0, Ok(at_top_rate())));
        present_at(&mut probe, stream, &mut at, rate, FIXED_FRAMES + 2);
        assert!(probe.fixed_refresh(), "{first_second} then {rate}: {}", probe.line());
        assert_eq!(after_display_check(PacingMode::HostTimes, probe.verdict(), PacingMode::Vsync), PacingMode::Vsync);
    }
}

#[test]
fn a_fixed_display_under_a_game_that_swings_still_turns_to_v_sync() {
    use super::super::present::{after_display_check, PacingMode};
    // 65 and 90 fps in turns, 0.4 s each, on a fixed 120 Hz panel: whole seconds stay below the
    // busy rate of 84, a quarter of a second at 90 is above it. That must not read as a busy
    // check, which would keep the tearing for the rest of the stream.
    let stream = Surroundings { content_hz: 120.0, ..around() };
    let mut at = Instant::now();
    let mut probe = RaceProbe::new(true, "display check");
    probe.apply_surroundings(stream);
    present_at(&mut probe, stream, &mut at, 78.0, 80);
    *probe.calibration.lock().unwrap() = Some((0, Ok(at_top_rate())));
    for _ in 0..4 {
        present_at(&mut probe, stream, &mut at, 65.0, 26);
        present_at(&mut probe, stream, &mut at, 90.0, 36);
    }
    assert!(probe.fixed_refresh(), "{}", probe.line());
    assert_eq!(after_display_check(PacingMode::HostTimes, probe.verdict(), PacingMode::Vsync), PacingMode::Vsync);
}

#[test]
fn a_burst_after_a_stall_leaves_a_fixed_display_to_its_check() {
    let stream = Surroundings { content_hz: 120.0, ..around() };
    // A game at 60 on a fixed 120 Hz panel stalls and catches up with 26 pictures, wherever those
    // fall among the presents the recent rate is taken over. No whole second reads busy (83 at
    // most): only the recent rate does, for one reading, when its presents start with the burst
    // (with the stall among them, the catch-up cancels out).
    let mut read_busy = false;
    for phase in 0..PresentRate::RECENT {
        let mut at = Instant::now();
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(stream);
        present_at(&mut probe, stream, &mut at, 60.0, 62);
        *probe.calibration.lock().unwrap() = Some((0, Ok(at_top_rate())));
        present_at(&mut probe, stream, &mut at, 60.0, phase);
        at += std::time::Duration::from_millis(433);
        present_at(&mut probe, stream, &mut at, 500.0, 26);
        present_at(&mut probe, stream, &mut at, 60.0, 5);
        read_busy |= probe.rate.recent_hz(120.0) > 100.0;
        present_at(&mut probe, stream, &mut at, 60.0, CHECK_FRAMES);
        assert!(probe.fixed_refresh(), "phase {phase}: {}", probe.line());
        assert_eq!(probe.generation, 0, "phase {phase}: not measured again, it never read busy");
    }
    assert!(read_busy, "the burst has to read busy somewhere, or this tests nothing");
}

#[test]
fn a_fixed_refresh_stays_fixed_under_the_v_sync_it_gets_until_the_window_moves() {
    let mut probe = RaceProbe::new(true, "display check");
    *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.01, refresh_hz: 119.5, ..beam() })));
    probe.decide(around());
    assert!(!probe.fixed_refresh(), "the calibration alone decides nothing");
    for _ in 0..super::super::scanline::FIXED_FRAMES { probe.check.push(super::super::scanline::Restart::NotTested); }
    probe.decide(around());
    assert!(probe.fixed_refresh(), "{}", probe.line());
    assert!(probe.beam().is_some());
    // V-Sync presents do not tear: that is not the reason the display was found fixed.
    probe.decide(Surroundings { tearing: false, ..around() });
    assert!(probe.fixed_refresh(), "{}", probe.line());
    // Nor do the window or the rate change the display.
    assert!(!present(&mut probe, Surroundings { fullscreen: false, ..around() }, 1));
    assert!(!present(&mut probe, Surroundings { focused: false, ..around() }, 1));
    assert!(!present(&mut probe, Surroundings { content_hz: 20.0, ..around() }, 1));
    assert!(!present(&mut probe, around(), 1));
    assert!(probe.fixed_refresh(), "{}", probe.line());
    assert!(probe.apply_follow(Follow::Moved));
    assert!(!probe.fixed_refresh(), "another display is checked again");
    assert!(probe.beam().is_none());
}

/// Presents `n` pictures through `fold`, as `after` does on Windows, each with `result` when
/// the check takes it (a calibrated display, as `test_present`). Returns whether the display
/// was to be calibrated again.
fn present_with(probe: &mut RaceProbe, around: Surroundings, n: u32, result: Restart) -> bool {
    let mut again = false;
    for _ in 0..n {
        again |= probe.fold(around, Follow::Same, |probe| if probe.beam().is_some() { probe.check.push(result) });
    }
    again
}

/// Presents that start no scan.
fn present(probe: &mut RaceProbe, around: Surroundings, n: u32) -> bool { present_with(probe, around, n, Restart::NotTested) }

#[test]
fn what_a_composed_window_saw_does_not_decide_the_display_once_it_is_fullscreen() {
    // This PC's 240 Hz VRR panel behind a window, composed: little blanking, and no present
    // starts a scan.
    let windowed = Surroundings { fullscreen: false, ..around() };
    let mut probe = RaceProbe::new(true, "display check");
    assert!(!probe.apply_surroundings(windowed), "where the stream starts is not a change");
    *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.05, ..beam() })));
    assert!(!present(&mut probe, windowed, CHECK_FRAMES));
    assert_eq!(probe.check, RestartCheck::default(), "composed presents feed no check");
    assert_eq!(probe.line(), "off (not fullscreen)");
    // What the check had from those presents before: starved under a 60 fps stream.
    for _ in 0..CHECK_FRAMES { probe.check.push(Restart::NotTested); }
    let mut presented = false;
    assert!(probe.fold(around(), Follow::Same, |_| presented = true), "F11: the display is measured again");
    assert!(presented, "this present is the check's");
    assert_eq!(*probe.verdict(), Verdict::Checking, "not no VRR");
    assert_eq!(probe.check, RestartCheck::default());
    assert!(probe.beam().is_none(), "the composed calibration is dropped");
}

#[test]
fn a_fullscreen_window_under_another_feeds_no_check_and_is_measured_again_with_the_focus() {
    let mut probe = RaceProbe::new(true, "display check");
    probe.apply_surroundings(around());
    *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.05, ..beam() })));
    assert!(!present(&mut probe, around(), 10));
    let frames = probe.check.frames;
    assert!(frames > 0);
    // Alt-Tab in borderless fullscreen: the window stays fullscreen under another, composed.
    let behind = Surroundings { focused: false, ..around() };
    assert!(!present(&mut probe, behind, CHECK_FRAMES));
    assert_eq!(probe.check.frames, frames, "composed presents feed no check");
    assert!(!probe.fixed_refresh(), "{}", probe.line());
    assert!(present(&mut probe, around(), 1), "the focus back: measured again");
    assert_eq!(probe.check, RestartCheck::default());
    // A fullscreen window that never had the focus (started from a script) is checked.
    let mut unfocused = RaceProbe::new(true, "display check");
    unfocused.apply_surroundings(behind);
    assert!(unfocused.checking());
}

#[test]
fn a_check_at_a_busy_rate_is_measured_again_when_the_rate_falls_not_called_no_vrr() {
    // This PC's 240 Hz VRR panel calibrated while it scanned at its top rate.
    let vrr_busy = Beam { vblank_fraction: 0.12, ..beam() };
    let fast = Surroundings { content_hz: 200.0, ..around() };
    let still = Surroundings { content_hz: 20.0, ..around() };
    // The check starved at 200 fps: no idle at this rate.
    let mut probe = RaceProbe::new(true, "display check");
    probe.apply_surroundings(fast);
    *probe.calibration.lock().unwrap() = Some((0, Ok(vrr_busy)));
    assert!(!present(&mut probe, fast, CHECK_FRAMES + 1));
    assert_eq!(probe.line(), "off (no idle at this frame rate)");
    // A still screen the host repeats at 20 fps.
    assert!(present(&mut probe, still, 1), "measured again at the slower rate");
    assert!(!probe.fixed_refresh(), "{}", probe.line());
    assert_eq!(*probe.verdict(), Verdict::Checking);
    // Still running, past the presents the quick rule waits for, at 200 fps; then a menu at 60.
    let mut probe = RaceProbe::new(true, "display check");
    probe.apply_surroundings(fast);
    *probe.calibration.lock().unwrap() = Some((0, Ok(vrr_busy)));
    assert!(!present(&mut probe, fast, FIXED_FRAMES * 2));
    assert_eq!(*probe.verdict(), Verdict::Checking);
    assert!(present(&mut probe, around(), 1));
    assert!(!probe.fixed_refresh(), "{}", probe.line());
    // A display that is fixed after all is found at the slower rate.
    *probe.calibration.lock().unwrap() = Some((1, Ok(Beam { vblank_fraction: 0.01, ..beam() })));
    assert!(!present(&mut probe, around(), FIXED_FRAMES + 1));
    assert!(probe.fixed_refresh(), "{}", probe.line());
}

#[test]
fn a_check_that_failed_at_a_busy_rate_is_measured_again_when_the_rate_falls() {
    // At 200 fps behind a deep queue the tested presents missed: the check failed, and at that
    // rate it says only that no restart was seen.
    let fast = Surroundings { content_hz: 200.0, ..around() };
    let mut probe = RaceProbe::new(true, "display check");
    probe.apply_surroundings(fast);
    *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.12, ..beam() })));
    assert!(!present_with(&mut probe, fast, CHECK_MISSES + 1, Restart::Missed));
    assert!(probe.check.failed());
    assert_eq!(probe.line(), "off (no VRR scan restart)");
    // A menu the host repeats at 20 fps: measured again, not no VRR.
    assert!(present(&mut probe, Surroundings { content_hz: 20.0, ..around() }, 1));
    assert!(!probe.fixed_refresh(), "{}", probe.line());
    assert_eq!(*probe.verdict(), Verdict::Checking);
    // A check that failed at a slow rate stays failed.
    let mut slow = RaceProbe::new(true, "display check");
    slow.apply_surroundings(around());
    *slow.calibration.lock().unwrap() = Some((0, Ok(beam())));
    assert!(!present_with(&mut slow, around(), CHECK_MISSES + 1, Restart::Missed));
    assert!(!present(&mut slow, Surroundings { content_hz: 20.0, ..around() }, 1));
    assert!(slow.check.failed());
}

#[test]
fn a_recheck_measures_a_no_vrr_display_again_and_finds_it_following_presents() {
    let mut probe = RaceProbe::new(true, "display check");
    *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.01, ..beam() })));
    assert!(!present(&mut probe, around(), FIXED_FRAMES + 1));
    assert!(probe.fixed_refresh(), "{}", probe.line());
    probe.recheck();
    assert!(probe.fixed_refresh(), "until the next present");
    assert!(present(&mut probe, around(), 1), "calibrated again");
    assert_eq!(*probe.verdict(), Verdict::Checking);
    assert!(probe.beam().is_none() && probe.check == RestartCheck::default());
    assert!(!present(&mut probe, around(), 1), "asked once");
    // The display waits for presents now (nightjar#72 after its resume settled) and they restart its scan.
    *probe.calibration.lock().unwrap() = Some((1, Ok(Beam { vblank_fraction: 0.45, longest_idle_ns: 17_000_000, idle_scans: 30, ..beam() })));
    assert!(!present_with(&mut probe, around(), super::super::scanline::CHECK_TESTED + 1, Restart::Began));
    assert!(probe.verdict().refreshes_on_present(), "{}", probe.line());
    // A display that still scans at its own rate is found so again.
    let mut fixed = RaceProbe::new(true, "display check");
    *fixed.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.01, ..beam() })));
    assert!(!present(&mut fixed, around(), FIXED_FRAMES + 1));
    fixed.recheck();
    assert!(present(&mut fixed, around(), 1));
    *fixed.calibration.lock().unwrap() = Some((1, Ok(Beam { vblank_fraction: 0.01, ..beam() })));
    assert!(!present(&mut fixed, around(), FIXED_FRAMES + 1));
    assert!(fixed.fixed_refresh(), "{}", fixed.line());
}

#[cfg(target_os = "linux")]
fn drm_probe(reader: DrmReader) -> RaceProbe {
    let mut probe = RaceProbe::new(false, "display check");
    probe.drm_reader = Some(reader);
    probe
}

#[cfg(target_os = "linux")]
fn reads_off(_: &super::super::drm_vrr::Display) -> super::super::drm_vrr::Reading {
    super::super::drm_vrr::Reading::Vrr { enabled: false, output: "DP-1".into(), capable: Some(1) }
}

#[cfg(target_os = "linux")]
fn reads_on(_: &super::super::drm_vrr::Display) -> super::super::drm_vrr::Reading {
    super::super::drm_vrr::Reading::Vrr { enabled: true, output: "DP-1".into(), capable: Some(1) }
}

#[cfg(target_os = "linux")]
fn dp1(_: u32) -> super::super::drm_vrr::Display {
    super::super::drm_vrr::Display { name: Some("DP-1".into()), width: 3440, height: 1440, refresh_hz: 144.0 }
}

/// Waits for the background read. A quick read can land before the present that started it
/// decides, which takes it at once: then the probe holds it and the slot stays empty.
#[cfg(target_os = "linux")]
fn read_done(probe: &RaceProbe) {
    for _ in 0..5000 {
        if probe.drm.is_some() || probe.drm_slot.lock().unwrap().is_some() { return; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the read never came");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_reads_drm_a_second_after_the_window_settles_again_when_it_moves_and_on_a_recheck() {
    let start = Instant::now();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    let windowed = Surroundings { fullscreen: false, ..around() };
    let mut probe = drm_probe(reads_off);
    probe.fold_unavailable(windowed, 1, at(0), dp1);
    assert_eq!(probe.line(), "off (no scanline)", "a windowed stream shows what it did before");
    probe.fold_unavailable(around(), 1, at(100), dp1);
    assert_eq!(*probe.verdict(), Verdict::Checking, "fullscreen and focused: reading soon");
    probe.fold_unavailable(around(), 1, at(1000), dp1);
    assert!(probe.drm_slot.lock().unwrap().is_none(), "not before a second");
    probe.fold_unavailable(around(), 1, at(1100), dp1);
    read_done(&probe);
    probe.fold_unavailable(around(), 1, at(1110), dp1);
    assert!(probe.fixed_refresh(), "{}", probe.line());
    // Another display: read again once settled there.
    probe.fold_unavailable(around(), 2, at(1200), dp1);
    assert_eq!(*probe.verdict(), Verdict::Checking, "the old display's reading is dropped");
    probe.fold_unavailable(around(), 2, at(1300), |_| unreachable!("not settled yet"));
    probe.fold_unavailable(around(), 2, at(2300), dp1);
    read_done(&probe);
    probe.fold_unavailable(around(), 2, at(2310), dp1);
    assert!(probe.fixed_refresh(), "{}", probe.line());
    // A recheck after the user turned VRR on.
    probe.drm_reader = Some(reads_on as DrmReader);
    probe.recheck();
    probe.fold_unavailable(around(), 2, at(2400), dp1);
    assert_eq!(*probe.verdict(), Verdict::Checking);
    probe.fold_unavailable(around(), 2, at(3400), dp1);
    read_done(&probe);
    probe.fold_unavailable(around(), 2, at(3410), dp1);
    assert!(probe.verdict().refreshes_on_present(), "{}", probe.line());
}

#[cfg(target_os = "linux")]
#[test]
fn gamescope_keeps_the_pacing_and_reads_no_drm() {
    assert!(drm_reader("display check", true).is_none(), "a Steam Deck panel has no VRR: V-Sync there is unmeasured");
    assert!(drm_reader("display check", false).is_some());
    let start = Instant::now();
    let mut probe = RaceProbe::new(false, "display check");
    for ms in [0, 1500, 3000, 10_000] {
        probe.fold_unavailable(around(), 1, start + std::time::Duration::from_millis(ms), |_| unreachable!("no read under gamescope"));
        assert_eq!(probe.line(), "off (no scanline)");
    }
    assert!(probe.drm_slot.lock().unwrap().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_takes_the_drm_reading_for_its_display_and_a_recheck_reads_again() {
    use super::super::drm_vrr::Reading;
    let off = Reading::Vrr { enabled: false, output: "DP-1".into(), capable: Some(1) };
    let on = Reading::Vrr { enabled: true, output: "DP-1".into(), capable: Some(1) };
    let mut probe = drm_probe(reads_off);
    probe.decide_unavailable(around());
    assert_eq!(*probe.verdict(), Verdict::Checking, "no longer \"no scanline\" while it reads");
    *probe.drm_slot.lock().unwrap() = Some((0, off));
    probe.decide_unavailable(around());
    assert!(probe.fixed_refresh(), "{}", probe.line());
    probe.recheck();
    assert!(probe.apply_recheck());
    probe.decide_unavailable(around());
    assert_eq!(*probe.verdict(), Verdict::Checking, "the old reading is dropped");
    *probe.drm_slot.lock().unwrap() = Some((0, on.clone()));
    probe.decide_unavailable(around());
    assert_eq!(*probe.verdict(), Verdict::Checking, "a read from before the recheck says nothing");
    *probe.drm_slot.lock().unwrap() = Some((1, on));
    probe.decide_unavailable(around());
    assert!(probe.verdict().refreshes_on_present(), "{}", probe.line());
    *probe.drm_slot.lock().unwrap() = Some((1, Reading::Unknown("two outputs".into())));
    probe.decide_unavailable(around());
    assert_eq!(probe.line(), "off (no scanline)", "as before there was a reading");
}

#[test]
fn a_rate_that_hovers_about_seventy_percent_does_not_restart_the_check() {
    let fast = Surroundings { content_hz: 200.0, ..around() };
    let mut probe = RaceProbe::new(true, "display check");
    probe.apply_surroundings(fast);
    *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.12, ..beam() })));
    assert!(!present(&mut probe, fast, CHECK_FRAMES + 1));
    assert_eq!(probe.line(), "off (no idle at this frame rate)");
    // 243.9 Hz: 70% is 170.7, 60% is 146.3.
    assert!(!present(&mut probe, Surroundings { content_hz: 160.0, ..around() }, 10), "just under 70%");
    assert_eq!(probe.line(), "off (no idle at this frame rate)");
    assert!(present(&mut probe, Surroundings { content_hz: 140.0, ..around() }, 1), "under 60%");
}
