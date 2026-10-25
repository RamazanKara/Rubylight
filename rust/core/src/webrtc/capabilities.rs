//! What this host offers a WebRTC client, and the check that a
//! `start-session` or `switch-session` asks for only that.
//!
//! The mode table is the protocol's. The host adds two inputs: what the
//! encoder probe found (AV1, AV1 10-bit, HEVC Main10) and which frame rates
//! it offers per mode. A request for anything else is `unsupported-settings`;
//! there is never a fallback to another codec, to SDR or to stereo.
use super::protocol::{AUDIO_SAMPLE_RATE, AudioSettings, ErrorCode, ProtocolError, VideoSettings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Frame rates the protocol knows.
pub const FRAME_RATES: [u32; 4] = [30, 60, 90, 120];
/// What the legacy `fps` field of a mode always says, for TV apps from before
/// frame-rate selection.
pub const DEFAULT_FRAME_RATE: u32 = 60;
/// The bitrates the TV offers to select.
pub const SELECTABLE_BITRATES_KBPS: [u32; 11] = [
    10_000, 12_000, 15_000, 20_000, 25_000, 30_000, 40_000, 50_000, 60_000, 80_000, 100_000,
];
/// A request outside this range is refused.
pub const BITRATE_RANGE_KBPS: std::ops::RangeInclusive<u32> = 10_000..=100_000;
/// Audio channel counts: stereo is the protocol's, 6 and 8 are the extension.
pub const AUDIO_CHANNELS: [u32; 3] = [2, 6, 8];
/// The label of the client-opened input DataChannel (extension).
pub const INPUT_CHANNEL_LABEL: &str = "input";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    /// In the order the TV lists them.
    pub const ALL: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];

    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }

    /// Exactly `"h264"`, `"hevc"` or `"av1"`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|codec| codec.name() == name)
    }
}

/// One resolution of the protocol's mode table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeSpec {
    pub width: u32,
    pub height: u32,
    pub h264: bool,
    pub hevc: bool,
    pub av1: bool,
    pub default_codec: Codec,
    pub default_bitrate_kbps: u32,
    pub experimental: bool,
    pub hdr: bool,
}

/// The protocol's table. 1440p is experimental as a whole because Samsung
/// doesn't list it in its Cloud Gaming resolution table.
pub const MODES: [ModeSpec; 4] = [
    ModeSpec {
        width: 1280,
        height: 720,
        h264: true,
        hevc: true,
        av1: true,
        default_codec: Codec::H264,
        default_bitrate_kbps: 12_000,
        experimental: false,
        hdr: false,
    },
    ModeSpec {
        width: 1920,
        height: 1080,
        h264: true,
        hevc: true,
        av1: true,
        default_codec: Codec::H264,
        default_bitrate_kbps: 20_000,
        experimental: false,
        hdr: true,
    },
    ModeSpec {
        width: 2560,
        height: 1440,
        h264: true,
        hevc: true,
        av1: true,
        default_codec: Codec::Hevc,
        default_bitrate_kbps: 30_000,
        experimental: true,
        hdr: true,
    },
    ModeSpec {
        width: 3840,
        height: 2160,
        h264: false,
        hevc: true,
        av1: true,
        default_codec: Codec::Hevc,
        default_bitrate_kbps: 50_000,
        experimental: false,
        hdr: true,
    },
];

impl ModeSpec {
    pub fn find(width: u32, height: u32) -> Option<&'static ModeSpec> {
        MODES
            .iter()
            .find(|m| m.width == width && m.height == height)
    }

    fn offers(&self, codec: Codec) -> bool {
        match codec {
            Codec::H264 => self.h264,
            Codec::Hevc => self.hevc,
            Codec::Av1 => self.av1,
        }
    }
}

/// What the AMF encoder probe found. HEVC Main and H.264 are always there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncoderProbe {
    /// An AV1 encoder exists.
    pub av1: bool,
    /// That encoder also does Main 10-bit (needed for AV1 HDR).
    pub av1_10bit: bool,
    /// The HEVC encoder does Main10 (needed for HEVC HDR).
    pub hevc_main10: bool,
}

impl EncoderProbe {
    fn av1_hdr(self) -> bool {
        self.av1 && self.av1_10bit
    }

    fn supports(self, codec: Codec) -> bool {
        codec != Codec::Av1 || self.av1
    }

    fn supports_hdr(self, codec: Codec) -> bool {
        match codec {
            Codec::H264 => false,
            Codec::Hevc => self.hevc_main10,
            Codec::Av1 => self.av1_hdr(),
        }
    }
}

/// The frame rates the host offers for each mode. A rate the protocol
/// doesn't know is dropped. A mode with no rates isn't offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRateOffer {
    rates: BTreeMap<(u32, u32), Vec<u32>>,
}

impl FrameRateOffer {
    /// The same rates for every mode.
    pub fn uniform(rates: &[u32]) -> Self {
        let mut offer = Self {
            rates: BTreeMap::new(),
        };
        for mode in &MODES {
            offer.set(mode.width, mode.height, rates);
        }
        offer
    }

