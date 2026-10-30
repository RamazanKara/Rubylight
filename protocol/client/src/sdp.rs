//! The stream description the client sends in RTSP ANNOUNCE, and the stream configuration
//! it is built from.
//!
//! Port of Nightjar's `android_sdp.c`, which is moonlight-common-c's `SdpGenerator.c`
//! (874ac954, GPL-3.0, Cameron Gutman and contributors) with Rubylight's PyroWave
//! negotiation, microphone encryption and USB haptics flags. The text matches the C byte for
//! byte for every configuration, including its trailing space before each CRLF.

use std::net::IpAddr;

use rubylight_protocol::fec_report::{self, FecChoice};

pub const VIDEO_FORMAT_H264: u32 = 0x0001;
pub const VIDEO_FORMAT_H264_HIGH8_444: u32 = 0x0004;
pub const VIDEO_FORMAT_H265: u32 = 0x0100;
pub const VIDEO_FORMAT_H265_MAIN10: u32 = 0x0200;
pub const VIDEO_FORMAT_H265_REXT8_444: u32 = 0x0400;
pub const VIDEO_FORMAT_H265_REXT10_444: u32 = 0x0800;
pub const VIDEO_FORMAT_AV1_MAIN8: u32 = 0x1000;
pub const VIDEO_FORMAT_AV1_MAIN10: u32 = 0x2000;
pub const VIDEO_FORMAT_AV1_HIGH8_444: u32 = 0x4000;
pub const VIDEO_FORMAT_AV1_HIGH10_444: u32 = 0x8000;
pub const VIDEO_FORMAT_PYROWAVE: u32 = 0x01_0000;
pub const VIDEO_FORMAT_PYROWAVE_444: u32 = 0x02_0000;
pub const VIDEO_FORMAT_PYROWAVE_MAIN10: u32 = 0x04_0000;
pub const VIDEO_FORMAT_PYROWAVE_MAIN10_444: u32 = 0x08_0000;
pub const VIDEO_FORMAT_MASK_H264: u32 = 0x000F;
pub const VIDEO_FORMAT_MASK_H265: u32 = 0x0F00;
pub const VIDEO_FORMAT_MASK_AV1: u32 = 0xF000;
pub const VIDEO_FORMAT_MASK_PYROWAVE: u32 = 0x0F_0000;
pub const VIDEO_FORMAT_MASK_10BIT: u32 = 0x0C_AA00;
pub const VIDEO_FORMAT_MASK_YUV444: u32 = 0x0A_CC04;

/// ServerCodecModeSupport bits.
pub const SCM_H264: u32 = 0x0000_0001;
pub const SCM_HEVC: u32 = 0x0000_0100;
pub const SCM_HEVC_MAIN10: u32 = 0x0000_0200;
pub const SCM_AV1_MAIN8: u32 = 0x0001_0000;
pub const SCM_AV1_MAIN10: u32 = 0x0002_0000;
pub const SCM_H264_HIGH8_444: u32 = 0x0004_0000;
pub const SCM_HEVC_REXT8_444: u32 = 0x0008_0000;
pub const SCM_HEVC_REXT10_444: u32 = 0x0010_0000;
pub const SCM_AV1_HIGH8_444: u32 = 0x0020_0000;
pub const SCM_AV1_HIGH10_444: u32 = 0x0040_0000;
pub const SCM_PYROWAVE: u32 = 0x0080_0000;
pub const SCM_PYROWAVE_444: u32 = 0x0100_0000;
pub const SCM_PYROWAVE_MAIN10: u32 = 0x0200_0000;
pub const SCM_PYROWAVE_MAIN10_444: u32 = 0x0400_0000;

/// Streams the client wants encrypted where the host supports it (`encryptionFlags`).
pub const ENCFLG_NONE: u32 = 0;
pub const ENCFLG_AUDIO: u32 = 0x01;
pub const ENCFLG_VIDEO: u32 = 0x02;
pub const ENCFLG_ALL: u32 = 0xFFFF_FFFF;

/// Sunshine encryption features (`x-ss-general.encryption*`).
pub const SS_ENC_CONTROL_V2: u32 = 0x01;
pub const SS_ENC_VIDEO: u32 = 0x02;
pub const SS_ENC_AUDIO: u32 = 0x04;
pub const SS_ENC_MICROPHONE: u32 = 0x08;

