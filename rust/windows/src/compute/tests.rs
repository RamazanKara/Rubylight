use super::*;
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use std::time::Instant;

#[test]
fn removed_device_fence_never_counts_as_completed_work() {
    for value in [0, 1, 100, u64::MAX - 1] {
        assert_eq!(completed_value(value).unwrap(), value);
    }
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&completed_value(u64::MAX).unwrap_err()),
        Some(crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0))
    );
}

#[test]
fn each_stream_converts_a_shared_capture_for_its_own_dynamic_range() {
    // One capture serves every stream on a virtual display, so its frames are
    // in the display's format: FP16 while Windows composes it in HDR or
    // advanced colour, BGRA otherwise. Each stream's conversion takes them
    // to its own range: an SDR stream maps FP16 back to sRGB with SDR white
    // at full scale, and an HDR stream places SDR pixels at SDR white.
    let stream = |hdr| butterpollo_core::rtsp::Negotiated {
        width: 1920,
        height: 1080,
        codec: 1,
        hdr,
        ..Default::default()
    };
    let sdr_of_fp16 = constants(&stream(false), (1920, 1080, Pixel::RgbaF16), [100., 1.]);
    assert_eq!(sdr_of_fp16[4..6], [1, 0], "FP16 source, SDR output");
    let hdr_of_bgra = constants(&stream(true), (1920, 1080, Pixel::Bgra8), [200., 1.]);
    assert_eq!(hdr_of_bgra[4..6], [0, 1], "BGRA source, HDR output");
    assert_eq!(f32::from_bits(hdr_of_bgra[9]), 200. / 80.);
}

#[test]
fn retired_conversion_settings_still_parse() {
    // The setting no longer chooses a path; reading it must not fail a stream.
    for text in [
        "",
        "gpu_compute_conversion = true\n",
        "gpu_compute_conversion = false\n",
    ] {
        warn_retired_settings(&butterpollo_core::config::Config::parse(text).unwrap());
    }
}

fn make_image(colors: &[[f32; 3]], patch: usize, height: usize) -> Image {
    let width = colors.len() * patch;
    let mut bytes = vec![0; width * height * 8];
    for (index, pixel) in bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        for (channel, value) in colors[(index % width) / patch].iter().enumerate() {
            pixel[channel * 2..channel * 2 + 2]
                .copy_from_slice(&half::f16::from_f32(*value).to_bits().to_le_bytes());
        }
        pixel[6..].copy_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
    }
    Image {
        width: width as u32,
        height: height as u32,
        stride: width * 8,
        bytes,
        pixel: Pixel::RgbaF16,
        captured: Instant::now(),
    }
}

/// A desktop BGRA gradient with distinct channels.
fn gradient(width: usize, height: usize) -> Image {
    let mut bytes = vec![0u8; width * height * 4];
    for (index, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (index % width, index / width);
        *pixel = [(x * 2) as u8, (y * 6) as u8, (255 - x * 2) as u8, 255];
    }
    Image {
        width: width as u32,
        height: height as u32,
        stride: width * 4,
        bytes,
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    }
}

/// A converted P010 picture as ten-bit codes: luma rows, then interleaved chroma rows.
fn p010_codes(compute: &Compute, converted: &Converted) -> Result<Vec<u16>> {
    let ready = Ready::on(compute, converted.fence.clone(), converted.value);
    let planes = read_planes(compute, &converted.texture, Some(&ready), &mut None)?;
    Ok(planes
        .iter()
        .flat_map(|plane| {
            plane
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| u16::from_le_bytes(*v) >> 6)
                .collect::<Vec<_>>()
        })
        .collect())
}

/// `image` converted for `config` with `luminance`, as ten-bit codes.
fn convert_p010(
    gpu: &Device,
    config: &butterpollo_core::rtsp::Negotiated,
    image: &GpuImage,
    luminance: [f32; 2],
) -> Result<Vec<u16>> {
    anyhow::ensure!(config.ten_bit(), "a ten-bit stream");
    let mut converter = Converter::new(gpu.compute.clone(), config.width, config.height, true)?;
    converter.values = constants(config, (image.width, image.height, image.pixel), luminance);
    let converted = converter.convert(image)?;
    p010_codes(&gpu.compute, &converted)
}

