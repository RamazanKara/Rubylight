//! Stream DVR clips and recordings read back by FFmpeg: short H.264, HEVC (SDR and 10-bit
//! HDR10) and AV1 encodes with stereo and 5.1 Opus, split into access units and packets,
//! pushed through the replay ring and the recorder, muxed, then probed and decoded.
//!
//! Needs ffmpeg with libx264, libx265, libaom-av1 and libopus plus ffprobe:
//! `BUTTERPOLLO_FFMPEG_DIR`, else `C:\msys64\ucrt64\bin`, else PATH. Run with
//! `cargo test -p butterpollo-media-policy --test dvr_ffprobe -- --ignored`.
use butterpollo_media_policy::dvr::{
    AudioPacket, HdrInfo, Limits, OpusConfig, Pushed, RecPush, Recorder, Ring, StreamInfo,
    VideoCodec, VideoFrame, salvage, write_clip,
};
use butterpollo_media_policy::hdr::Metadata;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const START_US: u64 = 5_000_000;
const FPS: u64 = 30;
const FRAMES: u64 = 90;

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

fn ffmpeg(args: &[&str], out: &Path) -> Vec<u8> {
    let status = Command::new(tool("ffmpeg"))
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg {args:?} failed");
    std::fs::read(out).unwrap()
}

#[derive(Clone, Copy)]
enum Video {
    H264,
    Hevc,
    HevcHdr10,
    Av1,
}

const MASTER_DISPLAY: &str =
    "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50)";

/// Three seconds of test pattern, a keyframe every 15 frames, no B-frames.
fn encode_video(dir: &Path, video: Video) -> Vec<u8> {
    let x265 = "keyint=15:min-keyint=15:scenecut=0:bframes=0:open-gop=0:aud=1:\
                repeat-headers=1:log-level=error";
    let hdr10 = format!(
        "{x265}:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:\
         master-display={MASTER_DISPLAY}:max-cll=900,400"
    );
    let (size, pix_fmt, codec, format): (&str, &str, Vec<&str>, &str) = match video {
        Video::H264 => (
            "200x120",
            "yuv420p",
            vec![
                "-c:v",
                "libx264",
                "-x264-params",
                "keyint=15:min-keyint=15:scenecut=0:bframes=0:aud=1:log-level=error",
            ],
            "h264",
        ),
        Video::Hevc => (
            "202x118",
            "yuv420p",
            vec!["-c:v", "libx265", "-x265-params", x265],
            "hevc",
        ),
        Video::HevcHdr10 => (
            "202x118",
            "yuv420p10le",
            vec!["-c:v", "libx265", "-x265-params", hdr10.as_str()],
            "hevc",
        ),
        Video::Av1 => (
            "200x120",
            "yuv420p10le",
            vec![
                "-c:v",
                "libaom-av1",
                "-usage",
                "realtime",
                "-cpu-used",
                "8",
                "-lag-in-frames",
                "0",
                "-g",
                "15",
                "-keyint_min",
                "15",
            ],
            "obu",
        ),
    };
    let source = format!("testsrc2=size={size}:rate={FPS}");
    let frames = FRAMES.to_string();
    let mut args = vec![
        "-f",
        "lavfi",
        "-i",
        source.as_str(),
        "-frames:v",
        frames.as_str(),
        "-pix_fmt",
        pix_fmt,
    ];
    args.extend(codec);
    args.extend(["-f", format]);
    ffmpeg(&args, &dir.join(format!("video.{format}")))
}

/// Access units: Annex-B split before each AUD, or OBUs split before each temporal
/// delimiter; with whether each is a keyframe.
fn access_units(video: Video, stream: &[u8]) -> Vec<(bool, Vec<u8>)> {
    let units: Vec<(bool, Vec<u8>)> = match video {
        Video::Av1 => split_obus(stream),
        _ => split_annexb(stream, matches!(video, Video::H264)),
    };
    assert_eq!(units.len() as u64, FRAMES);
    assert!(units[0].0, "starts on a keyframe");
    assert!(units.iter().filter(|unit| unit.0).count() >= 3);
    units
}

