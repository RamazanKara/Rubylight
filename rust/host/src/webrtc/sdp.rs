//! Offer construction and SDP text handling for the WebRTC transport.
//!
//! The host sends the offer (protocol v2): one video codec at a fixed payload type, Opus at a fixed
//! payload type, a `control` and a `gamepad` DataChannel. str0m builds the offer; the text is then
//! edited for what str0m cannot express (Samsung's `imageattr`, `multiopus`), and the answer is
//! checked before str0m parses it. See `docs/webrtc-design.md` ("Library").
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_core::audio::OpusLayout;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::format::{Codec, FormatParams};
use str0m::media::{Direction, Frequency, MediaKind, Mid, Pt};
use str0m::rtp::{Extension, ExtensionMap};
use str0m::{Rtc, RtcConfig};

pub const VIDEO_PT: u8 = 96;
pub const VIDEO_RTX_PT: u8 = 97;
pub const AUDIO_PT: u8 = 111;
pub const MID_EXT_ID: u8 = 4;
pub const PLAYOUT_DELAY_EXT_ID: u8 = 6;
pub const COLOR_SPACE_EXT_ID: u8 = 9;
pub const PLAYOUT_DELAY_URI: &str = "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";
pub const COLOR_SPACE_URI: &str = "http://www.webrtc.org/experiments/rtp-hdrext/color-space";
pub const CONTROL_LABEL: &str = "control";
pub const GAMEPAD_LABEL: &str = "gamepad";
pub const INPUT_LABEL: &str = "input";
/// The protocol's stereo fmtp. str0m has no `maxaveragebitrate`, so the line is written as text.
const STEREO_FMTP: &str =
    "minptime=20;maxaveragebitrate=128000;stereo=1;sprop-stereo=1;useinbandfec=0";
/// What libdatachannel offers by default, which the Tizen app and browsers accept for the High
/// profile stream AMF writes (receivers decode from the SPS and treat the level as asymmetric).
const H264_PROFILE_LEVEL_ID: u32 = 0x42e01f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    fn rtpmap_name(self) -> &'static str {
        match self {
            Self::H264 => "H264/90000",
            Self::Hevc => "H265/90000",
            Self::Av1 => "AV1/90000",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub hdr: bool,
}

impl VideoMode {
    fn hevc_profile_id(&self) -> u8 {
        if self.hdr { 2 } else { 1 }
    }
}

/// Stereo, or 5.1/7.1 Opus multistream (a Rubylight extension, sent as `multiopus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioMode {
    pub channels: u8,
    layout: Option<OpusLayout>,
}

impl AudioMode {
    pub fn stereo() -> Self {
        Self {
            channels: 2,
            layout: None,
        }
    }

    /// `layout` is the one the encoder uses (`OpusLayout::select`), so the mapping in the fmtp is
    /// the mapping of the packets.
    pub fn surround(layout: OpusLayout) -> Result<Self> {
        if !matches!(layout.channels, 6 | 8) || layout.mapping.len() != layout.channels {
            bail!(
                "multiopus needs a 6 or 8 channel layout, got {}",
                layout.channels
            );
        }
        Ok(Self {
            channels: layout.channels as u8,
            layout: Some(layout),
        })
    }

    pub fn layout(&self) -> Option<&OpusLayout> {
        self.layout.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct OfferSpec {
    pub video: VideoMode,
    pub audio: AudioMode,
    /// RTX and NACK feedback for video. Off by default: str0m answers NACK only through RTX.
    pub rtx: bool,
}

/// The media and channels the offer created, for the media thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionIds {
    pub video_mid: Mid,
    pub audio_mid: Mid,
    pub control: ChannelId,
    pub gamepad: ChannelId,
}

/// An offer in flight: the text for the client plus what the media thread needs afterwards.
pub struct Offer {
    pub sdp: String,
    pub pending: SdpPendingOffer,
    pub ids: SessionIds,
}

/// What the client's answer agreed to, for the log and for sending decisions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnswerInfo {
    pub playout_delay: bool,
    pub video_level: Option<u32>,
}

