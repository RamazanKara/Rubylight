//! Unit tests for mid-stream bitrate and frame-rate changes (0x553C).

use super::*;
use rubylight_protocol::rate_change::Outcome;

fn stats(second: u64, loss: f32) -> Stats {
    Stats { now_ms: second * 1000, recent_video: true, loss_percent: Some(loss), ..Stats::default() }
}

fn result(request: &RateChangeRequest, bitrate_kbps: u32, fps_millihz: u32, bitrate: Outcome, fps: Outcome) -> RateChangeResult {
    RateChangeResult { serial: request.serial, bitrate_kbps, fps_millihz, bitrate, fps }
}

#[test]
fn only_a_full_stream_whose_host_listed_0x553c_offers_the_menu() {
    assert!(offered(&[0x5530, 0x553C], AppRole::Stream));
    assert!(!offered(&[0x5530, 0x5532], AppRole::Stream), "a launch the host did not take");
    assert!(!offered(&[], AppRole::Stream));
    assert!(!offered(&[0x553C], AppRole::InputOnly));
    assert!(!offered(&[0x553C], AppRole::RemoteMonitor));
}

#[test]
fn a_full_stream_asks_for_rate_changes_at_launch_and_no_other_does() {
    use nightjar_core::http::{HostCapabilities, LaunchRequest};
    for (role, asked) in [(AppRole::Stream, true), (AppRole::RemoteMonitor, false), (AppRole::InputOnly, false)] {
        let request = LaunchRequest { role, rate_change: asked_at_launch(role), ..LaunchRequest::default() };
        assert_eq!(request.query(&HostCapabilities::UNKNOWN, false).contains("&rlRateChange=1"), asked, "{role:?}");
    }
}

#[test]
fn the_rate_change_entry_replaces_the_other_bitrate_and_rate_controls() {
    // A PC with 0x553C: one bitrate and one rate control; the size editor keeps the size.
    assert_eq!(menu_controls(true, true, true),
        MenuControls { rate_change: true, http_bitrate: false, size_editor: true, size_editor_fps: false });
    assert_eq!(menu_controls(false, false, true),
        MenuControls { rate_change: true, http_bitrate: false, size_editor: false, size_editor_fps: false });
    // A PC without it keeps the older controls.
    assert_eq!(menu_controls(true, true, false),
        MenuControls { rate_change: false, http_bitrate: true, size_editor: true, size_editor_fps: true });
    assert_eq!(menu_controls(true, false, false),
        MenuControls { rate_change: false, http_bitrate: true, size_editor: false, size_editor_fps: false });
    assert_eq!(menu_controls(false, false, false),
        MenuControls { rate_change: false, http_bitrate: false, size_editor: false, size_editor_fps: false });
}

#[test]
fn the_menu_asks_only_for_what_changed_and_shows_every_answer() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    assert!(rates.menu_request(0).is_err(), "nothing to change");
    rates.menu_kbps = 40_000;
    let request = rates.menu_request(0).unwrap();
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (40_000, 0));
    assert!(rates.menu_busy());
    let answer = rates.result(&result(&request, 30_000, 60_000, Outcome::clamped(Reason::HostLimit), Outcome::UNCHANGED), 1000);
    assert_eq!(answer.toast.as_deref(), Some("Bitrate limited to 30000 kbps: the PC’s limit."));
    assert_eq!((answer.bitrate_kbps, answer.fps_millihz), (Some(30_000), None));
    assert!(!rates.menu_busy());
    assert_eq!(rates.menu_kbps, 30_000, "the field shows what the stream runs at");

    rates.menu_kbps = 50_000;
    rates.menu_fps = 119.88;
    let request = rates.menu_request(0).unwrap();
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (50_000, 119_880));
    let answer = rates.result(&result(&request, 50_000, 119_880, Outcome::APPLIED, Outcome::APPLIED), 2000);
    assert_eq!(answer.toast.as_deref(), Some("Bitrate set to 50000 kbps. Frame rate set to 119.880 fps."));
    assert_eq!(answer.fps_millihz, Some(119_880));

    // Refusals are loud too.
    rates.menu_fps = 144.0;
    let request = rates.menu_request(0).unwrap();
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 144_000));
    let answer = rates.result(&result(&request, 50_000, 119_880, Outcome::UNCHANGED, Outcome::refused(Reason::Disabled)), 3000);
    assert_eq!(answer.toast.as_deref(), Some("Frame rate not changed: the PC does not change this stream’s rates."));
    assert_eq!(answer.fps_millihz, None);
    assert_eq!(rates.menu_fps, 119.88, "the field goes back to the stream's rate");

    rates.menu_fps = 5.0;
    assert!(rates.menu_request(0).is_err(), "below the protocol's lowest rate");
    assert!(!rates.menu_busy());
    rates.menu_fps = 119.88;

    // A result nobody asked for changes nothing.
    let stray = RateChangeResult { serial: 99, ..result(&request, 1000, 30_000, Outcome::APPLIED, Outcome::APPLIED) };
    assert_eq!(rates.result(&stray, 4000), Answer::default());

    crate::strings::set(crate::strings::Lang::De);
    rates.menu_kbps = 10_000;
    let request = rates.menu_request(0).unwrap();
    let answer = rates.result(&result(&request, 10_000, 119_880, Outcome::APPLIED, Outcome::UNCHANGED), 5000);
    assert_eq!(answer.toast.as_deref(), Some("Bitrate auf 10000 kbit/s gesetzt."));
}

