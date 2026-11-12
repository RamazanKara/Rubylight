//! "Sharper colored text" at a stream's start: decided before the launch
//! (`nightjar_core::sharp_color::decide`) from what this device decodes, the PC's codec support
//! and packed modes and this device's packed decoders (`nightjar_core::full_chroma`). The codec's
//! own 4:4:4 comes first, packed full chroma second; a stream that can have neither starts
//! without it and says so once per PC, and a stream the PC refused packed after the launch starts
//! once more without it. Android's `SharpColor`, `PackedFallback` and `HostPackedModes`.

use crate::config::{HostRecord, Settings, NOTICE_PACKED_UNAVAILABLE};
use crate::strings::{en, tr};
use nightjar_core::sharp_color::{self, Input, Pick, Why};

/// The start's notice when the PC rules sharper colored text out, or refused it after the launch.
pub const HOST_NOTICE: &str = en("Sharper colored text isn’t available from this PC. Starting without it.");
/// The start's notice when this device's decoders rule it out.
pub const DEVICE_NOTICE: &str = en("Sharper colored text isn’t available on this device. Starting without it.");

/// The decoder's reason, for the log, when Linux has no GPU renderer to draw either way with.
const NO_GPU_RENDERER: &str = "no GPU renderer";

/// The notice for `why`, untranslated; None for the player's own choices (Off, PyroWave, HDR),
/// which go without a word.
pub fn notice(why: &Why) -> Option<&'static str> {
    match why {
        Why::Decoder(_) | Why::Device => Some(DEVICE_NOTICE),
        Why::Host | Why::NoCommonMode => Some(HOST_NOTICE),
        Why::Off | Why::PyroWave | Why::Hdr => None,
    }
}

/// "Sharper colored text" is Automatic: it is stored in two keys, and either one asks (settings
/// saved before the two became one may hold only one of them).
pub fn automatic(settings: &Settings) -> bool {
    settings.prefer_yuv444 || settings.experimental_packed_444
}

/// The settings the decoder checks run with before [`decide`]: Automatic tries the codec's own
/// 4:4:4 first, so they check it (`stream::probes_color`) and the plain formats
/// (`stream::base_probe_formats`) as a stream without packing has them.
pub fn native_probe_settings(settings: &Settings) -> Settings {
    Settings { prefer_yuv444: automatic(settings), experimental_packed_444: false, ..settings.clone() }
}

/// What the launch knows before it decides, besides `settings` and the PC's record.
#[derive(Debug)]
pub struct Known<'a> {
    /// The PC's packed modes in its serverinfo.
    pub serverinfo_modes: Option<u8>,
    /// The PC's codec support (`ServerInfo::trusted_codec_mode_support`).
    pub host_scm: u32,
    /// The 4:4:4 and 10-bit formats this device decodes and draws, and why not the others
    /// (`stream::color_probe::probe` with [`native_probe_settings`]).
    pub color_decodable: &'a (u32, String),
    /// The plain formats the stream's decoder decodes (`stream::color_probe::probe_base`).
    pub hardware_decodable: u32,
    /// Linux without the GPU renderer ([`gpu_check`]): neither way can be drawn.
    pub renderer_missing: bool,
}

/// What a launch asks for.
#[derive(Debug, PartialEq, Eq)]
pub struct Decision {
    /// How the stream gets sharper colored text.
    pub pick: Pick,
    /// The packed modes this device offers in the handshake; 0 without full chroma.
    pub modes: u8,
    /// What the start says about it.
    pub note: Option<String>,
    /// `record` changed and is saved.
    pub record_changed: bool,
}

