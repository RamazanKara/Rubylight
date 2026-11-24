//! The client RTSP handshake: OPTIONS, DESCRIBE, SETUP of audio, video, control and the
//! microphone, ANNOUNCE with the stream description, and PLAY.
//!
//! Port of Nightjar's `android_rtsp.c`, which is moonlight-common-c's `RtspConnection.c`
//! (874ac954, GPL-3.0, Cameron Gutman and contributors) with Rubylight's PyroWave and control
//! message negotiation and the microphone negotiation of ClassicOldSong/moonlight-common-c
//! (784fa1d0, GPL-3.0). [`Handshake`] is the socket-free state machine;
//! [`perform_handshake`] drives it over TCP, one connection per request, including
//! Sunshine's encrypted RTSP (`rtspenc://`). RTSP over ENet (GFE before 3.22's 7.1.404) is
//! not supported.

mod describe;
mod handshake;
mod message;
mod transport;

pub use describe::{
    parse_control_messages, parse_opus_configs, sdp_attribute_u32, DescribeInfo, OpusConfig,
    OpusConfigs, MAX_CONTROL_MESSAGES, PYROWAVE_BITSTREAM,
};
pub use handshake::Handshake;
pub use message::{parse_response, RtspRequest, RtspResponse};
pub use transport::{perform_handshake, transact};

use crate::sdp::{self, StreamConfig};
use std::net::IpAddr;

pub const DEFAULT_RTSP_PORT: u16 = 48010;

/// Why the handshake failed.
#[derive(Debug)]
pub enum RtspError {
    Io(std::io::Error),
    /// The host answered `request` with a status other than 200.
    Status {
        request: &'static str,
        code: i32,
        reason: String,
    },
    Malformed(String),
    Encryption(String),
    Unsupported(String),
    /// Packed 4:4:4 was requested and cannot be streamed: no mode both endpoints offer, or the
    /// host refused it at ANNOUNCE. The client may reconnect without it.
    PackedRefused(String),
    InvalidConfig(String),
    Interrupted,
}

impl std::fmt::Display for RtspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RtspError::Io(e) => write!(f, "RTSP: {e}"),
            RtspError::Status {
                request,
                code,
                reason,
            } => {
                write!(f, "RTSP {request} request failed: {code} {reason}")
            }
            RtspError::Malformed(e)
            | RtspError::Encryption(e)
            | RtspError::Unsupported(e)
            | RtspError::PackedRefused(e)
            | RtspError::InvalidConfig(e) => {
                write!(f, "RTSP: {e}")
            }
            RtspError::Interrupted => f.write_str("RTSP handshake interrupted"),
        }
    }
}

impl std::error::Error for RtspError {}

impl From<std::io::Error> for RtspError {
    fn from(e: std::io::Error) -> Self {
        RtspError::Io(e)
    }
}

/// moonlight-common-c's extractVersionQuadFromString: lenient, missing parts are 0.
pub fn parse_app_version(version: &str) -> [i32; 4] {
    let mut quad = [0; 4];
    let mut rest = version;
    for part in &mut quad {
        let end = rest
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
            .map_or(rest.len(), |(i, _)| i);
        *part = message::strtol(&rest[..end], 10).0 as i32;
        rest = &rest[end..];
        if !rest.is_empty() {
            rest = &rest[rest.chars().next().unwrap().len_utf8()..];
        }
    }
    quad
}

/// The RTSP port in a session URL (after its last ':'), else 48010.
pub fn rtsp_port_from_session_url(url: Option<&str>) -> u16 {
    url.and_then(|u| u.rfind(':').map(|at| message::strtol(&u[at + 1..], 10).0))
        .filter(|p| (1..=65535).contains(p))
        .map_or(DEFAULT_RTSP_PORT, |p| p as u16)
}

/// The host part of an RTSP URL, as moonlight-common-c's parseUrlAddrFromRtspUrlString cuts it.
fn url_address(url: &str) -> Option<String> {
    let prefix = url.find("//")? + 2;
    let cut = if let Some(bracket) = url.find(']') {
        bracket + 1
    } else if let Some(colon) = url.rfind(':') {
        // A colon in the scheme leaves the rest whole.
        if colon >= prefix {
            colon
        } else {
            url.len()
        }
    } else {
        url[prefix..]
            .find('/')
            .map_or(url.len(), |slash| prefix + slash)
    };
    (cut >= prefix).then(|| url[prefix..cut].to_owned())
}

/// The value after `server_port=` in a Transport option.
fn transport_port(response: &RtspResponse) -> Option<u16> {
    let transport = response.option("Transport")?;
    let at = transport.find("server_port=")?;
    let (port, _) = message::strtol(&transport[at + 12..], 10);
    (1..=65535).contains(&port).then_some(port as u16)
}

fn ping_payload(response: &RtspResponse) -> Option<[u8; 16]> {
    response
        .option("X-SS-Ping-Payload")?
        .as_bytes()
        .try_into()
        .ok()
}

