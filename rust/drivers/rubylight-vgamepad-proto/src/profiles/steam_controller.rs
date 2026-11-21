// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/sc26_usb.h and
// driver/src/steam_controller.h / steam_controller.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Steam Controller (2026 wired "Ibex" Triton, `VID_28DE&PID_1302`) profile.

use crate::protocol::{
    BatteryState, BatteryStateRequest, FeedbackEvent, FeedbackType, InputStateRequest, MotionKind,
    MotionStateRequest, STEAM_SETTINGS_TRIPLES_MAX, SteamControllerAttributesRequest,
    SteamHapticFeedback, SteamSettingsFeedback, TouchEvent, TouchStateRequest, button_mask,
    sc_attr_flags,
};

pub const VENDOR_ID: u16 = 0x28DE;
pub const PRODUCT_ID: u16 = 0x1302;
pub const VERSION: u16 = 0x0307;

pub const REPORT_DESCRIPTOR_IS_PROVISIONAL: bool = false;

pub const MOUSE_REPORT_ID: u8 = 0x40;
pub const KEYBOARD_REPORT_ID: u8 = 0x41;
pub const FEATURES_REPORT_ID: u8 = 0x01;
pub const FEATURES_REPORT_ID_2: u8 = 0x02;
pub const INPUT_REPORT_ID: u8 = 0x42;
pub const BATTERY_REPORT_ID: u8 = 0x43;
pub const HAPTIC_ACK_REPORT_ID: u8 = 0x44;
pub const INPUT_REPORT_ID_BLE: u8 = 0x45;
pub const WIRELESS_REPORT_ID: u8 = 0x79;
pub const REPORT_ID_7B: u8 = 0x7B;
pub const HAPTIC_RUMBLE_ID: u8 = 0x80;
pub const HAPTIC_PULSE_ID: u8 = 0x81;
pub const HAPTIC_COMMAND_ID: u8 = 0x82;
pub const HAPTIC_LFO_ID: u8 = 0x83;
pub const HAPTIC_SWEEP_ID: u8 = 0x84;
pub const HAPTIC_SCRIPT_ID: u8 = 0x85;
pub const OUTPUT_REPORT_FIRST_ID: u8 = HAPTIC_RUMBLE_ID;
pub const OUTPUT_REPORT_LAST_ID: u8 = 0x89;

pub const INPUT_REPORT_SIZE: usize = 54;
pub const BATTERY_REPORT_SIZE: usize = 15;
pub const FEATURE_REPORT_SIZE: usize = 64;
pub const HAPTIC_REPORT_SIZE: usize = 10;
pub const HAPTIC_PULSE_REPORT_SIZE: usize = 8;
pub const HAPTIC_COMMAND_REPORT_SIZE: usize = 4;

pub const CMD_CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
pub const CMD_GET_ATTRIBUTES_VALUES: u8 = 0x83;
pub const CMD_SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85;
pub const CMD_SET_SETTINGS_VALUES: u8 = 0x87;
pub const CMD_GET_SETTINGS_VALUES: u8 = 0x89;
pub const CMD_GET_SETTINGS_MAXS: u8 = 0x8B;
pub const CMD_GET_SETTINGS_DEFAULTS: u8 = 0x8C;
pub const CMD_LOAD_DEFAULT_SETTINGS: u8 = 0x8E;
pub const CMD_TRIGGER_HAPTIC_PULSE: u8 = 0x8F;
pub const CMD_GET_STRING_ATTRIBUTE: u8 = 0xAE;
pub const CMD_WRITE_C1: u8 = 0xC1;
pub const CMD_WRITE_DC: u8 = 0xDC;
pub const CMD_WRITE_E2: u8 = 0xE2;
pub const CMD_GET_KEYED_VALUE: u8 = 0xED;
pub const CMD_GET_DEVICE_INFO: u8 = 0xF2;

pub const ATTR_PRODUCT_ID: u8 = 1;
pub const ATTR_CAPABILITIES: u8 = 2;
pub const ATTR_BOOTLOADER_BUILD_TIME: u8 = 10;
pub const ATTR_FIRMWARE_BUILD_TIME: u8 = 4;
pub const ATTR_BOARD_REVISION: u8 = 9;
pub const ATTR_CONNECTION_INTERVAL_US: u8 = 11;

pub const STRING_ATTR_BOARD_SERIAL: u8 = 0x00;
pub const STRING_ATTR_UNIT_SERIAL: u8 = 0x01;
pub const STRING_ATTR_REPLY_LENGTH: u8 = 20;

pub const SETTING_LIZARD_MODE: u8 = 9;
pub const SETTING_IMU_MODE: u8 = 48;
pub const SETTING_COUNT: usize = 99;

pub const CHARGE_RESET: u8 = 0;
pub const CHARGE_DISCHARGING: u8 = 1;
pub const CHARGE_CHARGING: u8 = 2;
pub const CHARGE_SRC_VALIDATE: u8 = 3;
pub const CHARGE_DONE: u8 = 4;

pub const GYRO_COUNTS_PER_1000_DPS: i64 = 16384;
pub const ACCEL_COUNTS_PER_G: i64 = 16384;
pub const MILLI_G: i64 = 9807;

pub const CAPTURED_FIRMWARE_BUILD_TIME: u32 = 0x6A4D_85E3;
pub const CAPTURED_BOOTLOADER_BUILD_TIME: u32 = 0x68D2_F92E;
pub const CAPTURED_BOARD_REVISION: u32 = 0x4A;
pub const CAPTURED_CAPABILITIES: u32 = 0;
pub const CAPTURED_CONNECTION_INTERVAL_US: u32 = 4000;

pub const TRIGGER_CLICK_THRESHOLD: u8 = 0xF0;
pub const STICK_TOUCH_DEFLECTION: i32 = 3276;

