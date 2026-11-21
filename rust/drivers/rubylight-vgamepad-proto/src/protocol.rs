// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/protocol.h.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Fixed-size `METHOD_BUFFERED` IOCTL ABI shared between the Rubylight host and
//! the `rubylight_vgamepad` UMDF2 VHF source driver.

/// Wire protocol version. Version 2 widened the feedback event payload to
/// accommodate 23-byte DualSense adaptive trigger programs and SC2026 reports.
pub const PROTOCOL_VERSION: u16 = 2;

/// Maximum simultaneously owned controller slots on the virtual gamepad device.
pub const MAX_CONTROLLERS: u32 = 16;

/// Private control device interface published by `rubylight_vgamepad.dll`.
/// Distinct from Vibeshine's interface so side-by-side installs never collide.
pub const DEVICE_INTERFACE_GUID: u128 = 0xb8a3e710_4f2d_4c91_9a18_6e2f5b841a01;

/// Base GUID for each virtual HID child's ContainerID (`Data4[7]` = controller index).
pub const CONTROLLER_CONTAINER_BASE: u128 = 0x9a2f1c74_6b83_4d51_a70e_351dc6840000;

/// Root-enumerated PnP hardware ID installed by `rubylight_vgamepad.inf`.
pub const ROOT_HARDWARE_ID: &str = "Root\\RubylightVirtualGamepad";

/// Prefix for per-slot child InstanceIDs (`RubylightGamepad<n>`).
pub const INSTANCE_ID_PREFIX: &str = "RubylightGamepad";

/// Maximum Steam Settings `(id, lo, hi)` triples carried in one feedback event.
pub const STEAM_SETTINGS_TRIPLES_MAX: usize = 9;

/// Sentinel length returned by [`feedback_payload_size`] for unknown types.
pub const FEEDBACK_PAYLOAD_UNKNOWN: u16 = 0xFFFF;

/// Device-wide HID capability bits advertised in [`QueryInfoResponse`].
pub mod feature_mask {
    pub const INPUT_STATE: u32 = 1 << 0;
    pub const FEEDBACK: u32 = 1 << 1;
    pub const TOUCH: u32 = 1 << 2;
    pub const MOTION: u32 = 1 << 3;
    pub const BATTERY: u32 = 1 << 4;
    pub const HID_FEATURE_REPORTS: u32 = 1 << 5;
    pub const ALL: u32 = INPUT_STATE | FEEDBACK | TOUCH | MOTION | BATTERY | HID_FEATURE_REPORTS;
}

/// Normalized controller button bitmask shared across host and driver.
pub mod button_mask {
    pub const DPAD_UP: u32 = 0x0000_0001;
    pub const DPAD_DOWN: u32 = 0x0000_0002;
    pub const DPAD_LEFT: u32 = 0x0000_0004;
    pub const DPAD_RIGHT: u32 = 0x0000_0008;
    pub const START: u32 = 0x0000_0010;
    pub const BACK: u32 = 0x0000_0020;
    pub const LEFT_STICK: u32 = 0x0000_0040;
    pub const RIGHT_STICK: u32 = 0x0000_0080;
    pub const LEFT_SHOULDER: u32 = 0x0000_0100;
    pub const RIGHT_SHOULDER: u32 = 0x0000_0200;
    pub const HOME: u32 = 0x0000_0400;
    pub const SOUTH: u32 = 0x0000_1000;
    pub const EAST: u32 = 0x0000_2000;
    pub const WEST: u32 = 0x0000_4000;
    pub const NORTH: u32 = 0x0000_8000;
    pub const PADDLE_1: u32 = 0x0001_0000;
    pub const PADDLE_2: u32 = 0x0002_0000;
    pub const PADDLE_3: u32 = 0x0004_0000;
    pub const PADDLE_4: u32 = 0x0008_0000;
    pub const TOUCHPAD: u32 = 0x0010_0000;
    pub const MISC: u32 = 0x0020_0000;
    pub const LEFT_GRIP_TOUCH: u32 = 0x0040_0000;
    pub const RIGHT_GRIP_TOUCH: u32 = 0x0080_0000;
    pub const LEFT_STICK_TOUCH: u32 = 0x0100_0000;
    pub const RIGHT_STICK_TOUCH: u32 = 0x0200_0000;
    pub const LEFT_PAD_CLICK: u32 = 0x0400_0000;
    pub const RIGHT_PAD_CLICK: u32 = 0x0800_0000;
    pub const LEFT_TRIGGER_CLICK: u32 = 0x1000_0000;
    pub const RIGHT_TRIGGER_CLICK: u32 = 0x2000_0000;
    /// Host-owned metadata marking explicit SC2026 pad/trigger clicks and touch states.
    pub const SC26_BUTTONS_EXPLICIT: u32 = 0x4000_0000;
}

