//! `/pairing/*`: the real handshake behind an approval from another device.
use super::*;
use crate::{
    native_api::pairing,
    nvhttp::parity_tests::{device_request, finish_pairing},
};
use butterpollo_core::{
    native_api::pairing::PairingRequests,
    state::{LATER_DEVICE_PERMISSION, PLAY_DEVICE_PERMISSION},
};
use std::time::{Duration, Instant};

const PEER: &str = "127.0.0.1";

async fn approve(f: &Fixture, connection: &Connection, id: &str, pin: &str) -> Reply {
    send(
        f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/pairing/approve"),
        Some("application/json"),
        serde_json::json!({"id": id, "pin": pin}).to_string(),
    )
    .await
}
async fn listed(f: &Fixture, connection: &Connection) -> Vec<String> {
    let reply = api(f, connection, "GET", "/pairing/requests").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let requests: PairingRequests = serde_json::from_slice(&reply.body).unwrap();
    requests.requests.into_iter().map(|r| r.name).collect()
}
/// A device asking to pair: its certificate request waits for the PIN.
async fn asking(f: &Fixture, name: &str) -> (crypto::Identity, String) {
    let client = Identity::generate().unwrap();
    let request = device_request(&client, name, None);
    let host = f.host.clone();
    tokio::spawn(async move { do_pair(host, &request, PEER.parse().unwrap()).await });
    let id = pairing_id(name, PEER.parse().unwrap());
    while !f.host.pins.lock().unwrap().contains_key(&id) {
        tokio::task::yield_now().await;
    }
    (client, id)
}
fn saved(f: &Fixture, name: &str) -> Option<Client> {
    f.host
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|c| c.name == name)
        .cloned()
}
/// The approval and the new device's handshake with `pin`, as they interleave.
async fn run(f: &Fixture, approver: &Connection, name: &str, typed: &str, shown: &str) -> Reply {
    let (client, id) = asking(f, name).await;
    let (reply, _) = tokio::join!(approve(f, approver, &id, typed), async {
        while f.host.pins.lock().unwrap().contains_key(&id)
            || !f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        {
            tokio::task::yield_now().await;
        }
        let _ = finish_pairing(f, &client, name, shown).await;
    });
    reply
}

#[tokio::test]
async fn the_routes_need_a_paired_device_with_host_control() {
    let f = Fixture::new();
    f.client(ALL);
    let (_, id) = asking(&f, "TV").await;
    let plain = other_device(&f, ALL, false);
    let stranger = Connection {
        certificate: None,
        ..f.connection(true)
    };
    for connection in [&plain, &stranger] {
        let code = if connection.certificate.is_some() {
            "host_control_required"
        } else {
            "not_paired"
        };
        api(&f, connection, "GET", "/pairing/requests")
            .await
            .assert_error(403, code);
        approve(&f, connection, &id, "1234")
            .await
            .assert_error(403, code);
        send(
            &f,
            true,
            connection,
            "POST",
            &format!("{PREFIX}/pairing/deny"),
            None,
            serde_json::json!({"id": id}).to_string(),
        )
        .await
        .assert_error(403, code);
    }
    // No PIN reached the waiting device.
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn the_list_names_a_waiting_device_and_never_its_certificate() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    let (client, id) = asking(&f, "LivingRoomTV").await;
    let reply = api(&f, &approver, "GET", "/pairing/requests").await;
    let body = reply.json();
    assert_eq!(body["requests"][0]["id"], id.as_str());
    assert_eq!(body["requests"][0]["name"], "LivingRoomTV");
    assert_eq!(body["requests"][0]["address"], PEER);
    assert!(body["requests"][0]["age_seconds"].is_u64());
    let text = reply.text();
    assert!(!text.contains("cert"), "{text}");
    assert!(!text.contains(&client.certificate), "{text}");
}

#[tokio::test]
async fn a_right_pin_pairs_with_the_consoles_default_and_answers_when_it_is_done() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["name"], "TV");
    let tv = saved(&f, "TV").unwrap();
    assert_eq!(tv.perm, PLAY_DEVICE_PERMISSION);
    assert!(tv.allows_coop_join());
    assert!(!tv.allows_host_control());
    assert!(f.host.approvals.lock().unwrap().is_idle());
}

#[tokio::test]
async fn the_new_device_never_gets_more_than_its_approver_holds() {
    let f = Fixture::new();
    // Host control, but no input and no launching: the approver's to cap.
    let approver = other_device(&f, LATER_DEVICE_PERMISSION, true);
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(saved(&f, "TV").unwrap().perm, LATER_DEVICE_PERMISSION);
}

