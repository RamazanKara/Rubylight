//! The 3.0 protocol extensions and what the host offers of them.
//!
//! Each extension is advertised by the host (a serverinfo element, a DESCRIBE
//! line or a control message id) and used only when the client opts in (an
//! ANNOUNCE attribute, a launch parameter or a request message). The host
//! crate's `extensions::offered` fills in [`Offered`] for one stream; until a
//! workstream turns its offer on, that is [`Offered::NONE`], and serverinfo and
//! DESCRIBE stay byte for byte what stock Moonlight and older Nightjars read.
//! The wire formats are in `rubylight_protocol`.
use std::collections::BTreeMap;

use anyhow::{Result, bail};
use rubylight_protocol::{
    audio_ext, clipboard_sync, controller_identity, cursor_shape, fec_report, frame_warp, hdr,
    keyboard_layout, pen, pipeline_rates, rate_change, text_input, transport_feedback, usb,
};

pub use rubylight_protocol::fec_report::FecChoice;

/// The first version of every extension's serverinfo element.
const VERSION: u8 = 1;

/// `GET` and `POST /actions/clipboard?type=` value for an image clipboard (PNG).
pub const CLIPBOARD_TYPE_PNG: &str = "png";

/// The `ServerCodecModeSupport` bit of HEVC Main10, which Dolby Vision
/// profile 8 needs.
const HEVC_MAIN10: u32 = 0x200;
/// The `ServerCodecModeSupport` bit of AV1 Main10, which Dolby Vision profile
/// 10 needs.
const AV1_MAIN10: u32 = 0x2_0000;
/// The `ServerCodecModeSupport` bit of PyroWave Main10 (4:2:0), from the
/// separate PyroWave probe.
const PYROWAVE_MAIN10: u32 = 0x0200_0000;

/// The `hdr` formats a host with these `ServerCodecModeSupport` bits offers:
/// HLG on HEVC, AV1 or PyroWave Main10 while `hlg` (`hdr_hlg`) is on, Dolby
/// Vision 8.1 on HEVC Main10 and 10.1 on AV1 Main10 while `dolby_vision`
/// (`hdr_dolby_vision`) is on, and Dolby Vision 8.4, which is HLG underneath,
/// while both are, beside stock PQ. HLG on PyroWave also sets the offer flag
/// `hdr::PYROWAVE_HLG`, which 3.0 rc hosts never sent. PQ alone is stock HDR
/// and is not listed, so a host with nothing beyond it shows stock clients
/// nothing new.
pub fn hdr_offer(codecs: u32, hlg: bool, dolby_vision: bool) -> u8 {
    let hevc = codecs & HEVC_MAIN10 != 0;
    let av1 = codecs & AV1_MAIN10 != 0;
    let mut formats = 0;
    if hlg && (hevc || av1) {
        formats |= hdr::HLG;
    }
    if hlg && codecs & PYROWAVE_MAIN10 != 0 {
        formats |= hdr::HLG | hdr::PYROWAVE_HLG;
    }
    if dolby_vision && hevc {
        formats |= hdr::DV_81;
        if hlg {
            formats |= hdr::DV_84;
        }
    }
    if dolby_vision && av1 {
        formats |= hdr::DV_101;
    }
    if formats == 0 { 0 } else { formats | hdr::PQ }
}

/// What the host offers this client, for serverinfo, DESCRIBE and the checks
/// on what the client then asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offered {
    /// `rubylight_protocol::hdr` bits the host can encode; 0 is stock HDR only.
    pub hdr_formats: u8,
    /// `FecControlVersion`, `rlFecPercentage` and FEC reports (0x5539).
    pub client_fec: bool,
    /// `BandwidthProbeVersion` and `GET /bandwidth-probe`.
    pub bandwidth_probe: bool,
    /// The USB channel (0x553A). The native `usb` feature is listed separately.
    pub usb: bool,
    /// `audio_ext::CODEC_*` bits; Opus is always implied.
    pub audio_codecs: u8,
    /// 7.1.4: `AudioLayouts` bit 8, the `layout714` lines and 12 channels.
    pub surround_714: bool,
    /// `TextInputVersion` and text input (0x553B).
    pub text_input: bool,
    /// `KeyboardLayoutVersion` and keyboard layouts (0x553E).
    pub keyboard_layout: bool,
    /// `PenBarrelRollVersion`, the `penBarrelRoll` line and its input packet.
    pub pen_barrel_roll: bool,
    /// `CursorShapeVersion` and cursor shape requests (0x5540).
    pub cursor_shape: bool,
    /// `PipelineRatesVersion` and pipeline rate requests (0x553D).
    pub pipeline_rates: bool,
    /// `DisplayScaleVersion` and the `rlDisplayScale` launch option.
    pub display_scale: bool,
    /// `SecondScreenVersion` and the `rlSecondScreen` launch option.
    pub second_screen: bool,
    /// `FrameWarpVersion` and Frame Warp's messages (0x5543-0x5546).
    pub frame_warp: bool,
    /// `ControllerIdentityVersion`: controller identity packets (0x55000080)
    /// and the C button flag.
    pub controller_identity: bool,
    /// Rate changes (0x553C), in the DESCRIBE of a stream whose client launched with
    /// `rlRateChange=1`; no serverinfo element.
    pub rate_change: bool,
    /// Clipboard sync (0x5508), only on a stream whose launch asked with
    /// `rlClipboardSync=1`, so serverinfo never shows it.
    pub clipboard_sync: bool,
    /// Per-packet arrival feedback (0x5547) for congestion control, while
    /// `congestion_control` is on; a DESCRIBE id only, no serverinfo element.
    pub transport_feedback: bool,
    /// `NvidiaEnhancements`: the NVIDIA image enhancements the GPU and
    /// driver offer with the NVIDIA component (`nvidia_enhance` bits), which a
    /// launch may ask for with `rlRtxHdr` and `rlRtxVideo`. 0 without it.
    pub nvidia_enhancements: u8,
}

