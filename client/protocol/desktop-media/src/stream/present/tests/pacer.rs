//! `Pacer` queue, host-time scheduling, refresh grid, FPS cap, and steered-hold tests.

use super::super::*;
use super::decoded;

#[test]
fn immediate_pacing_takes_the_newest_ready_picture_without_holding_it() {
    let mut pacer = Pacer::new(PacingMode::Immediate, 16_667);
    assert!(pacer.next_due().is_none());
    for ready in [10, 20, 30] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(25);
    let picture = picture.unwrap();
    assert_eq!(picture.decoded.ready_us, 20);
    assert_eq!(picture.present_us, 20);
    assert_eq!(picture.hold_us, 0);
    assert_eq!(dropped, 1);
    assert_eq!(pacer.next_due(), Some(30));
    let (picture, dropped) = pacer.take_due(30);
    assert_eq!(picture.unwrap().decoded.ready_us, 30);
    assert_eq!(dropped, 0);
    assert!(pacer.next_due().is_none());
}

#[test]
fn the_front_picture_says_which_raced_frame_it_is_and_when_it_is_due() {
    let mut pacer = Pacer::new(PacingMode::BeamRace, 16_667);
    assert_eq!(pacer.front_race(), None);
    let mut picture = decoded(40);
    picture.race_seq = 7;
    pacer.accept(picture);
    pacer.accept(decoded(50));
    assert_eq!(pacer.front_race(), Some((7, 40)));
}

// Real VRR's delay follows the link and stays within a ceiling above the typical frame: on Wi-Fi
// the unbounded percentile rule held every frame for the lateness tail (ADAPTIVE_SPREAD_US).
#[test]
fn host_times_bound_the_wait_for_late_frames() {
    let pacer = Pacer::new(PacingMode::HostTimes, 6_944);
    assert_eq!(pacer.clock.settings.max_spread_us, Some(rubylight_protocol::present_clock::ADAPTIVE_SPREAD_US));
    assert_eq!(pacer.clock.settings.min_interval_us, 6_944);
    assert_eq!(pacer.clock.settings.max_hold_us, ClockSettings::default().max_hold_us, "the backlog bound is unchanged");
}

// Regression (nightjar#80): PC's frame times held pictures by the lateness a decoder behind
// the stream made, up to the longest hold.
#[test]
fn host_times_show_a_picture_at_once_while_the_decoder_is_behind() {
    let mut pacer = Pacer::new(PacingMode::HostTimes, 0);
    let mut reference = pacer.clock.clone();
    let mut ready = 0;
    let mut accept = |pacer: &mut Pacer, queued: usize| {
        ready += 10_000;
        let rtp = (ready * 9 / 100) as u32;
        let mut picture = decoded(ready);
        picture.rtp = rtp;
        pacer.set_decode_queue(queued);
        pacer.accept(picture);
        let last = crate::stream::pipeline::lock(&pacer.pending).back().map(|p| (p.present_us, p.hold_us));
        (rtp, ready, last.unwrap())
    };
    // Review of nightjar#80: a network burst of three frames leaves the queue non-empty for
    // two of them; they are paced as usual.
    for queued in [2, 1, 0] {
        let (rtp, ready, got) = accept(&mut pacer, queued);
        let s = reference.schedule(rtp, ready);
        assert_eq!(got, (s.present_us, s.hold_us), "burst frame with {queued} queued");
    }
    // A decoder that cannot keep up keeps the queue non-empty: after a few frames each
    // picture is shown as soon as it is ready and its lateness is not learnt.
    for i in 1..=12 {
        let (rtp, ready, got) = accept(&mut pacer, 3);
        let s = if i < rubylight_protocol::present_clock::BEHIND_AFTER_FRAMES {
            reference.schedule(rtp, ready)
        } else {
            reference.schedule_behind(rtp, ready)
        };
        assert_eq!(got, (s.present_us, s.hold_us), "backlog frame {i}");
    }
    // Caught up: scheduled as before.
    let (rtp, ready, got) = accept(&mut pacer, 0);
    let s = reference.schedule(rtp, ready);
    assert_eq!(got, (s.present_us, s.hold_us));
}

