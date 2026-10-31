//! RTSP message parsing and responses, and the stream parameters a client
//! negotiates in its `ANNOUNCE`/`SETUP` exchange.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_MESSAGE: usize = 65536;
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub cseq: u32,
}
pub fn complete_length(b: &[u8]) -> Result<Option<usize>> {
    if b.len() > MAX_MESSAGE {
        bail!("RTSP message too large");
    }
    let Some(i) = b.windows(4).position(|b| b == b"\r\n\r\n") else {
        return Ok(None);
    };
    let total = message_length(b, i)?;
    Ok(if b.len() >= total { Some(total) } else { None })
}
fn message_length(b: &[u8], i: usize) -> Result<usize> {
    let head = std::str::from_utf8(&b[..i])?;
    let mut length = 0;
    let mut seen = false;
    for l in head.lines().skip(1) {
        if let Some((k, v)) = l.split_once(':')
            && k.eq_ignore_ascii_case("Content-Length")
        {
            if seen {
                bail!("duplicate Content-Length");
            }
            seen = true;
            length = v.trim().parse::<usize>()?;
        }
    }
    let total = (i + 4)
        .checked_add(length)
        .context("RTSP length overflow")?;
    if total > MAX_MESSAGE {
        bail!("RTSP message too large");
    }
    Ok(total)
}
/// Incremental framing for one RTSP request. Search new header bytes only and
/// parse Content-Length once, even when the peer fragments a large request.
#[derive(Default)]
pub struct MessageBuffer {
    bytes: Vec<u8>,
    expected: Option<usize>,
}
impl MessageBuffer {
    pub fn append(&mut self, bytes: &[u8]) -> Result<bool> {
        if bytes.len() > MAX_MESSAGE - self.bytes.len() {
            bail!("RTSP message too large");
        }
        let scan_from = self.bytes.len().saturating_sub(3);
        self.bytes.extend_from_slice(bytes);
        if self.expected.is_none()
            && let Some(offset) = self.bytes[scan_from..]
                .windows(4)
                .position(|b| b == b"\r\n\r\n")
        {
            self.expected = Some(message_length(&self.bytes, scan_from + offset)?);
        }
        if let Some(total) = self.expected {
            if self.bytes.len() > total {
                bail!("unexpected RTSP trailing data");
            }
            return Ok(self.bytes.len() == total);
        }
        Ok(false)
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}
impl Request {
    pub fn parse(b: &[u8]) -> Result<Self> {
        let n = complete_length(b)?.context("incomplete RTSP message")?;
        if n != b.len() {
            bail!("multiple messages passed to RTSP parser");
        }
        let i = b.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&b[..i])?;
        let mut lines = head.lines();
        let parts: Vec<_> = lines
            .next()
            .context("empty request")?
            .split_whitespace()
            .collect();
        if parts.len() != 3 || parts[2] != "RTSP/1.0" {
            bail!("invalid RTSP request line");
        }
        let mut headers = BTreeMap::new();
        for l in lines {
            let (k, v) = l.split_once(':').context("invalid RTSP header")?;
            if headers
                .insert(k.to_ascii_lowercase(), v.trim().to_owned())
                .is_some()
            {
                bail!("duplicate RTSP header");
            }
        }
        let cseq = headers.get("cseq").context("missing CSeq")?.parse()?;
        Ok(Self {
            method: parts[0].into(),
            target: parts[1].into(),
            headers,
            body: b[i + 4..].to_vec(),
            cseq,
        })
    }
}
pub fn response(
    cseq: u32,
    code: u16,
    reason: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut s = format!("RTSP/1.0 {code} {reason}\r\nCSeq: {cseq}\r\n");
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        s.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    s.push_str("\r\n");
    let mut b = s.into_bytes();
    b.extend_from_slice(body);
    b
}
/// `ML_FF_FEC_STATUS` in [`Negotiated::moonlight_features`]
/// (moonlight-common-c `Limelight-internal.h:88`): the client sends
/// `SS_FRAME_FEC_STATUS` (0x5502) for every frame it repaired or lost.
pub const FEC_STATUS_FEATURE: u32 = 0x01;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Negotiated {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[serde(default)]
    pub rate_millihz: u32,
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub configured_bitrate_kbps: u32,
    #[serde(default)]
    pub csc_mode: u8,
    pub codec: u8,
    pub hdr: bool,
    #[serde(default)]
    pub sdr_10bit: bool,
    pub yuv444: bool,
    #[serde(default)]
    pub chroma_packing: u8,
    #[serde(default)]
    pub slices: u32,
    #[serde(default)]
    pub references: u32,
    #[serde(default)]
    pub intra_refresh: bool,
    #[serde(default)]
    pub vrr_low_latency: bool,
    #[serde(default)]
    pub pyrowave_records: bool,
    /// The client's own wired link in Mbps (`x-ss-video[0].pyrowaveLinkMbps`,
    /// Nonary's Moonlight), 0 when it did not say.
    #[serde(default)]
    pub pyrowave_client_link_mbps: u32,
    /// The client's route to the host is Wi-Fi or cellular
    /// (`x-rl-video[0].clientWireless:1`); false when absent or 0 (unknown).
    #[serde(default)]
    pub client_wireless: bool,
    /// The client asked for long-term reference recovery on H.264 and HEVC
    /// (`rubylight_protocol::ltr_recovery`, a launch parameter, not SDP; set
    /// at ANNOUNCE). Since 3.0.0 every client has it without asking; still
    /// parsed from clients that send it.
    #[serde(default)]
    pub ltr_recovery: bool,
    /// The pace the client's calibration found loss-free, in Mbps
    /// (`x-ss-video[0].pyrowavePaceMbps`), 0 when it has none.
    #[serde(default)]
    pub pyrowave_client_pace_mbps: u32,
    /// The client wants each picture band's PyroWave records together, the coarse ones first
    /// (`x-ss-video[0].pyrowaveRowOrder:1`). Off without the key, and for any other value.
    #[serde(default)]
    pub pyrowave_row_order: bool,
    pub packet_size: usize,
    pub min_fec: usize,
    pub audio_channels: u8,
    pub audio_packet_ms: u8,
    pub audio_quality: bool,
    pub encryption: u32,
    pub reliable_control: u32,
    /// Client extensions announced in x-ml-general.featureFlags.
    #[serde(default)]
    pub moonlight_features: u32,
    /// Moonlight asks for QoS tags on a local network and not over the
    /// internet, where routers may drop tagged packets.
    #[serde(default)]
    pub video_qos: bool,
    #[serde(default)]
    pub audio_qos: bool,
    /// The client set up the microphone stream (`streamid=mic`) and
    /// encrypts it.
    #[serde(default)]
    pub mic: bool,
    /// The HDR format the client picked from the host's offer
    /// (`x-rl-video[0].hdrFormat`, one `rubylight_protocol::hdr` bit); 0 is
    /// stock HDR. `extensions::Offered::check_announce` refuses one not offered.
    #[serde(default)]
    pub hdr_format: u8,
    /// The audio codec the client picked (`x-rl-audio.codec`, one
    /// `rubylight_protocol::audio_ext::CODEC_*` bit); 0 or 1 is Opus.
    #[serde(default)]
    pub audio_codec: u8,
}
impl Default for Negotiated {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            rate_millihz: 0,
            bitrate_kbps: 20000,
            configured_bitrate_kbps: 0,
            csc_mode: 2,
            codec: 0,
            hdr: false,
            sdr_10bit: false,
            yuv444: false,
            chroma_packing: 0,
            slices: 1,
            references: 0,
            intra_refresh: false,
            vrr_low_latency: false,
            pyrowave_records: false,
            pyrowave_client_link_mbps: 0,
            client_wireless: false,
            ltr_recovery: false,
            pyrowave_client_pace_mbps: 0,
            pyrowave_row_order: false,
            packet_size: 1024,
            min_fec: 0,
            audio_channels: 2,
            audio_packet_ms: 5,
            audio_quality: false,
            encryption: 1,
            reliable_control: 13,
            moonlight_features: 0,
            video_qos: true,
            audio_qos: true,
            mic: false,
            hdr_format: 0,
            audio_codec: 0,
        }
    }
}
/// The datagram str0m targets for WebRTC video and how much of it is payload: the RTP header
/// with its extensions and the SRTP tag take the rest. `webrtc::media` packetizes to these.
pub const WEBRTC_DATAGRAM_BYTES: u64 = 1150;
pub const WEBRTC_PAYLOAD_BYTES: u64 = WEBRTC_DATAGRAM_BYTES - 50;
/// The Opus bitrate of a WebRTC audio layout: the protocol's 128 kbps for stereo and
/// GameStream's surround bitrates for 6 and 8 channels.
pub fn webrtc_audio_kbps(channels: u8) -> u32 {
    match channels {
        6 => 256,
        8 => 450,
        _ => 128,
    }
}
impl Negotiated {
    /// What `start-session` negotiates, in place of RTSP's ANNOUNCE: one slice, no FEC (WebRTC
    /// retransmits and the client asks for keyframes), 5 ms Opus packets, and a single
    /// reference frame so the encoder keeps no long-term references for invalidation, which
    /// this transport never uses. `bitrate_kbps` is what the client asked for in all; the
    /// video budget is what is left after the audio layout and the RTP and SRTP overhead.
    pub fn webrtc(
        width: u32,
        height: u32,
        fps: u32,
        codec: crate::webrtc::capabilities::Codec,
        hdr: bool,
        bitrate_kbps: u32,
        channels: u8,
    ) -> Self {
        use crate::webrtc::capabilities::Codec;
        let video = u64::from(bitrate_kbps).saturating_sub(u64::from(webrtc_audio_kbps(channels)))
            * WEBRTC_PAYLOAD_BYTES
            / WEBRTC_DATAGRAM_BYTES;
        Self {
            width,
            height,
            fps,
            bitrate_kbps: video.clamp(1, u64::from(u32::MAX)) as u32,
            configured_bitrate_kbps: bitrate_kbps,
            codec: match codec {
                Codec::H264 => 0,
                Codec::Hevc => 1,
                Codec::Av1 => 2,
            },
            hdr,
            slices: 1,
            references: 1,
            min_fec: 0,
            audio_channels: channels,
            audio_packet_ms: 5,
            // Neither GameStream's AES layer nor its QoS tags apply: DTLS-SRTP encrypts.
            encryption: 0,
            video_qos: false,
            audio_qos: false,
            ..Self::default()
        }
    }
    pub fn coded_size(&self) -> (u32, u32) {
        if crate::packed444::supported(self.chroma_packing) {
            crate::packed444::Geometry::new(self.width, self.height)
                .expect("validated packed geometry")
                .coded_size()
        } else {
            (self.width, self.height)
        }
    }
    pub fn ten_bit(&self) -> bool {
        self.hdr || self.sdr_10bit
    }
    /// An HDR stream with the HLG transfer (ARIB STD-B67) instead of PQ: HLG,
    /// and Dolby Vision 8.4, whose base layer is HLG.
    pub fn hlg(&self) -> bool {
        use rubylight_protocol::hdr;
        self.hdr && self.hdr_format & (hdr::HLG | hdr::DV_84) != 0
    }
    pub fn color_matrix(&self) -> u8 {
        if self.hdr {
            2
        } else {
            match self.csc_mode >> 1 {
                0 => 0,
                2 if self.ten_bit() => 2,
                _ => 1,
            }
        }
    }
    pub fn full_range(&self) -> bool {
        self.csc_mode & 1 != 0
    }
    pub fn fps_millihz(&self) -> u32 {
        if self.rate_millihz > 0 {
            self.rate_millihz
        } else {
            self.fps.saturating_mul(1000)
        }
    }
    pub fn from_sdp(b: &[u8]) -> Result<Self> {
        let mut attrs = BTreeMap::new();
        for l in std::str::from_utf8(b)?.lines() {
            if let Some(a) = l.trim().strip_prefix("a=")
                && let Some((k, v)) = a.split_once(':')
            {
                attrs.insert(k.trim(), v.trim());
            }
        }
        let mut n = Self::default();
        let get = |k: &str, d: u32| -> Result<u32> {
            Ok(match attrs.get(k) {
                Some(s) => s.parse()?,
                None => d,
            })
        };
        n.chroma_packing = u8::try_from(get(crate::packed444::REQUEST_ATTRIBUTE, 0)?)?;
        n.width = get("x-nv-video[0].clientViewportWd", n.width)?;
        n.height = get("x-nv-video[0].clientViewportHt", n.height)?;
        if n.chroma_packing == 0 {
            n.width &= !1;
            n.height &= !1;
        }
        n.fps = get("x-nv-video[0].maxFPS", n.fps)?;
        n.rate_millihz = if n.fps > 1000 {
            n.fps
        } else {
            n.fps.saturating_mul(1000)
        };
        if n.fps > 4000 {
            n.rate_millihz = n.fps;
            n.fps = n.fps.saturating_add(500) / 1000;
        }
        let refresh_x100 = get("x-nv-video[0].clientRefreshRateX100", 0)?;
        if refresh_x100 > 0 && refresh_x100.saturating_add(50) / 100 == n.fps {
            n.rate_millihz = refresh_x100
                .checked_mul(10)
                .ok_or_else(|| anyhow::anyhow!("refresh rate overflow"))?;
        }
        n.bitrate_kbps = get("x-nv-vqos[0].bw.maximumBitrateKbps", n.bitrate_kbps)?;
        n.configured_bitrate_kbps = get("x-ml-video.configuredBitrateKbps", 0)?;
        n.csc_mode = u8::try_from(get("x-nv-video[0].encoderCscMode", 0)?)?;
        n.codec = u8::try_from(get("x-nv-vqos[0].bitStreamFormat", 0)?)?;
        n.hdr = get("x-nv-video[0].dynamicRangeMode", 0)? != 0;
        n.yuv444 = get("x-ss-video[0].chromaSamplingType", 0)? != 0;
        n.slices = get("x-nv-video[0].videoEncoderSlicesPerFrame", 1)?;
        n.references = get("x-nv-video[0].maxNumReferenceFrames", 0)?;
        n.intra_refresh = get(
            "x-ss-video[0].intraRefresh",
            get("x-nv-video[0].enableIntraRefresh", 0)?,
        )? != 0;
        n.vrr_low_latency = get("x-ss-video[0].vrrLowLatency", 0)? != 0;
        n.pyrowave_records = attrs.contains_key("x-ss-video[0].pyrowaveAdaptiveFec")
            || get("x-ss-video[0].pyrowaveFeatures", 0)? & 1 != 0;
        n.pyrowave_client_link_mbps = get("x-ss-video[0].pyrowaveLinkMbps", 0)?;
        n.client_wireless = get("x-rl-video[0].clientWireless", 0)? == 1;
        n.pyrowave_client_pace_mbps = get("x-ss-video[0].pyrowavePaceMbps", 0)?;
        // Read whether or not there is record framing, so a bad value fails either way.
        let row_order = get("x-ss-video[0].pyrowaveRowOrder", 0)?;
        n.pyrowave_row_order = n.pyrowave_records && row_order == 1;
        n.packet_size = get("x-nv-video[0].packetSize", 1024)? as usize;
        n.min_fec = get("x-nv-vqos[0].fec.minRequiredFecPackets", 0)? as usize;
        n.audio_channels = u8::try_from(get("x-nv-audio.surround.numChannels", 2)?)?;
        n.audio_packet_ms = u8::try_from(get("x-nv-aqos.packetDuration", 5)?)?;
        n.audio_quality = get("x-nv-audio.surround.AudioQuality", 0)? != 0;
        n.encryption = get("x-ss-general.encryptionEnabled", 0)?;
        if get("x-nv-general.featureFlags", 0)? & 0x20 != 0 {
            n.encryption |= 4;
        }
        n.reliable_control = get("x-nv-general.useReliableUdp", 13)?;
        n.moonlight_features = get("x-ml-general.featureFlags", 0)?;
        // Tagged unless the client says not to, as in Vibepollo.
        n.video_qos = get("x-nv-vqos[0].qosTrafficType", 5)? != 0;
        n.audio_qos = get("x-nv-aqos.qosTrafficType", 4)? != 0;

        // 3.0 extensions, sent only for what the host offered.
        n.hdr_format = u8::try_from(get(rubylight_protocol::hdr::ANNOUNCE_ATTRIBUTE, 0)?)?;
        n.audio_codec = u8::try_from(get(
            rubylight_protocol::audio_ext::CODEC_ANNOUNCE_ATTRIBUTE,
            0,
        )?)?;
        n.validate()?;
        Ok(n)
    }
    /// What the client asked for in its `/launch` or `/resume` query rather
    /// than in the ANNOUNCE.
    pub fn apply_launch_options(&mut self, options: &BTreeMap<String, String>) {
        use rubylight_protocol::ltr_recovery;
        self.ltr_recovery = ltr_recovery::requested(
            options
                .get(ltr_recovery::LAUNCH_PARAMETER)
                .map(String::as_str),
        );
    }
    pub fn validate(&self) -> Result<()> {
        if self.chroma_packing != 0
            && (!crate::packed444::supported(self.chroma_packing)
                || self.yuv444
                || crate::packed444::mode_bit(self.codec, self.ten_bit()) == 0
                || crate::packed444::Geometry::new(self.width, self.height).is_none())
        {
            bail!("unsupported packed 4:4:4 version, codec, or geometry");
        }
        if !(64..=16384).contains(&self.width)
            || !(64..=16384).contains(&self.height)
            || (self.chroma_packing == 0
                && (!self.width.is_multiple_of(2) || !self.height.is_multiple_of(2)))
        {
            bail!("invalid stream dimensions");
        }
        // AMF can report a 128-pixel minimum even though HEVC requires 192.
        // Reject before probing: submitting the smaller surface can hang VCN.
        if self.chroma_packing != 0 && self.codec == 1 && self.coded_size().0 < 192 {
            bail!("AMD packed HEVC requires a coded width of at least 192 pixels");
        }
        if !(1..=4000).contains(&self.fps)
            || (self.rate_millihz != 0 && self.rate_millihz < 1000)
            || self.rate_millihz > 4_000_000
            || !(1..=2_000_000).contains(&self.bitrate_kbps)
            || self.configured_bitrate_kbps > i32::MAX as u32
            || self.csc_mode > 5
            || self.codec > 3
            || self.slices > 16
            || self.references > 16
        {
            bail!("invalid encoder parameters");
        }
        if !(256..=1400).contains(&self.packet_size) || self.min_fec > 255 {
            bail!("invalid packet size or FEC count");
        }
        // 12 is 7.1.4, which `extensions::Offered::check_announce` refuses unless offered.
        if !matches!(self.audio_channels, 2 | 6 | 8 | 12)
            || !matches!(self.audio_packet_ms, 5 | 10 | 20)
        {
            bail!("unsupported audio layout");
        }
        if self.encryption & !(7 | crate::mic::ENCRYPTION) != 0 {
            bail!("unsupported encryption flags");
        }
        Ok(())
    }
}
/// The Rubylight control messages this host takes, by id: phase lock reports
/// (0x5530), display luminance (0x5531), reconfiguration (0x5532) and USB
/// controller speaker, microphone and status (0x5533–0x5535), and host timing
/// requests (0x5536), which it answers with reports, and clock sync probes (0x5537),
/// which it answers at once, and link notice requests (0x5538), after which it tells the
/// client when it lowers the video bitrate for the network.
pub const CONTROL_MESSAGES: &str =
    "a=x-rl-control:0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537,0x5538\r\n";
