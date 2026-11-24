//! End-to-end client roaming, metered hold, and roaming-disabled loopback tests.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::roaming::RoamEvent;
use nightjar_core::session::{NetworkChange, NetworkHint, SessionConfig};
use nightjar_core::video::FrameType;
use rubylight_protocol::roaming::Payload;

use crate::host::{start_host_with, Media, CHALLENGE};
use crate::media::{audio_stream, h264_idr, Packetizer};
use crate::recorder::{
    await_mic_from, input_arrives, loopback_addresses, roam_events, roaming_config, silence_host,
    start_session, until, Record,
};

/// A roam onto another local address: PROBE, CONFIRM and MIGRATED move the control stream,
/// and video, audio and the microphone carry on to and from the new address.
#[test]
fn a_session_roams_control_and_media_to_a_new_address() {
    let Some((old, new)) = loopback_addresses() else {
        return;
    };
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    assert!(session.microphone_available());

    // Before the roam everything runs from the first address.
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let client = host.peer_from(Media::Video, old);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame", || record.lock().unwrap().frames.len() == 1);
    let audio = audio_stream(1300);
    let (before, after) = audio.split_at(300);
    let client = host.peer_from(Media::Audio, old);
    for packet in before {
        host.audio.send_to(packet, client).unwrap();
        thread::sleep(Duration::from_micros(200));
    }
    until("audio", || record.lock().unwrap().audio.len() >= 90);
    session.send_microphone(b"before").unwrap();
    assert_eq!(await_mic_from(&host.mic, old), b"before");

    session.roam_to(Some(new)).unwrap();
    until("the roam", || {
        roam_events(&record)
            .iter()
            .any(|e| matches!(e, RoamEvent::Roamed { .. }))
    });
    let roaming = host.roaming.lock().unwrap().clone();
    assert!(
        matches!(roaming[0].1, Payload::Probe { flags: 1, .. }),
        "a PROBE with the media rebound flag first: {roaming:?}"
    );
    assert!(roaming.contains(&(
        roaming[0].0,
        Payload::Confirm {
            challenge: CHALLENGE
        }
    )));
    assert!(
        roaming.iter().all(|(from, _)| from.ip() == new),
        "{roaming:?}"
    );

    // Video, audio and the microphone resume on the new address.
    let client = host.peer_from(Media::Video, new);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame after the roam", || {
        record.lock().unwrap().frames.len() == 2
    });
    let played = record.lock().unwrap().audio.len();
    let client = host.peer_from(Media::Audio, new);
    // Sent as a host streams, until enough of it plays: one burst could all
    // land before a slow (sanitizer) client reads the new socket, and the
    // wait then timed out with nothing more coming.
    let mut packets = after.iter();
    until("audio after the roam", || {
        if let Some(packet) = packets.next() {
            host.audio.send_to(packet, client).unwrap();
        }
        record.lock().unwrap().audio.len() >= played + 50
    });
    session.send_microphone(b"after").unwrap();
    assert_eq!(await_mic_from(&host.mic, new), b"after");
    // So does the control stream.
    input_arrives(&session, &host);

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    let r = record.lock().unwrap();
    assert_eq!(r.frames, [(1, FrameType::Idr), (2, FrameType::Idr)]);
    assert!(r.audio.iter().all(Option::is_some));
    assert_eq!(r.terminated, None);
}

