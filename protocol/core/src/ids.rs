//! Every control-stream message type and input packet magic in use, in one place, so a new
//! one cannot reuse an old one.
//!
//! Control message types are the u16 at the start of a control-stream packet; input magics
//! are the u32 LE at offset 4 of an input packet, after its 4-byte big-endian length. The
//! tables list what moonlight-common-c, the Sunshine family and Rubylight use, the numbers
//! held back for later, and the range other clients use, which Rubylight keeps out of except
//! for the messages it takes over from them as they are.

use crate::{
    clipboard_sync, control, cursor_shape, fec_report, keyboard_layout, pen, phase_lock,
    pipeline_rates, rate_change, steam_controller, text_input, usb,
};
use core::ops::RangeInclusive;

/// Control message types in use, with what they carry.
pub const CONTROL_IDS: &[(u16, &str)] = &[
    (0x0001, "encrypted control packet"),
    (0x0100, "GameStream control"),
    (0x0109, "termination"),
    (0x010b, "rumble"),
    (0x010e, "HDR mode"),
    (0x0200, "periodic ping"),
    (0x0201, "loss statistics"),
    (0x0204, "frame statistics"),
    (0x0206, "input"),
    (0x0301, "invalidate reference frames"),
    (0x0302, "IDR request"),
    (0x0305, "start A"),
    (0x0307, "start B"),
    (0x3000, "Apollo host extension"),
    (0x5500, "trigger rumble"),
    (0x5501, "motion event request"),
    (
        0x5502,
        "RGB LED (host to client), frame FEC status (client to host)",
    ),
    (0x5503, "adaptive triggers"),
    (
        clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE,
        "clipboard sync (Moonlight V+)",
    ),
    (
        fec_report::FEC_SUMMARY_MESSAGE_TYPE,
        "FEC summary (Moonlight V+, Foundation Sunshine)",
    ),
    (
        steam_controller::STEAM_HAPTIC_MESSAGE_TYPE,
        "Steam Controller haptics",
    ),
    (
        steam_controller::STEAM_SETTINGS_MESSAGE_TYPE,
        "Steam Controller settings",
    ),
    (phase_lock::REPORT_MESSAGE_TYPE, "phase lock report"),
    (control::DISPLAY_CAPS_MESSAGE_TYPE, "display luminance"),
    (control::RECONFIGURE_MESSAGE_TYPE, "reconfigure"),
    (0x5533, "controller speaker"),
    (0x5534, "controller microphone"),
    (0x5535, "controller audio control"),
    (control::HOST_TIMING_MESSAGE_TYPE, "host timing"),
    (control::CLOCK_SYNC_MESSAGE_TYPE, "clock sync"),
    (control::LINK_NOTICE_MESSAGE_TYPE, "link notice"),
    (fec_report::FEC_REPORT_MESSAGE_TYPE, "FEC report"),
    (usb::USB_MESSAGE_TYPE, "USB forwarding"),
    (text_input::TEXT_INPUT_MESSAGE_TYPE, "text input"),
    (rate_change::RATE_CHANGE_MESSAGE_TYPE, "rate change"),
    (
        pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE,
        "pipeline rates",
    ),
    (
        keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE,
        "keyboard layout",
    ),
    (
        cursor_shape::CURSOR_SHAPE_REQUEST_MESSAGE_TYPE,
        "cursor shape request",
    ),
    (cursor_shape::CURSOR_SHAPE_MESSAGE_TYPE, "cursor shape"),
    (0x5601, "DualSense haptics"),
];

/// Control message types held back for a message that may be needed later.
pub const RESERVED_CONTROL_IDS: &[(u16, &str)] = &[(0x5542, "cursor position")];

/// Control message types other clients use; Rubylight defines nothing here.
pub const OTHER_CLIENTS_CONTROL_IDS: RangeInclusive<u16> = 0x5506..=0x5510;

/// Control message types of other clients Rubylight takes with their wire format unchanged.
pub const ADOPTED_CONTROL_IDS: &[u16] = &[
    clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE,
    fec_report::FEC_SUMMARY_MESSAGE_TYPE,
];