#[tokio::test]
async fn an_approver_kept_out_of_coop_cannot_hand_it_on() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    {
        let mut state = f.host.paired.write().unwrap();
        let other = state
            .clients
            .iter_mut()
            .find(|c| c.name == "Other device")
            .unwrap();
        other.extra.insert("coop_join".into(), false.into());
    }
    let reply = run(&f, &approver, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let tv = saved(&f, "TV").unwrap();
    assert_eq!(tv.perm, PLAY_DEVICE_PERMISSION);
    assert!(!tv.allows_coop_join());
}

#[tokio::test]
async fn host_control_lost_during_the_handshake_refuses_the_new_device() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let (client, id) = asking(&f, "TV").await;
    let (reply, finished) = tokio::join!(approve(&f, &approver, &id, "1234"), async {
        while !f.host.pairings.lock().unwrap().sessions.contains_key(&id) {
            tokio::task::yield_now().await;
        }
        f.host
            .paired
            .write()
            .unwrap()
            .clients
            .iter_mut()
            .find(|c| c.name == "Other device")
            .unwrap()
            .extra
            .remove("host_control");
        finish_pairing(&f, &client, "TV", "1234").await
    });
    assert!(finished.is_err());
    reply.assert_error(422, "wrong_pin");
    assert!(saved(&f, "TV").is_none());
}

#[tokio::test]
async fn three_wrong_pins_end_the_devices_requests_here_and_the_console_still_can() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    for _ in 0..3 {
        run(&f, &approver, "TV", "1111", "2222")
            .await
            .assert_error(422, "wrong_pin");
        assert!(saved(&f, "TV").is_none());
    }
    // The device asks again: not offered, refused, and left for the console.
    let (_, id) = asking(&f, "TV").await;
    assert!(listed(&f, &approver).await.is_empty());
    approve(&f, &approver, &id, "2222")
        .await
        .assert_error(409, "pairing_ended");
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
    assert!(saved(&f, "TV").is_none());
}

#[tokio::test]
async fn another_device_at_the_same_address_keeps_its_own_count() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    for _ in 0..3 {
        run(&f, &approver, "TV", "1111", "2222")
            .await
            .assert_error(422, "wrong_pin");
    }
    asking(&f, "Console").await;
    assert_eq!(listed(&f, &approver).await, ["Console"]);
}

#[tokio::test]
async fn a_right_pin_clears_the_wrong_ones() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    run(&f, &approver, "TV", "1111", "2222")
        .await
        .assert_error(422, "wrong_pin");
    let reply = run(&f, &approver, "TV", "2222", "2222").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert!(f.host.approvals.lock().unwrap().is_idle());
}

#[tokio::test]
async fn moonlights_unpair_after_a_wrong_pin_counts_at_once() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let (_client, id) = asking(&f, "TV").await;
    let plain = Connection {
        certificate: None,
        tls: false,
        ..f.connection(false)
    };
    let (reply, _) = tokio::join!(approve(&f, &approver, &id, "1111"), async {
        while !f.host.pairings.lock().unwrap().sessions.contains_key(&id) {
            tokio::task::yield_now().await;
        }
        send(
            &f,
            false,
            &plain,
            "GET",
            "/unpair?uniqueid=TV",
            None,
            vec![],
        )
        .await
    });
    reply.assert_error(422, "wrong_pin");
}

#[tokio::test]
async fn a_device_that_goes_quiet_counts_and_may_still_finish_under_the_cap() {
    let f = Fixture::new();
    let approver = other_device(&f, LATER_DEVICE_PERMISSION, true);
    let (client, id) = asking(&f, "TV").await;
    approve(&f, &approver, &id, "1234")
        .await
        .assert_error(409, "pairing_unconfirmed");
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_for("TV"), 1);
    finish_pairing(&f, &client, "TV", "1234").await.unwrap();
    assert_eq!(saved(&f, "TV").unwrap().perm, LATER_DEVICE_PERMISSION);
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_for("TV"), 0);
    assert!(f.host.approvals.lock().unwrap().is_idle());
}

#[tokio::test]
async fn an_expired_or_unknown_request_is_not_found_and_not_listed() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    let (_, id) = asking(&f, "TV").await;
    f.host.pins.lock().unwrap().get_mut(&id).unwrap().created =
        Instant::now() - Duration::from_secs(301);
    assert!(listed(&f, &approver).await.is_empty());
    approve(&f, &approver, &id, "1234")
        .await
        .assert_error(404, "not_found");
    approve(&f, &approver, "nobody", "1234")
        .await
        .assert_error(404, "not_found");
    assert!(f.host.approvals.lock().unwrap().is_idle());
}