impl Offered {
    /// Nothing offered: what stock clients see.
    pub const NONE: Offered = Offered {
        hdr_formats: 0,
        client_fec: false,
        bandwidth_probe: false,
        usb: false,
        audio_codecs: 0,
        surround_714: false,
        text_input: false,
        keyboard_layout: false,
        pen_barrel_roll: false,
        cursor_shape: false,
        pipeline_rates: false,
        display_scale: false,
        second_screen: false,
        frame_warp: false,
        controller_identity: false,
        rate_change: false,
        clipboard_sync: false,
        transport_feedback: false,
        nvidia_enhancements: 0,
    };

    /// The serverinfo elements of what is offered, in a fixed order; none for
    /// [`Offered::NONE`].
    pub fn serverinfo_fields(&self) -> Vec<(&'static str, String)> {
        let version = || VERSION.to_string();
        let mut fields = Vec::new();
        if self.hdr_formats & hdr::ALL != 0 {
            fields.push((hdr::SERVERINFO_FIELD, self.hdr_bits().to_string()));
        }
        if self.client_fec {
            fields.push((fec_report::SERVERINFO_FIELD, version()));
        }
        if self.bandwidth_probe {
            fields.push((crate::bandwidth_probe::SERVERINFO_FIELD, version()));
        }
        if let Some(codecs) = self.codecs() {
            fields.push((audio_ext::CODECS_SERVERINFO_FIELD, codecs.to_string()));
        }
        if self.surround_714 {
            fields.push((
                audio_ext::LAYOUTS_SERVERINFO_FIELD,
                (audio_ext::LAYOUT_MASK_STEREO
                    | audio_ext::LAYOUT_MASK_51
                    | audio_ext::LAYOUT_MASK_71
                    | audio_ext::LAYOUT_MASK_714)
                    .to_string(),
            ));
        }
        if self.text_input {
            fields.push((
                text_input::SERVERINFO_FIELD,
                text_input::SERVERINFO_VERSION.to_string(),
            ));
        }
        for (offered, field) in [
            (self.keyboard_layout, keyboard_layout::SERVERINFO_FIELD),
            (self.pen_barrel_roll, pen::SERVERINFO_FIELD),
            (self.cursor_shape, cursor_shape::SERVERINFO_FIELD),
            (self.pipeline_rates, pipeline_rates::SERVERINFO_FIELD),
            (
                self.display_scale,
                crate::display_policy::DISPLAY_SCALE_SERVERINFO_FIELD,
            ),
            (
                self.second_screen,
                crate::display_policy::SECOND_SCREEN_SERVERINFO_FIELD,
            ),
            (self.frame_warp, frame_warp::SERVERINFO_FIELD),
            (
                self.controller_identity,
                controller_identity::SERVERINFO_FIELD,
            ),
        ] {
            if offered {
                fields.push((field, version()));
            }
        }
        let nvidia = self.nvidia_enhancements & crate::nvidia_enhance::ALL;
        if nvidia != 0 {
            fields.push((crate::nvidia_enhance::SERVERINFO_FIELD, nvidia.to_string()));
        }
        fields
    }

    /// The DESCRIBE lines of what is offered, appended after the stock answer;
    /// empty for [`Offered::NONE`]. A second `a=x-rl-control:` line lists the
    /// new control messages: clients merge every such line.
    pub fn describe_lines(&self) -> String {
        let mut lines = String::new();
        if self.hdr_formats & hdr::ALL != 0 {
            lines.push_str(&format!(
                "a={}:{}\r\n",
                hdr::DESCRIBE_ATTRIBUTE,
                self.hdr_bits()
            ));
        }
        if let Some(codecs) = self.codecs() {
            lines.push_str(&format!(
                "a={}:{codecs}\r\n",
                audio_ext::CODECS_DESCRIBE_ATTRIBUTE
            ));
        }
        if self.surround_714 {
            lines.extend(audio_ext::layout_714_lines());
        }
        if self.pen_barrel_roll {
            lines.push_str(&format!("a={}:1\r\n", pen::DESCRIBE_ATTRIBUTE));
        }
        let ids = self.control_ids();
        if !ids.is_empty() {
            let ids: Vec<String> = ids.iter().map(|id| format!("{id:#06x}")).collect();
            lines.push_str(&format!("a=x-rl-control:{}\r\n", ids.join(",")));
        }
        lines
    }

