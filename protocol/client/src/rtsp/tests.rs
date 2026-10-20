use super::*;
use crate::sdp::*;

const DESCRIBE: &str = "a=x-ss-general.featureFlags:3\r\na=x-ss-general.encryptionSupported:15\r\n\
    a=x-ss-general.encryptionRequested:9\r\na=x-rl-control:0x5530,0x5536\r\nsprop-parameter-sets=AAAAAU\r\n\
    a=fmtp:97 surround-params=21101\r\na=x-nv-video[0].refPicInvalidation:1\r\n\
    m=audio 48012 RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\n";

fn params(session_url: Option<&str>) -> HandshakeParams {
    HandshakeParams {
        address: "192.168.1.20".parse().unwrap(),
        ipv6_scope_id: 0,
        app_version: "7.1.431.-1".into(),
        server_codec_mode_support: SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10,
        rtsp_session_url: session_url.map(Into::into),
        config: StreamConfig {
            supported_video_formats: VIDEO_FORMAT_H264
                | VIDEO_FORMAT_H265
                | VIDEO_FORMAT_H265_MAIN10,
            packet_size: 1400,
            height: 1081,
            encryption_flags: ENCFLG_ALL,
            microphone: true,
            remote_input_aes_key: [7; 16],
            video_capabilities: CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC,
            ..Default::default()
        },
    }
}

fn ok(cseq: u32, headers: &str, body: &str) -> Vec<u8> {
    let length = if body.is_empty() {
        String::new()
    } else {
        format!("Content-Length: {}\r\n", body.len())
    };
    format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\n{headers}{length}\r\n{body}").into_bytes()
}

/// Runs a handshake against scripted responses; returns the requests sent.
fn run(handshake: &mut Handshake, respond: impl Fn(&str, &str) -> Vec<u8>) -> Vec<String> {
    let mut sent = Vec::new();
    while let Some(request) = handshake.next_plain_request().unwrap() {
        let text = String::from_utf8(request.serialize()).unwrap();
        let response = parse_response(&respond(&request.method, &request.target)).unwrap();
        handshake.handle_plain_response(&response).unwrap();
        sent.push(text);
    }
    sent
}

fn sunshine_host(method: &str, target: &str) -> Vec<u8> {
    match (method, target) {
        ("DESCRIBE", _) => ok(2, "", DESCRIBE),
        ("SETUP", t) if t.contains("audio") => ok(
            3,
            "Session: DEADBEEFCAFE;timeout = 90\r\nTransport: server_port=48100\r\nX-SS-Ping-Payload: AAAABBBBCCCCDDDD\r\n",
            "",
        ),
        ("SETUP", t) if t.contains("video") => ok(4, "Transport: server_port=48098\r\nX-SS-Ping-Payload: short\r\n", ""),
        ("SETUP", t) if t.contains("control") => ok(5, "Transport: server_port=48099\r\nX-SS-Connect-Data: 0x10\r\n", ""),
        ("SETUP", t) if t.contains("mic") => ok(6, "Transport: unicast\r\n", ""),
        _ => ok(1, "", ""),
    }
}

