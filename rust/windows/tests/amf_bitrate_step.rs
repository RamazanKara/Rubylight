//! Does changing the bitrate of a running AMF encoder force a keyframe, and how
//! fast does the rate follow? The link-rate controller steps the bitrate while
//! the stream runs, and a keyframe on every step would cost more than the step
//! saves (design risk 4).
//!
//! Needs an AMD adapter and nothing else using it; run by hand:
//!
//!   cargo test -p butterpollo-windows --test amf_bitrate_step --release -- --ignored --nocapture
//!
//! Two contents: per-pixel noise, the worst case for rate control (the encoder
//! always has more to spend than the target), and a gradient with mild noise.
//! H.264 has a floor on both (its quantiser limit): it settles at about 14 Mbps
//! on noise and 5.4 Mbps on the mild picture however low the target, and
//! follows every target above that, so those are what it is held to there. Per codec, 600 frames at 1080p60 step
//! 15, 8, 4, 6, 9, 15 Mbps every 30 frames, and none of them after the first
//! may be a keyframe. Then each level is held for 90 frames, and the mean over
//! the last 60 (one second) must be within 10 % of the target.
#![cfg(windows)]

use anyhow::{Result, bail};
use butterpollo_core::rtsp::Negotiated;
use butterpollo_windows::{
    capture::{ComGuard, Device, GpuImage, Image, Pixel},
    encoder::Encoder,
};
use std::time::{Duration, Instant};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: u32 = 60;
const STEPS_KBPS: [u32; 6] = [15_000, 8_000, 4_000, 6_000, 9_000, 15_000];
const STRESS_FRAMES: usize = 600;
const STRESS_HOLD: usize = 30;
const SETTLE_HOLD: usize = 90;
const IMAGES: usize = 6;
// Measured on this adapter: what H.264 settles at when asked for less.
const H264_NOISE_FLOOR_KBPS: u32 = 14_000;
const H264_MILD_FLOOR_KBPS: u32 = 5_400;

fn noise(seed: &mut u64) -> Vec<u8> {
    let mut bytes = vec![0u8; WIDTH as usize * HEIGHT as usize * 4];
    for (i, byte) in bytes.iter_mut().enumerate() {
        if i % 8 == 0 {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
        }
        *byte = (*seed >> (8 * (i % 8))) as u8;
    }
    for alpha in bytes.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    bytes
}

/// A gradient with `amplitude` levels of noise on it: a stand-in for a picture
/// that rate control can compress to whatever the target asks.
fn mild(seed: &mut u64, amplitude: u8) -> Vec<u8> {
    let mut bytes = noise(seed);
    let stride = WIDTH as usize * 4;
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let at = y * stride + x * 4;
            for channel in 0..3 {
                let base = ((x * (channel + 1) + y) / 8) as u8;
                bytes[at + channel] = base.wrapping_add(bytes[at + channel] % amplitude);
            }
        }
    }
    bytes
}

/// Noise in `block` by `block` pixel squares: less to encode than per-pixel
/// noise, still far from a desktop.
fn blocky(seed: &mut u64, block: usize) -> Vec<u8> {
    let mut bytes = noise(seed);
    let stride = WIDTH as usize * 4;
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let from = (y / block * block) * stride + (x / block * block) * 4;
            let to = y * stride + x * 4;
            bytes.copy_within(from..from + 4, to);
        }
    }
    bytes
}

/// Encodes one frame at `kbps` and returns what came out, in order.
fn encode(
    encoder: &mut Encoder,
    image: &GpuImage,
    idr: bool,
    kbps: u32,
) -> Result<Vec<(bool, usize)>> {
    let mut output = encoder.encode_gpu(image, idr, kbps)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while encoder.pending() {
        output.extend(encoder.poll()?);
        if Instant::now() >= deadline {
            bail!(
                "a frame was not encoded within two seconds: an encoder hang, stop and investigate"
            );
        }
    }
    Ok(output.into_iter().map(|f| (f.idr, f.bytes.len())).collect())
}