/// Client feature flags (`x-ml-general.featureFlags`).
pub const ML_FF_FEC_STATUS: u32 = 0x01;
pub const ML_FF_SESSION_ID_V1: u32 = 0x02;
pub const ML_FF_HAPTICS_PCM: u32 = 0x04;

/// Decoder capabilities; slices per frame go in the top byte.
pub const CAPABILITY_DIRECT_SUBMIT: u32 = 0x1;
pub const CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC: u32 = 0x2;
pub const CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC: u32 = 0x4;
pub const CAPABILITY_SLOW_OPUS_DECODER: u32 = 0x8;
pub const CAPABILITY_SUPPORTS_ARBITRARY_AUDIO_DURATION: u32 = 0x10;
pub const CAPABILITY_PULL_RENDERER: u32 = 0x20;
pub const CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1: u32 = 0x40;

pub const fn capability_slices_per_frame(slices: u8) -> u32 {
    (slices as u32) << 24
}

pub const COLORSPACE_REC_601: i32 = 0;
pub const COLORSPACE_REC_709: i32 = 1;
pub const COLORSPACE_REC_2020: i32 = 2;
pub const COLOR_RANGE_LIMITED: i32 = 0;
pub const COLOR_RANGE_FULL: i32 = 1;

/// High-quality surround needs at least this video bitrate (Kbps).
pub const HIGH_AUDIO_BITRATE_THRESHOLD: i32 = 15000;
pub const LOW_AUDIO_BITRATE_THRESHOLD: i32 = 5000;

/// Encrypted video adds this header to every packet.
const ENC_VIDEO_HEADER_SIZE: i32 = 32;

/// Channel count and Windows channel mask (MAKE_AUDIO_CONFIGURATION).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioConfiguration {
    pub channel_count: u8,
    pub channel_mask: u16,
}

impl AudioConfiguration {
    pub const STEREO: Self = Self {
        channel_count: 2,
        channel_mask: 0x3,
    };
    pub const SURROUND_51: Self = Self {
        channel_count: 6,
        channel_mask: 0x3F,
    };
    pub const SURROUND_71: Self = Self {
        channel_count: 8,
        channel_mask: 0x63F,
    };
    /// Moonlight V+'s `AUDIO_CONFIGURATION_714_SURROUND`, which Foundation Sunshine and
    /// Rubylight hosts stream.
    pub const SURROUND_714: Self = Self {
        channel_count: rubylight_protocol::audio_ext::CHANNELS_714,
        channel_mask: rubylight_protocol::audio_ext::WIRE_CHANNEL_MASK_714,
    };

    /// The `surroundAudioInfo` launch parameter.
    pub fn surround_audio_info(self) -> u32 {
        (self.channel_mask as u32) << 16 | self.channel_count as u32
    }
}

/// Whether the stream crosses the internet; the caller resolves moonlight-common-c's
/// STREAM_CFG_AUTO from the address (private and not NAT64 is local) and caps the packet
/// size for remote streams (1024 for IPv4 and NAT64, 1184 for IPv6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamLocation {
    Local,
    Remote,
}

