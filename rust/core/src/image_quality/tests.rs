use super::*;

fn spec(transfer: Transfer, source: Source, width: usize, height: usize) -> Spec {
    Spec {
        width,
        height,
        source,
        transfer,
        matrix: if transfer == Transfer::Sdr { 1 } else { 2 },
        full_range: false,
        ten_bit: transfer != Transfer::Sdr,
    }
}
fn flat(width: usize, height: usize, rgb: [f64; 3]) -> Picture {
    Picture {
        width,
        height,
        rgb: vec![rgb; width * height],
    }
}
fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

#[test]
fn transfer_functions_invert() {
    for v in [0., 0.01, 0.2, 0.5, 0.9, 1.] {
        assert!(close(srgb_encode(srgb_decode(v)), v, 1e-9));
        // ST 2084 puts 0 nits at C1^M2 (about 7e-7), not 0, so the round trip holds from there.
        assert!(close(pq(depq(v.max(pq(0.)))), v.max(pq(0.)), 1e-9));
        assert!(close(depq(pq(v)), v, 1e-9));
        assert!(close(bt2020_oetf(bt2020_eotf(v)), v, 1e-9));
        assert!(close(hlg_oetf(hlg_inverse_oetf(v)), v, 1e-9));
    }
    // 100 nits is PQ code 0.508; 10000 nits is 1.
    assert!(close(pq(0.01), 0.5081, 1e-3));
    assert!(close(pq(1.), 1., 1e-12));
    let display = [0.3, 0.2, 0.1];
    let back = hlg_display(hlg_scene(display));
    for c in 0..3 {
        assert!(close(back[c], display[c], 1e-9));
    }
}

#[test]
fn sdr_white_and_black_land_on_the_studio_range_codes() {
    let s = spec(Transfer::Sdr, Source::Bgra8, 16, 16);
    let white = reference(&flat(16, 16, [1.; 3]), &s);
    assert!(white.y.iter().all(|&y| close(y, 235., 1e-9)));
    assert!(white.cb.iter().all(|&c| close(c, 128., 1e-9)));
    let black = reference(&flat(16, 16, [0.; 3]), &s);
    assert!(black.y.iter().all(|&y| close(y, 16., 1e-9)));
    let mut full = s;
    full.full_range = true;
    assert!(
        reference(&flat(16, 16, [1.; 3]), &full)
            .y
            .iter()
            .all(|&y| close(y, 255., 1e-9))
    );
}

#[test]
fn hdr_80_nit_white_lands_on_its_pq_code() {
    let s = spec(Transfer::Pq, Source::ScRgbF16, 16, 16);
    let planes = reference(&flat(16, 16, [1.; 3]), &s);
    // 80 nits white: PQ 0.4867, 10-bit studio range 64 + 876 * code. The BT.709 to BT.2020
    // matrix is the shaders' (rows rounded to six places, so G and B sum to 0.999999): exact
    // through it, and within a thousandth of a code of the ideal white.
    let code = each(apply(&BT709_TO_2020, [80.; 3]), |c| pq(c / 10000.));
    let (expected, expected_cr) = (luma_code(&s, code), chroma_codes(&s, code).1);
    assert!(close(expected, 64. + 876. * pq(0.008), 1e-3) && close(expected_cr, 512., 1e-3));
    assert!(planes.y.iter().all(|&y| close(y, expected, 1e-6)));
    assert!(planes.cr.iter().all(|&c| close(c, expected_cr, 1e-6)));
}

#[test]
fn codes_decode_back_to_their_r_g_b() {
    for s in [
        spec(Transfer::Sdr, Source::Bgra8, 2, 2),
        spec(Transfer::Pq, Source::ScRgbF16, 2, 2),
        Spec {
            matrix: 0,
            full_range: true,
            ..spec(Transfer::Sdr, Source::Bgra8, 2, 2)
        },
    ] {
        let rgb = [0.7, 0.3, 0.5];
        let (cb, cr) = chroma_codes(&s, rgb);
        let back = rgb_from_codes(&s, luma_code(&s, rgb), cb, cr);
        for c in 0..3 {
            assert!(close(back[c], rgb[c], 1e-9), "{s:?}");
        }
    }
}

#[test]
fn identical_planes_score_perfectly_and_noise_scores_lower() {
    let a: Vec<f64> = (0..64 * 64).map(|i| 100. + (i % 64) as f64 * 0.5).collect();
    assert_eq!(psnr(&a, &a, 255.), f64::INFINITY);
    assert!(close(ssim(&a, &a, 64, 64, 255.), 1., 1e-12));
    let b: Vec<f64> = a.iter().map(|v| v + 1.).collect();
    // A uniform error of one code: 10 log10(255^2) = 48.13 dB.
    assert!(close(psnr(&b, &a, 255.), 48.13, 0.01));
    let noisy: Vec<f64> = a
        .iter()
        .enumerate()
        .map(|(i, v)| v + if i % 2 == 0 { 20. } else { -20. })
        .collect();
    assert!(ssim(&noisy, &a, 64, 64, 255.) < 0.95);
}

