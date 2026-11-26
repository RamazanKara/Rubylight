//! HDR desktop to SDR stream (BT.2446-1 Method B) GPU tests.

use super::*;
use crate::color::{METHOD_B_PATCHES, method_b, method_b_peak, pq_scrgb, scrgb_pq10};

/// [`make_image`] as an HDR10 desktop hands it over: 10-bit PQ BT.2020.
fn make_pq_image(colors: &[[f32; 3]], patch: usize, height: usize) -> Image {
    let width = colors.len() * patch;
    let mut bytes = Vec::with_capacity(width * height * 4);
    for index in 0..width * height {
        let [r, g, b] = scrgb_pq10(colors[(index % width) / patch]).map(u32::from);
        bytes.extend_from_slice(&(r | (g << 10) | (b << 20) | (3 << 30)).to_le_bytes());
    }
    Image {
        width: width as u32,
        height: height as u32,
        stride: width * 4,
        bytes,
        pixel: Pixel::Rgba10Pq,
        captured: Instant::now(),
    }
}

/// The luma and chroma codes of `rgb`, an SDR signal, at each patch centre.
fn codes(config: &butterpollo_media_policy::rtsp::Negotiated, rgb: [f64; 3]) -> [f64; 3] {
    let matrix = usize::from(config.color_matrix());
    let (kr, kb) = [(0.299, 0.114), (0.2126, 0.0722), (0.2627, 0.0593)][matrix];
    let y = kr * rgb[0] + (1. - kr - kb) * rgb[1] + kb * rgb[2];
    let scale = if config.ten_bit() { 4. } else { 1. };
    [
        16. + 219. * y,
        128. + 224. * (rgb[2] - y) / (2. * (1. - kb)),
        128. + 224. * (rgb[0] - y) / (2. * (1. - kr)),
    ]
    .map(|v| (v * scale).round())
}

