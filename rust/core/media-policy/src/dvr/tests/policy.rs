//! Slice reassembly, the soft keyframe cadence, clip ids and the quota plan.

use crate::dvr::cadence::RETRY;
use crate::dvr::{
    ClipKind, DEFAULT_QUOTA_BYTES, DISK_FLOOR_BYTES, GOP_SECS, KeyframeCadence, Reassembler,
    Refusal, StoredClip, UnitKind, VideoCodec, gop, plan, valid_clip_id,
};
use std::time::{Duration, Instant};

#[test]
fn whole_units_pass_through() {
    let mut slices = Reassembler::new();
    let frame = slices.push(UnitKind::Whole, true, 5, &[1, 2, 3]).unwrap();
    assert_eq!(
        (frame.pts_us, frame.idr, frame.data.as_slice()),
        (5, true, &[1, 2, 3][..])
    );
    assert!(!slices.take_discontinuity());
}

#[test]
fn slices_join_in_order() {
    let mut slices = Reassembler::default();
    assert!(
        slices
            .push(UnitKind::Slice { index: 0 }, true, 9, &[1])
            .is_none()
    );
    assert!(
        slices
            .push(UnitKind::Slice { index: 1 }, false, 9, &[2, 3])
            .is_none()
    );
    let frame = slices
        .push(UnitKind::Last { index: 2 }, false, 9, &[4])
        .unwrap();
    assert_eq!(frame.data.as_slice(), [1, 2, 3, 4]);
    assert!(frame.idr);
    let single = slices
        .push(UnitKind::Last { index: 0 }, false, 10, &[7])
        .unwrap();
    assert_eq!(single.data.as_slice(), [7]);
    assert!(!slices.take_discontinuity());
}

#[test]
fn out_of_order_slices_drop_the_frame() {
    let mut slices = Reassembler::new();
    slices.push(UnitKind::Slice { index: 0 }, false, 1, &[1]);
    assert!(
        slices
            .push(UnitKind::Slice { index: 2 }, false, 1, &[3])
            .is_none()
    );
    assert!(slices.take_discontinuity());
    assert!(!slices.take_discontinuity(), "taking clears it");
    assert!(
        slices
            .push(UnitKind::Last { index: 3 }, false, 1, &[4])
            .is_none()
    );
    assert!(slices.take_discontinuity());
    let next = slices
        .push(UnitKind::Last { index: 0 }, true, 2, &[5])
        .unwrap();
    assert_eq!(next.pts_us, 2);
}

#[test]
fn a_new_frame_while_one_is_partial_reports_the_loss() {
    let mut slices = Reassembler::new();
    slices.push(UnitKind::Slice { index: 0 }, false, 1, &[1]);
    let whole = slices.push(UnitKind::Whole, false, 2, &[2]).unwrap();
    assert_eq!(whole.data.as_slice(), [2]);
    assert!(slices.take_discontinuity());

    slices.push(UnitKind::Slice { index: 0 }, false, 3, &[1]);
    slices.push(UnitKind::Slice { index: 0 }, false, 4, &[9]);
    assert!(slices.take_discontinuity());
    let frame = slices
        .push(UnitKind::Last { index: 1 }, false, 4, &[8])
        .unwrap();
    assert_eq!(frame.data.as_slice(), [9, 8]);

    slices.push(UnitKind::Slice { index: 0 }, false, 5, &[1]);
    assert!(
        slices
            .push(UnitKind::Last { index: 1 }, false, 6, &[2])
            .is_none()
    );
    assert!(slices.take_discontinuity(), "a slice of another frame");

    slices.push(UnitKind::Slice { index: 0 }, false, 7, &[1]);
    slices.reset();
    assert!(!slices.take_discontinuity());
}

#[test]
fn gop_length_is_clamped() {
    assert_eq!(GOP_SECS, 4);
    assert_eq!(gop(GOP_SECS), Duration::from_secs(4));
    assert_eq!(gop(0), Duration::from_secs(2));
    assert_eq!(gop(30), Duration::from_secs(10));
}

