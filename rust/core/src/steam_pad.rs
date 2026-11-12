//! The VHF driver's Steam Controller (2026) profile (libvirtualgamepad
//! 0.1.0-beta.10, `include/libvirtualgamepad/protocol.h`): the input word it
//! takes, the haptic and settings feedback it hands back, and the firmware
//! attributes request. How the host maps a client onto it follows Vibepollo's
//! `docs/steam_controller.md` (GPL-3.0, Nonary/Vibepollo).
use rubylight_protocol::steam_controller::{
    LI_CCAP_GRIP_SENSE, LI_CCAP_SC26_BUTTONS, LI_CCAP_STEAM_HAPTIC, LI_CCAP_STEAM_SETTINGS,
    LI_CCAP_STICK_TOUCH, LI_SCATTR_BOARD_REVISION, LI_SCATTR_BOOTLOADER_BUILD_TIME,
    LI_SCATTR_BUILD_ID, LI_SCATTR_CAPABILITIES, LI_SCATTR_FIRMWARE_BUILD_TIME, LI_SCATTR_SERIALS,
    STEAM_HAPTIC_MESSAGE_TYPE, STEAM_SETTINGS_MESSAGE_TYPE,
};
pub use rubylight_protocol::steam_controller::{
    LI_CCAP_STEAM_ATTRIBUTES, SteamAttributes, SteamHaptic, SteamSettings,
};

/// The driver's `feedback_type::steam_haptic`: a haptic output report as Steam wrote it.
pub const DRIVER_STEAM_HAPTIC: u16 = 6;
/// The driver's `feedback_type::steam_settings`: a control-channel settings write.
pub const DRIVER_STEAM_SETTINGS: u16 = 7;
/// `sizeof(steam_haptic_feedback)`: length, three reserved bytes, a 12-byte report.
const HAPTIC_PAYLOAD: usize = 16;
/// `sizeof(steam_settings_feedback)`: command, count, three reserved bytes, nine triples.
const SETTINGS_PAYLOAD: usize = 32;
/// The driver's `sc26_buttons_explicit`: host-owned, never taken from the client.
pub const BUTTONS_EXPLICIT: u32 = 0x4000_0000;
/// The arrival capabilities only a Steam Controller (2026) client announces.
pub const CAPABILITIES: u16 = LI_CCAP_GRIP_SENSE
    | LI_CCAP_STICK_TOUCH
    | LI_CCAP_STEAM_HAPTIC
    | LI_CCAP_SC26_BUTTONS
    | LI_CCAP_STEAM_SETTINGS
    | LI_CCAP_STEAM_ATTRIBUTES;
/// `ioctl_set_steam_controller_attributes`'s function code.
pub const ATTRIBUTES_IOCTL: u32 = 0x808;
/// `sizeof(steam_controller_attributes_request)`.
pub const ATTRIBUTES_REQUEST: usize = 80;
/// The `LI_SCATTR_*` fields the driver applies; the radio and dongle build
/// times have no tag on a wired unit.
const DRIVER_ATTRIBUTES: u8 = LI_SCATTR_FIRMWARE_BUILD_TIME
    | LI_SCATTR_BOOTLOADER_BUILD_TIME
    | LI_SCATTR_BOARD_REVISION
    | LI_SCATTR_CAPABILITIES
    | LI_SCATTR_BUILD_ID
    | LI_SCATTR_SERIALS;

/// The button word for the driver. The grip, stick-touch and click flags ride
/// in the upper half as the client sent them; the explicit-click flag comes
/// from the arrival capability alone, so an all-released first state is
/// authoritative and the driver stops guessing clicks from trigger travel.
pub fn buttons(buttons: u32, capabilities: u16) -> u32 {
    let buttons = buttons & !BUTTONS_EXPLICIT;
    if capabilities & LI_CCAP_SC26_BUTTONS != 0 {
        buttons | BUTTONS_EXPLICIT
    } else {
        buttons
    }
}

/// What becomes of a Steam feedback event from the driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// A control packet for a client with the real controller: its type and payload.
    Forward(u16, Vec<u8>),
    /// A haptic report for a client without the real pads: synthesized rumble.
    Rumble(SteamHaptic),
}

