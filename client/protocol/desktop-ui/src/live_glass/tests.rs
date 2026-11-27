use super::*;
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{pos2, Color32};

fn mesh(rects: &[(Rect, Color32)]) -> ClippedPrimitive {
    let mut m = Mesh::default();
    for (rect, color) in rects {
        let base = m.vertices.len() as u32;
        for pos in [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ] {
            m.vertices.push(Vertex {
                pos,
                uv: WHITE_UV,
                color: *color,
            });
        }
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    ClippedPrimitive {
        clip_rect: Rect::EVERYTHING,
        primitive: Primitive::Mesh(m),
    }
}

#[test]
fn only_what_lies_under_the_chrome_changes_the_print() {
    let chrome = [Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))];
    let under = Rect::from_min_max(pos2(50.0, 40.0), pos2(90.0, 90.0));
    let below = Rect::from_min_max(pos2(50.0, 300.0), pos2(90.0, 400.0));
    let print = |under_color, below_color| {
        fingerprint(
            &[mesh(&[(under, under_color), (below, below_color)])],
            &chrome,
        )
    };
    let base = print(Color32::RED, Color32::RED);
    assert_eq!(
        base,
        print(Color32::RED, Color32::BLUE),
        "a hover far below runs no chain"
    );
    assert_ne!(
        base,
        print(Color32::BLUE, Color32::RED),
        "a change under the chrome does"
    );
}

#[test]
fn scrolling_content_under_the_chrome_changes_the_print() {
    let chrome = [Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))];
    let at = |y: f32| {
        fingerprint(
            &[mesh(&[(
                Rect::from_min_max(pos2(50.0, y), pos2(90.0, y + 100.0)),
                Color32::RED,
            )])],
            &chrome,
        )
    };
    assert_ne!(at(10.0), at(11.0));
    assert_eq!(at(200.0), at(300.0), "nothing under the chrome both times");
}

#[test]
fn a_change_just_outside_the_chrome_reaches_it_through_the_blur() {
    let chrome = [Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))];
    let beside = Rect::from_min_max(pos2(50.0, 80.0), pos2(90.0, 90.0));
    let print = |color| fingerprint(&[mesh(&[(beside, color)])], &chrome);
    assert_ne!(print(Color32::RED), print(Color32::BLUE));
}

#[test]
fn a_clip_change_on_content_under_the_chrome_changes_the_print() {
    let chrome = [bar()];
    let print = |bottom: f32| {
        let mut content = mesh(&[(bar(), Color32::RED)]);
        content.clip_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, bottom));
        fingerprint(&[content], &chrome)
    };
    assert_ne!(print(60.0), print(30.0), "the same triangles, less of them showing");
}

/// What reached the stage, in order.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Call {
    Mesh,
    Begin,
    End { timed: bool },
    Release,
    Try,
}

#[derive(Default)]
struct Recorder {
    calls: Vec<Call>,
    size: (i32, i32),
    targets_fail: bool,
    begin_fails: bool,
    end_fails: bool,
    ms: f32,
}

impl Stage for Recorder {
    fn size(&self) -> (i32, i32) {
        self.size
    }
    fn mesh(&mut self, _: Rect, _: &Mesh) {
        self.calls.push(Call::Mesh);
    }
    fn targets_work(&mut self) -> Result<(), String> {
        self.calls.push(Call::Try);
        if self.targets_fail {
            Err("no targets".into())
        } else {
            Ok(())
        }
    }
    fn begin_scene(&mut self) -> Result<(), String> {
        self.calls.push(Call::Begin);
        if self.begin_fails {
            Err("no scene".into())
        } else {
            Ok(())
        }
    }
    fn end_scene(&mut self, timed: bool) -> Result<Option<f32>, String> {
        self.calls.push(Call::End { timed });
        if self.end_fails {
            Err("no pass".into())
        } else {
            Ok(timed.then_some(self.ms))
        }
    }
    fn release(&mut self) {
        self.calls.push(Call::Release);
    }
}

fn boundary_at(regions: Vec<Rect>) -> ClippedPrimitive {
    ClippedPrimitive {
        clip_rect: Rect::EVERYTHING,
        primitive: Primitive::Callback(egui::epaint::PaintCallback {
            rect: Rect::EVERYTHING,
            callback: std::sync::Arc::new(Boundary { regions }),
        }),
    }
}