#[test]
fn cadence_asks_for_a_keyframe_once_per_stale_gop() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut cadence = KeyframeCadence::new(gop(GOP_SECS));
    assert!(cadence.due(at(0)), "no keyframe yet");
    assert!(!cadence.due(at(500)), "request outstanding");
    assert!(cadence.due(at(RETRY.as_millis() as u64)), "retried");
    cadence.observe_idr(at(1_100));
    assert!(!cadence.due(at(5_000)));
    assert!(cadence.due(at(5_100)));
    assert!(!cadence.due(at(5_600)));
    cadence.observe_idr(at(5_700));
    assert!(!cadence.due(at(6_000)), "answered");
    assert!(!cadence.due(at(9_600)));
    assert!(cadence.due(at(9_700)));
    cadence.reset();
    assert!(cadence.due(at(9_800)));
}

#[test]
fn clip_ids_are_safe_file_stems() {
    assert!(valid_clip_id("abcdEFGH"));
    assert!(valid_clip_id("2026-11-26_replay-01"));
    assert!(valid_clip_id(&"a".repeat(64)));
    assert!(!valid_clip_id("short"));
    assert!(!valid_clip_id(&"a".repeat(65)));
    for bad in [
        "../../etc",
        "abc def gh",
        "abcdefg.mp4",
        "abcdefgh\\",
        "abcdéfgh",
    ] {
        assert!(!valid_clip_id(bad), "{bad}");
    }
}

fn clip(id: &str, bytes: u64, created: u64, kind: ClipKind, open: bool) -> StoredClip {
    StoredClip {
        id: id.into(),
        bytes,
        created_unix_ms: created,
        kind,
        open,
    }
}

#[test]
fn quota_plan_evicts_the_oldest_replays_only() {
    const GIB: u64 = 1 << 30;
    assert_eq!(DEFAULT_QUOTA_BYTES, 20 * GIB);
    assert_eq!(DISK_FLOOR_BYTES, GIB);
    let clips = [
        clip("manual-oldest", 4 * GIB, 1, ClipKind::Manual, false),
        clip("replay-b", 3 * GIB, 3, ClipKind::Replay, false),
        clip("replay-a", 2 * GIB, 2, ClipKind::Replay, false),
        clip("replay-open", GIB, 0, ClipKind::Replay, true),
        clip("replay-c", 5 * GIB, 4, ClipKind::Replay, false),
    ];
    let used = 15 * GIB;
    assert_eq!(plan(&clips, GIB, 20 * GIB, None, GIB), Ok(vec![]));
    assert_eq!(
        plan(&clips, 8 * GIB, 20 * GIB, None, GIB),
        Ok(vec!["replay-a".to_string(), "replay-b".to_string()])
    );
    assert_eq!(
        plan(&clips, 20 * GIB - used + 10 * GIB, 20 * GIB, None, GIB),
        Ok(vec![
            "replay-a".to_string(),
            "replay-b".to_string(),
            "replay-c".to_string()
        ])
    );
    assert_eq!(
        plan(&clips, 16 * GIB, 20 * GIB, None, GIB),
        Err(Refusal::QuotaExceeded),
        "manual and open clips stay"
    );
}

#[test]
fn quota_plan_keeps_the_disk_floor() {
    const GIB: u64 = 1 << 30;
    let clips = [clip("replay-a", 2 * GIB, 1, ClipKind::Replay, false)];
    assert_eq!(
        plan(&clips, GIB, 100 * GIB, Some(GIB), GIB),
        Err(Refusal::DiskFull)
    );
    assert_eq!(plan(&clips, GIB, 100 * GIB, Some(2 * GIB), GIB), Ok(vec![]));
    // Evicting for the quota frees disk space too.
    assert_eq!(
        plan(&clips, 2 * GIB, 2 * GIB, Some(GIB), GIB),
        Ok(vec!["replay-a".to_string()])
    );
    assert_eq!(
        plan(&[], GIB, 2 * GIB, Some(GIB / 2), GIB),
        Err(Refusal::DiskFull)
    );
}

#[test]
fn codecs_map_from_the_negotiated_value() {
    assert_eq!(VideoCodec::from_negotiated(0), Some(VideoCodec::H264));
    assert_eq!(VideoCodec::from_negotiated(1), Some(VideoCodec::Hevc));
    assert_eq!(VideoCodec::from_negotiated(2), Some(VideoCodec::Av1));
    assert_eq!(VideoCodec::from_negotiated(3), None, "PyroWave");
    assert_eq!(VideoCodec::from_negotiated(9), None);
}