/// PlayStation output validity flags in [`PlayStationOutputFeedback::valid`].
pub mod ps_output_valid {
    pub const LIGHTBAR: u8 = 0x01;
    pub const TRIGGERS: u8 = 0x02;
    pub const PLAYER_LEDS: u8 = 0x04;
    pub const MICROPHONE_LED: u8 = 0x08;
}

/// Steam Controller attribute update bitmask (`ioctl_set_steam_controller_attributes`).
pub mod sc_attr_flags {
    pub const FIRMWARE_BUILD_TIME: u32 = 0x01;
    pub const BOOTLOADER_BUILD_TIME: u32 = 0x02;
    pub const BOARD_REVISION: u32 = 0x04;
    pub const CAPABILITIES: u32 = 0x08;
    pub const BUILD_ID: u32 = 0x40;
    pub const SERIALS: u32 = 0x80;
}

/// Requested virtual gamepad profile ID (`u16` wire discriminant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Profile {
    GenericHid = 1,
    Xbox360 = 2,
    XboxOne = 3,
    XboxSeries = 4,
    DualShock4 = 5,
    DualSense = 6,
    SwitchPro = 7,
    GenericPid = 8,
    SteamController = 9,
}

impl Profile {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::GenericHid),
            2 => Some(Self::Xbox360),
            3 => Some(Self::XboxOne),
            4 => Some(Self::XboxSeries),
            5 => Some(Self::DualShock4),
            6 => Some(Self::DualSense),
            7 => Some(Self::SwitchPro),
            8 => Some(Self::GenericPid),
            9 => Some(Self::SteamController),
            _ => None,
        }
    }

    pub const fn bit(self) -> u32 {
        profile_bit(self as u16)
    }
}

/// Converts a 1-based profile ID into its mask bit in `QueryInfoResponse::available_profiles`.
pub const fn profile_bit(profile_id: u16) -> u32 {
    if profile_id == 0 || profile_id > 32 {
        0
    } else {
        1u32 << (profile_id - 1)
    }
}

/// Discriminant for [`FeedbackEvent::feedback_type`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum FeedbackType {
    None = 0,
    GenericRumbleRgb = 1,
    RawOutputReport = 2,
    GenericRumble = 3,
    XboxRumble = 4,
    PlayStationOutput = 5,
    SteamHaptic = 6,
    SteamSettings = 7,
}

impl FeedbackType {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::None),
            1 => Some(Self::GenericRumbleRgb),
            2 => Some(Self::RawOutputReport),
            3 => Some(Self::GenericRumble),
            4 => Some(Self::XboxRumble),
            5 => Some(Self::PlayStationOutput),
            6 => Some(Self::SteamHaptic),
            7 => Some(Self::SteamSettings),
            _ => None,
        }
    }
}

/// Expected payload byte count for a [`FeedbackType`].
pub const fn feedback_payload_size(feedback_type: FeedbackType) -> u16 {
    match feedback_type {
        FeedbackType::None => 0,
        FeedbackType::GenericRumbleRgb | FeedbackType::GenericRumble => {
            GenericRumbleRgbFeedback::WIRE_SIZE as u16
        }
        FeedbackType::RawOutputReport => 16,
        FeedbackType::XboxRumble => XboxRumbleFeedback::WIRE_SIZE as u16,
        FeedbackType::PlayStationOutput => PlayStationOutputFeedback::WIRE_SIZE as u16,
        FeedbackType::SteamHaptic => SteamHapticFeedback::WIRE_SIZE as u16,
        FeedbackType::SteamSettings => SteamSettingsFeedback::WIRE_SIZE as u16,
    }
}

