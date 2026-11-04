//! `/power` and `/update` against a fake backend: every grant, gate and
//! refusal. Nothing here can reach Windows' restart or shutdown: the default
//! backend under `cfg(test)` panics, and each test installs [`Fake`].
use super::*;
use crate::{
    power_action::{Backend, PowerActions},
    tls::ConnectionEnd,
    wake::fake::Fake as FakeWake,
};
use anyhow::Result;
use butterpollo_core::{
    native_api::power::{Power, Update},
    power_action::{Action, RESTART_SHUTDOWN_OFF, SleepStates, Unconfirmed},
};
use std::{
    sync::{Mutex, Weak},
    time::{Duration, Instant},
};

/// Records each restart or shutdown and whether the lifecycle gate was held,
/// and answers with `refusal` as Windows' error when set, as an unanswered
/// request with `unconfirmed`.
struct Fake {
    host: Weak<crate::state::Host>,
    calls: Mutex<Vec<(Action, bool)>>,
    refusal: Mutex<Option<String>>,
    unconfirmed: Mutex<bool>,
}
impl Backend for Fake {
    fn states(&self) -> Result<SleepStates> {
        Ok(SleepStates::default())
    }
    fn suspend(&self) -> Result<()> {
        panic!("a power test reached the suspend")
    }
    fn power_off(&self, action: Action) -> Result<()> {
        let host = self.host.upgrade().unwrap();
        let held = host.launch_transition.try_lock().is_err();
        self.calls.lock().unwrap().push((action, held));
        match self.refusal.lock().unwrap().clone() {
            Some(refusal) if *self.unconfirmed.lock().unwrap() => Err(Unconfirmed(refusal).into()),
            Some(refusal) => anyhow::bail!(refusal),
            None => Ok(()),
        }
    }
}
fn fake(s: &Setup) -> Arc<Fake> {
    let fake = Arc::new(Fake {
        host: Arc::downgrade(&s.f.host),
        calls: Mutex::default(),
        refusal: Mutex::default(),
        unconfirmed: Mutex::default(),
    });
    s.host().power.set_backend(fake.clone());
    FakeWake::new().on(s.host());
    fake
}
fn grant(s: &Setup, granted: bool) {
    let mut paired = s.host().paired.write().unwrap();
    let extra = &mut paired.clients[0].extra;
    if granted {
        extra.insert("allow_restart_shutdown".into(), true.into());
    } else {
        extra.remove("allow_restart_shutdown");
    }
}
/// The request reached the PC on the wake fixture's Ethernet address.
fn lan(s: &Setup) -> Connection {
    Connection {
        local: "192.168.4.20:47984".parse().unwrap(),
        ..s.connection.clone()
    }
}
async fn power(s: &Setup) -> Power {
    let reply = api(&s.f, &lan(s), "GET", "/power").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    serde_json::from_slice(&reply.body).unwrap()
}
/// A restart or shutdown whose connection then ends `clean` (or never, for
/// `None`), and the time for its blocking half to finish.
async fn act(
    s: &Setup,
    connection: &Connection,
    action: &str,
    body: &str,
    clean: Option<bool>,
) -> Reply {
    act_then(s, connection, action, body, clean, || {}).await
}
/// [`act`], with `meanwhile` run after the answer and before its connection
/// ends: what the PC does during the delivery wait.
async fn act_then(
    s: &Setup,
    connection: &Connection,
    action: &str,
    body: &str,
    clean: Option<bool>,
    meanwhile: impl FnOnce(),
) -> Reply {
    let end = ConnectionEnd::default();
    let response = router(s.f.host.clone(), true)
        .layer(Extension(connection.clone()))
        .layer(Extension(end.clone()))
        .oneshot(
            Request::post(format!("{PREFIX}/power/{action}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let reply = Reply {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, 1 << 20).await.unwrap().to_vec(),
    };
    meanwhile();
    if let Some(clean) = clean {
        end.finish(clean);
    }
    let started = Instant::now();
    while s.host().launch_transition.try_lock().is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "gate still held"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    reply
}
fn token_body(token: &str) -> String {
    serde_json::json!({ "token": token }).to_string()
}

#[test]
fn tests_never_get_the_backend_that_reaches_windows() {
    assert!(!PowerActions::default().backend().native());
    assert!(!Fixture::new().host.power.backend().native());
}

#[tokio::test]
async fn without_the_grant_both_are_offered_off_with_the_reason_and_refused() {
    let s = Setup::new();
    let fake = fake(&s);
    // Host control and Sleep PC don't grant it.
    s.host_control(true);
    s.host().paired.write().unwrap().clients[0]
        .extra
        .insert("allow_sleep".into(), true.into());
    let offer = power(&s).await;
    assert!(!offer.restart.available && !offer.shutdown.available);
    assert_eq!(offer.restart.reason.as_deref(), Some(RESTART_SHUTDOWN_OFF));
    assert_eq!(offer.token, None);
    let reply = act(
        &s,
        &s.connection,
        "restart",
        &token_body(&"ab".repeat(32)),
        Some(true),
    )
    .await;
    reply.assert_error(403, "permission_required");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Restart and shut down PC")
    );
    assert!(fake.calls.lock().unwrap().is_empty());
    let capabilities: Capabilities =
        serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(!capabilities.permissions.restart_shutdown);
    for feature in ["power.restart", "power.shutdown"] {
        assert!(
            capabilities.features.iter().any(|f| f == feature),
            "{feature}"
        );
    }
    // Tests run outside an installed service, which can't update itself.
    assert!(!capabilities.features.iter().any(|f| f == "update"));
}

#[tokio::test]
async fn an_idle_pc_restarts_and_shuts_down_once_per_token_after_the_answer_left() {
    for (action, expected) in [("restart", Action::Restart), ("shutdown", Action::Shutdown)] {
        let s = Setup::new();
        let fake = fake(&s);
        grant(&s, true);
        let capabilities: Capabilities =
            serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
        assert!(capabilities.permissions.restart_shutdown);
        let offer = power(&s).await;
        assert!(
            offer.restart.available && offer.shutdown.available,
            "{offer:?}"
        );
        assert_eq!(
            (offer.restart.reason.as_deref(), offer.token_lifetime_s),
            (None, 30)
        );
        assert!(offer.wake_from_off);
        let token = offer.token.unwrap();
        let reply = act(&s, &s.connection, action, &token_body(&token), Some(true)).await;
        assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
        assert_eq!(reply.headers[header::CONNECTION], "close");
        assert_eq!(reply.json()["action"], action);
        // Once, with the gate held, so a racing launch waits.
        assert_eq!(*fake.calls.lock().unwrap(), [(expected, true)]);
        // The token is spent.
        act(&s, &s.connection, action, &token_body(&token), Some(true))
            .await
            .assert_error(409, "token_expired");
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn an_answer_that_did_not_arrive_or_a_revoked_grant_never_goes_ahead() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    let token = power(&s).await.token.unwrap();
    let reply = act(
        &s,
        &s.connection,
        "shutdown",
        &token_body(&token),
        Some(false),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert!(fake.calls.lock().unwrap().is_empty());
    // Disabled after the token: refused before anything runs.
    let token = power(&s).await.token.unwrap();
    grant(&s, false);
    act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await
    .assert_error(403, "permission_required");
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_grant_revoked_after_the_answer_stops_it_and_says_why() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    let token = power(&s).await.token.unwrap();
    let reply = act_then(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
        || grant(&s, false),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert!(fake.calls.lock().unwrap().is_empty());
    // The device's follow-up read shows why nothing happened.
    assert_eq!(
        power(&s).await.last_refusal.as_deref(),
        Some(RESTART_SHUTDOWN_OFF)
    );
}

#[tokio::test]
async fn another_devices_stream_refuses_both_and_names_it_and_the_token_survives() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    let token = power(&s).await.token.unwrap();
    other_device(&s.f, ALL, false);
    let other = s.host().paired.read().unwrap().clients[1].clone();
    let session = s.f.session(other, Role::Stream);
    let named = "Other device is streaming from this PC.";
    let offer = power(&s).await;
    assert_eq!(offer.restart.reason.as_deref(), Some(named));
    assert_eq!(offer.shutdown.reason.as_deref(), Some(named));
    assert_eq!(offer.token, None);
    for action in ["restart", "shutdown"] {
        let reply = act(&s, &s.connection, action, &token_body(&token), Some(true)).await;
        reply.assert_error(409, "host_in_use");
        assert_eq!(reply.json()["error"]["message"], named);
    }
    // The device's own stream counts too: the same gate as Sleep PC.
    drop(session);
    s.host().sessions.lock().unwrap().active.clear();
    let own = s.stream_as(s.client.clone());
    act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await
    .assert_error(409, "host_in_use");
    drop(own);
    s.host().sessions.lock().unwrap().active.clear();
    // A game left running without a stream, too.
    s.run_game();
    let reply = act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    reply.assert_error(409, "host_in_use");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .ends_with("is still running on this PC.")
    );
    *s.host().current_app.lock().unwrap() = None;
    assert!(fake.calls.lock().unwrap().is_empty());
    // None of these spent the token.
    let reply = act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
}

#[tokio::test]
async fn windows_refusal_is_on_the_next_read_and_a_bad_body_is_400() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    *fake.refusal.lock().unwrap() =
        Some("Windows refused to restart the PC: users are signed in".into());
    let token = power(&s).await.token.unwrap();
    let reply = act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert_eq!(
        power(&s).await.last_refusal.as_deref(),
        Some("Windows refused to restart the PC: users are signed in")
    );
    // The next request Windows takes clears it.
    *fake.refusal.lock().unwrap() = None;
    let token = power(&s).await.token.unwrap();
    act(
        &s,
        &s.connection,
        "shutdown",
        &token_body(&token),
        Some(true),
    )
    .await;
    assert_eq!(power(&s).await.last_refusal, None);
    for body in ["", "{}", r#"{"token":"x","force":true}"#, "["] {
        act(&s, &s.connection, "restart", body, Some(true))
            .await
            .assert_error(400, "invalid_request");
    }
    act(
        &s,
        &s.connection,
        "restart",
        &token_body("short"),
        Some(true),
    )
    .await
    .assert_error(409, "token_expired");
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_request_windows_did_not_confirm_is_said_as_unconfirmed() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    let unanswered =
        "Windows did not confirm the request in time; the PC may still restart: process timed out";
    *fake.refusal.lock().unwrap() = Some(unanswered.into());
    *fake.unconfirmed.lock().unwrap() = true;
    let token = power(&s).await.token.unwrap();
    let reply = act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    let read = power(&s).await;
    assert_eq!(
        (read.last_refusal.as_deref(), read.last_refusal_unconfirmed),
        (Some(unanswered), true)
    );
    // A refusal is one.
    *fake.unconfirmed.lock().unwrap() = false;
    let token = read.token.unwrap();
    act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    let read = power(&s).await;
    assert_eq!(
        (read.last_refusal.as_deref(), read.last_refusal_unconfirmed),
        (Some(unanswered), false)
    );
    // Nothing to say, nothing unconfirmed.
    *fake.refusal.lock().unwrap() = None;
    let token = read.token.unwrap();
    act(
        &s,
        &s.connection,
        "shutdown",
        &token_body(&token),
        Some(true),
    )
    .await;
    let read = power(&s).await;
    assert_eq!(
        (read.last_refusal, read.last_refusal_unconfirmed),
        (None, false)
    );
}

#[tokio::test]
async fn a_refusal_shows_only_to_its_device_and_not_for_ever() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    let refusal = "Windows refused to restart the PC: no";
    *fake.refusal.lock().unwrap() = Some(refusal.into());
    let token = power(&s).await.token.unwrap();
    act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    assert_eq!(power(&s).await.last_refusal.as_deref(), Some(refusal));
    // Another device, one with the grant too, never sees it.
    let other = Connection {
        local: lan(&s).local,
        ..other_device(&s.f, ALL, false)
    };
    s.host().paired.write().unwrap().clients[1]
        .extra
        .insert("allow_restart_shutdown".into(), true.into());
    let reply = api(&s.f, &other, "GET", "/power").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let theirs: Power = serde_json::from_slice(&reply.body).unwrap();
    assert!(theirs.token.is_some());
    assert_eq!(theirs.last_refusal, None);
    // Nor does it stay for ever.
    let uuid = s.client.uuid.as_str();
    let power_actions = &s.host().power;
    assert!(power_actions.last_refusal(uuid, Instant::now()).is_some());
    let later = Instant::now() + crate::power_action::REFUSAL_KEPT;
    assert_eq!(power_actions.last_refusal(uuid, later), None);
}

