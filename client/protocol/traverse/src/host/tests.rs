//! Host rendezvous service unit and loopback tests.

use super::*;

fn config() -> HostConfig {
    HostConfig {
        ports: Ports::default(),
        service: "127.0.0.1".parse().unwrap(),
        per_device_source: false,
        bind: "127.0.0.1:0".parse().unwrap(),
        stun_servers: Vec::new(),
        bootstrap: Vec::new(),
    }
}

/// The host's paths, once `count` of them are there (a punch starts its path on a thread of
/// its own), or after two seconds.
fn paths(host: &HostService, count: usize) -> Vec<PathStatus> {
    for _ in 0..100 {
        let paths = host.status().paths;
        if paths.len() >= count && paths.iter().all(|path| path.connected) {
            return paths;
        }
        thread::sleep(Duration::from_millis(20));
    }
    host.status().paths
}

#[test]
fn a_device_that_is_not_trying_is_looked_for_less_often() {
    let device = Device {
        id: "a".into(),
        name: "Phone".into(),
        secret: [3; 32],
    };
    let keys = Keys::new(device.secret);
    let mut state = DeviceState {
        tag: keys.device_tag(&[1; 16]),
        keys,
        device,
        published: None,
        last_poll: None,
        polling: false,
        heard: None,
        answered: None,
    };
    let now = 3_600_000;
    assert_eq!(poll_interval(&state, &[], &[], now), Some(POLL_IDLE));
    // A path that is still being punched, or a record that answered lately, keeps it fast.
    assert_eq!(
        poll_interval(&state, &[state.tag], &[], now),
        Some(POLL_EVERY)
    );
    state.heard = Some(now - ACTIVE_FOR + 1);
    assert_eq!(poll_interval(&state, &[], &[], now), Some(POLL_EVERY));
    state.heard = Some(now - ACTIVE_FOR);
    assert_eq!(poll_interval(&state, &[], &[], now), Some(POLL_IDLE));
    // A connected path that is heard from needs no look at all.
    assert_eq!(poll_interval(&state, &[state.tag], &[state.tag], now), None);
    state.heard = Some(now);
    assert_eq!(poll_interval(&state, &[state.tag], &[state.tag], now), None);
}

/// The loop's once-a-second prune never write-locks the map while every path is open, so the
/// receive thread's per-datagram read lock is never queued behind it; and `receive` holds no
/// lock through a tunnel's handling.
#[test]
fn pruning_paths_leaves_the_receive_lock_alone_unless_one_closed() {
    let host = HostService::start(config()).unwrap();
    let device = Device {
        id: "a".into(),
        name: "Phone".into(),
        secret: [3; 32],
    };
    host.set_devices(vec![device.clone()]);
    let path = Keys::new(device.secret).path(&host.session(), &[9; 16]);
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut punch = wire::Sealer::new(&path.client_to_host, path.tag, [9; 16]).seal(
        wire::Kind::Punch,
        0,
        &[],
    );
    host.receive(&mut punch, probe.local_addr().unwrap());
    assert_eq!(paths(&host, 1).len(), 1);

    // A reader (a datagram being looked up) holds the map: the prune must not wait for it.
    let reader = host.shared.tunnels.read().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let pruning = host.clone();
    thread::spawn(move || {
        pruning.prune_closed();
        let _ = done.send(());
    });
    assert!(
        finished.recv_timeout(Duration::from_secs(5)).is_ok(),
        "the prune waited for a reader with every path open"
    );
    drop(reader);

    // A closed path is dropped, once the readers are gone.
    for (_, tunnel) in host.shared.tunnels.read().unwrap().values() {
        tunnel.close("test");
    }
    host.prune_closed();
    assert!(host.shared.tunnels.read().unwrap().is_empty());
    host.stop();
}

/// A device whose path is connected and heard from is not looked for in the DHT; one with
/// no path is.
#[test]
fn a_connected_path_is_not_polled_but_an_absent_device_is() {
    let host = HostService::start(config()).unwrap();
    let phone = Device {
        id: "a".into(),
        name: "Phone".into(),
        secret: [3; 32],
    };
    let tablet = Device {
        id: "b".into(),
        name: "Tablet".into(),
        secret: [5; 32],
    };
    host.set_devices(vec![phone.clone(), tablet.clone()]);
    let path = Keys::new(phone.secret).path(&host.session(), &[9; 16]);
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut punch = wire::Sealer::new(&path.client_to_host, path.tag, [9; 16]).seal(
        wire::Kind::Punch,
        0,
        &[],
    );
    host.receive(&mut punch, probe.local_addr().unwrap());
    assert_eq!(paths(&host, 1).len(), 1);

    // The loop's first round may have looked for both before the path was there.
    for state in host.shared.devices.lock().unwrap().iter_mut() {
        state.last_poll = None;
        state.polling = false;
    }
    host.serve_devices();
    let polled = |host: &HostService, id: &str| {
        let devices = host.shared.devices.lock().unwrap();
        devices
            .iter()
            .find(|state| state.device.id == id)
            .unwrap()
            .last_poll
            .is_some()
    };
    assert!(!polled(&host, "a"), "a healthy connected path was polled");
    assert!(polled(&host, "b"), "an absent device was not polled");
    host.stop();
}