/// The PC's frame times without VRR wait for the refresh, whatever the V-Sync setting says, and a
/// VRR stream keeps its own pacing.
#[test]
fn the_pcs_frame_times_without_vrr_wait_for_the_refresh() {
    let host = crate::config::Settings { vrr: false, vsync: false, host_frame_times: true, ..Default::default() };
    assert_eq!(pacing(&host, true), (PacingMode::Vsync, false));
    assert_eq!(without_vrr(&host), PacingMode::Vsync);
    let vrr = crate::config::Settings { host_frame_times: true, ..Default::default() };
    assert_eq!(pacing(&vrr, true), (PacingMode::HostTimes, true), "VRR's own On");
    assert_eq!(without_vrr(&vrr), PacingMode::Vsync, "and its fall back on a fixed refresh");
    let grid = refresh_grid(1_000, 8_333_333);
    assert_eq!((grid.anchor_us, grid.interval_us, grid.lead_us, grid.slack_us), (1_000, 8_333, 2_083, 4_166));
}

/// A picture of the PC's frame times goes out on the refresh grid: the deadline plus the lead
/// before the refresh the clock picked for it (the clock sees it ready the deadline later), never
/// before it is ready; every refresh gets one picture of a stream at the display's rate.
#[test]
fn the_pcs_frame_times_place_pictures_on_the_refresh_grid() {
    let interval = 8_333;
    let deadline = 5_000;
    let mut pacer = Pacer::new(PacingMode::Vsync, interval).with_frame_pacing(FramePacing::Smoothness, 120).with_host_frame_times(true);
    assert!(pacer.host_grid());
    let grid = refresh_grid(0, interval * 1000);
    pacer.set_grid(Some(grid), deadline);
    let mut reference = pacer.clock.clone();
    let mut refreshes = Vec::new();
    for i in 1..=240u64 {
        // Pictures from the PC every refresh, arriving with up to 3 ms of jitter.
        let host = i * interval as u64;
        let ready = 50_000 + host + (i * 7 % 4) * 1_000;
        let mut picture = decoded(ready);
        picture.rtp = (host * 9 / 100) as u32;
        let rtp = picture.rtp;
        pacer.accept(picture);
        let p = crate::stream::pipeline::lock(&pacer.pending).pop_back().unwrap();
        let s = reference.schedule_on_grid_queued(rtp, ready + deadline as u64, grid, 0);
        assert_eq!(p.present_us, s.release_us.saturating_sub(deadline as u64).max(ready), "picture {i}");
        assert!(p.present_us >= ready, "never before it is ready");
        if s.release_us > ready + deadline as u64 {
            // Held: it goes out the lead and the deadline before its refresh.
            assert_eq!(p.present_us + deadline as u64 + grid.lead_us as u64, s.vsync_us, "picture {i}");
        }
        if i > 128 { refreshes.push(s.vsync_index); }
    }
    refreshes.dedup();
    assert_eq!(refreshes.len(), 112, "one picture a refresh once the delay is learnt");
    // The lowest latency's newest picture: the in-order pacing under it is not used.
    assert!(pacer.skips_instead_of_waiting(120.0));
    assert!(!pacer.paced(FramePacing::Smoothness));
}

/// Without the display's timing the picture goes out at its time and V-Sync takes the refresh
/// after; the clock still sees it the deadline later, so the delay it learns does not jump when
/// the timing comes and goes.
#[test]
fn the_pcs_frame_times_without_display_timing_present_at_their_time() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_host_frame_times(true);
    pacer.set_grid(None, 5_000);
    let mut reference = pacer.clock.clone();
    let mut picture = decoded(20_000);
    picture.rtp = 900;
    pacer.accept(picture);
    let p = crate::stream::pipeline::lock(&pacer.pending).pop_back().unwrap();
    let s = reference.schedule_queued(900, 25_000, 0);
    assert_eq!(p.present_us, s.present_us.saturating_sub(5_000).max(20_000));
    // Off, or a VRR stream: no grid.
    assert!(!Pacer::new(PacingMode::Vsync, 8_333).host_grid());
    assert!(!Pacer::new(PacingMode::HostTimes, 8_333).with_host_frame_times(true).host_grid());
}