// TODO(B4): use core's level helper (rust/core/src/webrtc/capabilities.rs) once B1a has landed.
/// The lowest Main-tier HEVC level (as `level-id`, 30 times the level) whose picture size and luma
/// sample rate hold the mode (H.265 table A.8).
pub fn hevc_level_id(width: u32, height: u32, fps: u32) -> u8 {
    // (level-id, MaxLumaPs, MaxLumaSr)
    const LEVELS: [(u8, u64, u64); 13] = [
        (30, 36_864, 552_960),
        (60, 122_880, 3_686_400),
        (63, 245_760, 7_372_800),
        (90, 552_960, 16_588_800),
        (93, 983_040, 33_177_600),
        (120, 2_228_224, 66_846_720),
        (123, 2_228_224, 133_693_440),
        (150, 8_912_896, 267_386_880),
        (153, 8_912_896, 534_773_760),
        (156, 8_912_896, 1_069_547_520),
        (180, 35_651_584, 1_069_547_520),
        (183, 35_651_584, 2_139_095_040),
        (186, 35_651_584, 4_278_190_080),
    ];
    let size = u64::from(width) * u64::from(height);
    let rate = size * u64::from(fps);
    let max_dimension = |max_ps: u64| ((max_ps * 8) as f64).sqrt() as u64;
    LEVELS
        .iter()
        .find(|(_, ps, sr)| {
            size <= *ps
                && rate <= *sr
                && u64::from(width) <= max_dimension(*ps)
                && u64::from(height) <= max_dimension(*ps)
        })
        .map_or(LEVELS[LEVELS.len() - 1].0, |l| l.0)
}

