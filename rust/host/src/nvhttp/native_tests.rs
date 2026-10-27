//! The native API v1 (`docs/native-api.md`) through the router: one test per
//! endpoint and per error code, and snapshots that pin the Moonlight
//! endpoints byte for byte.
use super::*;
use crate::{
    display_session::tests::retained_display, library, native_api::Failure, state::Session,
    state::test_support::Fixture,
};
use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request},
};
use butterpollo_core::{
    crypto::Identity,
    native::Scan,
    native_api::{
        capabilities::Capabilities,
        error::ErrorCode,
        library::Library,
        session::{Owner, Session as ApiSession, State as SessionState},
    },
};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

mod compat;
mod couch_menu;
mod files;
mod launcher;
mod radeon;
mod usb;
mod wake;

const LIST: u32 = 1 << 24;
const VIEW_BIT: u32 = 1 << 25;
const LAUNCH: u32 = 1 << 26;
const ALL: u32 = LIST | VIEW_BIT | LAUNCH;
const PREFIX: &str = "/api/native/v1";

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
    fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }
    fn session(&self) -> ApiSession {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
        serde_json::from_slice(&self.body).unwrap()
    }
    /// The answer is the documented error: status, shape and headers.
    fn assert_error(&self, status: u16, code: &str) {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        let body = self.json();
        assert_eq!(body["error"]["code"], code, "{body}");
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
        for field in ["setting", "retry_after_ms"] {
            assert!(
                body["error"].get(field).is_some(),
                "{field} is null, not missing"
            );
        }
        assert_eq!(
            self.headers[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert_eq!(self.headers[header::CACHE_CONTROL], "no-store");
    }
}
async fn send(
    f: &Fixture,
    https: bool,
    connection: &Connection,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: impl Into<Vec<u8>>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    let response = router(f.host.clone(), https)
        .layer(Extension(connection.clone()))
        .oneshot(request.body(Body::from(body.into())).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, 1 << 20).await.unwrap().to_vec(),
    }
}
/// A request to `PREFIX` + `path` with no body.
async fn api(f: &Fixture, connection: &Connection, method: &str, path: &str) -> Reply {
    send(
        f,
        true,
        connection,
        method,
        &format!("{PREFIX}{path}"),
        None,
        vec![],
    )
    .await
}

