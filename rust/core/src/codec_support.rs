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
    /// Native 4:4:4 chroma (H.264 High 4:4:4, HEVC Rext, AV1 High), which
    /// Moonlight asks for with `x-ss-video[0].chromaSamplingType`.
    pub yuv444: bool,
}

impl Codec {
    pub const ALL: [Codec; 5] = [
        Codec::new(0, false, false),
        Codec::new(1, false, false),
        Codec::new(1, true, false),
        Codec::new(2, false, false),
        Codec::new(2, true, false),
    ];
    /// The native 4:4:4 modes moonlight-common-c defines, probed after
    /// [`Codec::ALL`]: H.264 has no 10-bit one.
    pub const NATIVE_444: [Codec; 5] = [
        Codec::new(0, false, true),
        Codec::new(1, false, true),
        Codec::new(1, true, true),
        Codec::new(2, false, true),
        Codec::new(2, true, true),
    ];
    pub const fn new(id: u8, hdr: bool, yuv444: bool) -> Self {
        Self { id, hdr, yuv444 }
    }
    /// The SCM_* bit the host advertises (moonlight-common-c `Limelight.h`).
    pub fn bit(self) -> u32 {
        match (self.id, self.hdr, self.yuv444) {
            (0, _, false) => 1,
            (1, false, false) => 0x100,
            (1, true, false) => 0x200,
            (_, false, false) => 0x10000,
            (_, true, false) => 0x20000,
            // SCM_H264_HIGH8_444; there is no 10-bit H.264 4:4:4 bit.
            (0, false, true) => 0x40000,
            (0, true, true) => 0,
            (1, false, true) => 0x80000,
            (1, true, true) => 0x100000,
            (_, false, true) => 0x200000,
            (_, true, true) => 0x400000,
        }
    }
    /// The 4:2:0 mode of the same codec and bit depth.
    pub fn chroma_420(self) -> Self {
        Self {
            yuv444: false,
            ..self
        }
    }
    pub fn label(self) -> &'static str {
        match (self.id, self.hdr, self.yuv444) {
            (0, _, false) => "H.264",
            (1, false, false) => "HEVC",
            (1, true, false) => "HEVC 10-bit HDR",
            (_, false, false) => "AV1",
            (_, true, false) => "AV1 10-bit HDR",
            (0, false, true) => "H.264 4:4:4",
            (0, true, true) => "H.264 10-bit 4:4:4",
            (1, false, true) => "HEVC 4:4:4",
            (1, true, true) => "HEVC 10-bit 4:4:4",
            (_, false, true) => "AV1 4:4:4",
            (_, true, true) => "AV1 10-bit 4:4:4",
        }
    }
}

/// The native 4:4:4 modes in `flags` (`ServerCodecModeSupport`), named for
/// the console.
pub fn native_444_labels(flags: u32) -> Vec<&'static str> {
    Codec::NATIVE_444
        .iter()
        .filter(|codec| flags & codec.bit() != 0)
        .map(|codec| codec.label())
        .collect()
}

/// Why an AMD host offers no native 4:4:4, as `amf::create` refuses it.
pub const AMF_NATIVE_444: &str =
    "AMD's video encoder supports only 4:2:0; PyroWave can stream 4:4:4";

/// Whether the AMD table says `engine` has an encoder for `codec`. None means
/// the table does not say (older or unidentified hardware): then only the
/// driver's own error explains the gap.
pub fn expected(engine: Engine, codec: Codec) -> Option<bool> {
    match engine {
        Engine::Vce => {
            // VCE encodes H.264; HEVC and 10-bit support depend on the part.
            // The table only documents VCN 2.0 and later, but no VCE part has AV1.
            (codec.id == 2).then_some(false)
        }
        // An unidentified GPU may be anything, including a new one with AV1.
        Engine::Unknown => None,
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
        // AMD's table says nothing about 4:4:4, which VCN never encodes: the
        // encoder's own words say why (NVENC's capability or AMF's 4:2:0).
        Outcome::Failed(error) if codec.yuv444 => {
            format!("{} is not available: {}", codec.label(), error.trim())
        }
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
                    "{} encoding is not available: the AMF driver refused it{}. Driver: {driver}",
                    codec.label(),
                    if adapter.is_empty() {
                        String::new()
                    } else {
                        format!(" on {adapter}")
                    },
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
/// others do not. Codecs the user turned off are not a warning, and neither is
/// a missing native 4:4:4 mode: most GPUs encode 4:2:0 only. None when
/// nothing a user would expect is missing.
pub fn warning(missing: &[Unavailable]) -> Option<String> {
    let failed: Vec<&Unavailable> = missing
        .iter()
        .filter(|m| !m.codec.yuv444 && !m.reason.contains("turned off in the host settings"))
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
            if codec.yuv444 {
                // Not probed: its 4:2:0 mode is missing, or the pass has not
                // reached it.
                format!(
                    "{} is not advertised by this host's encoder check; turn off YUV 4:4:4 on the device to stream 4:2:0.",
                    codec.label()
                )
            } else {
                format!(
                    "{} was not found by the host's encoder check. Check the host console for the encoder status.",
                    codec.label()
                )
            }
        })
}

/// `refusal` for what a client negotiated: `codec` as in RTSP (0 H.264, 1 HEVC,
/// 2 AV1, 3 PyroWave), `ten_bit` for an HDR or 10-bit stream, `yuv444` for a
/// native 4:4:4 mode. H.264, HEVC and AV1 have a probe reason in either chroma.
pub fn refusal_for(missing: &[Unavailable], codec: u8, ten_bit: bool, yuv444: bool) -> String {
    match (codec, ten_bit, yuv444) {
        (0, true, _) => {
            "H.264 has no 10-bit mode on this host; choose HEVC or AV1 for HDR.".to_string()
        }
        (0..=2, ten_bit, yuv444) => refusal(missing, Codec::new(codec, ten_bit, yuv444)),
        _ => "The requested codec is not available on this host; see the host console for the PyroWave and encoder status.".to_string(),
    }
}

#[cfg(test)]
mod tests;
