//! Unit tests for Frame Warp 2 on the client: the envelope, the fill references, the gyro's
//! part of the shift and how the predictor folds them in.

use super::*;
use crate::frame_warp::{FADE, Predictor, Tag, Tags};
use rubylight_protocol::frame_warp::{FrameWarpRequest, Model};
use rubylight_protocol::frame_warp_v2::{DEFAULT_EDGE_FADE, RADIANS_PER_DEGREE, to_unit16};

const MS: u64 = 1000;

fn request(gyro: bool) -> RequestV2 {
    RequestV2 {
        gyro,
        axes: GyroAxes {
            yaw: YawSource::Auto,
            ..GyroAxes::default()
        },
        ..RequestV2::new(FrameWarpRequest {
            enabled: true,
            controller: Some(0),
            mask_max_width: 256,
            lens: true,
        })
    }
}

fn meta(frame: u32, confidence: f32) -> FrameWarpV2Meta {
    FrameWarpV2Meta {
        flags: 0,
        frame,
        camera_confidence: to_unit16(confidence),
        ..FrameWarpV2Meta::default()
    }
}

#[test]
fn the_envelope_falls_within_100_ms_and_rises_along_an_s_curve() {
    let mut e = Envelope::default();
    assert_eq!(e.update(1.0, 0), 1.0);
    e.update(0.0, 0);
    let at_40 = e.update(0.0, 40 * MS);
    assert!((at_40 - (-1.0f32).exp()).abs() < 1e-3, "{at_40}");
    let at_100 = e.update(0.0, 100 * MS);
    assert!(
        at_100 < 0.1,
        "a pause menu stops the warp within 100 ms: {at_100}"
    );
    let mut low = Envelope::default();
    low.update(0.0, 0);
    low.update(0.0, 10_000 * MS);
    assert_eq!(low.value(), 0.0);
    // The rise starts when the target goes up and follows smoothstep over 180 ms.
    let start = 10_000 * MS;
    assert_eq!(low.update(1.0, start), 0.0);
    let mid = low.update(1.0, start + 90 * MS);
    assert!((mid - 0.5).abs() < 1e-3, "{mid}");
    let mut last = mid;
    for t in (100..=180).step_by(10) {
        let v = low.update(1.0, start + t * MS);
        assert!(v >= last, "monotonic: {v} after {last}");
        last = v;
    }
    assert_eq!(last, 1.0);
    // A drop during the rise falls from where the rise had got to.
    let mut e = Envelope::default();
    e.update(0.0, 0);
    e.update(0.0, 1_000 * MS);
    e.update(1.0, 1_000 * MS);
    let part = e.update(1.0, 1_090 * MS);
    let fell = e.update(0.2, 1_130 * MS);
    assert!(fell < part && fell > 0.2, "{fell} {part}");
    // A target that is not a number counts as none.
    let mut e = Envelope::default();
    e.update(f32::NAN, 0);
    assert_eq!(e.update(f32::NAN, 10_000 * MS), 0.0);
}

#[test]
fn the_edge_fill_squeezes_instead_of_mirroring() {
    assert_eq!(edge_v2([0.3, 0.7], 0.35), ([0.3, 0.7], 1.0));
    let (at, fade) = edge_v2([-0.04, 0.5], 0.35);
    assert!((at[0] - 0.01).abs() < 1e-6, "{at:?}");
    // Halfway along the ramp the smoothstep is a half.
    assert!((fade - (1.0 - 0.35 * 0.5)).abs() < 1e-6, "{fade}");
    let (at, fade) = edge_v2([1.2, 1.5], 0.35);
    assert!(
        (at[0] - 0.95).abs() < 1e-6 && (at[1] - 0.875).abs() < 1e-6,
        "{at:?}"
    );
    assert!((fade - 0.65).abs() < 1e-6, "{fade}");
    // Far past the edge the sample stays inside.
    assert_eq!(edge_v2([-10.0, 11.0], 0.0), ([1.0, 0.0], 1.0));
}

