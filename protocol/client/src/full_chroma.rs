//! Whether a stream asks for packed full chroma, decided before the launch, and what each PC's
//! answers leave remembered for the next one. Mirrors Android's `PackedFallback.beforeLaunch` and
//! `HostPackedModes`: a stream that cannot have full chroma starts without it, instead of
//! launching the game, being refused in the handshake ([`crate::rtsp::RtspError::PackedRefused`])
//! and failing with the game already running.
//!
//! Modes are `rubylight_protocol::packed444::mode_bit` bits: HEVC 8-bit 1, HEVC 10-bit 2, AV1
//! 8-bit 4, AV1 10-bit 8.

use crate::session::error;
use rubylight_protocol::packed444::ALL_MODES;

/// Why a stream starts without full chroma, known before anything is launched on the PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The PC says it offers no packed mode, or refused the last stream that asked and has not
    /// said otherwise.
    Host,
    /// No decoder on this device passed the check for this stream.
    Device,
    /// The PC and this device each have modes, none of them the same.
    NoCommonMode,
}

impl Unavailable {
    /// Whether the PC's side ruled it out, which picks the notice's wording ("with this PC").
    pub fn from_host(self) -> bool {
        matches!(self, Self::Host | Self::NoCommonMode)
    }
}

/// `Packed444Modes` from serverinfo: the modes this client knows (bits a newer host adds are left
/// out), or None when absent or not a number (Android's `NvHTTP.readPacked444Modes`).
pub fn read_serverinfo_modes(value: Option<&str>) -> Option<u8> {
    let parsed: i64 = value?.trim().parse().ok()?;
    (parsed >= 0).then(|| (parsed & i64::from(ALL_MODES)) as u8)
}

/// Decides before the launch whether the stream asks for full chroma.
///
/// `host_modes`: the PC's packed modes, from its serverinfo, else its last DESCRIBE
/// ([`Memory::known`]), else None. `refused_before`: the PC refused the last stream that asked;
/// it counts only while the PC does not say its modes. `device_modes`: the modes this device
/// verified, asked only when the PC does not rule it out, since verifying costs a decode.
///
/// Ok with the device's modes for the handshake, or why the stream starts without it.
pub fn before_launch(
    host_modes: Option<u8>,
    refused_before: bool,
    device_modes: impl FnOnce() -> u8,
) -> Result<u8, Unavailable> {
    let host_modes = host_modes.map(|modes| modes & ALL_MODES);
    if host_modes == Some(0) || (host_modes.is_none() && refused_before) {
        return Err(Unavailable::Host);
    }
    let device = device_modes() & ALL_MODES;
    if device == 0 {
        return Err(Unavailable::Device);
    }
    if host_modes.is_some_and(|host| host & device == 0) {
        return Err(Unavailable::NoCommonMode);
    }
    Ok(device)
}

/// Whether a start that asked for full chroma (`asked`) and failed with `code` starts once more
/// without it: the PC refused it in the handshake. A start that did not ask was not refused for
/// it, and asking again would only be refused again.
pub fn restarts_without(asked: bool, code: i32) -> bool {
    asked && code == error::PACKED_444_REFUSED
}

/// What a PC's answer is given under: this client's version and the PC's own (`RustHostVersion`;
/// None for a PC that does not say it). An update on either side may change the answer.
pub fn stamp(client_version: &str, host_version: Option<&str>) -> String {
    format!("{client_version}/{}", host_version.unwrap_or(""))
}

/// What one PC's answers to full chroma left: the modes its last DESCRIBE offered, and whether it
/// refused the last stream that asked (Android's `HostPackedModes`, kept per PC).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Memory {
    /// The modes the PC's last full chroma stream's DESCRIBE offered; None when none was seen.
    pub modes: Option<u8>,
    /// The PC refused the last stream that asked for full chroma.
    pub refused: bool,
    /// The [`stamp`] of the last launch that asked about full chroma there; empty before any, or
    /// in a record saved before versions were kept.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub under: String,
}

