//! Interop with the Rubylight host's own protocol code (butterpollo-core): the stream
//! description must negotiate what the client intends, the host's DESCRIBE answer must read
//! back, and pairing, HTTPS and the RTSP handshake must work end to end against servers built
//! from the host's pairing and RTSP logic.

use butterpollo_core::{crypto, mic, pairing::Pairing, rtsp as host_rtsp};
use nightjar_core::http::{
    AppControl, ClientIdentity, Error as HttpError, HostClient, LaunchRequest, PairResult,
};
use nightjar_core::rtsp::{self, DescribeInfo, HandshakeParams, MicrophoneState};
use nightjar_core::sdp::*;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

fn client_identity() -> Arc<ClientIdentity> {
    static ID: OnceLock<Arc<ClientIdentity>> = OnceLock::new();
    ID.get_or_init(|| Arc::new(ClientIdentity::generate().unwrap()))
        .clone()
}

fn host_identity() -> &'static crypto::Identity {
    static ID: OnceLock<crypto::Identity> = OnceLock::new();
    ID.get_or_init(|| crypto::Identity::generate().unwrap())
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

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
        video_fec_percentage: None,
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
        let config = StreamConfig { pyrowave_row_order: row_order, ..Default::default() };
        let n = host_rtsp::Negotiated::from_sdp(sdp_for(&config, SUNSHINE, format, 7, 0).as_bytes()).unwrap();
        assert_eq!(n.pyrowave_row_order, expected, "{format:#x} row_order={row_order}");
    }
    for (width, height, chroma444) in [(1920, 1080, false), (1920, 1080, true), (3840, 2160, false), (3840, 2160, true), (1280, 720, false), (1000, 500, true), (640, 480, false)] {
        assert_eq!(
            rubylight_protocol::pyrowave::rows::row_steps(width, height, chroma444),
            butterpollo_core::pyrowave::row_steps(width, height, chroma444),
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
    let mut answer = host_rtsp::describe(1, 7, 7, true, true, true, true);
    answer.push_str("a=x-nv-video[0].refPicInvalidation:1\r\n");
    answer.push_str(&mic::sdp(48012));
    let info = DescribeInfo::parse(&answer);
    assert!(info.hevc && info.av1 && info.pyrowave && info.reference_frame_invalidation);
    assert_eq!(info.sunshine_feature_flags, 1);
    assert_eq!(info.encryption_supported, 7 | mic::ENCRYPTION);
    assert_eq!(info.encryption_requested, 7 | mic::ENCRYPTION);
    assert_eq!(
        info.control_messages,
        [0x5530, 0x5531, 0x5532, 0x5533, 0x5534, 0x5535, 0x5536, 0x5537]
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

    let plain = host_rtsp::describe(0, 5, 1, false, false, false, false);
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
                    let mut answer = host_rtsp::describe(0, 7, 1, true, true, false, true);
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
    assert_eq!(outcome.host_control_messages.len(), 8);

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

// ---------------------------------------------------------------------------------------
// Pairing and HTTPS against a host built from butterpollo-core's pairing (rust/host/src/nvhttp.rs).

const PIN: &str = "4321";

#[derive(Default)]
struct HostState {
    pairing: Option<Pairing>,
    paired_certificate: Option<Vec<u8>>,
    unpairs: u32,
    launches: Vec<HashMap<String, String>>,
    clipboard: String,
    reject_clipboard: bool,
    bitrate_reply: Option<String>,
}

fn decode_query(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let decode = |s: &str| {
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(if b[i] == b'+' { b' ' } else { b[i] });
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    };
    let args = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), decode(v)))
        .collect();
    (path.trim_start_matches('/').to_owned(), args)
}

fn xml(code: u16, fields: &[(&str, String)]) -> String {
    let mut s = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"{code}\">");
    for (k, v) in fields {
        s.push_str(&format!("<{k}>{v}</{k}>"));
    }
    s + "</root>"
}

