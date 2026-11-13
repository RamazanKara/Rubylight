use super::*;

fn game(id: u32) -> SessionApp {
    SessionApp {
        install_id: "c6f1a9a4".into(),
        app_id: id,
        title: "Dota 2".into(),
    }
}
fn set(devices: &[&str]) -> BTreeSet<String> {
    devices.iter().map(|d| (*d).to_owned()).collect()
}
/// A game that "me" started, with the given streams and retained displays.
fn facts(streaming: &[&str], retained: &[&str]) -> Facts {
    Facts {
        launch_pending: false,
        streaming: set(streaming),
        game_running: true,
        game: Some((game(5), "me".into())),
        retained: set(retained),
    }
}
fn nothing() -> Facts {
    Facts::default()
}
fn streaming_machine() -> Machine {
    let mut m = Machine::default();
    m.begin("me", Some(game(5)), 10);
    m.app_starting(11);
    m.launched(true, &facts(&[], &["me"]), 12);
    assert_eq!(m.state(), State::Launching);
    assert!(m.observe(&facts(&["me"], &["me"]), 13));
    assert_eq!(m.state(), State::Streaming);
    m
}
fn suspended_machine() -> Machine {
    let mut m = streaming_machine();
    assert!(m.observe(&facts(&[], &["me"]), 20));
    assert_eq!(m.state(), State::Suspended);
    m
}

#[test]
fn a_new_machine_is_idle_and_has_nothing_to_do() {
    let mut m = Machine::default();
    assert_eq!(m.state(), State::Idle);
    assert_eq!(m.journal(), None);
    assert!(!m.observe(&nothing(), 1));
    assert_eq!(m.plan_disconnect(), Err(Refusal::NoSession));
    assert_eq!(m.plan_end(), Err(Refusal::NoSession));
    assert_eq!(m.plan_close(false), Err(Refusal::NoSession));
    assert_eq!(m.plan_resume("me"), Err(Refusal::NoSession));
}

#[test]
fn a_launch_goes_through_preparing_and_launching_to_streaming() {
    let mut m = Machine::default();
    m.begin("me", Some(game(5)), 10);
    assert_eq!(m.state(), State::Preparing);
    assert_eq!(m.owner(), Some("me"));
    // Facts cannot move a session whose launch call still runs.
    assert!(!m.observe(&nothing(), 11));
    assert_eq!(m.state(), State::Preparing);
    m.app_starting(12);
    assert_eq!(m.state(), State::Launching);
    m.launched(true, &nothing(), 13);
    assert_eq!(m.state(), State::Launching);
    // The launch waits for its client while it is pending.
    let pending = Facts {
        launch_pending: true,
        ..facts(&[], &["me"])
    };
    assert!(!m.observe(&pending, 14));
    assert_eq!(m.state(), State::Launching);
    assert!(m.observe(&facts(&["me"], &["me"]), 15));
    assert_eq!(m.state(), State::Streaming);
    assert_eq!(
        m.snapshot(&facts(&["me"], &[]), None).since_unix_ms,
        Some(15)
    );
}

#[test]
fn a_failed_launch_falls_back_to_what_still_runs() {
    // Nothing before: idle.
    let mut m = Machine::default();
    m.begin("me", Some(game(5)), 1);
    m.launched(false, &nothing(), 2);
    assert_eq!(m.state(), State::Idle);
    assert_eq!(m.owner(), None);
    // A suspended session whose resume failed is suspended again.
    let mut m = suspended_machine();
    m.begin("me", Some(game(5)), 30);
    assert_eq!(m.state(), State::Preparing);
    m.launched(false, &facts(&[], &["me"]), 31);
    assert_eq!(m.state(), State::Suspended);
    // Another device's stream is untouched by a failed launch of mine.
    let mut m = streaming_machine();
    m.begin("other", Some(game(5)), 30);
    m.launched(false, &facts(&["me"], &["me"]), 31);
    assert_eq!(m.state(), State::Streaming);
    assert_eq!(m.owner(), Some("me"));
}

#[test]
fn a_client_that_never_connects_returns_the_session_to_where_it_was() {
    let mut m = suspended_machine();
    m.begin("me", Some(game(5)), 30);
    m.launched(true, &facts(&[], &["me"]), 31);
    assert_eq!(m.state(), State::Launching);
    // The pending launch expired.
    assert!(m.observe(&facts(&[], &["me"]), 62));
    assert_eq!(m.state(), State::Suspended);
    // With no game left it is idle.
    let mut m = Machine::default();
    m.begin("me", Some(game(5)), 1);
    m.launched(true, &nothing(), 2);
    assert!(m.observe(&nothing(), 40));
    assert_eq!(m.state(), State::Idle);
}

