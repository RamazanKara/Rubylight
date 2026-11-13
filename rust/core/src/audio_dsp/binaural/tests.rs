use super::*;

const SEVEN_ONE: u32 = 0x63f;
const FIVE_ONE_BACK: u32 = 0x3f;
const FIVE_ONE_SIDE: u32 = 0x60f;

fn impulse(slot: usize, frames: usize) -> Vec<f32> {
    let mut input = vec![0.; frames * 8];
    input[slot] = 1.;
    input
}

fn render(binaural: &mut Binaural, input: &[f32]) -> Vec<f32> {
    let mut output = vec![0.; input.len() / 4];
    binaural.render(input, &mut output);
    output
}

fn ear(output: &[f32], ear: usize) -> Vec<f32> {
    output.iter().skip(ear).step_by(2).copied().collect()
}

fn noise(frames: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..frames * 8)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        })
        .collect()
}

#[test]
fn the_7_1_and_5_1_direction_sets() {
    let seven = Layout::new(8, SEVEN_ONE).unwrap();
    assert!(seven.surround());
    assert_eq!(
        seven.directions(),
        [
            Some(30),
            Some(-30),
            Some(0),
            None,
            Some(135),
            Some(-135),
            Some(90),
            Some(-90)
        ]
    );
    let back = Layout::new(6, FIVE_ONE_BACK).unwrap();
    assert_eq!(
        back.directions(),
        [
            Some(30),
            Some(-30),
            Some(0),
            None,
            Some(110),
            Some(-110),
            None,
            None
        ]
    );
    let side = Layout::new(6, FIVE_ONE_SIDE).unwrap();
    assert_eq!(
        side.directions(),
        [
            Some(30),
            Some(-30),
            Some(0),
            None,
            None,
            None,
            Some(110),
            Some(-110)
        ]
    );
    // No surround speakers: today's downmix stays.
    let stereo = Layout::new(2, 3).unwrap();
    assert!(!stereo.surround());
    assert_eq!(stereo.downmix[0], [1., 0., 0., 0., 0., 0., 0., 0.]);
    assert_eq!(stereo.downmix[1], [0., 1., 0., 0., 0., 0., 0., 0.]);
    assert!(!Layout::new(1, 4).unwrap().surround());
}

#[test]
fn the_downmix_of_the_8_slots_matches_2_channel_capture() {
    for (channels, mask) in [
        (8, SEVEN_ONE),
        (6, FIVE_ONE_BACK),
        (6, FIVE_ONE_SIDE),
        (2, 3),
        (1, 4),
    ] {
        let layout = Layout::new(channels, mask).unwrap();
        let stereo = crate::audio::mix_matrix(channels, mask, 2).unwrap();
        let eight = crate::audio::mix_matrix(channels, mask, 8).unwrap();
        let source: Vec<f32> = (0..channels).map(|i| 0.1 + i as f32 * 0.07).collect();
        let capture: Vec<f32> = eight
            .iter()
            .map(|row| row.iter().zip(&source).map(|(a, b)| a * b).sum())
            .collect();
        let mut output = [0.; 2];
        downmix(&layout.downmix, &capture, &mut output);
        for ear in 0..2 {
            let want: f32 = stereo[ear].iter().zip(&source).map(|(a, b)| a * b).sum();
            assert!((output[ear] - want).abs() < 1e-6, "{channels} {mask:x}");
        }
    }
}

#[test]
fn a_stereo_downmix_is_bit_exact() {
    let layout = Layout::new(2, 3).unwrap();
    let mut input = noise(480, 7);
    input[0] = -0.;
    input[9] = f32::MIN_POSITIVE / 4.;
    let mut output = vec![0.; 960];
    downmix(&layout.downmix, &input, &mut output);
    for (out, frame) in output
        .as_chunks::<2>()
        .0
        .iter()
        .zip(input.as_chunks::<8>().0)
    {
        assert_eq!(out[0].to_bits(), frame[0].to_bits());
        assert_eq!(out[1].to_bits(), frame[1].to_bits());
    }
}

#[test]
fn an_impulse_in_each_slot_comes_out_as_its_hrir() {
    let layout = Layout::new(8, SEVEN_ONE).unwrap();
    let mut binaural = Binaural::new(&layout, 1.);
    for (slot, azimuth) in layout.directions().iter().enumerate() {
        let output = render(&mut binaural, &impulse(slot, TAPS * 2));
        match azimuth {
            Some(azimuth) => {
                let pair = hrir::pair(*azimuth);
                for (e, filter) in pair.iter().enumerate() {
                    let got = ear(&output, e);
                    let want: Vec<f32> = filter
                        .iter()
                        .map(|v| if v.abs() < TAP_FLOOR { 0. } else { *v })
                        .collect();
                    assert_eq!(&got[..TAPS], &want[..], "{slot} {e}");
                    assert!(got[TAPS..].iter().all(|v| *v == 0.));
                }
            }
            None => {
                // LFE: both ears at -6 dB, unfiltered.
                assert_eq!(&output[..2], &[0.5, 0.5]);
                assert!(output[2..].iter().all(|v| *v == 0.));
            }
        }
    }
}