/// A device that started again punches through a path that still looks connected, at once;
/// an unstamped, stale, older or forged punch (what a replay on the way can send) cannot.
#[test]
fn a_device_that_started_again_replaces_its_connected_path_at_once() {
    let host = HostService::start(config()).unwrap();
    let device = Device {
        id: "a".into(),
        name: "Phone".into(),
        secret: [3; 32],
    };
    host.set_devices(vec![device.clone()]);
    let keys = Keys::new(device.secret);
    let tag = keys.device_tag(&host.session());
    let punch = |secret: [u8; 32], session: [u8; 16], stamp: Option<u64>| {
        let path = Keys::new(secret).path(&host.session(), &session);
        let payload = stamp.map(wire::punch_stamp);
        wire::Sealer::new(&path.client_to_host, path.tag, session).seal(
            wire::Kind::Punch,
            0,
            payload.as_ref().map_or(&[][..], |bytes| &bytes[..]),
        )
    };
    let session = |host: &HostService| {
        host.shared
            .tunnels
            .read()
            .unwrap()
            .get(&tag)
            .map(|(session, _)| *session)
    };
    let old_address = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let new_address = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let now = unix_now();

    host.receive(
        &mut punch(device.secret, [9; 16], Some(now - 10)),
        old_address.local_addr().unwrap(),
    );
    assert_eq!(paths(&host, 1).len(), 1);
    assert_eq!(session(&host), Some([9; 16]));

    // None of these replaces it: no clock, a stale one, one from before the old session
    // began, or another device's keys.
    let refused = [
        punch(device.secret, [8; 16], None),
        punch(device.secret, [8; 16], Some(now - 1_000)),
        punch(device.secret, [8; 16], Some(now - 10)),
        punch(device.secret, [8; 16], Some(now - 20)),
        punch([4; 32], [8; 16], Some(now)),
    ];
    for mut forged in refused {
        host.receive(&mut forged, new_address.local_addr().unwrap());
    }
    thread::sleep(Duration::from_millis(300));
    assert_eq!(session(&host), Some([9; 16]));
    assert_eq!(paths(&host, 1).len(), 1);

    // The device's next session, stamped after the old one began, takes over in a moment.
    let started = std::time::Instant::now();
    host.receive(
        &mut punch(device.secret, [8; 16], Some(now)),
        new_address.local_addr().unwrap(),
    );
    while session(&host) != Some([8; 16]) && started.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(session(&host), Some([8; 16]));
    let took = started.elapsed();
    // ~0.1 s alone; the bound only says "not the 45 s of a silent path", under a loaded test run.
    assert!(took < Duration::from_secs(5), "{took:?}");
    let paths = paths(&host, 1);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].peer, Some(new_address.local_addr().unwrap()));

    // The old session's punch, replayed now, does not take it back.
    host.receive(
        &mut punch(device.secret, [9; 16], Some(now - 10)),
        old_address.local_addr().unwrap(),
    );
    host.receive(
        &mut punch(device.secret, [9; 16], None),
        old_address.local_addr().unwrap(),
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(session(&host), Some([8; 16]));
    host.stop();
}

#[test]
fn only_a_paired_devices_punch_opens_a_path() {
    let host = HostService::start(config()).unwrap();
    let device = Device {
        id: "a".into(),
        name: "Phone".into(),
        secret: [3; 32],
    };
    host.set_devices(vec![device.clone()]);
    let keys = Keys::new(device.secret);
    let path = keys.path(&host.session(), &[9; 16]);
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let from = probe.local_addr().unwrap();

    // A stranger's punch with the right tag but another secret opens nothing.
    let stranger = Keys::new([4; 32]).path(&host.session(), &[9; 16]);
    let forged = wire::Sealer::new(&stranger.client_to_host, path.tag, [9; 16]).seal(
        wire::Kind::Punch,
        0,
        &[],
    );
    host.receive(&mut forged.clone(), from);
    assert!(host.status().paths.is_empty());

    let punch = wire::Sealer::new(&path.client_to_host, path.tag, [9; 16]).seal(
        wire::Kind::Punch,
        0,
        &[],
    );
    host.receive(&mut punch.clone(), from);
    let paths = paths(&host, 1);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].device, "Phone");
    assert!(paths[0].connected);

    // Unpairing closes it.
    host.set_devices(Vec::new());
    assert!(host.status().paths.is_empty());
    host.stop();
}

#[test]
fn a_host_on_one_address_serves_one_device_at_a_time() {
    let host = HostService::start(config()).unwrap();
    let phone = Device {
        id: "a".into(),
        name: "Phone".into(),
        secret: [3; 32],
    };
    let tablet = Device {
        id: "b".into(),
        name: "Tablet".into(),
        secret: [5; 32],
    };
    host.set_devices(vec![phone.clone(), tablet.clone()]);
    let punch = |device: &Device, session: [u8; 16]| {
        let path = Keys::new(device.secret).path(&host.session(), &session);
        wire::Sealer::new(&path.client_to_host, path.tag, session).seal(
            wire::Kind::Punch,
            0,
            &[],
        )
    };
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let from = probe.local_addr().unwrap();

    host.receive(&mut punch(&phone, [9; 16]), from);
    assert_eq!(paths(&host, 1).len(), 1);
    host.receive(&mut punch(&tablet, [8; 16]), from);
    for _ in 0..100 {
        if !host.status().path_errors.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let status = host.status();
    assert_eq!(status.paths.len(), 1);
    assert_eq!(status.paths[0].device, "Phone");
    // The tablet's wait is its own, not the DHT's.
    assert_eq!(status.path_errors.len(), 1);
    assert_eq!(status.path_errors[0].0, "Tablet");
    assert!(status.path_errors[0].1.contains("waits"));
    assert_eq!(status.dht_error, None);

    // Once the phone's path is gone, the tablet gets one, and its wait is over.
    host.set_devices(vec![tablet.clone()]);
    host.receive(&mut punch(&tablet, [8; 16]), from);
    let paths = paths(&host, 1);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].device, "Tablet");
    assert!(host.status().path_errors.is_empty());
    host.stop();
}
