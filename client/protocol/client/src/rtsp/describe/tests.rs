use super::*;

const RUBYLIGHT: &str = "a=x-ss-general.featureFlags:3\r\na=x-ss-general.encryptionSupported:15\r\n\
        a=x-ss-general.encryptionRequested:9\r\n\
        a=x-rl-control:0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537\r\n\
        sprop-parameter-sets=AAAAAU\r\na=rtpmap:98 AV1/90000\r\n\
        a=rtpmap:99 PYROWAVE/90000\r\na=x-ss-pyrowave.bitstream:186f0393\r\n\
        a=fmtp:97 surround-params=21101\r\na=fmtp:97 surround-params=642014523\r\n\
        a=fmtp:97 surround-params=85301456723\r\na=fmtp:97 surround-params=21101\r\n\
        a=fmtp:97 surround-params=660014523\r\na=fmtp:97 surround-params=88001456723\r\n\
        a=x-nv-video[0].refPicInvalidation:1\r\n\
        m=audio 48012 RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\na=fmtp:96 minptime=10;useinbandfec=1\r\n";

#[test]
fn rubylight_describe() {
    let info = DescribeInfo::parse(RUBYLIGHT);
    assert!(info.av1 && info.hevc && info.pyrowave && info.reference_frame_invalidation);
    assert_eq!(
        (
            info.sunshine_feature_flags,
            info.encryption_supported,
            info.encryption_requested
        ),
        (3, 15, 9)
    );
    assert_eq!(
        info.control_messages,
        [0x5530, 0x5531, 0x5532, 0x5533, 0x5534, 0x5535, 0x5536, 0x5537]
    );
    assert_eq!(info.microphone, Some(48012));
    assert!(!info.roaming, "no advertisement, no roaming");
    // A host that offers no 3.0 extension reads as before.
    assert_eq!((info.hdr_formats, info.audio_codecs), (0, 0));
    assert_eq!((info.layout_714, info.layout_714_hq), (None, None));
    assert!(!info.pen_barrel_roll);
}

#[test]
fn the_30_extensions_follow_their_attributes() {
    use rubylight_protocol::audio_ext::{layout_714_lines, LAYOUT_714, LAYOUT_714_HQ};
    let with = |lines: &str| DescribeInfo::parse(&format!("{RUBYLIGHT}{lines}"));
    let offered = format!(
        "a=x-rl-video.hdrFormats:15\r\na=x-rl-audio.codecs:15\r\n\
             {}a=x-ss-general.penBarrelRoll:1\r\n\
             a=x-rl-control:0x5539,0x553a,0x553b,0x553d,0x553e,0x5540\r\n",
        layout_714_lines().concat()
    );
    let info = with(&offered);
    assert_eq!((info.hdr_formats, info.audio_codecs), (15, 15));
    assert_eq!(
        (info.layout_714, info.layout_714_hq),
        (Some(LAYOUT_714), Some(LAYOUT_714_HQ))
    );
    assert!(info.pen_barrel_roll);
    // The second x-rl-control line adds to the first.
    assert_eq!(info.control_messages.len(), 14);
    assert!(info.control_messages.contains(&0x553e));
    assert_eq!(
        DescribeInfo::parse(&offered.replace("\r\n", "\n")),
        DescribeInfo::parse(&offered)
    );
    // Unknown bits are dropped; malformed values are no offer.
    assert_eq!(
        with("a=x-rl-video.hdrFormats:255\r\n").hdr_formats,
        hdr::ALL | hdr::OFFER_FLAGS
    );
    assert_eq!(with("a=x-rl-video.hdrFormats:31\r\n").hdr_formats, 31);
    // The host's offer flags (PyroWave HLG) stay: the ANNOUNCE asks HLG on PyroWave only
    // with it, and never asks for a flag (sdp::announced_hdr_format).
    assert_eq!(with("a=x-rl-video.hdrFormats:131\r\n").hdr_formats, 131);
    assert_eq!(with("a=x-rl-video.hdrFormats:143\r\n").hdr_formats, 143);
    assert_eq!(with("a=x-rl-video.hdrFormats:128\r\n").hdr_formats, 128);
    assert_eq!(with("a=x-rl-video.hdrFormats: 2 \r\n").hdr_formats, 2);
    for line in [
        "a=x-rl-video.hdrFormats:256",
        "a=x-rl-video.hdrFormats:-1",
        "a=x-rl-video.hdrFormats:0x2",
        "a=x-rl-video.hdrFormatsX:2",
        "b=x-rl-video.hdrFormats:2",
    ] {
        assert_eq!(with(&format!("{line}\r\n")).hdr_formats, 0, "{line}");
    }
    assert_eq!(with("a=x-rl-audio.codecs:6\r\n").audio_codecs, 6);
    assert_eq!(with("a=x-rl-audio.codecs:\r\n").audio_codecs, 0);
    for line in [
        "a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8",
        "a=x-rl-audio.layout714:12,7,6,0,1,10,11,2,3,4,5,6,7,8,9",
        "a=x-rl-audio.layout714:8,5,3,0,1,2,3,4,5,6,7",
    ] {
        assert_eq!(with(&format!("{line}\r\n")).layout_714, None, "{line}");
    }
    for line in [
        "a=x-ss-general.penBarrelRoll:0",
        "a=x-ss-general.penBarrelRoll:",
        "a=x-ss-general.penBarrelRoll:yes",
    ] {
        assert!(!with(&format!("{line}\r\n")).pen_barrel_roll, "{line}");
    }
}

