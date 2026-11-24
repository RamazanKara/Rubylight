//! End-to-end streaming, recovery, input-only, and mouse pacing loopback tests.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::control::ptype;
use nightjar_core::sdp::*;
use nightjar_core::session::{
    HostInfo, Interrupt, Location, Session, SessionConfig, VideoSetup,
};
use nightjar_core::video::FrameType;

use crate::host::{start_host, Media};
use crate::media::{audio_stream, h264_idr, h264_p, Packetizer, KEY, PACKET_SIZE};
use crate::recorder::{
    input_arrives, roaming_config, start_session, until, Record, Recorder,
};

fn run_session(direct_submit: bool, packed_format: Option<u32>) {
    let host = start_host(packed_format);
    let record = Arc::new(Mutex::new(Record::default()));
    let recorder = |need_idr_at| {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at,
        })
    };
    let config = SessionConfig {
        stream: StreamConfig {
            width: 1280,
            height: 720,
            fps: 60,
            packet_size: PACKET_SIZE as i32,
            remote_input_aes_key: KEY,
            experimental_packed_444: packed_format.is_some(),
            packed444_modes: if packed_format.is_some() { 15 } else { 0 },
            supported_video_formats: packed_format.unwrap_or(VIDEO_FORMAT_H264),
            video_capabilities: if direct_submit {
                CAPABILITY_DIRECT_SUBMIT
            } else {
                0
            },
            ..Default::default()
        },
        location: Location::Auto,
        // As the apps' default.
        receive_coalescing: true,
        ..Default::default()
    };
    let session = Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        config,
        recorder(Some(3)),
        recorder(None),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap();
    {
        let r = record.lock().unwrap();
        assert!(r.started);
        assert_eq!(r.stages.len(), 11);
        // Before the decoder's setup, which names from them why 10-bit packed went 8-bit.
        assert_eq!(
            r.packed_modes,
            Some((if packed_format.is_some() { 15 } else { 0 }, false))
        );
        assert_eq!(
            r.setup,
            Some(VideoSetup {
                format: packed_format.unwrap_or(VIDEO_FORMAT_H264),
                width: 1280,
                height: if packed_format.is_some() { 1440 } else { 720 },
                fps: 60,
                chroma_packing: u8::from(packed_format.is_some()),
                logical_width: 1280,
                logical_height: 720,
                color_space: nightjar_core::sdp::COLORSPACE_REC_709,
                color_range: nightjar_core::sdp::COLOR_RANGE_LIMITED,
            })
        );
        let audio = r.audio_setup.unwrap();
        assert_eq!(
            (audio.opus.channel_count, audio.samples_per_frame),
            (2, 240)
        );
    }
    // Loopback is not private, so Auto streams as remote with 1024-byte packets.
    assert_eq!(session.outcome().config.location, StreamLocation::Remote);
    assert_eq!(session.outcome().config.packet_size, 1024);

    // Video: an IDR and P frames; the decoder needs a key frame at frame 3, so 4 and 5 are
    // dropped and the next IDR (6) recovers.
    let client = host.peer(Media::Video);
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let idr = if packed_format.is_some() {
        include_bytes!("../../../../app/src/main/assets/packed444/av1-8.bin").to_vec()
    } else {
        h264_idr()
    };
    let p = if packed_format.is_some() {
        vec![0x5a; 2500]
    } else {
        h264_p()
    };
    // Every video and audio datagram the host sends, parity included: the session's wire bytes.
    let mut sent = 0u64;
    let frames = [(&idr, 2), (&p, 1), (&p, 1), (&p, 1), (&p, 1)];
    for (payload, kind) in &frames {
        for (index, packet) in packetizer.frame(payload, *kind).into_iter().enumerate() {
            if packed_format.is_some() && index == 1 {
                continue;
            }
            sent += host.video.send_to(&packet, client).unwrap() as u64;
        }
        thread::sleep(Duration::from_millis(20));
    }
    until("the key frame request", || {
        host.control_messages
            .lock()
            .unwrap()
            .iter()
            .filter(|&&t| t == ptype::REQUEST_IDR_FRAME)
            .count()
            >= 2
    });
    for packet in packetizer.frame(&idr, 2) {
        sent += host.video.send_to(&packet, client).unwrap() as u64;
    }
    until("the recovery frame", || {
        record.lock().unwrap().frames.len() >= 4
    });

    // Audio: 100 packets of backlog are dropped, the rest play in order.
    let client = host.peer(Media::Audio);
    for packet in audio_stream(200) {
        sent += host.audio.send_to(&packet, client).unwrap() as u64;
        thread::sleep(Duration::from_micros(200));
    }
    until("audio", || record.lock().unwrap().audio.len() >= 90);
    until("every datagram counted on the wire", || {
        session.wire_bytes() == sent
    });

    // What Android's getVideoReceiveBufferInfo reads: the real socket's answer, not "unknown".
    let buffer = session
        .video_receive_buffer()
        .expect("the video socket's receive buffer is known");
    assert!(buffer.asked > 0 && buffer.granted > 0, "{buffer:?}");

    let stats = session.stats();
    assert!(stats.rtt.is_some());
    assert_eq!(stats.frames_submitted, 4);
    assert_eq!(stats.frames_need_idr, 1);

    // The host ends the stream: the listener hears it once, then the app stops.
    host.terminate.store(true, Ordering::SeqCst);
    until("termination", || {
        record.lock().unwrap().terminated.is_some()
    });
    session.stop();

    let r = record.lock().unwrap();
    assert_eq!(
        r.frames,
        [
            (1, FrameType::Idr),
            (2, FrameType::P),
            (3, FrameType::P),
            (6, FrameType::Idr)
        ]
    );
    assert!(r.audio.iter().all(Option::is_some));
    // The renderers' start can already ask what the host announced (the Android engine runs the
    // clock sync for the timing CSV only for a host that announced it, and decides in `start`).
    assert_eq!(
        r.startup.first().map(String::as_str),
        Some("control messages [5530, 5531, 5532, 5533, 5534, 5535, 5536, 5537, 5538]"),
        "{:?}",
        r.startup
    );
    assert_eq!(r.startup.get(1).map(String::as_str), Some("video start"));
    assert_eq!(r.audio[0].as_deref(), Some(&audio_stream(200)[150][12..]));
    assert_eq!(r.terminated, Some(0));
    assert_eq!(
        r.lifecycle,
        ["audio cleanup", "video stop", "video cleanup"]
    );
    let messages = host.control_messages.lock().unwrap();
    assert!(messages.contains(&ptype::START_B));
    assert!(messages.contains(&ptype::PERIODIC_PING));
    assert!(messages.contains(&ptype::INPUT_DATA));
}