fn bar() -> Rect {
    Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))
}

/// Content under the chrome in `color`, the boundary, one chrome mesh.
fn chrome_frame(color: Color32) -> Vec<ClippedPrimitive> {
    vec![
        mesh(&[(bar(), color)]),
        boundary_at(vec![bar()]),
        mesh(&[(bar(), Color32::WHITE)]),
    ]
}

fn stage() -> Recorder {
    Recorder {
        size: (1000, 800),
        ms: 1.0,
        ..Default::default()
    }
}

#[test]
fn a_stream_frame_reaches_the_stage_as_its_meshes_alone() {
    let frame = [
        mesh(&[(bar(), Color32::RED)]),
        mesh(&[(bar(), Color32::BLUE)]),
    ];
    // On the machine's every landing: nothing but the two draws today's path makes.
    for mut glass in [
        Glass::new(None),
        Glass::new(Some(Tier::Baked)),
        Glass::live(),
    ] {
        let mut stage = stage();
        glass.paint(&mut stage, &frame, true);
        assert_eq!(stage.calls, [Call::Mesh, Call::Mesh]);
    }
}

#[test]
fn over_a_streams_video_there_is_no_chain_even_with_a_boundary() {
    let mut glass = Glass::live();
    assert!(!glass.set_over_video(true), "nothing held, nothing to free");
    assert_eq!(glass.pass_tier(), Tier::Baked, "so no boundary is laid out");
    let mut stage = stage();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
    glass.settle(&mut stage);
    assert_eq!(
        stage.calls,
        [Call::Mesh, Call::Mesh],
        "the video stays on the back buffer"
    );
}

#[test]
fn a_stream_starting_frees_the_targets_even_if_it_never_draws_an_overlay() {
    let mut glass = Glass::live();
    let mut stage = stage();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
    assert!(
        glass.set_over_video(true),
        "the launcher's targets go at the stream's first frame"
    );
    assert!(!glass.set_over_video(true), "and only once");
    // Back on the launcher the chain is built anew.
    assert!(!glass.set_over_video(false));
    assert_eq!(glass.pass_tier(), Tier::Live);
    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert_eq!(stage.calls[0], Call::Begin);
}

#[test]
fn a_page_without_chrome_frees_the_targets_once() {
    let mut glass = Glass::live();
    let mut stage = stage();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
    stage.calls.clear();
    let plain = [mesh(&[(bar(), Color32::RED)])];
    glass.paint(&mut stage, &plain, false);
    glass.paint(&mut stage, &plain, false);
    assert_eq!(stage.calls, [Call::Release, Call::Mesh, Call::Mesh]);
}

#[test]
fn content_goes_through_the_scene_and_chrome_on_top_of_it() {
    let mut glass = Glass::live();
    let mut stage = stage();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert_eq!(
        stage.calls,
        [
            Call::Begin,
            Call::Mesh,
            Call::End { timed: true },
            Call::Mesh
        ]
    );
}

#[test]
fn nothing_changed_under_the_chrome_runs_no_chain() {
    let mut glass = Glass::live();
    let mut stage = stage();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert_eq!(stage.calls, [Call::Mesh, Call::Mesh], "the same frame again");

    stage.calls.clear();
    let mut hover_far_below = chrome_frame(Color32::RED);
    hover_far_below.insert(
        0,
        mesh(&[(
            Rect::from_min_max(pos2(0.0, 500.0), pos2(90.0, 600.0)),
            Color32::BLUE,
        )]),
    );
    glass.paint(&mut stage, &hover_far_below, false);
    assert!(!stage.calls.contains(&Call::Begin), "{:?}", stage.calls);

    for (change, frame_color, size) in [
        ("content under it", Color32::BLUE, (1000, 800)),
        ("the window's size", Color32::BLUE, (900, 800)),
    ] {
        stage.calls.clear();
        stage.size = size;
        glass.paint(&mut stage, &chrome_frame(frame_color), false);
        assert_eq!(stage.calls[0], Call::Begin, "{change}");
    }

    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::BLUE), true);
    assert_eq!(stage.calls[0], Call::Begin, "an uploaded texture");

    glass.invalidate();
    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::BLUE), false);
    assert_eq!(stage.calls[0], Call::Begin, "targets reset");
}

