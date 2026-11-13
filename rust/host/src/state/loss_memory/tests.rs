use super::*;
use crate::state::test_support::Fixture;
use butterpollo_core::{config::Config, rtsp::Negotiated, session::Role};

fn session(f: &Fixture, client: &Client, address: &str, codec: u8) -> Arc<Session> {
    let mut launch = f.launch(client.clone(), Role::Stream);
    launch.peer = address.parse().unwrap();
    Session::new(
        launch,
        Negotiated {
            codec,
            ..Default::default()
        },
    )
}
/// Sustained loss: three distinct frames the client could not repair.
fn loss(s: &Session) {
    for frame in 10..=12 {
        s.keyframe_loss.lock().unwrap().unrecoverable(
            frame,
            frame,
            Duration::from_millis(16),
            Instant::now(),
        );
    }
}
/// What a rough start looks like: the stream's first keyframe is lost.
fn rough_start(s: &Session) {
    let mut loss = s.keyframe_loss.lock().unwrap();
    // The first keyframe went out a second ago, so the client's reports
    // count: it is lost.
    loss.sending_at(1, true, Instant::now() - Duration::from_secs(1));
    loss.unrecoverable(1, 1, Duration::from_millis(16), Instant::now());
}
fn seed(f: &Fixture, client: &Client) {
    let s = session(f, client, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    assert!(!s.keyframe_loss_active());
    loss(&s);
    assert!(s.keyframe_loss_active());
    drop(guard);
}

#[test]
fn fresh_loss_persists_and_an_inherited_latch_does_not_renew_it() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    let initial = read(&PairedState::load(&f.host.paired_path).unwrap(), now());
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    assert_eq!(
        s.keyframe_loss_reason(),
        Some("remembered from the previous session")
    );
    assert_eq!(
        s.keyframe_loss_rate_bps(800_000_000, 60_000_000),
        60_000_000
    );
    assert!(s.keyframe_loss_observed_at().is_none());
    drop(guard);
    let saved = read(&PairedState::load(&f.host.paired_path).unwrap(), now());
    assert_eq!(saved[0].observed_at, initial[0].observed_at);
    assert_eq!(saved[0].revision, initial[0].revision);
    assert_eq!(saved[0].remembered_sessions, 1);
    // The count survives a restart: the next session is the probe.
    let reopened = Host::load(f.host.directory.clone(), f.host.assets.clone(), None).unwrap();
    let next = session(&f, &c, "192.0.2.1", 0);
    let guard = reopened.configure_keyframe_loss(&next, true);
    assert!(!next.keyframe_loss_active());
    assert!(guard.token.as_ref().unwrap().probe_revision.is_some());
    drop(guard);
}

#[test]
fn a_second_screen_keeps_and_reads_its_devices_history() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let screen = c.clone().second_screen();
    // Loss on the second screen is the device's.
    seed(&f, &screen);
    let saved = read(&PairedState::load(&f.host.paired_path).unwrap(), now());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].client_id, c.uuid);
    // And the second screen starts from it; the device's next session is the probe.
    let s = session(&f, &screen, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    assert_eq!(
        s.keyframe_loss_reason(),
        Some("remembered from the previous session")
    );
    drop(guard);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    assert!(guard.token.as_ref().unwrap().probe_revision.is_some());
    drop(guard);
}

