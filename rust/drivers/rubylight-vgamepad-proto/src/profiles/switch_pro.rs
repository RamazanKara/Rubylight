// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/switch_pro.h and switch_pro.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Nintendo Switch Pro Controller (USB, `VID_057E&PID_2009`) profile.

use crate::protocol::{
    BatteryState, BatteryStateRequest, InputStateRequest, MotionKind, MotionStateRequest,
    PlayStationOutputFeedback, button_mask,
};
use crate::report_pump::{ReportKind, ReportPump};

pub const INPUT_REPORT_ID: u8 = 0x30;
pub const SUBCOMMAND_REPLY_ID: u8 = 0x21;
pub const USB_REPLY_ID: u8 = 0x81;
pub const RUMBLE_SUBCOMMAND_ID: u8 = 0x01;
pub const RUMBLE_ONLY_ID: u8 = 0x10;
pub const USB_COMMAND_ID: u8 = 0x80;

pub const REPORT_SIZE: usize = 64;

pub const VENDOR_ID: u16 = 0x057E;
pub const PRODUCT_ID: u16 = 0x2009;
pub const VERSION: u16 = 0x0200;

pub const STICK_CENTER: i32 = 2048;
pub const STICK_RANGE: i32 = 1400;
pub const STICK_MAX: i32 = 4095;

pub const AMPLITUDE_MAX: usize = 0x64;

const ACCEL_SENSITIVITY: i64 = 16384;
const GYRO_SENSITIVITY: i64 = 13371;

/// SDL's 101-entry HD rumble amplitude table (`SDL_hidapi_switch.c`).
#[rustfmt::skip]
pub const AMPLITUDE_BY_INDEX: [u16; AMPLITUDE_MAX + 1] = [
    0, 514, 775, 921, 1096, 1303, 1550, 1843, 2192, 2606,
    3100, 3686, 4383, 5213, 6199, 7372, 7698, 8039, 8395, 8767,
    9155, 9560, 9984, 10426, 10887, 11369, 11873, 12398, 12947, 13520,
    14119, 14744, 15067, 15397, 15734, 16079, 16431, 16790, 17158, 17534,
    17918, 18310, 18711, 19121, 19540, 19967, 20405, 20851, 21308, 21775,
    22251, 22739, 23236, 23745, 24265, 24797, 25340, 25894, 26462, 27041,
    27633, 28238, 28856, 29488, 30134, 30794, 31468, 32157, 32861, 33581,
    34316, 35068, 35836, 36620, 37422, 38242, 39079, 39935, 40809, 41703,
    42616, 43549, 44503, 45477, 46473, 47491, 48531, 49593, 50679, 51789,
    52923, 54082, 55266, 56476, 57713, 58977, 60268, 61588, 62936, 64315,
    65535,
];

pub const USB_REQUEST_STATUS: u8 = 0x01;
pub const USB_HANDSHAKE: u8 = 0x02;
pub const USB_BAUDRATE: u8 = 0x03;
pub const USB_HID_ONLY: u8 = 0x04;
pub const USB_DISABLE_TIMEOUT: u8 = 0x05;

pub const SUB_REQUEST_DEVICE_INFO: u8 = 0x02;
pub const SUB_SET_INPUT_MODE: u8 = 0x03;
pub const SUB_SHIPMENT_STATE: u8 = 0x08;
pub const SUB_SPI_READ: u8 = 0x10;
pub const SUB_SET_PLAYER_LIGHTS: u8 = 0x30;
pub const SUB_GET_PLAYER_LIGHTS: u8 = 0x31;
pub const SUB_SET_HOME_LIGHT: u8 = 0x38;
pub const SUB_ENABLE_IMU: u8 = 0x40;
pub const SUB_ENABLE_VIBRATION: u8 = 0x48;

mod switch_btn {
    pub const Y: u8 = 0x01;
    pub const X: u8 = 0x02;
    pub const B: u8 = 0x04;
    pub const A: u8 = 0x08;
    pub const R: u8 = 0x40;
    pub const ZR: u8 = 0x80;

    pub const MINUS: u8 = 0x01;
    pub const PLUS: u8 = 0x02;
    pub const RSTICK: u8 = 0x04;
    pub const LSTICK: u8 = 0x08;
    pub const HOME: u8 = 0x10;
    pub const CAPTURE: u8 = 0x20;

    pub const DOWN: u8 = 0x01;
    pub const UP: u8 = 0x02;
    pub const RIGHT: u8 = 0x04;
    pub const LEFT: u8 = 0x08;
    pub const L: u8 = 0x40;
    pub const ZL: u8 = 0x80;
}

