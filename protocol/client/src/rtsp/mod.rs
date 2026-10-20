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
mod message;

pub use describe::{
    parse_control_messages, parse_opus_configs, sdp_attribute_u32, DescribeInfo, OpusConfig,
    OpusConfigs, MAX_CONTROL_MESSAGES, PYROWAVE_BITSTREAM,
};
pub use message::{parse_response, RtspRequest, RtspResponse};

use crate::sdp::{self, StreamConfig};
use ring::aead;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, SocketAddrV6, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_RTSP_PORT: u16 = 48010;
const DEFAULT_AUDIO_PORT: u16 = 48000;
const DEFAULT_VIDEO_PORT: u16 = 47998;
const DEFAULT_CONTROL_PORT: u16 = 47999;
const DEFAULT_MIC_PORT: u16 = 48001;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_millis(500);
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

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

const ENCRYPTED_RTSP_BIT: u32 = 0x8000_0000;
const ENCRYPTED_HEADER: usize = 24;

/// Sunshine's encrypted RTSP: AES-128-GCM with the remote input key. Each message carries
/// its length with the top bit set, a sequence number and the tag; the IV is the sequence
/// number (little-endian) with 'C' or 'H' and 'R' in its last two bytes.
struct RtspCipher {
    key: aead::LessSafeKey,
    sequence: u32,
}

fn rtsp_iv(sequence: u32, origin: u8) -> aead::Nonce {
    let mut iv = [0u8; 12];
    iv[..4].copy_from_slice(&sequence.to_le_bytes());
    iv[10] = origin;
    iv[11] = b'R';
    aead::Nonce::assume_unique_for_key(iv)
}

impl RtspCipher {
    fn new(key: &[u8; 16]) -> Self {
        let key = aead::UnboundKey::new(&aead::AES_128_GCM, key).expect("AES-128 key length");
        Self {
            key: aead::LessSafeKey::new(key),
            sequence: 0,
        }
    }

    fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>, RtspError> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut body = plain.to_vec();
        let tag = self
            .key
            .seal_in_place_separate_tag(rtsp_iv(self.sequence, b'C'), aead::Aad::empty(), &mut body)
            .map_err(|_| RtspError::Encryption("RTSP encryption failed".into()))?;
        let mut out = Vec::with_capacity(ENCRYPTED_HEADER + body.len());
        out.extend_from_slice(&(ENCRYPTED_RTSP_BIT | plain.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(tag.as_ref());
        out.extend_from_slice(&body);
        Ok(out)
    }

    fn open(&self, raw: &[u8]) -> Result<Vec<u8>, RtspError> {
        let fail = |what: &str| RtspError::Encryption(what.into());
        if raw.len() <= ENCRYPTED_HEADER {
            return Err(fail("RTSP encrypted header too small"));
        }
        let type_and_length = u32::from_be_bytes(raw[..4].try_into().unwrap());
        if type_and_length & ENCRYPTED_RTSP_BIT == 0 {
            return Err(fail("rejecting unencrypted RTSP message"));
        }
        let length = (type_and_length & !ENCRYPTED_RTSP_BIT) as usize;
        if length + ENCRYPTED_HEADER != raw.len() {
            return Err(fail("encrypted RTSP message length mismatch"));
        }
        let sequence = u32::from_be_bytes(raw[4..8].try_into().unwrap());
        let mut body = raw[ENCRYPTED_HEADER..].to_vec();
        body.extend_from_slice(&raw[8..24]);
        let plain = self
            .key
            .open_in_place(rtsp_iv(sequence, b'H'), aead::Aad::empty(), &mut body)
            .map_err(|_| fail("failed to decrypt RTSP response"))?;
        Ok(plain.to_vec())
    }
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
    pub microphone: MicrophoneState,
    pub opus: OpusConfigs,
    pub high_quality_surround_enabled: bool,
    pub audio_packet_duration_ms: u8,
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
}

impl RtspOutcome {
    /// Reference frame invalidation needs both the host and the decoder.
    pub fn reference_frame_invalidation_enabled(&self) -> bool {
        self.host_reference_frame_invalidation
            && sdp::decoder_supports_rfi(self.negotiated_video_format, self.video_capabilities)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Options,
    Describe,
    SetupAudio,
    SetupVideo,
    SetupControl,
    SetupMic,
    Announce,
    Play,
    PlayVideo,
    PlayAudio,
    PlayMic,
    Done,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Options => "OPTIONS",
            Stage::Describe => "DESCRIBE",
            Stage::SetupAudio => "SETUP streamid=audio",
            Stage::SetupVideo => "SETUP streamid=video",
            Stage::SetupControl => "SETUP streamid=control",
            Stage::SetupMic => "SETUP streamid=mic",
            Stage::Announce => "ANNOUNCE",
            Stage::Play => "PLAY",
            Stage::PlayVideo => "PLAY streamid=video",
            Stage::PlayAudio => "PLAY streamid=audio",
            Stage::PlayMic => "PLAY streamid=mic",
            Stage::Done => "done",
        }
    }
}

