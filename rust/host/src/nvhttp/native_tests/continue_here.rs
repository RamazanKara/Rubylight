//! `POST /session/continue-here` and what the session says around it.
use super::*;
use butterpollo_core::session::{Seat, Transport};

/// Another paired device, by name.
fn device(s: &Setup, uuid: &str, name: &str, perm: u32) -> (Client, Connection) {
    let identity = Identity::generate().unwrap();
    let client = Client {
        name: name.into(),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm,
        enabled: true,
        extra: Default::default(),
    };
    s.host()
        .paired
        .write()
        .unwrap()
        .add(&s.host().paired_path, client.clone())
        .unwrap();
    let connection = Connection {
        certificate: Some(identity.der.clone()),
        ..s.f.connection(true)
    };
    (client, connection)
}
fn game_owner(s: &Setup) -> String {
    let current = s.host().current_app.lock().unwrap();
    current.as_ref().unwrap().owner.clone()
}
async fn press(s: &Setup, connection: &Connection) -> Reply {
    api(&s.f, connection, "POST", "/session/continue-here").await
}
async fn session_of(s: &Setup, connection: &Connection) -> ApiSession {
    api(&s.f, connection, "GET", "/session").await.session()
}
/// A session with this launch, running.
fn running(s: &Setup, launch: Launch) -> Arc<Session> {
    let session = Session::new(launch, Default::default());
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    session
}

#[tokio::test]
async fn the_stream_moves_and_the_game_and_its_display_stay() {
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "Living room TV", ALL);
    let before = session_of(&s, &tv_connection).await;
    let playing = before.playing_on.expect("the TV sees who plays");
    assert_eq!(playing.device, s.client.name);
    assert_eq!(
        before.owner,
        butterpollo_core::native_api::session::Owner::OtherDevice
    );

    let reply = press(&s, &tv_connection).await.session();
    assert!(phone.stopping(), "the phone's stream ends");
    assert!(reply.moved_to.is_none());
    assert!(s.game_running(), "the game keeps running");
    assert_eq!(
        s.displays(),
        1,
        "the phone's display stays until the TV's stream runs"
    );
    assert_eq!(
        game_owner(&s),
        tv.uuid,
        "the phone's recovery finds no game of its own"
    );

    let moved = s.get("/session").await.session();
    assert_eq!(moved.moved_to.expect("the note").device, "Living room TV");
    assert_eq!(
        moved.playing_on.expect("the phone can take it back").device,
        "Living room TV"
    );

    // The TV's own stream settles it, and the phone's display goes without the grace period.
    let tv_stream = s.stream_as(tv.clone());
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 0);
    assert!(s.game_running());
    assert!(!s.host().claim_live());
    let streaming = session_of(&s, &tv_connection).await;
    assert_eq!(
        streaming.owner,
        butterpollo_core::native_api::session::Owner::You
    );
    assert!(streaming.playing_on.is_none());
    assert_eq!(s.state(), SessionState::Streaming);
    drop(tv_stream);
}

#[tokio::test]
async fn the_note_lasts_about_two_minutes_and_its_own_press_clears_it() {
    let s = Setup::new();
    let _phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    assert!(s.get("/session").await.session().moved_to.is_some());
    s.host().moves.expire_notes();
    assert!(s.get("/session").await.session().moved_to.is_none());

    let s = Setup::new();
    let _phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    assert!(s.get("/session").await.session().moved_to.is_some());
    s.host().moves.expire_claim();
    s.host().settle_move();
    assert!(
        s.get("/session").await.session().moved_to.is_none(),
        "the TV never came, so the stream did not move"
    );

    // The phone's own press, once the TV's is over and with a newer note of its own.
    let (laptop, laptop_connection) = device(&s, "laptop", "Laptop", ALL);
    press(&s, &laptop_connection).await.session();
    let laptop = s.stream_as(laptop);
    s.host().settle_move();
    assert!(!s.host().claim_live());
    assert!(s.get("/session").await.session().moved_to.is_some());
    press(&s, &s.connection).await.session();
    assert!(
        s.get("/session").await.session().moved_to.is_none(),
        "the phone asked for the game itself"
    );
    drop(laptop);
}

