//! Opt-in native coverage; synthetic textures never change a desktop or display.
use super::*;
use std::{
    fs,
    io::Write,
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
};

const FRAMES: usize = 900;
const RECOVERY: usize = 733;

#[test]
#[ignore = "requires an idle AMD GPU, AMF and a task-owned report path"]
fn native_amf_recycled_surface_barriers_are_valid() -> Result<()> {
    use windows::Win32::Graphics::Direct3D12::*;
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_AMF_STATE_REPORT")
            .context("set BUTTERPOLLO_TEST_AMF_STATE_REPORT")?,
    );
    anyhow::ensure!(report.is_absolute(), "report path must be absolute");
    fs::create_dir_all(report.parent().context("report directory")?)?;
    let mut debug: Option<ID3D12Debug> = None;
    // SAFETY: This isolated native test has created no D3D12 device yet; the out-value
    // is checked before enabling validation for this process.
    unsafe {
        D3D12GetDebugInterface(&mut debug)?;
        debug
            .context("D3D12 debug layer unavailable")?
            .EnableDebugLayer();
    }
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    anyhow::ensure!(crate::compute::copies_on(&device.device), "requires AMD");
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let info: ID3D12InfoQueue = compute.device.cast()?;
    let mut reports = Vec::new();
    for (hdr, codec) in [(false, 1), (true, 1), (false, 2), (true, 2)] {
        let picture = picture(&device, hdr, 0)?;
        let config = butterpollo_core::rtsp::Negotiated {
            width: 3840,
            height: 2160,
            fps: 120,
            rate_millihz: 119_880,
            bitrate_kbps: 89_388,
            codec,
            hdr,
            references: 1,
            intra_refresh: true,
            csc_mode: if hdr { 4 } else { 0 },
            ..Default::default()
        };
        let mut encoder = Encoder::new_gpu(
            &config,
            device.clone(),
            &Default::default(),
            Some(compute.clone()),
        )?;
        anyhow::ensure!(encoder.compute.is_some(), "requires D3D12 AMF");
        let format = if codec == 1 { "hevc" } else { "av1" };
        let range = if hdr { "hdr" } else { "sdr" };
        let stream_path = report.with_extension(format!("{range}.{format}"));
        let mut stream = std::io::BufWriter::new(fs::File::create(&stream_path)?);
        let mut states = Vec::new();
        for index in 0..32 {
            let started = Instant::now();
            let mut output = encoder.encode_gpu(&picture, index == 0, config.bitrate_kbps)?;
            while encoder.pending() {
                anyhow::ensure!(
                    started.elapsed() < Duration::from_secs(2),
                    "encoder stalled"
                );
                output.extend(encoder.poll()?);
            }
            anyhow::ensure!(output.len() == 1, "missing output");
            stream.write_all(&output[0].bytes)?;
            let frame_states = encoder
                .compute
                .as_ref()
                .unwrap()
                .converter
                .test_amf_target_states();
            anyhow::ensure!(
                !frame_states.is_empty() && frame_states.iter().all(Option::is_some),
                "missing AMF resource state"
            );
            states.push(frame_states);
        }
        stream.flush()?;
        reports.push(serde_json::json!({"codec":codec,"hdr":hdr,"frames":states.len(),"stream":stream_path,"states":states}));
    }
    let mut errors = Vec::new();
    // SAFETY: The info queue owns its messages; each aligned buffer is sized from the
    // initial query and kept alive while reading its returned message and description.
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
                errors.push(serde_json::json!({"id":message.ID.0,"description":String::from_utf8_lossy(std::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength))}));
            }
        }
    }
    fs::write(
        &report,
        serde_json::to_vec_pretty(
            &serde_json::json!({"codecs":reports,"validation_errors":errors}),
        )?,
    )?;
    anyhow::ensure!(
        errors.is_empty(),
        "D3D12 validation errors; see {}",
        report.display()
    );
    Ok(())
}

