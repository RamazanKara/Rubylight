//! Unit tests for the Android engine lifecycle, gauges, clipboard sync, and present clock.

use super::*;

#[test]
fn an_interrupted_start_leaves_no_host_messages_behind() {
    let _serial = lock(&ENGINE_TESTS);
    const CLOCK_SYNC: u16 = 0x5537;
    *lock(&ENGINE.interrupt) = Interrupt::default();
    set_host_messages(&[]);
    announce_host_messages(&[CLOCK_SYNC]);
    assert!(
        host_supports(CLOCK_SYNC),
        "a live start records the announcement"
    );
    set_host_messages(&[]);

    // `LiInterruptConnection` and `LiStopConnection` run while the handshake is still going;
    // the announcement that arrives afterwards is dropped.
    interrupt();
    stop();
    announce_host_messages(&[CLOCK_SYNC]);
    assert!(
        !host_supports(CLOCK_SYNC),
        "an interrupted start must not record messages"
    );

    *lock(&ENGINE.interrupt) = Interrupt::default();
}

#[test]
fn clipboard_sync_runs_only_with_a_host_that_offered_it_and_ends_with_the_stream() {
    let _serial = lock(&ENGINE_TESTS);
    *lock(&ENGINE.interrupt) = Interrupt::default();
    stop();
    let from_host = || Frame {
        kind: rubylight_protocol::clipboard_sync::KIND_TEXT,
        token: 9,
        payload: b"pc".to_vec(),
    };
    assert_eq!(
        clipboard_sync_start(Some("here".into())),
        CLIPBOARD_SYNC_NOT_OFFERED
    );
    assert_eq!(clipboard_sync_local("copied".into()), CLIPBOARD_COPY_FAILED);
    assert!(clipboard_sync_received(from_host()).is_none());

    // Offered, but no stream takes the hello: said, and the sync stays off.
    set_host_messages(&[CLIPBOARD_SYNC_MESSAGE_TYPE]);
    assert_eq!(clipboard_sync_start(None), CLIPBOARD_SYNC_FAILED);
    assert!(clipboard_sync_received(from_host()).is_none());

    // Running, the host's copy applies once; a stop ends it.
    *lock(&ENGINE.clipboard) = Some(ClipboardSync::new(false));
    assert!(matches!(
        clipboard_sync_received(from_host()),
        Some(Received::Apply(ClipboardItem::Text(ref text))) if text == "pc"
    ));
    assert_eq!(
        clipboard_sync_local("pc".into()),
        CLIPBOARD_COPY_SKIPPED,
        "the host's copy does not go back"
    );
    assert_eq!(
        clipboard_sync_local("x".repeat(70_000)),
        CLIPBOARD_COPY_TOO_LARGE
    );
    stop();
    assert!(clipboard_sync_received(from_host()).is_none());
}

#[test]
fn the_host_packed_modes_last_from_the_handshake_until_stop() {
    let _serial = lock(&ENGINE_TESTS);
    *lock(&ENGINE.interrupt) = Interrupt::default();
    stop();
    assert_eq!(host_packed444_modes(), -1, "unknown before a handshake");
    announce_host_packed444_modes(0b0101);
    assert_eq!(
        host_packed444_modes(),
        0b0101,
        "known before the session is stored"
    );
    stop();
    assert_eq!(
        host_packed444_modes(),
        -1,
        "a stopped stream leaves none behind"
    );

    // A DESCRIBE answered after an interrupt is not kept for the next stream.
    interrupt();
    announce_host_packed444_modes(0b1111);
    assert_eq!(host_packed444_modes(), -1);
    *lock(&ENGINE.interrupt) = Interrupt::default();
}

#[test]
fn the_announced_hdr_format_lasts_from_the_handshake_until_stop() {
    let _serial = lock(&ENGINE_TESTS);
    *lock(&ENGINE.interrupt) = Interrupt::default();
    stop();
    assert_eq!(announced_hdr_format(), 0, "stock before a handshake");
    announce_hdr_format(rubylight_protocol::hdr::DV_81);
    assert_eq!(
        announced_hdr_format(),
        4,
        "known before the decoder's setup"
    );
    stop();
    assert_eq!(
        announced_hdr_format(),
        0,
        "a stopped stream leaves none behind"
    );

    interrupt();
    announce_hdr_format(rubylight_protocol::hdr::HLG);
    assert_eq!(announced_hdr_format(), 0);
    *lock(&ENGINE.interrupt) = Interrupt::default();
}

#[test]
fn a_network_change_without_a_stream_starts_no_roam() {
    let _serial = lock(&ENGINE_TESTS);
    stop();
    // The network callback can fire after the stream ended; it must not panic or roam.
    assert_eq!(network_changed(false), NetworkChange::CarriesOn);
    assert_eq!(network_changed(true), NetworkChange::CarriesOn);
}