/// Routes the driver's feedback event of `kind` for `controller`: `None` for
/// another kind, Some(None) for a Steam event dropped (malformed, or settings
/// for a client that cannot take them). Neither kind is ever deduplicated:
/// Steam re-sends a click to keep a buzz going, and a settings chunk cannot
/// replace its siblings.
pub fn route(
    controller: u16,
    kind: u16,
    payload: &[u8],
    capabilities: u16,
) -> Option<Option<Feedback>> {
    Some(match kind {
        DRIVER_STEAM_HAPTIC => driver_haptic(controller, payload).map(|haptic| {
            if capabilities & LI_CCAP_STEAM_HAPTIC != 0 {
                Feedback::Forward(STEAM_HAPTIC_MESSAGE_TYPE, haptic.encode().to_vec())
            } else {
                Feedback::Rumble(haptic)
            }
        }),
        DRIVER_STEAM_SETTINGS => driver_settings(controller, payload)
            .filter(|_| capabilities & LI_CCAP_STEAM_SETTINGS != 0)
            .map(|settings| {
                Feedback::Forward(STEAM_SETTINGS_MESSAGE_TYPE, settings.encode().to_vec())
            }),
        _ => return None,
    })
}

/// A `steam_haptic_feedback` payload; one whose size or length disagrees is refused.
pub fn driver_haptic(controller: u16, payload: &[u8]) -> Option<SteamHaptic> {
    if payload.len() != HAPTIC_PAYLOAD {
        return None;
    }
    let length = usize::from(payload[0]);
    SteamHaptic::new(controller, payload.get(4..4 + length)?).ok()
}

/// A `steam_settings_feedback` payload; an unknown command or more than nine
/// triples is refused.
pub fn driver_settings(controller: u16, payload: &[u8]) -> Option<SteamSettings> {
    if payload.len() != SETTINGS_PAYLOAD {
        return None;
    }
    let count = usize::from(payload[1]);
    let triples: Vec<[u8; 3]> = payload[5..]
        .as_chunks::<3>()
        .0
        .iter()
        .take(count)
        .copied()
        .collect();
    if triples.len() != count {
        return None;
    }
    SteamSettings::new(controller, payload[0], &triples).ok()
}

/// The firmware build time a virtual Steam Controller reports when its client
/// has no real one to read: 2036-01-01 UTC, later than any firmware Steam
/// ships. The driver's own default is the firmware of the unit it was captured
/// from, which Steam offers to update once it ships newer firmware, and an
/// update cannot reach a virtual controller.
pub const VIRTUAL_FIRMWARE_BUILD_TIME: u32 = 0x7c24_5f00;

/// The attributes for a virtual Steam Controller whose client sent none: only
/// the firmware build time, so the driver keeps its other defaults.
pub fn virtual_attributes(controller: u8) -> SteamAttributes {
    SteamAttributes {
        controller,
        valid_mask: LI_SCATTR_FIRMWARE_BUILD_TIME,
        firmware_build_time: VIRTUAL_FIRMWARE_BUILD_TIME,
        ..Default::default()
    }
}

/// The `steam_controller_attributes_request` for driver slot `slot`. The
/// strings go verbatim: the driver keeps one only when it is printable ASCII
/// and terminates it at 15 characters.
pub fn attributes_request(slot: u32, attributes: &SteamAttributes) -> [u8; ATTRIBUTES_REQUEST] {
    let mut out = [0; ATTRIBUTES_REQUEST];
    out[0..4].copy_from_slice(&(ATTRIBUTES_REQUEST as u32).to_le_bytes());
    out[4..6].copy_from_slice(&2u16.to_le_bytes());
    out[8..12].copy_from_slice(&slot.to_le_bytes());
    let flags = u32::from(attributes.valid_mask & DRIVER_ATTRIBUTES);
    for (at, word) in [
        flags,
        attributes.firmware_build_time,
        attributes.bootloader_build_time,
        attributes.board_revision,
        attributes.capabilities,
    ]
    .into_iter()
    .enumerate()
    {
        out[12 + 4 * at..16 + 4 * at].copy_from_slice(&word.to_le_bytes());
    }
    out[32..48].copy_from_slice(&attributes.build_id);
    out[48..64].copy_from_slice(&attributes.unit_serial);
    out[64..80].copy_from_slice(&attributes.board_serial);
    out
}

/// The pad (0 left, 1 right) a finger going down lands on: the touchpad's
/// index for a client with two, the half of its one pad otherwise.
pub fn touch_pad(x: f32, touchpad: u8, dual: bool) -> u8 {
    if dual {
        touchpad.min(1)
    } else {
        u8::from(x >= 0.5)
    }
}