    pub fn set(&mut self, width: u32, height: u32, rates: &[u32]) {
        let mut rates: Vec<u32> = rates
            .iter()
            .copied()
            .filter(|rate| FRAME_RATES.contains(rate))
            .collect();
        rates.sort_unstable();
        rates.dedup();
        self.rates.insert((width, height), rates);
    }

    pub fn rates(&self, width: u32, height: u32) -> &[u32] {
        self.rates.get(&(width, height)).map_or(&[], Vec::as_slice)
    }
}

impl Default for FrameRateOffer {
    /// 30 and 60 fps everywhere: 90 and 120 are offered only for modes that
    /// have been measured clean. 1080p measured clean at 90 and 120 with
    /// H.264, HEVC and AV1 (rust/PERFORMANCE.md); the others have not been
    /// measured above 60.
    fn default() -> Self {
        let mut offer = Self::uniform(&[30, 60]);
        offer.set(1920, 1080, &[30, 60, 90, 120]);
        offer
    }
}

/// One entry of `videoModes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    /// Always 60, for TV apps from before frame-rate selection.
    pub fps: u32,
    pub frame_rates: Vec<u32>,
    pub codecs: Vec<Codec>,
    pub hdr_codecs: Vec<Codec>,
    pub default_codec: Codec,
    pub default_bitrate_kbps: u32,
    pub experimental: bool,
    pub hdr_supported: bool,
    pub hdr_experimental: bool,
}

/// One entry of the legacy `resolutions` list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub experimental: bool,
}

/// The legacy `defaults` object: the default bitrate at 60 fps per resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(rename = "720p60")]
    pub p720_60: u32,
    #[serde(rename = "1080p60")]
    pub p1080_60: u32,
    #[serde(rename = "1440p60")]
    pub p1440_60: u32,
    #[serde(rename = "2160p60")]
    pub p2160_60: u32,
}

/// What a client opts into with an `input` DataChannel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputExtension {
    pub label: String,
    pub version: u32,
    pub keyboard: bool,
    pub mouse: bool,
}

/// Rubylight's additions to protocol v2. Stock clients ignore this object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extensions {
    pub audio_channels: Vec<u32>,
    pub input: InputExtension,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            audio_channels: AUDIO_CHANNELS.to_vec(),
            input: InputExtension {
                label: INPUT_CHANNEL_LABEL.into(),
                version: 1,
                keyboard: true,
                mouse: true,
            },
        }
    }
}

/// The body of the `capabilities` message. The top-level fields after
/// `videoModes` are the older summary the reference host also sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesBody {
    pub video_modes: Vec<VideoMode>,
    pub resolutions: Vec<Resolution>,
    pub frame_rates: Vec<u32>,
    pub codecs: Vec<Codec>,
    pub hdr: bool,
    pub audio: String,
    pub audio_sample_rate: u32,
    pub bitrates_kbps: Vec<u32>,
    pub defaults: Defaults,
    pub extensions: Extensions,
}

/// A request that passed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    pub bitrate_kbps: u32,
    pub hdr: bool,
    pub audio_channels: u32,
}

impl StreamSettings {
    /// The `video` and `audio` objects of a `session-status`.
    pub fn to_wire(&self) -> (VideoSettings, AudioSettings) {
        (
            VideoSettings {
                width: self.width.into(),
                height: self.height.into(),
                fps: self.fps.into(),
                codec: self.codec.name().into(),
                bitrate_kbps: self.bitrate_kbps.into(),
                hdr: self.hdr,
            },
            AudioSettings {
                channels: self.audio_channels.into(),
                sample_rate: Some(AUDIO_SAMPLE_RATE),
            },
        )
    }
}

fn unsupported(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::UnsupportedSettings, message)
}

/// The host's offer: the encoder probe and the frame rates per mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCapabilities {
    pub probe: EncoderProbe,
    pub frame_rates: FrameRateOffer,
}

impl HostCapabilities {
    pub fn new(probe: EncoderProbe, frame_rates: FrameRateOffer) -> Self {
        Self { probe, frame_rates }
    }

    /// The mode as offered, or `None` when it has no frame rates.
    fn listing(&self, mode: &ModeSpec) -> Option<VideoMode> {
        let frame_rates = self.frame_rates.rates(mode.width, mode.height).to_vec();
        if frame_rates.is_empty() {
            return None;
        }
        let codecs: Vec<Codec> = Codec::ALL
            .into_iter()
            .filter(|&c| mode.offers(c) && self.probe.supports(c))
            .collect();
        let hdr_codecs: Vec<Codec> = if mode.hdr {
            codecs
                .iter()
                .copied()
                .filter(|&c| self.probe.supports_hdr(c))
                .collect()
        } else {
            Vec::new()
        };
        let hdr = !hdr_codecs.is_empty();
        Some(VideoMode {
            width: mode.width,
            height: mode.height,
            fps: DEFAULT_FRAME_RATE,
            frame_rates,
            codecs,
            hdr_codecs,
            default_codec: mode.default_codec,
            default_bitrate_kbps: mode.default_bitrate_kbps,
            experimental: mode.experimental,
            hdr_supported: hdr,
            // HDR is experimental wherever it is offered.
            hdr_experimental: hdr,
        })
    }

