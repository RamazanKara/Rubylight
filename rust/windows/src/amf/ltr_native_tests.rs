//! Opt-in native measurement of long-term reference recovery on H.264, HEVC and AV1
//! against the keyframe a client without it gets.
use super::intra_refresh_native_tests::scrolled;
use super::*;
use std::{
    fs,
    io::Write,
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
};

const WIDTH: usize = 2560;
const HEIGHT: usize = 1440;
const FRAMES: usize = 480;
/// The first frame the client doesn't show. From a gap of 12 on, the only
/// long-term reference older than the loss is the IDR; at 16, 32 and 130 the
/// repair lands 8 to 10 of the scrolling texture's 32 phases away from it,
/// not on a picture that repeats it.
const LOST: usize = 264;
const PHASES: usize = 32;

/// The scrolling texture held still, with one 64-pixel square crossing it
/// 32 px a frame: mostly static content, like a desktop and its pointer.
fn mostly_static(device: &Device, width: usize, height: usize, phase: usize) -> Result<GpuImage> {
    let mut bytes = vec![0; width * height * 4];
    let (left, top) = (256 + phase * 32, 400);
    for (y, row) in bytes.chunks_exact_mut(width * 4).enumerate() {
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let u = x % 512;
            let cell = (u / 16) ^ (y / 16);
            let texture = (u * 7 + y * 13 + (u * y) / 37) % 64;
            let square = (left..left + 64).contains(&x) && (top..top + 64).contains(&y);
            pixel.copy_from_slice(&if square {
                [255, 255, 255, 255]
            } else {
                [
                    (cell * 37 % 192 + texture) as u8,
                    (cell * 59 % 160 + texture * 2) as u8,
                    ((u + y) % 192 + texture / 2) as u8,
                    255,
                ]
            });
        }
    }
    let image = GpuImage::upload(
        device,
        &Image {
            width: width as u32,
            height: height as u32,
            stride: width * 4,
            bytes,
            captured: Instant::now(),
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    image.readback(&mut None)?;
    Ok(image)
}

/// Per-frame MD5s of one decode and its error output. Lossy streams are
/// decoded leniently, as a client's decoder would.
fn decode(
    ffmpeg: &std::ffi::OsStr,
    stream: &Path,
    format: &str,
    decoder: &str,
) -> Result<(Vec<String>, String)> {
    let label = decoder.replace(['/', '\\'], "_");
    let hashes = stream.with_extension(format!("{label}.md5"));
    let errors = stream.with_extension(format!("{label}.err"));
    let mut command = Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-flags",
        "low_delay",
    ]);
    match decoder {
        "sw" => {}
        "amf" => {
            command.args(["-c:v", &format!("{format}_amf")]);
        }
        hwaccel => {
            command.args(["-hwaccel", hwaccel]);
        }
    }
    let mut child = command
        .args(["-f", format, "-i"])
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
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{decoder} decoder timed out: {}", stream.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stderr = fs::read_to_string(errors)?;
    let md5s = fs::read_to_string(hashes)?
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.rsplit(',')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .collect();
    anyhow::ensure!(
        status.success() || !stderr.is_empty(),
        "{decoder} decoder failed silently on {}",
        stream.display()
    );
    Ok((md5s, stderr))
}

