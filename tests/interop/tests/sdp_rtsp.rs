//! SDP and RTSP handshake interop between Nightjar and butterpollo-core's RTSP/mic helpers.
mod common;

use butterpollo_crypto::crypto;
use butterpollo_media_policy::rtsp as host_rtsp;
use butterpollo_session::mic;
use nightjar_core::rtsp::{self, DescribeInfo, HandshakeParams, MicrophoneState};
use nightjar_core::sdp::*;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------------------
// SDP: what the client sends must negotiate what it intends on the host.

fn sdp_for(
    config: &StreamConfig,
    version: [i32; 4],
    format: u32,
    supported: u32,
    mic_port: u16,
) -> String {
    generate(&SdpParams {
        config,
        app_version: version,
        address: "192.168.1.20".parse().unwrap(),
        rtsp_port: 48010,
        video_port: 47998,
        rtsp_client_version: 14,
        negotiated_video_format: format,
        video_capabilities: config.video_capabilities,
        encryption_supported: supported,
        encryption_requested: 1,
        mic_port,
        high_quality_surround_supported: true,
        pyrowave_records_enabled: format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
        hdr_format: 0,
        hdr_formats_offered: 0,
        audio_codec: 0,
        audio_codecs_offered: 0,
        audio_codec_accept: None,
        video_fec_percentage: None,
        max_video_packet: 0,
    })
    .text
}

const SUNSHINE: [i32; 4] = [7, 1, 431, -1];

#[test]
fn every_configuration_negotiates_as_intended_on_the_host() {
    struct Case {
        config: StreamConfig,
        version: [i32; 4],
        format: u32,
        codec: u8,
        hdr: bool,
        yuv444: bool,
        bitrate: u32,
        encryption: u32,
    }
    let base = StreamConfig {
        encryption_flags: ENCFLG_ALL,
        ..Default::default()
    };
    let cases = [
        Case {
            config: base.clone(),
            version: SUNSHINE,
            format: VIDEO_FORMAT_H264,
            codec: 0,
            hdr: false,
            yuv444: false,
            bitrate: 16000,
            encryption: 7,
        },
        Case {
            config: StreamConfig {
                width: 3840,
                height: 2160,
                fps: 120,
                bitrate_kbps: 150_000,
                audio: AudioConfiguration::SURROUND_71,
                ..base.clone()
            },
            version: SUNSHINE,
            format: VIDEO_FORMAT_AV1_HIGH10_444,
            codec: 2,
            hdr: true,
            yuv444: true,
            bitrate: 100_000,
            encryption: 7,
        },
        Case {
            config: StreamConfig {
                location: StreamLocation::Remote,
                packet_size: 1024,
                audio: AudioConfiguration::SURROUND_51,
                ..base.clone()
            },
            version: SUNSHINE,
            format: VIDEO_FORMAT_H265_MAIN10,
            codec: 1,
            hdr: true,
            yuv444: false,
            bitrate: 15500,
            encryption: 7,
        },
        Case {
            config: StreamConfig {
                width: 2560,
                height: 1440,
                fps: 240,
                bitrate_kbps: 800_000,
                pyrowave_link_mbps: 2500,
                pyrowave_pace_mbps: 2100,
                ..base.clone()
            },
            version: SUNSHINE,
            format: VIDEO_FORMAT_PYROWAVE_MAIN10_444,
            codec: 3,
            hdr: true,
            yuv444: true,
            bitrate: 100_000,
            encryption: 7,
        },
        Case {
            config: StreamConfig {
                width: 640,
                height: 480,
                encryption_flags: ENCFLG_NONE,
                ..base.clone()
            },
            version: SUNSHINE,
            format: VIDEO_FORMAT_H265,
            codec: 1,
            hdr: false,
            yuv444: false,
            bitrate: 16000,
            encryption: 1,
        },
        // Native 4:4:4 as an NVENC host encodes it: H.264 High 4:4:4 and HEVC RExt 10-bit.
        Case {
            config: base.clone(),
            version: SUNSHINE,
            format: VIDEO_FORMAT_H264_HIGH8_444,
            codec: 0,
            hdr: false,
            yuv444: true,
            bitrate: 16000,
            encryption: 7,
        },
        Case {
            config: base.clone(),
            version: SUNSHINE,
            format: VIDEO_FORMAT_H265_REXT10_444,
            codec: 1,
            hdr: true,
            yuv444: true,
            bitrate: 16000,
            encryption: 7,
        },
        Case {
            config: base.clone(),
            version: [7, 1, 431, 0],
            format: VIDEO_FORMAT_H265,
            codec: 1,
            hdr: false,
            yuv444: false,
            bitrate: 16000,
            encryption: 4,
        },
    ];
    for (i, case) in cases.iter().enumerate() {
        let text = sdp_for(&case.config, case.version, case.format, 7, 0);
        let n = host_rtsp::Negotiated::from_sdp(text.as_bytes())
            .unwrap_or_else(|e| panic!("case {i}: {e:#}\n{text}"));
        let c = &case.config;
        assert_eq!(
            (n.width, n.height, n.fps),
            (c.width as u32, c.height as u32, c.fps as u32),
            "case {i}"
        );
        assert_eq!(
            (n.codec, n.hdr, n.yuv444),
            (case.codec, case.hdr, case.yuv444),
            "case {i}"
        );
        assert_eq!(n.bitrate_kbps, case.bitrate, "case {i}");
        assert_eq!(n.encryption, case.encryption, "case {i}");
        assert_eq!(n.audio_channels, c.audio.channel_count, "case {i}");
        let expected_packet = if case.encryption & SS_ENC_VIDEO != 0 {
            c.packet_size - 32
        } else {
            c.packet_size
        };
        assert_eq!(n.packet_size, expected_packet as usize, "case {i}");
        assert_eq!(n.min_fec, 2, "case {i}");
        assert_eq!(n.reliable_control, 13, "case {i}");
        let local = c.location == StreamLocation::Local;
        assert_eq!((n.video_qos, n.audio_qos), (local, local), "case {i}");
        assert_eq!(
            n.csc_mode,
            ((c.color_space << 1) | c.color_range) as u8,
            "case {i}"
        );
        if case.version[3] < 0 {
            assert_eq!(n.configured_bitrate_kbps, c.bitrate_kbps as u32, "case {i}");
            assert_eq!(
                n.moonlight_features,
                ML_FF_FEC_STATUS | ML_FF_SESSION_ID_V1,
                "case {i}"
            );
        }
        if case.codec == 3 {
            assert!(n.pyrowave_records);
            assert_eq!(
                (n.pyrowave_client_link_mbps, n.pyrowave_client_pace_mbps),
                (2500, 2100)
            );
        }
        // High-quality surround at 15 Mbps and more on a LAN.
        assert_eq!(
            n.audio_quality,
            c.audio.channel_count > 2 && c.bitrate_kbps >= 15000,
            "case {i}"
        );
    }
}

