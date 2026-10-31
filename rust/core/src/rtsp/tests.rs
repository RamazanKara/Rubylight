use super::*;
use crate::webrtc::capabilities::Codec;

#[test]
fn webrtc_negotiation_has_no_fec_one_slice_and_5_ms_opus() {
    for (codec, number) in [(Codec::H264, 0), (Codec::Hevc, 1), (Codec::Av1, 2)] {
        let n = Negotiated::webrtc(1920, 1080, 60, codec, false, 20_000, 2);
        n.validate().unwrap();
        assert_eq!(
            (n.width, n.height, n.fps, n.codec),
            (1920, 1080, 60, number)
        );
        assert_eq!((n.slices, n.min_fec, n.audio_packet_ms), (1, 0, 5));
        assert_eq!(n.references, 1, "no long-term references for invalidation");
        assert_eq!(n.encryption, 0);
        assert!(!n.video_qos && !n.audio_qos && !n.hdr && !n.mic);
        assert_eq!((n.chroma_packing, n.yuv444), (0, false));
        assert_eq!(n.fps_millihz(), 60_000);
    }
    let hdr = Negotiated::webrtc(3840, 2160, 120, Codec::Hevc, true, 50_000, 2);
    hdr.validate().unwrap();
    assert!(hdr.hdr && hdr.ten_bit());
}

#[test]
fn webrtc_video_budget_is_the_bitrate_minus_audio_and_rtp_overhead() {
    let video = |kbps, channels| {
        Negotiated::webrtc(1920, 1080, 60, Codec::H264, false, kbps, channels).bitrate_kbps
    };
    // 128 kbps of stereo Opus, then 1100 payload bytes of every 1150.
    assert_eq!(video(20_000, 2), (20_000 - 128) * 1100 / 1150);
    assert_eq!(video(20_000, 6), (20_000 - 256) * 1100 / 1150);
    assert_eq!(video(20_000, 8), (20_000 - 450) * 1100 / 1150);
    assert!(video(20_000, 8) < video(20_000, 6) && video(20_000, 6) < video(20_000, 2));
    // The whole stream never exceeds what the client asked for.
    for channels in [2, 6, 8] {
        let overhead =
            u64::from(video(50_000, channels)) * WEBRTC_DATAGRAM_BYTES / WEBRTC_PAYLOAD_BYTES;
        assert!(overhead + u64::from(webrtc_audio_kbps(channels)) <= 50_000);
    }
    // A degenerate request still leaves the encoder a positive rate.
    assert_eq!(video(100, 8), 1);
    assert_eq!(
        Negotiated::webrtc(1920, 1080, 60, Codec::H264, false, 20_000, 2).configured_bitrate_kbps,
        20_000
    );
}

#[test]
fn webrtc_negotiation_with_an_unsupported_layout_does_not_validate() {
    for channels in [0, 1, 3, 4, 5, 7] {
        assert!(
            Negotiated::webrtc(1920, 1080, 60, Codec::H264, false, 20_000, channels)
                .validate()
                .is_err(),
            "{channels}"
        );
    }
}