/// Decides sharper colored text for a launch of `settings` on the PC of `record`, and leaves the
/// pick in `settings` for the rest of the start: the codec's own 4:4:4 (`prefer_yuv444` alone),
/// packed full chroma (`experimental_packed_444` alone, with [`Decision::modes`]) or neither.
/// `device_modes` checks the packed modes this device's decoders pass (and what to say when none
/// did); it costs a decode, so it runs only when the packed step needs it. `refused_now`: the PC
/// refused packed after this start's launch, and the start runs once more without it (the native
/// step had already ruled itself out). A decision before the launch is said once per PC, until a
/// stream there starts with it or the setting is switched; a refusal after the launch is always said.
pub fn decide(settings: &mut Settings, record: &mut HostRecord, known: &Known, refused_now: bool,
    device_modes: impl FnOnce() -> (u8, Option<String>)) -> Decision {
    let auto = automatic(settings);
    if refused_now && auto {
        eprintln!("nightjar: full chroma: the PC refused it after the launch; starting again without it");
        (settings.prefer_yuv444, settings.experimental_packed_444) = (false, false);
        return Decision { pick: Pick::Without(Why::Host), modes: 0, note: Some(tr(HOST_NOTICE).to_owned()), record_changed: false };
    }
    let memory = &record.full_chroma;
    let (host_packed, refused_before) = (memory.known(known.serverinfo_modes), memory.refused_before(known.serverinfo_modes));
    let (decodable, decoder_reason) = if known.renderer_missing {
        (0, NO_GPU_RENDERER.to_owned())
    } else {
        known.color_decodable.clone()
    };
    let input = Input {
        auto,
        hdr: settings.hdr,
        sdr10: settings.sdr_10bit,
        pyrowave: settings.codec.is_pyrowave(),
        packed_engine: true,
        base: crate::stream::plain_formats(settings.codec, settings.hdr, known.host_scm, known.hardware_decodable),
        decodable,
        decoder_reason,
        host_scm: known.host_scm,
        host_packed,
        packed_refused_before: refused_before,
    };
    let mut device_note = None;
    let pick = sharp_color::decide(&input, || {
        let (modes, note) = device_modes();
        device_note = note;
        modes
    });
    let mut record_changed = false;
    let (native, packed, modes, note) = match &pick {
        Pick::Native { formats } => {
            eprintln!("nightjar: sharper colored text: the codec's own 4:4:4, offering {formats:#x}");
            // It works on this PC again: should it stop, the start says so again.
            record_changed = record.reset_notice(NOTICE_PACKED_UNAVAILABLE);
            (true, false, 0, None)
        }
        Pick::Packed { modes } => {
            eprintln!("nightjar: sharper colored text: packed full chroma, modes {modes:#x}");
            (false, true, *modes, None)
        }
        Pick::Without(Why::Off) => (false, false, 0, None),
        Pick::Without(why) => {
            let from = if known.serverinfo_modes.is_some() { "serverinfo" } else if refused_before { "a refusal before" } else { "last stream" };
            eprintln!("nightjar: sharper colored text: starting without it ({why:?}): PC packed modes {} ({from}), 4:4:4 decodable {:#x}",
                host_packed.map_or_else(|| "unknown".to_owned(), |modes| format!("{modes:#x}")), input.decodable);
            let note = match notice(why) {
                Some(text) if record.claim_notice(NOTICE_PACKED_UNAVAILABLE) => {
                    record_changed = true;
                    // The packed check's own reason, such as a packed picture larger than the decoder opens, says more.
                    Some(device_note.unwrap_or_else(|| tr(text).to_owned()))
                }
                _ => None,
            };
            (false, false, 0, note)
        }
    };
    (settings.prefer_yuv444, settings.experimental_packed_444) = (native, packed);
    Decision { pick, modes, note, record_changed }
}

