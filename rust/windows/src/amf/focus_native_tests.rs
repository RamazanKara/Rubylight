//! Opt-in M0 probe (docs/encoder-focus-design.md section 7): does this GPU's
//! AMF honour an ROI map, per codec and bit depth? Synthetic textures only.
use super::*;
use butterpollo_core::encoder_focus::Support;

#[test]
#[ignore = "requires an idle AMD GPU and AMF"]
fn focus_native_probe_verifies_roi_per_codec_and_depth() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for (name, codec, ten_bit) in [
        ("h264", 0, false),
        ("hevc", 1, false),
        ("hevc", 1, true),
        ("av1", 2, false),
        ("av1", 2, true),
    ] {
        for (width, height, bitrate_kbps) in [(640, 480, 2_000), (1920, 1080, 20_000)] {
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                fps: 60,
                bitrate_kbps,
                codec,
                sdr_10bit: ten_bit,
                ..Default::default()
            };
            let mut encoder = Encoder::new_gpu(
                &config,
                device.clone(),
                &Default::default(),
                Some(compute.clone()),
            )?;
            let probe = encoder
                .probe_focus()
                .with_context(|| format!("{name} {width}x{height}"))?;
            let depth = if ten_bit { 10 } else { 8 };
            let (verified, reason, message) = match &probe.support {
                Support::Verified { .. } => (true, None, None),
                Support::Unsupported { reason, message } => {
                    (false, Some(reason.wire()), Some(message.clone()))
                }
                Support::NotProbed => (false, None, None),
            };
            println!(
                "{name} {depth}-bit {width}x{height}: {} (qp_delta {:?}, picture {:?}, feedback {:?})",
                probe.support.summary(),
                probe.qp_delta,
                probe.control_delta,
                probe.feedback_grid
            );
            if !verified {
                failures.push(format!("{name} {depth}-bit {width}x{height}"));
            }
            rows.push(serde_json::json!({
                "codec": name,
                "depth": depth,
                "width": width,
                "height": height,
                "verified": verified,
                "qp_delta": probe.qp_delta,
                "control_delta": probe.control_delta,
                "feedback_grid": probe.feedback_grid,
                "reason": reason,
                "message": message,
            }));
        }
    }
    if let Some(path) = std::env::var_os("BUTTERPOLLO_FOCUS_PROBE_OUT") {
        let path = std::path::PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&rows)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    anyhow::ensure!(
        failures.is_empty(),
        "ROI not verified for {}",
        failures.join(", ")
    );
    Ok(())
}

/// Means of a QP grid's left, right, top and bottom halves, and its min and max.
fn halves(grid: &focus::Grid) -> [f64; 6] {
    let (w, h) = (grid.width, grid.height);
    let mut sums = [(0f64, 0u32); 4];
    let (mut min, mut max) = (u32::MAX, 0);
    for (index, &value) in grid.values.iter().enumerate() {
        let (x, y) = (index as u32 % w, index as u32 / w);
        for (side, inside) in [x < w / 2, x >= w - w / 2, y < h / 2, y >= h - h / 2]
            .into_iter()
            .enumerate()
        {
            if inside {
                sums[side].0 += f64::from(value);
                sums[side].1 += 1;
            }
        }
        min = min.min(value);
        max = max.max(value);
    }
    let mean = |(sum, count): (f64, u32)| sum / f64::from(count.max(1));
    [
        mean(sums[0]),
        mean(sums[1]),
        mean(sums[2]),
        mean(sums[3]),
        f64::from(min),
        f64::from(max),
    ]
}