#[tokio::test]
async fn a_caller_without_view_and_launch_is_refused_and_changes_nothing() {
    for (uuid, perm, code) in [
        ("view", VIEW_BIT, "permission_required"),
        ("launch", LAUNCH, "permission_required"),
        ("list", LIST, "permission_required"),
        ("list-launch", LIST | LAUNCH, "permission_required"),
        // No permission at all reads as a device that is turned off.
        ("none", 0, "not_paired"),
    ] {
        let s = Setup::new();
        let phone = s.streaming();
        let (_, connection) = device(&s, uuid, "Refused", perm);
        press(&s, &connection).await.assert_error(403, code);
        assert!(!phone.stopping(), "{uuid}");
        assert_eq!(game_owner(&s), s.client.uuid, "{uuid}");
        assert!(
            s.get("/session").await.session().moved_to.is_none(),
            "{uuid}"
        );
        assert!(!s.host().claim_live(), "{uuid}");
    }
}

#[tokio::test]
async fn a_device_without_list_or_view_does_not_learn_the_name() {
    let s = Setup::new();
    let _phone = s.streaming();
    let (_, launch_only) = device(&s, "launch", "Launch only", LAUNCH);
    assert!(session_of(&s, &launch_only).await.playing_on.is_none());
    let (_, list_only) = device(&s, "list", "List only", LIST);
    assert!(session_of(&s, &list_only).await.playing_on.is_some());
}

#[tokio::test]
async fn a_device_without_list_or_view_learns_that_it_moved_but_not_where() {
    for (perm, named) in [(LAUNCH, false), (LIST, true), (VIEW_BIT, true)] {
        let s = Setup::with(perm);
        let _phone = s.streaming();
        let (_, tv_connection) = device(&s, "tv", "Living room TV", ALL);
        press(&s, &tv_connection).await.session();
        let moved = s.get("/session").await.session().moved_to;
        let moved = moved.expect("it must stop recovering either way");
        assert_eq!(moved.device, if named { "Living room TV" } else { "" });
        assert!(moved.since_unix_ms > 0);
    }
}

#[tokio::test]
async fn co_op_background_input_only_and_monitor_sessions_are_untouched() {
    let s = Setup::new();
    let phone = s.streaming();
    let (guest, _) = device(&s, "guest", "Guest", ALL);
    let mut coop = s.f.launch(guest.clone(), Role::Stream);
    coop.nucleus = Some(Seat {
        session: 1,
        slot: 0,
    });
    let coop = running(&s, coop);
    let mut background = s.f.launch(guest.clone(), Role::Stream);
    background.background = Some(true);
    let background = running(&s, background);
    let input = s.f.session(guest.clone(), Role::InputOnly);
    let monitor = s.f.session(guest, Role::RemoteMonitor);
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    let own = s.f.session(tv, Role::InputOnly);

    press(&s, &tv_connection).await.session();
    assert!(phone.stopping());
    for session in [&coop, &background, &input, &monitor, &own] {
        assert!(!session.stopping(), "{:?}", session.launch.role);
    }
    assert!(
        s.host().moves.note_for("guest").is_none(),
        "nothing moved away from the co-op player"
    );
}

#[tokio::test]
async fn only_a_device_that_loses_a_stream_or_a_display_reads_that_it_moved() {
    // The game's owner left long ago, and the session's owner is a co-op player.
    let s = Setup::new();
    s.run_game();
    let (guest, _) = device(&s, "guest", "Guest", ALL);
    let mut coop = s.f.launch(guest, Role::Stream);
    coop.nucleus = Some(Seat {
        session: 1,
        slot: 0,
    });
    let coop = running(&s, coop);
    assert_eq!(
        s.host().session_snapshot().owner.as_deref(),
        Some("guest"),
        "the machine names the co-op player"
    );
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    assert!(!coop.stopping());
    assert_eq!(game_owner(&s), tv.uuid);
    assert!(s.host().moves.note_for("guest").is_none());
    assert!(s.host().moves.note_for(&s.client.uuid).is_none());
    // The game still goes back to its owner when the TV does not come.
    s.host().moves.expire_claim();
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
}

#[tokio::test]
async fn a_co_op_or_background_game_is_not_taken() {
    let s = Setup::new();
    let (guest, _) = device(&s, "guest", "Guest", ALL);
    let mut coop = s.f.launch(guest, Role::Stream);
    coop.nucleus = Some(Seat {
        session: 1,
        slot: 0,
    });
    let coop = running(&s, coop);
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    assert!(session_of(&s, &tv_connection).await.playing_on.is_none());
    press(&s, &tv_connection)
        .await
        .assert_error(409, "no_session");
    assert!(!coop.stopping());

    s.run_game();
    s.host()
        .current_app
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .background = true;
    assert!(session_of(&s, &tv_connection).await.playing_on.is_none());
    press(&s, &tv_connection)
        .await
        .assert_error(409, "no_session");
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(!coop.stopping());
}