/// The client's stream configuration (moonlight-common-c's STREAM_CONFIGURATION plus the
/// decoder capabilities and Nightjar's per-connection choices).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    /// Includes FEC; the encoder gets about 80%.
    pub bitrate_kbps: i32,
    /// Maximum video packet size; rounded down to a multiple of 16.
    pub packet_size: i32,
    pub location: StreamLocation,
    pub audio: AudioConfiguration,
    /// VIDEO_FORMAT_* the decoder takes.
    pub supported_video_formats: u32,
    pub experimental_packed_444: bool,
    /// Packed modes verified by the platform's raw-plane renderer and decoder.
    pub packed444_modes: u8,
    /// Selected by the RTSP handshake, independently of the saved preference.
    pub packed444_version: u8,
    /// The display's refresh rate x 100; 0 when unknown.
    pub client_refresh_rate_x100: i32,
    pub color_space: i32,
    pub color_range: i32,
    /// ENCFLG_* streams to encrypt where the host supports it; remote input always is.
    pub encryption_flags: u32,
    /// The `rikey` sent at launch; also the encrypted RTSP key.
    pub remote_input_aes_key: [u8; 16],
    pub remote_input_aes_iv: [u8; 16],
    /// The client's wired link speed toward the host (host pacing, any codec); 0 when unknown or
    /// on Wi-Fi or cellular.
    pub pyrowave_link_mbps: i32,
    /// A loss-free pace found by calibration; 0 when none.
    pub pyrowave_pace_mbps: i32,
    /// The client's route to the host is Wi-Fi or cellular, so a Rubylight host paces for a
    /// wireless link from the first frame. False when wired, unknown or undeterminable.
    pub client_wireless: bool,
    /// CAPABILITY_* of the video decoder, with slices per frame in the top byte.
    pub video_capabilities: u32,
    /// CAPABILITY_* of the audio decoder.
    pub audio_capabilities: u32,
    /// The `audio_ext::CODEC_*` bits besides Opus (PCM, AC3, E-AC3) the audio output can play
    /// for `audio`'s channels, when the user turned passthrough on; 0 streams Opus.
    pub audio_passthrough: u8,
    /// Ask for PCM controller haptics (Nightjar's USB DualSense setting).
    pub haptics_pcm: bool,
    /// The user turned the microphone on.
    pub microphone: bool,
    /// The host frames PyroWave records (HostCapabilities PyroWaveRecords).
    pub pyrowave_records_supported: bool,
    /// Ask the host to send PyroWave records in picture row order (`pyrowaveRowOrder`), which a
    /// client that decodes bands as they arrive relies on. Only sent with records.
    pub pyrowave_row_order: bool,
    /// The HDR format to ask a Rubylight 3.0 host for (one `rubylight_protocol::hdr` bit), 0 for
    /// stock HDR. The ANNOUNCE carries it only under [`SdpParams::hdr_format`]'s rules.
    pub hdr_format: u8,
    /// The video FEC to ask a Foundation Sunshine host for in the ANNOUNCE
    /// ([`SdpParams::video_fec_percentage`]); Rubylight hosts take it at launch instead.
    pub video_fec: FecChoice,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 20000,
            packet_size: 1392,
            location: StreamLocation::Local,
            audio: AudioConfiguration::STEREO,
            supported_video_formats: VIDEO_FORMAT_H264,
            experimental_packed_444: false,
            packed444_modes: 0,
            packed444_version: rubylight_protocol::packed444::VERSION,
            client_refresh_rate_x100: 0,
            color_space: COLORSPACE_REC_709,
            color_range: COLOR_RANGE_LIMITED,
            encryption_flags: ENCFLG_NONE,
            remote_input_aes_key: [0; 16],
            remote_input_aes_iv: [0; 16],
            pyrowave_link_mbps: 0,
            pyrowave_pace_mbps: 0,
            client_wireless: false,
            video_capabilities: 0,
            audio_capabilities: 0,
            audio_passthrough: 0,
            haptics_pcm: false,
            microphone: false,
            pyrowave_records_supported: false,
            pyrowave_row_order: false,
            hdr_format: 0,
            video_fec: FecChoice::Host,
        }
    }
}

/// Whether the decoder can recover with reference frame invalidation for this format.
pub fn decoder_supports_rfi(format: u32, video_capabilities: u32) -> bool {
    (format & VIDEO_FORMAT_MASK_H264 != 0
        && video_capabilities & CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC != 0)
        || (format & VIDEO_FORMAT_MASK_H265 != 0
            && video_capabilities & CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC != 0)
        || (format & VIDEO_FORMAT_MASK_AV1 != 0
            && video_capabilities & CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1 != 0)
}

