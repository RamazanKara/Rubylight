//! `GET /coop`, `POST /coop/join`, the `coop` feature and the co-op library
//! entries through the router, with the fake Nucleus Co-op PC.
use super::*;
use crate::nucleus::{Admission, fake};
use butterpollo_core::{native_api::coop::Coop, state::App};

async fn join(s: &Setup, connection: &Connection, body: &str) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/coop/join"),
        Some("application/json"),
        body.as_bytes().to_vec(),
    )
    .await
}

/// A host with Nucleus Co-op (the fake) and Portal 2, which it has a
/// handler for. Returns the co-op entry's id.
fn with_nucleus(s: &Setup) -> (Arc<fake::Pc>, u32) {
    let pc = Arc::new(fake::Pc::installed());
    s.host().nucleus.use_pc(pc.clone());
    s.host().apps.write().unwrap().push(App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    });
    let apps = s.host().apps.read().unwrap().clone();
    let id = s.host().nucleus.entries(&apps, "")[0].id();
    (pc, id)
}

/// Another paired device allowed to join co-op sessions.
fn device(f: &Fixture, uuid: &str) -> Connection {
    let identity = Identity::generate().unwrap();
    let mut client = Client {
        name: uuid.into(),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm: ALL,
        enabled: true,
        extra: Default::default(),
    };
    client.extra.insert("coop_join".into(), true.into());
    f.host
        .paired
        .write()
        .unwrap()
        .add(&f.host.paired_path, client)
        .unwrap();
    Connection {
        certificate: Some(identity.der.clone()),
        ..f.connection(true)
    }
}

/// Device `index` launches the co-op entry: it starts the session or joins it.
fn launch(s: &Setup, index: usize, launch: &str) -> butterpollo_core::session::Seat {
    let apps = s.host().apps.read().unwrap().clone();
    let coop = s.host().nucleus.entries(&apps, "").remove(0);
    let client = s.host().paired.read().unwrap().clients[index].clone();
    s.host()
        .nucleus
        .admit(Admission {
            launch,
            client: &client,
            coop: &coop,
            size: (1920, 1080),
            config: &Default::default(),
            warnings: Default::default(),
        })
        .unwrap()
}

fn allow_coop_join(s: &Setup, index: usize) {
    s.host().paired.write().unwrap().clients[index]
        .extra
        .insert("coop_join".into(), true.into());
}

fn coop(reply: &Reply) -> Coop {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    serde_json::from_slice(&reply.body).unwrap()
}

