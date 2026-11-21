// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/profile.h and profile.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Portable virtual gamepad profiles, HID descriptors, input/output codecs,
//! and DirectInput force feedback.

pub mod dualsense;
pub mod dualshock4;
pub mod generic;
pub mod pid_ff;
pub mod steam_controller;
pub mod switch_pro;
pub mod xbox_one;
pub mod xbox_series;

use crate::protocol::Profile;

const HID_DS4_HARDWARE_IDS: &[u16] = utf16_slice!(b"HID\\VID_054C&PID_09CC\0\0");
const HID_DS5_HARDWARE_IDS: &[u16] = utf16_slice!(b"HID\\VID_054C&PID_0CE6\0\0");
const HID_SWITCH_HARDWARE_IDS: &[u16] = utf16_slice!(b"HID\\VID_057E&PID_2009\0\0");
const HID_SC26_HARDWARE_IDS: &[u16] = utf16_slice!(b"HID\\VID_28DE&PID_1302\0\0");

macro_rules! utf16_slice {
    ($bytes:literal) => {{
        const LEN: usize = $bytes.len();
        const ARR: [u16; LEN] = {
            let mut out = [0u16; LEN];
            let mut i = 0;
            while i < LEN {
                out[i] = $bytes[i] as u16;
                i += 1;
            }
            out
        };
        &ARR
    }};
}
use utf16_slice;

/// Metadata and HID descriptor for an implemented virtual gamepad profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileDefinition {
    pub id: Profile,
    pub report_descriptor: &'static [u8],
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
    pub force_feedback: bool,
    /// `REG_MULTI_SZ` UTF-16 code units (null-terminated list ending with an extra null).
    pub hardware_ids_utf16: &'static [u16],
}

pub const fn is_playstation(profile: Profile) -> bool {
    matches!(profile, Profile::DualShock4 | Profile::DualSense)
}

pub const fn is_xbox(profile: Profile) -> bool {
    matches!(profile, Profile::XboxSeries | Profile::XboxOne)
}

pub const fn is_steam_controller(profile: Profile) -> bool {
    matches!(profile, Profile::SteamController)
}

const XBOX_SERIES_DEF: ProfileDefinition = ProfileDefinition {
    id: Profile::XboxSeries,
    report_descriptor: xbox_series::DESCRIPTOR,
    vendor_id: xbox_series::VENDOR_ID,
    product_id: xbox_series::SERIES_PRODUCT_ID,
    version_number: xbox_series::SERIES_VERSION,
    force_feedback: false,
    hardware_ids_utf16: xbox_series::HARDWARE_IDS_UTF16,
};

const XBOX_ONE_DEF: ProfileDefinition = ProfileDefinition {
    id: Profile::XboxOne,
    report_descriptor: xbox_one::DESCRIPTOR,
    vendor_id: xbox_series::VENDOR_ID,
    product_id: xbox_one::PRODUCT_ID,
    version_number: xbox_one::VERSION,
    force_feedback: false,
    hardware_ids_utf16: xbox_one::HARDWARE_IDS_UTF16,
};

const DS4_DEF: ProfileDefinition = ProfileDefinition {
    id: Profile::DualShock4,
    report_descriptor: dualshock4::REPORT_DESCRIPTOR,
    vendor_id: dualshock4::VENDOR_ID,
    product_id: dualshock4::PRODUCT_ID,
    version_number: dualshock4::VERSION,
    force_feedback: false,
    hardware_ids_utf16: HID_DS4_HARDWARE_IDS,
};

const DS5_DEF: ProfileDefinition = ProfileDefinition {
    id: Profile::DualSense,
    report_descriptor: dualsense::REPORT_DESCRIPTOR,
    vendor_id: dualsense::VENDOR_ID,
    product_id: dualsense::PRODUCT_ID,
    version_number: dualsense::VERSION,
    force_feedback: false,
    hardware_ids_utf16: HID_DS5_HARDWARE_IDS,
};

