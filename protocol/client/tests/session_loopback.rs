//! A whole session against an in-process host on loopback: the Rubylight host's RTSP
//! helpers answer the handshake, a rusty_enet host plays the control stream with the host's
//! own sealing, and UDP sockets answer the video and audio pings with packetized frames and
//! Opus packets.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use butterpollo_core::packet::{decrypt_control, encrypted_control};
use butterpollo_core::rtsp as host_rtsp;
use nightjar_core::control::{ptype, ControlEvent};
use nightjar_core::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};
use nightjar_core::sdp::*;
use nightjar_core::session::{
    AudioRenderer, AudioSetup, DecodeStatus, HostInfo, Interrupt, Listener, Location, Session,
    SessionConfig, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::video::{Frame, FrameType};
use rusty_enet::{Event, Host, HostSettings, Packet};

const KEY: [u8; 16] = *b"session-test-key";
const PING: &[u8; 16] = b"0123456789abcdef";
const PACKET_SIZE: usize = 1024;

/// A single-block video packetizer like the host's (see `video/tests.rs`).
struct Packetizer {
    seq: u16,
    spi: u32,
    frame: u32,
}

impl Packetizer {
    fn frame(&mut self, payload: &[u8], kind: u8) -> Vec<Vec<u8>> {
        let slice = PACKET_SIZE - 16;
        let mut data = vec![1, 0, 0, kind];
        let total = payload.len() + 8;
        let last = if total % slice == 0 {
            slice
        } else {
            total % slice
        };
        data.extend_from_slice(&(last as u16).to_le_bytes());
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(payload);
        let count = total.div_ceil(slice);
        let parity = (count * 20).div_ceil(100);
        let mut shards: Vec<Vec<u8>> = (0..count + parity)
            .map(|i| {
                let mut s = vec![0u8; PACKET_SIZE + 16];
                if i < count {
                    s[16..20].copy_from_slice(&((self.spi + i as u32) << 8).to_le_bytes());
                    s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
                    s[26] = 0x10;
                    let chunk =
                        &data[(i * slice).min(data.len())..((i + 1) * slice).min(data.len())];
                    s[32..32 + chunk.len()].copy_from_slice(chunk);
                }
                s
            })
            .collect();
        let rs = ReedSolomon::new(count, parity).unwrap();
        let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
        rs.encode(&mut refs, PACKET_SIZE + 16).unwrap();
        for (i, s) in shards.iter_mut().enumerate() {
            s[0] = 0x90;
            s[2..4].copy_from_slice(&self.seq.wrapping_add(i as u16).to_be_bytes());
            s[4..8].copy_from_slice(&(self.frame * 1500).to_be_bytes());
            s[20..24].copy_from_slice(&self.frame.to_le_bytes());
            let info = ((i as u32) << 12) | ((count as u32) << 22) | (20 << 4);
            s[28..32].copy_from_slice(&info.to_le_bytes());
        }
        self.seq = self.seq.wrapping_add((count + parity) as u16);
        self.spi += count as u32;
        self.frame += 1;
        shards
    }
}

fn h264_idr() -> Vec<u8> {
    let mut f = vec![
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80,
    ];
    f.extend_from_slice(&[0, 0, 0, 1, 0x65]);
    f.extend((0..3000u32).map(|i| (i % 251) as u8 | 1));
    f
}

fn h264_p() -> Vec<u8> {
    let mut f = vec![0, 0, 0, 1, 0x41];
    f.extend((0..2000u32).map(|i| (i % 249) as u8 | 1));
    f
}

/// Audio data packets with two parity packets after every fourth (see `audio/tests.rs`).
fn audio_stream(count: u16) -> Vec<Vec<u8>> {
    let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
    let mut out = Vec::new();
    let mut block: Vec<Vec<u8>> = Vec::new();
    for seq in 0..count {
        let payload: Vec<u8> = (0..60).map(|i| (i as u16 ^ seq) as u8).collect();
        let mut p = vec![0x80, 97];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&(u32::from(seq) * 5).to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(&payload);
        out.push(p);
        block.push(payload);
        if block.len() == 4 {
            let mut shards = block.clone();
            shards.push(vec![0; 60]);
            shards.push(vec![0; 60]);
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, 60).unwrap();
            for (index, shard) in shards[4..].iter().enumerate() {
                let mut p = vec![0x80, 127];
                p.extend_from_slice(&(seq + 1 + index as u16).to_be_bytes());
                p.extend_from_slice(&[0; 8]);
                p.push(index as u8);
                p.push(97);
                p.extend_from_slice(&(seq - 3).to_be_bytes());
                p.extend_from_slice(&(u32::from(seq - 3) * 5).to_be_bytes());
                p.extend_from_slice(&[0; 4]);
                p.extend_from_slice(shard);
                out.push(p);
            }
            block.clear();
        }
    }
    out
}