#[test]
fn no_scaled_tap_is_below_the_floor() {
    // The SADIE fade leaves ~1e-37 in every filter's last tap.
    assert!(
        hrir::pair(30)
            .iter()
            .any(|h| h.iter().any(|v| *v != 0. && v.abs() < TAP_FLOOR))
    );
    for (channels, mask) in [(8, SEVEN_ONE), (6, FIVE_ONE_BACK), (6, FIVE_ONE_SIDE)] {
        let layout = Layout::new(channels, mask).unwrap();
        for level in [layout.level(), 1.] {
            let binaural = Binaural::new(&layout, level);
            for speaker in &binaural.speakers {
                for v in speaker.taps.iter().flatten() {
                    assert!(*v == 0. || v.abs() >= TAP_FLOOR, "{mask:x} {v:e}");
                }
            }
        }
    }
}

#[test]
fn a_normal_level_sine_makes_no_subnormal_products_or_outputs() {
    let layout = Layout::new(8, SEVEN_ONE).unwrap();
    let mut binaural = Binaural::new(&layout, layout.level());
    let frames = 4800;
    let input: Vec<f32> = (0..frames * 8)
        .map(|i| {
            let (n, slot) = (i / 8, i % 8);
            let hz = 220. + 110. * slot as f32;
            0.05 * (std::f32::consts::TAU * hz * n as f32 / 48_000.).sin()
        })
        .collect();
    let subnormal = |v: f32| v != 0. && !v.is_normal();
    assert!(!input.iter().any(|v| subnormal(*v)));
    for chunk in input.chunks(480 * 8) {
        // Every product the FIR forms over this packet: history and packet
        // samples against every tap of both ears.
        for speaker in &binaural.speakers {
            let line: Vec<f32> = speaker
                .line
                .iter()
                .copied()
                .chain(chunk.iter().skip(speaker.slot).step_by(8).copied())
                .collect();
            for h in speaker.taps.iter().flatten() {
                assert!(!line.iter().any(|x| subnormal(h * x)), "{h:e}");
            }
        }
        let output = render(&mut binaural, chunk);
        assert!(!output.iter().any(|v| subnormal(*v)));
    }
}

#[test]
fn front_left_reaches_the_left_ear_earlier_and_louder_and_mirrors_front_right() {
    let layout = Layout::new(8, SEVEN_ONE).unwrap();
    let mut binaural = Binaural::new(&layout, 1.);
    let fl = render(&mut binaural, &impulse(0, TAPS));
    let fr = render(&mut binaural, &impulse(1, TAPS));
    let (left, right) = (ear(&fl, 0), ear(&fl, 1));
    let onset = |h: &[f32]| h.iter().position(|v| v.abs() > 0.05).unwrap();
    let energy = |h: &[f32]| h.iter().map(|v| v * v).sum::<f32>();
    assert!(onset(&left) < onset(&right));
    assert!(energy(&left) > 2. * energy(&right));
    assert_eq!(ear(&fr, 1), left);
    assert_eq!(ear(&fr, 0), right);
    let fc = render(&mut binaural, &impulse(2, TAPS));
    assert_eq!(ear(&fc, 0), ear(&fc, 1));
}

#[test]
fn rendering_matches_an_f64_reference_convolution_in_any_packet_size() {
    let layout = Layout::new(8, SEVEN_ONE).unwrap();
    let level = layout.level();
    let input = noise(4800, 1);
    let frames = input.len() / 8;
    let mut want = vec![0f64; frames * 2];
    for (slot, azimuth) in layout.directions().iter().enumerate() {
        for n in 0..frames {
            let x = |k: usize| input[(n - k) * 8 + slot] as f64;
            for e in 0..2 {
                want[n * 2 + e] += match azimuth {
                    Some(azimuth) => {
                        let h = &hrir::pair(*azimuth)[e];
                        (0..TAPS.min(n + 1))
                            .map(|k| h[k] as f64 * x(k))
                            .sum::<f64>()
                    }
                    None => 0.5 * x(0),
                } * level as f64;
            }
        }
    }
    for packet in [100, 240, 480, 960, 4800] {
        let mut binaural = Binaural::new(&layout, level);
        let mut got = Vec::new();
        for chunk in input.chunks(packet * 8) {
            got.extend(render(&mut binaural, chunk));
        }
        let error = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (*a as f64 - b).abs())
            .fold(0., f64::max);
        assert!(error < 1e-5, "{packet}: {error}");
    }
}

#[test]
fn listening_keeps_the_history_so_a_switch_is_seamless() {
    let layout = Layout::new(6, FIVE_ONE_BACK).unwrap();
    let input = noise(960, 3);
    let mut always = Binaural::new(&layout, 1.);
    let whole = render(&mut always, &input);
    let mut switched = Binaural::new(&layout, 1.);
    switched.listen(&input[..480 * 8]);
    let tail = render(&mut switched, &input[480 * 8..]);
    assert_eq!(tail, whole[960..]);
}

#[test]
fn noise_through_the_hrirs_is_as_loud_as_the_downmix() {
    for (channels, mask) in [(8, SEVEN_ONE), (6, FIVE_ONE_BACK), (6, FIVE_ONE_SIDE)] {
        let layout = Layout::new(channels, mask).unwrap();
        let mut input = noise(48_000 * 2, 11);
        for frame in input.as_chunks_mut::<8>().0 {
            for (slot, v) in frame.iter_mut().enumerate() {
                if !layout.present[slot] {
                    *v = 0.;
                }
            }
        }
        let mut binaural = Binaural::new(&layout, layout.level());
        let surround = render(&mut binaural, &input);
        let mut stereo = vec![0.; surround.len()];
        downmix(&layout.downmix, &input, &mut stereo);
        let power = |s: &[f32]| s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
        let db = 10. * (power(&surround) / power(&stereo)).log10();
        assert!(db.abs() < 0.5, "{channels} {mask:x}: {db} dB");
    }
}