#[test]
fn the_last_stream_ending_suspends_a_game_that_keeps_its_display() {
    let mut m = streaming_machine();
    assert!(!m.observe(&facts(&["me"], &["me"]), 14));
    assert!(m.observe(&facts(&[], &["me"]), 20));
    assert_eq!(m.state(), State::Suspended);
    assert_eq!(m.owner(), Some("me"));
    assert_eq!(m.suspended_at_ms(), Some(20));
    // The same facts again change nothing, not even the revision.
    let revision = m.revision();
    assert!(!m.observe(&facts(&[], &["me"]), 25));
    assert_eq!(m.revision(), revision);
    assert_eq!(m.suspended_at_ms(), Some(20));
}

#[test]
fn a_stream_with_no_game_or_no_display_ends_idle_not_suspended() {
    for facts in [
        nothing(),
        Facts {
            game_running: true,
            game: Some((game(5), "me".into())),
            ..Default::default()
        },
        Facts {
            retained: set(&["me"]),
            ..Default::default()
        },
    ] {
        let mut m = streaming_machine();
        assert!(m.observe(&facts, 20));
        assert_eq!(m.state(), State::Idle);
        assert_eq!(m.owner(), None);
    }
}

#[test]
fn the_grace_period_ending_makes_a_suspended_session_idle() {
    let mut m = suspended_machine();
    // The host released the owner's display; the game keeps running.
    let after = Facts {
        retained: set(&[]),
        ..facts(&[], &[])
    };
    assert!(m.observe(&after, 99));
    assert_eq!(m.state(), State::Idle);
    let snapshot = m.snapshot(&after, Some(5));
    assert!(snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    assert_eq!(snapshot.suspended_until_unix_ms, None);
    assert_eq!(m.suspended_at_ms(), None);
}

#[test]
fn another_devices_retained_display_keeps_the_session_suspended() {
    let mut m = suspended_machine();
    assert!(m.observe(&facts(&[], &["other"]), 40));
    assert_eq!(m.state(), State::Suspended);
    assert_eq!(m.owner(), Some("other"));
}

#[test]
fn a_suspended_session_reports_when_its_grace_period_ends() {
    let m = suspended_machine();
    let f = facts(&[], &["me"]);
    assert_eq!(
        m.snapshot(&f, Some(7_200)).suspended_until_unix_ms,
        Some(7_220)
    );
    // A grace period of zero seconds in the settings means it never ends.
    assert_eq!(m.snapshot(&f, None).suspended_until_unix_ms, None);
    // Only a suspended session has a deadline.
    let m = streaming_machine();
    assert_eq!(m.snapshot(&f, Some(1)).suspended_until_unix_ms, None);
}

#[test]
fn reconnecting_resumes_a_suspended_session_and_supersedes_a_streaming_one() {
    for mut m in [suspended_machine(), streaming_machine()] {
        assert_eq!(m.plan_resume("me"), Ok(Plan::Run));
        assert_eq!(m.plan_resume("other"), Err(Refusal::NotOwner));
        m.begin("me", Some(game(5)), 40);
        assert_eq!(m.state(), State::Preparing);
        assert_eq!(m.plan_resume("me"), Err(Refusal::Busy));
        m.app_starting(41);
        m.launched(true, &facts(&[], &["me"]), 42);
        assert!(m.observe(&facts(&["me"], &["me"]), 43));
        assert_eq!(m.state(), State::Streaming);
        assert_eq!(m.owner(), Some("me"));
    }
}

#[test]
fn disconnect_only_acts_on_a_streaming_session_and_repeats_safely() {
    assert_eq!(streaming_machine().plan_disconnect(), Ok(Plan::Run));
    let mut m = streaming_machine();
    m.disconnected(&facts(&[], &["me"]), 30);
    assert_eq!(m.state(), State::Suspended);
    // The game is untouched, and a repeat is a no-op.
    assert_eq!(m.plan_disconnect(), Ok(Plan::Skip));
    let mut busy = Machine::default();
    busy.begin("me", None, 1);
    assert_eq!(busy.plan_disconnect(), Ok(Plan::Skip));
    // A launch that still runs cannot be torn down from underneath.
    assert!(!busy.stopping(None, None, 2));
    assert_eq!(busy.state(), State::Preparing);
}

#[test]
fn end_session_leaves_the_game_running_and_the_session_idle() {
    for mut m in [streaming_machine(), suspended_machine()] {
        assert_eq!(m.plan_end(), Ok(Plan::Run));
        assert!(m.stopping(None, None, 50));
        assert_eq!(m.state(), State::Stopping);
        // A second teardown joins the first.
        assert!(!m.stopping(None, None, 50));
        assert_eq!(m.owner(), Some("me"));
        assert_eq!(m.journal().unwrap().state, State::Stopping);
        // A repeat while stopping does nothing.
        assert_eq!(m.plan_end(), Ok(Plan::Skip));
        assert_eq!(m.plan_close(true), Ok(Plan::Skip));
        // Nothing is observed while the cleanup runs.
        assert!(!m.observe(&facts(&[], &["me"]), 51));
        assert_eq!(m.state(), State::Stopping);
        // The display is gone; the game is not.
        let after = facts(&[], &[]);
        m.stopped(&after, 52);
        assert_eq!(m.state(), State::Idle);
        let snapshot = m.snapshot(&after, None);
        assert!(snapshot.game_running);
        assert_eq!(snapshot.owner, None);
        assert_eq!(m.journal(), None);
        assert_eq!(m.plan_end(), Err(Refusal::NoSession));
    }
}

#[test]
fn close_game_also_closes_a_game_that_end_session_left_running() {
    let mut m = Machine::default();
    assert_eq!(m.plan_close(true), Ok(Plan::Run));
    assert!(m.stopping(Some("me"), Some(game(5)), 60));
    // A stopping session always has an owner.
    assert_eq!(m.owner(), Some("me"));
    m.stopped(&nothing(), 61);
    assert_eq!(m.state(), State::Idle);
    assert_eq!(m.plan_close(false), Err(Refusal::NoSession));
}

#[test]
fn end_and_close_are_busy_while_a_launch_runs() {
    let mut m = Machine::default();
    m.begin("me", Some(game(5)), 1);
    for _ in 0..2 {
        assert_eq!(m.plan_end(), Err(Refusal::Busy));
        assert_eq!(m.plan_close(true), Err(Refusal::Busy));
        assert_eq!(m.plan_resume("me"), Err(Refusal::Busy));
        m.app_starting(2);
        assert_eq!(m.state(), State::Launching);
    }
    m.launched(true, &nothing(), 3);
    assert_eq!(m.plan_end(), Err(Refusal::Busy));
}

#[test]
fn a_clean_shutdown_forgets_the_session() {
    let mut m = streaming_machine();
    assert!(m.journal().is_some());
    m.reset(70);
    assert_eq!(m.state(), State::Idle);
    assert_eq!(m.owner(), None);
    assert_eq!(m.journal(), None);
    // A launch that was running no longer blocks observation.
    let mut m = Machine::default();
    m.begin("me", None, 1);
    m.reset(2);
    assert!(m.observe(&facts(&["me"], &["me"]), 3));
    assert_eq!(m.state(), State::Streaming);
}

#[test]
fn a_stream_the_machine_did_not_begin_is_adopted() {
    let mut m = Machine::default();
    assert!(m.observe(&facts(&["me"], &["me"]), 5));
    assert_eq!(m.state(), State::Streaming);
    assert_eq!(m.owner(), Some("me"));
    assert_eq!(m.app(), Some(&game(5)));
}

#[test]
fn the_revision_moves_only_when_something_a_client_sees_changes() {
    let mut m = Machine::default();
    let start = m.revision();
    m.observe(&nothing(), 1);
    assert_eq!(m.revision(), start);
    m.begin("me", Some(game(5)), 2);
    let preparing = m.revision();
    assert!(preparing > start);
    m.app_starting(3);
    assert!(m.revision() > preparing);
}

#[test]
fn the_journal_round_trips_and_a_damaged_one_is_ignored() {
    let m = suspended_machine();
    let journal = m.journal().unwrap();
    assert_eq!(journal.state, State::Suspended);
    assert_eq!(Journal::parse(&journal.to_bytes()), Some(journal));
    assert_eq!(Journal::parse(b"{"), None);
    assert_eq!(Journal::parse(b""), None);
}

#[test]
fn the_snapshot_feeds_session_view_without_breaking_its_owner_rule() {
    use crate::native::{LAUNCH, LIST, VIEW, session_view};
    use crate::native_api::session::{ActionId, Owner};
    let caller: crate::state::Client = serde_json::from_value(serde_json::json!({
        "name": "Phone", "cert": "", "uuid": "me", "perm": LIST | VIEW | LAUNCH
    }))
    .unwrap();
    for (m, expected) in [
        (Machine::default(), Owner::None),
        (streaming_machine(), Owner::You),
        (suspended_machine(), Owner::You),
    ] {
        let f = facts(&[], &["me"]);
        let view = session_view(&m.snapshot(&f, Some(1000)), &caller);
        assert_eq!(view.owner, expected);
        // The machine's refusals agree with the action rules of the view.
        let allowed = |id: ActionId| view.actions.iter().find(|a| a.id == id).unwrap().allowed;
        assert_eq!(allowed(ActionId::End), m.plan_end().is_ok());
        assert_eq!(allowed(ActionId::Resume), m.plan_resume("me").is_ok());
        assert_eq!(
            allowed(ActionId::Disconnect),
            m.plan_disconnect() == Ok(Plan::Run)
        );
    }
}

fn pending(mut f: Facts) -> Facts {
    f.launch_pending = true;
    f
}

#[test]
fn a_launch_that_begins_during_a_teardown_keeps_the_state_when_the_teardown_ends() {
    // Teardown, then begin, then stopped.
    let mut m = suspended_machine();
    assert!(m.stopping(None, None, 30));
    assert_eq!(m.state(), State::Stopping);
    m.begin("you", Some(game(5)), 31);
    assert_eq!(m.state(), State::Preparing);
    assert_eq!(m.owner(), Some("you"));
    // The teardown ends while the launch call runs.
    m.stopped(&facts(&[], &["me"]), 32);
    assert_eq!(m.state(), State::Preparing, "the launch owns the state");
    assert_eq!(m.owner(), Some("you"));
    m.app_starting(33);
    assert_eq!(m.state(), State::Launching);
    m.launched(true, &pending(facts(&[], &[])), 34);
    assert_eq!(m.state(), State::Launching);
    assert!(m.observe(&facts(&["you"], &[]), 35));
    assert_eq!(m.state(), State::Streaming);

    // The teardown ends after the launch replied and waits for its client.
    let mut m = suspended_machine();
    assert!(m.stopping(None, None, 30));
    m.begin("you", Some(game(5)), 31);
    m.app_starting(32);
    m.launched(true, &pending(facts(&[], &[])), 33);
    m.stopped(&pending(facts(&[], &[])), 34);
    assert_eq!(m.state(), State::Launching);
    assert_eq!(m.owner(), Some("you"));
    // Nobody joins: it settles with what runs.
    assert!(m.observe(&facts(&[], &[]), 35));
    assert_eq!(m.state(), State::Idle);

    // The launch fails first: the teardown still runs, so the session
    // stays stopping until it ends.
    let mut m = suspended_machine();
    assert!(m.stopping(None, None, 30));
    m.begin("you", Some(game(5)), 31);
    m.launched(false, &facts(&[], &["me"]), 32);
    assert_eq!(m.state(), State::Stopping);
    assert!(!m.observe(&facts(&[], &["me"]), 32), "observe waits for it");
    m.stopped(&facts(&[], &["me"]), 33);
    assert_eq!(m.state(), State::Suspended);

    // A failed launch with no teardown running still falls back at once.
    let mut m = suspended_machine();
    m.begin("you", Some(game(5)), 31);
    m.launched(false, &facts(&[], &["me"]), 32);
    assert_eq!(m.state(), State::Suspended);
}

#[test]
fn a_second_teardown_keeps_stopping_until_the_last_one_ends() {
    let mut m = suspended_machine();
    assert!(m.stopping(None, None, 30));
    m.begin("me", Some(game(5)), 31);
    m.app_starting(32);
    m.launched(true, &pending(facts(&[], &[])), 33);
    // /cancel while the launch waits for its client.
    assert!(m.stopping(None, None, 34));
    assert_eq!(m.state(), State::Stopping);
    // The first teardown ends: the second is still running.
    m.stopped(&nothing(), 35);
    assert_eq!(m.state(), State::Stopping);
    m.stopped(&nothing(), 36);
    assert_eq!(m.state(), State::Idle);
}

#[test]
fn disconnect_does_not_overwrite_a_launch() {
    let mut m = suspended_machine();
    m.begin("me", Some(game(5)), 30);
    m.disconnected(&facts(&[], &["me"]), 31);
    assert_eq!(m.state(), State::Preparing);
    m.app_starting(32);
    m.launched(true, &pending(facts(&[], &["me"])), 33);
    m.disconnected(&pending(facts(&[], &["me"])), 34);
    assert_eq!(m.state(), State::Launching);
}
