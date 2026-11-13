use super::*;

/// One scan of a 240, 144, 120 and 100 Hz panel.
const HZ240: u64 = 4_167;
const HZ144: u64 = 6_944;
const HZ120: u64 = 8_333;
const HZ100: u64 = 10_000;

fn rtp(us: u64) -> u32 {
    (us * 9 / 100) as u32
}

/// A keeper that has shown pictures `interval_us` apart, the last at the returned time.
fn shown(scan_us: u64, interval_us: u64, pictures: u64) -> (Keeper, u64) {
    let mut keeper = Keeper::new(scan_us);
    let mut at = 1_000_000;
    for i in 0..pictures {
        at = 1_000_000 + i * interval_us;
        keeper.picture(at, rtp(at));
    }
    (keeper, at)
}

/// Shows the next picture of a game `interval_us` apart, `late_us` after its time. Returns when.
fn next(keeper: &mut Keeper, on_time_us: &mut u64, interval_us: u64, late_us: u64) -> u64 {
    *on_time_us += interval_us;
    keeper.picture(*on_time_us + late_us, rtp(*on_time_us));
    *on_time_us + late_us
}

#[test]
fn a_game_well_inside_the_range_is_only_repeated_when_a_picture_is_overdue() {
    // 100 fps on 48-240 Hz: a frame 6 ms late still comes before the panel's own refresh.
    let (keeper, last) = shown(HZ240, 10_000, 6);
    assert_eq!(keeper.expected_us(), Some(10_000));
    assert_eq!(keeper.due_us(None), Some(last + ASSUMED_FLOOR_US - GUARD_US));
    // A picture that waits and is due before that needs nothing.
    assert_eq!(keeper.due_us(Some(last + 10_000)), None);
}

#[test]
fn a_game_below_the_floor_is_repeated_halfway_and_then_left_alone() {
    // 40 fps on 48-240 Hz.
    let (mut keeper, last) = shown(HZ240, 25_000, 6);
    assert_eq!(keeper.due_us(None), Some(last + 12_500));
    keeper.repeated(last + 12_500);
    assert_eq!(keeper.repeats, 1);
    // The next repeat would still be scanned when the picture is expected: it waits for the
    // floor instead, so a picture up to 7 ms late finds the panel idle.
    assert_eq!(keeper.due_us(None), Some(last + 12_500 + ASSUMED_FLOOR_US - GUARD_US));
}

#[test]
fn a_game_far_below_the_floor_is_repeated_evenly() {
    // 20 fps on 48-240 Hz: three parts of 16.7 ms.
    let (mut keeper, last) = shown(HZ240, 50_000, 6);
    assert_eq!(keeper.due_us(None), Some(last + 16_666));
    keeper.repeated(last + 16_666);
    assert_eq!(keeper.due_us(None), Some(last + 33_332));
    keeper.repeated(last + 33_332);
    assert_eq!(keeper.due_us(None), Some(last + 33_332 + ASSUMED_FLOOR_US - GUARD_US));
}

#[test]
fn a_late_picture_brings_its_repeat_closer_or_leaves_it_out() {
    // 40 fps on 48-240 Hz. A picture 9 ms late: the next is expected 16 ms after it.
    let (mut keeper, mut on_time) = shown(HZ240, 25_000, 6);
    let at = next(&mut keeper, &mut on_time, 25_000, 9_000);
    assert_eq!(keeper.due_us(None), Some(at + 8_000));
    // The next one is on time again, 16 ms on: its own repeat is back halfway.
    let at = next(&mut keeper, &mut on_time, 25_000, 0);
    assert_eq!(keeper.due_us(None), Some(at + 12_500));
    // 14 ms late leaves 11 ms, which two scans and their room do not fit into.
    let at = next(&mut keeper, &mut on_time, 25_000, 14_000);
    assert_eq!(keeper.due_us(None), Some(at + ASSUMED_FLOOR_US - GUARD_US));
}