/// Waits for a ping carrying the RTSP payload and returns where it came from.
fn await_ping(socket: &UdpSocket) -> SocketAddr {
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0u8; 64];
    loop {
        let (n, from) = socket.recv_from(&mut buffer).expect("a ping");
        if n == 20 && &buffer[..16] == PING {
            return from;
        }
    }
}

struct FakeHost {
    rtsp_port: u16,
    video: UdpSocket,
    audio: UdpSocket,
    /// Message types the control stream received, in order.
    control_messages: Arc<Mutex<Vec<u16>>>,
    control_disconnected: Arc<AtomicBool>,
    terminate: Arc<AtomicBool>,
}

fn start_host(packed_format: Option<u32>) -> FakeHost {
    let video = UdpSocket::bind("127.0.0.1:0").unwrap();
    let audio = UdpSocket::bind("127.0.0.1:0").unwrap();
    let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = (
        video.local_addr().unwrap().port(),
        audio.local_addr().unwrap().port(),
        control_socket.local_addr().unwrap().port(),
    );

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let rtsp_port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for socket in listener.incoming() {
            let mut socket = socket.unwrap();
            let mut message = host_rtsp::MessageBuffer::default();
            let mut chunk = [0u8; 4096];
            loop {
                let n = socket.read(&mut chunk).unwrap();
                assert!(n > 0, "client closed mid-request");
                if message.append(&chunk[..n]).unwrap() {
                    break;
                }
            }
            let request = host_rtsp::Request::parse(&message.into_bytes()).unwrap();
            let mut headers: Vec<(&str, String)> = Vec::new();
            let mut body = Vec::new();
            match request.method.as_str() {
                "DESCRIBE" => {
                    // Control encryption v2 only: plain audio and video.
                    let mut describe =
                        host_rtsp::describe(0, 1, 0, false, packed_format.is_some(), false, false);
                    if packed_format.is_some() {
                        describe.push_str(
                            "a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:15\r\n",
                        );
                    }
                    body = describe.into_bytes();
                    headers.push(("Content-Type", "application/sdp".into()));
                }
                "SETUP" => {
                    let (port, control) = if request.target.contains("=audio") {
                        (ports.1, false)
                    } else if request.target.contains("=video") {
                        (ports.0, false)
                    } else {
                        (ports.2, true)
                    };
                    headers.push(("Session", "DEADBEEFCAFE;timeout = 90".into()));
                    headers.push(("Transport", format!("server_port={port}")));
                    if control {
                        headers.push(("X-SS-Connect-Data", "305419896".into()));
                    } else {
                        headers.push(("X-SS-Ping-Payload", "0123456789abcdef".into()));
                    }
                }
                _ => {}
            }
            let response = host_rtsp::response(request.cseq, 200, "OK", &headers, &body);
            socket.write_all(&response).unwrap();
        }
    });

    let control_messages = Arc::new(Mutex::new(Vec::new()));
    let control_disconnected = Arc::new(AtomicBool::new(false));
    let terminate = Arc::new(AtomicBool::new(false));
    let (messages, disconnected, terminating) = (
        control_messages.clone(),
        control_disconnected.clone(),
        terminate.clone(),
    );
    thread::spawn(move || {
        let settings = HostSettings {
            peer_limit: 4,
            channel_limit: 255,
            ..Default::default()
        };
        let mut host = Host::new(control_socket, settings).unwrap();
        let mut peer = None;
        let mut sequence = 0u32;
        loop {
            while let Some(event) = host.service().unwrap() {
                match event {
                    Event::Connect { peer: p, data } => {
                        assert_eq!(data, 305419896);
                        peer = Some(p.id());
                    }
                    Event::Receive { packet, .. } => {
                        let (_, kind, _) = decrypt_control(&KEY, packet.data(), true).unwrap();
                        messages.lock().unwrap().push(kind);
                    }
                    Event::Disconnect { .. } => {
                        disconnected.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
            if let Some(id) = peer {
                if terminating.swap(false, Ordering::SeqCst) {
                    sequence += 1;
                    // 0x0109 with a four-byte reason: 0x80030023 ends gracefully.
                    let datagram = encrypted_control(
                        &KEY,
                        sequence,
                        0x0109,
                        &0x8003_0023u32.to_be_bytes(),
                        true,
                    )
                    .unwrap();
                    host.peer_mut(id)
                        .send(0, &Packet::reliable(datagram))
                        .unwrap();
                    host.flush();
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    FakeHost {
        rtsp_port,
        video,
        audio,
        control_messages,
        control_disconnected,
        terminate,
    }
}

#[derive(Default)]
struct Record {
    setup: Option<VideoSetup>,
    frames: Vec<(u32, FrameType)>,
    audio_setup: Option<AudioSetup>,
    audio: Vec<Option<Vec<u8>>>,
    stages: Vec<Stage>,
    started: bool,
    terminated: Option<i32>,
    events: Vec<ControlEvent>,
    lifecycle: Vec<&'static str>,
    /// What the listener and the renderers saw, in order, up to the first frame.
    startup: Vec<String>,
    /// The DESCRIBE's packed modes as the listener got them, and whether the decoder was set up.
    packed_modes: Option<(u8, bool)>,
}

struct Recorder {
    record: Arc<Mutex<Record>>,
    need_idr_at: Option<u32>,
}

impl VideoDecoder for Recorder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.record.lock().unwrap().setup = Some(*setup);
        Ok(())
    }

    fn start(&mut self) {
        self.record.lock().unwrap().startup.push("video start".into());
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        self.record
            .lock()
            .unwrap()
            .frames
            .push((frame.number, frame.frame_type));
        if Some(frame.number) == self.need_idr_at {
            DecodeStatus::NeedIdr
        } else {
            DecodeStatus::Ok
        }
    }

    fn stop(&mut self) {
        self.record.lock().unwrap().lifecycle.push("video stop");
    }

    fn cleanup(&mut self) {
        self.record.lock().unwrap().lifecycle.push("video cleanup");
    }
}

impl AudioRenderer for Recorder {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        self.record.lock().unwrap().audio_setup = Some(*setup);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        self.record
            .lock()
            .unwrap()
            .audio
            .push(packet.map(<[u8]>::to_vec));
    }

    fn cleanup(&mut self) {
        self.record.lock().unwrap().lifecycle.push("audio cleanup");
    }
}

impl Listener for Recorder {
    fn stage_complete(&self, stage: Stage) {
        self.record.lock().unwrap().stages.push(stage);
    }

    fn host_control_messages(&self, messages: &[u16]) {
        self.record
            .lock()
            .unwrap()
            .startup
            .push(format!("control messages {messages:x?}"));
    }

    fn host_packed444_modes(&self, modes: u8) {
        let mut r = self.record.lock().unwrap();
        r.packed_modes = Some((modes, r.setup.is_some()));
    }

    fn connection_started(&self) {
        self.record.lock().unwrap().started = true;
    }

    fn connection_terminated(&self, error: i32) {
        self.record.lock().unwrap().terminated = Some(error);
    }

    fn control_event(&self, event: ControlEvent) {
        self.record.lock().unwrap().events.push(event);
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

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
    let client = await_ping(&host.video);
    let mut packetizer = Packetizer {
        seq: 100,
        spi: 0,
        frame: 1,
    };
    let idr = if packed_format.is_some() {
        include_bytes!("../../../app/src/main/assets/packed444/av1-8.bin").to_vec()
    } else {
        h264_idr()
    };
    let p = if packed_format.is_some() {
        vec![0x5a; 2500]
    } else {
        h264_p()
    };
    let frames = [(&idr, 2), (&p, 1), (&p, 1), (&p, 1), (&p, 1)];
    for (payload, kind) in &frames {
        for (index, packet) in packetizer.frame(payload, *kind).into_iter().enumerate() {
            if packed_format.is_some() && index == 1 {
                continue;
            }
            host.video.send_to(&packet, client).unwrap();
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
        host.video.send_to(&packet, client).unwrap();
    }
    until("the recovery frame", || {
        record.lock().unwrap().frames.len() >= 4
    });

    // Audio: 100 packets of backlog are dropped, the rest play in order.
    let client = await_ping(&host.audio);
    for packet in audio_stream(200) {
        host.audio.send_to(&packet, client).unwrap();
        thread::sleep(Duration::from_micros(200));
    }
    until("audio", || record.lock().unwrap().audio.len() >= 90);

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
        Some("control messages [5530, 5531, 5532, 5533, 5534, 5535, 5536, 5537]"),
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
    host.video
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0u8; 64];
    let mut sequences = Vec::new();
    while sequences.len() < 2 {
        let (n, _) = host.video.recv_from(&mut buffer).expect("a video ping");
        assert!(n == 20 && &buffer[..16] == PING, "{:?}", &buffer[..n]);
        sequences.push(u32::from_be_bytes(buffer[16..20].try_into().unwrap()));
    }
    assert_eq!(sequences, [1, 2]);
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
    host.video
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    while host.video.recv_from(&mut buffer).is_ok() {}
    host.video
        .set_read_timeout(Some(Duration::from_millis(1200)))
        .unwrap();
    assert!(host.video.recv_from(&mut buffer).is_err(), "pings continued after stop");
    assert_eq!(record.lock().unwrap().terminated, None);
}