#[test]
fn roaming_needs_the_advertisement() {
    let with = |line: &str| DescribeInfo::parse(&format!("{RUBYLIGHT}{line}\r\n")).roaming;
    assert!(with(rubylight_protocol::roaming::SDP_ATTRIBUTE));
    assert!(with("a=x-rl-roaming:1"));
    assert!(
        with("a=x-rl-roaming:2"),
        "a newer host still speaks version 1"
    );
    assert!(!with("a=x-rl-roaming:0"));
    assert!(!with("a=x-rl-roaming:"));
    assert!(!with("a=x-rl-roaming:yes"));
    assert!(!with("a=x-rl-roamingx:1"));
    assert!(!with("b=x-rl-roaming:1"));
}

#[test]
fn video_format_selection() {
    let info = DescribeInfo::parse(RUBYLIGHT);
    let all = 0x0F_FF05;
    assert_eq!(
        info.select_video_format(all, SCM_PYROWAVE | SCM_PYROWAVE_MAIN10),
        VIDEO_FORMAT_PYROWAVE_MAIN10
    );
    assert_eq!(
        info.select_video_format(
            all & !VIDEO_FORMAT_MASK_PYROWAVE,
            SCM_AV1_MAIN8 | SCM_AV1_MAIN10
        ),
        VIDEO_FORMAT_AV1_MAIN10
    );
    assert_eq!(
        info.select_video_format(
            VIDEO_FORMAT_H265 | VIDEO_FORMAT_H264,
            SCM_HEVC | SCM_HEVC_MAIN10
        ),
        VIDEO_FORMAT_H265
    );
    assert_eq!(
        info.select_video_format(
            VIDEO_FORMAT_H264 | VIDEO_FORMAT_H264_HIGH8_444,
            SCM_H264_HIGH8_444
        ),
        VIDEO_FORMAT_H264_HIGH8_444
    );
    // A future PyroWave bitstream falls back to the ordinary codecs.
    let future = DescribeInfo::parse(&RUBYLIGHT.replace("186f0393", "deadbeef"));
    assert!(!future.pyrowave);
    assert_eq!(
        future.select_video_format(all, SCM_PYROWAVE | SCM_AV1_MAIN8),
        VIDEO_FORMAT_AV1_MAIN8
    );
    let plain = DescribeInfo::parse("a=x-ss-general.featureFlags:0\r\n");
    assert_eq!(
        plain.select_video_format(all, 0x0FFF_FFFF),
        VIDEO_FORMAT_H264_HIGH8_444
    );
    assert!(plain.microphone.is_none());
}

