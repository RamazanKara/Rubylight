use super::*;

#[test]
fn a_pairing_link_or_a_frontend_is_not_the_player_opening_nightjar() {
    assert!(opened_by_player(false, false, false));
    assert!(!opened_by_player(true, false, false));
    assert!(!opened_by_player(false, true, false));
    assert!(!opened_by_player(false, false, true));
}

fn target() -> Target {
    Target { host: 1, pc: "Desk".into(), game: OpenGame { host: "uid".into(), install_id: "x".into(), app_id: 7, title: "Dota 2".into() } }
}

fn online(apps: AppsSeen) -> Option<HostSeen> {
    Some(HostSeen { reach: Reach::Online, can_wake: true, paired: true, apps, session: SessionSeen::Free, reads: None })
}

fn ready() -> Seen {
    Seen { no_dialog: true, identity: true, network: true, host: online(AppsSeen::Loaded(Some(7))), ..Seen::default() }
}

/// The dialog stands in front while the start is under way.
fn going() -> Seen {
    Seen { countdown: true, no_dialog: false, ..ready() }
}

fn offline(can_wake: bool) -> Option<HostSeen> {
    Some(HostSeen { reach: Reach::Offline, can_wake, paired: true, apps: AppsSeen::NotLoaded, session: SessionSeen::Waiting, reads: None })
}

/// A native PC's page during the start, with its session reads as counted.
fn reading(asked: u64, answered: u64, session: SessionSeen) -> Seen {
    let host = HostSeen { session, reads: Some(SessionReads { asked, answered }), ..online(AppsSeen::Loaded(Some(7))).unwrap() };
    Seen { host: Some(host), ..going() }
}

#[test]
fn who_plays_is_read_again_when_the_countdown_ends() {
    let now = Instant::now();
    let mut run = Run::new(target());
    let free = SessionSeen::Free;
    assert_eq!(run.next(&ready(), now), Step::Count(3));
    let end = now + Duration::from_millis(policy::COUNTDOWN_MS);
    assert_eq!(run.next(&reading(5, 5, free), end), Step::Begin, "free as of a read from before the end");
    assert_eq!(run.next(&reading(5, 5, free), end), Step::ReadSession);
    assert_eq!(run.next(&reading(6, 5, free), end), Step::Wait, "asked, so not asked again");
    assert_eq!(run.next(&reading(6, 6, free), end), Step::Start(7));
}

#[test]
fn a_read_under_way_when_the_countdown_ends_does_not_count() {
    let now = Instant::now();
    let mut run = Run::new(target());
    let free = SessionSeen::Free;
    assert_eq!(run.next(&ready(), now), Step::Count(3));
    let end = now + Duration::from_millis(policy::COUNTDOWN_MS);
    assert_eq!(run.next(&reading(5, 4, free), end), Step::Begin);
    assert_eq!(run.next(&reading(5, 5, free), end), Step::ReadSession, "that read started before the end");
}

#[test]
fn a_game_another_device_started_during_the_countdown_stops_the_start() {
    let now = Instant::now();
    let mut run = Run::new(target());
    assert_eq!(run.next(&ready(), now), Step::Count(3));
    let end = now + Duration::from_millis(policy::COUNTDOWN_MS);
    assert_eq!(run.next(&reading(5, 5, SessionSeen::Free), end), Step::Begin);
    assert!(matches!(run.next(&reading(6, 6, SessionSeen::Elsewhere), end), Step::Stopped(_)));
}

#[test]
fn a_game_that_ended_during_the_countdown_does_not_stop_the_start() {
    let now = Instant::now();
    let mut run = Run::new(target());
    assert_eq!(run.next(&ready(), now), Step::Count(3));
    let end = now + Duration::from_millis(policy::COUNTDOWN_MS);
    let elsewhere = SessionSeen::Elsewhere;
    assert_eq!(run.next(&reading(5, 5, elsewhere), end), Step::Begin);
    assert_eq!(run.next(&reading(5, 5, elsewhere), end), Step::ReadSession, "that says who played before the end");
    assert_eq!(run.next(&reading(6, 6, SessionSeen::Free), end), Step::Start(7));
}

