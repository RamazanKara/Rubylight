//! Regression tests for `HostManager`, `HostStore`, `PollCancel`, pairing checks,
//! tombstone races, poll cancellation on network reset, and 3+ endpoint probing (#164).

use super::client_pool::HostClientPool;
use super::manager::HostManager;
use super::pairing::{PairSecret, execute_pair_checked};
use super::poll::PollCancel;
use super::tests::{read_http_head, reply_http_xml};
use super::*;
use crate::http::identity::tests::fixture;
use crate::http::{ClientIdentity, HostClient};
use nightjar_policy::hosts::HostReachState;
use std::net::TcpListener;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nightjar-hosts-reg-{tag}-{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn host_manager_clear_mutations_persist_across_reload() {
    let dir = temp_dir("clear-persist");
    let cert_pem = fixture().certificate_pem().to_owned();
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-clear".into();
    entry.shared.name = "Rig".into();
    entry.shared.alias = Some("Office".into());
    entry.shared.server_certificate = Some(cert_pem);
    entry.shared.internet_play = Some(r#"{"version":1}"#.into());
    entry.local_endpoint = EndpointSlot::new("192.168.1.20", 47989);
    entry.pinned_endpoint = EndpointSlot::new("100.64.0.20", 47989);
    entry.extra.insert("future".into(), serde_json::json!("preserved"));
    entry.sync_shared_from_slots();
    mgr.store().upsert(entry.clone()).unwrap();
    mgr.upsert_tuple(entry);

    mgr.pin_address("pc-clear", None).unwrap();
    mgr.rename_computer("pc-clear", None).unwrap();
    mgr.forget_certificate("pc-clear").unwrap();
    mgr.import_internet_play("pc-clear", None).unwrap();
    drop(mgr);

    let reopened = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    let snap = reopened.get_snapshot("pc-clear").expect("host still saved");
    assert_eq!(snap.pinned_endpoint, None);
    assert_eq!(snap.entry.pinned_endpoint, None);
    assert_eq!(snap.entry.shared.pinned_address, None);
    assert_eq!(snap.alias, None);
    assert_eq!(snap.entry.shared.alias, None);
    assert_eq!(snap.entry.shared.server_certificate, None);
    assert_eq!(snap.entry.shared.internet_play, None);
    assert_eq!(
        snap.entry.extra.get("future"),
        Some(&serde_json::json!("preserved"))
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stale_poll_cancel_after_pool_return_does_not_abort_subsequent_checkout() {
    let pool = HostClientPool::new(Arc::new(fixture()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut sock, _) = listener.accept().unwrap();
            reply_http_xml(
                &mut sock,
                r#"<root status_code="200"><hostname>Box</hostname><uniqueid>pc-pool</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort></root>"#,
            );
        }
    });

    let old_cancel = PollCancel::new();
    let (first, generation) = pool.take("127.0.0.1", port, 47984, None).unwrap();
    {
        let _guard = old_cancel.register(first.canceller());
        assert!(first.server_info_http(true).is_ok());
    }
    pool.give_back("127.0.0.1", port, None, generation, first);

    // Borrow the same pooled client again, then cancel the OLD scope: must not cancel `second`.
    let (second, generation2) = pool.take("127.0.0.1", port, 47984, None).unwrap();
    old_cancel.cancel();
    assert!(
        second.server_info_http(true).is_ok(),
        "cancelling an already-returned poll scope must not poison the next borrower"
    );
    pool.give_back("127.0.0.1", port, None, generation2, second);
    srv.join().unwrap();
}

#[test]
fn execute_pair_checked_does_not_trust_unauthenticated_http_pair_status() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Start an HTTPS server presenting a DIFFERENT certificate than the one pinned on `client`,
    // forcing `server_info_with_cert_check` to fall back to HTTP with `InfoPath::CertificateMismatch`.
    let other_host = ClientIdentity::generate().unwrap();
    let tls_cfg = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                other_host.certificate_der().to_vec(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                other_host.private_key_pkcs8().to_vec(),
            )),
        )
        .unwrap(),
    );
    let https_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let https_port = https_listener.local_addr().unwrap().port();
    let https_srv = std::thread::spawn(move || {
        if let Ok((mut tcp, _)) = https_listener.accept()
            && let Ok(mut conn) = rustls::ServerConnection::new(tls_cfg)
        {
            while conn.is_handshaking() {
                if conn.complete_io(&mut tcp).is_err() {
                    break;
                }
            }
        }
    });

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let saw_pair = Arc::new(AtomicBool::new(false));
    let saw_pair_srv = Arc::clone(&saw_pair);
    let srv = std::thread::spawn(move || {
        let (mut sock1, _) = listener.accept().unwrap();
        reply_http_xml(
            &mut sock1,
            &format!(
                r#"<root status_code="200"><hostname>Spoof</hostname><appversion>7.1.431.0</appversion><uniqueid>pc-spoof</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>1</PairStatus><HttpsPort>{https_port}</HttpsPort></root>"#
            ),
        );
        // After refusing the unauthenticated PairStatus=1 shortcut, pairing attempts /pair.
        let (mut sock2, _) = listener.accept().unwrap();
        let head = read_http_head(&mut sock2);
        if String::from_utf8_lossy(&head).contains("/pair?") {
            saw_pair_srv.store(true, Ordering::SeqCst);
        }
        use std::io::{Read, Write};
        let body = r#"<root status_code="200"><paired>0</paired></root>"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock2.write_all(reply.as_bytes());
        let _ = sock2.flush();
        let _ = sock2.shutdown(std::net::Shutdown::Write);
        let mut drain = [0u8; 64];
        while matches!(sock2.read(&mut drain), Ok(n) if n > 0) {}
    });

    let identity = Arc::new(fixture());
    let pinned_der = identity.certificate_der().to_vec();
    let client = HostClient::new("127.0.0.1", port, identity)
        .unwrap()
        .with_https_port(https_port);
    client.pin_server_certificate_der(Some(pinned_der));
    let outcome = execute_pair_checked(
        &client,
        &PairSecret::Pin("1234".into()),
        true,
        false,
        &[],
    );
    assert!(
        outcome.is_err(),
        "plain HTTP PairStatus=1 after cert mismatch must never short-circuit as already paired: {outcome:?}"
    );
    assert!(
        saw_pair.load(Ordering::SeqCst),
        "must fall through to /pair when serverinfo came over InfoPath::CertificateMismatch; outcome={outcome:?}"
    );
    srv.join().unwrap();
    https_srv.join().unwrap();
}