#[test]
fn the_hud_probe_finds_the_background_behind_the_shift() {
    // A 10 by 10 mask with column 5 set.
    let hud = |p: [f32; 2]| (p[0] * 10.0).floor() as i32 == 5;
    // The source landed on the HUD; the shift moved content right, so the probe steps left.
    let source = [0.55, 0.5];
    let shift = [0.03, 0.0];
    let found = hud_probe(source, shift, [10.0, 10.0], 2, 0.35, hud).unwrap();
    assert!((found.0[0] - 0.45).abs() < 1e-6, "{found:?}");
    assert_eq!(found.1, 1.0);
    assert_eq!(
        hud_probe(source, shift, [10.0, 10.0], 0, 0.35, hud),
        None,
        "no guard"
    );
    assert_eq!(
        hud_probe(source, [0.0; 2], [10.0, 10.0], 2, 0.35, hud),
        None,
        "no shift"
    );
    // A HUD wider than the guard keeps the point's own pixel.
    let wide = |p: [f32; 2]| p[0] > 0.2;
    assert_eq!(
        hud_probe([0.9, 0.5], shift, [10.0, 10.0], 2, 0.35, wide),
        None
    );
    // The guard never reaches past MAX_HUD_GUARD cells: the scene nine cells back stays out.
    assert_eq!(
        hud_probe([1.05, 0.5], shift, [10.0, 10.0], 100, 0.0, wide),
        None,
        "nothing within {MAX_HUD_GUARD} cells"
    );
    let near = hud_probe([0.95, 0.5], shift, [10.0, 10.0], 100, 0.0, wide);
    assert!(near.is_some_and(|(at, _)| at[0] < 0.2), "eight cells back: {near:?}");
}

#[test]
fn only_gyro_readings_of_the_gyro_pad_count() {
    let mut v2 = V2::new(request(true));
    assert!(
        !v2.note_motion(0, 1, [1.0; 3], 0),
        "an accelerometer reading"
    );
    assert!(!v2.note_motion(1, GYRO, [1.0; 3], 0), "another pad");
    assert!(v2.note_motion(0, GYRO, [1.0; 3], 0));
    let mut off = V2::new(request(false));
    assert!(!off.note_motion(0, GYRO, [1.0; 3], 0), "no gyro asked for");
}

/// A gyro turning at `deg_s` about its y axis, one packet every 4 ms from 0 to `until`.
fn turning(v2: &mut V2, deg_s: f32, until: u64) {
    for at in (0..=until).step_by(4 * MS as usize) {
        v2.note_motion(0, GYRO, [0.0, deg_s, 0.0], at);
    }
}

#[test]
fn the_gyro_turn_the_frame_lacks_shifts_it() {
    let mut v2 = V2::new(request(true));
    turning(&mut v2, 90.0, 1_000 * MS);
    let gain = [800.0, 400.0];
    // The frame reflects gyro packet 200 (sent at 796 ms), held 4 ms: 200 ms of turn remain.
    let locked = FrameWarpV2Meta {
        flags: meta_flags::GYRO_LOCKED,
        frame: 7,
        gyro_seq: 200,
        gyro_held_us: 4_000,
        gyro_gain: gain,
        ..meta(7, 1.0)
    };
    v2.on_meta(FrameWarpV2Meta { flags: 0, ..locked }, 1_000 * MS);
    assert_eq!(v2.gyro_shift(7, 1_000 * MS), [0.0; 2], "not locked");
    v2.on_meta(FrameWarpV2Meta { frame: 8, ..locked }, 1_000 * MS);
    let shift = v2.gyro_shift(8, 1_000 * MS);
    let expected = gain[0] * 90.0 * RADIANS_PER_DEGREE * 0.2;
    assert!(
        (shift[0] - expected).abs() < expected * 0.01,
        "{shift:?} vs {expected}"
    );
    assert!(shift[1].abs() < 1e-3);
    // The host found the yaw on z: a y turn moves nothing then.
    v2.on_meta(
        FrameWarpV2Meta {
            frame: 9,
            flags: locked.flags | meta_flags::GYRO_YAW_Z,
            ..locked
        },
        1_000 * MS,
    );
    assert!(v2.gyro_shift(9, 1_000 * MS)[0].abs() < 1e-3);
    // A host count ahead of what was sent shifts nothing until the next rebase.
    v2.on_meta(
        FrameWarpV2Meta {
            frame: 10,
            gyro_seq: 1_000,
            ..locked
        },
        1_000 * MS,
    );
    assert_eq!(v2.gyro_shift(10, 1_000 * MS), [0.0; 2]);
}