#[test]
fn the_controller_ticks_once_a_second_and_one_request_at_a_time() {
    let mut off = RateChange::new(20_000, 60_000, false, 0, 0);
    assert!((0..30).all(|s| off.tick(&stats(s, 10.0)).is_none()), "off by default");

    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    assert!(rates.hint_active());
    let at = |ms: u64| Stats { now_ms: ms, ..stats(0, 10.0) };
    assert_eq!(rates.tick(&at(2500)), None, "start delay");
    assert_eq!(rates.tick(&at(3000)), None, "under a second since the last tick");
    let request = rates.tick(&at(3500)).unwrap();
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (14_000, 0));
    assert!(!rates.menu_busy(), "the controller's request is not the menu's");
    assert_eq!(rates.tick(&stats(4, 10.0)), None, "the first is still pending");
    let answer = rates.result(&result(&request, 14_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 4000);
    assert_eq!(answer, Answer { bitrate_kbps: Some(14_000), ..Answer::default() }, "the controller's changes are quiet");
    assert_eq!(rates.tick(&stats(5, 10.0)), None, "cooldown");
    let request = rates.tick(&stats(6, 10.0)).unwrap();
    assert_eq!(request.bitrate_kbps, 9800);

    // A change by hand supersedes it: the controller follows instead of stopping.
    rates.menu_kbps = 30_000;
    let manual = rates.menu_request(0).unwrap();
    assert!(rates.result(&result(&request, 14_000, 60_000, Outcome::refused(Reason::Superseded), Outcome::UNCHANGED), 6100).toast.is_none());
    rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 6200);
    assert!(rates.hint_active());
    let request = rates.tick(&stats(9, 10.0)).unwrap();
    assert_eq!(request.bitrate_kbps, 21_000, "from the bitrate set by hand");

    // A refusal stops it, loudly.
    let answer = rates.result(&result(&request, 30_000, 60_000, Outcome::refused(Reason::Codec), Outcome::UNCHANGED), 10_000);
    assert_eq!(answer.toast.as_deref(), Some("Loss-based bitrate stopped: this stream’s codec keeps its rates"));
    assert!(!rates.hint_active() && !rates.hint_has_room());
    assert!((11..60).all(|s| rates.tick(&stats(s, 10.0)).is_none()));
}

/// After an automatic reconnect the client-hint controller's first request is the cut the
/// stream before had made; without a controller nothing is asked.
#[test]
fn a_resumed_hint_asks_for_the_earlier_cut_first() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    rates.resume_hint(9_000);
    assert_eq!(rates.tick(&stats(4, 0.0)).map(|request| request.bitrate_kbps), Some(9_000));
    let mut off = RateChange::new(20_000, 60_000, false, 0, 0);
    off.resume_hint(9_000);
    assert!(off.tick(&stats(4, 0.0)).is_none());
}