#[test]
fn a_read_that_never_answers_ends_within_the_limit() {
    let now = Instant::now();
    let mut run = Run::new(target());
    assert_eq!(run.next(&ready(), now), Step::Count(3));
    let end = now + Duration::from_millis(policy::COUNTDOWN_MS);
    assert_eq!(run.next(&reading(5, 5, SessionSeen::Free), end), Step::Begin);
    let late = end + START_LIMIT + Duration::from_secs(1);
    assert!(matches!(run.next(&reading(6, 5, SessionSeen::Free), late), Step::Failed(Some(_))));
}

/// A run past its countdown.
fn begun(now: Instant) -> Run {
    let mut run = Run::new(target());
    assert_eq!(run.next(&ready(), now), Step::Count(3));
    let counting = Seen { countdown: true, no_dialog: false, ..ready() };
    assert_eq!(run.next(&counting, now + Duration::from_millis(2100)), Step::Count(1));
    assert_eq!(run.next(&counting, now + Duration::from_secs(3)), Step::Begin);
    assert!(run.armed());
    run
}

#[test]
fn the_pin_screen_and_a_dialog_from_before_come_first() {
    let now = Instant::now();
    let mut run = Run::new(target());
    assert_eq!(run.next(&Seen { pin_screen: true, ..ready() }, now), Step::Wait);
    assert_eq!(run.next(&Seen { no_dialog: false, ..ready() }, now), Step::Wait);
    assert!(!run.armed());
    assert_eq!(run.next(&ready(), now), Step::Count(3), "the full countdown after them");
}

#[test]
fn a_start_with_no_network_opens_as_usual_and_never_arms() {
    let now = Instant::now();
    let mut run = Run::new(target());
    let step = run.next(&Seen { network: false, ..ready() }, now);
    assert_eq!(step, Step::Stopped("No network, so Nightjar opened as usual.".into()));
    assert!(!run.armed());
}

#[test]
fn a_sign_in_start_waits_for_the_network_and_then_hears_from_the_pc_first() {
    let now = Instant::now();
    let mut run = Run { at_sign_in: true, ..Run::new(target()) };
    let down = Seen { network: false, ..ready() };
    assert_eq!(run.next(&down, now), Step::Wait);
    assert_eq!(run.next(&down, now + Duration::from_secs(59)), Step::Wait);
    assert!(!run.armed());
    let back = now + Duration::from_secs(59);
    assert_eq!(run.next(&ready(), back), Step::Repoll);
    assert_eq!(run.next(&ready(), back + Duration::from_secs(5)), Step::Wait, "a poll's time to answer");
    assert_eq!(run.next(&ready(), back + NETWORK_SETTLE), Step::Count(3));
}

#[test]
fn a_sign_in_start_gives_up_on_the_network_in_the_end() {
    let now = Instant::now();
    let mut run = Run { at_sign_in: true, ..Run::new(target()) };
    let down = Seen { network: false, ..ready() };
    assert_eq!(run.next(&down, now), Step::Wait);
    assert!(matches!(run.next(&down, now + NETWORK_LIMIT), Step::Stopped(_)));
    assert!(!run.armed());
}

#[test]
fn a_network_there_from_the_start_needs_no_settling() {
    let now = Instant::now();
    let mut run = Run { at_sign_in: true, ..Run::new(target()) };
    assert_eq!(run.next(&ready(), now), Step::Count(3));
}

#[test]
fn a_pc_not_reached_yet_gets_its_time_at_sign_in_only() {
    let now = Instant::now();
    let mut run = Run { at_sign_in: true, ..begun(now) };
    let away = Seen { host: offline(false), ..going() };
    assert_eq!(run.next(&away, now), Step::Wait);
    assert_eq!(run.next(&going(), now), Step::Start(7), "it answered");
    let mut run = Run { at_sign_in: true, ..begun(now) };
    assert!(matches!(run.next(&away, now + Duration::from_secs(3) + START_LIMIT + Duration::from_secs(1)), Step::Failed(Some(_))));
}

