//! nightjar-cli: a headless Moonlight-protocol client on `nightjar-core`, a drop-in for
//! Rubylight's `rust/tests/moonlight_client.c` so `rust/tests/interop.py` and
//! `rust/release/e2e.py` drive it unchanged. The arguments, environment variables, output
//! lines and exit codes are that client's; see it for the meaning of each.
//!
//! usage: nightjar-cli SESSION_URL [CODEC [WIDTH HEIGHT FPS [SECONDS [BITRATE [THREADS]]]]]
//!
//! Video is decoded with libavcodec and audio with libopus (the `ffmpeg` feature), as the C
//! client does, and every decoded picture is checked: size, luma detail, HDR format and the
//! motion probe's barcode. PyroWave is not supported.

mod decode;
mod report;

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use nightjar_core::control::{ClientMessage, ControlEvent, HdrMetadata};
use nightjar_core::sdp::*;
use nightjar_core::session::{
    self, AudioRenderer, AudioSetup, DecodeStatus, HostInfo, Interrupt, Listener, Location,
    Session, SessionConfig, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::timing_record as record;
use nightjar_core::video::Frame;

use decode::Received;
use report::{distribution, IdrProbe, Measurements};
use rubylight_protocol::control::{
    HostTimingRequest, CLOCK_SYNC_MESSAGE_TYPE, HOST_TIMING_MESSAGE_TYPE,
};

const MAX_MEASUREMENTS: usize = 3600 * 240;
const MAX_IDR_PROBES: i64 = 2000;
const CLOCK_PROBE_INTERVAL_US: u64 = 250_000;
const SERVER_CODEC_MODE_SUPPORT: u32 = 0x30301;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn env_is(name: &str, value: &str) -> bool {
    env(name).as_deref() == Some(value)
}

/// C's atoi: leading whitespace, a sign and digits; anything else ends the number.
fn atoi(text: &str) -> i64 {
    let text = text.trim_start();
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    sign * digits[..end].parse::<i64>().unwrap_or(0)
}

/// C's atof for the plain decimal numbers the harness passes.
fn atof(text: &str) -> f64 {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(text.len(), |(i, _)| i);
    text[..end].parse().unwrap_or(0.0)
}

/// Milliseconds on the clock the motion probe stamps its barcode with: the performance
/// counter on Windows.
#[cfg(windows)]
mod clock {
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    }

    pub fn frequency() -> f64 {
        let mut f = 0i64;
        // SAFETY: a valid out pointer.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f as f64
    }

    pub fn ms() -> f64 {
        let mut n = 0i64;
        // SAFETY: a valid out pointer.
        unsafe { QueryPerformanceCounter(&mut n) };
        n as f64 * 1000.0 / frequency()
    }
}

#[cfg(not(windows))]
mod clock {
    pub fn ms() -> f64 {
        static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        EPOCH
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
            * 1000.0
    }
}

/// Counters shared by the decoder, the renderer, the listener and the main thread.
#[derive(Default)]
struct Counters {
    frames: AtomicI32,
    decoded_frames: AtomicI32,
    audio_packets: AtomicI32,
    failures: AtomicI32,
    detailed_frames: AtomicI32,
    ended: AtomicBool,
    hdr_notifications: AtomicI32,
    hdr_enabled_notifications: AtomicI32,
    hdr_disabled_notifications: AtomicI32,
    hdr_control_mismatches: AtomicI32,
    hdr_invalid_metadata: AtomicI32,
}

impl Counters {
    fn fail(&self) {
        self.failures.fetch_add(1, Ordering::SeqCst);
    }
}

/// The run's settings from the arguments and environment.
#[derive(Clone)]
struct Options {
    requested_format: u32,
    requested_hdr: bool,
    requested_width: i32,
    requested_height: i32,
    decoder_threads: i32,
    hardware: Option<String>,
    hardware_device: Option<String>,
    barcode_bottom: bool,
    barcode_scale: f64,
    barcode_left: f64,
    barcode_bottom_margin: f64,
    expected_hdr_control: i32,
    idr_probe_count: i64,
    first_frame_dump: Option<String>,
    bitstream: Option<String>,
    frame_dump: Option<String>,
    frame_dump_at: i32,
    host: Option<String>,
    audio_tone: bool,
}

struct State {
    options: Options,
    counters: Counters,
    measurements: Mutex<Measurements>,
    probe: Mutex<IdrProbe>,
    audio: Mutex<report::AudioSignal>,
    /// Host timing reports and clock sync for the timing CSV's host columns; `None` without one.
    record: Option<Mutex<record::Record>>,
}

/// The barcode's age in milliseconds, comparable only on the same Windows machine (both
/// processes read the performance counter): None elsewhere.
fn picture_age(host: Option<&str>, ticks: u64) -> Option<f64> {
    let local = host.map_or(true, |h| h.starts_with("127.") || h == "::1");
    #[cfg(windows)]
    if local {
        return Some(clock::ms() - ticks as f64 * 1000.0 / clock::frequency());
    }
    let _ = (local, ticks);
    None
}

