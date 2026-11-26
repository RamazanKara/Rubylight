//! Native API pairing rate limits, deny, browser pairing, and `approve_new_devices` grant tests.
use super::*;

#[tokio::test]
async fn after_a_wrong_pin_the_address_waits_and_another_one_does_not() {
    let f = Fixture::new();
    let approver = other_device(&f, u32::MAX, true);
    run(&f, &approver, "TV", "1111", "2222")
        .await
        .assert_error(422, "wrong_pin");
    // The look-alike asks again at once: the same PIN must not reach it.
    let (_, id) = asking(&f, "TV").await;
    assert_eq!(listed(&f, &approver).await, ["TV"]);
    approve(&f, &approver, &id, "2222")
        .await
        .assert_error(409, "pairing_cooldown");
    assert!(f.host.pins.lock().unwrap().contains_key(&id));
    assert!(f.host.approvals.lock().unwrap().is_idle());
    // Another address is not held up by it.
    let client = Identity::generate().unwrap();
    let request = device_request(&client, "Console", None);
    let host = f.host.clone();
    let elsewhere: std::net::IpAddr = "192.168.4.9".parse().unwrap();
    tokio::spawn(async move { do_pair(host, &request, elsewhere).await });
    let other = pairing_id("Console", elsewhere);
    until("the device's request", || {
        f.host.pins.lock().unwrap().contains_key(&other)
    })
    .await;
    approve(&f, &approver, &other, "1234")
        .await
        .assert_error(409, "pairing_unconfirmed");
    // When it has passed, the first address may be approved again.
    f.host.approvals.lock().unwrap().cool_down();
    approve(&f, &approver, &id, "2222")
        .await
        .assert_error(409, "pairing_unconfirmed");
}

#[test]
fn an_ipv6_prefix_counts_as_one_and_another_prefix_does_not() {
    let f = Fixture::new();
    let same = [
        "fd00:1:2:3::1",
        "fd00:1:2:3:aaaa:bbbb:cccc:dddd",
        "fd00:1:2:3::9",
    ];
    let mut approvals = f.host.approvals.lock().unwrap();
    for address in same {
        approvals.strike_at(address.parse().unwrap());
    }
    assert_eq!(approvals.wrong_pins_at("fd00:1:2:3::77"), 3);
    assert_eq!(approvals.wrong_pins_at("fd00:1:2:4::1"), 0);
    // An IPv4 peer counts the same however it reached the socket.
    approvals.strike_at("::ffff:192.168.4.9".parse().unwrap());
    assert_eq!(approvals.wrong_pins_at("192.168.4.9"), 1);
}

