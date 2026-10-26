//! Reading the host's DESCRIBE answer: codecs, Sunshine feature and encryption flags,
//! Rubylight's control messages (`control_negotiation.h`), PyroWave (`pyrowave_protocol.h`),
//! the microphone offer (`android_microphone_packet.h`) and the Opus surround parameters
//! (`android_rtsp.c`, from moonlight-common-c `RtspConnection.c`).

use super::message::strtol;
use super::RtspError;
use crate::sdp::*;

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
    pub mapping: [u8; 8],
}

/// The normal layout and, when the host offers one, the high-quality surround layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpusConfigs {
    pub normal: OpusConfig,
    pub high_quality: Option<OpusConfig>,
}

fn parse_opus_params(params: &str, channels: u8) -> Result<OpusConfig, RtspError> {
    let digits: Vec<u8> = params.bytes().take(2 + channels as usize).collect();
    if digits.len() < 2 + channels as usize || !digits.iter().all(u8::is_ascii_digit) {
        return Err(RtspError::Malformed(format!(
            "invalid surround-params: {params:.16}"
        )));
    }
    let mut config = OpusConfig {
        sample_rate: 48000,
        channel_count: channels,
        streams: digits[0] - b'0',
        coupled_streams: digits[1] - b'0',
        mapping: [0; 8],
    };
    for (i, d) in digits[2..].iter().enumerate() {
        config.mapping[i] = d - b'0';
    }
    Ok(config)
}

/// The Opus layouts for `channels` from the DESCRIBE answer's `surround-params`.
pub fn parse_opus_configs(sdp: &str, channels: u8) -> Result<OpusConfigs, RtspError> {
    if channels as usize > 8 {
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
                mapping: [0, 1, 0, 0, 0, 0, 0, 0],
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
                    mapping: [0, 4, 1, 5, 2, 3, 0, 0],
                },
                high_quality: None,
            });
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

#[cfg(test)]
mod tests {
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
        assert_eq!(seven.normal.mapping, [0, 1, 4, 3, 5, 6, 7, 2]);
        let legacy = parse_opus_configs("", 6).unwrap();
        assert_eq!(legacy.normal.mapping[..6], [0, 4, 1, 5, 2, 3]);
        assert!(parse_opus_configs("", 8).is_err());
        assert!(parse_opus_configs("a=fmtp:97 surround-params=64201", 6).is_err());
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
}
