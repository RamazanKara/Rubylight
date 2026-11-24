//! Unit tests for SDP extension attributes: jumbo packets, packed 4:4:4 bitrate reserve,
//! Foundation FEC control, 7.1.4 surround, link/wireless hints, HDR/audio codecs and PyroWave
//! row order.

use super::*;

/// The `a=` line of an attribute, or None when the description lacks it.
fn attribute<'a>(sdp: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("a={name}:");
    sdp.lines().find(|l| l.starts_with(&prefix)).map(|l| l.trim_end())
}

#[test]
fn jumbo_packets_are_announced_within_the_streams_offer() {
    let announced = |packet_size: i32, offer: u16, encrypted: bool| {
        let config = StreamConfig {
            packet_size,
            encryption_flags: ENCFLG_ALL,
            ..Default::default()
        };
        let mut p = params(&config, SUNSHINE, "192.168.1.20", VIDEO_FORMAT_H265);
        p.max_video_packet = offer;
        p.encryption_supported = if encrypted { SS_ENC_VIDEO } else { 0 };
        generate(&p).packet_size
    };
    // The size includes the encryption header, so an encrypted stream asks for 32 less.
    assert_eq!(announced(8192, 8192, false), 8192);
    assert_eq!(announced(8192, 8192, true), 8160);
    // Never above the stream's own offer, rounded down to a multiple of 16.
    assert_eq!(announced(8192, 8160, false), 8160);
    assert_eq!(announced(8192, 1424, true), 1392);
    assert_eq!(announced(8190, 8192, false), 8176);
    // A jumbo request without an offer streams stock packets; a stock request stays as it was.
    assert_eq!(announced(8192, 0, true), 1360);
    assert_eq!(announced(1392, 8192, true), 1360);
    // A request smaller than the encrypted header cannot become a negative size: the smallest
    // datagram that has room for the header is announced instead.
    assert_eq!(announced(16, 8192, true), 32);
    assert_eq!(announced(0, 0, true), 32);
    assert_eq!(announced(16, 8192, false), 16, "unencrypted keeps the request");
}

#[test]
fn packed_bitrate_reserves_packet_overhead_and_fec() {
    for (budget, location, video) in [
        (80_000, StreamLocation::Local, 56_000),
        (200_000, StreamLocation::Local, 140_000),
        (200_000, StreamLocation::Remote, 139_500),
        (300_000, StreamLocation::Local, 140_000),
    ] {
        let config = StreamConfig {
            bitrate_kbps: budget,
            location,
            packet_size: 1392,
            encryption_flags: ENCFLG_ALL,
            experimental_packed_444: true,
            ..Default::default()
        };
        let mut p = params(&config, SUNSHINE, "::1", VIDEO_FORMAT_H265);
        p.encryption_supported = SS_ENC_VIDEO;
        let sdp = generate(&p);
        assert_eq!(sdp.packet_size, 1360);
        assert!(sdp.text.contains(&format!(
            "a=x-nv-vqos[0].bw.maximumBitrateKbps:{video} \r\n"
        )));
        assert!(sdp
            .text
            .contains(&format!("a=x-ml-video.configuredBitrateKbps:{budget} \r\n")));
    }
}

#[test]
fn foundation_fec_control_goes_where_v_plus_puts_it() {
    // V+'s SdpGenerator.c: right after the packet size, the percentage, then summary version 1.
    let config = StreamConfig::default();
    let mut p = params(&config, SUNSHINE, "192.168.1.20", VIDEO_FORMAT_H265);
    for (percentage, line) in [
        (-2, "a=x-ml-video.fecPercentage:-2 \r\n"),
        (-1, "a=x-ml-video.fecPercentage:-1 \r\n"),
        (0, "a=x-ml-video.fecPercentage:0 \r\n"),
        (100, "a=x-ml-video.fecPercentage:100 \r\n"),
    ] {
        p.video_fec_percentage = Some(percentage);
        let sdp = generate(&p).text;
        assert!(
            sdp.contains(&format!(
                "a=x-nv-video[0].packetSize:1392 \r\n{line}a=x-ml-video.fecSummaryVersion:1 \r\n\
                 a=x-nv-video[0].rateControlMode:4 \r\n"
            )),
            "{sdp}"
        );
    }
    // Without the host's FEC control (Rubylight, Sunshine, Apollo, GameStream): neither.
    p.video_fec_percentage = None;
    let sdp = generate(&p).text;
    assert!(!sdp.contains("x-ml-video.fec"), "{sdp}");
    assert!(sdp.contains("a=x-nv-video[0].packetSize:1392 \r\na=x-nv-video[0].rateControlMode:4 \r\n"));
}