/// Linux's check of what needs the GPU renderer, before a stream's other checks: `missing` says
/// what to tell when a need finds none. Sharper colored text does not refuse the stream: both its
/// ways are left out, Ok(true), and [`decide`] starts without it as on a decoder without it. Any
/// other need that refuses without the renderer (`handheld::refused_without_gpu`) does, with
/// `missing`'s words.
#[cfg_attr(windows, allow(dead_code))]
pub fn gpu_check(settings: &Settings, missing: impl Fn(&'static str) -> Option<String>) -> Result<bool, String> {
    let need = if settings.experimental_packed_444 { "packed full chroma" } else { "4:4:4" };
    let left_out = automatic(settings) && missing(need).is_some();
    let rest = Settings {
        experimental_packed_444: settings.experimental_packed_444 && !left_out,
        prefer_yuv444: settings.prefer_yuv444 && !left_out,
        ..settings.clone()
    };
    match rest.gpu_need().filter(|&need| crate::handheld::refused_without_gpu(need)).and_then(missing) {
        Some(advice) => Err(advice),
        None => Ok(left_out),
    }
}

/// A stream started, `packed` with full chroma and the modes its DESCRIBE offered: they are kept
/// for the next launch, a refusal ends, and the notice is said again should it stop working
/// there. True when `record` changed.
pub fn started(record: &mut HostRecord, packed: bool, describe_modes: u8) -> bool {
    if !packed {
        return false;
    }
    let kept = record.full_chroma.started(describe_modes);
    record.reset_notice(NOTICE_PACKED_UNAVAILABLE) | kept
}

/// The PC refused full chroma after a launch. True when `record` changed.
pub fn refused(record: &mut HostRecord) -> bool {
    record.full_chroma.refused()
}

/// A launch on the PC of `record`, from this client at `client_version` to a PC that says
/// `host_version`: a refusal given under other versions ends, so the PC is asked again after
/// either side was updated (Android's `HostPackedModes.refused`). True when `record` changed.
pub fn renew(record: &mut HostRecord, client_version: &str, host_version: Option<&str>) -> bool {
    record.full_chroma.renew(&nightjar_core::full_chroma::stamp(client_version, host_version))
}

/// The user switched full chroma: a PC that refused it is asked again, and a start without it
/// says so again. True when `record` changed.
pub fn ask_again(record: &mut HostRecord) -> bool {
    let asked = record.full_chroma.ask_again();
    record.reset_notice(NOTICE_PACKED_UNAVAILABLE) | asked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::color_probe::BASE_FORMATS;
    use nightjar_core::color::{SDR10_FORMATS, YUV444_10_FORMATS, YUV444_FORMATS};
    use nightjar_core::sdp::{SCM_H264, SCM_HEVC, SCM_HEVC_REXT8_444, VIDEO_FORMAT_H265_REXT8_444};

    const HEVC8: u8 = 1;
    const HEVC10: u8 = 2;
    const AV1_8: u8 = 4;
    /// A PC that encodes HEVC but none of its 4:4:4 formats.
    const PLAIN_SCM: u32 = SCM_H264 | SCM_HEVC;
    /// A PC that encodes HEVC RExt 4:4:4 too.
    const SCM_444: u32 = PLAIN_SCM | SCM_HEVC_REXT8_444;
    /// This device decodes and draws every 4:4:4 and 10-bit format, or none of them.
    static DECODES_444: (u32, String) = (YUV444_FORMATS | SDR10_FORMATS | YUV444_10_FORMATS, String::new());
    static DECODES_NONE: (u32, String) = (0, String::new());

    /// "Sharper colored text" Automatic: both its keys.
    fn on() -> Settings {
        Settings { prefer_yuv444: true, experimental_packed_444: true, ..Settings::default() }
    }

    fn known(host_scm: u32, serverinfo_modes: Option<u8>, color_decodable: &'static (u32, String)) -> Known<'static> {
        Known { serverinfo_modes, host_scm, color_decodable, hardware_decodable: BASE_FORMATS, renderer_missing: false }
    }

    /// A PC that encodes no 4:4:4 to a device that decodes it: the packed step decides alone.
    fn packed_pc(serverinfo_modes: Option<u8>) -> Known<'static> {
        known(PLAIN_SCM, serverinfo_modes, &DECODES_444)
    }

    #[test]
    fn the_codecs_own_444_is_picked_before_packed() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        record.claim_notice(NOTICE_PACKED_UNAVAILABLE);
        let both = known(SCM_444, Some(HEVC8), &DECODES_444);
        let decision = decide(&mut settings, &mut record, &both, false, || panic!("no packed check is spent"));
        let &Pick::Native { formats } = &decision.pick else { panic!("{decision:?}") };
        assert_ne!(formats & VIDEO_FORMAT_H265_REXT8_444, 0, "{formats:#x}");
        assert_eq!((decision.modes, decision.note.as_deref(), decision.record_changed), (0, None, true),
            "should it stop working there, the start says so again");
        assert!(settings.prefer_yuv444 && !settings.experimental_packed_444, "the launch offers 4:4:4 and asks for no packing");
        // The next start there changes nothing.
        let mut settings = on();
        assert!(!decide(&mut settings, &mut record, &both, false, || panic!("no packed check is spent")).record_changed);
    }

    #[test]
    fn packed_runs_where_the_codecs_own_444_does_not() {
        // This device decodes no 4:4:4 format, or the PC encodes none: packed, with the device's modes.
        for native_refused in [known(SCM_444, Some(HEVC8 | HEVC10), &DECODES_NONE), packed_pc(Some(HEVC8 | HEVC10))] {
            let (mut settings, mut record) = (on(), HostRecord::default());
            let decision = decide(&mut settings, &mut record, &native_refused, false, || (HEVC8, None));
            assert_eq!(decision, Decision { pick: Pick::Packed { modes: HEVC8 }, modes: HEVC8, note: None, record_changed: false });
            assert!(settings.experimental_packed_444 && !settings.prefer_yuv444, "the launch asks for packing and offers no 4:4:4");
        }
    }

    #[test]
    fn hdr_stays_and_packs_only_with_a_ten_bit_mode_both_have() {
        let hdr = || Settings { hdr: true, ..on() };
        let mut record = HostRecord::default();
        // No 10-bit packed mode in common: HDR wins without a word, and native 4:4:4 is not tried.
        let mut settings = hdr();
        let decision = decide(&mut settings, &mut record, &known(SCM_444, Some(HEVC8 | HEVC10), &DECODES_444), false, || (HEVC8, None));
        assert_eq!(decision, Decision { pick: Pick::Without(Why::Hdr), modes: 0, note: None, record_changed: false });
        assert!(settings.hdr && !settings.prefer_yuv444 && !settings.experimental_packed_444);
        // A PC whose modes are not known before the launch is not asked with HDR on.
        let mut settings = hdr();
        let decision = decide(&mut settings, &mut record, &known(SCM_444, None, &DECODES_444), false, || (HEVC8 | HEVC10, None));
        assert_eq!(decision.pick, Pick::Without(Why::Hdr));
        assert!(settings.hdr && !settings.experimental_packed_444);
        // A 10-bit mode both have: packed, in HDR.
        let mut settings = hdr();
        let decision = decide(&mut settings, &mut record, &known(SCM_444, Some(HEVC8 | HEVC10), &DECODES_444), false, || (HEVC8 | HEVC10, None));
        assert_eq!((decision.pick, decision.modes), (Pick::Packed { modes: HEVC10 }, HEVC10));
        assert!(settings.hdr && settings.experimental_packed_444 && !settings.prefer_yuv444);
    }

    #[test]
    fn the_notice_names_the_side_that_rules_it_out() {
        fn start(mut settings: Settings, known: &Known, device: u8) -> (Pick, Option<String>) {
            let mut record = HostRecord::default();
            let decision = decide(&mut settings, &mut record, known, false, || (device, None));
            assert!(!settings.prefer_yuv444 && !settings.experimental_packed_444, "{decision:?}");
            (decision.pick, decision.note)
        }
        assert_eq!(HOST_NOTICE, "Sharper colored text isn’t available from this PC. Starting without it.");
        assert_eq!(DEVICE_NOTICE, "Sharper colored text isn’t available on this device. Starting without it.");
        assert_eq!(notice(&Why::Decoder("no decoder".into())), Some(DEVICE_NOTICE));
        assert_eq!(notice(&Why::Device), Some(DEVICE_NOTICE));
        assert_eq!(notice(&Why::Host), Some(HOST_NOTICE));
        assert_eq!(notice(&Why::NoCommonMode), Some(HOST_NOTICE));
        for chosen in [Why::Off, Why::PyroWave, Why::Hdr] {
            assert_eq!(notice(&chosen), None, "{chosen:?}");
        }
        let (device, host) = (Some(DEVICE_NOTICE.to_owned()), Some(HOST_NOTICE.to_owned()));
        // This device decodes no 4:4:4 and the PC packs nothing: packing was the way left, so the PC is why.
        assert_eq!(start(on(), &known(SCM_444, Some(0), &DECODES_NONE), HEVC8), (Pick::Without(Why::Host), host.clone()));
        // The PC packs and this device takes neither way: the decoder is why.
        assert_eq!(start(on(), &known(SCM_444, Some(HEVC8), &DECODES_NONE), 0), (Pick::Without(Why::Decoder(String::new())), device.clone()));
        // The PC encodes no 4:4:4 and packs nothing: the PC is why.
        assert_eq!(start(on(), &packed_pc(Some(0)), HEVC8), (Pick::Without(Why::Host), host.clone()));
        // With HDR there is no native step: no packed decoder here, or no packed mode in common.
        let hdr = Settings { hdr: true, ..on() };
        assert_eq!(start(hdr.clone(), &known(SCM_444, Some(HEVC10), &DECODES_444), 0), (Pick::Without(Why::Device), device));
        assert_eq!(start(hdr, &known(SCM_444, Some(AV1_8), &DECODES_444), HEVC8), (Pick::Without(Why::NoCommonMode), host));
        // The player's own choices go without a word.
        assert_eq!(start(Settings::default(), &known(SCM_444, Some(HEVC8), &DECODES_444), HEVC8), (Pick::Without(Why::Off), None));
        let pyrowave = Settings { codec: crate::config::CodecPreference::PyroWave, ..on() };
        assert_eq!(start(pyrowave, &known(SCM_444, Some(HEVC8), &DECODES_444), HEVC8), (Pick::Without(Why::PyroWave), None));
    }

    #[test]
    fn a_pc_without_either_way_starts_without_it_once_said() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        let decision = decide(&mut settings, &mut record, &packed_pc(Some(0)), false, || panic!("no decode is spent"));
        assert_eq!(decision, Decision { pick: Pick::Without(Why::Host), modes: 0, note: Some(HOST_NOTICE.to_owned()), record_changed: true });
        assert!(!settings.experimental_packed_444 && !settings.prefer_yuv444, "the launch asks for neither");
        // The next start there says nothing more.
        let mut settings = on();
        let decision = decide(&mut settings, &mut record, &packed_pc(Some(0)), false, || panic!("no decode is spent"));
        assert_eq!(decision, Decision { pick: Pick::Without(Why::Host), modes: 0, note: None, record_changed: false });
        assert!(!settings.experimental_packed_444 && !settings.prefer_yuv444);
    }

    #[test]
    fn a_decoder_without_either_way_starts_without_it() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        let decision = decide(&mut settings, &mut record, &known(SCM_444, Some(HEVC8), &DECODES_NONE), false, || (0, None));
        assert_eq!(decision.note.as_deref(), Some(DEVICE_NOTICE));
        assert!(!settings.experimental_packed_444 && !settings.prefer_yuv444);
        // The packed check's own reason, such as a packed picture larger than the decoder opens, is said instead.
        let (mut settings, mut record) = (on(), HostRecord::default());
        let decision = decide(&mut settings, &mut record, &packed_pc(None), false, || (0, Some("too large".into())));
        assert_eq!(decision.note.as_deref(), Some("too large"));
    }

    #[test]
    fn sharper_colored_text_without_the_gpu_renderer_starts_without_it_once_said() {
        let no_gpu = |need: &'static str| Some(format!("{need} needs the GPU renderer"));
        assert_eq!(gpu_check(&on(), no_gpu), Ok(true), "left out, not refused");
        // Either key alone (settings saved before the two became one) is left out too.
        assert_eq!(gpu_check(&Settings { prefer_yuv444: true, ..Settings::default() }, no_gpu), Ok(true));
        assert_eq!(gpu_check(&Settings { experimental_packed_444: true, ..Settings::default() }, no_gpu), Ok(true));
        // Another need still refuses, in its own words.
        assert_eq!(gpu_check(&Settings { sdr_10bit: true, ..on() }, no_gpu), Err("10-bit SDR needs the GPU renderer".into()));
        // HDR says it in its own check.
        assert_eq!(gpu_check(&Settings { hdr: true, ..on() }, no_gpu), Ok(true));
        // With the renderer, or without sharper colored text, nothing is left out.
        assert_eq!(gpu_check(&on(), |_| None), Ok(false));
        assert_eq!(gpu_check(&Settings::default(), no_gpu), Ok(false));
        // The launch then decides as for a decoder without either way: without it, said once per PC.
        let mut record = HostRecord::default();
        let missing = Known { renderer_missing: true, ..known(SCM_444, Some(HEVC8), &DECODES_444) };
        for said in [true, false] {
            let mut settings = on();
            let decision = decide(&mut settings, &mut record, &missing, false, || (0, None));
            assert_eq!(decision.pick, Pick::Without(Why::Decoder(NO_GPU_RENDERER.to_owned())));
            assert_eq!(decision.note.as_deref(), said.then_some(DEVICE_NOTICE));
            assert!(!settings.experimental_packed_444 && !settings.prefer_yuv444 && decision.modes == 0);
        }
    }

    #[test]
    fn a_refusal_after_the_launch_starts_again_without_it_and_says_so() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        assert!(refused(&mut record));
        let decision = decide(&mut settings, &mut record, &known(SCM_444, Some(HEVC8), &DECODES_444), true, || panic!("not asked"));
        assert_eq!(decision, Decision { pick: Pick::Without(Why::Host), modes: 0, note: Some(HOST_NOTICE.to_owned()), record_changed: false });
        assert!(!settings.experimental_packed_444 && !settings.prefer_yuv444, "native 4:4:4 is not tried again");
        // A start that did not ask is not turned into anything.
        let mut off = Settings::default();
        assert_eq!(decide(&mut off, &mut record, &known(SCM_444, None, &DECODES_444), true, || panic!("not asked")).note, None);
    }

    #[test]
    fn a_remembered_refusal_holds_until_a_stream_starts_with_it_or_the_setting_is_switched() {
        let mut record = HostRecord::default();
        refused(&mut record);
        // A PC that does not say its modes is not asked again at every launch.
        let mut settings = on();
        let decision = decide(&mut settings, &mut record, &packed_pc(None), false, || panic!("no decode is spent"));
        assert_eq!(decision.note.as_deref(), Some(HOST_NOTICE));
        assert!(!settings.experimental_packed_444);
        // Its serverinfo naming modes outranks it.
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, &packed_pc(Some(HEVC8)), false, || (HEVC8, None)).modes, HEVC8);
        // Switching the setting asks again, and says it again.
        assert!(ask_again(&mut record));
        assert!(!ask_again(&mut record));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, &packed_pc(None), false, || (HEVC8, None)).modes, HEVC8);
        // So does a stream that starts with it; one without it changes nothing.
        refused(&mut record);
        record.claim_notice(NOTICE_PACKED_UNAVAILABLE);
        assert!(!started(&mut record, false, HEVC8));
        assert!(started(&mut record, true, HEVC8));
        assert!(!record.full_chroma.refused && record.full_chroma.modes == Some(HEVC8));
        assert!(record.claim_notice(NOTICE_PACKED_UNAVAILABLE), "said again");
        // The kept modes rule out a device without them before the next launch.
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, &packed_pc(None), false, || (4, None)).modes, 0);
        // The record survives a save.
        let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(saved.full_chroma, record.full_chroma);
    }

    #[test]
    fn a_refusal_is_asked_again_after_an_update_on_either_side() {
        let mut record = HostRecord::default();
        assert!(renew(&mut record, "0.9.0", Some("3.11.2")));
        refused(&mut record);
        // The same versions: the PC is not asked again.
        assert!(!renew(&mut record, "0.9.0", Some("3.11.2")));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, &packed_pc(None), false, || panic!("no decode is spent")).modes, 0);
        // The PC was updated: asked again.
        assert!(renew(&mut record, "0.9.0", Some("3.12.0")));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, &packed_pc(None), false, || (HEVC8, None)).modes, HEVC8);
        // This client was updated: asked again, also on a PC that does not say its version.
        renew(&mut record, "0.9.0", None);
        refused(&mut record);
        assert!(renew(&mut record, "0.9.1", None));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, &packed_pc(None), false, || (HEVC8, None)).modes, HEVC8);
        // The versions survive a save.
        let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(saved.full_chroma.under, "0.9.1/");
    }
}
