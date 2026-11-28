use super::*;
use crate::state::test_support::Fixture;

#[test]
#[ignore = "requires the packaged libopus DLL"]
fn damaged_microphone_packets_keep_the_playout_clock_running() -> Result<()> {
    let root = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
    );
    let mut encoder = butterpollo_win_io::audio::Opus::new(&root, 2, false)?;
    let valid = encoder.encode(&vec![0.25; 960 * 2])?;
    // Code 3 requires a second framing byte. This cannot contain a valid
    // Opus frame, even though its encrypted transport can be intact.
    let damaged = [3];
    for missing in [0, 1, 3] {
        let mut decoder = OpusDecoder::new(&root)?;
        let mut sequencer = Sequencer::default();
        let mut stats = Stats::default();
        let mut pcm = Vec::new();
        decode_packet(
            &mut decoder,
            sequencer.arrive(10),
            &valid,
            &mut pcm,
            &mut stats,
        );
        assert_eq!(pcm.len(), 960);
        pcm.clear();
        decode_packet(
            &mut decoder,
            sequencer.arrive(11 + missing),
            &damaged,
            &mut pcm,
            &mut stats,
        );
        assert_eq!(pcm.len(), (usize::from(missing) + 1) * 960);
        assert_eq!(stats.damaged, 1);
        assert_eq!(stats.concealed, u64::from(missing) + 1);
        pcm.clear();
        decode_packet(
            &mut decoder,
            sequencer.arrive(12 + missing),
            &valid,
            &mut pcm,
            &mut stats,
        );
        assert_eq!(pcm.len(), 960);
        assert!(pcm.iter().all(|sample| sample.is_finite()));
    }
    Ok(())
}

#[test]
#[ignore = "requires the packaged libopus DLL"]
fn recovered_microphone_burst_does_not_expand_playout_delay() -> Result<()> {
    let root = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
    );
    let mut encoder = butterpollo_win_io::audio::Opus::new(&root, 2, false)?;
    let valid = encoder.encode(&vec![0.25; 960 * 2])?;
    let mut decoder = OpusDecoder::new(&root)?;
    let mut sequencer = Sequencer::default();
    let mut stats = Stats::default();
    let mut pcm = Vec::new();
    decode_packet(
        &mut decoder,
        sequencer.arrive(10),
        &valid,
        &mut pcm,
        &mut stats,
    );
    pcm.clear();
    let frames = decode_packet(
        &mut decoder,
        sequencer.arrive(16),
        &valid,
        &mut pcm,
        &mut stats,
    );
    // Five lost packets plus the current packet produce 120 ms of PCM.
    // Recovery must retain only the newest sound within the normal queue
    // bound; the loss burst does not make the client's packets longer.
    assert_eq!(frames, 960);
    assert_eq!(pcm.len(), 960 * 6);
    let mut playout = Playout::default();
    playout.push_batch(&pcm, frames);
    assert_eq!(playout.len(), 960);
    assert_eq!(playout.trimmed, 960 * 5);
    let mut actual = vec![0.; 960];
    assert_eq!(playout.take(&mut actual), 960);
    assert_eq!(actual, pcm[pcm.len() - 960..]);
    Ok(())
}

/// A 437 Hz voice through the real encoder and decoder, 20 ms packets
/// held back 60 ms twice a second, played by 10 ms device wake-ups with
/// `conceal` (or silence): what the device played.
fn played_through_opus(root: &std::path::Path, conceal: bool) -> Result<Vec<f32>> {
    let tone = |n: usize| ((n * 437 % 48_000) as f64 * std::f64::consts::TAU / 48_000.).sin();
    let mut encoder = butterpollo_win_io::audio::Opus::new(root, 2, false)?;
    let decoder = Mutex::new(Some(OpusDecoder::new(root)?));
    let mut playout = Playout::default();
    let (mut played, mut out, mut pcm) = (Vec::new(), Vec::new(), Vec::new());
    let (mut padding, mut warned) = (0, None);
    let mut packets = (0..150usize).map(|k| {
        let at = k * 20 + 5;
        // Held back 60 ms at the start of each half second, then sent.
        let at = if at % 500 < 60 {
            at - at % 500 + 60
        } else {
            at
        };
        let frame: Vec<f32> = (k * 960..(k + 1) * 960)
            .flat_map(|n| [tone(n) as f32 * 0.5; 2])
            .collect();
        (at, frame)
    });
    let mut next = packets.next();
    for now in (0..3_000).step_by(10) {
        while let Some((_, frame)) = next.take_if(|(at, _)| *at < now) {
            let packet = encoder.encode(&frame)?;
            // As the receiver: the decoder is held until the audio is queued.
            let mut decoder = decoder.lock().unwrap();
            pcm.clear();
            decoder
                .as_mut()
                .unwrap()
                .decode(Some(&packet), false, &mut pcm)?;
            playout.push_batch(&pcm, pcm.len());
            drop(decoder);
            next = packets.next();
        }
        padding = usize::saturating_sub(padding, 480);
        let silence = playout.pull_concealing(960, padding, 480, &mut out, &mut |out| {
            if conceal {
                conceal_frame(&decoder, out, &mut warned, Instant::now())
            } else {
                Ok(0)
            }
        });
        played.extend(&out);
        played.extend(std::iter::repeat_n(0., silence));
        padding += out.len() + silence;
    }
    assert_eq!(playout.conceal_failed, 0);
    Ok(played)
}