/// Forced PyroWave with HDR on, against a Rubylight 3.0 host (ServerCodecModeSupport
/// 0x47830301): the formats the Java side hands over keep PyroWave 10-bit, and so does the pick.
#[test]
fn forced_pyrowave_hdr_selects_pyrowave_main10() {
    let info = DescribeInfo::parse(RUBYLIGHT);
    let scm = 0x4783_0301;
    let hdr_codecs = VIDEO_FORMAT_H264
        | VIDEO_FORMAT_H265
        | VIDEO_FORMAT_H265_MAIN10
        | VIDEO_FORMAT_AV1_MAIN8
        | VIDEO_FORMAT_AV1_MAIN10;
    assert_eq!(
        info.select_video_format(hdr_codecs | VIDEO_FORMAT_PYROWAVE_MAIN10, scm),
        VIDEO_FORMAT_PYROWAVE_MAIN10
    );
    assert_eq!(
        info.select_video_format(hdr_codecs | VIDEO_FORMAT_PYROWAVE_MAIN10_444, scm),
        VIDEO_FORMAT_PYROWAVE_MAIN10_444
    );
    // Without PyroWave 10-bit on the device, HDR is AV1 Main10 from the same host.
    assert_eq!(
        info.select_video_format(hdr_codecs | VIDEO_FORMAT_PYROWAVE, scm),
        VIDEO_FORMAT_PYROWAVE
    );
    assert_eq!(
        info.select_video_format(hdr_codecs, scm),
        VIDEO_FORMAT_AV1_MAIN10
    );
}

#[test]
fn opus_layouts() {
    let stereo = parse_opus_configs(RUBYLIGHT, 2).unwrap();
    assert_eq!(
        (
            stereo.normal.streams,
            stereo.normal.coupled_streams,
            stereo.high_quality
        ),
        (1, 1, None)
    );
    let surround = parse_opus_configs(RUBYLIGHT, 6).unwrap();
    assert_eq!(
        (surround.normal.streams, surround.normal.coupled_streams),
        (4, 2)
    );
    // GFE's order with LFE moved after C.
    assert_eq!(surround.normal.mapping[..6], [0, 1, 4, 3, 5, 2]);
    let hq = surround.high_quality.unwrap();
    assert_eq!(
        (hq.streams, hq.coupled_streams, hq.mapping[..6].to_vec()),
        (6, 0, vec![0, 1, 4, 5, 2, 3])
    );
    let seven = parse_opus_configs(RUBYLIGHT, 8).unwrap();
    assert_eq!((seven.normal.streams, seven.normal.coupled_streams), (5, 3));
    assert_eq!(seven.normal.mapping[..8], [0, 1, 4, 3, 5, 6, 7, 2]);
    let legacy = parse_opus_configs("", 6).unwrap();
    assert_eq!(legacy.normal.mapping[..6], [0, 4, 1, 5, 2, 3]);
    assert!(parse_opus_configs("", 8).is_err());
    assert!(parse_opus_configs("a=fmtp:97 surround-params=64201", 6).is_err());
    assert!(parse_opus_configs(RUBYLIGHT, 13).is_err());
}

