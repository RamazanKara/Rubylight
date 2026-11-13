use super::*;

fn sine(dbfs: f32, hz: f32, frames: usize, channels: usize) -> Vec<f32> {
    let amplitude = 10f32.powf(dbfs / 20.);
    (0..frames)
        .flat_map(|n| {
            let v = amplitude * (std::f32::consts::TAU * hz * n as f32 / RATE).sin();
            std::iter::repeat_n(v, channels)
        })
        .collect()
}

fn rms_db(samples: &[f32]) -> f32 {
    10. * (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).log10()
}

#[test]
fn the_curve_is_3_to_1_above_minus_30_with_a_6_db_knee() {
    assert_eq!(curve(-60.), 0.);
    assert_eq!(curve(-33.), 0.);
    assert!((curve(-30.) - -0.5).abs() < 1e-5);
    assert!((curve(-27.) - -2.).abs() < 1e-5);
    assert!((curve(-6.) - -16.).abs() < 1e-4);
    assert!((curve(0.) - -20.).abs() < 1e-4);
}

#[test]
fn the_clipper_is_unity_below_minus_3_dbfs_and_bounded_by_0_98() {
    assert_eq!(soft_clip(0.5), 0.5);
    assert_eq!(soft_clip(-0.7), -0.7);
    for x in [0.75, 0.9, 1., 2., 100., f32::MAX] {
        let y = soft_clip(x);
        assert!(y > CLIP_KNEE && y <= CLIP_CEILING, "{x} -> {y}");
        assert_eq!(soft_clip(-x), -y);
    }
    // Monotonic, and continuous at the knee.
    let mut last = 0.;
    for i in 0..2000 {
        let y = soft_clip(i as f32 / 1000.);
        assert!(y >= last);
        assert!(y - last <= 0.0011);
        last = y;
    }
}

#[test]
fn silence_stays_exactly_zero() {
    let mut night = Night::default();
    let mut samples = vec![0f32; 48_000 * 2];
    night.process(&mut samples, 2, true);
    assert!(samples.iter().all(|x| x.to_bits() == 0));
}

#[test]
fn a_minus_6_dbfs_sine_gets_the_reduction_of_the_curve() {
    let mut night = Night::default();
    let mut samples = sine(-6., 1000., 48_000, 2);
    night.process(&mut samples, 2, true);
    assert!(
        (night.compressor_db() - curve(-6.)).abs() < 0.5,
        "{}",
        night.compressor_db()
    );
}

#[test]
fn a_step_from_minus_40_to_minus_6_dbfs_stays_under_0_98_and_settles_in_10_ms() {
    let mut night = Night::default();
    let mut quiet = sine(-40., 1000., 48_000 * 10, 2);
    night.process(&mut quiet, 2, true);
    assert!(night.compressor_db() > -0.1);
    let mut loud = sine(-6., 1000., 480, 2);
    night.process(&mut loud, 2, true);
    assert!(loud.iter().all(|x| x.abs() < CLIP_CEILING));
    assert!(
        (night.compressor_db() - curve(-6.)).abs() < 1.,
        "{}",
        night.compressor_db()
    );
}

#[test]
fn the_leveller_brings_a_quiet_sine_up_by_its_limit_and_a_moderate_one_to_minus_23() {
    let mut night = Night::default();
    let mut quiet = sine(-40., 440., 48_000 * 12, 2);
    night.process(&mut quiet, 2, true);
    assert!((night.leveller_db() - LEVEL_MAX_DB).abs() < 1.);
    let tail = &quiet[quiet.len() - 96_000..];
    assert!(
        (rms_db(tail) - (-43. + LEVEL_MAX_DB)).abs() < 1.,
        "{}",
        rms_db(tail)
    );

    let mut night = Night::default();
    let mut moderate = sine(-20., 440., 48_000 * 20, 2);
    night.process(&mut moderate, 2, true);
    let tail = &moderate[moderate.len() - 96_000..];
    assert!((rms_db(tail) - TARGET_DB).abs() < 1., "{}", rms_db(tail));
}

