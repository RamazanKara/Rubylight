//! Unit tests for the stream picture loop and crop/fallback decisions.

use super::*;

/// With Upscaling set to AI, a picture AI enhancement does not read, or a failed enhancer,
/// draws as bilinear scaling does: straight into the window's target, with no copy for it.
#[test]
fn ai_enhancement_is_in_effect_only_where_it_runs() {
    use config::Upscaling;
    assert!(ai_in_effect(Upscaling::Ai, false, true));
    assert!(!ai_in_effect(Upscaling::Ai, true, true), "failed this stream");
    assert!(!ai_in_effect(Upscaling::Ai, false, false), "a picture it does not read");
    assert!(!ai_in_effect(Upscaling::default(), false, true));
}

/// A size or frame rate the PC did not confirm says automatic quality stopped only when it
/// was on: a change from the stream menu or a window on another monitor says the change did
/// not apply.
#[test]
fn an_unconfirmed_mode_says_automatic_quality_stopped_only_when_it_was_on() {
    assert!(unconfirmed_mode_toast(true).contains("Automatic quality stopped"));
    let manual = unconfirmed_mode_toast(false);
    assert!(!manual.contains("Automatic quality"), "{manual}");
    assert!(manual.contains("did not apply"), "{manual}");
}

/// A picture format the presenter cannot show hands the decoder over after a few pictures in
/// a row, with the time the last was ready (never 0, which means none); a shown picture
/// starts the count over, and so does the hand-over, for the next decoder's pictures.
#[test]
fn pictures_that_cannot_be_shown_hand_the_decoder_over_after_a_few_in_a_row() {
    let mut unshown = Unshown::default();
    for ready_us in 1..UNSHOWN_BEFORE_FALLBACK as u64 { assert_eq!(unshown.failed(ready_us), None); }
    unshown.shown();
    for ready_us in 1..UNSHOWN_BEFORE_FALLBACK as u64 { assert_eq!(unshown.failed(ready_us), None, "a shown picture starts over"); }
    assert_eq!(unshown.failed(9_000), Some(9_000));
    for _ in 1..UNSHOWN_BEFORE_FALLBACK { assert_eq!(unshown.failed(9_500), None, "the next decoder counts anew"); }
    assert_eq!(unshown.failed(0), Some(1), "never 0");
}

/// The overlay's Render ends where the present call begins: a V-Sync wait or a compositor's
/// layer inside the call is the system's share, not Nightjar's drawing.
#[test]
fn a_frames_drawing_ends_at_the_present_call() {
    assert_eq!(super::frame_shares(1_000, 1_600, 9_900), (600, 8_300));
    assert_eq!(super::frame_shares(1_000, 1_000, 1_000), (0, 0));
    // Clocks that step back count nothing rather than wrap.
    assert_eq!(super::frame_shares(2_000, 1_500, 1_400), (0, 0));
}

/// `pump`'s crop over a stream: pictures of `coded` size arriving at `t0 + ms`, with `asked`
/// pending since `asked_ms` until a picture confirms it, as `pump` clears it. Returns each
/// picture's visible size and whether it confirmed.
fn stream(start: (i32, i32), asked: Option<((i32, i32), u64)>, pictures: &[((i32, i32), u64)]) -> Vec<((i32, i32), bool)> {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut sizes = StreamSizes::new(start);
    let mut pending = asked.map(|(size, ms)| { sizes.ask(size); (size, at(ms)) });
    pictures.iter().map(|&(coded, ms)| {
        let (visible, confirmed) = sizes.arrive(coded, pending, at(ms));
        if confirmed { pending = None; }
        (visible, confirmed)
    }).collect()
}

#[test]
fn old_pictures_during_a_size_change_keep_their_columns_and_do_not_confirm_it() {
    // An AV1 1920x1080 stream (coded 1920x1082) asked to step its width down to 1904, which AMD
    // codes 1920x1082 as well.
    let shown = stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1082), 100), ((1920, 1082), 900)]);
    assert_eq!(shown, [((1920, 1080), false); 2], "the old size's pictures, before SWITCH_GRACE");
    // After SWITCH_GRACE the PC's new pictures confirm the new size.
    let shown = stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1082), 100), ((1920, 1082), 1100)]);
    assert_eq!(shown, [((1920, 1080), false), ((1904, 1080), true)]);
    // A picture that is exactly an old size stays whole whatever is pending.
    assert_eq!(stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1080), 1100)]), [((1920, 1080), false)]);
    // Sizes far apart need no grace: 2184x1968 (coded 2240x1968) after 1920x1080.
    let shown = stream((1920, 1080), Some(((2184, 1968), 0)), &[((1920, 1082), 10), ((2240, 1968), 20)]);
    assert_eq!(shown, [((1920, 1080), false), ((2184, 1968), true)]);
}