#[tokio::test]
async fn an_approvers_revocation_still_counts_after_the_approval_is_dropped() {
    let f = Fixture::new();
    let approver = other_device(&f, PLAY_DEVICE_PERMISSION, true);
    let (client, id) = asking(&f, "TV").await;
    let (_, finished) = tokio::join!(approve(&f, &approver, &id, "1234"), async {
        until("the handshake to start", || {
            f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        pairing::failed(&f.host, &id);
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
    assert!(saved(&f, "TV").is_none());
}

#[test]
fn the_approver_comes_from_the_session_and_a_missing_one_fails_closed() {
    let f = Fixture::new();
    other_device(&f, LATER_DEVICE_PERMISSION, true);
    let state = f.host.paired.read().unwrap();
    let granted = pairing::capped(
        &state,
        Some("other-device"),
        PLAY_DEVICE_PERMISSION,
        Default::default(),
    )
    .unwrap();
    assert_eq!(granted.perm, LATER_DEVICE_PERMISSION);
    assert_eq!(granted.approver.unwrap().name, "Other device");
    assert!(pairing::capped(&state, Some("gone"), 7, Default::default()).is_err());
}

#[tokio::test]
async fn a_device_that_goes_quiet_counts_and_may_still_finish_under_the_cap() {
    let f = Fixture::new();
    let approver = other_device(&f, LATER_DEVICE_PERMISSION, true);
    let (client, id) = asking(&f, "TV").await;
    approve(&f, &approver, &id, "1234")
        .await
        .assert_error(409, "pairing_unconfirmed");
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 1);
    finish_pairing(&f, &client, "TV", "1234").await.unwrap();
    assert_eq!(saved(&f, "TV").unwrap().perm, LATER_DEVICE_PERMISSION);
    assert_eq!(f.host.approvals.lock().unwrap().wrong_pins_at(PEER), 0);
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
    until("the device's request", || {
        f.host.pins.lock().unwrap().contains_key(&id)
    })
    .await;
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
        clients(&f.host).begin_pairing(&f.host, "4321", "Browser TV", PEER.parse().unwrap());
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
        clients(&f.host).begin_pairing(&f.host, "4321", "Browser TV", PEER.parse().unwrap());
    let host = f.host.clone();
    let finishing = {
        let id = id.clone();
        tokio::spawn(async move {
            let (pin, name, access, approver) = entered.await.unwrap();
            assert_eq!(access, None, "a browser device takes no access choice");
            clients(&host)
                .finish_pairing(
                    &host,
                    &id,
                    &pin,
                    &name,
                    approver.as_deref(),
                    PEER.parse().unwrap(),
                )
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
        clients(&f.host).begin_pairing(&f.host, "4321", "Browser TV 2", PEER.parse().unwrap());
    let host = f.host.clone();
    let finishing = {
        let id = id.clone();
        tokio::spawn(async move {
            let (pin, name, _, approver) = entered.await.unwrap();
            clients(&host)
                .finish_pairing(
                    &host,
                    &id,
                    &pin,
                    &name,
                    approver.as_deref(),
                    PEER.parse().unwrap(),
                )
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
    // No approver: the console's pairings keep their permission.
    let f = Fixture::new();
    let state = f.host.paired.read().unwrap();
    let granted = pairing::capped(&state, None, 7, Default::default()).unwrap();
    assert_eq!(granted.perm, 7);
    assert!(granted.extra.is_empty());
    assert!(granted.approver.is_none());
}

#[test]
fn the_notices_name_both_devices_and_cut_long_names() {
    let (title, text) = butterpollo_stream::state::pairing_approved_notice("Phone", "TV");
    assert_eq!(title, "Device paired");
    assert!(text.contains("Phone approved TV"), "{text}");
    let long = "x".repeat(100);
    let (_, text) = butterpollo_stream::state::pairing_approved_notice(&long, "TV");
    assert!(text.len() < 100, "{text}");
    // A name from before they were refused can't turn the notice around.
    let (_, text) =
        butterpollo_stream::state::pairing_approved_notice("Phone\u{202e}", "\u{2067}TV");
    assert!(text.contains("Phone approved TV."), "{text}");
    let (_, text) = butterpollo_stream::state::pairing_ended_notice("192.168.4.9".parse().unwrap());
    assert!(text.contains("Three wrong PINs from 192.168.4.9"), "{text}");
}

/// A device paired with the console's one-time PIN, nobody approving.
async fn pair_from_the_console(f: &Fixture, name: &str) -> Client {
    let client = Identity::generate().unwrap();
    f.host.otp.lock().unwrap().typed = Some(one_time_pin("1234", "phrase", None));
    let request = device_request(&client, name, Some(("1234", "phrase")));
    do_pair(f.host.clone(), &request, PEER.parse().unwrap())
        .await
        .unwrap();
    finish_pairing(f, &client, name, "1234").await.unwrap();
    saved(f, name).unwrap()
}
/// Sets or clears "Approve new devices" on the paired device `uuid`.
fn grant(f: &Fixture, uuid: &str, on: bool) {
    let mut paired = f.host.paired.write().unwrap();
    let extra = &mut paired
        .clients
        .iter_mut()
        .find(|c| c.uuid == uuid)
        .unwrap()
        .extra;
    if on {
        extra.insert("pairing_approve".into(), true.into());
    } else {
        extra.remove("pairing_approve");
    }
}

#[tokio::test]
async fn the_first_device_may_approve_and_the_second_may_not() {
    let f = Fixture::new();
    let first = pair_from_the_console(&f, "Phone").await;
    assert_eq!(first.perm, FIRST_DEVICE_PERMISSION);
    assert!(first.allows_pairing_approve());
    assert!(!first.allows_host_control());
    let second = pair_from_the_console(&f, "Tablet").await;
    assert!(!second.extra.contains_key("pairing_approve"));
    assert!(!second.may_approve_pairing());
    // What pairing gave, and nothing more, is still there after a restart.
    let reloaded = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(reloaded.clients[0].allows_pairing_approve());
    assert!(!reloaded.clients[1].may_approve_pairing());
}

#[tokio::test]
async fn devices_paired_before_the_upgrade_stay_as_they_were() {
    let f = Fixture::new();
    // As an older host saved it: everything, and no such key.
    let existing = f.client(FIRST_DEVICE_PERMISSION);
    let connection = f.connection(true);
    let read: Capabilities =
        serde_json::from_slice(&api(&f, &connection, "GET", "/capabilities").await.body).unwrap();
    assert!(!read.permissions.pairing_approve && !read.permissions.host_control);
    api(&f, &connection, "GET", "/pairing/requests")
        .await
        .assert_error(403, "permission_required");
    let next = pair_from_the_console(&f, "Tablet").await;
    assert!(!next.may_approve_pairing());
    let after = saved(&f, &existing.name).unwrap();
    assert_eq!((after.perm, after.extra), (existing.perm, existing.extra));
}

#[tokio::test]
async fn approve_new_devices_opens_the_three_routes_and_no_other_host_control_action() {
    let s = Setup::new();
    grant(&s.f, &s.client.uuid, true);
    let read: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(read.permissions.pairing_approve && !read.permissions.host_control);
    assert!(listed(&s.f, &s.connection).await.is_empty());
    let reply = run(&s.f, &s.connection, "TV", "1234", "1234").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let (_, id) = asking(&s.f, "Console").await;
    let reply = send(
        &s.f,
        true,
        &s.connection,
        "POST",
        &format!("{PREFIX}/pairing/deny"),
        None,
        serde_json::json!({"id": id}).to_string(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let _stream = s.streaming();
    for path in [
        "/session/close-game",
        "/radeon/settings",
        "/radeon/restore",
        "/update/install",
    ] {
        s.post(path)
            .await
            .assert_error(403, "host_control_required");
    }
    assert!(s.game_running());
    assert_eq!(s.state(), SessionState::Streaming);
}

#[tokio::test]
async fn an_approved_device_never_inherits_approve_new_devices() {
    for host_control in [false, true] {
        let f = Fixture::new();
        let approver = other_device(&f, ALL, host_control);
        grant(&f, "other-device", !host_control);
        let reply = run(&f, &approver, "TV", "1234", "1234").await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        let tv = saved(&f, "TV").unwrap();
        assert!(!tv.extra.contains_key("pairing_approve"));
        assert!(!tv.may_approve_pairing());
    }
}

#[tokio::test]
async fn approve_new_devices_lost_during_the_handshake_refuses_the_new_device() {
    let f = Fixture::new();
    let approver = other_device(&f, ALL, false);
    grant(&f, "other-device", true);
    let (client, id) = asking(&f, "TV").await;
    let _ = tokio::join!(approve(&f, &approver, &id, "1234"), async {
        until("the PIN to reach the handshake", || {
            !f.host.pins.lock().unwrap().contains_key(&id)
                && f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        grant(&f, "other-device", false);
        let _ = finish_pairing(&f, &client, "TV", "1234").await;
    });
    assert!(saved(&f, "TV").is_none());
}

#[tokio::test]
async fn an_ipv6_prefix_holds_two_pending_requests_whatever_address_it_rotates_to() {
    let f = Fixture::new();
    for (name, address) in [("TV", "fd00:1:2:3::1"), ("Tablet", "fd00:1:2:3::2")] {
        let client = Identity::generate().unwrap();
        let request = device_request(&client, name, None);
        let host = f.host.clone();
        let peer: std::net::IpAddr = address.parse().unwrap();
        tokio::spawn(async move { do_pair(host, &request, peer).await });
        let id = pairing_id(name, peer);
        until("the device's request", || {
            f.host.pins.lock().unwrap().contains_key(&id)
        })
        .await;
    }
    // A third privacy address of the same /64 is refused at once instead of waiting for a PIN.
    let client = Identity::generate().unwrap();
    let rotated: std::net::IpAddr = "fd00:1:2:3:aaaa:bbbb:cccc:3".parse().unwrap();
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        do_pair(
            f.host.clone(),
            &device_request(&client, "Phone", None),
            rotated,
        ),
    )
    .await
    .expect("the request is refused, not left waiting for a PIN")
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("pairing request limit reached for this address"),
        "{refused:#}"
    );
    // Another prefix is not held up by it.
    let request = device_request(&client, "Phone", None);
    let host = f.host.clone();
    let elsewhere: std::net::IpAddr = "fd00:1:2:4::3".parse().unwrap();
    tokio::spawn(async move { do_pair(host, &request, elsewhere).await });
    let id = pairing_id("Phone", elsewhere);
    until("the other prefix's request", || {
        f.host.pins.lock().unwrap().contains_key(&id)
    })
    .await;
}