fn kbps_of(bytes: usize, frames: usize) -> f64 {
    bytes as f64 * 8. * f64::from(FPS) / frames as f64 / 1000.
}

fn upload_images(device: &Device, block: usize) -> Result<Vec<GpuImage>> {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut images = Vec::new();
    for _ in 0..IMAGES {
        let image = GpuImage::upload(
            device,
            &Image {
                width: WIDTH,
                height: HEIGHT,
                stride: WIDTH as usize * 4,
                bytes: match block {
                    0 => mild(&mut seed, 24),
                    1 => noise(&mut seed),
                    _ => blocky(&mut seed, block),
                },
                captured: Instant::now(),
                pixel: Pixel::Bgra8,
            },
        )?;
        // An uploaded texture has no fence; make sure the copy is done.
        image.readback(&mut None)?;
        images.push(image);
    }
    Ok(images)
}

#[test]
#[ignore = "requires a live AMD adapter with AMF; hold the GPU lock and keep the host idle"]
fn a_bitrate_step_never_forces_a_keyframe_and_the_rate_follows() -> Result<()> {
    let _com = ComGuard::new()?;
    let device = Device::new("")?;
    let tuning = butterpollo_core::config::Config::default();
    let mut failures = Vec::new();
    println!("codec/content  target_kbps  keyframes  last_second_kbps  error_pct");
    for (content, block) in [("noise", 1), ("mild", 0)] {
        let images = upload_images(&device, block)?;
        for (codec_name, codec) in [("h264", 0), ("hevc", 1), ("av1", 2)] {
            let name = format!("{codec_name}/{content}");
            let name = name.as_str();
            // H.264 cannot go below this on the content: its quantiser limit, not
            // a bitrate it ignores (the rate follows 30 Mbps up and 8 Mbps down).
            let floor = match (codec, content) {
                (0, "noise") => H264_NOISE_FLOOR_KBPS,
                (0, _) => H264_MILD_FLOOR_KBPS,
                _ => 0,
            };
            let stream = Negotiated {
                width: WIDTH,
                height: HEIGHT,
                fps: FPS,
                bitrate_kbps: STEPS_KBPS[0],
                codec,
                ..Default::default()
            };
            let warnings = std::sync::Arc::new(butterpollo_core::session::Warnings::default());
            let mut encoder = Encoder::new_gpu_reported(&stream, &images[0], &tuning, &warnings)?;
            let mut picture = 0usize;
            let mut next =
                |encoder: &mut Encoder, kbps: u32, idr: bool| -> Result<Vec<(bool, usize)>> {
                    picture += 1;
                    encode(encoder, &images[picture % IMAGES], idr, kbps)
                };
            // Warm up at the first rate so rate control has settled.
            for index in 0..120 {
                next(&mut encoder, STEPS_KBPS[0], index == 0)?;
            }
            // Stress: step every 30 frames, record each output frame.
            let mut sizes: Vec<(usize, bool, usize)> = Vec::new(); // (step, idr, bytes)
            let mut submitted = 0;
            while submitted < STRESS_FRAMES {
                let step = (submitted / STRESS_HOLD) % STEPS_KBPS.len();
                for (idr, bytes) in next(&mut encoder, STEPS_KBPS[step], false)? {
                    sizes.push((submitted / STRESS_HOLD, idr, bytes));
                }
                submitted += 1;
            }
            for warning in warnings.snapshot() {
                println!(
                    "{name:11}  encoder warning {}: {}",
                    warning.code, warning.message
                );
            }
            let keyframes = sizes.iter().filter(|(_, idr, _)| *idr).count();
            if keyframes > 0 {
                failures.push(format!(
                    "{name}: {keyframes} keyframes while stepping the bitrate"
                ));
            }
            // Frames to convergence per step: the first frame after which the mean
            // of the last 15 frames is within 10 % of the target.
            let mut report = Vec::new();
            for block in 0..(STRESS_FRAMES / STRESS_HOLD) {
                let target = STEPS_KBPS[block % STEPS_KBPS.len()];
                let frames: Vec<usize> = sizes
                    .iter()
                    .filter(|(b, ..)| *b == block)
                    .map(|(.., bytes)| *bytes)
                    .collect();
                let converged = (15..=frames.len()).find(|&end| {
                    let mean = kbps_of(frames[end - 15..end].iter().sum(), 15);
                    (mean - f64::from(target)).abs() <= f64::from(target) * 0.1
                });
                report.push((target, converged.map(|end| end - 15)));
            }
            println!(
                "{name:11}  stress: keyframes {keyframes}, frames to 10 % per step (of {STRESS_HOLD}): {report:?}"
            );
            // Settle: hold each level and measure the last second.
            for target in STEPS_KBPS {
                let mut held = Vec::new();
                let mut idrs = 0;
                for _ in 0..SETTLE_HOLD {
                    for (idr, bytes) in next(&mut encoder, target, false)? {
                        idrs += usize::from(idr);
                        held.push(bytes);
                    }
                }
                let tail = &held[held.len().saturating_sub(60)..];
                let mean = kbps_of(tail.iter().sum(), tail.len());
                let expected = f64::from(target.max(floor));
                let error = (mean - expected) / expected * 100.;
                println!("{name:11}  {target:>11}  {idrs:>9}  {mean:>16.0}  {error:>9.1}");
                if idrs > 0 {
                    failures.push(format!("{name}: keyframe while holding {target} kbps"));
                }
                if error.abs() > 10. {
                    failures.push(format!(
                    "{name}: {mean:.0} kbps held at {target} (expected {expected:.0}, {error:.1} %)"
                ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}

/// Where does the rate land when the encoder starts at other rates? Tells a
/// rate that is stuck from a floor the noise cannot go below.
#[test]
#[ignore = "requires a live AMD adapter with AMF; hold the GPU lock and keep the host idle"]
fn the_rate_follows_from_other_starting_rates() -> Result<()> {
    let _com = ComGuard::new()?;
    let device = Device::new("")?;
    let block = std::env::var("BLOCK")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    println!("noise in {block} pixel blocks");
    let images = upload_images(&device, block)?;
    let tuning = butterpollo_core::config::Config::default();
    println!("codec  start_kbps  then_kbps  mean_kbps");
    for (name, codec) in [("h264", 0), ("hevc", 1)] {
        for start in [15_000u32] {
            let stream = Negotiated {
                width: WIDTH,
                height: HEIGHT,
                fps: FPS,
                bitrate_kbps: start,
                codec,
                ..Default::default()
            };
            let warnings = std::sync::Arc::new(butterpollo_core::session::Warnings::default());
            let mut encoder = Encoder::new_gpu_reported(&stream, &images[0], &tuning, &warnings)?;
            let mut picture = 0usize;
            for index in 0..60 {
                picture += 1;
                encode(&mut encoder, &images[picture % IMAGES], index == 0, start)?;
            }
            for target in [start, 8_000, 4_000, 2_000, 6_000, 15_000] {
                let mut bytes = 0;
                let mut frames = 0;
                for index in 0..90 {
                    picture += 1;
                    for (_, size) in encode(&mut encoder, &images[picture % IMAGES], false, target)?
                    {
                        if index >= 30 {
                            bytes += size;
                            frames += 1;
                        }
                    }
                }
                println!(
                    "{name:11}  {start:>10}  {target:>9}  {:>9.0}",
                    kbps_of(bytes, frames.max(1))
                );
            }
            for warning in warnings.snapshot() {
                println!(
                    "{name:11}  encoder warning {}: {}",
                    warning.code, warning.message
                );
            }
        }
    }
    Ok(())
}