#[tokio::test]
async fn a_guest_or_controller_session_during_the_delivery_wait_stops_it_and_says_why() {
    let s = Setup::new();
    let fake = fake(&s);
    grant(&s, true);
    // Neither takes the lifecycle gate the request holds.
    let token = power(&s).await.token.unwrap();
    let reply = act_then(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
        || {
            let (handle, _) = crate::webrtc::media::tests::real_handle(Default::default());
            s.host().webrtc_links.put("guest".into(), handle);
        },
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert!(fake.calls.lock().unwrap().is_empty());
    assert_eq!(
        power(&s).await.last_refusal.as_deref(),
        Some("A browser guest is connected.")
    );
    s.host().webrtc_links.discard("guest");
    let token = power(&s).await.token.unwrap();
    let mut controller = None;
    act_then(
        &s,
        &s.connection,
        "shutdown",
        &token_body(&token),
        Some(true),
        || {
            let launch = s.f.launch(s.client.clone(), Role::Stream);
            controller = Some(
                s.host()
                    .controller_sessions
                    .acquire(&launch, &Default::default())
                    .unwrap(),
            );
        },
    )
    .await;
    assert!(fake.calls.lock().unwrap().is_empty());
    assert_eq!(
        power(&s).await.last_refusal.as_deref(),
        Some("A controller session is in use.")
    );
    drop(controller);
    // With nothing in the way it goes ahead, and the refusal is over.
    let token = power(&s).await.token.unwrap();
    act(
        &s,
        &s.connection,
        "restart",
        &token_body(&token),
        Some(true),
    )
    .await;
    assert_eq!(*fake.calls.lock().unwrap(), [(Action::Restart, true)]);
    assert_eq!(power(&s).await.last_refusal, None);
}

#[tokio::test]
async fn update_reads_for_any_paired_device_and_installs_only_with_host_control_and_no_other_stream()
 {
    let s = Setup::with(LIST);
    let reply = s.get("/update").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let update: Update = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(update.current_version, env!("CARGO_PKG_VERSION"));
    assert!(!update.install_supported && !update.install_now);
    // A queued Install now says so; an automatic update waiting for an idle
    // host does not, so a device still offers Install now for it.
    let before = s.host().updates.lock().unwrap().clone();
    for (now, expected) in [(false, false), (true, true)] {
        *s.host().updates.lock().unwrap() =
            serde_json::json!({"phase": "ready", "idle_since": 0, "now": now});
        let update: Update = serde_json::from_slice(&s.get("/update").await.body).unwrap();
        assert_eq!(
            (update.phase.as_str(), update.install_now),
            ("ready", expected)
        );
    }
    *s.host().updates.lock().unwrap() = before;
    s.post("/update/install")
        .await
        .assert_error(403, "host_control_required");
    s.host_control(true);
    other_device(&s.f, ALL, false);
    let other = s.host().paired.read().unwrap().clients[1].clone();
    s.f.session(other, Role::Stream);
    let reply = s.post("/update/install").await;
    reply.assert_error(409, "host_in_use");
    assert_eq!(
        reply.json()["error"]["message"],
        "Other device is streaming from this PC."
    );
    // The caller's own stream does not stop it: Install now ends it. This host
    // is no installed service, so the updater says why it can't.
    s.host().sessions.lock().unwrap().active.clear();
    s.stream_as(s.client.clone());
    let reply = s.post("/update/install").await;
    reply.assert_error(409, "update_unavailable");
    assert_eq!(
        reply.json()["error"]["message"],
        "Install Rubylight as a Windows service to use in-app updates."
    );
}