#[test]
fn a_result_that_comes_after_a_later_requests_keeps_that_ones_bitrate() {
    // The host holds the answer to a request with a rate until its switch ran, with the
    // bitrate it set then; a bitrate-only request sent after it is answered first.
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    rates.menu_kbps = 30_000;
    rates.menu_fps = 120.0;
    let first = rates.menu_request(0).unwrap();
    rates.menu_kbps = 40_000;
    rates.menu_fps = 60.0;
    let second = rates.menu_request(100).unwrap();
    assert_eq!((second.bitrate_kbps, second.fps_millihz), (40_000, 0));
    let newer = rates.result(&result(&second, 40_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 200);
    assert_eq!(newer.bitrate_kbps, Some(40_000));
    let older = rates.result(&result(&first, 30_000, 120_000, Outcome::APPLIED, Outcome::APPLIED), 900);
    assert_eq!(older.toast.as_deref(), Some("Bitrate set to 30000 kbps. Frame rate set to 120 fps."), "still said");
    assert_eq!((older.bitrate_kbps, older.fps_millihz), (None, Some(120_000)), "the rate is the stream's, the bitrate not");
    assert_eq!(rates.menu_kbps, 40_000);
    let request = rates.tick(&stats(4, 10.0)).unwrap();
    assert_eq!(request.bitrate_kbps, 28_000, "the controller follows the later bitrate");

    // Serials wrap past 0; one after the wrap is still newer.
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    rates.next_serial = u32::MAX;
    rates.menu_kbps = 30_000;
    let before = rates.menu_request(0).unwrap();
    rates.menu_kbps = 40_000;
    let after = rates.menu_request(0).unwrap();
    assert_eq!((before.serial, after.serial), (u32::MAX, 1));
    assert_eq!(rates.result(&result(&before, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 100).bitrate_kbps, Some(30_000));
    assert_eq!(rates.result(&result(&after, 40_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 200).bitrate_kbps, Some(40_000));
}

#[test]
fn never_keep_my_bitrate_sends_the_streams_bitrate_once_and_only_when_chosen() {
    let mut plain = RateChange::new(25_000, 60_000, false, 0, 0);
    assert_eq!(plain.takeover_request(1000), None, "another choice sends nothing");
    let mut rates = RateChange::new(25_000, 60_000, false, 0, 0);
    rates.keep_bitrate();
    let request = rates.takeover_request(1000).expect("the takeover");
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (25_000, 0), "the configured bitrate, no rate");
    assert_eq!(rates.takeover_request(2000), None, "once");
    assert!((0..30).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "no controller of its own runs");
    // The PC's answer is followed and shows nothing.
    let answer = rates.result(&result(&request, 25_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 1100);
    assert_eq!((answer.bitrate_kbps, answer.toast, answer.by_hand), (Some(25_000), None, false));
    // A PC that clamped it: the stream follows the PC's rate.
    let mut clamped = RateChange::new(25_000, 60_000, false, 0, 0);
    clamped.keep_bitrate();
    let request = clamped.takeover_request(0).unwrap();
    let answer = clamped.result(&result(&request, 20_000, 60_000, Outcome::clamped(Reason::HostLimit), Outcome::UNCHANGED), 100);
    assert_eq!((answer.bitrate_kbps, answer.toast), (Some(20_000), None));
}

#[test]
fn keep_my_bitrate_in_the_menu_sends_once_the_bitrate_the_player_chose_or_the_streams() {
    // Absent on a PC that did not take this stream's rate changes.
    assert_eq!(keep_switch(None, false), None);
    // A stream started with Never shows it on before and after the takeover, and turning it on sends nothing more.
    let mut never = RateChange::new(25_000, 60_000, false, 0, 0);
    never.keep_bitrate();
    assert_eq!(keep_switch(Some(&never), false), Some(true));
    assert_eq!(never.keep_now(500), None, "the takeover is still to go out");
    assert!(never.takeover_request(1000).is_some());
    assert_eq!(keep_switch(Some(&never), false), Some(true));
    assert_eq!((never.keep_now(2000), never.takeover_request(3000)), (None, None), "nothing more");
    // The PC decides: off until turned on, then exactly one change at the stream's bitrate.
    let mut rates = RateChange::new(25_000, 60_000, false, 0, 0);
    assert_eq!(keep_switch(Some(&rates), false), Some(false));
    let request = rates.keep_now(1000).expect("one change");
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (25_000, 0), "the stream's bitrate, no rate");
    assert_eq!(rates.keep_now(2000), None, "once");
    let answer = rates.result(&result(&request, 25_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 1100);
    assert_eq!((answer.toast, answer.by_hand), (None, false), "its answer shows nothing");
    // Off shows off and sends nothing.
    rates.release();
    assert_eq!(keep_switch(Some(&rates), false), Some(false));
    assert_eq!(rates.takeover_request(3000), None);
    // A bitrate the player chose in the menu already makes the PC stand back: on, and kept at that bitrate.
    let mut chosen = RateChange::new(25_000, 60_000, false, 0, 0);
    chosen.menu_kbps = 40_000;
    let manual = chosen.menu_request(0).unwrap();
    chosen.result(&result(&manual, 40_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 100);
    assert_eq!(keep_switch(Some(&chosen), false), Some(true));
    assert_eq!(chosen.keep_now(150), None, "the PC already keeps it");
    chosen.release();
    assert_eq!(chosen.keep_now(200).map(|r| r.bitrate_kbps), Some(40_000), "the player's bitrate, not the stream's");
    // Lower it sooner steering: off; on stops that controller and sends the stream's own bitrate.
    let mut hint = RateChange::new(25_000, 60_000, true, 0, 0);
    assert_eq!(keep_switch(Some(&hint), false), Some(false));
    assert_eq!(hint.keep_now(1000).map(|r| r.bitrate_kbps), Some(25_000));
    assert!(!hint.hint_active() && hint.kept());
    assert!((0..30).all(|s| hint.tick(&stats(s, 10.0)).is_none()), "this device lowers nothing");
    // The busy-network controller steering: off.
    assert_eq!(keep_switch(Some(&never), true), Some(false));
}

#[test]
fn a_takeover_the_pc_refused_never_answered_or_that_could_not_go_out_is_not_kept() {
    let mut refused = RateChange::new(25_000, 60_000, false, 0, 0);
    refused.keep_bitrate();
    let request = refused.takeover_on_tick(true, 1000).unwrap();
    refused.result(&result(&request, 25_000, 60_000, Outcome::refused(Reason::Disabled), Outcome::UNCHANGED), 1100);
    assert_eq!(keep_switch(Some(&refused), false), Some(false));
    let mut silent = RateChange::new(25_000, 60_000, false, 0, 0);
    silent.keep_bitrate();
    silent.takeover_on_tick(true, 1000).unwrap();
    assert_eq!(silent.expire(1000 + ANSWER_MS), None, "says nothing");
    assert_eq!(keep_switch(Some(&silent), false), Some(false));
    let mut unsent = RateChange::new(25_000, 60_000, false, 0, 0);
    let request = unsent.keep_now(0).unwrap();
    unsent.send_failed(&request, &"closed");
    assert!(!unsent.kept());
}

#[test]
fn the_takeover_goes_out_with_the_first_tick_that_saw_video_and_the_start_rules_hold() {
    let mut rates = RateChange::new(25_000, 60_000, false, 0, 0);
    rates.keep_bitrate();
    assert_eq!(rates.takeover_on_tick(false, 500), None, "no video yet");
    assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(25_000));
    assert_eq!(rates.takeover_on_tick(true, 2000), None);
    // Under Never, or after a bitrate the player chose, the stream starts no controller of its own.
    assert!(!own_lowering_at_start(false, true, true, true));
    assert!(!own_lowering_at_start(true, false, true, false));
    assert!(own_lowering_at_start(false, false, true, false) && own_lowering_at_start(false, false, false, true));
    // A reconnect sends the takeover again for Never and for a bitrate the player chose.
    assert!(takeover_at_start(true, false, false));
    assert!(takeover_at_start(false, true, false));
    assert!(!takeover_at_start(false, true, true), "Lower it sooner steers it");
    assert!(!takeover_at_start(false, false, false));
}

/// The PC's answer to the query: the stream's bitrate as it counts it, nothing changed. Rubylight
/// 6.0.1 counts a stream launched at 20000 as its encoder's 14988, newer PCs as 20000.
fn answered(request: &RateChangeRequest, kbps: u32) -> RateChangeResult {
    result(request, kbps, 60_000, Outcome::UNCHANGED, Outcome::refused(Reason::Invalid))
}

#[test]
fn on_an_older_pc_keep_my_bitrate_and_the_menu_start_from_what_it_answered() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    rates.keep_bitrate();
    let query = rates.query_request(0);
    assert_eq!((query.bitrate_kbps, query.fps_millihz), (0, 0), "asks for nothing");
    assert_eq!(rates.takeover_on_tick(true, 1000), None, "nothing sets a bitrate before the answer");
    assert_eq!(keep_switch(Some(&rates), false), Some(true));
    let answer = rates.result(&answered(&query, 14_988), 1100);
    assert_eq!((answer.toast, answer.by_hand, answer.fps_millihz), (None, false, None), "says nothing");
    assert_eq!(answer.bitrate_kbps, Some(14_988), "the stream follows the PC's count");
    assert_eq!(rates.menu_kbps, 14_988, "the menu's field");
    let keep = rates.takeover_on_tick(true, 2000).expect("the takeover");
    assert_eq!((keep.bitrate_kbps, keep.fps_millihz), (14_988, 0), "not 20000, which would raise the encoder");
    assert_eq!(rates.takeover_on_tick(true, 3000), None, "once");
    // A PC's answer to the query is never a takeover's: the switch stays on.
    assert_eq!(keep_switch(Some(&rates), false), Some(true));
}

#[test]
fn on_an_older_pc_the_client_hint_controller_starts_from_what_it_answered() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    let query = rates.query_request(0);
    assert!((0..10).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "nothing before the answer");
    assert!(rates.hint_active(), "it owns the bitrate meanwhile, so the PC's adaptive bitrate stays off");
    rates.result(&answered(&query, 14_988), 10_000);
    let cut = (11..20).find_map(|s| rates.tick(&stats(s, 10.0))).expect("a cut");
    assert!(cut.bitrate_kbps < 14_988, "{}", cut.bitrate_kbps);
}

#[test]
fn on_a_newer_pc_the_answer_is_the_streams_own_bitrate_and_everything_is_as_before() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    let query = rates.query_request(0);
    let answer = rates.result(&answered(&query, 20_000), 100);
    assert_eq!(answer, Answer::default(), "nothing to follow");
    assert_eq!(rates.tick(&stats(2, 10.0)), None, "the start delay");
    assert_eq!(rates.tick(&stats(4, 10.0)).map(|r| r.bitrate_kbps), Some(14_000));
    let mut kept = RateChange::new(20_000, 60_000, false, 0, 0);
    kept.keep_bitrate();
    let query = kept.query_request(0);
    kept.result(&answered(&query, 20_000), 100);
    assert_eq!(kept.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(20_000));
}

