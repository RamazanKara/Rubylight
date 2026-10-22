//! The compatibility matrix of `docs/native-api.md` (Brief 5): stock Moonlight
//! and Nightjar 0.7.x know nothing of the native API, and their requests to
//! `/serverinfo`, `/applist` and `/cancel` behave as they did before it.
//! Both clients call exactly these endpoints (plus `/launch`, `/resume` and
//! `/appasset`, which `parity_tests` covers), so the tests below are the
//! host-side half of the matrix. The client-side half (a 2.6-style
//! `/serverinfo` without the field reads as "no native API") lives in
//! nightjar-core, which owns that parser.
use super::*;

/// The `NativeApiVersion` field of a `/serverinfo` answer, read the way a
/// client does: parsed XML, one element under the root.
fn native_api_version(xml: &str) -> String {
    let root = xmltree::Element::parse(xml.as_bytes()).unwrap();
    root.get_child("NativeApiVersion")
        .expect("the field is there")
        .get_text()
        .unwrap_or_default()
        .into_owned()
}

/// A `/serverinfo` answer with the one new field taken out: what a client
/// that predates the field would have received.
fn without_the_field(xml: &str) -> String {
    let start = xml.find("<NativeApiVersion>").unwrap();
    let end =
        start + xml[start..].find("</NativeApiVersion>").unwrap() + "</NativeApiVersion>".len();
    format!("{}{}", &xml[..start], &xml[end..])
}

/// `/serverinfo` through the real router, with the query string Moonlight and
/// Nightjar send.
async fn serverinfo_request(f: &Fixture, tls: bool) -> Reply {
    send(
        f,
        tls,
        &f.connection(tls),
        "GET",
        "/serverinfo?uniqueid=0123456789ABCDEF&uuid=7c4d0c5e-0a0e-4a39-8f1e-2d6f2a0e9b11",
        None,
        vec![],
    )
    .await
}

#[tokio::test]
async fn serverinfo_over_plain_http_and_unpaired_is_version_0_with_every_stock_field() {
    let f = Fixture::new();
    for tls in [false, true] {
        let reply = serverinfo_request(&f, tls).await;
        assert_eq!(reply.status, StatusCode::OK);
        let xml = reply.text();
        assert_eq!(native_api_version(&xml), "0", "tls: {tls}");
        // Everything a stock client reads is still there, and the answer is
        // the pre-native-API one apart from the one field.
        let root = xmltree::Element::parse(xml.as_bytes()).unwrap();
        assert_eq!(root.attributes["status_code"], "200");
        for name in [
            "hostname",
            "appversion",
            "GfeVersion",
            "uniqueid",
            "HttpsPort",
            "ExternalPort",
            "PairStatus",
            "currentgame",
            "state",
            "LocalIP",
            "mac",
            "ServerCodecModeSupport",
        ] {
            assert!(root.get_child(name).is_some(), "{name} is missing");
        }
        assert_eq!(
            root.get_child("PairStatus").unwrap().get_text().unwrap(),
            "0"
        );
        assert!(!without_the_field(&xml).contains("NativeApi"));
    }
}

#[tokio::test]
async fn serverinfo_for_a_paired_device_over_https_is_version_1() {
    let f = Fixture::new();
    f.client(0x071f1f00);
    let https = serverinfo_request(&f, true).await.text();
    assert_eq!(native_api_version(&https), "1");
    // The same device on plain HTTP cannot be identified: version 0.
    let http = serverinfo_request(&f, false).await.text();
    assert_eq!(native_api_version(&http), "0");
}

#[tokio::test]
async fn serverinfo_drops_back_to_version_0_when_the_device_is_disabled_or_removed() {
    let f = Fixture::new();
    f.client(ALL);
    assert_eq!(
        native_api_version(&serverinfo_request(&f, true).await.text()),
        "1"
    );
    f.host.paired.write().unwrap().clients[0].enabled = false;
    assert_eq!(
        native_api_version(&serverinfo_request(&f, true).await.text()),
        "0"
    );
    f.host.paired.write().unwrap().clients[0].enabled = true;
    f.host.paired.write().unwrap().clients[0].perm = 0;
    assert_eq!(
        native_api_version(&serverinfo_request(&f, true).await.text()),
        "0"
    );
}