struct Decoder {
    state: Arc<State>,
    decoder: Option<decode::VideoDecoder>,
    timing_csv: Option<File>,
    bitstream: Option<File>,
}

impl Decoder {
    fn barcode_sample_y(&self, height: i32, row: i32) -> i32 {
        let o = &self.state.options;
        if o.barcode_bottom {
            height
                - (o.barcode_bottom_margin + f64::from(128 - 20 - row * 24) * o.barcode_scale)
                    as i32
        } else {
            (f64::from(20 + row * 24) * o.barcode_scale) as i32
        }
    }

    /// The motion probe's barcode: sequence and performance-counter ticks.
    fn picture_timestamp(&self, d: &decode::VideoDecoder) -> Option<(u32, u64)> {
        let o = &self.state.options;
        let s = o.barcode_scale;
        let (width, height) = (d.width(), d.height());
        if f64::from(width) < 640.0 * s || f64::from(height) < 128.0 * s {
            return None;
        }
        let mut words = [0u32; 4];
        for (row, word) in words.iter_mut().enumerate() {
            let y = self.barcode_sample_y(height, row as i32);
            let black = d.luma((o.barcode_left + 12.0 * s) as i32, y);
            let white = d.luma((o.barcode_left + 36.0 * s) as i32, y);
            if white <= black + 32 {
                return None;
            }
            let threshold = (black + white) / 2;
            for bit in 0..32 {
                let x = (o.barcode_left + f64::from(72 + bit * 16) * s) as i32;
                if d.luma(x, y) > threshold {
                    *word |= 1 << bit;
                }
            }
        }
        if words[3] != 0xB17E_2212 || words[0] == 0 {
            return None;
        }
        Some((words[0], (u64::from(words[2]) << 32) | u64::from(words[1])))
    }

    /// Checks one decoded picture; returns its barcode sequence, ticks and age.
    fn check_picture(&mut self) -> (u32, u64, f64) {
        let state = self.state.clone();
        let o = &state.options;
        let c = &state.counters;
        let d = self.decoder.as_mut().expect("open decoder");
        if o.idr_probe_count != 0 {
            lock(&state.probe).decoded(d.pts(), session::now_us());
        }
        // AMD AV1 surfaces can include alignment padding outside the stream.
        if o.requested_format & VIDEO_FORMAT_MASK_AV1 != 0
            && (o.requested_width..=o.requested_width + 64).contains(&d.width())
            && (o.requested_height..=o.requested_height + 16).contains(&d.height())
        {
            d.crop(o.requested_width, o.requested_height);
        }
        let d = self.decoder.as_ref().expect("open decoder");
        if d.width() != o.requested_width || d.height() != o.requested_height {
            c.fail();
        }
        let (mut low, mut high) = (65535u32, 0u32);
        for y in 1..8 {
            for x in 1..12 {
                let value = d.luma(d.width() * x / 12, d.height() * y / 8);
                low = low.min(value);
                high = high.max(value);
            }
        }
        let depth = d.depth();
        if depth > 0 && high > low + (16u32 << (depth - 8).max(0)) {
            c.detailed_frames.fetch_add(1, Ordering::SeqCst);
        }
        if o.requested_hdr && (depth < 10 || d.primaries() != 9 || d.transfer() != 16) {
            c.fail();
        }
        let decoded = c.decoded_frames.fetch_add(1, Ordering::SeqCst) + 1;
        if decoded == 1 {
            println!(
                "DECODED {}x{} pixel_format={} primaries={} transfer={}",
                d.width(),
                d.height(),
                d.format(),
                d.primaries(),
                d.transfer()
            );
        }
        if let Some(path) = &o.frame_dump {
            if decoded == o.frame_dump_at {
                d.dump(path);
                println!(
                    "FRAME_DUMP frame={} format={}",
                    o.frame_dump_at,
                    d.format_name()
                );
            }
        }
        let Some((sequence, ticks)) = self.picture_timestamp(d) else {
            return (0, 0, -1.0);
        };
        match picture_age(o.host.as_deref(), ticks) {
            // A signature alone must not turn unrelated content into a latency sample.
            Some(age) if !(0.0..=3000.0).contains(&age) => (0, ticks, -1.0),
            Some(age) => (sequence, ticks, age),
            None => (sequence, ticks, -1.0),
        }
    }
}

