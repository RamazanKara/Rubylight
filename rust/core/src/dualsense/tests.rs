use super::*;

#[test]
fn controller_audio_separates_speaker_and_haptics_and_gates_microphone() {
    let mut pad = DualSense::new(0, 2);
    pad.capabilities(haptics::CAPABILITY);
    pad.audio(controller_audio::CONTROL, &[1, 1, 2, 0, 7, 0, 0, 0]);
    assert_eq!(pad.input_report()[54], 9);
    let data = [1, 2, 3, 4, 5, 6, 7, 8].repeat(48);
    // Drain as of before the frames were made: a loaded test machine must
    // not age them past the queues' 20 ms limit.
    let start = Instant::now();
    for _ in 0..5 {
        pad.iso(1, false, &data, data.len()).unwrap();
    }
    let speaker: Vec<_> = pad.speaker.drain(start).collect();
    let pcm: Vec<_> = pad.pcm.drain(start).collect();
    assert_eq!(speaker[0].samples, [1, 2, 3, 4].repeat(240));
    assert_eq!(pcm[0].samples, [5, 6, 7, 8].repeat(240));
    let mic = haptics::Frame {
        id: 2,
        sequence: 1,
        created: Instant::now(),
        samples: vec![9; 192],
    }
    .payload()
    .unwrap();
    pad.audio(controller_audio::MICROPHONE, &mic);
    assert_eq!(pad.iso(2, true, &[], 196), Some(vec![9; 192]));
    pad.audio(controller_audio::CONTROL, &[1, 1, 2, 0, 15, 0, 0, 0]);
    pad.audio(controller_audio::MICROPHONE, &mic);
    assert_eq!(pad.iso(2, true, &[], 196), Some(vec![0; 192]));
    pad.suspend_audio();
    pad.audio(controller_audio::MICROPHONE, &mic);
    assert_eq!(pad.iso(2, true, &[], 196), Some(vec![0; 192]));
}

#[test]
fn reconnect_replays_persistent_effects_and_audio_controls() {
    let mut pad = DualSense::new(0, 0);
    let mut report = [0; 48];
    report[0] = 2;
    report[1] = 0x28;
    report[6] = 61;
    report[22] = 1;
    pad.output(3, &report);
    pad.take_feedback();
    pad.suspend_audio();
    pad.capabilities(haptics::CAPABILITY);
    pad.audio(controller_audio::CONTROL, &[1, 1, 0, 0, 1, 0, 0, 0]);
    let feedback = pad.take_feedback();
    assert!(
        feedback
            .iter()
            .any(|(kind, p)| *kind == 0x5503 && p[..2] == [8, 1])
    );
    assert!(
        feedback
            .iter()
            .any(|(kind, p)| *kind == controller_audio::CONTROL && p[7] == 61)
    );
}

fn request(ty: u8, request: u8, value: u16, index: u16, length: u16) -> Setup {
    Setup {
        request_type: ty,
        request,
        value,
        index,
        length,
    }
}
#[test]
fn composite_enumeration_exposes_native_usb_features_and_fixed_audio() {
    let mut pad = DualSense::new(3, 1);
    assert_eq!(pad.describe().speed, 3);
    let config = pad.control(&request(0x80, 6, 0x0200, 0, 255), &[]).unwrap();
    assert_eq!(
        config.len(),
        u16::from_le_bytes([config[2], config[3]]) as usize
    );
    assert_eq!(config[4], 4);
    let mut at = 0;
    let mut endpoints = vec![];
    while at < config.len() {
        if config[at + 1] == 5 {
            endpoints.push((config[at + 2], config[at + 6]));
        }
        at += usize::from(config[at]);
    }
    assert_eq!(endpoints, [(1, 4), (0x82, 4), (0x84, 6), (3, 6)]);
    assert_eq!(
        pad.control(&request(0xa1, 1, 0x0385, 3, 3), &[]),
        Some(vec![0x85, 0, 0])
    );
    let firmware = pad.control(&request(0xa1, 1, 0x0320, 3, 64), &[]).unwrap();
    assert_eq!(&firmware[28..32], &[0x3e, 0, 0, 1]);
    assert!(u16::from_le_bytes([firmware[44], firmware[45]]) >= 0x390);
    let pairing = pad.control(&request(0xa1, 1, 0x0309, 3, 20), &[]).unwrap();
    assert_eq!(&pairing[7..10], &[8, 0x25, 0]);
    assert_ne!(pairing, DualSense::new(4, 1).feature(9).unwrap());
    assert_eq!(
        pad.control(&request(0xa2, 0x81, 0x100, 1, 3), &[]),
        Some(vec![0x80, 0xbb, 0])
    );
    assert!(
        pad.control(&request(0x22, 1, 0x100, 1, 3), &[0x44, 0xac, 0])
            .is_none()
    );
    assert!(
        pad.control(&request(0x21, 1, 0x200, 0x200, 2), &[0, 0])
            .is_some()
    );
    assert_eq!(
        pad.control(&request(0xa1, 0x81, 0x200, 0x200, 2), &[]),
        Some(vec![0, 0])
    );
}