/// DualSense adaptive trigger effect mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TriggerEffectMode {
    Off = 0x00,
    Feedback = 0x01,
    Weapon = 0x02,
    Vibration = 0x06,
}

/// Normalized touch lifecycle event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchEvent {
    Hover = 0,
    Down = 1,
    Up = 2,
    Move = 3,
    Cancel = 4,
    CancelAll = 5,
}

impl TouchEvent {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Hover),
            1 => Some(Self::Down),
            2 => Some(Self::Up),
            3 => Some(Self::Move),
            4 => Some(Self::Cancel),
            5 => Some(Self::CancelAll),
            _ => None,
        }
    }
}

/// Motion sensor kind (`Touch/Motion` IOCTLs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MotionKind {
    Accelerometer = 1,
    Gyroscope = 2,
}

impl MotionKind {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Accelerometer),
            2 => Some(Self::Gyroscope),
            _ => None,
        }
    }
}

/// Normalized battery charging state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BatteryState {
    Unknown = 0,
    NotPresent = 1,
    Discharging = 2,
    Charging = 3,
    Full = 4,
    NotCharging = 5,
}

impl BatteryState {
    pub const fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::NotPresent,
            2 => Self::Discharging,
            3 => Self::Charging,
            4 => Self::Full,
            5 => Self::NotCharging,
            _ => Self::Unknown,
        }
    }
}

/// Saturates a `u32` counter to `u8` for wire diagnostics.
pub const fn saturate_counter(value: u32) -> u8 {
    if value > 0xFF { 0xFF } else { value as u8 }
}

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const FILE_DEVICE_UNKNOWN: u32 = 0x0022;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_WRITE_ACCESS: u32 = 0x0003;

pub const IOCTL_QUERY_INFO: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x800,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_CREATE_CONTROLLER: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x801,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_DESTROY_CONTROLLER: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x802,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_INPUT_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x803,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_POLL_FEEDBACK: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x804,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_TOUCH_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x805,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_MOTION_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x806,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_BATTERY_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x807,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x808,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);

/// Common 8-byte header beginning every control request and response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RequestHeader {
    pub size: u32,
    pub version: u16,
    pub reserved: u16,
}

impl RequestHeader {
    pub const WIRE_SIZE: usize = 8;

    pub const fn new(size: u32) -> Self {
        Self {
            size,
            version: PROTOCOL_VERSION,
            reserved: 0,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            size: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            version: u16::from_le_bytes([bytes[4], bytes[5]]),
            reserved: u16::from_le_bytes([bytes[6], bytes[7]]),
        })
    }

    pub fn write_to(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.size.to_le_bytes());
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.reserved.to_le_bytes());
    }

    pub const fn is_valid(&self, expected_size: usize) -> bool {
        self.size as usize == expected_size
            && self.version == PROTOCOL_VERSION
            && self.reserved == 0
    }
}

/// 16-byte controller creation request (`IOCTL_CREATE_CONTROLLER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CreateControllerRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub requested_profile: u16,
    pub reserved: u16,
}

impl CreateControllerRequest {
    pub const WIRE_SIZE: usize = 16;

    pub fn new(controller_id: u32, requested_profile: u16) -> Self {
        Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
            controller_id,
            requested_profile,
            reserved: 0,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            requested_profile: u16::from_le_bytes([bytes[12], bytes[13]]),
            reserved: u16::from_le_bytes([bytes[14], bytes[15]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.requested_profile.to_le_bytes());
        out[14..16].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 8-byte device-wide info query (`IOCTL_QUERY_INFO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct QueryInfoRequest {
    pub header: RequestHeader,
}

impl QueryInfoRequest {
    pub const WIRE_SIZE: usize = 8;

    pub fn new() -> Self {
        Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        header.is_valid(Self::WIRE_SIZE).then_some(Self { header })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out);
        out
    }
}

/// Saturated per-slot discard/unknown counters returned in [`QueryInfoResponse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FeedbackCounters {
    pub haptic_evicted: u8,
    pub settings_evicted: u8,
    pub transitions_dropped: u8,
    pub unknown_commands: u8,
}

impl FeedbackCounters {
    pub const WIRE_SIZE: usize = 4;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            haptic_evicted: bytes[0],
            settings_evicted: bytes[1],
            transitions_dropped: bytes[2],
            unknown_commands: bytes[3],
        })
    }

    pub fn write_to(&self, out: &mut [u8]) {
        out[0] = self.haptic_evicted;
        out[1] = self.settings_evicted;
        out[2] = self.transitions_dropped;
        out[3] = self.unknown_commands;
    }
}

/// 28-byte capability and counters response (`IOCTL_QUERY_INFO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct QueryInfoResponse {
    pub header: RequestHeader,
    pub minimum_protocol_version: u16,
    pub maximum_protocol_version: u16,
    pub available_profiles: u32,
    pub available_features: u32,
    pub maximum_controllers: u32,
    pub counters: FeedbackCounters,
}