#[test]
fn with_no_answer_the_stream_goes_on_as_before_once_and_a_late_answer_changes_nothing() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    rates.keep_bitrate();
    let query = rates.query_request(0);
    assert_eq!(rates.takeover_on_tick(true, 4000), None);
    assert_eq!(rates.expire(ANSWER_MS - 1), None);
    assert_eq!(rates.expire(ANSWER_MS), None, "quietly");
    assert_eq!(rates.takeover_on_tick(true, ANSWER_MS).map(|r| r.bitrate_kbps), Some(20_000), "the stream's own");
    assert_eq!(rates.result(&answered(&query, 14_988), ANSWER_MS + 100), Answer::default(), "too late");
    assert_eq!(rates.menu_kbps, 20_000);
    assert_eq!(rates.takeover_on_tick(true, ANSWER_MS + 1000), None, "once");

    let mut hint = RateChange::new(20_000, 60_000, true, 0, 0);
    hint.query_request(0);
    assert!((0..5).all(|s| hint.tick(&stats(s, 10.0)).is_none()));
    assert_eq!(hint.expire(ANSWER_MS), None);
    assert_eq!(hint.tick(&stats(5, 10.0)).map(|r| r.bitrate_kbps), Some(14_000), "from the stream's own");
}

#[test]
fn a_query_that_cannot_be_sent_yet_goes_once_more_a_second_later() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    rates.keep_bitrate();
    let query = rates.query_request(0);
    rates.query_unsent(&query, &"not yet", 0);
    assert_eq!(rates.query_again(500), None);
    assert_eq!(rates.takeover_on_tick(true, 500), None, "still waits for it");
    let again = rates.query_again(1000).expect("once more");
    assert_eq!((again.bitrate_kbps, again.fps_millihz), (0, 0));
    assert_eq!(rates.query_again(2000), None);
    // Video ran meanwhile, so a cut by the PC may be in the answer: not kept, the stream's own goes out.
    assert_eq!(rates.result(&answered(&again, 14_988), 1100), Answer::default());
    assert_eq!(rates.menu_kbps, 20_000);
    assert_eq!(rates.takeover_on_tick(true, 2000).map(|r| r.bitrate_kbps), Some(20_000));

    // Twice unsent: the stream goes on from its own bitrate, as before the query.
    let mut unsent = RateChange::new(20_000, 60_000, false, 0, 0);
    unsent.keep_bitrate();
    let query = unsent.query_request(0);
    unsent.query_unsent(&query, &"closed", 0);
    let again = unsent.query_again(1000).unwrap();
    unsent.query_unsent(&again, &"closed", 1000);
    assert_eq!(unsent.query_again(5000), None);
    assert_eq!(unsent.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(20_000));
}

