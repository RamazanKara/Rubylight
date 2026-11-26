//! Unit and golden tests for Frame Warp 2's request, meta and gyro helpers.

use super::*;
use crate::frame_warp::{HELD_NONE, REQUEST_BYTES as V1_REQUEST_BYTES};

fn base() -> FrameWarpRequest {
    FrameWarpRequest {
        enabled: true,
        controller: Some(3),
        mask_max_width: 960,
        lens: true,
    }
}

#[test]
fn request_v2_golden_bytes_round_trip() {
    let request = RequestV2 {
        gyro: true,
        edge_v2: true,
        confidence_v2: true,
        gyro_controller: Some(1),
        axes: GyroAxes {
            invert_yaw: true,
            invert_pitch: false,
            yaw: YawSource::Auto,
        },
        max_offset_milli: 120,
        ..RequestV2::new(base())
    };
    let wire = request.encode();
    assert_eq!(wire, [2, 1, 3, 0b1111, 0xC0, 0x03, 0, 0, 1, 0b1001, 120, 0]);
    assert_eq!(RequestV2::decode(&wire), Some(request));
    // A later version's trailing bytes and high axis bits are ignored.
    let mut longer = wire.to_vec();
    longer[9] |= 0xF0;
    longer.extend_from_slice(&[7, 7]);
    assert_eq!(RequestV2::decode(&longer), Some(request));
    assert_eq!(request.gyro_pad(), Some(1));
    assert!((request.max_offset() - 0.12).abs() < 1e-6);
}

#[test]
fn a_v1_host_reads_a_v2_request_as_version_1() {
    let request = RequestV2 {
        gyro: true,
        ..RequestV2::new(base())
    };
    // Version 1's decoder takes the prefix and ignores the flags it does not know.
    assert_eq!(FrameWarpRequest::decode(&request.encode()), Some(base()));
}

#[test]
fn a_v1_request_is_not_a_v2_request() {
    let v1 = base().encode();
    assert_eq!(v1.len(), V1_REQUEST_BYTES);
    assert_eq!(RequestV2::decode(&v1), None);
    // Version 1 padded to twelve bytes is still version 1.
    let mut padded = v1.to_vec();
    padded.extend_from_slice(&[0; 4]);
    assert_eq!(RequestV2::decode(&padded), None);
    // Version 2 cut to eight bytes reads as version 1 only.
    let short = RequestV2::new(base()).encode();
    assert_eq!(RequestV2::decode(&short[..8]), None);
    assert_eq!(FrameWarpRequest::decode(&short[..8]), Some(base()));
}

#[test]
fn request_v2_rejects_bad_fields() {
    let good = RequestV2::new(base()).encode();
    let with = |at: usize, value: u8| {
        let mut bad = good;
        bad[at] = value;
        RequestV2::decode(&bad)
    };
    assert_eq!(with(1, 2), None, "enabled 2");
    assert_eq!(with(2, 16), None, "stick pad 16");
    assert_eq!(with(8, 16), None, "gyro pad 16");
    assert_eq!(with(9, 0b1100), None, "yaw source 3");
    assert_eq!(with(11, 0x04), None, "limit past the picture");
    assert!(with(8, NO_CONTROLLER).is_some());
    let none = RequestV2::decode(&good).unwrap();
    assert_eq!(none.gyro_pad(), None, "no gyro without the flag");
    assert_eq!(none.max_offset(), DEFAULT_MAX_OFFSET);
    // The gyro falls back to the stick's pad.
    let same = RequestV2 { gyro: true, ..none };
    assert_eq!(same.gyro_pad(), Some(3));
}

fn meta() -> FrameWarpV2Meta {
    FrameWarpV2Meta {
        flags: meta_flags::GYRO_LOCKED | meta_flags::DISOCCLUSION_V2,
        model_generation: 0x0102,
        frame: 0x0A0B_0C0D,
        camera_confidence: 0xFFFF,
        hud_stability: 0x8000,
        gyro_seq: 42,
        gyro_held_us: 1500,
        gyro_gain: [1000.0, -500.0],
        omega_mrad_s: [-1234, 56],
        edge_fade_strength: DEFAULT_EDGE_FADE,
        hud_guard: DEFAULT_HUD_GUARD,
    }
}

#[test]
fn meta_golden_bytes_round_trip() {
    let wire = meta().encode().unwrap();
    let mut expected = [0u8; META_BYTES];
    expected[..20].copy_from_slice(&[
        2, 0b10_0001, 0x02, 0x01, 0x0D, 0x0C, 0x0B, 0x0A, 0xFF, 0xFF, 0x00, 0x80, 42, 0, 0, 0,
        0xDC, 0x05, 0, 0,
    ]);
    expected[20..24].copy_from_slice(&1000.0f32.to_le_bytes());
    expected[24..28].copy_from_slice(&(-500.0f32).to_le_bytes());
    expected[28..32].copy_from_slice(&(-1234i32).to_le_bytes());
    expected[32..36].copy_from_slice(&56i32.to_le_bytes());
    expected[36] = 90;
    expected[37] = 2;
    assert_eq!(wire, expected);
    assert_eq!(FrameWarpV2Meta::decode(&wire), Some(meta()));
    assert!((meta().confidence() - 1.0).abs() < 1e-6);
    assert!(meta().has(meta_flags::GYRO_LOCKED));
    assert!(!meta().has(meta_flags::SCENE_CUT));
}