#[test]
fn a_game_just_above_the_floor_is_doubled_once_its_late_frames_cross_it() {
    // 60 fps on 48-240 Hz, on time: nothing to repeat.
    let (mut keeper, mut on_time) = shown(HZ240, 16_667, 8);
    assert_eq!(keeper.due_us(None), Some(on_time + ASSUMED_FLOOR_US - GUARD_US));
    // A frame 4.5 ms late is shown 21.2 ms after the one before, into the panel's own refresh.
    // One of them changes nothing.
    next(&mut keeper, &mut on_time, 16_667, 4_500);
    for _ in 0..20 {
        next(&mut keeper, &mut on_time, 16_667, 0);
    }
    assert_eq!(keeper.due_us(None), Some(on_time + ASSUMED_FLOOR_US - GUARD_US));
    // The second within two seconds does: every picture goes out twice, the panel at 120 Hz.
    next(&mut keeper, &mut on_time, 16_667, 4_500);
    let at = next(&mut keeper, &mut on_time, 16_667, 0);
    // (Within what the 90 kHz stamps round the frame times by.)
    let near = |due: Option<u64>, us: u64| due.is_some_and(|due| due.abs_diff(us) <= 5);
    assert!(near(keeper.due_us(None), at + 8_333), "{:?} after {at}", keeper.due_us(None));
    // A late frame then has its repeat sooner, and the one after it is met by neither.
    let at = next(&mut keeper, &mut on_time, 16_667, 4_000);
    assert!(near(keeper.due_us(None), at + (16_667 - 4_000) / 2), "{:?} after {at}", keeper.due_us(None));
    // Ten seconds without a frame that needed it, and the picture goes out once again.
    for _ in 0..CALM_PICTURES + 1 {
        next(&mut keeper, &mut on_time, 16_667, 0);
    }
    assert_eq!(keeper.due_us(None), Some(on_time + ASSUMED_FLOOR_US - GUARD_US));
}

#[test]
fn the_scan_more_is_given_up_when_pictures_meet_it() {
    let (mut keeper, _) = shown(HZ240, 16_667, 8);
    // Pictures that came 10.5 ms after the one before, while the halfway repeat was scanned.
    keeper.parts = 2;
    for shown in &mut keeper.shown[..6] {
        *shown = Shown { span_us: 10_500, late_before_us: 0, offset_us: 0 };
    }
    keeper.shown_len = 6;
    assert_eq!((keeper.would_meet(16_667, 1), keeper.would_meet(16_667, 2)), (0, 6));
    keeper.choose();
    assert_eq!(keeper.parts, 1);
    // One or two of them are not worth a change of the panel's rate.
    keeper.parts = 2;
    keeper.shown_len = 1;
    keeper.choose();
    assert_eq!(keeper.parts, 2);
}

#[test]
fn a_repeat_needs_room_for_its_scan() {
    // 60 fps on 48-144 Hz: half the interval is not a scan and its room, so only when overdue.
    let (keeper, last) = shown(HZ144, 16_667, 6);
    assert_eq!(keeper.due_us(None), Some(last + ASSUMED_FLOOR_US - GUARD_US));
    // 40 fps on 48-120 Hz fits one.
    let (keeper, last) = shown(HZ120, 25_000, 6);
    assert_eq!(keeper.due_us(None), Some(last + 12_500));
    // 48-100 Hz has no room between two scans at all.
    assert!(!Panel::new(HZ100).has_room());
    let (keeper, _) = shown(HZ100, 25_000, 6);
    assert_eq!(keeper.due_us(None), None);
    assert_eq!(keeper.due_us(Some(1_500_000)), None);
}

#[test]
fn a_repeat_never_starts_while_the_last_scan_runs() {
    let (mut keeper, last) = shown(HZ240, 25_000, 6);
    // A redraw (the overlay) started a scan just before the planned repeat.
    keeper.redrawn(last + 12_000);
    let due = keeper.due_us(None).unwrap();
    assert!(due >= last + 12_000 + keeper.panel.clear_us(), "{due}");
}

