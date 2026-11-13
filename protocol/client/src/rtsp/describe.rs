//! Reading the host's DESCRIBE answer: codecs, Sunshine feature and encryption flags,
//! Rubylight's control messages (`control_negotiation.h`), PyroWave (`pyrowave_protocol.h`),
//! the microphone offer (`android_microphone_packet.h`) and the Opus surround parameters
//! (`android_rtsp.c`, from moonlight-common-c `RtspConnection.c`).

use super::message::strtol;
use super::RtspError;
use crate::sdp::*;
use rubylight_protocol::audio_ext::{self, Layout};
use rubylight_protocol::{fec_report, hdr, pen};

/// The PyroWave bitstream revision this client decodes; a future revision falls back to the
/// ordinary codecs.
pub const PYROWAVE_BITSTREAM: &str = "a=x-ss-pyrowave.bitstream:186f0393";
/// The most control message ids kept.
pub const MAX_CONTROL_MESSAGES: usize = 32;

/// What the DESCRIBE answer says about the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescribeInfo {
    pub av1: bool,
    /// The base64 HEVC VPS prefix (hosts label HEVC as H264).
    pub hevc: bool,
    pub packed444_modes: u8,
    pub packed444_layered_modes: u8,
    /// A `PYROWAVE/90000` rtpmap and the supported bitstream revision.
    pub pyrowave: bool,
    pub reference_frame_invalidation: bool,
    pub sunshine_feature_flags: u32,
    pub encryption_supported: u32,
    pub encryption_requested: u32,
    /// Rubylight control messages the host accepts (`a=x-rl-control`).
    pub control_messages: Vec<u16>,
    /// The host takes a microphone; the port its `m=audio` line names, 0 for none.
    pub microphone: Option<u16>,
    /// The host takes client roaming probes (`a=x-rl-roaming`, version 1 or later).
    pub roaming: bool,
    /// The HDR formats this stream offers (`a=x-rl-video.hdrFormats`, `rubylight_protocol::hdr`
    /// bits) and the host's offer flags (`hdr::OFFER_FLAGS`); 0 for stock HDR only.
    pub hdr_formats: u8,
    /// The audio codecs the host sends (`a=x-rl-audio.codecs`, `audio_ext::CODEC_*` bits); 0
    /// when it names none, which is Opus only.
    pub audio_codecs: u8,
    /// The 7.1.4 Opus layouts (`a=x-rl-audio.layout714` and `a=x-rl-audio.layout714Hq`).
    pub layout_714: Option<Layout>,
    pub layout_714_hq: Option<Layout>,
    /// The host reads pen barrel roll (`a=x-ss-general.penBarrelRoll:1`). Other Sunshine-family
    /// hosts may send the same attribute: send the input only with `Feature::PenBarrelRoll` too.
    pub pen_barrel_roll: bool,
    /// Foundation Sunshine's client FEC control (`a=x-ss-video[0].fecControlVersion:1`): the
    /// host takes `x-ml-video.fecPercentage` and 0x5510 summaries (`fec_report::FecSummary`).
    pub fec_control: bool,
}

/// moonlight-common-c's parseSdpAttributeToUInt: the first occurrence of `name` anywhere,
/// then the number after the next ':' (strtoul with base 0).
pub fn sdp_attribute_u32(sdp: &str, name: &str) -> Option<u32> {
    let at = sdp.find(name)?;
    let colon = sdp[at..].find(':')?;
    let (value, _) = strtol(&sdp[at + colon + 1..], 0);
    // strtoul wraps negative input and the caller keeps the low 32 bits.
    Some(value as u32)
}

fn lines(sdp: &str) -> impl Iterator<Item = &str> {
    sdp.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// The value of the first `a={name}:` line, without surrounding blanks.
fn attribute<'a>(sdp: &'a str, name: &str) -> Option<&'a str> {
    lines(sdp).find_map(|line| {
        let (key, value) = line.strip_prefix("a=")?.split_once(':')?;
        (key == name).then(|| value.trim_matches([' ', '\t']))
    })
}