/// The socket-free handshake. Alternate [`Handshake::next_request`] and
/// [`Handshake::handle_response`], one TCP connection per request read to EOF, then call
/// [`Handshake::finish`].
pub struct Handshake {
    address: IpAddr,
    rtsp_port: u16,
    url_address: String,
    target_url: String,
    sequence: u32,
    cipher: Option<RtspCipher>,
    control_stream_id: &'static str,
    stage: Stage,
    awaiting: bool,
    server_codec_mode_support: u32,
    mic_wanted: bool,
    mic_offered_port: u16,
    has_session: bool,
    describe: Option<DescribeInfo>,
    out: RtspOutcome,
}

impl Handshake {
    pub fn new(params: HandshakeParams) -> Result<Self, RtspError> {
        let mut config = params.config;
        let quad = parse_app_version(&params.app_version);
        if quad[0] >= 5 && quad[0] <= 7 && quad[2] < 404 {
            return Err(RtspError::Unsupported(
                "RTSP over ENet (GeForce Experience before 3.22)".into(),
            ));
        }
        if params.server_codec_mode_support == 0 {
            return Err(RtspError::InvalidConfig(
                "ServerCodecModeSupport must be set".into(),
            ));
        }
        let channels = config.audio.channel_count;
        if !(1..=8).contains(&channels) {
            return Err(RtspError::InvalidConfig(
                "invalid audio configuration".into(),
            ));
        }
        // FEC works in 16-byte blocks; NVENC fails on odd heights.
        config.packet_size -= config.packet_size % 16;
        if config.packet_size <= 0 {
            return Err(RtspError::InvalidConfig("invalid packet size".into()));
        }
        if config.experimental_packed_444 {
            if rubylight_protocol::packed444::Geometry::new(
                config.width as u32,
                config.height as u32,
            )
            .is_none()
            {
                return Err(RtspError::InvalidConfig(
                    "invalid packed 4:4:4 display geometry".into(),
                ));
            }
        } else {
            config.height &= !1;
        }
        let sunshine = quad[3] < 0;
        let mut video_capabilities = config.video_capabilities;
        if config.width == 3840 && config.height == 2160 && !sunshine {
            // GFE's RFI leaves artifacts at 4K.
            video_capabilities &= !sdp::CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC;
        }
        let at_least = |a: i32, b: i32, c: i32| {
            quad[0] > a
                || (quad[0] == a && quad[1] > b)
                || (quad[0] == a && quad[1] == b && quad[2] >= c)
        };
        let rtsp_port = rtsp_port_from_session_url(params.rtsp_session_url.as_deref());
        let session_url = params.rtsp_session_url.as_deref();
        // GFE gives low-quality stereo only to clients it thinks are remote, so the target is
        // faked as 0.0.0.0 unless high-quality audio is wanted.
        let high_quality_audio = config.bitrate_kbps >= sdp::HIGH_AUDIO_BITRATE_THRESHOLD
            && config.audio_capabilities & sdp::CAPABILITY_SLOW_OPUS_DECODER == 0
            && (config.location != sdp::StreamLocation::Remote || channels <= 2);
        let (url_address, target_url) = match (
            high_quality_audio,
            session_url.and_then(|u| url_address(u).map(|a| (a, u))),
        ) {
            (true, Some((address, url))) => (address, url.to_owned()),
            (true, None) => {
                let address = sdp::url_safe_address(params.address);
                let url = format!("rtsp://{address}:{rtsp_port}");
                (address, url)
            }
            (false, _) => ("0.0.0.0".to_owned(), format!("rtsp://0.0.0.0:{rtsp_port}")),
        };
        let encrypted = session_url.is_some_and(|u| u.contains("rtspenc://"));
        let rtsp_client_version = match quad[0] {
            3 => 10,
            4 => 11,
            5 => 12,
            6 => 13,
            _ => 14,
        };
        let cipher = encrypted.then(|| RtspCipher::new(&config.remote_input_aes_key));
        let mic = if config.microphone {
            MicrophoneState::NotOffered
        } else {
            MicrophoneState::Off
        };
        Ok(Self {
            address: params.address,
            rtsp_port,
            url_address,
            target_url,
            sequence: 1,
            cipher,
            control_stream_id: if at_least(7, 1, 431) {
                "streamid=control/13/0"
            } else {
                "streamid=control/1/0"
            },
            stage: Stage::Options,
            awaiting: false,
            server_codec_mode_support: params.server_codec_mode_support,
            mic_wanted: false,
            mic_offered_port: 0,
            has_session: false,
            describe: None,
            out: RtspOutcome {
                config,
                app_version: quad,
                rtsp_client_version,
                session_id: String::new(),
                negotiated_video_format: 0,
                video_capabilities,
                host_reference_frame_invalidation: false,
                sunshine_feature_flags: 0,
                encryption_supported: 0,
                encryption_requested: 0,
                encryption_enabled: 0,
                audio_encryption_enabled: false,
                host_control_messages: Vec::new(),
                microphone: mic,
                opus: OpusConfigs::default(),
                high_quality_surround_enabled: false,
                audio_packet_duration_ms: 5,
                pyrowave_records_enabled: false,
                audio_port: 0,
                video_port: 0,
                control_port: 0,
                mic_port: 0,
                audio_ping_payload: None,
                video_ping_payload: None,
                control_connect_data: 0,
                sdp: String::new(),
            },
        })
    }