#[test]
fn a_rough_start_is_not_remembered_but_sustained_loss_is() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    // The first keyframe lost, nothing since: provisional, forgotten at
    // the end of the stream, and the next session starts at the route rate.
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    rough_start(&s);
    assert!(s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_some());
    assert!(s.keyframe_loss_sustained_at().is_none());
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
    let next = session(&f, &c, "192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&next, true));
    assert!(!next.keyframe_loss_active());
    // The same stream losing frames after that is sustained: remembered.
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    rough_start(&s);
    loss(&s);
    assert!(s.keyframe_loss_sustained_at().is_some());
    drop(guard);
    assert_eq!(
        f.host.keyframe_loss_routes()[&c.uuid]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let next = session(&f, &c, "192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&next, true));
    assert!(next.keyframe_loss_active());
}

#[test]
fn loss_after_a_roam_is_not_remembered_for_the_launch_address() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    s.roamed("198.51.100.4".parse().unwrap(), Instant::now());
    loss(&s);
    assert!(s.keyframe_loss_sustained_at().is_some());
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
    let next = session(&f, &c, "192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&next, true));
    assert!(!next.keyframe_loss_active());
}

#[test]
fn a_probe_that_had_a_rough_start_does_not_forget_the_history() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    for rough in [true, false] {
        seed(&f, &c);
        for _ in 1..PROBE_EVERY {
            drop(
                f.host
                    .configure_keyframe_loss(&session(&f, &c, "192.0.2.1", 0), true),
            );
        }
        let s = session(&f, &c, "192.0.2.1", 0);
        let mut guard = f.host.configure_keyframe_loss(&s, true);
        assert!(guard.token.as_ref().unwrap().probe_revision.is_some());
        // Long and busy enough to count as a probe.
        guard.started = Instant::now() - Duration::from_secs(10);
        s.stats
            .frames
            .store(600, std::sync::atomic::Ordering::Relaxed);
        if rough {
            rough_start(&s);
        }
        drop(guard);
        assert_eq!(f.host.keyframe_loss_routes().is_empty(), !rough);
        f.host.reset_keyframe_loss(&c.uuid).unwrap();
    }
}

#[test]
fn records_without_a_kind_are_ignored_and_pruned() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    // What every release before this one wrote: no kind, no interface.
    let old = json!([{
        "client_id": c.uuid, "address": "192.0.2.1", "observed_at": now(),
        "revision": "old", "remembered_sessions": 0
    }]);
    f.host.paired.write().unwrap().document["root"][FIELD] = old;
    assert!(f.host.keyframe_loss_routes().is_empty());
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    assert!(!s.keyframe_loss_active());
    // Fresh sustained loss replaces it on disk.
    loss(&s);
    drop(guard);
    let saved = PairedState::load(&f.host.paired_path).unwrap();
    let routes = saved.document["root"][FIELD].as_array().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0]["kind"], "reported");
    assert_ne!(routes[0]["revision"], "old");
}

#[test]
fn an_older_sustained_record_is_not_read() {
    // What rc.4 wrote, decoder re-requests and send loss included.
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let old = json!([{
        "client_id": c.uuid, "address": "192.0.2.1", "observed_at": now(),
        "revision": "rc4", "remembered_sessions": 0, "kind": "sustained"
    }]);
    f.host.paired.write().unwrap().document["root"][FIELD] = old;
    assert!(f.host.keyframe_loss_routes().is_empty());
    let s = session(&f, &c, "192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&s, true));
    assert!(!s.keyframe_loss_active());
}

#[test]
fn send_loss_alone_is_not_remembered() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    s.keyframe_loss_bps(800_000_000, 60_000_000, 1, true);
    for frame in 2..=40 {
        s.keyframe_loss_bps(800_000_000, 60_000_000, frame, false);
    }
    // A keyframe and three frames the host's socket refused.
    s.keyframe_loss_bps(800_000_000, 60_000_000, 41, true);
    s.keyframe_abandoned(41);
    for frame in 42..=44 {
        s.keyframe_loss_bps(800_000_000, 60_000_000, frame, false);
        s.keyframe_abandoned(frame);
    }
    assert!(s.keyframe_loss_active());
    assert!(s.keyframe_loss_sustained_at().is_none());
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
    let next = session(&f, &c, "192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&next, true));
    assert!(!next.keyframe_loss_active());
}

#[test]
fn another_host_interface_or_client_link_starts_fresh() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let on = |interface: u64, hint: u32| {
        let s = Session::new(
            {
                let mut launch = f.launch(c.clone(), butterpollo_core::session::Role::Stream);
                launch.peer = "192.0.2.1".parse().unwrap();
                launch
            },
            Negotiated {
                pyrowave_client_link_mbps: hint,
                ..Default::default()
            },
        );
        let guard = f.host.configure_keyframe_loss_on(&s, true, interface);
        (s, guard)
    };
    let (s, guard) = on(7, 1000);
    assert!(!s.keyframe_loss_active());
    loss(&s);
    drop(guard);
    // The same interface and link: remembered.
    let (s, guard) = on(7, 1000);
    assert!(s.keyframe_loss_active());
    drop(guard);
    // A host that switched NIC or Wi-Fi, a client that moved to another
    // port or stopped sending a hint: no history, and it keeps its own.
    for (interface, hint) in [(8, 1000), (7, 100), (7, 0), (0, 1000)] {
        let (s, guard) = on(interface, hint);
        assert!(!s.keyframe_loss_active(), "{interface} {hint}");
        drop(guard);
    }
    let (s, guard) = on(8, 1000);
    loss(&s);
    drop(guard);
    // Both are kept, each for its own path.
    let saved = read(&f.host.paired.read().unwrap(), now());
    assert_eq!(saved.len(), 2);
    for (interface, hint) in [(7, 1000), (8, 1000)] {
        assert!(
            saved
                .iter()
                .any(|r| r.interface == interface && r.client_link_mbps == hint)
        );
    }
}