#[test]
fn packets_in_flight_while_the_pad_rests_keep_the_counts_together() {
    let mut v2 = V2::new(request(true));
    // A resting pad still sends a packet every 4 ms (its noise).
    for at in (0..=1_000 * MS).step_by(4 * MS as usize) {
        v2.note_motion(0, GYRO, [0.0; 3], at);
    }
    let sent = v2.status().gyro_packets;
    let gain = [800.0, 400.0];
    // The frames reflect packets 100 ms old: 25 are in flight, none lost.
    let locked = FrameWarpV2Meta {
        flags: meta_flags::GYRO_LOCKED,
        gyro_seq: sent - 25,
        gyro_held_us: 0,
        gyro_gain: gain,
        ..meta(1, 1.0)
    };
    v2.on_meta(locked, 1_000 * MS);
    // Then the player turns at 90 degrees a second.
    for at in (1_004 * MS..=1_100 * MS).step_by(4 * MS as usize) {
        v2.note_motion(0, GYRO, [0.0, 90.0, 0.0], at);
    }
    // The frame drawn now reflects the last resting packet: the whole turn is missing from it.
    v2.on_meta(
        FrameWarpV2Meta {
            frame: 2,
            gyro_seq: sent,
            ..locked
        },
        1_100 * MS,
    );
    let shift = v2.gyro_shift(2, 1_100 * MS);
    let expected = gain[0] * 90.0 * RADIANS_PER_DEGREE * 0.096;
    assert!(
        (shift[0] - expected).abs() < expected * 0.01,
        "{shift:?} vs {expected}"
    );
    // Once nothing has gone out for a while, the next meta sets the counts equal.
    v2.on_meta(
        FrameWarpV2Meta {
            frame: 3,
            gyro_seq: sent + 20,
            ..locked
        },
        1_800 * MS,
    );
    assert_eq!(v2.seq_offset, 5, "five packets lost on the way");
}

#[test]
fn packets_lost_while_the_gyro_flows_are_counted_from_the_hosts_receipts() {
    let mut v2 = V2::new(request(true));
    let mut lost = 0;
    let mut frame = 0;
    // A packet every 4 ms; a meta every 16 ms, with what the host had received 20 ms earlier
    // (five packets in flight). Five packets go missing at 1.5 s.
    for at in (0..=3_000 * MS).step_by(4 * MS as usize) {
        v2.note_motion(0, GYRO, [0.0; 3], at);
        if at == 1_500 * MS {
            lost = 5;
        }
        if at % (16 * MS) == 0 && at >= 20 * MS {
            let received = ((at - 20 * MS) / (4 * MS)) as u32 + 1 - lost;
            frame += 1;
            v2.on_meta(
                FrameWarpV2Meta {
                    gyro_seq: received.saturating_sub(10),
                    gyro_received: received as u16,
                    ..meta(frame, 1.0)
                },
                at,
            );
            if at == 1_392 * MS {
                assert_eq!(v2.seq_offset, 0, "in flight is not lost");
            }
        }
    }
    assert_eq!(v2.seq_offset, 5);
}

#[test]
fn the_row_carries_the_fill_hints() {
    let mut v2 = V2::new(request(false));
    assert_eq!(v2.row(3), DEFAULT_ROW, "the defaults before a meta");
    assert!((DEFAULT_ROW[0] - f32::from(DEFAULT_EDGE_FADE) / 255.0).abs() < 1e-6);
    assert_eq!(DEFAULT_ROW[1], 2.0);
    v2.on_meta(
        FrameWarpV2Meta {
            flags: meta_flags::HUD_SOFT_EDGE,
            edge_fade_strength: 51,
            hud_guard: 4,
            ..meta(3, 1.0)
        },
        0,
    );
    let row = v2.row(3);
    assert!((row[0] - 0.2).abs() < 1e-6, "{row:?}");
    assert_eq!(row[1], 4.0);
    assert_eq!(row[2], 1.0);
    assert_eq!(row[3], f32::from(meta_flags::HUD_SOFT_EDGE));
    // A host that fills nothing and probes nothing.
    v2.on_meta(
        FrameWarpV2Meta {
            edge_fade_strength: 0,
            hud_guard: 0,
            ..meta(4, 1.0)
        },
        0,
    );
    assert_eq!(&v2.row(4)[..2], &[0.0, 0.0]);
}