#[test]
#[ignore = "requires the packaged libopus DLL"]
fn opus_concealment_fills_microphone_underruns_without_clicks() -> Result<()> {
    let root = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
    );
    let step = |played: &[f32]| {
        played[4_800..]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0., f32::max)
    };
    let silent_runs = |played: &[f32]| {
        played[4_800..]
            .windows(48)
            .filter(|run| run.iter().all(|sample| *sample == 0.))
            .count()
    };
    let silent = played_through_opus(&root, false)?;
    let concealed = played_through_opus(&root, true)?;
    // Silence broke the voice off with hard edges; Opus concealment
    // carries it through and crossfades back into it. The tone itself
    // moves at most 0.029 a sample.
    assert!(silent_runs(&silent) > 0);
    assert!(step(&silent) > 0.1, "{}", step(&silent));
    assert_eq!(silent_runs(&concealed), 0);
    assert!(step(&concealed) < 0.06, "{}", step(&concealed));
    // The same amount played: no added delay.
    assert_eq!(silent.len(), concealed.len());
    Ok(())
}

#[test]
fn concealment_never_waits_for_the_receiver_and_failures_are_logged_rarely() {
    let decoder = Mutex::new(None);
    let (mut out, mut warned) = (Vec::new(), None);
    let now = Instant::now();
    // No decoder yet, or the receiver holding it: nothing, at once.
    assert_eq!(conceal_frame(&decoder, &mut out, &mut warned, now), Ok(0));
    let held = decoder.lock().unwrap();
    assert_eq!(conceal_frame(&decoder, &mut out, &mut warned, now), Ok(0));
    drop(held);
    assert!(out.is_empty() && warned.is_none());
    // A decoder failing every wake-up is logged once in ten seconds.
    assert!(warning_due(&mut warned, now));
    assert!(!warning_due(&mut warned, now + Duration::from_secs(9)));
    assert!(warning_due(&mut warned, now + CONCEAL_WARNING));
}

#[test]
fn the_microphone_level_is_checked_at_once_and_then_every_second() {
    // An app's automatic gain can raise it mid-stream (issue 56).
    let mut cap = GainCap {
        gain: None,
        next: None,
    };
    let start = Instant::now();
    assert!(cap.due(start));
    assert!(!cap.due(start + Duration::from_millis(999)));
    assert!(cap.due(start + GAIN_CHECK));
    assert!(!cap.due(start + GAIN_CHECK + Duration::from_millis(500)));
    assert!(cap.due(start + GAIN_CHECK * 2));
}
#[test]
fn a_failed_microphone_install_is_tried_again_after_a_pause() {
    let start = Instant::now();
    let mut install = Install {
        installed: false,
        tried: None,
    };
    assert!(install.begin(start));
    // Running, or failed a moment ago: other connections do not install.
    assert!(!install.begin(start + Duration::from_secs(5)));
    assert!(install.begin(start + INSTALL_BACKOFF));
    // Nothing was tried (Steam missing): the next connection checks again.
    install.tried = None;
    assert!(install.begin(start + INSTALL_BACKOFF));
    install.installed = true;
    install.tried = None;
    assert!(!install.begin(start + INSTALL_BACKOFF * 2));
}

