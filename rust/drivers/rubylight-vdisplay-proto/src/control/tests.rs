use super::*;
use alloc::vec::Vec;

#[test]
fn ioctl_codes_are_buffered_any_access_and_distinct() {
    assert_eq!(IOCTL_GET_INFO, 0x0022_2C00);
    for (i, a) in ALL_IOCTLS.iter().enumerate() {
        // METHOD_BUFFERED (0) and FILE_ANY_ACCESS (0) in the low and access bits.
        assert_eq!(a & 0b11, 0, "{a:#x}");
        assert_eq!((a >> 14) & 0b11, 0, "{a:#x}");
        assert_eq!(a >> 16, DEVICE_TYPE);
        for b in &ALL_IOCTLS[i + 1..] {
            assert_ne!(a, b);
        }
    }
    // Outside both foreign drivers' 0x900 ranges.
    assert!(ALL_IOCTLS.iter().all(|c| function_of(*c) >= 0xB00));
}

#[test]
fn add_request_round_trips_at_fixed_offsets() {
    let request = AddRequest {
        session_id: 0x6000_1234_5678_9ABC,
        width: 3840,
        height: 2160,
        refresh_mhz: 239_760,
        preferred_monitor_id: 7,
        flags: ADD_FLAG_EXACT_ID,
        max_luminance_nits: 800,
        max_frame_avg_nits: 400,
        min_luminance_millinits: 50,
    };
    let bytes = request.encode();
    assert_eq!(bytes.len(), AddRequest::SIZE);
    assert_eq!(bytes[0..8], 0x6000_1234_5678_9ABCu64.to_le_bytes());
    assert_eq!(bytes[16..20], 239_760u32.to_le_bytes());
    assert_eq!(bytes[20..24], 7u32.to_le_bytes());
    assert_eq!(bytes[24..28], 1u32.to_le_bytes());
    assert_eq!(bytes[40..48], [0; 8]);
    assert_eq!(AddRequest::decode(&bytes), Ok(request));
}

