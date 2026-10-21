use super::{
    tests::{make_image, plane_codes, planes_for, yuv_codes},
    *,
};
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use butterpollo_core::{packed444::Geometry, rtsp::Negotiated};
use std::time::Instant;
use std::{os::windows::process::CommandExt, process::Command};

fn planar420(samples: &[u16], layout: Geometry) -> Vec<u16> {
    let pixels = layout.padded_width as usize * layout.padded_height as usize * 2;
    let mut planar = samples[..pixels].to_vec();
    planar.extend(samples[pixels..].iter().step_by(2));
    planar.extend(samples[pixels + 1..].iter().step_by(2));
    planar
}

#[test]
#[ignore = "requires D3D12 NV12 and P010 GPU conversion"]
fn packed444_gpu_exact() -> Result<()> {
    verify(
        &[(66, 70), (320, 180)],
        &[(false, false), (true, false), (true, true)],
        &[],
        60,
        100_000,
    )
}

#[test]
#[ignore = "requires AMD AMF, D3D12 and an independent FFmpeg decoder"]
fn packed444_independent_decoder() -> Result<()> {
    verify(
        &[(130, 70), (320, 180)],
        &[(false, false), (true, false), (true, true)],
        &[1, 2],
        60,
        100_000,
    )
}

#[test]
#[ignore = "requires AMD HEVC, D3D12 and an independent FFmpeg decoder"]
fn packed444_hevc_minimum_width() -> Result<()> {
    verify(&[(130, 70)], &[(false, false)], &[1], 60, 100_000)
}

#[test]
#[ignore = "compares packed layouts at 1080p60/20 Mbps using AMD AMF and an independent decoder"]
fn packed444_layered_quality() -> Result<()> {
    verify(
        &[(1920, 1080)],
        &[(false, false), (true, true)],
        &[1, 2],
        60,
        20_000,
    )
}

#[test]
#[ignore = "requires AMD AMF at 1440p120, D3D12 and an independent FFmpeg decoder"]
fn packed444_target_geometry() -> Result<()> {
    verify(
        &[(2560, 1440)],
        &[(false, false), (true, true)],
        &[1, 2],
        120,
        160_000,
    )
}

