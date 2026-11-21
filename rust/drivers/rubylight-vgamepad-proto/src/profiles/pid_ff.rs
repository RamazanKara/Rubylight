// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/pid_ff.h and pid_ff.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! DirectInput Physical Interface Device (PID) force-feedback engine.

pub const PID_STATE_REPORT_ID: u8 = 3;
pub const PID_SET_EFFECT_REPORT_ID: u8 = 0x11;
pub const PID_SET_ENVELOPE_REPORT_ID: u8 = 0x12;
pub const PID_SET_CONDITION_REPORT_ID: u8 = 0x13;
pub const PID_SET_PERIODIC_REPORT_ID: u8 = 0x14;
pub const PID_SET_CONSTANT_FORCE_REPORT_ID: u8 = 0x15;
pub const PID_SET_RAMP_FORCE_REPORT_ID: u8 = 0x16;
pub const PID_EFFECT_OPERATION_REPORT_ID: u8 = 0x17;
pub const PID_BLOCK_FREE_REPORT_ID: u8 = 0x18;
pub const PID_DEVICE_CONTROL_REPORT_ID: u8 = 0x19;
pub const PID_DEVICE_GAIN_REPORT_ID: u8 = 0x1A;
pub const PID_CREATE_NEW_EFFECT_REPORT_ID: u8 = 0x1B;
pub const PID_BLOCK_LOAD_REPORT_ID: u8 = 0x1C;
pub const PID_POOL_REPORT_ID: u8 = 0x1D;

pub const PID_NOMINAL_MAX: i32 = 10000;
pub const PID_MAX_EFFECTS: usize = 16;

