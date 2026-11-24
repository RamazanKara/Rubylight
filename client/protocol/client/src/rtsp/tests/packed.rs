//! Unit tests for packed 4:4:4 chroma RTSP negotiation and refusal handling.

use super::*;

#[test]
fn packed_chroma_requires_distinct_capabilities_and_keeps_codec_ids() {
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
fn packed_ten_bit_without_a_common_ten_bit_mode_streams_eight_bit_packed() {
    let base = "a=rtpmap:98 AV1/90000\r\na=sprop-parameter-sets=AAAAAU\r\n";
    let offer = |modes: u8| {
        DescribeInfo::parse(&format!(
            "{base}a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:{modes}\r\n"
        ))
    };
    // An HDR request: both depths of both codecs on the client.
    let hdr = StreamConfig {
        experimental_packed_444: true,
        packed444_modes: 15,
        supported_video_formats: VIDEO_FORMAT_H265
            | VIDEO_FORMAT_H265_MAIN10
            | VIDEO_FORMAT_AV1_MAIN8
            | VIDEO_FORMAT_AV1_MAIN10,
        ..Default::default()
    };
    // (host modes, client modes, client formats, the format streamed)
    for (host, client, formats, expected) in [
        // A host whose 10-bit probes failed (the Retroid and RX 7900 XT case).
        (5, 15, hdr.supported_video_formats, VIDEO_FORMAT_AV1_MAIN8),
        (1, 15, hdr.supported_video_formats, VIDEO_FORMAT_H265),
        // A client that verified no 10-bit path.
        (15, 5, hdr.supported_video_formats, VIDEO_FORMAT_AV1_MAIN8),
        // 10-bit in common on one codec: still 10-bit, on that codec.
        (
            2 | 4,
            15,
            hdr.supported_video_formats,
            VIDEO_FORMAT_H265_MAIN10,
        ),
        (15, 15, hdr.supported_video_formats, VIDEO_FORMAT_AV1_MAIN10),
        // 10-bit AV1 on the host, but AV1 not on the client: 8-bit HEVC.
        (
            8 | 1,
            15,
            VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10,
            VIDEO_FORMAT_H265,
        ),
    ] {
        let config = StreamConfig {
            packed444_modes: client,
            supported_video_formats: formats,
            ..hdr.clone()
        };
        assert_eq!(
            offer(host).select_packed_format(&config).unwrap(),
            expected,
            "host {host} client {client}"
        );
    }
    // No mode at all in common is a packed refusal, not a generic failure.
    for (host, client, formats) in [
        (4, 15, VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10),
        (1 | 2, 4 | 8, hdr.supported_video_formats),
        (0, 15, hdr.supported_video_formats),
        (15, 0, hdr.supported_video_formats),
    ] {
        let config = StreamConfig {
            packed444_modes: client,
            supported_video_formats: formats,
            ..hdr.clone()
        };
        assert!(
            matches!(
                offer(host).select_packed_format(&config),
                Err(RtspError::PackedRefused(_))
            ),
            "host {host} client {client}"
        );
    }
}

fn packed_handshake(formats: u32) -> Handshake {
    let mut request = params(None);
    request.server_codec_mode_support |= SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    request.config.experimental_packed_444 = true;
    request.config.packed444_modes = 15;
    request.config.supported_video_formats = formats;
    Handshake::new(request).unwrap()
}

/// Runs a handshake until it fails or finishes; returns the failure.
fn run_to_error(
    handshake: &mut Handshake,
    respond: impl Fn(&str, &str) -> Vec<u8>,
) -> Option<RtspError> {
    loop {
        let request = match handshake.next_plain_request() {
            Ok(Some(request)) => request,
            Ok(None) => return None,
            Err(e) => return Some(e),
        };
        let response = parse_response(&respond(&request.method, &request.target)).unwrap();
        if let Err(e) = handshake.handle_plain_response(&response) {
            return Some(e);
        }
    }
}

#[test]
fn packed_hdr_against_a_host_without_ten_bit_packed_starts_in_eight_bit() {
    let hdr = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    for (modes, expected, dropped) in [
        (
            1,
            VIDEO_FORMAT_H265,
            Some("host doesn't offer 10-bit full chroma"),
        ),
        (3, VIDEO_FORMAT_H265_MAIN10, None),
        // 10-bit AV1 on the host, but this client offers HEVC only.
        (
            1 | 8,
            VIDEO_FORMAT_H265,
            Some("this device and PC share no 10-bit full chroma mode"),
        ),
    ] {
        let mut handshake = packed_handshake(hdr);
        let sent = run(&mut handshake, |method, target| {
            if method == "DESCRIBE" {
                ok(2, "", &format!("{DESCRIBE}a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:{modes}\r\n"))
            } else {
                sunshine_host(method, target)
            }
        });
        let outcome = handshake.finish().unwrap();
        assert_eq!(outcome.negotiated_video_format, expected);
        assert_eq!(outcome.packed_ten_bit_dropped(), dropped);
        let announce = sent
            .iter()
            .find(|message| message.starts_with("ANNOUNCE "))
            .unwrap();
        assert!(announce.contains("a=x-rl-video[0].chromaPacking:1"));
        // 8-bit packed asks the host for SDR, not HDR.
        let range = if dropped.is_some() { 0 } else { 1 };
        assert!(
            announce.contains(&format!("x-nv-video[0].dynamicRangeMode:{range}")),
            "{announce}"
        );
    }
    // An SDR request that streams 8-bit dropped nothing.
    let mut handshake = packed_handshake(VIDEO_FORMAT_H265);
    run(&mut handshake, |method, target| {
        if method == "DESCRIBE" {
            ok(2, "", &format!("{DESCRIBE}a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:1\r\n"))
        } else {
            sunshine_host(method, target)
        }
    });
    assert_eq!(handshake.finish().unwrap().packed_ten_bit_dropped(), None);
}

#[test]
fn packed_refusals_at_describe_and_announce_are_packed_refused() {
    let hdr = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    // A host without packed 4:4:4 (its setting off): refused at DESCRIBE, before ANNOUNCE.
    let mut handshake = packed_handshake(hdr);
    let error = run_to_error(&mut handshake, sunshine_host).unwrap();
    assert!(
        matches!(&error, RtspError::PackedRefused(m) if m.contains("does not offer packed")),
        "{error:?}"
    );
    let announce_refusal_with = |status: &'static str, body: &'static str| {
        move |method: &str, target: &str| {
            if method == "DESCRIBE" {
                ok(2, "", &format!("{DESCRIBE}a=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:3\r\n"))
            } else if method == "ANNOUNCE" {
                format!(
                    "RTSP/1.0 {status}\r\nCSeq: 7\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .into_bytes()
            } else {
                sunshine_host(method, target)
            }
        }
    };
    let announce_refusal = |body: &'static str| announce_refusal_with("400 Bad Request", body);
    let geometry =
        "packed encoder cannot support the requested dimensions and frame rate: 1920x1080 at 120";
    for body in [
        geometry,
        "Experimental Packed 4:4:4 is disabled on the host",
    ] {
        let mut handshake = packed_handshake(hdr);
        let error = run_to_error(&mut handshake, announce_refusal(body)).unwrap();
        assert!(
            matches!(&error, RtspError::PackedRefused(m) if m.contains(body)),
            "{error:?}"
        );
    }
    // Only a 400 is a refusal of packing; a server error naming it stays a server error.
    let mut handshake = packed_handshake(hdr);
    let error = run_to_error(
        &mut handshake,
        announce_refusal_with("500 Internal Server Error", geometry),
    )
    .unwrap();
    assert!(
        matches!(error, RtspError::Status { code: 500, .. }),
        "{error:?}"
    );
    // Any other ANNOUNCE refusal keeps its status, packed or not.
    let mut handshake = packed_handshake(hdr);
    let error = run_to_error(&mut handshake, announce_refusal("encoder busy")).unwrap();
    assert!(
        matches!(error, RtspError::Status { code: 400, .. }),
        "{error:?}"
    );
    let mut handshake = Handshake::new(params(None)).unwrap();
    let error = run_to_error(&mut handshake, announce_refusal(geometry)).unwrap();
    assert!(
        matches!(error, RtspError::Status { code: 400, .. }),
        "{error:?}"
    );
}

#[test]
fn layered_packing_is_negotiated_per_codec_and_falls_back_to_version_one() {
    for (format, layered, expected) in [
        (VIDEO_FORMAT_H265, "", 1),
        (VIDEO_FORMAT_H265, "0", 1),
        (VIDEO_FORMAT_H265, "4", 1),
        (VIDEO_FORMAT_H265, "1", 2),
        (VIDEO_FORMAT_H265, "15", 2),
        (VIDEO_FORMAT_H265, "256", 1),
        (VIDEO_FORMAT_H265, "bad", 1),
        (VIDEO_FORMAT_H265, "10", 1),
        (VIDEO_FORMAT_H265_MAIN10, "10", 2),
        (VIDEO_FORMAT_AV1_MAIN8, "10", 1),
        (VIDEO_FORMAT_AV1_MAIN10, "10", 2),
        (VIDEO_FORMAT_H265_MAIN10, "8", 1),
        (VIDEO_FORMAT_AV1_MAIN10, "2", 1),
    ] {
        let mut request = params(None);
        request.server_codec_mode_support |= SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
        request.config.experimental_packed_444 = true;
        request.config.packed444_modes = 15;
        request.config.supported_video_formats = format;
        request.config.packed444_version = 255;
        let mut handshake = Handshake::new(request).unwrap();
        let sent = run(&mut handshake, |method, target| {
            if method == "DESCRIBE" {
                ok(2, "", &format!("{DESCRIBE}a=rtpmap:98 AV1/90000\r\na=x-rl-video.packed444Version:1\r\na=x-rl-video.packed444Modes:15\r\na=x-rl-video.packed444LayeredModes:{layered}\r\n"))
            } else {
                sunshine_host(method, target)
            }
        });
        let outcome = handshake.finish().unwrap();
        assert_eq!(outcome.config.packed444_version, expected);
        assert!(sent.iter().any(|message| message.starts_with("ANNOUNCE ")
            && message.contains(&format!("a=x-rl-video[0].chromaPacking:{expected}"))));
        assert_eq!(outcome.negotiated_video_format, format);
    }
}