/// What the description depends on besides the configuration.
#[derive(Clone, Debug)]
pub struct SdpParams<'a> {
    pub config: &'a StreamConfig,
    /// The host's `appversion` quad; a negative last part marks Sunshine and its forks.
    pub app_version: [i32; 4],
    pub address: IpAddr,
    pub rtsp_port: u16,
    pub video_port: u16,
    pub rtsp_client_version: i32,
    pub negotiated_video_format: u32,
    /// Video decoder capabilities after the 4K GameStream RFI rule.
    pub video_capabilities: u32,
    pub encryption_supported: u32,
    pub encryption_requested: u32,
    /// Nonzero when the microphone stream was set up.
    pub mic_port: u16,
    pub high_quality_surround_supported: bool,
    pub pyrowave_records_enabled: bool,
    /// The HDR format asked for (`x-rl-video[0].hdrFormat`, one `rubylight_protocol::hdr` bit);
    /// 0 sends nothing and streams stock HDR. Sent only on a 10-bit (HDR) stream, Dolby Vision
    /// 8.1 and 8.4 only on HEVC and 10.1 only on AV1, HLG on PyroWave only with the host's
    /// PyroWave HLG offer flag, and only when `hdr_formats_offered` has it.
    pub hdr_format: u8,
    /// The host's `DescribeInfo::hdr_formats`, its offer flags included.
    pub hdr_formats_offered: u8,
    /// The audio codec asked for (`x-rl-audio.codec`, one `audio_ext::CODEC_*` bit); 0 sends
    /// nothing and streams Opus. Sent only when `audio_codecs_offered` has it.
    pub audio_codec: u8,
    /// The host's `DescribeInfo::audio_codecs`.
    pub audio_codecs_offered: u8,
    /// `x-ml-video.fecPercentage` ([`FecChoice::summary_percentage`]), with
    /// `x-ml-video.fecSummaryVersion:1`, for a host whose FEC control the handshake turned on
    /// (`RtspOutcome::fec_summaries`); `None` sends neither.
    pub video_fec_percentage: Option<i32>,
}

/// `asked` when it is exactly one bit of `offered`.
fn one_offered_bit(asked: u8, offered: u8) -> Option<u8> {
    (asked.count_ones() == 1 && asked & offered != 0).then_some(asked)
}

/// The HDR format the description asks for, if the stream can carry it: a 10-bit HEVC or AV1
/// stream, Dolby Vision 8.1 and 8.4 only on HEVC and 10.1 only on AV1; on a 10-bit PyroWave
/// stream only HLG, and only when the host offers
/// [`hdr::PYROWAVE_HLG`](rubylight_protocol::hdr::PYROWAVE_HLG).
fn announced_hdr_format(p: &SdpParams<'_>) -> Option<u8> {
    use rubylight_protocol::hdr;
    let format = p.negotiated_video_format;
    let offered = p.hdr_formats_offered;
    one_offered_bit(p.hdr_format, offered & hdr::ALL).filter(|&bit| {
        format & VIDEO_FORMAT_MASK_10BIT != 0
            && if format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                bit == hdr::HLG && offered & hdr::PYROWAVE_HLG != 0
            } else {
                format & (VIDEO_FORMAT_MASK_H265 | VIDEO_FORMAT_MASK_AV1) != 0
                    && (bit & hdr::DOLBY_VISION_HEVC == 0 || format & VIDEO_FORMAT_MASK_H265 != 0)
                    && (bit & hdr::DOLBY_VISION_AV1 == 0 || format & VIDEO_FORMAT_MASK_AV1 != 0)
            }
    })
}

/// The description and the stream settings it fixes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sdp {
    pub text: String,
    /// SS_ENC_* features turned on (Sunshine hosts).
    pub encryption_enabled: u32,
    pub audio_encryption_enabled: bool,
    /// The video packet size after encryption overhead.
    pub packet_size: i32,
    pub high_quality_surround_enabled: bool,
    pub audio_packet_duration_ms: u8,
    /// The HDR format the description asks for, 0 for none (stock HDR).
    pub hdr_format: u8,
}