#[test]
fn a_chain_over_budget_steps_down_once_keeping_the_frame_it_decided_on() {
    let mut glass = Glass::live();
    let mut stage = stage();
    stage.ms = 30.0;
    // Seven slow runs of sixty decide the p90 at once (`glass_probe::slow_frames_to_fail`).
    for n in 0..probe::slow_frames_to_fail(probe::PROBE_FRAMES) {
        assert_eq!(glass.pass_tier(), Tier::Live, "run {n}");
        let color = Color32::from_gray(n as u8);
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(color), false);
        assert_eq!(stage.calls[2], Call::End { timed: true });
    }
    assert_eq!(glass.pass_tier(), Tier::Baked);
    assert!(glass.take_changed(), "the next pass follows at once");
    assert!(!glass.take_changed(), "once");
    assert!(
        !stage.calls.contains(&Call::Release),
        "chrome of the last frame sampled the output"
    );
    stage.calls.clear();
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Release]);
    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::GREEN), true);
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Mesh, Call::Mesh], "and never comes back");
}

#[test]
fn a_chain_within_budget_stays_live_and_stops_timing() {
    let mut glass = Glass::live();
    let mut stage = stage();
    for n in 0..probe::PROBE_FRAMES {
        // Asked before the frame draws, so the GPU renderer keeps it readable.
        assert!(glass.timing(), "run {n}");
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
        assert_eq!(stage.calls[2], Call::End { timed: true });
    }
    assert_eq!(glass.pass_tier(), Tier::Live);
    assert!(!glass.timing(), "the frames go straight into the swapchain again");
    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::GREEN), false);
    assert_eq!(stage.calls[2], Call::End { timed: false }, "no more waits");
}

#[test]
fn only_a_live_chain_still_measured_times_its_frames() {
    assert!(!Glass::new(None).timing(), "untried: baked until the targets are tried");
    assert!(!Glass::new(Some(Tier::Baked)).timing());
    let mut glass = Glass::live();
    assert!(glass.timing());
    assert!(!glass.set_over_video(true));
    assert!(!glass.timing(), "no chain over a stream's video");
}

#[test]
fn a_chain_that_cannot_be_timed_steps_down_saying_why() {
    let mut glass = Glass::live();
    let mut stage = Recorder { end_fails: true, ..stage() };
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert_eq!(glass.pass_tier(), Tier::Baked, "a time that waited for nothing is no verdict");
    assert!(!glass.timing());
}

#[test]
fn another_refresh_rate_judges_the_chain_against_its_own_budget() {
    let mut glass = Glass::live();
    let mut stage = stage();
    stage.ms = 10.0;
    glass.set_refresh(60.0);
    for n in 0..probe::PROBE_FRAMES {
        glass.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
    }
    assert_eq!(glass.pass_tier(), Tier::Live, "10 ms holds at 60 Hz");
    // The same rate changes nothing: no new timing.
    glass.set_refresh(60.0);
    stage.calls.clear();
    glass.paint(&mut stage, &chrome_frame(Color32::GREEN), false);
    assert!(stage.calls.contains(&Call::End { timed: false }), "{:?}", stage.calls);
    glass.set_refresh(120.0);
    for n in 0..probe::PROBE_FRAMES {
        glass.paint(&mut stage, &chrome_frame(Color32::from_rgb(n as u8, 1, 2)), false);
    }
    assert_eq!(glass.pass_tier(), Tier::Baked, "10 ms is over budget at 120 Hz");
}

#[test]
fn a_window_with_no_pixels_skips_the_frame_and_stays_live() {
    let mut glass = Glass::live();
    let mut stage = stage();
    let size = stage.size;
    for empty in [(0, 0), (0, 800), (1000, 0)] {
        stage.size = empty;
        stage.begin_fails = true;
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        assert!(stage.calls.is_empty(), "{empty:?}: {:?}", stage.calls);
        assert_eq!(glass.pass_tier(), Tier::Live, "{empty:?}");
        assert!(!glass.take_changed());
    }
    stage.size = size;
    stage.begin_fails = false;
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert!(stage.calls.contains(&Call::Begin), "back with its pixels: {:?}", stage.calls);
}