// TODO(B4): use core's level helper (rust/core/src/webrtc/capabilities.rs) once B1a has landed.
/// The lowest AV1 `level-idx` (`seq_level_idx`, Tier 0) whose limits hold the mode (AV1 Annex A).
pub fn av1_level_idx(width: u32, height: u32, fps: u32) -> u8 {
    // (level-idx, MaxPicSize, MaxHSize, MaxVSize, MaxDisplayRate)
    const LEVELS: [(u8, u64, u64, u64, u64); 14] = [
        (0, 147_456, 2048, 1152, 4_423_680),
        (1, 278_784, 2816, 1584, 8_363_520),
        (4, 665_856, 4352, 2448, 19_975_680),
        (5, 1_065_024, 5504, 3096, 31_950_720),
        (8, 2_359_296, 6144, 3456, 70_778_880),
        (9, 2_359_296, 6144, 3456, 141_557_760),
        (12, 8_912_896, 8192, 4352, 267_386_880),
        (13, 8_912_896, 8192, 4352, 534_773_760),
        (14, 8_912_896, 8192, 4352, 1_069_547_520),
        (16, 35_651_584, 16_384, 8704, 1_069_547_520),
        (17, 35_651_584, 16_384, 8704, 2_139_095_040),
        (18, 35_651_584, 16_384, 8704, 4_278_190_080),
        (19, 35_651_584, 16_384, 8704, 4_278_190_080),
        (31, u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ];
    let (w, h) = (u64::from(width), u64::from(height));
    let rate = w * h * u64::from(fps);
    LEVELS
        .iter()
        .find(|(_, ps, hs, vs, dr)| w * h <= *ps && w <= *hs && h <= *vs && rate <= *dr)
        .map_or(LEVELS[LEVELS.len() - 2].0, |l| l.0)
}

fn video_codec_and_format(mode: &VideoMode) -> (Codec, FormatParams) {
    match mode.codec {
        VideoCodec::H264 => (
            Codec::H264,
            FormatParams {
                level_asymmetry_allowed: Some(true),
                packetization_mode: Some(1),
                profile_level_id: Some(H264_PROFILE_LEVEL_ID),
                ..Default::default()
            },
        ),
        VideoCodec::Hevc => (
            Codec::H265,
            FormatParams {
                h265_profile_tier_level: Some(
                    (
                        mode.hevc_profile_id(),
                        0,
                        hevc_level_id(mode.width, mode.height, mode.fps),
                    )
                        .into(),
                ),
                ..Default::default()
            },
        ),
        VideoCodec::Av1 => (
            Codec::Av1,
            FormatParams {
                profile: Some(0),
                tier: Some(0),
                level_idx: Some(av1_level_idx(mode.width, mode.height, mode.fps)),
                ..Default::default()
            },
        ),
    }
}

/// The Rtc configuration both for the host and, with `set_ice_lite(false)`, for a test client: one
/// video codec and Opus at fixed payload types, and no transport-cc, REMB or (unless asked) RTX.
pub fn rtc_config(spec: &OfferSpec) -> RtcConfig {
    let mut exts = ExtensionMap::empty();
    exts.set(MID_EXT_ID, Extension::RtpMid);
    exts.set(PLAYOUT_DELAY_EXT_ID, Extension::PlayoutDelay);
    if spec.video.hdr {
        // Offered as the protocol requires, never sent.
        exts.set(COLOR_SPACE_EXT_ID, Extension::ColorSpace);
    }
    let mut config = Rtc::builder()
        .set_ice_lite(true)
        .clear_codecs()
        .set_extension_map(exts)
        // A 4K keyframe is a few thousand packets; the default keeps 1000.
        .set_send_buffer_video(4096);
    let (codec, format) = video_codec_and_format(&spec.video);
    let codecs = config.codec_config();
    codecs.add_config(
        Pt::new_with_value(VIDEO_PT),
        spec.rtx.then_some(Pt::new_with_value(VIDEO_RTX_PT)),
        codec,
        Frequency::NINETY_KHZ,
        None,
        format,
    );
    codecs.add_config(
        Pt::new_with_value(AUDIO_PT),
        None,
        Codec::Opus,
        Frequency::FORTY_EIGHT_KHZ,
        Some(2),
        FormatParams {
            min_p_time: Some(if spec.audio.layout.is_some() { 10 } else { 20 }),
            stereo: spec.audio.layout.is_none().then_some(true),
            sprop_stereo: spec.audio.layout.is_none().then_some(true),
            use_inband_fec: Some(false),
            ..Default::default()
        },
    );
    for params in codecs.iter_mut() {
        params.set_fb_transport_cc(false);
        params.set_fb_remb(false);
        if params.spec().codec.is_video() {
            params.set_fb_pli(true);
            params.set_fb_fir(true);
            params.set_fb_nack(spec.rtx);
        }
    }
    config
}

/// Add the media and the two host-created DataChannels to a fresh Rtc and return the edited offer.
pub fn create_offer(rtc: &mut Rtc, spec: &OfferSpec) -> Result<Offer> {
    let mut api = rtc.sdp_api();
    // Separate msids, so a browser doesn't hold video back for lip sync with the audio.
    let video_mid = api.add_media(
        MediaKind::Video,
        Direction::SendOnly,
        Some("rubylight-video".into()),
        Some("video".into()),
        None,
    );
    let audio_mid = api.add_media(
        MediaKind::Audio,
        Direction::SendOnly,
        Some("rubylight-audio".into()),
        Some("audio".into()),
        None,
    );
    let control = api.add_channel_with_config(ChannelConfig {
        label: CONTROL_LABEL.into(),
        ordered: true,
        reliability: Reliability::Reliable,
        ..Default::default()
    });
    let gamepad = api.add_channel_with_config(ChannelConfig {
        label: GAMEPAD_LABEL.into(),
        ordered: false,
        reliability: Reliability::MaxRetransmits { retransmits: 0 },
        ..Default::default()
    });
    let (offer, pending) = api
        .apply()
        .ok_or_else(|| anyhow!("the offer has no changes"))?;
    let sdp = edit_offer(&offer.to_sdp_string(), spec)?;
    let ids = SessionIds {
        video_mid,
        audio_mid,
        control,
        gamepad,
    };
    Ok(Offer { sdp, pending, ids })
}

/// Samsung Game Mode wants exactly one of these in the video section.
pub fn imageattr_line(mode: &VideoMode) -> String {
    format!(
        "a=imageattr:{VIDEO_PT} send [x=[{w}:{w}],y=[{h}:{h}],fps=[{f}:{f}]]",
        w = mode.width,
        h = mode.height,
        f = mode.fps
    )
}

fn lines(sdp: &str) -> Vec<String> {
    sdp.lines().map(str::to_owned).collect()
}

fn join(lines: &[String]) -> String {
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

/// Line range of the first media section of `kind` (`video`, `audio`, `application`).
fn section(lines: &[String], kind: &str) -> Option<std::ops::Range<usize>> {
    let prefix = format!("m={kind} ");
    let start = lines.iter().position(|l| l.starts_with(&prefix))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("m="))
        .map_or(lines.len(), |i| start + 1 + i);
    Some(start..end)
}

fn attribute_line(lines: &[String], range: &std::ops::Range<usize>, prefix: &str) -> Option<usize> {
    range.clone().find(|&i| lines[i].starts_with(prefix))
}

fn fmtp_params(line: &str) -> Vec<(&str, &str)> {
    let value = line.split_once(' ').map_or("", |(_, v)| v);
    value
        .split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

/// `a=imageattr`, the stereo fmtp or the multiopus lines, applied to str0m's offer text.
fn edit_offer(sdp: &str, spec: &OfferSpec) -> Result<String> {
    let mut lines = lines(sdp);
    let audio = section(&lines, "audio").context("the offer has no audio section")?;
    let audio_pt = AUDIO_PT;
    let rtpmap = attribute_line(&lines, &audio, &format!("a=rtpmap:{audio_pt} opus/48000/2"))
        .context("the offer has no Opus rtpmap")?;
    let fmtp = attribute_line(&lines, &audio, &format!("a=fmtp:{audio_pt} "));
    let fmtp_text = match spec.audio.layout() {
        Some(layout) => {
            lines[rtpmap] = format!("a=rtpmap:{audio_pt} multiopus/48000/{}", layout.channels);
            let mapping: Vec<String> = layout.mapping.iter().map(u8::to_string).collect();
            format!(
                "a=fmtp:{audio_pt} channel_mapping={};num_streams={};coupled_streams={};minptime=10;useinbandfec=0",
                mapping.join(","),
                layout.streams,
                layout.coupled
            )
        }
        None => format!("a=fmtp:{audio_pt} {STEREO_FMTP}"),
    };
    match fmtp {
        Some(i) => lines[i] = fmtp_text,
        None => lines.insert(rtpmap + 1, fmtp_text),
    }
    let video = section(&lines, "video").context("the offer has no video section")?;
    if attribute_line(&lines, &video, "a=imageattr:").is_some() {
        bail!("the offer already has an imageattr line");
    }
    lines.insert(video.end, imageattr_line(&spec.video));
    if !spec.rtx {
        strip_rtx_ssrcs(&mut lines);
    }
    Ok(join(&lines))
}

/// str0m always reserves an RTX SSRC for video and advertises it in a FID group, even with no RTX
/// payload type. A receiver would wait for retransmissions that never come, so drop it.
fn strip_rtx_ssrcs(lines: &mut Vec<String>) {
    let rtx: Vec<String> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("a=ssrc-group:FID "))
        .filter_map(|v| v.split(' ').nth(1).map(str::to_owned))
        .collect();
    lines.retain(|l| {
        !l.starts_with("a=ssrc-group:FID ")
            && !rtx.iter().any(|s| l.starts_with(&format!("a=ssrc:{s} ")))
    });
}

/// Check the client's answer against the offer and return the text str0m should parse: the
/// `multiopus` answer goes back to `opus/48000/2`. A changed codec or profile, or a dropped
/// `multiopus` or DataChannel, is an error: the host never falls back.
pub fn check_answer(sdp: &str, spec: &OfferSpec) -> Result<(String, AnswerInfo)> {
    let mut lines = lines(sdp);
    let mut info = AnswerInfo::default();

    let video = section(&lines, "video").context("the answer has no video section")?;
    if lines[video.start].starts_with("m=video 0 ") {
        bail!("the answer rejected the video");
    }
    let expected = format!("a=rtpmap:{VIDEO_PT} {}", spec.video.codec.rtpmap_name());
    let rtpmaps: Vec<&str> = lines[video.clone()]
        .iter()
        .filter(|l| l.starts_with("a=rtpmap:"))
        .map(String::as_str)
        .collect();
    if !rtpmaps.iter().any(|l| l.eq_ignore_ascii_case(&expected)) {
        bail!(
            "the answer changed the video codec (wanted {}, got {})",
            spec.video.codec.rtpmap_name(),
            rtpmaps.join(", ")
        );
    }
    if rtpmaps
        .iter()
        .any(|l| !l.eq_ignore_ascii_case(&expected) && !is_rtx_rtpmap(l, spec.rtx))
    {
        bail!("the answer added a video codec: {}", rtpmaps.join(", "));
    }
    let fmtp = attribute_line(&lines, &video, &format!("a=fmtp:{VIDEO_PT} "))
        .map(|i| lines[i].clone())
        .unwrap_or_default();
    let params = fmtp_params(&fmtp);
    let param = |name: &str| params.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
    match spec.video.codec {
        VideoCodec::H264 => {
            let want = format!("{:02x}", (H264_PROFILE_LEVEL_ID >> 16) & 0xff);
            let got = param("profile-level-id").unwrap_or("42e01f");
            if !got.get(..2).is_some_and(|p| p.eq_ignore_ascii_case(&want)) {
                bail!("the answer changed the H.264 profile (wanted {want}xxxx, got {got})");
            }
        }
        VideoCodec::Hevc => {
            let want = spec.video.hevc_profile_id().to_string();
            if param("profile-id").unwrap_or("1") != want {
                bail!("the answer changed the HEVC profile-id (wanted {want})");
            }
            if param("tier-flag").unwrap_or("0") != "0" {
                bail!("the answer changed the HEVC tier");
            }
            info.video_level = param("level-id").and_then(|v| v.parse().ok());
        }
        VideoCodec::Av1 => {
            if param("profile").unwrap_or("0") != "0" {
                bail!("the answer changed the AV1 profile");
            }
            if param("tier").unwrap_or("0") != "0" {
                bail!("the answer changed the AV1 tier");
            }
            info.video_level = param("level-idx").and_then(|v| v.parse().ok());
        }
    }
    info.playout_delay = lines[video.clone()]
        .iter()
        .any(|l| l.starts_with("a=extmap:") && l.contains(PLAYOUT_DELAY_URI));

    let audio = section(&lines, "audio").context("the answer has no audio section")?;
    if lines[audio.start].starts_with("m=audio 0 ") {
        bail!("the answer rejected the audio");
    }
    let rtpmap = attribute_line(&lines, &audio, &format!("a=rtpmap:{AUDIO_PT} "))
        .map(|i| (i, lines[i].clone()));
    match (spec.audio.layout(), rtpmap) {
        (None, Some((_, line)))
            if line.eq_ignore_ascii_case(&format!("a=rtpmap:{AUDIO_PT} opus/48000/2")) => {}
        (None, _) => bail!("the answer changed the audio codec (wanted opus/48000/2)"),
        (Some(layout), Some((i, line)))
            if line.eq_ignore_ascii_case(&format!(
                "a=rtpmap:{AUDIO_PT} multiopus/48000/{}",
                layout.channels
            )) =>
        {
            let fmtp = attribute_line(&lines, &audio, &format!("a=fmtp:{AUDIO_PT} "))
                .map(|i| lines[i].clone())
                .unwrap_or_default();
            let params = fmtp_params(&fmtp);
            let param = |name: &str| params.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
            let mapping: Vec<String> = layout.mapping.iter().map(u8::to_string).collect();
            if param("channel_mapping") != Some(mapping.join(",").as_str()) {
                bail!("the answer changed the multiopus channel mapping");
            }
            if param("num_streams") != Some(layout.streams.to_string().as_str())
                || param("coupled_streams") != Some(layout.coupled.to_string().as_str())
            {
                bail!("the answer changed the multiopus stream counts");
            }
            lines[i] = format!("a=rtpmap:{AUDIO_PT} opus/48000/2");
            if let Some(f) = attribute_line(&lines, &audio, &format!("a=fmtp:{AUDIO_PT} ")) {
                lines[f] = format!("a=fmtp:{AUDIO_PT} minptime=10;useinbandfec=0");
            }
        }
        (Some(layout), _) => bail!(
            "the answer dropped multiopus/48000/{} (only Chromium-based receivers decode it)",
            layout.channels
        ),
    }

    let application =
        section(&lines, "application").context("the answer has no DataChannel section")?;
    if lines[application.start].starts_with("m=application 0 ") {
        bail!("the answer rejected the DataChannels");
    }
    Ok((join(&lines), info))
}

fn is_rtx_rtpmap(line: &str, rtx: bool) -> bool {
    rtx && line.starts_with(&format!("a=rtpmap:{VIDEO_RTX_PT} "))
        && line.to_ascii_lowercase().contains("rtx/90000")
}

/// Parse a (checked) answer text and apply it.
pub fn accept_answer(
    rtc: &mut Rtc,
    offer: Offer,
    answer: &str,
    spec: &OfferSpec,
) -> Result<(SessionIds, AnswerInfo)> {
    let (text, info) = check_answer(answer, spec)?;
    let ids = offer.ids;
    let answer =
        SdpAnswer::from_sdp_string(&text).map_err(|e| anyhow!("unparsable answer: {e}"))?;
    rtc.sdp_api()
        .accept_answer(offer.pending, answer)
        .map_err(|e| anyhow!("answer rejected: {e}"))?;
    Ok((ids, info))
}

#[cfg(test)]
pub(crate) mod testing {
    //! A plain str0m peer that answers the offers.
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Once;
    use std::time::Instant;
    use str0m::Candidate;
    use str0m::change::SdpOffer;

    pub fn install_crypto() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| str0m::crypto::from_feature_flags().install_process_default());
    }

    pub fn spec(codec: VideoCodec, hdr: bool) -> OfferSpec {
        OfferSpec {
            video: VideoMode {
                codec,
                width: 1920,
                height: 1080,
                fps: 60,
                hdr,
            },
            audio: AudioMode::stereo(),
            rtx: false,
        }
    }

    pub fn surround_spec(channels: usize) -> OfferSpec {
        OfferSpec {
            audio: AudioMode::surround(OpusLayout::select(channels, false, None).unwrap()).unwrap(),
            ..spec(VideoCodec::H264, false)
        }
    }

    /// The host's Rtc (ICE-lite) and the offer text for it.
    pub fn host_offer(spec: &OfferSpec, addr: SocketAddr) -> (Rtc, Offer) {
        install_crypto();
        let mut rtc = rtc_config(spec).build(Instant::now());
        rtc.add_local_candidate(Candidate::host(addr, "udp").unwrap());
        let offer = create_offer(&mut rtc, spec).unwrap();
        (rtc, offer)
    }

    /// A browser-like client: full ICE, the same codecs, raw packets on so tests can read headers.
    pub fn client(spec: &OfferSpec, addr: SocketAddr) -> Rtc {
        install_crypto();
        let mut rtc = rtc_config(spec)
            .set_ice_lite(false)
            .enable_raw_packets(true)
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(addr, "udp").unwrap());
        rtc
    }

    pub fn answer(client: &mut Rtc, offer_sdp: &str) -> String {
        let offer = SdpOffer::from_sdp_string(offer_sdp).expect("a parsable offer");
        client
            .sdp_api()
            .accept_offer(offer)
            .expect("an answer")
            .to_sdp_string()
    }
}

