// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/profile.h and profile.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Reserved generic HID/PID report encoder (retained for internal/PID tests).

use crate::protocol::{
    FeedbackEvent, FeedbackType, GenericRumbleRgbFeedback, InputStateRequest, button_mask,
};

pub const GENERIC_INPUT_REPORT_ID: u8 = 1;
pub const GENERIC_OUTPUT_REPORT_ID: u8 = 2;

pub const GENERIC_INPUT_REPORT_SIZE: usize = 16;
pub const GENERIC_OUTPUT_REPORT_SIZE: usize = 6;

const SOURCE_BUTTONS: [u32; 17] = [
    button_mask::SOUTH,
    button_mask::EAST,
    button_mask::WEST,
    button_mask::NORTH,
    button_mask::LEFT_SHOULDER,
    button_mask::RIGHT_SHOULDER,
    button_mask::BACK,
    button_mask::START,
    button_mask::LEFT_STICK,
    button_mask::RIGHT_STICK,
    button_mask::HOME,
    button_mask::PADDLE_1,
    button_mask::PADDLE_2,
    button_mask::PADDLE_3,
    button_mask::PADDLE_4,
    button_mask::TOUCHPAD,
    button_mask::MISC,
];

fn encode_hat(buttons: u32) -> u8 {
    let up = (buttons & button_mask::DPAD_UP) != 0;
    let down = (buttons & button_mask::DPAD_DOWN) != 0;
    let left = (buttons & button_mask::DPAD_LEFT) != 0;
    let right = (buttons & button_mask::DPAD_RIGHT) != 0;

    if (up && down) || (left && right) {
        return 8;
    }
    if up {
        return if right {
            1
        } else if left {
            7
        } else {
            0
        };
    }
    if down {
        return if right {
            3
        } else if left {
            5
        } else {
            4
        };
    }
    if right {
        2
    } else if left {
        6
    } else {
        8
    }
}

fn map_buttons(buttons: u32) -> u32 {
    let mut mapped = 0u32;
    for (idx, &mask) in SOURCE_BUTTONS.iter().enumerate() {
        if (buttons & mask) != 0 {
            mapped |= 1u32 << idx;
        }
    }
    mapped
}

pub const fn to_hid_vertical_axis(value: i16) -> i16 {
    if value == i16::MIN { i16::MAX } else { -value }
}

pub fn encode_generic_input(input: &InputStateRequest) -> [u8; GENERIC_INPUT_REPORT_SIZE] {
    let mut out = [0u8; GENERIC_INPUT_REPORT_SIZE];
    out[0] = GENERIC_INPUT_REPORT_ID;
    out[1..5].copy_from_slice(&map_buttons(input.buttons).to_le_bytes());
    out[5] = encode_hat(input.buttons);
    out[6..8].copy_from_slice(&input.left_x.to_le_bytes());
    out[8..10].copy_from_slice(&to_hid_vertical_axis(input.left_y).to_le_bytes());
    out[10..12].copy_from_slice(&input.right_x.to_le_bytes());
    out[12..14].copy_from_slice(&to_hid_vertical_axis(input.right_y).to_le_bytes());
    out[14] = input.left_trigger;
    out[15] = input.right_trigger;
    out
}

pub fn encode_generic_feedback(controller_id: u32, output: &[u8]) -> Option<FeedbackEvent> {
    if output.len() < GENERIC_OUTPUT_REPORT_SIZE || output[0] != GENERIC_OUTPUT_REPORT_ID {
        return None;
    }
    let fb = GenericRumbleRgbFeedback {
        low_frequency: u16::from(output[1]) << 8,
        high_frequency: u16::from(output[2]) << 8,
        red: output[3],
        green: output[4],
        blue: output[5],
        reserved: 0,
    };
    Some(FeedbackEvent::new(
        controller_id,
        FeedbackType::GenericRumbleRgb,
        &fb.to_bytes(),
    ))
}