#[test]
fn a_failing_chain_lands_on_baked_with_the_content_still_drawn() {
    for fails in ["begin", "end"] {
        let mut glass = Glass::live();
        let mut stage = stage();
        stage.begin_fails = fails == "begin";
        stage.end_fails = fails == "end";
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        assert_eq!(
            stage.calls.iter().filter(|c| **c == Call::Mesh).count(),
            2,
            "{fails}: content and chrome"
        );
        assert_eq!(glass.pass_tier(), Tier::Baked, "{fails}");
        assert!(glass.take_changed(), "{fails}");
        glass.settle(&mut stage);
        assert_eq!(stage.calls.last(), Some(&Call::Release), "{fails}");
    }
}

#[test]
fn the_targets_are_tried_once_on_the_first_launcher_frame() {
    let mut glass = Glass::new(None);
    assert_eq!(glass.pass_tier(), Tier::Baked, "baked until they worked");
    let mut stage = stage();
    assert!(!glass.set_over_video(true));
    glass.settle(&mut stage);
    assert!(stage.calls.is_empty(), "a stream never tries them");
    assert!(!glass.set_over_video(false));
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Try]);
    assert_eq!(glass.pass_tier(), Tier::Lens, "the best tier: the live chain with its band");
    assert!(glass.take_changed(), "the next pass lays out lens");
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Try], "once");
    assert!(!glass.take_changed(), "and idle stays idle");

    let mut glass = Glass::new(None);
    let mut stage = self::stage();
    stage.targets_fail = true;
    glass.settle(&mut stage);
    assert_eq!(glass.pass_tier(), Tier::Baked);
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Try], "a failure is not retried");
}

#[test]
fn nightjar_glass_holds_the_tier_for_an_a_b_run() {
    assert_eq!(forced_by(None), None);
    assert_eq!(forced_by(Some("nonsense")), None);
    let mut baked = Glass::new(forced_by(Some("baked")));
    let mut stage = stage();
    baked.settle(&mut stage);
    assert!(stage.calls.is_empty(), "baked never tries the targets");
    assert_eq!(baked.pass_tier(), Tier::Baked);

    let mut live = Glass::new(forced_by(Some("live")));
    live.settle(&mut stage);
    live.head_start_ms = probe::SETTLE_MS;
    stage.ms = 30.0;
    for n in 0..probe::PROBE_FRAMES {
        live.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
    }
    assert_eq!(live.pass_tier(), Tier::Live, "over budget and held");
}

#[test]
fn a_frame_without_a_boundary_has_none_to_find() {
    // Stream frames: the overlay's meshes alone.
    assert!(find_boundary(&[mesh(&[(Rect::ZERO, Color32::RED)])]).is_none());
}

#[test]
fn the_boundary_is_found_at_its_place() {
    let boundary = ClippedPrimitive {
        clip_rect: Rect::EVERYTHING,
        primitive: Primitive::Callback(egui::epaint::PaintCallback {
            rect: Rect::EVERYTHING,
            callback: std::sync::Arc::new(Boundary {
                regions: vec![Rect::ZERO],
            }),
        }),
    };
    let frame = [
        mesh(&[(Rect::ZERO, Color32::RED)]),
        boundary,
        mesh(&[(Rect::ZERO, Color32::RED)]),
    ];
    assert_eq!(
        find_boundary(&frame).map(|(at, b)| (at, b.regions.len())),
        Some((1, 1))
    );
}

#[test]
fn lens_steps_down_to_live_then_to_baked_and_stores_each() {
    let mut glass = Glass::landed(Tier::Lens);
    let mut stage = stage();
    stage.ms = 30.0;
    let mut n = 0u8;
    let mut run = |glass: &mut Glass, stage: &mut Recorder| {
        n = n.wrapping_add(1);
        glass.paint(stage, &chrome_frame(Color32::from_gray(n)), false);
    };
    let slow = probe::slow_frames_to_fail(probe::PROBE_FRAMES);
    for _ in 0..slow {
        assert_eq!(glass.pass_tier(), Tier::Lens);
        run(&mut glass, &mut stage);
    }
    assert_eq!(glass.pass_tier(), Tier::Live, "one tier down");
    assert!(glass.take_changed(), "the next pass drops the band");
    assert_eq!(glass.memory().stored().tier, Some(Tier::Live));
    assert!(glass.timing(), "live is probed in turn");
    for _ in 0..slow {
        run(&mut glass, &mut stage);
    }
    assert_eq!(glass.pass_tier(), Tier::Baked);
    assert_eq!(glass.memory().stored().tier, Some(Tier::Baked));
    stage.calls.clear();
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Release], "baked holds no targets");
}