#[rustfmt::skip]
pub const DESCRIPTOR: &[u8] = &[
    0x05, 0x01,       // Usage Page (Generic Desktop)
    0x09, 0x05,       // Usage (Game Pad)
    0xA1, 0x01,       // Collection (Application)
    0x06, 0x01, 0xFF, //   Usage Page (Vendor-defined 0xFF01)
    0x85, 0x21,       //   Report ID (33)
    0x09, 0x21,       //   Usage (0x21)
    0x75, 0x08,       //   Report Size (8)
    0x95, 0x3F,       //   Report Count (63)
    0x81, 0x02,       //   Input
    0x85, 0x30,       //   Report ID (48)
    0x09, 0x30,       //   Usage (0x30)
    0x75, 0x08,       //   Report Size (8)
    0x95, 0x3F,       //   Report Count (63)
    0x81, 0x02,       //   Input
    0x85, 0x81,       //   Report ID (129)
    0x09, 0x81,       //   Usage (0x81)
    0x75, 0x08,       //   Report Size (8)
    0x95, 0x3F,       //   Report Count (63)
    0x81, 0x02,       //   Input
    0x85, 0x01,       //   Report ID (1)
    0x09, 0x01,       //   Usage (0x01)
    0x75, 0x08,       //   Report Size (8)
    0x95, 0x3F,       //   Report Count (63)
    0x91, 0x02,       //   Output
    0x85, 0x10,       //   Report ID (16)
    0x09, 0x10,       //   Usage (0x10)
    0x75, 0x08,       //   Report Size (8)
    0x95, 0x3F,       //   Report Count (63)
    0x91, 0x02,       //   Output
    0x85, 0x80,       //   Report ID (128)
    0x09, 0x80,       //   Usage (0x80)
    0x75, 0x08,       //   Report Size (8)
    0x95, 0x3F,       //   Report Count (63)
    0x91, 0x02,       //   Output
    0xC0,             // End Collection
];

const SWITCH_MAC: [u8; 6] = [0x02, 0x56, 0x47, 0x53, 0x57, 0x50];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchState {
    pub timer: u8,
    pub battery_level: u8,
    pub cable_connected: bool,
    pub charging: bool,
    pub gyro: [i16; 3],
    pub accel: [i16; 3],
    pub player_lights: u8,
    pub imu_enabled: bool,
    pub vibration_enabled: bool,
    pub handshake_complete: bool,
}

impl Default for SwitchState {
    fn default() -> Self {
        Self {
            timer: 0,
            battery_level: 8,
            cable_connected: true,
            charging: false,
            gyro: [0; 3],
            accel: [0, 0, 4096],
            player_lights: 0x01,
            imu_enabled: false,
            vibration_enabled: false,
            handshake_complete: false,
        }
    }
}

impl SwitchState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

const fn clamp_stick(value: i32) -> u16 {
    if value < 0 {
        0
    } else if value > STICK_MAX {
        STICK_MAX as u16
    } else {
        value as u16
    }
}

fn pack_stick(out: &mut [u8], horizontal: u16, vertical: u16) {
    out[0] = (horizontal & 0xFF) as u8;
    out[1] = (((horizontal >> 8) & 0x0F) as u8) | (((vertical & 0x0F) as u8) << 4);
    out[2] = ((vertical >> 4) & 0xFF) as u8;
}

/// Positive-up and positive-right sticks (never vertically inverted for Switch Pro).
const fn to_stick(value: i16) -> u16 {
    clamp_stick(STICK_CENTER + ((value as i32) * STICK_RANGE) / 32767)
}

const fn clamp_i16(value: i64) -> i16 {
    if value > 32767 {
        32767
    } else if value < -32768 {
        -32768
    } else {
        value as i16
    }
}

fn read_spi(address: u32, length: u8, out: &mut [u8]) {
    let n = usize::from(length).min(out.len());
    out[..n].fill(0xFF);

    match address {
        0x603D => {
            if n < 18 {
                return;
            }
            pack_stick(&mut out[0..3], STICK_RANGE as u16, STICK_RANGE as u16);
            pack_stick(&mut out[3..6], STICK_CENTER as u16, STICK_CENTER as u16);
            pack_stick(&mut out[6..9], STICK_RANGE as u16, STICK_RANGE as u16);
            pack_stick(&mut out[9..12], STICK_CENTER as u16, STICK_CENTER as u16);
            pack_stick(&mut out[12..15], STICK_RANGE as u16, STICK_RANGE as u16);
            pack_stick(&mut out[15..18], STICK_RANGE as u16, STICK_RANGE as u16);
        }
        0x6020 => {
            if n < 24 {
                return;
            }
            out[..n].fill(0);
            let accel_scale = (ACCEL_SENSITIVITY as u16).to_le_bytes();
            let gyro_scale = (GYRO_SENSITIVITY as u16).to_le_bytes();
            for axis in 0..3 {
                out[6 + axis * 2..8 + axis * 2].copy_from_slice(&accel_scale);
                out[18 + axis * 2..20 + axis * 2].copy_from_slice(&gyro_scale);
            }
        }
        0x6050 => {
            if n < 6 {
                return;
            }
            out[..6].copy_from_slice(&[0x32, 0x32, 0x32, 0xE6, 0xE6, 0xE6]);
        }
        _ => {}
    }
}