fn handle(state: &Mutex<HostState>, target: &str, peer: Option<&[u8]>, https_port: u16) -> String {
    let (path, args) = decode_query(target);
    let mut state = state.lock().unwrap();
    let identity = host_identity();
    let paired = peer.is_some() && peer == state.paired_certificate.as_deref();
    assert!(
        args.contains_key("uniqueid") && args.contains_key("uuid"),
        "{target}"
    );
    match path.as_str() {
        "serverinfo" => xml(
            200,
            &[
                ("hostname", "Test PC".into()),
                ("appversion", "7.1.431.-1".into()),
                ("GfeVersion", "3.23.0.74".into()),
                ("uniqueid", "HOST-1".into()),
                ("HttpsPort", https_port.to_string()),
                ("PairStatus", u8::from(paired).to_string()),
                ("currentgame", if state.launches.is_empty() { "0" } else { "881448767" }.into()),
                ("state", if state.launches.is_empty() { "SUNSHINE_SERVER_FREE" } else { "SUNSHINE_SERVER_BUSY" }.into()),
                ("ServerCodecModeSupport", "65793".into()),
                ("RustHostVersion", "2.2.21".into()),
                ("Permission", if paired { "119480064" } else { "0" }.into()),
            ],
        ),
        "pair" if args.get("phrase").map(String::as_str) == Some("getservercert") => {
            assert_eq!(args["devicename"], "Nightjar test");
            let certificate = String::from_utf8(unhex(&args["clientcert"])).unwrap();
            let salt = unhex(&args["salt"]);
            let pairing = Pairing::new(args["uniqueid"].clone(), args["devicename"].clone(), certificate, &salt, PIN);
            match pairing {
                Ok(p) => {
                    state.pairing = Some(p);
                    xml(200, &[("paired", "1".into()), ("plaincert", hex(identity.certificate.as_bytes()))])
                }
                Err(e) => xml(400, &[("paired", format!("0 {e}"))]),
            }
        }
        "pair" if args.get("phrase").map(String::as_str) == Some("pairchallenge") => {
            xml(200, &[("paired", u8::from(paired).to_string())])
        }
        "pair" => {
            let mut p = state.pairing.take().expect("no pending pairing");
            let result = if let Some(v) = args.get("clientchallenge") {
                p.client_challenge(identity, &unhex(v)).map(|r| ("challengeresponse", hex(&r)))
            } else if let Some(v) = args.get("serverchallengeresp") {
                p.server_response(identity, &unhex(v)).map(|r| ("pairingsecret", hex(&r)))
            } else if let Some(v) = args.get("clientpairingsecret") {
                p.finish(&unhex(v)).map(|()| ("paired", "1".into()))
            } else {
                panic!("unknown pairing phase {target}");
            };
            match result {
                Ok(("paired", _)) => {
                    state.paired_certificate =
                        Some(CertificateDer::from_pem_slice(p.certificate.as_bytes()).unwrap().to_vec());
                    xml(200, &[("paired", "1".into())])
                }
                Ok((field, value)) => {
                    state.pairing = Some(p);
                    xml(200, &[("paired", "1".into()), (field, value)])
                }
                Err(e) => xml(400, &[("paired", "0".into()), ("error", e.to_string())]),
            }
        }
        "unpair" => {
            state.unpairs += 1;
            state.pairing = None;
            if paired {
                state.paired_certificate = None;
            }
            xml(200, &[("unpaired", u8::from(paired).to_string())])
        }
        _ if !paired => xml(401, &[]),
        "applist" => "<?xml version=\"1.0\"?><root status_code=\"200\">\
            <App><AppTitle>Desktop</AppTitle><UUID>u-1</UUID><IDX>0</IDX><ID>881448767</ID><ArtVersion>1</ArtVersion>\
            <IsHdrSupported>1</IsHdrSupported></App>\
            <App><AppTitle>Terminate</AppTitle><UUID></UUID><IDX>1</IDX><ID>2147483504</ID></App></root>"
            .into(),
        "launch" => {
            state.launches.push(args);
            xml(
                200,
                &[("gamesession", "1".into()), ("sessionUrl0", "rtspenc://127.0.0.1:48010".into())],
            )
        }
        "cancel" => {
            state.launches.clear();
            xml(200, &[("cancel", "1".into())])
        }
        "appasset" => format!("PNG:{}", args["appid"]),
        "bitrate" => xml(200, &[("bitrate", state.bitrate_reply.clone().unwrap_or_else(|| {
            args["bitrate"].parse::<u32>().unwrap().min(80_000).to_string()
        }))]),
        other => panic!("unexpected request {other}"),
    }
}