impl VideoDecoder for Decoder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        let o = &self.state.options;
        println!(
            "VIDEO format={} {}x{}@{}",
            setup.format, setup.width, setup.height, setup.fps
        );
        if setup.format != o.requested_format {
            eprintln!(
                "Codec fallback: requested={} negotiated={}",
                o.requested_format, setup.format
            );
            return Err(-1);
        }
        let codec = if setup.format & VIDEO_FORMAT_MASK_H264 != 0 {
            0
        } else if setup.format & VIDEO_FORMAT_MASK_H265 != 0 {
            1
        } else {
            2
        };
        let (decoder, info, error) = decode::VideoDecoder::open(
            codec,
            o.decoder_threads,
            o.hardware.as_deref(),
            o.hardware_device.as_deref(),
        );
        if let Some(hardware) = &o.hardware {
            if !info.hardware_format.is_empty() {
                println!(
                    "HARDWARE_DECODER type={hardware} format={}",
                    info.hardware_format
                );
            }
        }
        if !info.name.is_empty() {
            println!(
                "DECODER codec={} active_thread_type={} delay={}",
                info.name, info.active_thread_type, info.delay
            );
        }
        self.decoder = decoder;
        if self.decoder.is_none() {
            return Err(if error < 0 { error } else { -1 });
        }
        Ok(())
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let state = self.state.clone();
        let o = &state.options;
        let c = &state.counters;
        let decode_started = clock::ms();
        if o.idr_probe_count != 0 {
            lock(&state.probe).arrived(frame);
        }
        let hardware = o.hardware.is_some();
        let arrival_ms = if hardware {
            decode_started
                - (session::now_us() as f64 / 1000.0 - frame.enqueue_time_us as f64 / 1000.0)
        } else {
            decode_started
        };
        let data = frame.data();
        if data.is_empty() || data.len() > 32 * 1024 * 1024 {
            c.frames.fetch_add(1, Ordering::SeqCst);
            c.fail();
            return DecodeStatus::Ok;
        }
        if let Some(path) = &o.first_frame_dump {
            if c.frames.load(Ordering::SeqCst) == 0 {
                let written = File::create(path).and_then(|mut f| f.write_all(data));
                if written.is_err() {
                    c.fail();
                }
            }
        }
        if self.bitstream.is_none() {
            if let Some(path) = &o.bitstream {
                self.bitstream = File::create(path).ok();
            }
        }
        if let Some(file) = &mut self.bitstream {
            let _ = file.write_all(data).and_then(|_| file.flush());
        }
        let (mut sequence, mut ticks, mut age) = (0u32, 0u64, -1.0f64);
        let decoder = self.decoder.as_mut().expect("open decoder");
        if decoder.send(data, i64::from(frame.number)) < 0 {
            c.fail();
        }
        let mut received = decoder.receive();
        while received == Received::Frame || received == Received::Readback {
            if received == Received::Readback {
                c.fail();
            } else {
                (sequence, ticks, age) = self.check_picture();
            }
            received = self.decoder.as_mut().expect("open decoder").receive();
        }
        if let Received::Error(_) = received {
            c.fail();
        }
        let decode_ms = clock::ms() - decode_started;
        let host_ms = f64::from(frame.host_processing_latency) / 10.0;
        lock(&state.measurements).push(
            MAX_MEASUREMENTS,
            host_ms,
            decode_ms,
            arrival_ms,
            frame.enqueue_time_us as f64 / 1000.0,
            age,
            sequence,
        );
        if let Some(csv) = &mut self.timing_csv {
            let line = format!(
                "{},{:.6},{:.3},{:.6},{},{},{:.6},{},{},{},{},{}",
                frame.number,
                arrival_ms,
                host_ms,
                decode_ms,
                sequence,
                ticks,
                age,
                frame.receive_time_us,
                frame.enqueue_time_us,
                frame.presentation_time_us,
                frame.frame_type as i32,
                data.len()
            );
            match &state.record {
                // The host columns wait for the frame's report, which follows its last packet.
                Some(record) => {
                    let mut record = lock(record);
                    let row = record::PendingRow {
                        number: frame.number,
                        line,
                        rtp_timestamp: frame.rtp_timestamp,
                        clock: record.clock(frame.enqueue_time_us),
                        due_us: session::now_us() + record::ROW_DELAY_US,
                    };
                    record.push(row);
                    for row in record.take_due(session::now_us()) {
                        let _ = writeln!(csv, "{row}");
                    }
                }
                None => {
                    let _ = writeln!(csv, "{line}");
                }
            }
        }
        let frames = c.frames.fetch_add(1, Ordering::SeqCst) + 1;
        if frames < 4 {
            println!(
                "FRAME {} bytes={} type={}",
                frame.number,
                data.len(),
                frame.frame_type as i32
            );
        }
        DecodeStatus::Ok
    }

    fn cleanup(&mut self) {
        self.decoder = None;
        if let Some(csv) = &mut self.timing_csv {
            if let Some(record) = &self.state.record {
                for row in lock(record).take_due(u64::MAX) {
                    let _ = writeln!(csv, "{row}");
                }
            }
            let _ = csv.flush();
        }
    }
}

struct Audio {
    state: Arc<State>,
    decoder: Option<decode::OpusDecoder>,
    channels: usize,
    csv: Option<File>,
    pcm: Vec<f32>,
}