#[test]
fn a_game_started_before_the_countdown_ends_the_run_without_touching_the_guard() {
    let now = Instant::now();
    let mut run = Run { at_sign_in: true, ..Run::new(target()) };
    assert_eq!(run.next(&Seen { busy: true, network: false, ..ready() }, now), Step::Over { disarm: false });
}

#[test]
fn cancelling_the_countdown_starts_nothing_and_leaves_the_guard_alone() {
    let now = Instant::now();
    let mut run = Run::new(target());
    run.next(&ready(), now);
    assert_eq!(run.next(&ready(), now), Step::Over { disarm: false }, "the dialog is gone");
}

#[test]
fn an_online_pc_starts_the_game_and_a_lasting_stream_disarms() {
    let now = Instant::now();
    let mut run = begun(now);
    assert_eq!(run.next(&going(), now), Step::Start(7));
    assert_eq!(run.next(&Seen { busy: true, ..ready() }, now), Step::Wait);
    let streaming = Seen { streaming: true, ..ready() };
    assert_eq!(run.next(&streaming, now), Step::Wait);
    assert_eq!(run.next(&streaming, now + Duration::from_secs(9)), Step::Wait);
    assert_eq!(run.next(&streaming, now + Duration::from_secs(10)), Step::Over { disarm: true });
}

#[test]
fn a_stream_that_dies_young_keeps_the_guard_unless_the_player_left_it() {
    let now = Instant::now();
    let start = |seen: &Seen| {
        let mut run = begun(now);
        run.next(&going(), now);
        run.next(&Seen { streaming: true, ..ready() }, now);
        run.next(seen, now + Duration::from_secs(4))
    };
    assert_eq!(start(&ready()), Step::Over { disarm: false });
    assert_eq!(start(&Seen { left_by_player: true, ..ready() }), Step::Over { disarm: true });
    assert_eq!(start(&Seen { busy: true, ..ready() }), Step::Wait, "reconnecting");
}

#[test]
fn a_stream_that_died_young_is_not_cleared_by_a_reconnect_across_the_mark() {
    let now = Instant::now();
    let at = |secs| now + Duration::from_secs(secs);
    let reconnecting = Seen { busy: true, ..ready() };
    let streaming = Seen { streaming: true, ..ready() };
    let mut run = begun(now);
    run.next(&going(), now);
    assert_eq!(run.next(&streaming, now), Step::Wait);
    assert_eq!(run.next(&reconnecting, at(4)), Step::Wait, "the host died");
    assert_eq!(run.next(&reconnecting, at(11)), Step::Wait, "no stream, so nothing lasted");
    assert_eq!(run.next(&ready(), at(12)), Step::Over { disarm: false }, "the reconnect gave up");

    let mut run = begun(now);
    run.next(&going(), now);
    run.next(&streaming, now);
    run.next(&reconnecting, at(4));
    assert_eq!(run.next(&streaming, at(8)), Step::Wait, "back, and its clock starts over");
    assert_eq!(run.next(&streaming, at(17)), Step::Wait, "9 s of it");
    assert_eq!(run.next(&streaming, at(18)), Step::Over { disarm: true });
}

#[test]
fn a_failed_launch_ends_on_its_message_with_the_guard_armed() {
    let now = Instant::now();
    let mut run = begun(now);
    run.next(&going(), now);
    assert_eq!(run.next(&Seen { message: true, no_dialog: false, ..ready() }, now), Step::Failed(None));
}

#[test]
fn an_automatic_start_launches_without_reading_who_plays() {
    assert!(matches!(unasked(Command::Launch { host: 1, app: 7, resume: true }), Command::LaunchUnasked { host: 1, app: 7, resume: true }));
    assert!(matches!(unasked(Command::Wake(1)), Command::Wake(1)));
}

#[test]
fn another_game_met_at_launch_stops_the_start_and_asks_nothing() {
    let now = Instant::now();
    let mut run = begun(now);
    assert_eq!(run.game_running(), None, "a launch of the player's own still asks");
    assert_eq!(run.next(&going(), now), Step::Start(7));
    let said = run.game_running().expect("this run launched");
    assert!(said.contains(&run.target.pc), "{said}");
}