#[test]
fn over_video_with_the_menu_closed_nothing_is_held_and_no_pass_runs() {
    for tier in [Tier::Lens, Tier::Live] {
        let mut glass = Glass::landed(tier);
        let mut stage = stage();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
        assert!(stage.calls.contains(&Call::Begin), "{tier:?}: the launcher ran its chain");
        assert!(glass.set_over_video(true), "{tier:?}: its targets go with the stream");
        stage.calls.clear();
        // The stream's frames: its overlay's meshes, and none at all with the menu closed.
        glass.paint(&mut stage, &[], false);
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        glass.settle(&mut stage);
        assert!(!stage.calls.iter().any(|c| matches!(c, Call::Begin | Call::End { .. } | Call::Release | Call::Try)), "{tier:?}: {:?}", stage.calls);
        assert_eq!(glass.pass_tier(), Tier::Baked);
        assert!(!glass.timing());
    }
}

#[test]
fn one_unfinished_probe_gets_another_try_and_a_finished_one_forgets_it() {
    let fingerprint = fingerprint_of(&stage());
    // The last start armed the guard and never cleared it: a crash, or the app killed, or a
    // device reset, between landing and the first clean frame.
    let once = Stored { tier: None, guard: Guard::arm(&fingerprint), trips: 0 };
    let mut glass = Glass::new(None).remembering(Memory::in_memory(once));
    let mut stage = stage();
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Try], "the targets are tried again");
    assert_eq!(glass.pass_tier(), Tier::Lens);
    assert_eq!(glass.memory().stored(), &Stored { tier: None, guard: Guard::arm(&fingerprint), trips: 1 }, "armed, counting the unfinished one");
    // This try gets through its first frame: the count starts over.
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert_eq!(glass.memory().stored(), &Stored { tier: None, guard: Guard::clear(&fingerprint), trips: 0 });
}

#[test]
fn a_probe_that_never_finished_twice_in_a_row_trips_the_guard() {
    let fingerprint = fingerprint_of(&stage());
    assert!(fingerprint.starts_with(crate::VERSION));
    // The second start in a row whose probe never finished on this build and driver.
    let crashed = Stored { tier: None, guard: Guard::arm(&fingerprint), trips: 1 };
    let mut glass = Glass::new(None).remembering(Memory::in_memory(crashed));
    let mut stage = stage();
    glass.settle(&mut stage);
    assert!(stage.calls.is_empty(), "no target is made: {:?}", stage.calls);
    assert_eq!(glass.pass_tier(), Tier::Baked);
    let stored = glass.memory().stored().clone();
    assert_eq!(stored, Stored { tier: Some(Tier::Baked), guard: Guard::clear(&fingerprint), trips: 0 }, "baked stored, the guard cleared");

    // The next start finds baked stored for this build: baked, and no guard armed again.
    let mut next = Glass::new(None).remembering(Memory::in_memory(stored.clone()));
    next.settle(&mut stage);
    assert!(stage.calls.is_empty());
    assert_eq!(next.pass_tier(), Tier::Baked);
    assert!(!next.memory().stored().guard.in_flight);

    // An update of the build, the GPU or its driver gets a fresh try, the count with it.
    let other = Stored { tier: Some(Tier::Baked), guard: Guard::arm("another build or driver"), trips: 1 };
    let mut updated = Glass::new(None).remembering(Memory::in_memory(other));
    updated.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Try]);
    assert_eq!(updated.pass_tier(), Tier::Lens);
    assert_eq!(updated.memory().stored().trips, 0);
}