/// A VRR stream with the PC's frame times that the display check turns to V-Sync learns its
/// delay on the grid from the start, and learns Real VRR's again when the check brings VRR back.
#[test]
fn the_pcs_frame_times_start_their_delay_again_between_vrr_and_the_grid() {
    let interval = 8_333;
    let grid = refresh_grid(0, interval * 1000);
    let mut pacer = Pacer::new(PacingMode::HostTimes, interval).with_host_frame_times(true);
    pacer.set_grid(Some(grid), 0);
    let mut i = 0u64;
    let mut next = |pacer: &mut Pacer| {
        i += 1;
        let host = i * interval as u64;
        let mut picture = decoded(50_000 + host + (i * 7 % 4) * 1_000);
        picture.rtp = (host * 9 / 100) as u32;
        let (rtp, ready) = (picture.rtp, picture.ready_us);
        pacer.accept(picture);
        (crate::stream::pipeline::lock(&pacer.pending).pop_back().unwrap().present_us, rtp, ready)
    };
    for _ in 0..60 {
        next(&mut pacer);
    }
    // Fallen back to V-Sync: the first picture is scheduled as by a clock that saw nothing.
    pacer.mode = PacingMode::Vsync;
    let mut fresh = pacer.clock.clone();
    fresh.reset();
    let (present, rtp, ready) = next(&mut pacer);
    assert_eq!(present, fresh.schedule_on_grid_queued(rtp, ready, grid, 0).release_us.max(ready));
    for _ in 0..60 {
        next(&mut pacer);
    }
    // VRR back: the same again, with Real VRR's schedule.
    pacer.mode = PacingMode::HostTimes;
    let mut fresh = pacer.clock.clone();
    fresh.reset();
    let (present, rtp, ready) = next(&mut pacer);
    assert_eq!(present, fresh.schedule_queued(rtp, ready, 0).present_us);
}

#[test]
fn lowest_and_balanced_pacing_show_the_newest_due_picture() {
    for mode in [FramePacing::Latency, FramePacing::Balanced] {
        let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(mode, 60);
        for ready in [1_000, 9_000, 17_000] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(17_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 2), "{mode:?}");
    }
}

#[test]
fn balanced_without_keep_only_the_newest_frame_shows_each_picture_in_turn() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Balanced, 60).with_drop_late_frames(false);
    for ready in [1_000, 9_000, 17_000] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(17_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0), "the oldest due one, as Android's queue");
    assert!(!pacer.skips_instead_of_waiting(60.0), "a kept picture waits its turn");
    // The switch is Balanced's: the lowest latency pacing and VRR always take the newest.
    for (mode, pacing) in [(PacingMode::Vsync, FramePacing::Latency), (PacingMode::Immediate, FramePacing::Balanced)] {
        let mut pacer = Pacer::new(mode, 8_333).with_frame_pacing(pacing, 60).with_drop_late_frames(false);
        for ready in [1_000, 9_000, 17_000] {
            pacer.accept(decoded(ready));
        }
        assert_eq!(pacer.take_due(17_000).1, 2, "{mode:?} {pacing:?}");
    }
    let on = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Balanced, 60);
    assert!(on.skips_instead_of_waiting(60.0), "on, the default, keeps today's skip");
}