#[test]
fn delta_e_is_zero_for_the_same_colour_and_grows_with_distance() {
    let white = ictcp([100.; 3]);
    assert_eq!(delta_e_itp(white, white), 0.);
    let warmer = ictcp([104., 100., 96.]);
    let warmest = ictcp([120., 100., 80.]);
    assert!(delta_e_itp(white, warmer) > 0.5);
    assert!(delta_e_itp(white, warmest) > delta_e_itp(white, warmer));
    let s = spec(Transfer::Sdr, Source::Bgra8, 16, 16);
    let planes = reference(&flat(16, 16, [0.5, 0.4, 0.3]), &s);
    assert_eq!(delta_e(&planes, &planes, &s), [0.; 3]);
}

#[test]
fn a_halving_averages_each_two_by_two_block() {
    let s = spec(Transfer::Sdr, Source::Bgra8, 4, 4);
    let mut source = flat(8, 8, [0.; 3]);
    for y in 0..8 {
        for x in 0..8 {
            if (x + y) % 2 == 0 {
                source.rgb[y * 8 + x] = [1.; 3];
            }
        }
    }
    let rgb = nonlinear(&source, &s);
    assert!(rgb.iter().all(|p| close(p[0], 0.5, 1e-9)));
}

#[test]
fn hdr_resamples_in_linear_light() {
    let s = spec(Transfer::Pq, Source::ScRgbF16, 2, 2);
    let mut source = flat(4, 4, [0.; 3]);
    for (i, p) in source.rgb.iter_mut().enumerate() {
        if i % 2 == 0 {
            *p = [2.; 3];
        }
    }
    // Half of 160 nits is 80 nits, not the PQ code halfway to black.
    let rgb = nonlinear(&source, &s);
    // Through the shaders' BT.709 to BT.2020 matrix, whose green row sums to 0.999999.
    let green = apply(&BT709_TO_2020, [80.; 3])[1];
    assert!(close(rgb[0][1], pq(green / 10000.), 1e-9));
    assert!(close(rgb[0][1], pq(0.008), 1e-6));
}

#[test]
fn other_shapes_get_black_bars() {
    let s = spec(Transfer::Sdr, Source::Bgra8, 16, 8);
    let rgb = nonlinear(&flat(8, 8, [1.; 3]), &s);
    // An 8x8 square in a 16x8 stream: four columns of black each side.
    assert_eq!(rgb[0], [0.; 3]);
    assert_eq!(rgb[3], [0.; 3]);
    assert_eq!(rgb[4], [1.; 3]);
    assert_eq!(rgb[11], [1.; 3]);
    assert_eq!(rgb[12], [0.; 3]);
}

#[test]
fn planes_read_an_nv12_layout() {
    let codes: Vec<u16> = (0..4 * 2 + 4).collect();
    let planes = Planes::from_semi_planar(&codes, 4, 2).unwrap();
    assert_eq!(planes.y, (0..8).map(f64::from).collect::<Vec<_>>());
    assert_eq!(planes.cb, vec![8., 10.]);
    assert_eq!(planes.cr, vec![9., 11.]);
    assert!(Planes::from_semi_planar(&codes[..10], 4, 2).is_none());
}

#[test]
fn the_reference_chroma_is_left_sited_and_vertically_centred() {
    for (transfer, source) in [
        (Transfer::Sdr, Source::Bgra8),
        (Transfer::Pq, Source::ScRgbF16),
        (Transfer::Hlg, Source::ScRgbF16),
        (Transfer::Pq, Source::Pq10),
    ] {
        let s = spec(transfer, source, 64, 32);
        let across = reference(&ramp(&s, true), &s);
        let row: Vec<f64> = across.cr[..32].to_vec();
        let h = siting(&row, |x| ramp_chroma(&s, true, x).1).unwrap();
        assert!(close(h, 0., 1e-3), "{transfer:?} horizontal {h}");
        let down = reference(&ramp(&s, false), &s);
        let column: Vec<f64> = (0..16).map(|j| down.cr[j * 32]).collect();
        let v = siting(&column, |y| ramp_chroma(&s, false, y).1).unwrap();
        assert!(close(v, 0.5, 1e-3), "{transfer:?} vertical {v}");
    }
}

#[test]
fn siting_finds_a_centred_converter() {
    let s = spec(Transfer::Sdr, Source::Bgra8, 64, 32);
    // A converter averaging luma samples 2i and 2i+1 samples at 2i + 0.5.
    let row: Vec<f64> = (0..32)
        .map(|i| ramp_chroma(&s, true, 2. * i as f64 + 0.5).1)
        .collect();
    let offset = siting(&row, |x| ramp_chroma(&s, true, x).1).unwrap();
    assert!(close(offset, 0.5, 1e-9));
    assert_eq!(siting(&row[..4], |x| x), None);
}

#[test]
fn summary_reports_mean_p99_and_worst() {
    let mut values: Vec<f64> = (1..=100).map(f64::from).collect();
    let [mean, p99, worst] = summarize(&mut values);
    assert_eq!(mean, 50.5);
    assert_eq!(p99, 99.);
    assert_eq!(worst, 100.);
    assert!(summarize(&mut []).iter().all(|v| v.is_nan()));
}