fn verify(
    sizes: &[(u32, u32)],
    formats: &[(bool, bool)],
    codecs: &[u8],
    fps: u32,
    bitrate_kbps: u32,
) -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let decoder = std::env::var_os("BUTTERPOLLO_TEST_FFMPEG");
    let directory = tempfile::tempdir()?;
    for &(width, height) in sizes {
        let layout = Geometry::new(width, height).unwrap();
        let image = Image {
            width,
            height,
            stride: width as usize * 4,
            pixel: Pixel::Bgra8,
            captured: Instant::now(),
            bytes: (0..width * height)
                .flat_map(|i| {
                    let x = i % width;
                    let y = i / width;
                    if y % 16 == 0 {
                        [255, 255, 255, 255]
                    } else if x % 2 == 0 {
                        [0, (y * 255 / height) as u8, 255, 255]
                    } else {
                        [255, (x * 255 / width) as u8, 0, 255]
                    }
                })
                .collect(),
        };
        let colors: Vec<_> = (0..width)
            .map(|x| {
                let level = 125. * x as f32 / (width - 1) as f32;
                if x % 2 == 0 {
                    [level; 3]
                } else {
                    [level, 0., level]
                }
            })
            .collect();
        let hdr_image = make_image(&colors, 1, height as usize);
        for ((ten_bit, hdr), packing) in formats
            .iter()
            .copied()
            .flat_map(|format| [1, 2].map(|packing| (format, packing)))
        {
            let image = if hdr { &hdr_image } else { &image };
            let uploaded = GpuImage::upload(&gpu, image)?;
            eprintln!("Packing v{packing} {width}x{height}, ten_bit={ten_bit}, hdr={hdr}");
            let config = Negotiated {
                width,
                height,
                codec: 1,
                sdr_10bit: ten_bit,
                hdr,
                chroma_packing: packing,
                fps,
                bitrate_kbps,
                ..Default::default()
            };
            let reference = Negotiated {
                chroma_packing: 0,
                yuv444: true,
                ..config.clone()
            };
            // PyroWave's planar conversion supplies the full-resolution 4:4:4 reference.
            let mut full = Converter::new(gpu.compute.clone(), width, height, ten_bit)?;
            full.values = constants(&reference, (width, height, image.pixel), [100., 1.]);
            // PyroWave's full-resolution chroma.
            full.values[11] = 1;
            let full_planes = planes_for(&gpu.compute, &reference, true, 0)?;
            full.convert_planes(&uploaded, &full_planes)?;
            wait_fence(&full_planes.fence, full_planes.done)?;
            let planes: Vec<Vec<u16>> = full_planes
                .planes
                .iter()
                .map(|plane| plane_codes(&gpu.compute, plane, ten_bit))
                .collect::<Result<_>>()?;
            let mut expected = vec![0; layout.samples()];
            let source = [&planes[0][..], &planes[1][..], &planes[2][..]];
            let maximum = if ten_bit { 1023 } else { 255 };
            assert!(if packing == 2 {
                layout.pack_layered(source, &mut expected, maximum)
            } else {
                layout.pack(source, &mut expected)
            });
            let compute = &gpu.compute;
            let (cw, ch) = layout.coded_size();
            let mut converter = Converter::new(compute.clone(), cw, ch, ten_bit)?;
            converter.pack_chroma()?;
            converter.values = constants(&config, (width, height, image.pixel), [100., 1.]);
            let converted = converter.convert(&uploaded)?;
            let actual = planar420(&yuv_codes(compute, &converted, ten_bit)?, layout);
            if let Some(at) =
                (0..expected.len().max(actual.len())).find(|&at| actual.get(at) != expected.get(at))
            {
                panic!(
                    "D3D12 packing v{packing} {width}x{height}, ten_bit={ten_bit}: sample {at} is {:?}, expected {:?} ({} versus {} samples)",
                    actual.get(at),
                    expected.get(at),
                    actual.len(),
                    expected.len()
                );
            }
            if codecs.is_empty() {
                continue;
            }
            let decoder = decoder.as_ref().context("set BUTTERPOLLO_TEST_FFMPEG")?;
            for &codec in codecs {
                eprintln!("Encoding codec={codec}, ten_bit={ten_bit}");
                let config = Negotiated {
                    codec,
                    ..config.clone()
                };
                let mut encoder = crate::amf::Encoder::new_device(&config, gpu.clone())?;
                let mut packets = Vec::new();
                for i in 0..8 {
                    eprintln!("Submitting frame {i}");
                    packets.extend(encoder.encode_gpu(&uploaded, i == 0, config.bitrate_kbps)?);
                    let deadline = Instant::now() + std::time::Duration::from_secs(3);
                    while encoder.pending() {
                        packets.extend(encoder.poll()?);
                        anyhow::ensure!(
                            Instant::now() < deadline,
                            "packed encoder failed to drain"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    anyhow::ensure!(
                        packets.len() == i + 1,
                        "packed encoder failed to return frame {}",
                        i + 1
                    );
                    eprintln!("Drained frame {i}, packets={}", packets.len());
                }
                assert_eq!(packets.len(), 8);
                eprintln!(
                    "packing={packing}, codec={codec}, ten_bit={ten_bit}: {} encoded bytes",
                    packets.iter().map(|p| p.bytes.len()).sum::<usize>()
                );
                eprintln!("Decoding {} packets independently", packets.len());
                let path = directory.path().join(if codec == 1 {
                    "packed.hevc"
                } else {
                    "packed.obu"
                });
                std::fs::write(
                    &path,
                    packets
                        .iter()
                        .flat_map(|p| p.bytes.iter().copied())
                        .collect::<Vec<_>>(),
                )?;
                let output = Command::new(decoder)
                    .args([
                        "-v",
                        "error",
                        "-f",
                        if codec == 1 { "hevc" } else { "obu" },
                        "-i",
                    ])
                    .arg(&path)
                    .args([
                        "-frames:v",
                        "8",
                        "-vf",
                        "scale=in_range=full:out_range=full",
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        if ten_bit { "yuv420p10le" } else { "yuv420p" },
                        "pipe:1",
                    ])
                    .creation_flags(0x08000000)
                    .output()?;
                anyhow::ensure!(
                    output.status.success() && output.stderr.is_empty(),
                    "independent decode: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let decoded: Vec<u16> = if ten_bit {
                    output
                        .stdout
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| u16::from_le_bytes([b[0], b[1]]))
                        .collect()
                } else {
                    output.stdout.iter().map(|&b| u16::from(b)).collect()
                };
                assert_eq!(
                    decoded.len(),
                    layout.samples() * packets.len(),
                    "independent decoder lost frames or changed geometry"
                );
                let reference: Vec<_> = planes.iter().flatten().copied().collect();
                let mut restored = vec![0; width as usize * height as usize * 3];
                for (frame, decoded) in decoded.chunks_exact(layout.samples()).enumerate() {
                    assert!(if packing == 2 {
                        layout.unpack_layered(decoded, &mut restored, maximum)
                    } else {
                        layout.unpack(decoded, &mut restored)
                    });
                    let error: f64 = restored
                        .iter()
                        .zip(&reference)
                        .map(|(&a, &b)| f64::from(a.abs_diff(b)))
                        .sum::<f64>()
                        / restored.len() as f64;
                    assert!(
                        error < if ten_bit { 32. } else { 8. },
                        "codec={codec}, ten_bit={ten_bit}, frame={frame}, mean code error={error}"
                    );
                    eprintln!(
                        "codec={codec}, ten_bit={ten_bit}, frame={frame}: mean code error={error}"
                    );
                }
            }
        }
    }
    Ok(())
}