#[test]
#[ignore = "requires D3D12 compute"]
fn compute_sdr_ten_bit_respects_client_matrix_and_full_range() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes: [0, 0, 255, 255].repeat(64 * 64),
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    let uploaded = GpuImage::upload(&gpu, &image)?;
    for (csc_mode, expected) in [
        (0, [326, 361, 960]),
        (1, [306, 339, 1023]),
        (2, [250, 409, 960]),
        (3, [217, 395, 1023]),
    ] {
        let config = butterpollo_core::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            sdr_10bit: true,
            csc_mode,
            ..Default::default()
        };
        let output = convert_p010(&gpu, &config, &uploaded, [100., 1.])?;
        let observed = [output[0], output[4096], output[4097]];
        for (actual, wanted) in observed.into_iter().zip(expected) {
            assert!(actual.abs_diff(wanted) <= 1, "CSC {csc_mode}: {observed:?}");
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn hdr_full_and_limited_range_match_absolute_pq() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // scRGB 1.0 is 80 cd/m². These PQ values independently anchor black,
    // SDR reference white, 1000-nit highlights and the 10000-nit ceiling.
    let pq = [0., 0.48585677, 0.7518271, 1.];
    let image = make_image(&[[0.; 3], [1.; 3], [12.5; 3], [125.; 3]], 64, 64);
    let source = GpuImage::upload(&gpu, &image)?;
    for full in [false, true] {
        let config = butterpollo_core::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            codec: 2,
            hdr: true,
            csc_mode: 4 | u8::from(full),
            ..Default::default()
        };
        let pixels = convert_p010(&gpu, &config, &source, [100., 1.])?;
        let width = image.width as usize;
        for (patch, pq) in pq.into_iter().enumerate() {
            let x = patch * 64 + 32;
            let wanted = if full { 1023. * pq } else { 64. + 876. * pq };
            let luma = pixels[32 * width + x];
            assert!(
                (f64::from(luma) - wanted).abs() <= 2.,
                "full={full} patch={patch}: luma {luma}, wanted {wanted}"
            );
            for c in 0..2 {
                let chroma = pixels[width * image.height as usize + 16 * width + x + c];
                assert!(
                    chroma.abs_diff(512) <= 1,
                    "full={full} patch={patch}: neutral chroma {chroma}"
                );
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn compute_hdr_preserves_absolute_luminance_gamut_and_linear_resize() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = make_image(
        &[
            [0.; 3],
            [1.; 3],
            [12.5; 3],
            [125.; 3],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [-0.5, 1., 0.5],
        ],
        64,
        64,
    );
    let hdr = |width, height| butterpollo_core::rtsp::Negotiated {
        width,
        height,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let config = hdr(image.width, image.height);
    let uploaded = GpuImage::upload(&gpu, &image)?;
    let output = convert_p010(&gpu, &config, &uploaded, [100., 1.])?;
    let mut reference = vec![];
    crate::color::hdr_rgba(&image, &mut reference);
    let width = image.width as usize;
    for patch in 0..8 {
        let x = patch * 64 + 32;
        let offset = (32 * width + x) * 8;
        let rgb: [f32; 3] = std::array::from_fn(|c| {
            u16::from_le_bytes([reference[offset + c * 2], reference[offset + c * 2 + 1]]) as f32
                / 65535.
        });
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        let expected = [
            (64. + 876. * y).round(),
            (512. + 896. * (rgb[2] - y) / (2. * (1. - 0.0593))).round(),
            (512. + 896. * (rgb[0] - y) / (2. * (1. - 0.2627))).round(),
        ];
        let actual = [
            output[32 * width + x],
            output[width * image.height as usize + 16 * width + x],
            output[width * image.height as usize + 16 * width + x + 1],
        ];
        for c in 0..3 {
            assert!(
                (actual[c] as f32 - expected[c]).abs() <= 2.,
                "patch {patch} channel {c}: {} vs {}",
                actual[c],
                expected[c]
            );
        }
    }
    assert_eq!(output[32 * width + 32], 64);
    assert_eq!(output[32 * width + 3 * 64 + 32], 940);
    let expanded = convert_p010(&gpu, &config, &uploaded, [100., 2.])?;
    let expected = (64. + 876. * 0.82742465f32).round() as u16; // ST.2084 at 2000 nits
    assert!(expanded[32 * width + 2 * 64 + 32].abs_diff(expected) <= 2);
    let sdr = GpuImage::upload(
        &gpu,
        &Image {
            width: 64,
            height: 64,
            stride: 256,
            bytes: vec![255; 64 * 256],
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    for (white, pq) in [(100., 0.5080784f32), (200., 0.5791332f32)] {
        let output = convert_p010(&gpu, &hdr(64, 64), &sdr, [white, 1.])?;
        assert!(output[32 * 64 + 32].abs_diff((64. + 876. * pq).round() as u16) <= 2);
    }
    let resized = make_image(&[[0.; 3], [25.; 3], [0.; 3], [25.; 3]], 1, 4);
    let output = convert_p010(
        &gpu,
        &hdr(2, 2),
        &GpuImage::upload(&gpu, &resized)?,
        [100., 1.],
    )?;
    for value in &output[..4] {
        assert!(value.abs_diff(723) <= 1, "1000-nit linear average: {value}");
    }
    // A wider source is letterboxed: an 8x2 image fills one row of 4x4.
    let wide = make_image(&[[125.; 3]], 8, 2);
    let output = convert_p010(
        &gpu,
        &hdr(4, 4),
        &GpuImage::upload(&gpu, &wide)?,
        [100., 1.],
    )?;
    for (row, picture) in [(0, false), (1, true), (2, false), (3, false)] {
        for value in &output[row * 4..row * 4 + 4] {
            assert_eq!(*value > 100, picture, "row {row}: {value}");
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn compute_targets_are_bounded_and_never_overwrite_held_pictures() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let config = butterpollo_core::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let black = GpuImage::upload(&gpu, &make_image(&[[0.; 3]], 64, 64))?;
    let white = GpuImage::upload(&gpu, &make_image(&[[125.; 3]], 64, 64))?;
    let mut converter = Converter::new(gpu.compute.clone(), 64, 64, true)?;
    converter.values = constants(&config, (64, 64, Pixel::RgbaF16), [100., 1.]);
    let first = converter.convert(&black)?;
    let mut held = vec![];
    for _ in 0..7 {
        let converted = converter.convert(&white)?;
        converted.wait()?;
        held.push(converted);
    }
    assert!(
        converter.convert(&white).is_err(),
        "a ninth picture while eight are held"
    );
    assert!(
        p010_codes(&gpu.compute, &first)?[..64 * 64]
            .iter()
            .all(|y| *y == 64)
    );
    held.pop();
    let next = converter.convert(&white)?;
    assert!(
        p010_codes(&gpu.compute, &next)?[..64 * 64]
            .iter()
            .all(|y| *y == 940)
    );
    Ok(())
}

/// One plane's codes, row by row.
fn plane_codes(compute: &Compute, plane: &ID3D12Resource, ten_bit: bool) -> Result<Vec<u16>> {
    let bytes = read_planes(compute, plane, None, &mut None)?.remove(0);
    Ok(if ten_bit {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| (f32::from(u16::from_le_bytes(*v)) * 1023. / 65535.).round() as u16)
            .collect()
    } else {
        bytes.iter().map(|&v| u16::from(v)).collect()
    })
}

#[test]
#[ignore = "requires D3D12 compute"]
fn pyrowave_planes_carry_the_codes_of_the_yuv420_conversion() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = &gpu.compute;
    let colors = [
        [0., 0., 0.],
        [1., 1., 1.],
        [12.5, 12.5, 12.5],
        [1.2, 0.1, 0.05],
        [0.05, 0.8, 0.3],
        [0.2, 0.3, 4.0],
    ];
    let hdr_image = GpuImage::upload(&gpu, &make_image(&colors, 16, 40))?;
    let sdr_image = GpuImage::upload(&gpu, &gradient(96, 40))?;
    // SAFETY: `compute.device` is a live D3D12 device.
    let fence: ID3D12Fence = unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
    let mut value = 0;
    // The same size, a downscale and a letterboxed shape.
    for (width, height) in [(96, 40), (48, 20), (64, 64)] {
        for (hdr, image) in [(true, &hdr_image), (false, &sdr_image)] {
            for ten_bit in [false, true].into_iter().filter(|ten| hdr <= *ten) {
                let mut luma420 = None;
                for yuv444 in [false, true] {
                    let config = butterpollo_core::rtsp::Negotiated {
                        width,
                        height,
                        codec: 3,
                        hdr,
                        sdr_10bit: ten_bit && !hdr,
                        yuv444,
                        ..Default::default()
                    };
                    assert_eq!(config.ten_bit(), ten_bit);
                    let source = (image.width, image.height, image.pixel);
                    // Planes filled with a sentinel, so a plane the conversion skipped shows.
                    let format = if ten_bit {
                        DXGI_FORMAT_R16_UNORM
                    } else {
                        DXGI_FORMAT_R8_UNORM
                    };
                    let divisor = if yuv444 { 1 } else { 2 };
                    let planes = [1, divisor, divisor].map(|divisor| -> Result<ID3D12Resource> {
                        let (w, h) = (width / divisor, height / divisor);
                        let pitch = w as usize * if ten_bit { 2 } else { 1 };
                        let sentinel = vec![0xab_u8; pitch * h as usize];
                        Ok(upload(
                            compute,
                            (w, h),
                            format,
                            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                            &sentinel,
                            pitch,
                        )?
                        .0)
                    });
                    let [Ok(y), Ok(u), Ok(v)] = planes else {
                        bail!("plane allocation failed");
                    };
                    let mut converter = Converter::new(compute.clone(), width, height, ten_bit)?;
                    converter.values = constants(&config, source, [100., 1.]);
                    converter.values[11] = u32::from(yuv444);
                    let target = Planes {
                        planes: [y, u, v],
                        format,
                        fence: fence.clone(),
                        after: value,
                        done: value + 1,
                    };
                    value += 1;
                    converter.convert_planes(image, &target)?;
                    wait_fence(&fence, value)?;
                    let codes = target
                        .planes
                        .iter()
                        .map(|plane| plane_codes(compute, plane, ten_bit))
                        .collect::<Result<Vec<_>>>()?;
                    let sentinel = if ten_bit {
                        (f32::from(0xabab_u16) * 1023. / 65535.).round() as u16
                    } else {
                        0xab
                    };
                    let what =
                        format!("{width}x{height} hdr {hdr} ten-bit {ten_bit} 4:4:4 {yuv444}");
                    // Each plane is written everywhere: a row of the sentinel is a pixel the
                    // dispatch did not cover.
                    for (index, plane) in codes.iter().enumerate() {
                        let row = (width / if index == 0 { 1 } else { divisor }) as usize;
                        anyhow::ensure!(
                            !plane
                                .chunks(row)
                                .any(|row| row.iter().all(|c| *c == sentinel)),
                            "{what}: plane {index} has an unwritten row"
                        );
                    }
                    match &luma420 {
                        // 4:4:4 changes chroma only.
                        Some(luma) => assert_eq!(&codes[0], luma, "{what}: luma"),
                        None => {
                            // The YUV420 conversion's codes for the same stream.
                            let reference = if ten_bit {
                                convert_p010(&gpu, &config, image, [100., 1.])?
                            } else {
                                let mut nv12 =
                                    Converter::new(compute.clone(), width, height, false)?;
                                nv12.values = constants(&config, source, [100., 1.]);
                                let converted = nv12.convert(image)?;
                                let ready =
                                    Ready::on(compute, converted.fence.clone(), converted.value);
                                read_planes(compute, &converted.texture, Some(&ready), &mut None)?
                                    .concat()
                                    .into_iter()
                                    .map(u16::from)
                                    .collect()
                            };
                            let pixels = (width * height) as usize;
                            assert_eq!(&codes[0][..], &reference[..pixels], "{what}: luma");
                            let chroma = &reference[pixels..];
                            for (index, plane) in codes[1..].iter().enumerate() {
                                let interleaved: Vec<u16> =
                                    chroma.iter().skip(index).step_by(2).copied().collect();
                                for (at, (a, e)) in plane.iter().zip(&interleaved).enumerate() {
                                    assert!(
                                        a.abs_diff(*e) <= 1,
                                        "{what}: chroma {index} code {at} is {a}, YUV420 gives {e}"
                                    );
                                }
                            }
                            luma420 = Some(codes[0].clone());
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn a_conversion_holds_its_frame_until_done_and_delays_the_next_copy_into_it() -> Result<()> {
    use std::sync::atomic::Ordering;
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let pixels = Image {
        width: 4,
        height: 4,
        stride: 16,
        bytes: vec![128; 64],
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let source = GpuImage::upload(&gpu, &pixels)?;
    let mut copier = Copier::new(gpu.compute.clone())?;
    let destination = Arc::new(gpu.compute.texture(
        4,
        4,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        D3D12_RESOURCE_FLAG_NONE,
        D3D12_HEAP_FLAG_NONE,
    )?);
    let mut image = source.clone();
    image.texture = destination.clone();
    image.ready = Some(copier.copy(&destination, &source.texture, source.ready.as_ref())?);
    let mut converter = Converter::new(gpu.compute.clone(), 4, 4, false)?;
    converter.values = constants(
        &butterpollo_core::rtsp::Negotiated {
            width: 4,
            height: 4,
            ..Default::default()
        },
        (4, 4, Pixel::Bgra8),
        [100., 1.],
    );
    let before = Arc::strong_count(&image.texture);
    let converted = converter.convert(&image)?;
    assert_eq!(Arc::strong_count(&image.texture), before + 1);
    // The next copy into the texture waits for this conversion's fence value.
    assert_eq!(
        image
            .ready
            .as_ref()
            .unwrap()
            .readers
            .as_ref()
            .unwrap()
            .load(Ordering::Acquire),
        converter.slots[0].fence
    );
    converted.wait()?;
    converter.release_completed();
    assert_eq!(Arc::strong_count(&image.texture), before);
    // Copied again over the frame the conversion read, then read back.
    let again = copier.copy(&destination, &source.texture, None)?;
    image.ready = Some(again);
    assert_eq!(image.readback(&mut None)?.bytes, vec![128; 64]);
    copier.forget(&destination);
    assert!(copier.readers.is_empty());
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn uploads_read_back_every_capture_format_at_odd_sizes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let mut staging = None;
    for (pixel, size) in [(Pixel::Bgra8, 4), (Pixel::RgbaF16, 8), (Pixel::Rgba10Pq, 4)] {
        for (width, height) in [(5usize, 3usize), (67, 9), (1, 1)] {
            // Rows padded past the pixels, as a CPU image may be.
            let stride = width * size + 12;
            let bytes: Vec<u8> = (0..stride * height).map(|i| (i * 7 % 251) as u8).collect();
            let image = Image {
                width: width as u32,
                height: height as u32,
                stride,
                bytes: bytes.clone(),
                captured: Instant::now(),
                pixel,
            };
            let read = GpuImage::upload(&gpu, &image)?.readback(&mut staging)?;
            assert_eq!(
                (read.width, read.height, read.pixel),
                (image.width, image.height, pixel)
            );
            assert_eq!(read.stride, width * size);
            for y in 0..height {
                assert_eq!(
                    &read.bytes[y * read.stride..(y + 1) * read.stride],
                    &bytes[y * stride..y * stride + width * size],
                    "{pixel:?} {width}x{height} row {y}"
                );
            }
        }
    }
    Ok(())
}