#[test]
fn a_cut_the_pc_made_before_the_answer_is_not_kept() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    rates.keep_bitrate();
    let query = rates.query_request(0);
    // The PC's link-rate controller lowered the bitrate before it handled the query: its answer is the cut.
    rates.pc_lowered();
    assert_eq!(rates.result(&answered(&query, 12_000), 100), Answer::default());
    assert_eq!(rates.menu_kbps, 20_000);
    assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(20_000), "the stream's own, as before the query");

    let mut hint = RateChange::new(20_000, 60_000, true, 0, 0);
    let query = hint.query_request(0);
    hint.pc_lowered();
    hint.result(&answered(&query, 12_000), 100);
    assert_eq!(hint.tick(&stats(4, 10.0)).map(|r| r.bitrate_kbps), Some(14_000), "from the stream's own");

    // After the answer a cut changes nothing of it.
    let mut later = RateChange::new(20_000, 60_000, false, 0, 0);
    later.keep_bitrate();
    let query = later.query_request(0);
    later.result(&answered(&query, 14_988), 100);
    later.pc_lowered();
    assert_eq!(later.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(14_988));
}

#[test]
fn while_the_query_waits_the_menu_cannot_change_the_bitrate() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    let query = rates.query_request(0);
    assert!(rates.bitrate_locked());
    // Refused by the state, not only by the disabled control: the field goes back to the stream's.
    rates.menu_kbps = 30_000;
    assert!(rates.menu_change(100).is_err(), "nothing else to change");
    assert_eq!(rates.menu_kbps, 20_000);
    rates.menu_kbps = 30_000;
    rates.menu_fps = 120.0;
    let request = rates.menu_change(200).unwrap();
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 120_000), "the rate alone");
    // A clean answer: usable, at the PC's number.
    rates.result(&answered(&query, 14_988), 300);
    assert!(!rates.bitrate_locked());
    assert_eq!(rates.menu_kbps, 14_988);
    rates.menu_kbps = 30_000;
    assert_eq!(rates.menu_change(400).map(|r| r.bitrate_kbps), Ok(30_000));

    // An answer that is not used: usable, at the stream's own number.
    let mut tainted = RateChange::new(20_000, 60_000, false, 0, 0);
    let query = tainted.query_request(0);
    tainted.pc_lowered();
    tainted.result(&answered(&query, 12_000), 100);
    assert!(!tainted.bitrate_locked());
    assert_eq!(tainted.menu_kbps, 20_000);

    // No answer: usable after the timeout, at the stream's own number.
    let mut silent = RateChange::new(20_000, 60_000, false, 0, 0);
    silent.query_request(0);
    assert!(silent.bitrate_locked());
    silent.expire(ANSWER_MS);
    assert!(!silent.bitrate_locked());
    assert_eq!(silent.menu_kbps, 20_000);

    // A reconnect is a new stream: a new wait, the control locked and nothing left from before.
    let mut again = RateChange::new(30_000, 60_000, false, 0, 0);
    again.query_request(0);
    assert!(again.bitrate_locked());
    assert_eq!(again.menu_kbps, 30_000);
}

