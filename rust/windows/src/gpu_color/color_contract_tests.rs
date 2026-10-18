use super::*;

#[test]
#[ignore = "requires D3D11 video views/UAVs and D3D12 compute"]
fn standard_chroma_is_left_sited_and_pyrowave_remains_center_sited() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let mut bytes = [0, 0, 0, 255].repeat(64 * 64);
    // The interior impulse has equal contributions at the chroma samples
    // on either side. The boundary impulse also checks target-edge clamping.
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
    let mut cpu =
        crate::encoder::Convert::new_config(&config, crate::ff::AVPixelFormat_AV_PIX_FMT_P010LE)?;
    cpu.convert(&image)?;
    // SAFETY: The live converter owns an initialized 64-word UV row.
    unsafe {
        let uv = (*cpu.frame).data[1].cast::<u16>();
        let left = [*uv.add(4) >> 6, *uv.add(5) >> 6];
        let right = [*uv.add(6) >> 6, *uv.add(7) >> 6];
        assert!(
            left[0] < 512 && left[1] > 512,
            "CPU lost chroma impulse: {left:?}"
        );
        for (a, b) in left.into_iter().zip(right) {
            assert!(
                a.abs_diff(b) <= 1,
                "CPU chroma impulse is not centered between left-sited samples: {left:?}, {right:?}"
            );
        }
        assert_eq!(
            (*cpu.frame).chroma_location,
            crate::ff::AVChromaLocation_AVCHROMA_LOC_LEFT
        );
    }
    let check_standard = |output: &[u16]| {
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
    };
    for compute in [false, true] {
        let mut converter = Converter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        if !compute {
            converter.compute = None;
        }
        check_standard(&readback(&gpu, converter.convert(&uploaded)?.as_ref())?);
        let mut planar = PlanarConverter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        if !compute {
            planar.compute = None;
        }
        planar.convert(&uploaded, [100., 1.])?;
        for (plane, expected) in [(1, [461_u16, 512, 461, 512]), (2, [736, 512, 736, 512])] {
            let bytes = plane_bytes(&gpu, &planar.textures[plane])?;
            // PyroWave uses full-normalized 10-bit codes in R16_UNORM planes.
            for (word, expected) in bytes[..8].as_chunks::<2>().0.iter().zip(expected) {
                let actual = (f64::from(u16::from_le_bytes(*word)) * 1023. / 65535.).round() as u16;
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "PyroWave plane {plane}: {actual} != {expected}"
                );
            }
        }
    }
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let mut converter = crate::compute::Converter::new(compute.clone(), 64, 64, true)?;
    converter.values = constants(&config, (64, 64, Pixel::Bgra8), [100., 1.], None);
    let converted = converter.convert(
        &compute.open(&uploaded.texture)?,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        None,
        &uploaded,
    )?;
    converted.wait()?;
    check_standard(&readback_compute(&compute, &converted.texture)?);
    Ok(())
}

#[test]
#[ignore = "requires D3D11 P010 conversion and the packaged FFmpeg libraries"]
fn sdr_bt2020_cpu_and_gpu_match_reference_primaries_and_transfer() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
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
        let config = butterpollo_core::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            sdr_10bit: true,
            csc_mode: 4,
            ..Default::default()
        };
        let mut cpu = crate::encoder::Convert::new_config(
            &config,
            crate::ff::AVPixelFormat_AV_PIX_FMT_P010LE,
        )?;
        cpu.convert(&image)?;
        // SAFETY: The live converter owns initialized 64x64 P010 planes.
        let cpu_codes = unsafe {
            let y = (*cpu.frame).data[0].cast::<u16>();
            let uv = (*cpu.frame).data[1].cast::<u16>();
            [*y >> 6, *uv >> 6, *uv.add(1) >> 6]
        };
        for (actual, expected) in cpu_codes.into_iter().zip(expected) {
            assert!(
                actual.abs_diff(expected) <= 2,
                "CPU: {actual} != {expected}"
            );
        }
        // Advanced-color Windows desktops can deliver the same SDR picture
        // as FP16 scRGB, with 100-nit SDR white represented by 1.25.
        for image in [image, make_image(&[linear], 64, 64)] {
            let uploaded = GpuImage::upload(&gpu, &image)?;
            for compute in [false, true] {
                let mut converter = Converter::new(&gpu, &config, (64, 64, image.pixel))?;
                if !compute {
                    converter.compute = None;
                }
                let output = readback(&gpu, converter.convert(&uploaded)?.as_ref())?;
                for (actual, expected) in [output[0], output[4096], output[4097]]
                    .into_iter()
                    .zip(expected)
                {
                    assert!(
                        actual.abs_diff(expected) <= 1,
                        "GPU: {actual} != {expected}"
                    );
                }
            }
        }
    }
    // Extended scRGB channels may be outside Rec.709 while the color is
    // inside BT.2020. Mapping must happen before clamping the SDR output.
    let image = make_image(&[[-0.125, 1.2, -0.0125]], 64, 64);
    let uploaded = GpuImage::upload(&gpu, &image)?;
    let config = butterpollo_core::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 4,
        ..Default::default()
    };
    for compute in [false, true] {
        let mut converter = Converter::new(&gpu, &config, (64, 64, Pixel::RgbaF16))?;
        if !compute {
            converter.compute = None;
        }
        let output = readback(&gpu, converter.convert(&uploaded)?.as_ref())?;
        for (actual, expected) in [output[0], output[4096], output[4097]]
            .into_iter()
            .zip([746, 256, 338])
        {
            assert!(
                actual.abs_diff(expected) <= 1,
                "extended scRGB: {actual} != {expected}"
            );
        }
    }
    Ok(())
}
