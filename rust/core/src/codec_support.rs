//! Why a codec is not advertised on an AMD GPU.
//!
//! The host decides which codecs it advertises by opening a real AMF encoder
//! for each one and encoding a frame (`probe_codecs_once`). That stays the only
//! source of truth: nothing here gates a codec by GPU name. This module words
//! the outcome. When a codec is missing it names the cause (a setting, or the
//! driver's own error) and, from the AMD multimedia table, whether the GPU's
//! video engine is expected to lack it, so a Radeon RX 5000 or 6000 owner reads
//! "AV1 encoding needs a newer video engine" instead of a silent gap or a bare
//! 406 on launch.
//!
//! The generation table is `GPU and APU HW Features and Support` in the AMF
//! wiki (github.com/GPUOpen-LibrariesAndSDKs/AMF). It is untested on hardware
//! other than the RX 7900 XT and the Radeon 780M, and it only ever explains a
//! failed probe; a codec the probe finds is advertised whatever the table says.
//!
//! The file uses only `std`.

/// The video-engine generation a Radeon name implies. Only used for wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Polaris, Vega and earlier GCN (VCE 3.x and 4.x).
    Vce,
    /// Navi 10 and 14, Cezanne (VCN 2.0).
    Vcn2,
    /// Navi 21 to 23, Rembrandt, Raphael (VCN 3.x).
    Vcn3,
    /// Navi 24: VCN 3.0 with the encoder absent.
    DecodeOnly,
    /// Navi 31 to 33, Phoenix, Hawk Point (VCN 4.0).
    Vcn4,
    /// Navi 44 and 48 (VCN 5.0).
    Vcn5,
    /// A name that does not say (for example "AMD Radeon(TM) Graphics").
    Unknown,
}

impl Engine {
    /// What the AMD table says about this engine's encoder, as a phrase.
    pub fn label(self) -> &'static str {
        match self {
            Self::Vce => "VCE (Polaris/Vega)",
            Self::Vcn2 => "VCN 2.0 (RDNA 1)",
            Self::Vcn3 => "VCN 3.x (RDNA 2)",
            Self::DecodeOnly => "VCN 3.0 without an encoder (Navi 24)",
            Self::Vcn4 => "VCN 4.0 (RDNA 3)",
            Self::Vcn5 => "VCN 5.0 (RDNA 4)",
            Self::Unknown => "an unidentified video engine",
        }
    }
}

/// The video engine implied by an adapter name such as
/// `AMD Radeon RX 6800 XT` or `AMD Radeon(TM) 780M Graphics`.
pub fn engine(name: &str) -> Engine {
    let upper = name.to_ascii_uppercase();
    let tokens: Vec<&str> = upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    // The model number is the first token of 3 or 4 digits (optionally with a
    // letter suffix such as 780M or 7900XT), or a Vega/Polaris name.
    if tokens.iter().any(|t| *t == "VEGA" || t.starts_with("VEGA")) {
        return Engine::Vce;
    }
    let model = tokens.iter().find_map(|t| {
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        let rest = &t[digits.len()..];
        (matches!(digits.len(), 3 | 4) && rest.len() <= 2).then(|| (digits, rest.to_string()))
    });
    let Some((digits, suffix)) = model else {
        return Engine::Unknown;
    };
    let number: u32 = digits.parse().unwrap_or(0);
    let rx = tokens.contains(&"RX") || tokens.iter().any(|t| t.starts_with("RX") && t.len() > 2);
    if digits.len() == 3 {
        // iGPU names: 610M (RDNA 2), 660M/680M (RDNA 2), 740M/760M/780M (RDNA 3),
        // 840M/860M/880M/890M (RDNA 3.5). RX 4xx/5xx are Polaris.
        if suffix == "M" {
            return match number {
                610 | 660 | 680 => Engine::Vcn3,
                740 | 760 | 780 | 840 | 860 | 880 | 890 => Engine::Vcn4,
                _ => Engine::Unknown,
            };
        }
        if rx && (400..600).contains(&number) {
            return Engine::Vce;
        }
        return Engine::Unknown;
    }
    if !rx {
        return Engine::Unknown;
    }
    match number {
        5300..=5799 => Engine::Vcn2,
        6300..=6599 => Engine::DecodeOnly,
        6600..=6999 => Engine::Vcn3,
        7000..=7999 => Engine::Vcn4,
        9000..=9999 => Engine::Vcn5,
        _ => Engine::Unknown,
    }
}

/// An AMF version as `major.minor.release.build`, from the packed 64-bit value
/// `AMFInit` and `AMFQueryVersion` use.
pub fn amf_version_text(version: u64) -> String {
    format!(
        "{}.{}.{}.{}",
        version >> 48,
        (version >> 32) & 0xffff,
        (version >> 16) & 0xffff,
        version & 0xffff
    )
}

