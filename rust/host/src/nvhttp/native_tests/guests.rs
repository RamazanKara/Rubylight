//! `/session/guests`: a device joining another device's game waits for the device playing, which
//! lets it in, changes what it may do and kicks it (`crate::multiclient`).
use super::*;
use crate::multiclient::{Gate, Join};
use butterpollo_core::{
    input::Input,
    multiclient::{KICKED, LAUNCH_OPTION, Permission, REVOKED, Refusal},
    native_api::guests::SessionGuests,
};
use std::time::{Duration, Instant};

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

fn set(s: &Setup, key: &str, value: &str) {
    s.host()
        .config
        .write()
        .unwrap()
        .update(serde_json::json!({ key: value }).as_object().unwrap())
        .unwrap();
}

async fn guests(s: &Setup, connection: &Connection) -> SessionGuests {
    let reply = api(&s.f, connection, "GET", "/session/guests").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    serde_json::from_slice(&reply.body).unwrap()
}

async fn action(s: &Setup, connection: &Connection, path: &str, body: Value) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/session/guests/{path}"),
        Some("application/json"),
        body.to_string(),
    )
    .await
}

/// `client` asks to join the running game, on a thread of its own as `/resume` would.
fn join(s: &Setup, client: &Client) -> tokio::task::JoinHandle<Result<Gate, Refusal>> {
    let host = s.host().clone();
    let client = client.clone();
    tokio::task::spawn_blocking(move || {
        host.device_guests.gate(
            &host,
            &Join {
                client: &client,
                joins_game: true,
                plain: true,
            },
        )
    })
}

async fn until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A guest's stream of the game, as its launch would make it.
fn guest_stream(s: &Setup, client: Client, id: u32) -> Arc<Session> {
    let mut launch = s.f.launch(client, Role::Stream);
    launch.options.insert(LAUNCH_OPTION.into(), id.to_string());
    let session = Session::new(launch, Default::default());
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    s.host().device_guests.joined(id, &session.launch.id);
    session
}

#[tokio::test]
async fn the_feature_follows_the_setting() {
    let s = Setup::new();
    let listed = |caps: Capabilities| caps.features.iter().any(|f| f == "session.guests");
    let caps: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(!listed(caps), "off by default");
    for policy in ["ask", "allow"] {
        set(&s, "guest_join", policy);
        let caps: Capabilities =
            serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
        assert!(listed(caps), "{policy}");
    }
    set(&s, "guest_join", "off");
    let caps: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(!listed(caps));
}