#[tokio::test]
async fn a_stream_that_ended_in_the_meantime_is_no_failure() {
    let s = Setup::new();
    let phone = s.streaming();
    phone.stop();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    assert_eq!(s.state(), SessionState::Suspended);
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    assert!(session_of(&s, &tv_connection).await.playing_on.is_some());
    press(&s, &tv_connection).await.session();
    assert!(s.game_running());
    assert_eq!(game_owner(&s), tv.uuid);
    assert!(s.get("/session").await.session().moved_to.is_some());
}

#[tokio::test]
async fn with_no_game_left_the_press_fails_and_says_so() {
    let s = Setup::new();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection)
        .await
        .assert_error(409, "no_session");
    assert!(!s.host().claim_live());
}

#[tokio::test]
async fn of_two_devices_pressing_at_once_the_first_wins() {
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    let (laptop, laptop_connection) = device(&s, "laptop", "Laptop", ALL);
    let (first, second) = tokio::join!(press(&s, &tv_connection), press(&s, &laptop_connection));
    assert!(phone.stopping());
    let (winner, loser, loser_connection) = if first.status == StatusCode::OK {
        second.assert_error(409, "session_busy");
        (tv, laptop, laptop_connection)
    } else {
        first.assert_error(409, "session_busy");
        second.session();
        (laptop, tv, tv_connection)
    };
    assert_eq!(game_owner(&s), winner.uuid);
    assert!(session_of(&s, &loser_connection).await.moved_to.is_none());
    // The loser still waits.
    press(&s, &loser_connection)
        .await
        .assert_error(409, "session_busy");
    assert_eq!(game_owner(&s), winner.uuid);
    // Once the winner streams, a later press is a new move, and it ends the winner's stream.
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    let stream = s.stream_as(winner.clone());
    s.host().reap_paused_display();
    press(&s, &loser_connection).await.session();
    assert!(stream.stopping());
    assert_eq!(game_owner(&s), loser.uuid);
}

#[tokio::test]
async fn a_second_press_by_the_winner_does_not_make_it_the_previous_owner() {
    let s = Setup::new();
    let _phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    press(&s, &tv_connection).await.session();
    assert_eq!(game_owner(&s), tv.uuid);
    assert!(s.host().claim_live());
    assert!(
        s.get("/session").await.session().moved_to.is_some(),
        "the phone's note stays"
    );
    s.host().moves.expire_claim();
    s.host().settle_move();
    assert_eq!(
        game_owner(&s),
        s.client.uuid,
        "the game goes back to the phone, not to a device that never streamed"
    );
}

#[tokio::test]
async fn a_press_whose_own_launch_would_be_refused_changes_nothing() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert(butterpollo_core::background::SETTING.into(), "true".into());
    press(&s, &tv_connection)
        .await
        .assert_error(409, "host_in_use");
    assert!(!phone.stopping());
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(!s.host().claim_live());
    assert!(s.get("/session").await.session().moved_to.is_none());
}

#[tokio::test]
async fn a_failed_launch_ends_the_press_at_once() {
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    assert!(phone.stopping());
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    // An app the host does not have: the launch is refused like any other that fails.
    let refused = launch_webrtc(
        s.host(),
        WebRtcLaunch {
            client: tv.clone(),
            peer: std::net::Ipv4Addr::LOCALHOST.into(),
            app_id: 999_999,
            width: 1280,
            height: 720,
            fps: 60,
            hdr: false,
        },
    );
    assert_eq!(refused.err(), Some(WebRtcLaunchError::InvalidApplication));
    assert!(!s.host().claim_live(), "no need to wait out the 15 s");
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(s.get("/session").await.session().moved_to.is_none());
    assert!(s.game_running());
    assert_eq!(s.displays(), 1, "the phone can come back to its display");
}

#[tokio::test]
async fn another_devices_failed_launch_leaves_the_press_alone() {
    let s = Setup::new();
    let _phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host().launch_failed(&s.client.uuid);
    assert!(s.host().claim_live());
    // Nor does the device's own failure while an earlier launch of it can still connect.
    let pending = s.f.launch(tv.clone(), Role::Stream);
    let canceled = s.host().sessions.lock().unwrap().queue(pending).unwrap();
    drop(canceled);
    s.host().launch_failed(&tv.uuid);
    assert!(s.host().claim_live());
}