pub mod sc26_btn {
    pub const A: u32 = 0x0000_0001;
    pub const B: u32 = 0x0000_0002;
    pub const X: u32 = 0x0000_0004;
    pub const Y: u32 = 0x0000_0008;
    pub const QAM: u32 = 0x0000_0010;
    pub const R3: u32 = 0x0000_0020;
    pub const VIEW: u32 = 0x0000_0040;
    pub const R4: u32 = 0x0000_0080;
    pub const R5: u32 = 0x0000_0100;
    pub const R: u32 = 0x0000_0200;
    pub const DPAD_DOWN: u32 = 0x0000_0400;
    pub const DPAD_RIGHT: u32 = 0x0000_0800;
    pub const DPAD_LEFT: u32 = 0x0000_1000;
    pub const DPAD_UP: u32 = 0x0000_2000;
    pub const MENU: u32 = 0x0000_4000;
    pub const L3: u32 = 0x0000_8000;
    pub const STEAM: u32 = 0x0001_0000;
    pub const L4: u32 = 0x0002_0000;
    pub const L5: u32 = 0x0004_0000;
    pub const L: u32 = 0x0008_0000;
    pub const RIGHT_STICK_TOUCH: u32 = 0x0010_0000;
    pub const RIGHT_PAD_TOUCH: u32 = 0x0020_0000;
    pub const RIGHT_PAD_CLICK: u32 = 0x0040_0000;
    pub const RIGHT_TRIGGER_CLICK: u32 = 0x0080_0000;
    pub const LEFT_STICK_TOUCH: u32 = 0x0100_0000;
    pub const LEFT_PAD_TOUCH: u32 = 0x0200_0000;
    pub const LEFT_PAD_CLICK: u32 = 0x0400_0000;
    pub const LEFT_TRIGGER_CLICK: u32 = 0x0800_0000;
    pub const RIGHT_GRIP_TOUCH: u32 = 0x1000_0000;
    pub const LEFT_GRIP_TOUCH: u32 = 0x2000_0000;
}

/// Exact 372-byte HID report descriptor of the wired 2026 Steam Controller.
#[rustfmt::skip]
pub const REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x40, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x02, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02, 0x75, 0x06, 0x95, 0x01,
    0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x02,
    0x81, 0x06, 0x95, 0x01, 0x09, 0x38, 0x81, 0x06, 0x05, 0x0c, 0x0a, 0x38, 0x02, 0x95, 0x01, 0x81,
    0x06, 0xc0, 0xc0, 0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, 0x41, 0x05, 0x07, 0x19, 0xe0, 0x29,
    0xe7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x81, 0x01, 0x19, 0x00, 0x29,
    0x65, 0x15, 0x00, 0x25, 0x65, 0x75, 0x08, 0x95, 0x06, 0x81, 0x00, 0xc0, 0x06, 0x00, 0xff, 0x09,
    0x01, 0xa1, 0x01, 0x85, 0x42, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x35, 0x09, 0x42,
    0x81, 0x02, 0x85, 0x44, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x05, 0x09, 0x44, 0x81,
    0x02, 0x85, 0x79, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x01, 0x09, 0x79, 0x81, 0x02,
    0x85, 0x43, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x0e, 0x09, 0x43, 0x81, 0x02, 0x85,
    0x7b, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x0c, 0x09, 0x7b, 0x81, 0x02, 0x85, 0x45,
    0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x2d, 0x09, 0x45, 0x81, 0x02, 0x85, 0x80, 0x15,
    0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x09, 0x09, 0x80, 0x91, 0x02, 0x85, 0x81, 0x15, 0x00,
    0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x07, 0x09, 0x81, 0x91, 0x02, 0x85, 0x82, 0x15, 0x00, 0x26,
    0xff, 0x00, 0x75, 0x08, 0x95, 0x03, 0x09, 0x82, 0x91, 0x02, 0x85, 0x83, 0x15, 0x00, 0x26, 0xff,
    0x00, 0x75, 0x08, 0x95, 0x09, 0x09, 0x83, 0x91, 0x02, 0x85, 0x84, 0x15, 0x00, 0x26, 0xff, 0x00,
    0x75, 0x08, 0x95, 0x08, 0x09, 0x84, 0x91, 0x02, 0x85, 0x85, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75,
    0x08, 0x95, 0x03, 0x09, 0x85, 0x91, 0x02, 0x85, 0x86, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08,
    0x95, 0x03, 0x09, 0x86, 0x91, 0x02, 0x85, 0x87, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95,
    0x3f, 0x09, 0x87, 0x91, 0x02, 0x85, 0x89, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x3f,
    0x09, 0x89, 0x91, 0x02, 0x85, 0x88, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x3f, 0x09,
    0x88, 0x91, 0x02, 0x85, 0x01, 0x95, 0x3f, 0x09, 0x01, 0xb1, 0x02, 0x85, 0x02, 0x95, 0x3f, 0x09,
    0x01, 0xb1, 0x02, 0xc0,
];

pub const fn report_size(id: u8) -> usize {
    match id {
        FEATURES_REPORT_ID | FEATURES_REPORT_ID_2 => FEATURE_REPORT_SIZE,
        MOUSE_REPORT_ID => 6,
        KEYBOARD_REPORT_ID => 9,
        INPUT_REPORT_ID => INPUT_REPORT_SIZE,
        BATTERY_REPORT_ID => BATTERY_REPORT_SIZE,
        HAPTIC_ACK_REPORT_ID => 6,
        INPUT_REPORT_ID_BLE => 46,
        WIRELESS_REPORT_ID => 2,
        REPORT_ID_7B => 13,
        HAPTIC_RUMBLE_ID => HAPTIC_REPORT_SIZE,
        HAPTIC_PULSE_ID => HAPTIC_PULSE_REPORT_SIZE,
        HAPTIC_COMMAND_ID => HAPTIC_COMMAND_REPORT_SIZE,
        HAPTIC_LFO_ID => 10,
        HAPTIC_SWEEP_ID => 9,
        HAPTIC_SCRIPT_ID => 4,
        0x86 => 4,
        0x87..=0x89 => 64,
        _ => 0,
    }
}