#[test]
fn stopping_a_render_thread_that_is_still_opening_does_not_wait_for_it() {
    // The thread is still in the install wait and does not see the flag.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let stop = Arc::new(AtomicBool::new(false));
    let mut output = Output::default();
    output.thread = Some((
        stop.clone(),
        thread::spawn(move || {
            let _ = held.recv();
            Ok(())
        }),
    ));
    let opened = output.opened.clone();
    let started = Instant::now();
    assert!(output.retire().is_none());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(stop.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&opened, &output.opened));
    // No second render thread starts while the first one still runs.
    assert!(!output.stopped());
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !output.stopped() {
        assert!(
            Instant::now() < deadline,
            "the stopped thread was not collected"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(output.stopping.is_none());
    // A thread that has already ended gives its result straight away.
    output.thread = Some((
        Arc::default(),
        thread::spawn(|| Err(anyhow::anyhow!("device gone"))),
    ));
    while !output.thread.as_ref().unwrap().1.is_finished() {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(output.retire(), Some(Ok(Err(_)))));
}

#[test]
fn a_host_that_cannot_find_its_executable_fails_loudly() {
    let missing = exe_directory(Err(std::io::Error::other("no image path"))).unwrap_err();
    let message = format!("{missing:#}");
    assert!(
        message.contains("could not be found") && message.contains("no image path"),
        "{message}"
    );
    // A bare file name has no folder, which is not the working directory.
    let bare = exe_directory(Ok(PathBuf::from("butterpollo.exe"))).unwrap_err();
    assert!(format!("{bare:#}").contains("has no folder"), "{bare:#}");
    assert_eq!(
        exe_directory(Ok(PathBuf::from(
            r"C:\Program Files\Rubylight\butterpollo.exe"
        )))
        .unwrap(),
        PathBuf::from(r"C:\Program Files\Rubylight")
    );
}

#[test]
fn a_microphone_only_session_plays_its_microphone() {
    use butterpollo_session::session::Role;
    let f = Fixture::new();
    let mut launch = f.launch(f.client(u32::MAX), Role::InputOnly);
    launch
        .options
        .insert(butterpollo_session::mic::ONLY_OPTION.into(), "1".into());
    assert!(launch.mic_only());
    let negotiated = butterpollo_media_policy::rtsp::Negotiated {
        mic: true,
        ..Default::default()
    };
    let session = Session::new(launch, negotiated);
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    let counters = Arc::new(Counters::default());
    let mut receiver = Receiver::new(f.host.clone(), counters.clone(), PathBuf::new());
    receiver.native = false;
    let packet = mic::seal(
        &session.launch.key,
        session.launch.key_id,
        7,
        140,
        &[1, 2, 3],
    );
    receiver.receive(&packet, "127.0.0.2".parse().unwrap());
    assert_eq!(counters.refused.load(Ordering::Relaxed), 1);
    assert!(receiver.speaker.is_none());
    receiver.receive(&packet, session.client_ip());
    assert_eq!(counters.accepted.load(Ordering::Relaxed), 1);
    let speaker = receiver
        .speaker
        .as_ref()
        .expect("the session's microphone plays");
    assert!(Arc::ptr_eq(&speaker.session, &session));
    assert_eq!(speaker.stats.packets, 1);
}

#[test]
fn a_second_microphone_is_refused_and_told_while_one_plays() {
    use butterpollo_session::session::Role;
    let f = Fixture::new();
    let negotiated = butterpollo_media_policy::rtsp::Negotiated {
        mic: true,
        ..Default::default()
    };
    let session = |name: &str, uuid: &str, key: u8| {
        let mut client = f.client(u32::MAX);
        client.name = name.into();
        client.uuid = uuid.into();
        let mut launch = f.launch(client, Role::Stream);
        // Each session's own key, as launches get.
        launch.key = [key; 16];
        let session = Session::new(launch, negotiated.clone());
        f.host
            .sessions
            .lock()
            .unwrap()
            .active
            .insert(session.launch.id.clone(), session.clone());
        session
    };
    let phone = session("Phone", "phone", 1);
    let tablet = session("Tablet", "tablet", 2);
    let counters = Arc::new(Counters::default());
    let mut receiver = Receiver::new(f.host.clone(), counters.clone(), PathBuf::new());
    receiver.native = false;
    let packet = |s: &Arc<Session>| mic::seal(&s.launch.key, s.launch.key_id, 7, 140, &[1, 2, 3]);
    receiver.receive(&packet(&phone), phone.client_ip());
    receiver.receive(&packet(&tablet), tablet.client_ip());
    assert_eq!(counters.accepted.load(Ordering::Relaxed), 1);
    assert_eq!(counters.refused.load(Ordering::Relaxed), 1);
    assert!(Arc::ptr_eq(
        &receiver.speaker.as_ref().unwrap().session,
        &phone
    ));
    let warnings = tablet.launch.warnings.snapshot();
    assert!(
        warnings
            .iter()
            .any(|w| w.code == MIC_BUSY_WARNING && w.message.contains("Phone")),
        "{warnings:?}"
    );
    assert!(phone.launch.warnings.snapshot().is_empty());
    // `/session` tells the waiting device, and only it.
    assert!(busy_for(&f.host, "tablet") && !busy_for(&f.host, "phone"));
    // The phone goes quiet: the tablet's microphone plays and is no longer busy.
    receiver.speaker.as_mut().unwrap().heard -= HANDOVER;
    receiver.receive(&packet(&tablet), tablet.client_ip());
    assert!(Arc::ptr_eq(
        &receiver.speaker.as_ref().unwrap().session,
        &tablet
    ));
    assert!(!busy_for(&f.host, "tablet"));
}

#[test]
fn a_packet_of_a_second_session_resolves_while_the_cache_holds_the_first() {
    use butterpollo_session::session::Role;
    let f = Fixture::new();
    let negotiated = butterpollo_media_policy::rtsp::Negotiated {
        mic: true,
        ..Default::default()
    };
    // Every launch here comes from one address, so the two sessions are told
    // apart by their keys alone, as two clients behind one NAT are.
    let session = |name: &str, uuid: &str, key: u8| {
        let mut client = f.client(u32::MAX);
        client.name = name.into();
        client.uuid = uuid.into();
        let mut launch = f.launch(client, Role::Stream);
        launch.key = [key; 16];
        let session = Session::new(launch, negotiated.clone());
        f.host
            .sessions
            .lock()
            .unwrap()
            .active
            .insert(session.launch.id.clone(), session.clone());
        session
    };
    let phone = session("Phone", "phone", 1);
    let tablet = session("Tablet", "tablet", 2);
    let counters = Arc::new(Counters::default());
    let mut receiver = Receiver::new(f.host.clone(), counters.clone(), PathBuf::new());
    receiver.native = false;
    let packet = |s: &Arc<Session>| mic::seal(&s.launch.key, s.launch.key_id, 7, 140, &[1, 2, 3]);
    // The phone's packet fills the cache; the speaker it started goes quiet
    // so the next one may take over.
    receiver.receive(&packet(&phone), phone.client_ip());
    receiver.speaker.as_mut().unwrap().heard -= HANDOVER;
    // The tablet's packet does not open with the phone's key: the full lookup
    // still finds the tablet, and it takes the microphone over.
    receiver.receive(&packet(&tablet), tablet.client_ip());
    assert_eq!(counters.accepted.load(Ordering::Relaxed), 2);
    assert_eq!(counters.refused.load(Ordering::Relaxed), 0);
    assert!(Arc::ptr_eq(
        &receiver.speaker.as_ref().unwrap().session,
        &tablet
    ));
    // The cache holds the tablet now; the phone's packet resolves the same way.
    receiver.speaker.as_mut().unwrap().heard -= HANDOVER;
    receiver.receive(&packet(&phone), phone.client_ip());
    assert!(Arc::ptr_eq(
        &receiver.speaker.as_ref().unwrap().session,
        &phone
    ));
}

#[test]
fn an_ended_session_is_not_used_from_the_cache() {
    use butterpollo_session::session::Role;
    let f = Fixture::new();
    let negotiated = butterpollo_media_policy::rtsp::Negotiated {
        mic: true,
        ..Default::default()
    };
    let session = |name: &str, uuid: &str, key: u8| {
        let mut client = f.client(u32::MAX);
        client.name = name.into();
        client.uuid = uuid.into();
        let mut launch = f.launch(client, Role::Stream);
        launch.key = [key; 16];
        let session = Session::new(launch, negotiated.clone());
        f.host
            .sessions
            .lock()
            .unwrap()
            .active
            .insert(session.launch.id.clone(), session.clone());
        session
    };
    let phone = session("Phone", "phone", 1);
    let tablet = session("Tablet", "tablet", 2);
    let counters = Arc::new(Counters::default());
    let mut receiver = Receiver::new(f.host.clone(), counters.clone(), PathBuf::new());
    receiver.native = false;
    let packet = |s: &Arc<Session>| mic::seal(&s.launch.key, s.launch.key_id, 7, 140, &[1, 2, 3]);
    receiver.receive(&packet(&phone), phone.client_ip());
    // The tablet's packet resolves too, and fills the cache with the tablet,
    // but the fresh phone keeps the microphone: the tablet is refused.
    receiver.receive(&packet(&tablet), tablet.client_ip());
    assert_eq!(counters.refused.load(Ordering::Relaxed), 1);
    // The tablet's session ends and is dropped: the cache's handle goes with
    // it, and its packet resolves to nothing rather than playing from the
    // cache.
    let tablet_packet = packet(&tablet);
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&tablet.launch.id);
    drop(tablet);
    receiver.receive(&tablet_packet, "127.0.0.1".parse().unwrap());
    assert_eq!(counters.refused.load(Ordering::Relaxed), 2);
    assert!(Arc::ptr_eq(
        &receiver.speaker.as_ref().unwrap().session,
        &phone
    ));
    // A stopping session is not used either, even though its key still opens
    // the packet.
    phone.stop();
    receiver.receive(&packet(&phone), phone.client_ip());
    assert_eq!(counters.refused.load(Ordering::Relaxed), 3);
    // The phone still plays: its last packet was refused, not handed over.
    assert!(Arc::ptr_eq(
        &receiver.speaker.as_ref().unwrap().session,
        &phone
    ));
}

