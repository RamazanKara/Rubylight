//! Full chroma at a stream's start: decided before the launch from the PC's modes and this
//! device's decoders (`nightjar_core::full_chroma`), so a stream that cannot have it starts
//! without it and says so, and started once more without it when the PC refuses it after the
//! launch. Android's `PackedFallback` and `HostPackedModes`.

use crate::config::{HostRecord, Settings, NOTICE_PACKED_UNAVAILABLE};
use crate::strings::{en, tr};
use nightjar_core::full_chroma::{before_launch, Unavailable};

/// The start's notice when the PC rules full chroma out, or refused it after the launch.
pub const HOST_NOTICE: &str = en("PC can't stream full chroma. Starting without it.");
/// The start's notice when this device's decoders rule it out.
pub const DEVICE_NOTICE: &str = en("Full chroma isn't available for this stream. Starting without it.");

/// The notice for `why`, untranslated.
pub fn notice(why: Unavailable) -> &'static str {
    if why.from_host() { HOST_NOTICE } else { DEVICE_NOTICE }
}

/// What a launch asks for.
#[derive(Debug, PartialEq, Eq)]
pub struct Decision {
    /// The packed modes this device offers in the handshake; 0 without full chroma.
    pub modes: u8,
    /// What the start says about it.
    pub note: Option<String>,
    /// `record` changed and is saved.
    pub record_changed: bool,
}

/// Decides full chroma for a launch of `settings` on the PC of `record`, which says
/// `serverinfo_modes` in its serverinfo: turns it off in `settings` when the PC or this device
/// (`device_modes`: the modes its decoders passed, and what to say when none did) cannot do it,
/// or when `refused_now`, the PC refused it after this start's launch. A decision before the
/// launch is said once per PC, until a stream there starts with it or the setting is switched;
/// a refusal after the launch is always said.
pub fn decide(settings: &mut Settings, record: &mut HostRecord, serverinfo_modes: Option<u8>, refused_now: bool,
    device_modes: impl FnOnce() -> (u8, Option<String>)) -> Decision {
    if !settings.experimental_packed_444 {
        return Decision { modes: 0, note: None, record_changed: false };
    }
    if refused_now {
        eprintln!("nightjar: full chroma: the PC refused it after the launch; starting again without it");
        settings.experimental_packed_444 = false;
        return Decision { modes: 0, note: Some(tr(HOST_NOTICE).to_owned()), record_changed: false };
    }
    let memory = &record.full_chroma;
    let (known, refused) = (memory.known(serverinfo_modes), memory.refused_before(serverinfo_modes));
    let mut device_note = None;
    let decided = before_launch(known, refused, || {
        let (modes, note) = device_modes();
        device_note = note;
        modes
    });
    match decided {
        Ok(modes) => Decision { modes, note: None, record_changed: false },
        Err(why) => {
            let from = if serverinfo_modes.is_some() { "serverinfo" } else if refused { "a refusal before" } else { "last stream" };
            eprintln!("nightjar: full chroma is unavailable before the launch ({why:?}): PC modes {} ({from}); starting without it",
                known.map_or_else(|| "unknown".to_owned(), |modes| format!("{modes:#x}")));
            settings.experimental_packed_444 = false;
            let first = record.claim_notice(NOTICE_PACKED_UNAVAILABLE);
            Decision { modes: 0, note: first.then(|| device_note.unwrap_or_else(|| tr(notice(why)).to_owned())), record_changed: first }
        }
    }
}