    /// The `capabilities` body: only what this host can send.
    pub fn capabilities(&self) -> CapabilitiesBody {
        let video_modes: Vec<VideoMode> = MODES.iter().filter_map(|m| self.listing(m)).collect();
        let resolutions = video_modes
            .iter()
            .map(|m| Resolution {
                width: m.width,
                height: m.height,
                experimental: m.experimental,
            })
            .collect();
        let frame_rates = FRAME_RATES
            .into_iter()
            .filter(|rate| video_modes.iter().any(|m| m.frame_rates.contains(rate)))
            .collect();
        let codecs = Codec::ALL
            .into_iter()
            .filter(|c| video_modes.iter().any(|m| m.codecs.contains(c)))
            .collect();
        let hdr = video_modes.iter().any(|m| m.hdr_supported);
        let default_bitrate = |mode: &ModeSpec| mode.default_bitrate_kbps;
        CapabilitiesBody {
            video_modes,
            resolutions,
            frame_rates,
            codecs,
            hdr,
            audio: "stereo".into(),
            audio_sample_rate: AUDIO_SAMPLE_RATE,
            bitrates_kbps: SELECTABLE_BITRATES_KBPS.to_vec(),
            defaults: Defaults {
                p720_60: default_bitrate(&MODES[0]),
                p1080_60: default_bitrate(&MODES[1]),
                p1440_60: default_bitrate(&MODES[2]),
                p2160_60: default_bitrate(&MODES[3]),
            },
            extensions: Extensions::default(),
        }
    }

    /// Check a `start-session` or `switch-session` against the offer. Any
    /// request outside it is `unsupported-settings` with a message the client
    /// can show; nothing is adjusted or substituted.
    pub fn validate(
        &self,
        video: &VideoSettings,
        audio: &AudioSettings,
    ) -> Result<StreamSettings, ProtocolError> {
        let codec =
            Codec::parse(&video.codec).ok_or_else(|| unsupported("Unsupported video codec"))?;
        // Negative or huge numbers are unsupported settings: they map to a
        // value no mode, rate, bitrate or channel count has.
        let size = |n: i64| u32::try_from(n).unwrap_or(u32::MAX);
        let (width, height, fps) = (size(video.width), size(video.height), size(video.fps));
        let (bitrate_kbps, channels) = (size(video.bitrate_kbps), size(audio.channels));
        let mode = ModeSpec::find(width, height)
            .filter(|m| !self.frame_rates.rates(m.width, m.height).is_empty())
            .ok_or_else(|| unsupported("Unsupported resolution"))?;
        if !self
            .frame_rates
            .rates(mode.width, mode.height)
            .contains(&fps)
        {
            return Err(unsupported("Unsupported frame rate"));
        }
        if !mode.offers(codec) {
            return Err(unsupported("Unsupported resolution and codec combination"));
        }
        if !self.probe.supports(codec) {
            return Err(unsupported("AV1 is not available on this host"));
        }
        if video.hdr {
            if codec == Codec::H264 {
                return Err(unsupported("HDR is supported only with HEVC or AV1"));
            }
            if !mode.hdr {
                return Err(unsupported("HDR is supported only at 1080p, 1440p or 4K"));
            }
            if !self.probe.supports_hdr(codec) {
                return Err(unsupported(match codec {
                    Codec::Av1 => "AV1 HDR (10-bit) is not available on this host",
                    _ => "HEVC HDR (Main10) is not available on this host",
                }));
            }
        }
        if !AUDIO_CHANNELS.contains(&channels) {
            return Err(unsupported(
                "Unsupported audio channel count: use 2, 6 or 8",
            ));
        }
        if !BITRATE_RANGE_KBPS.contains(&bitrate_kbps) {
            return Err(unsupported(format!(
                "Bitrate must be between {} and {} kbps",
                BITRATE_RANGE_KBPS.start(),
                BITRATE_RANGE_KBPS.end()
            )));
        }
        Ok(StreamSettings {
            width,
            height,
            fps,
            codec,
            bitrate_kbps,
            hdr: video.hdr,
            audio_channels: channels,
        })
    }
}

/// The tier of a level. Main is what a decoder is guaranteed to take; High
/// is used only when no Main-tier level has the bitrate for the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Main,
    High,
}

impl Tier {
    /// HEVC `tier-flag` and AV1 `tier` in the SDP: 0 for Main, 1 for High.
    pub fn flag(self) -> u32 {
        match self {
            Self::Main => 0,
            Self::High => 1,
        }
    }
}

/// A HEVC level as the SDP `level-id`: `general_level_idc`, which is 30 times
/// the level number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HevcLevel {
    pub name: &'static str,
    pub level_id: u32,
}

/// An AV1 `seq_level_idx` for the SDP `level-idx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1Level {
    pub name: &'static str,
    pub level_idx: u32,
}

/// The limits of one level that decide whether it holds a stream.
struct Limits {
    max_pic_size: u64,
    max_h_size: u64,
    max_v_size: u64,
    /// Luma samples per second.
    max_sample_rate: u64,
    /// Bitrate in kbps for the Main and the High tier.
    max_kbps_main: u64,
    max_kbps_high: u64,
}