fn integer(encoder: &Encoder, name: &str) -> Result<i64> {
    // SAFETY: The live encoder owns the component; the terminated property name
    // and initialized out-value outlive the synchronous call. Read the union
    // only after checking its tag. These properties return integers, not refs.
    unsafe {
        let mut value: AMFVariantStruct = std::mem::zeroed();
        check(((*(*encoder.component).pVtbl).GetProperty.unwrap())(
            encoder.component,
            to_wide(name).as_ptr(),
            &mut value,
        ))
        .with_context(|| format!("read {name}"))?;
        anyhow::ensure!(
            value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64,
            "{name} is not integer"
        );
        Ok(value.__bindgen_anon_1.int64Value)
    }
}

fn picture(device: &Device, hdr: bool, phase: usize) -> Result<GpuImage> {
    let (width, height, pixel_bytes) = (3840usize, 2160usize, if hdr { 8 } else { 4 });
    let palette: Vec<Vec<u8>> = (0..16)
        .map(|n| {
            let rgb = [n as f32 / 15., (15 - n) as f32 / 15., (n % 5) as f32 / 4.];
            if hdr {
                // Linear scRGB: diffuse white is 203 nits; selected highlights reach 1000.
                let scale = if n == 15 { 12.5 } else { 203. / 80. };
                [rgb[0] * scale, rgb[1] * scale, rgb[2] * scale, 1.]
                    .into_iter()
                    .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
                    .collect()
            } else {
                vec![
                    (rgb[2] * 255.) as u8,
                    (rgb[1] * 255.) as u8,
                    (rgb[0] * 255.) as u8,
                    255,
                ]
            }
        })
        .collect();
    let mut bytes = vec![0; width * height * pixel_bytes];
    for (y, row) in bytes.chunks_exact_mut(width * pixel_bytes).enumerate() {
        for (x, pixel) in row.chunks_exact_mut(pixel_bytes).enumerate() {
            let detail =
                (x + phase * 97) % width < width / 6 && (y + phase * 53) % height < height / 6;
            let index = if detail {
                x * 13 + y * 7 + phase * 3
            } else {
                (x / 32 + phase * 3) ^ (y / 32)
            };
            pixel.copy_from_slice(&palette[index % 16]);
        }
    }
    let image = GpuImage::upload(
        device,
        &Image {
            width: width as u32,
            height: height as u32,
            stride: width * pixel_bytes,
            bytes,
            captured: Instant::now(),
            pixel: if hdr {
                crate::capture::Pixel::RgbaF16
            } else {
                crate::capture::Pixel::Bgra8
            },
        },
    )?;
    // Finish the initial D3D11 upload before a D3D12 queue reads it.
    image.readback(&mut None)?;
    Ok(image)
}

fn decode(ffmpeg: &std::ffi::OsStr, stream: &Path) -> Result<serde_json::Value> {
    let hashes = stream.with_extension("framemd5");
    let errors = stream.with_extension("decode.log");
    let mut child = Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-xerror",
            "-err_detect",
            "explode",
            "-threads",
            "4",
            "-f",
            "hevc",
            "-i",
        ])
        .arg(stream)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "framemd5",
            "-",
        ])
        .stdin(Stdio::null())
        .creation_flags(0x08000000)
        .stdout(fs::File::create(&hashes)?)
        .stderr(fs::File::create(&errors)?)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("independent decoder timed out: {}", stream.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let decoded = fs::read_to_string(hashes)?
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .count();
    let stderr = fs::read_to_string(errors)?;
    Ok(
        serde_json::json!({"frames":decoded,"success":status.success() && stderr.is_empty(),"stderr":stderr}),
    )
}

