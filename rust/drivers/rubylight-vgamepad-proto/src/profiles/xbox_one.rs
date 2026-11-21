// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/xbox_one.h and xbox_one.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Xbox One S HID gamepad profile (`VID_045E&PID_02EA&IG_00` / `PID_02FF&IG_00`).

use super::xbox_series;
use crate::protocol::InputStateRequest;

pub const INPUT_REPORT_ID: u8 = xbox_series::INPUT_REPORT_ID;
pub const OUTPUT_REPORT_ID: u8 = xbox_series::OUTPUT_REPORT_ID;
pub const INPUT_REPORT_SIZE: usize = 16;

pub const PRODUCT_ID: u16 = 0x02EA;
pub const VERSION: u16 = 0x0408;

/// Series descriptor with the trailing Consumer Record (Share button) block removed.
#[rustfmt::skip]
pub const DESCRIPTOR: &[u8] = &[
    0x05, 0x01,                    // Usage Page (Generic Desktop)
    0x09, 0x05,                    // Usage (Game Pad)
    0xA1, 0x01,                    // Collection (Application)
    0x85, 0x01,                    //   Report ID (1)
    0x09, 0x01,                    //   Usage (Pointer)
    0xA1, 0x00,                    //   Collection (Physical)
    0x09, 0x30,                    //     Usage (X)
    0x09, 0x31,                    //     Usage (Y)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x27, 0xFF, 0xFF, 0x00, 0x00,  //     Logical Maximum (65535)
    0x95, 0x02,                    //     Report Count (2)
    0x75, 0x10,                    //     Report Size (16)
    0x81, 0x02,                    //     Input (Data, Variable, Absolute)
    0xC0,                          //   End Collection
    0x09, 0x01,                    //   Usage (Pointer)
    0xA1, 0x00,                    //   Collection (Physical)
    0x09, 0x32,                    //     Usage (Z)
    0x09, 0x35,                    //     Usage (Rz)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x27, 0xFF, 0xFF, 0x00, 0x00,  //     Logical Maximum (65535)
    0x95, 0x02,                    //     Report Count (2)
    0x75, 0x10,                    //     Report Size (16)
    0x81, 0x02,                    //     Input (Data, Variable, Absolute)
    0xC0,                          //   End Collection
    0x05, 0x02,                    //   Usage Page (Simulation Controls)
    0x09, 0xC5,                    //   Usage (Brake)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x26, 0xFF, 0x03,              //   Logical Maximum (1023)
    0x95, 0x01,                    //   Report Count (1)
    0x75, 0x0A,                    //   Report Size (10)
    0x81, 0x02,                    //   Input (Data, Variable, Absolute)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x00,                    //   Logical Maximum (0)
    0x75, 0x06,                    //   Report Size (6)
    0x95, 0x01,                    //   Report Count (1)
    0x81, 0x03,                    //   Input (Constant, Variable, Absolute)
    0x05, 0x02,                    //   Usage Page (Simulation Controls)
    0x09, 0xC4,                    //   Usage (Accelerator)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x26, 0xFF, 0x03,              //   Logical Maximum (1023)
    0x95, 0x01,                    //   Report Count (1)
    0x75, 0x0A,                    //   Report Size (10)
    0x81, 0x02,                    //   Input (Data, Variable, Absolute)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x00,                    //   Logical Maximum (0)
    0x75, 0x06,                    //   Report Size (6)
    0x95, 0x01,                    //   Report Count (1)
    0x81, 0x03,                    //   Input (Constant, Variable, Absolute)
    0x05, 0x01,                    //   Usage Page (Generic Desktop)
    0x09, 0x39,                    //   Usage (Hat switch)
    0x15, 0x01,                    //   Logical Minimum (1)
    0x25, 0x08,                    //   Logical Maximum (8)
    0x35, 0x00,                    //   Physical Minimum (0)
    0x46, 0x3B, 0x01,              //   Physical Maximum (315)
    0x66, 0x14, 0x00,              //   Unit (English Rotation: Degrees)
    0x75, 0x04,                    //   Report Size (4)
    0x95, 0x01,                    //   Report Count (1)
    0x81, 0x42,                    //   Input (Data, Variable, Absolute, Null state)
    0x75, 0x04,                    //   Report Size (4)
    0x95, 0x01,                    //   Report Count (1)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x00,                    //   Logical Maximum (0)
    0x35, 0x00,                    //   Physical Minimum (0)
    0x45, 0x00,                    //   Physical Maximum (0)
    0x65, 0x00,                    //   Unit (None)
    0x81, 0x03,                    //   Input (Constant, Variable, Absolute)
    0x05, 0x09,                    //   Usage Page (Button)
    0x19, 0x01,                    //   Usage Minimum (1)
    0x29, 0x0F,                    //   Usage Maximum (15)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x01,                    //   Logical Maximum (1)
    0x75, 0x01,                    //   Report Size (1)
    0x95, 0x0F,                    //   Report Count (15)
    0x81, 0x02,                    //   Input (Data, Variable, Absolute)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x00,                    //   Logical Maximum (0)
    0x75, 0x01,                    //   Report Size (1)
    0x95, 0x01,                    //   Report Count (1)
    0x81, 0x03,                    //   Input (Constant, Variable, Absolute)
    0x05, 0x0F,                    //   Usage Page (Physical Interface Device)
    0x09, 0x21,                    //   Usage (Set Effect Report)
    0x85, 0x03,                    //   Report ID (3)
    0xA1, 0x02,                    //   Collection (Logical)
    0x09, 0x97,                    //     Usage (DC Enable Actuators)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x25, 0x01,                    //     Logical Maximum (1)
    0x75, 0x04,                    //     Report Size (4)
    0x95, 0x01,                    //     Report Count (1)
    0x91, 0x02,                    //     Output (Data, Variable, Absolute)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x25, 0x00,                    //     Logical Maximum (0)
    0x75, 0x04,                    //     Report Size (4)
    0x95, 0x01,                    //     Report Count (1)
    0x91, 0x03,                    //     Output (Constant, Variable, Absolute)
    0x09, 0x70,                    //     Usage (Magnitude)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x25, 0x64,                    //     Logical Maximum (100)
    0x75, 0x08,                    //     Report Size (8)
    0x95, 0x04,                    //     Report Count (4)
    0x91, 0x02,                    //     Output (Data, Variable, Absolute)
    0x09, 0x50,                    //     Usage (Duration)
    0x66, 0x01, 0x10,              //     Unit (SI Linear: Seconds)
    0x55, 0x0E,                    //     Unit Exponent (-2)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x26, 0xFF, 0x00,              //     Logical Maximum (255)
    0x75, 0x08,                    //     Report Size (8)
    0x95, 0x01,                    //     Report Count (1)
    0x91, 0x02,                    //     Output (Data, Variable, Absolute)
    0x09, 0xA7,                    //     Usage (Start Delay)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x26, 0xFF, 0x00,              //     Logical Maximum (255)
    0x75, 0x08,                    //     Report Size (8)
    0x95, 0x01,                    //     Report Count (1)
    0x91, 0x02,                    //     Output (Data, Variable, Absolute)
    0x65, 0x00,                    //     Unit (None)
    0x55, 0x00,                    //     Unit Exponent (0)
    0x09, 0x7C,                    //     Usage (Loop Count)
    0x15, 0x00,                    //     Logical Minimum (0)
    0x26, 0xFF, 0x00,              //     Logical Maximum (255)
    0x75, 0x08,                    //     Report Size (8)
    0x95, 0x01,                    //     Report Count (1)
    0x91, 0x02,                    //     Output (Data, Variable, Absolute)
    0xC0,                          //   End Collection
    0xC0,                          // End Collection
];