/// The bitrate's unit plays no part in a frame rate: picked before the answer, it goes out at once
/// with the bitrate unchanged, and the bitrate stays the stream's after an older PC's answer.
#[test]
fn a_frame_rate_picked_before_the_answer_goes_out_at_once_and_leaves_the_bitrate_alone() {
    let mut rates = RateChange::new(20_000, 60_000, false, 0, 0);
    let query = rates.query_request(0);
    rates.menu_fps = 120.0;
    let request = rates.menu_change(100).expect("goes out at once");
    assert_eq!((request.bitrate_kbps, request.fps_millihz), (0, 120_000));
    rates.result(&answered(&query, 14_988), 200);
    assert_eq!(rates.menu_kbps, 14_988, "the menu shows the stream's bitrate as the PC counts it");
    rates.result(&result(&request, 14_988, 120_000, Outcome::UNCHANGED, Outcome::APPLIED), 300);
    rates.menu_fps = 90.0;
    let next = rates.menu_change(400).expect("a rate");
    assert_eq!((next.bitrate_kbps, next.fps_millihz), (0, 90_000), "the encoder's bitrate is left alone");
    assert_eq!(rates.menu_kbps, 14_988);
}

#[test]
fn a_reconnect_after_the_player_chose_a_bitrate_sends_that_bitrate_whatever_the_pc_answers() {
    // The player's 30000 came from this PC's own answers, so it is in the PC's unit already.
    let mut rates = RateChange::new(30_000, 60_000, false, 0, 0);
    rates.chosen_before(30_000);
    rates.keep_bitrate();
    let query = rates.query_request(0);
    rates.result(&answered(&query, 22_488), 100);
    assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(30_000));
}

