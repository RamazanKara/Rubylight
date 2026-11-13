use super::*;

#[test]
fn link_notice_requests_round_trip() {
    for enabled in [true, false] {
        let request = LinkNoticeRequest { enabled };
        let bytes = request.encode();
        assert_eq!(bytes.len(), LINK_NOTICE_REQUEST_BYTES);
        assert_eq!(LinkNoticeRequest::decode(&bytes), Some(request));
    }
    assert_eq!(LINK_NOTICE_MESSAGE_TYPE, 0x5538);
    assert_eq!(LinkNoticeRequest { enabled: true }.encode(), [1, 1, 0, 0]);
    assert_eq!(LinkNoticeRequest::decode(&[1, 1, 0]), None);
    assert_eq!(LinkNoticeRequest::decode(&[2, 1, 0, 0]), None);
    let mut longer = LinkNoticeRequest { enabled: true }.encode().to_vec();
    longer.extend_from_slice(&[9; 4]);
    assert_eq!(
        LinkNoticeRequest::decode(&longer),
        Some(LinkNoticeRequest { enabled: true })
    );
}

#[test]
fn link_notices_round_trip_and_reject_nonsense() {
    let limited = LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 14_988,
        current_kbps: 9_100,
    };
    let bytes = limited.encode();
    assert_eq!(bytes.len(), LINK_NOTICE_BYTES);
    assert_eq!(bytes, [1, 1, 1, 0, 0x8c, 0x3a, 0, 0, 0x8c, 0x23, 0, 0]);
    assert_eq!(LinkNotice::decode(&bytes), Some(limited));
    assert!(limited.limited());
    let full = LinkNotice {
        state: LinkState::Full,
        reason: LinkReason::None,
        ceiling_kbps: 14_988,
        current_kbps: 14_988,
    };
    assert_eq!(LinkNotice::decode(&full.encode()), Some(full));
    assert!(!full.limited());
    let floor = LinkNotice {
        state: LinkState::Floor,
        reason: LinkReason::Network,
        ceiling_kbps: 40_000,
        current_kbps: 2_000,
    };
    assert_eq!(LinkNotice::decode(&floor.encode()), Some(floor));

    let mut longer = bytes.to_vec();
    longer.extend_from_slice(&[0xff; 4]);
    assert_eq!(
        LinkNotice::decode(&longer),
        Some(limited),
        "newer hosts may append fields"
    );
    assert_eq!(LinkNotice::decode(&bytes[..LINK_NOTICE_BYTES - 1]), None);
    let edit = |at: usize, value: u8| {
        let mut changed = bytes;
        changed[at] = value;
        LinkNotice::decode(&changed)
    };
    assert_eq!(edit(0, 2), None, "unknown version");
    assert_eq!(edit(1, 3), None, "unknown state");
    assert_eq!(edit(2, 2), None, "unknown reason");
    assert_eq!(edit(2, 0), None, "limited without a reason");
    let mut with_reason = full.encode();
    with_reason[2] = 1;
    assert_eq!(LinkNotice::decode(&with_reason), None, "full with a reason");
    for zero in [
        LinkNotice {
            ceiling_kbps: 0,
            ..limited
        },
        LinkNotice {
            current_kbps: 0,
            ..limited
        },
    ] {
        assert_eq!(LinkNotice::decode(&zero.encode()), None, "{zero:?}");
    }
}