#[test]
fn idle_microphone_socket_waits_for_the_receive_timeout() -> Result<()> {
    let f = Fixture::new();
    // Inspect the worker's shared socket without competing for its packets.
    f.host.stop.store(true, Ordering::Release);
    let socket = crate::network::udp("127.0.0.1:0".parse()?)?;
    let receiver = socket.try_clone()?;
    // Clones share blocking mode, but keep their own receive timeout on Windows.
    receiver.set_read_timeout(Some(Duration::from_millis(250)))?;
    spawn(f.host.clone(), socket)?;

    let started = Instant::now();
    let error = receiver.recv_from(&mut [0; 1]).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    assert!(started.elapsed() >= Duration::from_millis(100));
    Ok(())
}

#[test]
fn microphone_install_restores_defaults_on_success_timeout_and_error() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Found: restores defaults once and returns the endpoint id.
    let restored = AtomicUsize::new(0);
    let found = wait_for_installed_endpoint(
        Duration::from_millis(50),
        Duration::ZERO,
        Duration::ZERO,
        || Ok(Some("mic-id".into())),
        || {
            restored.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(found.as_deref(), Some("mic-id"));
    assert_eq!(restored.load(Ordering::Relaxed), 1);

    // Timeout: never found, yet defaults are still restored once.
    let restored = AtomicUsize::new(0);
    let timed_out = wait_for_installed_endpoint(
        Duration::from_millis(5),
        Duration::from_millis(1),
        Duration::ZERO,
        || Ok(None),
        || {
            restored.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(timed_out, None);
    assert_eq!(restored.load(Ordering::Relaxed), 1);

    // Enumeration error: propagates the error AND restores defaults once.
    let restored = AtomicUsize::new(0);
    let failed = wait_for_installed_endpoint(
        Duration::from_millis(50),
        Duration::ZERO,
        Duration::ZERO,
        || anyhow::bail!("COM disconnected"),
        || {
            restored.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
    );
    assert!(failed.is_err());
    assert_eq!(restored.load(Ordering::Relaxed), 1);
}

#[test]
fn oversized_datagram_is_classified_as_datagram_lost_in_microphone_loop() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender
        .send_to(&vec![0xAA; 4096], receiver.local_addr().unwrap())
        .unwrap();
    let mut small = [0u8; 2048];
    let err = receiver.recv_from(&mut small).unwrap_err();
    assert!(
        butterpollo_win_io::net::datagram_lost(&err),
        "expected datagram_lost for oversized UDP datagram, got {err:?}"
    );
}

#[test]
fn open_packet_respects_single_pkcs7_and_legacy_modes() {
    let key = [0x3du8; 16];
    let cbc = CbcKey::new(&key);
    let mut out = Vec::new();

    // 16-byte-aligned Opus packet ending in 0x01: preserved verbatim when single_pkcs7 is true.
    let mut aligned = vec![0x88u8; 32];
    aligned[31] = 0x01;
    let dg_single = {
        let mut raw = vec![0, mic::PACKET_TYPE_OPUS];
        raw.extend_from_slice(&7u16.to_le_bytes());
        raw.extend_from_slice(&100u32.to_le_bytes());
        raw.extend_from_slice(&mic::MAGIC.to_le_bytes());
        raw.extend_from_slice(&butterpollo_crypto::crypto::cbc_seal(
            &key,
            &mic::iv(99, 7),
            &aligned,
        ));
        raw
    };
    let pkt = mic::parse(&dg_single).unwrap();
    assert_eq!(
        open_packet(&cbc, 99, true, &pkt, &mut out).unwrap(),
        aligned.as_slice()
    );

    // Legacy double-padded packet still opens cleanly when single_pkcs7 is false.
    let legacy_opus = b"legacy-21-byte-packet";
    let dg_legacy = mic::seal(&key, 99, 8, 120, legacy_opus);
    let pkt = mic::parse(&dg_legacy).unwrap();
    assert_eq!(
        open_packet(&cbc, 99, false, &pkt, &mut out).unwrap(),
        legacy_opus
    );
}