#[cfg(test)]
mod tests {
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
        assert!(video.contains(
            &"a=imageattr:96 send [x=[1920:1920],y=[1080:1080],fps=[60:60]]".to_string()
        ));
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
            offer_for(&s)
                .contains("a=imageattr:96 send [x=[3840:3840],y=[2160:2160],fps=[120:120]]")
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

    #[test]
    fn hevc_fmtp_levels_for_main_and_main10() {
        let cases = [
            (1280, 720, 60, 120),
            (1920, 1080, 60, 123),
            (1920, 1080, 120, 150),
            (2560, 1440, 60, 150),
            (2560, 1440, 120, 153),
            (3840, 2160, 60, 153),
            (3840, 2160, 120, 156),
        ];
        for (w, h, f, level) in cases {
            assert_eq!(hevc_level_id(w, h, f), level, "{w}x{h}@{f}");
            for (hdr, profile) in [(false, 1), (true, 2)] {
                let mut s = spec(VideoCodec::Hevc, hdr);
                (s.video.width, s.video.height, s.video.fps) = (w, h, f);
                let line = fmtp(&offer_for(&s));
                assert!(
                    line.contains(&format!("profile-id={profile}"))
                        && line.contains("tier-flag=0")
                        && line.contains(&format!("level-id={level}")),
                    "{w}x{h}@{f} hdr={hdr}: {line}"
                );
            }
        }
    }

