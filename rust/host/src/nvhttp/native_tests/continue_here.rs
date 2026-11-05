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
    assert!(!s.host().moves.claim_live());
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
    press(&s, &s.connection).await.session();
    assert!(
        s.get("/session").await.session().moved_to.is_none(),
        "the phone asked for the game itself"
    );
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
        assert!(!s.host().moves.claim_live(), "{uuid}");
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
    assert!(!s.host().moves.claim_live());
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
    // The winner repeating itself is fine; the loser still waits.
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
    assert!(s.host().moves.claim_live());
    s.host().moves.expire_claim();
    s.host().reap_paused_display();
    assert!(!s.host().moves.claim_live());
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