/// A decimal bit mask attribute, limited to `known`; 0 when absent or malformed.
fn attribute_bits(sdp: &str, name: &str, known: u8) -> u8 {
    attribute(sdp, name)
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
        & known
}

/// Rubylight 2.2.0+: `a=x-rl-control:0x5530,0x5531`; hex with 0x or decimal, malformed and
/// duplicate ids skipped.
pub fn parse_control_messages(sdp: &str) -> Vec<u16> {
    let mut ids = Vec::new();
    for line in lines(sdp) {
        let Some(list) = line.strip_prefix("a=x-rl-control:") else {
            continue;
        };
        for item in list.split(',') {
            let item = item.trim_matches([' ', '\t']);
            let (digits, radix) = match item.strip_prefix("0x").or_else(|| item.strip_prefix("0X"))
            {
                Some(hex) if item.len() > 2 => (hex, 16),
                _ => (item, 10),
            };
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                continue;
            }
            if let Ok(id) = u16::from_str_radix(digits, radix) {
                if !ids.contains(&id) && ids.len() < MAX_CONTROL_MESSAGES {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

fn offers_pyrowave(sdp: &str) -> bool {
    let mut codec = false;
    let mut bitstream = false;
    for line in lines(sdp) {
        bitstream |= line == PYROWAVE_BITSTREAM;
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            codec |= digits > 0 && &rest[digits..] == " PYROWAVE/90000";
        }
    }
    codec && bitstream
}

/// `a=x-rl-roaming:<n>` names the highest roaming wire format the host speaks; this client
/// speaks version 1.
fn offers_roaming(sdp: &str) -> bool {
    use rubylight_protocol::roaming::{SDP_ATTRIBUTE, VERSION};
    let (name, _) = SDP_ATTRIBUTE
        .rsplit_once(':')
        .expect("the attribute has a value");
    lines(sdp).any(|line| {
        line.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(':'))
            .and_then(|version| version.trim().parse::<u8>().ok())
            .is_some_and(|version| version >= VERSION)
    })
}

/// The microphone offer: a mono (or stereo) Opus rtpmap; the port of the last `m=audio`
/// line before it.
fn microphone_offer(sdp: &str) -> Option<u16> {
    let rtpmap = sdp
        .find("a=rtpmap:96 opus/48000/1")
        .or_else(|| sdp.find("a=rtpmap:96 opus/48000/2"))?;
    let mut port = 0;
    let mut search = 0;
    while let Some(found) = sdp[search..].find("m=audio ") {
        let at = search + found;
        if at >= rtpmap {
            break;
        }
        let digits: String = sdp[at + 8..]
            .chars()
            .take_while(char::is_ascii_digit)
            .take(6)
            .collect();
        port = digits
            .parse::<u32>()
            .ok()
            .filter(|p| (1..=65535).contains(p))
            .unwrap_or(0) as u16;
        search = at + 1;
    }
    Some(port)
}

impl DescribeInfo {
    pub fn parse(sdp: &str) -> Self {
        Self {
            av1: sdp.contains("AV1/90000"),
            hevc: sdp.contains("sprop-parameter-sets=AAAAAU"),
            packed444_modes: {
                use rubylight_protocol::packed444;
                let read = |name: &str| {
                    lines(sdp).find_map(|line| {
                        let (key, value) = line.strip_prefix("a=")?.split_once(':')?;
                        (key == name).then(|| value.parse::<u8>().ok()).flatten()
                    })
                };
                if read(packed444::VERSION_ATTRIBUTE) == Some(packed444::VERSION) {
                    read(packed444::MODES_ATTRIBUTE).unwrap_or(0) & packed444::ALL_MODES
                } else {
                    0
                }
            },
            pyrowave: offers_pyrowave(sdp),
            packed444_layered_modes: lines(sdp)
                .find_map(|line| {
                    let (key, value) = line.strip_prefix("a=")?.split_once(':')?;
                    (key == rubylight_protocol::packed444::LAYERED_MODES_ATTRIBUTE)
                        .then(|| value.parse::<u8>().ok())
                        .flatten()
                })
                .unwrap_or(0)
                & rubylight_protocol::packed444::ALL_MODES,
            reference_frame_invalidation: sdp.contains("x-nv-video[0].refPicInvalidation"),
            sunshine_feature_flags: sdp_attribute_u32(sdp, "x-ss-general.featureFlags")
                .unwrap_or(0),
            encryption_supported: sdp_attribute_u32(sdp, "x-ss-general.encryptionSupported")
                .unwrap_or(0),
            encryption_requested: sdp_attribute_u32(sdp, "x-ss-general.encryptionRequested")
                .unwrap_or(0),
            control_messages: parse_control_messages(sdp),
            microphone: microphone_offer(sdp),
            roaming: offers_roaming(sdp),
            hdr_formats: attribute_bits(sdp, hdr::DESCRIBE_ATTRIBUTE, hdr::ALL | hdr::OFFER_FLAGS),
            audio_codecs: attribute_bits(
                sdp,
                audio_ext::CODECS_DESCRIBE_ATTRIBUTE,
                audio_ext::CODECS_ALL,
            ),
            layout_714: attribute(sdp, audio_ext::LAYOUT_714_DESCRIBE_ATTRIBUTE)
                .and_then(Layout::parse),
            layout_714_hq: attribute(sdp, audio_ext::LAYOUT_714_HQ_DESCRIBE_ATTRIBUTE)
                .and_then(Layout::parse),
            pen_barrel_roll: attribute(sdp, pen::DESCRIBE_ATTRIBUTE)
                .and_then(|value| value.parse::<u8>().ok())
                .is_some_and(|version| version >= 1),
            fec_control: sdp_attribute_u32(sdp, fec_report::FEC_CONTROL_DESCRIBE_ATTRIBUTE)
                == Some(fec_report::FEC_CONTROL_VERSION),
        }
    }

    /// The video format to stream: PyroWave, then AV1, HEVC and H.264, each at the best
    /// profile both sides support. `server_codec_mode_support` should be the trusted value
    /// (PyroWave bits only from a host that shows PyroWave).
    pub fn select_video_format(&self, supported: u32, server_codec_mode_support: u32) -> u32 {
        let scm = server_codec_mode_support;
        let pick = |choices: &[(u32, u32)], fallback: u32| {
            choices
                .iter()
                .find(|&&(host, client)| scm & host != 0 && supported & client != 0)
                .map_or(fallback, |&(_, client)| client)
        };
        if self.pyrowave {
            let format = pick(
                &[
                    (SCM_PYROWAVE_MAIN10_444, VIDEO_FORMAT_PYROWAVE_MAIN10_444),
                    (SCM_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10),
                    (SCM_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_444),
                    (SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE),
                ],
                0,
            );
            if format != 0 {
                return format;
            }
        }
        if supported & VIDEO_FORMAT_MASK_AV1 != 0 && self.av1 {
            pick(
                &[
                    (SCM_AV1_HIGH10_444, VIDEO_FORMAT_AV1_HIGH10_444),
                    (SCM_AV1_MAIN10, VIDEO_FORMAT_AV1_MAIN10),
                    (SCM_AV1_HIGH8_444, VIDEO_FORMAT_AV1_HIGH8_444),
                ],
                VIDEO_FORMAT_AV1_MAIN8,
            )
        } else if supported & VIDEO_FORMAT_MASK_H265 != 0 && self.hevc {
            pick(
                &[
                    (SCM_HEVC_REXT10_444, VIDEO_FORMAT_H265_REXT10_444),
                    (SCM_HEVC_MAIN10, VIDEO_FORMAT_H265_MAIN10),
                    (SCM_HEVC_REXT8_444, VIDEO_FORMAT_H265_REXT8_444),
                ],
                VIDEO_FORMAT_H265,
            )
        } else {
            pick(
                &[(SCM_H264_HIGH8_444, VIDEO_FORMAT_H264_HIGH8_444)],
                VIDEO_FORMAT_H264,
            )
        }
    }

    /// The packed 4:4:4 format: a mode both endpoints offer, AV1 before HEVC. A 10-bit request
    /// with no 10-bit mode in common streams 8-bit packed instead (the format is chosen before
    /// ANNOUNCE, so this costs no reconnect); [`super::RtspOutcome::packed_ten_bit_dropped`] tells.
    /// No packed mode in common at all is [`RtspError::PackedRefused`].
    pub fn select_packed_format(&self, config: &StreamConfig) -> Result<u32, RtspError> {
        if self.packed444_modes == 0 {
            return Err(RtspError::PackedRefused("host does not offer packed 4:4:4 version 1; enable its experimental setting on a supported AMD encoder".into()));
        }
        if config.packed444_modes == 0 {
            return Err(RtspError::PackedRefused(
                "client raw YUV reconstruction is unavailable for packed 4:4:4".into(),
            ));
        }
        let ten_bit = config.supported_video_formats & VIDEO_FORMAT_MASK_10BIT != 0;
        let depths: &[bool] = if ten_bit { &[true, false] } else { &[false] };
        for &ten_bit in depths {
            for (codec, available, format) in [
                (
                    2,
                    self.av1,
                    if ten_bit {
                        VIDEO_FORMAT_AV1_MAIN10
                    } else {
                        VIDEO_FORMAT_AV1_MAIN8
                    },
                ),
                (
                    1,
                    self.hevc,
                    if ten_bit {
                        VIDEO_FORMAT_H265_MAIN10
                    } else {
                        VIDEO_FORMAT_H265
                    },
                ),
            ] {
                let bit = rubylight_protocol::packed444::mode_bit(codec, ten_bit);
                if available
                    && config.supported_video_formats & format != 0
                    && self.packed444_modes & config.packed444_modes & bit != 0
                {
                    return Ok(format);
                }
            }
        }
        Err(RtspError::PackedRefused(
            "requested packed codec and bit depth are not supported by both endpoints".into(),
        ))
    }

    pub fn packed_version(&self, format: u32) -> u8 {
        use rubylight_protocol::packed444;
        let codec = if format & VIDEO_FORMAT_MASK_AV1 != 0 {
            2
        } else {
            1
        };
        let bit = packed444::mode_bit(codec, format & VIDEO_FORMAT_MASK_10BIT != 0);
        if self.packed444_modes & self.packed444_layered_modes & bit != 0 {
            packed444::LAYERED_VERSION
        } else {
            packed444::VERSION
        }
    }
}

/// One Opus multistream layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpusConfig {
    pub sample_rate: u32,
    pub channel_count: u8,
    pub streams: u8,
    pub coupled_streams: u8,
    pub mapping: [u8; MAX_CHANNELS],
}

/// The most channels a layout has: 7.1.4.
pub const MAX_CHANNELS: usize = 12;

/// The normal layout and, when the host offers one, the high-quality surround layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpusConfigs {
    pub normal: OpusConfig,
    pub high_quality: Option<OpusConfig>,
}