#[tokio::test]
async fn an_exclusive_layouts_short_grace_does_not_take_the_display_during_a_press() {
    let s = Setup::new();
    s.run_game();
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("dd_config_revert_delay".into(), "0".into());
    let config = s.host().config.read().unwrap().clone();
    let display = crate::display_session::tests::retained_exclusive_display(s.host(), &config);
    s.host()
        .app_display
        .lock()
        .unwrap()
        .insert(s.client.uuid.clone(), (display, None));
    let phone = s.stream_as(s.client.clone());
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 1, "it stays until the TV's stream runs");
    assert_eq!(s.state(), SessionState::Suspended);
    // The TV's stream runs: it goes right away.
    let stream = s.stream_as(tv);
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 0);
    drop(stream);
}

#[tokio::test]
async fn after_a_failed_press_the_grace_of_the_kept_display_applies_again() {
    let s = Setup::new();
    let phone = s.streaming();
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("dd_config_revert_delay".into(), "0".into());
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 1);
    s.host().moves.expire_claim();
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 0);
    assert!(s.game_running());
}

#[tokio::test]
async fn a_stream_that_arrives_after_the_press_expired_still_releases_the_display() {
    let s = Setup::new();
    let phone = s.streaming();
    // A kept display that would otherwise stay for hours.
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("dd_config_revert_on_disconnect".into(), "false".into());
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 1);
    let stream = s.stream_as(tv);
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 0, "the late stream ends the wait");
    assert!(s.game_running());
    drop(stream);

    // The device the press displaced came back in the meantime: its display stays its own.
    let s = Setup::new();
    let phone = s.streaming();
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("dd_config_revert_on_disconnect".into(), "false".into());
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().settle_move();
    s.host().moves.forget(&s.client.uuid);
    let stream = s.stream_as(tv);
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 1);
    drop(stream);
}

#[tokio::test]
async fn a_press_is_tied_to_its_game() {
    let s = Setup::new();
    let _phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    assert!(s.host().claim_live());
    // Another game runs now: the press and what it said are over, for the first to ask.
    s.run_game();
    assert!(s.get("/session").await.session().moved_to.is_none());
    assert!(!s.host().claim_live());
    assert_eq!(game_owner(&s), s.client.uuid);
    // So is it when the game is gone.
    let s = Setup::new();
    let _phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    *s.host().current_app.lock().unwrap() = None;
    assert!(s.get("/session").await.session().moved_to.is_none());
    assert!(!s.host().claim_live());
}

#[tokio::test]
async fn the_note_of_a_move_that_happened_ends_with_its_game() {
    let s = Setup::new();
    let _phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    let _tv = s.stream_as(tv);
    s.host().settle_move();
    assert!(!s.host().claim_live(), "the move happened");
    assert!(s.get("/session").await.session().moved_to.is_some());
    *s.host().current_app.lock().unwrap() = None;
    assert!(s.get("/session").await.session().moved_to.is_none());
}

#[tokio::test]
async fn the_window_after_a_failed_press_is_a_minute_whatever_the_display_grace_says() {
    for (revert, timeout) in [
        (None, None),
        (Some("false"), None),
        (Some("false"), Some("0")),
    ] {
        let s = Setup::new();
        let _phone = s.streaming();
        {
            let mut config = s.host().config.write().unwrap();
            if let Some(revert) = revert {
                config
                    .values
                    .insert("dd_config_revert_on_disconnect".into(), revert.into());
            }
            if let Some(timeout) = timeout {
                config.values.insert(
                    "dd_paused_virtual_display_timeout_secs".into(),
                    timeout.into(),
                );
            }
        }
        let (_, tv_connection) = device(&s, "tv", "TV", ALL);
        press(&s, &tv_connection).await.session();
        s.host().moves.expire_claim();
        s.host().settle_move();
        let left = s.host().moves.back_left().expect("the window");
        assert!(
            left > std::time::Duration::from_secs(50) && left <= std::time::Duration::from_secs(60),
            "{left:?} with {revert:?} and {timeout:?}"
        );
    }
}