pub const fn is_output_report(id: u8) -> bool {
    id >= OUTPUT_REPORT_FIRST_ID && id <= OUTPUT_REPORT_LAST_ID
}

pub const fn default_setting(id: u8) -> u16 {
    if id == SETTING_LIZARD_MODE { 1 } else { 0 }
}

pub const fn default_settings() -> [u16; SETTING_COUNT] {
    let mut table = [0u16; SETTING_COUNT];
    let mut id = 0usize;
    while id < SETTING_COUNT {
        table[id] = default_setting(id as u8);
        id += 1;
    }
    table
}

const fn fixed_cstr(s: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut i = 0usize;
    while i < s.len() && i < 16 {
        out[i] = s[i];
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attributes {
    pub unique_id: u32,
    pub product_id: u32,
    pub capabilities: u32,
    pub firmware_build_time: u32,
    pub board_revision: u32,
    pub bootloader_build_time: u32,
    pub connection_interval_us: u32,
}

impl Default for Attributes {
    fn default() -> Self {
        Self {
            unique_id: 0,
            product_id: u32::from(PRODUCT_ID),
            capabilities: CAPTURED_CAPABILITIES,
            firmware_build_time: CAPTURED_FIRMWARE_BUILD_TIME,
            board_revision: CAPTURED_BOARD_REVISION,
            bootloader_build_time: CAPTURED_BOOTLOADER_BUILD_TIME,
            connection_interval_us: CAPTURED_CONNECTION_INTERVAL_US,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FeatureState {
    pub attrs: Attributes,
    pub unit_serial: [u8; 16],
    pub board_serial: [u8; 16],
    pub build_id: [u8; 16],
    pub settings: [u16; SETTING_COUNT],
    pub lizard_mode: bool,
    pub imu_mode: u8,
    pub reply: [u8; FEATURE_REPORT_SIZE],
    pub have_reply: bool,
    pub last_unknown_command: u8,
    pub unknown_commands: u32,
}

impl Default for FeatureState {
    fn default() -> Self {
        Self {
            attrs: Attributes::default(),
            unit_serial: fixed_cstr(b"LVGSC260000"),
            board_serial: fixed_cstr(b"LVGSC26BOARD0"),
            build_id: fixed_cstr(b"000000000000"),
            settings: default_settings(),
            lizard_mode: true,
            imu_mode: 0,
            reply: [0u8; FEATURE_REPORT_SIZE],
            have_reply: false,
            last_unknown_command: 0,
            unknown_commands: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sc26DeviceState {
    pub sequence: u8,
    pub imu_timestamp: u32,
    pub grip_touch: [bool; 2],
    pub stick_touch: [bool; 2],
    pub pad_touched: [bool; 2],
    pub pad_x: [u16; 2],
    pub pad_y: [u16; 2],
    pub pad_pressure: [u16; 2],
    pub accel: [i16; 3],
    pub gyro: [i16; 3],
    pub battery_percent: u8,
    pub charge_state: u8,
    pub stick_touch_from_deflection: bool,
    pub trigger_click_from_analog: bool,
}

impl Default for Sc26DeviceState {
    fn default() -> Self {
        Self {
            sequence: 0,
            imu_timestamp: 0,
            grip_touch: [false; 2],
            stick_touch: [false; 2],
            pad_touched: [false; 2],
            pad_x: [0; 2],
            pad_y: [0; 2],
            pad_pressure: [0; 2],
            accel: [0, 0, ACCEL_COUNTS_PER_G as i16],
            gyro: [0; 3],
            battery_percent: 100,
            charge_state: CHARGE_DISCHARGING,
            stick_touch_from_deflection: true,
            trigger_click_from_analog: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sc26State {
    pub device: Sc26DeviceState,
    pub features: FeatureState,
    pub last_motion_us: u64,
    pub last_battery_us: u64,
    pub last_report_us: u64,
    pub grip_explicit: bool,
    pub stick_touch_explicit: bool,
    pub trigger_click_explicit: bool,
}

impl Sc26State {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Stamps a per-slot serial `"LVGSC2600<NN>"` after reset so multiple virtual
    /// Steam Controllers never collide in Steam.
    pub fn stamp_slot_serial(&mut self, controller_id: u32) {
        self.features.unit_serial[9] = b'0' + ((controller_id / 10) % 10) as u8;
        self.features.unit_serial[10] = b'0' + (controller_id % 10) as u8;
    }
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

const fn pad_axis_x(x: u16) -> i16 {
    clamp_i16((x as i64) - 32768)
}

const fn pad_axis_y(y: u16) -> i16 {
    clamp_i16(32768 - (y as i64))
}

const fn trigger_axis(trigger: u8) -> i16 {
    (((trigger as i32) * 32767) / 255) as i16
}

pub fn sc26_tick(state: &mut Sc26State, now_us: u64) {
    state.device.imu_timestamp = now_us as u32;
    if !state.grip_explicit {
        state.device.grip_touch = [false; 2];
    }
}

pub fn sc26_buttons(buttons: u32, state: &Sc26State) -> u32 {
    const MAP: [(u32, u32); 20] = [
        (button_mask::SOUTH, sc26_btn::A),
        (button_mask::EAST, sc26_btn::B),
        (button_mask::WEST, sc26_btn::X),
        (button_mask::NORTH, sc26_btn::Y),
        (button_mask::DPAD_UP, sc26_btn::DPAD_UP),
        (button_mask::DPAD_DOWN, sc26_btn::DPAD_DOWN),
        (button_mask::DPAD_LEFT, sc26_btn::DPAD_LEFT),
        (button_mask::DPAD_RIGHT, sc26_btn::DPAD_RIGHT),
        (button_mask::START, sc26_btn::VIEW),
        (button_mask::BACK, sc26_btn::MENU),
        (button_mask::LEFT_STICK, sc26_btn::L3),
        (button_mask::RIGHT_STICK, sc26_btn::R3),
        (button_mask::LEFT_SHOULDER, sc26_btn::L),
        (button_mask::RIGHT_SHOULDER, sc26_btn::R),
        (button_mask::HOME, sc26_btn::STEAM),
        (button_mask::MISC, sc26_btn::QAM),
        (button_mask::PADDLE_1, sc26_btn::R4),
        (button_mask::PADDLE_2, sc26_btn::L4),
        (button_mask::PADDLE_3, sc26_btn::R5),
        (button_mask::PADDLE_4, sc26_btn::L5),
    ];

    let mut out = 0u32;
    for &(from, to) in &MAP {
        if (buttons & from) != 0 {
            out |= to;
        }
    }
    if (buttons & button_mask::SC26_BUTTONS_EXPLICIT) != 0 {
        if (buttons & button_mask::LEFT_PAD_CLICK) != 0 {
            out |= sc26_btn::LEFT_PAD_CLICK;
        }
        if (buttons & button_mask::RIGHT_PAD_CLICK) != 0 {
            out |= sc26_btn::RIGHT_PAD_CLICK;
        }
        if (buttons & button_mask::LEFT_TRIGGER_CLICK) != 0 {
            out |= sc26_btn::LEFT_TRIGGER_CLICK;
        }
        if (buttons & button_mask::RIGHT_TRIGGER_CLICK) != 0 {
            out |= sc26_btn::RIGHT_TRIGGER_CLICK;
        }
    } else if (buttons & button_mask::TOUCHPAD) != 0 {
        let left = state.device.pad_touched[0];
        let right = state.device.pad_touched[1];
        if left {
            out |= sc26_btn::LEFT_PAD_CLICK;
        }
        if right || !left {
            out |= sc26_btn::RIGHT_PAD_CLICK;
        }
    }
    out
}

pub const fn classify_buttons(st: &Sc26DeviceState, mut buttons: u32) -> u32 {
    if st.stick_touch_from_deflection {
        buttons &= !(sc26_btn::LEFT_STICK_TOUCH | sc26_btn::RIGHT_STICK_TOUCH);
    }
    buttons
}

pub fn encode_input_with_buttons(
    buttons: u32,
    input: &InputStateRequest,
    st: &mut Sc26DeviceState,
) -> [u8; INPUT_REPORT_SIZE] {
    let mut r = [0u8; INPUT_REPORT_SIZE];
    r[0] = INPUT_REPORT_ID;
    r[1] = st.sequence;
    st.sequence = st.sequence.wrapping_add(1);

    let mut bits = buttons;
    if st.trigger_click_from_analog {
        if input.left_trigger >= TRIGGER_CLICK_THRESHOLD {
            bits |= sc26_btn::LEFT_TRIGGER_CLICK;
        }
        if input.right_trigger >= TRIGGER_CLICK_THRESHOLD {
            bits |= sc26_btn::RIGHT_TRIGGER_CLICK;
        }
    }
    if st.pad_touched[0] {
        bits |= sc26_btn::LEFT_PAD_TOUCH;
    }
    if st.pad_touched[1] {
        bits |= sc26_btn::RIGHT_PAD_TOUCH;
    }
    if st.grip_touch[0] {
        bits |= sc26_btn::LEFT_GRIP_TOUCH;
    }
    if st.grip_touch[1] {
        bits |= sc26_btn::RIGHT_GRIP_TOUCH;
    }
    if st.stick_touch[0] {
        bits |= sc26_btn::LEFT_STICK_TOUCH;
    }
    if st.stick_touch[1] {
        bits |= sc26_btn::RIGHT_STICK_TOUCH;
    }
    if st.stick_touch_from_deflection {
        let deflected = |x: i16, y: i16| -> bool {
            i32::from(x).abs() > STICK_TOUCH_DEFLECTION
                || i32::from(y).abs() > STICK_TOUCH_DEFLECTION
        };
        if deflected(input.left_x, input.left_y) {
            bits |= sc26_btn::LEFT_STICK_TOUCH;
        }
        if deflected(input.right_x, input.right_y) {
            bits |= sc26_btn::RIGHT_STICK_TOUCH;
        }
    }

    r[2..6].copy_from_slice(&bits.to_le_bytes());
    r[6..8].copy_from_slice(&trigger_axis(input.left_trigger).to_le_bytes());
    r[8..10].copy_from_slice(&trigger_axis(input.right_trigger).to_le_bytes());
    r[10..12].copy_from_slice(&input.left_x.to_le_bytes());
    r[12..14].copy_from_slice(&input.left_y.to_le_bytes());
    r[14..16].copy_from_slice(&input.right_x.to_le_bytes());
    r[16..18].copy_from_slice(&input.right_y.to_le_bytes());

    if st.pad_touched[0] {
        r[18..20].copy_from_slice(&pad_axis_x(st.pad_x[0]).to_le_bytes());
        r[20..22].copy_from_slice(&pad_axis_y(st.pad_y[0]).to_le_bytes());
        r[22..24].copy_from_slice(&(st.pad_pressure[0] >> 1).to_le_bytes());
    }
    if st.pad_touched[1] {
        r[24..26].copy_from_slice(&pad_axis_x(st.pad_x[1]).to_le_bytes());
        r[26..28].copy_from_slice(&pad_axis_y(st.pad_y[1]).to_le_bytes());
        r[28..30].copy_from_slice(&(st.pad_pressure[1] >> 1).to_le_bytes());
    }

    r[30..34].copy_from_slice(&st.imu_timestamp.to_le_bytes());
    r[34..36].copy_from_slice(&st.accel[0].to_le_bytes());
    r[36..38].copy_from_slice(&st.accel[1].to_le_bytes());
    r[38..40].copy_from_slice(&st.accel[2].to_le_bytes());
    r[40..42].copy_from_slice(&st.gyro[0].to_le_bytes());
    r[42..44].copy_from_slice(&st.gyro[1].to_le_bytes());
    r[44..46].copy_from_slice(&st.gyro[2].to_le_bytes());
    r[46..48].copy_from_slice(&32767i16.to_le_bytes()); // identity quat_w
    r
}

pub fn encode_input(input: &InputStateRequest, state: &mut Sc26State) -> [u8; INPUT_REPORT_SIZE] {
    let explicit_buttons = (input.buttons & button_mask::SC26_BUTTONS_EXPLICIT) != 0;
    if explicit_buttons {
        state.trigger_click_explicit = true;
        state.grip_explicit = true;
        state.stick_touch_explicit = true;
        state.device.stick_touch_from_deflection = false;
    }
    state.device.trigger_click_from_analog = !state.trigger_click_explicit;
    let grip_bits = button_mask::LEFT_GRIP_TOUCH | button_mask::RIGHT_GRIP_TOUCH;
    if (input.buttons & grip_bits) != 0 {
        state.grip_explicit = true;
    }
    if state.grip_explicit {
        state.device.grip_touch[0] = (input.buttons & button_mask::LEFT_GRIP_TOUCH) != 0;
        state.device.grip_touch[1] = (input.buttons & button_mask::RIGHT_GRIP_TOUCH) != 0;
    }
    let stick_touch_bits = button_mask::LEFT_STICK_TOUCH | button_mask::RIGHT_STICK_TOUCH;
    if (input.buttons & stick_touch_bits) != 0 {
        state.stick_touch_explicit = true;
        state.device.stick_touch_from_deflection = false;
    }
    if state.stick_touch_explicit {
        state.device.stick_touch[0] = (input.buttons & button_mask::LEFT_STICK_TOUCH) != 0;
        state.device.stick_touch[1] = (input.buttons & button_mask::RIGHT_STICK_TOUCH) != 0;
    }

    let btns = sc26_buttons(input.buttons, state);
    encode_input_with_buttons(btns, input, &mut state.device)
}

pub fn encode_battery(state: &Sc26State) -> [u8; BATTERY_REPORT_SIZE] {
    let st = &state.device;
    let mut r = [0u8; BATTERY_REPORT_SIZE];
    let level = st.battery_percent.min(100);
    let on_usb = st.charge_state == CHARGE_CHARGING || st.charge_state == CHARGE_DONE;
    let vbat = 3522u16 + u16::from(level) * 6;
    let vin = if on_usb { 4980u16 } else { 0 };
    let curr = if on_usb { 157u16 } else { 0 };
    let iin = if on_usb { 239u16 } else { 0 };

    r[0] = BATTERY_REPORT_ID;
    r[1] = st.charge_state;
    r[2] = level;
    r[3..5].copy_from_slice(&vbat.to_le_bytes());
    r[5..7].copy_from_slice(&4160u16.to_le_bytes());
    r[7..9].copy_from_slice(&vin.to_le_bytes());
    r[9..11].copy_from_slice(&curr.to_le_bytes());
    r[11..13].copy_from_slice(&iin.to_le_bytes());
    r[13..15].copy_from_slice(&0x76EDu16.to_le_bytes());
    r
}

pub fn apply_touch(touch: &TouchStateRequest, state: &mut Sc26State) -> bool {
    let Some(event) = TouchEvent::from_raw(touch.event_type) else {
        return false;
    };
    let st = &mut state.device;
    if event == TouchEvent::CancelAll {
        st.pad_touched = [false; 2];
        return true;
    }
    if touch.contact_index >= 2 {
        return false;
    }
    let pad = usize::from(touch.contact_index);
    match event {
        TouchEvent::Hover | TouchEvent::Move => {
            if !st.pad_touched[pad] {
                return false;
            }
            st.pad_x[pad] = touch.x;
            st.pad_y[pad] = touch.y;
            st.pad_pressure[pad] = touch.pressure;
            true
        }
        TouchEvent::Down => {
            st.pad_touched[pad] = true;
            st.pad_x[pad] = touch.x;
            st.pad_y[pad] = touch.y;
            st.pad_pressure[pad] = touch.pressure;
            true
        }
        TouchEvent::Up | TouchEvent::Cancel => {
            st.pad_touched[pad] = false;
            st.pad_pressure[pad] = 0;
            true
        }
        TouchEvent::CancelAll => true,
    }
}

pub fn apply_accel_milli(st: &mut Sc26DeviceState, x: i32, y: i32, z: i32) {
    let convert = |milli: i64| clamp_i16((milli * ACCEL_COUNTS_PER_G) / MILLI_G);
    st.accel[0] = convert(i64::from(x));
    st.accel[2] = convert(i64::from(y));
    st.accel[1] = convert(-i64::from(z));
}

pub fn apply_gyro_milli(st: &mut Sc26DeviceState, x: i32, y: i32, z: i32) {
    let convert = |milli: i64| clamp_i16((milli * GYRO_COUNTS_PER_1000_DPS) / 1_000_000);
    st.gyro[0] = convert(i64::from(x));
    st.gyro[2] = convert(i64::from(y));
    st.gyro[1] = convert(-i64::from(z));
}

pub fn apply_motion(motion: &MotionStateRequest, state: &mut Sc26State, now_us: u64) -> bool {
    match MotionKind::from_raw(motion.motion_type) {
        Some(MotionKind::Accelerometer) => {
            apply_accel_milli(
                &mut state.device,
                motion.x_milli,
                motion.y_milli,
                motion.z_milli,
            );
        }
        Some(MotionKind::Gyroscope) => {
            apply_gyro_milli(
                &mut state.device,
                motion.x_milli,
                motion.y_milli,
                motion.z_milli,
            );
        }
        None => return false,
    }
    state.last_motion_us = if now_us != 0 { now_us } else { 1 };
    true
}

pub fn apply_battery(battery: &BatteryStateRequest, state: &mut Sc26State) -> bool {
    let st = &mut state.device;
    if battery.percent <= 100 {
        st.battery_percent = battery.percent;
    }
    match BatteryState::from_raw(battery.flags) {
        BatteryState::Charging => st.charge_state = CHARGE_CHARGING,
        BatteryState::Full => {
            st.charge_state = CHARGE_DONE;
            st.battery_percent = 100;
        }
        _ => st.charge_state = CHARGE_DISCHARGING,
    }
    true
}

pub fn printable_attribute_string(text: &[u8; 16]) -> bool {
    let mut n = 0usize;
    while n < 16 && text[n] != 0 {
        if !(0x20..=0x7E).contains(&text[n]) {
            return false;
        }
        n += 1;
    }
    n != 0
}

fn store_attribute_string(out: &mut [u8; 16], text: &[u8; 16]) {
    out.fill(0);
    for n in 0..15 {
        if text[n] == 0 {
            break;
        }
        out[n] = text[n];
    }
}

pub fn apply_attributes(req: &SteamControllerAttributesRequest, state: &mut Sc26State) -> u32 {
    let fs = &mut state.features;
    let mut applied = 0u32;
    if (req.flags & sc_attr_flags::FIRMWARE_BUILD_TIME) != 0 {
        fs.attrs.firmware_build_time = req.firmware_build_time;
        applied |= sc_attr_flags::FIRMWARE_BUILD_TIME;
    }
    if (req.flags & sc_attr_flags::BOOTLOADER_BUILD_TIME) != 0 {
        fs.attrs.bootloader_build_time = req.bootloader_build_time;
        applied |= sc_attr_flags::BOOTLOADER_BUILD_TIME;
    }
    if (req.flags & sc_attr_flags::BOARD_REVISION) != 0 {
        fs.attrs.board_revision = req.board_revision;
        applied |= sc_attr_flags::BOARD_REVISION;
    }
    if (req.flags & sc_attr_flags::CAPABILITIES) != 0 {
        fs.attrs.capabilities = req.capabilities;
        applied |= sc_attr_flags::CAPABILITIES;
    }
    if (req.flags & sc_attr_flags::BUILD_ID) != 0 && printable_attribute_string(&req.build_id) {
        store_attribute_string(&mut fs.build_id, &req.build_id);
        applied |= sc_attr_flags::BUILD_ID;
    }
    if (req.flags & sc_attr_flags::SERIALS) != 0 {
        if printable_attribute_string(&req.unit_serial) {
            store_attribute_string(&mut fs.unit_serial, &req.unit_serial);
            applied |= sc_attr_flags::SERIALS;
        }
        if printable_attribute_string(&req.board_serial) {
            store_attribute_string(&mut fs.board_serial, &req.board_serial);
            applied |= sc_attr_flags::SERIALS;
        }
    }
    applied
}

pub fn apply_output(data: &[u8], controller_id: u32) -> Option<FeedbackEvent> {
    if data.len() < 2 || !is_output_report(data[0]) || data[0] > HAPTIC_SCRIPT_ID {
        return None;
    }
    let declared = report_size(data[0]);
    if data.len() < declared || declared == 0 || declared > 12 {
        return None;
    }
    let mut payload = SteamHapticFeedback {
        length: declared as u8,
        ..Default::default()
    };
    payload.report[..declared].copy_from_slice(&data[..declared]);
    Some(FeedbackEvent::new(
        controller_id,
        FeedbackType::SteamHaptic,
        &payload.to_bytes(),
    ))
}

pub fn apply_settings(data: &[u8], controller_id: u32, events: &mut [FeedbackEvent]) -> usize {
    if data.len() < 3 || data[0] != FEATURES_REPORT_ID {
        return 0;
    }
    let command = data[1];
    let length = usize::from(data[2]);
    if length > data.len() - 3 {
        return 0;
    }
    let payload = &data[3..];
    let triples = if command == CMD_SET_SETTINGS_VALUES {
        let count = length / 3;
        if count == 0 {
            return 0;
        }
        count
    } else if command == CMD_LOAD_DEFAULT_SETTINGS {
        0
    } else {
        return 0;
    };

    let needed = if command == CMD_LOAD_DEFAULT_SETTINGS {
        1
    } else {
        triples.div_ceil(STEAM_SETTINGS_TRIPLES_MAX)
    };
    if needed > events.len() {
        return 0;
    }

    for (n, slot) in events[..needed].iter_mut().enumerate() {
        let first = n * STEAM_SETTINGS_TRIPLES_MAX;
        let count = (triples - first).min(STEAM_SETTINGS_TRIPLES_MAX);
        let mut chunk = SteamSettingsFeedback {
            command,
            count: count as u8,
            ..Default::default()
        };
        if count != 0 {
            chunk.triples[..count * 3].copy_from_slice(&payload[first * 3..first * 3 + count * 3]);
        }
        *slot = FeedbackEvent::new(
            controller_id,
            FeedbackType::SteamSettings,
            &chunk.to_bytes(),
        );
    }
    needed
}

pub fn set_feature(id: u8, data: &[u8], state: &mut Sc26State) -> bool {
    if id != FEATURES_REPORT_ID || data.len() < 3 || data[0] != FEATURES_REPORT_ID {
        return false;
    }
    let kind = data[1];
    let length = usize::from(data[2]);
    if length > data.len() - 3 {
        return false;
    }
    let payload = &data[3..3 + length];
    let fs = &mut state.features;
    fs.reply.fill(0);
    fs.reply[0] = FEATURES_REPORT_ID;
    fs.reply[1] = kind;
    fs.reply[2] = 0;
    fs.have_reply = true;
    let out_cap = FEATURE_REPORT_SIZE - 3;

    match kind {
        CMD_CLEAR_DIGITAL_MAPPINGS | CMD_SET_DEFAULT_DIGITAL_MAPPINGS => true,
        CMD_LOAD_DEFAULT_SETTINGS => {
            fs.settings = default_settings();
            fs.lizard_mode = true;
            fs.imu_mode = 0;
            true
        }
        CMD_SET_SETTINGS_VALUES => {
            for chunk in payload.as_chunks::<3>().0 {
                let id = chunk[0];
                let val = u16::from_le_bytes([chunk[1], chunk[2]]);
                if usize::from(id) < SETTING_COUNT {
                    fs.settings[usize::from(id)] = val;
                }
                if id == SETTING_LIZARD_MODE {
                    fs.lizard_mode = val != 0;
                }
                if id == SETTING_IMU_MODE {
                    fs.imu_mode = val as u8;
                }
            }
            true
        }
        CMD_GET_SETTINGS_VALUES | CMD_GET_SETTINGS_DEFAULTS | CMD_GET_SETTINGS_MAXS => {
            let max_triples = out_cap / 3;
            let mut n = 0usize;
            if length == 0 {
                for id in 0..SETTING_COUNT {
                    if n >= max_triples {
                        break;
                    }
                    let v = match kind {
                        CMD_GET_SETTINGS_VALUES => fs.settings[id],
                        CMD_GET_SETTINGS_MAXS => 0xFFFF,
                        _ => default_setting(id as u8),
                    };
                    fs.reply[3 + n * 3] = id as u8;
                    fs.reply[4 + n * 3..6 + n * 3].copy_from_slice(&v.to_le_bytes());
                    n += 1;
                }
            } else {
                for &id in payload {
                    if n >= max_triples {
                        break;
                    }
                    let v = match kind {
                        CMD_GET_SETTINGS_VALUES if usize::from(id) < SETTING_COUNT => {
                            fs.settings[usize::from(id)]
                        }
                        CMD_GET_SETTINGS_MAXS => 0xFFFF,
                        _ if usize::from(id) < SETTING_COUNT => default_setting(id),
                        _ => 0,
                    };
                    fs.reply[3 + n * 3] = id;
                    fs.reply[4 + n * 3..6 + n * 3].copy_from_slice(&v.to_le_bytes());
                    n += 1;
                }
            }
            fs.reply[2] = (n * 3) as u8;
            true
        }
        CMD_GET_ATTRIBUTES_VALUES => {
            let list = [
                (ATTR_PRODUCT_ID, fs.attrs.product_id),
                (ATTR_CAPABILITIES, fs.attrs.capabilities),
                (ATTR_BOOTLOADER_BUILD_TIME, fs.attrs.bootloader_build_time),
                (ATTR_FIRMWARE_BUILD_TIME, fs.attrs.firmware_build_time),
                (ATTR_BOARD_REVISION, fs.attrs.board_revision),
                (ATTR_CONNECTION_INTERVAL_US, fs.attrs.connection_interval_us),
            ];
            let mut n = 0usize;
            for (tag, val) in list {
                if (n + 1) * 5 > out_cap {
                    break;
                }
                fs.reply[3 + n * 5] = tag;
                fs.reply[4 + n * 5..8 + n * 5].copy_from_slice(&val.to_le_bytes());
                n += 1;
            }
            fs.reply[2] = (n * 5) as u8;
            true
        }
        CMD_GET_STRING_ATTRIBUTE => {
            let tag = payload.first().copied().unwrap_or(STRING_ATTR_UNIT_SERIAL);
            fs.reply[3] = tag;
            let text = if tag == STRING_ATTR_BOARD_SERIAL {
                &fs.board_serial
            } else {
                &fs.unit_serial
            };
            let max_copy = usize::from(STRING_ATTR_REPLY_LENGTH - 1);
            for (n, &b) in text.iter().take(max_copy).enumerate() {
                if b == 0 {
                    break;
                }
                fs.reply[4 + n] = b;
            }
            fs.reply[2] = STRING_ATTR_REPLY_LENGTH;
            true
        }
        CMD_WRITE_C1 | CMD_WRITE_DC | CMD_WRITE_E2 => true,
        CMD_GET_KEYED_VALUE => {
            if payload == b"user/wireless_transport\0" {
                fs.reply[3] = 0;
                fs.reply[2] = 1;
            }
            true
        }
        CMD_GET_DEVICE_INFO => {
            let sub = payload.first().copied().unwrap_or(0);
            fs.reply[3] = sub;
            if sub == 0 {
                fs.reply[4..8].copy_from_slice(&fs.attrs.firmware_build_time.to_le_bytes());
                fs.reply[8..12].copy_from_slice(&fs.attrs.board_revision.to_le_bytes());
                for (i, &b) in fs.build_id.iter().enumerate() {
                    if b == 0 {
                        break;
                    }
                    fs.reply[12 + i] = b;
                }
                for (i, &b) in fs.unit_serial.iter().enumerate() {
                    if b == 0 {
                        break;
                    }
                    fs.reply[28 + i] = b;
                }
                fs.reply[2] = 41;
            } else if sub == 1 {
                fs.reply[2] = 34;
            } else {
                fs.reply[6] = 1;
                fs.reply[2] = 9;
            }
            true
        }
        CMD_TRIGGER_HAPTIC_PULSE => true,
        _ => {
            fs.last_unknown_command = kind;
            fs.unknown_commands += 1;
            true
        }
    }
}

pub fn get_feature(id: u8, buffer: &mut [u8], state: &Sc26State) -> usize {
    if id != FEATURES_REPORT_ID || buffer.len() < FEATURE_REPORT_SIZE {
        return 0;
    }
    buffer.fill(0);
    if state.features.have_reply {
        buffer[..FEATURE_REPORT_SIZE].copy_from_slice(&state.features.reply);
    } else {
        buffer[0] = FEATURES_REPORT_ID;
    }
    FEATURE_REPORT_SIZE
}

pub fn decode_haptic_output(data: &[u8], left: &mut u16, right: &mut u16) -> bool {
    if data.len() < 2 {
        return false;
    }
    if data[0] == HAPTIC_RUMBLE_ID {
        if data.len() < HAPTIC_REPORT_SIZE {
            return false;
        }
        *left = u16::from_le_bytes([data[4], data[5]]);
        *right = u16::from_le_bytes([data[7], data[8]]);
        return true;
    }
    if data[0] == HAPTIC_PULSE_ID {
        if data.len() < HAPTIC_PULSE_REPORT_SIZE {
            return false;
        }
        let on_us = u16::from_le_bytes([data[2], data[3]]);
        let off_us = u16::from_le_bytes([data[4], data[5]]);
        let repeat = u16::from_le_bytes([data[6], data[7]]);
        let mut magnitude = 0u16;
        if repeat != 0 && on_us != 0 {
            let period = (u32::from(on_us) + u32::from(off_us)).max(1);
            magnitude = ((u32::from(on_us) * 65535) / period) as u16;
        }
        if data[1] == 1 {
            *left = magnitude;
        } else {
            *right = magnitude;
        }
        return true;
    }
    false
}

pub fn encode_haptic_ack(data: &[u8], ack: &mut [u8]) -> usize {
    if ack.len() < 6 || data.len() < HAPTIC_PULSE_REPORT_SIZE || data[0] != HAPTIC_PULSE_ID {
        return 0;
    }
    if data[2..8].iter().any(|&b| b != 0) {
        return 0;
    }
    ack[0] = HAPTIC_ACK_REPORT_ID;
    ack[1] = if data[1] == 0 { 0x04 } else { 0x03 };
    ack[2] = 0x02;
    ack[3..6].fill(0);
    6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_cmd(hex: &str) -> [u8; 64] {
        let mut out = [0u8; 64];
        let bytes = hex.as_bytes();
        for i in 0..(bytes.len() / 2) {
            let nibble = |b: u8| match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => panic!("bad hex"),
            };
            out[i] = (nibble(bytes[i * 2]) << 4) | nibble(bytes[i * 2 + 1]);
        }
        out
    }

    #[test]
    fn sc26_handshake_replay_and_attributes() {
        assert_eq!(REPORT_DESCRIPTOR.len(), 372);
        let mut st = Sc26State::default();
        st.stamp_slot_serial(7);
        assert_eq!(&st.features.unit_serial[..11], b"LVGSC260007");

        let steam_cmds = [
            ("018300", 30u8),
            ("01ae15010213000002000000000a2ef9d26804e3854d6a09", 20),
            ("018703328403", 0),
            ("018703090000", 0),
            ("01ae150000000000009fbeeea6fa7f0000f001bd04e70100", 20),
            ("018100", 0),
            ("01870f301800070700080700310200520300", 0),
            ("01870f1800002e000034ffff35ffff2e0000", 0),
            ("01870634ffff35ffff", 0),
            ("01f20100", 41),
            ("01f20101", 34),
            ("01f20102", 9),
            ("0187032d6400", 0),
            ("01c110ffffffff030905ffffffffffffffffff", 0),
            ("01dc020102", 0),
            ("01e2020120", 0),
            ("01ed096573622f626f6e6400", 0),
            ("01ed0b6573622f626f6e645f3200", 0),
            ("01ed18757365722f776972656c6573735f7472616e73706f727400", 1),
            ("018500", 0),
            ("018e00", 0),
        ];

        let mut reply = [0u8; 64];
        for (hex, reply_len) in steam_cmds {
            let cmd = hex_cmd(hex);
            assert!(set_feature(FEATURES_REPORT_ID, &cmd, &mut st), "{hex}");
            assert_eq!(get_feature(FEATURES_REPORT_ID, &mut reply, &st), 64);
            assert_eq!(reply[1], cmd[1]);
            assert_eq!(reply[2], reply_len, "{hex}");
            if hex == "0187032d6400" {
                assert!(!st.features.lizard_mode);
                assert_eq!(st.features.imu_mode, 0x18);
                assert_eq!(st.features.settings[50], 900);
                assert_eq!(st.features.settings[45], 100);
            }
        }
        assert_eq!(st.features.unknown_commands, 0);
        assert!(st.features.lizard_mode);
        assert_eq!(st.features.imu_mode, 0);

        // Haptic stop ack & rumble duty cycle.
        let stop = [HAPTIC_PULSE_ID, 1, 0, 0, 0, 0, 0, 0];
        let mut ack = [0u8; 6];
        assert_eq!(encode_haptic_ack(&stop, &mut ack), 6);
        assert_eq!(ack, [0x44, 0x03, 0x02, 0, 0, 0]);
        let click = [HAPTIC_PULSE_ID, 1, 0x90, 0x01, 0, 0, 0x01, 0];
        let (mut left, mut right) = (0, 0);
        assert!(decode_haptic_output(&click, &mut left, &mut right));
        assert_eq!(left, 65535);
    }

    #[test]
    fn sc26_explicit_latches_and_trigger_click_threshold() {
        let mut st = Sc26State::default();
        let below = encode_input(
            &InputStateRequest {
                left_trigger: 0xEF,
                right_trigger: 0xEF,
                ..Default::default()
            },
            &mut st,
        );
        let b_below = u32::from_le_bytes([below[2], below[3], below[4], below[5]]);
        assert_eq!(
            b_below & (sc26_btn::LEFT_TRIGGER_CLICK | sc26_btn::RIGHT_TRIGGER_CLICK),
            0
        );

        let at = encode_input(
            &InputStateRequest {
                left_trigger: 0xF0,
                right_trigger: 0xF0,
                ..Default::default()
            },
            &mut st,
        );
        let b_at = u32::from_le_bytes([at[2], at[3], at[4], at[5]]);
        assert_eq!(
            b_at & (sc26_btn::LEFT_TRIGGER_CLICK | sc26_btn::RIGHT_TRIGGER_CLICK),
            sc26_btn::LEFT_TRIGGER_CLICK | sc26_btn::RIGHT_TRIGGER_CLICK
        );

        // Negative saturation on IMU scaling (INT32_MIN).
        apply_accel_milli(&mut st.device, i32::MAX, i32::MIN, i32::MIN);
        assert_eq!(st.device.accel[0], 32767);
        assert_eq!(st.device.accel[1], 32767);
        assert_eq!(st.device.accel[2], -32768);
    }
}