fn split_annexb(stream: &[u8], h264: bool) -> Vec<(bool, Vec<u8>)> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at + 3 < stream.len() {
        if stream[at..at + 3] == [0, 0, 1] {
            let kind = if h264 {
                stream[at + 3] & 0x1f
            } else {
                (stream[at + 3] >> 1) & 0x3f
            };
            starts.push((at, kind));
            at += 3;
        } else {
            at += 1;
        }
    }
    let aud = if h264 { 9 } else { 35 };
    let cuts: Vec<usize> = starts
        .iter()
        .filter(|&&(_, kind)| kind == aud)
        .map(|&(at, _)| {
            if at > 0 && stream[at - 1] == 0 {
                at - 1
            } else {
                at
            }
        })
        .collect();
    assert_eq!(cuts.first(), Some(&0), "AUD first");
    cuts.iter()
        .zip(cuts[1..].iter().chain([&stream.len()]))
        .map(|(&from, &to)| {
            let kinds = starts.iter().filter(|&&(at, _)| at >= from && at < to);
            let idr = kinds.map(|&(_, kind)| kind).any(|kind| {
                if h264 {
                    kind == 5
                } else {
                    kind == 19 || kind == 20
                }
            });
            (idr, stream[from..to].to_vec())
        })
        .collect()
}

fn leb128(data: &[u8]) -> (usize, usize) {
    let (mut value, mut used) = (0usize, 0usize);
    for byte in data {
        value |= usize::from(byte & 0x7f) << (7 * used);
        used += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
    (value, used)
}

fn split_obus(stream: &[u8]) -> Vec<(bool, Vec<u8>)> {
    let mut units: Vec<(bool, Vec<u8>)> = Vec::new();
    let mut at = 0;
    while at < stream.len() {
        let header = stream[at];
        let kind = (header >> 3) & 0x0f;
        let extension = usize::from(header & 0x04 != 0);
        assert!(header & 0x02 != 0, "ffmpeg writes sized OBUs");
        let (size, used) = leb128(&stream[at + 1 + extension..]);
        let payload = at + 1 + extension + used;
        let end = payload + size;
        if kind == 2 {
            units.push((false, Vec::new()));
        }
        let unit = units.last_mut().expect("a temporal delimiter first");
        // FRAME or FRAME_HEADER: show_existing_frame 0 and frame_type KEY_FRAME (0).
        if matches!(kind, 3 | 6) && stream[payload] & 0xe0 == 0 {
            unit.0 = true;
        }
        unit.1.extend_from_slice(&stream[at..end]);
        at = end;
    }
    units
}

/// Ogg packets (lacing joined) of an Ogg Opus file.
fn ogg_packets(data: &[u8]) -> Vec<Vec<u8>> {
    let (mut packets, mut current, mut at) = (Vec::new(), Vec::new(), 0);
    while at + 27 <= data.len() {
        assert_eq!(&data[at..at + 4], b"OggS");
        let segments = usize::from(data[at + 26]);
        let table = &data[at + 27..at + 27 + segments];
        let mut body = at + 27 + segments;
        for &lace in table {
            current.extend_from_slice(&data[body..body + usize::from(lace)]);
            body += usize::from(lace);
            if lace < 255 {
                packets.push(std::mem::take(&mut current));
            }
        }
        at = body;
    }
    packets
}

/// Samples at 48 kHz of one Opus packet, from its TOC byte.
fn opus_samples(packet: &[u8]) -> u32 {
    let config = packet[0] >> 3;
    let frame = match config {
        0..=11 => [480, 960, 1920, 2880][usize::from(config % 4)],
        12..=15 => [480, 960][usize::from(config % 2)],
        _ => [120, 240, 480, 960][usize::from(config % 4)],
    };
    let frames = match packet[0] & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(packet[1] & 0x3f),
    };
    frame * frames
}

