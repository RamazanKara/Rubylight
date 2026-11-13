use super::testing::*;
use super::*;

const HOST: &str = "192.168.1.10:8001";
const CLIENT: &str = "192.168.1.20:50000";

fn offer_for(spec: &OfferSpec) -> String {
    host_offer(spec, HOST.parse().unwrap()).1.sdp
}

fn video_section_of(sdp: &str) -> Vec<String> {
    let l = lines(sdp);
    l[section(&l, "video").unwrap()].to_vec()
}

#[test]
fn offer_has_one_codec_at_the_fixed_payload_type() {
    for (codec, name) in [
        (VideoCodec::H264, "H264/90000"),
        (VideoCodec::Hevc, "H265/90000"),
        (VideoCodec::Av1, "AV1/90000"),
    ] {
        let sdp = offer_for(&spec(codec, false));
        let video = video_section_of(&sdp);
        assert!(
            video[0].starts_with("m=video ") && video[0].ends_with(" 96"),
            "{}",
            video[0]
        );
        let rtpmaps: Vec<_> = video
            .iter()
            .filter(|l| l.starts_with("a=rtpmap:"))
            .collect();
        assert_eq!(rtpmaps, [&format!("a=rtpmap:96 {name}")]);
        assert!(sdp.contains("a=ice-lite"));
    }
}

#[test]
fn offer_has_exactly_one_imageattr_and_the_playout_delay_extension() {
    let sdp = offer_for(&spec(VideoCodec::Hevc, false));
    assert_eq!(sdp.matches("a=imageattr:").count(), 1);
    let video = video_section_of(&sdp);
    assert!(
        video
            .contains(&"a=imageattr:96 send [x=[1920:1920],y=[1080:1080],fps=[60:60]]".to_string())
    );
    assert!(
        video
            .iter()
            .any(|l| l.starts_with("a=extmap:6 ") && l.contains(PLAYOUT_DELAY_URI))
    );
    assert!(!sdp.contains("rtx") && !sdp.contains("ssrc-group"), "{sdp}");
    assert_eq!(
        video
            .iter()
            .filter(|l| l.starts_with("a=ssrc:") && l.contains(" cname:"))
            .count(),
        1
    );
    assert!(
        !sdp.contains("transport-cc") && !sdp.contains("goog-remb"),
        "{sdp}"
    );
    assert!(!sdp.contains("a=rtcp-fb:96 nack\r\n"), "{sdp}");
    assert!(
        video.iter().any(|l| l == "a=rtcp-fb:96 nack pli"),
        "{video:?}"
    );
    assert!(
        video.iter().any(|l| l == "a=rtcp-fb:96 ccm fir"),
        "{video:?}"
    );
    assert!(sdp.contains("m=application"));
    assert!(sdp.contains("a=group:BUNDLE"));
}

#[test]
fn offer_for_a_different_mode_has_its_own_imageattr() {
    let mut s = spec(VideoCodec::Av1, false);
    s.video.width = 3840;
    s.video.height = 2160;
    s.video.fps = 120;
    assert!(
        offer_for(&s).contains("a=imageattr:96 send [x=[3840:3840],y=[2160:2160],fps=[120:120]]")
    );
}

#[test]
fn rtx_and_nack_come_back_only_when_asked() {
    let mut s = spec(VideoCodec::H264, false);
    s.rtx = true;
    let sdp = offer_for(&s);
    assert!(sdp.contains("a=rtpmap:97 rtx/90000"), "{sdp}");
    assert!(sdp.contains("a=rtcp-fb:96 nack\r\n"), "{sdp}");
    assert!(sdp.contains("a=ssrc-group:FID"), "{sdp}");
    assert!(!sdp.contains("transport-cc") && !sdp.contains("goog-remb"));
}

fn fmtp(sdp: &str) -> String {
    video_section_of(sdp)
        .iter()
        .find(|l| l.starts_with("a=fmtp:96 "))
        .cloned()
        .unwrap_or_default()
}

