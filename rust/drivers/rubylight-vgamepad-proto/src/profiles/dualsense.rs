// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/ds5_usb.h and
// driver/src/dualsense.h / dualsense.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! DualSense (USB, `VID_054C&PID_0CE6`) profile with adaptive triggers and LEDs.

use super::dualshock4::{
    ACCEL_COUNTS_PER_G, encode_ps_hat, pack_touch_point, scale_ps_accel, scale_ps_gyro,
    to_ps_stick, to_ps_stick_inverted,
};
use crate::protocol::{
    BatteryState, BatteryStateRequest, InputStateRequest, MotionKind, MotionStateRequest,
    PlayStationOutputFeedback, TouchEvent, TouchStateRequest, button_mask, ps_output_valid,
};

pub const INPUT_REPORT_ID: u8 = 0x01;
pub const OUTPUT_REPORT_ID: u8 = 0x02;
pub const OUTPUT_REPORT_ID_BT: u8 = 0x31;
pub const FEATURE_CALIBRATION_ID: u8 = 0x05;
pub const FEATURE_CONTROL_ID: u8 = 0x08;
pub const FEATURE_PAIRING_ID: u8 = 0x09;
pub const FEATURE_FIRMWARE_ID: u8 = 0x20;

pub const VENDOR_ID: u16 = 0x054C;
pub const PRODUCT_ID: u16 = 0x0CE6;
pub const VERSION: u16 = 0x0100;

pub const INPUT_REPORT_SIZE: usize = 64;
pub const OUTPUT_REPORT_SIZE: usize = 48;

pub const TOUCH_WIDTH: u16 = 1920;
pub const TOUCH_HEIGHT: u16 = 1080;
pub const TOUCH_CONTACTS: usize = 2;

pub const FLAG0_COMPATIBLE_VIBRATION: u8 = 0x01;
pub const FLAG0_HAPTICS_SELECT: u8 = 0x02;
pub const FLAG0_RIGHT_TRIGGER_EFFECT: u8 = 0x04;
pub const FLAG0_LEFT_TRIGGER_EFFECT: u8 = 0x08;
pub const FLAG1_MIC_MUTE_LED: u8 = 0x01;
pub const FLAG1_POWER_SAVE: u8 = 0x02;
pub const FLAG1_LIGHTBAR: u8 = 0x04;
pub const FLAG1_RELEASE_LEDS: u8 = 0x08;
pub const FLAG1_PLAYER_INDICATOR: u8 = 0x10;
pub const FLAG2_COMPATIBLE_VIBRATION: u8 = 0x04;

#[rustfmt::skip]
pub const REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0x85, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35,
    0x09, 0x33, 0x09, 0x34, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x06, 0x81, 0x02, 0x06,
    0x00, 0xff, 0x09, 0x20, 0x95, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07,
    0x35, 0x00, 0x46, 0x3b, 0x01, 0x65, 0x14, 0x75, 0x04, 0x95, 0x01, 0x81, 0x42, 0x65, 0x00, 0x05,
    0x09, 0x19, 0x01, 0x29, 0x0f, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x0f, 0x81, 0x02, 0x06,
    0x00, 0xff, 0x09, 0x21, 0x75, 0x01, 0x95, 0x0d, 0x81, 0x02, 0x09, 0x22, 0x15, 0x00, 0x26, 0xff,
    0x00, 0x75, 0x08, 0x95, 0x34, 0x81, 0x02, 0x85, 0x02, 0x09, 0x23, 0x95, 0x2f, 0x91, 0x02, 0x85,
    0x05, 0x09, 0x33, 0x95, 0x28, 0xb1, 0x02, 0x85, 0x08, 0x09, 0x34, 0x95, 0x2f, 0xb1, 0x02, 0x85,
    0x09, 0x09, 0x24, 0x95, 0x13, 0xb1, 0x02, 0x85, 0x0a, 0x09, 0x25, 0x95, 0x1a, 0xb1, 0x02, 0x85,
    0x20, 0x09, 0x26, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0x21, 0x09, 0x27, 0x95, 0x04, 0xb1, 0x02, 0x85,
    0x22, 0x09, 0x40, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0x80, 0x09, 0x28, 0x95, 0x3f, 0xb1, 0x02, 0x85,
    0x81, 0x09, 0x29, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0x82, 0x09, 0x2a, 0x95, 0x09, 0xb1, 0x02, 0x85,
    0x83, 0x09, 0x2b, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0x84, 0x09, 0x2c, 0x95, 0x3f, 0xb1, 0x02, 0x85,
    0x85, 0x09, 0x2d, 0x95, 0x02, 0xb1, 0x02, 0x85, 0xa0, 0x09, 0x2e, 0x95, 0x01, 0xb1, 0x02, 0x85,
    0xe0, 0x09, 0x2f, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xf0, 0x09, 0x30, 0x95, 0x3f, 0xb1, 0x02, 0x85,
    0xf1, 0x09, 0x31, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xf2, 0x09, 0x32, 0x95, 0x0f, 0xb1, 0x02, 0x85,
    0xf4, 0x09, 0x35, 0x95, 0x3f, 0xb1, 0x02, 0x85, 0xf5, 0x09, 0x36, 0x95, 0x03, 0xb1, 0x02, 0xc0,
];