impl Memory {
    /// What the next launch takes the PC to offer: its serverinfo when it says, else the last
    /// DESCRIBE.
    pub fn known(&self, serverinfo: Option<u8>) -> Option<u8> {
        serverinfo.or(self.modes).map(|modes| modes & ALL_MODES)
    }

    /// Whether a launch that does not hear the PC's modes counts its last refusal.
    pub fn refused_before(&self, serverinfo: Option<u8>) -> bool {
        serverinfo.is_none() && self.refused
    }

    /// A full chroma stream started with the modes its DESCRIBE offered: they are kept, and the
    /// refusal ends. No modes forgets the PC's. True when anything changed.
    pub fn started(&mut self, describe_modes: u8) -> bool {
        let modes = describe_modes & ALL_MODES;
        let next = Self {
            modes: (modes != 0).then_some(modes),
            refused: false,
            under: self.under.clone(),
        };
        std::mem::replace(self, next.clone()) != next
    }

    /// The PC refused a stream that asked for full chroma: its modes are forgotten and the next
    /// launch on a PC that does not say its modes starts without it. True when anything changed.
    pub fn refused(&mut self) -> bool {
        let next = Self {
            modes: None,
            refused: true,
            under: self.under.clone(),
        };
        std::mem::replace(self, next.clone()) != next
    }

    /// A launch that asks about full chroma under `stamp` ([`stamp`]): a refusal given under other
    /// versions ends, so a PC that refused is asked again after this client or the PC is updated.
    /// Its known modes stay; serverinfo or the next DESCRIBE corrects them. True when anything
    /// changed.
    pub fn renew(&mut self, stamp: &str) -> bool {
        if self.under == stamp {
            return false;
        }
        self.under = stamp.to_owned();
        self.refused = false;
        true
    }