#[test]
fn usb_audio_preserves_actuator_samples_only_and_obeys_capability_and_altsetting() {
    let mut pad = DualSense::new(0, 2);
    let data: Vec<_> = (0..48)
        .flat_map(|n| [0x11, 0x22, 0x33, 0x44, n, 0x80, 255 - n, 0x7f])
        .collect();
    for _ in 0..5 {
        assert!(pad.iso(1, false, &data, data.len()).is_some());
    }
    assert!(pad.take_pcm().is_empty());
    pad.capabilities(haptics::CAPABILITY);
    for _ in 0..5 {
        pad.iso(1, false, &data, data.len()).unwrap();
    }
    let frames = pad.take_pcm();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].id, 2);
    let expected: Vec<_> = (0..5)
        .flat_map(|_| (0..48).flat_map(|n| [n, 0x80, 255 - n, 0x7f]))
        .collect();
    assert_eq!(frames[0].samples, expected);
    assert_eq!(pad.iso(2, true, &[], 196), Some(vec![0; 192]));
    assert!(pad.iso(1, false, &[0; 385], 385).is_none());
    pad.control(&request(1, 11, 0, 1, 0), &[]).unwrap();
    assert!(pad.iso(1, false, &data, data.len()).is_none());
    pad.control(&request(1, 11, 1, 1, 0), &[]).unwrap();
    assert!(pad.iso(1, false, &data, data.len()).is_some());
    pad.control(&request(0x23, 3, 4, 1, 0), &[]).unwrap();
    assert!(pad.take_pcm().is_empty());
}

#[test]
fn hid_input_presses_guide_touchpad_click_and_mute_from_the_extended_flags() {
    let mut pad = DualSense::new(0, 0);
    for (buttons, byte) in [
        (0x400, 1),
        (0x10_0000, 2),
        (0x20_0000, 4),
        (0x30_0400, 7),
        (0, 0),
    ] {
        pad.apply(&Input::Controller {
            id: 0,
            active: 1,
            buttons,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        });
        assert_eq!(pad.input_report()[10], byte, "buttons {buttons:#x}");
    }
}

#[test]
fn hid_input_keeps_center_motion_and_both_touch_contacts() {
    let mut pad = DualSense::new(0, 0);
    pad.apply(&Input::Controller {
        id: 0,
        active: 1,
        buttons: 0x1009,
        left_trigger: 1,
        right_trigger: 0,
        sticks: [0, 0, i16::MIN, i16::MAX],
    });
    let report = pad.input_report();
    assert_eq!(&report[1..7], &[128, 128, 0, 0, 1, 0]);
    assert_eq!(report[8], 0x21);
    assert_eq!(report[9], 4);
    pad.apply(&Input::Motion {
        id: 0,
        kind: 2,
        xyz: [1., -2., 3.],
    });
    assert_eq!(&pad.input_report()[16..22], &[16, 0, 224, 255, 48, 0]);
    for pointer in [9, 10] {
        pad.apply(&Input::ControllerTouch {
            id: 0,
            touchpad: 0,
            event: 1,
            pointer,
            x: 1.,
            y: 1.,
            pressure: 1.,
        });
    }
    assert_eq!(&pad.input_report()[34..37], &[0x7f, 0x77, 0x43]);
    assert_eq!(pad.input_report()[37] & 0x80, 0);
    pad.apply(&Input::ControllerTouch {
        id: 0,
        touchpad: 1,
        event: 7,
        pointer: 9,
        x: 0.,
        y: 0.,
        pressure: 0.,
    });
    assert_eq!(
        pad.input_report()[33] & 0x80,
        0,
        "unannounced second touchpad must not cancel the first"
    );
    pad.apply(&Input::Battery {
        id: 0,
        state: 5,
        percent: 100,
    });
    assert_eq!(pad.input_report()[53], 0x2a);
}

#[test]
fn partial_hid_output_retains_other_effects_and_explicit_off_is_forwarded() {
    let mut pad = DualSense::new(0, 0);
    let mut report = [0; 48];
    report[0] = 2;
    report[1] = 12;
    report[11] = 2;
    report[12] = 17;
    report[22] = 1;
    report[23] = 42;
    assert!(pad.output(3, &report));
    let first = pad.take_feedback();
    assert_eq!(first.len(), 1);
    assert_eq!(&first[0].1[..4], &[12, 1, 2, 42]);
    report.fill(0);
    report[0] = 2;
    report[1] = 4;
    pad.output(3, &report);
    let off = pad.take_feedback();
    assert_eq!(&off[0].1[..4], &[4, 1, 0, 42]);
    report[1] = 0;
    report[2] = 4;
    report[45..].copy_from_slice(&[10, 20, 30]);
    pad.output(3, &report);
    assert_eq!(pad.take_feedback(), [(0x5502, vec![10, 20, 30])]);
    report[2] = 0;
    report[39] = 4;
    report[3] = 7;
    report[4] = 9;
    pad.output(3, &report);
    assert_eq!(pad.take_feedback(), [(0x010b, vec![0, 9, 0, 7])]);
    pad.output(3, &report);
    assert!(pad.take_feedback().is_empty());
}