fn picture(output: &[u16], width: usize, height: usize) -> Vec<[f64; 3]> {
    (0..width / 64)
        .map(|patch| {
            let x = patch * 64 + 32;
            let chroma = width * height + height / 4 * width + x;
            [
                output[height / 2 * width + x],
                output[chroma],
                output[chroma + 1],
            ]
            .map(f64::from)
        })
        .collect()
}

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views and D3D12 compute"]
fn gpu_sdr_tone_map_matches_the_cpu_reference_in_every_converter() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let colours: Vec<[f32; 3]> = METHOD_B_PATCHES.iter().map(|(rgb, _)| *rgb).collect();
    let image = make_image(&colours, 64, 64);
    let (width, height) = (image.width as usize, image.height as usize);
    let source = (image.width, image.height, image.pixel);
    let mut uploaded = GpuImage::upload_settled(&gpu, &image)?;
    // Whatever this PC's display runs, the picture is from an HDR desktop.
    uploaded.gpu.forget_color();
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    // csc_mode >> 1: 0 BT.601, 1 BT.709, 2 BT.2020.
    for (csc_mode, ten_bit) in [(0, false), (2, false), (4, false), (2, true), (4, true)] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            codec: 1,
            sdr_10bit: ten_bit,
            csc_mode,
            sdr_tone_map: true,
            ..Default::default()
        };
        let mut paths = vec![];
        for white in [80., 400.] {
            if ten_bit {
                for pixel_passes in [true, false] {
                    let mut graphics = Converter::new(&gpu, &config, source)?;
                    if pixel_passes {
                        graphics.compute = None;
                    }
                    graphics.set_luminance([white, 1.]);
                    let output = readback(&gpu, graphics.convert(&uploaded)?.as_ref())?;
                    paths.push(("D3D11", output));
                }
            }
            let mut converter =
                crate::compute::Converter::new(queue.clone(), image.width, image.height, ten_bit)?;
            converter.values = constants(&config, source, [white, 1.], None);
            let opened = queue.open(&uploaded.texture)?;
            let converted = converter.convert(
                &opened,
                crate::compute::format(image.pixel),
                None,
                &uploaded,
            )?;
            converted.wait()?;
            paths.push(("D3D12", readback_compute(&queue, &converted.texture)?));
        }
        // The SDR white level changes nothing.
        let half = paths.len() / 2;
        for (low, high) in paths[..half].iter().zip(&paths[half..]) {
            assert!(low.1 == high.1, "{} csc={csc_mode} ten={ten_bit}", low.0);
        }
        for (path, output) in &paths {
            for (patch, actual) in picture(output, width, height).into_iter().enumerate() {
                let light = colours[patch].map(f64::from);
                let wanted = codes(&config, method_b(light, config.color_matrix()));
                for (actual, wanted) in actual.into_iter().zip(wanted) {
                    assert!(
                        (actual - wanted).abs() <= if ten_bit { 4. } else { 1. },
                        "{path} csc={csc_mode} ten={ten_bit} patch {patch}: {actual}, wanted {wanted}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views"]
fn gpu_sdr_clips_at_sdr_white_when_off_or_on_an_sdr_desktop() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // 80 and 400 nits against an SDR white of 400.
    let image = make_image(&[[1.; 3], [5.; 3]], 64, 64);
    let source = (image.width, image.height, image.pixel);
    let mut uploaded = GpuImage::upload_settled(&gpu, &image)?;
    uploaded.gpu.forget_color();
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: image.width,
        height: image.height,
        codec: 1,
        sdr_10bit: true,
        sdr_tone_map: false,
        ..Default::default()
    };
    let mut graphics = Converter::new(&gpu, &config, source)?;
    graphics.set_luminance([400., 1.]);
    let output = readback(&gpu, graphics.convert(&uploaded)?.as_ref())?;
    // sRGB of 0.2 and of 1.0 in ten-bit limited range.
    let row = 32 * image.width as usize;
    assert!(output[row + 32].abs_diff(488) <= 1, "{}", output[row + 32]);
    assert!(output[row + 96].abs_diff(940) <= 1, "{}", output[row + 96]);
    // An SDR desktop keeps those codes with the tone mapping on. The compute
    // converter takes `sdrDesktop` as given, whatever this PC's display runs.
    let on = butterpollo_media_policy::rtsp::Negotiated {
        sdr_tone_map: true,
        ..config
    };
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    let mut converter =
        crate::compute::Converter::new(queue.clone(), image.width, image.height, true)?;
    converter.values = constants(&on, source, [400., 1.], None);
    assert_eq!(converter.values[17..19], [1, 0]);
    converter.values[18] = 1;
    let opened = queue.open(&uploaded.texture)?;
    let converted = converter.convert(
        &opened,
        crate::compute::format(image.pixel),
        None,
        &uploaded,
    )?;
    converted.wait()?;
    let output = readback_compute(&queue, &converted.texture)?;
    assert!(output[row + 32].abs_diff(488) <= 1, "{}", output[row + 32]);
    assert!(output[row + 96].abs_diff(940) <= 1, "{}", output[row + 96]);
    Ok(())
}

#[test]
fn only_an_fp16_capture_of_a_display_known_to_be_sdr_is_an_sdr_desktop() {
    assert_eq!(sdr_desktop(Pixel::RgbaF16, Some(false)), 1);
    for (pixel, in_hdr) in [
        (Pixel::RgbaF16, Some(true)),
        (Pixel::RgbaF16, None),
        (Pixel::Bgra8, Some(false)),
        (Pixel::Rgba10Pq, Some(false)),
    ] {
        assert_eq!(sdr_desktop(pixel, in_hdr), 0, "{pixel:?} {in_hdr:?}");
    }
}

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views and D3D12 compute"]
fn gpu_sdr_tone_map_maps_an_hdr10_capture_like_the_cpu_reference() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let colours: Vec<[f32; 3]> = METHOD_B_PATCHES.iter().map(|(rgb, _)| *rgb).collect();
    let image = make_pq_image(&colours, 64, 64);
    let (width, height) = (image.width as usize, image.height as usize);
    let source = (image.width, image.height, image.pixel);
    let mut uploaded = GpuImage::upload_settled(&gpu, &image)?;
    uploaded.gpu.forget_color();
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    for (csc_mode, ten_bit) in [(2, false), (0, true), (2, true), (4, true)] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            codec: 1,
            sdr_10bit: ten_bit,
            csc_mode,
            sdr_tone_map: true,
            ..Default::default()
        };
        let mut paths = vec![];
        if ten_bit {
            for pixel_passes in [true, false] {
                let mut graphics = Converter::new(&gpu, &config, source)?;
                if pixel_passes {
                    graphics.compute = None;
                }
                graphics.set_luminance([240., 1.]);
                let output = readback(&gpu, graphics.convert(&uploaded)?.as_ref())?;
                paths.push(("D3D11", output));
            }
        }
        let mut converter =
            crate::compute::Converter::new(queue.clone(), image.width, image.height, ten_bit)?;
        converter.values = constants(&config, source, [240., 1.], None);
        let opened = queue.open(&uploaded.texture)?;
        let converted = converter.convert(
            &opened,
            crate::compute::format(image.pixel),
            None,
            &uploaded,
        )?;
        converted.wait()?;
        paths.push(("D3D12", readback_compute(&queue, &converted.texture)?));
        for (path, output) in &paths {
            for (patch, actual) in picture(output, width, height).into_iter().enumerate() {
                let light = pq_scrgb(scrgb_pq10(colours[patch]));
                let wanted = codes(&config, method_b(light, config.color_matrix()));
                for (actual, wanted) in actual.into_iter().zip(wanted) {
                    assert!(
                        (actual - wanted).abs() <= if ten_bit { 4. } else { 1. },
                        "{path} csc={csc_mode} ten={ten_bit} patch {patch}: {actual}, wanted {wanted}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn the_tone_mapping_follows_the_displays_reported_peak() {
    let peak = |nits| f32::from_bits(tone_map_peak(nits)).exp2();
    for (reported, wanted) in [
        (0, 1000.),
        (270, 400.),
        (600, 600.),
        (1499, 1499.),
        (10000, 2000.),
    ] {
        assert!((peak(reported) - wanted).abs() < 0.01, "{reported}");
    }
    let config = butterpollo_media_policy::rtsp::Negotiated {
        sdr_tone_map: true,
        ..Default::default()
    };
    // Whatever the display, the words start at Method B's 1000 nits.
    let values = constants(&config, (64, 64, Pixel::Rgba10Pq), [240., 1.], None);
    assert_eq!(values[17..], [1, 0, tone_map_peak(1000)]);
}

#[test]
#[ignore = "requires a D3D12 compute GPU"]
fn gpu_sdr_tone_map_follows_the_source_peak() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let colours: Vec<[f32; 3]> = METHOD_B_PATCHES.iter().map(|(rgb, _)| *rgb).collect();
    let image = make_image(&colours, 64, 64);
    let (width, height) = (image.width as usize, image.height as usize);
    let source = (image.width, image.height, image.pixel);
    let uploaded = GpuImage::upload_settled(&gpu, &image)?;
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: image.width,
        height: image.height,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 2,
        sdr_tone_map: true,
        ..Default::default()
    };
    for peak in [400u16, 1500, 2000] {
        let mut converter =
            crate::compute::Converter::new(queue.clone(), image.width, image.height, true)?;
        converter.values = constants(&config, source, [240., 1.], None);
        converter.values[19] = tone_map_peak(peak);
        let opened = queue.open(&uploaded.texture)?;
        let converted = converter.convert(
            &opened,
            crate::compute::format(image.pixel),
            None,
            &uploaded,
        )?;
        converted.wait()?;
        let output = readback_compute(&queue, &converted.texture)?;
        for (patch, actual) in picture(&output, width, height).into_iter().enumerate() {
            let light = colours[patch].map(f64::from);
            let rgb = method_b_peak(light, config.color_matrix(), f64::from(peak));
            for (actual, wanted) in actual.into_iter().zip(codes(&config, rgb)) {
                assert!(
                    (actual - wanted).abs() <= 4.,
                    "peak {peak} patch {patch}: {actual}, wanted {wanted}"
                );
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires a D3D11 GPU"]
fn gpu_hdr10_capture_is_refused_for_sdr_without_tone_mapping() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let source = (64, 64, Pixel::Rgba10Pq);
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        sdr_10bit: true,
        ..Default::default()
    };
    let refused = Converter::new(&gpu, &config, source)
        .err()
        .context("PQ accepted")?;
    assert!(format!("{refused:#}").contains("hdr_sdr_tone_map is off"));
    let on = butterpollo_media_policy::rtsp::Negotiated {
        sdr_tone_map: true,
        ..config
    };
    Converter::new(&gpu, &on, source)?;
    Ok(())
}

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views and D3D12 compute"]
fn gpu_sdr_tone_map_scales_in_linear_light_before_mapping() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // Alternating one-pixel columns, halved: each output pixel is their mean
    // light. Averaging after the curve would give other codes.
    let pairs: [[[f32; 3]; 2]; 3] = [
        [[0.; 3], [5.; 3]],
        [[0.25; 3], [12.5; 3]],
        [[4., 0., 0.], [0., 0., 4.]],
    ];
    let columns: Vec<[f32; 3]> = pairs
        .iter()
        .flat_map(|pair| (0..128).map(|x| pair[x % 2]))
        .collect();
    let image = make_image(&columns, 1, 128);
    let source = (image.width, image.height, image.pixel);
    let mut uploaded = GpuImage::upload_settled(&gpu, &image)?;
    uploaded.gpu.forget_color();
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: image.width / 2,
        height: image.height / 2,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 2,
        sdr_tone_map: true,
        ..Default::default()
    };
    let (width, height) = (config.width as usize, config.height as usize);
    let mut paths = vec![];
    for pixel_passes in [true, false] {
        let mut graphics = Converter::new(&gpu, &config, source)?;
        if pixel_passes {
            graphics.compute = None;
        }
        let output = readback(&gpu, graphics.convert(&uploaded)?.as_ref())?;
        paths.push(("D3D11", output));
    }
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    let mut converter =
        crate::compute::Converter::new(queue.clone(), config.width, config.height, true)?;
    converter.values = constants(&config, source, [400., 1.], None);
    let opened = queue.open(&uploaded.texture)?;
    let converted = converter.convert(
        &opened,
        crate::compute::format(image.pixel),
        None,
        &uploaded,
    )?;
    converted.wait()?;
    paths.push(("D3D12", readback_compute(&queue, &converted.texture)?));
    for (path, output) in &paths {
        for (patch, actual) in picture(output, width, height).into_iter().enumerate() {
            let [a, b] = pairs[patch].map(|rgb| rgb.map(f64::from));
            let mean = std::array::from_fn(|i| (a[i] + b[i]) / 2.);
            let wanted = codes(&config, method_b(mean, config.color_matrix()));
            for (actual, wanted) in actual.into_iter().zip(wanted) {
                assert!(
                    (actual - wanted).abs() <= 4.,
                    "{path} patch {patch}: {actual}, wanted {wanted}"
                );
            }
        }
    }
    Ok(())
}