impl AudioRenderer for Audio {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        let opus = &setup.opus;
        println!(
            "AUDIO channels={} samples={}",
            opus.channel_count, setup.samples_per_frame
        );
        self.channels = usize::from(opus.channel_count);
        self.decoder = Some(decode::OpusDecoder::new(
            i32::from(opus.channel_count),
            i32::from(opus.streams),
            i32::from(opus.coupled_streams),
            &opus.mapping,
        )?);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        let count = decoder.decode(packet, &mut self.pcm, 5760);
        let c = &self.state.counters;
        let (mut minimum, mut maximum) = (f64::INFINITY, 0.0f64);
        if count > 0 {
            c.audio_packets.fetch_add(1, Ordering::SeqCst);
            let samples = &self.pcm[..count as usize * self.channels];
            let mut signal = lock(&self.state.audio);
            (minimum, maximum) = signal.add(samples, self.channels, self.state.options.audio_tone);
        } else {
            c.fail();
        }
        if let Some(csv) = &mut self.csv {
            let _ = writeln!(
                csv,
                "{:.6},{},{:.9},{:.9}",
                clock::ms(),
                count,
                if minimum.is_finite() {
                    minimum.sqrt()
                } else {
                    -1.0
                },
                maximum.sqrt()
            );
        }
    }

    fn cleanup(&mut self) {
        self.decoder = None;
        if let Some(csv) = &mut self.csv {
            let _ = csv.flush();
        }
    }
}

struct Events {
    state: Arc<State>,
}

fn hdr_metadata_valid(m: &HdrMetadata) -> bool {
    let chromaticity = |(x, y): (u16, u16)| {
        let (x, y) = (u32::from(x), u32::from(y));
        y > 0 && x <= 50000 && y <= 50000 && x + y <= 50000
    };
    m.max_display_luminance != 0
        && chromaticity(m.white_point)
        && u32::from(m.min_display_luminance) <= u32::from(m.max_display_luminance) * 10000
        && m.max_full_frame_luminance <= m.max_display_luminance
        && m.display_primaries.iter().all(|&p| chromaticity(p))
}

impl Listener for Events {
    fn stage_starting(&self, stage: Stage) {
        println!("STAGE {}", stage.name());
    }

    fn stage_failed(&self, stage: Stage, error: i32) {
        println!("FAILED {} error={error}", stage.name());
    }

    fn connection_terminated(&self, error: i32) {
        println!("TERMINATED error={error}");
        self.state.counters.ended.store(true, Ordering::SeqCst);
    }

    fn log(&self, message: &str) {
        println!("{message}");
    }

    fn control_event(&self, event: ControlEvent) {
        let (enabled, metadata) = match event {
            ControlEvent::HdrMode { enabled, metadata } => (enabled, metadata),
            ControlEvent::HostTiming(records) => {
                if let Some(record) = &self.state.record {
                    lock(record).add_reports(records);
                }
                return;
            }
            ControlEvent::ClockSync { reply, received } => {
                if let Some(record) = &self.state.record {
                    lock(record)
                        .offset
                        .add(&reply, record::local_us_at(received));
                }
                return;
            }
            _ => return,
        };
        let c = &self.state.counters;
        let valid = metadata.as_ref().is_some_and(hdr_metadata_valid);
        let notification = c.hdr_notifications.fetch_add(1, Ordering::SeqCst) + 1;
        if enabled {
            c.hdr_enabled_notifications.fetch_add(1, Ordering::SeqCst);
        } else {
            c.hdr_disabled_notifications.fetch_add(1, Ordering::SeqCst);
        }
        let expected = self.state.options.expected_hdr_control;
        if expected >= 0 && i32::from(enabled) != expected {
            c.hdr_control_mismatches.fetch_add(1, Ordering::SeqCst);
        }
        if enabled && !valid {
            c.hdr_invalid_metadata.fetch_add(1, Ordering::SeqCst);
        }
        match metadata {
            Some(m) => println!(
                "HDR_CONTROL {{\"notification\":{notification},\"enabled\":{enabled},\"metadata_available\":true,\"metadata_valid\":{valid},\
\"primaries_xy_50000\":[[{},{}],[{},{}],[{},{}]],\"white_xy_50000\":[{},{}],\
\"maximum_nits\":{},\"minimum_10000th_nit\":{},\"max_cll_nits\":{},\"max_fall_nits\":{},\"full_frame_nits\":{}}}",
                m.display_primaries[0].0,
                m.display_primaries[0].1,
                m.display_primaries[1].0,
                m.display_primaries[1].1,
                m.display_primaries[2].0,
                m.display_primaries[2].1,
                m.white_point.0,
                m.white_point.1,
                m.max_display_luminance,
                m.min_display_luminance,
                m.max_content_light_level,
                m.max_frame_average_light_level,
                m.max_full_frame_luminance
            ),
            None => println!(
                "HDR_CONTROL {{\"notification\":{notification},\"enabled\":{enabled},\"metadata_available\":false,\"metadata_valid\":false}}"
            ),
        }
    }
}