#[test]
fn a_prompt_waits_and_its_cancel_is_the_players_doing() {
    let now = Instant::now();
    let mut run = begun(now);
    run.next(&going(), now);
    assert_eq!(run.next(&Seen { no_dialog: false, ..ready() }, now), Step::Wait);
    assert_eq!(run.next(&ready(), now), Step::Over { disarm: true });
}

#[test]
fn an_offline_pc_is_woken_and_then_started() {
    let now = Instant::now();
    let mut run = begun(now);
    assert_eq!(run.next(&Seen { host: offline(true), ..going() }, now), Step::Wake);
    let away = Seen { host: offline(true), ..ready() };
    assert_eq!(run.next(&Seen { waking: true, ..away }, now + Duration::from_secs(120)), Step::Wait, "the wake has its own limit");
    assert_eq!(run.next(&Seen { host: online(AppsSeen::NotLoaded), ..ready() }, now + Duration::from_secs(120)), Step::Count(0), "Cancel again");
    assert_eq!(run.next(&Seen { host: online(AppsSeen::NotLoaded), ..going() }, now + Duration::from_secs(120)), Step::LoadApps);
    assert_eq!(run.next(&Seen { host: online(AppsSeen::Loading), ..going() }, now + Duration::from_secs(121)), Step::Wait);
    assert_eq!(run.next(&going(), now + Duration::from_secs(122)), Step::Start(7));
}

#[test]
fn a_wake_that_times_out_fails_on_its_message_and_a_stopped_one_is_the_players() {
    let now = Instant::now();
    let away = Seen { host: offline(true), ..ready() };
    let mut run = begun(now);
    run.next(&Seen { host: offline(true), ..going() }, now);
    assert_eq!(run.next(&Seen { message: true, ..away.clone() }, now), Step::Failed(None));
    let mut run = begun(now);
    run.next(&Seen { host: offline(true), ..going() }, now);
    assert_eq!(run.next(&away, now), Step::Over { disarm: true });
}

#[test]
fn the_player_can_still_say_no_while_the_list_loads() {
    let now = Instant::now();
    let mut run = begun(now);
    assert_eq!(run.next(&Seen { host: online(AppsSeen::NotLoaded), ..going() }, now), Step::LoadApps);
    let cancelled = Seen { host: online(AppsSeen::Loading), ..ready() };
    assert_eq!(run.next(&cancelled, now), Step::Over { disarm: true }, "Cancel closed the dialog");
}

#[test]
fn a_game_the_player_started_meanwhile_ends_the_run() {
    let now = Instant::now();
    for (name, seen) in [
        ("connecting to it", Seen { busy: true, ..going() }),
        ("streaming it", Seen { streaming: true, ..ready() }),
    ] {
        let mut run = begun(now);
        assert_eq!(run.next(&Seen { host: online(AppsSeen::Loading), ..going() }, now), Step::Wait);
        assert_eq!(run.next(&seen, now), Step::Over { disarm: true }, "{name}");
    }
}

#[test]
fn a_game_playing_on_another_device_stops_the_start_and_is_never_moved() {
    let now = Instant::now();
    let mut there = online(AppsSeen::Loaded(Some(7))).unwrap();
    there.session = SessionSeen::Elsewhere;
    let mut run = begun(now);
    let step = run.next(&Seen { host: Some(there), ..going() }, now);
    assert!(matches!(&step, Step::Stopped(why) if why.contains("Desk")), "{step:?}");
}

#[test]
fn the_start_waits_for_the_first_read_of_the_session() {
    let now = Instant::now();
    let mut there = online(AppsSeen::Loaded(Some(7))).unwrap();
    there.session = SessionSeen::Waiting;
    let waiting = Seen { host: Some(there), ..going() };
    let mut run = begun(now);
    assert_eq!(run.next(&waiting, now), Step::Wait);
    assert!(matches!(run.next(&waiting, now + Duration::from_secs(3) + START_LIMIT + Duration::from_secs(1)), Step::Failed(Some(_))), "a read that never comes");
    let mut run = begun(now);
    assert_eq!(run.next(&waiting, now), Step::Wait);
    assert_eq!(run.next(&going(), now), Step::Start(7), "the read says it is free");
}