    pub fn rtsp_port(&self) -> u16 {
        self.rtsp_port
    }

    pub fn address(&self) -> IpAddr {
        self.address
    }

    pub fn encrypted(&self) -> bool {
        self.cipher.is_some()
    }

    /// The audio port once SETUP audio succeeded; the audio stream may start pinging then.
    pub fn audio_port(&self) -> Option<u16> {
        (self.out.audio_port != 0).then_some(self.out.audio_port)
    }

    /// The audio ping payload from SETUP audio, for pings sent before the handshake ends.
    pub fn audio_ping_payload(&self) -> Option<[u8; 16]> {
        self.out.audio_ping_payload
    }

    fn quad(&self) -> [i32; 4] {
        self.out.app_version
    }

    fn at_least(&self, a: i32, b: i32, c: i32) -> bool {
        let q = self.quad();
        q[0] > a || (q[0] == a && q[1] > b) || (q[0] == a && q[1] == b && q[2] >= c)
    }

    fn request(&mut self, method: &str, target: &str) -> RtspRequest {
        let mut request = RtspRequest::new(method, target);
        request
            .option("CSeq", &self.sequence.to_string())
            .option(
                "X-GS-ClientVersion",
                &self.out.rtsp_client_version.to_string(),
            )
            .option("Host", &self.url_address);
        self.sequence += 1;
        request
    }

    fn stream_target(&self, stream: &str) -> String {
        if self.quad()[0] >= 5 {
            format!("streamid={stream}/0/0")
        } else {
            format!("streamid={stream}")
        }
    }

    fn setup(&mut self, target: &str) -> RtspRequest {
        let session = self.out.session_id.clone();
        let has_session = self.has_session;
        let transport = if self.quad()[0] >= 6 {
            "unicast;X-GS-ClientPort=50000-50001"
        } else {
            " "
        };
        let mut request = self.request("SETUP", target);
        if has_session {
            request.option("Session", &session);
        }
        request
            .option("Transport", transport)
            .option("If-Modified-Since", "Thu, 01 Jan 1970 00:00:00 GMT");
        request
    }

    fn play(&mut self, target: &str) -> RtspRequest {
        let session = self.out.session_id.clone();
        let mut request = self.request("PLAY", target);
        request.option("Session", &session);
        request
    }