/// Diagnostics for the M0 probe, configured by environment:
/// `FOCUS_DIAG_CASES` `codec:WxH:kbps[:10]` comma-separated (codec 0/1/2),
/// `FOCUS_DIAG_VARIANTS` `|`-separated `key=value;key=value` host settings
/// (`default` for none), `FOCUS_DIAG_PATTERNS` of `lr`, `rl`, `tb`, `row1`,
/// `above2`, `below2`, `ramp`, `zero`, `ten`, `uN` (every block N), `hA-B`
/// (left A, right B; anything else is 5), or `>`-chained per frame,
/// `FOCUS_DIAG_GRAIN` 0 or 1, `FOCUS_DIAG_FRAMES` per arm, `FOCUS_DIAG_OUT`
/// a JSON file of every grid.
#[test]
#[ignore = "requires an idle AMD GPU and AMF"]
fn focus_native_diag() -> Result<()> {
    let env = |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.into());
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let grain = env("FOCUS_DIAG_GRAIN", "1") == "1";
    let frames: usize = env("FOCUS_DIAG_FRAMES", "3").parse()?;
    let mut rows = Vec::new();
    for case in env("FOCUS_DIAG_CASES", "1:1920x1080:20000").split(',') {
        let parts: Vec<&str> = case.split(':').collect();
        let codec: u8 = parts[0].parse()?;
        let (width, height) = parts[1].split_once('x').context("WxH")?;
        let (width, height): (u32, u32) = (width.parse()?, height.parse()?);
        let bitrate_kbps: u32 = parts[2].parse()?;
        let ten_bit = parts.get(3) == Some(&"10");
        for variant in env("FOCUS_DIAG_VARIANTS", "default").split('|') {
            let mut options = butterpollo_core::config::Config::default();
            for pair in variant.split(';').filter(|pair| pair.contains('=')) {
                let (key, value) = pair.split_once('=').unwrap();
                options.values.insert(key.into(), value.into());
            }
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                fps: 60,
                bitrate_kbps,
                codec,
                sdr_10bit: ten_bit,
                ..Default::default()
            };
            let mut encoder =
                Encoder::new_gpu(&config, device.clone(), &options, Some(compute.clone()))?;
            let names: &[&str] = match codec {
                0 => &[
                    "Usage",
                    "RateControlMethod",
                    "EnableVBAQ",
                    "QualityPreset",
                    "MinQP",
                    "MaxQP",
                    "EnablePreAnalysis",
                    "LowLatencyInternal",
                ],
                1 => &[
                    "HevcUsage",
                    "HevcRateControlMethod",
                    "HevcEnableVBAQ",
                    "HevcQualityPreset",
                    "HevcMinQP_P",
                    "HevcMaxQP_P",
                    "HevcEnablePreAnalysis",
                    "HevcMultiHwInstanceEncode",
                ],
                _ => &[
                    "Av1Usage",
                    "Av1RateControlMethod",
                    "Av1AQMode",
                    "Av1QualityPreset",
                    "Av1MinQIndex_Inter",
                    "Av1MaxQIndex_Inter",
                    "Av1EnablePreAnalysis",
                    "Av1MultiHwInstanceEncode",
                ],
            };
            let settings: Vec<String> = names
                .iter()
                .map(|name| format!("{name}={:?}", encoder.read(name)))
                .collect();
            let (cw, ch) = focus::coded_size((width, height));
            let (columns, lines) = butterpollo_core::encoder_focus::grid(codec, cw, ch);
            let pitch = focus::map_pitch(encoder.context_for_tests(), columns, lines)?;
            println!(
                "== codec {codec} {width}x{height} {bitrate_kbps} kbps ten_bit {ten_bit} [{variant}] map {columns}x{lines} pitch {pitch}\n   {}",
                settings.join(" ")
            );
            for pattern in env("FOCUS_DIAG_PATTERNS", "lr").split(',') {
                let map = |pattern: &str| focus::Grid {
                    width: columns,
                    height: lines,
                    values: (0..columns * lines)
                        .map(|index| {
                            let (x, y) = (index % columns, index / columns);
                            match pattern {
                                "lr" => u32::from(x < columns / 2) * 10,
                                "tb" => u32::from(y < lines / 2) * 10,
                                "zero" => 0,
                                "ten" => 10,
                                "ramp" => x % 11,
                                "below2" => u32::from(y >= 2) * 10,
                                "above2" => u32::from(y < 2) * 10,
                                "rl" => u32::from(x >= columns / 2) * 10,
                                "row1" => u32::from(y == 1) * 10,
                                // `uN`: every block at N; `hA-B`: left A, right B.
                                _ if pattern.starts_with('u') => pattern[1..].parse().unwrap_or(5),
                                _ if pattern.starts_with('h') => {
                                    let (left, right) =
                                        pattern[1..].split_once('-').unwrap_or(("5", "5"));
                                    let side = if x < columns / 2 { left } else { right };
                                    side.parse().unwrap_or(5)
                                }
                                _ => 5,
                            }
                        })
                        .collect(),
                };
                // `a>b>c`: one map per frame, the last repeated; `none` is no map.
                let steps: Vec<Option<focus::Grid>> = pattern
                    .split('>')
                    .map(|step| (step != "none").then(|| map(step)))
                    .collect();
                let mut maps = vec![None; frames];
                maps.extend((0..frames).map(|index| steps[index.min(steps.len() - 1)].clone()));
                let measured = encoder.focus_diag(4, &maps, grain)?;
                for (index, (grid, size)) in measured.iter().enumerate() {
                    let arm = if index < frames { "ctrl" } else { pattern };
                    match grid {
                        Some(grid) => {
                            let [l, r, t, b, min, max] = halves(grid);
                            let known: Vec<String> = grid
                                .values
                                .chunks(grid.width as usize)
                                .enumerate()
                                .filter_map(|(y, row)| {
                                    let row: Vec<u32> =
                                        row.iter().copied().filter(|&v| v != 0).collect();
                                    (!row.is_empty()).then(|| {
                                        format!(
                                            "{y}:{:.1}x{}",
                                            row.iter().sum::<u32>() as f64 / row.len() as f64,
                                            row.len()
                                        )
                                    })
                                })
                                .collect();
                            println!(
                                "   {arm:>4} L {l:6.2} R {r:6.2} T {t:6.2} B {b:6.2} min {min:3} max {max:3} bytes {size} grid {}x{} delta {:?} rows {}",
                                grid.width,
                                grid.height,
                                focus::half_delta(grid),
                                known.join(" ")
                            );
                            rows.push(serde_json::json!({
                                "codec": codec, "width": width, "height": height,
                                "kbps": bitrate_kbps, "ten_bit": ten_bit, "variant": variant,
                                "pattern": pattern, "arm": arm, "frame": index,
                                "left": l, "right": r, "top": t, "bottom": b,
                                "min": min, "max": max, "bytes": size,
                                "grid": grid.values,
                            }));
                        }
                        None => println!("   {arm:>4} no feedback, bytes {size}"),
                    }
                }
            }
        }
    }
    if let Some(path) = std::env::var_os("FOCUS_DIAG_OUT") {
        std::fs::write(&path, serde_json::to_string(&rows)?)?;
    }
    Ok(())
}
