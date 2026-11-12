//! "Sharper colored text": one choice, Off or Automatic, for full color detail in a stream. Both
//! clients ask here which way a stream gets it: the codec's own 4:4:4 ([`crate::color::offer`])
//! where this device decodes it and the PC encodes it, else packed full chroma
//! ([`crate::full_chroma::before_launch`]) where the PC and this device share a packed mode.
//!
//! Native comes first: it is the codec's own 4:4:4, needs no packing pass on either side, and runs
//! on every engine and any host that encodes it. Packed is the way for decoders that only handle
//! 4:2:0. HDR takes priority over native 4:4:4 (as before), and packs only where the PC and this
//! device have a 10-bit packed mode in common, so a stream never drops HDR for it.
//!
//! The setting is stored in the two keys older versions read (`prefer_yuv444` or Android's
//! `checkbox_yuv444`, and `experimental_packed_444`): Off is both off, Automatic both on. Android
//! asks through JSON over JNI ([`json`]); desktop calls [`decide`] directly.

use crate::color::{self, Refusal, Wants};
use crate::full_chroma::{self, Unavailable};
use rubylight_protocol::packed444::LAYERED_MODES;

/// What a stream's decision is made from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    /// The setting is Automatic.
    pub auto: bool,
    /// HDR is on for this stream (the setting and an HDR display).
    pub hdr: bool,
    /// "Less color banding" (10-bit SDR) is on.
    pub sdr10: bool,
    /// PyroWave is the chosen codec: its 4:4:4 is a codec choice of its own.
    pub pyrowave: bool,
    /// The engine can stream packed full chroma (Android's classic engine cannot).
    pub packed_engine: bool,
    /// The formats the codec choice offers without 4:4:4 or 10-bit SDR.
    pub base: u32,
    /// The 4:4:4 and 10-bit formats this device decodes and draws ([`color::offer`]).
    pub decodable: u32,
    /// Why this device decodes none of the others.
    pub decoder_reason: String,
    /// The PC's codec support (`ServerCodecModeSupport`).
    pub host_scm: u32,
    /// The PC's packed modes, from its serverinfo or its last DESCRIBE; None when not known.
    pub host_packed: Option<u8>,
    /// The PC refused the last stream that asked for packed full chroma.
    pub packed_refused_before: bool,
}

/// How a stream gets sharper colored text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The codec's own 4:4:4: offer `formats` (the base formats with the 4:4:4 ones, and with
    /// 10-bit SDR on the 10-bit 4:4:4 format where both sides have it).
    Native { formats: u32 },
    /// Packed full chroma with these modes (`rubylight_protocol::packed444::mode_bit` bits).
    Packed { modes: u8 },
    /// Without it, and why.
    Without(Why),
}

/// Why a stream goes without sharper colored text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The setting is Off.
    Off,
    /// PyroWave is chosen.
    PyroWave,
    /// HDR is on and the PC and this device have no 10-bit packed mode in common: HDR wins.
    Hdr,
    /// This device decodes no 4:4:4 format and packs none of the PC's modes either, with the
    /// decoder check's reason.
    Decoder(String),
    /// The PC offers no packed mode (or refused it before) where packing was the way left, or it
    /// encodes no 4:4:4 format for this stream.
    Host,
    /// No decoder on this device passed the packed check.
    Device,
    /// The PC and this device each have packed modes, none of them the same.
    NoCommonMode,
}

impl Why {
    /// Whether the player is told, once per PC: a choice of theirs (Off, PyroWave, HDR) needs no word.
    pub fn worth_saying(&self) -> bool {
        !matches!(self, Why::Off | Why::PyroWave | Why::Hdr)
    }

    /// The reason's name on the JSON wire.
    pub fn key(&self) -> &'static str {
        match self {
            Why::Off => "off",
            Why::PyroWave => "pyrowave",
            Why::Hdr => "hdr",
            Why::Decoder(_) => "decoder",
            Why::Host => "host",
            Why::Device => "device",
            Why::NoCommonMode => "no_common_mode",
        }
    }
}