#[test]
fn smoothest_pacing_shows_each_picture_in_turn_and_skips_only_stale_ones() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
    for ready in [1_000, 9_000, 17_000] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(17_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0), "16 ms old: still within a frame");
    let (picture, dropped) = pacer.take_due(26_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 1), "17 ms old with a newer one due: skipped");
    assert!(pacer.next_due().is_none());
    pacer.accept(decoded(30_000));
    let (picture, dropped) = pacer.take_due(60_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (30_000, 0), "a lone late picture still shows");
    let mut vrr = Pacer::new(PacingMode::Immediate, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
    vrr.accept(decoded(1_000));
    vrr.accept(decoded(2_000));
    assert_eq!(vrr.take_due(2_000).1, 1, "without V-Sync the setting does not apply");
}

#[test]
fn the_fps_limit_gives_each_picture_its_own_slot_holding_it_at_most_a_frame() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::CapFps, 60);
    // Two pictures that came 2 ms apart, then one late.
    for ready in [10_000, 12_000, 60_000] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(12_000);
    let picture = picture.unwrap();
    assert_eq!((picture.decoded.ready_us, picture.present_us, picture.hold_us, dropped), (10_000, 10_000, 0, 0));
    assert_eq!(pacer.next_due(), Some(26_666), "the early one waits for the next slot");
    assert!(pacer.take_due(20_000).0.is_none());
    let (picture, dropped) = pacer.take_due(26_666);
    let picture = picture.unwrap();
    assert_eq!((picture.decoded.ready_us, picture.hold_us, dropped), (12_000, 14_666, 0));
    assert_eq!(pacer.next_due(), Some(60_000), "a late picture starts the slots again");
    pacer.accept(decoded(60_500));
    pacer.accept(decoded(61_000));
    assert_eq!(pacer.take_due(100_000).1, 2, "pictures more than a frame old give way to the newest due one");
}

/// The phase report, the lock and the hold, end to end with the stream loop: the report aims at the
/// present lead (`held_present_lead_us`), the lock puts the picture `DEFAULT_MARGIN_NS` past that
/// aim, the loop takes it (`Pacer::take_due`) and holds it until the refresh that opens its turn.
/// Every present then goes out just after that refresh, one hold apart, and the hold really waits;
/// aimed at the deadline the turn must make, the lock delivered after the present target and the
/// hold never waited.
#[test]
fn a_steered_stream_meets_its_hold_at_the_present_lead() {
    use crate::stream::phase;
    use rubylight_protocol::phase_lock::PhaseLock;
    const REFRESH_US: u64 = 8_333;
    const HELD: u32 = 2;
    const MARGIN_US: u64 = 5_000;
    const GUARD_US: u64 = phase::HELD_LATCH_GUARD_US;
    // Where a steering host puts the early edge past the aim (`PhaseLock::DEFAULT_MARGIN_NS`).
    const LOCK_US: i64 = PhaseLock::DEFAULT_MARGIN_NS / 1_000;
    let lead = phase::held_present_lead_us(REFRESH_US, GUARD_US);
    assert_eq!(lead, REFRESH_US - GUARD_US);
    let mut pacer = Pacer::new(PacingMode::Vsync, i64::from(HELD) * REFRESH_US as i64).with_frame_pacing(FramePacing::Latency, 60);
    let mut anchor = phase::HeldAnchor::default();
    // The last picture was shown at refresh 100 (t = 833_300): the next turn is refresh 102.
    let mut shown = Some(100u32);
    let mut presents = Vec::new();
    for turn in (102..=108).step_by(HELD as usize) {
        let turn_us = u64::from(turn) * REFRESH_US;
        let ready_us = turn_us - lead - LOCK_US as u64;
        pacer.accept(decoded(ready_us));
        let (picture, replaced) = pacer.take_due(ready_us);
        assert!(picture.is_some() && replaced == 0, "the lock delivers the picture when its turn is due");
        // The report latches on the latest grid refresh at the delivery, and the lock reads its own
        // margin one report period on from the aim.
        let latest = (ready_us / REFRESH_US) as u32;
        let (latch_us, report_period_us) = phase::held_latch(latest as u64 * REFRESH_US, latest, shown, HELD, REFRESH_US);
        let (aimed_latch, report_period_ns) = phase::aimed(Some((latch_us as i64, (report_period_us * 1_000) as i64)), lead as i64).unwrap();
        assert_eq!(report_period_ns / 1_000, (u64::from(HELD) * REFRESH_US) as i64, "the report's period is the hold");
        assert_eq!(aimed_latch - ready_us as i64 + report_period_ns / 1_000, LOCK_US, "the lock's margin, one period on");
        let wait = anchor.hold(ready_us, latest as u64 * REFRESH_US, latest, shown, HELD, REFRESH_US, MARGIN_US, GUARD_US,
            phase::EARLY_SPARE_US, phase::EARLY_FOR_US);
        assert_eq!(wait as i64, LOCK_US, "the picture is ready before its hold: the hold waits the lock's margin");
        let present = ready_us + wait;
        assert_eq!(present, turn_us - lead, "the present goes out just after the refresh that opens the turn");
        presents.push(present);
        anchor.presented(turn - 100);
        // The display shows it at that turn's refresh, where the next grid counts from.
        shown = Some(present.div_ceil(REFRESH_US) as u32);
        assert_eq!(shown, Some(turn));
    }
    assert!(presents.windows(2).all(|pair| pair[1] - pair[0] == u64::from(HELD) * REFRESH_US), "one hold apart: {presents:?}");
}