fn usage_error(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage_error("session URL required");
    }
    let arg = |i: usize| args.get(i).map(String::as_str);

    let mut stream = StreamConfig {
        width: 640,
        height: 480,
        fps: 30,
        bitrate_kbps: 2000,
        packet_size: 1024,
        location: StreamLocation::Local,
        audio: AudioConfiguration::STEREO,
        supported_video_formats: VIDEO_FORMAT_H264,
        color_space: COLORSPACE_REC_601,
        color_range: COLOR_RANGE_LIMITED,
        encryption_flags: ENCFLG_ALL,
        ..Default::default()
    };
    if let Some(codec) = arg(2) {
        stream.supported_video_formats = match codec {
            "h264" => VIDEO_FORMAT_H264,
            "hevc" => VIDEO_FORMAT_H265,
            "hevc-hdr" => VIDEO_FORMAT_H265_MAIN10,
            "av1" => VIDEO_FORMAT_AV1_MAIN8,
            "av1-hdr" => VIDEO_FORMAT_AV1_MAIN10,
            other => usage_error(&format!("Unknown codec: {other}")),
        };
    }
    let requested_format = stream.supported_video_formats;
    let requested_hdr =
        requested_format & (VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10) != 0;

    let profile = env("BUTTERPOLLO_TEST_CLIENT_PROFILE");
    let xbox = profile.as_deref() == Some("xbox");
    if profile.is_some() && !xbox {
        usage_error("BUTTERPOLLO_TEST_CLIENT_PROFILE must be xbox");
    }
    if xbox {
        if requested_format & !(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10)
            != 0
        {
            usage_error("The Xbox profile offers H.264 and HEVC only");
        }
        if requested_format != VIDEO_FORMAT_H264 {
            stream.supported_video_formats = VIDEO_FORMAT_H264
                | VIDEO_FORMAT_H265
                | (requested_format & VIDEO_FORMAT_H265_MAIN10);
        }
        stream.encryption_flags = ENCFLG_AUDIO;
        stream.packet_size = 1392;
        stream.color_space = COLORSPACE_REC_601;
        stream.color_range = COLOR_RANGE_LIMITED;
    }
    let expected_hdr_control = match env("BUTTERPOLLO_TEST_EXPECT_HDR_CONTROL").as_deref() {
        None => -1,
        Some("auto") => i32::from(requested_hdr),
        Some("0") => 0,
        Some("1") => 1,
        Some(_) => usage_error("BUTTERPOLLO_TEST_EXPECT_HDR_CONTROL must be auto, 0 or 1"),
    };
    let expected_hdr_messages =
        env("BUTTERPOLLO_TEST_EXPECT_HDR_CONTROL_COUNT").map_or(0, |v| atoi(&v) as i32);
    let duration = arg(6).map_or(0, atoi);
    let warmup_seconds = env("BUTTERPOLLO_TEST_WARMUP_SECONDS").map_or(2.0, |v| atof(&v));
    let idr_probe_count = match env("BUTTERPOLLO_TEST_IDR_PROBE") {
        None => 0,
        Some(text) => match text.parse::<i64>() {
            Ok(n)
                if text.starts_with(|c: char| c.is_ascii_digit())
                    && (1..=MAX_IDR_PROBES).contains(&n) =>
            {
                n
            }
            _ => usage_error(&format!(
                "BUTTERPOLLO_TEST_IDR_PROBE must be an integer from 1 to {MAX_IDR_PROBES}"
            )),
        },
    };
    let idr_probe_interval_ms = match env("BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS") {
        None => 1500,
        Some(text) => match text.parse::<i64>() {
            Ok(n)
                if text.starts_with(|c: char| c.is_ascii_digit()) && (100..=10000).contains(&n) =>
            {
                n
            }
            _ => usage_error(
                "BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS must be an integer from 100 to 10000",
            ),
        },
    };
    if idr_probe_count != 0
        && (!warmup_seconds.is_finite()
            || (duration as f64)
                < warmup_seconds + idr_probe_count as f64 * idr_probe_interval_ms as f64 / 1000.0)
    {
        usage_error(
            "IDR probe requires an explicit duration of at least warmup + N request intervals",
        );
    }
    let mut barcode_scale = env("BUTTERPOLLO_TEST_BARCODE_SCALE").map_or(1.0, |v| atof(&v));
    if !(barcode_scale > 0.0 && barcode_scale <= 1.0) {
        barcode_scale = 1.0;
    }
    if !(0.0..=60.0).contains(&warmup_seconds) {
        return 2;
    }
    let mut idr_csv = None;
    if idr_probe_count != 0 {
        let path = env("BUTTERPOLLO_TEST_TIMING_CSV")
            .map_or_else(|| "idr-probe.csv".to_owned(), |p| format!("{p}.idr.csv"));
        match File::create(&path) {
            Ok(file) => idr_csv = Some(file),
            Err(e) => usage_error(&format!("IDR probe CSV: {e}")),
        }
    }
    let timing_csv = match env("BUTTERPOLLO_TEST_TIMING_CSV") {
        None => None,
        Some(path) => match File::create(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "wire_frame,arrival_ms,host_ms,decode_ms,render_frame,render_qpc,picture_age_ms,first_packet_us,assembled_us,presentation_us,frame_type,bytes,{}", record::HEADER);
                Some(file)
            }
            Err(e) => usage_error(&format!("timing CSV: {e}")),
        },
    };
    let audio_csv = match env("BUTTERPOLLO_TEST_AUDIO_CSV") {
        None => None,
        Some(path) => match File::create(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "arrival_ms,samples,min_rms,max_rms");
                Some(file)
            }
            Err(e) => usage_error(&format!("audio CSV: {e}")),
        },
    };
    if let Some(v) = arg(3) {
        stream.width = atoi(v) as i32;
    }
    if let Some(v) = arg(4) {
        stream.height = atoi(v) as i32;
    }
    if let Some(v) = arg(5) {
        stream.fps = atoi(v) as i32;
    }
    if let Some(v) = arg(7) {
        stream.bitrate_kbps = atoi(v) as i32;
    }
    let decoder_threads = arg(8).map_or(1, |v| atoi(v) as i32);
    if let Some(v) = env("BUTTERPOLLO_TEST_REFRESH_X100") {
        stream.client_refresh_rate_x100 = atoi(&v) as i32;
    }
    if !(2..=8192).contains(&stream.width)
        || !(2..=8192).contains(&stream.height)
        || !(1..=240).contains(&stream.fps)
        || !(0..=3500).contains(&duration)
        || !(1..=16).contains(&decoder_threads)
    {
        return 2;
    }
    println!("DECODER threads={decoder_threads}");
    for (i, byte) in stream.remote_input_aes_key.iter_mut().enumerate() {
        *byte = i as u8;
    }
    stream.remote_input_aes_iv[3] = 123;
    // Concurrent clients need distinct stream keys: rikey byte i is i+seed, rikeyid 123+seed.
    if let Some(seed) = env("BUTTERPOLLO_TEST_KEY_SEED") {
        let seed = (atoi(&seed) & 0x7f) as u8;
        for (i, byte) in stream.remote_input_aes_key.iter_mut().enumerate() {
            *byte = i as u8 + seed;
        }
        stream.remote_input_aes_iv[3] = 123 + seed;
    }
    if env_is("BUTTERPOLLO_TEST_SIGNED_KEY_ID", "1") {
        stream.remote_input_aes_iv[0] = 0x80;
    }
    let hardware = env("BUTTERPOLLO_TEST_HW_DECODER");
    // Direct submission decodes on the receive thread; a hardware decoder gets its own.
    stream.video_capabilities = if hardware.is_some() {
        0
    } else {
        CAPABILITY_DIRECT_SUBMIT
    };
    if env_is("BUTTERPOLLO_TEST_RFI", "1") {
        stream.video_capabilities |= CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC
            | CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC
            | CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;
    }
    if xbox {
        println!(
            "CLIENT_PROFILE xbox formats=0x{:x} encryption={} packet_size={} refresh_x100={} intra_refresh=0",
            stream.supported_video_formats,
            stream.encryption_flags as i32,
            stream.packet_size,
            stream.client_refresh_rate_x100
        );
    }
    stream.audio_capabilities = CAPABILITY_DIRECT_SUBMIT;

    let host = env("BUTTERPOLLO_TEST_HOST");
    let options = Options {
        requested_format,
        requested_hdr,
        requested_width: stream.width,
        requested_height: stream.height,
        decoder_threads,
        hardware,
        hardware_device: env("BUTTERPOLLO_TEST_HW_DEVICE"),
        barcode_bottom: env_is("BUTTERPOLLO_TEST_BARCODE_BOTTOM", "1"),
        barcode_scale,
        barcode_left: env("BUTTERPOLLO_TEST_BARCODE_LEFT").map_or(0.0, |v| atof(&v)),
        barcode_bottom_margin: env("BUTTERPOLLO_TEST_BARCODE_BOTTOM_MARGIN")
            .map_or(0.0, |v| atof(&v)),
        expected_hdr_control,
        idr_probe_count,
        first_frame_dump: env("BUTTERPOLLO_TEST_FIRST_FRAME"),
        bitstream: env("BUTTERPOLLO_TEST_BITSTREAM"),
        frame_dump: env("BUTTERPOLLO_TEST_FRAME_DUMP"),
        frame_dump_at: env("BUTTERPOLLO_TEST_FRAME_DUMP_AT").map_or(600, |v| atoi(&v) as i32),
        host: host.clone(),
        audio_tone: env("BUTTERPOLLO_TEST_AUDIO_TONE").is_some(),
    };
    let fps = stream.fps;
    let state = Arc::new(State {
        options,
        counters: Counters::default(),
        measurements: Mutex::new(Measurements::default()),
        probe: Mutex::new(IdrProbe::new(
            idr_probe_count as usize,
            idr_probe_interval_ms as u64 * 1000,
            warmup_seconds,
        )),
        audio: Mutex::new(report::AudioSignal::default()),
        record: timing_csv
            .is_some()
            .then(|| Mutex::new(record::Record::default())),
    });

    let server = HostInfo {
        address: host.unwrap_or_else(|| "127.0.0.1".into()),
        app_version: "7.1.431.-1".into(),
        rtsp_session_url: Some(args[1].clone()),
        server_codec_mode_support: SERVER_CODEC_MODE_SUPPORT,
    };
    let session = match Session::start(
        &server,
        SessionConfig {
            stream,
            location: Location::Local,
            ..Default::default()
        },
        Box::new(Decoder {
            state: state.clone(),
            decoder: None,
            timing_csv,
            bitstream: None,
        }),
        Box::new(Audio {
            state: state.clone(),
            decoder: None,
            channels: 2,
            csv: audio_csv,
            pcm: vec![0.0; 5760 * 8],
        }),
        Arc::new(Events {
            state: state.clone(),
        }),
        &Interrupt::default(),
    ) {
        Ok(session) => session,
        Err(error) => {
            println!("CONNECT FAILED {}", error.code);
            eprintln!("{error}");
            return 1;
        }
    };

    let c = &state.counters;
    // With a timing CSV, ask for host timing reports and probe the clock offset four times a
    // second, when the host announced them.
    let announced = |message_type| {
        state.record.is_some()
            && session
                .outcome()
                .host_control_messages
                .contains(&message_type)
    };
    if announced(HOST_TIMING_MESSAGE_TYPE) {
        let _ = session.send(ClientMessage::HostTimingRequest(HostTimingRequest {
            enabled: true,
        }));
    }
    let clock_sync = announced(CLOCK_SYNC_MESSAGE_TYPE);
    let mut last_clock_probe_us = 0u64;
    let started = clock::ms();
    let probe_started = session::now_us();
    let limit_ms = if duration != 0 {
        duration as f64 * 1000.0
    } else {
        10000.0
    };
    while clock::ms() - started < limit_ms
        && !c.ended.load(Ordering::SeqCst)
        && (duration != 0 || c.frames.load(Ordering::SeqCst) < 30)
    {
        if idr_probe_count != 0 {
            let decoded = c.decoded_frames.load(Ordering::SeqCst) != 0;
            if lock(&state.probe).poll(session::now_us(), probe_started, decoded) {
                session.request_idr_frame();
            }
        }
        let now_us = session::now_us();
        if clock_sync && now_us.saturating_sub(last_clock_probe_us) >= CLOCK_PROBE_INTERVAL_US {
            last_clock_probe_us = now_us;
            let _ = session.send_clock_sync_probe(now_us);
        }
        std::thread::sleep(Duration::from_millis(if clock_sync { 50 } else { 100 }));
    }
    let seconds = (clock::ms() - started) / 1000.0;
    let premature =
        c.ended.load(Ordering::SeqCst) || (duration != 0 && seconds < duration as f64 * 0.98);
    session.stop();

    let out = &mut std::io::stdout().lock();
    if let Some(mut csv) = idr_csv {
        if !lock(&state.probe).summarize(out, &mut csv) {
            eprintln!("IDR probe incomplete: not every request received and decoded an IDR");
            c.fail();
        }
        if csv.flush().is_err() {
            c.fail();
        }
    }
    let notifications = c.hdr_notifications.load(Ordering::SeqCst);
    let hdr_control_valid = (expected_hdr_control < 0
        || (notifications > 0
            && c.hdr_control_mismatches.load(Ordering::SeqCst) == 0
            && c.hdr_invalid_metadata.load(Ordering::SeqCst) == 0))
        && (expected_hdr_messages == 0 || notifications == expected_hdr_messages);
    let _ = writeln!(
        out,
        "HDR_CONTROL_RESULT {{\"checked\":{},\"expected_enabled\":{},\"expected_notifications\":{},\"notifications\":{},\"enabled_notifications\":{},\"disabled_notifications\":{},\"mismatches\":{},\"invalid_metadata\":{},\"passed\":{}}}",
        expected_hdr_control >= 0 || expected_hdr_messages != 0,
        expected_hdr_control,
        expected_hdr_messages,
        notifications,
        c.hdr_enabled_notifications.load(Ordering::SeqCst),
        c.hdr_disabled_notifications.load(Ordering::SeqCst),
        c.hdr_control_mismatches.load(Ordering::SeqCst),
        c.hdr_invalid_metadata.load(Ordering::SeqCst),
        hdr_control_valid
    );
    if !hdr_control_valid {
        eprintln!("HDR control expectation failed: missing, extra or wrong mode notification, or invalid HDR metadata");
        c.fail();
    }
    let frames = c.frames.load(Ordering::SeqCst);
    let decoded = c.decoded_frames.load(Ordering::SeqCst);
    let audio_packets = c.audio_packets.load(Ordering::SeqCst);
    let _ = writeln!(
        out,
        "RESULT frames={frames} decoded_frames={decoded} audio_packets={audio_packets} failures={}",
        c.failures.load(Ordering::SeqCst)
    );
    let detailed = c.detailed_frames.load(Ordering::SeqCst);
    let _ = writeln!(out, "PICTURE_CONTENT frames_with_luma_contrast={detailed}");
    let audio = lock(&state.audio);
    let _ = writeln!(
        out,
        "AUDIO_SIGNAL samples={} peak={:.6} rms={:.6}",
        audio.samples,
        audio.peak,
        if audio.samples != 0 {
            (audio.energy / audio.samples as f64).sqrt()
        } else {
            0.0
        }
    );
    if state.options.audio_tone {
        let continuous = audio.tone_blocks != 0 && audio.tone_min >= audio.tone_max * 0.25;
        let _ = writeln!(
            out,
            "AUDIO_TONE blocks={} min_rms={:.6} max_rms={:.6} continuous={}",
            audio.tone_blocks,
            audio.tone_min.sqrt(),
            audio.tone_max.sqrt(),
            i32::from(continuous)
        );
        if !continuous {
            c.fail();
        }
    }
    let mut motion_valid = env("BUTTERPOLLO_TEST_REQUIRE_MOTION").is_none();
    let minimum_fps_text = env("BUTTERPOLLO_TEST_MIN_FPS");
    let minimum_fps = minimum_fps_text.as_deref().map_or(0.0, atof);
    let mut rate_valid = minimum_fps_text.is_none();
    let mut m = lock(&state.measurements);
    if duration != 0 && !m.host_latency.is_empty() {
        let steady = m.steady(warmup_seconds, fps);
        let _ = writeln!(
            out,
            "STEADY warmup_seconds={:.3} seconds={:.3} frames={} fps={:.3} intervals_over_1_5_period={}",
            warmup_seconds,
            steady.seconds,
            steady.host.len(),
            steady.fps,
            steady.late
        );
        if minimum_fps_text.is_some() {
            rate_valid = minimum_fps.is_finite() && minimum_fps > 0.0 && steady.fps >= minimum_fps;
        }
        let mut host_steady = steady.host.clone();
        distribution(out, "STEADY_HOST", &mut host_steady);
        let mut intervals = steady.intervals.clone();
        distribution(out, "ARRIVAL_INTERVAL", &mut intervals);
        if steady.visual != 0 {
            let _ = writeln!(
                out,
                "VISUAL frames={} unique={} repeats={} skipped_render_frames={} unique_fps={:.3} coverage={:.6}",
                steady.visual,
                steady.unique,
                steady.repeats,
                steady.skips,
                if steady.seconds > 0.0 {
                    f64::from(steady.unique.saturating_sub(1)) / steady.seconds
                } else {
                    0.0
                },
                if steady.host.is_empty() {
                    0.0
                } else {
                    f64::from(steady.visual) / steady.host.len() as f64
                }
            );
            let mut ages = steady.ages.clone();
            distribution(out, "PICTURE_AGE", &mut ages);
            if !steady.host.is_empty()
                && f64::from(steady.visual) >= steady.host.len() as f64 * 0.95
                && steady.unique > 1
            {
                motion_valid = true;
            }
        }
        let count = m.host_latency.len();
        let host_sum: f64 = m.host_latency.iter().sum();
        let decode_sum: f64 = m.decode_ms.iter().sum();
        m.host_latency.sort_by(f64::total_cmp);
        let _ = writeln!(
            out,
            "PERFORMANCE seconds={:.3} received_fps={:.2} decoded_fps={:.2} host_mean_ms={:.3} host_p50_ms={:.3} host_p95_ms={:.3} decoder_mean_ms={:.3}",
            seconds,
            f64::from(frames) / seconds,
            f64::from(decoded) / seconds,
            host_sum / count as f64,
            m.host_latency[(count - 1) / 2],
            m.host_latency[(count - 1) * 95 / 100],
            decode_sum / count as f64
        );
    }
    let _ = out.flush();
    if !motion_valid {
        eprintln!("Motion measurement failed: missing timestamps or static test content");
    }
    if !rate_valid {
        eprintln!("Stream did not sustain the required {minimum_fps:.3} FPS after warmup");
    }
    let passed = !premature
        && motion_valid
        && rate_valid
        && (idr_probe_count != 0 || decoded >= 30)
        && audio_packets > 0
        && c.failures.load(Ordering::SeqCst) == 0
        && (!state.options.audio_tone || audio.peak > 0.01)
        && (env("BUTTERPOLLO_TEST_REQUIRE_PICTURE").is_none() || detailed > 0);
    i32::from(!passed)
}