#[test]
fn a_session_streams_recovers_and_ends_direct_submit() {
    run_session(true, None);
}

#[test]
fn a_session_streams_recovers_and_ends_through_the_decode_queue() {
    run_session(false, None);
}

#[test]
fn packed_chroma_recovers_packet_loss_and_key_frames_with_logical_decoder_geometry() {
    run_session(false, Some(VIDEO_FORMAT_AV1_MAIN8));
}

#[test]
fn stop_disconnects_the_control_stream() {
    let host = start_host(None);
    let record = Arc::new(Mutex::new(Record::default()));
    let make = || {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        })
    };
    let session = Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        SessionConfig {
            stream: StreamConfig {
                remote_input_aes_key: KEY,
                ..Default::default()
            },
            location: Location::Local,
            input_only: true,
            ..Default::default()
        },
        make(),
        make(),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap();
    assert_eq!(session.outcome().config.packet_size, 1392);
    session
        .input(|q| q.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    let r = record.lock().unwrap();
    assert_eq!(r.terminated, None);
    assert!(r.setup.is_none() && r.audio_setup.is_none());
}

/// Vibepollo and Apollo hosts run their video thread for an input-only session too and end it
/// when no video ping arrives within ten seconds.
#[test]
fn input_only_pings_the_video_port_until_stop() {
    let host = start_host(None);
    let record = Arc::new(Mutex::new(Record::default()));
    let make = || {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        })
    };
    let session = Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        SessionConfig {
            stream: StreamConfig {
                remote_input_aes_key: KEY,
                ..Default::default()
            },
            location: Location::Local,
            input_only: true,
            ..Default::default()
        },
        make(),
        make(),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap();
    // The payload from the RTSP SETUP and a sequence number that counts up.
    let video_pings = || -> Vec<u32> {
        let media = host.media.lock().unwrap();
        let pings = media.pings.iter().filter(|(m, ..)| *m == Media::Video);
        pings.map(|&(_, _, sequence)| sequence).collect()
    };
    until("two video pings", || video_pings().len() >= 2);
    assert_eq!(video_pings()[..2], [1, 2]);
    // The session is up and decoder and renderer were never touched.
    session
        .input(|q| q.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    {
        let r = record.lock().unwrap();
        assert!(r.setup.is_none() && r.audio_setup.is_none());
    }
    session.stop();
    until("the disconnect", || {
        host.control_disconnected.load(Ordering::SeqCst)
    });
    // Stop joined the ping thread: whatever was in flight is drained, then it is quiet.
    thread::sleep(Duration::from_millis(50));
    let pinged = video_pings().len();
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(video_pings().len(), pinged, "pings continued after stop");
    assert_eq!(record.lock().unwrap().terminated, None);
}