/// A device reset or the switch to another renderer in the first frames is no crash: the guard
/// is cleared on disk before the new renderer's landing arms it again.
#[test]
fn a_new_renderer_clears_the_guard_on_disk_before_it_is_armed_again() {
    let mut glass = Glass::new(None).remembering(Memory::in_memory(Stored::default()));
    let mut stage = stage();
    glass.settle(&mut stage);
    let fingerprint = fingerprint_of(&stage);
    assert!(glass.memory().stored().guard.tripped(&fingerprint), "armed before the first frame");
    glass.renderer_changed();
    assert!(!glass.memory().stored().guard.in_flight, "a start after this finds no unfinished probe");
    // The new renderer's first launcher frame tries its targets and arms the guard again.
    glass.settle(&mut stage);
    assert_eq!(stage.calls, [Call::Try, Call::Try]);
    assert_eq!(glass.memory().stored(), &Stored { tier: None, guard: Guard::arm(&fingerprint), trips: 0 });
}

#[test]
fn the_guard_is_armed_before_the_first_probed_frame_and_cleared_when_the_probe_ends() {
    let mut glass = Glass::new(None).remembering(Memory::in_memory(Stored::default()));
    let mut stage = stage();
    glass.settle(&mut stage);
    let fingerprint = fingerprint_of(&stage);
    assert!(glass.memory().stored().guard.tripped(&fingerprint), "armed before the probe");
    assert_eq!(glass.pass_tier(), Tier::Lens);
    assert!(!glass.timing(), "the screen settles first");
    glass.head_start_ms = probe::SETTLE_MS;
    for n in 0..probe::PROBE_FRAMES {
        glass.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
    }
    assert_eq!(glass.pass_tier(), Tier::Lens, "a fast chain keeps the lens");
    assert_eq!(glass.memory().stored(), &Stored { tier: Some(Tier::Lens), guard: Guard::clear(&fingerprint), trips: 0 });

    // A failing chain ends the probe too: the guard clears, nothing new is stored.
    let mut failing = Glass::new(None).remembering(Memory::in_memory(Stored::default()));
    let mut stage = Recorder { end_fails: true, ..self::stage() };
    failing.settle(&mut stage);
    failing.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert_eq!(failing.pass_tier(), Tier::Baked);
    assert!(!failing.memory().stored().guard.in_flight);
    assert_eq!(failing.memory().stored().tier, None);
}

/// The guard covers the chain's first frame only: an app that crashes later, in a stream or before
/// the probe's verdict, keeps its glass on the next start.
#[test]
fn the_guard_clears_after_the_first_clean_frame_and_when_a_stream_starts() {
    let mut glass = Glass::new(None).remembering(Memory::in_memory(Stored::default()));
    let mut stage = stage();
    glass.settle(&mut stage);
    let fingerprint = fingerprint_of(&stage);
    assert!(glass.memory().stored().guard.tripped(&fingerprint));
    glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
    assert!(!glass.memory().stored().guard.in_flight, "one clean frame clears it");
    assert_eq!(glass.memory().stored().tier, None, "the probe has no verdict yet");
    assert_eq!(glass.pass_tier(), Tier::Lens);

    // A stream that starts before the chain ever ran draws no chain either.
    let mut streaming = Glass::new(None).remembering(Memory::in_memory(Stored::default()));
    let mut stage = self::stage();
    streaming.settle(&mut stage);
    assert!(streaming.memory().stored().guard.in_flight);
    let _ = streaming.set_over_video(true);
    assert!(!streaming.memory().stored().guard.in_flight);
}

#[test]
fn a_fast_screen_is_judged_against_a_120_hz_frame() {
    let mut glass = Glass::live();
    let mut stage = stage();
    // 5.5 ms: over 70 % of a 240 Hz frame, within 70 % of a 120 Hz one.
    stage.ms = 5.5;
    glass.set_refresh(240.0);
    for n in 0..probe::PROBE_FRAMES {
        glass.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
    }
    assert_eq!(glass.pass_tier(), Tier::Live);
    assert!(!glass.timing(), "decided");
    assert_eq!(probe::budget_ns(240.0), probe::budget_ns(120.0));
}

#[test]
fn the_tier_is_baked_until_set_and_solid_in_high_contrast() {
    crate::theme::own_palette();
    let ctx = Context::default();
    assert_eq!(tier(&ctx), Tier::Baked);
    set_tier(&ctx, Tier::Live);
    assert_eq!(tier(&ctx), Tier::Live);
    crate::theme::set_high_contrast(true);
    assert_eq!(tier(&ctx), Tier::Solid);
    crate::theme::set_high_contrast(false);
}