pub fn encode_input(
    input: &InputStateRequest,
    state: Option<&mut SwitchState>,
) -> [u8; REPORT_SIZE] {
    let mut r = [0u8; REPORT_SIZE];
    r[0] = INPUT_REPORT_ID;
    if let Some(st) = state {
        st.timer = st.timer.wrapping_add(1);
        r[1] = st.timer;
        r[2] = ((st.battery_level & 0x0E) << 4)
            | (if st.charging { 0x10 } else { 0 })
            | (if st.cable_connected { 0x01 } else { 0 });
        r[12] = 0x0C;

        if st.imu_enabled {
            for sample in 0..3 {
                let slot = &mut r[13 + sample * 12..13 + (sample + 1) * 12];
                for axis in 0..3 {
                    slot[axis * 2..axis * 2 + 2].copy_from_slice(&st.accel[axis].to_le_bytes());
                    slot[6 + axis * 2..8 + axis * 2].copy_from_slice(&st.gyro[axis].to_le_bytes());
                }
            }
        }
    } else {
        r[2] = 0x81;
        r[12] = 0x0C;
    }

    // Positional face buttons: South=B, East=A, West=Y, North=X.
    let mut right = 0u8;
    if (input.buttons & button_mask::SOUTH) != 0 {
        right |= switch_btn::B;
    }
    if (input.buttons & button_mask::EAST) != 0 {
        right |= switch_btn::A;
    }
    if (input.buttons & button_mask::WEST) != 0 {
        right |= switch_btn::Y;
    }
    if (input.buttons & button_mask::NORTH) != 0 {
        right |= switch_btn::X;
    }
    if (input.buttons & button_mask::RIGHT_SHOULDER) != 0 {
        right |= switch_btn::R;
    }
    if input.right_trigger > 0 {
        right |= switch_btn::ZR;
    }
    r[3] = right;

    let mut shared = 0u8;
    if (input.buttons & button_mask::BACK) != 0 {
        shared |= switch_btn::MINUS;
    }
    if (input.buttons & button_mask::START) != 0 {
        shared |= switch_btn::PLUS;
    }
    if (input.buttons & button_mask::LEFT_STICK) != 0 {
        shared |= switch_btn::LSTICK;
    }
    if (input.buttons & button_mask::RIGHT_STICK) != 0 {
        shared |= switch_btn::RSTICK;
    }
    if (input.buttons & button_mask::HOME) != 0 {
        shared |= switch_btn::HOME;
    }
    if (input.buttons & (button_mask::MISC | button_mask::TOUCHPAD)) != 0 {
        shared |= switch_btn::CAPTURE;
    }
    r[4] = shared;

    let mut left = 0u8;
    if (input.buttons & button_mask::DPAD_DOWN) != 0 {
        left |= switch_btn::DOWN;
    }
    if (input.buttons & button_mask::DPAD_UP) != 0 {
        left |= switch_btn::UP;
    }
    if (input.buttons & button_mask::DPAD_RIGHT) != 0 {
        left |= switch_btn::RIGHT;
    }
    if (input.buttons & button_mask::DPAD_LEFT) != 0 {
        left |= switch_btn::LEFT;
    }
    if (input.buttons & button_mask::LEFT_SHOULDER) != 0 {
        left |= switch_btn::L;
    }
    if input.left_trigger > 0 {
        left |= switch_btn::ZL;
    }
    r[5] = left;

    pack_stick(&mut r[6..9], to_stick(input.left_x), to_stick(input.left_y));
    pack_stick(
        &mut r[9..12],
        to_stick(input.right_x),
        to_stick(input.right_y),
    );
    r
}