/// 7.1.4 as Foundation Sunshine and Rubylight's DESCRIBE send it, in the comma form, a 3.0
/// rc host's x-rl lines, and V+'s fallback when a host sends none.
#[test]
fn seven_one_four_layouts() {
    use rubylight_protocol::audio_ext::{layout_714_lines, LAYOUT_714, LAYOUT_714_HQ};
    let sdp = format!("{RUBYLIGHT}{}", layout_714_lines().concat());
    let configs = parse_opus_configs(&sdp, 12).unwrap();
    let config = |layout: rubylight_protocol::audio_ext::Layout| OpusConfig {
        sample_rate: 48000,
        channel_count: 12,
        streams: layout.streams,
        coupled_streams: layout.coupled,
        mapping: layout.mapping,
    };
    assert_eq!(configs.normal, config(LAYOUT_714));
    assert_eq!(configs.high_quality, Some(config(LAYOUT_714_HQ)));
    assert_eq!(
        (configs.normal.streams, configs.normal.coupled_streams),
        (8, 4)
    );
    assert_eq!(
        configs.normal.mapping,
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
    );
    // The 12-channel lines leave 5.1 and 7.1 as they were.
    assert_eq!(
        parse_opus_configs(&sdp, 8).unwrap(),
        parse_opus_configs(RUBYLIGHT, 8).unwrap()
    );
    // A host without the lines: V+'s fallback, normal quality only.
    let fallback = parse_opus_configs(RUBYLIGHT, 12).unwrap();
    assert_eq!(
        (fallback.normal, fallback.high_quality),
        (config(LAYOUT_714), None)
    );
    // V+ takes ; : | too, and a delimited 7.1 line.
    let other = parse_opus_configs(
        "a=fmtp:97 surround-params=12;8;4;0;1;2;3;4;5;6;7;8;9;10;11\r\n",
        12,
    )
    .unwrap();
    assert_eq!(other.normal, config(LAYOUT_714));
    let seven =
        parse_opus_configs("a=fmtp:97 surround-params=8,5,3,0,1,4,5,6,7,2,3\r\n", 8).unwrap();
    assert_eq!(seven.normal.mapping[..8], [0, 1, 4, 3, 5, 6, 7, 2]);
    // A 3.0 rc host (rc.4, rc.5) sends no surround-params for 7.1.4, only its x-rl lines,
    // exactly as written here; those win over V+'s fallback.
    let rc = format!(
        "{RUBYLIGHT}a=x-rl-audio.codecs:15\r\n\
             a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-audio.layout714Hq:12,12,0,0,1,10,11,2,3,4,5,6,7,8,9\r\n"
    );
    let rc_mapping = [0, 1, 10, 11, 2, 3, 4, 5, 6, 7, 8, 9];
    let configs = parse_opus_configs(&rc, 12).unwrap();
    assert_eq!(
        (
            configs.normal.channel_count,
            configs.normal.streams,
            configs.normal.coupled_streams,
            configs.normal.mapping
        ),
        (12, 7, 5, rc_mapping)
    );
    let hq = configs.high_quality.unwrap();
    assert_eq!(
        (hq.streams, hq.coupled_streams, hq.mapping),
        (12, 0, rc_mapping)
    );
    // Without the HQ line there is no HQ layout; surround-params still come first.
    let configs = parse_opus_configs(&rc.replace("layout714Hq", "other"), 12).unwrap();
    assert_eq!((configs.normal.streams, configs.high_quality), (7, None));
    let both = parse_opus_configs(&format!("{rc}{}", layout_714_lines().concat()), 12).unwrap();
    assert_eq!(
        (both.normal, both.high_quality),
        (config(LAYOUT_714), Some(config(LAYOUT_714_HQ)))
    );
    // A malformed x-rl line is an error, not V+'s layout.
    for bad in [
        "a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8\r\n",
        "a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\na=x-rl-audio.layout714Hq:12,7\r\n",
    ] {
        assert!(parse_opus_configs(bad, 12).is_err(), "{bad}");
    }
    for bad in [
        "a=fmtp:97 surround-params=12,8,4,0,1,2,3,4,5,6,7,8,9,10\r\n",
        "a=fmtp:97 surround-params=12,8,4,0,1,2,3,4,5,6,7,8,9,10,x\r\n",
        "a=fmtp:97 surround-params=12,8,4,0,1,2,3,4,5,6,7,8,9,10,256\r\n",
        "a=fmtp:97 surround-params=12,8,4,0,1,2,3,4,5,6,7,8,9,10 11\r\n",
    ] {
        assert!(parse_opus_configs(bad, 12).is_err(), "{bad}");
    }
}

#[test]
fn control_message_lists() {
    assert_eq!(
        parse_control_messages("a=x-rl-control: 0x5530 ,21809,0x5530,0xZZ,70000,\r\nx"),
        [0x5530, 21809]
    );
    assert_eq!(
        parse_control_messages("a=x-rl-control:0x\r\na=x-rl-control:0X10"),
        [16]
    );
    assert!(parse_control_messages("b=x-rl-control:1").is_empty());
}

#[test]
fn attributes_parse_like_strtoul() {
    assert_eq!(sdp_attribute_u32("a=x:0x10\r\n", "x"), Some(16));
    assert_eq!(sdp_attribute_u32("a=x:-1\r\n", "x"), Some(u32::MAX));
    assert_eq!(sdp_attribute_u32("a=y:1", "x"), None);
}