impl Limits {
    fn holds_picture(&self, width: u64, height: u64, fps: u64) -> bool {
        width * height <= self.max_pic_size
            && width <= self.max_h_size
            && height <= self.max_v_size
            && width * height * fps <= self.max_sample_rate
    }
}

/// The lowest entry, then tier, that holds the stream. Every Main-tier level is
/// tried before any High tier one.
fn pick<L: Copy>(
    levels: &[(L, Limits)],
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
) -> Option<(L, Tier)> {
    let (w, h, f, kbps) = (
        u64::from(width),
        u64::from(height),
        u64::from(fps),
        u64::from(bitrate_kbps),
    );
    let holds = |limits: &Limits| limits.holds_picture(w, h, f);
    levels
        .iter()
        .find(|(_, l)| holds(l) && kbps <= l.max_kbps_main)
        .map(|(level, _)| (*level, Tier::Main))
        .or_else(|| {
            levels
                .iter()
                .find(|(_, l)| holds(l) && kbps <= l.max_kbps_high)
                .map(|(level, _)| (*level, Tier::High))
        })
}

const fn hevc(
    name: &'static str,
    level_id: u32,
    max_luma_ps: u64,
    max_luma_sr: u64,
    max_kbps_main: u64,
    max_kbps_high: u64,
) -> (HevcLevel, Limits) {
    (
        HevcLevel { name, level_id },
        Limits {
            max_pic_size: max_luma_ps,
            // Each side is at most sqrt(8 * MaxLumaPs).
            max_h_size: isqrt(8 * max_luma_ps),
            max_v_size: isqrt(8 * max_luma_ps),
            max_sample_rate: max_luma_sr,
            max_kbps_main,
            max_kbps_high,
        },
    )
}

/// The integer square root, rounded down.
const fn isqrt(n: u64) -> u64 {
    let mut root = 0;
    while (root + 1) * (root + 1) <= n {
        root += 1;
    }
    root
}

/// H.265 tables A.8 and A.9, from level 4.1 up to 5.2. Below 4.1 a Samsung
/// decoder may refuse the offer, as the reference host found, and above 5.2
/// is 8K. A stream bigger than 5.2 holds has no level here.
const HEVC_LEVELS: [(HevcLevel, Limits); 4] = [
    hevc("4.1", 123, 2_228_224, 133_693_440, 20_000, 50_000),
    hevc("5", 150, 8_912_896, 267_386_880, 25_000, 100_000),
    hevc("5.1", 153, 8_912_896, 534_773_760, 40_000, 160_000),
    hevc("5.2", 156, 8_912_896, 1_069_547_520, 60_000, 240_000),
];

const fn av1(name: &'static str, level_idx: u32, limits: Limits) -> (Av1Level, Limits) {
    (Av1Level { name, level_idx }, limits)
}

const fn limits(
    max_pic_size: u64,
    max_h_size: u64,
    max_v_size: u64,
    max_sample_rate: u64,
    max_kbps_main: u64,
    max_kbps_high: u64,
) -> Limits {
    Limits {
        max_pic_size,
        max_h_size,
        max_v_size,
        max_sample_rate,
        max_kbps_main,
        max_kbps_high,
    }
}

/// AV1 spec annex A, from level 4.1 up to 5.2. Levels 4.2 and 4.3 are
/// undefined, and 5.3 has the limits of 5.2, so 5.2 always wins.
const AV1_LEVELS: [(Av1Level, Limits); 4] = [
    av1(
        "4.1",
        9,
        limits(2_359_296, 6_144, 3_456, 141_557_760, 20_000, 50_000),
    ),
    av1(
        "5.0",
        12,
        limits(8_912_896, 8_192, 4_352, 267_386_880, 30_000, 100_000),
    ),
    av1(
        "5.1",
        13,
        limits(8_912_896, 8_192, 4_352, 534_773_760, 40_000, 160_000),
    ),
    av1(
        "5.2",
        14,
        limits(8_912_896, 8_192, 4_352, 1_069_547_520, 60_000, 240_000),
    ),
];

/// The lowest HEVC level, not below 4.1, whose picture size, sample rate and
/// Main-tier bitrate hold the stream; if no Main-tier level has the bitrate,
/// the lowest level of the High tier that does. For the HEVC fmtp:
/// `level-id` and `tier-flag`. `None` when even 5.2 High is too small.
/// At a bitrate of 10 Mbps this gives 4.1 at 1080p60, 5 at 1440p60 or
/// 1080p120, 5.1 at 4K60 or 1440p120 and 5.2 at 4K120.
pub fn hevc_level(
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
) -> Option<(HevcLevel, Tier)> {
    pick(&HEVC_LEVELS, width, height, fps, bitrate_kbps)
}