#[test]
fn add_request_refuses_bad_sizes_and_fields() {
    let good = AddRequest {
        session_id: 1,
        width: 1920,
        height: 1080,
        refresh_mhz: 60_000,
        preferred_monitor_id: 0,
        flags: 0,
        max_luminance_nits: 0,
        max_frame_avg_nits: 0,
        min_luminance_millinits: 0,
    };
    let bytes = good.encode();
    assert_eq!(
        AddRequest::decode(&bytes[..40]),
        Err(WireError::Size {
            expected: 48,
            got: 40
        })
    );
    let mut longer = Vec::from(&bytes[..]);
    longer.push(0);
    assert!(AddRequest::decode(&longer).is_err());
    let mut reserved = bytes;
    reserved[44] = 1;
    assert_eq!(
        AddRequest::decode(&reserved),
        Err(WireError::Invalid("reserved"))
    );
    for (bad, field) in [
        (
            AddRequest {
                session_id: 0,
                ..good
            },
            "session id",
        ),
        (AddRequest { width: 0, ..good }, "mode"),
        (
            AddRequest {
                refresh_mhz: 999,
                ..good
            },
            "mode",
        ),
        (
            AddRequest {
                refresh_mhz: 1_000_001,
                ..good
            },
            "mode",
        ),
        (
            AddRequest {
                preferred_monitor_id: 16,
                ..good
            },
            "monitor id",
        ),
        (AddRequest { flags: 2, ..good }, "flags"),
    ] {
        assert_eq!(
            AddRequest::decode(&bad.encode()),
            Err(WireError::Invalid(field))
        );
    }
    // 480 Hz is a mode the driver serves.
    assert!(
        AddRequest {
            refresh_mhz: 480_000,
            ..good
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn replies_round_trip() {
    let info = InfoReply {
        magic: INFO_MAGIC,
        protocol_version: crate::PROTOCOL_VERSION,
        watchdog_timeout_ms: 10_000,
        max_monitors: 16,
        max_ring_slots: ring::MAX_SLOTS as u32,
        caps: CAP_FRAME_RING | CAP_HDR | CAP_UPDATE_MODES,
        driver_version: 0x0001_0000,
    };
    let bytes = info.encode();
    assert_eq!(&bytes[..4], b"RLVD");
    assert_eq!(InfoReply::decode(&bytes), Ok(info));
    assert!(info.compatible());
    assert!(
        !InfoReply {
            protocol_version: 2,
            ..info
        }
        .compatible()
    );
    let mut foreign = bytes;
    foreign[0] = b'P';
    assert_eq!(
        InfoReply::decode(&foreign),
        Err(WireError::Invalid("driver magic"))
    );

    let add = AddReply {
        adapter_luid_low: 0x1234_5678,
        adapter_luid_high: -2,
        target_id: 262,
        monitor_id: 3,
        wudf_pid: 4242,
    };
    let bytes = add.encode();
    assert_eq!(bytes[4..8], (-2i32).to_le_bytes());
    assert_eq!(AddReply::decode(&bytes), Ok(add));
    assert!(
        AddReply::decode(
            &AddReply {
                target_id: 0,
                ..add
            }
            .encode()
        )
        .is_err()
    );

    let status = MonitorStatus {
        target_id: 262,
        monitor_id: 3,
        render_luid_low: 0xABCD,
        render_luid_high: 0,
        flags: MONITOR_ARRIVED | MONITOR_SWAP_CHAIN | MONITOR_RING | MONITOR_RING_OPEN,
        surface_width: 2560,
        surface_height: 1440,
        surface_format: ring::FORMAT_RGBA16F,
        frames_acquired: 1 << 40,
        frames_pushed: 99,
        ring_generation: 4,
        width: 2560,
        height: 1440,
        refresh_mhz: 240_000,
    };
    let bytes = status.encode();
    assert_eq!(bytes[32..40], (1u64 << 40).to_le_bytes());
    assert_eq!(MonitorStatus::decode(&bytes), Ok(status));
}

#[test]
fn small_requests_round_trip_and_validate() {
    let session = SessionRequest { session_id: 9 };
    assert_eq!(SessionRequest::decode(&session.encode()), Ok(session));

    let clear = ClearFrameRingRequest {
        session_id: 9,
        generation: 4,
    };
    assert_eq!(ClearFrameRingRequest::decode(&clear.encode()), Ok(clear));
    let unnamed = ClearFrameRingRequest {
        generation: 0,
        ..clear
    };
    assert!(ClearFrameRingRequest::decode(&unnamed.encode()).is_err());

    let update = UpdateModesRequest {
        session_id: 9,
        width: 2560,
        height: 1409,
        refresh_mhz: 120_000,
    };
    assert_eq!(UpdateModesRequest::decode(&update.encode()), Ok(update));
    assert!(
        UpdateModesRequest::decode(
            &UpdateModesRequest {
                height: 0,
                ..update
            }
            .encode()
        )
        .is_err()
    );

    let pin = RenderAdapterRequest {
        luid_low: 0xD00D,
        luid_high: 1,
    };
    assert_eq!(RenderAdapterRequest::decode(&pin.encode()), Ok(pin));
    assert!(
        RenderAdapterRequest::decode(
            &RenderAdapterRequest {
                luid_low: 0,
                luid_high: 0
            }
            .encode()
        )
        .is_err()
    );
}

fn ring_request() -> SetFrameRingRequest {
    let mut textures = [0; ring::MAX_SLOTS];
    textures[..3].copy_from_slice(&[0x104, 0x108, 0x10C]);
    SetFrameRingRequest {
        session_id: 0x6000_0000_0000_0042,
        generation: 2,
        slot_count: 3,
        width: 1920,
        height: 1080,
        format: ring::FORMAT_BGRA8,
        header_bytes: ring::SECTION_BYTES as u32,
        adapter_luid_low: 0xC0FFEE,
        adapter_luid_high: 0,
        header_section: 0x200,
        event: 0x204,
        textures,
    }
}

#[test]
fn frame_ring_request_round_trips_at_fixed_offsets() {
    let request = ring_request();
    let bytes = request.encode();
    assert_eq!(bytes.len(), 128);
    assert_eq!(bytes[8..12], 2u32.to_le_bytes());
    assert_eq!(bytes[24..28], 87u32.to_le_bytes());
    assert_eq!(bytes[40..48], 0x200u64.to_le_bytes());
    assert_eq!(bytes[48..56], 0x204u64.to_le_bytes());
    assert_eq!(bytes[56..64], 0x104u64.to_le_bytes());
    assert_eq!(bytes[72..80], 0x10Cu64.to_le_bytes());
    assert_eq!(bytes[80..128], [0; 48]);
    assert_eq!(SetFrameRingRequest::decode(&bytes), Ok(request));
    let handles: Vec<u64> = request.handles().collect();
    assert_eq!(handles, [0x200, 0x204, 0x104, 0x108, 0x10C]);
}

#[test]
fn frame_ring_request_refuses_inconsistent_rings() {
    let good = ring_request();
    let mut gap = good;
    gap.textures[1] = 0;
    let mut extra = good;
    extra.textures[5] = 0x300;
    let cases = [
        (
            SetFrameRingRequest {
                generation: 0,
                ..good
            },
            "generation",
        ),
        (
            SetFrameRingRequest {
                generation: 1 << 24,
                ..good
            },
            "generation",
        ),
        (
            SetFrameRingRequest {
                slot_count: 1,
                ..good
            },
            "slot count",
        ),
        (
            SetFrameRingRequest {
                slot_count: 9,
                ..good
            },
            "slot count",
        ),
        (SetFrameRingRequest { width: 0, ..good }, "size"),
        (SetFrameRingRequest { format: 28, ..good }, "format"),
        (
            SetFrameRingRequest {
                header_bytes: 64,
                ..good
            },
            "header size",
        ),
        (SetFrameRingRequest { event: 0, ..good }, "handle"),
        (gap, "textures"),
        (extra, "textures"),
    ];
    for (bad, field) in cases {
        assert_eq!(
            SetFrameRingRequest::decode(&bad.encode()),
            Err(WireError::Invalid(field)),
            "{field}"
        );
    }
    // FP16 is the HDR desktop's format.
    assert!(
        SetFrameRingRequest {
            format: ring::FORMAT_RGBA16F,
            ..good
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn fence_ring_request_extends_the_ring_request() {
    let request = SetFenceRingRequest {
        ring: ring_request(),
        written_fence: 0x300,
        released_fence: 0x304,
    };
    let bytes = request.encode();
    assert_eq!(bytes.len(), 152);
    assert_eq!(bytes[..128], ring_request().encode());
    assert_eq!(bytes[128..136], 0x300u64.to_le_bytes());
    assert_eq!(bytes[136..144], 0x304u64.to_le_bytes());
    assert_eq!(bytes[144..152], [0; 8]);
    assert_eq!(SetFenceRingRequest::decode(&bytes), Ok(request));
    let handles: Vec<u64> = request.handles().collect();
    assert_eq!(handles, [0x200, 0x204, 0x104, 0x108, 0x10C, 0x300, 0x304]);
    // A keyed-mutex request is not a fence ring request, nor the other way round: an
    // older driver refuses the fence ring by its size, never misreads it.
    assert!(SetFenceRingRequest::decode(&ring_request().encode()).is_err());
    assert!(SetFrameRingRequest::decode(&bytes).is_err());
    let mut reserved = bytes;
    reserved[150] = 1;
    assert_eq!(
        SetFenceRingRequest::decode(&reserved),
        Err(WireError::Invalid("reserved"))
    );
    for (written_fence, released_fence) in [(0, 0x304), (0x300, 0), (0x300, 0x300)] {
        let bad = SetFenceRingRequest {
            written_fence,
            released_fence,
            ..request
        };
        assert_eq!(
            SetFenceRingRequest::decode(&bad.encode()),
            Err(WireError::Invalid("fences"))
        );
    }
    let mut ring = ring_request();
    let duplicate = SetFenceRingRequest {
        written_fence: ring.header_section,
        released_fence: 0x304,
        ring,
    };
    assert_eq!(
        duplicate.validate(),
        Err(WireError::Invalid("duplicate handles"))
    );
    ring.slot_count = 1;
    assert_eq!(
        SetFenceRingRequest::decode(&SetFenceRingRequest { ring, ..request }.encode()),
        Err(WireError::Invalid("slot count")),
        "the ring part is validated too"
    );
}

#[test]
fn capabilities_are_distinct_bits() {
    let caps = [
        CAP_FRAME_RING,
        CAP_HDR,
        CAP_UPDATE_MODES,
        CAP_SCOPED_CLEAR,
        CAP_FENCE_RING,
    ];
    for (i, a) in caps.iter().enumerate() {
        assert_eq!(a.count_ones(), 1);
        for b in &caps[i + 1..] {
            assert_eq!(a & b, 0);
        }
    }
}

#[test]
fn log_records_round_trip_one_line_each() {
    let mut buf = Vec::new();
    write_log_record(&mut buf, LOG_WARN, "ring open failed");
    write_log_record(&mut buf, LOG_INFO, "two\nlines");
    let got: Vec<_> = log_lines(&buf).collect();
    assert_eq!(
        got,
        [(LOG_WARN, "ring open failed"), (LOG_INFO, "two lines")]
    );
    assert_eq!(buf.iter().filter(|&&b| b == b'\n').count(), 2);
}