fn mouse_model() -> Model {
    Model {
        generation: 1,
        mouse_locked: true,
        picture_width: 1000,
        picture_height: 500,
        mouse_gain: [-0.5, -0.25],
        max_offset: [200.0, 100.0],
        lag_us: 30_000,
        confidence: 0.9,
        ..Model::default()
    }
}

const SETTLED: u64 = 10_000_000;

#[test]
fn a_falling_confidence_stops_the_shift_within_100_ms() {
    let mut p = Predictor::new(None);
    p.set_request(request(false));
    p.on_model(mouse_model());
    p.warp(0, SETTLED - FADE.as_micros() as u64 - 1);
    p.note_mouse(100, 0, SETTLED);
    let tag = Tag {
        frame: 5,
        mouse_x: 60,
        ..Tag::default()
    };
    p.on_tags(
        &Tags {
            model_generation: 1,
            tags: vec![tag],
        },
        SETTLED,
    );
    p.on_meta(meta(5, 1.0), SETTLED);
    let full = p.warp(5, SETTLED + MS);
    assert_eq!(full.pixels, [-20.0, 0.0]);
    assert!(full.v2[0] > 0.0, "the fill: {:?}", full.v2);
    // A pause menu: the host's confidence drops to 0.
    p.on_meta(meta(6, 0.0), SETTLED + 2 * MS);
    p.warp(5, SETTLED + 2 * MS);
    let after = p.warp(5, SETTLED + 102 * MS);
    let length = after.pixels[0].hypot(after.pixels[1]);
    assert!(
        length < 2.0,
        "under 10% of the shift after 100 ms: {length}"
    );
    // And it comes back smoothly once the camera does.
    p.on_meta(meta(7, 1.0), SETTLED + 500 * MS);
    p.warp(5, SETTLED + 500 * MS);
    let back = p.warp(5, SETTLED + 700 * MS);
    assert_eq!(back.pixels, [-20.0, 0.0]);
    assert_eq!(p.status(SETTLED + 700 * MS).v2.confidence, 1.0);
}

#[test]
fn the_gyro_alone_locks_a_stream() {
    let mut p = Predictor::new(Some(0));
    p.set_request(request(true));
    p.on_model(Model {
        mouse_locked: false,
        ..mouse_model()
    });
    for at in (0..=SETTLED).step_by(4 * MS as usize) {
        p.note_motion(0, GYRO, [0.0, 45.0, 0.0], at);
    }
    let packets = (SETTLED / (4 * MS)) as u32 + 1;
    p.on_tags(
        &Tags {
            model_generation: 1,
            tags: vec![Tag {
                frame: 2,
                ..Tag::default()
            }],
        },
        SETTLED,
    );
    let locked = FrameWarpV2Meta {
        flags: meta_flags::GYRO_LOCKED,
        gyro_seq: packets - 10,
        gyro_held_us: 0,
        gyro_gain: [-300.0, 0.0],
        ..meta(2, 1.0)
    };
    p.on_meta(locked, SETTLED);
    p.warp(2, SETTLED);
    let warp = p.warp(2, SETTLED + FADE.as_micros() as u64);
    assert!(warp.active, "{warp:?}");
    assert!(warp.pixels[0] < -5.0, "{warp:?}");
}

#[test]
fn before_any_meta_the_shift_is_whole_with_the_default_row() {
    let mut p = Predictor::new(None);
    p.on_model(mouse_model());
    p.warp(0, SETTLED - FADE.as_micros() as u64 - 1);
    p.note_mouse(100, 0, SETTLED);
    p.on_tags(
        &Tags {
            model_generation: 1,
            tags: vec![Tag {
                frame: 5,
                mouse_x: 60,
                ..Tag::default()
            }],
        },
        SETTLED,
    );
    let warp = p.warp(5, SETTLED + MS);
    assert_eq!(
        warp.pixels,
        [-20.0, 0.0],
        "full confidence until the host says otherwise"
    );
    assert_eq!(warp.v2, DEFAULT_ROW);
    assert_eq!(p.status(SETTLED).v2.meta, None);
}