/// Linux's check of what needs the GPU renderer, before a stream's other checks: `missing` says
/// what to tell when a need finds none. Full chroma does not refuse the stream: it is left out,
/// Ok(true), and [`decide`] starts without it as on a decoder without it. Any other need that
/// refuses without the renderer (`handheld::refused_without_gpu`) does, with `missing`'s words.
#[cfg_attr(windows, allow(dead_code))]
pub fn gpu_check(settings: &Settings, missing: impl Fn(&'static str) -> Option<String>) -> Result<bool, String> {
    let left_out = settings.experimental_packed_444 && missing("packed full chroma").is_some();
    let rest = Settings { experimental_packed_444: settings.experimental_packed_444 && !left_out, ..settings.clone() };
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

    const HEVC8: u8 = 1;

    fn on() -> Settings {
        Settings { experimental_packed_444: true, ..Settings::default() }
    }

    #[test]
    fn a_pc_without_the_mode_starts_without_full_chroma_once_said() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        let decision = decide(&mut settings, &mut record, Some(0), false, || panic!("no decode is spent"));
        assert_eq!(decision, Decision { modes: 0, note: Some(HOST_NOTICE.to_owned()), record_changed: true });
        assert!(!settings.experimental_packed_444, "the launch does not ask");
        // The next start there says nothing more.
        let mut settings = on();
        let decision = decide(&mut settings, &mut record, Some(0), false, || panic!("no decode is spent"));
        assert_eq!(decision, Decision { modes: 0, note: None, record_changed: false });
        assert!(!settings.experimental_packed_444);
    }

    #[test]
    fn a_decoder_without_the_mode_starts_without_full_chroma() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        let decision = decide(&mut settings, &mut record, Some(HEVC8), false, || (0, None));
        assert_eq!(decision.note.as_deref(), Some(DEVICE_NOTICE));
        assert!(!settings.experimental_packed_444);
        // The decoder's own reason, such as a packed picture larger than it opens, is said instead.
        let (mut settings, mut record) = (on(), HostRecord::default());
        let decision = decide(&mut settings, &mut record, None, false, || (0, Some("too large".into())));
        assert_eq!(decision.note.as_deref(), Some("too large"));
        // Both able: the launch asks with the device's modes.
        let (mut settings, mut record) = (on(), HostRecord::default());
        assert_eq!(decide(&mut settings, &mut record, Some(HEVC8 | 2), false, || (HEVC8, None)),
            Decision { modes: HEVC8, note: None, record_changed: false });
        assert!(settings.experimental_packed_444);
    }

    #[test]
    fn full_chroma_without_the_gpu_renderer_starts_without_it_once_said() {
        let no_gpu = |need: &'static str| Some(format!("{need} needs the GPU renderer"));
        assert_eq!(gpu_check(&on(), no_gpu), Ok(true), "left out, not refused");
        // Another need still refuses, in its own words.
        assert_eq!(gpu_check(&Settings { prefer_yuv444: true, ..on() }, no_gpu), Err("4:4:4 needs the GPU renderer".into()));
        // HDR says it in its own check.
        assert_eq!(gpu_check(&Settings { hdr: true, ..on() }, no_gpu), Ok(true));
        // With the renderer, or without full chroma, nothing is left out.
        assert_eq!(gpu_check(&on(), |_| None), Ok(false));
        assert_eq!(gpu_check(&Settings::default(), no_gpu), Ok(false));
        // The launch then decides as for a decoder without it: without full chroma, said once per PC.
        let mut record = HostRecord::default();
        for said in [true, false] {
            let mut settings = on();
            let decision = decide(&mut settings, &mut record, Some(HEVC8), false, || (0, None));
            assert_eq!(decision.note.as_deref(), said.then_some(DEVICE_NOTICE));
            assert!(!settings.experimental_packed_444 && decision.modes == 0);
        }
    }

    #[test]
    fn a_refusal_after_the_launch_starts_again_without_it_and_says_so() {
        let (mut settings, mut record) = (on(), HostRecord::default());
        assert!(refused(&mut record));
        let decision = decide(&mut settings, &mut record, Some(HEVC8), true, || panic!("not asked"));
        assert_eq!(decision, Decision { modes: 0, note: Some(HOST_NOTICE.to_owned()), record_changed: false });
        assert!(!settings.experimental_packed_444);
        // A start that did not ask is not turned into anything.
        let mut off = Settings::default();
        assert_eq!(decide(&mut off, &mut record, None, true, || panic!("not asked")).note, None);
    }

    #[test]
    fn a_remembered_refusal_holds_until_a_stream_starts_with_it_or_the_setting_is_switched() {
        let mut record = HostRecord::default();
        refused(&mut record);
        // A PC that does not say its modes is not asked again at every launch.
        let mut settings = on();
        let decision = decide(&mut settings, &mut record, None, false, || panic!("no decode is spent"));
        assert_eq!(decision.note.as_deref(), Some(HOST_NOTICE));
        assert!(!settings.experimental_packed_444);
        // Its serverinfo naming modes outranks it.
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, Some(HEVC8), false, || (HEVC8, None)).modes, HEVC8);
        // Switching the setting asks again, and says it again.
        assert!(ask_again(&mut record));
        assert!(!ask_again(&mut record));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, None, false, || (HEVC8, None)).modes, HEVC8);
        // So does a stream that starts with it; one without it changes nothing.
        refused(&mut record);
        record.claim_notice(NOTICE_PACKED_UNAVAILABLE);
        assert!(!started(&mut record, false, HEVC8));
        assert!(started(&mut record, true, HEVC8));
        assert!(!record.full_chroma.refused && record.full_chroma.modes == Some(HEVC8));
        assert!(record.claim_notice(NOTICE_PACKED_UNAVAILABLE), "said again");
        // The kept modes rule out a device without them before the next launch.
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, None, false, || (4, None)).modes, 0);
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
        assert_eq!(decide(&mut settings, &mut record, None, false, || panic!("no decode is spent")).modes, 0);
        // The PC was updated: asked again.
        assert!(renew(&mut record, "0.9.0", Some("3.12.0")));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, None, false, || (HEVC8, None)).modes, HEVC8);
        // This client was updated: asked again, also on a PC that does not say its version.
        renew(&mut record, "0.9.0", None);
        refused(&mut record);
        assert!(renew(&mut record, "0.9.1", None));
        let mut settings = on();
        assert_eq!(decide(&mut settings, &mut record, None, false, || (HEVC8, None)).modes, HEVC8);
        // The versions survive a save.
        let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(saved.full_chroma.under, "0.9.1/");
    }
}
