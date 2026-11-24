//! Unit tests for Frame Warp wire formats, stick response curve and HUD masks.

use super::*;
use alloc::vec;

#[test]
fn request_round_trips_and_rejects_bad_fields() {
    let request = FrameWarpRequest {
        enabled: true,
        controller: Some(3),
        mask_max_width: 960,
        lens: false,
    };
    assert_eq!(request.encode(), [1, 1, 3, 0, 0xC0, 0x03, 0, 0]);
    assert_eq!(FrameWarpRequest::decode(&request.encode()), Some(request));
    // A client that draws the lens says so in the flags; their other bits are ignored.
    let lens = FrameWarpRequest {
        lens: true,
        ..request
    };
    assert_eq!(lens.encode(), [1, 1, 3, 1, 0xC0, 0x03, 0, 0]);
    assert_eq!(FrameWarpRequest::decode(&lens.encode()), Some(lens));
    assert_eq!(
        FrameWarpRequest::decode(&[1, 1, 3, 0xFE, 0xC0, 0x03, 0, 0]),
        Some(request)
    );
    let none = FrameWarpRequest {
        enabled: false,
        controller: None,
        mask_max_width: 0,
        lens: false,
    };
    assert_eq!(FrameWarpRequest::decode(&none.encode()), Some(none));
    assert_eq!(FrameWarpRequest::decode(&[0, 1, 0, 0, 0, 0, 0, 0]), None);
    assert_eq!(FrameWarpRequest::decode(&[1, 2, 0, 0, 0, 0, 0, 0]), None);
    assert_eq!(FrameWarpRequest::decode(&[1, 1, 16, 0, 0, 0, 0, 0]), None);
    assert_eq!(FrameWarpRequest::decode(&[1, 1, 0, 0, 0x01, 0x08, 0, 0]), None);
    assert_eq!(FrameWarpRequest::decode(&[1, 1, 0, 0, 0, 0, 0]), None);
    // A later version may append.
    assert!(FrameWarpRequest::decode(&[2, 1, 0, 0, 0, 0, 0, 0, 9]).is_some());
}

#[test]
fn tags_round_trip_oldest_first() {
    let tags = Tags {
        model_generation: 7,
        tags: vec![
            Tag {
                frame: 10,
                mouse_x: -5,
                mouse_y: i32::MAX,
                controller_seq: 3,
                controller_held_us: 1234,
            },
            Tag {
                frame: 11,
                mouse_x: 1,
                mouse_y: i32::MIN,
                controller_seq: 4,
                controller_held_us: HELD_NONE,
            },
        ],
    };
    let wire = tags.encode().unwrap();
    assert_eq!(wire.len(), TAGS_HEADER_BYTES + 2 * TAG_BYTES);
    assert_eq!(Tags::decode(&wire), Some(tags.clone()));
    let mut longer = wire.clone();
    longer.push(0);
    assert_eq!(Tags::decode(&longer), Some(tags));
    assert_eq!(Tags::decode(&wire[..wire.len() - 1]), None);
    assert!(Tags { model_generation: 0, tags: vec![] }.encode().is_none());
    assert!(Tags { model_generation: 0, tags: vec![Tag::default(); 5] }.encode().is_none());
    let mut bad = wire;
    bad[1] = 0;
    assert_eq!(Tags::decode(&bad), None);
}

fn model() -> Model {
    Model {
        generation: 3,
        mouse_locked: true,
        stick_locked: false,
        picture_width: 1920,
        picture_height: 1080,
        mouse_gain: [-0.75, 0.5],
        stick_gain: [-900.0, 400.0],
        stick_deadzone: 0.15,
        stick_exponent: 1.5,
        max_offset: [230.0, 130.0],
        lag_us: 32_000,
        confidence: 0.93,
        focal: 960.0,
    }
}

#[test]
fn model_round_trips_and_rejects_out_of_bounds() {
    let wire = model().encode().unwrap();
    assert_eq!(wire[1], 1);
    assert_eq!(wire.len(), MODEL_BYTES);
    assert_eq!(Model::decode(&wire), Some(model()));
    assert_eq!(model().lens(), Some(960.0));
    let mut extended = wire.to_vec();
    extended.extend_from_slice(&[1, 2, 3]);
    assert_eq!(Model::decode(&extended), Some(model()));
    // A model as first defined, and one cut inside the focal length, have no lens.
    let flat = Model { focal: 0.0, ..model() };
    assert_eq!(Model::decode(&wire[..MODEL_FLAT_BYTES]), Some(flat));
    assert_eq!(Model::decode(&wire[..MODEL_BYTES - 1]), Some(flat));
    assert_eq!(flat.lens(), None);
    assert_eq!(Model::decode(&wire[..MODEL_FLAT_BYTES - 1]), None);
    for broken in [
        Model { mouse_gain: [f32::NAN, 0.0], ..model() },
        Model { stick_gain: [0.0, f32::INFINITY], ..model() },
        Model { stick_deadzone: 0.6, ..model() },
        Model { stick_exponent: 1.2, ..model() },
        Model { max_offset: [-1.0, 0.0], ..model() },
        Model { confidence: 1.5, ..model() },
        Model { picture_width: 0, ..model() },
        Model { focal: -1.0, ..model() },
        Model { focal: 0.5, ..model() },
        Model { focal: f32::NAN, ..model() },
        Model { focal: 2_000_000.0, ..model() },
    ] {
        assert!(broken.encode().is_none(), "{broken:?}");
    }
    let mut flags = wire;
    flags[1] = 4;
    assert_eq!(Model::decode(&flags), None);
}

