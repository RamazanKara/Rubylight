//! `GET /wake` against a fake PC (`crate::wake::fake`): the documented shape, and who may read
//! it. Nothing here touches the real adapters.
use super::*;
use crate::wake::fake::Fake;
use butterpollo_core::native_api::wake::{TargetState, Wake};

const FIXTURE: &str = include_str!("../../../../core/native-api/tests/fixtures/wake.json");

/// The request reached the fixture's PC on its Ethernet address.
fn lan(s: &Setup) -> Connection {
    Connection {
        local: "192.168.4.20:47984".parse().unwrap(),
        ..s.connection.clone()
    }
}

#[tokio::test]
async fn wake_matches_the_documented_fixture() {
    let s = Setup::with(LIST);
    Fake::new().on(s.host());
    let reply = api(&s.f, &lan(&s), "GET", "/wake").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    let wake: Wake = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(wake.targets[0].state, TargetState::Ready);
    let mut got = reply.json();
    let expected: Value = serde_json::from_str(FIXTURE).unwrap();
    let targets = got["targets"].as_array_mut().unwrap();
    for (target, documented) in targets
        .iter_mut()
        .zip(expected["targets"].as_array().unwrap())
    {
        // The fixture shows the first checks; the host may add information-only ones.
        let checks = target["checks"].as_array_mut().unwrap();
        let shown = documented["checks"].as_array().unwrap().len();
        assert!(checks.len() >= shown, "{checks:?}");
        checks.truncate(shown);
    }
    assert_eq!(got, expected);
}

#[tokio::test]
async fn wake_needs_list_view_or_launch() {
    for perm in [LIST, VIEW_BIT, LAUNCH] {
        let s = Setup::with(perm);
        Fake::new().on(s.host());
        assert_eq!(s.get("/wake").await.status, StatusCode::OK, "{perm:x}");
    }
    // Paired, with input only: no List, View or Launch.
    let s = Setup::with(1);
    Fake::new().on(s.host());
    s.get("/wake")
        .await
        .assert_error(403, "permission_required");
    // Only GET: the endpoint never changes a setting.
    s.post("/wake").await.assert_error(404, "not_found");
}

#[tokio::test]
async fn wake_from_this_pc_has_no_primary_and_a_failed_adapter_read_is_internal() {
    let s = Setup::new();
    let fake = Fake::new().on(s.host());
    // `Setup` connects over loopback, where /serverinfo reports zeros.
    let wake: Wake = serde_json::from_slice(&s.get("/wake").await.body).unwrap();
    assert_eq!(wake.primary, None);
    assert_eq!(wake.targets.len(), 2);
    *fake.adapters.lock().unwrap() = Err("IP Helper failed".into());
    s.get("/wake").await.assert_error(500, "internal");
}