/// A capability bit (DualSense haptics) that must never become a message type.
pub const HAPTICS_CAPABILITY: u16 = 0x8000;

/// The first and last control message types Rubylight 3.0 defines.
pub const RUBYLIGHT_3_CONTROL_IDS: RangeInclusive<u16> = 0x5539..=0x5542;

/// Input packet magics in use, with what they carry.
pub const INPUT_MAGICS: &[(u32, &str)] = &[
    (3, "key down"),
    (4, "key up"),
    (5, "absolute mouse"),
    (6, "relative mouse (old)"),
    (7, "relative mouse"),
    (8, "mouse button down"),
    (9, "mouse button up"),
    (10, "scroll"),
    (12, "controller"),
    (13, "haptics enable"),
    (23, "UTF-8 text"),
    (0x5500_0001, "horizontal scroll"),
    (0x5500_0002, "touch"),
    (pen::PEN_MAGIC, "pen"),
    (0x5500_0004, "controller arrival"),
    (0x5500_0005, "controller touch"),
    (0x5500_0006, "controller motion"),
    (0x5500_0007, "controller battery"),
    (
        steam_controller::STEAM_ATTRIBUTES_MAGIC,
        "Steam Controller attributes",
    ),
    (pen::PEN_BARREL_ROLL_MAGIC, "pen with barrel roll"),
];

/// Input magics other clients use for touchpad packets.
pub const RESERVED_INPUT_MAGICS: [u32; 2] = pen::RESERVED_MAGICS;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn control_ids_are_unique() {
        let mut ids: Vec<u16> = CONTROL_IDS
            .iter()
            .chain(RESERVED_CONTROL_IDS)
            .map(|(id, _)| *id)
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "a control message type is listed twice");
    }

    #[test]
    fn nothing_lands_where_other_clients_are() {
        for (id, meaning) in CONTROL_IDS.iter().chain(RESERVED_CONTROL_IDS) {
            assert!(
                !OTHER_CLIENTS_CONTROL_IDS.contains(id) || ADOPTED_CONTROL_IDS.contains(id),
                "{id:#06x} {meaning}"
            );
            assert_ne!(*id, HAPTICS_CAPABILITY, "{meaning}");
            assert_eq!(id & HAPTICS_CAPABILITY, 0, "{id:#06x} {meaning}");
        }
    }

    #[test]
    fn adopted_ids_keep_their_meaning_from_other_clients() {
        assert_eq!(ADOPTED_CONTROL_IDS, [0x5508, 0x5510]);
        for id in ADOPTED_CONTROL_IDS {
            assert!(OTHER_CLIENTS_CONTROL_IDS.contains(id), "{id:#06x}");
            assert!(
                CONTROL_IDS.iter().any(|(listed, _)| listed == id),
                "{id:#06x}"
            );
        }
    }

    #[test]
    fn reserved_ids_stay_free() {
        for (reserved, _) in RESERVED_CONTROL_IDS {
            assert!(RUBYLIGHT_3_CONTROL_IDS.contains(reserved));
            assert!(
                CONTROL_IDS.iter().all(|(id, _)| id != reserved),
                "{reserved:#06x}"
            );
        }
        assert_eq!(
            RESERVED_CONTROL_IDS
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [0x5542]
        );
        let new: Vec<u16> = CONTROL_IDS
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| RUBYLIGHT_3_CONTROL_IDS.contains(id))
            .collect();
        assert_eq!(
            new,
            [0x5539, 0x553A, 0x553B, 0x553C, 0x553D, 0x553E, 0x5540, 0x5541]
        );
    }

    #[test]
    fn input_magics_are_unique() {
        let mut magics: Vec<u32> = INPUT_MAGICS
            .iter()
            .map(|(magic, _)| *magic)
            .chain(RESERVED_INPUT_MAGICS)
            .collect();
        let count = magics.len();
        magics.sort_unstable();
        magics.dedup();
        assert_eq!(magics.len(), count, "an input magic is listed twice");
        assert_eq!(RESERVED_INPUT_MAGICS, [0x5500_0009, 0x5500_000A]);
    }
}