#[test]
fn the_leveller_holds_below_minus_60_dbfs_and_slews_at_3_db_per_second() {
    let mut night = Night::default();
    let mut hiss = sine(-67., 3000., 48_000 * 20, 2);
    let input = hiss.clone();
    night.process(&mut hiss, 2, true);
    assert!(night.leveller_db().abs() < 0.5, "{}", night.leveller_db());
    assert!(rms_db(&hiss[hiss.len() - 48_000..]) - rms_db(&input) < 0.5);

    // A pause after quiet speech keeps the gain it had.
    let mut night = Night::default();
    let mut speech = sine(-40., 440., 48_000 * 6, 2);
    night.process(&mut speech, 2, true);
    let before = night.leveller_db();
    assert!(before > 5.);
    let mut pause = sine(-70., 440., 48_000 * 10, 2);
    night.process(&mut pause, 2, true);
    assert!(
        (night.leveller_db() - before).abs() < 0.2,
        "{before} {}",
        night.leveller_db()
    );

    let mut night = Night::default();
    let mut quiet = sine(-40., 440., 48_000, 2);
    night.process(&mut quiet, 2, true);
    assert!(night.leveller_db() <= 3.0001, "{}", night.leveller_db());
}

#[test]
fn gain_is_linked_across_channels() {
    let mut night = Night::default();
    let mut samples: Vec<f32> = sine(-6., 500., 9600, 1)
        .into_iter()
        .flat_map(|v| [v, 0., 0., 0., 0., 0.])
        .collect();
    night.process(&mut samples, 6, true);
    for frame in samples.as_chunks::<6>().0 {
        assert!(frame[1..].iter().all(|x| x.to_bits() == 0));
    }
}

#[test]
fn the_leveller_reads_stereo_the_same_in_2_and_8_channels_without_the_lfe() {
    let left = sine(-35., 440., 48_000 * 4, 1);
    let right = sine(-41., 660., 48_000 * 4, 1);
    let stereo: Vec<f32> = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r])
        .collect();
    let padded: Vec<f32> = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r, 0., 0., 0., 0., 0., 0.])
        .collect();
    let mut two = Night::default();
    two.process(&mut stereo.clone(), 2, true);
    let mut eight = Night::default();
    eight.process(&mut padded.clone(), 8, true);
    assert!(two.leveller_db() > 3., "{}", two.leveller_db());
    assert_eq!(eight.leveller_db(), two.leveller_db());
    assert_eq!(eight.compressor_db(), two.compressor_db());
    // The LFE's power doesn't count (its peaks, linked, would).
    let rumble = sine(-10., 40., 48_000 * 4, 1);
    let mut lfe = padded;
    for (frame, x) in lfe.as_chunks_mut::<8>().0.iter_mut().zip(&rumble) {
        frame[LFE] = *x * 1e-3;
    }
    let mut with_lfe = Night::default();
    with_lfe.process(&mut lfe, 8, true);
    assert_eq!(with_lfe.leveller_db(), two.leveller_db());
}

#[test]
fn an_impulse_peaks_at_sample_0() {
    let mut night = Night::default();
    let mut samples = vec![0f32; 960];
    samples[0] = 1.;
    samples[1] = 1.;
    night.process(&mut samples, 2, true);
    assert!(samples[2..].iter().all(|x| x.abs() < samples[0]));
    assert_eq!(samples[0], samples[1]);
    assert!(samples[0] > 0.9 && samples[0] < CLIP_CEILING);
}

#[test]
fn output_does_not_depend_on_the_packet_size() {
    let mut input = sine(-30., 300., 48_000 * 2, 2);
    for (i, x) in sine(-8., 2100., 48_000 * 2, 2).into_iter().enumerate() {
        if (i / 20_000) % 2 == 1 {
            input[i] += x;
        }
    }
    let mut whole = input.clone();
    Night::default().process(&mut whole, 2, true);
    for ms in [5, 10, 20] {
        let mut night = Night::default();
        let mut packets = input.clone();
        for packet in packets.chunks_mut(48 * ms * 2) {
            night.process(packet, 2, true);
        }
        assert_eq!(packets, whole, "{ms} ms");
    }
}