#[tokio::test]
async fn a_malformed_pin_or_body_leaves_the_request_waiting() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    let (_, id) = asking(&f, "TV").await;
    for pin in ["123", "12345", "12a4", ""] {
        approve(&f, &approver, &id, pin)
            .await
            .assert_error(422, "invalid_value");
    }
    send(
        &f,
        true,
        &approver,
        "POST",
        &format!("{PREFIX}/pairing/approve"),
        None,
        serde_json::json!({"id": id, "pin": "1234", "access": "all"}).to_string(),
    )
    .await
    .assert_error(400, "invalid_request");
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn deny_ends_the_devices_wait_and_answers_the_rest() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, true);
    let client = Identity::generate().unwrap();
    let request = device_request(&client, "TV", None);
    let host = f.host.clone();
    let waiting = tokio::spawn(async move { do_pair(host, &request, PEER.parse().unwrap()).await });
    let id = pairing_id("TV", PEER.parse().unwrap());
    while !f.host.pins.lock().unwrap().contains_key(&id) {
        tokio::task::yield_now().await;
    }
    let reply = send(
        &f,
        true,
        &approver,
        "POST",
        &format!("{PREFIX}/pairing/deny"),
        None,
        serde_json::json!({"id": id}).to_string(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["requests"], serde_json::json!([]));
    assert!(waiting.await.unwrap().is_err());
    assert!(saved(&f, "TV").is_none());
    send(
        &f,
        true,
        &approver,
        "POST",
        &format!("{PREFIX}/pairing/deny"),
        None,
        serde_json::json!({"id": id}).to_string(),
    )
    .await
    .assert_error(404, "not_found");
}

#[tokio::test]
async fn a_browser_device_needs_an_approver_who_holds_its_default() {
    let f = Fixture::new();
    let approver = other_device(&f, 1 << 24, true);
    let (id, _entered) =
        f.host
            .webrtc
            .begin_pairing(&f.host, "4321", "Browser TV", PEER.parse().unwrap());
    assert_eq!(listed(&f, &approver).await, ["Browser TV"]);
    approve(&f, &approver, &id, "4321")
        .await
        .assert_error(403, "permission_required");
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn a_browser_devices_pin_is_checked_and_its_approver_named() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    let (id, entered) =
        f.host
            .webrtc
            .begin_pairing(&f.host, "4321", "Browser TV", PEER.parse().unwrap());
    let host = f.host.clone();
    let finishing = {
        let id = id.clone();
        tokio::spawn(async move {
            let (pin, name, access) = entered.await.unwrap();
            assert_eq!(access, None, "a browser device takes no access choice");
            host.webrtc
                .finish_pairing(&host, &id, &pin, &name)
                .map(drop)
        })
    };
    let reply = approve(&f, &approver, &id, "4321").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    finishing.await.unwrap().unwrap();
    assert_eq!(
        saved(&f, "Browser TV").unwrap().perm,
        LATER_DEVICE_PERMISSION
    );

    let (id, entered) =
        f.host
            .webrtc
            .begin_pairing(&f.host, "4321", "Browser TV 2", PEER.parse().unwrap());
    let host = f.host.clone();
    let finishing = {
        let id = id.clone();
        tokio::spawn(async move {
            let (pin, name, _) = entered.await.unwrap();
            host.webrtc
                .finish_pairing(&host, &id, &pin, &name)
                .map(drop)
        })
    };
    approve(&f, &approver, &id, "0000")
        .await
        .assert_error(422, "wrong_pin");
    assert!(finishing.await.unwrap().is_err());
    assert!(saved(&f, "Browser TV 2").is_none());
}

#[test]
fn an_approval_carries_no_state_a_console_pin_would_touch() {
    // No approval registered: the console's pairings keep their permission.
    let f = Fixture::new();
    let state = f.host.paired.read().unwrap();
    let (perm, extra) = pairing::capped(&f.host, &state, "nobody", 7, Default::default()).unwrap();
    assert_eq!(perm, 7);
    assert!(extra.is_empty());
}

#[test]
fn the_notices_name_both_devices_and_cut_long_names() {
    let (title, text) = crate::state::pairing_approved_notice("Phone", "TV");
    assert_eq!(title, "Device paired");
    assert!(text.contains("Phone approved TV"), "{text}");
    let long = "x".repeat(100);
    let (_, text) = crate::state::pairing_approved_notice(&long, "TV");
    assert!(text.len() < 100, "{text}");
    let (_, text) = crate::state::pairing_ended_notice("TV");
    assert!(text.contains("Three wrong PINs for TV"), "{text}");
}
