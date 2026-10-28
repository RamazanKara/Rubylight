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
    audio_ext, clipboard_sync, cursor_shape, fec_report, hdr, keyboard_layout, pen, pipeline_rates,
    rate_change, text_input, usb,
};

pub use rubylight_protocol::fec_report::FecChoice;

/// The first version of every extension's serverinfo element.
const VERSION: u8 = 1;

/// `GET` and `POST /actions/clipboard?type=` value for an image clipboard (PNG).
pub const CLIPBOARD_TYPE_PNG: &str = "png";

/// The `ServerCodecModeSupport` bit of HEVC Main10, which Dolby Vision
/// profile 8 needs.
const HEVC_MAIN10: u32 = 0x200;
/// The `ServerCodecModeSupport` bit of AV1 Main10.
const AV1_MAIN10: u32 = 0x2_0000;

/// The `hdr` formats a host with these `ServerCodecModeSupport` bits offers:
/// HLG on HEVC or AV1 Main10 while `hlg` (`hdr_hlg`) is on, Dolby Vision 8.1
/// on HEVC Main10 while `dolby_vision` (`hdr_dolby_vision`) is on, and Dolby
/// Vision 8.4, which is HLG underneath, while both are, beside stock PQ. PQ
/// alone is stock HDR and is not listed, so a host with nothing beyond it
/// shows stock clients nothing new.
pub fn hdr_offer(codecs: u32, hlg: bool, dolby_vision: bool) -> u8 {
    let hevc = codecs & HEVC_MAIN10 != 0;
    let mut formats = 0;
    if hlg && (hevc || codecs & AV1_MAIN10 != 0) {
        formats |= hdr::HLG;
    }
    if dolby_vision && hevc {
        formats |= hdr::DV_81;
        if hlg {
            formats |= hdr::DV_84;
        }
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
    /// Rate changes (0x553C), in the DESCRIBE of a stream whose client launched with
    /// `rlRateChange=1`; no serverinfo element.
    pub rate_change: bool,
    /// Clipboard sync (0x5508), only on a stream whose launch asked with
    /// `rlClipboardSync=1`, so serverinfo never shows it.
    pub clipboard_sync: bool,
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
        rate_change: false,
        clipboard_sync: false,
    };

    /// The serverinfo elements of what is offered, in a fixed order; none for
    /// [`Offered::NONE`].
    pub fn serverinfo_fields(&self) -> Vec<(&'static str, String)> {
        let version = || VERSION.to_string();
        let mut fields = Vec::new();
        if self.hdr_formats & hdr::ALL != 0 {
            fields.push((
                hdr::SERVERINFO_FIELD,
                (self.hdr_formats & hdr::ALL).to_string(),
            ));
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
        ] {
            if offered {
                fields.push((field, version()));
            }
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
                self.hdr_formats & hdr::ALL
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
            if format & hdr::DOLBY_VISION != 0 && n.codec != 1 {
                bail!("{attribute} {format} (Dolby Vision) needs HEVC");
            }
            if format == hdr::HLG && !matches!(n.codec, 1 | 2) {
                bail!("{attribute} {format} (HLG) needs HEVC or AV1");
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
mod tests {
    use super::*;
    use crate::rtsp::Negotiated;

    mod stock_announce;

    const ALL_ON: Offered = Offered {
        hdr_formats: hdr::ALL,
        client_fec: true,
        bandwidth_probe: true,
        usb: true,
        audio_codecs: audio_ext::CODECS_ALL,
        surround_714: true,
        text_input: true,
        keyboard_layout: true,
        pen_barrel_roll: true,
        cursor_shape: true,
        pipeline_rates: true,
        rate_change: true,
        clipboard_sync: true,
    };

    #[test]
    fn nothing_offered_adds_nothing() {
        assert_eq!(Offered::NONE, Offered::default());
        assert!(Offered::NONE.serverinfo_fields().is_empty());
        assert_eq!(Offered::NONE.describe_lines(), "");
        assert!(Offered::NONE.control_ids().is_empty());
        // Opus alone is what every host already sends.
        let opus = Offered {
            audio_codecs: audio_ext::CODEC_OPUS,
            ..Offered::NONE
        };
        assert!(opus.serverinfo_fields().is_empty());
        assert_eq!(opus.describe_lines(), "");
    }

    #[test]
    fn everything_offered_gives_the_exact_fields_and_lines() {
        let fields: Vec<(&str, String)> = ALL_ON.serverinfo_fields();
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(
            fields,
            [
                ("HdrFormats", "15"),
                ("FecControlVersion", "1"),
                ("BandwidthProbeVersion", "1"),
                ("AudioCodecs", "15"),
                ("AudioLayouts", "15"),
                ("TextInputVersion", "2"),
                ("KeyboardLayoutVersion", "1"),
                ("PenBarrelRollVersion", "1"),
                ("CursorShapeVersion", "1"),
                ("PipelineRatesVersion", "1"),
            ]
        );
        assert_eq!(
            ALL_ON.describe_lines(),
            "a=x-rl-video.hdrFormats:15\r\n\
             a=x-rl-audio.codecs:15\r\n\
             a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-audio.layout714Hq:12,12,0,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-ss-general.penBarrelRoll:1\r\n\
             a=x-rl-control:0x5508,0x5539,0x553a,0x553b,0x553c,0x553d,0x553e,0x5540\r\n"
        );
        assert_eq!(
            ALL_ON.control_ids(),
            [0x5508, 0x5539, 0x553A, 0x553B, 0x553C, 0x553D, 0x553E, 0x5540]
        );
    }

    #[test]
    fn rate_changes_are_a_describe_id_only() {
        let offered = Offered {
            rate_change: true,
            ..Offered::NONE
        };
        assert!(offered.serverinfo_fields().is_empty());
        assert_eq!(offered.describe_lines(), "a=x-rl-control:0x553c\r\n");
        let defaults = Offered {
            client_fec: true,
            pipeline_rates: true,
            ..Offered::NONE
        };
        assert_eq!(
            Offered {
                rate_change: true,
                ..defaults
            }
            .describe_lines(),
            "a=x-rl-control:0x5539,0x553c,0x553d\r\n"
        );
        assert_eq!(
            Offered {
                rate_change: true,
                ..defaults
            }
            .serverinfo_fields(),
            defaults.serverinfo_fields()
        );
    }

    #[test]
    fn partial_offers_keep_their_order_and_add_opus() {
        let offered = Offered {
            audio_codecs: audio_ext::CODEC_PCM,
            cursor_shape: true,
            client_fec: true,
            ..Offered::NONE
        };
        assert_eq!(
            offered.describe_lines(),
            "a=x-rl-audio.codecs:3\r\na=x-rl-control:0x5539,0x5540\r\n"
        );
        let hdr = Offered {
            hdr_formats: hdr::HLG | 0x30,
            ..Offered::NONE
        };
        assert_eq!(hdr.serverinfo_fields(), [("HdrFormats", "2".to_owned())]);
    }

    #[test]
    fn the_new_control_ids_are_not_the_stock_ones() {
        for id in ALL_ON.control_ids() {
            assert!(
                !crate::rtsp::CONTROL_MESSAGES.contains(&format!("{id:#06x}")),
                "{id:#06x}"
            );
        }
    }

    fn announce(sdp: &str) -> Negotiated {
        Negotiated::from_sdp(sdp.as_bytes()).unwrap()
    }

    #[test]
    fn a_stock_announce_passes_whatever_is_offered() {
        for sdp in [
            "",
            "a=x-nv-video[0].dynamicRangeMode:1\n",
            "a=x-nv-audio.surround.numChannels:6\na=x-nv-audio.surround.channelMask:63\n",
            "a=x-nv-audio.surround.numChannels:8\n",
        ] {
            Offered::NONE.check_announce(&announce(sdp)).unwrap();
            ALL_ON.check_announce(&announce(sdp)).unwrap();
        }
        // An explicit Opus pick is stock too.
        Offered::NONE
            .check_announce(&announce("a=x-rl-audio.codec:1\n"))
            .unwrap();
    }

    #[test]
    fn hdr_formats_not_offered_or_not_possible_are_refused() {
        let hdr = "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:1\n";
        let refused = |offered: &Offered, sdp: &str| {
            let error = offered.check_announce(&announce(sdp)).unwrap_err();
            assert!(
                error.to_string().contains("x-rl-video[0].hdrFormat"),
                "{error}"
            );
        };
        refused(
            &Offered::NONE,
            &format!("{hdr}a=x-rl-video[0].hdrFormat:1\n"),
        );
        refused(&ALL_ON, &format!("{hdr}a=x-rl-video[0].hdrFormat:3\n"));
        refused(&ALL_ON, &format!("{hdr}a=x-rl-video[0].hdrFormat:16\n"));
        let pq_only = Offered {
            hdr_formats: hdr::PQ,
            ..Offered::NONE
        };
        refused(&pq_only, &format!("{hdr}a=x-rl-video[0].hdrFormat:2\n"));
        // SDR with a format picked.
        refused(
            &ALL_ON,
            "a=x-nv-vqos[0].bitStreamFormat:1\na=x-rl-video[0].hdrFormat:2\n",
        );
        // Dolby Vision on H.264 or AV1.
        for codec in [0, 2] {
            refused(
                &ALL_ON,
                &format!(
                    "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:{codec}\na=x-rl-video[0].hdrFormat:4\n"
                ),
            );
        }
        for format in [1, 2, 4, 8] {
            ALL_ON
                .check_announce(&announce(&format!(
                    "{hdr}a=x-rl-video[0].hdrFormat:{format}\n"
                )))
                .unwrap();
        }
        pq_only
            .check_announce(&announce(&format!("{hdr}a=x-rl-video[0].hdrFormat:1\n")))
            .unwrap();
    }

    #[test]
    fn hlg_and_dolby_vision_are_offered_by_codec_while_their_settings_are_on() {
        let hevc_main10 = 0x1 | 0x100 | 0x200;
        let av1_main10 = 0x1 | 0x100 | 0x2_0000;
        // HLG on HEVC or AV1 Main10, Dolby Vision on HEVC Main10 only, and 8.4
        // (HLG underneath) only with both.
        for (codecs, hlg, dolby_vision, formats) in [
            (hevc_main10, true, true, hdr::ALL),
            (hevc_main10, false, true, hdr::PQ | hdr::DV_81),
            (hevc_main10, true, false, hdr::PQ | hdr::HLG),
            (av1_main10, true, true, hdr::PQ | hdr::HLG),
            (av1_main10 | 0x200, true, true, hdr::ALL),
        ] {
            assert_eq!(
                hdr_offer(codecs, hlg, dolby_vision),
                formats,
                "{codecs:#x} {hlg} {dolby_vision}"
            );
        }
        // Both off, or no Main10 encoder (AV1 Main10 without HLG): nothing
        // beyond stock HDR, so serverinfo and DESCRIBE stay stock.
        for (codecs, hlg, dolby_vision) in [
            (hevc_main10, false, false),
            (av1_main10, false, true),
            (0x1 | 0x100 | 0x1_0000, true, true),
            (0, true, true),
        ] {
            assert_eq!(hdr_offer(codecs, hlg, dolby_vision), 0, "{codecs:#x}");
            let offered = Offered {
                hdr_formats: hdr_offer(codecs, hlg, dolby_vision),
                ..Offered::NONE
            };
            assert!(offered.serverinfo_fields().is_empty());
            assert!(offered.describe_lines().is_empty());
        }
        let offered = Offered {
            hdr_formats: hdr_offer(hevc_main10, false, true),
            ..Offered::NONE
        };
        assert_eq!(
            offered.serverinfo_fields(),
            [("HdrFormats", "5".to_owned())]
        );
        assert_eq!(offered.describe_lines(), "a=x-rl-video.hdrFormats:5\r\n");
        let hdr = "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:1\n";
        let pick = |offered: &Offered, format: u8| {
            offered.check_announce(&announce(&format!(
                "{hdr}a=x-rl-video[0].hdrFormat:{format}\n"
            )))
        };
        for format in [1, 4] {
            pick(&offered, format).unwrap();
        }
        for format in [2, 8] {
            pick(&offered, format).unwrap_err();
        }
        let all = Offered {
            hdr_formats: hdr_offer(hevc_main10, true, true),
            ..Offered::NONE
        };
        assert_eq!(all.describe_lines(), "a=x-rl-video.hdrFormats:15\r\n");
        for format in [1, 2, 4, 8] {
            pick(&all, format).unwrap();
        }
    }

    #[test]
    fn hlg_needs_hevc_or_av1() {
        let pick = |codec: u8| {
            ALL_ON.check_announce(&announce(&format!(
                "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:{codec}\na=x-rl-video[0].hdrFormat:2\n"
            )))
        };
        for codec in [1, 2] {
            pick(codec).unwrap();
        }
        // H.264 has no HDR here and PyroWave carries its own picture format.
        for codec in [0, 3] {
            let error = pick(codec).unwrap_err().to_string();
            assert!(
                error.contains("(HLG) needs HEVC or AV1"),
                "{codec}: {error}"
            );
        }
    }

    #[test]
    fn audio_codecs_and_layouts_not_offered_or_not_possible_are_refused() {
        let with = |codec: u8, channels: u8| {
            format!("a=x-rl-audio.codec:{codec}\na=x-nv-audio.surround.numChannels:{channels}\n")
        };
        let refused = |offered: &Offered, sdp: &str, needle: &str| {
            let error = offered.check_announce(&announce(sdp)).unwrap_err();
            assert!(error.to_string().contains(needle), "{sdp}: {error}");
        };
        refused(&Offered::NONE, &with(2, 2), "x-rl-audio.codec");
        refused(&ALL_ON, &with(3, 2), "x-rl-audio.codec");
        refused(&ALL_ON, &with(16, 2), "x-rl-audio.codec");
        let pcm = Offered {
            audio_codecs: audio_ext::CODEC_PCM,
            ..Offered::NONE
        };
        refused(&pcm, &with(4, 2), "x-rl-audio.codec");
        refused(&ALL_ON, &with(4, 8), "x-rl-audio.codec");
        refused(&ALL_ON, &with(4, 12), "x-rl-audio.codec");
        refused(&ALL_ON, &with(8, 12), "x-rl-audio.codec");
        // 12 channels without 7.1.4 offered, with Opus or PCM.
        refused(&Offered::NONE, &with(0, 12), "7.1.4");
        refused(&pcm, &with(2, 12), "7.1.4");
        for (codec, channels) in [
            (0, 12),
            (2, 2),
            (2, 6),
            (2, 8),
            (2, 12),
            (4, 2),
            (4, 6),
            (8, 2),
            (8, 6),
            (8, 8),
        ] {
            ALL_ON
                .check_announce(&announce(&with(codec, channels)))
                .unwrap_or_else(|error| panic!("{codec} {channels}: {error}"));
        }
    }

    #[test]
    fn fec_choice_reads_the_launch_parameter() {
        let options = |value: Option<&str>| -> BTreeMap<String, String> {
            value
                .map(|value| ("rlFecPercentage".to_owned(), value.to_owned()))
                .into_iter()
                .collect()
        };
        assert_eq!(fec_choice(&options(None)).unwrap(), FecChoice::Host);
        assert_eq!(fec_choice(&options(Some("host"))).unwrap(), FecChoice::Host);
        assert_eq!(fec_choice(&options(Some("auto"))).unwrap(), FecChoice::Auto);
        assert_eq!(
            fec_choice(&options(Some("0"))).unwrap(),
            FecChoice::Fixed(0)
        );
        assert_eq!(
            fec_choice(&options(Some("255"))).unwrap(),
            FecChoice::Fixed(255)
        );
        for bad in ["256", "-1", "+5", "05x", "", "Auto"] {
            let error = fec_choice(&options(Some(bad))).unwrap_err();
            assert_eq!(error.to_string(), "invalid rlFecPercentage", "{bad}");
        }
    }
}