#[tokio::test]
async fn coop_answers_503_and_is_no_feature_while_nucleus_cannot_run() {
    let s = Setup::new();
    s.get("/coop")
        .await
        .assert_error(503, "temporarily_unavailable");
    join(&s, &s.connection, r#"{"session":1}"#)
        .await
        .assert_error(503, "temporarily_unavailable");
    let capabilities: Capabilities =
        serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(!capabilities.features.iter().any(|f| f == "coop"));
    assert!(!capabilities.permissions.coop_join);
}

#[tokio::test]
async fn capabilities_list_coop_while_nucleus_can_run_and_show_join_co_op() {
    let s = Setup::new();
    with_nucleus(&s);
    let capabilities: Capabilities =
        serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(capabilities.features.iter().any(|f| f == "coop"));
    assert!(!capabilities.permissions.coop_join);
    allow_coop_join(&s, 0);
    let granted: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(granted.permissions.coop_join);
}

#[tokio::test]
async fn the_library_lists_the_co_op_entries_after_the_apps() {
    let s = Setup::new();
    let (_, id) = with_nucleus(&s);
    let reply = s.get("/library").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let library: Library = serde_json::from_slice(&reply.body).unwrap();
    let last = library.games.last().unwrap();
    assert_eq!(
        (last.app_id, last.title.as_str()),
        (id, "Co-op (Nucleus): Portal 2")
    );
    assert_eq!(library.games[library.games.len() - 2].title, "Portal 2");
}

#[tokio::test]
async fn coop_lists_the_games_and_the_sessions_as_the_device_sees_them() {
    let s = Setup::new();
    let (_, id) = with_nucleus(&s);
    let empty = coop(&s.get("/coop").await);
    assert!(empty.available);
    assert_eq!(empty.games.len(), 1);
    assert_eq!(
        (empty.games[0].app_id, empty.games[0].title.as_str()),
        (id, "Co-op (Nucleus): Portal 2")
    );
    assert_eq!(
        (empty.games[0].game.as_str(), empty.games[0].max_players),
        (fake::GUID, 2)
    );
    assert!(empty.sessions.is_empty());
    // Another device starts the game.
    other_device(&s.f, ALL, false);
    let seat = launch(&s, 1, "other-launch");
    let listed = coop(&s.get("/coop").await);
    let session = &listed.sessions[0];
    assert_eq!(session.id, seat.session);
    assert_eq!((session.app_id, session.title.as_str()), (id, "Portal 2"));
    assert_eq!(
        session.phase,
        butterpollo_core::native_api::coop::Phase::Starting
    );
    assert_eq!(session.players.len(), 2);
    assert_eq!(session.players[0].name.as_deref(), Some("Other device"));
    assert!(!session.players[0].you);
    assert_eq!(session.players[1].name, None);
    assert_eq!(session.free_slots, 1);
    assert!(!session.joinable);
    assert!(
        session.refusal.as_deref().unwrap().contains("Join co-op"),
        "{session:?}"
    );
    allow_coop_join(&s, 0);
    let allowed = coop(&s.get("/coop").await);
    assert!(allowed.sessions[0].joinable);
    assert_eq!(allowed.sessions[0].refusal, None);
    // Ended from the console: still listed, with why.
    assert!(s.host().nucleus.end(seat.session));
    let ended = coop(&s.get("/coop").await);
    assert_eq!(
        ended.sessions[0].phase,
        butterpollo_core::native_api::coop::Phase::Ended
    );
    assert_eq!(
        ended.sessions[0].end.as_ref().unwrap().reason,
        butterpollo_core::native_api::coop::EndReason::Ended
    );
    assert!(!ended.sessions[0].joinable);
}

#[tokio::test]
async fn coop_join_seats_the_device_and_answers_each_refusal() {
    let s = Setup::new();
    let (_, id) = with_nucleus(&s);
    join(&s, &s.connection, r#"{"session":"one"}"#)
        .await
        .assert_error(400, "invalid_request");
    join(&s, &s.connection, r#"{"session":1,"slot":0}"#)
        .await
        .assert_error(400, "invalid_request");
    join(&s, &s.connection, r#"{"session":1}"#)
        .await
        .assert_error(409, "no_session");
    let other = other_device(&s.f, ALL, false);
    let seat = launch(&s, 1, "other-launch");
    let body = format!(r#"{{"session":{}}}"#, seat.session);
    let refused = join(&s, &s.connection, &body).await;
    refused.assert_error(403, "permission_required");
    assert_eq!(
        refused.json()["error"]["message"],
        "Turn on Join co-op for this device in the Rubylight console."
    );
    allow_coop_join(&s, 0);
    let joined = join(&s, &s.connection, &body).await;
    assert_eq!(joined.status, StatusCode::OK, "{}", joined.text());
    assert_eq!(
        joined.json(),
        serde_json::json!({"session": seat.session, "slot": 1, "app_id": id})
    );
    // Held for the launch: listed as the device's own, away until it streams.
    let listed = coop(&s.get("/coop").await);
    let mine = &listed.sessions[0].players[1];
    assert!(mine.you && mine.away, "{mine:?}");
    assert!(listed.sessions[0].joinable);
    // Joining again returns the device to its own slot.
    let again = join(&s, &s.connection, &body).await;
    assert_eq!(again.json()["slot"], 1);
    // Full for a third device.
    let third = device(&s.f, "third");
    let full = join(&s, &third, &body).await;
    full.assert_error(409, "coop_refused");
    let message = full.json()["error"]["message"].clone();
    assert!(message.as_str().unwrap().contains("is taken"), "{message}");
    drop(other);
}

#[tokio::test]
async fn coop_needs_list_applications_and_joining_launch_applications() {
    let s = Setup::with(VIEW_BIT | LAUNCH);
    with_nucleus(&s);
    s.get("/coop")
        .await
        .assert_error(403, "permission_required");
    let s = Setup::with(LIST);
    with_nucleus(&s);
    coop(&s.get("/coop").await);
    join(&s, &s.connection, r#"{"session":1}"#)
        .await
        .assert_error(403, "permission_required");
}