/// The version to retry `AMFInit` with after it refused `header`. The runtime
/// accepts a header no newer than itself, so an older Radeon driver (a Polaris
/// or Vega card on a legacy driver branch) can refuse this host's SDK version
/// while still encoding. Only an older runtime of the same major version
/// qualifies; a newer or equal one is a different failure and not retried.
pub fn amf_retry_version(header: u64, runtime: Option<u64>) -> Option<u64> {
    runtime.filter(|r| *r != 0 && *r < header && (r >> 48) == (header >> 48))
}

/// One codec the probe tries, in `ServerCodecModeSupport` terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Codec {
    /// 0 H.264, 1 HEVC, 2 AV1.
    pub id: u8,
    /// 10-bit HDR (Main10 / AV1 10-bit).
    pub hdr: bool,
}

impl Codec {
    pub const ALL: [Codec; 5] = [
        Codec { id: 0, hdr: false },
        Codec { id: 1, hdr: false },
        Codec { id: 1, hdr: true },
        Codec { id: 2, hdr: false },
        Codec { id: 2, hdr: true },
    ];
    /// The SCM_* bit the host advertises.
    pub fn bit(self) -> u32 {
        match (self.id, self.hdr) {
            (0, _) => 1,
            (1, false) => 0x100,
            (1, true) => 0x200,
            (2, false) => 0x10000,
            _ => 0x20000,
        }
    }
    pub fn label(self) -> &'static str {
        match (self.id, self.hdr) {
            (0, _) => "H.264",
            (1, false) => "HEVC",
            (1, true) => "HEVC 10-bit HDR",
            (2, false) => "AV1",
            _ => "AV1 10-bit HDR",
        }
    }
}

/// Whether the AMD table says `engine` has an encoder for `codec`. None means
/// the table does not say (older or unidentified hardware): then only the
/// driver's own error explains the gap.
pub fn expected(engine: Engine, codec: Codec) -> Option<bool> {
    match engine {
        Engine::Unknown | Engine::Vce => {
            // VCE encodes H.264; HEVC and 10-bit support depend on the part.
            // The table only documents VCN 2.0 and later.
            (codec.id == 2).then_some(false)
        }
        Engine::DecodeOnly => Some(false),
        Engine::Vcn2 | Engine::Vcn3 => Some(codec.id != 2),
        Engine::Vcn4 | Engine::Vcn5 => Some(true),
    }
}

/// What the probe found for one codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Available,
    /// The user's `hevc_mode` / `av1_mode` setting turned it off.
    Disabled,
    /// The encoder failed to open or produced no packet: the driver's text.
    Failed(String),
}

/// A codec the host does not advertise, with the reason shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable {
    pub codec: Codec,
    pub reason: String,
}

/// The reason `codec` is not advertised, given the probe `outcome` on
/// `adapter`. None when it is available.
pub fn explain(adapter: &str, codec: Codec, outcome: &Outcome) -> Option<Unavailable> {
    let reason = match outcome {
        Outcome::Available => return None,
        Outcome::Disabled => format!(
            "{} is turned off in the host settings (HEVC/AV1 mode).",
            codec.label()
        ),
        Outcome::Failed(error) => {
            let engine = engine(adapter);
            let driver = error.trim();
            match expected(engine, codec) {
                Some(false) => format!(
                    "{} encoding is not available on this GPU: {} has no {} encoder in AMD's multimedia table. Driver: {driver}",
                    codec.label(),
                    engine.label(),
                    match codec.id {
                        0 => "H.264",
                        1 => "HEVC",
                        _ => "AV1",
                    },
                ),
                Some(true) => format!(
                    "{} encoding failed although {} should support it. Update the Radeon driver; if it persists, report it with the host log. Driver: {driver}",
                    codec.label(),
                    engine.label(),
                ),
                None => format!(
                    "{} encoding is not available: the AMF driver refused it on {adapter}. Driver: {driver}",
                    codec.label()
                ),
            }
        }
    };
    Some(Unavailable { codec, reason })
}

/// The unavailable codecs for a full probe pass, in `Codec::ALL` order.
pub fn explain_all(adapter: &str, outcomes: &[(Codec, Outcome)]) -> Vec<Unavailable> {
    outcomes
        .iter()
        .filter_map(|(codec, outcome)| explain(adapter, *codec, outcome))
        .collect()
}