    /// The plain request for the current stage, or None when the handshake is complete.
    pub fn next_plain_request(&mut self) -> Result<Option<RtspRequest>, RtspError> {
        if self.awaiting {
            return Err(RtspError::InvalidConfig(
                "previous RTSP response not handled".into(),
            ));
        }
        let request = match self.stage {
            Stage::Done => return Ok(None),
            Stage::Options => {
                let target = self.target_url.clone();
                self.request("OPTIONS", &target)
            }
            Stage::Describe => {
                let target = self.target_url.clone();
                let mut request = self.request("DESCRIBE", &target);
                request
                    .option("Accept", "application/sdp")
                    .option("If-Modified-Since", "Thu, 01 Jan 1970 00:00:00 GMT");
                request
            }
            Stage::SetupAudio => {
                let target = self.stream_target("audio");
                self.setup(&target)
            }
            Stage::SetupVideo => {
                let target = self.stream_target("video");
                self.setup(&target)
            }
            Stage::SetupControl => {
                let target = self.control_stream_id;
                self.setup(target)
            }
            Stage::SetupMic => {
                let target = self.stream_target("mic");
                self.setup(&target)
            }
            Stage::Announce => {
                let target = if self.at_least(7, 1, 431) {
                    self.control_stream_id
                } else {
                    "streamid=video"
                };
                let sdp = self.generate_sdp();
                let session = self.out.session_id.clone();
                let mut request = self.request("ANNOUNCE", target);
                request
                    .option("Session", &session)
                    .option("Content-type", "application/sdp")
                    .option("Content-length", &sdp.len().to_string());
                request.payload = sdp.into_bytes();
                request
            }
            Stage::Play => self.play("/"),
            Stage::PlayVideo => self.play("streamid=video"),
            Stage::PlayAudio => self.play("streamid=audio"),
            Stage::PlayMic => self.play("streamid=mic"),
        };
        self.awaiting = true;
        Ok(Some(request))
    }

    /// The wire bytes for the current stage (encrypted when the session URL is rtspenc://).
    pub fn next_request(&mut self) -> Result<Option<Vec<u8>>, RtspError> {
        let Some(request) = self.next_plain_request()? else {
            return Ok(None);
        };
        let plain = request.serialize();
        match &mut self.cipher {
            Some(cipher) => cipher.seal(&plain).map(Some),
            None => Ok(Some(plain)),
        }
    }

    fn generate_sdp(&mut self) -> String {
        let describe = self
            .describe
            .clone()
            .unwrap_or_else(|| DescribeInfo::parse(""));
        let sdp = sdp::generate(&sdp::SdpParams {
            config: &self.out.config,
            app_version: self.quad(),
            address: self.address,
            rtsp_port: self.rtsp_port,
            video_port: self.out.video_port,
            rtsp_client_version: self.out.rtsp_client_version,
            negotiated_video_format: self.out.negotiated_video_format,
            video_capabilities: self.out.video_capabilities,
            encryption_supported: describe.encryption_supported,
            encryption_requested: describe.encryption_requested,
            mic_port: self.out.mic_port,
            high_quality_surround_supported: self.out.opus.high_quality.is_some(),
            pyrowave_records_enabled: self.out.pyrowave_records_enabled,
        });
        self.out.encryption_enabled = sdp.encryption_enabled;
        self.out.audio_encryption_enabled = sdp.audio_encryption_enabled;
        self.out.config.packet_size = sdp.packet_size;
        self.out.high_quality_surround_enabled = sdp.high_quality_surround_enabled;
        self.out.audio_packet_duration_ms = sdp.audio_packet_duration_ms;
        self.out.sdp = sdp.text.clone();
        sdp.text
    }

    /// Handles the whole response to the last request (decrypting it when needed).
    pub fn handle_response(&mut self, raw: &[u8]) -> Result<(), RtspError> {
        if !self.awaiting {
            return Err(RtspError::InvalidConfig(
                "no RTSP request outstanding".into(),
            ));
        }
        if raw.is_empty() {
            return Err(RtspError::Malformed(format!(
                "{}: the host closed without a response",
                self.stage.name()
            )));
        }
        let plain = match &self.cipher {
            Some(cipher) => cipher.open(raw)?,
            None => raw.to_vec(),
        };
        self.handle_plain_response(&parse_response(&plain)?)
    }