#[test]
fn full_handshake_with_a_sunshine_family_host() {
    let mut handshake = Handshake::new(params(Some("rtsp://192.168.1.20:48010"))).unwrap();
    assert_eq!(handshake.rtsp_port(), 48010);
    assert!(!handshake.encrypted());
    let sent = run(&mut handshake, sunshine_host);
    let firsts: Vec<&str> = sent.iter().map(|r| r.lines().next().unwrap()).collect();
    assert_eq!(
        firsts,
        [
            "OPTIONS rtsp://192.168.1.20:48010 RTSP/1.0",
            "DESCRIBE rtsp://192.168.1.20:48010 RTSP/1.0",
            "SETUP streamid=audio/0/0 RTSP/1.0",
            "SETUP streamid=video/0/0 RTSP/1.0",
            "SETUP streamid=control/13/0 RTSP/1.0",
            "SETUP streamid=mic/0/0 RTSP/1.0",
            "ANNOUNCE streamid=control/13/0 RTSP/1.0",
            "PLAY / RTSP/1.0",
        ]
    );
    assert_eq!(
        sent[0],
        "OPTIONS rtsp://192.168.1.20:48010 RTSP/1.0\r\nCSeq: 1\r\nX-GS-ClientVersion: 14\r\nHost: 192.168.1.20\r\n\r\n"
    );
    assert_eq!(
        sent[1],
        "DESCRIBE rtsp://192.168.1.20:48010 RTSP/1.0\r\nCSeq: 2\r\nX-GS-ClientVersion: 14\r\nHost: 192.168.1.20\r\n\
         Accept: application/sdp\r\nIf-Modified-Since: Thu, 01 Jan 1970 00:00:00 GMT\r\n\r\n"
    );
    assert_eq!(
        sent[2],
        "SETUP streamid=audio/0/0 RTSP/1.0\r\nCSeq: 3\r\nX-GS-ClientVersion: 14\r\nHost: 192.168.1.20\r\n\
         Transport: unicast;X-GS-ClientPort=50000-50001\r\nIf-Modified-Since: Thu, 01 Jan 1970 00:00:00 GMT\r\n\r\n"
    );
    assert!(
        sent[3].contains("\r\nHost: 192.168.1.20\r\nSession: DEADBEEFCAFE\r\nTransport: unicast;")
    );
    assert!(sent[7].ends_with(
        "CSeq: 8\r\nX-GS-ClientVersion: 14\r\nHost: 192.168.1.20\r\nSession: DEADBEEFCAFE\r\n\r\n"
    ));

    let outcome = handshake.finish().unwrap();
    let announce = &sent[6];
    let body = announce.split_once("\r\n\r\n").unwrap().1;
    assert_eq!(body, outcome.sdp);
    assert!(announce.contains(&format!(
        "Content-type: application/sdp\r\nContent-length: {}\r\n\r\n",
        body.len()
    )));
    assert_eq!(outcome.session_id, "DEADBEEFCAFE");
    assert_eq!(outcome.negotiated_video_format, VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(
        (outcome.audio_port, outcome.video_port, outcome.control_port),
        (48100, 48098, 48099)
    );
    assert_eq!(
        outcome.mic_port, 48012,
        "the offer's port when SETUP names none"
    );
    assert_eq!(outcome.microphone, MicrophoneState::On);
    assert_eq!(outcome.audio_ping_payload, Some(*b"AAAABBBBCCCCDDDD"));
    assert_eq!(outcome.video_ping_payload, None);
    assert_eq!(outcome.control_connect_data, 16);
    assert_eq!(outcome.host_control_messages, [0x5530, 0x5536]);
    assert_eq!(outcome.encryption_enabled, 15);
    assert!(outcome.audio_encryption_enabled);
    // 1400 rounds to 1392; encrypted video takes 32 more.
    assert_eq!(outcome.config.packet_size, 1360);
    assert_eq!(outcome.config.height, 1080);
    assert!(outcome.reference_frame_invalidation_enabled());
    assert!(outcome
        .sdp
        .contains("a=x-nv-video[0].clientViewportHt:1080 \r\n"));
    assert!(outcome
        .sdp
        .contains("a=x-ss-general.encryptionEnabled:15 \r\n"));
    assert!(outcome.sdp.contains("m=video 48098  \r\n"));
}

#[test]
fn microphone_refusals_cost_only_the_microphone() {
    let mut handshake = Handshake::new(params(None)).unwrap();
    run(&mut handshake, |method, target| {
        if method == "SETUP" && target.contains("mic") {
            b"RTSP/1.0 404 Not Found\r\nCSeq: 6\r\n\r\n".to_vec()
        } else {
            sunshine_host(method, target)
        }
    });
    let outcome = handshake.finish().unwrap();
    assert_eq!(
        (outcome.microphone, outcome.mic_port),
        (MicrophoneState::Unavailable, 0)
    );
    assert_eq!(outcome.encryption_enabled, 7);

    // Without microphone encryption the host's offer is not taken.
    let mut handshake = Handshake::new(params(None)).unwrap();
    let sent = run(&mut handshake, |method, target| {
        if method == "DESCRIBE" {
            ok(
                2,
                "",
                &DESCRIBE.replace("encryptionSupported:15", "encryptionSupported:7"),
            )
        } else {
            sunshine_host(method, target)
        }
    });
    assert!(!sent.iter().any(|r| r.contains("streamid=mic")));
    assert_eq!(
        handshake.finish().unwrap().microphone,
        MicrophoneState::Unavailable
    );
}

#[test]
fn failures_report_the_request() {
    let mut handshake = Handshake::new(params(None)).unwrap();
    handshake.next_plain_request().unwrap();
    let refused = parse_response(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n").unwrap();
    handshake.handle_plain_response(&refused).unwrap();
    handshake.next_plain_request().unwrap();
    let refused = parse_response(b"RTSP/1.0 503 Busy\r\nCSeq: 2\r\n\r\n").unwrap();
    match handshake.handle_plain_response(&refused) {
        Err(RtspError::Status {
            request: "DESCRIBE",
            code: 503,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }

    let mut handshake = Handshake::new(params(None)).unwrap();
    let result = (|| {
        while let Some(request) = handshake.next_plain_request()? {
            let response = if request.method == "SETUP" {
                ok(3, "Transport: server_port=1\r\n", "")
            } else {
                sunshine_host(&request.method, &request.target)
            };
            handshake.handle_plain_response(&parse_response(&response)?)?;
        }
        Ok::<_, RtspError>(())
    })();
    assert!(matches!(result, Err(RtspError::Malformed(m)) if m.contains("no session")));
}

#[test]
fn packed_chroma_requires_distinct_capabilities_and_keeps_codec_ids() {
    use crate::sdp::*;
    let base = "a=rtpmap:98 AV1/90000\r\na=sprop-parameter-sets=AAAAAU\r\n";
    let config = StreamConfig {
        experimental_packed_444: true,
        packed444_modes: 15,
        supported_video_formats: VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8,
        ..Default::default()
    };
    assert!(!StreamConfig::default().experimental_packed_444);
    for offer in [
        "",
        "a=x-rl-video.packed444Version:2\r\na=x-rl-video.packed444Modes:15\r\n",
        "a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:0\r\n",
    ] {
        assert!(DescribeInfo::parse(&format!("{base}{offer}"))
            .select_packed_format(&config)
            .is_err());
    }
    let info = DescribeInfo::parse(&format!(
        "{base}a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:15\r\n"
    ));
    assert_eq!(
        info.select_packed_format(&config).unwrap(),
        VIDEO_FORMAT_AV1_MAIN8
    );
    assert_eq!(
        info.select_packed_format(&StreamConfig {
            supported_video_formats: VIDEO_FORMAT_H265_MAIN10,
            ..config.clone()
        })
        .unwrap(),
        VIDEO_FORMAT_H265_MAIN10
    );
    assert!(info
        .select_packed_format(&StreamConfig {
            packed444_modes: 0,
            ..config.clone()
        })
        .is_err());
    assert!(info
        .select_packed_format(&StreamConfig {
            supported_video_formats: VIDEO_FORMAT_H265_REXT10_444,
            ..config.clone()
        })
        .is_err());
    let mut request = params(None);
    request.config = StreamConfig { width: 0, ..config };
    assert!(Handshake::new(request).is_err());
}

#[test]
fn gamestream_before_3_22_plays_each_stream() {
    let mut p = params(None);
    p.app_version = "7.1.415.0".into();
    p.config.microphone = false;
    p.config.bitrate_kbps = 10000;
    let mut handshake = Handshake::new(p).unwrap();
    let sent = run(&mut handshake, sunshine_host);
    let firsts: Vec<&str> = sent.iter().map(|r| r.lines().next().unwrap()).collect();
    assert_eq!(
        firsts,
        [
            "OPTIONS rtsp://0.0.0.0:48010 RTSP/1.0",
            "DESCRIBE rtsp://0.0.0.0:48010 RTSP/1.0",
            "SETUP streamid=audio/0/0 RTSP/1.0",
            "SETUP streamid=video/0/0 RTSP/1.0",
            "SETUP streamid=control/1/0 RTSP/1.0",
            "ANNOUNCE streamid=video RTSP/1.0",
            "PLAY streamid=video RTSP/1.0",
            "PLAY streamid=audio RTSP/1.0",
        ]
    );
    assert!(sent[0].contains("\r\nHost: 0.0.0.0\r\n"));

    let mut p = params(None);
    p.app_version = "7.1.400.0".into();
    assert!(matches!(Handshake::new(p), Err(RtspError::Unsupported(_))));
}

#[test]
fn encrypted_rtsp_round_trip_and_framing() {
    let mut handshake = Handshake::new(params(Some("rtspenc://192.168.1.20:48011"))).unwrap();
    assert!(handshake.encrypted());
    assert_eq!(handshake.rtsp_port(), 48011);
    let wire = handshake.next_request().unwrap().unwrap();
    let plain_len =
        (u32::from_be_bytes(wire[..4].try_into().unwrap()) & !ENCRYPTED_RTSP_BIT) as usize;
    assert_eq!(wire[0] & 0x80, 0x80);
    assert_eq!(u32::from_be_bytes(wire[4..8].try_into().unwrap()), 1);
    assert_eq!(wire.len(), ENCRYPTED_HEADER + plain_len);

    // The host's side: decrypt with 'C', answer with 'H'.
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &[7; 16]).unwrap());
    let mut body = wire[24..].to_vec();
    body.extend_from_slice(&wire[8..24]);
    let request = key
        .open_in_place(rtsp_iv(1, b'C'), aead::Aad::empty(), &mut body)
        .unwrap();
    assert!(request.starts_with(b"OPTIONS rtspenc://192.168.1.20:48011 RTSP/1.0\r\nCSeq: 1\r\n"));

    let seal = |sequence: u32, plain: &[u8]| {
        let mut body = plain.to_vec();
        let tag = key
            .seal_in_place_separate_tag(rtsp_iv(sequence, b'H'), aead::Aad::empty(), &mut body)
            .unwrap();
        let mut out = (ENCRYPTED_RTSP_BIT | plain.len() as u32)
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(tag.as_ref());
        out.extend_from_slice(&body);
        out
    };
    let response = seal(0, b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n");
    let mut tampered = response.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert!(matches!(
        handshake.handle_response(&tampered),
        Err(RtspError::Encryption(_))
    ));

    let mut handshake = Handshake::new(params(Some("rtspenc://192.168.1.20:48011"))).unwrap();
    handshake.next_request().unwrap();
    handshake.handle_response(&response).unwrap();
    assert!(handshake.next_request().unwrap().unwrap()[4..8] == 2u32.to_be_bytes());
    let mut handshake = Handshake::new(params(Some("rtspenc://192.168.1.20:48011"))).unwrap();
    handshake.next_request().unwrap();
    assert!(
        handshake
            .handle_response(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n")
            .is_err(),
        "plain text is refused"
    );
}

#[test]
fn url_and_header_helpers() {
    assert_eq!(
        url_address("rtsp://192.168.1.2:48010").as_deref(),
        Some("192.168.1.2")
    );
    assert_eq!(
        url_address("rtspenc://[fe80::1]:48010").as_deref(),
        Some("[fe80::1]")
    );
    assert_eq!(url_address("rtsp://host").as_deref(), Some("host"));
    assert_eq!(url_address("no-scheme"), None);
    assert_eq!(
        rtsp_port_from_session_url(Some("rtsp://10.0.0.1:48011")),
        48011
    );
    assert_eq!(rtsp_port_from_session_url(Some("rtsp://10.0.0.1")), 48010);
    assert_eq!(rtsp_port_from_session_url(None), 48010);
    assert_eq!(parse_app_version("7.1.431.-1"), [7, 1, 431, -1]);
    assert_eq!(parse_app_version("7.1"), [7, 1, 0, 0]);
    assert_eq!(parse_app_version("3.20.3.70"), [3, 20, 3, 70]);
    let r = parse_response(
        b"RTSP/1.0 200 OK\r\nTransport: unicast;server_port=48000-48001;source=x\r\n\r\n",
    )
    .unwrap();
    assert_eq!(transport_port(&r), Some(48000));
    let r = parse_response(b"RTSP/1.0 200 OK\r\nTransport: server_port=0\r\n\r\n").unwrap();
    assert_eq!(transport_port(&r), None);
}

#[test]
fn configuration_checks() {
    let mut p = params(None);
    p.config.packet_size = 15;
    assert!(Handshake::new(p).is_err());
    let mut p = params(None);
    p.server_codec_mode_support = 0;
    assert!(Handshake::new(p).is_err());
    // GameStream at 4K loses H.264 RFI; Sunshine keeps it.
    let mut p = params(None);
    p.app_version = "7.1.431.0".into();
    p.config.width = 3840;
    p.config.height = 2160;
    p.config.video_capabilities = CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC;
    let handshake = Handshake::new(p.clone()).unwrap();
    assert_eq!(handshake.out.video_capabilities, 0);
    p.app_version = "7.1.431.-1".into();
    assert_eq!(
        Handshake::new(p).unwrap().out.video_capabilities,
        CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC
    );
}