/// The host reads the row order request, and sends records by the same step table the client
/// uses to follow them.
#[test]
fn row_order_is_requested_from_the_host_and_both_sides_share_the_step_table() {
    for (format, row_order, expected) in [
        (VIDEO_FORMAT_PYROWAVE, true, true),
        (VIDEO_FORMAT_PYROWAVE, false, false),
        (VIDEO_FORMAT_H264, true, false),
    ] {
        let config = StreamConfig {
            pyrowave_row_order: row_order,
            ..Default::default()
        };
        let n =
            host_rtsp::Negotiated::from_sdp(sdp_for(&config, SUNSHINE, format, 7, 0).as_bytes())
                .unwrap();
        assert_eq!(
            n.pyrowave_row_order, expected,
            "{format:#x} row_order={row_order}"
        );
    }
    for (width, height, chroma444) in [
        (1920, 1080, false),
        (1920, 1080, true),
        (3840, 2160, false),
        (3840, 2160, true),
        (1280, 720, false),
        (1000, 500, true),
        (640, 480, false),
    ] {
        assert_eq!(
            rubylight_protocol::pyrowave::rows::row_steps(width, height, chroma444),
            butterpollo_media_policy::pyrowave::row_steps(width, height, chroma444),
            "{width}x{height} 444={chroma444}"
        );
    }
}