#[test]
fn a_waiting_picture_moves_or_cancels_the_repeat() {
    let (keeper, last) = shown(HZ240, 25_000, 6);
    let clear = keeper.panel.clear_us();
    // Due while the planned repeat (at 12.5 ms) would be scanned, before the panel's own refresh.
    assert_eq!(keeper.due_us(Some(last + 14_000)), None);
    // Due well after it: the plan stands.
    assert_eq!(keeper.due_us(Some(last + 24_000)), Some(last + 12_500));
    // Overdue, the repeat waits for the floor (19.8 ms); a picture due just after the floor
    // would meet the panel's refresh, so the repeat goes early enough to be over by then.
    let (mut keeper, last) = shown(HZ240, 10_000, 6);
    assert_eq!(keeper.due_us(Some(last + 22_000)), Some(last + 22_000 - clear));
    // One due before the floor needs nothing.
    assert_eq!(keeper.due_us(Some(last + 19_000)), None);
    // The same counted from a scan the stream started later.
    keeper.redrawn(last + 17_000);
    assert_eq!(keeper.due_us(Some(last + 17_000 + ASSUMED_FLOOR_US + 500)), Some(last + 17_000 + ASSUMED_FLOOR_US + 500 - clear));
}

#[test]
fn a_game_that_stopped_is_left_to_the_panel() {
    let (mut keeper, last) = shown(HZ240, 16_667, 6);
    let mut at = last;
    let mut repeats = 0;
    while let Some(due) = keeper.due_us(None) {
        assert!(due > at, "{due} after {at}");
        at = due;
        keeper.repeated(at);
        repeats += 1;
        assert!(repeats < 10, "the repeats end");
    }
    // Those before the floor while the picture is a few frames overdue.
    assert_eq!(repeats, 2);
    assert!(at <= last + 16_667 + 2 * ASSUMED_FLOOR_US);
}

#[test]
fn the_expected_interval_is_the_median_and_a_pause_starts_it_over() {
    let mut keeper = Keeper::new(HZ240);
    let mut at = 0u64;
    for interval in [25_000u64, 25_000, 60_000, 25_000] {
        at += interval;
        keeper.picture(at, rtp(at));
    }
    assert_eq!(keeper.expected_us(), Some(25_000), "one long frame does not move it");
    at += 2_000_000;
    keeper.picture(at, rtp(at));
    assert_eq!(keeper.expected_us(), None, "frame times from before a pause say nothing");
    // Without an expectation only the floor counts.
    assert_eq!(keeper.due_us(None), Some(at + ASSUMED_FLOOR_US - GUARD_US));
}

#[test]
fn rtp_timestamps_wrap() {
    let mut keeper = Keeper::new(HZ240);
    let mut rtp = u32::MAX - 3_000;
    for i in 0..5u64 {
        keeper.picture(1_000_000 + i * 25_000, rtp);
        rtp = rtp.wrapping_add(2_250);
    }
    assert_eq!(keeper.expected_us(), Some(25_000));
    assert_eq!(keeper.late_us, 0);
    assert_eq!(keeper.due_us(None), Some(1_100_000 + 12_500));
}

#[test]
fn the_floor_is_learned_from_scans_the_stream_did_not_start() {
    // A panel that refreshes by itself after 16.7 ms (60-240 Hz), the stream assuming 20.8.
    let (mut keeper, mut at) = shown(HZ240, 10_000, 6);
    for seen in 0..3 {
        // The next picture comes 19 ms on; the panel began a scan 16.7 ms after the last present.
        let now = at + 19_000;
        let observed = keeper.observe(now, 19_000 - 16_700, true, true);
        assert!(observed.foreign);
        assert_eq!(observed.floor_us.is_some(), seen == 2, "three sightings that agree");
        at = now;
        keeper.picture(at, rtp(at));
    }
    assert_eq!(keeper.panel.floor_us, 16_700);
    assert_eq!(keeper.met_scan, 3);
    // From then on the repeat comes before that.
    for _ in 0..5 {
        at += 10_000;
        keeper.picture(at, rtp(at));
    }
    assert_eq!(keeper.due_us(None), Some(at + 15_700));
}

