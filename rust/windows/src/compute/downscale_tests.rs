//! Independent pixel-coverage checks of the D3D12 downscale filter.
use super::{tests::p010_codes, *};
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use std::time::Instant;

pub(super) fn pattern(width: usize, height: usize, phase: usize, hdr: bool) -> (Image, Vec<f64>) {
    let samples: Vec<_> = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let white = if y < height / 2 {
                !(x + phase).is_multiple_of(2)
            } else {
                (x + phase).is_multiple_of(3) ^ (y + phase).is_multiple_of(2)
            };
            f64::from(u8::from(white))
        })
        .collect();
    let pixel_bytes = if hdr { 8 } else { 4 };
    let mut bytes = Vec::with_capacity(width * height * pixel_bytes);
    for sample in &samples {
        if hdr {
            for value in [*sample as f32 * 12.5; 3].into_iter().chain([1.]) {
                bytes.extend_from_slice(&half::f16::from_f32(value).to_le_bytes());
            }
        } else {
            bytes.extend_from_slice(&[(*sample * 255.) as u8; 3]);
            bytes.push(255);
        }
    }
    (
        Image {
            width: width as u32,
            height: height as u32,
            stride: width * pixel_bytes,
            bytes,
            pixel: if hdr { Pixel::RgbaF16 } else { Pixel::Bgra8 },
            captured: Instant::now(),
        },
        samples,
    )
}

/// The independent reference intersects the destination rectangle with EVERY
/// source pixel, rather than reproducing the shader's local bounded loops.
/// Border pixels extend outward indefinitely, matching clamped texture access
/// when a fractional letterbox edge intersects a destination pixel.
fn coverage(samples: &[f64], width: usize, height: usize, rect: [f64; 4]) -> f64 {
    let [left, top, right, bottom] = rect;
    let mut sum = 0.;
    for y in 0..height {
        let start_y = if y == 0 { f64::NEG_INFINITY } else { y as f64 };
        let end_y = if y + 1 == height {
            f64::INFINITY
        } else {
            y as f64 + 1.
        };
        let vertical = (bottom.min(end_y) - top.max(start_y)).max(0.);
        if vertical == 0. {
            continue;
        }
        for x in 0..width {
            let start_x = if x == 0 { f64::NEG_INFINITY } else { x as f64 };
            let end_x = if x + 1 == width {
                f64::INFINITY
            } else {
                x as f64 + 1.
            };
            let horizontal = (right.min(end_x) - left.max(start_x)).max(0.);
            sum += samples[y * width + x] * horizontal * vertical;
        }
    }
    sum / ((right - left) * (bottom - top))
}

fn interpolate(samples: &[f64], width: usize, height: usize, x: f64, y: f64) -> f64 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let sample = |x: f64, y: f64| {
        samples[y.clamp(0., (height - 1) as f64) as usize * width
            + x.clamp(0., (width - 1) as f64) as usize]
    };
    (sample(x0, y0) * (1. - fx) + sample(x0 + 1., y0) * fx) * (1. - fy)
        + (sample(x0, y0 + 1.) * (1. - fx) + sample(x0 + 1., y0 + 1.) * fx) * fy
}

fn reference(
    samples: &[f64],
    source: (usize, usize),
    target: (usize, usize),
    hdr: bool,
) -> Vec<u16> {
    let (sw, sh) = source;
    let (tw, th) = target;
    let scale = (tw as f64 / sw as f64).min(th as f64 / sh as f64);
    let (mut cw, mut ch) = (sw as f64 * scale, sh as f64 * scale);
    // The existing fit policy fills a subpixel aspect mismatch independently
    // on each axis, avoiding a line caused only by integer size rounding.
    if (tw as f64 - cw).abs() < 1. && (th as f64 - ch).abs() < 1. {
        (cw, ch) = (tw as f64, th as f64);
    }
    let (scale_x, scale_y) = (cw / sw as f64, ch / sh as f64);
    let (ox, oy) = ((tw as f64 - cw) / 2., (th as f64 - ch) / 2.);
    let mut output = Vec::with_capacity(tw * th * 3 / 2);
    for y in 0..th {
        for x in 0..tw {
            let (cx, cy) = (x as f64 + 0.5 - ox, y as f64 + 0.5 - oy);
            let value = if cx < 0. || cy < 0. || cx >= cw || cy >= ch {
                0.
            } else if scale_x > 1. || scale_y > 1. || scale_x < 1. / 8. || scale_y < 1. / 8. {
                // Enlargement and extreme reduction explicitly retain the
                // former bilinear behavior; these are compatibility controls.
                interpolate(samples, sw, sh, cx / scale_x - 0.5, cy / scale_y - 0.5)
            } else {
                coverage(
                    samples,
                    sw,
                    sh,
                    [
                        (cx - 0.5) / scale_x,
                        (cy - 0.5) / scale_y,
                        (cx + 0.5) / scale_x,
                        (cy + 0.5) / scale_y,
                    ],
                )
            };
            let value = if hdr {
                // The FP16 pattern is 0 or 12.5 scRGB (1000 nits), filtered
                // in linear light before the unchanged ST.2084 conversion.
                let power = (value * 0.1).powf(2610. / 16384.);
                ((3424. / 4096. + 2413. / 128. * power) / (1. + 2392. / 128. * power))
                    .powf(2523. / 32.)
            } else {
                value
            };
            output.push((64. + 876. * value).round() as u16);
        }
    }
    output.resize(tw * th * 3 / 2, 512);
    output
}