impl From<Unavailable> for Why {
    fn from(value: Unavailable) -> Self {
        match value {
            Unavailable::Host => Why::Host,
            Unavailable::Device => Why::Device,
            Unavailable::NoCommonMode => Why::NoCommonMode,
        }
    }
}

/// How the stream for `input` gets sharper colored text. `device_packed` verifies the packed modes
/// this device decodes; it costs a decode, so it runs only when the packed step needs it.
pub fn decide(input: &Input, device_packed: impl FnOnce() -> u8) -> Pick {
    if !input.auto {
        return Pick::Without(Why::Off);
    }
    if input.pyrowave {
        return Pick::Without(Why::PyroWave);
    }
    // The codec's own 4:4:4. HDR takes priority over it.
    let mut native_refusal = None;
    if !input.hdr {
        let wants = Wants { yuv444: true, sdr10: input.sdr10, hdr: false, packed444: false, pyrowave: false };
        // A PC that would stream AV1 without the 4:4:4 format it encodes only for HEVC streams
        // HEVC instead, as `NvConnection.negotiateVideoFormats` and desktop's offer do.
        let mut base = input.base;
        if color::av1_given_up(base, wants, input.decodable, input.host_scm) != 0 {
            base &= !crate::sdp::VIDEO_FORMAT_AV1_MAIN8;
        }
        let offer = color::offer(base, wants, input.decodable, &input.decoder_reason, input.host_scm);
        match offer.yuv444 {
            Ok(()) => return Pick::Native { formats: offer.formats },
            Err(Refusal::Decoder(reason)) => native_refusal = Some(Why::Decoder(reason)),
            Err(_) => native_refusal = Some(Why::Host),
        }
    }
    let mut packed_refusal = None;
    if input.packed_engine {
        match full_chroma::before_launch(input.host_packed, input.packed_refused_before, device_packed) {
            Ok(modes) if !input.hdr => return Pick::Packed { modes },
            // With HDR only a 10-bit mode both sides have: a PC whose modes are not known before
            // the launch is not asked for packed with HDR on.
            Ok(modes) => {
                let ten_bit = modes & input.host_packed.unwrap_or(0) & LAYERED_MODES;
                return if ten_bit != 0 { Pick::Packed { modes: ten_bit } } else { Pick::Without(Why::Hdr) };
            }
            Err(unavailable) => packed_refusal = Some(Why::from(unavailable)),
        }
    }
    Pick::Without(limit(native_refusal, packed_refusal))
}

/// The side that rules a stream out, from each step's refusal (None: the step did not run). Packed
/// is the way left for a device whose decoder has no 4:4:4 of the codec's own, so a PC that offers
/// no packed mode (or refused it before) is the limit even then; the device is named only where the
/// PC could have streamed it and this device can't take it.
fn limit(native: Option<Why>, packed: Option<Why>) -> Why {
    match (native, packed) {
        (_, Some(Why::Host)) => Why::Host,
        (Some(native), _) => native,
        (None, Some(packed)) => packed,
        (None, None) => Why::Hdr,
    }
}

/// One JSON request in, one JSON answer out, for Android's `SharpColor` (JNI): `{"ok": …}` or
/// `{"error": "invalid", "detail": …}`. The wire contract is `sharp_color_contract.json`, which
/// this crate's test and the app's test both read.
///
/// The request is `{"op": "decide", …}` with [`Input`]'s fields (`host_packed` may be null) and
/// `device_modes`: null until the app has verified them. When the packed step needs them and they
/// are null, the answer is `{"need_device_modes": true}`; the app verifies and asks again with them.
/// Otherwise the answer is one of `{"pick": "native", "formats": n}`, `{"pick": "packed",
/// "modes": n}` and `{"pick": "without", "why": "off" | "pyrowave" | "hdr" | "decoder" | "host" |
/// "device" | "no_common_mode", "detail": text, "tell": bool}`, where `detail` is the decoder's
/// reason (empty for the others) and `tell` says whether the player hears about it.
pub mod json {
    use super::*;
    use serde_json::{json, Value};