    /// Handles a parsed response to the last request.
    pub fn handle_plain_response(&mut self, response: &RtspResponse) -> Result<(), RtspError> {
        self.awaiting = false;
        let stage = self.stage;
        let ok = response.status_code == 200;
        let failed = || RtspError::Status {
            request: stage.name(),
            code: response.status_code,
            reason: response
                .payload
                .as_ref()
                .and_then(|p| std::str::from_utf8(p).ok())
                .filter(|p| !p.is_empty())
                .unwrap_or(&response.status_text)
                .chars()
                .take(1024)
                .collect(),
        };
        let quad = self.quad();
        self.stage = match stage {
            Stage::Done => {
                return Err(RtspError::InvalidConfig(
                    "handshake already complete".into(),
                ))
            }
            Stage::Options => {
                if !ok {
                    return Err(failed());
                }
                Stage::Describe
            }
            Stage::Describe => {
                if !ok {
                    return Err(failed());
                }
                let text = response
                    .payload_text()
                    .ok_or_else(|| RtspError::Malformed("DESCRIBE has no content".into()))?;
                self.handle_describe(&text)?;
                Stage::SetupAudio
            }
            Stage::SetupAudio => {
                if !ok {
                    return Err(failed());
                }
                self.out.audio_port = transport_port(response).unwrap_or(DEFAULT_AUDIO_PORT);
                self.out.audio_ping_payload = ping_payload(response);
                let session = response.option("Session").ok_or_else(|| {
                    RtspError::Malformed("SETUP streamid=audio has no session".into())
                })?;
                // Only the first token: "DEADBEEFCAFE;timeout = 90".
                let token = session.split(';').next().unwrap_or_default();
                if token.is_empty() {
                    return Err(RtspError::Malformed(
                        "SETUP streamid=audio has a malformed session".into(),
                    ));
                }
                self.out.session_id = token.to_owned();
                self.has_session = true;
                Stage::SetupVideo
            }
            Stage::SetupVideo => {
                if !ok {
                    return Err(failed());
                }
                self.out.video_ping_payload = ping_payload(response);
                self.out.video_port = transport_port(response).unwrap_or(DEFAULT_VIDEO_PORT);
                if quad[0] >= 5 {
                    Stage::SetupControl
                } else {
                    self.after_setup()
                }
            }
            Stage::SetupControl => {
                if !ok {
                    return Err(failed());
                }
                self.out.control_connect_data = response
                    .option("X-SS-Connect-Data")
                    .map_or(0, |d| message::strtol(d, 0).0 as u32);
                self.out.control_port = transport_port(response).unwrap_or(DEFAULT_CONTROL_PORT);
                self.after_setup()
            }
            Stage::SetupMic => {
                if ok {
                    self.out.mic_port =
                        transport_port(response).unwrap_or(if self.mic_offered_port != 0 {
                            self.mic_offered_port
                        } else {
                            DEFAULT_MIC_PORT
                        });
                    self.out.microphone = MicrophoneState::On;
                } else {
                    // A host that refuses costs only the microphone.
                    self.out.microphone = MicrophoneState::Unavailable;
                }
                Stage::Announce
            }
            Stage::Announce => {
                if !ok {
                    return Err(failed());
                }
                if self.at_least(7, 1, 431) {
                    Stage::Play
                } else {
                    Stage::PlayVideo
                }
            }
            Stage::Play => {
                if !ok {
                    return Err(failed());
                }
                Stage::Done
            }
            Stage::PlayVideo => {
                if !ok {
                    return Err(failed());
                }
                Stage::PlayAudio
            }
            Stage::PlayAudio => {
                if !ok {
                    return Err(failed());
                }
                if self.out.mic_port != 0 {
                    Stage::PlayMic
                } else {
                    Stage::Done
                }
            }
            Stage::PlayMic => {
                if !ok {
                    self.out.mic_port = 0;
                    self.out.microphone = MicrophoneState::Unavailable;
                }
                Stage::Done
            }
        };
        Ok(())
    }

    fn after_setup(&self) -> Stage {
        if self.mic_wanted {
            Stage::SetupMic
        } else {
            Stage::Announce
        }
    }