/// Another paired device, with its own certificate.
fn other_device(f: &Fixture, perm: u32, host_control: bool) -> Connection {
    let identity = Identity::generate().unwrap();
    let mut client = Client {
        name: "Other device".into(),
        cert: identity.certificate.clone(),
        uuid: "other-device".into(),
        perm,
        enabled: true,
        extra: Default::default(),
    };
    if host_control {
        client.extra.insert("host_control".into(), true.into());
    }
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

/// A host with one paired device, which owns whatever the test starts.
struct Setup {
    f: Fixture,
    client: Client,
    connection: Connection,
}
impl Setup {
    fn new() -> Self {
        Self::with(ALL)
    }
    fn with(perm: u32) -> Self {
        let f = Fixture::new();
        let client = f.client(perm);
        let connection = f.connection(true);
        Self {
            f,
            client,
            connection,
        }
    }
    fn host(&self) -> &Shared {
        &self.f.host
    }
    /// The console grants or revokes Host control for the device.
    fn host_control(&self, granted: bool) {
        let mut paired = self.host().paired.write().unwrap();
        let extra = &mut paired.clients[0].extra;
        if granted {
            extra.insert("host_control".into(), true.into());
        } else {
            extra.remove("host_control");
        }
    }
    fn run_game(&self) {
        let app = self.host().apps.read().unwrap()[0].clone();
        let mut running =
            crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
        running.owner = self.client.uuid.clone();
        *self.host().current_app.lock().unwrap() = Some(running);
    }
    fn retain_display(&self) {
        let config = self.host().config.read().unwrap().clone();
        let display = retained_display(self.host(), &config);
        self.host()
            .app_display
            .lock()
            .unwrap()
            .insert(self.client.uuid.clone(), (display, None));
    }
    /// A stream whose launch copy of the device is `client`.
    fn stream_as(&self, client: Client) -> Arc<Session> {
        self.f.session(client, Role::Stream)
    }
    /// A game, its display and a stream: `streaming`.
    fn streaming(&self) -> Arc<Session> {
        self.run_game();
        self.retain_display();
        let session = self.stream_as(self.client.clone());
        assert_eq!(self.state(), SessionState::Streaming);
        session
    }
    fn state(&self) -> SessionState {
        self.host().session_snapshot().state
    }
    fn game_running(&self) -> bool {
        self.host().current_app.lock().unwrap().is_some()
    }
    fn displays(&self) -> usize {
        self.host().app_display.lock().unwrap().len()
    }
    async fn get(&self, path: &str) -> Reply {
        api(&self.f, &self.connection, "GET", path).await
    }
    async fn post(&self, path: &str) -> Reply {
        api(&self.f, &self.connection, "POST", path).await
    }
}

#[tokio::test]
async fn an_unpaired_request_gets_403_not_paired_on_every_endpoint() {
    let f = Fixture::new();
    let endpoints = [
        ("GET", "/capabilities"),
        ("GET", "/library"),
        ("GET", "/session"),
        ("POST", "/session/disconnect"),
        ("POST", "/session/end"),
        ("POST", "/session/close-game"),
        ("POST", "/session/couch-menu"),
        ("GET", "/radeon"),
        ("POST", "/radeon/settings"),
        ("POST", "/radeon/restore"),
        ("GET", "/wake"),
    ];
    let certificate_less = Connection {
        certificate: None,
        ..f.connection(true)
    };
    let not_tls = f.connection(false);
    for (method, path) in endpoints {
        // The host's own certificate, but nobody is paired with it.
        let unknown = f.connection(true);
        api(&f, &unknown, method, path)
            .await
            .assert_error(403, "not_paired");
        api(&f, &certificate_less, method, path)
            .await
            .assert_error(403, "not_paired");
        api(&f, &not_tls, method, path)
            .await
            .assert_error(403, "not_paired");
        // Garbage in the body does not change the answer.
        send(
            &f,
            true,
            &unknown,
            method,
            &format!("{PREFIX}{path}"),
            Some("text/xml"),
            "{",
        )
        .await
        .assert_error(403, "not_paired");
        // Plain HTTP has no native API at all.
        assert_eq!(
            send(
                &f,
                false,
                &unknown,
                method,
                &format!("{PREFIX}{path}"),
                None,
                vec![]
            )
            .await
            .status,
            StatusCode::NOT_FOUND
        );
    }
    // A device turned off, or one without any permission, is not paired either.
    let client = f.client(ALL);
    f.host.paired.write().unwrap().clients[0].enabled = false;
    api(&f, &f.connection(true), "GET", "/session")
        .await
        .assert_error(403, "not_paired");
    f.host.paired.write().unwrap().clients[0].enabled = true;
    f.host.paired.write().unwrap().clients[0].perm = 0;
    api(&f, &f.connection(true), "GET", "/session")
        .await
        .assert_error(403, "not_paired");
    f.host.paired.write().unwrap().clients[0].perm = client.perm;
    assert_eq!(
        api(&f, &f.connection(true), "GET", "/session").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn unknown_paths_and_methods_answer_404_not_found_in_the_error_shape() {
    let s = Setup::new();
    for (method, path) in [
        ("GET", "/nope"),
        ("GET", ""),
        ("GET", "/"),
        ("GET", "/session/nope"),
        ("GET", "/radeon/nope"),
        // Wrong methods.
        ("POST", "/radeon"),
        ("GET", "/radeon/settings"),
        ("GET", "/radeon/restore"),
        ("POST", "/capabilities"),
        ("POST", "/library"),
        ("DELETE", "/session"),
        ("GET", "/session/end"),
        ("GET", "/session/close-game"),
        ("GET", "/session/couch-menu"),
        ("POST", "/usb/tunnel"),
        // Defined for 3.0, not yet served.
        ("GET", "/folders"),
        ("GET", "/folders/games/entries?path="),
        ("GET", "/folders/games/content?path=a"),
        ("PUT", "/folders/games/content?path=a"),
        ("PROPFIND", "/folders/games/dav/"),
    ] {
        api(&s.f, &s.connection, method, path)
            .await
            .assert_error(404, "not_found");
    }
    // An unpaired caller learns nothing more from an unknown path.
    api(
        &s.f,
        &Connection {
            certificate: None,
            ..s.connection.clone()
        },
        "GET",
        "/radeon/nope",
    )
    .await
    .assert_error(404, "not_found");
}

#[tokio::test]
async fn capabilities_list_the_features_and_the_callers_permissions() {
    let s = Setup::with(LIST | VIEW_BIT);
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("sunshine_name".into(), "Couch PC".into());
    let reply = s.get("/capabilities").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    let capabilities: Capabilities = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(capabilities.api_version, 1);
    assert_eq!(capabilities.host.name, "Couch PC");
    assert_eq!(capabilities.host.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        capabilities.host.unique_id,
        s.host().paired.read().unwrap().unique_id
    );
    // Radeon features appear when the host started its Radeon worker (see native_tests/radeon.rs).
    // Audio processing is not served yet, so it is not listed.
    assert_eq!(
        capabilities.features,
        [
            "library",
            "session",
            "session.close_game",
            "session.launcher",
            "wake",
            "session.couch_menu"
        ]
    );
    let permissions = capabilities.permissions;
    assert_eq!(
        (permissions.list, permissions.view, permissions.launch),
        (true, true, false)
    );
    assert!(!permissions.host_control);
    // The console grants Host control: the very next request shows it.
    s.host_control(true);
    let granted: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(granted.permissions.host_control);
    s.host_control(false);
    let revoked: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(!revoked.permissions.host_control);
}

#[tokio::test]
async fn usb_and_shared_folders_show_what_the_device_holds_and_list_no_feature() {
    let s = Setup::new();
    let capabilities = || async {
        serde_json::from_slice::<Capabilities>(&s.get("/capabilities").await.body).unwrap()
    };
    let none = capabilities().await;
    assert_eq!(
        (none.permissions.usb, none.permissions.shared_folders),
        (false, false)
    );
    {
        let mut paired = s.host().paired.write().unwrap();
        let extra = &mut paired.clients[0].extra;
        extra.insert("allow_usb".into(), true.into());
        extra.insert(
            "shared_folders".into(),
            serde_json::json!({"games": "read"}),
        );
    }
    let granted = capabilities().await;
    assert_eq!(
        (granted.permissions.usb, granted.permissions.shared_folders),
        (true, true)
    );
    // USB forwarding is off by default and shared folders are not served yet, so the features
    // stay what a 2.x client reads.
    assert_eq!(granted.features, none.features);
}

#[tokio::test]
async fn a_missing_permission_is_403_and_names_what_to_turn_on() {
    // Library needs List apps.
    let s = Setup::with(VIEW_BIT | LAUNCH);
    let reply = s.get("/library").await;
    reply.assert_error(403, "permission_required");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("List applications")
    );
    // The session is readable by any paired device.
    assert_eq!(s.get("/session").await.status, StatusCode::OK);
    // End needs Launch.
    let s = Setup::with(LIST | VIEW_BIT);
    let reply = s.post("/session/end").await;
    reply.assert_error(403, "permission_required");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Launch")
    );
    // Close game needs Launch before it looks at Host control.
    s.host_control(true);
    s.post("/session/close-game")
        .await
        .assert_error(403, "permission_required");
    // Disconnect needs View or Launch.
    let s = Setup::with(LIST);
    s.post("/session/disconnect")
        .await
        .assert_error(403, "permission_required");
    for perm in [VIEW_BIT, LAUNCH] {
        let s = Setup::with(perm);
        s.post("/session/disconnect")
            .await
            .assert_error(409, "no_session");
    }
}

#[tokio::test]
async fn library_lists_the_added_games_with_ids_that_survive_a_rename() {
    let s = Setup::new();
    let assets = s.host().assets.parent().unwrap().to_owned();
    std::fs::create_dir_all(&assets).unwrap();
    let cover = assets.join("dota.png");
    std::fs::write(&cover, b"\x89PNG\r\n\x1a\nfirst").unwrap();
    let apps = |title: &str| -> Vec<App> {
        [
            json!({"name": "Desktop", "uuid": "11111111-1111-4111-8111-111111111111"}),
            json!({"name": title, "uuid": "22222222-2222-4222-8222-222222222222",
                   "steam-id": "570", "image-path": cover.to_string_lossy()}),
            json!({"name": "Mirage", "uuid": "33333333-3333-4333-8333-333333333333",
                   "launcher-source": "ubisoft", "launcher-id": "13504"}),
            // The synthetic tile of the Remote session, not a game.
            json!({"name": "Remote Monitor", "uuid": "9a1c5a25-58fe-40e0-b9aa-7d3f00000005"}),
        ]
        .into_iter()
        .map(|app| serde_json::from_value(app).unwrap())
        .collect()
    };
    *s.host().apps.write().unwrap() = apps("Dota 2");
    let first = s.get("/library").await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.headers[header::CACHE_CONTROL], "no-store");
    let first: Library = serde_json::from_slice(&first.body).unwrap();
    let titles: Vec<_> = first.games.iter().map(|game| game.title.as_str()).collect();
    assert_eq!(titles, ["Desktop", "Dota 2", "Mirage"]);
    let dota = &first.games[1];
    assert_eq!(dota.install_id, "22222222-2222-4222-8222-222222222222");
    assert_eq!(dota.game_id.as_deref(), Some("steam:570"));
    assert_eq!(dota.store.as_ref().unwrap().source, "steam");
    assert_eq!(first.games[2].game_id.as_deref(), Some("ubisoft:13504"));
    assert_eq!(first.games[0].game_id, None);
    assert!(first.games.iter().all(|game| !game.running));
    let artwork = dota.artwork.cover.clone().unwrap();
    assert_eq!(
        artwork,
        format!("/appasset?appid={}&AssetType=2&AssetIdx=0", dota.app_id)
    );

    // The first request started the scan and did not wait for it. Once it is
    // in, a game of a store is installed or not, and a manual app is null.
    let scanned = loop {
        let library: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
        if library.games[1].installed.is_some() {
            break library;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(scanned.games[0].installed, None);
    assert_eq!(scanned.games[1].installed, Some(false));
    library::seed(s.host(), Scan::new(["steam:570".to_owned()]));
    let installed: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_eq!(
        (installed.games[1].installed, installed.games[2].installed),
        (Some(true), Some(false))
    );
    // Installing changes the revision, which hashes the whole body.
    assert_ne!(installed.revision, first.revision);

    // A game that runs sets `running` and leaves the revision alone.
    let app = s.host().apps.read().unwrap()[1].clone();
    let mut running =
        crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
    running.owner = s.client.uuid.clone();
    *s.host().current_app.lock().unwrap() = Some(running);
    let playing: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_eq!(playing.revision, installed.revision);
    assert_eq!(
        playing
            .games
            .iter()
            .map(|game| game.running)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    s.host().stop_app(crate::state::AppStop::Test);

    // A rename keeps the ids and changes the revision. The app id is the CRC
    // of the name, so it moves, and `install_id` and `game_id` are the keys.
    *s.host().apps.write().unwrap() = apps("Dota 2 (renamed)");
    let renamed: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    let after = &renamed.games[1];
    assert_eq!(after.title, "Dota 2 (renamed)");
    assert_eq!(after.install_id, dota.install_id);
    assert_eq!(after.game_id, dota.game_id);
    assert_ne!(after.app_id, dota.app_id);
    assert_ne!(renamed.revision, installed.revision);

    // The cover version follows the image file.
    let version = after.artwork.version.clone();
    assert_eq!(version.len(), 8);
    let again: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_eq!(again.games[1].artwork.version, version);
    std::fs::write(&cover, b"\x89PNG\r\n\x1a\nsecond, longer").unwrap();
    let changed: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_ne!(changed.games[1].artwork.version, version);
    assert_ne!(changed.revision, again.revision);
}

#[tokio::test]
async fn session_shows_idle_to_everyone_and_the_owner_alone_the_details() {
    let s = Setup::new();
    let idle = s.get("/session").await.session();
    assert_eq!((idle.state, idle.owner), (SessionState::Idle, Owner::None));
    assert!(idle.app.is_none() && !idle.game_running);
    assert_eq!(idle.actions.len(), 4);

    let _stream = s.streaming();
    let mine = s.get("/session").await.session();
    assert_eq!(
        (mine.state, mine.owner),
        (SessionState::Streaming, Owner::You)
    );
    assert!(mine.game_running && mine.since_unix_ms.is_some());
    assert_eq!(
        mine.app.unwrap().title,
        s.host().apps.read().unwrap()[0].name
    );

    // Another device sees the state and the owner, never the app.
    let other = other_device(&s.f, ALL, true);
    let theirs = api(&s.f, &other, "GET", "/session").await.session();
    assert_eq!(
        (theirs.state, theirs.owner),
        (SessionState::Streaming, Owner::OtherDevice)
    );
    assert!(theirs.app.is_none() && theirs.since_unix_ms.is_none());
    assert!(theirs.actions.iter().all(|action| !action.allowed));
}

#[tokio::test]
async fn disconnect_suspends_the_session_and_never_stops_the_game() {
    let s = Setup::new();
    let stream = s.streaming();
    // The three body forms of an empty POST.
    for (content_type, body) in [
        (None, ""),
        (Some("application/json"), "{}"),
        (Some("text/plain"), " {} "),
    ] {
        let reply = send(
            &s.f,
            true,
            &s.connection,
            "POST",
            &format!("{PREFIX}/session/disconnect"),
            content_type,
            body,
        )
        .await;
        let session = reply.session();
        assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            (session.state, session.owner),
            (SessionState::Suspended, Owner::You)
        );
        assert!(session.game_running && session.suspended_until_unix_ms.is_some());
        assert!(stream.stopping());
    }
    // The game and its display stay for the grace period.
    assert!(s.game_running());
    assert_eq!(s.displays(), 1);
    // Repeating is safe.
    let repeat = s.post("/session/disconnect").await.session();
    assert_eq!(repeat.state, SessionState::Suspended);
    assert!(s.game_running());
}

#[tokio::test]
async fn end_keeps_the_game_running_and_repeats_safely() {
    let s = Setup::new();
    let stream = s.streaming();
    let ended = s.post("/session/end").await.session();
    assert_eq!(
        (ended.state, ended.owner),
        (SessionState::Idle, Owner::None)
    );
    assert!(ended.game_running, "End session leaves the game running");
    assert!(stream.stopping());
    assert!(s.game_running());
    assert_eq!(s.displays(), 0);
    // Nothing is left to end: a refusal, and nothing changes.
    s.post("/session/end").await.assert_error(409, "no_session");
    s.post("/session/disconnect")
        .await
        .assert_error(409, "no_session");
    assert!(s.game_running());
    // While a teardown runs, repeating End is safe and answers `stopping`.
    let _stream = s.streaming();
    let teardown = s
        .host()
        .session_teardown(crate::session_control::Scope::Session);
    let repeat = s.post("/session/end").await.session();
    assert_eq!(repeat.state, SessionState::Stopping);
    drop(teardown);
}

#[tokio::test]
async fn close_game_needs_host_control_and_ends_the_game() {
    let s = Setup::new();
    let _stream = s.streaming();
    // Without Host control, nothing happens.
    s.post("/session/close-game")
        .await
        .assert_error(403, "host_control_required");
    assert!(s.game_running());
    assert_eq!(s.state(), SessionState::Streaming);
    s.host_control(true);
    let closed = s.post("/session/close-game").await.session();
    assert_eq!(closed.state, SessionState::Idle);
    assert!(!closed.game_running);
    assert!(!s.game_running());
    assert_eq!(s.displays(), 0);
    // A repeat has no game and no session.
    s.post("/session/close-game")
        .await
        .assert_error(409, "no_session");
}

#[tokio::test]
async fn close_game_also_closes_a_game_that_end_left_running() {
    let s = Setup::new();
    s.host_control(true);
    let _stream = s.streaming();
    assert_eq!(
        s.post("/session/end").await.session().state,
        SessionState::Idle
    );
    assert!(s.game_running());
    // Another device with Host control may close it: nobody owns a session.
    let other = other_device(&s.f, ALL, true);
    let closed = api(&s.f, &other, "POST", "/session/close-game")
        .await
        .session();
    assert_eq!(closed.state, SessionState::Idle);
    assert!(!closed.game_running && !s.game_running());
}

#[tokio::test]
async fn host_control_is_read_from_the_paired_state_on_every_request() {
    let s = Setup::new();
    // The launch holds a copy of the device from the time it started. The
    // console's grant and revoke apply to the next request, not to that copy.
    let mut granted_at_launch = s.client.clone();
    granted_at_launch
        .extra
        .insert("host_control".into(), true.into());
    s.run_game();
    s.retain_display();
    let _stream = s.stream_as(granted_at_launch);
    s.post("/session/close-game")
        .await
        .assert_error(403, "host_control_required");
    assert!(s.game_running(), "the stale copy did not close the game");
    // A grant applies to the very next request, and so does its revoke.
    s.host_control(true);
    s.host_control(false);
    s.post("/session/close-game")
        .await
        .assert_error(403, "host_control_required");
    s.host_control(true);
    assert_eq!(
        s.post("/session/close-game").await.session().state,
        SessionState::Idle
    );
}

#[tokio::test]
async fn an_action_from_another_device_is_409_not_session_owner_and_changes_nothing() {
    let s = Setup::new();
    let _stream = s.streaming();
    let other = other_device(&s.f, ALL, true);
    for path in ["/session/disconnect", "/session/end", "/session/close-game"] {
        api(&s.f, &other, "POST", path)
            .await
            .assert_error(409, "not_session_owner");
    }
    assert_eq!(s.state(), SessionState::Streaming);
    assert!(s.game_running());
    assert_eq!(s.displays(), 1);
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "the launch lock is held on purpose while the handler tries it from the blocking pool"
)]
async fn an_action_during_a_launch_is_409_session_busy_with_a_retry_hint() {
    let s = Setup::new();
    let _stream = s.streaming();
    let launch = s.host().launch_transition.lock().unwrap();
    for path in ["/session/disconnect", "/session/end", "/session/close-game"] {
        s.host_control(true);
        let reply = s.post(path).await;
        reply.assert_error(409, "session_busy");
        assert_eq!(reply.json()["error"]["retry_after_ms"], 1000);
    }
    drop(launch);
    assert_eq!(s.state(), SessionState::Streaming);
    assert!(s.game_running());
}

#[tokio::test]
async fn an_action_with_no_session_is_409_no_session() {
    let s = Setup::new();
    s.host_control(true);
    for path in ["/session/disconnect", "/session/end", "/session/close-game"] {
        s.post(path).await.assert_error(409, "no_session");
    }
}

#[tokio::test]
async fn a_bad_body_is_400_invalid_request() {
    let s = Setup::new();
    let url = format!("{PREFIX}/session/end");
    let post = |content_type: Option<&'static str>, body: Vec<u8>| {
        send(&s.f, true, &s.connection, "POST", &url, content_type, body)
    };
    post(Some("application/json"), b"{".to_vec())
        .await
        .assert_error(400, "invalid_request");
    post(None, b"[]".to_vec())
        .await
        .assert_error(400, "invalid_request");
    post(Some("text/xml"), b"{}".to_vec())
        .await
        .assert_error(400, "invalid_request");
    let huge = format!("{{\"padding\":\"{}\"}}", "x".repeat(16 * 1024));
    post(Some("application/json"), huge.into_bytes())
        .await
        .assert_error(400, "invalid_request");
    // Valid bodies reach the action, which refuses on the state alone.
    post(
        Some("application/json; charset=utf-8"),
        b"{\"a\":1}".to_vec(),
    )
    .await
    .assert_error(409, "no_session");
    post(Some("text/plain"), b"".to_vec())
        .await
        .assert_error(409, "no_session");
}