#[test]
fn fractional_rates_and_microphone_encryption_reach_the_host() {
    let config = StreamConfig {
        fps: 60,
        client_refresh_rate_x100: 5994,
        haptics_pcm: true,
        ..Default::default()
    };
    let n = host_rtsp::Negotiated::from_sdp(
        sdp_for(&config, SUNSHINE, VIDEO_FORMAT_H264, 15, 48001).as_bytes(),
    )
    .unwrap();
    assert_eq!(n.fps_millihz(), 59_940);
    assert_eq!(n.encryption & mic::ENCRYPTION, mic::ENCRYPTION);
    assert_eq!(
        n.moonlight_features,
        ML_FF_FEC_STATUS | ML_FF_SESSION_ID_V1 | ML_FF_HAPTICS_PCM
    );
    let config = StreamConfig {
        fps: 120,
        client_refresh_rate_x100: 5994,
        ..Default::default()
    };
    let n = host_rtsp::Negotiated::from_sdp(
        sdp_for(&config, SUNSHINE, VIDEO_FORMAT_H264, 7, 0).as_bytes(),
    )
    .unwrap();
    assert_eq!(n.fps_millihz(), 120_000);
}

#[test]
fn packed_formats_preserve_logical_geometry_and_native_codec_ids_on_the_host() {
    use rubylight_protocol::packed444;
    for (format, codec, ten_bit) in [
        (VIDEO_FORMAT_H265, 1, false),
        (VIDEO_FORMAT_H265_MAIN10, 1, true),
        (VIDEO_FORMAT_AV1_MAIN8, 2, false),
        (VIDEO_FORMAT_AV1_MAIN10, 2, true),
    ] {
        let config = StreamConfig {
            width: 2559,
            height: 1439,
            fps: 120,
            bitrate_kbps: 200_000,
            experimental_packed_444: true,
            packed444_modes: packed444::mode_bit(codec, ten_bit),
            supported_video_formats: format,
            color_space: if ten_bit {
                COLORSPACE_REC_2020
            } else {
                COLORSPACE_REC_709
            },
            ..Default::default()
        };
        let n =
            host_rtsp::Negotiated::from_sdp(sdp_for(&config, SUNSHINE, format, 7, 0).as_bytes())
                .unwrap();
        assert_eq!((n.width, n.height, n.fps), (2559, 1439, 120));
        assert_eq!(n.coded_size(), (2560, 2880));
        assert_eq!((n.codec, n.ten_bit(), n.yuv444), (codec, ten_bit, false));
        assert_eq!(n.chroma_packing, packed444::VERSION);
        assert_eq!(n.bitrate_kbps, 140_000);
        assert_eq!(
            n.csc_mode,
            ((config.color_space as u8) << 1) | config.color_range as u8
        );
        let plain = StreamConfig {
            experimental_packed_444: false,
            ..config
        };
        let plain_sdp = sdp_for(&plain, SUNSHINE, format, 7, 0);
        assert!(!plain_sdp.contains(packed444::REQUEST_ATTRIBUTE));
        let ordinary = host_rtsp::Negotiated::from_sdp(plain_sdp.as_bytes()).unwrap();
        assert_eq!(ordinary.chroma_packing, 0);
        assert_eq!(ordinary.bitrate_kbps, 100_000);
    }
}

#[test]
fn the_hosts_describe_answer_reads_back() {
    let mut answer = host_rtsp::describe(1, 7, 7, true, true, true, true, false);
    answer.push_str("a=x-nv-video[0].refPicInvalidation:1\r\n");
    answer.push_str(&mic::sdp(48012));
    let info = DescribeInfo::parse(&answer);
    assert!(info.hevc && info.av1 && info.pyrowave && info.reference_frame_invalidation);
    assert_eq!(info.sunshine_feature_flags, 1);
    assert_eq!(info.encryption_supported, 7 | mic::ENCRYPTION);
    assert_eq!(info.encryption_requested, 7 | mic::ENCRYPTION);
    assert_eq!(
        info.control_messages,
        [
            0x5530, 0x5531, 0x5532, 0x5533, 0x5534, 0x5535, 0x5536, 0x5537, 0x5538
        ]
    );
    assert_eq!(info.microphone, Some(48012));
    let all = 0x0F_FFFF;
    assert_eq!(
        info.select_video_format(all, SCM_PYROWAVE | SCM_PYROWAVE_444),
        VIDEO_FORMAT_PYROWAVE_444
    );
    for channels in [2, 6, 8] {
        let opus = rtsp::parse_opus_configs(&answer, channels).unwrap();
        assert_eq!(opus.normal.channel_count, channels);
        assert_eq!(opus.high_quality.is_some(), channels > 2);
    }

    let plain = host_rtsp::describe(0, 5, 1, false, false, false, false, false);
    let info = DescribeInfo::parse(&plain);
    assert!(!info.hevc && !info.av1 && !info.pyrowave && info.microphone.is_none());
    assert_eq!(
        info.select_video_format(all, 0x0FFF_FFFF),
        VIDEO_FORMAT_H264_HIGH8_444
    );
}