pub fn apply_motion(motion: &MotionStateRequest, state: &mut SwitchState) -> bool {
    match MotionKind::from_raw(motion.motion_type) {
        Some(MotionKind::Accelerometer) => {
            let convert = |milli: i64| clamp_i16((milli * 4096) / 9807);
            state.accel[0] = convert(-i64::from(motion.z_milli));
            state.accel[1] = convert(-i64::from(motion.x_milli));
            state.accel[2] = convert(i64::from(motion.y_milli));
            true
        }
        Some(MotionKind::Gyroscope) => {
            let convert = |milli: i64| clamp_i16((milli * GYRO_SENSITIVITY) / (936 * 1000));
            state.gyro[0] = convert(-i64::from(motion.z_milli));
            state.gyro[1] = convert(-i64::from(motion.x_milli));
            state.gyro[2] = convert(i64::from(motion.y_milli));
            true
        }
        None => false,
    }
}

pub fn apply_battery(battery: &BatteryStateRequest, state: &mut SwitchState) -> bool {
    let reported = BatteryState::from_raw(battery.flags);
    state.cable_connected = matches!(
        reported,
        BatteryState::Charging | BatteryState::Full | BatteryState::NotCharging
    );
    state.charging = reported == BatteryState::Charging;
    if battery.percent <= 100 {
        state.battery_level = if battery.percent == 0 {
            0
        } else {
            (2 * u32::from(battery.percent).div_ceil(25)) as u8
        };
    }
    true
}

pub fn handle_usb_command(
    report: &[u8],
    state: Option<&mut SwitchState>,
    reply: &mut [u8; REPORT_SIZE],
) -> usize {
    if report.len() < 2 || report[0] != USB_COMMAND_ID {
        return 0;
    }
    reply.fill(0);
    reply[0] = USB_REPLY_ID;
    reply[1] = report[1];
    match report[1] {
        USB_REQUEST_STATUS => {
            reply[2] = 0x00;
            reply[3] = 0x03;
            reply[4..10].copy_from_slice(&SWITCH_MAC);
            10
        }
        USB_HANDSHAKE => {
            if let Some(st) = state {
                st.handshake_complete = true;
            }
            2
        }
        USB_BAUDRATE | USB_HID_ONLY => 2,
        USB_DISABLE_TIMEOUT => 0,
        _ => 0,
    }
}

pub fn handle_subcommand(
    report: &[u8],
    last_input: &InputStateRequest,
    state: &mut SwitchState,
    reply: &mut [u8; REPORT_SIZE],
) -> usize {
    if report.len() < 11 || report[0] != RUMBLE_SUBCOMMAND_ID {
        return 0;
    }
    let subcommand = report[10];
    let args = &report[11..];

    let input = encode_input(last_input, Some(state));
    reply.fill(0);
    reply[0] = SUBCOMMAND_REPLY_ID;
    reply[1..13].copy_from_slice(&input[1..13]);
    reply[14] = subcommand;

    match subcommand {
        SUB_REQUEST_DEVICE_INFO => {
            reply[13] = 0x82;
            reply[15] = 0x04;
            reply[16] = 0x21;
            reply[17] = 0x03;
            reply[18] = 0x02;
            reply[19..25].copy_from_slice(&SWITCH_MAC);
            reply[25] = 0x01;
            reply[26] = 0x01;
            REPORT_SIZE
        }
        SUB_SPI_READ => {
            if args.len() < 5 {
                reply[13] = 0x80;
                return REPORT_SIZE;
            }
            let address = u32::from_le_bytes([args[0], args[1], args[2], args[3]]);
            let length = args[4].min((REPORT_SIZE - 15 - 5) as u8);
            reply[13] = 0x90;
            reply[15..20].copy_from_slice(&args[..5]);
            read_spi(address, length, &mut reply[20..]);
            REPORT_SIZE
        }
        SUB_ENABLE_IMU => {
            if let Some(&arg0) = args.first() {
                state.imu_enabled = arg0 != 0;
            }
            reply[13] = 0x80;
            REPORT_SIZE
        }
        SUB_ENABLE_VIBRATION => {
            if let Some(&arg0) = args.first() {
                state.vibration_enabled = arg0 != 0;
            }
            reply[13] = 0x80;
            REPORT_SIZE
        }
        SUB_SET_PLAYER_LIGHTS => {
            if let Some(&arg0) = args.first() {
                state.player_lights = arg0;
            }
            reply[13] = 0x80;
            REPORT_SIZE
        }
        SUB_GET_PLAYER_LIGHTS => {
            reply[13] = 0xB0;
            reply[15] = state.player_lights;
            REPORT_SIZE
        }
        SUB_SET_INPUT_MODE | SUB_SHIPMENT_STATE | SUB_SET_HOME_LIGHT => {
            reply[13] = 0x80;
            REPORT_SIZE
        }
        _ => {
            reply[13] = 0x80;
            REPORT_SIZE
        }
    }
}