#[tokio::test]
async fn every_error_code_renders_with_its_status_and_the_one_body_shape() {
    for code in [
        ErrorCode::NotPaired,
        ErrorCode::PermissionRequired,
        ErrorCode::HostControlRequired,
        ErrorCode::NotFound,
        ErrorCode::InvalidRequest,
        ErrorCode::InvalidValue,
        ErrorCode::NotWritable,
        ErrorCode::NoSession,
        ErrorCode::NotSessionOwner,
        ErrorCode::SessionBusy,
        ErrorCode::DriverRejected,
        ErrorCode::TemporarilyUnavailable,
        ErrorCode::Forbidden,
        ErrorCode::FileTransferDisabled,
        ErrorCode::InvalidName,
        ErrorCode::IncompleteUpload,
        ErrorCode::LengthRequired,
        ErrorCode::TooLarge,
        ErrorCode::InsufficientStorage,
        ErrorCode::NoConsoleUser,
        ErrorCode::TransferBusy,
        ErrorCode::Internal,
    ] {
        let response = Failure::new(code, "Something to read.").into_response();
        let (parts, body) = response.into_parts();
        let reply = Reply {
            status: parts.status,
            headers: parts.headers,
            body: to_bytes(body, 4096).await.unwrap().to_vec(),
        };
        let name = serde_json::to_value(code).unwrap();
        reply.assert_error(code.status(), name.as_str().unwrap());
    }
    let internal = Failure::internal("do a thing", "disk on fire").into_response();
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

const SERVERINFO_UNPAIRED: &str = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><NetworkPathVersion>0</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion><hostname>Snapshot</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion><uniqueid>{UNIQUE}</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>0</PairStatus><currentgame>0</currentgame><currentgameuuid></currentgameuuid><state>SUNSHINE_SERVER_FREE</state><LocalIP>127.0.0.1</LocalIP><mac>01:02:03:04:05:06</mac><MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>769</ServerCodecModeSupport><RustHostVersion>{VERSION}</RustHostVersion><RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps><RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive><RustHostProfile></RustHostProfile><PyroWaveHostLinkMbps>1000</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>0</PyroWaveBandwidthProbeBytes><PyroWaveUdpProbeVersion>0</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>0</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>0</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>0</PyroWaveWireBudgetVersion><PyroWaveCriticalFecPercentage></PyroWaveCriticalFecPercentage><PyroWaveMinParityShards></PyroWaveMinParityShards><Permission>0</Permission><FrameLimiterSupported>1</FrameLimiterSupported><FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>1</VirtualDisplayFrameLimiterEnabled><FrameLimiterFpsLimitMilliHz>0</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation><VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><HdrFormats>15</HdrFormats><FecControlVersion>1</FecControlVersion><BandwidthProbeVersion>1</BandwidthProbeVersion><AudioCodecs>3</AudioCodecs><AudioLayouts>15</AudioLayouts><TextInputVersion>1</TextInputVersion><KeyboardLayoutVersion>1</KeyboardLayoutVersion><PipelineRatesVersion>1</PipelineRatesVersion></root>"#;
const SERVERINFO_PAIRED: &str = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><NetworkPathVersion>1</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion><hostname>Snapshot</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion><uniqueid>{UNIQUE}</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><currentgameuuid></currentgameuuid><state>SUNSHINE_SERVER_FREE</state><LocalIP>127.0.0.1</LocalIP><mac>01:02:03:04:05:06</mac><MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>769</ServerCodecModeSupport><RustHostVersion>{VERSION}</RustHostVersion><RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps><RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive><RustHostProfile></RustHostProfile><PyroWaveHostLinkMbps>1000</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes><PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion><PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>2</PyroWaveMinParityShards><Permission>119480064</Permission><FrameLimiterSupported>1</FrameLimiterSupported><FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>1</VirtualDisplayFrameLimiterEnabled><FrameLimiterFpsLimitMilliHz>0</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation><VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><PowerActionVersion>1</PowerActionVersion><SleepSupported>0</SleepSupported><SleepAllowed>0</SleepAllowed><HdrFormats>15</HdrFormats><FecControlVersion>1</FecControlVersion><BandwidthProbeVersion>1</BandwidthProbeVersion><AudioCodecs>3</AudioCodecs><AudioLayouts>15</AudioLayouts><TextInputVersion>1</TextInputVersion><KeyboardLayoutVersion>1</KeyboardLayoutVersion><PipelineRatesVersion>1</PipelineRatesVersion></root>"#;
const APPLIST: &str = r#"<?xml version="1.0"?><root status_code="200"><App><AppTitle>Desktop</AppTitle><UUID>11111111-1111-4111-8111-111111111111</UUID><IDX>0</IDX><ID>958645192</ID><ArtVersion>958645192</ArtVersion><IsHdrSupported>1</IsHdrSupported></App><App><AppTitle>Dota 2</AppTitle><UUID>22222222-2222-4222-8222-222222222222</UUID><IDX>1</IDX><ID>831329605</ID><ArtVersion>831329605</ArtVersion><IsHdrSupported>1</IsHdrSupported></App><App><AppTitle>Remote Monitor</AppTitle><UUID>9a1c5a25-58fe-40e0-b9aa-7d3f00000005</UUID><IDX>0</IDX><ID>2147483505</ID><ArtVersion>remote-session-v6</ArtVersion><IsHdrSupported>1</IsHdrSupported></App></root>"#;
const CANCEL: &str =
    r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><cancel>1</cancel></root>"#;

async fn serverinfo_xml(f: &Fixture, tls: bool) -> String {
    let mut connection = f.connection(tls);
    connection.peer = "192.168.1.5:1234".parse().unwrap();
    let client = authenticated(&f.host, &connection, 0).ok();
    let config = butterpollo_core::config::Config::parse("sunshine_name=Snapshot").unwrap();
    let response = serverinfo_response(
        &f.host,
        &connection,
        &config,
        client.as_ref(),
        true,
        "01:02:03:04:05:06".into(),
        1000,
        false,
    );
    String::from_utf8(
        to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn golden(template: &str, f: &Fixture) -> String {
    template
        .replace("{UNIQUE}", &f.host.paired.read().unwrap().unique_id)
        .replace("{VERSION}", env!("CARGO_PKG_VERSION"))
}

#[tokio::test]
async fn serverinfo_only_gains_native_api_version_and_the_rest_is_byte_identical() {
    let f = Fixture::new();
    let field = |xml: &str| -> String {
        let start = xml.find("<NativeApiVersion>").expect("the field is there");
        let end = start + xml[start..].find("</NativeApiVersion>").unwrap();
        xml[start + "<NativeApiVersion>".len()..end].to_owned()
    };
    let strip = |xml: &str| -> String {
        let start = xml.find("<NativeApiVersion>").unwrap();
        let end =
            start + xml[start..].find("</NativeApiVersion>").unwrap() + "</NativeApiVersion>".len();
        format!("{}{}", &xml[..start], &xml[end..])
    };
    // Unpaired HTTPS, and plain HTTP (which can never identify a client).
    for tls in [true, false] {
        let xml = serverinfo_xml(&f, tls).await;
        assert_eq!(field(&xml), "0");
        assert_eq!(strip(&xml), golden(SERVERINFO_UNPAIRED, &f));
    }
    f.client(0x071f1f00);
    let xml = serverinfo_xml(&f, true).await;
    assert_eq!(field(&xml), "1");
    assert_eq!(strip(&xml), golden(SERVERINFO_PAIRED, &f));
    // Paired state without TLS is still version 0.
    assert_eq!(field(&serverinfo_xml(&f, false).await), "0");
    // A stock Moonlight client ignores the field: it sits among the others.
    assert!(xml.contains("<HardwareMetricsVersion>0</HardwareMetricsVersion><NativeApiVersion>1</NativeApiVersion><hostname>"));
}

#[tokio::test]
async fn applist_and_cancel_are_byte_identical_to_before_the_native_api() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    *f.host.apps.write().unwrap() = [
        json!({"name": "Desktop", "uuid": "11111111-1111-4111-8111-111111111111"}),
        json!({"name": "Dota 2", "uuid": "22222222-2222-4222-8222-222222222222", "steam-id": "570"}),
    ]
    .into_iter()
    .map(|app| serde_json::from_value(app).unwrap())
    .collect();
    let connection = f.connection(true);
    let applist = send(&f, true, &connection, "GET", "/applist", None, vec![]).await;
    assert_eq!(applist.text(), APPLIST);
    let cancel = send(&f, true, &connection, "GET", "/cancel", None, vec![]).await;
    assert_eq!(cancel.text(), CANCEL);
    // The native API does not need the Moonlight endpoints' permissions
    // changed: the same device reads both.
    assert_eq!(client.perm, 0x071f1f00);
    assert_eq!(
        send(
            &f,
            true,
            &connection,
            "GET",
            &format!("{PREFIX}/library"),
            None,
            vec![]
        )
        .await
        .status,
        StatusCode::OK
    );
}