#[tokio::test]
async fn a_join_waits_until_the_device_playing_lets_it_in() {
    let s = Setup::new();
    set(&s, "guest_join", "ask");
    let _owner = s.streaming();
    let (phone, phone_connection) = device(&s, "phone", "Phone", ALL);
    let waiting = join(&s, &phone);
    until("the request", || {
        !s.host().device_guests.view(s.host()).waiting.is_empty()
    })
    .await;
    let seen = guests(&s, &s.connection).await;
    assert!(seen.manage);
    assert_eq!(seen.policy, "ask");
    assert_eq!(seen.owner.as_deref(), Some(s.client.name.as_str()));
    let request = &seen.waiting[0];
    assert_eq!(request.name, "Phone");
    // The phone sees the list but may not answer for the game.
    let theirs = guests(&s, &phone_connection).await;
    assert!(!theirs.manage);
    action(
        &s,
        &phone_connection,
        "approve",
        serde_json::json!({"id": request.id}),
    )
    .await
    .assert_error(409, "not_session_owner");
    let reply = action(
        &s,
        &s.connection,
        "approve",
        serde_json::json!({"id": request.id, "permission": "controller-keyboard-mouse"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let gate = waiting.await.unwrap().unwrap();
    assert_eq!(
        gate,
        Gate::Guest {
            id: request.id,
            permission: Permission::ControllerKeyboardMouse
        }
    );
    let theirs = guests(&s, &phone_connection).await;
    assert_eq!(theirs.you, Some(request.id));
}

#[tokio::test]
async fn a_denied_join_is_refused_and_the_owners_own_join_is_no_guests() {
    let s = Setup::new();
    set(&s, "guest_join", "ask");
    let _owner = s.streaming();
    let (phone, _) = device(&s, "phone", "Phone", ALL);
    let waiting = join(&s, &phone);
    until("the request", || {
        !s.host().device_guests.view(s.host()).waiting.is_empty()
    })
    .await;
    let id = s.host().device_guests.view(s.host()).waiting[0].id;
    let reply = action(&s, &s.connection, "deny", serde_json::json!({"id": id})).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(waiting.await.unwrap(), Err(Refusal::Denied));
    // The device playing resumes its own game as always.
    let own = s.host().device_guests.gate(
        s.host(),
        &Join {
            client: &s.client,
            joins_game: true,
            plain: true,
        },
    );
    assert_eq!(own, Ok(Gate::NotGuest));
    // Off: a join streams beside the game as before guests.
    set(&s, "guest_join", "off");
    let beside = s.host().device_guests.gate(
        s.host(),
        &Join {
            client: &phone,
            joins_game: true,
            plain: true,
        },
    );
    assert_eq!(beside, Ok(Gate::NotGuest));
}

#[tokio::test]
async fn allow_lets_in_at_once_and_guest_input_follows_the_permission() {
    let s = Setup::new();
    let owner = s.streaming();
    set(&s, "guest_join", "allow");
    set(&s, "guest_permission", "controller");
    let (phone, _) = device(&s, "phone", "Phone", ALL);
    let gate = join(&s, &phone).await.unwrap().unwrap();
    let Gate::Guest { id, permission } = gate else {
        panic!("{gate:?}")
    };
    assert_eq!(permission, Permission::Controller);
    let stream = guest_stream(&s, phone, id);
    let guests = &s.host().device_guests;
    let pad = Input::Controller {
        id: 0,
        active: 1,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
    .required_permission();
    let mouse = Input::Relative { x: 1, y: 1 }.required_permission();
    assert!(guests.admits(&stream, pad));
    assert!(!guests.admits(&stream, mouse));
    assert!(!guests.full_access(&stream));
    // With the keyboard and mouse, the owner takes them back by using them.
    guests
        .set_permission(id, Permission::ControllerKeyboardMouse)
        .unwrap();
    assert!(guests.full_access(&stream));
    assert!(guests.admits(&stream, mouse), "nobody used them yet");
    assert!(guests.admits(&owner, mouse), "the owner takes them back");
    assert!(
        guests.take_release(&stream),
        "the guest lets go of its keys"
    );
    assert!(!guests.take_release(&stream));
    assert!(!guests.admits(&stream, mouse));
    assert!(guests.admits(&owner, mouse));
    // The owner hands them over.
    guests.give_keyboard_mouse(Some(id)).unwrap();
    assert!(guests.admits(&stream, mouse));
}

#[tokio::test]
async fn kick_and_revoke_end_the_guests_stream_with_a_reason() {
    let s = Setup::new();
    let _owner = s.streaming();
    set(&s, "guest_join", "allow");
    let (phone, _) = device(&s, "phone", "Phone", ALL);
    let Gate::Guest { id, .. } = join(&s, &phone).await.unwrap().unwrap() else {
        panic!("not a guest")
    };
    let stream = guest_stream(&s, phone.clone(), id);
    let reply = action(&s, &s.connection, "kick", serde_json::json!({"id": id})).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert!(stream.stopping());
    assert_eq!(stream.failure().as_deref(), Some(KICKED));
    action(&s, &s.connection, "kick", serde_json::json!({"id": id}))
        .await
        .assert_error(404, "not_found");
    // Kicked, it joins again; revoked, it may not for the rest of the game.
    let Gate::Guest { id, .. } = join(&s, &phone).await.unwrap().unwrap() else {
        panic!("not a guest again")
    };
    let stream = guest_stream(&s, phone.clone(), id);
    let reply = action(&s, &s.connection, "revoke", serde_json::json!({"id": id})).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(stream.failure().as_deref(), Some(REVOKED));
    assert_eq!(join(&s, &phone).await.unwrap(), Err(Refusal::Revoked));
    action(
        &s,
        &s.connection,
        "permission",
        serde_json::json!({"id": id, "permission": "admin"}),
    )
    .await
    .assert_error(422, "invalid_value");
}

#[tokio::test]
async fn the_routes_need_a_paired_device() {
    let f = Fixture::new();
    for (method, path) in [
        ("GET", "/session/guests"),
        ("POST", "/session/guests/approve"),
        ("POST", "/session/guests/deny"),
        ("POST", "/session/guests/kick"),
        ("POST", "/session/guests/revoke"),
        ("POST", "/session/guests/permission"),
        ("POST", "/session/guests/keyboard-mouse"),
    ] {
        api(&f, &f.connection(false), method, path)
            .await
            .assert_error(403, "not_paired");
    }
}
