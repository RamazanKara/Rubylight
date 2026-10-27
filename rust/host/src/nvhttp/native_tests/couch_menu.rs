//! `POST /session/couch-menu` (`docs/couch-menu-design.md` section 7.1)
//! through the router.
use super::*;
use butterpollo_core::couch_menu::Shared as Menu;

async fn couch_menu(s: &Setup, connection: &Connection, body: &str) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/session/couch-menu"),
        Some("application/json"),
        body.as_bytes().to_vec(),
    )
    .await
}

#[tokio::test]
async fn couch_menu_opens_and_closes_the_callers_stream() {
    let s = Setup::new();
    let session = s.streaming();
    let reply = couch_menu(&s, &s.connection, r#"{"open":true}"#).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json(), serde_json::json!({"open": true}));
    assert!(Menu::is_open(session.couch_menu.load()));
    // Idempotent both ways.
    let reply = couch_menu(&s, &s.connection, r#"{"open":true}"#).await;
    assert_eq!(reply.json(), serde_json::json!({"open": true}));
    let reply = couch_menu(&s, &s.connection, r#"{"open":false}"#).await;
    assert_eq!(reply.json(), serde_json::json!({"open": false}));
    let reply = couch_menu(&s, &s.connection, r#"{"open":false}"#).await;
    assert_eq!(reply.json(), serde_json::json!({"open": false}));
    assert!(!Menu::is_open(session.couch_menu.load()));
}

#[tokio::test]
async fn couch_menu_refuses_bad_bodies_permissions_and_other_sessions() {
    let s = Setup::new();
    for body in ["", "{}", r#"{"open":1}"#, r#"{"open":true,"page":"main"}"#] {
        couch_menu(&s, &s.connection, body)
            .await
            .assert_error(400, "invalid_request");
    }
    couch_menu(&s, &s.connection, r#"{"open":true}"#)
        .await
        .assert_error(409, "no_session");
    let session = s.streaming();
    let other = other_device(&s.f, ALL, true);
    couch_menu(&s, &other, r#"{"open":true}"#)
        .await
        .assert_error(409, "not_session_owner");
    assert!(!Menu::is_open(session.couch_menu.load()));
    let lister = Setup::with(LIST);
    couch_menu(&lister, &lister.connection, r#"{"open":true}"#)
        .await
        .assert_error(403, "permission_required");
}

#[tokio::test]
async fn couch_menu_off_for_the_device_is_unadvertised() {
    let s = Setup::new();
    let listed = async || {
        let capabilities: Capabilities =
            serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
        capabilities
            .features
            .iter()
            .any(|f| f == "session.couch_menu")
    };
    assert!(listed().await);
    s.host().paired.write().unwrap().clients[0].extra.insert(
        "config_overrides".into(),
        serde_json::json!({"couch_menu": false}),
    );
    assert!(!listed().await, "the device's own setting turns it off");
}

#[tokio::test]
async fn couch_menu_off_is_temporarily_unavailable_and_unadvertised() {
    let s = Setup::new();
    let session = s.streaming();
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("couch_menu".into(), "false".into());
    couch_menu(&s, &s.connection, r#"{"open":true}"#)
        .await
        .assert_error(503, "temporarily_unavailable");
    assert!(!Menu::is_open(session.couch_menu.load()));
    let capabilities: Capabilities =
        serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(
        !capabilities
            .features
            .iter()
            .any(|f| f == "session.couch_menu")
    );
}