/// What the handshake needs to know.
#[derive(Clone, Debug)]
pub struct HandshakeParams {
    /// The host's resolved address.
    pub address: IpAddr,
    /// The scope of a link-local IPv6 address, 0 otherwise.
    pub ipv6_scope_id: u32,
    /// The serverinfo `appversion`.
    pub app_version: String,
    /// ServerCodecModeSupport, trusted (see `ServerInfo::trusted_codec_mode_support`).
    pub server_codec_mode_support: u32,
    /// `sessionUrl0` from the launch reply.
    pub rtsp_session_url: Option<String>,
    pub config: StreamConfig,
}

/// The microphone after negotiation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicrophoneState {
    /// Not requested.
    Off,
    NotOffered,
    /// Offered without encryption, or the host refused its SETUP or PLAY.
    Unavailable,
    On,
}

/// The negotiated stream: what the session layer needs to open its sockets and decoders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtspOutcome {
    /// The configuration as streamed: packet size rounded to 16 and less encryption
    /// overhead, height even.
    pub config: StreamConfig,
    pub app_version: [i32; 4],
    pub rtsp_client_version: i32,
    pub session_id: String,
    pub negotiated_video_format: u32,
    /// Video decoder capabilities after the 4K GameStream RFI rule; direct submit is
    /// cleared for PyroWave so GPU waits never block the receive thread.
    pub video_capabilities: u32,
    /// The host supports reference frame invalidation.
    pub host_reference_frame_invalidation: bool,
    pub sunshine_feature_flags: u32,
    pub encryption_supported: u32,
    pub encryption_requested: u32,
    /// SS_ENC_* in use; SS_ENC_CONTROL_V2 selects the new control encryption.
    pub encryption_enabled: u32,
    /// GameStream audio encryption (featureFlags 0x20) or SS_ENC_AUDIO.
    pub audio_encryption_enabled: bool,
    pub host_control_messages: Vec<u16>,
    /// The host takes client roaming probes (`a=x-rl-roaming`).
    pub host_roaming: bool,
    /// The stream takes pen barrel roll (`a=x-ss-general.penBarrelRoll:1`); send it only
    /// with `Feature::PenBarrelRoll` too.
    pub host_pen_barrel_roll: bool,
    pub microphone: MicrophoneState,
    pub opus: OpusConfigs,
    pub high_quality_surround_enabled: bool,
    pub audio_packet_duration_ms: u8,
    /// The `audio_ext::CODEC_*` bit the ANNOUNCE asked for, 0 for stock Opus.
    pub audio_codec: u8,
    pub pyrowave_records_enabled: bool,
    pub audio_port: u16,
    pub video_port: u16,
    /// 0 for GameStream generations before 5.
    pub control_port: u16,
    /// 0 without a microphone stream.
    pub mic_port: u16,
    pub audio_ping_payload: Option<[u8; 16]>,
    pub video_ping_payload: Option<[u8; 16]>,
    pub control_connect_data: u32,
    /// The ANNOUNCE body.
    pub sdp: String,
    /// The packed 4:4:4 modes the host's DESCRIBE offered (`packed444::mode_bit`), 0 without.
    pub host_packed444_modes: u8,
    /// The HDR format the ANNOUNCE asked for (one `rubylight_protocol::hdr` bit), 0 for stock.
    pub hdr_format: u8,
    /// Foundation Sunshine's client FEC control is on (V+'s `VideoFecControlSupported`): the
    /// ANNOUNCE carried `x-ml-video.fecPercentage`, and the control stream sends 0x5510
    /// summaries. Never with PyroWave, nor for a host that takes Rubylight's 0x5539 reports.
    pub fec_summaries: bool,
}

impl RtspOutcome {
    /// Reference frame invalidation needs both the host and the decoder.
    pub fn reference_frame_invalidation_enabled(&self) -> bool {
        self.host_reference_frame_invalidation
            && sdp::decoder_supports_rfi(self.negotiated_video_format, self.video_capabilities)
    }

    /// Why packed 4:4:4 asked for in 10-bit streams 8-bit (no 10-bit mode in common), or None:
    /// the host offers no 10-bit packed mode at all, or only for a codec this client lacks.
    pub fn packed_ten_bit_dropped(&self) -> Option<&'static str> {
        if !self.config.experimental_packed_444
            || self.config.supported_video_formats & sdp::VIDEO_FORMAT_MASK_10BIT == 0
            || self.negotiated_video_format & sdp::VIDEO_FORMAT_MASK_10BIT != 0
        {
            return None;
        }
        use rubylight_protocol::packed444::mode_bit;
        Some(
            if self.host_packed444_modes & (mode_bit(1, true) | mode_bit(2, true)) == 0 {
                "host doesn't offer 10-bit full chroma"
            } else {
                "this device and PC share no 10-bit full chroma mode"
            },
        )
    }
}

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;