#[test]
fn keep_my_bitrate_turned_on_before_the_answer_sends_what_the_pc_answered() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    let query = rates.query_request(0);
    assert_eq!(rates.keep_now(500), None, "nothing before the answer");
    assert!(rates.kept() && !rates.hint_active());
    rates.result(&answered(&query, 14_988), 600);
    assert_eq!(rates.takeover_on_tick(true, 1000).map(|r| r.bitrate_kbps), Some(14_988));
    assert!((0..30).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "this device lowers nothing");
}

#[test]
fn keep_my_bitrate_is_stored_as_settings_stores_never_and_the_pc_decides() {
    let mut s = crate::config::Settings { lower_bitrate: LowerBitrate::WhenFramesLost, ..Default::default() };
    store_keep(&mut s, true);
    assert_eq!(s.lower_bitrate, LowerBitrate::Keep);
    store_keep(&mut s, false);
    assert_eq!(s.lower_bitrate, LowerBitrate::Off, "the default");
}

#[test]
fn the_controller_waits_for_a_change_by_hand() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    rates.menu_kbps = 30_000;
    let manual = rates.menu_request(0).unwrap();
    assert!((3..10).all(|s| rates.tick(&stats(s, 10.0)).is_none()), "the menu's request is pending");
    rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 10_000);
    assert_eq!(rates.tick(&stats(12, 10.0)).map(|r| r.bitrate_kbps), Some(21_000));
}

#[test]
fn a_request_that_cannot_be_sent_is_loud_and_stops_the_controller() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    let request = rates.tick(&stats(3, 10.0)).unwrap();
    assert_eq!(rates.send_failed(&request, &"closed"), "Loss-based bitrate stopped: closed");
    assert!(!rates.hint_active());
    rates.menu_kbps = 30_000;
    let request = rates.menu_request(0).unwrap();
    assert_eq!(rates.send_failed(&request, &"closed"), "Could not change the stream: closed");
    assert!(!rates.menu_busy());
}

#[test]
fn a_request_the_host_never_answers_is_given_up_on_loudly() {
    let mut rates = RateChange::new(20_000, 60_000, true, 0, 0);
    rates.menu_kbps = 30_000;
    let manual = rates.menu_request(1000).unwrap();
    assert_eq!(rates.expire(5999), None);
    assert_eq!(rates.expire(6000).as_deref(), Some("The PC did not answer the rate change."));
    assert!(!rates.menu_busy(), "the menu takes a new request");
    assert_eq!(rates.result(&result(&manual, 30_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 7000), Answer::default(), "too late");
    let request = rates.tick(&stats(8, 10.0)).unwrap();
    assert_eq!(rates.expire(12_999), None);
    assert_eq!(rates.expire(13_000).as_deref(), Some("Loss-based bitrate stopped: the PC did not answer"));
    assert!(!rates.hint_active());
    assert_eq!(rates.result(&result(&request, 14_000, 60_000, Outcome::APPLIED, Outcome::UNCHANGED), 13_100), Answer::default());
}