#[test]
fn the_floor_rises_for_a_panel_that_waits_longer() {
    // A panel that refreshes by itself after 33.3 ms, seen when the stream left it alone.
    let (mut keeper, mut at) = shown(HZ240, 10_000, 6);
    for _ in 0..3 {
        let now = at + 38_000;
        assert!(keeper.observe(now, 38_000 - 33_300, false, true).foreign);
        at = now;
        keeper.picture(at, rtp(at));
    }
    assert_eq!(keeper.panel.floor_us, 33_300);
}

#[test]
fn readings_that_say_nothing_about_the_floor_leave_it() {
    let (mut keeper, at) = shown(HZ240, 10_000, 6);
    // The stream's own scan, begun 2 ms after the present call: the wait counts from there.
    let observed = keeper.observe(at + 9_000, 7_000, false, true);
    assert_eq!(observed, Observed::default());
    assert_eq!(keeper.due_us(None), Some(at + 2_000 + ASSUMED_FLOOR_US - GUARD_US));
    // A flip the GPU got to 8 ms late is no refresh of the panel's own.
    let (mut keeper, at) = shown(HZ240, 10_000, 6);
    assert_eq!(keeper.observe(at + 9_000, 1_000, true, false), Observed { foreign: true, floor_us: None });
    // Two of the panel's refreshes went by since that scan (at 8 ms): the wait is not known.
    assert_eq!(keeper.observe(at + 52_000, 3_300, false, true).floor_us, None);
    // The refresh the stream expects, at the floor in use after the scan at 48.7 ms.
    assert_eq!(keeper.observe(at + 69_700, 200, true, true).floor_us, None);
    assert_eq!(keeper.sighted, 0);
    assert_eq!(keeper.panel.floor_us, ASSUMED_FLOOR_US);
    // Sightings that do not agree move nothing.
    let (mut keeper, mut at) = shown(HZ240, 10_000, 6);
    for waited in [12_000u64, 16_000, 14_000, 17_500, 13_000] {
        let now = at + 19_500;
        assert_eq!(keeper.observe(now, 19_500 - waited, false, true).floor_us, None);
        at = now;
        keeper.picture(at, rtp(at));
    }
    assert_eq!(keeper.sighted, 5);
    assert_eq!(keeper.panel.floor_us, ASSUMED_FLOOR_US);
}

#[test]
fn a_foreign_scan_starts_the_panels_wait_over() {
    let (mut keeper, at) = shown(HZ240, 10_000, 6);
    // Overdue at 19.8 ms; the reading before the repeat finds the panel already scanning by
    // itself (begun 18.8 ms after the present): no repeat now, the next counts from that scan.
    let now = at + ASSUMED_FLOOR_US - GUARD_US;
    let observed = keeper.observe(now, 1_033, true, false);
    assert!(observed.foreign);
    assert_eq!(keeper.met_scan, 0, "a repeat is no picture");
    assert_eq!(keeper.due_us(None), Some(now - 1_033 + ASSUMED_FLOOR_US - GUARD_US));
}

#[test]
fn the_line_names_what_was_done() {
    let (mut keeper, at) = shown(HZ240, 25_000, 4);
    keeper.repeated(at + 12_500);
    keeper.observe(at + 25_000, 1_000, true, true);
    assert_eq!(keeper.line(), "1 repeats, 25.0% of pictures met a scan, panel waits 20.8 ms");
}

/// A panel that scans a picture when it is presented to, and again by itself `floor` after a
/// scan began. A picture presented during a scan waits for its end.
struct Screen {
    scan: u64,
    floor: u64,
    began: u64,
}

impl Screen {
    /// How long ago the scan on the display began, the panel having refreshed by itself since.
    fn since_scan(&mut self, now: u64) -> u64 {
        while self.began + self.floor <= now {
            self.began += self.floor;
        }
        now - self.began
    }
}