/// The `DESCRIBE` answer. `mic` is set when the host takes a client's
/// microphone: its encryption is then supported and requested, and the
/// caller ends the answer with `mic::sdp`, whose `m=` line must come last.
/// `roaming` offers client roaming (`config.client_roaming()`).
#[allow(clippy::too_many_arguments)]
pub fn describe(
    feature_flags: u32,
    supported: u32,
    requested: u32,
    hevc: bool,
    av1: bool,
    pyrowave: bool,
    mic: bool,
    roaming: bool,
) -> String {
    let (supported, requested) = if mic {
        (
            supported | crate::mic::ENCRYPTION,
            requested | crate::mic::ENCRYPTION,
        )
    } else {
        (supported, requested)
    };
    let mut s = format!(
        "a=x-ss-general.featureFlags:{feature_flags}\r\na=x-ss-general.encryptionSupported:{supported}\r\na=x-ss-general.encryptionRequested:{requested}\r\n"
    );
    // Rubylight's own control messages (rubylight-protocol); a client sends them
    // only to a host that lists them.
    s.push_str(CONTROL_MESSAGES);
    // Client roaming, wire format v1; clients that don't know it ignore it.
    if roaming {
        s.push_str(crate::roaming::SDP_ATTRIBUTE);
        s.push_str("\r\n");
    }
    if hevc {
        s.push_str("sprop-parameter-sets=AAAAAU\r\n");
    }
    if av1 {
        s.push_str("a=rtpmap:98 AV1/90000\r\n");
    }
    if pyrowave {
        s.push_str("a=rtpmap:99 PYROWAVE/90000\r\na=x-ss-pyrowave.bitstream:186f0393\r\n");
    }
    s.push_str("a=fmtp:97 surround-params=21101\r\na=fmtp:97 surround-params=642014523\r\na=fmtp:97 surround-params=85301456723\r\na=fmtp:97 surround-params=21101\r\na=fmtp:97 surround-params=660014523\r\na=fmtp:97 surround-params=88001456723\r\n");
    s
}
pub fn codec_warning(preference: &str, codec: u8, pyrowave_available: bool) -> Option<String> {
    if preference != "pyrowave" || codec == 3 {
        return None;
    }
    let selected = ["H.264", "HEVC", "AV1"][codec as usize];
    let reason = if pyrowave_available {
        "the client selected another codec"
    } else {
        "the host did not advertise a usable PyroWave mode"
    };
    Some(format!(
        "PyroWave was selected on the host, but the client negotiated {selected}: {reason}. Quality and latency may differ; use a compatible Nonary Moonlight client and check PyroWave capability-probe warnings, or select HEVC/AV1 intentionally."
    ))
}

#[cfg(test)]
mod tests;