#[tokio::test]
async fn native_requests_do_not_change_what_moonlight_sees() {
    let s = Setup::new();
    *s.host().apps.write().unwrap() = [
        json!({"name": "Desktop", "uuid": "11111111-1111-4111-8111-111111111111"}),
        json!({"name": "Dota 2", "uuid": "22222222-2222-4222-8222-222222222222"}),
    ]
    .into_iter()
    .map(|app| serde_json::from_value(app).unwrap())
    .collect();
    let before = (
        serverinfo_request(&s.f, true).await.text(),
        send(&s.f, true, &s.connection, "GET", "/applist", None, vec![])
            .await
            .text(),
    );
    // A client that uses the native API reads everything and runs actions
    // that refuse (no session): none of it leaves a trace in Moonlight's view.
    s.host_control(true);
    for path in ["/capabilities", "/library", "/session", "/radeon"] {
        s.get(path).await;
    }
    for path in [
        "/session/disconnect",
        "/session/end",
        "/session/close-game",
        "/radeon/restore",
    ] {
        s.post(path).await;
    }
    let after = (
        serverinfo_request(&s.f, true).await.text(),
        send(&s.f, true, &s.connection, "GET", "/applist", None, vec![])
            .await
            .text(),
    );
    assert_eq!(before, after);
}

#[tokio::test]
async fn stock_cancel_with_launch_permission_alone_ends_the_streams_and_the_game() {
    // No Host control: decision 2 freezes `/cancel`, which needs Launch only.
    let s = Setup::new();
    let stream = s.streaming();
    let reply = send(&s.f, true, &s.connection, "GET", "/cancel", None, vec![]).await;
    assert_eq!(reply.text(), CANCEL);
    assert!(stream.stopping());
    assert!(!s.game_running());
    assert_eq!(s.state(), SessionState::Idle);
    // Repeating it is the same plain answer.
    let again = send(&s.f, true, &s.connection, "GET", "/cancel", None, vec![]).await;
    assert_eq!(again.text(), CANCEL);
}

#[tokio::test]
async fn stock_cancel_closes_a_game_that_the_native_end_session_left_running() {
    let s = Setup::new();
    let _stream = s.streaming();
    assert_eq!(
        s.post("/session/end").await.session().state,
        SessionState::Idle
    );
    assert!(s.game_running());
    let reply = send(&s.f, true, &s.connection, "GET", "/cancel", None, vec![]).await;
    assert_eq!(reply.text(), CANCEL);
    assert!(!s.game_running());
}

#[tokio::test]
async fn stock_cancel_without_a_game_is_a_plain_answer_and_changes_nothing() {
    let s = Setup::new();
    let reply = send(&s.f, true, &s.connection, "GET", "/cancel", None, vec![]).await;
    assert_eq!(
        (reply.status, reply.text().as_str()),
        (StatusCode::OK, CANCEL)
    );
    assert_eq!(s.state(), SessionState::Idle);
    assert!(!s.game_running());
}

#[tokio::test]
async fn stock_cancel_without_launch_permission_is_refused_and_leaves_the_game() {
    let s = Setup::with(LIST | VIEW_BIT);
    s.run_game();
    let reply = send(&s.f, true, &s.connection, "GET", "/cancel", None, vec![]).await;
    // Moonlight's convention: HTTP 200 with the refusal in the XML status.
    assert!(
        reply.text().contains(r#"status_code="401""#),
        "{}",
        reply.text()
    );
    assert!(!reply.text().contains("<cancel>1</cancel>"));
    assert!(s.game_running());
}
