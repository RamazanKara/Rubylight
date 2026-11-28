//! Unit tests for Steam Deck hidraw IMU report decoding, sysfs matching, rate gating, and lifecycle.

use super::*;
use crate::gamepad_types::CTYPE_XBOX;
use nightjar_core::control::HostCapabilities;
use nightjar_core::input::InputQueue;

fn deck_packet(accel: [i16; 3], gyro: [i16; 3]) -> [u8; 64] {
    let mut report = [0u8; 64];
    report[..4].copy_from_slice(&[0x01, 0x00, 0x09, 64]);
    for (at, value) in [
        (24, accel[0]),
        (26, accel[1]),
        (28, accel[2]),
        (30, gyro[0]),
        (32, gyro[1]),
        (34, gyro[2]),
    ] {
        report[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    report
}

#[test]
fn parses_64_and_65_byte_reports_with_exact_axis_transform_and_roundtrip() {
    let raw = deck_packet([1000, -2000, 3000], [-4000, 5000, -6000]);
    let sample = parse_deck_state_report(&raw).expect("valid 64-byte packet");
    assert_eq!(
        sample.accel_ms2,
        [1000.0 * ACCEL_SCALE, 3000.0 * ACCEL_SCALE, 2000.0 * ACCEL_SCALE]
    );
    assert_eq!(
        sample.gyro_dps,
        [-4000.0 * GYRO_SCALE, -6000.0 * GYRO_SCALE, -5000.0 * GYRO_SCALE]
    );

    let mut prefixed = [0u8; 65];
    prefixed[1..].copy_from_slice(&raw);
    assert_eq!(parse_deck_state_report(&prefixed), Some(sample));

    // Negating i16::MIN (-32768) must not overflow.
    let min_pkt = deck_packet([0, i16::MIN, 0], [0, i16::MIN, 0]);
    let min_sample = parse_deck_state_report(&min_pkt).expect("min i16 safe");
    assert_eq!(min_sample.accel_ms2[2], 32768.0 * ACCEL_SCALE);
    assert_eq!(min_sample.gyro_dps[2], 32768.0 * GYRO_SCALE);

    // Round-trip against `rust/core/input-policy/src/steam_deck.rs` axis packing.
    let (sdl_accel, sdl_gyro) = ([1.25_f32, -9.81, 4.5], [120.0_f32, -310.0, 45.0]);
    let enc = |v: f32, scale: f32| (v * scale).round().clamp(-32768.0, 32767.0) as i16;
    let accel_encode_scale = 32768.0 / (2.0 * 9.80665);
    let gyro_encode_scale = 32768.0 / 2000.0;
    let roundtrip = deck_packet(
        [
            enc(sdl_accel[0], accel_encode_scale),
            enc(-sdl_accel[2], accel_encode_scale),
            enc(sdl_accel[1], accel_encode_scale),
        ],
        [
            enc(sdl_gyro[0], gyro_encode_scale),
            enc(-sdl_gyro[2], gyro_encode_scale),
            enc(sdl_gyro[1], gyro_encode_scale),
        ],
    );
    let decoded = parse_deck_state_report(&roundtrip).unwrap();
    for i in 0..3 {
        assert!((decoded.accel_ms2[i] - sdl_accel[i]).abs() <= ACCEL_SCALE);
        assert!((decoded.gyro_dps[i] - sdl_gyro[i]).abs() <= GYRO_SCALE);
    }
}

#[test]
fn rejects_short_and_malformed_reports() {
    let valid = deck_packet([1, 2, 3], [4, 5, 6]);
    assert!(parse_deck_state_report(&valid[..63]).is_none());
    for (idx, val) in [(0, 2), (1, 1), (2, 0x01), (3, 63)] {
        let mut bad = valid;
        bad[idx] = val;
        assert!(parse_deck_state_report(&bad).is_none());
    }
    let mut bad_prefix = [0u8; 65];
    bad_prefix[0] = 0x01;
    bad_prefix[1..].copy_from_slice(&valid);
    assert!(parse_deck_state_report(&bad_prefix).is_none());
    assert!(parse_deck_state_report(&[0u8; 66]).is_none());
}

#[test]
fn uevent_interface_and_candidate_matching() {
    assert!(uevent_matches_steam_deck("HID_ID=0003:000028DE:00001205\n"));
    assert!(uevent_matches_steam_deck("DRIVER=hid\nHID_ID=0003:000028de:00001205\n"));
    assert!(!uevent_matches_steam_deck("HID_ID=0003:000028DE:000011FF\n"));
    assert!(!uevent_matches_steam_deck("HID_ID=0003:000028DE:00001205:extra\n"));
    assert!(!uevent_matches_steam_deck("OTHER_HID_ID=0003:000028DE:00001205\n"));

    assert!(is_controller_interface("02\n"));
    assert!(is_controller_interface(" 2 "));
    assert!(!is_controller_interface("00"));
    assert!(!is_controller_interface("01"));
    assert!(!is_controller_interface(""));
    assert!(!is_controller_interface("0x02"));

    assert!(is_deck_imu_candidate((0x28de, 0x11ff), "Other", false));
    assert!(is_deck_imu_candidate((0x28de, 0x1205), "Other", false));
    assert!(is_deck_imu_candidate((0, 0), "steam virtual gamepad", false));
    assert!(!is_deck_imu_candidate((0x28de, 0x11ff), "Steam Virtual Gamepad", true));
    assert!(!is_deck_imu_candidate((0x054c, 0x0ce6), "DualSense", false));
}

#[test]
fn rate_gate_decimates_and_stops_at_zero_hz() {
    let mut gate = RateGate::default();
    assert!(gate.should_emit(0, 100));
    assert!(!gate.should_emit(9_999, 100));
    assert!(gate.should_emit(10_000, 100));
    assert!(!gate.should_emit(10_000, 0));
    // Re-enabling after 0 Hz emits immediately without needing a full previous interval.
    assert!(gate.should_emit(10_500, 200));
    assert!(!gate.should_emit(14_999, 200));
    assert!(gate.should_emit(15_500, 200));
    // Rates > 1000 Hz clamp to 1000 Hz (1000 us interval).
    assert!(!gate.should_emit(16_499, 2000));
    assert!(gate.should_emit(16_500, 2000));
}

#[test]
fn shared_prepare_motion_applies_mapping_gates_and_xbox_suppression() {
    let shared = DeckImuShared::default();
    shared.sending.store(true, Ordering::Release);
    shared.sent_as.store(2, Ordering::Release);
    shared.accel_rate_hz.store(100, Ordering::Release);
    shared.gyro_rate_hz.store(250, Ordering::Release);
    {
        let mut mapping = shared.mapping.lock().unwrap();
        mapping.gyro_axes = [-2, 3, 1];
        mapping.gyro_pitch = 50;
        mapping.gyro_yaw = 200;
    }

    let pkt = deck_packet([100, 200, 300], [1000, 2000, 3000]);
    let (mut ag, mut gg) = (RateGate::default(), RateGate::default());
    let first = shared.prepare_motion(&pkt, 0, &mut ag, &mut gg).unwrap();
    let sample = parse_deck_state_report(&pkt).unwrap();
    assert_eq!(first.0, 2);
    assert_eq!(first.1, Some(sample.accel_ms2));
    assert_eq!(
        first.2,
        Some([
            -sample.gyro_dps[1] * 0.5,
            sample.gyro_dps[2] * 2.0,
            sample.gyro_dps[0],
        ])
    );

    let mut queue = InputQueue::new(&HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: 0x03,
        ..Default::default()
    });
    send_motion(&mut queue, first);
    assert_eq!(queue.len(), 2);

    // At 4 ms, accel (100 Hz = 10 ms) is gated while gyro (250 Hz = 4 ms) emits.
    let second = shared.prepare_motion(&pkt, 4_000, &mut ag, &mut gg).unwrap();
    assert!(second.1.is_none() && second.2.is_some());

    // Xbox pin suppresses all motion.
    shared.mapping.lock().unwrap().connect_as = CTYPE_XBOX;
    assert!(shared.prepare_motion(&pkt, 20_000, &mut ag, &mut gg).is_none());
    shared.mapping.lock().unwrap().connect_as = 0;

    // Paused / menu / unfocused (`sending = false`) and `refused = true` suppress all motion.
    shared.sending.store(false, Ordering::Release);
    assert!(shared.prepare_motion(&pkt, 30_000, &mut ag, &mut gg).is_none());
    shared.sending.store(true, Ordering::Release);
    shared.refused.store(true, Ordering::Release);
    assert!(shared.prepare_motion(&pkt, 40_000, &mut ag, &mut gg).is_none());
}

#[test]
fn deck_imu_stop_and_drop_join_worker_cleanly() {
    let spawn_dummy = |imu: &mut DeckImu| {
        let stop = imu.shared.stop.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let done = exited.clone();
        imu.worker = Some(std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            done.store(true, Ordering::Release);
        }));
        exited
    };

    let mut imu = DeckImu::new(PathBuf::from("test-hidraw"));
    imu.shared.accel_rate_hz.store(100, Ordering::Release);
    let exited = spawn_dummy(&mut imu);
    imu.set_rate(motion_type::GYRO, 0, None);
    assert!(imu.worker.is_some(), "active accel keeps worker running");
    imu.set_rate(motion_type::ACCEL, 0, None);
    assert!(imu.worker.is_none());
    assert!(exited.load(Ordering::Acquire));

    let mut imu2 = DeckImu::new(PathBuf::from("test-hidraw"));
    let exited2 = spawn_dummy(&mut imu2);
    drop(imu2);
    assert!(exited2.load(Ordering::Acquire));

    #[cfg(target_os = "linux")]
    assert_eq!(std::mem::size_of::<HidrawDevinfo>(), 8);
}