    /// The user switched the setting, which is how to ask a PC again after changing it. True when
    /// a refusal ended.
    pub fn ask_again(&mut self) -> bool {
        std::mem::take(&mut self.refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEVC8: u8 = 1;
    const HEVC10: u8 = 2;
    const AV1_8: u8 = 4;

    #[test]
    fn serverinfo_modes_are_read_as_android_does() {
        assert_eq!(read_serverinfo_modes(None), None);
        assert_eq!(read_serverinfo_modes(Some("x")), None);
        assert_eq!(read_serverinfo_modes(Some("-1")), None);
        assert_eq!(read_serverinfo_modes(Some(" 0 ")), Some(0));
        assert_eq!(read_serverinfo_modes(Some("3")), Some(3));
        // Bits a newer host adds are not this client's business.
        assert_eq!(read_serverinfo_modes(Some("19")), Some(3));
    }

    #[test]
    fn a_pc_without_the_mode_starts_without_full_chroma_and_no_device_check_runs() {
        let mut checked = false;
        let mut check = || {
            checked = true;
            HEVC8
        };
        assert_eq!(
            before_launch(Some(0), false, &mut check),
            Err(Unavailable::Host)
        );
        assert!(!checked, "the PC ruled it out: no decode is spent");
        assert!(Unavailable::Host.from_host());
    }

    #[test]
    fn a_decoder_without_the_mode_starts_without_full_chroma() {
        assert_eq!(
            before_launch(Some(HEVC8 | HEVC10), false, || 0),
            Err(Unavailable::Device)
        );
        assert_eq!(before_launch(None, false, || 0), Err(Unavailable::Device));
        assert!(!Unavailable::Device.from_host());
    }

    #[test]
    fn a_pc_and_device_with_no_mode_in_common_start_without_full_chroma() {
        assert_eq!(
            before_launch(Some(AV1_8), false, || HEVC8),
            Err(Unavailable::NoCommonMode)
        );
        assert!(Unavailable::NoCommonMode.from_host());
        assert_eq!(
            before_launch(Some(AV1_8 | HEVC8), false, || HEVC8),
            Ok(HEVC8)
        );
        // A PC that does not say leaves it to the handshake.
        assert_eq!(before_launch(None, false, || HEVC8), Ok(HEVC8));
    }

    #[test]
    fn a_refusal_restarts_once_without_full_chroma() {
        assert!(restarts_without(true, error::PACKED_444_REFUSED));
        // The restart does not ask, so a second refusal cannot come from it.
        assert!(!restarts_without(false, error::PACKED_444_REFUSED));
        assert!(!restarts_without(true, error::NO_VIDEO_TRAFFIC));
    }

    #[test]
    fn a_remembered_refusal_counts_until_the_pc_says_its_modes_or_the_user_asks_again() {
        let mut memory = Memory::default();
        assert!(memory.started(HEVC8 | 0x30));
        assert_eq!(memory.known(None), Some(HEVC8), "the last DESCRIBE");
        assert!(memory.refused());
        assert!(!memory.refused(), "nothing new");
        assert_eq!(memory.known(None), None, "a refusal forgets the PC's modes");
        // The next launch on a PC that does not say its modes starts without full chroma.
        assert!(memory.refused_before(None));
        assert_eq!(
            before_launch(memory.known(None), memory.refused_before(None), || HEVC8),
            Err(Unavailable::Host)
        );
        // Serverinfo that names modes outranks it.
        assert!(!memory.refused_before(Some(HEVC8)));
        assert_eq!(
            before_launch(
                memory.known(Some(HEVC8)),
                memory.refused_before(Some(HEVC8)),
                || HEVC8
            ),
            Ok(HEVC8)
        );
        assert_eq!(
            memory.known(Some(0)),
            Some(0),
            "serverinfo saying none is kept"
        );
        // Switching the setting asks again.
        assert!(memory.ask_again());
        assert!(!memory.ask_again());
        assert_eq!(
            before_launch(None, memory.refused_before(None), || HEVC8),
            Ok(HEVC8)
        );
        // A stream that starts with it ends the refusal; one whose DESCRIBE offers none forgets.
        memory.refused();
        assert!(memory.started(HEVC10));
        assert_eq!(
            memory,
            Memory {
                modes: Some(HEVC10),
                refused: false,
                under: String::new()
            }
        );
        assert!(memory.started(0));
        assert_eq!(memory, Memory::default());
    }

    #[test]
    fn a_refusal_ends_when_this_client_or_the_pc_is_updated() {
        let first = stamp("0.9.0", Some("3.11.2"));
        let mut memory = Memory::default();
        assert!(memory.renew(&first));
        assert!(!memory.renew(&first), "the same versions change nothing");
        assert!(memory.refused());
        assert_eq!(memory.under, first, "a refusal keeps the versions it was given under");
        assert!(!memory.renew(&first));
        assert!(memory.refused_before(None), "same versions: not asked again");
        // The PC was updated: it is asked again.
        assert!(memory.renew(&stamp("0.9.0", Some("3.12.0"))));
        assert!(!memory.refused_before(None));
        // So is it after this client was updated, and for a PC that does not say its version.
        let other = stamp("0.9.0", None);
        memory.renew(&other);
        memory.refused();
        assert!(memory.refused_before(None));
        assert!(memory.renew(&stamp("0.9.1", None)));
        assert!(!memory.refused_before(None));
        // The known modes stay across an update; started and refused keep the versions.
        memory.started(HEVC8);
        assert_eq!((memory.modes, memory.under.as_str()), (Some(HEVC8), "0.9.1/"));
        // A refusal saved before versions were kept is asked again at the next launch.
        let old: Memory = serde_json::from_str(r#"{"modes":null,"refused":true}"#).unwrap();
        assert_eq!(old.under, "");
        let mut old = old;
        assert!(old.renew(&first));
        assert!(!old.refused);
        // Nothing to say is not written.
        assert!(!serde_json::to_string(&Memory::default()).unwrap().contains("under"));
    }
}