/// WAVE input channel of each Vorbis-order output channel.
fn vorbis_order(channels: usize) -> Vec<usize> {
    match channels {
        1 => vec![0],
        2 => vec![0, 1],
        6 => vec![0, 2, 1, 4, 5, 3],
        _ => panic!("no test layout for {channels} channels"),
    }
}

/// Three seconds of Opus in 10 ms packets; the layout as the host states it (WAVE order).
fn encode_opus(dir: &Path, channels: u8) -> (OpusConfig, Vec<AudioPacket>) {
    let source = if channels == 6 {
        "aevalsrc=0.1*sin(440*2*PI*t)|0.1*sin(550*2*PI*t)|0.1*sin(660*2*PI*t)|\
         0.1*sin(110*2*PI*t)|0.1*sin(770*2*PI*t)|0.1*sin(880*2*PI*t):s=48000:d=3:c=5.1"
    } else {
        "sine=frequency=440:sample_rate=48000:duration=3"
    };
    let channel_count = channels.to_string();
    let args = [
        "-f",
        "lavfi",
        "-i",
        source,
        "-ac",
        channel_count.as_str(),
        "-c:a",
        "libopus",
        "-frame_duration",
        "10",
        "-b:a",
        "128k",
        "-f",
        "ogg",
    ];
    let data = ffmpeg(&args, &dir.join("audio.opus"));
    let packets = ogg_packets(&data);
    let head = &packets[0];
    assert_eq!(&head[..8], b"OpusHead");
    assert_eq!(head[9], channels);
    let config = if head[18] == 0 {
        OpusConfig::stereo()
    } else {
        let vorbis = &head[21..21 + usize::from(channels)];
        let mut wave = vec![0u8; usize::from(channels)];
        for (position, &input) in vorbis_order(usize::from(channels)).iter().enumerate() {
            wave[input] = vorbis[position];
        }
        OpusConfig {
            channels,
            streams: head[19],
            coupled: head[20],
            mapping: wave,
        }
    };
    let mut pts = START_US;
    let mut samples_total = 0u64;
    let audio = packets[2..]
        .iter()
        .map(|data| {
            let samples = opus_samples(data);
            let packet = AudioPacket::new(pts, samples, data.clone());
            samples_total += u64::from(samples);
            pts = START_US + samples_total * 1_000_000 / 48_000;
            packet
        })
        .collect();
    (config, audio)
}

fn video_frames(units: Vec<(bool, Vec<u8>)>) -> Vec<VideoFrame> {
    units
        .into_iter()
        .enumerate()
        .map(|(index, (idr, data))| {
            VideoFrame::new(START_US + index as u64 * 1_000_000 / FPS, idr, data)
        })
        .collect()
}

enum Event {
    Video(VideoFrame),
    Audio(AudioPacket),
}

/// Video and audio in timestamp order, as the taps deliver them.
fn interleave(video: Vec<VideoFrame>, audio: Vec<AudioPacket>) -> Vec<Event> {
    let mut events: Vec<(u64, Event)> = video
        .into_iter()
        .map(|frame| (frame.pts_us, Event::Video(frame)))
        .chain(
            audio
                .into_iter()
                .map(|packet| (packet.pts_us, Event::Audio(packet))),
        )
        .collect();
    events.sort_by_key(|(pts, _)| *pts);
    events.into_iter().map(|(_, event)| event).collect()
}

