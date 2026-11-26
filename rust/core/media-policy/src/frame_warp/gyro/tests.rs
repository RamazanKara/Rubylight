//! The gyro's history: what counts, the angle on each yaw axis, the tags and the yaw choice.

use super::*;

const AXES: GyroAxes = GyroAxes {
    invert_yaw: false,
    invert_pitch: false,
    yaw: YawSource::Auto,
};

#[test]
fn only_the_gyro_pads_gyro_readings_count() {
    let mut gyro = GyroHistory::new(Some(1), AXES);
    assert!(!gyro.on_motion(0, GYRO_KIND, [10.0; 3], 1_000));
    assert!(!gyro.on_motion(1, 1, [10.0; 3], 1_000));
    assert!(gyro.on_motion(1, GYRO_KIND, [10.0; 3], 2_000));
    assert_eq!(gyro.seq(), 1);
    // No pad: nothing counts.
    let mut none = GyroHistory::new(None, AXES);
    assert!(!none.on_motion(0, GYRO_KIND, [1.0; 3], 1));
    assert_eq!(none.seq(), 0);
}

#[test]
fn the_angle_holds_each_reading_until_the_next_and_picks_the_yaw_axis() {
    let mut gyro = GyroHistory::new(Some(0), AXES);
    // 90 deg/s of y-yaw for 100 ms, then 180 deg/s of z-yaw for 100 ms; 30 deg/s of pitch.
    gyro.on_motion(0, GYRO_KIND, [30.0, 90.0, 0.0], 1_000_000);
    gyro.on_motion(0, GYRO_KIND, [30.0, 0.0, 180.0], 1_100_000);
    gyro.on_motion(0, GYRO_KIND, [0.0, 0.0, 0.0], 1_200_000);
    let quarter = std::f64::consts::FRAC_PI_2;
    let y = gyro.angle(1_000_000, 1_200_000, false);
    let z = gyro.angle(1_000_000, 1_200_000, true);
    assert!((y[0] - quarter * 0.1).abs() < 1e-5, "{y:?}");
    assert!((z[0] - quarter * 0.2).abs() < 1e-5, "{z:?}");
    let pitch = 30f64.to_radians() * 0.2;
    assert!((y[1] - pitch).abs() < 1e-5 && (z[1] - pitch).abs() < 1e-5);
    // A span inside a reading, and spans before the first or reversed.
    let part = gyro.angle(1_025_000, 1_050_000, false);
    assert!((part[0] - quarter * 0.025).abs() < 1e-5, "{part:?}");
    assert_eq!(gyro.angle(0, 900_000, false), [0.0; 2]);
    assert_eq!(gyro.angle(1_100_000, 1_000_000, false), [0.0; 2]);
    assert!(gyro.active(1_050_000, 1_060_000));
    assert!(!gyro.active(1_250_000, 1_300_000));
}

#[test]
fn the_bias_and_rates_match_what_the_client_keeps() {
    // Both ends run the protocol's bias over the same readings.
    let axes = GyroAxes {
        invert_yaw: true,
        ..AXES
    };
    let mut gyro = GyroHistory::new(Some(0), axes);
    let mut client = [GyroBias::default(); 2];
    let mut t = 0;
    for i in 0..200u32 {
        let xyz = [0.5, 1.0 + (i % 3) as f32 * 0.1, -0.7];
        gyro.on_motion(0, GYRO_KIND, xyz, t);
        let expected = [
            client[0].observe(gyro_rate(
                xyz,
                GyroAxes {
                    yaw: YawSource::Y,
                    ..axes
                },
                false,
            )),
            client[1].observe(gyro_rate(
                xyz,
                GyroAxes {
                    yaw: YawSource::Z,
                    ..axes
                },
                true,
            )),
        ];
        assert_eq!(gyro.samples.back().unwrap().rate, expected);
        t += 4_000;
    }
    // Lying still, the bias learned the readings away.
    let still = gyro.samples.back().unwrap().rate;
    assert!(still.iter().flatten().all(|r| r.abs() < 0.01), "{still:?}");
}

#[test]
fn tags_name_the_reading_held_at_the_reflection_time() {
    let mut gyro = GyroHistory::new(Some(0), AXES);
    assert_eq!(gyro.tag(5_000), (0, HELD_NONE));
    gyro.on_motion(0, GYRO_KIND, [0.0; 3], 10_000);
    gyro.on_motion(0, GYRO_KIND, [0.0; 3], 14_000);
    assert_eq!(gyro.tag(15_500), (2, 1_500));
    assert_eq!(gyro.tag(12_000), (1, 2_000));
    // Before the first kept reading: the one before it, not held.
    assert_eq!(gyro.tag(9_000), (0, HELD_NONE));
}

#[test]
fn an_automatic_yaw_waits_for_a_clear_winner_and_keeps_it() {
    let mut choice = YawChoice::new(YawSource::Auto);
    assert!(!choice.decided());
    assert!(!choice.observe([Some(0.4), Some(0.3)]));
    assert!(!choice.decided());
    assert!(choice.observe([Some(0.2), Some(0.9)]));
    assert!(choice.decided());
    // Close calls keep it.
    assert!(choice.observe([Some(0.85), Some(0.8)]));
    assert!(!choice.observe([Some(0.95), Some(0.6)]));
    // A request's axis is fixed.
    let mut fixed = YawChoice::new(YawSource::Z);
    assert!(fixed.decided() && fixed.observe([Some(1.0), Some(0.0)]));
}