    fn handle_describe(&mut self, text: &str) -> Result<(), RtspError> {
        let info = DescribeInfo::parse(text);
        let config = self.out.config.clone();
        let format = if config.experimental_packed_444 {
            info.select_packed_format(&config)?
        } else {
            info.select_video_format(
                config.supported_video_formats,
                self.server_codec_mode_support,
            )
        };
        let pyrowave = format & sdp::VIDEO_FORMAT_MASK_PYROWAVE != 0;
        self.out.pyrowave_records_enabled = pyrowave && config.pyrowave_records_supported;
        if pyrowave {
            self.out.video_capabilities &= !sdp::CAPABILITY_DIRECT_SUBMIT;
        }
        self.out.negotiated_video_format = format;
        self.out.host_reference_frame_invalidation = info.reference_frame_invalidation;
        self.out.sunshine_feature_flags = info.sunshine_feature_flags;
        self.out.encryption_supported = info.encryption_supported;
        self.out.encryption_requested = info.encryption_requested;
        self.out.encryption_enabled = 0;
        self.out.host_control_messages = info.control_messages.clone();
        // A host takes a microphone only encrypted.
        if config.microphone {
            match info.microphone {
                None => self.out.microphone = MicrophoneState::NotOffered,
                Some(_) if info.encryption_supported & sdp::SS_ENC_MICROPHONE == 0 => {
                    self.out.microphone = MicrophoneState::Unavailable
                }
                Some(port) => {
                    self.mic_wanted = true;
                    self.mic_offered_port = port;
                }
            }
        }
        self.out.opus = parse_opus_configs(text, config.audio.channel_count)?;
        self.describe = Some(info);
        Ok(())
    }

    pub fn is_done(&self) -> bool {
        self.stage == Stage::Done
    }

    pub fn finish(self) -> Result<RtspOutcome, RtspError> {
        if self.stage != Stage::Done {
            return Err(RtspError::InvalidConfig(format!(
                "handshake stopped at {}",
                self.stage.name()
            )));
        }
        Ok(self.out)
    }
}

/// Runs the handshake over TCP. `interrupted` aborts it from another thread.
pub fn perform_handshake(
    params: HandshakeParams,
    interrupted: &AtomicBool,
) -> Result<RtspOutcome, RtspError> {
    let scope = params.ipv6_scope_id;
    let mut handshake = Handshake::new(params)?;
    let address = match handshake.address() {
        IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, handshake.rtsp_port(), 0, scope)),
        v4 => SocketAddr::new(v4, handshake.rtsp_port()),
    };
    while let Some(request) = handshake.next_request()? {
        let response = transact(address, &request, interrupted)?;
        handshake.handle_response(&response)?;
    }
    handshake.finish()
}

/// One request on its own connection, read until the host closes it. Connection refusals
/// are retried for 10 seconds: GFE 3.22 answers /launch before it listens for RTSP.
pub fn transact(
    address: SocketAddr,
    request: &[u8],
    interrupted: &AtomicBool,
) -> Result<Vec<u8>, RtspError> {
    let check = || {
        if interrupted.load(Ordering::SeqCst) {
            Err(RtspError::Interrupted)
        } else {
            Ok(())
        }
    };
    let mut retries = 0;
    let mut socket = loop {
        check()?;
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(socket) => break socket,
            Err(e) if e.kind() == ErrorKind::ConnectionRefused && retries < 20 => {
                retries += 1;
                std::thread::sleep(RETRY_DELAY);
            }
            Err(e) => return Err(e.into()),
        }
    };
    socket.set_nodelay(true)?;
    socket.write_all(request)?;
    let deadline = Instant::now() + RECEIVE_TIMEOUT;
    let mut response = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        check()?;
        let now = Instant::now();
        if now >= deadline {
            return Err(RtspError::Io(ErrorKind::TimedOut.into()));
        }
        socket.set_read_timeout(Some((deadline - now).min(Duration::from_millis(100))))?;
        match socket.read(&mut chunk) {
            Ok(0) => return Ok(response),
            Ok(n) => {
                if response.len() + n > MAX_RESPONSE_SIZE {
                    return Err(RtspError::Malformed(
                        "RTSP response exceeded maximum allowed size".into(),
                    ));
                }
                response.extend_from_slice(&chunk[..n]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests;