#[test]
fn stick_curve_has_a_radial_dead_zone_and_exponent() {
    assert_eq!(stick_curve(0, 0, 0.1, 1.0), [0.0, 0.0]);
    assert_eq!(stick_curve(3000, 0, 0.1, 1.0), [0.0, 0.0]);
    let full = stick_curve(32767, 0, 0.1, 2.0);
    assert!((full[0] - 1.0).abs() < 1e-5 && full[1] == 0.0);
    let left = stick_curve(-32768, 0, 0.0, 1.0);
    assert!((left[0] + 1.0).abs() < 1e-5);
    // Half way out with no dead zone and exponent 1.5: 0.5^1.5.
    let half = stick_curve(16384, 0, 0.0, 1.5);
    assert!((half[0] - 0.353_6).abs() < 1e-3, "{half:?}");
    // Diagonal keeps its direction.
    let diagonal = stick_curve(20000, 20000, 0.0, 1.0);
    assert!((diagonal[0] - diagonal[1]).abs() < 1e-6);
    assert!(diagonal[0] > 0.6 && diagonal[0] <= 0.7072);
    // Corners saturate at length 1.
    let corner = stick_curve(32767, 32767, 0.0, 1.0);
    let length = sqrt(corner[0] * corner[0] + corner[1] * corner[1]);
    assert!((length - 1.0).abs() < 1e-5);
}

#[test]
fn sqrt_is_exact_enough() {
    for value in [1e-6f32, 0.01, 0.25, 0.5, 1.0, 2.0, 1234.5] {
        let root = sqrt(value);
        assert!((root * root - value).abs() <= value * 1e-6, "{value}");
    }
    assert_eq!(sqrt(0.0), 0.0);
    assert_eq!(sqrt(-1.0), 0.0);
}

#[test]
fn mask_round_trips_through_runs() {
    let mut mask = Mask::new(9, 70, 3);
    for x in 0..5 {
        mask.set(x, 0, true);
    }
    mask.set(69, 1, true);
    for x in 30..40 {
        mask.set(x, 2, true);
    }
    let wire = mask.encode().unwrap();
    // Row 0 starts set: a zero clear run, then 5 set, then 65 clear.
    assert_eq!(&wire[MASK_HEADER_BYTES..MASK_HEADER_BYTES + 3], &[0, 5, 65]);
    assert_eq!(Mask::decode(&wire), Some(mask.clone()));
    assert_eq!(mask.count(), 16);
    let empty = Mask { generation: 4, ..Mask::default() };
    let wire = empty.encode().unwrap();
    assert_eq!(wire.len(), MASK_HEADER_BYTES);
    assert_eq!(Mask::decode(&wire), Some(empty));
}

#[test]
fn mask_decode_refuses_broken_runs() {
    let header = |kind: u8, w: u16, h: u16| {
        let mut out = vec![1, kind, 0, 0];
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out
    };
    // Runs past the row's end.
    let mut long = header(1, 4, 1);
    long.extend_from_slice(&[2, 3]);
    assert_eq!(Mask::decode(&long), None);
    // A zero run after the first.
    let mut zero = header(1, 4, 1);
    zero.extend_from_slice(&[2, 0, 2]);
    assert_eq!(Mask::decode(&zero), None);
    // A row that stops short.
    let mut short = header(1, 4, 1);
    short.push(2);
    assert_eq!(Mask::decode(&short), None);
    // Kinds that do not match the size.
    assert_eq!(Mask::decode(&header(0, 4, 1)), None);
    assert_eq!(Mask::decode(&header(1, 0, 0)), None);
    assert_eq!(Mask::decode(&header(1, 0, 4)), None);
    assert_eq!(Mask::decode(&header(1, MAX_MASK_EDGE + 1, 1)), None);
    // A run over the largest edge.
    let mut huge = header(1, 4, 1);
    huge.extend_from_slice(&[0xFF, 0xFF, 0x7F]);
    assert_eq!(Mask::decode(&huge), None);
}

#[test]
fn halved_mask_keeps_every_set_cell() {
    let mut mask = Mask::new(1, 5, 3);
    mask.set(4, 2, true);
    mask.set(1, 0, true);
    let half = mask.halved();
    assert_eq!((half.width, half.height), (3, 2));
    assert!(half.get(2, 1) && half.get(0, 0));
    assert_eq!(half.count(), 2);
}

#[test]
fn a_full_size_checkerboard_does_not_fit_and_its_halves_do() {
    let mut mask = Mask::new(1, 1920, 1080);
    for y in 0..1080 {
        for x in (y % 2..1920).step_by(2) {
            mask.set(x, y, true);
        }
    }
    assert!(mask.encode().is_none());
    let mut smaller = mask.halved();
    while smaller.encode().is_none() {
        smaller = smaller.halved();
    }
    assert!(smaller.width <= 960);
}
