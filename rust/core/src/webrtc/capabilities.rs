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
        // H.264 stays out of 4K: 4K120 needs level 6.0, which browsers refuse, so there is no
        // H.264 4K offer to hold at 60.
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
    /// The most 16x16 macroblocks per second each encoder takes in real time: AMF's
    /// `MaxThroughput`, `HevcMaxThroughput` and `Av1CapMaxThroughput`. 0 when the driver
    /// reported none or the codec was not probed, which allows 60 fps and below only.
    /// AMF has no separate 10-bit cap, so HDR is gated by the same number.
    pub h264_mb_rate: u64,
    pub hevc_mb_rate: u64,
    pub av1_mb_rate: u64,
}

impl EncoderProbe {
    fn av1_hdr(self) -> bool {
        self.av1 && self.av1_10bit
    }

    fn supports(self, codec: Codec) -> bool {
        codec != Codec::Av1 || self.av1
    }

    /// Whether the encoder takes this picture size and rate in real time. A codec whose
    /// throughput is unknown gets 60 fps and below, as before the probe.
    fn can_encode(self, codec: Codec, width: u32, height: u32, fps: u32) -> bool {
        let limit = match codec {
            Codec::H264 => self.h264_mb_rate,
            Codec::Hevc => self.hevc_mb_rate,
            Codec::Av1 => self.av1_mb_rate,
        };
        if limit == 0 {
            return fps <= DEFAULT_FRAME_RATE;
        }
        let needed =
            u64::from(width.div_ceil(16)) * u64::from(height.div_ceil(16)) * u64::from(fps);
        needed <= limit
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
    /// Every rate at every mode. What this machine's encoder cannot do in real time is
    /// taken out of the listing by `HostCapabilities` (the AMF throughput probe), and a
    /// request for it is refused. 1080p measured clean at 90 and 120 with H.264, HEVC and
    /// AV1 (rust/PERFORMANCE.md).
    fn default() -> Self {
        Self::uniform(&FRAME_RATES)
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

/// The rates one codec takes at one mode. `videoModes` lists only the rates every codec
/// of a mode takes; this lists each codec's own, which `validate` accepts as well.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecFrameRates {
    pub width: u32,
    pub height: u32,
    pub codec: Codec,
    pub frame_rates: Vec<u32>,
}

/// Rubylight's additions to protocol v2. Stock clients ignore this object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extensions {
    pub audio_channels: Vec<u32>,
    pub input: InputExtension,
    pub codec_frame_rates: Vec<CodecFrameRates>,
}

impl Extensions {
    fn new(codec_frame_rates: Vec<CodecFrameRates>) -> Self {
        Self {
            audio_channels: AUDIO_CHANNELS.to_vec(),
            input: InputExtension {
                label: INPUT_CHANNEL_LABEL.into(),
                version: 1,
                keyboard: true,
                mouse: true,
            },
            codec_frame_rates,
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

    /// The codecs the mode offers on this encoder.
    fn able(&self, mode: &ModeSpec) -> impl Iterator<Item = Codec> {
        Codec::ALL
            .into_iter()
            .filter(move |&c| mode.offers(c) && self.probe.supports(c))
    }

    /// The offered rates one codec takes in real time at the mode.
    fn codec_rates(&self, mode: &ModeSpec, codec: Codec) -> Vec<u32> {
        self.frame_rates
            .rates(mode.width, mode.height)
            .iter()
            .copied()
            .filter(|&fps| self.probe.can_encode(codec, mode.width, mode.height, fps))
            .collect()
    }

    /// The mode as offered, or `None` when no codec takes any of its rates. Every listed
    /// rate works with every listed codec. The codecs are those that take 60 fps, or the
    /// lowest rate some codec takes when none does; the rates are those all of them take.
    /// The rates each codec takes on its own go in the `codecFrameRates` extension.
    fn listing(&self, mode: &ModeSpec) -> Option<VideoMode> {
        let able: Vec<(Codec, Vec<u32>)> = self
            .able(mode)
            .map(|c| (c, self.codec_rates(mode, c)))
            .collect();
        let takes = |fps: u32| able.iter().any(|(_, rates)| rates.contains(&fps));
        let reference = if takes(DEFAULT_FRAME_RATE) {
            DEFAULT_FRAME_RATE
        } else {
            self.frame_rates
                .rates(mode.width, mode.height)
                .iter()
                .copied()
                .find(|&fps| takes(fps))?
        };
        let able: Vec<(Codec, Vec<u32>)> = able
            .into_iter()
            .filter(|(_, rates)| rates.contains(&reference))
            .collect();
        let codecs: Vec<Codec> = able.iter().map(|&(c, _)| c).collect();
        let frame_rates: Vec<u32> = able[0]
            .1
            .iter()
            .copied()
            .filter(|fps| able.iter().all(|(_, rates)| rates.contains(fps)))
            .collect();
        let fps = if frame_rates.contains(&DEFAULT_FRAME_RATE) {
            DEFAULT_FRAME_RATE
        } else {
            *frame_rates.last()?
        };
        let default_codec = if codecs.contains(&mode.default_codec) {
            mode.default_codec
        } else {
            codecs[0]
        };
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
            fps,
            frame_rates,
            codecs,
            hdr_codecs,
            default_codec,
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
        let codec_frame_rates = MODES
            .iter()
            .flat_map(|mode| {
                self.able(mode).map(move |codec| CodecFrameRates {
                    width: mode.width,
                    height: mode.height,
                    codec,
                    frame_rates: self.codec_rates(mode, codec),
                })
            })
            .filter(|entry| !entry.frame_rates.is_empty())
            .collect();
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
            extensions: Extensions::new(codec_frame_rates),
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
        if !self.probe.can_encode(codec, width, height, fps) {
            return Err(unsupported(format!(
                "{} cannot encode {width}x{height} at {fps} fps on this host",
                codec.name()
            )));
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

/// An H.264 level as the last byte of the SDP `profile-level-id`: `level_idc`, ten times the
/// level number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H264Level {
    pub name: &'static str,
    pub level_idc: u32,
}

/// H.264 table A-1 from level 3.1, the level the offer always had, up to 5.2: MaxMBPS
/// (macroblocks a second), MaxFS (macroblocks a frame) and MaxBR (kbps, the Baseline factor the
/// offer's profile byte names). Level 6 and above, which 4K120 would need, browsers refuse.
const H264_LEVELS: [(H264Level, u64, u64, u64); 8] = [
    (
        H264Level {
            name: "3.1",
            level_idc: 31,
        },
        108_000,
        3_600,
        14_000,
    ),
    (
        H264Level {
            name: "3.2",
            level_idc: 32,
        },
        216_000,
        5_120,
        20_000,
    ),
    (
        H264Level {
            name: "4",
            level_idc: 40,
        },
        245_760,
        8_192,
        20_000,
    ),
    (
        H264Level {
            name: "4.1",
            level_idc: 41,
        },
        245_760,
        8_192,
        50_000,
    ),
    (
        H264Level {
            name: "4.2",
            level_idc: 42,
        },
        522_240,
        8_704,
        50_000,
    ),
    (
        H264Level {
            name: "5",
            level_idc: 50,
        },
        589_824,
        22_080,
        135_000,
    ),
    (
        H264Level {
            name: "5.1",
            level_idc: 51,
        },
        983_040,
        36_864,
        240_000,
    ),
    (
        H264Level {
            name: "5.2",
            level_idc: 52,
        },
        2_073_600,
        36_864,
        240_000,
    ),
];

/// The lowest H.264 level, not below 3.1, whose frame size, macroblock rate and bitrate hold the
/// stream, for the `profile-level-id`. `None` above 5.2. At 20 Mbps this gives 4.2 at 1080p60,
/// 5.1 at 1080p90 and 120 or 1440p60, and 5.2 at 1440p90 and 120.
pub fn h264_level(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Option<H264Level> {
    // Pictures are coded in 16x16 macroblocks; a side may be at most sqrt(8 * MaxFS) of them.
    let (w, h) = (
        u64::from(width).div_ceil(16),
        u64::from(height).div_ceil(16),
    );
    H264_LEVELS
        .iter()
        .find(|(_, max_mbps, max_fs, max_kbps)| {
            let side = isqrt(8 * max_fs);
            w * h <= *max_fs
                && w <= side
                && h <= side
                && w * h * u64::from(fps) <= *max_mbps
                && u64::from(bitrate_kbps) <= *max_kbps
        })
        .map(|(level, ..)| *level)
}

#[cfg(test)]
mod tests;
