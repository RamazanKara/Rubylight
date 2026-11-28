//! RTSP message parsing and responses, and the stream parameters a client
//! negotiates in its `ANNOUNCE`/`SETUP` exchange.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use rubylight_wire::rtsp::{
    CONTROL_MESSAGES, CONTROL_MESSAGES_WITHOUT_LINK_NOTICE, MAX_MESSAGE, MessageBuffer, Request,
    complete_length, describe, describe_with_link_notice, response,
};

/// 2 Gbit/s stays inside them; the negotiation and NVENC's bitrate changes
/// use this one ceiling.
pub const MAX_BITRATE_KBPS: u32 = 2_000_000;
/// `ML_FF_FEC_STATUS` in [`Negotiated::moonlight_features`]
/// (moonlight-common-c `Limelight-internal.h:88`): the client sends
/// `SS_FRAME_FEC_STATUS` (0x5502) for every frame it repaired or lost.
pub const FEC_STATUS_FEATURE: u32 = 0x01;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Negotiated {
    /// The size the encoder codes and the client decodes, after the host's
    /// `max_stream_resolution` cap.
    pub width: u32,
    pub height: u32,
    /// The size the stream's display keeps while the cap shrinks the coded
    /// picture; 0 when the display is coded as it is (no cap applied).
    #[serde(default)]
    pub display_width: u32,
    #[serde(default)]
    pub display_height: u32,
    pub fps: u32,
    #[serde(default)]
    pub rate_millihz: u32,
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub configured_bitrate_kbps: u32,
    /// The FEC share [`crate::stream_policy::apply`] took off the bitrate the
    /// client's user set: `fec_percentage` when the stream launched, 0 above
    /// 80 %. The stream keeps the FEC it started with, so a bitrate asked for
    /// mid-stream takes this share off too, whatever the setting says by then.
    #[serde(default = "default_launch_fec")]
    pub launch_fec: u8,
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
    /// The client negotiated single-PKCS#7 microphone framing (`x-rl-mic.framing:1`)
    /// so block-aligned Opus packets ending in `0x01`/`[k; k]` are never stripped.
    #[serde(default)]
    pub mic_single_pkcs7: bool,
    /// The HDR format the client picked from the host's offer
    /// (`x-rl-video[0].hdrFormat`, one `rubylight_protocol::hdr` bit); 0 is
    /// stock HDR. `extensions::Offered::check_announce` refuses one not offered.
    #[serde(default)]
    pub hdr_format: u8,
    /// The client asked for its Dolby Vision 8.1 stream full range, with RPUs
    /// that say so (`x-rl-video[0].dolbyVisionFullRange:1`).
    #[serde(default)]
    pub dolby_vision_full_range: bool,
    /// The audio codec the client picked (`x-rl-audio.codec`, one
    /// `rubylight_protocol::audio_ext::CODEC_*` bit); 0 or 1 is Opus.
    #[serde(default)]
    pub audio_codec: u8,
    /// The host setting `hdr_sdr_tone_map` is on: an SDR stream of an HDR
    /// desktop compresses highlights with BT.2446 Method B instead of clipping
    /// at SDR white. The encoder sets it, not the client.
    #[serde(default)]
    pub sdr_tone_map: bool,
}
/// `fec_percentage`'s default.
fn default_launch_fec() -> u8 {
    20
}
impl Default for Negotiated {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            display_width: 0,
            display_height: 0,
            fps: 60,
            rate_millihz: 0,
            bitrate_kbps: 20000,
            configured_bitrate_kbps: 0,
            launch_fec: default_launch_fec(),
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
            mic_single_pkcs7: false,
            hdr_format: 0,
            dolby_vision_full_range: false,
            audio_codec: 0,
            sdr_tone_map: false,
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
/// Video codecs offered by the WebRTC guest transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebRtcCodec {
    H264,
    Hevc,
    Av1,
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
        codec: WebRtcCodec,
        hdr: bool,
        bitrate_kbps: u32,
        channels: u8,
    ) -> Self {
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
                WebRtcCodec::H264 => 0,
                WebRtcCodec::Hevc => 1,
                WebRtcCodec::Av1 => 2,
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
        if rubylight_protocol::packed444::supported(self.chroma_packing) {
            rubylight_protocol::packed444::Geometry::new(self.width, self.height)
                .expect("validated packed geometry")
                .coded_size()
        } else {
            (self.width, self.height)
        }
    }
    /// The size the stream's display keeps: the coded size unless a host cap
    /// (`max_stream_resolution`) shrank the picture.
    pub fn display_size(&self) -> (u32, u32) {
        if self.display_width == 0 || self.display_height == 0 {
            (self.width, self.height)
        } else {
            (self.display_width, self.display_height)
        }
    }
    /// Sets the size the client asked for, which the display keeps, and codes
    /// the picture no larger than the host's `cap`
    /// ([`crate::display_policy::capped_encode_size`]). A request the cap
    /// allows is coded as it is.
    pub fn set_display_size(&mut self, requested: (u32, u32), cap: Option<(u32, u32)>) {
        let mut coded = crate::display_policy::capped_encode_size(requested, cap);
        // AMD packed HEVC refuses a coded width below 192 (`validate`): a very
        // tall picture under a cap is widened, never refused for the cap's doing.
        if self.chroma_packing != 0
            && coded != requested
            && let Some((cap_width, _)) = cap
        {
            coded.0 = coded.0.max(192.min(requested.0).min(cap_width));
        }
        self.width = coded.0;
        self.height = coded.1;
        if coded == requested {
            self.display_width = 0;
            self.display_height = 0;
        } else {
            self.display_width = requested.0;
            self.display_height = requested.1;
        }
    }
    /// Whether a host cap makes this stream's coded picture smaller than the
    /// size its display keeps.
    pub fn capped(&self) -> bool {
        self.display_size() != (self.width, self.height)
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
        n.chroma_packing = u8::try_from(get(rubylight_protocol::packed444::REQUEST_ATTRIBUTE, 0)?)?;
        n.width = get("x-nv-video[0].clientViewportWd", n.width)?;
        n.height = get("x-nv-video[0].clientViewportHt", n.height)?;
        if n.chroma_packing == 0 {
            n.width &= !1;
            n.height &= !1;
        }
        let raw_fps = get("x-nv-video[0].maxFPS", n.fps)?;
        let explicit_millihz = raw_fps > 1000;
        if explicit_millihz {
            n.rate_millihz = raw_fps;
            n.fps = raw_fps.saturating_add(500) / 1000;
        } else {
            n.fps = raw_fps;
            n.rate_millihz = raw_fps.saturating_mul(1000);
        }
        let refresh_x100 = get("x-nv-video[0].clientRefreshRateX100", 0)?;
        if !explicit_millihz && refresh_x100 > 0 && refresh_x100.saturating_add(50) / 100 == n.fps {
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
        n.dolby_vision_full_range =
            get(rubylight_protocol::hdr::FULL_RANGE_ANNOUNCE_ATTRIBUTE, 0)? != 0;
        n.audio_codec = u8::try_from(get(
            rubylight_protocol::audio_ext::CODEC_ANNOUNCE_ATTRIBUTE,
            0,
        )?)?;
        let mic_framing = get(rubylight_protocol::mic::FRAMING_ATTRIBUTE, 0)?;
        if mic_framing > u32::from(rubylight_protocol::mic::FRAMING_SINGLE_PKCS7) {
            bail!("unsupported microphone framing mode {mic_framing}");
        }
        n.mic_single_pkcs7 =
            mic_framing == u32::from(rubylight_protocol::mic::FRAMING_SINGLE_PKCS7);
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
            && (!rubylight_protocol::packed444::supported(self.chroma_packing)
                || self.yuv444
                || rubylight_protocol::packed444::mode_bit(self.codec, self.ten_bit()) == 0
                || rubylight_protocol::packed444::Geometry::new(self.width, self.height).is_none())
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
            || !(1..=MAX_BITRATE_KBPS).contains(&self.bitrate_kbps)
            || self.configured_bitrate_kbps > i32::MAX as u32
            || self.csc_mode > 5
            || self.codec > 3
            || self.slices > 16
            || self.references > 16
        {
            bail!("invalid encoder parameters");
        }
        if !(256..=usize::from(rubylight_protocol::video_packet::MAX_VIDEO_PACKET))
            .contains(&self.packet_size)
            || self.min_fec > 255
        {
            bail!("invalid packet size or FEC count");
        }
        // 12 is 7.1.4, which `extensions::Offered::check_announce` refuses unless offered.
        if !matches!(self.audio_channels, 2 | 6 | 8 | 12)
            || !matches!(self.audio_packet_ms, 5 | 10 | 20)
        {
            bail!("unsupported audio layout");
        }
        if self.encryption & !(7 | rubylight_wire::mic::ENCRYPTION) != 0 {
            bail!("unsupported encryption flags");
        }
        Ok(())
    }
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