/// The URL form of an address: IPv6 in brackets.
pub fn url_safe_address(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

struct Attributes(String);

impl Attributes {
    fn add(&mut self, name: &str, value: impl std::fmt::Display) {
        self.0.push_str(&format!("a={name}:{value} \r\n"));
    }
}

pub fn generate(p: &SdpParams<'_>) -> Sdp {
    let c = p.config;
    let quad = p.app_version;
    let sunshine = quad[3] < 0;
    let at_least = |a: i32, b: i32, d: i32| {
        quad[0] > a
            || (quad[0] == a && quad[1] > b)
            || (quad[0] == a && quad[1] == b && quad[2] >= d)
    };
    let address = url_safe_address(p.address);
    let format = p.negotiated_video_format;
    let mut a = Attributes(String::new());
    let mut enabled = 0u32;
    let mut audio_encryption = false;

    if sunshine {
        let flags = ML_FF_FEC_STATUS
            | ML_FF_SESSION_ID_V1
            | if c.haptics_pcm { ML_FF_HAPTICS_PCM } else { 0 };
        a.add("x-ml-general.featureFlags", flags);
        // New-style control encryption is cheap, so it is on whenever supported.
        if p.encryption_supported & SS_ENC_CONTROL_V2 != 0 {
            enabled |= SS_ENC_CONTROL_V2;
        }
        // A host that requests encryption gets it even when the client opted out.
        for (feature, flag) in [(SS_ENC_VIDEO, ENCFLG_VIDEO), (SS_ENC_AUDIO, ENCFLG_AUDIO)] {
            if (p.encryption_supported & feature != 0 && c.encryption_flags & flag != 0)
                || (p.encryption_requested & feature != 0 && c.encryption_flags & flag == 0)
            {
                enabled |= feature;
            }
        }
        // Hosts never take the microphone unencrypted.
        if p.mic_port != 0 && p.encryption_supported & SS_ENC_MICROPHONE != 0 {
            enabled |= SS_ENC_MICROPHONE;
        }
        a.add("x-ss-general.encryptionEnabled", enabled);
        a.add(
            "x-ss-video[0].chromaSamplingType",
            u8::from(format & VIDEO_FORMAT_MASK_YUV444 != 0),
        );
    }

    a.add("x-nv-video[0].clientViewportWd", c.width);
    if c.experimental_packed_444 {
        a.add(
            rubylight_protocol::packed444::REQUEST_ATTRIBUTE,
            c.packed444_version,
        );
    }
    a.add("x-nv-video[0].clientViewportHt", c.height);
    a.add("x-nv-video[0].maxFPS", c.fps);
    let mut packet_size = c.packet_size;
    if enabled & SS_ENC_VIDEO != 0 {
        packet_size -= ENC_VIDEO_HEADER_SIZE;
    }
    a.add("x-nv-video[0].packetSize", packet_size);
    // Foundation Sunshine's client FEC control, where V+'s SdpGenerator.c puts it.
    if let Some(percentage) = p.video_fec_percentage {
        a.add(fec_report::FEC_PERCENTAGE_ATTRIBUTE, percentage);
        a.add(fec_report::FEC_SUMMARY_VERSION_ATTRIBUTE, "1");
    }
    a.add("x-nv-video[0].rateControlMode", "4");
    a.add("x-nv-video[0].timeoutLengthMs", "7000");
    a.add("x-nv-video[0].framesWithInvalidRefThreshold", "0");

    // Packed streams also reserve room for encrypted packet headers and keyframe
    // overshoot: the legacy 20% reserve exceeds the wire budget with 20% FEC.
    let remote = c.location == StreamLocation::Remote;
    let video_share = if c.experimental_packed_444 {
        0.70
    } else {
        0.80
    };
    let mut adjusted = (c.bitrate_kbps as f64 * video_share) as i32;
    if remote && adjusted > 500 {
        adjusted -= 500;
    }
    adjusted = adjusted.min(if c.experimental_packed_444 {
        140_000
    } else {
        100_000
    });
    if quad[0] >= 5 {
        a.add("x-nv-video[0].initialBitrateKbps", adjusted);
        a.add("x-nv-video[0].initialPeakBitrateKbps", adjusted);
        a.add("x-nv-vqos[0].bw.minimumBitrateKbps", adjusted);
        a.add("x-nv-vqos[0].bw.maximumBitrateKbps", adjusted);
        if sunshine {
            a.add("x-ml-video.configuredBitrateKbps", c.bitrate_kbps as u32);
        }
    } else {
        if remote {
            a.add("x-nv-video[0].averageBitrate", "4");
            a.add("x-nv-video[0].peakBitrate", "4");
        }
        a.add("x-nv-vqos[0].bw.minimumBitrate", adjusted);
        a.add("x-nv-vqos[0].bw.maximumBitrate", adjusted);
    }
    a.add("x-nv-vqos[0].fec.enable", "1");
    a.add("x-nv-vqos[0].videoQualityScoreUpdateTime", "5000");
    // QoS tags only on a LAN, where they cannot black-hole traffic at an ISP.
    if c.location == StreamLocation::Local {
        a.add("x-nv-vqos[0].qosTrafficType", "5");
        a.add("x-nv-aqos.qosTrafficType", "4");
    } else {
        a.add("x-nv-vqos[0].qosTrafficType", "0");
        a.add("x-nv-aqos.qosTrafficType", "0");
    }

    if quad[0] == 3 {
        // GFE 2.x takes these as raw big-endian integers, which happen to be ASCII.
        a.add("x-nv-general.serverAddress", &address);
        a.add("x-nv-general.featureFlags", "BwAA");
        for i in 0..4 {
            a.add(&format!("x-nv-video[{i}].transferProtocol"), "AQAA");
        }
        a.add("x-nv-video[0].rateControlMode", "BAAA");
        for i in 1..4 {
            a.add(&format!("x-nv-video[{i}].rateControlMode"), "BQAA");
        }
        a.add("x-nv-vqos[0].bw.flags", "14083");
        for i in 0..4 {
            a.add(&format!("x-nv-vqos[{i}].videoQosMaxConsecutiveDrops"), "0");
        }
    } else if quad[0] == 4 {
        a.add(
            "x-nv-general.serverAddress",
            format!("rtsp://{address}:{}", p.rtsp_port),
        );
    } else {
        if at_least(7, 1, 431) {
            // Remote input encryption is always on.
            let mut flags = 0x07 | 0x80;
            if c.encryption_flags & ENCFLG_AUDIO != 0 || enabled & SS_ENC_AUDIO != 0 {
                flags |= 0x20;
                audio_encryption = true;
            }
            a.add("x-nv-general.featureFlags", flags);
            // The encrypted control protocol, which also encrypts input.
            a.add("x-nv-general.useReliableUdp", "13");
            // At least 2 FEC packets even for tiny frames.
            a.add("x-nv-vqos[0].fec.minRequiredFecPackets", "2");
            // BLL-FEC lowers FEC far below the set percentage; GFE then uses the legacy scheme.
            a.add("x-nv-vqos[0].bllFec.enable", "0");
        } else {
            a.add("x-nv-general.useReliableUdp", "1");
            a.add("x-nv-ri.useControlChannel", "1");
            if c.width >= 3840 && c.height >= 2160 {
                a.add("x-nv-vqos[0].fec.repairPercent", "5");
            } else {
                a.add("x-nv-vqos[0].fec.repairPercent", "20");
            }
        }
        if at_least(7, 1, 446) && (c.width < 720 || c.height < 540) {
            // GFE 3.26 crashes at very low resolutions unless DRC has a static table.
            a.add("x-nv-vqos[0].drc.enable", "1");
            a.add("x-nv-vqos[0].drc.tableType", "2");
        } else {
            a.add("x-nv-vqos[0].drc.enable", "0");
        }
        // Recovery mode can change the FEC percentage mid-frame.
        a.add("x-nv-general.enableRecoveryMode", "0");
    }

    // Every Rubylight host is past generation 4, where the 3.0 extensions go.
    let hdr_format = if quad[0] >= 4 {
        announced_hdr_format(p)
    } else {
        None
    };
    let channels = c.audio.channel_count as i32;
    if quad[0] >= 4 {
        let slices = match (p.video_capabilities >> 24) as u8 {
            0 => 1,
            n => n,
        };
        a.add("x-nv-video[0].videoEncoderSlicesPerFrame", slices);
        if format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            a.add("x-nv-vqos[0].bitStreamFormat", "3");
            if p.pyrowave_records_enabled {
                a.add("x-ss-video[0].pyrowaveFeatures", "1");
                // A host that does not know the key ignores it; one that does needs records for it.
                if c.pyrowave_row_order {
                    a.add("x-ss-video[0].pyrowaveRowOrder", "1");
                }
            }
            // The calibrated pace is a PyroWave frame pace, only when known.
            if c.pyrowave_pace_mbps > 0 {
                a.add("x-ss-video[0].pyrowavePaceMbps", c.pyrowave_pace_mbps);
            }
        } else if format & VIDEO_FORMAT_MASK_AV1 != 0 {
            a.add("x-nv-vqos[0].bitStreamFormat", "2");
        } else if format & VIDEO_FORMAT_MASK_H265 != 0 {
            a.add("x-nv-clientSupportHevc", "1");
            a.add("x-nv-vqos[0].bitStreamFormat", "1");
            if !at_least(7, 1, 408) {
                // GFE 3.10's split frame encode breaks HEVC.
                a.add("x-nv-video[0].encoderFeatureSetting", "0");
            }
        } else {
            a.add("x-nv-clientSupportHevc", "0");
            a.add("x-nv-vqos[0].bitStreamFormat", "0");
        }
        // This client's own wired-link speed, for every codec so a host can cap its pacing; only
        // when known (0 on Wi-Fi, cellular or an unreadable port).
        if c.pyrowave_link_mbps > 0 {
            a.add("x-ss-video[0].pyrowaveLinkMbps", c.pyrowave_link_mbps);
        }
        // Only ever 1: a wired or unknown route sends nothing, never a 0.
        if c.client_wireless {
            a.add("x-rl-video[0].clientWireless", 1);
        }
        // The 3.0 extensions, only when asked for and offered: a host without them never sees
        // the keys.
        if let Some(hdr_format) = hdr_format {
            a.add(rubylight_protocol::hdr::ANNOUNCE_ATTRIBUTE, hdr_format);
        }
        if let Some(codec) = one_offered_bit(p.audio_codec, p.audio_codecs_offered) {
            a.add(rubylight_protocol::audio_ext::CODEC_ANNOUNCE_ATTRIBUTE, codec);
        }
        if quad[0] >= 7 {
            a.add(
                "x-nv-video[0].dynamicRangeMode",
                u8::from(format & VIDEO_FORMAT_MASK_10BIT != 0),
            );
            // A decoder with RFI takes the codec's maximum reference frames; others get one.
            let references = if decoder_supports_rfi(format, p.video_capabilities) {
                "0"
            } else {
                "1"
            };
            a.add("x-nv-video[0].maxNumReferenceFrames", references);
            a.add(
                "x-nv-video[0].clientRefreshRateX100",
                c.client_refresh_rate_x100,
            );
        }
        a.add("x-nv-audio.surround.numChannels", channels);
        a.add(
            "x-nv-audio.surround.channelMask",
            c.audio.channel_mask as i32,
        );
        a.add("x-nv-audio.surround.enable", u8::from(channels > 2));
    }

    let slow_opus = c.audio_capabilities & CAPABILITY_SLOW_OPUS_DECODER != 0;
    let (high_quality, duration) = if quad[0] >= 7 {
        let high_quality = c.bitrate_kbps >= HIGH_AUDIO_BITRATE_THRESHOLD
            && channels > 2
            && p.high_quality_surround_supported
            && !slow_opus;
        let duration = if high_quality {
            5
        } else if slow_opus
            || (c.audio_capabilities & CAPABILITY_SUPPORTS_ARBITRARY_AUDIO_DURATION != 0
                && c.bitrate_kbps < LOW_AUDIO_BITRATE_THRESHOLD)
        {
            10
        } else {
            5
        };
        a.add("x-nv-audio.surround.AudioQuality", u8::from(high_quality));
        a.add("x-nv-aqos.packetDuration", duration);
        (high_quality, duration)
    } else {
        (false, 5)
    };
    if quad[0] >= 7 {
        a.add(
            "x-nv-video[0].encoderCscMode",
            (c.color_space << 1) | c.color_range,
        );
    }

    let family = if p.address.is_ipv4() { "IPv4" } else { "IPv6" };
    let video_port = if quad[0] < 4 { 47996 } else { p.video_port };
    let text = format!(
        "v=0\r\no=android 0 {} IN {family} {address}\r\ns=NVIDIA Streaming Client\r\n{}t=0 0\r\nm=video {video_port}  \r\n",
        p.rtsp_client_version, a.0
    );
    Sdp {
        text,
        encryption_enabled: enabled,
        audio_encryption_enabled: audio_encryption,
        packet_size,
        high_quality_surround_enabled: high_quality,
        audio_packet_duration_ms: duration,
        hdr_format: hdr_format.unwrap_or(0),
    }
}

#[cfg(test)]
mod tests;