fn parse_opus_params(params: &str, channels: u8) -> Result<OpusConfig, RtspError> {
    let invalid = || RtspError::Malformed(format!("invalid surround-params: {params:.16}"));
    // The value ends at the line's end or a space.
    let value = params
        .split([' ', '\r', '\n'])
        .next()
        .unwrap_or_default();
    let numbers: Vec<u8> = if channels > 8 || value.contains([',', ';', ':', '|']) {
        // Foundation Sunshine's form for 7.1.4, whose mapping reaches 11:
        // `12,8,4,0,1,...` after the channel count. Moonlight V+ (its moonlight-common-c
        // `parseOpusConfigFromDelimitedString`) takes any of these delimiters.
        let mut numbers = Vec::new();
        for part in value
            .split([',', ';', ':', '|'])
            .filter(|part| !part.is_empty())
            .take(2 + channels as usize)
        {
            numbers.push(part.parse::<u8>().map_err(|_| invalid())?);
        }
        numbers
    } else {
        let digits: Vec<u8> = value.bytes().take(2 + channels as usize).collect();
        if !digits.iter().all(u8::is_ascii_digit) {
            return Err(invalid());
        }
        digits.iter().map(|d| d - b'0').collect()
    };
    if numbers.len() < 2 + channels as usize {
        return Err(invalid());
    }
    let mut config = OpusConfig {
        sample_rate: 48000,
        channel_count: channels,
        streams: numbers[0],
        coupled_streams: numbers[1],
        mapping: [0; MAX_CHANNELS],
    };
    config.mapping[..channels as usize].copy_from_slice(&numbers[2..]);
    Ok(config)
}

