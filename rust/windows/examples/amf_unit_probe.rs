//! Slice and tile delivery probe (3.0 plan, B2 step 1). Does AMF on this GPU
//! hand back HEVC slices and AV1 tiles (and H.264 slices) before the whole
//! frame is done, and how early?
//!
//! For each codec it encodes moving 3840x2160 content paced at the stream's
//! frame rate, once with whole-frame output and once with slice or tile
//! output at 4 and 8 units. It polls QueryOutput the way the host does and
//! records, per frame, the time from AMF accepting the input to the first and
//! to the last output unit, the unit count and the bytes per unit. Times come
//! from `Instant` (QueryPerformanceCounter).
//!
//! The output properties are set by their exact AMF names through the
//! `probe_amf_properties` setting; a refusal is an error and is reported, never
//! replaced by a default. A frame with no output for five seconds is taken as
//! an encoder hang: the probe exits with code 3 and does not retry.
//!
//! Verdict per codec and unit count:
//!   KILL if the driver still returns one unit per frame, or if the median
//!   first unit arrives less than 1 ms before the median whole frame does in
//!   whole-frame mode; otherwise PASS.
#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use butterpollo_core::{config::Config, rtsp::Negotiated};
    use butterpollo_windows::{
        amf::Encoder,
        capture::{ComGuard, Device, GpuImage, Image, Pixel, Priority},
    };
    use serde_json::{Value, json};
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicU64, Ordering},
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
                "Rubylight slice/tile delivery probe\n\
--codecs h264,hevc,av1 --units 4,8 --frames 400 --warmup 30\n\
--width 3840 --height 2160 --fps 60 --bitrate 80000 (kbps)\n\
--watchdog-ms 5000 --verbose 1
\n--tile-group 1: also set Av1TileGroupObu for AV1 tile runs\n\
--out FILE: JSON result"
            );
            return Ok(());
        }
        fields.insert(key, args.next().context("option requires a value")?);
    }
    let number = |key: &str, default: u32| -> anyhow::Result<u32> {
        Ok(fields
            .get(key)
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(default))
    };
    let (width, height, fps) = (
        number("--width", 3840)?,
        number("--height", 2160)?,
        number("--fps", 60)?,
    );
    let bitrate = number("--bitrate", 80000)?;
    let frames = number("--frames", 400)? as usize;
    let warmup = number("--warmup", 30)? as usize;
    if frames <= warmup + 10 {
        bail!("--frames must exceed --warmup by more than 10");
    }
    let codecs: Vec<&str> = fields
        .get("--codecs")
        .map_or("h264,hevc,av1", String::as_str)
        .split(',')
        .collect();
    let unit_counts = fields
        .get("--units")
        .map_or("4,8", String::as_str)
        .split(',')
        .map(|n| n.parse::<u32>().context("--units takes numbers"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let verbose = number("--verbose", 0)? != 0;
    let tile_group = number("--tile-group", 0)? != 0;

    // A hung encoder blocks inside the driver, where nothing in this thread
    // can notice. The watchdog does.
    static BEAT: AtomicU64 = AtomicU64::new(0);
    static WATCHDOG_MS: AtomicU64 = AtomicU64::new(5000);
    WATCHDOG_MS.store(u64::from(number("--watchdog-ms", 5000)?), Ordering::Relaxed);
    static PHASE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let epoch = Instant::now();
    let beat = move || BEAT.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    beat();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let idle =
                (epoch.elapsed().as_millis() as u64).saturating_sub(BEAT.load(Ordering::Relaxed));
            if idle > WATCHDOG_MS.load(Ordering::Relaxed) {
                eprintln!(
                    "AMF HANG SUSPECTED: no progress for {idle} ms during {}. Exiting without retry (code 3).",
                    PHASE.lock().map(|p| p.clone()).unwrap_or_default()
                );
                std::process::exit(3);
            }
        }
    });
    let phase = |text: String| {
        if let Ok(mut p) = PHASE.lock() {
            *p = text;
        }
    };

    let _com = ComGuard::new()?;
    let _priority = Priority::new();
    let device = Device::new("")?;

    // Moving content: a scrolling colour field with fixed grain, and boxes
    // with sharp checker interiors travelling across it. Sixteen frames
    // repeat seamlessly, so no frame is a scene cut.
    phase("building the test pattern".into());
    const RING: usize = 16;
    let (w, h) = (width as usize, height as usize);
    let triangle = |v: usize| (((v & 511) as i32 - 256).unsigned_abs() / 2 + 64) as u8;
    let mut images = Vec::with_capacity(RING);
    for k in 0..RING {
        let mut bytes = vec![255u8; w * h * 4];
        for y in 0..h {
            let row = &mut bytes[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let grain = ((x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)) >> 5 & 15) as u8;
                let px = &mut row[x * 4..x * 4 + 4];
                px[0] = triangle(y * 2 + k * 32).saturating_add(grain);
                px[1] = triangle(x + y + k * 32).saturating_add(grain);
                px[2] = triangle(x * 3 + k * 32).saturating_add(grain);
            }
        }
        for b in 0..6usize {
            let x0 = (b * 640 + k * 240) % w;
            let y0 = 100 + b * 330;
            for y in y0..(y0 + 300).min(h) {
                for dx in 0..400usize {
                    let x = (x0 + dx) % w;
                    let on = ((dx / 8) + (y / 8)) % 2 == 0;
                    let v = if on { 235 } else { 20 + (b as u8) * 30 };
                    let at = (y * w + x) * 4;
                    bytes[at..at + 3].copy_from_slice(&[v, v ^ 0x55, 255 - v]);
                }
            }
        }
        let image = GpuImage::upload(
            &device,
            &Image {
                width,
                height,
                stride: w * 4,
                bytes,
                captured: Instant::now(),
                pixel: Pixel::Bgra8,
            },
        )?;
        // Make sure the copy has finished before encoding starts.
        image.readback(&mut None)?;
        images.push(image);
        beat();
    }

    let median = |values: &[f64]| percentile(values, 50);
    fn percentile(values: &[f64], p: usize) -> f64 {
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        v.get(v.len().saturating_sub(1) * p / 100)
            .copied()
            .unwrap_or(f64::NAN)
    }

    // One encoder per (codec, mode); returns the JSON for it or the error.
    let run = |codec: u8, units: u32, frames: usize, warmup: usize| -> anyhow::Result<Value> {
        let name = ["h264", "hevc", "av1"][codec as usize];
        let (mode_name, count_name, buffer_type) = match codec {
            0 => ("OutputMode", "SlicesPerFrame", "OutputBufferType"),
            1 => (
                "HevcOutputMode",
                "HevcSlicesPerFrame",
                "HevcOutputBufferType",
            ),
            _ => (
                "AV1OutputMode",
                "Av1NumTilesPerFrame",
                "AV1OutputBufferType",
            ),
        };
        let capability = [
            "SupportSliceOutput",
            "HevcSupportSliceOutput",
            "AV1SupportTileOutput",
        ][codec as usize];
        let mut properties = vec![];
        if units > 0 {
            properties.push(format!("{mode_name}=1"));
            properties.push(format!("{count_name}={units}"));
            if codec == 2 && tile_group {
                properties.push("Av1TileGroupObu=1".into());
            }
        }
        let mut tuning = Config::default();
        if !properties.is_empty() {
            tuning
                .values
                .insert("probe_amf_properties".into(), properties.join(","));
        }
        let config = Negotiated {
            width,
            height,
            fps,
            bitrate_kbps: bitrate,
            codec,
            ..Default::default()
        };
        phase(format!("{name} {units} units: creating the encoder"));
        let mut encoder = Encoder::new_gpu(&config, device.clone(), &tuning, None)
            .with_context(|| format!("{name} encoder with {properties:?}"))?;
        encoder.record_units();
        let effective: BTreeMap<_, _> = [mode_name, count_name, buffer_type]
            .into_iter()
            .chain((codec == 2).then_some("Av1TileGroupObu"))
            .map(|n| (n, encoder.read(n)))
            .collect();
        let tick = Duration::from_secs_f64(1. / f64::from(fps));
        let mut next = Instant::now() + tick;
        let (mut first, mut last, mut counts, mut first_bytes, mut total_bytes, mut spans) =
            (vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut shapes = BTreeMap::<String, u32>::new();
        let mut example_units = vec![];
        for i in 0..frames {
            phase(format!("{name} {units} units: frame {i}"));
            beat();
            let now = Instant::now();
            if next > now + Duration::from_millis(2) {
                std::thread::sleep(next - now - Duration::from_millis(2));
            }
            while Instant::now() < next {
                std::hint::spin_loop();
            }
            next += tick;
            if i == 0 && verbose {
                eprintln!("{name} {units}: submitting frame 0");
            }
            let mut done = !encoder
                .encode_gpu(&images[i % RING], i == 0, bitrate)?
                .is_empty();
            if i == 0 && verbose {
                eprintln!("{name} {units}: frame 0 accepted, done={done}");
            }
            while !done && encoder.pending() {
                beat();
                done = !encoder.poll()?.is_empty();
            }
            let log = encoder.take_units();
            let mine: Vec<_> = log.iter().filter(|u| u.pts == i as i64).collect();
            if mine.is_empty() {
                bail!("{name} {units} units: frame {i} produced no output unit");
            }
            if i < warmup {
                continue;
            }
            let Some(origin) = mine[0].accepted else {
                bail!("{name} {units} units: frame {i} has no submission time");
            };
            let ms = |at: Instant| at.saturating_duration_since(origin).as_secs_f64() * 1000.;
            first.push(ms(mine[0].at));
            last.push(ms(mine[mine.len() - 1].at));
            spans.push(ms(mine[mine.len() - 1].at) - ms(mine[0].at));
            counts.push(mine.len() as f64);
            first_bytes.push(mine[0].bytes as f64);
            total_bytes.push(mine.iter().map(|u| u.bytes).sum::<usize>() as f64);
            let kinds: Vec<_> = mine.iter().map(|u| u.kind).collect();
            *shapes.entry(format!("{kinds:?}")).or_default() += 1;
            if example_units.is_empty() && mine.len() > 1 {
                example_units = mine
                    .iter()
                    .map(|u| json!({"kind": u.kind, "bytes": u.bytes, "at_ms": ms(u.at)}))
                    .collect();
            }
        }
        let caps = encoder.capability(capability);
        let stat = |v: &[f64]| json!({"median": median(v), "p95": percentile(v, 95), "max": percentile(v, 100)});
        Ok(json!({
            "codec": name,
            "mode": if units > 0 { "units" } else { "frame" },
            "units_requested": units,
            "capability": {capability: caps},
            "properties_set": properties,
            "properties_effective": effective,
            "frames_measured": first.len(),
            "first_unit_ms": stat(&first),
            "last_unit_ms": stat(&last),
            "first_to_last_ms": stat(&spans),
            "units_per_frame": stat(&counts),
            "first_unit_bytes": stat(&first_bytes),
            "frame_bytes": stat(&total_bytes),
            "buffer_type_sequences": shapes,
            "example_frame": example_units,
        }))
    };

    // The process's first encode builds shader and driver state for seconds,
    // which the watchdog would take for a hang and which AMF's two-second
    // expiry would take for a lost frame. Do it once, unmeasured.
    WATCHDOG_MS.store(30000, Ordering::Relaxed);
    if let Err(error) = run(1, 0, 6, 6) {
        eprintln!("warm-up encode (not measured): {error:#}");
    }
    WATCHDOG_MS.store(u64::from(number("--watchdog-ms", 5000)?), Ordering::Relaxed);
    let mut runs = vec![];
    let mut verdicts = BTreeMap::new();
    for codec_name in &codecs {
        let codec = match *codec_name {
            "h264" => 0u8,
            "hevc" => 1,
            "av1" => 2,
            other => bail!("unknown codec {other}"),
        };
        let mut results = BTreeMap::new();
        for units in std::iter::once(0).chain(unit_counts.iter().copied()) {
            let outcome = run(codec, units, frames, warmup);
            let entry = match outcome {
                Ok(value) => value,
                Err(error) => json!({
                    "codec": codec_name,
                    "mode": if units > 0 { "units" } else { "frame" },
                    "units_requested": units,
                    "error": format!("{error:#}"),
                }),
            };
            eprintln!("{codec_name} units={units}: {}", entry);
            results.insert(units, entry.clone());
            runs.push(entry);
        }
        let get = |units: u32, group: &str, stat: &str| {
            results
                .get(&units)
                .and_then(|r| r.get(group))
                .and_then(|g| g.get(stat))
                .and_then(Value::as_f64)
        };
        let mut per_count = vec![];
        let mut any_pass = false;
        for &units in &unit_counts {
            let verdict = match (
                results.get(&0).and_then(|r| r.get("error")),
                results.get(&units).and_then(|r| r.get("error")),
            ) {
                (Some(e), _) | (_, Some(e)) => {
                    json!({"units": units, "verdict": "KILL", "reason": format!("encoder refused or failed: {e}")})
                }
                _ => {
                    let per_frame = get(units, "units_per_frame", "median").unwrap_or(0.);
                    let max_units = get(units, "units_per_frame", "max").unwrap_or(0.);
                    let frame_last = get(0, "last_unit_ms", "median").unwrap_or(f64::NAN);
                    let unit_first = get(units, "first_unit_ms", "median").unwrap_or(f64::NAN);
                    let unit_last = get(units, "last_unit_ms", "median").unwrap_or(f64::NAN);
                    let gain = frame_last - unit_first;
                    let (verdict, reason) = if max_units <= 1. {
                        (
                            "KILL",
                            "the driver still returns one unit per frame".to_string(),
                        )
                    } else if gain.is_nan() || gain < 1. {
                        (
                            "KILL",
                            format!(
                                "first unit comes only {gain:.3} ms before the whole frame in frame mode"
                            ),
                        )
                    } else {
                        (
                            "PASS",
                            format!("first unit {gain:.3} ms before the whole frame in frame mode"),
                        )
                    };
                    any_pass |= verdict == "PASS";
                    json!({
                        "units": units,
                        "verdict": verdict,
                        "reason": reason,
                        "units_per_frame_median": per_frame,
                        "first_unit_gain_vs_frame_mode_ms": gain,
                        "first_to_last_within_mode_ms": get(units, "first_to_last_ms", "median"),
                        "full_frame_vs_frame_mode_median_ms": unit_last - frame_last,
                        "full_frame_vs_frame_mode_p95_ms": get(units, "last_unit_ms", "p95").unwrap_or(f64::NAN)
                            - get(0, "last_unit_ms", "p95").unwrap_or(f64::NAN),
                        "slower_full_frame": unit_last - frame_last > 0.1,
                    })
                }
            };
            per_count.push(verdict);
        }
        let overall = if any_pass { "PASS" } else { "KILL" };
        println!("{codec_name}: {overall}");
        for v in &per_count {
            println!("  {v}");
        }
        verdicts.insert(
            codec_name.to_string(),
            json!({"verdict": overall, "by_units": per_count}),
        );
    }
    let result = json!({
        "width": width,
        "height": height,
        "fps": fps,
        "bitrate_kbps": bitrate,
        "frames": frames,
        "warmup": warmup,
        "timing": "Instant (QueryPerformanceCounter) from AMF accepting the input to QueryOutput returning each unit; QueryTimeout 1 ms as in the host",
        "verdicts": verdicts,
        "runs": runs,
    });
    if let Some(out) = fields.get("--out") {
        std::fs::write(out, serde_json::to_string_pretty(&result)?)?;
    }
    Ok(())
}
