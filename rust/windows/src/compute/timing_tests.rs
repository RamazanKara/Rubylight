//! GPU timestamp timing of the D3D12 conversion, for comparison with the D3D11 fixtures it replaced.
//! Each conversion's command list writes the GPU clock at its start and end (`Converter::stamps`),
//! so a sample is the list's own run time on the compute queue: no submission gaps, no capture,
//! encoder, network or decoder.
use super::{downscale_tests::pattern, tests::make_image, *};
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use std::time::Instant;
use windows::{
    Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile},
    core::PCSTR,
};

fn stats(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let last = values.len() - 1;
    serde_json::json!({
        "samples": values.len(),
        "mean_ms": values.iter().sum::<f64>() / values.len() as f64,
        "p50_ms": values[last / 2],
        "p95_ms": values[last * 95 / 100],
        "p99_ms": values[last * 99 / 100],
        "max_ms": values[last],
    })
}

/// The GPU run time of `samples` conversions of `source`, after 32 warm-up conversions, and the
/// CPU time of each `convert` call.
fn time_conversion(
    compute: &Compute,
    converter: &mut Converter,
    source: &GpuImage,
    samples: u32,
) -> Result<serde_json::Value> {
    for _ in 0..32 {
        converter.convert(source)?;
    }
    compute.wait(compute.signal()?)?;
    // SAFETY: The descriptor and out-slot are live locals on the converter's device.
    let heap: ID3D12QueryHeap = unsafe {
        let mut heap = None;
        compute.device.CreateQueryHeap(
            &D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                Count: samples * 2,
                NodeMask: 0,
            },
            &mut heap,
        )?;
        heap.context("no timestamp query heap")?
    };
    converter.stamps = Some((heap.clone(), std::cell::Cell::new(0)));
    let mut calls = Vec::new();
    let mut last = None;
    for _ in 0..samples {
        let begin = Instant::now();
        let converted = converter.convert(source);
        calls.push(begin.elapsed().as_secs_f64() * 1000.);
        last = Some(converted?);
    }
    converter.stamps = None;
    last.context("no samples")?.wait()?;
    let readback = buffer(
        &compute.device,
        D3D12_HEAP_TYPE_READBACK,
        u64::from(samples) * 16,
        D3D12_RESOURCE_STATE_COPY_DEST,
    )?;
    // SAFETY: The list, heap and readback buffer are on one device, the list is closed before it
    // runs, and the buffer is mapped only after the compute fence passes its resolve.
    let ticks = unsafe {
        let allocator: ID3D12CommandAllocator = compute
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList = compute.device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE,
            &allocator,
            None,
        )?;
        list.ResolveQueryData(
            &heap,
            D3D12_QUERY_TYPE_TIMESTAMP,
            0,
            samples * 2,
            &readback,
            0,
        );
        list.Close()?;
        compute.wait(compute.execute(&list)?)?;
        let mut mapped = std::ptr::null_mut();
        readback.Map(0, None, Some(&mut mapped))?;
        let ticks = std::slice::from_raw_parts(mapped.cast::<u64>(), samples as usize * 2).to_vec();
        readback.Unmap(0, None);
        ticks
    };
    // SAFETY: The queue is live.
    let frequency = unsafe { compute.queue.GetTimestampFrequency()? } as f64;
    let mut gpu = Vec::new();
    for pair in ticks.chunks(2) {
        anyhow::ensure!(pair[1] >= pair[0], "GPU clock ran backwards");
        gpu.push((pair[1] - pair[0]) as f64 * 1000. / frequency);
    }
    Ok(serde_json::json!({"gpu": stats(gpu), "cpu_call": stats(calls)}))
}