    #[test]
    fn av1_fmtp_matches_the_mode() {
        for (w, h, f, idx) in [
            (1920, 1080, 60, 9),
            (2560, 1440, 60, 12),
            (3840, 2160, 60, 13),
            (3840, 2160, 120, 14),
        ] {
            assert_eq!(av1_level_idx(w, h, f), idx, "{w}x{h}@{f}");
            let mut s = spec(VideoCodec::Av1, true);
            (s.video.width, s.video.height, s.video.fps) = (w, h, f);
            let line = fmtp(&offer_for(&s));
            assert!(
                line.contains("profile=0")
                    && line.contains("tier=0")
                    && line.contains(&format!("level-idx={idx}")),
                "{line}"
            );
        }
    }

    #[test]
    fn h264_fmtp_is_packetization_mode_one() {
        let line = fmtp(&offer_for(&spec(VideoCodec::H264, false)));
        assert!(
            line.contains("packetization-mode=1") && line.contains("profile-level-id=42e01f"),
            "{line}"
        );
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
    fn multiopus_offer_for_six_and_eight_channels() {
        for (channels, mapping, streams, coupled) in
            [(6, "0,1,4,5,2,3", 4, 2), (8, "0,1,4,5,6,7,2,3", 5, 3)]
        {
            let sdp = offer_for(&surround_spec(channels));
            assert!(
                sdp.contains(&format!("a=rtpmap:111 multiopus/48000/{channels}\r\n")),
                "{sdp}"
            );
            assert!(
                sdp.contains(&format!(
                    "a=fmtp:111 channel_mapping={mapping};num_streams={streams};coupled_streams={coupled};minptime=10;useinbandfec=0\r\n"
                )),
                "{sdp}"
            );
            assert!(!sdp.contains("opus/48000/2"));
        }
    }

    /// The client side of an answer: what a Chromium-like receiver would send back.
    fn answer_to(spec: &OfferSpec, offer: &str) -> String {
        let mut client = client(spec, CLIENT.parse().unwrap());
        // str0m's own client doesn't know multiopus, so give it the stereo text it would map to.
        let offer = offer.replace(
            &format!("multiopus/48000/{}", spec.audio.channels),
            "opus/48000/2",
        );
        answer(&mut client, &offer)
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
        let changed = answer.replace("profile-level-id=42e01f", "profile-level-id=64001f");
        assert!(
            check_answer(&changed, &s)
                .unwrap_err()
                .to_string()
                .contains("H.264 profile")
        );
        // A different level is fine: the level is asymmetric.
        let level = answer.replace("profile-level-id=42e01f", "profile-level-id=42e034");
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
    fn multiopus_answer_is_rewritten_back_to_stereo_opus() {
        for channels in [6, 8] {
            let s = surround_spec(channels);
            let offer = offer_for(&s);
            // A Chromium-like answer echoes the multiopus lines.
            let answer = answer_to(&s, &offer).replace(
                "a=rtpmap:111 opus/48000/2",
                &format!("a=rtpmap:111 multiopus/48000/{channels}"),
            );
            let mapping = s
                .audio
                .layout()
                .unwrap()
                .mapping
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let (streams, coupled) = (
                s.audio.layout().unwrap().streams,
                s.audio.layout().unwrap().coupled,
            );
            let answer = answer.lines().map(|l| {
                if l.starts_with("a=fmtp:111 ") {
                    format!("a=fmtp:111 channel_mapping={mapping};num_streams={streams};coupled_streams={coupled};minptime=10;useinbandfec=0")
                } else {
                    l.to_string()
                }
            }).collect::<Vec<_>>().join("\r\n") + "\r\n";
            let (text, _) = check_answer(&answer, &s).unwrap_or_else(|e| panic!("{e}\n{answer}"));
            assert!(text.contains("a=rtpmap:111 opus/48000/2"));
            assert!(!text.contains("multiopus") && !text.contains("channel_mapping"));
            // str0m accepts the rewritten text.
            let (mut rtc, o) = host_offer(&s, HOST.parse().unwrap());
            assert!(o.sdp.contains("multiopus"));
            let mut a = answer_to(&s, &o.sdp);
            a = a.replace(
                "a=rtpmap:111 opus/48000/2",
                &format!("a=rtpmap:111 multiopus/48000/{channels}"),
            );
            let a = a.lines().map(|l| if l.starts_with("a=fmtp:111 ") { format!("a=fmtp:111 channel_mapping={mapping};num_streams={streams};coupled_streams={coupled};minptime=10;useinbandfec=0") } else { l.to_string() }).collect::<Vec<_>>().join("\r\n") + "\r\n";
            accept_answer(&mut rtc, o, &a, &s).unwrap();
        }
    }

    #[test]
    fn multiopus_answer_validation_rejects_a_dropped_or_changed_layout() {
        let s = surround_spec(6);
        let offer = offer_for(&s);
        let stereo = answer_to(&s, &offer);
        let err = check_answer(&stereo, &s).unwrap_err().to_string();
        assert!(err.contains("dropped multiopus/48000/6"), "{err}");

        let with = |rtpmap: &str, fmtp: &str| {
            stereo
                .replace("a=rtpmap:111 opus/48000/2", rtpmap)
                .lines()
                .map(|l| {
                    if l.starts_with("a=fmtp:111 ") {
                        format!("a=fmtp:111 {fmtp}")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n")
                + "\r\n"
        };
        let good = "channel_mapping=0,1,4,5,2,3;num_streams=4;coupled_streams=2;minptime=10;useinbandfec=0";
        assert!(check_answer(&with("a=rtpmap:111 multiopus/48000/6", good), &s).is_ok());
        let err = |a: String| check_answer(&a, &s).unwrap_err().to_string();
        assert!(
            err(with(
                "a=rtpmap:111 multiopus/48000/6",
                &good.replace("0,1,4,5,2,3", "0,1,2,3,4,5")
            ))
            .contains("channel mapping")
        );
        assert!(
            err(with(
                "a=rtpmap:111 multiopus/48000/6",
                &good.replace("num_streams=4", "num_streams=3")
            ))
            .contains("stream counts")
        );
        assert!(err(with("a=rtpmap:111 multiopus/48000/8", good)).contains("dropped multiopus"));
    }

    #[test]
    fn surround_layouts_are_validated() {
        assert!(AudioMode::surround(OpusLayout::select(2, false, None).unwrap()).is_err());
        assert!(AudioMode::surround(OpusLayout::select(6, true, None).unwrap()).is_ok());
    }
}