/// The one-line console warning for a pass where some standard codecs work and
/// others do not. Codecs the user turned off are not a warning. None when
/// nothing a user would expect is missing.
pub fn warning(missing: &[Unavailable]) -> Option<String> {
    let failed: Vec<&Unavailable> = missing
        .iter()
        .filter(|m| !m.reason.contains("turned off in the host settings"))
        .collect();
    if failed.is_empty() {
        return None;
    }
    Some(
        failed
            .iter()
            .map(|m| m.reason.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Why a launch is refused when the client asks for `codec` and the host does
/// not advertise it: the probe's reason when one is known.
pub fn refusal(missing: &[Unavailable], codec: Codec) -> String {
    missing
        .iter()
        .find(|m| m.codec == codec)
        .map(|m| m.reason.clone())
        .unwrap_or_else(|| {
            format!(
                "{} was not found by the host's encoder check. Check the host console for the encoder status.",
                codec.label()
            )
        })
}

/// `refusal` for what a client negotiated: `codec` as in RTSP (0 H.264, 1 HEVC,
/// 2 AV1, 3 PyroWave), `ten_bit` for an HDR or 10-bit stream, `yuv444` for a
/// full-chroma mode. Only the 4:2:0 H.264/HEVC/AV1 modes have a probe reason.
pub fn refusal_for(missing: &[Unavailable], codec: u8, ten_bit: bool, yuv444: bool) -> String {
    match (codec, ten_bit, yuv444) {
        (0, true, _) => {
            "H.264 has no 10-bit mode on this host; choose HEVC or AV1 for HDR.".to_string()
        }
        (0..=2, _, true) => {
            "4:4:4 chroma is not advertised by this host's encoder check; stream 4:2:0 instead."
                .to_string()
        }
        (0..=2, ten_bit, false) => refusal(missing, Codec { id: codec, hdr: ten_bit }),
        _ => "The requested codec is not available on this host; see the host console for the PyroWave and encoder status.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake probe for one GPU: the codecs its engine encodes succeed, the
    /// rest fail with the driver's text.
    fn pass(_adapter: &str, found: &[Codec]) -> Vec<(Codec, Outcome)> {
        Codec::ALL
            .iter()
            .map(|c| {
                (
                    *c,
                    if found.contains(c) {
                        Outcome::Available
                    } else {
                        Outcome::Failed("AMF_NOT_SUPPORTED (10)".into())
                    },
                )
            })
            .collect()
    }
    const H264: Codec = Codec { id: 0, hdr: false };
    const HEVC: Codec = Codec { id: 1, hdr: false };
    const HEVC10: Codec = Codec { id: 1, hdr: true };
    const AV1: Codec = Codec { id: 2, hdr: false };
    const AV110: Codec = Codec { id: 2, hdr: true };

    #[test]
    fn names_map_to_engines() {
        for (name, want) in [
            ("AMD Radeon RX 580 Series", Engine::Vce),
            ("Radeon RX 480 Graphics", Engine::Vce),
            ("Radeon RX Vega 64", Engine::Vce),
            ("Radeon Vega 8 Graphics", Engine::Vce),
            ("AMD Radeon RX 5700 XT", Engine::Vcn2),
            ("AMD Radeon RX 5500 XT", Engine::Vcn2),
            ("AMD Radeon RX 6800 XT", Engine::Vcn3),
            ("AMD Radeon RX 6600", Engine::Vcn3),
            ("AMD Radeon RX 6400", Engine::DecodeOnly),
            ("AMD Radeon RX 6500 XT", Engine::DecodeOnly),
            ("AMD Radeon(TM) 680M", Engine::Vcn3),
            ("AMD Radeon RX 7900 XT", Engine::Vcn4),
            ("AMD Radeon RX 7600", Engine::Vcn4),
            ("AMD Radeon(TM) 780M Graphics", Engine::Vcn4),
            ("AMD Radeon RX 9070 XT", Engine::Vcn5),
            ("AMD Radeon RX 9060 XT", Engine::Vcn5),
            ("AMD Radeon(TM) Graphics", Engine::Unknown),
            ("AMD Radeon Pro W6800", Engine::Unknown),
        ] {
            assert_eq!(engine(name), want, "{name}");
        }
    }

    #[test]
    fn rdna1_lacks_av1_and_explains_it() {
        let name = "AMD Radeon RX 5700 XT";
        let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].codec, AV1);
        assert!(missing[0].reason.contains("no AV1 encoder"));
        assert!(missing[0].reason.contains("VCN 2.0"));
        assert!(missing[0].reason.contains("AMF_NOT_SUPPORTED"));
        assert_eq!(missing[1].codec, AV110);
        assert!(warning(&missing).unwrap().contains("AV1"));
    }

    #[test]
    fn rdna2_lacks_av1() {
        let name = "AMD Radeon RX 6800 XT";
        let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
        assert!(missing.iter().all(|m| m.codec.id == 2));
        assert!(missing[0].reason.contains("VCN 3.x"));
    }

    #[test]
    fn rdna3_and_rdna4_fail_loudly_when_a_supported_codec_fails() {
        for name in ["AMD Radeon RX 7900 XT", "AMD Radeon RX 9070 XT"] {
            let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10, AV1]));
            assert_eq!(missing.len(), 1);
            assert_eq!(missing[0].codec, AV110);
            assert!(missing[0].reason.contains("should support it"), "{name}");
            assert!(missing[0].reason.contains("Update the Radeon driver"));
        }
    }

    #[test]
    fn all_codecs_present_means_no_warning() {
        let name = "AMD Radeon RX 7900 XT";
        let missing = explain_all(name, &pass(name, &Codec::ALL));
        assert!(missing.is_empty());
        assert_eq!(warning(&missing), None);
    }

    #[test]
    fn navi24_has_no_encoder_at_all() {
        let name = "AMD Radeon RX 6400";
        let missing = explain_all(name, &pass(name, &[]));
        assert_eq!(missing.len(), 5);
        assert!(missing[0].reason.contains("no H.264 encoder"));
        assert!(missing[0].reason.contains("Navi 24"));
    }

    #[test]
    fn legacy_and_unknown_engines_report_the_driver_text_only() {
        for name in ["AMD Radeon RX 580 Series", "AMD Radeon(TM) Graphics"] {
            let missing = explain_all(name, &pass(name, &[H264, HEVC]));
            let hdr = missing.iter().find(|m| m.codec == HEVC10).unwrap();
            assert!(hdr.reason.contains("AMF driver refused"), "{name}");
            let av1 = missing.iter().find(|m| m.codec == AV1).unwrap();
            assert!(av1.reason.contains("no AV1 encoder"), "{name}");
        }
    }

    #[test]
    fn a_setting_is_not_a_warning() {
        let name = "AMD Radeon RX 7900 XT";
        let outcomes: Vec<_> = Codec::ALL
            .iter()
            .map(|c| {
                (
                    *c,
                    if c.id == 2 {
                        Outcome::Disabled
                    } else {
                        Outcome::Available
                    },
                )
            })
            .collect();
        let missing = explain_all(name, &outcomes);
        assert_eq!(missing.len(), 2);
        assert!(missing[0].reason.contains("turned off"));
        assert_eq!(warning(&missing), None);
    }

    #[test]
    fn launch_refusal_names_the_probe_reason() {
        let name = "AMD Radeon RX 6800 XT";
        let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
        assert!(refusal(&missing, AV1).contains("no AV1 encoder"));
        // A codec the probe never tried (for example a later pass) still gets
        // a sentence, not an empty reason.
        assert!(refusal(&[], AV1).contains("AV1 was not found"));
    }

    #[test]
    fn negotiated_refusals_are_specific() {
        let name = "AMD Radeon RX 6800 XT";
        let missing = explain_all(name, &pass(name, &[H264, HEVC, HEVC10]));
        assert!(refusal_for(&missing, 2, false, false).contains("no AV1 encoder"));
        assert!(refusal_for(&missing, 2, true, false).contains("AV1 10-bit HDR"));
        assert!(refusal_for(&missing, 0, true, false).contains("no 10-bit"));
        assert!(refusal_for(&missing, 1, false, true).contains("4:4:4"));
        assert!(refusal_for(&missing, 3, false, false).contains("host console"));
    }

    #[test]
    fn amf_versions_print_and_retry_only_for_an_older_runtime() {
        let header = (1u64 << 48) | (5 << 32) | (2 << 16);
        let old = (1u64 << 48) | (4 << 32) | (29 << 16) | 1;
        assert_eq!(amf_version_text(header), "1.5.2.0");
        assert_eq!(amf_version_text(old), "1.4.29.1");
        assert_eq!(amf_retry_version(header, Some(old)), Some(old));
        assert_eq!(amf_retry_version(header, Some(header)), None);
        assert_eq!(amf_retry_version(header, Some(header + (1 << 32))), None);
        assert_eq!(amf_retry_version(header, Some(0)), None);
        assert_eq!(amf_retry_version(header, None), None);
        // A different major version is not assumed compatible.
        assert_eq!(amf_retry_version(header, Some(1 << 40)), None);
    }

    #[test]
    fn bits_match_the_advertised_flags() {
        let bits: Vec<u32> = Codec::ALL.iter().map(|c| c.bit()).collect();
        assert_eq!(bits, [1, 0x100, 0x200, 0x10000, 0x20000]);
    }
}