/// A stream faster than the held rate, in the renderer's order: the loop sleeps until the oldest
/// waiting picture is due (`Pacer::next_due`), takes the due pictures (`Pacer::take_due`), and holds
/// the one it keeps until the refresh that opens its turn (`HeldAnchor::hold`). One picture a turn,
/// a whole hold apart; the pictures that come during a hold wait for the next turn, where the take
/// drops the ones it replaces. Taking all four at once would show the newest of them; the loop
/// cannot, the hold blocks it, so a turn shows the picture that was newest when the last present
/// went out, up to a hold after it was.
#[test]
fn a_stream_faster_than_the_hold_shows_one_picture_a_turn_in_the_renderers_order() {
    use crate::stream::phase;
    const REFRESH_US: u64 = 8_333;
    const HELD: u32 = 2;
    const MARGIN_US: u64 = 5_000;
    const GUARD_US: u64 = phase::HELD_LATCH_GUARD_US;
    // 240 fps: a picture every 4 167 µs, four to a turn of 16 666.
    const PICTURE_US: u64 = 4_167;
    let lead = phase::held_present_lead_us(REFRESH_US, GUARD_US);
    let mut pacer = Pacer::new(PacingMode::Vsync, i64::from(HELD) * REFRESH_US as i64).with_frame_pacing(FramePacing::Latency, 60);
    let mut anchor = phase::HeldAnchor::default();
    let (mut next_ready, mut now) = (100_000u64, 100_000u64);
    let mut last: Option<u32> = None;
    let (mut accepted, mut count, mut shown, mut dropped, mut presents) = (0u32, 0u32, Vec::new(), 0u32, Vec::new());
    for _ in 0..12 {
        // The receive side delivers the pictures whose time has come; the loop then waits for the
        // oldest one it has, or for the next picture when nothing waits.
        while next_ready <= now {
            pacer.accept(decoded(next_ready));
            accepted += 1;
            next_ready += PICTURE_US;
        }
        let Some(due) = pacer.next_due() else { now = next_ready; continue };
        if due > now { now = due; continue; }
        let (picture, replaced) = pacer.take_due(now);
        dropped += replaced;
        let Some(picture) = picture else { continue };
        let latest = (now / REFRESH_US) as u32;
        let wait = anchor.hold(now, u64::from(latest) * REFRESH_US, latest, last, HELD, REFRESH_US, MARGIN_US, GUARD_US,
            phase::EARLY_SPARE_US, phase::EARLY_FOR_US);
        now += wait;
        count += 1;
        anchor.presented(count);
        shown.push(picture.decoded.ready_us);
        presents.push(now);
        // The swap chain shows it at the next refresh, where the next turn counts from.
        last = Some(now.div_ceil(REFRESH_US) as u32);
        // The draw and the present call take a moment; the next picture is due after this one.
        now += 1;
    }
    assert!(shown.len() >= 6, "a few turns: {shown:?}");
    assert_eq!(accepted as usize, shown.len() + dropped as usize, "every picture shown or dropped: {shown:?} dropped {dropped}");
    assert!(dropped > 0, "the pictures a hold replaces are dropped");
    // Every present after the first startup one goes out just after the refresh that opens a turn,
    // a whole hold after the one before.
    for pair in presents[1..].windows(2) {
        assert_eq!(pair[1] - pair[0], u64::from(HELD) * REFRESH_US, "one turn apart: {presents:?}");
    }
    assert!(presents[1..].iter().all(|present| (present + lead).is_multiple_of(REFRESH_US)), "at the present lead: {presents:?}");
    // A picture shown at a turn was waiting before the previous turn's present went out: the take
    // after that present keeps the newest picture that came during the hold and drops the older ones,
    // so a turn shows a picture up to a hold after it was the newest.
    for (at, ready) in shown.iter().enumerate().skip(2) {
        assert!(*ready < presents[at - 1], "a picture waiting from before the previous present took the next turn: {shown:?} at {presents:?}");
    }
}