#[test]
fn stream_rate_cannot_be_below_one_frame_per_second() {
    for rate_millihz in [1, 500, 999] {
        assert!(
            Negotiated {
                fps: 1,
                rate_millihz,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    for rate_millihz in [0, 1000, 59_940, 4_000_000] {
        assert!(
            Negotiated {
                fps: 1,
                rate_millihz,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
    assert!(
        Negotiated::from_sdp(
            b"a=x-nv-video[0].maxFPS:1\r\na=x-nv-video[0].clientRefreshRateX100:50\r\n"
        )
        .is_err()
    );
}
#[test]
fn pyrowave_downgrades_report_the_negotiated_codec_without_rewriting_it() {
    assert!(
        codec_warning("pyrowave", 0, false)
            .unwrap()
            .contains("negotiated H.264")
    );
    assert!(
        codec_warning("pyrowave", 1, true)
            .unwrap()
            .contains("client selected another codec")
    );
    assert!(codec_warning("auto", 0, false).is_none());
    assert!(codec_warning("pyrowave", 3, true).is_none());
}

#[test]
fn fragmented_rtsp_and_duplicate_lengths() {
    let b = b"ANNOUNCE rtsp://host RTSP/1.0\r\nCSeq: 2\r\nContent-Length: 5\r\n\r\nhello";
    for i in 0..b.len() {
        assert_eq!(complete_length(&b[..i]).unwrap(), None);
    }
    let r = Request::parse(b).unwrap();
    assert_eq!(r.body, b"hello");
    assert_eq!(r.cseq, 2);
    assert!(
        complete_length(b"OPTIONS * RTSP/1.0\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n")
            .is_err()
    );
}
#[test]
fn message_buffer_handles_every_header_boundary_and_large_chunks() {
    let raw = b"ANNOUNCE rtsp://host RTSP/1.0\r\nCSeq: 2\r\nContent-Length: 5\r\n\r\nhello";
    for split in 0..raw.len() {
        let mut message = MessageBuffer::default();
        assert!(!message.append(&raw[..split]).unwrap());
        assert!(message.append(&raw[split..]).unwrap());
        assert_eq!(
            Request::parse(&message.into_bytes()).unwrap().body,
            b"hello"
        );
    }
    let mut raw = b"OPTIONS * RTSP/1.0\r\nCSeq: 2\r\nX-Fill: ".to_vec();
    raw.resize(MAX_MESSAGE - 4, b'a');
    raw.extend_from_slice(b"\r\n\r\n");
    for chunk in [1, 3, 4096, MAX_MESSAGE] {
        let mut message = MessageBuffer::default();
        let mut consumed = 0;
        for bytes in raw.chunks(chunk) {
            consumed += bytes.len();
            assert_eq!(message.append(bytes).unwrap(), consumed == raw.len());
        }
        let parsed = Request::parse(&message.into_bytes()).unwrap();
        assert_eq!(parsed.cseq, 2);
        assert!(parsed.body.is_empty());
    }
    let mut message = MessageBuffer::default();
    assert!(
        !message
            .append(b"ANNOUNCE * RTSP/1.0\r\nCSeq: 3\r\nContent-Length: 60000\r\n\r\n")
            .unwrap()
    );
    for i in 0..60 {
        assert_eq!(message.append(&[b'x'; 1000]).unwrap(), i == 59);
    }
    assert_eq!(
        Request::parse(&message.into_bytes()).unwrap().body.len(),
        60_000
    );
}
#[test]
fn message_buffer_rejects_oversized_duplicate_and_trailing_data() {
    for raw in [
        b"OPTIONS * RTSP/1.0\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n".as_slice(),
        b"OPTIONS * RTSP/1.0\r\nContent-Length: 65536\r\n\r\n",
        b"OPTIONS * RTSP/1.0\r\nContent-Length: 0\r\n\r\nx",
    ] {
        assert!(MessageBuffer::default().append(raw).is_err());
    }
    let mut message = MessageBuffer::default();
    assert!(!message.append(&vec![b'x'; MAX_MESSAGE]).unwrap());
    assert!(message.append(b"x").is_err());
    let mut message = MessageBuffer::default();
    assert!(message.append(b"OPTIONS * RTSP/1.0\r\n\r\n").unwrap());
    assert!(message.append(b"x").is_err());
}
#[test]
fn sdp_negotiation_rejects_integers_that_would_truncate() {
    assert!(Negotiated::from_sdp(b"a=x-nv-video[0].clientViewportWd:4294967295\n").is_err());
    assert!(Negotiated::from_sdp(b"a=x-nv-vqos[0].bitStreamFormat:256\n").is_err());
}
#[test]
fn pyrowave_framing_presence_and_feature_bit_match_vrr_clients() {
    for attribute in [
        "x-ss-video[0].pyrowaveAdaptiveFec:0",
        "x-ss-video[0].pyrowaveFeatures:1",
    ] {
        let sdp = format!(
            "a=x-nv-vqos[0].bitStreamFormat:3\na={attribute}\na=x-ss-video[0].vrrLowLatency:1\na=x-nv-vqos[0].bw.maximumBitrateKbps:800000\n"
        );
        let config = Negotiated::from_sdp(sdp.as_bytes()).unwrap();
        assert!(config.pyrowave_records && config.vrr_low_latency);
        assert_eq!(config.codec, 3);
    }
    assert!(
        !Negotiated::from_sdp(b"a=x-ss-video[0].pyrowaveFeatures:2\n")
            .unwrap()
            .pyrowave_records
    );
    // Issue #31: the client's wired link and calibrated pace, as Nonary's
    // Moonlight and Nightjar announce them.
    let hints = Negotiated::from_sdp(
        b"a=x-ss-video[0].pyrowaveLinkMbps:2500\na=x-ss-video[0].pyrowavePaceMbps:2100\n",
    )
    .unwrap();
    assert_eq!(
        (
            hints.pyrowave_client_link_mbps,
            hints.pyrowave_client_pace_mbps
        ),
        (2500, 2100)
    );
    let none = Negotiated::from_sdp(b"a=x-nv-video[0].maxFPS:60\n").unwrap();
    assert_eq!(
        (
            none.pyrowave_client_link_mbps,
            none.pyrowave_client_pace_mbps
        ),
        (0, 0)
    );
    assert!(
        describe(0, 7, 0, true, true, true, false, false)
            .contains("a=x-ss-pyrowave.bitstream:186f0393\r\n")
    );
}
#[test]
fn ltr_recovery_comes_from_the_launch_query_and_only_for_a_one() {
    let launch = |value: Option<&str>| {
        let mut n = Negotiated::from_sdp(b"a=x-nv-vqos[0].bitStreamFormat:1\n").unwrap();
        let options = value
            .map(|value| BTreeMap::from([("rlLtrRecovery".to_string(), value.to_string())]))
            .unwrap_or_default();
        n.apply_launch_options(&options);
        n.ltr_recovery
    };
    assert!(!Negotiated::default().ltr_recovery);
    assert!(launch(Some("1")));
    for value in [None, Some("0"), Some("true"), Some("")] {
        assert!(!launch(value), "{value:?}");
    }
    // An SDP line does not ask for it.
    let sdp = Negotiated::from_sdp(b"a=rlLtrRecovery:1\na=x-rl-video[0].ltrRecovery:1\n");
    assert!(!sdp.unwrap().ltr_recovery);
}
#[test]
fn the_client_wireless_hint_is_on_only_for_a_one() {
    let wireless = |sdp: &[u8]| Negotiated::from_sdp(sdp).unwrap().client_wireless;
    assert!(!Negotiated::default().client_wireless);
    assert!(!wireless(b"a=x-nv-video[0].maxFPS:60\n"));
    assert!(wireless(b"a=x-rl-video[0].clientWireless:1\n"));
    assert!(!wireless(b"a=x-rl-video[0].clientWireless:0\n"));
    assert!(!wireless(b"a=x-rl-video[0].clientWireless:2\n"));
    assert!(Negotiated::from_sdp(b"a=x-rl-video[0].clientWireless:yes\n").is_err());
    // The wired-only link hint is independent of it.
    let both = Negotiated::from_sdp(
        b"a=x-ss-video[0].pyrowaveLinkMbps:0\na=x-rl-video[0].clientWireless:1\n",
    )
    .unwrap();
    assert!((both.pyrowave_client_link_mbps, both.client_wireless) == (0, true));
}
#[test]
fn hlg_and_dolby_vision_8_4_streams_take_the_hlg_transfer() {
    let stream = |hdr, hdr_format| Negotiated {
        hdr,
        hdr_format,
        ..Negotiated::default()
    };
    assert!(stream(true, 2).hlg() && stream(true, 8).hlg());
    for (hdr, format) in [(true, 0), (true, 1), (true, 4), (false, 2), (false, 8)] {
        assert!(!stream(hdr, format).hlg(), "{hdr} {format}");
    }
}
#[test]
fn the_3_0_choices_parse_as_numbers_and_default_to_stock() {
    let stock = Negotiated::from_sdp(b"a=x-nv-video[0].dynamicRangeMode:1\n").unwrap();
    assert_eq!((stock.hdr_format, stock.audio_codec), (0, 0));
    let picked = Negotiated::from_sdp(
        b"a=x-nv-video[0].dynamicRangeMode:1\na=x-rl-video[0].hdrFormat:2\na=x-rl-audio.codec:8\n",
    )
    .unwrap();
    assert_eq!((picked.hdr_format, picked.audio_codec), (2, 8));
    for bad in [
        &b"a=x-rl-video[0].hdrFormat:pq\n"[..],
        b"a=x-rl-video[0].hdrFormat:256\n",
        b"a=x-rl-video[0].hdrFormat:-1\n",
        b"a=x-rl-audio.codec:pcm\n",
        b"a=x-rl-audio.codec:300\n",
    ] {
        assert!(
            Negotiated::from_sdp(bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
    // An ANNOUNCE from before these fields still loads.
    let mut old = serde_json::to_value(Negotiated::default()).unwrap();
    old.as_object_mut().unwrap().remove("hdr_format");
    old.as_object_mut().unwrap().remove("audio_codec");
    let old: Negotiated = serde_json::from_value(old).unwrap();
    assert_eq!((old.hdr_format, old.audio_codec), (0, 0));
}
#[test]
fn validate_admits_twelve_channels_and_no_other_new_count() {
    let channels = |audio_channels| {
        Negotiated {
            audio_channels,
            ..Default::default()
        }
        .validate()
    };
    for ok in [2, 6, 8, 12] {
        assert!(channels(ok).is_ok(), "{ok}");
    }
    for bad in [0, 1, 4, 7, 10, 11, 13, 16] {
        assert!(channels(bad).is_err(), "{bad}");
    }
}
#[test]
fn the_row_order_hint_is_on_only_for_a_one_with_record_framing() {
    let sdp = |hint: &str| format!("a=x-ss-video[0].pyrowaveAdaptiveFec:0\n{hint}");
    let rows = |hint: &str| {
        Negotiated::from_sdp(sdp(hint).as_bytes())
            .unwrap()
            .pyrowave_row_order
    };
    assert!(!rows(""), "an old client sends no key");
    assert!(!rows("a=x-ss-video[0].pyrowaveRowOrder:0\n"));
    assert!(rows("a=x-ss-video[0].pyrowaveRowOrder:1\n"));
    assert!(
        !rows("a=x-ss-video[0].pyrowaveRowOrder:2\n"),
        "a later value is not this one"
    );
    // Without record framing there are no records to reorder.
    assert!(
        !Negotiated::from_sdp(b"a=x-ss-video[0].pyrowaveRowOrder:1\n")
            .unwrap()
            .pyrowave_row_order
    );
    // Garbage is refused like every other numeric key, not read as off.
    assert!(
        Negotiated::from_sdp(sdp("a=x-ss-video[0].pyrowaveRowOrder:yes\n").as_bytes()).is_err()
    );
    assert!(Negotiated::from_sdp(b"a=x-ss-video[0].pyrowaveRowOrder:yes\n").is_err());
    assert!(!Negotiated::default().pyrowave_row_order);
}
#[test]
fn rubylight_control_messages_are_advertised_once_before_the_microphone() {
    for mic in [false, true] {
        let sdp = describe(0, 7, 1, true, true, true, mic, true);
        assert_eq!(
            sdp.lines()
                .filter(|line| line.starts_with("a=x-rl-control:"))
                .collect::<Vec<_>>(),
            ["a=x-rl-control:0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537,0x5538"]
        );
        assert!(
            !sdp.lines().any(|line| line.starts_with("m=")),
            "the microphone's m= line is appended last"
        );
    }
    assert_eq!(
        [
            crate::phase_sync::REPORT_MESSAGE_TYPE,
            crate::display_caps::DISPLAY_CAPS_MESSAGE_TYPE,
            rubylight_protocol::control::RECONFIGURE_MESSAGE_TYPE,
            crate::controller_audio::SPEAKER,
            crate::controller_audio::MICROPHONE,
            crate::controller_audio::CONTROL,
            crate::host_timing::HOST_TIMING_MESSAGE_TYPE,
            crate::clock_sync::CLOCK_SYNC_MESSAGE_TYPE,
            crate::link_notice::LINK_NOTICE_MESSAGE_TYPE,
        ]
        .map(|id| format!("{id:#06x}"))
        .join(","),
        "0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537,0x5538"
    );
}
#[test]
fn roaming_is_offered_once_only_when_on_and_changes_nothing_else() {
    for mic in [false, true] {
        let on = describe(0, 7, 1, true, true, true, mic, true);
        let off = describe(0, 7, 1, true, true, true, mic, false);
        assert_eq!(
            on.lines()
                .filter(|line| line.starts_with("a=x-rl-roaming"))
                .collect::<Vec<_>>(),
            ["a=x-rl-roaming:1"]
        );
        assert!(!off.contains("x-rl-roaming"));
        // Stock clients see the same answer plus one line they ignore.
        assert_eq!(on.replace("a=x-rl-roaming:1\r\n", ""), off);
    }
}
#[test]
fn the_microphone_is_offered_encrypted_only_when_the_host_takes_it() {
    let without = describe(0, 5, 1, false, false, false, false, false);
    assert!(without.contains("encryptionSupported:5\r\n"));
    assert!(without.contains("encryptionRequested:1\r\n"));
    let with = describe(0, 7, 7, false, false, false, true, false);
    assert!(with.contains("encryptionSupported:15\r\n"));
    assert!(with.contains("encryptionRequested:15\r\n"));
    // A client that encrypts its microphone negotiates bit 8.
    let n = Negotiated::from_sdp(b"a=x-ss-general.encryptionEnabled:15\n").unwrap();
    assert_eq!(n.encryption, 15);
    assert!(!n.mic, "only a microphone SETUP turns it on");
    assert!(Negotiated::from_sdp(b"a=x-ss-general.encryptionEnabled:16\n").is_err());
}
#[test]
fn qos_tags_follow_the_client() {
    let local =
        Negotiated::from_sdp(b"a=x-nv-vqos[0].qosTrafficType:5\na=x-nv-aqos.qosTrafficType:4\n")
            .unwrap();
    assert!(local.video_qos && local.audio_qos);
    let remote =
        Negotiated::from_sdp(b"a=x-nv-vqos[0].qosTrafficType:0\na=x-nv-aqos.qosTrafficType:0\n")
            .unwrap();
    assert!(!remote.video_qos && !remote.audio_qos);
    let silent = Negotiated::from_sdp(b"").unwrap();
    assert!(silent.video_qos && silent.audio_qos);
}
#[test]
fn packed_chroma_is_explicit_and_keeps_display_geometry() {
    let request = b"a=x-nv-vqos[0].bitStreamFormat:1\na=x-rl-video[0].chromaPacking:1\na=x-nv-video[0].clientViewportWd:1922\na=x-nv-video[0].clientViewportHt:1082\n";
    let config = Negotiated::from_sdp(request).unwrap();
    assert_eq!((config.width, config.height), (1922, 1082));
    assert_eq!(config.coded_size(), (1984, 2176));
    assert!(!config.yuv444);
    assert_eq!(Negotiated::default().chroma_packing, 0);
    for suffix in [
        "a=x-rl-video[0].chromaPacking:3",
        "a=x-ss-video[0].chromaSamplingType:1",
        "a=x-nv-vqos[0].bitStreamFormat:0",
        "a=x-nv-video[0].clientViewportWd:0",
    ] {
        let mut invalid = request.to_vec();
        invalid.extend_from_slice(suffix.as_bytes());
        assert!(Negotiated::from_sdp(&invalid).is_err());
    }
    let layered =
        Negotiated::from_sdp(&[request.as_slice(), b"a=x-rl-video[0].chromaPacking:2"].concat())
            .unwrap();
    assert_eq!(layered.chroma_packing, 2);
    assert_eq!(layered.coded_size(), config.coded_size());
}
#[test]
fn packed_hevc_rejects_coded_width_below_amfs_documented_minimum() {
    let mut config = Negotiated {
        width: 66,
        height: 70,
        codec: 1,
        chroma_packing: crate::packed444::VERSION,
        ..Default::default()
    };
    for ten_bit in [false, true] {
        config.sdr_10bit = ten_bit;
        for width in [64, 66, 128] {
            config.width = width;
            let error = config.validate().unwrap_err().to_string();
            assert!(error.contains("coded width of at least 192"), "{error}");
        }
        for width in [129, 130, 192] {
            config.width = width;
            assert_eq!(config.coded_size().0, 192);
            config.validate().unwrap();
        }
    }
    config.width = 128;
    config.codec = 2;
    config.validate().unwrap();
    config.codec = 1;
    config.chroma_packing = 0;
    config.validate().unwrap();
}
#[test]
fn fractional_negotiation_does_not_use_stale_display_refresh() {
    let n = Negotiated::from_sdp(
        b"a=x-nv-video[0].maxFPS:60\na=x-nv-video[0].clientRefreshRateX100:5994\n",
    )
    .unwrap();
    assert_eq!(n.fps_millihz(), 59940);
    let n = Negotiated::from_sdp(
        b"a=x-nv-video[0].maxFPS:120\na=x-nv-video[0].clientRefreshRateX100:5994\n",
    )
    .unwrap();
    assert_eq!(n.fps_millihz(), 120000);
    assert_eq!(
        Negotiated::from_sdp(b"a=x-nv-video[0].maxFPS:119880\n")
            .unwrap()
            .fps_millihz(),
        119880
    );
}