/// `ffprobe` JSON (streams with counted frames, format); asserts it printed no errors.
fn probe(path: &Path) -> Value {
    let output = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && errors.is_empty(),
        "ffprobe: {errors}"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Decodes every frame; asserts FFmpeg printed no errors.
fn decode(path: &Path) {
    let output = Command::new(tool("ffmpeg"))
        .args(["-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .expect("ffmpeg runs");
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && errors.is_empty(),
        "ffmpeg decode: {errors}"
    );
}

fn number(value: &Value) -> f64 {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .unwrap_or(f64::NAN)
}

struct Expect<'a> {
    codec: &'a str,
    size: (u64, u64),
    frames: usize,
    audio_channels: Option<u64>,
    duration_ms: u64,
}

/// Probes and decodes `bytes`; returns the video stream for codec-specific checks.
fn check(dir: &Path, name: &str, bytes: &[u8], expect: &Expect<'_>) -> Value {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    let probe = probe(&path);
    println!("{name}: {probe}");
    decode(&path);
    let streams = probe["streams"].as_array().unwrap();
    assert_eq!(
        streams.len(),
        1 + usize::from(expect.audio_channels.is_some())
    );
    let video = &streams[0];
    assert_eq!(video["codec_type"], "video");
    assert_eq!(video["codec_name"], expect.codec);
    assert_eq!(
        (video["width"].as_u64(), video["height"].as_u64()),
        (Some(expect.size.0), Some(expect.size.1))
    );
    assert_eq!(number(&video["nb_read_frames"]) as usize, expect.frames);
    if let Some(channels) = expect.audio_channels {
        let audio = &streams[1];
        assert_eq!(audio["codec_name"], "opus");
        assert_eq!(audio["channels"].as_u64(), Some(channels));
        let skew = (number(&audio["start_time"]) - number(&video["start_time"])).abs();
        assert!(skew <= 0.020, "A/V start skew {skew} s");
    }
    let duration = number(&probe["format"]["duration"]);
    let expected = expect.duration_ms as f64 / 1000.0;
    assert!(
        (duration - expected).abs() <= 0.1,
        "duration {duration} s, expected {expected} s"
    );
    video.clone()
}

fn info(codec: VideoCodec, hdr: Option<HdrInfo>, audio: Option<OpusConfig>) -> StreamInfo {
    StreamInfo { codec, hdr, audio }
}

/// Pushes everything through a replay ring and saves its last two seconds.
fn ring_clip(
    dir: &Path,
    name: &str,
    video: Video,
    info: StreamInfo,
    audio: Vec<AudioPacket>,
) -> Value {
    let frames = video_frames(access_units(video, &encode_video(dir, video)));
    let mut ring = Ring::new(Limits {
        max_us: 60_000_000,
        max_bytes: 512 << 20,
    });
    for event in interleave(frames, audio) {
        match event {
            Event::Video(frame) => assert_eq!(ring.push_video(&info, frame), Pushed::Stored),
            Event::Audio(packet) => ring.push_audio(packet),
        }
    }
    let clip = ring.snapshot(2_000_000).unwrap();
    assert!(
        clip.video[0].pts_us > START_US,
        "the window starts mid-stream"
    );
    let mut bytes = Vec::new();
    let written = write_clip(&mut bytes, &clip).unwrap();
    let expect = Expect {
        codec: match info.codec {
            VideoCodec::H264 => "h264",
            VideoCodec::Hevc => "hevc",
            VideoCodec::Av1 => "av1",
        },
        size: (u64::from(written.width), u64::from(written.height)),
        frames: clip.video.len(),
        audio_channels: info.audio.as_ref().map(|config| u64::from(config.channels)),
        duration_ms: written.duration_ms,
    };
    check(dir, name, &bytes, &expect)
}

#[test]
#[ignore = "needs ffmpeg and ffprobe"]
fn ring_clip_h264_with_stereo_opus() {
    let dir = tempfile::tempdir().unwrap();
    let (config, audio) = encode_opus(dir.path(), 2);
    let info = info(VideoCodec::H264, None, Some(config));
    let video = ring_clip(dir.path(), "h264.mp4", Video::H264, info, audio);
    assert_eq!(
        (video["width"].as_u64(), video["height"].as_u64()),
        (Some(200), Some(120))
    );
    assert_eq!(video["color_primaries"], "bt709");
}

#[test]
#[ignore = "needs ffmpeg and ffprobe"]
fn ring_clip_hevc_hdr10_carries_colour_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let metadata = Metadata {
        primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
        white: [15635, 16450],
        maximum_nits: 1000,
        minimum: 50,
        max_cll: 900,
        max_fall: 400,
        full_frame_nits: 400,
    };
    let hdr = HdrInfo {
        metadata: Some(metadata),
    };
    let info = info(VideoCodec::Hevc, Some(hdr), None);
    let video = ring_clip(
        dir.path(),
        "hevc-hdr10.mp4",
        Video::HevcHdr10,
        info,
        Vec::new(),
    );
    assert_eq!(
        (video["width"].as_u64(), video["height"].as_u64()),
        (Some(202), Some(118))
    );
    assert_eq!(video["pix_fmt"], "yuv420p10le");
    assert_eq!(video["color_primaries"], "bt2020");
    assert_eq!(video["color_transfer"], "smpte2084");
    assert_eq!(video["color_space"], "bt2020nc");
    let side_data = video["side_data_list"].to_string();
    assert!(
        side_data.contains("Mastering display metadata"),
        "{side_data}"
    );
    assert!(
        side_data.contains("Content light level metadata"),
        "{side_data}"
    );
    assert!(side_data.contains("\"max_content\":900"), "{side_data}");
}