#[test]
fn only_a_live_session_of_another_device_counts_as_elsewhere() {
    let session = |state: &str, owner: &str| -> Session {
        serde_json::from_str(&include_str!("../../../client/tests/fixtures/native-api/session.json").replace("\"suspended\"", &format!("\"{state}\"")).replace("\"you\"", &format!("\"{owner}\""))).unwrap()
    };
    assert_eq!(SessionSeen::of(true, Some(&session("streaming", "other_device")), false), SessionSeen::Elsewhere);
    assert_eq!(SessionSeen::of(true, Some(&session("suspended", "other_device")), false), SessionSeen::Elsewhere);
    let mut no_app = session("streaming", "other_device");
    no_app.app = None;
    assert_eq!(SessionSeen::of(true, Some(&no_app), false), SessionSeen::Elsewhere, "what a real host sends");
    assert_eq!(SessionSeen::of(true, Some(&session("streaming", "you")), false), SessionSeen::Free);
    assert_eq!(SessionSeen::of(true, Some(&session("idle", "other_device")), false), SessionSeen::Free);
    assert_eq!(SessionSeen::of(true, None, false), SessionSeen::Waiting);
    assert_eq!(SessionSeen::of(true, None, true), SessionSeen::Waiting, "a read that failed says nothing");
    assert_eq!(SessionSeen::of(true, Some(&session("idle", "you")), true), SessionSeen::Waiting, "and free before is old news");
    assert_eq!(SessionSeen::of(true, Some(&session("streaming", "other_device")), true), SessionSeen::Elsewhere);
    assert_eq!(SessionSeen::of(false, None, false), SessionSeen::Free, "no native API, no owner to wait for");
}

#[test]
fn what_cannot_start_says_why() {
    let now = Instant::now();
    let failed = |seen: Seen| matches!(begun(now).next(&seen, now), Step::Failed(Some(_)));
    assert!(failed(Seen { host: None, ..going() }), "the PC was removed");
    assert!(failed(Seen { host: offline(false), ..going() }), "no address");
    assert!(failed(Seen { host: Some(HostSeen { reach: Reach::Online, can_wake: true, paired: false, apps: AppsSeen::NotLoaded, session: SessionSeen::Waiting, reads: None }), ..going() }), "not paired");
    assert!(failed(Seen { host: online(AppsSeen::Loaded(None)), ..going() }), "not in the list");
    assert!(failed(Seen { host: online(AppsSeen::Failed("x".into())), ..going() }), "no list");
    let mut run = begun(now);
    let unknown = Seen { host: Some(HostSeen { reach: Reach::Unknown, can_wake: true, paired: true, apps: AppsSeen::NotLoaded, session: SessionSeen::Waiting, reads: None }), ..going() };
    assert_eq!(run.next(&unknown, now), Step::Wait);
    assert!(matches!(run.next(&unknown, now + START_LIMIT + Duration::from_secs(10)), Step::Failed(Some(_))), "a PC that never answers");
}

fn store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nightjar-open-play-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::at(dir.clone()).unwrap(), dir)
}

fn recent() -> Recent {
    Recent { host: "uid".into(), install_id: "x".into(), app_id: 7, title: "Dota 2".into(), at: 1 }
}

fn known(unique: &str) -> Option<(u64, String)> {
    (unique == "uid").then(|| (1, "Desk".to_owned()))
}