#[test]
fn display_caps_round_trip_and_reject_nonsense() {
    // A Galaxy Z Fold 7 inner screen: 2600 nits peak, 0.0005 nits black.
    let caps = DisplayCaps {
        hdr: true,
        max_centinits: 260_000,
        max_average_centinits: 120_000,
        min_decimillinits: 5,
    };
    let bytes = caps.encode();
    assert_eq!(DisplayCaps::decode(&bytes), Some(caps));
    assert_eq!(caps.max_nits(), Some(2600));
    assert_eq!(caps.max_average_nits(), Some(1200));
    let mut longer = bytes.to_vec();
    longer.extend_from_slice(&[0xff; 4]);
    assert_eq!(
        DisplayCaps::decode(&longer),
        Some(caps),
        "newer clients may append fields"
    );
    assert_eq!(DisplayCaps::decode(&bytes[..DISPLAY_CAPS_BYTES - 1]), None);
    let mut version = bytes;
    version[0] = 2;
    assert_eq!(DisplayCaps::decode(&version), None);
    let unknown = DisplayCaps {
        hdr: false,
        ..DisplayCaps::default()
    };
    assert_eq!(DisplayCaps::decode(&unknown.encode()), Some(unknown));
    assert_eq!(unknown.max_nits(), None);
    for wild in [
        DisplayCaps {
            max_centinits: 5_000_000,
            ..caps
        },
        DisplayCaps {
            max_average_centinits: 300_000,
            ..caps
        },
        DisplayCaps {
            min_decimillinits: 20_000,
            ..caps
        },
    ] {
        assert_eq!(DisplayCaps::decode(&wild.encode()), None, "{wild:?}");
    }
}

#[test]
fn reconfigure_round_trips_and_rejects_nonsense() {
    let request = Reconfigure {
        width: 2176,
        height: 1812,
        fps_millihz: 120_000,
    };
    let bytes = request.encode();
    assert_eq!(Reconfigure::decode(&bytes), Some(request));
    assert_eq!(Reconfigure::decode(&bytes[..RECONFIGURE_BYTES - 1]), None);
    for wild in [
        Reconfigure {
            width: 2177,
            ..request
        },
        Reconfigure {
            height: 100,
            ..request
        },
        Reconfigure {
            width: 10_000,
            ..request
        },
        Reconfigure {
            fps_millihz: 1_000,
            ..request
        },
        Reconfigure {
            fps_millihz: 1_000_000,
            ..request
        },
    ] {
        assert_eq!(Reconfigure::decode(&wild.encode()), None, "{wild:?}");
    }
}

#[test]
fn clock_sync_round_trips_and_rejects_nonsense() {
    let request = ClockSyncRequest {
        sequence: 7,
        client_us: 0x0102_0304_0506_0708,
    };
    let bytes = request.encode();
    assert_eq!(&bytes[..8], &[1, 0, 0, 0, 7, 0, 0, 0]);
    assert_eq!(ClockSyncRequest::decode(&bytes), Some(request));
    assert_eq!(ClockSyncRequest::decode(&bytes[..15]), None);
    let mut version = bytes;
    version[0] = 2;
    assert_eq!(ClockSyncRequest::decode(&version), None);

    let reply = ClockSyncReply {
        sequence: 7,
        client_us: request.client_us,
        host_receive_us: 5_000_000,
        host_send_us: 5_000_040,
    };
    let bytes = reply.encode();
    assert_eq!(ClockSyncReply::decode(&bytes), Some(reply));
    let mut longer = bytes.to_vec();
    longer.extend_from_slice(&[0xff; 8]);
    assert_eq!(ClockSyncReply::decode(&longer), Some(reply), "newer hosts may append");
    assert_eq!(ClockSyncReply::decode(&bytes[..31]), None);
    let backwards = ClockSyncReply {
        host_send_us: 4_999_999,
        ..reply
    };
    assert_eq!(ClockSyncReply::decode(&backwards.encode()), None);
}

#[test]
fn host_timing_request_round_trips() {
    for enabled in [true, false] {
        let request = HostTimingRequest { enabled };
        let bytes = request.encode();
        assert_eq!(bytes, [1, u8::from(enabled), 0, 0]);
        assert_eq!(HostTimingRequest::decode(&bytes), Some(request));
    }
    assert_eq!(HostTimingRequest::decode(&[1, 1, 0]), None);
    assert_eq!(HostTimingRequest::decode(&[2, 1, 0, 0]), None);
}

