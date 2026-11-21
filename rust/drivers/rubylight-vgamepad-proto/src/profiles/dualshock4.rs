// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/ds4_usb.h and
// driver/src/dualshock4.h / dualshock4.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! DualShock 4 v2 (USB, `VID_054C&PID_09CC`) profile.

use crate::protocol::{
    BatteryState, BatteryStateRequest, FeedbackEvent, FeedbackType, InputStateRequest, MotionKind,
    MotionStateRequest, PlayStationOutputFeedback, TouchEvent, TouchStateRequest, button_mask,
    ps_output_valid,
};

pub const INPUT_REPORT_ID: u8 = 0x01;
pub const OUTPUT_REPORT_ID: u8 = 0x05;
pub const FEATURE_CALIBRATION_ID: u8 = 0x02;
pub const FEATURE_PAIRING_ID: u8 = 0x12;
pub const FEATURE_CONTROL_ID: u8 = 0x14;
pub const FEATURE_FIRMWARE_ID: u8 = 0xA3;

pub const VENDOR_ID: u16 = 0x054C;
pub const PRODUCT_ID: u16 = 0x09CC;
pub const VERSION: u16 = 0x0100;

pub const INPUT_REPORT_SIZE: usize = 64;
pub const OUTPUT_REPORT_SIZE: usize = 32;

pub const TOUCH_WIDTH: u16 = 1920;
pub const TOUCH_HEIGHT: u16 = 942;
pub const TOUCH_CONTACTS: usize = 2;

pub const ACCEL_COUNTS_PER_G: i32 = 8192;
pub const GYRO_COUNTS_PER_DPS: i32 = 16;
pub const MILLI_G: i32 = 9807;