#[test]
#[ignore = "needs ffmpeg and ffprobe"]
fn ring_clip_av1_with_surround_opus() {
    let dir = tempfile::tempdir().unwrap();
    let (config, audio) = encode_opus(dir.path(), 6);
    assert_eq!(config.channels, 6);
    let info = info(VideoCodec::Av1, None, Some(config));
    let video = ring_clip(dir.path(), "av1.mp4", Video::Av1, info, audio);
    assert_eq!(
        (video["width"].as_u64(), video["height"].as_u64()),
        (Some(200), Some(120))
    );
}

#[test]
#[ignore = "needs ffmpeg and ffprobe"]
fn recording_hevc_with_stereo_opus_and_its_salvage() {
    let dir = tempfile::tempdir().unwrap();
    let (config, audio) = encode_opus(dir.path(), 2);
    let info = info(VideoCodec::Hevc, None, Some(config));
    let frames = video_frames(access_units(
        Video::Hevc,
        &encode_video(dir.path(), Video::Hevc),
    ));
    let mut events = interleave(frames, audio).into_iter();
    let first = loop {
        match events.next() {
            Some(Event::Video(frame)) => break frame,
            Some(Event::Audio(_)) => {}
            None => panic!("no video"),
        }
    };
    let (mut recorder, mut file) = Recorder::start(&info, first).unwrap();
    let mut fragments = 0;
    for event in events {
        match event {
            Event::Video(frame) => match recorder.push_video(&info, frame) {
                RecPush::Buffered => {}
                RecPush::Fragment(bytes) => {
                    file.extend_from_slice(&bytes);
                    fragments += 1;
                }
                other => panic!("unexpected {other:?}"),
            },
            Event::Audio(packet) => recorder.push_audio(packet),
        }
    }
    assert_eq!(fragments, 1, "frames 0..60, cut at the frame at 2 s");
    let finished = recorder.finish();
    file.extend_from_slice(&finished.tail);
    // Torn inside the tail fragment.
    let torn = file[..file.len() - finished.tail.len() / 2].to_vec();
    let (at, patch) = finished.duration_patch.unwrap();
    let mut complete = file.clone();
    complete[at as usize..at as usize + patch.len()].copy_from_slice(&patch);
    let expect = Expect {
        codec: "hevc",
        size: (202, 118),
        frames: FRAMES as usize,
        audio_channels: Some(2),
        duration_ms: finished.info.duration_ms,
    };
    check(dir.path(), "recording.mp4", &complete, &expect);

    let found = salvage(&mut std::io::Cursor::new(&torn)).unwrap();
    assert_eq!(found.fragments, 1);
    let mut recovered = torn[..found.valid_len as usize].to_vec();
    let (at, patch) = found.mehd_patch.unwrap();
    recovered[at as usize..at as usize + patch.len()].copy_from_slice(&patch);
    let salvaged = Expect {
        frames: 60,
        duration_ms: found.duration_us / 1000,
        ..expect
    };
    check(dir.path(), "recovered.mp4", &recovered, &salvaged);
}