#[test]
fn removed_host_is_not_resurrected_by_mutations() {
    let dir = temp_dir("tombstone");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-gone".into();
    entry.shared.name = "Gone".into();
    entry.local_endpoint = EndpointSlot::new("192.168.1.99", 47989);
    entry.sync_shared_from_slots();
    mgr.store().upsert(entry.clone()).unwrap();
    mgr.upsert_tuple(entry);

    mgr.remove_computer("pc-gone").unwrap();
    mgr.pin_address("pc-gone", EndpointSlot::new("192.168.1.99", 47989))
        .unwrap();
    mgr.rename_computer("pc-gone", Some("Zombie")).unwrap();
    mgr.set_remote_endpoint("pc-gone", EndpointSlot::new("203.0.113.5", 47989))
        .unwrap();
    mgr.forget_certificate("pc-gone").unwrap();
    mgr.import_internet_play("pc-gone", Some(r#"{"version":1}"#))
        .unwrap();

    assert!(mgr.get_snapshot("pc-gone").is_none());
    assert!(mgr.store().get_by_uuid("pc-gone").is_none());
    assert_eq!(mgr.meta().get_alias("pc-gone"), None);
    assert_eq!(mgr.meta().get_pinned("pc-gone"), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reset_network_state_and_invalidate_cancel_blocked_poll_promptly() {
    for use_reset in [true, false] {
        let dir = temp_dir(if use_reset { "cancel-reset" } else { "cancel-inv" });
        let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
        mgr.set_stun_resolver(Arc::new(|_| None));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let srv = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let _ = accepted_tx.send(());
            // Hold the TCP socket open without answering until the test finishes.
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            drop(sock);
        });

        let mut entry = HostEntry::default();
        entry.shared.unique_id = "pc-stall".into();
        entry.shared.name = "Stall".into();
        entry.local_endpoint = EndpointSlot::new("127.0.0.1", port);
        entry.sync_shared_from_slots();
        mgr.upsert_tuple(entry);

        let poller_mgr = mgr.clone();
        let poller = std::thread::spawn(move || poller_mgr.poll_host("pc-stall", true));
        accepted_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("poll connected to stalling listener");

        let start = Instant::now();
        if use_reset {
            mgr.reset_network_state(true, mgr.now_ms() + 1);
        } else {
            mgr.invalidate_state("pc-stall");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "should cancel in-flight poll promptly instead of stalling ({elapsed:?})"
        );
        let _ = poller.join();
        let _ = release_tx.send(());
        let _ = srv.join();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn three_plus_endpoints_preserved_in_other_addresses_and_probed() {
    let dir = temp_dir("three-endpoints");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        reply_http_xml(
            &mut sock,
            r#"<root status_code="200"><hostname>MultiHome</hostname><uniqueid>pc-multi</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#,
        );
    });

    // First record has a working endpoint (`127.0.0.1`) in `manual_endpoint` plus LAN & WAN.
    let mut first = HostEntry::default();
    first.shared.unique_id = "pc-multi".into();
    first.shared.name = "MultiHome".into();
    first.shared.http_port = port;
    first.local_endpoint = EndpointSlot::new("192.0.2.10", port);
    first.manual_endpoint = EndpointSlot::new("127.0.0.1", port);
    first.remote_endpoint = EndpointSlot::new("198.51.100.10", port);
    first.sync_shared_from_slots();
    mgr.upsert_tuple(first);

    // Updating `manual_endpoint` to a Tailscale IP must preserve `127.0.0.1` in `other_addresses`.
    let mut update = HostEntry::default();
    update.shared.unique_id = "pc-multi".into();
    update.shared.http_port = port;
    update.manual_endpoint = EndpointSlot::new("100.64.10.20", port);
    let merged = mgr.upsert_tuple(update);
    assert!(
        merged
            .entry
            .shared
            .other_addresses
            .contains(&"127.0.0.1".to_owned()),
        "displaced endpoint must be preserved in other_addresses: {:?}",
        merged.entry.shared.other_addresses
    );

    // Polling must probe `other_addresses` and reach `127.0.0.1:port` even though the 3 primary
    // slots (LAN, Tailscale manual, WAN) are unreachable.
    assert!(mgr.poll_host("pc-multi", true));
    let online = mgr.get_snapshot("pc-multi").unwrap();
    assert_eq!(online.state, HostReachState::Online);
    assert_eq!(
        online.active_endpoint,
        EndpointSlot::new("127.0.0.1", port)
    );

    srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn meta_and_wake_stores_refuse_corrupt_overwrite_and_clear_guard_after_good_read() {
    let dir = temp_dir("store-guard-clear");

    // HostStore: corrupt `hosts.json` prevents migration from falsely completing, then valid JSON
    // clears unreadable state.
    let hosts = HostStore::open(&dir);
    std::fs::write(dir.join("hosts.json"), b"{corrupt").unwrap();
    assert!(hosts.import_legacy_if_needed(Vec::new()).is_err());
    assert!(!hosts.has_migrated_legacy());
    std::fs::write(dir.join("hosts.json"), b"[]").unwrap();
    assert!(hosts.all().is_empty());
    assert!(hosts.save_records::<HostEntry>(&[]).is_ok());

    // HostMetaStore: unreadable file refuses overwrite, good read clears guard.
    let meta = HostMetaStore::open(&dir);
    std::fs::write(meta.path(), b"not json").unwrap();
    assert!(meta.get_alias("u1").is_none());
    assert!(meta.set_alias("u1", Some("Rig")).is_err());
    std::fs::write(meta.path(), b"{}").unwrap();
    assert!(meta.get_alias("u1").is_none());
    assert!(meta.set_alias("u1", Some("Rig")).unwrap());
    assert_eq!(meta.get_alias("u1").as_deref(), Some("Rig"));

    // WakeTargetsStore: unreadable file refuses overwrite, NotFound/good read clears guard.
    let wol = crate::wol::WakeTargetsStore::open(&dir);
    std::fs::write(wol.path(), b"{bad").unwrap();
    assert!(wol.manual("u1").is_empty());
    assert!(wol.store_manual("u1", &["aa:bb:cc:dd:ee:01".into()]).is_err());
    std::fs::remove_file(wol.path()).unwrap();
    assert!(wol.manual("u1").is_empty());
    assert!(wol.store_manual("u1", &["aa:bb:cc:dd:ee:01".into()]).is_ok());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_invited_computer_preserves_internet_play_and_path_on_existing_host() {
    let dir = temp_dir("invite-existing");
    let cert_pem = fixture().certificate_pem().to_owned();
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();

    let mut existing = HostEntry::default();
    existing.shared.unique_id = "pc-inv".into();
    existing.shared.name = "Existing".into();
    existing.local_endpoint = EndpointSlot::new("192.168.1.50", 47989);
    existing.shared.server_certificate = Some(cert_pem.clone());
    existing.sync_shared_from_slots();
    mgr.store().upsert(existing.clone()).unwrap();
    mgr.upsert_tuple(existing);

    let mut invited = HostEntry::default();
    invited.shared.unique_id = "pc-inv".into();
    invited.shared.name = "Existing".into();
    invited.shared.server_certificate = Some(cert_pem);
    let reply = r#"{"version":1,"servers":[{"host":"relay.example","port":47989}]}"#;
    assert!(mgr.add_invited_computer(invited, reply).unwrap());

    let snap = mgr.get_snapshot("pc-inv").unwrap();
    assert_eq!(snap.entry.shared.internet_play.as_deref(), Some(reply));
    assert!(snap.internet_path.is_some());
    assert!(snap.paired);

    let saved = mgr.store().get_by_uuid("pc-inv").unwrap();
    assert_eq!(saved.shared.internet_play.as_deref(), Some(reply));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pinned_add_refuses_unpinned_impostor_when_https_verification_fails() {
    let dir = temp_dir("pinned-add-impostor");
    let cert_pem = fixture().certificate_pem().to_owned();
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // Unused closed port for HTTPS so pinned HTTPS re-poll fails cleanly.
    let closed_https = TcpListener::bind("127.0.0.1:0").unwrap();
    let https_port = closed_https.local_addr().unwrap().port();
    drop(closed_https);

    let mut known = HostEntry::default();
    known.shared.unique_id = "pc-known".into();
    known.shared.name = "RealRig".into();
    known.local_endpoint = EndpointSlot::new("10.0.0.99", 47989);
    known.shared.server_certificate = Some(cert_pem);
    known.sync_shared_from_slots();
    mgr.upsert_tuple(known);

    let srv = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut sock, _) = listener.accept().unwrap();
            reply_http_xml(
                &mut sock,
                &format!(
                    r#"<root status_code="200"><hostname>FakeRig</hostname><uniqueid>pc-known</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>{https_port}</HttpsPort><LocalIP>127.0.0.1</LocalIP></root>"#
                ),
            );
        }
    });

    let mut cand = HostEntry::default();
    cand.shared.http_port = port;
    cand.local_endpoint = EndpointSlot::new("127.0.0.1", port);
    cand.sync_shared_from_slots();
    assert!(mgr.add_computer_blocking(cand.clone()).is_none());

    let cert_sha = crate::http::certificate_sha256_der(fixture().certificate_der());
    let (outcome, snap) = mgr.add_computer_for_link(cand, cert_sha);
    assert_eq!(outcome, super::manager::LinkAddOutcome::Unreachable);
    assert!(snap.is_none());

    let still = mgr.get_snapshot("pc-known").unwrap();
    assert_eq!(still.entry.shared.name, "RealRig");
    assert_eq!(
        still.entry.local_endpoint,
        EndpointSlot::new("10.0.0.99", 47989)
    );
    srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn poll_commits_before_blocking_stun_drops_locks_for_listener_and_backs_off_stun() {
    let dir = temp_dir("poll-stun-backoff");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_allow_loopback_stun_for_test(true);

    let (stun_entered_tx, stun_entered_rx) = mpsc::channel();
    let online_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let online_flag = Arc::clone(&online_seen);
    let mgr_for_cb = mgr.clone();
    mgr.set_listener(Some(Arc::new(move |snap: &super::manager::HostSnapshot| {
        if snap.state == HostReachState::Online {
            // Re-entering public snapshot/removal paths inside listener must not deadlock.
            assert!(mgr_for_cb.get_snapshot(&snap.entry.shared.unique_id).is_some());
            online_flag.store(true, std::sync::atomic::Ordering::Release);
        }
    })));

    let stun_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stun_calls_cb = Arc::clone(&stun_calls);
    let online_in_stun = Arc::clone(&online_seen);
    mgr.set_stun_resolver(Arc::new(move |_| {
        stun_calls_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = stun_entered_tx.send(online_in_stun.load(std::sync::atomic::Ordering::Acquire));
        None
    }));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut sock, _) = listener.accept().unwrap();
            reply_http_xml(
                &mut sock,
                r#"<root status_code="200"><hostname>LanBox</hostname><uniqueid>pc-stun</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#,
            );
        }
    });

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-stun".into();
    entry.shared.name = "LanBox".into();
    entry.shared.http_port = port;
    entry.local_endpoint = EndpointSlot::new("127.0.0.1", port);
    entry.sync_shared_from_slots();
    mgr.upsert_tuple(entry);

    assert!(mgr.poll_host("pc-stun", true));
    assert!(
        stun_entered_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        "listener must see Online before blocking STUN resolver runs"
    );
    assert_eq!(stun_calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Second immediate poll hits STUN failure backoff and skips STUN.
    let _ = mgr.poll_host("pc-stun", true);
    assert_eq!(stun_calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn applist_poller_reports_updated_unchanged_failed_and_stops_promptly() {
    use super::applist::{ApplistPollOutcome, ApplistPollerHandle};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::io::{Read, Write};

    let host_id = fixture();
    let cert_der = host_id.certificate_der().to_vec();
    let tls_cfg = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    host_id.private_key_pkcs8().to_vec(),
                )),
            )
            .unwrap(),
    );

    let pool = Arc::new(HostClientPool::new(Arc::new(fixture())));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let srv = std::thread::spawn(move || {
        let xml = r#"<root status_code="200"><App><AppTitle>Alpha</AppTitle><ID>1</ID></App></root>"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
            xml.len()
        );
        // 1st & 2nd polls: valid TLS XML replies (Updated then Unchanged).
        for _ in 0..2 {
            if let Ok((mut s, _)) = listener.accept() {
                let mut conn = rustls::ServerConnection::new(Arc::clone(&tls_cfg)).unwrap();
                let mut tls = rustls::Stream::new(&mut conn, &mut s);
                let mut buf = [0u8; 512];
                let _ = tls.read(&mut buf);
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.flush();
            }
        }
        // 3rd poll: non-TLS close triggers Failed immediately without blocking 5s.
        if let Ok((mut s, _)) = listener.accept() {
            let _ = s.write_all(b"NOT_TLS\r\n");
        }
        // 4th poll: stalled socket to prove `stop()` returns promptly.
        if let Ok((_stalled, _)) = listener.accept() {
            std::thread::sleep(Duration::from_millis(600));
        }
    });

    let (tx, rx) = mpsc::channel();
    let handle = ApplistPollerHandle::start_with_outcomes(
        pool,
        "127.0.0.1".into(),
        port,
        port,
        cert_der,
        Box::new(move |ev| {
            let _ = tx.send(ev);
        }),
    );

    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        ApplistPollOutcome::Updated(ref u) if u.apps.len() == 1
    ));
    handle.poll_now();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        ApplistPollOutcome::Unchanged
    ));
    handle.poll_now();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        ApplistPollOutcome::Failed(_)
    ));

    handle.poll_now();
    std::thread::sleep(Duration::from_millis(40));
    let started = Instant::now();
    handle.stop();
    assert!(started.elapsed() < Duration::from_secs(1), "stop took {:?}", started.elapsed());
    let _ = srv.join();
}