#[test]
fn turning_it_off_ramps_to_unity_without_a_step_then_bypasses() {
    let mut night = Night::default();
    let mut on = sine(-40., 200., 48_000 * 6, 2);
    night.process(&mut on, 2, true);
    assert!(night.gain() > 2.);
    let input = sine(-40., 200., 48_000, 2);
    let mut off = input.clone();
    for packet in off.chunks_mut(480) {
        night.process(packet, 2, false);
    }
    let largest = |s: &[f32]| {
        s.as_chunks::<2>()
            .0
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0f32, f32::max)
    };
    let before = largest(&on[on.len() - 9600..]);
    let joined: Vec<f32> = on[on.len() - 9600..]
        .iter()
        .chain(&off[..9600])
        .copied()
        .collect();
    assert!(largest(&joined) <= before * 1.01);
    // After 10 ms the samples pass through untouched.
    assert_eq!(&off[960..], &input[960..]);
}

#[test]
fn subnormal_input_and_decaying_state_never_stay_subnormal() {
    let mut night = Night::default();
    let mut tiny: Vec<f32> = (0..9600)
        .map(|i| [1e-40, -1e-39, 1e-31, f32::MIN_POSITIVE][i % 4])
        .collect();
    night.process(&mut tiny, 2, true);
    assert!(tiny.iter().all(|x| x.to_bits() == 0));
    // A loud burst, then 30 s of silence: the detector and the
    // compressor decay toward 0 without passing through the subnormals.
    let mut burst = sine(0., 1000., 4800, 2);
    night.process(&mut burst, 2, true);
    assert!(night.compressor_db() < -10.);
    let mut silence = vec![0f32; 240 * 2];
    for _ in 0..30 * 200 {
        night.process(&mut silence, 2, true);
        assert!(!night.detector.is_subnormal(), "{:e}", night.detector);
        assert!(
            !night.compressor_db.is_subnormal(),
            "{:e}",
            night.compressor_db
        );
    }
    assert_eq!((night.detector, night.compressor_db), (0., 0.));
    assert!(silence.iter().all(|x| x.to_bits() == 0));
}

#[test]
fn turning_it_back_on_during_the_off_ramp_starts_from_the_ramps_gain() {
    let mut night = Night::default();
    let mut on = sine(-40., 150., 48_000 * 6, 2);
    night.process(&mut on, 2, true);
    let full = night.gain();
    assert!(full > 2., "{full}");
    // Half the ramp, then on again at the sine's peak.
    let input = sine(-40., 150., 480, 2);
    let mut off = input[..480].to_vec();
    night.process(&mut off, 2, false);
    let halfway = ramp(full, OFF_RAMP_FRAMES / 2);
    assert!((night.gain() - full).abs() < 1e-3);
    let mut again = input[480..].to_vec();
    night.process(&mut again[..2], 2, true);
    assert!(
        (night.gain() - halfway).abs() < 0.05 * halfway,
        "{} {halfway}",
        night.gain()
    );
    night.process(&mut again[2..], 2, true);
    // No step at either switch larger than the signal's own.
    let largest = |s: &[f32]| {
        s.as_chunks::<2>()
            .0
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0f32, f32::max)
    };
    let before = largest(&on[on.len() - 9600..]);
    let joined: Vec<f32> = on[on.len() - 9600..]
        .iter()
        .chain(&off)
        .chain(&again)
        .copied()
        .collect();
    assert!(largest(&joined) <= before * 1.01);
}

#[test]
fn nan_and_infinite_samples_become_zero_and_are_reported() {
    let mut night = Night::default();
    let mut samples = vec![0.1, f32::NAN, f32::INFINITY, -0.1];
    night.process(&mut samples, 2, true);
    assert!(samples.iter().all(|x| x.is_finite()));
    assert_eq!(samples[1], 0.);
    assert!(night.take_bad_samples());
    assert!(!night.take_bad_samples());
}