#[test]
#[ignore = "requires an idle AMD GPU, AMF, independent FFmpeg and a task-owned report path"]
fn native_hevc_intra_refresh_decodes_across_gops_and_forced_recovery() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_HEVC_INTRA_REPORT")
            .context("set BUTTERPOLLO_TEST_HEVC_INTRA_REPORT to a task-owned JSON path")?,
    );
    anyhow::ensure!(report.is_absolute(), "report path must be absolute");
    fs::create_dir_all(report.parent().context("report directory")?)?;
    let ffmpeg =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let legacy = match std::env::var("BUTTERPOLLO_TEST_HEVC_INTRA_BASELINE").as_deref() {
        Err(_) | Ok("0") => false,
        Ok("1") => true,
        _ => bail!("baseline must be 0 or 1"),
    };
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    anyhow::ensure!(
        crate::compute::copies_on(&device.device),
        "requires an AMD adapter"
    );
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let timer = crate::timing::Timer::new()?;
    let mut reports = Vec::new();
    for hdr in [false, true] {
        let name = if hdr { "hdr" } else { "sdr" };
        let config = butterpollo_core::rtsp::Negotiated {
            width: 3840,
            height: 2160,
            fps: 120,
            rate_millihz: 119_880,
            bitrate_kbps: 117_964,
            codec: 1,
            hdr,
            references: 1,
            intra_refresh: true,
            csc_mode: if hdr { 4 } else { 0 },
            ..Default::default()
        };
        let pictures = (0..4)
            .map(|phase| picture(&device, hdr, phase))
            .collect::<Result<Vec<_>>>()?;
        let mut encoder = Encoder::new_gpu(
            &config,
            device.clone(),
            &Default::default(),
            Some(compute.clone()),
        )?;
        anyhow::ensure!(
            encoder.compute.is_some(),
            "D3D12 encoder input was not established"
        );
        if legacy {
            // Test-only dynamic override, after Init but before any submission.
            // This is a policy comparison, not a separate old-driver build.
            encoder.property_raw("HevcGOPSize", int(0))?;
            encoder.property_raw("HevcIntraRefreshCTBsNumberPerSlot", int(7))?;
        }
        let mut properties = serde_json::Map::new();
        for property in [
            "HevcGOPSize",
            "HevcGOPSPerIDR",
            "HevcIntraRefreshCTBsNumberPerSlot",
            "HevcMaxNumRefFrames",
            "HevcMaxAUSize",
            "HevcTargetBitrate",
        ] {
            properties.insert(
                property.into(),
                serde_json::json!(integer(&encoder, property)?),
            );
        }
        anyhow::ensure!(
            properties["HevcGOPSize"] == if legacy { 0 } else { 120 },
            "wrong GOP readback"
        );
        anyhow::ensure!(
            properties["HevcIntraRefreshCTBsNumberPerSlot"] == if legacy { 7 } else { 60 },
            "wrong refresh readback"
        );
        anyhow::ensure!(
            properties["HevcMaxNumRefFrames"] == 1,
            "one-reference client budget changed"
        );
        anyhow::ensure!(
            properties["HevcGOPSPerIDR"].as_i64().is_some_and(|n| n > 0),
            "GOPs per IDR must not be disabled"
        );
        fs::write(
            report.with_extension(format!("{name}.setup.json")),
            serde_json::to_vec_pretty(&properties)?,
        )?;
        if hdr {
            encoder.set_hdr_metadata(butterpollo_core::hdr::Metadata::display(1000., 0.005, 400.));
        }
        let full_path = report.with_extension(format!("{name}.hevc"));
        let tail_path = report.with_extension(format!("{name}.recovery.hevc"));
        let mut full = std::io::BufWriter::new(fs::File::create(&full_path)?);
        let mut tail = std::io::BufWriter::new(fs::File::create(&tail_path)?);
        let start = Instant::now();
        let period = Duration::from_secs_f64(1000. / 119_880.);
        let mut frames = Vec::new();
        for index in 0..FRAMES {
            timer.until(start + period * index as u32);
            // First expose periodic output with a completely static picture,
            // then alternate detail and colour changes across GOP boundaries.
            let source = if index < 240 {
                0
            } else {
                index % pictures.len()
            };
            let mut image = pictures[source].clone();
            image.captured = Instant::now();
            let deadline = image.captured + Duration::from_secs(2);
            let mut output =
                encoder.encode_gpu(&image, index == 0 || index == RECOVERY, config.bitrate_kbps)?;
            while encoder.pending() {
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "AMF stalled at {name} frame {index}"
                );
                output.extend(encoder.poll()?);
            }
            anyhow::ensure!(
                Instant::now() < deadline && output.len() == 1,
                "missing/late {name} output {index}"
            );
            let frame = output.pop().unwrap();
            anyhow::ensure!(
                frame.presentation == Some(image.captured),
                "output reordered at {index}"
            );
            if index == 0 || index == RECOVERY {
                anyhow::ensure!(frame.idr, "forced IDR absent at {index}");
            }
            full.write_all(&frame.bytes)?;
            if index >= RECOVERY {
                tail.write_all(&frame.bytes)?;
            }
            frames.push(
                serde_json::json!({"index":index,"idr":frame.idr,"bytes":frame.bytes.len(),
                "output_ms":image.captured.elapsed().as_secs_f64()*1000.,
                "encode_ms":frame.latency.map(|latency|latency.as_secs_f64()*1000.)}),
            );
        }
        let elapsed = start.elapsed().as_secs_f64();
        full.flush()?;
        tail.flush()?;
        drop(full);
        drop(tail);
        drop(encoder);
        let packetizer = butterpollo_core::packet::VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: None,
        };
        let maximum = frames
            .iter()
            .map(|frame| frame["bytes"].as_u64().unwrap() as usize)
            .max()
            .unwrap();
        let layout = packetizer.block_layout(maximum);
        let fits = layout.len() <= 4 && layout.iter().all(|(data, parity)| data + parity <= 255);
        let wire_bytes = layout
            .iter()
            .map(|(data, parity)| data + parity)
            .sum::<usize>()
            * (1392 + 16 + 66);
        let mut result = serde_json::json!({"variant":name,"legacy_after_init_override":legacy,
            "adapter":device.display.adapter,"width":3840,"height":2160,"fps":119.88,"bitrate_kbps":117964,
            "source":"240 static frames then four changing synthetic GPU textures; no capture/display/network", "input_memory":"D3D12",
            "properties":properties,"elapsed_seconds":elapsed,"frames":frames,"forced_idr_index":RECOVERY,
            "max_payload_bytes":maximum,"max_frame_block_layout":layout,"max_frame_wire_bytes":wire_bytes,
            "fec_percent":20,"fec_limited":packetizer.fec_limited(maximum),"packet_layout_valid":fits});
        // Preserve native evidence even when decoding or the packet budget fails.
        let case_report = report.with_extension(format!("{name}.json"));
        fs::write(&case_report, serde_json::to_vec_pretty(&result)?)?;
        result["full_decode"] = decode(&ffmpeg, &full_path)?;
        result["recovery_decode"] = decode(&ffmpeg, &tail_path)?;
        fs::write(&case_report, serde_json::to_vec_pretty(&result)?)?;
        reports.push(result.clone());
        fs::write(&report, serde_json::to_vec_pretty(&reports)?)?;
        anyhow::ensure!(fits, "{name} frame exceeds four 255-shard blocks");
        anyhow::ensure!(
            result["full_decode"]["success"] == true && result["full_decode"]["frames"] == FRAMES,
            "{name} full stream did not independently decode every frame"
        );
        anyhow::ensure!(
            result["recovery_decode"]["success"] == true
                && result["recovery_decode"]["frames"] == FRAMES - RECOVERY,
            "{name} forced-IDR tail did not independently decode every frame"
        );
        println!(
            "HEVC intra-refresh {name}: {FRAMES} frames, {maximum} max bytes, report {}",
            case_report.display()
        );
    }
    Ok(())
}