pub const HARDWARE_IDS_UTF16: &[u16] = {
    const RAW: &[u8] = b"HID\\VID_045E&PID_02EA&IG_00\0HID\\VID_045E&PID_02FF&IG_00\0\0";
    const LEN: usize = RAW.len();
    const ARR: [u16; LEN] = {
        let mut out = [0u16; LEN];
        let mut i = 0;
        while i < LEN {
            out[i] = RAW[i] as u16;
            i += 1;
        }
        out
    };
    &ARR
};

pub fn encode_input(input: &InputStateRequest) -> [u8; INPUT_REPORT_SIZE] {
    let full = xbox_series::encode_input(input);
    let mut out = [0u8; INPUT_REPORT_SIZE];
    out.copy_from_slice(&full[..INPUT_REPORT_SIZE]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_matches_xbox_series_without_consumer_record() {
        let series = xbox_series::DESCRIPTOR;
        let marker = [0x05, 0x0C, 0x0A, 0xB2, 0x00];
        let pid_marker = [0x05, 0x0F, 0x09, 0x21];
        let start = series
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap();
        let end = series
            .windows(pid_marker.len())
            .position(|w| w == pid_marker)
            .unwrap();
        let mut derived = [0u8; 512];
        let len = start + (series.len() - end);
        derived[..start].copy_from_slice(&series[..start]);
        derived[start..len].copy_from_slice(&series[end..]);
        assert_eq!(&derived[..len], DESCRIPTOR);
    }
}
