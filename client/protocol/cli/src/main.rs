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
//!
//! Beyond that client, for the host's input and Frame Warp tests: `NIGHTJAR_CLI_MOUSE_HZ` (a
//! one-count mouse jitter), `NIGHTJAR_CLI_LOOK` (a mouse-look pattern, `LookPattern`) and
//! `NIGHTJAR_CLI_FRAME_WARP` (a CSV of the shift Frame Warp gives each picture, `WarpLog`).

mod decode;
mod harness;
mod media;
mod report;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nightjar_core::control::ClientMessage;
use nightjar_core::frame_warp::FrameWarp;
use nightjar_core::sdp::*;
use nightjar_core::session::{self, HostInfo, Interrupt, Location, Session, SessionConfig};
use nightjar_core::timing_record as record;

use report::{distribution, IdrProbe, Measurements};
use rubylight_protocol::control::{
    HostTimingRequest, LinkNoticeRequest, CLOCK_SYNC_MESSAGE_TYPE, HOST_TIMING_MESSAGE_TYPE,
    LINK_NOTICE_MESSAGE_TYPE,
};

use harness::*;
use media::*;

fn usage_error(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut args: Vec<String> = std::env::args().collect();
    let roam_at = take_roam_options(&mut args).unwrap_or_else(|e| usage_error(&e));
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
    if roam_at.is_some_and(|r| r.seconds >= duration as f64) {
        usage_error("--roam-at needs an explicit duration longer than it");
    }
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
    // NIGHTJAR_CLI_FRAME_WARP=<csv>: ask the host for Frame Warp and log each picture's shift.
    let frame_warp_csv = match env("NIGHTJAR_CLI_FRAME_WARP") {
        None => None,
        Some(path) => match File::create(&path) {
            Ok(file) => {
                let mut csv = BufWriter::new(file);
                if let Err(e) = writeln!(csv, "{FRAME_WARP_HEADER}") {
                    usage_error(&format!("Frame Warp CSV: {e}"));
                }
                Some(csv)
            }
            Err(e) => usage_error(&format!("Frame Warp CSV: {e}")),
        },
    };
    let frame_warp = frame_warp_csv.as_ref().map(|_| FrameWarp::new(None, 256));
    // NIGHTJAR_CLI_LOOK=<seed>: the mouse-look pattern, in place of NIGHTJAR_CLI_MOUSE_HZ.
    let look_seed = env("NIGHTJAR_CLI_LOOK").map(|seed| {
        if env("NIGHTJAR_CLI_MOUSE_HZ").is_some() {
            usage_error("NIGHTJAR_CLI_LOOK and NIGHTJAR_CLI_MOUSE_HZ cannot be used together");
        }
        seed.trim().parse::<u64>().unwrap_or_else(|_| {
            usage_error("NIGHTJAR_CLI_LOOK must be a whole number, the pattern's seed")
        })
    });
    let sent = Arc::new(SentMouse::default());
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
    // The wired link speed an app reports for its port, sent for every codec (0 sends none).
    if let Some(v) = env("BUTTERPOLLO_TEST_LINK_MBPS") {
        stream.pyrowave_link_mbps = atoi(&v).clamp(0, 9999) as i32;
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
            receive_decode_priority: true,
            // As the apps' default.
            receive_coalescing: true,
            frame_warp: frame_warp.clone(),
            ..Default::default()
        },
        Box::new(Decoder {
            state: state.clone(),
            decoder: None,
            timing_csv,
            bitstream: None,
            frame_warp: frame_warp
                .clone()
                .zip(frame_warp_csv)
                .map(|(warp, csv)| WarpLog {
                    warp,
                    sent: sent.clone(),
                    csv,
                }),
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
    // The host's notice that it lowered the bitrate for the network is printed, with or without a CSV.
    if session
        .outcome()
        .host_control_messages
        .contains(&LINK_NOTICE_MESSAGE_TYPE)
    {
        let _ = session.send(ClientMessage::LinkNoticeRequest(LinkNoticeRequest {
            enabled: true,
        }));
    }
    let clock_sync = announced(CLOCK_SYNC_MESSAGE_TYPE);
    // NIGHTJAR_CLI_MOUSE_HZ: relative mouse moves at that rate, one pixel right then back,
    // for the host's input timings (a zero move is never sent).
    let mouse_stop = Arc::new(AtomicBool::new(false));
    let mouse = std::env::var("NIGHTJAR_CLI_MOUSE_HZ")
        .ok()
        .and_then(|hz| hz.parse::<f64>().ok())
        .filter(|hz| *hz > 0.0 && *hz <= 2000.0)
        .map(|hz| {
            let (input, stop, sent) = (session.input_handle(), mouse_stop.clone(), sent.clone());
            println!("MOUSE moves at {hz} Hz");
            std::thread::spawn(move || {
                let period = Duration::from_secs_f64(1.0 / hz);
                let mut step = 1i16;
                while !stop.load(Ordering::Relaxed) {
                    sent.send(&input, step, 0);
                    step = -step;
                    std::thread::sleep(period);
                }
            })
        });
    // NIGHTJAR_CLI_LOOK: the look pattern every 4 ms, from the first decoded picture on (the
    // stream runs and the host counts what arrives), its time counted from then.
    let look = look_seed.map(|seed| {
        let (input, stop, sent) = (session.input_handle(), mouse_stop.clone(), sent.clone());
        let state = state.clone();
        println!(
            "MOUSE look pattern seed={seed} every {} ms from the first decoded picture",
            LOOK_STEP.as_millis()
        );
        std::thread::spawn(move || {
            while state.counters.decoded_frames.load(Ordering::SeqCst) == 0 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(LOOK_STEP);
            }
            let mut pattern = LookPattern::new(seed);
            let start = Instant::now();
            let mut due = start;
            while !stop.load(Ordering::Relaxed) {
                let (dx, dy) = pattern.step(start.elapsed().as_micros() as u64);
                if dx != 0 || dy != 0 {
                    sent.send(&input, dx, dy);
                }
                // A late step catches up in counts, not in extra steps.
                due = (due + LOOK_STEP).max(Instant::now());
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
        })
    });
    let mut last_clock_probe_us = 0u64;
    let started = clock::ms();
    let probe_started = session::now_us();
    let mut roamed = false;
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
        if let Some(roam) = roam_at.filter(|r| !roamed && clock::ms() - started >= r.seconds * 1000.0)
        {
            roamed = true;
            match session.roam_to(roam.ip) {
                Ok(()) => println!(
                    "ROAM simulated at={:.3} ip={}",
                    (clock::ms() - started) / 1000.0,
                    roam.ip.map_or_else(|| "same".to_owned(), |ip| ip.to_string())
                ),
                Err(e) => {
                    eprintln!("Simulated roam failed: {e}");
                    c.fail();
                }
            }
        }
        let now_us = session::now_us();
        if clock_sync && now_us.saturating_sub(last_clock_probe_us) >= CLOCK_PROBE_INTERVAL_US {
            last_clock_probe_us = now_us;
            let _ = session.send_clock_sync_probe(now_us);
        }
        std::thread::sleep(Duration::from_millis(if clock_sync { 50 } else { 100 }));
    }
    mouse_stop.store(true, Ordering::Relaxed);
    if let Some(mouse) = mouse {
        let _ = mouse.join();
    }
    if let Some(look) = look {
        let _ = look.join();
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
    if roam_at.is_some() {
        let completed = c.roamed.load(Ordering::SeqCst);
        let _ = writeln!(
            out,
            "ROAM_RESULT {{\"simulated\":{roamed},\"roamed\":{completed}}}"
        );
        if completed == 0 {
            eprintln!("The simulated roam did not complete");
            c.fail();
        }
    }
    if let Some(warp) = &frame_warp {
        let status = warp.status();
        let model = match status.model {
            None => "none".to_owned(),
            Some(m) => format!(
                "{} mouse_gain={:.6},{:.6} lag_ms={:.3}",
                if m.mouse_locked { "locked" } else { "unlocked" },
                m.mouse_gain[0],
                m.mouse_gain[1],
                f64::from(m.lag_us) / 1000.0
            ),
        };
        let (sent_x, sent_y) = sent.get();
        let _ = writeln!(
            out,
            "FRAME_WARP rows={} warped={} extrapolated={} untagged={} sent={sent_x},{sent_y} model={model}",
            c.frame_warp_rows.load(Ordering::SeqCst),
            status.warped,
            status.extrapolated,
            status.untagged
        );
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

#[cfg(test)]
mod tests;