impl QueryInfoResponse {
    pub const WIRE_SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            header: RequestHeader::from_bytes(&bytes[0..8])?,
            minimum_protocol_version: u16::from_le_bytes([bytes[8], bytes[9]]),
            maximum_protocol_version: u16::from_le_bytes([bytes[10], bytes[11]]),
            available_profiles: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            available_features: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            maximum_controllers: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            counters: FeedbackCounters::from_bytes(&bytes[24..28])?,
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..10].copy_from_slice(&self.minimum_protocol_version.to_le_bytes());
        out[10..12].copy_from_slice(&self.maximum_protocol_version.to_le_bytes());
        out[12..16].copy_from_slice(&self.available_profiles.to_le_bytes());
        out[16..20].copy_from_slice(&self.available_features.to_le_bytes());
        out[20..24].copy_from_slice(&self.maximum_controllers.to_le_bytes());
        self.counters.write_to(&mut out[24..28]);
        out
    }
}

/// 12-byte controller-addressed request (`destroy`, `poll_feedback`, per-slot `query_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ControllerIdRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
}

impl ControllerIdRequest {
    pub const WIRE_SIZE: usize = 12;

    pub fn new(controller_id: u32) -> Self {
        Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
            controller_id,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out
    }
}

/// 28-byte controller input snapshot (`IOCTL_SUBMIT_INPUT_STATE`).
///
/// Axes use Rubylight/Moonlight normalized conventions: sticks are signed `i16`
/// with **positive = up and right**, and triggers are `0..=255`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InputStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub buttons: u32,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub reserved: u16,
}