pub fn queue_subcommand_reply(
    report: &[u8],
    last_input: &InputStateRequest,
    state: &mut SwitchState,
    pump: &mut ReportPump,
) -> bool {
    let mut reply = [0u8; REPORT_SIZE];
    if handle_subcommand(report, last_input, state, &mut reply) == 0
        || !pump.enqueue(&reply, reply[0], ReportKind::Priority)
    {
        return false;
    }
    if reply[14] == SUB_SET_INPUT_MODE && report.len() >= 12 && report[11] == INPUT_REPORT_ID {
        let input = encode_input(last_input, Some(state));
        return pump.enqueue(&input, input[0], ReportKind::Continuous);
    }
    true
}

pub fn decode_rumble(report: &[u8], feedback: &mut PlayStationOutputFeedback) -> bool {
    if report.len() < 10 || (report[0] != RUMBLE_SUBCOMMAND_ID && report[0] != RUMBLE_ONLY_ID) {
        return false;
    }
    let left = &report[2..6];
    let right = &report[6..10];

    let scale = |index: i32| -> u16 {
        if index <= 0 {
            0
        } else {
            let idx = (index as usize).min(AMPLITUDE_MAX);
            AMPLITUDE_BY_INDEX[idx]
        }
    };
    let high_band = |side: &[u8]| scale(i32::from((side[1] & 0xFE) >> 1));
    let low_band = |side: &[u8]| {
        scale((i32::from(side[3] & 0x7F) - 0x40) * 2 + i32::from((side[2] & 0x80) >> 7))
    };

    *feedback = PlayStationOutputFeedback::default();
    feedback.high_frequency = high_band(left).max(high_band(right));
    feedback.low_frequency = low_band(left).max(low_band(right));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_pro_handshake_spi_sticks_and_rumble() {
        let mut st = SwitchState::default();
        let mut reply = [0u8; REPORT_SIZE];
        assert_eq!(
            handle_usb_command(
                &[USB_COMMAND_ID, USB_REQUEST_STATUS],
                Some(&mut st),
                &mut reply
            ),
            10
        );
        assert_eq!(reply[0], USB_REPLY_ID);
        assert_eq!(&reply[4..10], &SWITCH_MAC);

        assert_eq!(
            handle_usb_command(&[USB_COMMAND_ID, USB_HANDSHAKE], Some(&mut st), &mut reply),
            2
        );
        assert!(st.handshake_complete);

        // Positional face buttons + positive-up stick convention.
        let input = InputStateRequest {
            buttons: button_mask::SOUTH | button_mask::EAST,
            left_y: 32767,
            ..Default::default()
        };
        let r = encode_input(&input, Some(&mut st));
        assert_eq!(
            r[3] & (switch_btn::B | switch_btn::A),
            switch_btn::B | switch_btn::A
        );
        let ly = u16::from(r[7] >> 4) | (u16::from(r[8]) << 4);
        assert!(ly > STICK_CENTER as u16);

        // SPI stick calibration + motion calibration + IMU rotation.
        let mut cmd = [0u8; 64];
        cmd[0] = RUMBLE_SUBCOMMAND_ID;
        cmd[10] = SUB_SPI_READ;
        cmd[11..15].copy_from_slice(&0x6020u32.to_le_bytes());
        cmd[15] = 24;
        assert_eq!(
            handle_subcommand(&cmd, &input, &mut st, &mut reply),
            REPORT_SIZE
        );
        assert_eq!(reply[13], 0x90);
        assert_eq!(u16::from_le_bytes([reply[26], reply[27]]), 16384);

        let motion = MotionStateRequest {
            motion_type: MotionKind::Accelerometer as u8,
            x_milli: 0,
            y_milli: 9807,
            z_milli: 0,
            ..Default::default()
        };
        assert!(apply_motion(&motion, &mut st));
        assert_eq!(st.accel, [0, 0, 4096]);

        // Rumble decoding uses non-linear table row 68 -> 32861 (~50%).
        let mut fb = PlayStationOutputFeedback::default();
        let rumble_pkt = [RUMBLE_ONLY_ID, 0, 0x00, 68 << 1, 0x00, 0x40, 0, 0, 0, 0x40];
        assert!(decode_rumble(&rumble_pkt, &mut fb));
        assert_eq!(fb.high_frequency, 32861);
        assert_eq!(fb.low_frequency, 0);
    }
}