// Regression (nightjar#80): MediaCodec's submit asked for the decoder drops through the
// session, from the receive thread under the receiver's lock, and every Rust-engine
// MediaCodec stream hung on its first frame. What the decoder reads must not wait for the
// session.
#[test]
fn the_decoders_own_reads_do_not_wait_for_the_session() {
    let _serial = lock(&ENGINE_TESTS);
    let held = lock(&ENGINE.session);
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send((decoder_drops(), decode_queue_len(), pending_audio_ms()));
    });
    let read = finished.recv_timeout(std::time::Duration::from_secs(5));
    drop(held);
    assert_eq!(read, Ok((0, 0, 0)), "the reads waited for the session lock");
}

#[test]
fn the_host_messages_are_known_before_a_session_is_stored() {
    let _serial = lock(&ENGINE_TESTS);
    // The video renderer starts inside Session::start, before the engine holds the session;
    // its start asks whether the host announced clock sync to decide on the probes.
    const CLOCK_SYNC: u16 = 0x5537;
    set_host_messages(&[]);
    assert!(!host_supports(CLOCK_SYNC));
    assert!(!with_session(false, |_| true), "no session stored");
    set_host_messages(&[0x5530, CLOCK_SYNC]);
    assert!(host_supports(CLOCK_SYNC));
    assert!(!host_supports(0x5531));
    set_host_messages(&[]);
    assert!(!host_supports(CLOCK_SYNC));
}

#[test]
fn the_phase_lock_report_follows_the_announced_messages_without_their_lock() {
    let _serial = lock(&ENGINE_TESTS);
    set_host_messages(&[]);
    assert!(
        !report_frame_slack(1_000, 8_333_333),
        "not announced: refused"
    );
    set_host_messages(&[0x5530]);
    assert!(ENGINE.phase_lock_supported.load(Ordering::SeqCst));
    set_host_messages(&[0x5531]);
    assert!(!ENGINE.phase_lock_supported.load(Ordering::SeqCst));
    set_host_messages(&[]);
}

#[test]
fn present_clock_offset_waits_for_a_clock_sync_exchange() {
    let _serial = lock(&ENGINE_TESTS);
    assert_eq!(present_clock_offset(1_000), None, "no clock running");
    present_clock_start(0, false);
    assert_eq!(present_clock_offset(1_000), None, "no exchange yet");
    // client = host + offset: sent at 1000, answered at 1100, the host at 5000 and 5010.
    clock_sync(
        &ClockSyncReply {
            sequence: 1,
            client_us: 1_000,
            host_receive_us: 5_000,
            host_send_us: 5_010,
        },
        1_100,
    );
    assert_eq!(present_clock_offset(1_050), Some((-3_955, 90)));
    present_clock_stop();
    assert_eq!(present_clock_offset(1_050), None);
}

#[test]
fn an_adaptive_display_bounds_the_wait_for_late_frames_and_a_grid_does_not() {
    let _serial = lock(&ENGINE_TESTS);
    present_clock_start(8_333, true);
    let spread = |clock: &Option<PresentClock>| {
        clock
            .as_ref()
            .map(|c| (c.settings.max_spread_us, c.settings.min_interval_us))
    };
    assert_eq!(
        spread(&lock(&ENGINE.present_clock)),
        Some((
            Some(rubylight_protocol::present_clock::ADAPTIVE_SPREAD_US),
            8_333
        ))
    );
    present_clock_start(8_333, false);
    assert_eq!(spread(&lock(&ENGINE.present_clock)), Some((None, 8_333)));
    present_clock_stop();
}

#[test]
fn the_engine_clock_schedules_on_a_grid_and_follows_the_content_rate() {
    let _serial = lock(&ENGINE_TESTS);
    let grid = Grid {
        anchor_us: 0,
        interval_us: 16_667,
        lead_us: 4_000,
        slack_us: 0,
    };
    assert_eq!(
        present_clock_schedule_on_grid(0, 1_000, grid),
        None,
        "no clock running"
    );
    assert_eq!(present_clock_content_rate(), 0);
    present_clock_start(0, false);
    let s = present_clock_schedule_on_grid(0, 1_000_000, grid).expect("scheduled");
    assert!(s.vsync_us >= 1_000_000 && s.release_us >= 1_000_000);
    for i in 0..400u32 {
        present_clock_observe(i * 1_500); // 60 fps in 90 kHz ticks
    }
    let rate = present_clock_content_rate();
    assert!(rate.abs_diff(60_000) <= 100, "{rate}");
    present_clock_reset();
    assert_eq!(
        present_clock_content_rate(),
        0,
        "a reset starts the rate over"
    );
    present_clock_stop();
    assert_eq!(present_clock_content_rate(), 0);
}

#[test]
fn quick_suspend_query_is_sent_only_when_0x554b_is_offered() {
    use rubylight_protocol::quick_suspend::QUICK_SUSPEND_MESSAGE_TYPE;
    assert!(!should_query_quick_suspend(&[]));
    assert!(!should_query_quick_suspend(&[0x554A, 0x5548]));
    assert!(should_query_quick_suspend(&[
        0x554A,
        QUICK_SUSPEND_MESSAGE_TYPE
    ]));
}