impl InputStateRequest {
    pub const WIRE_SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            buttons: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            left_x: i16::from_le_bytes([bytes[16], bytes[17]]),
            left_y: i16::from_le_bytes([bytes[18], bytes[19]]),
            right_x: i16::from_le_bytes([bytes[20], bytes[21]]),
            right_y: i16::from_le_bytes([bytes[22], bytes[23]]),
            left_trigger: bytes[24],
            right_trigger: bytes[25],
            reserved: u16::from_le_bytes([bytes[26], bytes[27]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.buttons.to_le_bytes());
        out[16..18].copy_from_slice(&self.left_x.to_le_bytes());
        out[18..20].copy_from_slice(&self.left_y.to_le_bytes());
        out[20..22].copy_from_slice(&self.right_x.to_le_bytes());
        out[22..24].copy_from_slice(&self.right_y.to_le_bytes());
        out[24] = self.left_trigger;
        out[25] = self.right_trigger;
        out[26..28].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 22-byte normalized touch update (`IOCTL_SUBMIT_TOUCH_STATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TouchStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub contact_index: u8,
    pub event_type: u8,
    pub x: u16,
    pub y: u16,
    pub pressure: u16,
    pub reserved: u16,
}

impl TouchStateRequest {
    pub const WIRE_SIZE: usize = 22;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            contact_index: bytes[12],
            event_type: bytes[13],
            x: u16::from_le_bytes([bytes[14], bytes[15]]),
            y: u16::from_le_bytes([bytes[16], bytes[17]]),
            pressure: u16::from_le_bytes([bytes[18], bytes[19]]),
            reserved: u16::from_le_bytes([bytes[20], bytes[21]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12] = self.contact_index;
        out[13] = self.event_type;
        out[14..16].copy_from_slice(&self.x.to_le_bytes());
        out[16..18].copy_from_slice(&self.y.to_le_bytes());
        out[18..20].copy_from_slice(&self.pressure.to_le_bytes());
        out[20..22].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 28-byte motion sensor sample (`IOCTL_SUBMIT_MOTION_STATE`), in milli-m/s^2 or milli-deg/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MotionStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub motion_type: u8,
    pub reserved0: [u8; 3],
    pub x_milli: i32,
    pub y_milli: i32,
    pub z_milli: i32,
}

impl MotionStateRequest {
    pub const WIRE_SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            motion_type: bytes[12],
            reserved0: [bytes[13], bytes[14], bytes[15]],
            x_milli: i32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            y_milli: i32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            z_milli: i32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12] = self.motion_type;
        out[13..16].copy_from_slice(&self.reserved0);
        out[16..20].copy_from_slice(&self.x_milli.to_le_bytes());
        out[20..24].copy_from_slice(&self.y_milli.to_le_bytes());
        out[24..28].copy_from_slice(&self.z_milli.to_le_bytes());
        out
    }
}

/// 16-byte battery state update (`IOCTL_SUBMIT_BATTERY_STATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BatteryStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub percent: u8,
    pub flags: u8,
    pub reserved: u16,
}

impl BatteryStateRequest {
    pub const WIRE_SIZE: usize = 16;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            percent: bytes[12],
            flags: bytes[13],
            reserved: u16::from_le_bytes([bytes[14], bytes[15]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12] = self.percent;
        out[13] = self.flags;
        out[14..16].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 80-byte real-hardware Steam Controller firmware attribute update (`IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SteamControllerAttributesRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub flags: u32,
    pub firmware_build_time: u32,
    pub bootloader_build_time: u32,
    pub board_revision: u32,
    pub capabilities: u32,
    pub build_id: [u8; 16],
    pub unit_serial: [u8; 16],
    pub board_serial: [u8; 16],
}

impl SteamControllerAttributesRequest {
    pub const WIRE_SIZE: usize = 80;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        let mut out = Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            flags: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            firmware_build_time: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            bootloader_build_time: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            board_revision: u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            capabilities: u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            ..Default::default()
        };
        out.build_id.copy_from_slice(&bytes[32..48]);
        out.unit_serial.copy_from_slice(&bytes[48..64]);
        out.board_serial.copy_from_slice(&bytes[64..80]);
        Some(out)
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        out[16..20].copy_from_slice(&self.firmware_build_time.to_le_bytes());
        out[20..24].copy_from_slice(&self.bootloader_build_time.to_le_bytes());
        out[24..28].copy_from_slice(&self.board_revision.to_le_bytes());
        out[28..32].copy_from_slice(&self.capabilities.to_le_bytes());
        out[32..48].copy_from_slice(&self.build_id);
        out[48..64].copy_from_slice(&self.unit_serial);
        out[64..80].copy_from_slice(&self.board_serial);
        out
    }
}

/// 8-byte generic rumble/RGB feedback payload (`FeedbackType::GenericRumbleRgb` / `GenericRumble`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GenericRumbleRgbFeedback {
    pub low_frequency: u16,
    pub high_frequency: u16,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub reserved: u8,
}

impl GenericRumbleRgbFeedback {
    pub const WIRE_SIZE: usize = 8;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0..2].copy_from_slice(&self.low_frequency.to_le_bytes());
        out[2..4].copy_from_slice(&self.high_frequency.to_le_bytes());
        out[4] = self.red;
        out[5] = self.green;
        out[6] = self.blue;
        out[7] = self.reserved;
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            low_frequency: u16::from_le_bytes([bytes[0], bytes[1]]),
            high_frequency: u16::from_le_bytes([bytes[2], bytes[3]]),
            red: bytes[4],
            green: bytes[5],
            blue: bytes[6],
            reserved: bytes[7],
        })
    }
}

/// 8-byte four-motor Xbox rumble feedback (`FeedbackType::XboxRumble`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct XboxRumbleFeedback {
    pub low_frequency: u16,
    pub high_frequency: u16,
    pub left_trigger: u16,
    pub right_trigger: u16,
}

impl XboxRumbleFeedback {
    pub const WIRE_SIZE: usize = 8;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0..2].copy_from_slice(&self.low_frequency.to_le_bytes());
        out[2..4].copy_from_slice(&self.high_frequency.to_le_bytes());
        out[4..6].copy_from_slice(&self.left_trigger.to_le_bytes());
        out[6..8].copy_from_slice(&self.right_trigger.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            low_frequency: u16::from_le_bytes([bytes[0], bytes[1]]),
            high_frequency: u16::from_le_bytes([bytes[2], bytes[3]]),
            left_trigger: u16::from_le_bytes([bytes[4], bytes[5]]),
            right_trigger: u16::from_le_bytes([bytes[6], bytes[7]]),
        })
    }
}

/// 11-byte DualSense adaptive trigger program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TriggerEffectFeedback {
    pub mode: u8,
    pub parameters: [u8; 10],
}

impl TriggerEffectFeedback {
    pub const WIRE_SIZE: usize = 11;

    pub fn write_to(&self, out: &mut [u8]) {
        out[0] = self.mode;
        out[1..11].copy_from_slice(&self.parameters);
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut parameters = [0u8; 10];
        parameters.copy_from_slice(&bytes[1..11]);
        Some(Self {
            mode: bytes[0],
            parameters,
        })
    }
}

/// 32-byte PlayStation / Switch rumble, LED, and trigger-effect feedback (`FeedbackType::PlayStationOutput`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PlayStationOutputFeedback {
    pub low_frequency: u16,
    pub high_frequency: u16,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub valid: u8,
    pub player_leds: u8,
    pub microphone_led: u8,
    pub left_trigger: TriggerEffectFeedback,
    pub right_trigger: TriggerEffectFeedback,
}

impl PlayStationOutputFeedback {
    pub const WIRE_SIZE: usize = 32;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0..2].copy_from_slice(&self.low_frequency.to_le_bytes());
        out[2..4].copy_from_slice(&self.high_frequency.to_le_bytes());
        out[4] = self.red;
        out[5] = self.green;
        out[6] = self.blue;
        out[7] = self.valid;
        out[8] = self.player_leds;
        out[9] = self.microphone_led;
        self.left_trigger.write_to(&mut out[10..21]);
        self.right_trigger.write_to(&mut out[21..32]);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            low_frequency: u16::from_le_bytes([bytes[0], bytes[1]]),
            high_frequency: u16::from_le_bytes([bytes[2], bytes[3]]),
            red: bytes[4],
            green: bytes[5],
            blue: bytes[6],
            valid: bytes[7],
            player_leds: bytes[8],
            microphone_led: bytes[9],
            left_trigger: TriggerEffectFeedback::from_bytes(&bytes[10..21])?,
            right_trigger: TriggerEffectFeedback::from_bytes(&bytes[21..32])?,
        })
    }
}

/// 16-byte Steam Controller haptic output payload (`FeedbackType::SteamHaptic`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SteamHapticFeedback {
    pub length: u8,
    pub reserved: [u8; 3],
    pub report: [u8; 12],
}

impl SteamHapticFeedback {
    pub const WIRE_SIZE: usize = 16;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0] = self.length;
        out[1..4].copy_from_slice(&self.reserved);
        out[4..16].copy_from_slice(&self.report);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut report = [0u8; 12];
        report.copy_from_slice(&bytes[4..16]);
        Some(Self {
            length: bytes[0],
            reserved: [bytes[1], bytes[2], bytes[3]],
            report,
        })
    }
}

/// 32-byte Steam Controller settings command chunk (`FeedbackType::SteamSettings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SteamSettingsFeedback {
    pub command: u8,
    pub count: u8,
    pub reserved: [u8; 3],
    pub triples: [u8; STEAM_SETTINGS_TRIPLES_MAX * 3],
}

impl SteamSettingsFeedback {
    pub const WIRE_SIZE: usize = 32;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0] = self.command;
        out[1] = self.count;
        out[2..5].copy_from_slice(&self.reserved);
        out[5..32].copy_from_slice(&self.triples);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut triples = [0u8; 27];
        triples.copy_from_slice(&bytes[5..32]);
        Some(Self {
            command: bytes[0],
            count: bytes[1],
            reserved: [bytes[2], bytes[3], bytes[4]],
            triples,
        })
    }
}

/// 48-byte feedback event dequeued via `IOCTL_POLL_FEEDBACK`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FeedbackEvent {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub feedback_type: u16,
    pub payload_size: u16,
    pub payload: [u8; 32],
}

impl FeedbackEvent {
    pub const WIRE_SIZE: usize = 48;

    pub fn new(controller_id: u32, feedback_type: FeedbackType, payload: &[u8]) -> Self {
        let mut out = Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
            controller_id,
            feedback_type: feedback_type as u16,
            payload_size: payload.len() as u16,
            payload: [0; 32],
        };
        let copy_len = if payload.len() <= 32 {
            payload.len()
        } else {
            32
        };
        out.payload[..copy_len].copy_from_slice(&payload[..copy_len]);
        out
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.feedback_type.to_le_bytes());
        out[14..16].copy_from_slice(&self.payload_size.to_le_bytes());
        out[16..48].copy_from_slice(&self.payload);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut payload = [0u8; 32];
        payload.copy_from_slice(&bytes[16..48]);
        Some(Self {
            header: RequestHeader::from_bytes(&bytes[0..8])?,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            feedback_type: u16::from_le_bytes([bytes[12], bytes[13]]),
            payload_size: u16::from_le_bytes([bytes[14], bytes[15]]),
            payload,
        })
    }
}