#[rustfmt::skip]
pub const CALIBRATION: [u8; 41] = [
    5, 0, 0, 0, 0, 0, 0,
    0, 0x20, 0, 0xe0, 0, 0x20, 0, 0xe0, 0, 0x20, 0, 0xe0,
    0, 2, 0, 2,
    0, 0x20, 0, 0xe0, 0, 0x20, 0, 0xe0, 0, 0x20, 0, 0xe0,
    0, 0, 0, 0, 0, 0,
];

#[rustfmt::skip]
pub const FIRMWARE: [u8; 64] = [
    FEATURE_FIRMWARE_ID,
    b'J', b'u', b'n', b' ', b'1', b'9', b' ', b'2', b'0', b'2', b'3',
    b'1', b'4', b':', b'4', b'7', b':', b'3', b'4',
    0x03, 0x00, 0x44, 0x00, 0x08, 0x02, 0x00, 0x01,
    0x3e, 0x00, 0x00, 0x01,
    0xC1, 0xC8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x30, 0x06, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00,
    0x0B, 0x00, 0x01, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ds5FeatureState {
    pub address: [u8; 6],
    pub sensors_enabled: bool,
}

impl Default for Ds5FeatureState {
    fn default() -> Self {
        Self {
            address: [0, 0x53, 0x50, 0x47, 0x56, 0x02],
            sensors_enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ds5State {
    pub features: Ds5FeatureState,
    pub sequence: u8,
    pub sensor_timestamp: u32,
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

impl Default for Ds5State {
    fn default() -> Self {
        Self {
            features: Ds5FeatureState::default(),
            sequence: 0,
            sensor_timestamp: 0,
            battery_level: 5,
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

impl Ds5State {
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

pub fn get_feature(id: u8, buffer: &mut [u8], state: &Ds5FeatureState) -> usize {
    let length = match id {
        FEATURE_CALIBRATION_ID => CALIBRATION.len(),
        FEATURE_PAIRING_ID => 20,
        FEATURE_FIRMWARE_ID => FIRMWARE.len(),
        _ => return 0,
    };
    if buffer.len() < length {
        return 0;
    }
    buffer.fill(0);
    match id {
        FEATURE_CALIBRATION_ID => buffer[..length].copy_from_slice(&CALIBRATION),
        FEATURE_FIRMWARE_ID => buffer[..length].copy_from_slice(&FIRMWARE),
        FEATURE_PAIRING_ID => fill_pairing(buffer, &state.address),
        _ => {}
    }
    length
}

pub fn set_feature(id: u8, buffer: &[u8], state: &mut Ds5FeatureState) -> bool {
    if id != FEATURE_CONTROL_ID
        || buffer.len() < 48
        || buffer[0] != FEATURE_CONTROL_ID
        || buffer[1] != 2
    {
        return false;
    }
    if buffer[2..48].iter().any(|&b| b != 0) {
        return false;
    }
    state.sensors_enabled = true;
    true
}

fn to_touch_x(value: u16) -> u16 {
    ((u32::from(value) * u32::from(TOUCH_WIDTH - 1)) / 65535) as u16
}

fn to_touch_y(value: u16) -> u16 {
    ((u32::from(value) * u32::from(TOUCH_HEIGHT - 1)) / 65535) as u16
}

pub fn encode_input(
    input: &InputStateRequest,
    state: Option<&mut Ds5State>,
    now_us: u64,
) -> [u8; INPUT_REPORT_SIZE] {
    let mut r = [0u8; INPUT_REPORT_SIZE];
    r[0] = INPUT_REPORT_ID;
    r[1] = to_ps_stick(input.left_x);
    r[2] = to_ps_stick_inverted(input.left_y);
    r[3] = to_ps_stick(input.right_x);
    r[4] = to_ps_stick_inverted(input.right_y);
    r[5] = input.left_trigger;
    r[6] = input.right_trigger;

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
    r[8] = b0;

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
    r[9] = b1;

    let mut b2 = 0u8;
    if (input.buttons & button_mask::HOME) != 0 {
        b2 |= 0x01;
    }
    if (input.buttons & button_mask::TOUCHPAD) != 0 {
        b2 |= 0x02;
    }
    if (input.buttons & button_mask::MISC) != 0 {
        b2 |= 0x04;
    }
    r[10] = b2;

    if let Some(st) = state {
        st.sequence = st.sequence.wrapping_add(1);
        r[7] = st.sequence;

        st.sensor_timestamp = if now_us != 0 {
            now_us.wrapping_mul(3) as u32
        } else {
            st.sensor_timestamp.wrapping_add(1333)
        };
        for axis in 0..3 {
            let g_off = 16 + axis * 2;
            let a_off = 22 + axis * 2;
            r[g_off..g_off + 2].copy_from_slice(&st.gyro[axis].to_le_bytes());
            r[a_off..a_off + 2].copy_from_slice(&st.accel[axis].to_le_bytes());
        }
        r[28..32].copy_from_slice(&st.sensor_timestamp.to_le_bytes());

        let mut status = st.battery_level & 0x0F;
        if st.battery_full {
            status = 0x20 | 0x0A;
        } else if st.cable_connected {
            status |= 0x10;
        }
        r[53] = status;

        for i in 0..TOUCH_CONTACTS {
            let off = 33 + i * 4;
            pack_touch_point(
                &mut r[off..off + 4],
                st.contact_active[i],
                st.contact_tracking_id[i],
                to_touch_x(st.contact_x[i]),
                to_touch_y(st.contact_y[i]),
            );
        }
    } else {
        r[24..26].copy_from_slice(&(ACCEL_COUNTS_PER_G as i16).to_le_bytes());
        pack_touch_point(&mut r[33..37], false, 0, 0, 0);
        pack_touch_point(&mut r[37..41], false, 0, 0, 0);
    }
    r
}

pub fn apply_touch(touch: &TouchStateRequest, state: &mut Ds5State) -> bool {
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

pub fn apply_motion(motion: &MotionStateRequest, state: &mut Ds5State) -> bool {
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

pub fn apply_battery(battery: &BatteryStateRequest, state: &mut Ds5State) -> bool {
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
    if output.is_empty() {
        return false;
    }
    let normalized_buf;
    let buf: &[u8] = if output[0] == OUTPUT_REPORT_ID {
        if output.len() < OUTPUT_REPORT_SIZE {
            return false;
        }
        &output[..OUTPUT_REPORT_SIZE]
    } else if output[0] == OUTPUT_REPORT_ID_BT && output.len() >= 3 + OUTPUT_REPORT_SIZE - 1 {
        let mut tmp = [0u8; OUTPUT_REPORT_SIZE];
        tmp[0] = OUTPUT_REPORT_ID;
        tmp[1..OUTPUT_REPORT_SIZE].copy_from_slice(&output[3..3 + OUTPUT_REPORT_SIZE - 1]);
        normalized_buf = tmp;
        &normalized_buf
    } else {
        return false;
    };

    let flag0 = buf[1];
    let flag1 = buf[2];
    let flag2 = buf[39];
    if (flag0 & FLAG0_COMPATIBLE_VIBRATION) != 0 || (flag2 & FLAG2_COMPATIBLE_VIBRATION) != 0 {
        feedback.low_frequency = u16::from(buf[4]) << 8;
        feedback.high_frequency = u16::from(buf[3]) << 8;
    }
    if (flag1 & FLAG1_LIGHTBAR) != 0 {
        feedback.red = buf[45];
        feedback.green = buf[46];
        feedback.blue = buf[47];
        feedback.valid |= ps_output_valid::LIGHTBAR;
    }
    if (flag1 & FLAG1_PLAYER_INDICATOR) != 0 {
        feedback.player_leds = buf[44];
        feedback.valid |= ps_output_valid::PLAYER_LEDS;
    }
    if (flag1 & FLAG1_MIC_MUTE_LED) != 0 {
        feedback.microphone_led = buf[9];
        feedback.valid |= ps_output_valid::MICROPHONE_LED;
    }
    if (flag0 & FLAG0_LEFT_TRIGGER_EFFECT) != 0 {
        feedback.left_trigger.mode = buf[22];
        feedback
            .left_trigger
            .parameters
            .copy_from_slice(&buf[23..33]);
        feedback.valid |= ps_output_valid::TRIGGERS;
    }
    if (flag0 & FLAG0_RIGHT_TRIGGER_EFFECT) != 0 {
        feedback.right_trigger.mode = buf[11];
        feedback
            .right_trigger
            .parameters
            .copy_from_slice(&buf[12..22]);
        feedback.valid |= ps_output_valid::TRIGGERS;
    }
    true
}

pub fn decode_output(output: &[u8]) -> Option<PlayStationOutputFeedback> {
    let mut feedback = PlayStationOutputFeedback::default();
    apply_output(output, &mut feedback).then_some(feedback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ds5_firmware_update_version_and_bt_output_bridge() {
        let mut features = Ds5FeatureState::default();
        let mut buf = [0u8; 80];
        assert_eq!(get_feature(FEATURE_FIRMWARE_ID, &mut buf, &features), 64);
        assert!(u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]) >= 0x1003E);
        assert!(u16::from_le_bytes([buf[44], buf[45]]) >= 0x0630);
        assert_eq!(buf[1], b'J');
        assert_ne!(buf[20], 0);

        buf.fill(0);
        buf[0] = FEATURE_CONTROL_ID;
        buf[1] = 2;
        features.sensors_enabled = false;
        assert!(set_feature(FEATURE_CONTROL_ID, &buf[..48], &mut features));
        assert!(features.sensors_enabled);

        // BT 0x31 layout bridges onto USB 0x02 at byte offset +2 (skip id, seq, tag 0x10).
        let mut bt = [0u8; 78];
        bt[0] = OUTPUT_REPORT_ID_BT;
        bt[1] = 0x01;
        bt[2] = 0x10;
        bt[3] = FLAG0_LEFT_TRIGGER_EFFECT | FLAG0_COMPATIBLE_VIBRATION;
        bt[4] = FLAG1_LIGHTBAR;
        bt[6] = 0xAA; // motor_left (USB offset 4 -> BT 6)
        bt[24] = 0x02; // left_trigger.mode (USB 22 -> BT 24)
        bt[47] = 0x11; // lightbar_red (USB 45 -> BT 47)
        let fb = decode_output(&bt).unwrap();
        assert_eq!(fb.low_frequency, 0xAA00);
        assert_eq!(fb.left_trigger.mode, 0x02);
        assert_eq!(fb.red, 0x11);
    }
}