#[test]
fn host_timing_reports_round_trip_and_reject_nonsense() {
    let records: Vec<HostTiming> = (0..3)
        .map(|i| HostTiming {
            frame: 1000 + i,
            host_us: 3_200,
            queue_us: 400,
            encode_us: 2_500,
            send_us: 120,
        })
        .collect();
    let bytes = HostTiming::encode_report(&records);
    assert_eq!(
        bytes.len(),
        HOST_TIMING_HEADER_BYTES + 3 * HOST_TIMING_RECORD_BYTES
    );
    assert_eq!(&bytes[..8], &[1, 3, 0, 0, 0xe8, 0x03, 0, 0]);
    assert_eq!(HostTiming::decode_report(&bytes), Some(records.clone()));
    assert_eq!(records[0].residual_us(), 180);
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0xff; 7]);
    assert_eq!(
        HostTiming::decode_report(&longer),
        Some(records.clone()),
        "newer hosts may append bytes"
    );
    assert_eq!(HostTiming::decode_report(&bytes[..bytes.len() - 1]), None);
    let mut version = bytes.clone();
    version[0] = 2;
    assert_eq!(HostTiming::decode_report(&version), None);
    assert_eq!(HostTiming::decode_report(&[1, 0, 0, 0]), None);
    let mut too_many = bytes.clone();
    too_many[1] = (HOST_TIMING_MAX_RECORDS + 1) as u8;
    assert_eq!(HostTiming::decode_report(&too_many), None);
    let over = HostTiming {
        send_us: 900,
        ..records[0]
    };
    assert_eq!(
        HostTiming::decode_report(&HostTiming::encode_report(&[over])),
        None
    );
    let many = alloc::vec![records[0]; HOST_TIMING_MAX_RECORDS + 5];
    let capped = HostTiming::encode_report(&many);
    assert_eq!(usize::from(capped[1]), HOST_TIMING_MAX_RECORDS);
    assert_eq!(
        HostTiming::decode_report(&capped).map(|r| r.len()),
        Some(HOST_TIMING_MAX_RECORDS)
    );
}

fn extended_sample(count: u32) -> Vec<(HostTiming, HostTimingExtra)> {
    (0..count)
        .map(|i| {
            (
                HostTiming {
                    frame: 1000 + i,
                    host_us: 3_200,
                    queue_us: 400,
                    encode_us: 2_500,
                    send_us: 120,
                },
                HostTimingExtra {
                    presented_at_us: 5_000_000_000 + u64::from(i) * 16_667,
                    capture_us: 150,
                    convert_us: 200,
                },
            )
        })
        .collect()
}

#[test]
fn host_timing_extended_report_round_trips() {
    let records = extended_sample(3);
    let bytes = HostTiming::encode_report_with(&records);
    assert_eq!(
        bytes.len(),
        HOST_TIMING_HEADER_BYTES + 3 * (HOST_TIMING_RECORD_BYTES + HOST_TIMING_EXTRA_BYTES)
    );
    assert_eq!(&bytes[..4], &[1, 3, 16, 0]);
    let decoded = HostTiming::decode_report_with(&bytes).unwrap();
    let expected: Vec<_> = records.iter().map(|(r, x)| (*r, Some(*x))).collect();
    assert_eq!(decoded, expected);
}

#[test]
fn host_timing_old_decoder_reads_extended_report() {
    let records = extended_sample(HOST_TIMING_EXTENDED_MAX_RECORDS as u32);
    let bytes = HostTiming::encode_report_with(&records);
    let base: Vec<HostTiming> = records.iter().map(|(r, _)| *r).collect();
    assert_eq!(HostTiming::decode_report(&bytes), Some(base.clone()));
    assert_eq!(
        HostTiming::decode_report(&HostTiming::encode_report(&base)),
        Some(base)
    );
}