/// The Opus layouts for `channels` from the DESCRIBE answer's `surround-params` (for 7.1.4
/// without them, [`layouts_714`]).
pub fn parse_opus_configs(sdp: &str, channels: u8) -> Result<OpusConfigs, RtspError> {
    if channels as usize > MAX_CHANNELS {
        return Err(RtspError::InvalidConfig(format!(
            "invalid channel count: {channels}"
        )));
    }
    if channels == 2 {
        // Stereo has no surround-params.
        return Ok(OpusConfigs {
            normal: OpusConfig {
                sample_rate: 48000,
                channel_count: 2,
                streams: 1,
                coupled_streams: 1,
                mapping: [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            },
            high_quality: None,
        });
    }
    let prefix = format!("a=fmtp:97 surround-params={channels}");
    let Some(at) = sdp.find(&prefix) else {
        // Older GFE may omit the parameters; 5.1 has a known layout.
        if channels == 6 {
            return Ok(OpusConfigs {
                normal: OpusConfig {
                    sample_rate: 48000,
                    channel_count: 6,
                    streams: 4,
                    coupled_streams: 2,
                    mapping: [0, 4, 1, 5, 2, 3, 0, 0, 0, 0, 0, 0],
                },
                high_quality: None,
            });
        }
        if channels == audio_ext::CHANNELS_714 {
            return layouts_714(sdp);
        }
        return Err(RtspError::Malformed(format!(
            "no surround parameters for {channels} channels"
        )));
    };
    let params = &sdp[at + prefix.len()..];
    let mut normal = parse_opus_params(params, channels)?;
    if channels == 6 || channels == 8 {
        // GFE orders FL FR C RL RR SL SR LFE; clients expect FL FR C LFE RL RR SL SR.
        let original = normal.mapping;
        let n = channels as usize;
        normal.mapping[3] = original[n - 1];
        normal.mapping[4..n].copy_from_slice(&original[3..n - 1]);
    }
    let high_quality = match params.find(&prefix) {
        Some(next) => Some(parse_opus_params(&params[next + prefix.len()..], channels)?),
        None => None,
    };
    Ok(OpusConfigs {
        normal,
        high_quality,
    })
}

/// 7.1.4 from a host that sends no `surround-params` for it: a Rubylight 3.0 rc names its
/// layouts only in `a=x-rl-audio.layout714` and `layout714Hq` (7 streams, 5 coupled). With
/// neither, Moonlight V+'s fallback: Foundation Sunshine's layout (`audio_ext::LAYOUT_714`).
fn layouts_714(sdp: &str) -> Result<OpusConfigs, RtspError> {
    let layout = |name: &str| {
        attribute(sdp, name)
            .map(|value| {
                Layout::parse(value)
                    .ok_or_else(|| RtspError::Malformed(format!("invalid {name}: {value:.16}")))
            })
            .transpose()
    };
    let config = |layout: Layout| OpusConfig {
        sample_rate: 48000,
        channel_count: layout.channels,
        streams: layout.streams,
        coupled_streams: layout.coupled,
        mapping: layout.mapping,
    };
    Ok(match layout(audio_ext::LAYOUT_714_DESCRIBE_ATTRIBUTE)? {
        Some(normal) => OpusConfigs {
            normal: config(normal),
            high_quality: layout(audio_ext::LAYOUT_714_HQ_DESCRIBE_ATTRIBUTE)?.map(config),
        },
        None => OpusConfigs {
            normal: config(audio_ext::LAYOUT_714),
            high_quality: None,
        },
    })
}

#[cfg(test)]
mod tests;