#[tokio::test]
async fn after_a_failed_press_the_session_reads_as_the_first_devices_own_again() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    // What the phone's open prompt watches: still moved.
    let moved = s.get("/session").await.session();
    assert!(moved.moved_to.is_some() && !moved.owns_game);
    s.host().moves.expire_claim();
    s.host().settle_move();
    let back = s.get("/session").await.session();
    assert!(back.moved_to.is_none());
    assert!(
        back.owns_game,
        "the prompt offers to resume on exactly this"
    );
    assert!(back.playing_on.is_none());
    assert_eq!(
        back.owner,
        butterpollo_core::native_api::session::Owner::You
    );

    // A device without a kept display has no session between streams, and the game is still its.
    let s = Setup::new();
    s.run_game();
    let phone = s.stream_as(s.client.clone());
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().settle_move();
    let back = s.get("/session").await.session();
    assert_eq!(s.state(), SessionState::Idle);
    assert!(back.moved_to.is_none() && back.owns_game);
}

#[tokio::test]
async fn a_note_that_runs_out_or_a_game_that_ended_does_not_read_as_coming_back() {
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    let _tv = s.stream_as(tv);
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    s.host().settle_move();
    s.host().moves.expire_notes();
    let read = s.get("/session").await.session();
    assert!(read.moved_to.is_none() && !read.owns_game);
    assert_eq!(
        read.owner,
        butterpollo_core::native_api::session::Owner::OtherDevice
    );

    // The game ended after the press failed.
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().settle_move();
    s.host()
        .stop_app(crate::state::AppStop::CloseGame(&s.client.uuid));
    s.host().reap_paused_display();
    let read = s.get("/session").await.session();
    assert!(read.moved_to.is_none() && !read.owns_game);
}

#[tokio::test]
async fn a_stream_that_arrives_after_the_press_expired_takes_the_game() {
    let expired = |s: &Setup, phone: Arc<Session>| {
        s.host()
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&phone.launch.id);
        drop(phone);
        s.host().moves.expire_claim();
        s.host().settle_move();
    };
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    assert_eq!(game_owner(&s), s.client.uuid);
    let stream = s.stream_as(tv.clone());
    s.host().settle_move();
    assert_eq!(
        game_owner(&s),
        tv.uuid,
        "its own recovery must find the game"
    );
    drop(stream);

    // The first device came back first: it keeps its game, the late one streams beside it.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    let back = s.stream_as(s.client.clone());
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
    drop((back, stream));

    // The game went on to a third device in the meantime: it is not taken from there.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    s.host().current_app.lock().unwrap().as_mut().unwrap().owner = "laptop".into();
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), "laptop");
    drop(stream);
}

#[tokio::test]
async fn the_displaced_device_cannot_close_the_game_while_the_press_is_live() {
    let s = Setup::new();
    s.host_control(true);
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.post("/session/close-game")
        .await
        .assert_error(409, "session_busy");
    assert!(s.game_running());
    assert!(
        terminate_refused(s.host(), &s.client.uuid),
        "nor by Moonlight's Quit"
    );
    // Once the press is over it is the phone's game again.
    s.host().moves.expire_claim();
    assert_eq!(
        s.post("/session/close-game").await.status,
        StatusCode::OK,
        "the phone owns it again"
    );
    assert!(!s.game_running());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // The press only tries the lock.
async fn a_launch_in_flight_refuses_the_press() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    let held = s.host().launch_transition.lock().unwrap();
    press(&s, &tv_connection)
        .await
        .assert_error(409, "session_busy");
    drop(held);
    assert!(!phone.stopping());
}

#[tokio::test]
async fn a_press_never_followed_by_a_stream_gives_the_game_back() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    // Still in time: the display and the claim stay.
    s.host().reap_paused_display();
    assert!(s.host().claim_live());
    s.host().moves.expire_claim();
    s.host().reap_paused_display();
    assert!(!s.host().claim_live());
    assert!(s.game_running());
    assert_eq!(game_owner(&s), s.client.uuid);
    assert_eq!(s.displays(), 1, "the phone can come back to its display");
    assert_eq!(s.state(), SessionState::Suspended);
}

#[tokio::test]
async fn an_ordinary_webrtc_stream_moves_but_one_guests_watch_is_refused() {
    let s = Setup::new();
    s.run_game();
    let mut launch = s.f.launch(s.client.clone(), Role::Stream);
    launch.transport = Transport::WebRtc;
    let console = running(&s, launch);
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    let seated = s.host().guests.seat_for_test();
    press(&s, &tv_connection)
        .await
        .assert_error(409, "host_in_use");
    assert!(!console.stopping());
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(s.host().moves.note_for(&s.client.uuid).is_none());
    s.host().guests.left(seated);
    press(&s, &tv_connection).await.session();
    assert!(console.stopping());
}