/// The host drops media pings from the new address until MIGRATED, so on Roamed video and
/// audio ping at once rather than an interval (500 ms) later.
#[test]
fn media_ping_from_the_new_address_at_once_on_roamed() {
    let Some((old, new)) = loopback_addresses() else {
        return;
    };
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    host.peer_from(Media::Video, old);
    host.peer_from(Media::Audio, old);

    session.roam_to(Some(new)).unwrap();
    until("the roam", || {
        roam_events(&record)
            .iter()
            .any(|e| matches!(e, RoamEvent::Roamed { .. }))
    });
    let roamed = Instant::now();
    let pinged_from_new = |media: Media| {
        let state = host.media.lock().unwrap();
        state
            .pings
            .iter()
            .any(|&(m, from, _)| m == media && from.ip() == new)
    };
    until("video and audio pings from the new address", || {
        pinged_from_new(Media::Video) && pinged_from_new(Media::Audio)
    });
    let waited = roamed.elapsed();
    assert!(
        waited < Duration::from_millis(250),
        "the media pinged {} ms after Roamed",
        waited.as_millis()
    );

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// A network change that leaves the route toward the host where it was (loopback stays on
/// 127.0.0.1) is spurious: nothing moves and nothing is probed.
#[test]
fn a_network_change_on_the_same_address_does_not_roam() {
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    let client = host.peer(Media::Video);

    // Not roamed, and the connection carries on: the front end must not reconnect.
    assert_eq!(
        session.network_changed(NetworkHint::default()),
        NetworkChange::CarriesOn
    );
    // The control stream carries on, the host saw no PROBE and video stays where it was.
    input_arrives(&session, &host);
    assert_eq!(*host.roaming.lock().unwrap(), []);
    assert!(roam_events(&record).is_empty());
    assert_eq!(host.peer(Media::Video), client);
    assert!(
        record
            .lock()
            .unwrap()
            .logs
            .iter()
            .any(|l| l.contains("still on 127.0.0.1; not roaming")),
        "{:?}",
        record.lock().unwrap().logs
    );

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// The host takes media pings only from the address the control stream roamed to: media left
/// behind by a control socket that moved alone follow it on Roamed, and video carries on.
#[test]
fn media_left_behind_follow_the_control_stream_on_roamed() {
    let Some((old, new)) = loopback_addresses() else {
        return;
    };
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let client = host.peer_from(Media::Video, old);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame", || record.lock().unwrap().frames.len() == 1);

    session.roam_control_only(new).unwrap();
    until("the roam", || {
        roam_events(&record)
            .iter()
            .any(|e| matches!(e, RoamEvent::Roamed { .. }))
    });
    let client = host.peer_from(Media::Video, new);
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame after the roam", || {
        record.lock().unwrap().frames.len() == 2
    });
    let _ = host.peer_from(Media::Audio, new);
    session.send_microphone(b"after").unwrap();
    assert_eq!(await_mic_from(&host.mic, new), b"after");

    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    let r = record.lock().unwrap();
    assert!(
        r.logs.iter().any(|l| l.contains("the media were left on")),
        "{:?}",
        r.logs
    );
    assert_eq!(r.terminated, None);
}

/// On a metered network without "Continue on mobile data" the client neither moves nor
/// probes, on the hint or through a silence after it: the network grace period decides,
/// and the stream rides the silence out where it was.
#[test]
fn on_a_metered_network_without_the_opt_in_the_grace_period_decides() {
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let config = SessionConfig {
        network_grace: Duration::from_secs(20),
        ..roaming_config()
    };
    let session = start_session(&host, config, &record);
    let client = host.peer(Media::Video);

    assert_eq!(
        session.network_changed(NetworkHint { metered: true }),
        NetworkChange::CarriesOn
    );
    until("the hold", || roam_events(&record) == [RoamEvent::Held]);
    silence_host(&host, &session);
    // The control stream carries on; the host has taken everything sent before this input.
    input_arrives(&session, &host);
    {
        let r = record.lock().unwrap();
        assert!(
            r.logs
                .iter()
                .any(|l| l.contains("the network grace period decides")),
            "{:?}",
            r.logs
        );
    }
    assert_eq!(roam_events(&record), [RoamEvent::Held]);
    assert_eq!(*host.roaming.lock().unwrap(), []);

    // The same video port carries on.
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    for packet in packetizer.frame(&h264_idr(), 2) {
        host.video.send_to(&packet, client).unwrap();
    }
    until("a frame", || record.lock().unwrap().frames.len() == 1);
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// With client roaming off, a host that advertises it is never probed: not on a network
/// change, not on a simulated roam, not through a silence.
#[test]
fn with_roaming_off_nothing_is_probed() {
    let host = start_host_with(None, true);
    let record = Arc::new(Mutex::new(Record::default()));
    let mut config = SessionConfig {
        network_grace: Duration::from_secs(20),
        ..roaming_config()
    };
    config.roaming.enabled = false;
    let session = start_session(&host, config, &record);
    assert!(
        record
            .lock()
            .unwrap()
            .logs
            .iter()
            .any(|l| l == "Roaming: off in the client's settings"),
        "{:?}",
        record.lock().unwrap().logs
    );

    // Without a roamer the session still tells the same address carries on, and a route from
    // another address than it started on cannot: it kept its start address.
    assert_eq!(
        session.network_changed(NetworkHint::default()),
        NetworkChange::CarriesOn
    );
    let elsewhere = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 7), 0));
    assert_eq!(
        session.network_changed_on(Ok(elsewhere)),
        NetworkChange::CannotContinue
    );
    let here = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    assert_eq!(
        session.network_changed_on(Ok(here)),
        NetworkChange::CarriesOn
    );
    for local in [None, Some(IpAddr::from(Ipv4Addr::LOCALHOST))] {
        let refused = session.roam_to(local).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::Unsupported, "{refused}");
    }
    silence_host(&host, &session);
    input_arrives(&session, &host);

    assert!(roam_events(&record).is_empty());
    assert_eq!(*host.roaming.lock().unwrap(), []);
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    assert_eq!(record.lock().unwrap().terminated, None);
}
