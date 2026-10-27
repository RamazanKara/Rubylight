//! Opt-in native measurement of long-term reference recovery on H.264 and HEVC, the
//! Experimental `rlLtrRecovery` path, against the keyframe a stock client gets.
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
const FRAMES: usize = 360;
/// The first frame the client doesn't show. Frame 240, the last it shows,
/// starts a 16-frame period of the encoder's four-bit H.264 frame number.
const LOST: usize = 241;
const PHASES: usize = 32;

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
/// after the loss and dropped until the recovery frame): the invalidated frame
/// an opted-in client gets, against the keyframe of a stock client. Each
/// decoder in `BUTTERPOLLO_TEST_LTR_DECODERS` (`sw`, `amf`, or an FFmpeg
/// `-hwaccel` name such as `d3d11va`) decodes the full and the lossy stream;
/// after an accepted invalidation every decoder must show the full stream's
/// pictures from the recovery frame on, and a declined one must be a
/// keyframe. The streams stay beside the report for decoders on other devices.
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
    let gaps = list("BUTTERPOLLO_TEST_LTR_GAPS", "1,3,7,8,11,15")
        .iter()
        .map(|gap| gap.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()?;
    let codecs = list("BUTTERPOLLO_TEST_LTR_CODECS", "0,1")
        .iter()
        .map(|codec| codec.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        gaps.iter().all(|&gap| gap >= 1 && LOST + gap < FRAMES - 30),
        "gaps must leave frames to compare"
    );
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let timer = crate::timing::Timer::new()?;
    let pictures = (0..PHASES)
        .map(|phase| scrolled(&device, WIDTH, HEIGHT, phase * 16))
        .collect::<Result<Vec<_>>>()?;
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    for &codec in &codecs {
        let (format, properties) = match codec {
            0 => ("h264", ["MaxOfLTRFrames", "LTRMode", "MaxNumRefFrames"]),
            1 => (
                "hevc",
                ["HevcMaxOfLTRFrames", "HevcLTRMode", "HevcMaxNumRefFrames"],
            ),
            _ => bail!("codec {codec}: H.264 (0) or HEVC (1)"),
        };
        // (opted in, gap): the stock client's keyframe once, then each gap.
        let arms = std::iter::once((false, gaps[0])).chain(gaps.iter().map(|&gap| (true, gap)));
        for (asked, gap) in arms {
            let name = format!(
                "{format}-{}-gap{gap}",
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
                &Default::default(),
                Some(compute.clone()),
            )?;
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
                    "without the opt-in"
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
                full.write_all(&frame.bytes)?;
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
                "source": "scrolling synthetic texture, 16 px a frame; no capture/display/network",
                "properties": read, "invalidation_accepted": accepted,
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
            if !answered || !healthy {
                failures.push(format!(
                    "{name}: invalidation accepted {accepted}, recovery keyframe {}, decoders {}",
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