#[test]
fn pictures_after_a_confirmed_step_down_stay_cropped_to_the_new_size() {
    // 1920x1080 changed to 1904x1080: both coded 1920x1082. Once confirmed, every later
    // picture is the new stream's, cropped to 1904, during and after SWITCH_GRACE.
    let shown = stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1082), 1100), ((1920, 1082), 1200), ((1920, 1082), 2500), ((1920, 1082), 60_000)]);
    assert_eq!(shown, [((1904, 1080), true), ((1904, 1080), false), ((1904, 1080), false), ((1904, 1080), false)]);
}



#[test]
fn a_late_size_applies_the_whole_mode_and_a_confirmation_only_the_rate() {
    let asked = Reconfigure { width: 2184, height: 1968, fps_millihz: 90_000 };
    let old = Reconfigure { width: 1920, height: 1080, fps_millihz: 60_000 };
    assert_eq!(mode_to_apply(true, false, Some(asked), Some(old)), Some(ModeApply::Rate(asked)));
    assert_eq!(mode_to_apply(false, true, None, Some(asked)), Some(ModeApply::Whole(asked)), "the timeout put the whole mode back");
    assert_eq!(mode_to_apply(false, false, Some(asked), Some(old)), None);
    assert_eq!(mode_to_apply(false, true, None, None), None);
}
#[test]
fn a_size_applied_after_the_timeout_reports_itself_for_its_mode() {
    let t0 = Instant::now();
    let mut sizes = StreamSizes::new((1920, 1080));
    sizes.ask((2184, 1968));
    // The timeout cleared the pending request; the PC switches later.
    assert_eq!(sizes.arrive_mode((2240, 1968), None, t0 + Duration::from_secs(6)), ((2184, 1968), false, true));
    // A picture of the size the stream already shows is no late apply.
    let (_, _, late) = sizes.arrive_mode((2240, 1968), None, t0 + Duration::from_secs(7));
    assert!(!late);
}
#[test]
fn a_size_the_pc_applies_after_the_timeout_is_still_cropped() {
    // The 5 s timeout cleared the pending 2184x1968 (`pump` drops it); the PC switches later.
    let t0 = Instant::now();
    let mut sizes = StreamSizes::new((1920, 1080));
    sizes.ask((2184, 1968));
    assert_eq!(sizes.arrive((2240, 1968), None, t0 + Duration::from_secs(6)), ((2184, 1968), false));
    // Another request the PC never answers: the late size stays the stream's.
    sizes.ask((2560, 1440));
    let pending = Some(((2560, 1440), t0 + Duration::from_secs(7)));
    assert_eq!(sizes.arrive((2240, 1968), pending, t0 + Duration::from_secs(8)), ((2184, 1968), false));
    assert_eq!(sizes.arrive((2240, 1968), None, t0 + Duration::from_secs(13)), ((2184, 1968), false), "after the second timeout");
}

#[test]
fn pictures_of_the_size_before_a_confirmed_change_are_still_cropped() {
    // 2184x1968 confirmed while 1920x1080 pictures (coded 1920x1082) are still queued.
    let shown = stream((1920, 1080), Some(((2184, 1968), 0)), &[((2240, 1968), 10), ((1920, 1082), 20), ((2240, 1968), 30), ((1920, 1082), 1500)]);
    assert_eq!(shown, [((2184, 1968), true), ((1920, 1080), false), ((2184, 1968), false), ((1920, 1082), false)], "the old size expires after SWITCH_GRACE");
}

#[test]
fn a_size_edited_in_the_menu_but_not_applied_does_not_change_the_crop() {
    // A 2184x1968 stream (coded 2240x1968); the menu's size editor writes `requested_mode`
    // while the user steps to 2200, but only `request_mode` (Apply) asks `StreamSizes`.
    let t0 = Instant::now();
    let mut sizes = StreamSizes::new((2184, 1968));
    assert_eq!(sizes.arrive((2240, 1968), None, t0), ((2184, 1968), false));
    // Applied, the PC's new pictures are cropped to it.
    sizes.ask((2200, 1968));
    assert_eq!(sizes.arrive((2240, 1968), Some(((2200, 1968), t0)), t0 + Duration::from_millis(10)), ((2200, 1968), true));
}

/// The header named a packed 4:4:4 stream by the setting asked for: its size is the logical one.
#[test]
fn a_packed_or_pyrowave_setup_gives_the_pictures_size() {
    let packed = session::VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 3840, height: 1080, fps: 60, chroma_packing: 1, logical_width: 1920, logical_height: 1080, color_space: 1, color_range: 0 };
    assert_eq!(setup_size(&packed), (1920, 1080));
    assert_eq!(setup_size(&session::VideoSetup { logical_width: 0, logical_height: 0, chroma_packing: 0, width: 2560, height: 1440, ..packed }), (2560, 1440));
}