// ---------------------------------------------------------------------------------------
// RTSP over TCP against a server built from the host's RTSP code (rust/host/src/rtsp_server.rs).

struct RtspHost {
    port: u16,
    negotiated: Arc<Mutex<Option<host_rtsp::Negotiated>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

fn read_request(socket: &mut TcpStream, key: Option<[u8; 16]>) -> Option<Vec<u8>> {
    let mut first = [0u8; 4];
    socket.read_exact(&mut first).ok()?;
    let word = u32::from_be_bytes(first);
    if let Some(key) = key {
        assert!(word & 0x8000_0000 != 0, "encrypted launch got plain RTSP");
        let mut header = [0u8; 20];
        socket.read_exact(&mut header).unwrap();
        let mut body = vec![0u8; (word & 0x7fff_ffff) as usize];
        socket.read_exact(&mut body).unwrap();
        let sequence = u32::from_be_bytes(header[..4].try_into().unwrap());
        let mut iv = [0u8; 12];
        iv[..4].copy_from_slice(&sequence.to_le_bytes());
        iv[10] = b'C';
        iv[11] = b'R';
        return Some(crypto::gcm_open(&key, &iv, &header[4..], &body).unwrap());
    }
    let mut message = host_rtsp::MessageBuffer::default();
    let mut complete = message.append(&first).unwrap();
    let mut chunk = [0u8; 4096];
    while !complete {
        let n = socket.read(&mut chunk).unwrap();
        assert!(n > 0, "client closed mid-request");
        complete = message.append(&chunk[..n]).unwrap();
    }
    Some(message.into_bytes())
}

fn start_rtsp_host(key: Option<[u8; 16]>) -> RtspHost {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let negotiated = Arc::new(Mutex::new(None));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (stored, log) = (negotiated.clone(), requests.clone());
    std::thread::spawn(move || {
        let mut counter = 0u32;
        let mut mic_setup = false;
        for socket in listener.incoming() {
            let mut socket = socket.unwrap();
            let Some(raw) = read_request(&mut socket, key) else {
                continue;
            };
            let req = host_rtsp::Request::parse(&raw).unwrap();
            log.lock()
                .unwrap()
                .push(format!("{} {}", req.method, req.target));
            let mut headers: Vec<(&str, String)> = Vec::new();
            let mut body = Vec::new();
            let (mut code, mut reason) = (200, "OK");
            match req.method.as_str() {
                "OPTIONS" => headers.push((
                    "Public",
                    "OPTIONS, DESCRIBE, SETUP, ANNOUNCE, PLAY, TEARDOWN".into(),
                )),
                "DESCRIBE" => {
                    let mut answer = host_rtsp::describe(0, 7, 1, true, true, false, true, false);
                    answer.push_str("a=x-nv-video[0].refPicInvalidation:1\r\n");
                    answer.push_str(&mic::sdp(48012));
                    body = answer.into_bytes();
                    headers.push(("Content-Type", "application/sdp".into()));
                }
                "SETUP" => {
                    let port = if req.target.contains("=audio") {
                        48000
                    } else if req.target.contains("=video") {
                        47998
                    } else if req.target.contains("=control") {
                        47999
                    } else {
                        mic_setup = true;
                        48012
                    };
                    headers.push(("Session", "DEADBEEFCAFE;timeout = 90".into()));
                    headers.push(("Transport", format!("server_port={port}")));
                    if port == 47999 {
                        headers.push(("X-SS-Connect-Data", "305419896".into()));
                    } else {
                        headers.push(("X-SS-Ping-Payload", "0123456789abcdef".into()));
                    }
                }
                "ANNOUNCE" => {
                    let mut n = host_rtsp::Negotiated::from_sdp(&req.body).unwrap();
                    n.mic = mic_setup && n.encryption & mic::ENCRYPTION != 0;
                    if n.encryption & 6 != 6 {
                        (code, reason) = (
                            403,
                            "Required audio and video encryption was not negotiated",
                        );
                    }
                    *stored.lock().unwrap() = Some(n);
                }
                "PLAY" => assert!(
                    stored.lock().unwrap().is_some(),
                    "ANNOUNCE required before PLAY"
                ),
                _ => (code, reason) = (405, "Method Not Allowed"),
            }
            let mut response = host_rtsp::response(req.cseq, code, reason, &headers, &body);
            if let Some(key) = key {
                let mut iv = [0u8; 12];
                iv[..4].copy_from_slice(&counter.to_le_bytes());
                iv[10] = b'H';
                iv[11] = b'R';
                let (tag, sealed) = crypto::gcm_seal(&key, &iv, &response).unwrap();
                let mut framed = ((sealed.len() as u32) | 0x8000_0000).to_be_bytes().to_vec();
                framed.extend_from_slice(&counter.to_be_bytes());
                framed.extend_from_slice(&tag);
                framed.extend_from_slice(&sealed);
                response = framed;
                counter += 1;
            }
            socket.write_all(&response).unwrap();
        }
    });
    RtspHost {
        port,
        negotiated,
        requests,
    }
}

fn handshake_against(key: Option<[u8; 16]>) {
    let host = start_rtsp_host(key);
    let scheme = if key.is_some() { "rtspenc" } else { "rtsp" };
    let config = StreamConfig {
        width: 2560,
        height: 1440,
        fps: 120,
        bitrate_kbps: 60000,
        audio: AudioConfiguration::SURROUND_51,
        supported_video_formats: VIDEO_FORMAT_H264
            | VIDEO_FORMAT_H265
            | VIDEO_FORMAT_H265_MAIN10
            | VIDEO_FORMAT_AV1_MAIN8,
        encryption_flags: ENCFLG_AUDIO | ENCFLG_VIDEO,
        remote_input_aes_key: key.unwrap_or([0; 16]),
        microphone: true,
        video_capabilities: CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1,
        ..Default::default()
    };
    let outcome = rtsp::perform_handshake(
        HandshakeParams {
            address: IpAddr::from([127, 0, 0, 1]),
            ipv6_scope_id: 0,
            app_version: "7.1.431.-1".into(),
            server_codec_mode_support: SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8,
            rtsp_session_url: Some(format!("{scheme}://127.0.0.1:{}", host.port)),
            config,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        *host.requests.lock().unwrap(),
        [
            format!("OPTIONS {scheme}://127.0.0.1:{}", host.port),
            format!("DESCRIBE {scheme}://127.0.0.1:{}", host.port),
            "SETUP streamid=audio/0/0".into(),
            "SETUP streamid=video/0/0".into(),
            "SETUP streamid=control/13/0".into(),
            "SETUP streamid=mic/0/0".into(),
            "ANNOUNCE streamid=control/13/0".into(),
            "PLAY /".into(),
        ]
    );
    assert_eq!(outcome.negotiated_video_format, VIDEO_FORMAT_AV1_MAIN8);
    assert_eq!(
        (
            outcome.audio_port,
            outcome.video_port,
            outcome.control_port,
            outcome.mic_port
        ),
        (48000, 47998, 47999, 48012)
    );
    assert_eq!(outcome.microphone, MicrophoneState::On);
    assert_eq!(outcome.control_connect_data, 0x1234_5678);
    assert_eq!(outcome.audio_ping_payload, Some(*b"0123456789abcdef"));
    assert_eq!(outcome.session_id, "DEADBEEFCAFE");
    assert_eq!(outcome.encryption_enabled, 15);
    assert!(outcome.high_quality_surround_enabled);
    assert!(outcome.reference_frame_invalidation_enabled());
    assert_eq!(outcome.host_control_messages.len(), 9);

    let n = host.negotiated.lock().unwrap().clone().unwrap();
    assert_eq!((n.width, n.height, n.fps, n.codec), (2560, 1440, 120, 2));
    assert_eq!((n.encryption, n.mic), (15, true));
    assert_eq!(
        (n.audio_channels, n.audio_quality, n.audio_packet_ms),
        (6, true, 5)
    );
    assert_eq!(n.packet_size, outcome.config.packet_size as usize);
    assert_eq!(n.bitrate_kbps, 48000);
}

#[test]
fn rtsp_handshake_with_the_host() {
    handshake_against(None);
}

#[test]
fn encrypted_rtsp_handshake_with_the_host() {
    handshake_against(Some(core::array::from_fn(|i| i as u8 * 3)));
}
