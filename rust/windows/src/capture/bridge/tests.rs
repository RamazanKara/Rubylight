#[test]
fn helper_reply_preserves_device_removal_and_accepts_older_errors() {
    let loss = crate::device_loss::DeviceLost(0x887a0006_u32 as i32);
    let reply = super::Reply::Error {
        message: "frame pool failed".into(),
        device_removed: Some(loss.0),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let super::Reply::Error {
        message,
        device_removed,
    } = serde_json::from_str(&json).unwrap()
    else {
        panic!("wrong reply")
    };
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&super::helper_error(message, device_removed)),
        Some(loss)
    );
    let old = r#"{"type":"Error","message":"capture unavailable"}"#;
    let super::Reply::Error {
        message,
        device_removed,
    } = serde_json::from_str(old).unwrap()
    else {
        panic!("wrong reply")
    };
    assert!(
        crate::device_loss::DeviceLost::from_error(&super::helper_error(message, device_removed))
            .is_none()
    );
}
use super::*;
#[test]
fn positive_mutex_wait_codes_never_grant_access_to_incomplete_pixels() {
    assert!(acquired_status(HRESULT(0)).unwrap());
    assert!(!acquired_status(HRESULT(WAIT_TIMEOUT.0 as i32)).unwrap());
    assert!(acquired_status(HRESULT(WAIT_ABANDONED.0 as i32)).is_err());
    assert!(acquired_status(HRESULT::from_win32(ERROR_ACCESS_DENIED.0)).is_err());
}
#[test]
fn private_pipe_checks_peers_bounds_messages_and_reports_disconnect() -> Result<()> {
    let (server, name) = Pipe::server(PIPE_PREFIX)?;
    // SAFETY: GetCurrentProcessId has no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    assert!(Pipe::client(&name, pid.wrapping_add(1), PIPE_PREFIX).is_err());
    // The rejected client closes its endpoint; use a fresh single-instance
    // server, just as failed capture startup does.
    drop(server);
    let (server, name) = Pipe::server(PIPE_PREFIX)?;
    let client = Pipe::client(&name, pid, PIPE_PREFIX)?;
    assert!(server.connected(pid)?);
    assert!(server.connected(pid.wrapping_add(1)).is_err());
    assert!(server.receive::<Request>()?.is_none());
    client.send(&Request::Release { slot: 2 })?;
    assert!(matches!(
        server.receive::<Request>()?,
        Some(Request::Release { slot: 2 })
    ));
    assert!(server.receive::<Request>()?.is_none());
    server.send(&Reply::Frame {
        slot: 1,
        sequence: 15,
        qpc: 1234,
        focus: Some(false),
    })?;
    assert!(matches!(
        client.receive::<Reply>()?,
        Some(Reply::Frame {
            slot: 1,
            sequence: 15,
            qpc: 1234,
            focus: Some(false)
        })
    ));
    // A frame WGC cannot label reads as before the label existed.
    let unlabelled = serde_json::to_string(&Reply::Frame {
        slot: 2,
        sequence: 16,
        qpc: 1235,
        focus: None,
    })
    .unwrap();
    assert_eq!(
        unlabelled,
        r#"{"type":"Frame","slot":2,"sequence":16,"qpc":1235}"#
    );
    assert!(matches!(
        serde_json::from_str(&unlabelled).unwrap(),
        Reply::Frame { focus: None, .. }
    ));
    assert!(client.send(&"x".repeat(crate::ipc::MESSAGE_LIMIT)).is_err());
    assert!(server.receive::<Request>()?.is_none());
    drop(client);
    assert!(server.receive::<Request>().is_err());
    Ok(())
}
#[test]
fn slots_bound_outstanding_frames_and_reject_stale_or_duplicate_messages() {
    let mut slots = Slots::default();
    for slot in 0..SLOTS {
        slots.publish(slot, slot as u64 + 1).unwrap();
    }
    let next = SLOTS as u64 + 1;
    assert!(slots.publish(0, next).is_err());
    assert!(slots.publish(SLOTS, next).is_err());
    assert!(slots.release(SLOTS).is_err());
    slots.release(0).unwrap();
    assert!(slots.release(0).is_err());
    assert!(slots.publish(0, next - 1).is_err());
    slots.publish(0, next).unwrap();
    assert!(slots.busy.iter().all(|busy| *busy));
}
#[test]
fn pending_capture_preserves_final_update_until_a_slot_returns() {
    let mut slots = Slots::default();
    for slot in 0..SLOTS {
        slots.publish(slot, slot as u64 + 1).unwrap();
    }
    let mut pending = None;
    assert!(frame_for_slot(&mut pending, Some(5), slots.busy.iter().any(|busy| !busy)).is_none());
    for _ in 0..10 {
        assert!(frame_for_slot(&mut pending, None, slots.busy.iter().any(|busy| !busy)).is_none());
    }
    slots.release(0).unwrap();
    assert_eq!(
        frame_for_slot(&mut pending, None, slots.busy.iter().any(|busy| !busy)),
        Some(5)
    );
    assert!(pending.is_none());
    assert!(frame_for_slot(&mut pending, None, true).is_none());
}
#[test]
fn pending_capture_replaces_old_updates_without_growing_a_queue() {
    let mut pending = None;
    let mut previous = std::sync::Weak::new();
    for update in 1..=10 {
        let frame = Arc::new(update);
        let current = Arc::downgrade(&frame);
        assert!(frame_for_slot(&mut pending, Some(frame), false).is_none());
        assert!(previous.upgrade().is_none());
        assert_eq!(current.strong_count(), 1);
        previous = current;
    }
    let frame = frame_for_slot(&mut pending, None, true).unwrap();
    assert_eq!(*frame, 10);
    assert!(pending.is_none());
    drop(frame);
    assert!(previous.upgrade().is_none());
}
#[test]
fn helper_receives_only_capture_settings_and_no_host_secrets_or_commands() {
    let source = Config::parse("adapter_name = AMD\ngpu_compute_conversion = false\nwgc_high_rate_capture = true\nwgc_drain_to_newest = true\nwgc_helper_streaming_scope = true\ncredentials_file = secret\nprep_cmd = run-something\nport = 47989\nwgc_user_helper = true\n").unwrap();
    let filtered = capture_config(&source);
    assert_eq!(filtered.values.len(), 5);
    assert_eq!(filtered.get("adapter_name", ""), "AMD");
    assert!(!filtered.boolean("gpu_compute_conversion", true));
    assert!(filtered.boolean("wgc_high_rate_capture", false));
    assert!(filtered.boolean("wgc_drain_to_newest", false));
    assert!(filtered.boolean("wgc_helper_streaming_scope", false));
    let message = Request::Start {
        version: VERSION,
        name: "display".into(),
        hdr: true,
        config: filtered,
        wake: 0x1234,
        window: None,
    };
    let encoded = serde_json::to_vec(&message).unwrap();
    assert!(encoded.len() < crate::ipc::MESSAGE_LIMIT);
    let decoded: Request = serde_json::from_slice(&encoded).unwrap();
    assert!(matches!(decoded, Request::Start { hdr: true, .. }));
    assert!(
        serde_json::from_str::<Request>(r#"{"type":"Release","slot":0,"command":"run"}"#).is_err()
    );
}
#[test]
fn a_window_rides_the_start_message_and_displays_send_what_they_did() {
    let start = |window| Request::Start {
        version: VERSION,
        name: r"\\.\DISPLAY1".into(),
        hdr: false,
        config: Config::default(),
        wake: 7,
        window,
    };
    // A display's start message has no window key, as before.
    let display = serde_json::to_string(&start(None)).unwrap();
    assert!(!display.contains("window"), "{display}");
    let Request::Start { window, .. } = serde_json::from_str(&display).unwrap() else {
        panic!("not a start message");
    };
    assert_eq!(window, None);
    let game = serde_json::to_string(&start(Some(0x2056c))).unwrap();
    let Request::Start { window, name, .. } = serde_json::from_str(&game).unwrap() else {
        panic!("not a start message");
    };
    assert_eq!((window, name.as_str()), (Some(0x2056c), r"\\.\DISPLAY1"));
}
#[test]
fn shared_descriptors_must_match_the_supported_bounded_texture_contract() {
    let mut desc = D3D11_TEXTURE2D_DESC {
        Width: 1920,
        Height: 1080,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0)
            as u32,
        ..Default::default()
    };
    assert!(validate_texture(&desc, 1920, 1080, desc.Format.0).is_ok());
    assert!(validate_texture(&desc, 16385, 1080, desc.Format.0).is_err());
    assert!(validate_texture(&desc, 1920, 1081, desc.Format.0).is_err());
    desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 as u32;
    assert!(validate_texture(&desc, 1920, 1080, desc.Format.0).is_err());
}