/// Shows pictures (when each is due, and its time on the PC) on a 240 Hz panel with that floor,
/// the way the presenter drives the keeper. Returns how many met a scan, and the keeper.
fn play(pictures: &[(u64, u64)], floor: u64, repeats: bool) -> (u64, Keeper) {
    let mut screen = Screen { scan: HZ240, floor, began: 0 };
    let mut keeper = Keeper::new(HZ240);
    let mut met = 0;
    for &(due, stamp) in pictures {
        while let Some(at) = keeper.due_us(None).filter(|at| repeats && *at < due) {
            let since = screen.since_scan(at);
            let scanning = since < screen.scan;
            let seen = keeper.observe(at, since, scanning, false);
            if scanning {
                keeper.scan_under_way(at, since);
            } else if !seen.foreign {
                screen.began = at;
                keeper.repeated(at);
            }
        }
        let since = screen.since_scan(due);
        let scanning = since < screen.scan;
        keeper.observe(due, since, scanning, true);
        let at = if scanning { screen.began + screen.scan } else { due };
        met += u64::from(scanning);
        screen.began = at;
        keeper.picture(at, rtp(stamp));
    }
    (met, keeper)
}

fn noise(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

/// `count` pictures of a game `interval` apart, each shown `late(i)` after its time.
fn stream(count: u64, interval: u64, mut late: impl FnMut(u64) -> u64) -> Vec<(u64, u64)> {
    (1..=count).map(|i| (1_000_000 + i * interval + late(i), 1_000_000 + i * interval)).collect()
}

#[test]
fn late_frames_of_a_60_fps_game_stop_meeting_the_panels_refresh() {
    // Real VRR: a frame 4.5 to 8 ms late every 25th, the delay it raised falling over the next.
    let mut seed = 7;
    let mut held = 0;
    let pictures = stream(3_000, 16_667, |i| {
        held = if i % 25 == 0 { 4_500 + noise(&mut seed) % 3_500 } else { held * 5 / 6 };
        held
    });
    let (alone, _) = play(&pictures, ASSUMED_FLOOR_US, false);
    let (kept, keeper) = play(&pictures, ASSUMED_FLOOR_US, true);
    assert!(alone >= 100, "{alone}");
    assert!(kept <= 4, "{kept} of {alone}");
    assert_eq!(keeper.met_scan, kept);
}

#[test]
fn a_40_fps_game_with_jitter_meets_far_fewer_scans() {
    // Shown as they come, each up to 6 ms late.
    let mut seed = 11;
    let pictures = stream(2_000, 25_000, |_| noise(&mut seed) % 6_000);
    let (alone, _) = play(&pictures, ASSUMED_FLOOR_US, false);
    let (kept, keeper) = play(&pictures, ASSUMED_FLOOR_US, true);
    assert!(alone >= 500, "{alone}");
    assert!(kept * 10 <= alone, "{kept} of {alone}");
    assert!(keeper.repeats >= 1_900, "{}", keeper.repeats);
}

#[test]
fn a_game_inside_the_range_is_left_alone() {
    // 100 fps, and 72 fps with frames up to 5 ms late: neither reaches the floor.
    let mut seed = 13;
    for pictures in [stream(2_000, 10_000, |_| 0), stream(2_000, 13_889, |_| noise(&mut seed) % 5_000)] {
        let (alone, _) = play(&pictures, ASSUMED_FLOOR_US, false);
        let (kept, keeper) = play(&pictures, ASSUMED_FLOOR_US, true);
        assert_eq!((alone, kept, keeper.repeats), (0, 0, 0));
    }
}

#[test]
fn a_shorter_floor_is_learned_and_kept_clear_of() {
    // 55 fps on a 60-240 Hz panel: its refresh after 16.7 ms is under way when a picture comes
    // 18.2 ms after the one before, which is every other one (the picture that waited for the
    // scan's end is followed sooner).
    let pictures = stream(1_000, 18_182, |_| 0);
    let (alone, _) = play(&pictures, 16_667, false);
    let (kept, keeper) = play(&pictures, 16_667, true);
    assert!(keeper.panel.floor_us.abs_diff(16_667) <= 100, "{}", keeper.panel.floor_us);
    assert!(alone >= 490 && kept <= 8, "{kept} of {alone}");
}
