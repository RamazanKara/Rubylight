// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/xbox_series.h and xbox_series.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Xbox Series HID gamepad profile (`VID_045E&PID_0B12&IG_00` / `PID_02FF&IG_00`).

use crate::protocol::{
    FeedbackEvent, FeedbackType, InputStateRequest, XboxRumbleFeedback, button_mask,
};

pub const INPUT_REPORT_ID: u8 = 1;
pub const OUTPUT_REPORT_ID: u8 = 3;

pub const INPUT_REPORT_SIZE: usize = 17;
pub const OUTPUT_REPORT_SIZE: usize = 9;

pub const VENDOR_ID: u16 = 0x045E;
pub const SERIES_PRODUCT_ID: u16 = 0x0B12;
pub const GIP_SOFTWARE_PRODUCT_ID: u16 = 0x02FF;
pub const SERIES_VERSION: u16 = 0x0509;

pub const TRIGGER_MAX: u16 = 1023;
pub const RUMBLE_MAX: u8 = 100;
pub const ENABLE_RIGHT_TRIGGER: u8 = 0x01;
pub const ENABLE_LEFT_TRIGGER: u8 = 0x02;
pub const ENABLE_RIGHT_MOTOR: u8 = 0x04;
pub const ENABLE_LEFT_MOTOR: u8 = 0x08;

pub mod xbox_button {
    pub const A: u16 = 1 << 0;
    pub const B: u16 = 1 << 1;
    pub const X: u16 = 1 << 3;
    pub const Y: u16 = 1 << 4;
    pub const LEFT_SHOULDER: u16 = 1 << 6;
    pub const RIGHT_SHOULDER: u16 = 1 << 7;
    pub const VIEW: u16 = 1 << 10;
    pub const MENU: u16 = 1 << 11;
    pub const GUIDE: u16 = 1 << 12;
    pub const LEFT_STICK: u16 = 1 << 13;
    pub const RIGHT_STICK: u16 = 1 << 14;
}

/// Xbox Series HID report descriptor (unsigned 16-bit sticks, 10-bit triggers,
/// 1-based hat with null state, 15 buttons with gaps, Consumer Record share, PID rumble).
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
    0x05, 0x0C,                    //   Usage Page (Consumer)
    0x0A, 0xB2, 0x00,              //   Usage (Record)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x01,                    //   Logical Maximum (1)
    0x95, 0x01,                    //   Report Count (1)
    0x75, 0x01,                    //   Report Size (1)
    0x81, 0x02,                    //   Input (Data, Variable, Absolute)
    0x15, 0x00,                    //   Logical Minimum (0)
    0x25, 0x00,                    //   Logical Maximum (0)
    0x75, 0x07,                    //   Report Size (7)
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
    const RAW: &[u8] = b"HID\\VID_045E&PID_0B12&IG_00\0HID\\VID_045E&PID_02FF&IG_00\0\0";
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

fn encode_hat(buttons: u32) -> u8 {
    let up = (buttons & button_mask::DPAD_UP) != 0;
    let down = (buttons & button_mask::DPAD_DOWN) != 0;
    let left = (buttons & button_mask::DPAD_LEFT) != 0;
    let right = (buttons & button_mask::DPAD_RIGHT) != 0;
    if (up && down) || (left && right) {
        return 0;
    }
    if up {
        return if right {
            2
        } else if left {
            8
        } else {
            1
        };
    }
    if down {
        return if right {
            4
        } else if left {
            6
        } else {
            5
        };
    }
    if right {
        3
    } else if left {
        7
    } else {
        0
    }
}

fn map_buttons(buttons: u32) -> u16 {
    let mut mapped = 0u16;
    if (buttons & button_mask::SOUTH) != 0 {
        mapped |= xbox_button::A;
    }
    if (buttons & button_mask::EAST) != 0 {
        mapped |= xbox_button::B;
    }
    if (buttons & button_mask::WEST) != 0 {
        mapped |= xbox_button::X;
    }
    if (buttons & button_mask::NORTH) != 0 {
        mapped |= xbox_button::Y;
    }
    if (buttons & button_mask::LEFT_SHOULDER) != 0 {
        mapped |= xbox_button::LEFT_SHOULDER;
    }
    if (buttons & button_mask::RIGHT_SHOULDER) != 0 {
        mapped |= xbox_button::RIGHT_SHOULDER;
    }
    if (buttons & button_mask::BACK) != 0 {
        mapped |= xbox_button::VIEW;
    }
    if (buttons & button_mask::START) != 0 {
        mapped |= xbox_button::MENU;
    }
    if (buttons & button_mask::HOME) != 0 {
        mapped |= xbox_button::GUIDE;
    }
    if (buttons & button_mask::LEFT_STICK) != 0 {
        mapped |= xbox_button::LEFT_STICK;
    }
    if (buttons & button_mask::RIGHT_STICK) != 0 {
        mapped |= xbox_button::RIGHT_STICK;
    }
    mapped
}