/// 7.1.4 asks as Moonlight V+ does: 12 channels with mask 0xF63F, in the launch's
/// `surroundAudioInfo` and the ANNOUNCE.
#[test]
fn seven_one_four_is_announced_as_moonlight_v_plus_does() {
    let config = StreamConfig {
        audio: AudioConfiguration::SURROUND_714,
        bitrate_kbps: 50_000,
        ..Default::default()
    };
    assert_eq!(config.audio.surround_audio_info(), 0xF63F_000C);
    let mut p = params(&config, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_H265);
    p.high_quality_surround_supported = true;
    let sdp = generate(&p);
    for line in [
        "a=x-nv-audio.surround.numChannels:12",
        "a=x-nv-audio.surround.channelMask:63039",
        "a=x-nv-audio.surround.enable:1",
        "a=x-nv-audio.surround.AudioQuality:1",
    ] {
        let name = &line[2..line.rfind(':').unwrap()];
        assert_eq!(attribute(&sdp.text, name), Some(line), "{}", sdp.text);
    }
    assert!(sdp.high_quality_surround_enabled);
}

#[test]
fn link_speed_hint_is_sent_for_every_codec_and_only_pyrowave_gets_the_pace() {
    const LINK: &str = "x-ss-video[0].pyrowaveLinkMbps";
    const PACE: &str = "x-ss-video[0].pyrowavePaceMbps";
    let formats = [
        (VIDEO_FORMAT_H264, false),
        (VIDEO_FORMAT_H265, false),
        (VIDEO_FORMAT_AV1_MAIN8, false),
        (VIDEO_FORMAT_PYROWAVE, true),
    ];
    for (format, pyrowave) in formats {
        // Wired with a known speed and a calibrated pace.
        let config =
            StreamConfig { pyrowave_link_mbps: 2500, pyrowave_pace_mbps: 2100, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert_eq!(attribute(&sdp, LINK), Some("a=x-ss-video[0].pyrowaveLinkMbps:2500"), "{format:#x}");
        assert_eq!(attribute(&sdp, PACE).is_some(), pyrowave, "{format:#x}");

        // Wi-Fi, cellular or an unreadable port reports 0: no hint, as before.
        let config = StreamConfig { pyrowave_link_mbps: 0, pyrowave_pace_mbps: 0, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert_eq!(attribute(&sdp, LINK), None, "{format:#x}");
        assert_eq!(attribute(&sdp, PACE), None, "{format:#x}");
    }
}

#[test]
fn link_speed_setting_off_leaves_out_the_link_attribute_but_keeps_the_calibrated_pace() {
    // The client reports 0 when "Tell the PC the link speed" is off; the pace stays as calibrated.
    for format in [VIDEO_FORMAT_H264, VIDEO_FORMAT_H265, VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_PYROWAVE] {
        let config = StreamConfig { pyrowave_link_mbps: 0, pyrowave_pace_mbps: 2100, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert!(!sdp.contains("pyrowaveLinkMbps"), "{format:#x}");
        assert_eq!(
            sdp.contains("x-ss-video[0].pyrowavePaceMbps:2100"),
            format == VIDEO_FORMAT_PYROWAVE,
            "{format:#x}"
        );
    }
}

#[test]
fn wireless_route_is_flagged_for_every_codec_and_never_sent_as_zero() {
    const WIRELESS: &str = "x-rl-video[0].clientWireless";
    for format in [
        VIDEO_FORMAT_H264,
        VIDEO_FORMAT_H265,
        VIDEO_FORMAT_AV1_MAIN8,
        VIDEO_FORMAT_PYROWAVE,
    ] {
        let config = StreamConfig { client_wireless: true, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert_eq!(attribute(&sdp, WIRELESS), Some("a=x-rl-video[0].clientWireless:1"), "{format:#x}");

        // Wired, unknown, VPN-only or undeterminable: nothing at all, not a 0.
        let config = StreamConfig { client_wireless: false, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert_eq!(attribute(&sdp, WIRELESS), None, "{format:#x}");
        assert!(!sdp.contains("clientWireless"), "{format:#x}");
    }
}

#[test]
fn hdr_format_and_audio_codec_are_sent_only_when_asked_for_and_offered() {
    use rubylight_protocol::audio_ext::{CODECS_ALL, CODEC_AC3, CODEC_PCM};
    use rubylight_protocol::hdr::{ALL, DV_101, DV_81, DV_84, HLG, PQ, PYROWAVE_HLG};
    const HDR: &str = rubylight_protocol::hdr::ANNOUNCE_ATTRIBUTE;
    const CODEC: &str = rubylight_protocol::audio_ext::CODEC_ANNOUNCE_ATTRIBUTE;
    let config = StreamConfig::default();
    let with = |format: u32, hdr: (u8, u8), codec: (u8, u8)| {
        let mut p = params(&config, SUNSHINE, "10.0.0.7", format);
        (p.hdr_format, p.hdr_formats_offered) = hdr;
        (p.audio_codec, p.audio_codecs_offered) = codec;
        generate(&p).text
    };
    let sent = |format: u32, hdr: (u8, u8), codec: (u8, u8)| {
        let text = with(format, hdr, codec);
        (
            attribute(&text, HDR).map(str::to_owned),
            attribute(&text, CODEC).map(str::to_owned),
        )
    };
    let dv = Some("a=x-rl-video[0].hdrFormat:8".to_owned());
    let pcm = Some("a=x-rl-audio.codec:2".to_owned());
    let formats = [
        VIDEO_FORMAT_H265,
        VIDEO_FORMAT_H265_MAIN10,
        VIDEO_FORMAT_AV1_MAIN10,
        VIDEO_FORMAT_PYROWAVE,
    ];
    for format in formats {
        // Nothing asked for: the description is exactly what it was before the extensions,
        // whatever the host offers.
        let plain = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert!(!plain.contains("hdrFormat") && !plain.contains("x-rl-audio"), "{format:#x}");
        assert_eq!(with(format, (0, ALL), (0, CODECS_ALL)), plain, "{format:#x}");
        // Not offered, or more than one bit: nothing either.
        assert_eq!(with(format, (DV_84, 0), (CODEC_PCM, 0)), plain, "{format:#x}");
        assert_eq!(with(format, (DV_84, HLG | PQ), (CODEC_AC3, CODEC_PCM)), plain);
        assert_eq!(with(format, (DV_81 | DV_84, ALL), (CODEC_PCM | CODEC_AC3, CODECS_ALL)), plain);
        // An offered codec goes on every stream.
        assert_eq!(sent(format, (0, 0), (CODEC_PCM, CODECS_ALL)), (None, pcm.clone()));
    }

    // An HDR format needs a 10-bit HEVC or AV1 stream, Dolby Vision 8.1 and 8.4 HEVC and
    // 10.1 AV1.
    let hdr = |format: u32, bit: u8| sent(format, (bit, ALL), (0, 0)).0;
    assert_eq!(hdr(VIDEO_FORMAT_H265_MAIN10, DV_84), dv);
    assert_eq!(hdr(VIDEO_FORMAT_H265_MAIN10, HLG), Some("a=x-rl-video[0].hdrFormat:2".into()));
    assert_eq!(hdr(VIDEO_FORMAT_AV1_MAIN10, HLG), Some("a=x-rl-video[0].hdrFormat:2".into()));
    assert_eq!(hdr(VIDEO_FORMAT_AV1_MAIN10, DV_84), None);
    assert_eq!(hdr(VIDEO_FORMAT_AV1_MAIN10, DV_81), None);
    assert_eq!(hdr(VIDEO_FORMAT_AV1_MAIN10, DV_101), Some("a=x-rl-video[0].hdrFormat:16".into()));
    assert_eq!(hdr(VIDEO_FORMAT_H265_MAIN10, DV_101), None);
    assert_eq!(hdr(VIDEO_FORMAT_AV1_MAIN8, DV_101), None);
    assert_eq!(hdr(VIDEO_FORMAT_H265, HLG), None);
    assert_eq!(hdr(VIDEO_FORMAT_PYROWAVE, HLG), None);
    // PyroWave's 10-bit formats are in the 10-bit mask but carry no HDR format from a host without
    // the PyroWave HLG offer flag: a 3.0 rc host refuses one in the ANNOUNCE.
    let flagged = |format: u32, bit: u8, offered: u8| sent(format, (bit, offered), (0, 0)).0;
    let hlg = Some("a=x-rl-video[0].hdrFormat:2".to_owned());
    for format in [VIDEO_FORMAT_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10_444] {
        for bit in [PQ, HLG, DV_81, DV_84, DV_101] {
            assert_eq!(hdr(format, bit), None, "{format:#x} {bit}");
        }
        // With the flag, HLG and only HLG: PQ stays stock HDR, Dolby Vision needs HEVC.
        assert_eq!(flagged(format, HLG, ALL | PYROWAVE_HLG), hlg, "{format:#x}");
        assert_eq!(flagged(format, HLG, HLG | PYROWAVE_HLG), hlg, "{format:#x}");
        for bit in [PQ, DV_81, DV_84] {
            assert_eq!(flagged(format, bit, ALL | PYROWAVE_HLG), None, "{format:#x} {bit}");
        }
        // The flag is never a format, and HLG still has to be offered beside it.
        assert_eq!(flagged(format, PYROWAVE_HLG, ALL | PYROWAVE_HLG), None);
        assert_eq!(flagged(format, HLG, PYROWAVE_HLG), None);
    }
    // 8-bit PyroWave carries no HDR, the flag or not; on HEVC the flag changes nothing.
    assert_eq!(flagged(VIDEO_FORMAT_PYROWAVE, HLG, ALL | PYROWAVE_HLG), None);
    assert_eq!(flagged(VIDEO_FORMAT_H265_MAIN10, HLG, ALL | PYROWAVE_HLG), hlg);
    assert_eq!(flagged(VIDEO_FORMAT_H265_MAIN10, PYROWAVE_HLG, ALL | PYROWAVE_HLG), None);

    // Only the two lines differ from the plain description.
    let plain = generate(&params(&config, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_H265_MAIN10)).text;
    let asked = with(VIDEO_FORMAT_H265_MAIN10, (DV_84, ALL), (CODEC_PCM, CODECS_ALL));
    assert_eq!(
        (attribute(&asked, HDR).map(str::to_owned), attribute(&asked, CODEC).map(str::to_owned)),
        (dv, pcm)
    );
    let removed: String = asked
        .split_inclusive("\r\n")
        .filter(|line| {
            !line.starts_with("a=x-rl-video[0].hdrFormat:")
                && !line.starts_with("a=x-rl-audio.codec:")
        })
        .collect();
    assert_eq!(removed, plain);
}

#[test]
fn row_order_is_requested_only_for_pyrowave_with_records_and_the_setting() {
    const ROW_ORDER: &str = "x-ss-video[0].pyrowaveRowOrder";
    for (format, records, row_order, sent) in [
        (VIDEO_FORMAT_PYROWAVE, true, true, true),
        (VIDEO_FORMAT_PYROWAVE, true, false, false),
        (VIDEO_FORMAT_PYROWAVE, false, true, false),
        (VIDEO_FORMAT_H264, true, true, false),
        (VIDEO_FORMAT_H265, true, true, false),
    ] {
        let config = StreamConfig { pyrowave_row_order: row_order, ..Default::default() };
        let mut p = params(&config, SUNSHINE, "10.0.0.7", format);
        p.pyrowave_records_enabled = records;
        let sdp = generate(&p).text;
        let line = attribute(&sdp, ROW_ORDER);
        assert_eq!(line, sent.then_some("a=x-ss-video[0].pyrowaveRowOrder:1"), "{format:#x} records={records} row_order={row_order}");
        // Next to the records key, before the pace hint, and nothing else moves.
        if sent {
            let features = sdp.find("pyrowaveFeatures:1").unwrap();
            assert!(features < sdp.find("pyrowaveRowOrder:1").unwrap());
        }
    }
    // With the request off the description is the same as a client that never heard of it.
    let off = StreamConfig { pyrowave_row_order: false, ..Default::default() };
    let on = StreamConfig { pyrowave_row_order: true, ..Default::default() };
    let mut p = params(&off, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_PYROWAVE);
    p.pyrowave_records_enabled = true;
    let without = generate(&p).text;
    let mut p = params(&on, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_PYROWAVE);
    p.pyrowave_records_enabled = true;
    let with = generate(&p).text;
    assert_eq!(with.replace("a=x-ss-video[0].pyrowaveRowOrder:1 \r\n", ""), without);
}
