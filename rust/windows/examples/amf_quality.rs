//! Encoder quality probe: encodes BGRA frames read from stdin with the
//! stream's encoder and writes the bitstream, so a reference decoder can
//! score it (VMAF, PSNR) against the same frames. Each frame is encoded on
//! its own, so the reported encode time excludes queueing.
#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use butterpollo_core::encoder_focus::{Levels, Mode, Requested};
    use butterpollo_core::rtsp::Negotiated;
    use butterpollo_windows::{
        capture::{ComGuard, Device, GpuImage, Image, Pixel, Priority},
        encoder::Encoder,
    };
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        time::{Duration, Instant},
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let mut fields = BTreeMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(key) = args.next() {
        if key == "--help" {
            println!(
                "Rubylight encoder quality probe\n\
--codec hevc (h264/hevc/av1) --width 1920 --height 1080 --fps 120 --bitrate 20000\n\
--frames N: frames to read from stdin (BGRA, or linear gbrpf32le with --hdr 1)\n\
--source-width N --source-height N: input size, defaults to the encoded size\n\
--packing 0|1|2: chroma packing (0 is 4:2:0, 1 and 2 are the packed 4:4:4 layouts)\n\
--ten-bit 1: 10-bit SDR encode (the input stays BGRA)\n\
--csc-mode N: SDR matrix and range as encoderCscMode (0 is Rec.601 limited, 2 is Rec.709 limited)\n\
--focus off|static|centre: encoder focus ROI map, over encoder_focus in --config\n\
  (levels from encoder_focus_levels in --config)\n\
--focus-verify 1: block QP feedback once a second, for focus_qp_delta (or encoder_focus_verify)\n\
--config FILE: host settings; --out FILE: bitstream"
            );
            return Ok(());
        }
        fields.insert(key, args.next().context("option requires a value")?);
    }
    let option = |key: &str| fields.get(key).cloned();
    let number = |key: &str, default: u32| -> anyhow::Result<u32> {
        Ok(option(key)
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(default))
    };
    let config = Negotiated {
        width: number("--width", 1920)?,
        height: number("--height", 1080)?,
        fps: number("--fps", 120)?,
        bitrate_kbps: number("--bitrate", 20000)?,
        codec: match option("--codec").as_deref().unwrap_or("hevc") {
            "h264" => 0,
            "hevc" => 1,
            "av1" => 2,
            other => bail!("unknown codec {other}"),
        },
        hdr: option("--hdr").is_some_and(|v| v == "1"),
        csc_mode: if option("--hdr").is_some_and(|v| v == "1") {
            4
        } else {
            number("--csc-mode", 0)?.try_into()?
        },
        chroma_packing: number("--packing", 0)?.try_into()?,
        sdr_10bit: option("--ten-bit").is_some_and(|v| v == "1"),
        ..Default::default()
    };
    let frames = number("--frames", 300)?;
    let source_width = number("--source-width", config.width)?;
    let source_height = number("--source-height", config.height)?;
    if source_width == 0 || source_height == 0 || source_width > 16384 || source_height > 16384 {
        bail!("source dimensions must be between 1 and 16384");
    }
    let out = option("--out").context("--out is required")?;
    let tuning = match option("--config") {
        Some(path) => butterpollo_core::config::Config::load(std::path::Path::new(&path))?,
        None => Default::default(),
    };
    // The stream's keys in --config, as focus_ab.py writes them; --focus wins.
    let focus = match option("--focus")
        .as_deref()
        .map_or_else(
            || Requested::parse(tuning.get("encoder_focus", "off")),
            Requested::parse,
        )
        .map_err(anyhow::Error::msg)?
    {
        Requested::Off => None,
        Requested::Static => Some(Mode::Static),
        Requested::Centre => Some(Mode::Centre),
    };
    let focus_verify = match option("--focus-verify") {
        Some(value) => value == "1",
        None => tuning.boolean("encoder_focus_verify", false),
    };
    let levels = match tuning.values.get("encoder_focus_levels") {
        Some(value) => Levels::parse(value).map_err(anyhow::Error::msg)?,
        None => Levels::default(),
    };
    let _com = ComGuard::new()?;
    let _priority = Priority::new();
    let device = Device::new("")?;
    let pixels = source_width as usize * source_height as usize;
    // HDR input is FFmpeg's planar linear-light gbrpf32le with SDR white at
    // 1.0; the capture format is scRGB FP16, where 1.0 is 80 nits. Scaling
    // by 203/80 puts SDR white at the 203 nits of ITU-R BT.2408.
    let size = if config.hdr { pixels * 12 } else { pixels * 4 };
    let mut input = std::io::stdin().lock();
    let mut read = |index: u32| -> anyhow::Result<GpuImage> {
        let mut bytes = vec![0; size];
        input
            .read_exact(&mut bytes)
            .with_context(|| format!("stdin ended before frame {index}"))?;
        let (bytes, stride, pixel) = if config.hdr {
            let plane = |p: usize, i: usize| {
                let at = (p * pixels + i) * 4;
                f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) * (203. / 80.)
            };
            let mut rgba = Vec::with_capacity(pixels * 8);
            for i in 0..pixels {
                // gbrp: planes in G, B, R order.
                for value in [plane(2, i), plane(0, i), plane(1, i), 1.] {
                    rgba.extend_from_slice(&half::f16::from_f32(value).to_le_bytes());
                }
            }
            (rgba, source_width as usize * 8, Pixel::RgbaF16)
        } else {
            (bytes, source_width as usize * 4, Pixel::Bgra8)
        };
        let image = GpuImage::upload(
            &device,
            &Image {
                width: source_width,
                height: source_height,
                stride,
                bytes,
                captured: Instant::now(),
                pixel,
            },
        )?;
        // An uploaded texture has no fence for the encoder's queues to
        // wait on, as a captured one does; reading it back makes sure the
        // copy has finished before encoding starts.
        image.readback(&mut None)?;
        Ok(image)
    };
    let first = read(0)?;
    let mut encoder = Encoder::new_gpu_options(&config, &first, &tuning)?;
    if config.hdr {
        // As a stream does before its first frame: the display's values,
        // with optional content light levels (--max-cll N --max-fall N).
        let mut metadata = butterpollo_core::hdr::Metadata::display(1000., 0.005, 400.);
        metadata.max_cll = number("--max-cll", 0)? as u16;
        metadata.max_fall = number("--max-fall", 0)? as u16;
        encoder.set_hdr_metadata(metadata);
    }
    if let Some(mode) = focus {
        let Encoder::Amf(amf) = &mut encoder else {
            bail!("--focus needs the AMF encoder, not {}", encoder.backend());
        };
        amf.set_focus(Some(butterpollo_windows::compute::FocusSettings {
            mode,
            levels,
            cursor: None,
        }))?;
        if focus_verify {
            amf.set_focus_verify(Some(config.fps.max(1)));
        }
    }
    let mut bitstream = std::io::BufWriter::new(std::fs::File::create(&out)?);
    let (mut latencies, mut sizes, mut idr_bytes) = (Vec::new(), Vec::new(), Vec::new());
    // The first frames' sizes in order, to compare a map's first frames with off.
    let mut head = Vec::new();
    let mut image = Some(first);
    // The first frame that carries a map, which the rate control did not plan.
    let mut first_map: Option<(u32, usize)> = None;
    let maps = |encoder: &Encoder| encoder.focus_stats().map_or(0, |stats| stats.maps);
    for index in 0..frames {
        let maps_before = maps(&encoder);
        let picture = match image.take() {
            Some(picture) => picture,
            None => read(index)?,
        };
        let mut output = encoder.encode_gpu(&picture, index == 0, config.bitrate_kbps)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while encoder.pending() {
            output.extend(encoder.poll()?);
            if Instant::now() >= deadline {
                bail!("frame {index} was not encoded within two seconds");
            }
        }
        let bytes = output.iter().map(|frame| frame.bytes.len()).sum::<usize>();
        if first_map.is_none() && maps_before == 0 && maps(&encoder) > 0 {
            first_map = Some((index, bytes));
        }
        if head.len() < 32 {
            head.push(bytes);
        }
        for frame in output {
            bitstream.write_all(&frame.bytes)?;
            if frame.idr {
                idr_bytes.push(frame.bytes.len());
            } else {
                sizes.push(frame.bytes.len());
                if let Some(latency) = frame.latency {
                    latencies.push(latency.as_secs_f64() * 1000.);
                }
            }
        }
    }
    bitstream.flush()?;
    let total_bytes = sizes.iter().chain(&idr_bytes).sum::<usize>();
    let encoded_frames = sizes.len() + idr_bytes.len();
    let mut all_sizes = sizes.iter().chain(&idr_bytes).copied().collect::<Vec<_>>();
    all_sizes.sort_unstable();
    latencies.sort_by(f64::total_cmp);
    sizes.sort_unstable();
    let focus_stats = encoder.focus_stats().unwrap_or_default();
    let percentile = |values: &[f64], p: usize| {
        values
            .get(values.len().saturating_sub(1) * p / 100)
            .copied()
            .unwrap_or(0.)
    };
    println!(
        "{}",
        serde_json::json!({
            "codec": option("--codec").unwrap_or_else(|| "hevc".into()),
            "width": config.width,
            "height": config.height,
            "source_width": source_width,
            "source_height": source_height,
            "fps": config.fps,
            "bitrate_kbps": config.bitrate_kbps,
            "frames": frames,
            "encoded_frames": encoded_frames,
            "references": config.references,
            "packing": config.chroma_packing,
            "ten_bit": config.sdr_10bit,
            "actual_bitrate_kbps": total_bytes as f64 * 8. * config.fps as f64 / frames as f64 / 1000.,
            "frame_bytes_max": all_sizes.last(),
            "all_frame_bytes_p99": all_sizes.get(all_sizes.len().saturating_sub(1) * 99 / 100),
            "encode_mean_ms": latencies.iter().sum::<f64>() / latencies.len().max(1) as f64,
            "encode_p95_ms": percentile(&latencies, 95),
            "encode_p99_ms": percentile(&latencies, 99),
            "frame_bytes_mean": sizes.iter().sum::<usize>() as f64 / sizes.len().max(1) as f64,
            "frame_bytes_p99": sizes.get(sizes.len().saturating_sub(1) * 99 / 100),
            "idr_bytes": idr_bytes,
            "backend": encoder.backend(),
            "focus_mode": focus.map_or("off", Mode::as_str),
            "focus_maps": focus_stats.maps,
            "focus_uniform": focus_stats.uniform,
            "focus_stale": focus_stats.stale,
            "focus_qp_delta": focus_stats.qp_delta,
            "focus_first_map_frame": first_map.map(|(index, _)| index),
            "focus_first_map_bytes": first_map.map(|(_, bytes)| bytes),
            "frame_cap_bytes": encoder.frame_cap(),
            "frame_bytes_head": head,
        })
    );
    Ok(())
}
