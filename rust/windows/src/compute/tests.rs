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

pub(super) fn make_image(colors: &[[f32; 3]], patch: usize, height: usize) -> Image {
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
pub(super) fn p010_codes(compute: &Compute, converted: &Converted) -> Result<Vec<u16>> {
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
pub(super) fn convert_p010(
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
pub(super) fn plane_codes(
    compute: &Compute,
    plane: &ID3D12Resource,
    ten_bit: bool,
) -> Result<Vec<u16>> {
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
    // The same size, a downscale, a letterboxed shape and an enlargement, under every client
    // matrix.
    let cases = [(96, 40), (48, 20), (64, 64), (130, 70)]
        .into_iter()
        .flat_map(|size| (0..=5).map(move |matrix| (size, matrix)));
    for ((width, height), csc_mode) in cases {
        for (hdr, image) in [(true, &hdr_image), (false, &sdr_image)] {
            for ten_bit in [false, true].into_iter().filter(|ten| hdr <= *ten) {
                let mut full: Option<Vec<Vec<u16>>> = None;
                for yuv444 in [true, false] {
                    let config = butterpollo_core::rtsp::Negotiated {
                        width,
                        height,
                        codec: 3,
                        hdr,
                        sdr_10bit: ten_bit && !hdr,
                        yuv444,
                        csc_mode,
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
                    let what = format!(
                        "{width}x{height} hdr {hdr} ten-bit {ten_bit} 4:4:4 {yuv444} matrix {csc_mode}"
                    );
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
                    let Some(full) = &full else {
                        // The 4:4:4 pass runs first and anchors the 4:2:0 checks.
                        full = Some(codes);
                        continue;
                    };
                    // 4:2:0 changes chroma only.
                    assert_eq!(&codes[0], &full[0], "{what}: luma");
                    // The YUV420 conversion's codes for the same stream.
                    let reference = if ten_bit {
                        convert_p010(&gpu, &config, image, [100., 1.])?
                    } else {
                        let mut nv12 = Converter::new(compute.clone(), width, height, false)?;
                        nv12.values = constants(&config, source, [100., 1.]);
                        let converted = nv12.convert(image)?;
                        let ready = Ready::on(compute, converted.fence.clone(), converted.value);
                        read_planes(compute, &converted.texture, Some(&ready), &mut None)?
                            .concat()
                            .into_iter()
                            .map(u16::from)
                            .collect()
                    };
                    let pixels = (width * height) as usize;
                    assert_eq!(&codes[0][..], &reference[..pixels], "{what}: luma");
                    let chroma = &reference[pixels..];
                    let (cw, ch) = ((width / 2) as usize, (height / 2) as usize);
                    for (index, plane) in codes[1..].iter().enumerate() {
                        let interleaved: Vec<u16> =
                            chroma.iter().skip(index).step_by(2).copied().collect();
                        let wide = &full[1 + index];
                        for at in 0..cw * ch {
                            let (x, y) = (at % cw, at / cw);
                            // PyroWave keeps its chroma at the centre of each 2x2 block, so it
                            // is the mean of the 4:4:4 chroma there.
                            let block = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
                                u32::from(wide[(2 * y + dy) * width as usize + 2 * x + dx])
                            });
                            let mean = (block.iter().sum::<u32>() as f32 / 4.).round() as u16;
                            assert!(
                                plane[at].abs_diff(mean) <= 2,
                                "{what}: chroma {index} code {at} is {}, the 4:4:4 block gives {mean}",
                                plane[at]
                            );
                            // The standard conversion is left-sited. Where the 4:4:4 chroma is
                            // flat across its filter, both sitings give the same code.
                            let flat = (0..2).all(|dy| {
                                let row = &wide[(2 * y + dy) * width as usize..][..width as usize];
                                let from = (2 * x).saturating_sub(2);
                                let to = (2 * x + 4).min(width as usize);
                                row[from..to]
                                    .windows(2)
                                    .all(|pair| pair[0].abs_diff(pair[1]) <= 1)
                            });
                            if flat {
                                assert!(
                                    plane[at].abs_diff(interleaved[at]) <= 1,
                                    "{what}: chroma {index} code {at} is {}, YUV420 gives {}",
                                    plane[at],
                                    interleaved[at]
                                );
                            }
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

#[test]
fn pq_input_is_rejected_only_for_sdr_output() {
    for pixel in 0..=2 {
        for hdr in 0..=1 {
            let mut values = [0; 20];
            values[4] = pixel;
            values[5] = hdr;
            assert_eq!(
                validate_colour_input(&values).is_ok(),
                pixel != 2 || hdr != 0
            );
        }
    }
}

#[test]
fn compute_texture_states_accept_only_supported_reads_or_one_write() {
    for state in [
        D3D12_RESOURCE_STATE_COMMON,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATE_COPY_SOURCE,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_COPY_SOURCE,
        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_DEST,
    ] {
        assert!(ensure_compute_texture_state(state).is_ok(), "{state:?}");
    }
    for state in [
        D3D12_RESOURCE_STATE_VIDEO_ENCODE_READ,
        D3D12_RESOURCE_STATE_VIDEO_ENCODE_WRITE,
        D3D12_RESOURCE_STATE_VIDEO_DECODE_READ,
        D3D12_RESOURCE_STATE_VIDEO_DECODE_WRITE,
        D3D12_RESOURCE_STATE_VIDEO_PROCESS_READ,
        D3D12_RESOURCE_STATE_VIDEO_PROCESS_WRITE,
        D3D12_RESOURCE_STATE_RENDER_TARGET,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATE_DEPTH_READ,
        D3D12_RESOURCE_STATE_DEPTH_WRITE,
        D3D12_RESOURCE_STATE_GENERIC_READ,
        D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_SOURCE | D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATES(i32::MIN),
    ] {
        assert!(ensure_compute_texture_state(state).is_err(), "{state:?}");
    }
}

/// Always release the task-owned queue gate, including on assertion failure.
struct Gate(ID3D12Fence);
impl Drop for Gate {
    fn drop(&mut self) {
        // SAFETY: CPU-signalling this test-owned fence releases only this test's queue. No
        // production queue waits for it.
        let _ = unsafe { self.0.Signal(1) };
    }
}

/// A 4x4 BGRA picture of one value, and the settings that convert it at its own size.
fn small_image(value: u8) -> Image {
    Image {
        width: 4,
        height: 4,
        stride: 16,
        bytes: vec![value; 64],
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    }
}
fn small_values() -> [u32; 20] {
    constants(
        &butterpollo_core::rtsp::Negotiated {
            width: 4,
            height: 4,
            ..Default::default()
        },
        (4, 4, Pixel::Bgra8),
        [100., 1.],
    )
}

#[test]
#[ignore = "requires D3D12 compute; gates only a task-owned compute queue for two seconds"]
fn timed_out_converter_retains_capture_until_its_queue_really_finishes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let luid = gpu.compute.luid();
    let compute = Compute::new(luid)?;
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.prepare_yuv()?;
    let image = GpuImage::upload(&gpu, &small_image(128))?;
    let capture = Arc::downgrade(&image.texture);
    converter.values = small_values();
    // SAFETY: This gate and queue belong exclusively to this test. Gate's destructor always
    // signals it, even if a later assertion panics.
    let gate = Gate(unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? });
    // SAFETY: The fence was created on this queue's device and stays owned.
    unsafe { compute.queue.Wait(&gate.0, 1)? };
    let converted = converter.convert(&image)?;
    drop(image);
    drop(converter);
    assert!(compute.is_stuck());
    assert!(
        capture.upgrade().is_some(),
        "timed-out GPU read lost its capture lease"
    );
    assert!(
        Compute::new(luid).is_err(),
        "retries must not grow retained GPU memory"
    );
    let tail = RETAINED
        .lock()
        .unwrap()
        .iter()
        .find(|work| !work.captures.is_empty())
        .and_then(|work| work.tail.clone())
        .context("retained converter tail missing")?;
    drop(gate);
    converted.wait()?;
    wait_fence(&tail, 1)?;
    let _replacement = Compute::new(luid)?;
    assert!(
        capture.upgrade().is_none(),
        "completed capture was never released"
    );
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute; gates only a task-owned copy queue for two seconds"]
fn timed_out_copier_keeps_its_copy_executable_after_teardown() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let luid = gpu.compute.luid();
    let compute = Compute::new(luid)?;
    let mut copier = Copier::new(compute.clone())?;
    let source = GpuImage::upload(&gpu, &small_image(85))?;
    let mut destination = GpuImage::upload(&gpu, &small_image(0))?;
    // SAFETY: Gate always signals on drop; only this test's copy queue waits for it, and every
    // texture it copies belongs to this test.
    let gate = Gate(unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? });
    // SAFETY: The gate is on the same live device as the copy queue.
    unsafe { copier.queue().Wait(&gate.0, 1)? };
    destination.ready =
        Some(copier.copy(&destination.texture, &source.texture, source.ready.as_ref())?);
    drop(source);
    drop(copier);
    assert!(compute.is_stuck());
    assert!(Compute::new(luid).is_err());
    let tail = RETAINED
        .lock()
        .unwrap()
        .iter()
        .find(|work| work.adapter == (luid.LowPart, luid.HighPart))
        .and_then(|work| work.tail.clone())
        .context("retained copy tail missing")?;
    drop(gate);
    destination.ready.as_ref().unwrap().wait()?;
    wait_fence(&tail, 1)?;
    let actual = destination.readback(&mut None)?;
    for row in actual.bytes.chunks_exact(actual.stride).take(4) {
        assert_eq!(&row[..16], &[85; 16]);
    }
    let _replacement = Compute::new(luid)?;
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute; validates distinct task-owned queues on a shared device"]
fn reader_values_belong_to_a_fence_not_the_shared_device() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let old = gpu.compute.clone();
    let new = Compute::new(old.luid())?;
    assert_eq!(
        old.device, new.device,
        "D3D12 should reuse the adapter device"
    );
    assert_ne!(old.fence, new.fence);
    let readers = Arc::new(AtomicU64::new(0));
    let ready = Ready {
        fence: old.fence.clone(),
        value: 0,
        device: old.device.as_raw() as usize,
        readers: Some(readers.clone()),
        reader_fence: Some(old.fence.clone()),
    };
    let old_value = old.signal()?;
    ready.read_by(&old, old_value)?;
    assert_eq!(readers.load(Ordering::Acquire), old_value);
    let _ = new.signal()?;
    let new_value = new.signal()?;
    ready.read_by(&new, new_value)?;
    assert!(
        new.completed(new_value),
        "cross-generation read returned before finishing"
    );
    assert_eq!(
        readers.load(Ordering::Acquire),
        old_value,
        "new fence value entered the old timeline"
    );
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute; validates bounded reuse after a delayed target fence"]
fn released_encoder_target_waits_for_its_gpu_fence() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::new(gpu.compute.luid())?;
    let mut converter = Converter::new(compute, 4, 4, false)?;
    converter.prepare_yuv()?;
    let mut busy = Vec::new();
    for _ in 0..8 {
        let index = converter.target()?;
        busy.push(converter.targets[index].texture.clone());
    }
    converter.targets[0].value = 1;
    drop(busy.remove(0));
    let fence = converter.targets[0].fence.clone();
    std::thread::scope(|scope| -> Result<()> {
        scope.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(25));
            // SAFETY: This test-owned signal models the encoder finishing after it has already
            // released its surface reference.
            unsafe { fence.Signal(1).unwrap() };
        });
        assert_eq!(converter.target()?, 0);
        Ok(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute; changes only task-owned AMF metadata"]
fn unsupported_amf_state_is_rejected_before_recording_conversion() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::new(gpu.compute.luid())?;
    let image = GpuImage::upload(&gpu, &small_image(128))?;
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.values = small_values();
    let index = converter.target()?;
    let submitted = *compute.next.lock().unwrap();
    let slot = converter.slot;
    for state in [
        D3D12_RESOURCE_STATE_VIDEO_ENCODE_READ,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_COPY_SOURCE,
    ] {
        // This changes only AMF's private-data record. The texture remains COMMON; rejection
        // must happen before recording any GPU commands.
        crate::amf::gpu::set_resource_state(&converter.targets[index].texture, state)?;
        let error = converter
            .convert(&image)
            .err()
            .expect("unsupported AMF state accepted");
        assert!(format!("{error:#}").contains("cannot transition on the compute queue"));
        assert_eq!(*compute.next.lock().unwrap(), submitted);
        assert_eq!(converter.slot, slot);
        assert_eq!(converter.targets.len(), 1);
        assert_eq!(converter.targets[index].value, 0);
        assert!(converter.slots.iter().all(|slot| slot.capture.is_none()));
        assert_eq!(
            crate::amf::gpu::resource_state(&converter.targets[index].texture)?,
            state
        );
    }
    crate::amf::gpu::set_resource_state(
        &converter.targets[index].texture,
        D3D12_RESOURCE_STATE_COMMON,
    )?;
    converter.convert(&image)?.wait()?;
    converter.release_completed();
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute; retires only a completed task-owned queue"]
fn retired_queue_rejects_conversion_upload_and_capture_before_mutating_resources() -> Result<()> {
    fn rejected<T>(result: Result<T>) {
        let error = result.err().expect("retired queue accepted new work");
        assert!(format!("{error:#}").contains("compute queue is retired"));
    }

    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::new(gpu.compute.luid())?;
    let image = GpuImage::upload(&gpu, &small_image(128))?;
    let destination = GpuImage::upload(&gpu, &small_image(128))?;
    let mut copier = Copier::new(compute.clone())?;
    let mut converter = Converter::new(compute.clone(), 4, 4, false)?;
    converter.values = small_values();
    converter.convert(&image)?.wait()?;
    converter.release_completed();

    let plane = || {
        compute.texture(
            4,
            4,
            DXGI_FORMAT_R8_UNORM,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_HEAP_FLAG_NONE,
        )
    };
    let planes = Planes {
        // Even a missing retirement guard must receive valid planes.
        planes: [plane()?, plane()?, plane()?],
        format: DXGI_FORMAT_R8_UNORM,
        // SAFETY: The test owns this unused fence on its compute device.
        fence: unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? },
        after: 0,
        done: 1,
    };
    let submitted = *compute.next.lock().unwrap();
    let slot = converter.slot;
    let targets = converter.targets.len();
    compute.retire();
    // Old fences remain usable for cleanup, but their completion cannot allow new work on the
    // quarantined queue.
    compute.wait(submitted)?;
    assert!(compute.completed(submitted));
    rejected(converter.convert(&image));
    rejected(converter.convert_planes(&image, &planes));
    rejected(compute.execute(&converter.slots[slot].list));
    rejected(upload(
        &compute,
        (4, 4),
        DXGI_FORMAT_B8G8R8A8_UNORM,
        D3D12_RESOURCE_FLAG_NONE,
        &[128; 64],
        16,
    ));
    rejected(copier.copy(&destination.texture, &image.texture, None));
    rejected(Converter::new(compute.clone(), 4, 4, false));
    rejected(Copier::new(compute.clone()));
    assert_eq!(*compute.next.lock().unwrap(), submitted);
    assert_eq!(converter.slot, slot);
    assert_eq!(converter.targets.len(), targets);
    assert!(converter.planar.is_none());
    assert!(copier.readers.is_empty());
    Ok(())
}

/// `converted` as its codes: the luma rows, then the interleaved chroma rows.
pub(super) fn yuv_codes(
    compute: &Compute,
    converted: &Converted,
    ten_bit: bool,
) -> Result<Vec<u16>> {
    if ten_bit {
        return p010_codes(compute, converted);
    }
    let ready = Ready::on(compute, converted.fence.clone(), converted.value);
    Ok(
        read_planes(compute, &converted.texture, Some(&ready), &mut None)?
            .concat()
            .into_iter()
            .map(u16::from)
            .collect(),
    )
}

/// PyroWave's planes for `config`'s shape, filled with `fill`, and the fence they signal.
pub(super) fn planes_for(
    compute: &Compute,
    config: &butterpollo_core::rtsp::Negotiated,
    yuv444: bool,
    fill: u8,
) -> Result<Planes> {
    let ten_bit = config.ten_bit();
    let format = if ten_bit {
        DXGI_FORMAT_R16_UNORM
    } else {
        DXGI_FORMAT_R8_UNORM
    };
    let divisor = if yuv444 { 1 } else { 2 };
    let plane = |divisor: u32| -> Result<ID3D12Resource> {
        let (w, h) = (config.width / divisor, config.height / divisor);
        let pitch = w as usize * if ten_bit { 2 } else { 1 };
        Ok(upload(
            compute,
            (w, h),
            format,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            &vec![fill; pitch * h as usize],
            pitch,
        )?
        .0)
    };
    Ok(Planes {
        planes: [plane(1)?, plane(divisor)?, plane(divisor)?],
        format,
        // SAFETY: `compute.device` is a live D3D12 device.
        fence: unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? },
        after: 0,
        done: 1,
    })
}

#[test]
#[ignore = "requires D3D12 compute"]
fn standard_chroma_is_left_sited_and_pyrowave_remains_center_sited() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let mut bytes = [0, 0, 0, 255].repeat(64 * 64);
    // The interior impulse has equal contributions at the chroma samples on either side. The
    // boundary impulse also checks target-edge clamping.
    for y in 0..64 {
        for x in [0, 5] {
            bytes[(y * 64 + x) * 4 + 2] = 255;
        }
    }
    let image = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes,
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    let uploaded = GpuImage::upload(&gpu, &image)?;
    let config = butterpollo_core::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 2,
        ..Default::default()
    };
    let output = convert_p010(&gpu, &config, &uploaded, [100., 1.])?;
    // BT.709 limited range, with 75% red at x=0 and 25% at x=4/6.
    for (x, expected) in [
        (0, [435, 848]),
        (2, [512, 512]),
        (4, [486, 624]),
        (6, [486, 624]),
    ] {
        let at = 4096 + x;
        for (actual, expected) in output[at..at + 2].iter().zip(expected) {
            assert!(
                actual.abs_diff(expected) <= 1,
                "standard chroma x={x}: {actual} != {expected}"
            );
        }
    }
    // PyroWave's chroma samples stay at the centre of each 2x2 block.
    let planes = planes_for(&gpu.compute, &config, false, 0)?;
    let mut converter = Converter::new(gpu.compute.clone(), 64, 64, true)?;
    converter.values = constants(&config, (64, 64, Pixel::Bgra8), [100., 1.]);
    converter.convert_planes(&uploaded, &planes)?;
    wait_fence(&planes.fence, planes.done)?;
    for (plane, expected) in [(1, [461_u16, 512, 461, 512]), (2, [736, 512, 736, 512])] {
        // PyroWave uses full-normalized 10-bit codes in R16_UNORM planes.
        let codes = plane_codes(&gpu.compute, &planes.planes[plane], true)?;
        for (actual, expected) in codes.iter().zip(expected) {
            assert!(
                actual.abs_diff(expected) <= 1,
                "PyroWave plane {plane}: {actual} != {expected}"
            );
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn sdr_bt2020_gpu_matches_reference_primaries_and_transfer() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let config = butterpollo_core::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 4,
        ..Default::default()
    };
    for (bgra, linear, expected) in [
        ([0, 0, 255, 255], [1.25, 0., 0.], [387_u16, 371, 769]),
        ([128, 128, 128, 255], [0.2698256; 3], [460, 512, 512]),
    ] {
        let image = Image {
            width: 64,
            height: 64,
            stride: 256,
            bytes: bgra.repeat(64 * 64),
            pixel: Pixel::Bgra8,
            captured: Instant::now(),
        };
        // Advanced-color Windows desktops can deliver the same SDR picture as FP16 scRGB, with
        // 100-nit SDR white represented by 1.25.
        for image in [image, make_image(&[linear], 64, 64)] {
            let uploaded = GpuImage::upload(&gpu, &image)?;
            let output = convert_p010(&gpu, &config, &uploaded, [100., 1.])?;
            for (actual, expected) in [output[0], output[4096], output[4097]]
                .into_iter()
                .zip(expected)
            {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "GPU {:?}: {actual} != {expected}",
                    image.pixel
                );
            }
        }
    }
    // Extended scRGB channels may be outside Rec.709 while the color is inside BT.2020. Mapping
    // must happen before clamping the SDR output.
    let uploaded = GpuImage::upload(&gpu, &make_image(&[[-0.125, 1.2, -0.0125]], 64, 64))?;
    let output = convert_p010(&gpu, &config, &uploaded, [100., 1.])?;
    for (actual, expected) in [output[0], output[4096], output[4097]]
        .into_iter()
        .zip([746, 256, 338])
    {
        assert!(
            actual.abs_diff(expected) <= 1,
            "extended scRGB: {actual} != {expected}"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute; sets this process's GPU scheduling class and restores it; run alone, the setting is process-wide"]
fn a_queue_made_after_realtime_is_turned_off_is_not_realtime() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let luid = gpu.compute.luid();
    /// Puts the process-wide setting back even when an assertion fails.
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = crate::gpu_priority::restore();
        }
    }
    let _restore = Restore;
    let configure = |setting: &str| {
        crate::gpu_priority::configure(&butterpollo_core::config::Config::parse(setting)?)
    };
    configure("compute_queue_realtime = true")?;
    let asked = Compute::for_adapter(luid)?;
    configure("compute_queue_realtime = false")?;
    let off = Compute::for_adapter(luid)?;
    crate::gpu_priority::restore()?;
    // A queue made while realtime was on is not handed to a stream that turned it off.
    assert!(!Arc::ptr_eq(&asked, &off));
    assert_ne!(off.priority, D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME.0);
    assert!(!realtime());
    Ok(())
}

/// The error and corruption messages the D3D12 debug layer stored for `info`'s device.
fn validation_errors(info: &ID3D12InfoQueue) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    // SAFETY: The info queue owns its messages; each aligned buffer is sized from the initial
    // query and kept alive while its message and description are read.
    unsafe {
        for index in 0..info.GetNumStoredMessagesAllowedByRetrievalFilter() {
            let mut bytes = 0usize;
            let _ = info.GetMessage(index, None, &mut bytes);
            let mut buffer = vec![0u64; bytes.div_ceil(8)];
            let message = buffer.as_mut_ptr().cast::<D3D12_MESSAGE>();
            info.GetMessage(index, Some(message), &mut bytes)?;
            let message = &*message;
            if matches!(
                message.Severity,
                D3D12_MESSAGE_SEVERITY_CORRUPTION | D3D12_MESSAGE_SEVERITY_ERROR
            ) {
                let text = String::from_utf8_lossy(std::slice::from_raw_parts(
                    message.pDescription.cast::<u8>(),
                    message.DescriptionByteLength,
                ));
                errors.push(text.trim_end_matches('\0').to_owned());
            }
        }
    }
    Ok(errors)
}

/// The two plane views of the NV12 and P010 AMF target (luma on plane 0, chroma on plane 1) are
/// valid for the D3D12 debug layer and write disjoint planes. The replacement for the D3D11
/// `gpu_420_video_plane_uavs_are_disjoint`.
#[test]
#[ignore = "requires D3D12 compute and the D3D12 debug layer; run alone with --exact, the layer must be on before the first device"]
fn amf_target_plane_views_pass_the_d3d12_debug_layer() -> Result<()> {
    let mut debug: Option<ID3D12Debug> = None;
    // SAFETY: No D3D12 device exists in this process yet (run alone); the out-value is checked
    // before it enables validation.
    unsafe {
        D3D12GetDebugInterface(&mut debug)?;
        debug
            .context("D3D12 debug layer unavailable")?
            .EnableDebugLayer();
    }
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = &gpu.compute;
    let info: ID3D12InfoQueue = compute.device.cast()?;
    let red = GpuImage::upload(
        &gpu,
        &Image {
            width: 64,
            height: 64,
            stride: 256,
            bytes: [0, 0, 255, 255].repeat(64 * 64),
            pixel: Pixel::Bgra8,
            captured: Instant::now(),
        },
    )?;
    for (ten_bit, expected) in [(false, [63, 102, 240]), (true, [250, 409, 960])] {
        let config = butterpollo_core::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            sdr_10bit: ten_bit,
            ..Default::default()
        };
        // SAFETY: The info queue belongs to the live compute device.
        unsafe { info.ClearStoredMessages() };
        let mut converter = Converter::new(compute.clone(), 64, 64, ten_bit)?;
        converter.values = constants(&config, (64, 64, Pixel::Bgra8), [100., 1.]);
        let converted = converter.convert(&red)?;
        let codes = yuv_codes(compute, &converted, ten_bit)?;
        let errors = validation_errors(&info)?;
        assert!(
            errors.is_empty(),
            "ten_bit {ten_bit}: the plane views broke D3D12 validation: {errors:?}"
        );
        // Luma, Cb and Cr each landed in their own plane: a view on the wrong plane would put
        // one of them where another belongs.
        let observed = [codes[0], codes[64 * 64], codes[64 * 64 + 1]];
        for (actual, wanted) in observed.into_iter().zip(expected) {
            assert!(
                actual.abs_diff(wanted) <= 1,
                "ten_bit {ten_bit}: {observed:?}"
            );
        }
        // Control: the reader sees an error message when the layer holds one, so the empty
        // list above means something. (A deliberately invalid view is not used: on the RX 7900 XT
        // it left the compute device unusable.)
        // SAFETY: The info queue belongs to the live compute device; the text is NUL-terminated.
        unsafe {
            info.AddApplicationMessage(
                D3D12_MESSAGE_SEVERITY_ERROR,
                windows::core::s!("control message"),
            )?;
        }
        assert_eq!(validation_errors(&info)?, ["control message"]);
        converter.release_completed();
    }
    Ok(())
}