#[test]
fn a_host_side_keyframe_and_the_clients_requests_are_not_remembered_for_a_day() {
    // Alt+Tab or a bitrate change on a clean link: the host sends a
    // keyframe nobody asked for and the client asks for two of its own.
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    s.keyframe_loss_bps(800_000_000, 60_000_000, 1, true);
    for frame in 2..=40 {
        s.keyframe_loss_bps(800_000_000, 60_000_000, frame, false);
    }
    s.keyframe_loss_bps(800_000_000, 60_000_000, 41, true);
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(800_000_000, 60_000_000, 42, true);
    s.request_recovery_keyframe();
    assert!(!s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_none());
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
    let next = session(&f, &c, "192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&next, true));
    assert!(!next.keyframe_loss_active());
}

#[test]
fn a_stream_start_with_repeated_client_requests_is_not_remembered_for_a_day() {
    // The client asks for a keyframe, gets frame 1, asks again and gets
    // frame 11: seen at the start of real streams, no packet lost.
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(800_000_000, 60_000_000, 1, true);
    for frame in 2..=10 {
        s.keyframe_loss_bps(800_000_000, 60_000_000, frame, false);
    }
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(800_000_000, 60_000_000, 11, true);
    s.request_recovery_keyframe();
    assert!(!s.keyframe_loss_active());
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
}

#[test]
fn the_session_after_one_remembered_one_probes_and_a_successful_probe_forgets_its_route() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    for _ in 1..PROBE_EVERY {
        let s = session(&f, &c, "192.0.2.1", 0);
        let guard = f.host.configure_keyframe_loss(&s, true);
        assert!(s.keyframe_loss_active());
        drop(guard);
    }
    let s = session(&f, &c, "192.0.2.1", 0);
    let mut guard = f.host.configure_keyframe_loss(&s, true);
    assert!(!s.keyframe_loss_active());
    let token = guard.token.take().unwrap();
    assert!(token.probe_revision.is_some());
    f.host
        .finish_keyframe_loss(token, None, true, now())
        .unwrap();
    assert!(f.host.keyframe_loss_routes().is_empty());
}

#[test]
fn a_short_probe_does_not_clear_history_and_fresh_loss_restarts_the_cycle() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    // One remembered session, then a probe too short to count: the
    // history stays and the cycle starts again.
    for _ in 0..PROBE_EVERY {
        let s = session(&f, &c, "192.0.2.1", 0);
        drop(f.host.configure_keyframe_loss(&s, true));
    }
    assert!(!f.host.keyframe_loss_routes().is_empty());
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    assert!(s.keyframe_loss_active());
    loss(&s);
    drop(guard);
    let saved = read(&f.host.paired.read().unwrap(), now());
    assert_eq!(saved[0].remembered_sessions, 0);
}

#[test]
fn history_is_bound_to_authenticated_identity_and_canonical_address() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    let mapped = session(&f, &c, "::ffff:192.0.2.1", 0);
    drop(f.host.configure_keyframe_loss(&mapped, true));
    assert!(mapped.keyframe_loss_active());
    let other_address = session(&f, &c, "192.0.2.2", 0);
    drop(f.host.configure_keyframe_loss(&other_address, true));
    assert!(!other_address.keyframe_loss_active());
    for (id, cert) in [
        ("other-id", c.cert.as_str()),
        (c.uuid.as_str(), "other-cert"),
    ] {
        let mut other = c.clone();
        other.uuid = id.into();
        other.cert = cert.into();
        let s = session(&f, &other, "192.0.2.1", 0);
        let guard = f.host.configure_keyframe_loss(&s, true);
        assert!(!s.keyframe_loss_active());
        loss(&s);
        drop(guard);
    }
    assert_eq!(read(&f.host.paired.read().unwrap(), now()).len(), 1);
}

#[test]
fn expiry_and_clock_rollback_restore_the_normal_rate() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    let at = now();
    for observed_at in [at - RETENTION, at + 60] {
        f.host.paired.write().unwrap().document["root"][FIELD][0]["observed_at"] =
            json!(observed_at);
        let s = session(&f, &c, "192.0.2.1", 0);
        drop(f.host.configure_keyframe_loss(&s, true));
        assert!(!s.keyframe_loss_active());
        assert!(f.host.keyframe_loss_routes().is_empty());
    }
}