    /// The control message ids the host takes for what is offered, ascending.
    pub fn control_ids(&self) -> Vec<u16> {
        let mut ids: Vec<u16> = [
            (self.client_fec, fec_report::FEC_REPORT_MESSAGE_TYPE),
            (self.usb, usb::USB_MESSAGE_TYPE),
            (
                self.clipboard_sync,
                clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE,
            ),
            (self.text_input, text_input::TEXT_INPUT_MESSAGE_TYPE),
            (self.rate_change, rate_change::RATE_CHANGE_MESSAGE_TYPE),
            (
                self.pipeline_rates,
                pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE,
            ),
            (
                self.keyboard_layout,
                keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE,
            ),
            (
                self.cursor_shape,
                cursor_shape::CURSOR_SHAPE_REQUEST_MESSAGE_TYPE,
            ),
            (self.frame_warp, frame_warp::FRAME_WARP_REQUEST_MESSAGE_TYPE),
            (self.frame_warp, frame_warp::FRAME_WARP_MODEL_MESSAGE_TYPE),
            (self.frame_warp, frame_warp::FRAME_WARP_TAGS_MESSAGE_TYPE),
            (self.frame_warp, frame_warp::FRAME_WARP_MASK_MESSAGE_TYPE),
            (
                self.transport_feedback,
                transport_feedback::TRANSPORT_FEEDBACK_MESSAGE_TYPE,
            ),
        ]
        .into_iter()
        .filter_map(|(offered, id)| offered.then_some(id))
        .collect();
        ids.sort_unstable();
        ids
    }

    /// Refuses an ANNOUNCE that picks something the host did not offer, naming
    /// the attribute. Stock clients send none of these, and their 12-channel
    /// layout, which `Negotiated::validate` now admits, is refused here unless
    /// 7.1.4 is offered.
    pub fn check_announce(&self, n: &crate::rtsp::Negotiated) -> Result<()> {
        let format = n.hdr_format;
        if format != 0 {
            let attribute = hdr::ANNOUNCE_ATTRIBUTE;
            if !hdr::single_bit(format) {
                bail!("{attribute} {format} is not a single HDR format");
            }
            if self.hdr_formats & format == 0 {
                bail!("{attribute} {format} was not offered");
            }
            if !n.hdr {
                bail!("{attribute} {format} needs HDR (x-nv-video[0].dynamicRangeMode)");
            }
            if format & hdr::DOLBY_VISION_HEVC != 0 && n.codec != 1 {
                bail!("{attribute} {format} (Dolby Vision profile 8) needs HEVC");
            }
            if format & hdr::DOLBY_VISION_AV1 != 0 && n.codec != 2 {
                bail!("{attribute} {format} (Dolby Vision profile 10) needs AV1");
            }
            if format == hdr::HLG
                && !(matches!(n.codec, 1 | 2)
                    || n.codec == 3 && self.hdr_formats & hdr::PYROWAVE_HLG != 0)
            {
                bail!("{attribute} {format} (HLG) needs HEVC or AV1, or PyroWave where offered");
            }
        }
        let codec = n.audio_codec;
        let attribute = audio_ext::CODEC_ANNOUNCE_ATTRIBUTE;
        if codec > audio_ext::CODEC_OPUS {
            if !codec.is_power_of_two() || self.audio_codecs & codec == 0 {
                bail!("{attribute} {codec} was not offered");
            }
            if !audio_ext::channels_fit(codec, n.audio_channels) {
                bail!(
                    "{attribute} {codec} cannot carry {} channels",
                    n.audio_channels
                );
            }
        }
        if n.audio_channels == audio_ext::CHANNELS_714 && !self.surround_714 {
            bail!("unsupported audio layout: 7.1.4 (12 channels) was not offered");
        }
        Ok(())
    }

    /// The `HdrFormats` and `hdrFormats` mask: the formats and the offer flags.
    fn hdr_bits(&self) -> u8 {
        self.hdr_formats & (hdr::ALL | hdr::OFFER_FLAGS)
    }

    /// The `AudioCodecs` mask, Opus included, when anything besides Opus is offered.
    fn codecs(&self) -> Option<u8> {
        let codecs = self.audio_codecs & audio_ext::CODECS_ALL;
        (codecs & !audio_ext::CODEC_OPUS != 0).then_some(codecs | audio_ext::CODEC_OPUS)
    }
}

/// The client's `rlFecPercentage` launch choice: [`FecChoice::Host`] when it
/// sent none, an error for a value [`FecChoice::parse`] refuses.
pub fn fec_choice(options: &BTreeMap<String, String>) -> Result<FecChoice> {
    match options.get(fec_report::LAUNCH_PARAMETER) {
        None => Ok(FecChoice::Host),
        Some(value) => match FecChoice::parse(value) {
            Some(choice) => Ok(choice),
            None => bail!("invalid {}", fec_report::LAUNCH_PARAMETER),
        },
    }
}

#[cfg(test)]
mod tests;