#[test]
fn host_timing_report_without_extension_has_no_extras() {
    let records = extended_sample(2);
    let base: Vec<HostTiming> = records.iter().map(|(r, _)| *r).collect();
    let decoded = HostTiming::decode_report_with(&HostTiming::encode_report(&base)).unwrap();
    assert_eq!(decoded.len(), 2);
    assert!(decoded.iter().all(|(_, x)| x.is_none()));
    assert_eq!(decoded[1].0, base[1]);
}

#[test]
fn host_timing_extension_over_queue_is_dropped_and_base_kept() {
    let mut records = extended_sample(2);
    records[1].1.convert_us = 251;
    let bytes = HostTiming::encode_report_with(&records);
    let decoded = HostTiming::decode_report_with(&bytes).unwrap();
    assert_eq!(decoded[0], (records[0].0, Some(records[0].1)));
    assert_eq!(decoded[1], (records[1].0, None));
    records[1].1.convert_us = 250;
    let ok = HostTiming::decode_report_with(&HostTiming::encode_report_with(&records));
    assert_eq!(ok.unwrap()[1].1, Some(records[1].1));
}

#[test]
fn host_timing_extended_report_is_capped_at_616_bytes() {
    let many = extended_sample(HOST_TIMING_EXTENDED_MAX_RECORDS as u32 + 8);
    let bytes = HostTiming::encode_report_with(&many);
    assert_eq!(bytes.len(), 616);
    // Under the 644 bytes of a full v1 report, which shipped clients still accept.
    assert!(
        bytes.len()
            <= HOST_TIMING_HEADER_BYTES + HOST_TIMING_MAX_RECORDS * HOST_TIMING_RECORD_BYTES
    );
    assert_eq!(usize::from(bytes[1]), HOST_TIMING_EXTENDED_MAX_RECORDS);
    let decoded = HostTiming::decode_report_with(&bytes).unwrap();
    assert_eq!(decoded.len(), HOST_TIMING_EXTENDED_MAX_RECORDS);
    assert!(decoded.iter().all(|(_, x)| x.is_some()));
    // More records than the extended limit: the base records are kept and any
    // appended extras are not trusted.
    let base: Vec<HostTiming> = many.iter().map(|(r, _)| *r).collect();
    let mut wide = HostTiming::encode_report(&base);
    wide[2] = 16;
    wide.extend_from_slice(&alloc::vec![0; base.len() * 16]);
    let decoded = HostTiming::decode_report_with(&wide).unwrap();
    assert_eq!(decoded.len(), base.len());
    assert!(decoded.iter().all(|(_, x)| x.is_none()));
}

#[test]
fn host_timing_extension_skips_bytes_past_the_known_fields() {
    let records = extended_sample(2);
    let bytes = HostTiming::encode_report_with(&records);
    let base_end = HOST_TIMING_HEADER_BYTES + 2 * HOST_TIMING_RECORD_BYTES;
    // Entries of 24 bytes: the 16 known bytes, then 8 newer ones.
    let mut wider = bytes[..base_end].to_vec();
    wider[2] = 24;
    for i in 0..2 {
        let at = base_end + i * 16;
        wider.extend_from_slice(&bytes[at..at + 16]);
        wider.extend_from_slice(&[0xee; 8]);
    }
    let decoded = HostTiming::decode_report_with(&wider).unwrap();
    for (i, (r, x)) in decoded.iter().enumerate() {
        assert_eq!(*r, records[i].0);
        assert_eq!(*x, Some(records[i].1));
    }
    // Declared extra bytes missing: base kept, no extras.
    let decoded = HostTiming::decode_report_with(&bytes[..bytes.len() - 1]).unwrap();
    assert!(decoded.iter().all(|(_, x)| x.is_none()));
    // Trailing bytes after the entries are ignored.
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0xff; 5]);
    assert_eq!(
        HostTiming::decode_report_with(&longer),
        HostTiming::decode_report_with(&bytes)
    );
}