const PID_RAM_POOL_SIZE: u16 = 1024;
const PID_BYTES_PER_EFFECT: u16 = PID_RAM_POOL_SIZE / (PID_MAX_EFFECTS as u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PidEffectType {
    #[default]
    None = 0,
    ConstantForce = 1,
    Ramp = 2,
    Square = 3,
    Sine = 4,
    Triangle = 5,
    SawtoothUp = 6,
    SawtoothDown = 7,
    Spring = 8,
    Damper = 9,
    Inertia = 10,
    Friction = 11,
    CustomForceData = 12,
}

impl PidEffectType {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::None),
            1 => Some(Self::ConstantForce),
            2 => Some(Self::Ramp),
            3 => Some(Self::Square),
            4 => Some(Self::Sine),
            5 => Some(Self::Triangle),
            6 => Some(Self::SawtoothUp),
            7 => Some(Self::SawtoothDown),
            8 => Some(Self::Spring),
            9 => Some(Self::Damper),
            10 => Some(Self::Inertia),
            11 => Some(Self::Friction),
            12 => Some(Self::CustomForceData),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PidEffectOperation {
    Start = 1,
    StartSolo = 2,
    Stop = 3,
}

impl PidEffectOperation {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Start),
            2 => Some(Self::StartSolo),
            3 => Some(Self::Stop),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PidDeviceControl {
    EnableActuators = 1,
    DisableActuators = 2,
    StopAllEffects = 3,
    DeviceReset = 4,
    DevicePause = 5,
    DeviceContinue = 6,
}

impl PidDeviceControl {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::EnableActuators),
            2 => Some(Self::DisableActuators),
            3 => Some(Self::StopAllEffects),
            4 => Some(Self::DeviceReset),
            5 => Some(Self::DevicePause),
            6 => Some(Self::DeviceContinue),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PidBlockLoadStatus {
    Success = 1,
    Full = 2,
    Error = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PidRumble {
    pub low_frequency: u16,
    pub high_frequency: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PidEffect {
    pub allocated: bool,
    pub playing: bool,
    pub effect_type: PidEffectType,
    pub duration_ms: u16,
    pub start_delay_ms: u16,
    pub gain: u8,
    pub loop_count: u8,
    pub constant_magnitude: i16,
    pub periodic_magnitude: u16,
    pub ramp_start: i16,
    pub ramp_end: i16,
    pub envelope_attack_level: u16,
    pub envelope_fade_level: u16,
    pub envelope_attack_ms: u16,
    pub envelope_fade_ms: u16,
    pub elapsed_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PidReportBits {
    pub report_id: u8,
    pub input_bits: usize,
    pub output_bits: usize,
    pub feature_bits: usize,
}

pub const PID_REPORT_BITS: &[PidReportBits] = &[
    PidReportBits {
        report_id: 1,
        input_bits: 32 + 4 + 4 + 64 + 16,
        output_bits: 0,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: 2,
        input_bits: 0,
        output_bits: 40,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_STATE_REPORT_ID,
        input_bits: 16,
        output_bits: 0,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_SET_EFFECT_REPORT_ID,
        input_bits: 0,
        output_bits: 120,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_SET_ENVELOPE_REPORT_ID,
        input_bits: 0,
        output_bits: 72,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_SET_CONDITION_REPORT_ID,
        input_bits: 0,
        output_bits: 112,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_SET_PERIODIC_REPORT_ID,
        input_bits: 0,
        output_bits: 72,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_SET_CONSTANT_FORCE_REPORT_ID,
        input_bits: 0,
        output_bits: 24,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_SET_RAMP_FORCE_REPORT_ID,
        input_bits: 0,
        output_bits: 40,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_EFFECT_OPERATION_REPORT_ID,
        input_bits: 0,
        output_bits: 24,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_BLOCK_FREE_REPORT_ID,
        input_bits: 0,
        output_bits: 8,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_DEVICE_CONTROL_REPORT_ID,
        input_bits: 0,
        output_bits: 8,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_DEVICE_GAIN_REPORT_ID,
        input_bits: 0,
        output_bits: 8,
        feature_bits: 0,
    },
    PidReportBits {
        report_id: PID_CREATE_NEW_EFFECT_REPORT_ID,
        input_bits: 0,
        output_bits: 0,
        feature_bits: 24,
    },
    PidReportBits {
        report_id: PID_BLOCK_LOAD_REPORT_ID,
        input_bits: 0,
        output_bits: 0,
        feature_bits: 32,
    },
    PidReportBits {
        report_id: PID_POOL_REPORT_ID,
        input_bits: 0,
        output_bits: 0,
        feature_bits: 32,
    },
];

/// Complete HID GamePad + Physical Interface Device (PID) descriptor.
#[rustfmt::skip]
pub const PID_GAMEPAD_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x85, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x20, 0x15, 0x00,
    0x25, 0x01, 0x75, 0x01, 0x95, 0x20, 0x81, 0x02, 0x05, 0x01, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07,
    0x35, 0x00, 0x46, 0x3B, 0x01, 0x65, 0x14, 0x75, 0x04, 0x95, 0x01, 0x81, 0x42, 0x65, 0x00, 0x75,
    0x04, 0x95, 0x01, 0x81, 0x03, 0x09, 0x30, 0x09, 0x31, 0x09, 0x33, 0x09, 0x34, 0x16, 0x00, 0x80,
    0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x04, 0x81, 0x02, 0x09, 0x32, 0x09, 0x35, 0x15, 0x00, 0x26,
    0xFF, 0x00, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02, 0x85, 0x02, 0x06, 0x00, 0xFF, 0x09, 0x01, 0x15,
    0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x05, 0x91, 0x02, 0xC0,
    0x05, 0x0F, 0x09, 0x01, 0xA1, 0x01,
    0x09, 0x92, 0xA1, 0x02, 0x85, 0x03, 0x09, 0x9F, 0x09, 0xA0, 0x09, 0xA4, 0x09, 0xA5, 0x09, 0xA6,
    0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x05, 0x81, 0x02, 0x75, 0x03, 0x95, 0x01, 0x81, 0x03,
    0x09, 0x94, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x81, 0x02, 0x09, 0x22, 0x15, 0x00,
    0x25, 0x10, 0x75, 0x07, 0x95, 0x01, 0x81, 0x02, 0xC0,
    0x09, 0x21, 0xA1, 0x02, 0x85, 0x11, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x25, 0xA1, 0x02, 0x09, 0x26, 0x09, 0x27, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32,
    0x09, 0x33, 0x09, 0x34, 0x09, 0x40, 0x09, 0x41, 0x09, 0x42, 0x09, 0x43, 0x09, 0x28, 0x15, 0x01,
    0x25, 0x0C, 0x75, 0x08, 0x95, 0x01, 0x91, 0x00, 0xC0, 0x09, 0x50, 0x09, 0x54, 0x09, 0x51, 0x15,
    0x00, 0x26, 0xFF, 0x7F, 0x66, 0x03, 0x10, 0x55, 0xFD, 0x75, 0x10, 0x95, 0x03, 0x91, 0x02, 0x55,
    0x00, 0x65, 0x00, 0x09, 0x52, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
    0x09, 0x53, 0x15, 0x00, 0x25, 0x20, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02, 0x09, 0x55, 0xA1, 0x02,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x91, 0x02,
    0xC0, 0x05, 0x0F, 0x09, 0x56, 0x75, 0x01, 0x95, 0x01, 0x91, 0x02, 0x75, 0x05, 0x95, 0x01, 0x91,
    0x03, 0x09, 0x57, 0xA1, 0x02, 0x0B, 0x01, 0x00, 0x0A, 0x00, 0x0B, 0x02, 0x00, 0x0A, 0x00, 0x66,
    0x14, 0x00, 0x55, 0xFE, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x35, 0x00, 0x47, 0xA0, 0x8C, 0x00, 0x00,
    0x75, 0x08, 0x95, 0x02, 0x91, 0x02, 0x55, 0x00, 0x65, 0x00, 0x45, 0x00, 0xC0, 0x09, 0xA7, 0x15,
    0x00, 0x26, 0xFF, 0x7F, 0x66, 0x03, 0x10, 0x55, 0xFD, 0x75, 0x10, 0x95, 0x01, 0x91, 0x02, 0x55,
    0x00, 0x65, 0x00, 0xC0,
    0x09, 0x5A, 0xA1, 0x02, 0x85, 0x12, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x5B, 0x09, 0x5D, 0x15, 0x00, 0x26, 0x10, 0x27, 0x75, 0x10, 0x95, 0x02, 0x91,
    0x02, 0x09, 0x5C, 0x09, 0x5E, 0x26, 0xFF, 0x7F, 0x66, 0x03, 0x10, 0x55, 0xFD, 0x75, 0x10, 0x95,
    0x02, 0x91, 0x02, 0x55, 0x00, 0x65, 0x00, 0xC0,
    0x09, 0x5F, 0xA1, 0x02, 0x85, 0x13, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x23, 0x15, 0x00, 0x25, 0x0F, 0x75, 0x04, 0x95, 0x01, 0x91, 0x02, 0x75, 0x04,
    0x95, 0x01, 0x91, 0x03, 0x09, 0x60, 0x09, 0x61, 0x09, 0x62, 0x16, 0xF0, 0xD8, 0x26, 0x10, 0x27,
    0x75, 0x10, 0x95, 0x03, 0x91, 0x02, 0x09, 0x63, 0x09, 0x64, 0x09, 0x65, 0x15, 0x00, 0x26, 0x10,
    0x27, 0x75, 0x10, 0x95, 0x03, 0x91, 0x02, 0xC0,
    0x09, 0x6E, 0xA1, 0x02, 0x85, 0x14, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x70, 0x15, 0x00, 0x26, 0x10, 0x27, 0x75, 0x10, 0x95, 0x01, 0x91, 0x02, 0x09,
    0x6F, 0x16, 0xF0, 0xD8, 0x26, 0x10, 0x27, 0x75, 0x10, 0x95, 0x01, 0x91, 0x02, 0x09, 0x71, 0x15,
    0x00, 0x27, 0x9F, 0x8C, 0x00, 0x00, 0x66, 0x14, 0x00, 0x55, 0xFE, 0x75, 0x10, 0x95, 0x01, 0x91,
    0x02, 0x55, 0x00, 0x65, 0x00, 0x09, 0x72, 0x15, 0x00, 0x26, 0xFF, 0x7F, 0x66, 0x03, 0x10, 0x55,
    0xFD, 0x75, 0x10, 0x95, 0x01, 0x91, 0x02, 0x55, 0x00, 0x65, 0x00, 0xC0,
    0x09, 0x73, 0xA1, 0x02, 0x85, 0x15, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x70, 0x16, 0xF0, 0xD8, 0x26, 0x10, 0x27, 0x75, 0x10, 0x95, 0x01, 0x91, 0x02,
    0xC0,
    0x09, 0x74, 0xA1, 0x02, 0x85, 0x16, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x75, 0x09, 0x76, 0x16, 0xF0, 0xD8, 0x26, 0x10, 0x27, 0x75, 0x10, 0x95, 0x02,
    0x91, 0x02, 0xC0,
    0x09, 0x77, 0xA1, 0x02, 0x85, 0x17, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0x09, 0x78, 0xA1, 0x02, 0x09, 0x79, 0x09, 0x7A, 0x09, 0x7B, 0x15, 0x01, 0x25, 0x03,
    0x75, 0x08, 0x95, 0x01, 0x91, 0x00, 0xC0, 0x09, 0x7C, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08,
    0x95, 0x01, 0x91, 0x02, 0xC0,
    0x09, 0x90, 0xA1, 0x02, 0x85, 0x18, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08, 0x95, 0x01,
    0x91, 0x02, 0xC0,
    0x09, 0x95, 0xA1, 0x02, 0x85, 0x19, 0x09, 0x96, 0xA1, 0x02, 0x09, 0x97, 0x09, 0x98, 0x09, 0x99,
    0x09, 0x9A, 0x09, 0x9B, 0x09, 0x9C, 0x15, 0x01, 0x25, 0x06, 0x75, 0x08, 0x95, 0x01, 0x91, 0x00,
    0xC0, 0xC0,
    0x09, 0x7D, 0xA1, 0x02, 0x85, 0x1A, 0x09, 0x7E, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95,
    0x01, 0x91, 0x02, 0xC0,
    0x09, 0xAB, 0xA1, 0x02, 0x85, 0x1B, 0x09, 0x25, 0xA1, 0x02, 0x09, 0x26, 0x09, 0x27, 0x09, 0x30,
    0x09, 0x31, 0x09, 0x32, 0x09, 0x33, 0x09, 0x34, 0x09, 0x40, 0x09, 0x41, 0x09, 0x42, 0x09, 0x43,
    0x09, 0x28, 0x15, 0x01, 0x25, 0x0C, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x00, 0xC0, 0x05, 0x01, 0x09,
    0x3B, 0x15, 0x00, 0x26, 0xFF, 0x01, 0x75, 0x10, 0x95, 0x01, 0xB1, 0x02, 0xC0,
    0x05, 0x0F, 0x09, 0x89, 0xA1, 0x02, 0x85, 0x1C, 0x09, 0x22, 0x15, 0x01, 0x25, 0x10, 0x75, 0x08,
    0x95, 0x01, 0xB1, 0x02, 0x09, 0x8B, 0xA1, 0x02, 0x09, 0x8C, 0x09, 0x8D, 0x09, 0x8E, 0x15, 0x01,
    0x25, 0x03, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x00, 0xC0, 0x09, 0xAC, 0x15, 0x00, 0x27, 0xFF, 0xFF,
    0x00, 0x00, 0x75, 0x10, 0x95, 0x01, 0xB1, 0x02, 0xC0,
    0x09, 0x7F, 0xA1, 0x02, 0x85, 0x1D, 0x09, 0x80, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0x75,
    0x10, 0x95, 0x01, 0xB1, 0x02, 0x09, 0x83, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01,
    0xB1, 0x02, 0x09, 0xA9, 0x09, 0xAA, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0xB1, 0x02,
    0x75, 0x06, 0x95, 0x01, 0xB1, 0x03, 0xC0,
    0xC0,
];

#[derive(Clone, Debug)]
pub struct PidEngine {
    effects: [PidEffect; PID_MAX_EFFECTS],
    last_allocated: u8,
    last_load_status: u8,
    device_gain: u8,
    actuators_enabled: bool,
    paused: bool,
}

impl Default for PidEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PidEngine {
    pub const fn new() -> Self {
        const EMPTY: PidEffect = PidEffect {
            allocated: false,
            playing: false,
            effect_type: PidEffectType::None,
            duration_ms: 0,
            start_delay_ms: 0,
            gain: 0,
            loop_count: 0,
            constant_magnitude: 0,
            periodic_magnitude: 0,
            ramp_start: 0,
            ramp_end: 0,
            envelope_attack_level: 0,
            envelope_fade_level: 0,
            envelope_attack_ms: 0,
            envelope_fade_ms: 0,
            elapsed_ms: 0,
        };
        Self {
            effects: [EMPTY; PID_MAX_EFFECTS],
            last_allocated: 0,
            last_load_status: PidBlockLoadStatus::Success as u8,
            device_gain: 255,
            actuators_enabled: true,
            paused: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn effect_mut(&mut self, block_index: u8) -> Option<&mut PidEffect> {
        if block_index == 0 || usize::from(block_index) > PID_MAX_EFFECTS {
            None
        } else {
            Some(&mut self.effects[usize::from(block_index - 1)])
        }
    }

    fn stop_all(&mut self) {
        for slot in &mut self.effects {
            slot.playing = false;
            slot.elapsed_ms = 0;
        }
    }

    pub fn create_new_effect(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 4 || buf[0] != PID_CREATE_NEW_EFFECT_REPORT_ID {
            self.last_load_status = PidBlockLoadStatus::Error as u8;
            return false;
        }
        let effect_type = buf[1];
        let Some(parsed_type) = PidEffectType::from_raw(effect_type) else {
            self.last_load_status = PidBlockLoadStatus::Error as u8;
            return false;
        };
        if parsed_type == PidEffectType::None {
            self.last_load_status = PidBlockLoadStatus::Error as u8;
            return false;
        }
        for (idx, slot) in self.effects.iter_mut().enumerate() {
            if !slot.allocated {
                *slot = PidEffect {
                    allocated: true,
                    effect_type: parsed_type,
                    gain: 255,
                    ..Default::default()
                };
                self.last_allocated = (idx + 1) as u8;
                self.last_load_status = PidBlockLoadStatus::Success as u8;
                return true;
            }
        }
        self.last_load_status = PidBlockLoadStatus::Full as u8;
        false
    }

    pub fn block_load(&self) -> [u8; 5] {
        let used: u16 = self
            .effects
            .iter()
            .filter(|e| e.allocated)
            .map(|_| PID_BYTES_PER_EFFECT)
            .sum();
        let avail = PID_RAM_POOL_SIZE - used;
        let mut out = [0u8; 5];
        out[0] = PID_BLOCK_LOAD_REPORT_ID;
        out[1] = self.last_allocated;
        out[2] = self.last_load_status;
        out[3..5].copy_from_slice(&avail.to_le_bytes());
        out
    }

    pub fn pool(&self) -> [u8; 5] {
        let mut out = [0u8; 5];
        out[0] = PID_POOL_REPORT_ID;
        out[1..3].copy_from_slice(&PID_RAM_POOL_SIZE.to_le_bytes());
        out[3] = PID_MAX_EFFECTS as u8;
        out[4] = 0x01;
        out
    }

    pub fn state_report(&self) -> [u8; 3] {
        let mut out = [0u8; 3];
        out[0] = PID_STATE_REPORT_ID;
        out[1] = (if self.paused { 0x01 } else { 0 })
            | (if self.actuators_enabled { 0x02 } else { 0 })
            | 0x10;
        for (idx, slot) in self.effects.iter().enumerate() {
            if slot.playing {
                out[2] = 0x01 | (((idx as u8) + 1) << 1);
                break;
            }
        }
        out
    }

    pub fn set_effect(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 16 || buf[0] != PID_SET_EFFECT_REPORT_ID {
            return false;
        }
        let Some(slot) = self.effect_mut(buf[1]) else {
            return false;
        };
        if !slot.allocated {
            return false;
        }
        if let Some(et) = PidEffectType::from_raw(buf[2])
            && et != PidEffectType::None
        {
            slot.effect_type = et;
        }
        slot.duration_ms = u16::from_le_bytes([buf[3], buf[4]]);
        slot.gain = buf[9];
        slot.start_delay_ms = u16::from_le_bytes([buf[14], buf[15]]);
        true
    }

    pub fn set_envelope(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 10 || buf[0] != PID_SET_ENVELOPE_REPORT_ID {
            return false;
        }
        let Some(slot) = self.effect_mut(buf[1]) else {
            return false;
        };
        if !slot.allocated {
            return false;
        }
        slot.envelope_attack_level = u16::from_le_bytes([buf[2], buf[3]]);
        slot.envelope_fade_level = u16::from_le_bytes([buf[4], buf[5]]);
        slot.envelope_attack_ms = u16::from_le_bytes([buf[6], buf[7]]);
        slot.envelope_fade_ms = u16::from_le_bytes([buf[8], buf[9]]);
        true
    }

    pub fn set_condition(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 15 || buf[0] != PID_SET_CONDITION_REPORT_ID {
            return false;
        }
        self.effect_mut(buf[1]).is_some_and(|s| s.allocated)
    }

    pub fn set_periodic(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 10 || buf[0] != PID_SET_PERIODIC_REPORT_ID {
            return false;
        }
        let Some(slot) = self.effect_mut(buf[1]) else {
            return false;
        };
        if !slot.allocated {
            return false;
        }
        slot.periodic_magnitude = u16::from_le_bytes([buf[2], buf[3]]);
        true
    }

    pub fn set_constant_force(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 4 || buf[0] != PID_SET_CONSTANT_FORCE_REPORT_ID {
            return false;
        }
        let Some(slot) = self.effect_mut(buf[1]) else {
            return false;
        };
        if !slot.allocated {
            return false;
        }
        slot.constant_magnitude = i16::from_le_bytes([buf[2], buf[3]]);
        true
    }

    pub fn set_ramp_force(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 6 || buf[0] != PID_SET_RAMP_FORCE_REPORT_ID {
            return false;
        }
        let Some(slot) = self.effect_mut(buf[1]) else {
            return false;
        };
        if !slot.allocated {
            return false;
        }
        slot.ramp_start = i16::from_le_bytes([buf[2], buf[3]]);
        slot.ramp_end = i16::from_le_bytes([buf[4], buf[5]]);
        true
    }

    pub fn effect_operation(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 4 || buf[0] != PID_EFFECT_OPERATION_REPORT_ID {
            return false;
        }
        let Some(op) = PidEffectOperation::from_raw(buf[2]) else {
            return false;
        };
        let block = buf[1];
        if block == 0
            || usize::from(block) > PID_MAX_EFFECTS
            || !self.effects[usize::from(block - 1)].allocated
        {
            return false;
        }
        match op {
            PidEffectOperation::StartSolo => {
                self.stop_all();
                let slot = &mut self.effects[usize::from(block - 1)];
                slot.playing = true;
                slot.elapsed_ms = 0;
                slot.loop_count = buf[3];
                true
            }
            PidEffectOperation::Start => {
                let slot = &mut self.effects[usize::from(block - 1)];
                slot.playing = true;
                slot.elapsed_ms = 0;
                slot.loop_count = buf[3];
                true
            }
            PidEffectOperation::Stop => {
                let slot = &mut self.effects[usize::from(block - 1)];
                slot.playing = false;
                slot.elapsed_ms = 0;
                true
            }
        }
    }

    pub fn block_free(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 2 || buf[0] != PID_BLOCK_FREE_REPORT_ID {
            return false;
        }
        let Some(slot) = self.effect_mut(buf[1]) else {
            return false;
        };
        if !slot.allocated {
            return false;
        }
        *slot = PidEffect::default();
        true
    }

    pub fn device_control(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 2 || buf[0] != PID_DEVICE_CONTROL_REPORT_ID {
            return false;
        }
        match PidDeviceControl::from_raw(buf[1]) {
            Some(PidDeviceControl::EnableActuators) => self.actuators_enabled = true,
            Some(PidDeviceControl::DisableActuators) => self.actuators_enabled = false,
            Some(PidDeviceControl::StopAllEffects) => self.stop_all(),
            Some(PidDeviceControl::DeviceReset) => self.reset(),
            Some(PidDeviceControl::DevicePause) => self.paused = true,
            Some(PidDeviceControl::DeviceContinue) => self.paused = false,
            None => return false,
        }
        true
    }

    pub fn device_gain(&mut self, buf: &[u8]) -> bool {
        if buf.len() < 2 || buf[0] != PID_DEVICE_GAIN_REPORT_ID {
            return false;
        }
        self.device_gain = buf[1];
        true
    }

    pub fn advance(&mut self, elapsed_ms: u32) {
        if self.paused {
            return;
        }
        for slot in &mut self.effects {
            if !slot.allocated || !slot.playing {
                continue;
            }
            slot.elapsed_ms = slot.elapsed_ms.saturating_add(elapsed_ms);
            if slot.duration_ms == 0 {
                continue;
            }
            let total = u32::from(slot.start_delay_ms) + u32::from(slot.duration_ms);
            if slot.elapsed_ms < total {
                continue;
            }
            if slot.loop_count == 0xFF {
                slot.elapsed_ms = u32::from(slot.start_delay_ms);
                continue;
            }
            if slot.loop_count > 1 {
                slot.loop_count -= 1;
                slot.elapsed_ms = u32::from(slot.start_delay_ms);
                continue;
            }
            slot.playing = false;
            slot.elapsed_ms = 0;
        }
    }

    pub fn needs_tick(&self) -> bool {
        self.effects.iter().any(|e| e.allocated && e.playing)
    }

    pub fn rumble(&self) -> PidRumble {
        if !self.actuators_enabled || self.paused {
            return PidRumble::default();
        }
        let mut low = 0i32;
        let mut high = 0i32;

        for slot in &self.effects {
            if !slot.allocated || !slot.playing || slot.elapsed_ms < u32::from(slot.start_delay_ms)
            {
                continue;
            }
            let active_ms = slot.elapsed_ms - u32::from(slot.start_delay_ms);
            let mut periodic = false;
            let mut magnitude = match slot.effect_type {
                PidEffectType::ConstantForce => i32::from(slot.constant_magnitude).abs(),
                PidEffectType::Ramp => {
                    let mut val = i32::from(slot.ramp_start);
                    if slot.duration_ms != 0 {
                        let span = u32::from(slot.duration_ms);
                        let pos = active_ms.min(span);
                        let delta = i64::from(slot.ramp_end) - i64::from(slot.ramp_start);
                        val = i32::from(slot.ramp_start)
                            + ((delta * i64::from(pos)) / i64::from(span)) as i32;
                    }
                    val.abs()
                }
                PidEffectType::Square
                | PidEffectType::Sine
                | PidEffectType::Triangle
                | PidEffectType::SawtoothUp
                | PidEffectType::SawtoothDown => {
                    periodic = true;
                    i32::from(slot.periodic_magnitude)
                }
                _ => continue,
            };

            magnitude = apply_envelope(slot, active_ms, magnitude);
            magnitude = (magnitude * i32::from(slot.gain)) / 255;
            magnitude = (magnitude * i32::from(self.device_gain)) / 255;
            magnitude = magnitude.clamp(0, PID_NOMINAL_MAX);

            if periodic {
                high = high.max(magnitude);
            } else {
                low = low.max(magnitude);
            }
        }

        PidRumble {
            low_frequency: to_rumble(low),
            high_frequency: to_rumble(high),
        }
    }
}

fn to_rumble(magnitude: i32) -> u16 {
    ((magnitude.clamp(0, PID_NOMINAL_MAX) * 65535) / PID_NOMINAL_MAX) as u16
}

fn apply_envelope(effect: &PidEffect, active_ms: u32, magnitude: i32) -> i32 {
    let mut result = magnitude;
    if effect.envelope_attack_ms != 0 && active_ms < u32::from(effect.envelope_attack_ms) {
        let start = i32::from(effect.envelope_attack_level);
        let span = i64::from(magnitude) - i64::from(start);
        result =
            start + ((span * i64::from(active_ms)) / i64::from(effect.envelope_attack_ms)) as i32;
    }
    if effect.duration_ms != 0 && effect.envelope_fade_ms != 0 {
        let fade_start = u32::from(effect.duration_ms.saturating_sub(effect.envelope_fade_ms));
        if active_ms >= fade_start {
            let into_fade = active_ms - fade_start;
            let fade_span = u32::from(effect.duration_ms) - fade_start;
            if fade_span != 0 {
                let end = i32::from(effect.envelope_fade_level);
                let span = i64::from(end) - i64::from(magnitude);
                let faded =
                    magnitude + ((span * i64::from(into_fade)) / i64::from(fade_span)) as i32;
                result = result.min(faded);
            }
        }
    }
    result.clamp(0, PID_NOMINAL_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_engine_constant_periodic_pool_gain_and_expiry() {
        let mut engine = PidEngine::new();
        assert!(engine.create_new_effect(&[PID_CREATE_NEW_EFFECT_REPORT_ID, 1, 0, 0]));
        assert_eq!(engine.block_load()[1], 1);
        assert_eq!(engine.block_load()[2], PidBlockLoadStatus::Success as u8);

        let mut se = [0u8; 16];
        se[0] = PID_SET_EFFECT_REPORT_ID;
        se[1] = 1;
        se[2] = PidEffectType::ConstantForce as u8;
        se[3..5].copy_from_slice(&100u16.to_le_bytes());
        se[9] = 255;
        assert!(engine.set_effect(&se));

        let mut cf = [0u8; 4];
        cf[0] = PID_SET_CONSTANT_FORCE_REPORT_ID;
        cf[1] = 1;
        cf[2..4].copy_from_slice(&(PID_NOMINAL_MAX as i16).to_le_bytes());
        assert!(engine.set_constant_force(&cf));

        assert!(engine.effect_operation(&[PID_EFFECT_OPERATION_REPORT_ID, 1, 1, 1]));
        assert_eq!(engine.rumble().low_frequency, 65535);
        assert!(engine.needs_tick());

        engine.advance(150);
        assert!(!engine.needs_tick());
        assert_eq!(engine.rumble().low_frequency, 0);

        // Fill remaining 15 slots and confirm overflow reports Full.
        for _ in 1..PID_MAX_EFFECTS {
            assert!(engine.create_new_effect(&[PID_CREATE_NEW_EFFECT_REPORT_ID, 1, 0, 0]));
        }
        assert!(!engine.create_new_effect(&[PID_CREATE_NEW_EFFECT_REPORT_ID, 1, 0, 0]));
        assert_eq!(engine.block_load()[2], PidBlockLoadStatus::Full as u8);
        assert!(engine.block_free(&[PID_BLOCK_FREE_REPORT_ID, 1]));
        assert!(engine.create_new_effect(&[PID_CREATE_NEW_EFFECT_REPORT_ID, 4, 0, 0]));
    }
}