#[rustfmt::skip]
pub const REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0x85, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35,
    0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x04, 0x81, 0x02, 0x09, 0x39, 0x15, 0x00, 0x25,
    0x07, 0x35, 0x00, 0x46, 0x3b, 0x01, 0x65, 0x14, 0x75, 0x04, 0x95, 0x01, 0x81, 0x42, 0x65, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x0e, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x0e, 0x81, 0x02,
    0x06, 0x00, 0xff, 0x09, 0x20, 0x75, 0x06, 0x95, 0x01, 0x15, 0x00, 0x25, 0x3f, 0x81, 0x02, 0x05,
    0x01, 0x09, 0x33, 0x09, 0x34, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02,
    0x06, 0x00, 0xff, 0x09, 0x21, 0x95, 0x36, 0x81, 0x02, 0x85, 0x05, 0x09, 0x22, 0x95, 0x1f, 0x91,
    0x02, 0x85, 0x04, 0x09, 0x23, 0x95, 0x24, 0xb1, 0x02, 0x85, 0x02, 0x09, 0x24, 0x95, 0x24, 0xb1,
    0x02, 0x85, 0x08, 0x09, 0x25, 0x95, 0x03, 0xb1, 0x02, 0x85, 0x10, 0x09, 0x26, 0x95, 0x04, 0xb1,
    0x02, 0x85, 0x11, 0x09, 0x27, 0x95, 0x02, 0xb1, 0x02, 0x85, 0x12, 0x06, 0x02, 0xff, 0x09, 0x21,
    0x95, 0x0f, 0xb1, 0x02, 0x85, 0x13, 0x09, 0x22, 0x95, 0x16, 0xb1, 0x02, 0x85, 0x14, 0x06, 0x05,
    0xff, 0x09, 0x20, 0x95, 0x10, 0xb1, 0x02, 0x85, 0x15, 0x09, 0x21, 0x95, 0x2c, 0xb1, 0x02, 0x06,
    0x80, 0xff, 0x85, 0x80, 0x09, 0x20, 0x95, 0x06, 0xb1, 0x02, 0x85, 0x81, 0x09, 0x21, 0x95, 0x06,
    0xb1, 0x02, 0x85, 0x82, 0x09, 0x22, 0x95, 0x05, 0xb1, 0x02, 0x85, 0x83, 0x09, 0x23, 0x95, 0x01,
    0xb1, 0x02, 0x85, 0x84, 0x09, 0x24, 0x95, 0x04, 0xb1, 0x02, 0x85, 0x85, 0x09, 0x25, 0x95, 0x06,
    0xb1, 0x02, 0x85, 0x86, 0x09, 0x26, 0x95, 0x06, 0xb1, 0x02, 0x85, 0x87, 0x09, 0x27, 0x95, 0x23,
    0xb1, 0x02, 0x85, 0x88, 0x09, 0x28, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0x89, 0x09, 0x29, 0x95, 0x02,
    0xb1, 0x02, 0x85, 0x90, 0x09, 0x30, 0x95, 0x05, 0xb1, 0x02, 0x85, 0x91, 0x09, 0x31, 0x95, 0x03,
    0xb1, 0x02, 0x85, 0x92, 0x09, 0x32, 0x95, 0x03, 0xb1, 0x02, 0x85, 0x93, 0x09, 0x33, 0x95, 0x0c,
    0xb1, 0x02, 0x85, 0x94, 0x09, 0x34, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xa0, 0x09, 0x40, 0x95, 0x06,
    0xb1, 0x02, 0x85, 0xa1, 0x09, 0x41, 0x95, 0x01, 0xb1, 0x02, 0x85, 0xa2, 0x09, 0x42, 0x95, 0x01,
    0xb1, 0x02, 0x85, 0xa3, 0x09, 0x43, 0x95, 0x30, 0xb1, 0x02, 0x85, 0xa4, 0x09, 0x44, 0x95, 0x0d,
    0xb1, 0x02, 0x85, 0xf0, 0x09, 0x47, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xf1, 0x09, 0x48, 0x95, 0x3f,
    0xb1, 0x02, 0x85, 0xf2, 0x09, 0x49, 0x95, 0x0f, 0xb1, 0x02, 0x85, 0xa7, 0x09, 0x4a, 0x95, 0x01,
    0xb1, 0x02, 0x85, 0xa8, 0x09, 0x4b, 0x95, 0x01, 0xb1, 0x02, 0x85, 0xa9, 0x09, 0x4c, 0x95, 0x08,
    0xb1, 0x02, 0x85, 0xaa, 0x09, 0x4e, 0x95, 0x01, 0xb1, 0x02, 0x85, 0xab, 0x09, 0x4f, 0x95, 0x39,
    0xb1, 0x02, 0x85, 0xac, 0x09, 0x50, 0x95, 0x39, 0xb1, 0x02, 0x85, 0xad, 0x09, 0x51, 0x95, 0x0b,
    0xb1, 0x02, 0x85, 0xae, 0x09, 0x52, 0x95, 0x01, 0xb1, 0x02, 0x85, 0xaf, 0x09, 0x53, 0x95, 0x02,
    0xb1, 0x02, 0x85, 0xb0, 0x09, 0x54, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xe0, 0x09, 0x57, 0x95, 0x02,
    0xb1, 0x02, 0x85, 0xb3, 0x09, 0x55, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xb4, 0x09, 0x55, 0x95, 0x3f,
    0xb1, 0x02, 0x85, 0xb5, 0x09, 0x56, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xd0, 0x09, 0x58, 0x95, 0x3f,
    0xb1, 0x02, 0x85, 0xd4, 0x09, 0x59, 0x95, 0x3f, 0xb1, 0x02, 0xc0,
];

#[rustfmt::skip]
pub const CALIBRATION: [u8; 37] = [
    0x02, 0, 0, 0, 0, 0, 0,
    0x00, 0x20, 0x00, 0xe0,
    0x00, 0x20, 0x00, 0xe0,
    0x00, 0x20, 0x00, 0xe0,
    0x00, 0x02, 0x00, 0x02,
    0x00, 0x20, 0x00, 0xe0,
    0x00, 0x20, 0x00, 0xe0,
    0x00, 0x20, 0x00, 0xe0,
    0, 0,
];