const fn to_unsigned_axis(value: i16) -> u16 {
    (value as i32 + 32768) as u16
}

const fn to_unsigned_vertical_axis(value: i16) -> u16 {
    (32767 - value as i32) as u16
}

const fn to_trigger(value: u8) -> u16 {
    ((value as u32 * TRIGGER_MAX as u32) / 255) as u16
}

const fn from_rumble_percent(value: u8) -> u16 {
    let clamped = if value > RUMBLE_MAX {
        RUMBLE_MAX as u32
    } else {
        value as u32
    };
    ((clamped * 65535) / RUMBLE_MAX as u32) as u16
}

pub fn encode_input(input: &InputStateRequest) -> [u8; INPUT_REPORT_SIZE] {
    let mut out = [0u8; INPUT_REPORT_SIZE];
    out[0] = INPUT_REPORT_ID;
    out[1..3].copy_from_slice(&to_unsigned_axis(input.left_x).to_le_bytes());
    out[3..5].copy_from_slice(&to_unsigned_vertical_axis(input.left_y).to_le_bytes());
    out[5..7].copy_from_slice(&to_unsigned_axis(input.right_x).to_le_bytes());
    out[7..9].copy_from_slice(&to_unsigned_vertical_axis(input.right_y).to_le_bytes());
    out[9..11].copy_from_slice(&to_trigger(input.left_trigger).to_le_bytes());
    out[11..13].copy_from_slice(&to_trigger(input.right_trigger).to_le_bytes());
    out[13] = encode_hat(input.buttons);
    out[14..16].copy_from_slice(&map_buttons(input.buttons).to_le_bytes());
    out[16] = u8::from((input.buttons & (button_mask::MISC | button_mask::TOUCHPAD)) != 0);
    out
}

pub fn decode_output(output: &[u8]) -> Option<XboxRumbleFeedback> {
    if output.len() < OUTPUT_REPORT_SIZE || output[0] != OUTPUT_REPORT_ID {
        return None;
    }
    let enable = output[1];
    let left_trigger_motor = output[2];
    let right_trigger_motor = output[3];
    let left_motor = output[4];
    let right_motor = output[5];

    Some(XboxRumbleFeedback {
        low_frequency: if (enable & ENABLE_LEFT_MOTOR) != 0 {
            from_rumble_percent(left_motor)
        } else {
            0
        },
        high_frequency: if (enable & ENABLE_RIGHT_MOTOR) != 0 {
            from_rumble_percent(right_motor)
        } else {
            0
        },
        left_trigger: if (enable & ENABLE_LEFT_TRIGGER) != 0 {
            from_rumble_percent(left_trigger_motor)
        } else {
            0
        },
        right_trigger: if (enable & ENABLE_RIGHT_TRIGGER) != 0 {
            from_rumble_percent(right_trigger_motor)
        } else {
            0
        },
    })
}

pub fn encode_feedback(controller_id: u32, rumble: &XboxRumbleFeedback) -> FeedbackEvent {
    FeedbackEvent::new(controller_id, FeedbackType::XboxRumble, &rumble.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_centers_axes_and_scales_triggers() {
        let report = encode_input(&InputStateRequest::default());
        assert_eq!(report[0], INPUT_REPORT_ID);
        assert_eq!(u16::from_le_bytes([report[1], report[2]]), 32768);
        assert_eq!(u16::from_le_bytes([report[3], report[4]]), 32767);
        assert_eq!(report[13], 0);
        assert_eq!(report[16], 0);

        let full = InputStateRequest {
            buttons: button_mask::SOUTH | button_mask::DPAD_UP | button_mask::MISC,
            left_x: i16::MAX,
            left_y: i16::MIN,
            left_trigger: 255,
            ..Default::default()
        };
        let r = encode_input(&full);
        assert_eq!(u16::from_le_bytes([r[9], r[10]]), 1023);
        assert_eq!(r[13], 1);
        assert_eq!(u16::from_le_bytes([r[14], r[15]]), xbox_button::A);
        assert_eq!(r[16], 1);
    }

    #[test]
    fn decode_output_honors_enable_mask() {
        let pkt = [
            OUTPUT_REPORT_ID,
            ENABLE_LEFT_MOTOR | ENABLE_RIGHT_TRIGGER,
            80,
            50,
            100,
            90,
            0,
            0,
            0,
        ];
        let fb = decode_output(&pkt).unwrap();
        assert_eq!(fb.low_frequency, 65535);
        assert_eq!(fb.high_frequency, 0);
        assert_eq!(fb.left_trigger, 0);
        assert_eq!(fb.right_trigger, 32767);
    }
}