/// Formats `RubylightGamepad<index>` as a null-terminated UTF-16 buffer for
/// `VHF_CONFIG.InstanceID` and returns the slice including its trailing NUL.
pub fn format_instance_id_utf16(index: u32, out: &mut [u16; 32]) -> usize {
    *out = [0u16; 32];
    let prefix = INSTANCE_ID_PREFIX.as_bytes();
    let mut pos = 0usize;
    for &b in prefix {
        out[pos] = u16::from(b);
        pos += 1;
    }
    if index >= 10 {
        out[pos] = u16::from(b'0' + ((index / 10) % 10) as u8);
        pos += 1;
    }
    out[pos] = u16::from(b'0' + (index % 10) as u8);
    pos += 1;
    // `pos + 1` includes the trailing UTF-16 NUL already in `out[pos]`.
    pos + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_codes_match_wire_contract() {
        assert_eq!(IOCTL_QUERY_INFO, 0x0022_E000);
        assert_eq!(IOCTL_CREATE_CONTROLLER, 0x0022_E004);
        assert_eq!(IOCTL_DESTROY_CONTROLLER, 0x0022_E008);
        assert_eq!(IOCTL_SUBMIT_INPUT_STATE, 0x0022_E00C);
        assert_eq!(IOCTL_POLL_FEEDBACK, 0x0022_E010);
        assert_eq!(IOCTL_SUBMIT_TOUCH_STATE, 0x0022_E014);
        assert_eq!(IOCTL_SUBMIT_MOTION_STATE, 0x0022_E018);
        assert_eq!(IOCTL_SUBMIT_BATTERY_STATE, 0x0022_E01C);
        assert_eq!(IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES, 0x0022_E020);
    }

    #[test]
    fn wire_sizes_and_counter_saturation() {
        assert_eq!(QueryInfoResponse::default().to_bytes().len(), 28);
        assert_eq!(FeedbackEvent::default().to_bytes().len(), 48);
        assert_eq!(
            SteamControllerAttributesRequest::default().to_bytes().len(),
            80
        );
        assert_eq!(feedback_payload_size(FeedbackType::PlayStationOutput), 32);
        assert_eq!(feedback_payload_size(FeedbackType::SteamSettings), 32);
        assert_eq!(feedback_payload_size(FeedbackType::SteamHaptic), 16);
        assert_eq!(saturate_counter(0), 0);
        assert_eq!(saturate_counter(255), 255);
        assert_eq!(saturate_counter(256), 255);
        assert_eq!(saturate_counter(u32::MAX), 255);
    }
}