/// Unbatched mouse motion leaves at the mouse's pace: 1 kHz moves sent from another thread (as
/// the desktop client's mouse thread does) reach the host in packets of their own about 1 ms
/// apart, not merged until the host's next datagram or the control thread's service tick, and
/// the desktop's 0.5 ms floor for relative motion holds none of them back (on an idle machine).
/// The tests running beside it can starve the control thread, which then merges moves as it
/// should; the best of three tries counts.
#[test]
fn unbatched_mouse_motion_reaches_the_host_at_the_mouse_pace() {
    let host = start_host(None);
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, roaming_config(), &record);
    input_arrives(&session, &host);
    session.input(|q| {
        q.set_unbatched(true);
        q.set_relative_motion_floor(nightjar_core::input::queue::FAST_MOUSE_FLOOR);
    });
    let moves = 300u32;
    let mut best = (0, f64::MAX);
    for _ in 0..3 {
        let input = session.input_handle();
        let start = Instant::now();
        thread::spawn(move || {
            for i in 0..moves {
                let due = start + Duration::from_millis(u64::from(i));
                while Instant::now() < due {
                    std::hint::spin_loop();
                }
                input.input(|q| q.send_mouse_move(1, 0)).unwrap();
            }
        })
        .join()
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        let arrivals: Vec<Instant> = host
            .input_arrivals
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|&at| at >= start)
            .collect();
        let mut gaps: Vec<f64> = arrivals
            .windows(2)
            .map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0)
            .collect();
        gaps.sort_by(f64::total_cmp);
        let p90 = gaps.get(gaps.len() * 9 / 10).copied().unwrap_or(f64::MAX);
        eprintln!(
            "{moves} moves arrived in {} packets, gap p90 {p90:.2} ms",
            arrivals.len()
        );
        let passed = arrivals.len() as u32 >= moves * 3 / 4 && p90 < 3.0;
        if passed || arrivals.len() > best.0 {
            best = (arrivals.len(), p90);
        }
        if passed {
            break;
        }
    }
    let (packets, p90) = best;
    assert!(
        packets as u32 >= moves * 3 / 4,
        "{packets} of {moves} moves arrived in packets of their own"
    );
    assert!(p90 < 3.0, "gap p90 {p90:.2} ms");
    session.stop();
}