#[test]
fn downscale_reference_covers_pixels_skipped_by_bilinear() {
    let samples = [0., 1., 0., 1., 0., 1.];
    assert_eq!(coverage(&samples, 6, 1, [0., 0., 3., 1.]), 1. / 3.);
    assert_eq!(coverage(&samples, 6, 1, [3., 0., 6., 1.]), 2. / 3.);
    assert_eq!(coverage(&samples, 6, 1, [0., 0., 2., 1.]), 0.5);
    assert!((coverage(&samples, 6, 1, [0.5, 0., 3.5, 1.]) - 0.5).abs() < 1e-12);
    // Both axes extend the corner, rather than introducing black beyond it.
    assert_eq!(coverage(&[1., 0., 0., 0.], 2, 2, [-1., -1., 1., 1.]), 1.);
    assert_eq!(coverage(&[0., 0., 0., 1.], 2, 2, [1., 1., 3., 3.]), 1.);
}

fn error(actual: &[u16], expected: &[u16]) -> serde_json::Value {
    let squared = actual
        .iter()
        .zip(expected)
        .map(|(a, e)| (f64::from(*a) - f64::from(*e)).powi(2))
        .sum::<f64>()
        / actual.len() as f64;
    serde_json::json!({
        "rmse_codes": squared.sqrt(),
        "psnr_db": if squared == 0. { None } else { Some(10. * (1023. * 1023. / squared).log10()) },
        "max_error_codes": actual.iter().zip(expected).map(|(a,e)| a.abs_diff(*e)).max(),
    })
}

#[test]
#[ignore = "requires D3D12 compute with native P010 views; no display or encoder changes"]
fn gpu_downscale_coverage_matches_independent_reference() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = &gpu.compute;
    let mut reports = Vec::new();
    for hdr in [false, true] {
        for (source_width, source_height, width, height) in [
            (96, 48, 96, 48),
            (96, 48, 144, 72),
            (96, 48, 48, 24),
            (96, 48, 72, 36),
            (96, 48, 36, 18),
            (96, 48, 32, 16),
            (96, 48, 32, 32),
            (96, 48, 12, 6),
            (96, 48, 8, 4),
            (95, 51, 32, 24), // Fractional bars cross an edge pixel's footprint.
            (95, 49, 32, 16), // Subpixel aspect mismatch fills both target axes.
        ] {
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                codec: 1,
                hdr,
                sdr_10bit: !hdr,
                ..Default::default()
            };
            let pixel = if hdr { Pixel::RgbaF16 } else { Pixel::Bgra8 };
            let shape = (source_width, source_height, pixel);
            let mut converter = Converter::new(compute.clone(), width, height, true)?;
            converter.values = constants(&config, shape, [100., 1.]);
            for phase in 0..3 {
                let (image, samples) =
                    pattern(source_width as usize, source_height as usize, phase, hdr);
                let source = GpuImage::upload(&gpu, &image)?;
                let expected = reference(
                    &samples,
                    (source_width as usize, source_height as usize),
                    (width as usize, height as usize),
                    hdr,
                );
                let converted = converter.convert(&source)?;
                let actual = p010_codes(compute, &converted)?;
                converter.release_completed();
                assert_eq!(actual.len(), expected.len());
                for (index, (a, e)) in actual.iter().zip(&expected).enumerate() {
                    assert!(
                        a.abs_diff(*e) <= 1,
                        "{source_width}x{source_height} -> {width}x{height} HDR={hdr} phase={phase} index={index}: {a} versus {e}"
                    );
                }
                let luma = (width * height) as usize;
                reports.push(
                    serde_json::json!({"source_width":source_width,"source_height":source_height,"width":width,"height":height,"hdr":hdr,"phase":phase,
                    "candidate":error(&actual[..luma], &expected[..luma])}),
                );
            }
        }
    }
    let report = serde_json::json!({"scope":"GPU luma versus independent exact pixel coverage; grayscale scrolling patterns, 10-bit SDR and linear-light HDR; all Y/UV samples verified on D3D12 compute. Native/enlargement/extreme reduction are compatibility controls. Excludes encoding and perceptual/game quality.","adapter":gpu.display.adapter,"cases":reports});
    eprintln!("{report}");
    if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_DOWNSCALE_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(())
}
