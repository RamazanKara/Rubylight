//! Dynamic HDR metadata spliced into real encoded frames, read back by FFmpeg.
//!
//! Needs ffmpeg with libx265 and libaom plus ffprobe with an HEVC and an AV1 decoder:
//! `BUTTERPOLLO_FFMPEG_DIR`, else `C:\msys64\ucrt64\bin`, else PATH. Run with
//! `cargo test -p butterpollo-core --test hdr_dynamic_ffprobe -- --ignored`.
use butterpollo_core::hdr_dynamic::{self, Codec, FrameStats, Hdr10Plus, Vivid};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(name: &str) -> PathBuf {
    let dirs = [
        std::env::var_os("BUTTERPOLLO_FFMPEG_DIR").map(PathBuf::from),
        Some(PathBuf::from(r"C:\msys64\ucrt64\bin")),
    ];
    dirs.into_iter()
        .flatten()
        .map(|dir| dir.join(format!("{name}.exe")))
        .find(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn encode(codec: Codec, path: &Path) {
    let (encoder, format): (&[&str], &str) = match codec {
        Codec::Hevc => (
            &["-c:v", "libx265", "-x265-params", "log-level=error"],
            "hevc",
        ),
        Codec::Av1 => (&["-c:v", "libaom-av1", "-cpu-used", "8"], "obu"),
    };
    let status = Command::new(tool("ffmpeg"))
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "testsrc2=size=64x64:rate=10", "-frames:v", "1"])
        .args(["-pix_fmt", "yuv420p10le"])
        .args(encoder)
        .args(["-f", format])
        .arg(path)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg encode failed");
}

/// Key-value lines of one side data entry, in ffprobe's order (repeated keys kept, which
/// its JSON writer would collapse).
type Entry = Vec<(String, String)>;

fn side_data(path: &Path) -> Vec<Entry> {
    let output = Command::new(tool("ffprobe"))
        .args(["-hide_banner", "-loglevel", "error", "-show_frames"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() && errors.is_empty(), "{errors}");
    let text = String::from_utf8(output.stdout).unwrap();
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    for line in text.lines().map(str::trim) {
        match line {
            "[SIDE_DATA]" => current = Some(Vec::new()),
            "[/SIDE_DATA]" => entries.extend(current.take()),
            _ => {
                if let (Some(entry), Some((key, value))) = (&mut current, line.split_once('=')) {
                    entry.push((key.into(), value.into()));
                }
            }
        }
    }
    entries
}

fn find<'a>(side_data: &'a [Entry], kind: &str) -> &'a Entry {
    side_data
        .iter()
        .find(|entry| {
            entry
                .iter()
                .any(|(key, value)| key == "side_data_type" && value == kind)
        })
        .unwrap_or_else(|| panic!("no {kind} in {side_data:#?}"))
}

fn values<'a>(entry: &'a Entry, key: &str) -> Vec<&'a str> {
    entry
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect()
}

fn stats() -> FrameStats {
    let nits = [0.5, 2., 5., 20., 80., 200., 600., 900., 1500.];
    FrameStats {
        min_pq: hdr_dynamic::nits_to_pq(0.05),
        mean_pq: hdr_dynamic::nits_to_pq(60.),
        max_pq: hdr_dynamic::nits_to_pq(4000.),
        mean_nits: 180.,
        percentiles_pq: nits.map(hdr_dynamic::nits_to_pq),
    }
}

fn spliced(codec: Codec, name: &str) -> Vec<Entry> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let (plain, tagged) = (dir.join(format!("{name}.plain")), dir.join(name));
    encode(codec, &plain);
    let mut stream = std::fs::read(&plain).unwrap();
    let units = hdr_dynamic::frame_units(codec, &stats(), 1000).unwrap();
    assert!(hdr_dynamic::splice(codec, &mut stream, &units));
    std::fs::write(&tagged, &stream).unwrap();
    side_data(&tagged)
}

const HDR10PLUS: &str = "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)";

fn check_hdr10plus(side_data: &[Entry]) {
    let expected = Hdr10Plus::from_stats(&stats(), 1000).unwrap();
    let hdr10plus = find(side_data, HDR10PLUS);
    println!("{hdr10plus:?}");
    assert_eq!(values(hdr10plus, "application version"), ["1"]);
    assert_eq!(values(hdr10plus, "num_windows"), ["1"]);
    assert_eq!(
        values(hdr10plus, "targeted_system_display_maximum_luminance"),
        ["1000/1"]
    );
    let maxscl = format!("{}/100000", expected.maxscl);
    assert_eq!(values(hdr10plus, "maxscl"), [maxscl.as_str(); 3]);
    let average = format!("{}/100000", expected.average_maxrgb);
    assert_eq!(values(hdr10plus, "average_maxrgb"), [average.as_str()]);
    assert_eq!(
        values(hdr10plus, "num_distribution_maxrgb_percentiles"),
        ["9"]
    );
    let percentages = hdr_dynamic::PERCENTAGES.map(|percentage| percentage.to_string());
    assert_eq!(
        values(hdr10plus, "distribution_maxrgb_percentage"),
        percentages
    );
    let percentiles = expected.distribution.map(|value| format!("{value}/100000"));
    assert_eq!(
        values(hdr10plus, "distribution_maxrgb_percentile"),
        percentiles
    );
    assert_eq!(values(hdr10plus, "fraction_bright_pixels"), ["0/1000"]);
}

#[test]
#[ignore = "needs ffmpeg and ffprobe"]
fn ffprobe_reads_hdr10plus_and_vivid_from_hevc() {
    let side_data = spliced(Codec::Hevc, "hdr-dynamic.hevc");
    check_hdr10plus(&side_data);
    let expected = Vivid::from_stats(&stats()).unwrap();
    let vivid = find(&side_data, "HDR Dynamic Metadata CUVA 005.1 2021 (Vivid)");
    println!("{vivid:?}");
    assert_eq!(values(vivid, "system_start_code"), ["1"]);
    assert_eq!(values(vivid, "num_windows"), ["1"]);
    for (key, code) in [
        ("minimum_maxrgb", expected.minimum_maxrgb),
        ("average_maxrgb", expected.average_maxrgb),
        ("variance_maxrgb", expected.variance_maxrgb),
        ("maximum_maxrgb", expected.maximum_maxrgb),
    ] {
        assert_eq!(values(vivid, key), [format!("{code}/4095")], "{key}");
    }
    assert_eq!(values(vivid, "tone_mapping_mode_flag"), ["0"]);
}

#[test]
#[ignore = "needs ffmpeg and ffprobe"]
fn ffprobe_reads_hdr10plus_from_av1() {
    let side_data = spliced(Codec::Av1, "hdr-dynamic.obu");
    check_hdr10plus(&side_data);
    assert!(
        !format!("{side_data:?}").contains("Vivid"),
        "Vivid is HEVC-only"
    );
}