fn respond(stream: &mut impl Write, body: &str) {
    let content_type = if body.starts_with("PNG") {
        "image/png"
    } else {
        "application/xml"
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn read_head(stream: &mut impl Read) -> Option<String> {
    Some(read_headers(stream)?.split(' ').nth(1)?.to_owned())
}

fn read_headers(stream: &mut impl Read) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    Some(String::from_utf8(head).unwrap())
}

#[derive(Debug)]
struct AnyClientCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::server::danger::ClientCertVerifier for AnyClientCertificate {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

struct TestHost {
    http_port: u16,
    https_port: u16,
    state: Arc<Mutex<HostState>>,
}

fn start_test_host() -> TestHost {
    let state = Arc::new(Mutex::new(HostState::default()));
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let https = TcpListener::bind("127.0.0.1:0").unwrap();
    let (http_port, https_port) = (
        http.local_addr().unwrap().port(),
        https.local_addr().unwrap().port(),
    );

    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in http.incoming() {
            let mut stream = stream.unwrap();
            if let Some(target) = read_head(&mut stream) {
                let body = handle(&shared, &target, None, https_port);
                respond(&mut stream, &body);
            }
        }
    });

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let identity = host_identity();
    let config = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(Arc::new(AnyClientCertificate(provider)))
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(identity.certificate.as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(identity.private_pem.as_bytes()).unwrap(),
        )
        .unwrap();
    let config = Arc::new(config);
    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in https.incoming() {
            let tcp = stream.unwrap();
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let Some(headers) = read_headers(&mut tls) else {
                continue;
            };
            let target = headers.split(' ').nth(1).unwrap();
            if target.starts_with("/pyrowave-udp-probe?") {
                use nightjar_core::http::pace;
                let (_, args) = decode_query(target);
                assert_eq!(tls.conn.peer_certificates().unwrap()[0].as_ref(), shared.lock().unwrap().paired_certificate.as_ref().unwrap());
                assert_eq!(args["handshake"], "1");
                let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
                socket.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
                let kbps: u32 = args["kbps"].parse().unwrap();
                let packet_size: u32 = args["packetsize"].parse().unwrap();
                let expected = pace::expected_packets(kbps, packet_size);
                let body = xml(200, &[("expected", expected.to_string()), ("sent", expected.to_string()), ("elapsedMs", "2000".into())]);
                write!(tls, "HTTP/1.1 200 OK\r\nX-PyroWave-Udp-Port: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", socket.local_addr().unwrap().port(), body.len()).unwrap();
                tls.flush().unwrap();
                let mut hello = [0; 64];
                let (length, peer) = socket.recv_from(&mut hello).unwrap();
                assert_eq!(&hello[..length], args["token"].as_bytes());
                assert_eq!(peer.port(), args["port"].parse::<u16>().unwrap());
                let mut packet = vec![0; packet_size as usize + 48];
                packet[..32].copy_from_slice(args["token"].as_bytes());
                packet[32..36].copy_from_slice(&5u32.to_be_bytes());
                let other = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
                other.send_to(&packet, peer).unwrap();
                packet[0] ^= 1;
                socket.send_to(&packet, peer).unwrap();
                packet[0] ^= 1;
                for seq in (0..expected).rev() {
                    if seq == 5 { continue; }
                    packet[32..36].copy_from_slice(&seq.to_be_bytes());
                    socket.send_to(&packet, peer).unwrap();
                    if seq == 7 { socket.send_to(&packet, peer).unwrap(); }
                    if seq % 16 == 0 { std::thread::sleep(std::time::Duration::from_millis(1)); }
                }
                tls.write_all(body.as_bytes()).unwrap();
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            if target.starts_with("/bandwidth-probe?") {
                assert_eq!(tls.conn.peer_certificates().unwrap()[0].as_ref(), shared.lock().unwrap().paired_certificate.as_ref().unwrap());
                assert!(target.starts_with("/bandwidth-probe?bytes=4194304&uniqueid="), "{target}");
                // As the host past its 3 s cap: chunked, and the body ends before the bytes asked for.
                write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
                for _ in 0..16 { write!(tls, "10000\r\n").unwrap(); tls.write_all(&[7; 65536]).unwrap(); write!(tls, "\r\n").unwrap(); }
                write!(tls, "0\r\n\r\n").unwrap();
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            if target.starts_with("/pyrowave-bandwidth-probe?") {
                assert_eq!(tls.conn.peer_certificates().unwrap()[0].as_ref(), shared.lock().unwrap().paired_certificate.as_ref().unwrap());
                write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 33554432\r\nConnection: close\r\n\r\n").unwrap();
                for _ in 0..4096 { tls.write_all(&[0; 8192]).unwrap(); }
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            if target.starts_with("/actions/clipboard?") {
                let mut state = shared.lock().unwrap();
                assert_eq!(tls.conn.peer_certificates().unwrap()[0].as_ref(), state.paired_certificate.as_ref().unwrap());
                assert!(target.contains("type=text") && target.contains("uniqueid="));
                // Read a POST body before answering, even when refusing it: closing a socket with unread
                // data can reset the connection, and the client would see a network error, not the 404.
                let posted = if headers.starts_with("POST ") {
                    let length: usize = headers.lines().find_map(|line| line.strip_prefix("Content-Length: ")).unwrap().trim().parse().unwrap();
                    assert!(headers.contains("Content-Type: text/plain; charset=utf-8\r\n"));
                    let mut bytes = vec![0; length];
                    tls.read_exact(&mut bytes).unwrap();
                    Some(String::from_utf8(bytes).unwrap())
                } else { None };
                if state.reject_clipboard {
                    respond(&mut tls, "<root status_code=\"404\"/>");
                } else {
                    let body = if let Some(text) = posted {
                        state.clipboard = text;
                        ""
                    } else { state.clipboard.as_str() };
                    write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            let peer = tls
                .conn
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| c.to_vec());
            let body = handle(&shared, target, peer.as_deref(), https_port);
            respond(&mut tls, &body);
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    TestHost {
        http_port,
        https_port,
        state,
    }
}

#[test]
fn pairing_https_launch_and_cancel_against_the_hosts_pairing() {
    let host = start_test_host();
    let client = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_device_name("Nightjar test");
    let info = client.server_info(true).unwrap();
    assert!(!info.paired);
    assert_eq!(info.https_port, host.https_port);
    assert!(matches!(
        client.app_list(&info.capabilities),
        Err(HttpError::NotPaired)
    ));

    let result = client.pair(&info, PIN).unwrap();
    let PairResult::Paired {
        server_certificate_pem,
    } = result
    else {
        panic!("{result:?}")
    };
    assert_eq!(server_certificate_pem, host_identity().certificate);
    assert_eq!(host.state.lock().unwrap().unpairs, 0);

    // Pinned HTTPS with the client certificate.
    let info = client.server_info(true).unwrap();
    assert!(info.paired);
    assert_eq!(info.permission, Some(119_480_064));
    let apps = client.app_list(&info.capabilities).unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[1].control, AppControl::Terminate);
    assert_eq!(client.box_art(apps[0].id).unwrap(), b"PNG:881448767");
    assert!(client
        .box_art_url(apps[0].id)
        .unwrap()
        .starts_with(&format!(
            "https://127.0.0.1:{}/appasset?appid=881448767&AssetType=2&AssetIdx=0&uniqueid=",
            host.https_port
        )));

    let request = LaunchRequest {
        app_id: apps[0].id,
        app_uuid: apps[0].uuid.clone(),
        ri_key: [9; 16],
        ri_key_id: 77,
        client_refresh_rate_x100: 5994,
        ..Default::default()
    };
    let launched = client.launch(&request, &info).unwrap().unwrap();
    assert_eq!(
        launched.rtsp_session_url.as_deref(),
        Some("rtspenc://127.0.0.1:48010")
    );
    let args = host.state.lock().unwrap().launches[0].clone();
    assert_eq!(args["appuuid"], "u-1");
    assert_eq!(args["mode"], "1920x1080x59.94");
    assert_eq!(args["rikey"], "09".repeat(16));
    assert_eq!(args["rikeyid"], "77");
    assert_eq!(args["corever"], "1");
    // The stream key ID reads the way the host reads it.
    assert!(client.cancel().unwrap());

    // A client stored with the pinned certificate reconnects without pairing again.
    let reloaded = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&server_certificate_pem)
        .unwrap();
    assert!(reloaded.server_info_https().unwrap().paired);

    // Another host certificate is refused over HTTPS; serverinfo falls back to HTTP.
    let other = crypto::Identity::generate().unwrap();
    let mismatched = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_server_certificate_pem(&other.certificate)
        .unwrap();
    assert!(matches!(
        mismatched.server_info_https(),
        Err(HttpError::CertificateMismatch)
    ));
    assert!(!mismatched.server_info(true).unwrap().paired);

    // A different client identity is not paired and gets 401 for everything else.
    let stranger = HostClient::new(
        "127.0.0.1",
        host.http_port,
        Arc::new(ClientIdentity::generate().unwrap()),
    )
    .unwrap()
    .with_server_certificate_pem(&server_certificate_pem)
    .unwrap();
    assert!(!stranger.server_info(true).unwrap().paired);
    assert!(matches!(
        stranger.app_list(&info.capabilities),
        Err(HttpError::Status { code: 401, .. })
    ));

    reloaded.unpair().unwrap();
    assert!(!client.server_info(true).unwrap().paired);
}

#[test]
fn a_wrong_pin_is_detected_and_the_pairing_cancelled() {
    let host = start_test_host();
    let client = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_device_name("Nightjar test");
    let info = client.server_info(true).unwrap();
    assert_eq!(client.pair(&info, "0000").unwrap(), PairResult::PinWrong);
    let state = host.state.lock().unwrap();
    assert_eq!(
        state.unpairs, 1,
        "the client ends the host's pending pairing over HTTP"
    );
    assert!(state.paired_certificate.is_none());
    drop(state);
    assert!(
        client.server_certificate_pem().is_none(),
        "a failed pairing pins nothing"
    );
}

#[test]
fn clipboard_uses_pinned_https_utf8_and_rejects_xml_errors() {
    let host = start_test_host();
    let identity = client_identity();
    host.state.lock().unwrap().paired_certificate = Some(identity.certificate_der().to_vec());
    let client = HostClient::new("127.0.0.1", host.http_port, identity).unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&host_identity().certificate).unwrap();
    let text = "Nightjar — Ελληνικά\nsecond line\r\n日本語";
    client.send_clipboard(text).unwrap();
    assert_eq!(client.clipboard().unwrap(), text);
    client.send_clipboard("").unwrap();
    assert_eq!(client.clipboard().unwrap(), "");
    assert!(matches!(client.send_clipboard(&"x".repeat(1024 * 1024 + 1)), Err(HttpError::InvalidArgument(_))));
    host.state.lock().unwrap().reject_clipboard = true;
    assert!(matches!(client.clipboard(), Err(HttpError::Malformed(_))));
    let refused = client.send_clipboard("test");
    assert!(matches!(refused, Err(HttpError::Malformed(_))), "refused clipboard write: {refused:?}");
}

#[test]
fn runtime_bitrate_returns_the_applied_budget_and_rejects_invalid_replies() {
    let host = start_test_host();
    let identity = client_identity();
    host.state.lock().unwrap().paired_certificate = Some(identity.certificate_der().to_vec());
    let client = HostClient::new("127.0.0.1", host.http_port, identity).unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&host_identity().certificate).unwrap();
    assert_eq!(client.set_bitrate(90_000).unwrap(), 80_000);
    assert_eq!(client.set_bitrate(30_000).unwrap(), 30_000);
    for bitrate in [0, 2_000_001, u32::MAX] {
        assert!(matches!(client.set_bitrate(bitrate), Err(HttpError::InvalidArgument(_))));
    }
    for reply in ["0", "-1", "2000001", "invalid"] {
        host.state.lock().unwrap().bitrate_reply = Some(reply.into());
        assert!(matches!(client.set_bitrate(20_000), Err(HttpError::Malformed(_))));
    }
}

#[test]
fn paired_probes_authenticate_udp_packets_and_transfer_the_advertised_bytes() {
    let host = start_test_host();
    let identity = client_identity();
    host.state.lock().unwrap().paired_certificate = Some(identity.certificate_der().to_vec());
    let client = HostClient::new("127.0.0.1", host.http_port, identity.clone()).unwrap()
        .with_https_port(host.https_port).with_server_certificate_pem(&host_identity().certificate).unwrap();
    let result = client.probe_pace(5000, 1392, 60, 50_000).unwrap();
    assert_eq!(result.received, result.expected - 1, "duplicates, foreign ports and wrong tokens must not repair the missing packet");
    assert!(result.measured());
    let info = nightjar_core::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>probe-host</uniqueid><state>SERVER_FREE</state><PairStatus>1</PairStatus><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes></root>", host.http_port).unwrap();
    let mbps = client.probe_bandwidth(&info).unwrap();
    assert!(mbps.is_finite() && mbps > 0.0);
    // Every codec's probe: a body the host cut short still measures, and only a host that offers it is asked.
    assert!(matches!(client.probe_link(&info), Err(HttpError::InvalidArgument(_))));
    let offered = nightjar_core::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>probe-host</uniqueid><state>SERVER_FREE</state><PairStatus>1</PairStatus><BandwidthProbeVersion>1</BandwidthProbeVersion></root>", host.http_port).unwrap();
    let mbps = client.probe_link(&offered).unwrap();
    assert!(mbps.is_finite() && mbps > 0.0);
    let unpaired = HostClient::new("127.0.0.1", host.http_port, identity.clone()).unwrap().with_https_port(host.https_port);
    assert!(matches!(unpaired.probe_pace(5000, 1392, 60, 50_000), Err(HttpError::NotPaired)));
    assert!(matches!(unpaired.probe_bandwidth(&info), Err(HttpError::NotPaired)));
    let other = crypto::Identity::generate().unwrap();
    let wrong_pin = HostClient::new("127.0.0.1", host.http_port, identity).unwrap().with_https_port(host.https_port)
        .with_server_certificate_pem(&other.certificate).unwrap();
    assert!(matches!(wrong_pin.probe_bandwidth(&info), Err(HttpError::CertificateMismatch)));
}