const SWITCH_PRO_DEF: ProfileDefinition = ProfileDefinition {
    id: Profile::SwitchPro,
    report_descriptor: switch_pro::DESCRIPTOR,
    vendor_id: switch_pro::VENDOR_ID,
    product_id: switch_pro::PRODUCT_ID,
    version_number: switch_pro::VERSION,
    force_feedback: false,
    hardware_ids_utf16: HID_SWITCH_HARDWARE_IDS,
};

const STEAM_CONTROLLER_DEF: ProfileDefinition = ProfileDefinition {
    id: Profile::SteamController,
    report_descriptor: steam_controller::REPORT_DESCRIPTOR,
    vendor_id: steam_controller::VENDOR_ID,
    product_id: steam_controller::PRODUCT_ID,
    version_number: steam_controller::VERSION,
    force_feedback: false,
    hardware_ids_utf16: HID_SC26_HARDWARE_IDS,
};

/// Returns the shipped definition for `profile`, or `None` for reserved/unsupported profiles.
pub const fn find_profile(profile: Profile) -> Option<&'static ProfileDefinition> {
    match profile {
        Profile::GenericHid | Profile::GenericPid | Profile::Xbox360 => None,
        Profile::XboxOne => Some(&XBOX_ONE_DEF),
        Profile::XboxSeries => Some(&XBOX_SERIES_DEF),
        Profile::DualShock4 => Some(&DS4_DEF),
        Profile::DualSense => Some(&DS5_DEF),
        Profile::SwitchPro => Some(&SWITCH_PRO_DEF),
        Profile::SteamController => {
            if steam_controller::REPORT_DESCRIPTOR_IS_PROVISIONAL {
                None
            } else {
                Some(&STEAM_CONTROLLER_DEF)
            }
        }
    }
}