fn write_report(variable: &str, report: &serde_json::Value) -> Result<()> {
    eprintln!("{report}");
    if let Some(path) = std::env::var_os(variable) {
        std::fs::write(path, serde_json::to_vec_pretty(report)?)?;
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute with native P010 views and GPU timestamp queries"]
fn gpu_conversion_timing() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = &gpu.compute;
    let image = make_image(&[[0.; 3], [1.; 3], [12.5; 3], [125.; 3]], 492, 2184);
    let source = GpuImage::upload(&gpu, &image)?;
    let mut reports = vec![];
    for (width, height) in [(1968, 2184), (3840, 2160)] {
        let config = butterpollo_core::rtsp::Negotiated {
            width,
            height,
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        let preparation = Instant::now();
        let mut converter = Converter::new(compute.clone(), width, height, config.ten_bit())?;
        converter.values = constants(
            &config,
            (image.width, image.height, image.pixel),
            [100., 1.],
        );
        converter.convert(&source)?.wait()?;
        let preparation_ms = preparation.elapsed().as_secs_f64() * 1000.;
        let timing = time_conversion(compute, &mut converter, &source, 64)?;
        reports.push(serde_json::json!({"width":width,"height":height,"preparation_ms":preparation_ms,"gpu":timing["gpu"],"cpu_call":timing["cpu_call"]}));
    }
    write_report(
        "BUTTERPOLLO_TEST_GPU_REPORT",
        &serde_json::json!({"scope":"isolated FP16-to-P010 conversion on a D3D12 compute queue, timestamps inside the conversion's command list; excludes capture, encoder, network and decoder","source_width":image.width,"source_height":image.height,"adapter":gpu.display.adapter,"cases":reports}),
    )
}

/// The common desktop sizes, SDR (BGRA to NV12) and HDR (FP16 to P010); the single compute pass
/// is the only conversion since the pixel-shader passes went away.
#[test]
#[ignore = "requires D3D12 compute with native video plane views and an idle GPU"]
fn gpu_420_single_pass_timing() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = &gpu.compute;
    let image = make_image(&[[0.; 3], [1.; 3], [12.5; 3], [125.; 3]], 960, 2160);
    let hdr_source = GpuImage::upload(&gpu, &image)?;
    let mut sdr = Image {
        width: image.width,
        height: image.height,
        stride: image.width as usize * 4,
        bytes: vec![0; image.width as usize * image.height as usize * 4],
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    for (index, pixel) in sdr.bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        *pixel = [
            (index % 256) as u8,
            ((index / image.width as usize) % 256) as u8,
            (255 - index % 256) as u8,
            255,
        ];
    }
    let sdr_source = GpuImage::upload(&gpu, &sdr)?;
    let mut reports = Vec::new();
    for (width, height) in [(1920, 1080), (2560, 1440), (3840, 2160)] {
        for hdr in [false, true] {
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                codec: 1,
                hdr,
                ..Default::default()
            };
            let source = if hdr { &hdr_source } else { &sdr_source };
            let mut converter = Converter::new(compute.clone(), width, height, config.ten_bit())?;
            converter.values = constants(
                &config,
                (source.width, source.height, source.pixel),
                [100., 1.],
            );
            let mut runs = Vec::new();
            for _ in 0..4 {
                runs.push(serde_json::json!({"single_pass":true,"stats":time_conversion(compute, &mut converter, source, 128)?}));
            }
            reports.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"planar":false,"runs":runs}));
        }
    }
    write_report(
        "BUTTERPOLLO_TEST_GPU_REPORT",
        &serde_json::json!({"scope":"D3D12 compute conversion; four runs per case, 128 GPU timestamps each; excludes capture/encoding/network/decoding","adapter":gpu.display.adapter,"cases":reports}),
    )
}

/// The former bilinear resize in an otherwise identical shader, so the comparison does not
/// depend on a stale release or GPU settings. The integer fast paths are bypassed too: the
/// baseline must not accidentally use them.
fn baseline_pipeline(converter: &Converter) -> Result<ID3D12PipelineState> {
    const SHADER: &str = include_str!("../shaders/color.hlsl");
    let needle = "rgb = resize(p, footprint);";
    assert_eq!(SHADER.matches(needle).count(), 1);
    let mut source = SHADER.replace(needle, "rgb = bilinear(p);");
    for condition in [
        "all(sourceSize == targetSize * 2)",
        "all(sourceSize == targetSize * 3)",
    ] {
        assert_eq!(source.matches(condition).count(), 1);
        source = source.replace(condition, "false");
    }
    // SAFETY: The shader text and out-slots outlive the calls, and the pipeline is made on the
    // converter's live device with its own root signature.
    unsafe {
        let mut bytecode = None;
        let mut errors = None;
        let result = D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(c"yuv420_cs".as_ptr().cast()),
            PCSTR(c"cs_5_0".as_ptr().cast()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut bytecode,
            Some(&mut errors),
        );
        if let Err(error) = result {
            let detail = errors
                .map(|blob| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_default();
            bail!("baseline shader: {error}: {detail}");
        }
        let bytecode = bytecode.context("baseline shader returned no bytecode")?;
        let code = std::slice::from_raw_parts(
            bytecode.GetBufferPointer().cast::<u8>(),
            bytecode.GetBufferSize(),
        );
        Ok(converter.compute.device.CreateComputePipelineState(
            &D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&converter.root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: code.as_ptr().cast(),
                    BytecodeLength: code.len(),
                },
                ..Default::default()
            },
        )?)
    }
}

#[test]
#[ignore = "requires D3D12 compute with native P010 views and an idle GPU; no display changes"]
fn gpu_downscale_timing() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = &gpu.compute;
    let mut cases = Vec::new();
    for hdr in [false, true] {
        let (image, _) = pattern(3840, 2160, 0, hdr);
        let source = GpuImage::upload(&gpu, &image)?;
        for (width, height) in [
            (3840, 2160),
            (2560, 1440),
            (1920, 1080),
            (1280, 720),
            (480, 270),
        ] {
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                codec: 1,
                hdr,
                sdr_10bit: !hdr,
                ..Default::default()
            };
            let values = constants(
                &config,
                (image.width, image.height, image.pixel),
                [100., 1.],
            );
            let mut candidate = Converter::new(compute.clone(), width, height, true)?;
            candidate.values = values;
            let mut baseline = Converter::new(compute.clone(), width, height, true)?;
            baseline.values = values;
            baseline.convert = baseline_pipeline(&baseline)?;
            let mut runs = Vec::new();
            for improved in [false, true, true, false] {
                let converter = if improved {
                    &mut candidate
                } else {
                    &mut baseline
                };
                let stats = time_conversion(compute, converter, &source, 128)?;
                runs.push(serde_json::json!({"area_filter":improved,"stats":stats}));
            }
            cases.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"runs":runs}));
        }
    }
    write_report(
        "BUTTERPOLLO_TEST_DOWNSCALE_REPORT",
        &serde_json::json!({"scope":"Identical D3D12 compute conversion except area versus previous bilinear resize; 4K source, ABBA, 128 GPU timestamps per arm; excludes capture, encoder, network, decoder and GPU game load.","adapter":gpu.display.adapter,"cases":cases}),
    )
}