/// Nightjar #175: Steam Deck OLED runs at 89.89 Hz (11_125 us refresh) while a 90 fps stream has
/// `frame_us(90) == 11_111 us`. In Smoothness mode, a frame waiting one normal V-Sync refresh plus
/// minor scheduling jitter must not be falsely dropped as stale; a genuinely stalled frame still is.
#[test]
fn smoothness_on_fractional_panel_does_not_falsely_drop_one_refresh_old_frames() {
    // 89.89 Hz panel -> min_interval_us = 11_125 us, 90 fps stream -> frame_us = 11_111 us.
    let mut pacer = Pacer::new(PacingMode::Vsync, 11_125).with_frame_pacing(FramePacing::Smoothness, 90);
    pacer.accept(decoded(10_000));
    pacer.accept(decoded(21_111));
    // 11_500 us old (> 11_111 us frame_us, <= 11_125 + 2_000 us stale_frame_us): kept in order!
    let (picture, dropped) = pacer.take_due(21_500);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (10_000, 0));
    // Genuinely stalled frame (> 13_125 us old) with a newer frame due is still dropped.
    pacer.accept(decoded(32_222));
    let (picture, dropped) = pacer.take_due(35_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (32_222, 1));
    // 59.94 Hz panel (16_683 us) @ 60 fps (16_666 us): same protection.
    let mut ntsc = Pacer::new(PacingMode::Vsync, 16_683).with_frame_pacing(FramePacing::Smoothness, 60);
    ntsc.accept(decoded(1_000));
    ntsc.accept(decoded(17_666));
    let (picture, dropped) = ntsc.take_due(18_200);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0));
}

#[test]
fn in_order_pacing_is_the_pacers_keep_every_picture_at_v_sync() {
    let pacings = [FramePacing::Latency, FramePacing::Balanced, FramePacing::CapFps, FramePacing::Smoothness];
    for pacing in pacings {
        for drop_late in [false, true] {
            for host_times in [false, true] {
                let in_order = in_order_pacing(pacing, drop_late, host_times);
                for mode in [PacingMode::Vsync, PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace] {
                    let pacer = Pacer::new(mode, 8_333).with_frame_pacing(pacing, 60).with_drop_late_frames(drop_late).with_host_frame_times(host_times);
                    assert_eq!(pacer.in_order(), in_order, "{pacing:?} drop {drop_late} host {host_times}");
                    assert_eq!(pacer.keeps_every_picture(), mode == PacingMode::Vsync && in_order, "{mode:?} {pacing:?} drop {drop_late} host {host_times}");
                }
            }
        }
    }
    assert!(!in_order_pacing(FramePacing::Smoothness, true, true), "the PC's frame times take the newest picture");
}