/// The same for AV1: `level-idx` and `tier`.
pub fn av1_level(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Option<(Av1Level, Tier)> {
    pick(&AV1_LEVELS, width, height, fps, bitrate_kbps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webrtc::protocol::{ClientMessage, ServerMessage, parse_client_message};
    use serde_json::{Value, json};

    const FULL: EncoderProbe = EncoderProbe {
        av1: true,
        av1_10bit: true,
        hevc_main10: true,
    };

    fn host(probe: EncoderProbe) -> HostCapabilities {
        HostCapabilities::new(probe, FrameRateOffer::uniform(&[30, 60, 90, 120]))
    }

    fn video(
        width: u32,
        height: u32,
        fps: u32,
        codec: &str,
        kbps: u32,
        hdr: bool,
    ) -> VideoSettings {
        VideoSettings {
            width: width.into(),
            height: height.into(),
            fps: fps.into(),
            codec: codec.into(),
            bitrate_kbps: kbps.into(),
            hdr,
        }
    }

    fn stereo() -> AudioSettings {
        AudioSettings {
            channels: 2,
            sample_rate: None,
        }
    }

    fn refuse(host: &HostCapabilities, v: VideoSettings, channels: u32) -> String {
        let audio = AudioSettings {
            channels: channels.into(),
            sample_rate: None,
        };
        let error = host.validate(&v, &audio).unwrap_err();
        assert_eq!(
            error.code,
            ErrorCode::UnsupportedSettings,
            "{}",
            error.message
        );
        assert!(!error.message.is_empty());
        error.message
    }

    fn mode(body: &CapabilitiesBody, height: u32) -> &VideoMode {
        body.video_modes
            .iter()
            .find(|m| m.height == height)
            .unwrap()
    }

    fn names(codecs: &[Codec]) -> Vec<&'static str> {
        codecs.iter().map(|c| c.name()).collect()
    }

    #[test]
    fn mode_table_matches_the_protocol_with_every_encoder() {
        let body = host(FULL).capabilities();
        let rows: Vec<_> = body
            .video_modes
            .iter()
            .map(|m| {
                (
                    m.width,
                    m.height,
                    names(&m.codecs),
                    names(&m.hdr_codecs),
                    m.default_codec.name(),
                    m.default_bitrate_kbps,
                    m.experimental,
                    m.hdr_supported,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    1280,
                    720,
                    vec!["h264", "hevc", "av1"],
                    vec![],
                    "h264",
                    12000,
                    false,
                    false
                ),
                (
                    1920,
                    1080,
                    vec!["h264", "hevc", "av1"],
                    vec!["hevc", "av1"],
                    "h264",
                    20000,
                    false,
                    true
                ),
                (
                    2560,
                    1440,
                    vec!["h264", "hevc", "av1"],
                    vec!["hevc", "av1"],
                    "hevc",
                    30000,
                    true,
                    true
                ),
                (
                    3840,
                    2160,
                    vec!["hevc", "av1"],
                    vec!["hevc", "av1"],
                    "hevc",
                    50000,
                    false,
                    true
                ),
            ]
        );
        for m in &body.video_modes {
            assert_eq!(m.fps, 60);
            assert_eq!(m.frame_rates, vec![30, 60, 90, 120]);
            assert_eq!(m.hdr_experimental, m.hdr_supported);
        }
        assert_eq!(body.frame_rates, vec![30, 60, 90, 120]);
        assert_eq!(names(&body.codecs), vec!["h264", "hevc", "av1"]);
        assert!(body.hdr);
        assert_eq!(body.audio, "stereo");
        assert_eq!(body.audio_sample_rate, 48000);
        assert_eq!(
            body.bitrates_kbps,
            vec![
                10000, 12000, 15000, 20000, 25000, 30000, 40000, 50000, 60000, 80000, 100000
            ]
        );
        assert_eq!(body.resolutions.len(), 4);
        assert!(body.resolutions[2].experimental);
    }

    #[test]
    fn av1_follows_the_probe() {
        let none = host(EncoderProbe::default()).capabilities();
        for m in &none.video_modes {
            assert!(!m.codecs.contains(&Codec::Av1), "{}", m.height);
            assert!(!m.hdr_codecs.contains(&Codec::Av1));
        }
        assert_eq!(names(&none.codecs), vec!["h264", "hevc"]);
        // No HEVC Main10 and no AV1: no HDR anywhere.
        assert!(!none.hdr);
        assert!(
            none.video_modes
                .iter()
                .all(|m| !m.hdr_supported && m.hdr_codecs.is_empty())
        );

        let av1_8bit = host(EncoderProbe {
            av1: true,
            av1_10bit: false,
            hevc_main10: true,
        })
        .capabilities();
        assert_eq!(
            names(&mode(&av1_8bit, 1080).codecs),
            vec!["h264", "hevc", "av1"]
        );
        assert_eq!(names(&mode(&av1_8bit, 1080).hdr_codecs), vec!["hevc"]);
        assert_eq!(names(&mode(&av1_8bit, 2160).codecs), vec!["hevc", "av1"]);

        // A probe that reports 10-bit without AV1 offers no AV1.
        let odd = host(EncoderProbe {
            av1: false,
            av1_10bit: true,
            hevc_main10: false,
        })
        .capabilities();
        assert_eq!(names(&odd.codecs), vec!["h264", "hevc"]);
        assert!(!odd.hdr);
    }

    #[test]
    fn hdr_only_with_main10_hevc_or_10bit_av1() {
        let only_av1 = host(EncoderProbe {
            av1: true,
            av1_10bit: true,
            hevc_main10: false,
        })
        .capabilities();
        assert_eq!(names(&mode(&only_av1, 1080).hdr_codecs), vec!["av1"]);
        assert_eq!(names(&mode(&only_av1, 2160).hdr_codecs), vec!["av1"]);
        assert!(mode(&only_av1, 1080).hdr_experimental);
        assert!(mode(&only_av1, 720).hdr_codecs.is_empty());
    }

    #[test]
    fn frame_rates_follow_the_offer() {
        let mut offer = FrameRateOffer::default();
        offer.set(3840, 2160, &[60, 30, 60, 144, 0]);
        offer.set(2560, 1440, &[]);
        let body = HostCapabilities::new(FULL, offer).capabilities();
        assert_eq!(mode(&body, 720).frame_rates, vec![30, 60]);
        assert_eq!(mode(&body, 1080).frame_rates, vec![30, 60, 90, 120]);
        assert_eq!(mode(&body, 2160).frame_rates, vec![30, 60]);
        // A mode without rates isn't offered at all.
        assert!(body.video_modes.iter().all(|m| m.height != 1440));
        assert_eq!(body.resolutions.len(), 3);
        assert_eq!(body.frame_rates, vec![30, 60, 90, 120]);
        // Nothing offered: nothing listed.
        let empty = HostCapabilities::new(FULL, FrameRateOffer::uniform(&[])).capabilities();
        assert!(empty.video_modes.is_empty() && empty.codecs.is_empty() && !empty.hdr);
    }

    #[test]
    fn capabilities_carry_the_extensions_object_exactly() {
        let message = ServerMessage::Capabilities(host(FULL).capabilities());
        let value: Value = serde_json::from_str(&message.to_json()).unwrap();
        assert_eq!(
            value["extensions"],
            json!({
                "audioChannels": [2, 6, 8],
                "input": {"label": "input", "version": 1, "keyboard": true, "mouse": true}
            })
        );
        assert_eq!(value["version"], 2);
        assert_eq!(value["type"], "capabilities");
        // The legacy fields a stock client reads.
        assert_eq!(value["videoModes"][1]["fps"], 60);
        assert_eq!(value["videoModes"][1]["hdrCodecs"], json!(["hevc", "av1"]));
        assert_eq!(value["defaults"]["1080p60"], 20000);
        assert_eq!(value["defaults"]["2160p60"], 50000);
        // And it round-trips.
        assert_eq!(
            ServerMessage::from_json(&message.to_json()).unwrap(),
            message
        );
    }

    #[test]
    fn a_complete_start_session_is_accepted_and_echoed() {
        let host = host(FULL);
        let msg = parse_client_message(
            r#"{"version":2,"type":"start-session","appId":"0","video":{"width":3840,"height":2160,"fps":120,"codec":"hevc","bitrateKbps":50000,"hdr":true},"audio":{"channels":2}}"#,
        )
        .unwrap();
        let ClientMessage::StartSession { video, audio, .. } = msg else {
            panic!("not start-session");
        };
        let settings = host.validate(&video, &audio).unwrap();
        assert_eq!(
            settings,
            StreamSettings {
                width: 3840,
                height: 2160,
                fps: 120,
                codec: Codec::Hevc,
                bitrate_kbps: 50000,
                hdr: true,
                audio_channels: 2,
            }
        );
        let (v, a) = settings.to_wire();
        assert_eq!(v, video);
        assert_eq!(a.sample_rate, Some(48000));
    }

    #[test]
    fn every_offered_combination_validates() {
        let host = host(FULL);
        let body = host.capabilities();
        for m in &body.video_modes {
            for &rate in &m.frame_rates {
                for codec in &m.codecs {
                    let v = video(
                        m.width,
                        m.height,
                        rate,
                        codec.name(),
                        m.default_bitrate_kbps,
                        false,
                    );
                    assert!(
                        host.validate(&v, &stereo()).is_ok(),
                        "{}p{rate} {codec:?}",
                        m.height
                    );
                    if m.hdr_codecs.contains(codec) {
                        let v = video(m.width, m.height, rate, codec.name(), 20000, true);
                        assert!(host.validate(&v, &stereo()).is_ok());
                    }
                }
            }
        }
    }

    #[test]
    fn h264_hdr_is_refused() {
        let host = host(FULL);
        let m = refuse(&host, video(1920, 1080, 60, "h264", 20000, true), 2);
        assert!(m.contains("HEVC or AV1"));
    }

    #[test]
    fn hdr_at_720p_is_refused_for_every_codec() {
        let host = host(FULL);
        for codec in ["h264", "hevc", "av1"] {
            refuse(&host, video(1280, 720, 60, codec, 12000, true), 2);
        }
        // SDR at 720p is fine.
        assert!(
            host.validate(&video(1280, 720, 60, "hevc", 12000, false), &stereo())
                .is_ok()
        );
    }

    #[test]
    fn four_k_is_refused_on_h264() {
        let host = host(FULL);
        refuse(&host, video(3840, 2160, 60, "h264", 50000, false), 2);
        assert!(
            host.validate(&video(2560, 1440, 60, "h264", 30000, false), &stereo())
                .is_ok()
        );
    }

    #[test]
    fn bitrate_outside_ten_to_one_hundred_mbps_is_refused() {
        let host = host(FULL);
        for kbps in [0, 9_999, 100_001, 1_000_000] {
            refuse(&host, video(1920, 1080, 60, "h264", kbps, false), 2);
        }
        for kbps in [10_000, 35_000, 100_000] {
            assert!(
                host.validate(&video(1920, 1080, 60, "h264", kbps, false), &stereo())
                    .is_ok()
            );
        }
    }

    #[test]
    fn unknown_codecs_resolutions_and_rates_are_refused() {
        let host = host(FULL);
        for codec in ["vp9", "H264", "HEVC", "h265", ""] {
            refuse(&host, video(1920, 1080, 60, codec, 20000, false), 2);
        }
        refuse(&host, video(1921, 1080, 60, "h264", 20000, false), 2);
        refuse(&host, video(1920, 1080, 144, "h264", 20000, false), 2);
        refuse(&host, video(1920, 1080, 0, "h264", 20000, false), 2);
        // 90 and 120 fps are offered at 1080p, which measured clean, and nowhere else.
        let limited = HostCapabilities::new(FULL, FrameRateOffer::default());
        for fps in [90, 120] {
            assert!(
                limited
                    .validate(&video(1920, 1080, fps, "h264", 20000, false), &stereo())
                    .is_ok()
            );
            for (width, height) in [(1280, 720), (2560, 1440), (3840, 2160)] {
                let m = refuse(&limited, video(width, height, fps, "hevc", 20000, false), 2);
                assert!(m.contains("frame rate"), "{height}p {fps}: {m}");
            }
        }
        let m = refuse(&limited, video(1000, 1080, 60, "h264", 20000, false), 2);
        assert!(m.contains("resolution"));
        // A mode with no rates is an unsupported resolution.
        let mut none = FrameRateOffer::default();
        none.set(1280, 720, &[]);
        let m = refuse(
            &HostCapabilities::new(FULL, none),
            video(1280, 720, 60, "h264", 12000, false),
            2,
        );
        assert!(m.contains("resolution"));
    }

    #[test]
    fn av1_needs_the_encoder_and_av1_hdr_needs_ten_bit() {
        let sdr = video(1920, 1080, 60, "av1", 20000, false);
        let hdr = video(1920, 1080, 60, "av1", 20000, true);
        // With everything.
        assert!(host(FULL).validate(&sdr, &stereo()).is_ok());
        assert!(host(FULL).validate(&hdr, &stereo()).is_ok());
        // AV1 encoder without 10-bit: SDR yes, HDR no.
        let eight = host(EncoderProbe {
            av1: true,
            av1_10bit: false,
            hevc_main10: true,
        });
        assert!(eight.validate(&sdr, &stereo()).is_ok());
        let m = refuse(&eight, hdr.clone(), 2);
        assert!(m.contains("AV1"));
        // No AV1 encoder: neither, even if the probe claims 10-bit.
        for probe in [
            EncoderProbe::default(),
            EncoderProbe {
                av1: false,
                av1_10bit: true,
                hevc_main10: true,
            },
        ] {
            let none = host(probe);
            let m = refuse(&none, sdr.clone(), 2);
            assert!(m.contains("AV1"));
            refuse(&none, hdr.clone(), 2);
        }
    }

    #[test]
    fn hevc_hdr_needs_main10() {
        let hdr = video(3840, 2160, 60, "hevc", 50000, true);
        let sdr = video(3840, 2160, 60, "hevc", 50000, false);
        let no_main10 = host(EncoderProbe {
            av1: true,
            av1_10bit: true,
            hevc_main10: false,
        });
        let m = refuse(&no_main10, hdr.clone(), 2);
        assert!(m.contains("HEVC"));
        assert!(no_main10.validate(&sdr, &stereo()).is_ok());
        assert!(host(FULL).validate(&hdr, &stereo()).is_ok());
    }

    #[test]
    fn audio_channels_two_six_eight_only() {
        let host = host(FULL);
        let v = video(1920, 1080, 60, "h264", 20000, false);
        for channels in [2, 6, 8] {
            let audio = AudioSettings {
                channels: channels.into(),
                sample_rate: None,
            };
            assert_eq!(host.validate(&v, &audio).unwrap().audio_channels, channels);
        }
        for channels in [0, 1, 3, 4, 5, 7, 10] {
            let m = refuse(&host, v.clone(), channels);
            assert!(m.contains("audio"), "{m}");
        }
    }

    #[test]
    fn refusals_never_fall_back() {
        // A refused request yields an error, never settings with a changed codec or HDR flag.
        let host = host(EncoderProbe::default());
        let asked = video(1920, 1080, 60, "av1", 20000, true);
        assert!(host.validate(&asked, &stereo()).is_err());
    }

    /// HEVC level name, id and tier flag at 10 Mbps unless a rate is given.
    fn hevc(w: u32, h: u32, fps: u32, kbps: u32) -> (&'static str, u32, u32) {
        let (level, tier) = hevc_level(w, h, fps, kbps).unwrap();
        (level.name, level.level_id, tier.flag())
    }

    fn av1(w: u32, h: u32, fps: u32, kbps: u32) -> (&'static str, u32, u32) {
        let (level, tier) = av1_level(w, h, fps, kbps).unwrap();
        (level.name, level.level_idx, tier.flag())
    }

    #[test]
    fn hevc_levels_match_the_protocol_examples() {
        assert_eq!(hevc(1920, 1080, 60, 10_000), ("4.1", 123, 0));
        assert_eq!(hevc(2560, 1440, 60, 10_000), ("5", 150, 0));
        assert_eq!(hevc(1920, 1080, 120, 10_000), ("5", 150, 0));
        assert_eq!(hevc(3840, 2160, 60, 10_000), ("5.1", 153, 0));
        assert_eq!(hevc(2560, 1440, 120, 10_000), ("5.1", 153, 0));
        assert_eq!(hevc(3840, 2160, 120, 10_000), ("5.2", 156, 0));
    }

    #[test]
    fn hevc_levels_never_drop_below_4_1() {
        // The reference floors at 4.1: a Samsung decoder may refuse a lower offer.
        assert_eq!(hevc(1920, 1080, 30, 10_000), ("4.1", 123, 0));
        assert_eq!(hevc(1280, 720, 30, 10_000), ("4.1", 123, 0));
        assert_eq!(hevc(1280, 720, 60, 10_000), ("4.1", 123, 0));
        assert_eq!(hevc(1280, 720, 120, 10_000), ("4.1", 123, 0));
        assert_eq!(hevc(1920, 1080, 30, 100_000).2, 1);
    }

    #[test]
    fn hevc_levels_take_the_bitrate_into_account() {
        // 1080p60 fits level 4.1, but 50 Mbps is over its 20 Mbps Main tier.
        assert_eq!(hevc(1920, 1080, 60, 20_000), ("4.1", 123, 0));
        assert_eq!(hevc(1920, 1080, 60, 20_001), ("5", 150, 0));
        assert_eq!(hevc(1920, 1080, 60, 50_000), ("5.2", 156, 0));
        // 4K120 at 100 Mbps is over every Main tier: 5.2 High.
        assert_eq!(hevc(3840, 2160, 120, 60_000), ("5.2", 156, 0));
        assert_eq!(hevc(3840, 2160, 120, 100_000), ("5.2", 156, 1));
        // 1080p30 HDR at its usual bitrate stays at 4.1.
        assert_eq!(hevc(1920, 1080, 30, 20_000), ("4.1", 123, 0));
        // Picture size or rate beyond 5.2 has no level at any bitrate.
        assert_eq!(hevc_level(16384, 16384, 60, 10_000), None);
        assert_eq!(hevc_level(3840, 2160, 240, 10_000), None);
    }

    #[test]
    fn every_offered_mode_has_a_level_across_the_bitrate_range() {
        for mode in &MODES {
            for fps in FRAME_RATES {
                for kbps in [10_000, 50_000, 100_000] {
                    assert!(hevc_level(mode.width, mode.height, fps, kbps).is_some());
                    assert!(av1_level(mode.width, mode.height, fps, kbps).is_some());
                }
            }
        }
    }

    #[test]
    fn av1_levels_follow_the_same_rules() {
        assert_eq!(av1(1920, 1080, 60, 10_000), ("4.1", 9, 0));
        assert_eq!(av1(1920, 1080, 30, 10_000), ("4.1", 9, 0));
        assert_eq!(av1(1280, 720, 30, 10_000), ("4.1", 9, 0));
        assert_eq!(av1(2560, 1440, 60, 10_000), ("5.0", 12, 0));
        assert_eq!(av1(1920, 1080, 120, 10_000), ("5.0", 12, 0));
        assert_eq!(av1(3840, 2160, 60, 10_000), ("5.1", 13, 0));
        assert_eq!(av1(2560, 1440, 120, 10_000), ("5.1", 13, 0));
        assert_eq!(av1(3840, 2160, 120, 10_000), ("5.2", 14, 0));
        assert_eq!(av1(1920, 1080, 60, 50_000), ("5.2", 14, 0));
        assert_eq!(av1(3840, 2160, 120, 100_000), ("5.2", 14, 1));
        assert_eq!(av1_level(100_000, 100_000, 30, 10_000), None);
    }

    #[test]
    fn out_of_range_numbers_are_unsupported_settings() {
        let host = host(FULL);
        let good = video(1920, 1080, 60, "h264", 20000, false);
        let bad = [
            VideoSettings {
                width: -1920,
                ..good.clone()
            },
            VideoSettings {
                height: -1,
                ..good.clone()
            },
            VideoSettings {
                width: i64::MAX,
                ..good.clone()
            },
            VideoSettings {
                fps: -60,
                ..good.clone()
            },
            VideoSettings {
                fps: 1 << 40,
                ..good.clone()
            },
            VideoSettings {
                bitrate_kbps: -20000,
                ..good.clone()
            },
            VideoSettings {
                bitrate_kbps: i64::MAX,
                ..good.clone()
            },
        ];
        for v in bad {
            let error = host.validate(&v, &stereo()).unwrap_err();
            assert_eq!(error.code, ErrorCode::UnsupportedSettings, "{v:?}");
        }
        let audio = AudioSettings {
            channels: -2,
            sample_rate: None,
        };
        let error = host.validate(&good, &audio).unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedSettings);
        assert!(host.validate(&good, &stereo()).is_ok());
    }
}