#[test]
fn store_ignores_empty_unique_id_and_cleans_stale_tmp_files() {
    let dir = temp_dir("empty_id_and_stale_tmp");
    let store = HostStore::open(&dir);
    let mut empty = HostEntry::default();
    empty.shared.name = "Alpha".into();
    assert!(!store.upsert(empty.clone()).unwrap());
    assert!(!store.replace_entry(empty).unwrap());
    assert!(store.all().is_empty());

    let stale = dir.join("hosts.json.99999.0.tmp");
    std::fs::write(&stale, b"stale").unwrap();
    let mut valid = HostEntry::default();
    valid.shared.unique_id = "pc-alpha".into();
    valid.shared.name = "Alpha".into();
    valid.local_endpoint = EndpointSlot::new("10.0.0.1", 47989);
    valid.sync_shared_from_slots();
    // Fresh tmp (< STALE_TMP_AGE) stays untouched; an entry save only prunes aged tmps safely.
    assert!(store.upsert(valid).unwrap());
    assert_eq!(store.all().len(), 1);
    assert!(stale.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn parallel_poll_direct_cancellable_wakes_and_exits_immediately_on_cancel() {
    use super::poll::parallel_poll_direct_cancellable;
    use nightjar_policy::hosts::{CandidateSlot, PollErrorKind, ProbeCandidate};

    let l1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let p1 = l1.local_addr().unwrap().port();
    let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let p2 = l2.local_addr().unwrap().port();
    let (acc_tx, acc_rx) = mpsc::channel();
    let (rel_tx, rel_rx) = mpsc::channel();
    let srv1 = std::thread::spawn(move || {
        let (_s, _) = l1.accept().unwrap();
        let _ = acc_tx.send(());
        let _ = rel_rx.recv_timeout(Duration::from_secs(5));
    });
    let (rel2_tx, rel2_rx) = mpsc::channel();
    let srv2 = std::thread::spawn(move || {
        let (_s, _) = l2.accept().unwrap();
        let _ = rel2_rx.recv_timeout(Duration::from_secs(5));
    });

    let pool = Arc::new(HostClientPool::new(Arc::new(fixture())));
    let cancel = PollCancel::new();
    let candidates = vec![
        ProbeCandidate {
            slot: CandidateSlot::Local,
            address: "127.0.0.1".into(),
            port: p1,
            https_port: 0,
        },
        ProbeCandidate {
            slot: CandidateSlot::Manual,
            address: "127.0.0.1".into(),
            port: p2,
            https_port: 0,
        },
    ];
    let cancel_bg = cancel.clone();
    let trigger = std::thread::spawn(move || {
        let _ = acc_rx.recv_timeout(Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(20));
        let t0 = Instant::now();
        cancel_bg.cancel();
        t0
    });

    let outcome = parallel_poll_direct_cancellable(&pool, &candidates, None, None, true, Some(&cancel));
    let cancelled_at = trigger.join().unwrap();
    let wake_latency = cancelled_at.elapsed();
    let _ = rel_tx.send(());
    let _ = rel2_tx.send(());
    let _ = srv1.join();
    let _ = srv2.join();

    assert!(outcome.winner.is_none());
    assert_eq!(outcome.last_error, Some(PollErrorKind::Cancelled));
    assert!(
        wake_latency < Duration::from_millis(500),
        "parallel_poll_direct_cancellable took too long after cancel: {wake_latency:?}"
    );
}
