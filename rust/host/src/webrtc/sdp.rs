//! Offer construction and SDP text handling for the WebRTC transport.
//!
//! The host sends the offer (protocol v2): one video codec at a fixed payload type, Opus at a fixed
//! payload type, a `control` and a `gamepad` DataChannel. str0m builds the offer; the text is then
//! edited for what str0m cannot express (Samsung's `imageattr`, `multiopus`), and the answer is
//! checked before str0m parses it. See `docs/webrtc-design.md` ("Library").
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_core::audio::OpusLayout;
use butterpollo_core::webrtc::capabilities::{av1_level, hevc_level};
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
    /// The stream bitrate, which decides the level's tier.
    pub bitrate_kbps: u32,
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

    /// Chromium's canonical 5.1 and 7.1 `multiopus` parameters, which is what a browser answers
    /// with (libwebrtc's own table, so no mapping surprise). The encoder is fed in that channel
    /// order: input channel `i` is the decoder's output channel `i`.
    pub fn chrome_surround(channels: usize) -> Result<Self> {
        let custom = match channels {
            6 => "642041235",
            8 => "85306123457",
            _ => bail!("multiopus needs 6 or 8 channels, got {channels}"),
        };
        Self::surround(OpusLayout::select(channels, false, Some(custom))?)
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
    /// The `multiopus` layout the client answered with, for 6 and 8 channels. The client picks its
    /// own parameters (libwebrtc answers with its canonical table, not necessarily the offered
    /// mapping), and the Opus encoder must be built with exactly these: input channel `i` is the
    /// decoder's output channel `i`. `None` for stereo.
    pub audio_layout: Option<OpusLayout>,
}

/// The HEVC `level-id` and `tier-flag` for the mode, from core's table (never below level 4.1).
fn hevc_params(mode: &VideoMode) -> Result<(u8, u8)> {
    let (level, tier) = hevc_level(mode.width, mode.height, mode.fps, mode.bitrate_kbps)
        .with_context(|| {
            format!(
                "no HEVC level holds {}x{}@{} at {} kbps",
                mode.width, mode.height, mode.fps, mode.bitrate_kbps
            )
        })?;
    Ok((u8::try_from(level.level_id)?, u8::try_from(tier.flag())?))
}

/// The AV1 `level-idx` and `tier` for the mode.
fn av1_params(mode: &VideoMode) -> Result<(u8, u8)> {
    let (level, tier) = av1_level(mode.width, mode.height, mode.fps, mode.bitrate_kbps)
        .with_context(|| {
            format!(
                "no AV1 level holds {}x{}@{} at {} kbps",
                mode.width, mode.height, mode.fps, mode.bitrate_kbps
            )
        })?;
    Ok((u8::try_from(level.level_idx)?, u8::try_from(tier.flag())?))
}

fn video_codec_and_format(mode: &VideoMode) -> Result<(Codec, FormatParams)> {
    Ok(match mode.codec {
        VideoCodec::H264 => (
            Codec::H264,
            FormatParams {
                level_asymmetry_allowed: Some(true),
                packetization_mode: Some(1),
                profile_level_id: Some(H264_PROFILE_LEVEL_ID),
                ..Default::default()
            },
        ),
        VideoCodec::Hevc => (Codec::H265, {
            let (level, tier) = hevc_params(mode)?;
            FormatParams {
                h265_profile_tier_level: Some((mode.hevc_profile_id(), tier, level).into()),
                ..Default::default()
            }
        }),
        VideoCodec::Av1 => (Codec::Av1, {
            let (level, tier) = av1_params(mode)?;
            FormatParams {
                profile: Some(0),
                tier: Some(tier),
                level_idx: Some(level),
                ..Default::default()
            }
        }),
    })
}