pub const FIRMWARE: [u8; 49] = {
    let mut v = [0u8; 49];
    v[0] = FEATURE_FIRMWARE_ID;
    v[34] = 0x01;
    v[36] = 0x43;
    v[37] = 0x03;
    v[41] = 0x51;
    v[43] = 0x05;
    v[46] = 0x80;
    v[47] = 0x03;
    v
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ds4FeatureState {
    pub address: [u8; 6],
    pub sensors_enabled: bool,
}

impl Default for Ds4FeatureState {
    fn default() -> Self {
        Self {
            address: [0x44, 0x41, 0x50, 0x47, 0x56, 0x02],
            sensors_enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ds4State {
    pub features: Ds4FeatureState,
    pub report_counter: u8,
    pub timestamp: u16,
    pub touch_timestamp: u8,
    pub battery_level: u8,
    pub cable_connected: bool,
    pub battery_full: bool,
    pub gyro: [i16; 3],
    pub accel: [i16; 3],
    pub next_tracking_id: u8,
    pub contact_active: [bool; TOUCH_CONTACTS],
    pub contact_tracking_id: [u8; TOUCH_CONTACTS],
    pub contact_x: [u16; TOUCH_CONTACTS],
    pub contact_y: [u16; TOUCH_CONTACTS],
}

impl Default for Ds4State {
    fn default() -> Self {
        Self {
            features: Ds4FeatureState::default(),
            report_counter: 0,
            timestamp: 0,
            touch_timestamp: 0,
            battery_level: 8,
            cable_connected: false,
            battery_full: false,
            gyro: [0; 3],
            accel: [0, ACCEL_COUNTS_PER_G as i16, 0],
            next_tracking_id: 1,
            contact_active: [false; TOUCH_CONTACTS],
            contact_tracking_id: [0; TOUCH_CONTACTS],
            contact_x: [0; TOUCH_CONTACTS],
            contact_y: [0; TOUCH_CONTACTS],
        }
    }
}

impl Ds4State {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn fill_pairing(buffer: &mut [u8], address: &[u8; 6]) {
    buffer[0] = FEATURE_PAIRING_ID;
    buffer[1..7].copy_from_slice(address);
    buffer[7] = 0x08;
    buffer[8] = 0x25;
    buffer[9] = 0x00;
}

pub fn get_feature(id: u8, buffer: &mut [u8], state: &Ds4FeatureState) -> usize {
    let length = match id {
        FEATURE_CALIBRATION_ID => CALIBRATION.len(),
        FEATURE_PAIRING_ID => 16,
        FEATURE_CONTROL_ID => 17,
        FEATURE_FIRMWARE_ID => FIRMWARE.len(),
        _ => return 0,
    };
    if buffer.len() < length {
        return 0;
    }
    buffer.fill(0);
    buffer[0] = id;
    match id {
        FEATURE_CALIBRATION_ID => buffer[..length].copy_from_slice(&CALIBRATION),
        FEATURE_FIRMWARE_ID => buffer[..length].copy_from_slice(&FIRMWARE),
        FEATURE_PAIRING_ID => fill_pairing(buffer, &state.address),
        FEATURE_CONTROL_ID => {
            buffer[1] = if state.sensors_enabled { 0x02 } else { 0 };
        }
        _ => {}
    }
    length
}

pub fn set_feature(id: u8, buffer: &[u8], state: &mut Ds4FeatureState) -> bool {
    if id != FEATURE_CONTROL_ID || buffer.len() < 17 || buffer[0] != id || buffer[1] != 0x02 {
        return false;
    }
    if buffer[2..17].iter().any(|&b| b != 0) {
        return false;
    }
    state.sensors_enabled = true;
    true
}

pub(crate) fn encode_ps_hat(buttons: u32) -> u8 {
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

pub(crate) const fn to_ps_stick(value: i16) -> u8 {
    ((value as i32 + 32768) >> 8) as u8
}

pub(crate) const fn to_ps_stick_inverted(value: i16) -> u8 {
    let scaled = (32768 - value as i32) >> 8;
    if scaled > 255 {
        255
    } else if scaled < 0 {
        0
    } else {
        scaled as u8
    }
}

pub(crate) const fn clamp_i16_i64(value: i64) -> i16 {
    if value > 32767 {
        32767
    } else if value < -32768 {
        -32768
    } else {
        value as i16
    }
}

pub(crate) fn pack_touch_point(out: &mut [u8], active: bool, tracking_id: u8, x: u16, y: u16) {
    out[0] = (if active { 0x00 } else { 0x80 }) | (tracking_id & 0x7F);
    out[1] = (x & 0xFF) as u8;
    out[2] = (((x >> 8) & 0x0F) as u8) | (((y & 0x0F) as u8) << 4);
    out[3] = ((y >> 4) & 0xFF) as u8;
}

fn to_touch_x(value: u16) -> u16 {
    ((u32::from(value) * u32::from(TOUCH_WIDTH - 1)) / 65535) as u16
}

fn to_touch_y(value: u16) -> u16 {
    ((u32::from(value) * u32::from(TOUCH_HEIGHT - 1)) / 65535) as u16
}

pub fn encode_input(
    input: &InputStateRequest,
    state: Option<&mut Ds4State>,
    now_us: u64,
) -> [u8; INPUT_REPORT_SIZE] {
    let mut r = [0u8; INPUT_REPORT_SIZE];
    r[0] = INPUT_REPORT_ID;
    r[1] = to_ps_stick(input.left_x);
    r[2] = to_ps_stick_inverted(input.left_y);
    r[3] = to_ps_stick(input.right_x);
    r[4] = to_ps_stick_inverted(input.right_y);

    let mut b0 = encode_ps_hat(input.buttons);
    if (input.buttons & button_mask::WEST) != 0 {
        b0 |= 0x10;
    }
    if (input.buttons & button_mask::SOUTH) != 0 {
        b0 |= 0x20;
    }
    if (input.buttons & button_mask::EAST) != 0 {
        b0 |= 0x40;
    }
    if (input.buttons & button_mask::NORTH) != 0 {
        b0 |= 0x80;
    }
    r[5] = b0;

    let mut b1 = 0u8;
    if (input.buttons & button_mask::LEFT_SHOULDER) != 0 {
        b1 |= 0x01;
    }
    if (input.buttons & button_mask::RIGHT_SHOULDER) != 0 {
        b1 |= 0x02;
    }
    if input.left_trigger > 0 {
        b1 |= 0x04;
    }
    if input.right_trigger > 0 {
        b1 |= 0x08;
    }
    if (input.buttons & button_mask::BACK) != 0 {
        b1 |= 0x10;
    }
    if (input.buttons & button_mask::START) != 0 {
        b1 |= 0x20;
    }
    if (input.buttons & button_mask::LEFT_STICK) != 0 {
        b1 |= 0x40;
    }
    if (input.buttons & button_mask::RIGHT_STICK) != 0 {
        b1 |= 0x80;
    }
    r[6] = b1;

    let mut b2 = 0u8;
    if (input.buttons & button_mask::HOME) != 0 {
        b2 |= 0x01;
    }
    if (input.buttons & (button_mask::TOUCHPAD | button_mask::MISC)) != 0 {
        b2 |= 0x02;
    }
    r[8] = input.left_trigger;
    r[9] = input.right_trigger;

    if let Some(st) = state {
        st.report_counter = (st.report_counter + 1) & 0x3F;
        b2 |= st.report_counter << 2;
        r[7] = b2;

        st.timestamp = if now_us != 0 {
            ((now_us.wrapping_mul(3)) / 16) as u16
        } else {
            st.timestamp.wrapping_add(188)
        };
        r[10..12].copy_from_slice(&st.timestamp.to_le_bytes());
        r[12] = st.battery_level;
        r[13..15].copy_from_slice(&st.gyro[0].to_le_bytes());
        r[15..17].copy_from_slice(&st.gyro[1].to_le_bytes());
        r[17..19].copy_from_slice(&st.gyro[2].to_le_bytes());
        r[19..21].copy_from_slice(&st.accel[0].to_le_bytes());
        r[21..23].copy_from_slice(&st.accel[1].to_le_bytes());
        r[23..25].copy_from_slice(&st.accel[2].to_le_bytes());

        let mut status = st.battery_level & 0x0F;
        if st.cable_connected {
            status |= 0x10;
        }
        if st.battery_full && st.cable_connected {
            status = 0x1B;
        }
        r[30] = status;

        r[33] = 1; // touch_packet_count
        st.touch_timestamp = st.touch_timestamp.wrapping_add(1);
        r[34] = st.touch_timestamp;
        for i in 0..TOUCH_CONTACTS {
            let off = 35 + i * 4;
            pack_touch_point(
                &mut r[off..off + 4],
                st.contact_active[i],
                st.contact_tracking_id[i],
                to_touch_x(st.contact_x[i]),
                to_touch_y(st.contact_y[i]),
            );
        }
    } else {
        r[7] = b2;
        r[12] = 8;
        r[21..23].copy_from_slice(&(ACCEL_COUNTS_PER_G as i16).to_le_bytes());
        r[30] = 0x08;
        pack_touch_point(&mut r[35..39], false, 0, 0, 0);
        pack_touch_point(&mut r[39..43], false, 0, 0, 0);
    }
    r
}

pub fn apply_touch(touch: &TouchStateRequest, state: &mut Ds4State) -> bool {
    let Some(event) = TouchEvent::from_raw(touch.event_type) else {
        return false;
    };
    if event == TouchEvent::CancelAll {
        state.contact_active = [false; TOUCH_CONTACTS];
        return true;
    }
    let idx = usize::from(touch.contact_index);
    if idx >= TOUCH_CONTACTS {
        return false;
    }
    match event {
        TouchEvent::Down => {
            if !state.contact_active[idx] {
                let other = 1 - idx;
                loop {
                    state.contact_tracking_id[idx] = state.next_tracking_id;
                    state.next_tracking_id = (state.next_tracking_id + 1) & 0x7F;
                    if state.next_tracking_id == 0 {
                        state.next_tracking_id = 1;
                    }
                    if !(state.contact_active[other]
                        && state.contact_tracking_id[idx] == state.contact_tracking_id[other])
                    {
                        break;
                    }
                }
            }
            state.contact_active[idx] = true;
            state.contact_x[idx] = touch.x;
            state.contact_y[idx] = touch.y;
            true
        }
        TouchEvent::Move | TouchEvent::Hover => {
            if !state.contact_active[idx] {
                return false;
            }
            state.contact_x[idx] = touch.x;
            state.contact_y[idx] = touch.y;
            true
        }
        TouchEvent::Up | TouchEvent::Cancel => {
            state.contact_active[idx] = false;
            true
        }
        TouchEvent::CancelAll => true,
    }
}

pub(crate) fn scale_ps_accel(milli: i32) -> i16 {
    let scaled = (i64::from(milli) * i64::from(ACCEL_COUNTS_PER_G)) / i64::from(MILLI_G);
    clamp_i16_i64(scaled)
}

pub(crate) fn scale_ps_gyro(milli: i32) -> i16 {
    let scaled = (i64::from(milli) * i64::from(GYRO_COUNTS_PER_DPS)) / 1000;
    clamp_i16_i64(scaled)
}

pub fn apply_motion(motion: &MotionStateRequest, state: &mut Ds4State) -> bool {
    match MotionKind::from_raw(motion.motion_type) {
        Some(MotionKind::Accelerometer) => {
            state.accel[0] = scale_ps_accel(motion.x_milli);
            state.accel[1] = scale_ps_accel(motion.y_milli);
            state.accel[2] = scale_ps_accel(motion.z_milli);
            true
        }
        Some(MotionKind::Gyroscope) => {
            state.gyro[0] = scale_ps_gyro(motion.x_milli);
            state.gyro[1] = scale_ps_gyro(motion.y_milli);
            state.gyro[2] = scale_ps_gyro(motion.z_milli);
            true
        }
        None => false,
    }
}

pub fn apply_battery(battery: &BatteryStateRequest, state: &mut Ds4State) -> bool {
    let reported = BatteryState::from_raw(battery.flags);
    state.cable_connected = matches!(
        reported,
        BatteryState::Charging | BatteryState::Full | BatteryState::NotCharging
    );
    state.battery_full = reported == BatteryState::Full;
    if battery.percent <= 100 {
        state.battery_level = if battery.percent >= 100 {
            10
        } else {
            battery.percent / 10
        };
    }
    true
}

pub fn apply_output(output: &[u8], feedback: &mut PlayStationOutputFeedback) -> bool {
    if output.len() < OUTPUT_REPORT_SIZE || output[0] != OUTPUT_REPORT_ID {
        return false;
    }
    let flags = output[1];
    if (flags & 0x01) != 0 {
        feedback.low_frequency = u16::from(output[5]) << 8;
        feedback.high_frequency = u16::from(output[4]) << 8;
    }
    if (flags & 0x02) != 0 {
        feedback.red = output[6];
        feedback.green = output[7];
        feedback.blue = output[8];
        feedback.valid |= ps_output_valid::LIGHTBAR;
    }
    true
}

pub fn decode_output(output: &[u8]) -> Option<PlayStationOutputFeedback> {
    let mut feedback = PlayStationOutputFeedback::default();
    apply_output(output, &mut feedback).then_some(feedback)
}

pub fn encode_playstation_feedback(
    controller_id: u32,
    feedback: &PlayStationOutputFeedback,
) -> FeedbackEvent {
    FeedbackEvent::new(
        controller_id,
        FeedbackType::PlayStationOutput,
        &feedback.to_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ds4_usb_descriptor_and_features_contract() {
        assert_eq!(REPORT_DESCRIPTOR.len(), 507);
        let mut features = Ds4FeatureState::default();
        let mut buf = [0u8; 64];

        for &(report, length) in &[(2u8, 37usize), (0x12, 16), (0x14, 17), (0xA3, 49)] {
            buf.fill(0xCD);
            assert_eq!(get_feature(report, &mut buf[..length - 1], &features), 0);
            assert_eq!(buf[0], 0xCD);
            assert_eq!(buf[63], 0xCD);

            assert_eq!(get_feature(report, &mut buf[..length], &features), length);
            assert_eq!(buf[length], 0xCD);

            assert_eq!(get_feature(report, &mut buf, &features), length);
            assert_eq!(buf[0], report);
            assert!(buf[length..].iter().all(|&b| b == 0));
        }

        get_feature(0xA3, &mut buf, &features);
        assert!(u16::from_le_bytes([buf[35], buf[36]]) >= 0x3100);
        get_feature(0x12, &mut buf, &features);
        assert_eq!(&buf[7..10], &[0x08, 0x25, 0x00]);
        assert_eq!(get_feature(0x7C, &mut buf, &features), 0);

        buf.fill(0);
        buf[0] = 0x14;
        buf[1] = 2;
        features.sensors_enabled = false;
        assert!(set_feature(0x14, &buf[..17], &mut features));
        assert!(features.sensors_enabled);
        assert!(set_feature(0x14, &buf, &mut features));
        assert!(!set_feature(0x14, &buf[..16], &mut features));
        buf[1] = 0xFF;
        assert!(!set_feature(0x14, &buf[..17], &mut features));
    }

    #[test]
    fn ds4_touch_multi_contact_wraparound_keeps_distinct_ids() {
        for held in 0..2u8 {
            let mut st = Ds4State::default();
            let mut ev = TouchStateRequest {
                contact_index: held,
                event_type: TouchEvent::Down as u8,
                ..Default::default()
            };
            assert!(apply_touch(&ev, &mut st));
            let r = encode_input(&InputStateRequest::default(), Some(&mut st), 0);
            let held_id = r[35 + usize::from(held) * 4];
            ev.contact_index = 1 - held;
            for tap in 0..400u16 {
                ev.event_type = TouchEvent::Down as u8;
                assert!(apply_touch(&ev, &mut st));
                let r = encode_input(&InputStateRequest::default(), Some(&mut st), 0);
                let ids = [r[35], r[39]];
                assert_eq!(ids[usize::from(held)], held_id);
                assert_eq!(ids[0] & 0x80, 0);
                assert_eq!(ids[1] & 0x80, 0);
                assert_ne!(ids[0], ids[1]);
                ev.event_type = TouchEvent::Up as u8;
                assert!(apply_touch(&ev, &mut st));
                let rel = encode_input(&InputStateRequest::default(), Some(&mut st), 0);
                assert_ne!(rel[35 + usize::from(1 - held) * 4] & 0x80, 0);
                let _ = tap;
            }
            ev.event_type = TouchEvent::CancelAll as u8;
            assert!(apply_touch(&ev, &mut st));
        }
    }

    #[test]
    fn ds4_output_preserves_rumble_on_lightbar_update() {
        let mut rumble = [0u8; OUTPUT_REPORT_SIZE];
        rumble[0] = OUTPUT_REPORT_ID;
        rumble[1] = 0x01;
        rumble[4] = 0x33;
        rumble[5] = 0x77;
        let mut fb = decode_output(&rumble).unwrap();
        assert_eq!(fb.low_frequency, 0x7700);
        assert_eq!(fb.high_frequency, 0x3300);

        let mut led = [0u8; OUTPUT_REPORT_SIZE];
        led[0] = OUTPUT_REPORT_ID;
        led[1] = 0x02;
        led[6] = 10;
        led[7] = 20;
        led[8] = 30;
        assert!(apply_output(&led, &mut fb));
        assert_eq!(fb.low_frequency, 0x7700);
        assert_eq!(fb.red, 10);
        assert_ne!(fb.valid & ps_output_valid::LIGHTBAR, 0);
    }
}
