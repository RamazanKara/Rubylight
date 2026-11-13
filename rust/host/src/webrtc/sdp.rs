//! Offer construction and SDP text handling for the WebRTC transport.
//!
//! The host sends the offer (protocol v2): one video codec at a fixed payload type, Opus at a fixed
//! payload type, a `control` and a `gamepad` DataChannel. str0m builds the offer; the text is then
//! edited for what str0m cannot express (Samsung's `imageattr`, the protocol's stereo fmtp), and
//! the answer is checked before str0m parses it. A surround stream (5.1, 7.1) is negotiated in the
//! answer: the offer is the same stereo Opus whatever the channel count, because Chromium refuses
//! `multiopus` in a remote offer, and the client answers `multiopus` with the layout it decodes.
//! See `docs/webrtc-design.md` ("Library").
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_core::audio::OpusLayout;
use butterpollo_core::webrtc::capabilities::{av1_level, h264_level, hevc_level};
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
/// The profile bytes of libdatachannel's default `42e01f`, which the Tizen app and browsers accept
/// for the High profile stream AMF writes (receivers decode from the SPS and treat the level as
/// asymmetric). The level byte comes from the mode ([`h264_params`]).
const H264_PROFILE: u32 = 0x42e0;

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

/// What the client asked for: stereo, or 5.1/7.1 Opus multistream (a Rubylight extension). The
/// offer is stereo Opus either way; for surround the client's answer must say `multiopus` with a
/// layout of its choosing ([`AnswerInfo::audio_layout`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioMode {
    pub channels: u8,
}

impl AudioMode {
    pub fn stereo() -> Self {
        Self { channels: 2 }
    }

    /// Stereo, 5.1 or 7.1; anything else is an error.
    pub fn request(channels: usize) -> Result<Self> {
        match channels {
            2 | 6 | 8 => Ok(Self {
                channels: channels as u8,
            }),
            _ => bail!("unsupported audio channel count {channels}"),
        }
    }

    fn surround(&self) -> bool {
        self.channels != 2
    }
}