/// The Rtc configuration both for the host and, with `set_ice_lite(false)`, for a test client: one
/// video codec and Opus at fixed payload types, and no transport-cc, REMB or (unless asked) RTX.
pub fn rtc_config(spec: &OfferSpec) -> Result<RtcConfig> {
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
    let (codec, format) = video_codec_and_format(&spec.video)?;
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
    Ok(config)
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
            let (_, tier) = hevc_params(&spec.video)?;
            if param("tier-flag").unwrap_or("0") != tier.to_string() {
                bail!("the answer changed the HEVC tier");
            }
            info.video_level = param("level-id").and_then(|v| v.parse().ok());
        }
        VideoCodec::Av1 => {
            if param("profile").unwrap_or("0") != "0" {
                bail!("the answer changed the AV1 profile");
            }
            let (_, tier) = av1_params(&spec.video)?;
            if param("tier").unwrap_or("0") != tier.to_string() {
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
            info.audio_layout = Some(answered_layout(&fmtp, layout)?);
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

/// The layout in a `multiopus` answer's fmtp. The answerer chooses its own parameters, so they
/// need not echo the offer; they only have to describe a valid RFC 7845-style multistream layout
/// for the channel count that was offered.
fn answered_layout(fmtp: &str, offered: &OpusLayout) -> Result<OpusLayout> {
    let params = fmtp_params(fmtp);
    let param = |name: &str| params.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
    let number = |name: &str| -> Result<i32> {
        param(name)
            .with_context(|| format!("the multiopus answer has no {name}"))?
            .parse::<u8>()
            .map(i32::from)
            .map_err(|_| anyhow!("the multiopus answer's {name} is not a number from 0 to 255"))
    };
    let streams = number("num_streams")?;
    let coupled = number("coupled_streams")?;
    if streams < 1 || coupled > streams {
        bail!(
            "the multiopus answer's stream counts are invalid ({streams} streams, {coupled} coupled)"
        );
    }
    let decoded = streams + coupled;
    if decoded > 255 {
        bail!("the multiopus answer has too many decoded channels ({decoded})");
    }
    let mapping = param("channel_mapping")
        .context("the multiopus answer has no channel_mapping")?
        .split(',')
        .map(|v| {
            v.trim()
                .parse::<u8>()
                .map_err(|_| anyhow!("the multiopus channel_mapping entry {v:?} is not a number"))
        })
        .collect::<Result<Vec<u8>>>()?;
    if mapping.len() != offered.channels {
        bail!(
            "the multiopus channel_mapping has {} entries for {} channels",
            mapping.len(),
            offered.channels
        );
    }
    if let Some(bad) = mapping
        .iter()
        .find(|&&m| m != 255 && i32::from(m) >= decoded)
    {
        bail!(
            "the multiopus channel_mapping entry {bad} is outside the {decoded} decoded channels"
        );
    }
    Ok(OpusLayout {
        channels: offered.channels,
        streams,
        coupled,
        mapping,
        bitrate: offered.bitrate,
    })
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
                bitrate_kbps: 20_000,
            },
            audio: AudioMode::stereo(),
            rtx: false,
        }
    }

    pub fn surround_spec(channels: usize) -> OfferSpec {
        OfferSpec {
            audio: AudioMode::chrome_surround(channels).unwrap(),
            ..spec(VideoCodec::H264, false)
        }
    }

    /// The host's Rtc (ICE-lite) and the offer text for it.
    pub fn host_offer(spec: &OfferSpec, addr: SocketAddr) -> (Rtc, Offer) {
        install_crypto();
        let mut rtc = rtc_config(spec).unwrap().build(Instant::now());
        rtc.add_local_candidate(Candidate::host(addr, "udp").unwrap());
        let offer = create_offer(&mut rtc, spec).unwrap();
        (rtc, offer)
    }

    /// A browser-like client: full ICE, the same codecs, raw packets on so tests can read headers.
    pub fn client(spec: &OfferSpec, addr: SocketAddr) -> Rtc {
        install_crypto();
        let mut rtc = rtc_config(spec)
            .unwrap()
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

    fn param_of(line: &str, name: &str) -> Option<String> {
        fmtp_params(line)
            .into_iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    }

    #[test]
    fn hevc_fmtp_levels_and_tier_come_from_core() {
        // (width, height, fps, kbps, level-id, tier-flag); core's table, never below level 4.1.
        let cases = [
            (1280, 720, 60, 10_000, 123, 0),
            (1920, 1080, 60, 10_000, 123, 0),
            (1920, 1080, 60, 20_001, 150, 0),
            (1920, 1080, 120, 10_000, 150, 0),
            (2560, 1440, 60, 10_000, 150, 0),
            (2560, 1440, 120, 10_000, 153, 0),
            (3840, 2160, 60, 10_000, 153, 0),
            (3840, 2160, 120, 10_000, 156, 0),
            (3840, 2160, 120, 100_000, 156, 1),
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
                ("profile-level-id".to_string(), "42e01f".to_string()),
            ],
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
    fn multiopus_offer_for_six_and_eight_channels_is_chromes_canonical_layout() {
        for (channels, mapping, streams, coupled) in
            [(6, "0,4,1,2,3,5", 4, 2), (8, "0,6,1,2,3,4,5,7", 5, 3)]
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

    /// A Chromium-style answer to the surround offer with these multiopus parameters.
    fn chrome_answer(s: &OfferSpec, rtpmap_channels: usize, fmtp: &str) -> String {
        let stereo = answer_to(s, &offer_for(s));
        stereo
            .replace(
                "a=rtpmap:111 opus/48000/2",
                &format!("a=rtpmap:111 multiopus/48000/{rtpmap_channels}"),
            )
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
    }

    const CHROME_51: &str =
        "channel_mapping=0,4,1,2,3,5;num_streams=4;coupled_streams=2;minptime=10;useinbandfec=1";
    const CHROME_71: &str = "channel_mapping=0,6,1,2,3,4,5,7;num_streams=5;coupled_streams=3;minptime=10;useinbandfec=1";

    #[test]
    fn a_dropped_multiopus_is_refused() {
        let s = surround_spec(6);
        let stereo = answer_to(&s, &offer_for(&s));
        let err = check_answer(&stereo, &s).unwrap_err().to_string();
        assert!(err.contains("dropped multiopus/48000/6"), "{err}");
        let wrong_count = chrome_answer(&s, 8, CHROME_71);
        let err = check_answer(&wrong_count, &s).unwrap_err().to_string();
        assert!(err.contains("dropped multiopus/48000/6"), "{err}");
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
                "channel_mapping=0,1,2,3,255,5;num_streams=4;coupled_streams=2;minptime=10",
                vec![0, 1, 2, 3, 255, 5],
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
    fn surround_layouts_are_validated() {
        assert!(AudioMode::surround(OpusLayout::select(2, false, None).unwrap()).is_err());
        assert!(AudioMode::surround(OpusLayout::select(6, true, None).unwrap()).is_ok());
    }
}