/// `x` on `pad`: a single-touchpad client's half stretched back to the pad's
/// full width.
pub fn touch_x(x: f32, pad: u8, dual: bool) -> f32 {
    let x = x.clamp(0., 1.);
    if dual {
        x
    } else {
        ((x - f32::from(pad) / 2.) * 2.).clamp(0., 1.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::steam_controller::{
        STEAM_SETTINGS_LOAD_DEFAULTS, STEAM_SETTINGS_SET_VALUES,
    };

    fn haptic_payload(report: &[u8]) -> Vec<u8> {
        let mut payload = vec![report.len() as u8, 0, 0, 0];
        payload.extend_from_slice(report);
        payload.resize(HAPTIC_PAYLOAD, 0);
        payload
    }

    fn settings_payload(command: u8, triples: &[[u8; 3]]) -> Vec<u8> {
        let mut payload = vec![command, triples.len() as u8, 0, 0, 0];
        payload.extend(triples.iter().flatten());
        payload.resize(SETTINGS_PAYLOAD, 0);
        payload
    }

    #[test]
    fn the_explicit_click_flag_comes_from_the_arrival_alone() {
        let grips = 0x0040_0000 | 0x0400_0000;
        assert_eq!(buttons(grips | BUTTONS_EXPLICIT, 0), grips);
        assert_eq!(
            buttons(0, LI_CCAP_SC26_BUTTONS | LI_CCAP_GRIP_SENSE),
            BUTTONS_EXPLICIT
        );
        assert_eq!(
            buttons(0x1000 | grips, LI_CCAP_SC26_BUTTONS),
            0x1000 | grips | BUTTONS_EXPLICIT
        );
    }

    #[test]
    fn haptics_go_to_a_client_with_the_pads_and_become_rumble_otherwise() {
        let report = [0x82, 0, 2, 0xfa];
        let payload = haptic_payload(&report);
        let haptic = SteamHaptic::new(3, &report).unwrap();
        assert_eq!(
            route(3, DRIVER_STEAM_HAPTIC, &payload, LI_CCAP_STEAM_HAPTIC),
            Some(Some(Feedback::Forward(
                STEAM_HAPTIC_MESSAGE_TYPE,
                haptic.encode().to_vec()
            )))
        );
        assert_eq!(
            route(3, DRIVER_STEAM_HAPTIC, &payload, LI_CCAP_SC26_BUTTONS),
            Some(Some(Feedback::Rumble(haptic)))
        );
        // A full ten-byte 0x80 rumble keeps every byte.
        let rumble = [0x80, 0, 0x10, 0x00, 0x34, 0x12, 0, 0x78, 0x56, 0];
        let decoded = driver_haptic(1, &haptic_payload(&rumble)).unwrap();
        assert_eq!(decoded.report(), rumble);
        assert_eq!(decoded.controller, 1);
    }

    #[test]
    fn malformed_haptics_are_dropped() {
        let mut payload = haptic_payload(&[0x81, 1, 0x90, 1, 0, 0, 1, 0]);
        assert_eq!(driver_haptic(0, &payload[..15]), None);
        payload[0] = 0;
        assert_eq!(driver_haptic(0, &payload), None);
        payload[0] = 13;
        assert_eq!(driver_haptic(0, &payload), None);
        assert_eq!(route(0, DRIVER_STEAM_HAPTIC, &payload, !0), Some(None));
    }

    #[test]
    fn settings_reach_only_a_client_that_takes_them() {
        let triples = [[0x30, 1, 0], [0x31, 0x40, 0x1f]];
        let payload = settings_payload(STEAM_SETTINGS_SET_VALUES, &triples);
        let settings = SteamSettings::new(2, STEAM_SETTINGS_SET_VALUES, &triples).unwrap();
        assert_eq!(
            route(2, DRIVER_STEAM_SETTINGS, &payload, LI_CCAP_STEAM_SETTINGS),
            Some(Some(Feedback::Forward(
                STEAM_SETTINGS_MESSAGE_TYPE,
                settings.encode().to_vec()
            )))
        );
        assert_eq!(
            route(2, DRIVER_STEAM_SETTINGS, &payload, LI_CCAP_STEAM_HAPTIC),
            Some(None)
        );
        let defaults = settings_payload(STEAM_SETTINGS_LOAD_DEFAULTS, &[]);
        assert_eq!(
            driver_settings(0, &defaults),
            Some(SteamSettings::new(0, STEAM_SETTINGS_LOAD_DEFAULTS, &[]).unwrap())
        );
        let full = [[1, 2, 3]; 9];
        assert_eq!(
            driver_settings(0, &settings_payload(STEAM_SETTINGS_SET_VALUES, &full))
                .unwrap()
                .triples(),
            full
        );
    }

    #[test]
    fn malformed_settings_are_dropped() {
        let mut payload = settings_payload(STEAM_SETTINGS_SET_VALUES, &[[1, 2, 3]]);
        assert_eq!(driver_settings(0, &payload[..31]), None);
        payload[1] = 10;
        assert_eq!(driver_settings(0, &payload), None);
        payload[1] = 1;
        payload[0] = 0x99;
        assert_eq!(driver_settings(0, &payload), None);
        let defaults_with_triples = settings_payload(STEAM_SETTINGS_LOAD_DEFAULTS, &[[1, 2, 3]]);
        assert_eq!(driver_settings(0, &defaults_with_triples), None);
    }

    #[test]
    fn other_feedback_is_not_steam_feedback() {
        for kind in [0, 1, 4, 5, 8] {
            assert_eq!(route(0, kind, &[0; 32], !0), None);
        }
    }

    #[test]
    fn the_attributes_request_matches_the_driver_layout() {
        let mut attributes = SteamAttributes {
            controller: 1,
            valid_mask: 0xff,
            firmware_build_time: 0x6a4d_85e3,
            bootloader_build_time: 0x68d2_f92e,
            board_revision: 74,
            capabilities: 0x1234,
            radio_firmware_build_time: 7,
            dongle_firmware_build_time: 8,
            ..Default::default()
        };
        attributes.build_id[..12].copy_from_slice(b"26e1f7ef1955");
        attributes.unit_serial[..3].copy_from_slice(b"FXA");
        attributes.board_serial[..2].copy_from_slice(b"BX");
        let b = attributes_request(5, &attributes);
        let word = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(word(0), 80);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), 2);
        assert_eq!(word(8), 5);
        // The radio and dongle bits (0x10, 0x20) have no wired tag.
        assert_eq!(word(12), 0xcf);
        assert_eq!(
            [word(16), word(20), word(24), word(28)],
            [0x6a4d_85e3, 0x68d2_f92e, 74, 0x1234]
        );
        assert_eq!(&b[32..44], b"26e1f7ef1955");
        assert_eq!(&b[48..51], b"FXA");
        assert_eq!(&b[64..66], b"BX");
        attributes.valid_mask = LI_SCATTR_SERIALS;
        assert_eq!(word_at(&attributes_request(0, &attributes), 12), 0x80);
    }

    #[test]
    fn a_virtual_controller_without_a_real_one_reports_firmware_steam_will_not_update() {
        // Issue 90: the driver's captured firmware (2026-07-07) is older than what Steam
        // ships, so Steam kept offering an update the virtual controller cannot take.
        const { assert!(VIRTUAL_FIRMWARE_BUILD_TIME > 0x6a4d_85e3) };
        let b = attributes_request(3, &virtual_attributes(2));
        assert_eq!(word_at(&b, 8), 3);
        // Only the firmware build time: the driver keeps its other captured values.
        assert_eq!(word_at(&b, 12), u32::from(LI_SCATTR_FIRMWARE_BUILD_TIME));
        assert_eq!(word_at(&b, 16), VIRTUAL_FIRMWARE_BUILD_TIME);
        assert!(b[32..].iter().all(|&byte| byte == 0));
    }

    fn word_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_single_touchpad_is_split_into_the_two_pads() {
        assert_eq!(touch_pad(0.2, 0, false), 0);
        assert_eq!(touch_pad(0.7, 0, false), 1);
        assert_eq!(touch_pad(0.7, 0, true), 0);
        assert_eq!(touch_pad(0.2, 1, true), 1);
        assert_eq!(touch_x(0.25, 0, false), 0.5);
        assert_eq!(touch_x(0.75, 1, false), 0.5);
        // A finger that went down on the left half and moved right stays on the left pad.
        assert_eq!(touch_x(0.8, 0, false), 1.0);
        assert_eq!(touch_x(0.3, 1, false), 0.0);
        assert_eq!(touch_x(0.3, 1, true), 0.3);
    }
}