    #[derive(serde::Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    enum Request {
        Decide {
            auto: bool,
            hdr: bool,
            sdr10: bool,
            pyrowave: bool,
            packed_engine: bool,
            base: u32,
            decodable: u32,
            #[serde(default)]
            decoder_reason: String,
            host_scm: u32,
            host_packed: Option<u8>,
            packed_refused_before: bool,
            device_modes: Option<u8>,
        },
    }

    fn answer(pick: Pick) -> Value {
        match pick {
            Pick::Native { formats } => json!({"pick": "native", "formats": formats}),
            Pick::Packed { modes } => json!({"pick": "packed", "modes": modes}),
            Pick::Without(why) => {
                let detail = match &why {
                    Why::Decoder(reason) => reason.clone(),
                    _ => String::new(),
                };
                json!({"pick": "without", "why": why.key(), "detail": detail, "tell": why.worth_saying()})
            }
        }
    }

    pub fn call(request: &str) -> String {
        let answer = match serde_json::from_str::<Request>(request) {
            Err(e) => return json!({"error": "invalid", "detail": e.to_string()}).to_string(),
            Ok(Request::Decide {
                auto,
                hdr,
                sdr10,
                pyrowave,
                packed_engine,
                base,
                decodable,
                decoder_reason,
                host_scm,
                host_packed,
                packed_refused_before,
                device_modes,
            }) => {
                let input = Input { auto, hdr, sdr10, pyrowave, packed_engine, base, decodable, decoder_reason, host_scm, host_packed, packed_refused_before };
                let mut asked = false;
                let pick = decide(&input, || {
                    asked = true;
                    device_modes.unwrap_or(0)
                });
                if asked && device_modes.is_none() {
                    json!({"need_device_modes": true})
                } else {
                    answer(pick)
                }
            }
        };
        json!({ "ok": answer }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{YUV444_FORMATS, SDR10_FORMATS, YUV444_10_FORMATS};
    use crate::sdp::*;

    const HEVC: u32 = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265;
    const AV1: u32 = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8;
    const SCM_444: u32 = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_H264_HIGH8_444 | SCM_HEVC_REXT8_444 | SCM_HEVC_REXT10_444;
    const SCM_420: u32 = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10;
    // Packed modes: HEVC 8-bit 1, HEVC 10-bit 2, AV1 8-bit 4, AV1 10-bit 8.
    const HEVC_8: u8 = 1;
    const HEVC_10: u8 = 2;
    const AV1_8: u8 = 4;

    fn auto() -> Input {
        Input {
            auto: true,
            packed_engine: true,
            base: HEVC,
            decodable: YUV444_FORMATS | SDR10_FORMATS | YUV444_10_FORMATS,
            host_scm: SCM_444,
            host_packed: Some(HEVC_8 | HEVC_10),
            ..Input::default()
        }
    }

    /// The decision, and whether the packed check ran.
    fn decided(input: &Input, device: u8) -> (Pick, bool) {
        let mut asked = false;
        let pick = decide(input, || {
            asked = true;
            device
        });
        (pick, asked)
    }

    #[test]
    fn each_case_picks_its_way() {
        let cases: Vec<(&str, Input, u8, Pick, bool)> = vec![
            ("off", Input { auto: false, ..auto() }, HEVC_8, Pick::Without(Why::Off), false),
            ("PyroWave", Input { pyrowave: true, ..auto() }, HEVC_8, Pick::Without(Why::PyroWave), false),
            ("native accepted", auto(), HEVC_8, Pick::Native { formats: HEVC | VIDEO_FORMAT_H265_REXT8_444 }, false),
            (
                "native refused by the decoder, then packed",
                Input { decodable: SDR10_FORMATS, decoder_reason: "no 4:4:4 profile".into(), ..auto() },
                HEVC_8,
                Pick::Packed { modes: HEVC_8 },
                true,
            ),
            (
                "neither side: the PC encodes no 4:4:4 and has no packed mode",
                Input { host_scm: SCM_420, host_packed: Some(0), ..auto() },
                HEVC_8,
                Pick::Without(Why::Host),
                false,
            ),
            (
                "HDR with a 10-bit packed mode on both sides",
                Input { hdr: true, ..auto() },
                HEVC_8 | HEVC_10,
                Pick::Packed { modes: HEVC_10 },
                true,
            ),
            (
                "HDR without a 10-bit packed mode in common: HDR wins",
                Input { hdr: true, ..auto() },
                HEVC_8,
                Pick::Without(Why::Hdr),
                true,
            ),
            (
                "HDR on a PC whose packed modes are not known before the launch",
                Input { hdr: true, host_packed: None, ..auto() },
                HEVC_8 | HEVC_10,
                Pick::Without(Why::Hdr),
                true,
            ),
            (
                "AV1 given up for HEVC RExt",
                Input { base: AV1, host_scm: SCM_444 | SCM_AV1_MAIN8, ..auto() },
                HEVC_8,
                Pick::Native { formats: HEVC | VIDEO_FORMAT_H265_REXT8_444 },
                false,
            ),
            (
                "10-bit SDR with native: RExt 4:4:4 10-bit",
                Input { sdr10: true, ..auto() },
                HEVC_8,
                Pick::Native { formats: HEVC | VIDEO_FORMAT_H265_REXT10_444 },
                false,
            ),
            (
                "the classic engine has no packed path",
                Input { packed_engine: false, decodable: SDR10_FORMATS, decoder_reason: "no 4:4:4 profile".into(), ..auto() },
                HEVC_8,
                Pick::Without(Why::Decoder("no 4:4:4 profile".into())),
                false,
            ),
            (
                "the device's packed check finds nothing: the native refusal is said first",
                Input { host_scm: SCM_420, ..auto() },
                0,
                Pick::Without(Why::Host),
                true,
            ),
            (
                "a decoder without 4:4:4 and a PC without a packed mode: the PC is the limit",
                Input { decodable: SDR10_FORMATS, decoder_reason: "no 4:4:4 profile".into(), host_packed: Some(0), ..auto() },
                HEVC_8,
                Pick::Without(Why::Host),
                false,
            ),
            (
                "a decoder without 4:4:4 and a PC that refused packed before",
                Input { decodable: SDR10_FORMATS, decoder_reason: "no 4:4:4 profile".into(), host_packed: None, packed_refused_before: true, ..auto() },
                HEVC_8,
                Pick::Without(Why::Host),
                false,
            ),
            (
                "a PC that packs and a device that takes neither way: the device is the limit",
                Input { decodable: SDR10_FORMATS, decoder_reason: "no 4:4:4 profile".into(), ..auto() },
                0,
                Pick::Without(Why::Decoder("no 4:4:4 profile".into())),
                true,
            ),
            (
                "no packed mode in common after a native refusal by the decoder",
                Input { decodable: 0, decoder_reason: "no 4:4:4 profile".into(), host_packed: Some(AV1_8), ..auto() },
                HEVC_8,
                Pick::Without(Why::Decoder("no 4:4:4 profile".into())),
                true,
            ),
            (
                "HDR and a PC that offers no packed mode",
                Input { hdr: true, host_packed: Some(0), ..auto() },
                HEVC_8 | HEVC_10,
                Pick::Without(Why::Host),
                false,
            ),
        ];
        for (name, input, device, pick, checked) in cases {
            assert_eq!(decided(&input, device), (pick, checked), "{name}");
        }
    }

    #[test]
    fn only_a_reason_the_player_did_not_choose_is_said() {
        for why in [Why::Off, Why::PyroWave, Why::Hdr] {
            assert!(!why.worth_saying(), "{why:?}");
        }
        for why in [Why::Decoder(String::new()), Why::Host, Why::Device, Why::NoCommonMode] {
            assert!(why.worth_saying(), "{why:?}");
        }
    }

    /// The wire contract with the app: each request as `SharpColor.java` sends it, and the answer.
    /// The app's test reads the same file, so a change on either side of the JNI call fails one.
    #[test]
    fn answers_each_request_of_the_shared_contract_exactly() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("sharp_color_contract.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let request = case["request"].to_string();
            let answer: serde_json::Value = serde_json::from_str(&json::call(&request)).unwrap();
            assert_eq!(answer, case["response"], "{}", case["name"]);
        }
    }

    #[test]
    fn json_names_a_bad_request() {
        let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"nope"}"#)).unwrap();
        assert_eq!(answer["error"], "invalid");
    }
}