#[test]
fn explicit_policies_and_pyrowave_neither_inherit_nor_consume_probes() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    for text in [
        "keyframe_loss_pacing=false",
        "pacing_max_bitrate_kbps=80000",
        "host_adaptive_pacing=true",
        "host_adaptive_bitrate=true",
    ] {
        let config = Config::parse(text).unwrap();
        let s = session(&f, &c, "192.0.2.1", 0);
        let guard = f
            .host
            .configure_keyframe_loss(&s, config.keyframe_loss_pacing(0, false));
        loss(&s);
        assert!(!s.keyframe_loss_active(), "{text}");
        drop(guard);
    }
    let s = session(&f, &c, "192.0.2.1", 3);
    let guard = f.host.configure_keyframe_loss(&s, true);
    loss(&s);
    assert!(!s.keyframe_loss_active());
    drop(guard);
    assert_eq!(
        read(&f.host.paired.read().unwrap(), now())[0].remembered_sessions,
        0
    );
}

#[test]
fn a_wireless_client_stream_the_controller_owns_leaves_remembered_routes_alone() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    let before = read(&f.host.paired.read().unwrap(), now());
    assert_eq!(before.len(), 1);
    let mut launch = f.launch(c.clone(), butterpollo_core::session::Role::Stream);
    launch.peer = "192.0.2.1".parse().unwrap();
    let s = Session::new(
        launch,
        Negotiated {
            codec: 1,
            client_wireless: true,
            ..Default::default()
        },
    );
    // The default configuration: the adaptive controller owns this stream.
    let keyframe_loss = crate::video_send::recovery::configure(&s, &Config::default());
    assert!(s.automatic_pacing_enabled() && !keyframe_loss);
    let guard = f.host.configure_keyframe_loss(&s, keyframe_loss);
    assert!(!s.keyframe_loss_active());
    loss(&s);
    assert!(!s.keyframe_loss_active());
    drop(guard);
    let after = read(&f.host.paired.read().unwrap(), now());
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].remembered_sessions, 0);
    assert_eq!(after[0].observed_at, before[0].observed_at);
}

#[test]
fn resetting_fences_existing_sessions_and_a_probe_cannot_erase_new_loss() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    loss(&s);
    f.host.reset_keyframe_loss(&c.uuid).unwrap();
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
    seed(&f, &c);
    for _ in 1..PROBE_EVERY {
        drop(
            f.host
                .configure_keyframe_loss(&session(&f, &c, "192.0.2.1", 0), true),
        );
    }
    let mut probe = f
        .host
        .configure_keyframe_loss(&session(&f, &c, "192.0.2.1", 0), true);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    loss(&s);
    drop(guard);
    f.host
        .finish_keyframe_loss(probe.token.take().unwrap(), None, true, now())
        .unwrap();
    assert!(!f.host.keyframe_loss_routes().is_empty());
}

#[test]
fn simultaneous_route_saves_preserve_each_other_and_device_settings() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    let guards: Vec<_> = (1..=8)
        .map(|last| {
            let s = session(&f, &c, &format!("192.0.2.{last}"), 0);
            let guard = f.host.configure_keyframe_loss(&s, true);
            loss(&s);
            guard
        })
        .collect();
    std::thread::scope(|scope| {
        for guard in guards {
            scope.spawn(move || drop(guard));
        }
        scope.spawn(|| {
            let mut state = f.host.paired.write().unwrap();
            state.clients[0]
                .extra
                .insert("custom".into(), json!("keep me"));
            state.save(&f.host.paired_path).unwrap();
        });
    });
    let saved = PairedState::load(&f.host.paired_path).unwrap();
    assert_eq!(read(&saved, now()).len(), 8);
    assert_eq!(saved.clients[0].extra["custom"], "keep me");
}

#[test]
fn unpair_is_idempotent_and_an_old_stream_cannot_recreate_repaired_history() {
    let f = Fixture::new();
    let c = f.client(0x071f1f00);
    seed(&f, &c);
    let s = session(&f, &c, "192.0.2.1", 0);
    let guard = f.host.configure_keyframe_loss(&s, true);
    loss(&s);
    f.host.unpair_with_network_history(Some(&c.uuid)).unwrap();
    f.host.unpair_with_network_history(Some(&c.uuid)).unwrap();
    f.client(0x071f1f00);
    drop(guard);
    assert!(f.host.keyframe_loss_routes().is_empty());
    seed(&f, &c);
    f.host.unpair_with_network_history(None).unwrap();
    let saved = PairedState::load(&f.host.paired_path).unwrap();
    assert!(saved.clients.is_empty());
    assert_eq!(saved.document["root"][FIELD], json!([]));
}