/// Recovery after `gap` frames the client never shows (lost, or received
/// after the loss and dropped until the recovery frame): the invalidated
/// frame, against the keyframe of a stream without long-term references
/// (`amd_ltr_frames=0`), at each gap, codec and content
/// (`BUTTERPOLLO_TEST_LTR_CONTENT`: `scrolling`, `static`). Each decoder in
/// `BUTTERPOLLO_TEST_LTR_DECODERS` (`sw`, `amf`, or an FFmpeg `-hwaccel` name
/// such as `d3d11va`) decodes the full stream (AMF's bytes, every frame) and
/// the lossy one (the bytes the client gets, without those frames); after an
/// accepted invalidation every decoder must show the full stream's pictures
/// from the recovery frame on, and a declined one must be a keyframe. The
/// host repairs what a decoder can still order (`ltr::Counter`: HEVC up to
/// `ORDER_WINDOW` frames after the last one shown) when its estimate makes
/// the repair the smaller frame; `BUTTERPOLLO_TEST_LTR_FORCE=1` repairs every
/// such gap, to measure what the estimate weighs. The streams stay beside the
/// report for decoders on other devices.
#[test]
#[ignore = "requires an idle AMD GPU, AMF, independent FFmpeg and a task-owned report path"]
fn native_ltr_recovery_after_lost_frames() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_LTR_REPORT")
            .context("set BUTTERPOLLO_TEST_LTR_REPORT to a task-owned JSON path")?,
    );
    anyhow::ensure!(report.is_absolute(), "report path must be absolute");
    fs::create_dir_all(report.parent().context("report directory")?)?;
    let ffmpeg =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let list = |name: &str, default: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| default.into())
            .split(',')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
    };
    let decoders = list("BUTTERPOLLO_TEST_LTR_DECODERS", "sw");
    let gaps = list("BUTTERPOLLO_TEST_LTR_GAPS", "1,2,4,8,16,32,130")
        .iter()
        .map(|gap| gap.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()?;
    let codecs = list("BUTTERPOLLO_TEST_LTR_CODECS", "0,1,2")
        .iter()
        .map(|codec| codec.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()?;
    let contents = list("BUTTERPOLLO_TEST_LTR_CONTENT", "scrolling");
    let forced = std::env::var("BUTTERPOLLO_TEST_LTR_FORCE").as_deref() == Ok("1");
    let without = butterpollo_core::config::Config::parse("amd_ltr_frames=0\n")?;
    let with = butterpollo_core::config::Config::default();
    anyhow::ensure!(
        gaps.iter().all(|&gap| gap >= 1 && LOST + gap < FRAMES - 30),
        "gaps must leave frames to compare"
    );
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let timer = crate::timing::Timer::new()?;
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    for (content, codec) in contents
        .iter()
        .flat_map(|content| codecs.iter().map(move |&codec| (content, codec)))
    {
        let pictures = (0..PHASES)
            .map(|phase| match content.as_str() {
                "scrolling" => scrolled(&device, WIDTH, HEIGHT, phase * 16),
                "static" => mostly_static(&device, WIDTH, HEIGHT, phase),
                other => bail!("content {other}: scrolling or static"),
            })
            .collect::<Result<Vec<_>>>()?;
        let (format, properties): (_, &[&str]) = match codec {
            0 => ("h264", &["MaxOfLTRFrames", "LTRMode", "MaxNumRefFrames"]),
            1 => (
                "hevc",
                &["HevcMaxOfLTRFrames", "HevcLTRMode", "HevcMaxNumRefFrames"],
            ),
            2 => ("obu", &["Av1MaxNumLTRFrames", "Av1LTRMode"]),
            _ => bail!("codec {codec}: H.264 (0), HEVC (1) or AV1 (2)"),
        };
        // (long-term references, gap): the keyframe, then the repair.
        let arms = gaps.iter().flat_map(|&gap| [(false, gap), (true, gap)]);
        for (asked, gap) in arms {
            let name = format!(
                "{content}-{format}-{}-gap{gap}",
                if asked { "ltr" } else { "keyframe" }
            );
            let config = butterpollo_core::rtsp::Negotiated {
                width: WIDTH as u32,
                height: HEIGHT as u32,
                fps: 120,
                rate_millihz: 120_000,
                bitrate_kbps: 50_000,
                codec,
                ltr_recovery: asked,
                ..Default::default()
            };
            let mut encoder = Encoder::new_gpu(
                &config,
                device.clone(),
                if asked { &with } else { &without },
                Some(compute.clone()),
            )?;
            if forced {
                encoder.references.repair_whatever_it_costs();
            }
            // The full stream is AMF's own: renumbered for a client that
            // skipped the lost frames, it would not decode with them in it.
            encoder.unrewritten = Some(Vec::new());
            // Only a stream with long-term references rewrites AMF's headers.
            anyhow::ensure!(
                encoder.repair_headers.is_some() == (asked && codec <= 1),
                "{name}: header rewrite {}",
                if asked { "missing" } else { "without long-term references" }
            );
            let read: serde_json::Map<_, _> = properties
                .iter()
                .map(|&property| (property.into(), serde_json::json!(encoder.read(property))))
                .collect();
            let supported = encoder.supports_invalidation();
            anyhow::ensure!(
                supported == asked,
                "{name}: long-term references {} ({read:?})",
                if asked {
                    "missing"
                } else {
                    "with amd_ltr_frames=0"
                }
            );
            let recovery = LOST + gap;
            let full_path = report.with_extension(format!("{name}.{format}"));
            let lossy_path = report.with_extension(format!("{name}.lossy.{format}"));
            let mut full = std::io::BufWriter::new(fs::File::create(&full_path)?);
            let mut lossy = std::io::BufWriter::new(fs::File::create(&lossy_path)?);
            let start = Instant::now();
            let period = Duration::from_secs_f64(1. / 120.);
            let mut frames = Vec::new();
            let mut accepted = None;
            for index in 0..FRAMES {
                timer.until(start + period * index as u32);
                let mut image = pictures[index % PHASES].clone();
                image.captured = Instant::now();
                let deadline = image.captured + Duration::from_secs(2);
                let mut idr = index == 0;
                if index == recovery {
                    // The client names the frames it couldn't show; wire
                    // frame numbers start at one.
                    accepted = Some(
                        asked && encoder.invalidate_ref_frames(LOST as u64 + 1, recovery as u64),
                    );
                    idr = accepted == Some(false);
                }
                let mut output = encoder.encode_gpu(&image, idr, config.bitrate_kbps)?;
                while encoder.pending() {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "AMF stalled at {name} frame {index}"
                    );
                    output.extend(encoder.poll()?);
                }
                anyhow::ensure!(output.len() == 1, "missing/late {name} output {index}");
                let frame = output.pop().unwrap();
                anyhow::ensure!(!idr || frame.idr, "forced IDR absent at {name} {index}");
                let unrewritten = encoder.unrewritten.replace(Vec::new()).unwrap_or_default();
                anyhow::ensure!(
                    !unrewritten.is_empty(),
                    "{name}: AMF's bytes missing at {index}"
                );
                full.write_all(&unrewritten)?;
                if !(LOST..recovery).contains(&index) {
                    lossy.write_all(&frame.bytes)?;
                }
                frames.push((frame.idr, frame.bytes.len(), frame.after_invalidation));
            }
            full.flush()?;
            lossy.flush()?;
            drop(full);
            drop(lossy);
            drop(encoder);
            let accepted = accepted == Some(true);
            // Frame LOST - 1 is the last shown: HEVC orders a repair up to
            // ORDER_WINDOW frames on, H.264 and AV1 at any distance.
            let repairable = asked
                && (codec != 1
                    || recovery - (LOST - 1)
                        <= butterpollo_core::bitstream::ORDER_WINDOW as usize);
            let keyframes: Vec<usize> = frames
                .iter()
                .filter(|frame| frame.0)
                .map(|frame| frame.1)
                .collect();
            let inter: Vec<usize> = frames
                .iter()
                .filter(|frame| !frame.0)
                .map(|frame| frame.1)
                .collect();
            let mut decoded = serde_json::Map::new();
            let mut healthy = true;
            for decoder in &decoders {
                let (clean, clean_errors) = decode(&ffmpeg, &full_path, format, decoder)?;
                let (damaged, damaged_errors) = decode(&ffmpeg, &lossy_path, format, decoder)?;
                // Decoded lossy frame j is source frame j, or j + gap from the loss on.
                let source = |j: usize| if j < LOST { j } else { j + gap };
                let complete = damaged.len() == FRAMES - gap;
                let compared: Vec<bool> = (LOST..damaged.len())
                    .map(|j| clean.get(source(j)) == damaged.get(j))
                    .collect();
                let first_mismatch = compared.iter().position(|matched| !matched);
                let before = (0..LOST).all(|j| clean.get(j) == damaged.get(j));
                let recovered = complete && before && first_mismatch.is_none();
                healthy &= clean.len() == FRAMES && recovered;
                decoded.insert(
                    decoder.clone(),
                    serde_json::json!({
                        "clean_frames": clean.len(),
                        "clean_error_lines": clean_errors.lines().count(),
                        "lossy_frames": damaged.len(), "lossy_frames_expected": FRAMES - gap,
                        "lossy_error_lines": damaged_errors.lines().count(),
                        "lossy_first_errors": damaged_errors.lines().take(3).collect::<Vec<_>>(),
                        "identical_from_recovery": recovered,
                        "first_mismatch_after_recovery": first_mismatch,
                        "frames_after_recovery_identical":
                            compared.iter().filter(|matched| **matched).count(),
                    }),
                );
            }
            let result = serde_json::json!({
                "variant": name, "codec": codec, "opted_in": asked,
                "adapter": device.display.adapter, "width": WIDTH, "height": HEIGHT,
                "fps": 120, "bitrate_kbps": config.bitrate_kbps, "frames": FRAMES,
                "first_frame_not_shown": LOST, "frames_not_shown": gap, "recovery_frame": recovery,
                "source": if content == "static" {
                    "still synthetic texture, a 64 px square moving 32 px a frame; no capture/display/network"
                } else {
                    "scrolling synthetic texture, 16 px a frame; no capture/display/network"
                },
                "content": content, "forced": forced,
                "properties": read, "invalidation_accepted": accepted,
                "invalidation_expected": repairable,
                "recovery_frame_idr": frames[recovery].0, "recovery_frame_bytes": frames[recovery].1,
                "recovery_after_invalidation": frames[recovery].2,
                "keyframes": keyframes.len(), "keyframe_bytes": keyframes,
                "max_frame_bytes_after_first": frames.iter().skip(1).map(|frame| frame.1).max(),
                "mean_inter_frame_bytes": inter.iter().sum::<usize>() / inter.len().max(1),
                "decoders": decoded,
            });
            println!("{name}: {result}");
            let answered = if accepted {
                !frames[recovery].0 && frames[recovery].2
            } else {
                frames[recovery].0
            };
            // Forced, every repairable gap is repaired; otherwise the
            // estimate may answer with a keyframe, but never repair beyond.
            let expected = if forced {
                accepted == repairable
            } else {
                repairable || !accepted
            };
            if !answered || !healthy || !expected {
                failures.push(format!(
                    "{name}: invalidation accepted {accepted} (expected {repairable}), recovery keyframe {}, decoders {}",
                    frames[recovery].0,
                    serde_json::Value::Object(decoded)
                ));
            }
            reports.push(result);
            fs::write(&report, serde_json::to_vec_pretty(&reports)?)?;
        }
    }
    anyhow::ensure!(failures.is_empty(), "{failures:#?}");
    Ok(())
}