#[test]
fn a_failed_start_skips_the_next_one_says_so_once_and_then_it_runs_again() {
    let (store, dir) = store("guard");
    let settings = Settings { open_on_start: OpenOnStart::Last, ..Settings::default() };
    let recents = [recent()];
    let (run, notice) = at_start(&store, &settings, &recents, true, known);
    assert_eq!(run.map(|r| r.target), Some(target()));
    assert!(notice.is_none());
    // The start failed, or the app died during it: nothing cleared the guard.
    store.save_open_play(&crate::config::OpenPlayState { game: None, armed: true }).unwrap();
    let (run, notice) = at_start(&store, &settings, &recents, true, known);
    assert!(run.is_none());
    assert!(notice.is_some_and(|(_, body)| body.contains("Dota 2")));
    let (run, notice) = at_start(&store, &settings, &recents, true, known);
    assert!(run.is_some() && notice.is_none(), "one failure skips one start");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_a_launcher_start_counts_and_the_default_starts_nothing() {
    let (store, dir) = store("entry");
    let last = Settings { open_on_start: OpenOnStart::Last, ..Settings::default() };
    let recents = [recent()];
    assert!(at_start(&store, &last, &recents, false, known).0.is_none(), "a command line");
    assert!(at_start(&store, &Settings::default(), &recents, true, known).0.is_none());
    assert!(at_start(&store, &last, &[], true, known).0.is_none(), "nothing played yet");
    assert!(at_start(&store, &last, &recents, true, |_| None).0.is_none(), "its PC is gone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_a_copy_the_player_opened_alone_starts_a_game() {
    assert!(may_start_a_game(true, false, false, false));
    assert!(!may_start_a_game(false, false, false, false), "a command line");
    assert!(!may_start_a_game(true, true, false, false), "an update's restart");
    assert!(!may_start_a_game(true, false, true, false), "a stream in another copy");
    assert!(!may_start_a_game(true, false, false, true), "another window");
}

#[test]
fn a_second_copy_leaves_the_guard_of_the_first_alone() {
    let (store, dir) = store("second");
    let settings = Settings { open_on_start: OpenOnStart::Last, ..Settings::default() };
    store.save_open_play(&crate::config::OpenPlayState { game: None, armed: true }).unwrap();
    let (run, notice) = at_start(&store, &settings, &[recent()], may_start_a_game(true, false, false, true), known);
    assert!(run.is_none() && notice.is_none());
    assert!(store.open_play().armed, "the first copy's start is still under way");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_one_copy_holds_the_window_until_it_ends() {
    let (store, dir) = store("window");
    let first = claim_window(store.dir()).unwrap();
    assert!(first.is_some());
    assert!(claim_window(store.dir()).unwrap().is_none(), "the second copy sees the first");
    drop(first);
    assert!(claim_window(store.dir()).unwrap().is_some(), "it ended");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_copy_that_lost_the_claim_takes_it_when_its_holder_ends_so_a_later_copy_stands_aside() {
    let (store, dir) = store("claim-again");
    let now = Instant::now();
    let first = claim_window(store.dir()).unwrap();
    let mut second = OpenPlay { claim_at: Some(now), ..OpenPlay::default() };
    second.keep_claiming(store.dir(), now);
    assert!(second._window.is_none(), "the first still holds it");
    assert_eq!(second.claim_at, Some(now + CLAIM_RETRY), "and it asks again");
    second.keep_claiming(store.dir(), now);
    assert_eq!(second.claim_at, Some(now + CLAIM_RETRY), "not before its time");
    drop(first);
    second.keep_claiming(store.dir(), now + CLAIM_RETRY);
    assert!(second._window.is_some());
    assert_eq!(second.claim_at, None);
    assert!(claim_window(store.dir()).unwrap().is_none(), "a third copy sees the second");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_claim_that_fails_says_so_when_a_start_was_due() {
    let why = std::io::Error::other("no locks");
    assert!(claim_failed(false, &why).is_none(), "nothing was going to start");
    let (title, body) = claim_failed(true, &why).unwrap();
    assert_eq!(title, "Automatic start skipped");
    assert!(body.contains("no locks"), "{body}");
}

#[test]
fn a_chosen_game_is_the_one_that_starts() {
    let (store, dir) = store("chosen");
    let settings = Settings { open_on_start: OpenOnStart::Chosen, ..Settings::default() };
    let chosen = OpenGame { host: "uid".into(), install_id: "y".into(), app_id: 9, title: "Hades".into() };
    assert!(at_start(&store, &settings, &[recent()], true, known).0.is_none(), "none chosen yet");
    store.save_open_play(&crate::config::OpenPlayState { game: Some(chosen.clone()), armed: false }).unwrap();
    let (run, _) = at_start(&store, &settings, &[recent()], true, known);
    assert_eq!(run.map(|r| r.target.game), Some(chosen));
    let _ = std::fs::remove_dir_all(&dir);
}