fn param_of(line: &str, name: &str) -> Option<String> {
    fmtp_params(line)
        .into_iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

#[test]
fn hevc_fmtp_levels_come_from_core_at_main_tier() {
    // (width, height, fps, kbps, level-id, tier-flag); core's levels, never below 4.1, and
    // Main tier even where core's table would need High (the last case).
    let cases = [
        (1280, 720, 60, 10_000, 123, 0),
        (1920, 1080, 60, 10_000, 123, 0),
        (1920, 1080, 60, 20_001, 150, 0),
        (1920, 1080, 120, 10_000, 150, 0),
        (2560, 1440, 60, 10_000, 150, 0),
        (2560, 1440, 120, 10_000, 153, 0),
        (3840, 2160, 60, 10_000, 153, 0),
        (3840, 2160, 120, 10_000, 156, 0),
        (3840, 2160, 120, 100_000, 156, 0),
    ];
    for (w, h, f, kbps, level, tier) in cases {
        for (hdr, profile) in [(false, 1), (true, 2)] {
            let mut s = spec(VideoCodec::Hevc, hdr);
            s.video.width = w;
            s.video.height = h;
            s.video.fps = f;
            s.video.bitrate_kbps = kbps;
            let line = fmtp(&offer_for(&s));
            let p = |n: &str| param_of(&line, n);
            assert_eq!(p("profile-id"), Some(profile.to_string()), "{line}");
            assert_eq!(p("level-asymmetry-allowed"), Some("1".into()), "{line}");
            assert_eq!(
                p("tier-flag"),
                Some(tier.to_string()),
                "{w}x{h}@{f} {kbps}: {line}"
            );
            assert_eq!(
                p("level-id"),
                Some(level.to_string()),
                "{w}x{h}@{f} {kbps}: {line}"
            );
        }
    }
}

#[test]
fn a_mode_no_level_holds_cannot_be_offered() {
    let mut s = spec(VideoCodec::Hevc, false);
    s.video.fps = 240;
    s.video.width = 3840;
    s.video.height = 2160;
    let Err(err) = rtc_config(&s) else {
        panic!("a mode no HEVC level holds was offered");
    };
    assert!(err.to_string().contains("no HEVC level"), "{err}");
    s.video.codec = VideoCodec::Av1;
    let Err(err) = rtc_config(&s) else {
        panic!("a mode no AV1 level holds was offered");
    };
    assert!(err.to_string().contains("no AV1 level"), "{err}");
}

#[test]
fn av1_fmtp_matches_the_mode() {
    for (w, h, f, kbps) in [
        (1920, 1080, 60, 20_000),
        (2560, 1440, 60, 20_000),
        (3840, 2160, 60, 20_000),
        (3840, 2160, 120, 20_000),
        (3840, 2160, 120, 200_000),
    ] {
        let (level, tier) = av1_level(w, h, f, kbps).unwrap();
        let mut s = spec(VideoCodec::Av1, true);
        s.video.width = w;
        s.video.height = h;
        s.video.fps = f;
        s.video.bitrate_kbps = kbps;
        let line = fmtp(&offer_for(&s));
        assert_eq!(param_of(&line, "profile").as_deref(), Some("0"), "{line}");
        assert_eq!(
            param_of(&line, "tier"),
            Some(tier.flag().to_string()),
            "{line}"
        );
        assert_eq!(
            param_of(&line, "level-idx"),
            Some(level.level_idx.to_string()),
            "{line}"
        );
    }
}

#[test]
fn h264_fmtp_is_packetization_mode_one_with_asymmetric_levels() {
    let line = fmtp(&offer_for(&spec(VideoCodec::H264, false)));
    let mut params: Vec<(String, String)> = fmtp_params(&line)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    params.sort();
    assert_eq!(
        params,
        [
            ("level-asymmetry-allowed".to_string(), "1".to_string()),
            ("packetization-mode".to_string(), "1".to_string()),
            ("profile-level-id".to_string(), "42e02a".to_string()),
        ],
        "{line}"
    );
}

#[test]
fn h264_level_comes_from_the_mode_and_rate() {
    use butterpollo_core::webrtc::capabilities::{FRAME_RATES, MODES};
    // (width, height, fps, profile-level-id) at each mode's default bitrate; the profile bytes
    // never change. 1080p60 is 4.2, 1080p120 5.1 and 1440p90 and 120 are 5.2 (table A-1).
    let expected = [
        (1280, 720, 30, "42e01f"),
        (1280, 720, 60, "42e020"),
        (1280, 720, 90, "42e02a"),
        (1280, 720, 120, "42e02a"),
        (1920, 1080, 30, "42e028"),
        (1920, 1080, 60, "42e02a"),
        (1920, 1080, 90, "42e033"),
        (1920, 1080, 120, "42e033"),
        (2560, 1440, 30, "42e032"),
        (2560, 1440, 60, "42e033"),
        (2560, 1440, 90, "42e034"),
        (2560, 1440, 120, "42e034"),
    ];
    let mut offered = 0;
    for mode in MODES.iter().filter(|m| m.h264) {
        for fps in FRAME_RATES {
            let mut s = spec(VideoCodec::H264, false);
            s.video.width = mode.width;
            s.video.height = mode.height;
            s.video.fps = fps;
            s.video.bitrate_kbps = mode.default_bitrate_kbps;
            let line = fmtp(&offer_for(&s));
            let want = expected
                .iter()
                .find(|(w, h, f, _)| (*w, *h, *f) == (mode.width, mode.height, fps))
                .map(|(.., id)| id.to_string());
            assert_eq!(
                param_of(&line, "profile-level-id"),
                want,
                "{}x{}@{fps}: {line}",
                mode.width,
                mode.height
            );
            // The answer keeps the profile and may name any level.
            let answer = answer_to(&s, &offer_for(&s));
            assert!(check_answer(&answer, &s).is_ok(), "{answer}");
            offered += 1;
        }
    }
    assert_eq!(offered, expected.len(), "every offered H.264 mode and rate");
}

#[test]
fn colour_space_extension_is_offered_for_hdr_only() {
    let hdr = offer_for(&spec(VideoCodec::Hevc, true));
    assert!(
        video_section_of(&hdr)
            .iter()
            .any(|l| l.starts_with("a=extmap:9 ") && l.contains(COLOR_SPACE_URI)),
        "{hdr}"
    );
    assert!(!offer_for(&spec(VideoCodec::Hevc, false)).contains(COLOR_SPACE_URI));
}

#[test]
fn stereo_audio_uses_the_protocol_fmtp() {
    let sdp = offer_for(&spec(VideoCodec::H264, false));
    assert!(sdp.contains("a=rtpmap:111 opus/48000/2"));
    assert!(sdp.contains(&format!("a=fmtp:111 {STEREO_FMTP}")), "{sdp}");
    assert!(!sdp.contains("multiopus"));
}

#[test]
fn the_offer_is_the_same_standard_opus_for_stereo_5_1_and_7_1() {
    // Only the identifiers str0m makes up for each offer differ; take them out.
    let normalised = |spec: &OfferSpec| {
        offer_for(spec)
            .lines()
            .filter(|l| {
                ![
                    "o=",
                    "a=ice-ufrag",
                    "a=ice-pwd",
                    "a=fingerprint:",
                    "a=ssrc",
                    "a=msid:",
                    "a=candidate:",
                    "a=mid:",
                    "a=group:BUNDLE",
                ]
                .iter()
                .any(|p| l.starts_with(p))
            })
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let stereo = normalised(&spec(VideoCodec::H264, false));
    for channels in [6, 8] {
        let surround = normalised(&surround_spec(channels));
        assert_eq!(stereo, surround, "{channels} channels");
    }
    let sdp = offer_for(&surround_spec(8));
    assert!(sdp.contains("a=rtpmap:111 opus/48000/2\r\n"), "{sdp}");
    assert!(
        sdp.contains(&format!("a=fmtp:111 {STEREO_FMTP}\r\n")),
        "{sdp}"
    );
    assert!(
        !sdp.contains("multiopus") && !sdp.contains("channel_mapping"),
        "{sdp}"
    );
}

/// The client side of an answer: what a Chromium-like receiver would send back.
fn answer_to(spec: &OfferSpec, offer: &str) -> String {
    let mut client = client(spec, CLIENT.parse().unwrap());
    answer(&mut client, offer)
}

#[test]
fn a_matching_answer_passes_for_every_codec() {
    for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
        for hdr in [false, true] {
            if hdr && codec == VideoCodec::H264 {
                continue;
            }
            let s = spec(codec, hdr);
            let answer = answer_to(&s, &offer_for(&s));
            let (_, info) = check_answer(&answer, &s)
                .unwrap_or_else(|e| panic!("{codec:?} {hdr}: {e}\n{answer}"));
            assert!(info.playout_delay, "{answer}");
        }
    }
}

/// Issue 64: at 4K120 above 60 Mbps AV1 is offered at High tier (HEVC stays Main), and a
/// Tizen TV or Chromium answers Main, the other tier, or no tier. The tier is the receiver's
/// to state, so the session starts; only a malformed tier is refused.
#[test]
fn an_answer_with_any_tier_starts_the_session() {
    for (codec, key, offered) in [
        (VideoCodec::Hevc, "tier-flag", 0),
        (VideoCodec::Av1, "tier", 1),
    ] {
        let mut s = spec(codec, false);
        s.video.width = 3840;
        s.video.height = 2160;
        s.video.fps = 120;
        // The most WebRTC accepts; HEVC and AV1 5.2 Main stop at 60 Mbps.
        s.video.bitrate_kbps = 100_000;
        let ours = format!("{key}={offered}");
        for (answered, tier) in [(Some(0), 0), (Some(1), 1), (None, 0)] {
            let (mut rtc, offer) = host_offer(&s, HOST.parse().unwrap());
            assert!(offer.sdp.contains(&ours), "{codec:?}: {}", offer.sdp);
            let answer = answer_to(&s, &offer.sdp);
            let answer = match answered {
                Some(t) => answer.replace(&ours, &format!("{key}={t}")),
                None => answer
                    .replace(&format!(";{ours}"), "")
                    .replace(&format!("{ours};"), ""),
            };
            assert_eq!(
                answer.contains(&format!("{key}=")),
                answered.is_some(),
                "{answer}"
            );
            let (_, info) = accept_answer(&mut rtc, offer, &answer, &s).unwrap_or_else(|e| {
                panic!(
                    "{codec:?} tier {answered:?}: {e}
{answer}"
                )
            });
            assert_eq!(
                info.video_tier,
                Some(tier),
                "{codec:?}
{answer}"
            );
        }
        let answer = answer_to(&s, &offer_for(&s));
        let bad = answer.replace(&ours, &format!("{key}=2"));
        let err = check_answer(&bad, &s).unwrap_err().to_string();
        assert!(err.contains("not 0 or 1"), "{err}");
    }
}

#[test]
fn answer_validation_rejects_a_changed_codec() {
    let s = spec(VideoCodec::Hevc, false);
    let answer = answer_to(&s, &offer_for(&s));
    let swapped = answer.replace("H265/90000", "H264/90000");
    let err = check_answer(&swapped, &s).unwrap_err().to_string();
    assert!(err.contains("changed the video codec"), "{err}");
    let added = answer.replace(
        "a=rtpmap:96 H265/90000",
        "a=rtpmap:96 H265/90000\r\na=rtpmap:98 VP8/90000",
    );
    assert!(
        check_answer(&added, &s)
            .unwrap_err()
            .to_string()
            .contains("added a video codec")
    );
}

#[test]
fn answer_validation_rejects_a_changed_profile() {
    let s = spec(VideoCodec::Hevc, true);
    let answer = answer_to(&s, &offer_for(&s));
    let changed = answer.replace("profile-id=2", "profile-id=1");
    assert!(
        check_answer(&changed, &s)
            .unwrap_err()
            .to_string()
            .contains("profile-id")
    );

    let s = spec(VideoCodec::Av1, false);
    let answer = answer_to(&s, &offer_for(&s));
    let changed = answer.replace("profile=0", "profile=1");
    assert!(
        check_answer(&changed, &s)
            .unwrap_err()
            .to_string()
            .contains("AV1 profile")
    );

    let s = spec(VideoCodec::H264, false);
    let answer = answer_to(&s, &offer_for(&s));
    let changed = answer.replace("profile-level-id=42e02a", "profile-level-id=64002a");
    assert_ne!(changed, answer);
    assert!(
        check_answer(&changed, &s)
            .unwrap_err()
            .to_string()
            .contains("H.264 profile")
    );
    // A different level is fine: the level is asymmetric.
    let level = answer.replace("profile-level-id=42e02a", "profile-level-id=42e034");
    assert_ne!(level, answer);
    assert!(check_answer(&level, &s).is_ok());
}

#[test]
fn answer_validation_rejects_rejected_sections() {
    let s = spec(VideoCodec::H264, false);
    let answer = answer_to(&s, &offer_for(&s));
    let video = answer.replacen("m=video 9 ", "m=video 0 ", 1);
    assert!(
        check_answer(&video, &s)
            .unwrap_err()
            .to_string()
            .contains("rejected the video")
    );
    let app = answer.replacen("m=application 9 ", "m=application 0 ", 1);
    assert!(
        check_answer(&app, &s)
            .unwrap_err()
            .to_string()
            .contains("DataChannel")
    );
}

#[test]
fn multiopus_answer_is_rewritten_back_to_stereo_opus_for_str0m() {
    for channels in [6, 8] {
        let s = surround_spec(channels);
        let (mut rtc, o) = host_offer(&s, HOST.parse().unwrap());
        // A Chromium-like client edits only its own answer.
        let answer = answer_to(&s, &o.sdp).replace(
            "a=rtpmap:111 opus/48000/2",
            &format!("a=rtpmap:111 multiopus/48000/{channels}"),
        );
        let answer = munge_answer(&answer, channels, &multiopus_fmtp(&chrome_layout(channels)));
        let (text, info) = check_answer(&answer, &s).unwrap_or_else(|e| panic!("{e}\n{answer}"));
        // What str0m parses is plain stereo Opus again, with nothing of the multistream.
        assert!(text.contains("a=rtpmap:111 opus/48000/2"));
        assert!(!text.contains("multiopus") && !text.contains("channel_mapping"));
        assert!(
            text.contains(&format!("a=fmtp:111 {STEREO_FMTP}")),
            "{text}"
        );
        // The layout the client chose is the one the encoder gets.
        let layout = chrome_layout(channels);
        assert_eq!(info.audio_layout, Some(layout));
        // And str0m accepts it.
        accept_answer(&mut rtc, o, &answer, &s).unwrap();
    }
}

/// A Chromium-style answer to the surround offer with these multiopus parameters.
fn chrome_answer(s: &OfferSpec, rtpmap_channels: usize, fmtp: &str) -> String {
    munge_answer(&answer_to(s, &offer_for(s)), rtpmap_channels, fmtp)
}

const CHROME_51: &str =
    "channel_mapping=0,4,1,2,3,5;num_streams=4;coupled_streams=2;minptime=10;useinbandfec=1";
const CHROME_71: &str =
    "channel_mapping=0,6,1,2,3,4,5,7;num_streams=5;coupled_streams=3;minptime=10;useinbandfec=1";

#[test]
fn a_stereo_answer_to_a_surround_request_is_refused_never_downmixed() {
    for channels in [6, 8] {
        let s = surround_spec(channels);
        let stereo = answer_to(&s, &offer_for(&s));
        let err = check_answer(&stereo, &s).unwrap_err().to_string();
        assert!(
            err.contains(&format!(
                "{channels}-channel request was answered with stereo"
            )) && err.contains("never falls back to stereo"),
            "{err}"
        );
    }
    // A multiopus answer of the wrong width is refused too.
    let s = surround_spec(6);
    let wrong_count = chrome_answer(&s, 8, CHROME_71);
    let err = check_answer(&wrong_count, &s).unwrap_err().to_string();
    assert!(err.contains("not multiopus/48000/6"), "{err}");
}

#[test]
fn a_multiopus_answer_to_a_stereo_request_is_refused() {
    let s = spec(VideoCodec::H264, false);
    let answer = chrome_answer(&s, 6, CHROME_51);
    let err = check_answer(&answer, &s).unwrap_err().to_string();
    assert!(err.contains("changed the audio codec"), "{err}");
}

#[test]
fn unknown_and_extra_fmtp_keys_in_the_answer_are_ignored() {
    let s = surround_spec(6);
    let fmtp = format!("{CHROME_51};cbr=1;x-google-start-bitrate=64;stereo=1;sprop-stereo=1");
    let (_, info) = check_answer(&chrome_answer(&s, 6, &fmtp), &s).unwrap();
    assert_eq!(info.audio_layout, Some(chrome_layout(6)));
    // The same for a stereo answer: its fmtp is not read at all.
    let s = spec(VideoCodec::H264, false);
    let stereo = answer_to(&s, &offer_for(&s)).replace(
        &format!("a=fmtp:111 {STEREO_FMTP}"),
        "a=fmtp:111 minptime=10;useinbandfec=1;surprise=yes",
    );
    assert!(check_answer(&stereo, &s).is_ok());
}

#[test]
fn chrome_shaped_answers_are_accepted_and_their_layout_returned() {
    for (channels, fmtp, streams, coupled, mapping) in [
        (6, CHROME_51, 4, 2, vec![0u8, 4, 1, 2, 3, 5]),
        (8, CHROME_71, 5, 3, vec![0u8, 6, 1, 2, 3, 4, 5, 7]),
    ] {
        let s = surround_spec(channels);
        let answer = chrome_answer(&s, channels, fmtp);
        let (text, info) = check_answer(&answer, &s).unwrap();
        assert!(text.contains("a=rtpmap:111 opus/48000/2") && !text.contains("multiopus"));
        let layout = info.audio_layout.expect("a negotiated layout");
        assert_eq!(
            (
                layout.channels,
                layout.streams,
                layout.coupled,
                &layout.mapping
            ),
            (channels, streams, coupled, &mapping)
        );
        // The same layout core would build from its custom string.
        let digits: String = mapping.iter().map(u8::to_string).collect();
        let custom = format!("{channels}{streams}{coupled}{digits}");
        assert_eq!(
            layout,
            OpusLayout::select(channels, false, Some(&custom)).unwrap()
        );
    }
}

#[test]
fn an_answer_need_not_echo_the_offered_mapping() {
    // The offer carries Chromium's table; a receiver that answers with core's Moonlight
    // mapping, or one with a silent channel (255), is taken at its word.
    let s = surround_spec(6);
    for (fmtp, mapping) in [
        (
            "channel_mapping=0,1,4,5,2,3;num_streams=4;coupled_streams=2",
            vec![0u8, 1, 4, 5, 2, 3],
        ),
        (
            "channel_mapping=0,1,2,3,4,255;num_streams=3;coupled_streams=2;minptime=10",
            vec![0, 1, 2, 3, 4, 255],
        ),
        (
            "channel_mapping=0,1,2,3,4,5;num_streams=6;coupled_streams=0",
            vec![0, 1, 2, 3, 4, 5],
        ),
    ] {
        let (_, info) = check_answer(&chrome_answer(&s, 6, fmtp), &s).unwrap();
        assert_eq!(info.audio_layout.unwrap().mapping, mapping, "{fmtp}");
    }
}

#[test]
fn invalid_multiopus_parameters_are_refused_clearly() {
    let s = surround_spec(6);
    let refused = |fmtp: &str| {
        check_answer(&chrome_answer(&s, 6, fmtp), &s)
            .unwrap_err()
            .to_string()
    };
    // Mapping length is not the channel count.
    let err = refused("channel_mapping=0,1,2,3,4;num_streams=4;coupled_streams=2");
    assert!(err.contains("5 entries for 6 channels"), "{err}");
    // An entry beyond the decoded channels.
    let err = refused("channel_mapping=0,1,2,3,4,6;num_streams=4;coupled_streams=2");
    assert!(err.contains("outside the 6 decoded channels"), "{err}");
    // More coupled streams than streams, and no streams at all.
    let err = refused("channel_mapping=0,1,2,3,4,5;num_streams=2;coupled_streams=3");
    assert!(err.contains("stream counts are invalid"), "{err}");
    let err = refused("channel_mapping=0,1,2,3,4,5;num_streams=0;coupled_streams=0");
    assert!(err.contains("stream counts are invalid"), "{err}");
    // Too many decoded channels.
    let err = refused("channel_mapping=0,1,2,3,4,5;num_streams=200;coupled_streams=100");
    assert!(err.contains("too many decoded channels"), "{err}");
    // Layouts libopus's encoder cannot be created with: a stream with no channel, or a
    // coupled stream missing one of its two.
    let err = refused("channel_mapping=0,0,0,0,0,0;num_streams=4;coupled_streams=2");
    assert!(err.contains("never uses decoded channel 1"), "{err}");
    let err = refused("channel_mapping=0,1,2,3,4,255;num_streams=4;coupled_streams=2");
    assert!(err.contains("never uses decoded channel 5"), "{err}");
    let err = refused("channel_mapping=0,1,2,2,4,5;num_streams=4;coupled_streams=2");
    assert!(err.contains("never uses decoded channel 3"), "{err}");
    // Missing or unreadable parameters.
    let err = refused("minptime=10");
    assert!(err.contains("no num_streams"), "{err}");
    let err = refused("num_streams=4;coupled_streams=2");
    assert!(err.contains("no channel_mapping"), "{err}");
    let err = refused("channel_mapping=0,1,x,3,4,5;num_streams=4;coupled_streams=2");
    assert!(err.contains("not a number"), "{err}");
    let err = refused("channel_mapping=0,1,2,3,4,5;num_streams=lots;coupled_streams=2");
    assert!(err.contains("num_streams is not a number"), "{err}");
}

#[test]
fn a_non_canonical_but_valid_mapping_is_kept_and_a_duplicate_one_libopus_takes_too() {
    // Any permutation is the receiver's to choose; only what libopus refuses is refused.
    let s = surround_spec(6);
    let (_, info) = check_answer(
        &chrome_answer(
            &s,
            6,
            "channel_mapping=5,3,1,0,2,4;num_streams=4;coupled_streams=2",
        ),
        &s,
    )
    .unwrap();
    assert_eq!(info.audio_layout.unwrap().mapping, [5, 3, 1, 0, 2, 4]);
    // Three streams, two coupled: five decoded channels, and input channel 5 duplicates
    // decoded channel 4. libopus encodes from the first and ignores the second.
    let (_, info) = check_answer(
        &chrome_answer(
            &s,
            6,
            "channel_mapping=0,1,2,3,4,4;num_streams=3;coupled_streams=2",
        ),
        &s,
    )
    .unwrap();
    assert_eq!(info.audio_layout.unwrap().mapping, [0, 1, 2, 3, 4, 4]);
    // The boundary: decoded channel 5 does not exist with five decoded channels.
    let refused = check_answer(
        &chrome_answer(
            &s,
            6,
            "channel_mapping=0,1,2,3,4,5;num_streams=3;coupled_streams=2",
        ),
        &s,
    )
    .unwrap_err()
    .to_string();
    assert!(
        refused.contains("outside the 5 decoded channels"),
        "{refused}"
    );
}

/// libopus's `validate_layout` and `validate_encoder_layout`, as the multistream encoder
/// applies them when it is created with these parameters.
fn libopus_creates_an_encoder(streams: i32, coupled: i32, mapping: &[u8]) -> bool {
    if coupled > streams || streams < 1 || coupled < 0 || streams > 255 - coupled {
        return false;
    }
    let decoded = streams + coupled;
    if mapping.iter().any(|&m| i32::from(m) >= decoded && m != 255) {
        return false;
    }
    let mapped = |channel: i32| mapping.iter().any(|&m| i32::from(m) == channel);
    (0..streams).all(|stream| {
        if stream < coupled {
            mapped(2 * stream) && mapped(2 * stream + 1)
        } else {
            mapped(stream + coupled)
        }
    })
}

#[test]
fn an_answered_layout_is_accepted_exactly_when_libopus_can_encode_it() {
    let mut checked = 0;
    // Stereo in full, and the surround layouts that matter: Chromium's, Moonlight's and
    // the in-between ones around them.
    let mut cases: Vec<(usize, i32, i32)> = (0..=3)
        .flat_map(|streams| (0..=4).map(move |coupled| (2, streams, coupled)))
        .collect();
    cases.extend([(6, 4, 2), (6, 3, 2), (6, 5, 1), (6, 2, 2)]);
    for (channels, streams, coupled) in cases {
        // Every mapping over the ids that exist, one beyond them, and silence.
        let ids: Vec<u8> = (0..=(streams + coupled) as u8).chain([255]).collect();
        let mut mapping = vec![0usize; channels];
        loop {
            let entries: Vec<u8> = mapping.iter().map(|&i| ids[i]).collect();
            let list: Vec<String> = entries.iter().map(u8::to_string).collect();
            let fmtp = format!(
                "a=fmtp:111 channel_mapping={};num_streams={streams};coupled_streams={coupled}",
                list.join(",")
            );
            assert_eq!(
                answered_layout(&fmtp, channels).is_ok(),
                libopus_creates_an_encoder(streams, coupled, &entries),
                "{channels} channels: {fmtp}"
            );
            checked += 1;
            // The next mapping, odometer style.
            let Some(at) = mapping.iter().position(|&i| i + 1 < ids.len()) else {
                break;
            };
            mapping[at] += 1;
            mapping[..at].fill(0);
        }
    }
    assert!(checked > 100_000, "{checked}");
}

#[test]
fn garbage_answers_are_errors_not_panics() {
    let s = spec(VideoCodec::H264, false);
    let good = answer_to(&s, &offer_for(&s));
    let mut cases: Vec<String> = vec![
        String::new(),
        "\r\n\r\n".into(),
        "not sdp at all".into(),
        "v=0\r\n".into(),
        "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n".into(),
        "m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 H264/90000\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n".into(),
        "\u{0}\u{1}\u{2}\u{ff}".into(),
        "a=rtpmap:96".into(),
        "a=fmtp:96\r\na=fmtp:111 ;;;===;\r\n".into(),
    ];
    // Every prefix of a real answer, cut at line boundaries and mid-line.
    for end in (0..good.len()).step_by(37) {
        if good.is_char_boundary(end) {
            cases.push(good[..end].to_string());
        }
    }
    for case in &cases {
        // Whatever the verdict, it is an Ok or an Err, never a panic.
        let _ = check_answer(case, &s);
        let (mut rtc, offer) = host_offer(&s, HOST.parse().unwrap());
        let verdict = accept_answer(&mut rtc, offer, case, &s);
        if !case.contains("m=application") {
            assert!(verdict.is_err(), "accepted {case:?}");
        }
    }
    for (case, want) in [
        ("", "no video section"),
        ("v=0\r\n", "no video section"),
        (
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "changed the video codec",
        ),
        (
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 H264/90000\r\n",
            "no audio section",
        ),
    ] {
        let err = check_answer(case, &s).unwrap_err().to_string();
        assert!(err.contains(want), "{case:?}: {err}");
    }
    // Valid to the checks but not to str0m.
    let broken = good.replace("v=0", "v=banana");
    let (mut rtc, offer) = host_offer(&s, HOST.parse().unwrap());
    if let Err(e) = accept_answer(&mut rtc, offer, &broken, &s) {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn only_stereo_5_1_and_7_1_can_be_requested() {
    for channels in [2, 6, 8] {
        assert_eq!(
            AudioMode::request(channels).unwrap().channels as usize,
            channels
        );
    }
    for channels in [0, 1, 3, 4, 5, 7, 9, 255] {
        assert!(AudioMode::request(channels).is_err(), "{channels}");
    }
}