#[derive(Clone, Debug)]
pub struct OfferSpec {
    pub video: VideoMode,
    pub audio: AudioMode,
    /// RTX and NACK feedback for video (`webrtc_rtx`, on by default): str0m answers NACK only
    /// through RTX.
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
    /// The HEVC `tier-flag` or AV1 `tier` the client answered (absent means 0, Main). Like the
    /// level it describes the receiver and may differ from the offer: a Tizen TV answers Main to
    /// the High tier a 4K120 stream at a high bitrate gets.
    pub video_tier: Option<u8>,
    /// The `multiopus` layout the client answered with, for 6 and 8 channels. The client picks its
    /// own parameters (libwebrtc's canonical table for Chromium), and the Opus encoder must be
    /// built with exactly these: input channel `i` is the decoder's output channel `i`. `None` for
    /// stereo.
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

/// The H.264 `profile-level-id` for the mode: the fixed profile bytes and the level from core's
/// table (never below level 3.1).
fn h264_params(mode: &VideoMode) -> Result<u32> {
    let level =
        h264_level(mode.width, mode.height, mode.fps, mode.bitrate_kbps).with_context(|| {
            format!(
                "no H.264 level holds {}x{}@{} at {} kbps",
                mode.width, mode.height, mode.fps, mode.bitrate_kbps
            )
        })?;
    Ok((H264_PROFILE << 8) | level.level_idc)
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
                profile_level_id: Some(h264_params(mode)?),
                ..Default::default()
            },
        ),
        VideoCodec::Hevc => (Codec::H265, {
            // Main tier at every bitrate: Chromium receives HEVC at Main tier only and rejects the
            // video of a High tier offer, which 4K120 above 60 Mbps would get. The level and the
            // bitrate are core's either way.
            let (level, _) = hevc_params(mode)?;
            FormatParams {
                level_asymmetry_allowed: Some(true),
                h265_profile_tier_level: Some((mode.hevc_profile_id(), 0, level).into()),
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
/// video codec and Opus at fixed payload types, RTX when the spec asks for it, and no transport-cc or REMB.
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
            min_p_time: Some(20),
            stereo: Some(true),
            sprop_stereo: Some(true),
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

/// `a=imageattr` and the stereo fmtp, applied to str0m's offer text. The audio is the same for
/// every channel count: standard `opus/48000/2`, which every client accepts.
fn edit_offer(sdp: &str, spec: &OfferSpec) -> Result<String> {
    let mut lines = lines(sdp);
    let audio = section(&lines, "audio").context("the offer has no audio section")?;
    let audio_pt = AUDIO_PT;
    let rtpmap = attribute_line(&lines, &audio, &format!("a=rtpmap:{audio_pt} opus/48000/2"))
        .context("the offer has no Opus rtpmap")?;
    let fmtp = attribute_line(&lines, &audio, &format!("a=fmtp:{audio_pt} "));
    let fmtp_text = format!("a=fmtp:{audio_pt} {STEREO_FMTP}");
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
/// `multiopus` answer to a surround request goes back to `opus/48000/2`. A changed codec or
/// profile, a stereo answer to a surround request, or a dropped DataChannel is an error: the host
/// never falls back.
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
            let want = format!("{:02x}", (H264_PROFILE >> 8) & 0xff);
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
            info.video_tier = Some(answered_tier(param("tier-flag"))?);
            info.video_level = param("level-id").and_then(|v| v.parse().ok());
        }
        VideoCodec::Av1 => {
            if param("profile").unwrap_or("0") != "0" {
                bail!("the answer changed the AV1 profile");
            }
            info.video_tier = Some(answered_tier(param("tier"))?);
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
    let rtpmap_is =
        |line: &str, name: &str| line.eq_ignore_ascii_case(&format!("a=rtpmap:{AUDIO_PT} {name}"));
    match (spec.audio.surround(), rtpmap) {
        (false, Some((_, line))) if rtpmap_is(&line, "opus/48000/2") => {}
        (false, _) => bail!("the answer changed the audio codec (wanted opus/48000/2)"),
        (true, Some((i, line)))
            if rtpmap_is(&line, &format!("multiopus/48000/{}", spec.audio.channels)) =>
        {
            let fmtp = attribute_line(&lines, &audio, &format!("a=fmtp:{AUDIO_PT} "))
                .map(|i| lines[i].clone())
                .unwrap_or_default();
            info.audio_layout = Some(answered_layout(&fmtp, usize::from(spec.audio.channels))?);
            lines[i] = format!("a=rtpmap:{AUDIO_PT} opus/48000/2");
            // What str0m offered, so the rewritten answer reads as a plain stereo Opus one.
            match attribute_line(&lines, &audio, &format!("a=fmtp:{AUDIO_PT} ")) {
                Some(f) => lines[f] = format!("a=fmtp:{AUDIO_PT} {STEREO_FMTP}"),
                None => lines.insert(i + 1, format!("a=fmtp:{AUDIO_PT} {STEREO_FMTP}")),
            }
        }
        (true, Some((_, line))) if rtpmap_is(&line, "opus/48000/2") => bail!(
            "the {n}-channel request was answered with stereo opus/48000/2; a surround client \
             answers multiopus/48000/{n} with its own layout, and the host never falls back to stereo",
            n = spec.audio.channels
        ),
        (true, _) => bail!(
            "the answer to the {n}-channel request is not multiopus/48000/{n} \
             (only Chromium-based receivers decode it)",
            n = spec.audio.channels
        ),
    }

    let application =
        section(&lines, "application").context("the answer has no DataChannel section")?;
    if lines[application.start].starts_with("m=application 0 ") {
        bail!("the answer rejected the DataChannels");
    }
    Ok((join(&lines), info))
}

/// The answered tier, 0 (Main) when absent. The tier, like the level, is the receiver's to state
/// (RFC 7798 and the AV1 RTP payload format): the stream is the one offered either way, so a
/// different tier is logged, never a refusal.
fn answered_tier(value: Option<&str>) -> Result<u8> {
    match value.unwrap_or("0") {
        "0" => Ok(0),
        "1" => Ok(1),
        other => bail!("the answer's video tier is {other}, not 0 or 1"),
    }
}

/// The layout in a `multiopus` answer's fmtp. The answerer chooses its own parameters; they only
/// have to describe a valid RFC 7845-style multistream layout for the `channels` requested, and
/// keys this reads nothing from are ignored.
fn answered_layout(fmtp: &str, channels: usize) -> Result<OpusLayout> {
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
    if mapping.len() != channels {
        bail!(
            "the multiopus channel_mapping has {} entries for {} channels",
            mapping.len(),
            channels
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
    // What libopus's multistream encoder demands: every stream's channels are mapped, the left
    // one of each stream and the right one too for a coupled stream.
    for stream in 0..streams {
        let (left, right) = if stream < coupled {
            (2 * stream, Some(2 * stream + 1))
        } else {
            (stream + coupled, None)
        };
        for channel in std::iter::once(left).chain(right) {
            if !mapping.iter().any(|&m| i32::from(m) == channel) {
                bail!(
                    "the multiopus channel_mapping never uses decoded channel {channel} \
                     (stream {stream} of {streams}, {coupled} coupled)"
                );
            }
        }
    }
    Ok(OpusLayout {
        channels,
        streams,
        coupled,
        mapping,
        // The caller sets the bitrate the video budget leaves.
        bitrate: OpusLayout::select(channels, false, None)?.bitrate,
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
            audio: AudioMode::request(channels).unwrap(),
            ..spec(VideoCodec::H264, false)
        }
    }

    /// Chromium's canonical 5.1 and 7.1 `multiopus` parameters (libwebrtc's own table), which is
    /// what a browser puts in its answer. The encoder is fed in that channel order: input
    /// channel `i` is the decoder's output channel `i`.
    pub fn chrome_layout(channels: usize) -> OpusLayout {
        let custom = match channels {
            6 => "642041235",
            8 => "85306123457",
            _ => panic!("no canonical multiopus layout for {channels} channels"),
        };
        OpusLayout::select(channels, false, Some(custom)).unwrap()
    }

    /// The `a=fmtp` text of a Chromium receiver's multiopus answer for this layout.
    pub fn multiopus_fmtp(layout: &OpusLayout) -> String {
        let mapping: Vec<String> = layout.mapping.iter().map(u8::to_string).collect();
        format!(
            "channel_mapping={};num_streams={};coupled_streams={};minptime=10;useinbandfec=1",
            mapping.join(","),
            layout.streams,
            layout.coupled
        )
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

    /// The answer a Chromium receiver gives to this offer: for surround, str0m's stereo answer
    /// munged the way a client edits its own answer, `multiopus` with the canonical layout (str0m
    /// has no multiopus).
    pub fn answer_for(client: &mut Rtc, offer_sdp: &str, spec: &OfferSpec) -> String {
        let stereo = answer(client, offer_sdp);
        if !spec.audio.surround() {
            return stereo;
        }
        munge_answer(
            &stereo,
            spec.audio.channels as usize,
            &multiopus_fmtp(&chrome_layout(spec.audio.channels as usize)),
        )
    }

    /// `stereo`, an answer to the stereo offer, with its Opus line replaced by `multiopus` and
    /// the given fmtp, which is all a surround client edits.
    pub fn munge_answer(stereo: &str, rtpmap_channels: usize, fmtp: &str) -> String {
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
}

#[cfg(test)]
mod tests;