/// Bitmask of all public profiles accepted by [`find_profile`].
pub const fn available_profiles() -> u32 {
    let candidates = [
        Profile::GenericHid,
        Profile::GenericPid,
        Profile::Xbox360,
        Profile::XboxOne,
        Profile::XboxSeries,
        Profile::DualShock4,
        Profile::DualSense,
        Profile::SwitchPro,
        Profile::SteamController,
    ];
    let mut mask = 0u32;
    let mut i = 0;
    while i < candidates.len() {
        if find_profile(candidates[i]).is_some() {
            mask |= candidates[i].bit();
        }
        i += 1;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::DEVICE_INTERFACE_GUID;

    #[derive(Clone, Copy, Default)]
    struct ParsedReport {
        id: u8,
        input_bits: usize,
        output_bits: usize,
        feature_bits: usize,
        feature_page: u32,
        feature_usage: u32,
        feature_count: usize,
    }

    struct WalkSummary {
        reports: [ParsedReport; 64],
        report_count: usize,
        top_level_collections: usize,
        app_pages: [u32; 8],
    }

    fn walk_hid_descriptor(data: &[u8]) -> WalkSummary {
        let mut out = WalkSummary {
            reports: [ParsedReport::default(); 64],
            report_count: 0,
            top_level_collections: 0,
            app_pages: [0; 8],
        };
        let mut page = 0u32;
        let mut usage = 0u32;
        let mut id = 0u8;
        let mut size = 0usize;
        let mut count = 0usize;
        let mut depth = 0i32;
        let mut off = 0usize;

        while off < data.len() {
            let prefix = data[off];
            off += 1;
            assert_ne!(prefix, 0xFE, "unexpected long HID item");
            let len = if (prefix & 3) == 3 {
                4usize
            } else {
                usize::from(prefix & 3)
            };
            assert!(off + len <= data.len(), "truncated HID item at {off}");
            let mut value = 0u32;
            for j in 0..len {
                value |= u32::from(data[off + j]) << (8 * j);
            }
            off += len;

            let ty = (prefix >> 2) & 0x03;
            let tag = (prefix >> 4) & 0x0F;
            match (ty, tag) {
                (1, 0x0) => page = value,
                (1, 0x7) => size = value as usize,
                (1, 0x8) => id = value as u8,
                (1, 0x9) => count = value as usize,
                (2, 0x0) => usage = value,
                (0, 0xA) => {
                    if depth == 0 && out.top_level_collections < out.app_pages.len() {
                        out.app_pages[out.top_level_collections] = (page << 16) | usage;
                        out.top_level_collections += 1;
                    }
                    depth += 1;
                }
                (0, 0xC) => {
                    depth -= 1;
                    assert!(depth >= 0, "unbalanced End Collection");
                }
                (0, 0x8 | 0x9 | 0xB) => {
                    assert!(depth > 0, "main item outside collection");
                    let bits = size * count;
                    let idx = out.reports[..out.report_count]
                        .iter()
                        .position(|r| r.id == id)
                        .unwrap_or_else(|| {
                            let i = out.report_count;
                            out.reports[i].id = id;
                            out.report_count += 1;
                            i
                        });
                    match tag {
                        0x8 => out.reports[idx].input_bits += bits,
                        0x9 => out.reports[idx].output_bits += bits,
                        _ => {
                            out.reports[idx].feature_bits += bits;
                            out.reports[idx].feature_page = page;
                            out.reports[idx].feature_usage = usage;
                            out.reports[idx].feature_count = count;
                        }
                    }
                }
                _ => {}
            }
            if ty == 0 {
                usage = 0;
            }
        }
        assert_eq!(depth, 0, "unclosed collection");
        out
    }

    fn find_report(summary: &WalkSummary, id: u8) -> Option<ParsedReport> {
        summary.reports[..summary.report_count]
            .iter()
            .copied()
            .find(|r| r.id == id)
    }

    #[test]
    fn available_profiles_matches_upstream_six_profiles() {
        assert_eq!(available_profiles(), 0x17C);
        assert!(find_profile(Profile::GenericHid).is_none());
        assert!(find_profile(Profile::GenericPid).is_none());
        assert!(find_profile(Profile::Xbox360).is_none());
        assert!(find_profile(Profile::XboxOne).is_some());
        assert!(find_profile(Profile::XboxSeries).is_some());
        assert!(find_profile(Profile::DualShock4).is_some());
        assert!(find_profile(Profile::DualSense).is_some());
        assert!(find_profile(Profile::SwitchPro).is_some());
        assert!(find_profile(Profile::SteamController).is_some());
        assert_ne!(
            DEVICE_INTERFACE_GUID, 0x27debbf5_1d1e_4e9c_906d_d104b1418b2b,
            "Rubylight interface GUID must stay distinct from Vibeshine's"
        );
    }

    #[test]
    fn shipped_profiles_use_valid_double_nul_terminated_multi_sz_hardware_ids() {
        for profile in [
            Profile::XboxOne,
            Profile::XboxSeries,
            Profile::DualShock4,
            Profile::DualSense,
            Profile::SwitchPro,
            Profile::SteamController,
        ] {
            let def = find_profile(profile).unwrap();
            let hw = def.hardware_ids_utf16;
            assert!(hw.len() >= 4, "{profile:?} hardware IDs too short");
            assert_eq!(
                &hw[hw.len() - 2..],
                &[0, 0],
                "{profile:?} missing double NUL"
            );
            assert_ne!(
                hw[hw.len() - 3],
                0,
                "{profile:?} ends with empty string entry"
            );
            assert!(
                !hw[..hw.len() - 1].windows(2).any(|pair| pair == [0, 0]),
                "{profile:?} has premature double NUL"
            );
        }
    }

    #[test]
    fn all_descriptors_parse_with_exact_report_sizes_and_usages() {
        let xs = walk_hid_descriptor(xbox_series::DESCRIPTOR);
        assert_eq!(xs.top_level_collections, 1);
        assert_eq!(
            find_report(&xs, 1).unwrap().input_bits / 8 + 1,
            xbox_series::INPUT_REPORT_SIZE
        );
        assert_eq!(
            find_report(&xs, 3).unwrap().output_bits / 8 + 1,
            xbox_series::OUTPUT_REPORT_SIZE
        );

        let xo = walk_hid_descriptor(xbox_one::DESCRIPTOR);
        assert_eq!(xo.top_level_collections, 1);
        assert_eq!(
            find_report(&xo, 1).unwrap().input_bits / 8 + 1,
            xbox_one::INPUT_REPORT_SIZE
        );
        assert_eq!(
            find_report(&xo, 3).unwrap().output_bits / 8 + 1,
            xbox_series::OUTPUT_REPORT_SIZE
        );

        let ds4 = walk_hid_descriptor(dualshock4::REPORT_DESCRIPTOR);
        assert_eq!(find_report(&ds4, 1).unwrap().input_bits / 8 + 1, 64);
        assert_eq!(find_report(&ds4, 5).unwrap().output_bits / 8 + 1, 32);
        let a3 = find_report(&ds4, 0xA3).unwrap();
        assert_eq!(
            (a3.feature_page, a3.feature_usage, a3.feature_count),
            (0xFF80, 0x43, 48)
        );
        let p12 = find_report(&ds4, 0x12).unwrap();
        assert_eq!(
            (p12.feature_page, p12.feature_usage, p12.feature_count),
            (0xFF02, 0x21, 15)
        );
        let p14 = find_report(&ds4, 0x14).unwrap();
        assert_eq!(
            (p14.feature_page, p14.feature_usage, p14.feature_count),
            (0xFF05, 0x20, 16)
        );
        assert!(find_report(&ds4, 6).is_none());

        let ds5 = walk_hid_descriptor(dualsense::REPORT_DESCRIPTOR);
        assert_eq!(find_report(&ds5, 1).unwrap().input_bits / 8 + 1, 64);
        assert_eq!(find_report(&ds5, 2).unwrap().output_bits / 8 + 1, 48);
        let f85 = find_report(&ds5, 0x85).unwrap();
        assert_eq!(
            (f85.feature_page, f85.feature_usage, f85.feature_count),
            (0xFF00, 0x2D, 2)
        );

        let sw = walk_hid_descriptor(switch_pro::DESCRIPTOR);
        for id in [0x21u8, 0x30, 0x81] {
            assert_eq!(find_report(&sw, id).unwrap().input_bits / 8 + 1, 64);
        }
        for id in [0x01u8, 0x10, 0x80] {
            assert_eq!(find_report(&sw, id).unwrap().output_bits / 8 + 1, 64);
        }

        let sc = walk_hid_descriptor(steam_controller::REPORT_DESCRIPTOR);
        assert_eq!(sc.top_level_collections, 3);
        assert_eq!(&sc.app_pages[..3], &[0x0001_0002, 0x0001_0006, 0xFF00_0001]);
        for r in &sc.reports[..sc.report_count] {
            let bits = r.input_bits + r.output_bits + r.feature_bits;
            assert_eq!(bits % 8, 0);
            assert_eq!(
                steam_controller::report_size(r.id),
                bits / 8 + 1,
                "report {:#x}",
                r.id
            );
        }
        let table_count = (0u8..=255)
            .filter(|&id| steam_controller::report_size(id) != 0)
            .count();
        assert_eq!(table_count, sc.report_count);

        let pid = walk_hid_descriptor(pid_ff::PID_GAMEPAD_DESCRIPTOR);
        assert_eq!(pid.top_level_collections, 2);
        assert_eq!(pid.report_count, pid_ff::PID_REPORT_BITS.len());
        for want in pid_ff::PID_REPORT_BITS {
            let got = find_report(&pid, want.report_id).unwrap();
            assert_eq!(
                got.input_bits, want.input_bits,
                "pid input {}",
                want.report_id
            );
            assert_eq!(
                got.output_bits, want.output_bits,
                "pid output {}",
                want.report_id
            );
            assert_eq!(
                got.feature_bits, want.feature_bits,
                "pid feature {}",
                want.report_id
            );
        }
    }
}