#[test]
fn meta_ignores_what_a_later_version_adds() {
    let mut wire = meta().encode().unwrap().to_vec();
    wire[0] = 3;
    wire[1] |= 0x80;
    wire[38] = 9;
    wire.extend_from_slice(&[1, 2, 3]);
    assert_eq!(FrameWarpV2Meta::decode(&wire), Some(meta()));
}

#[test]
fn meta_rejects_bad_fields() {
    let good = meta().encode().unwrap();
    assert_eq!(FrameWarpV2Meta::decode(&good[..META_BYTES - 1]), None);
    let mut zero = good;
    zero[0] = 0;
    assert_eq!(FrameWarpV2Meta::decode(&zero), None);
    let mut guard = good;
    guard[37] = MAX_HUD_GUARD + 1;
    assert_eq!(FrameWarpV2Meta::decode(&guard), None);
    for gain in [f32::NAN, f32::INFINITY, MAX_GAIN * 2.0] {
        let mut bad = good;
        bad[20..24].copy_from_slice(&gain.to_le_bytes());
        assert_eq!(FrameWarpV2Meta::decode(&bad), None, "{gain}");
        let mut out = meta();
        out.gyro_gain[1] = gain;
        assert_eq!(out.encode(), None, "{gain}");
    }
    let mut out = meta();
    out.hud_guard = 9;
    assert_eq!(out.encode(), None);
    // Unknown flags never go out.
    out = FrameWarpV2Meta {
        flags: 0xFF,
        ..meta()
    };
    assert_eq!(out.encode().unwrap()[1], meta_flags::ALL);
}

#[test]
fn default_meta_holds_no_gyro_packet() {
    let meta = FrameWarpV2Meta::default();
    assert_eq!(meta.gyro_held_us, HELD_NONE);
    assert_eq!(meta.edge_fade_strength, DEFAULT_EDGE_FADE);
    assert_eq!(FrameWarpV2Meta::decode(&meta.encode().unwrap()), Some(meta));
}

#[test]
fn unit16_round_trips_and_clamps() {
    assert_eq!(to_unit16(0.0), 0);
    assert_eq!(to_unit16(1.0), 65535);
    assert_eq!(to_unit16(2.0), 65535);
    assert_eq!(to_unit16(-1.0), 0);
    assert_eq!(to_unit16(f32::NAN), 0);
    assert_eq!(to_unit16(0.5), 32768);
    for value in [0u16, 1, 1000, 32768, 65535] {
        assert_eq!(to_unit16(from_unit16(value)), value);
    }
}

#[test]
fn gyro_rate_maps_axes_to_yaw_and_pitch() {
    let deg = [10.0, 20.0, 30.0];
    let r = RADIANS_PER_DEGREE;
    let level = GyroAxes::default();
    assert_eq!(gyro_rate(deg, level, true), [20.0 * r, 10.0 * r]);
    let upright = GyroAxes {
        yaw: YawSource::Z,
        ..level
    };
    assert_eq!(gyro_rate(deg, upright, false), [30.0 * r, 10.0 * r]);
    let auto = GyroAxes {
        yaw: YawSource::Auto,
        invert_yaw: true,
        invert_pitch: true,
    };
    assert_eq!(gyro_rate(deg, auto, true), [-30.0 * r, -10.0 * r]);
    assert_eq!(gyro_rate(deg, auto, false), [-20.0 * r, -10.0 * r]);
    assert_eq!(
        gyro_rate([f32::NAN, f32::INFINITY, 0.0], level, false),
        [0.0, 0.0]
    );
}

#[test]
fn gyro_bias_learns_only_while_still_and_alike_on_both_ends() {
    let mut host = GyroBias::default();
    let mut client = GyroBias::default();
    let drift = [0.01, -0.02];
    for _ in 0..2000 {
        assert_eq!(host.observe(drift), client.observe(drift));
    }
    assert!(
        (host.bias()[0] - drift[0]).abs() < 1e-4,
        "{:?}",
        host.bias()
    );
    assert!(
        (host.bias()[1] - drift[1]).abs() < 1e-4,
        "{:?}",
        host.bias()
    );
    // A turn is not still: the bias holds through it.
    let before = host.bias();
    for _ in 0..100 {
        let out = host.observe([1.0, 0.0]);
        assert!((out[0] - (1.0 - before[0])).abs() < 1e-6);
    }
    assert_eq!(host.bias(), before);
    // Fewer still packets than the threshold teach nothing.
    let mut fresh = GyroBias::default();
    for _ in 0..BIAS_STILL_PACKETS - 1 {
        fresh.observe(drift);
    }
    assert_eq!(fresh.bias(), [0.0; 2]);
}
