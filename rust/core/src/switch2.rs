//! A virtual Nintendo Switch 2 Pro Controller as the controller presents
//! itself over USB (057e:2069): its descriptors, its HID input reports
//! 0x05 and 0x09, the HD rumble output report and the command protocol of
//! its vendor bulk interface. Served over USB/IP ([`crate::usbip`]), it lets
//! SDL 3 and Steam on the host see a streaming Switch 2 Pro Controller as one,
//! with its C button, grip buttons, gyro and rumble.
//!
//! The descriptors, the bulk commands and their replies and the report
//! layouts are ported from libVIIPER's `device/ns2pro` (Alia5/VIIPER,
//! GPL-3.0, commit 20aa112), which Solstice (GPL-3.0) drives for the same
//! controller; the axis scales, the stick calibration read and the HD rumble
//! encoding follow SDL 3's `SDL_hidapi_switch2.c` (zlib), the driver that reads
//! the controller on the host. The button and motion mapping from Moonlight
//! follows Solstice's `viiper_gamepad.cpp`.
use crate::input::Input;
use crate::usbip::{Device, Setup, UsbDevice};
pub use rubylight_protocol::controller_identity::BUTTON_C;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub use rubylight_protocol::controller_identity::{NINTENDO as VENDOR, SWITCH2_PRO as PRODUCT};

/// The HID interface's interrupt IN and OUT endpoints.
pub const HID_ENDPOINT: u8 = 1;
/// The vendor interface's bulk OUT and IN endpoints, for commands.
pub const BULK_ENDPOINT: u8 = 2;
/// The controller reports every 4 ms (its HID endpoint's interval); SDL
/// calibrates its motion timestamps against that rate.
pub const REPORT_PERIOD: Duration = Duration::from_millis(4);

const REPORT_COMMON: u8 = 0x05;
const REPORT_PRO: u8 = 0x09;
const REPORT_OUTPUT: u8 = 0x02;

const FEATURE_BUTTONS: u8 = 0x01;
const FEATURE_STICKS: u8 = 0x02;
const FEATURE_IMU: u8 = 0x04;
const FEATURE_MOUSE: u8 = 0x10;
const FEATURE_RUMBLE: u8 = 0x20;

const CMD_FLASH: u8 = 0x02;
const CMD_USB: u8 = 0x03;
const CMD_PLAYER_LED: u8 = 0x09;
const CMD_FEATURE: u8 = 0x0C;
const FLASH_BLOCK: usize = 0x40;
/// The bulk endpoints' packet size; replies longer than one transfer are
/// handed out a transfer at a time.
const BULK_PACKET: usize = 64;

/// The Microsoft OS 1.0 vendor request code; Windows reads the WinUSB
/// compatible ID of the bulk interface with it.
const MS_OS_VENDOR_CODE: u8 = 0x20;
const DEVICE_INTERFACE_GUID: &str = "{7D8F1E5C-9D89-4E0D-A2F0-6D10F1F89A8F}";

/// Moonlight's trigger travel past which ZL or ZR is pressed (Solstice's).
pub const TRIGGER_THRESHOLD: u8 = 32;
const GRAVITY: f32 = 9.80665;
/// SDL decodes the accelerometer at ±8 g full scale.
const ACCEL_PER_MS2: f32 = 32767.0 / (8.0 * GRAVITY);
/// SDL decodes the gyroscope at 34.8 rad/s full scale.
const GYRO_PER_RADS: f32 = 32767.0 / 34.8;
/// SDL's clamped HD rumble amplitude: what it sends for full strength.
const RUMBLE_AMP_MAX: f32 = 29000.0;
/// IMU reports SDL reads (5 to settle, 100 to calibrate) before it trusts the
/// controller's timestamps, with margin; until then reports keep the 4 ms
/// cadence and timestamps step 4000 µs each.
const CALIBRATION_REPORTS: u32 = 120;

/// One Switch 2 Pro button: which byte and bit of report 0x05's button
/// field and of report 0x09's.
#[derive(Clone, Copy)]
struct Button {
    common: (usize, u8),
    pro: (usize, u8),
}
const fn button(common: (usize, u8), pro: (usize, u8)) -> Button {
    Button { common, pro }
}
const B: Button = button((0, 0x04), (0, 0x01));
const A: Button = button((0, 0x08), (0, 0x02));
const Y: Button = button((0, 0x01), (0, 0x04));
const X: Button = button((0, 0x02), (0, 0x08));
const R: Button = button((0, 0x40), (0, 0x10));
const ZR: Button = button((0, 0x80), (0, 0x20));
const PLUS: Button = button((1, 0x02), (0, 0x40));
const RIGHT_STICK: Button = button((1, 0x04), (0, 0x80));
const DOWN: Button = button((2, 0x01), (1, 0x01));
const RIGHT: Button = button((2, 0x04), (1, 0x02));
const LEFT: Button = button((2, 0x08), (1, 0x04));
const UP: Button = button((2, 0x02), (1, 0x08));
const L: Button = button((2, 0x40), (1, 0x10));
const ZL: Button = button((2, 0x80), (1, 0x20));
const MINUS: Button = button((1, 0x01), (1, 0x40));
const LEFT_STICK: Button = button((1, 0x08), (1, 0x80));
const HOME: Button = button((1, 0x10), (2, 0x01));
const CAPTURE: Button = button((1, 0x20), (2, 0x02));
const GR: Button = button((3, 0x01), (2, 0x04));
const GL: Button = button((3, 0x02), (2, 0x08));
const C: Button = button((1, 0x40), (2, 0x10));

/// Moonlight button flags and the button each presses. Face buttons go by
/// position, as Moonlight sends them: its A (south) is Nintendo's B. The
/// upper back grips (paddles 1 and 2, a Steam Deck's R4 and L4) are GR and
/// GL, as SDL 3 reports a Switch 2 Pro's; the touchpad click is Capture.
const BUTTONS: [(u32, Button); 22] = [
    (0x1000, B),
    (0x2000, A),
    (0x4000, Y),
    (0x8000, X),
    (0x0001, UP),
    (0x0002, DOWN),
    (0x0004, LEFT),
    (0x0008, RIGHT),
    (0x0010, PLUS),
    (0x0020, MINUS),
    (0x0040, LEFT_STICK),
    (0x0080, RIGHT_STICK),
    (0x0100, L),
    (0x0200, R),
    (0x0400, HOME),
    (0x01_0000, GR),
    (0x02_0000, GL),
    (0x10_0000, CAPTURE),
    (0x20_0000, CAPTURE),
    (BUTTON_C, C),
    // Trigger flags are never set by Moonlight; the triggers' travel is.
    (0, ZL),
    (0, ZR),
];

/// What the controller reports. Moonlight sends buttons, sticks and triggers
/// in one event, motion and battery in their own.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    buttons: u32,
    triggers: [u8; 2],
    /// Moonlight's, positive right and up.
    sticks: [i16; 4],
    /// Raw sensor words in the controller's axes.
    accel: [i16; 3],
    gyro: [i16; 3],
    /// Battery percentage, 0-100.
    battery: u8,
    charging: bool,
    external_power: bool,
}
impl Default for State {
    fn default() -> Self {
        let mut state = Self {
            buttons: 0,
            triggers: [0; 2],
            sticks: [0; 4],
            accel: [0; 3],
            gyro: [0; 3],
            battery: 100,
            charging: false,
            external_power: true,
        };
        // At rest, lying flat: gravity on SDL's Y axis.
        state.accel = accel_raw([0.0, GRAVITY, 0.0]);
        state
    }
}
fn clamp_raw(v: f32) -> i16 {
    v.round().clamp(-32768.0, 32767.0) as i16
}
/// SDL reads the controller's sensors as `(x, z, -y)`; Moonlight sends
/// SDL's axes, accel in m/s².
fn accel_raw(sdl: [f32; 3]) -> [i16; 3] {
    let k = ACCEL_PER_MS2;
    [
        clamp_raw(sdl[0] * k),
        clamp_raw(-sdl[2] * k),
        clamp_raw(sdl[1] * k),
    ]
}
/// The same for the gyroscope; Moonlight sends deg/s.
fn gyro_raw(sdl: [f32; 3]) -> [i16; 3] {
    let k = std::f32::consts::PI / 180.0 * GYRO_PER_RADS;
    [
        clamp_raw(sdl[0] * k),
        clamp_raw(-sdl[2] * k),
        clamp_raw(sdl[1] * k),
    ]
}
/// A Moonlight stick axis as the controller's 12-bit value, larger up and right.
pub fn stick(v: i16) -> u16 {
    (2048 + (f32::from(v) * 2047.0 / 32767.0).round() as i32).clamp(0, 4095) as u16
}
/// Two 12-bit values in three bytes, as the controller packs its sticks.
fn pack12(out: &mut [u8], x: u16, y: u16) {
    let (x, y) = (x.min(0xFFF), y.min(0xFFF));
    out[0] = x as u8;
    out[1] = ((x >> 8) & 0x0F) as u8 | ((y & 0x0F) << 4) as u8;
    out[2] = (y >> 4) as u8;
}
impl State {
    /// Applies a Moonlight event for this controller. Returns whether the
    /// report changed; other kinds of event are ignored.
    pub fn apply(&mut self, event: &Input) -> bool {
        let before = self.clone();
        match *event {
            Input::Controller {
                buttons,
                left_trigger,
                right_trigger,
                sticks,
                ..
            } => {
                self.buttons = buttons;
                self.triggers = [left_trigger, right_trigger];
                self.sticks = sticks;
            }
            Input::Motion { kind: 1, xyz, .. } => self.accel = accel_raw(xyz),
            Input::Motion { kind: 2, xyz, .. } => self.gyro = gyro_raw(xyz),
            Input::Battery { state, percent, .. } => {
                // LI_BATTERY_STATE_*: 3 charging, 4 plugged in and not
                // charging, 5 full; 255 is an unknown percentage.
                self.charging = state == 3;
                self.external_power = matches!(state, 3..=5);
                self.battery = if percent > 100 { 100 } else { percent };
            }
            _ => return false,
        }
        *self != before
    }
    fn pressed(&self) -> impl Iterator<Item = Button> + '_ {
        let zl = self.triggers[0] >= TRIGGER_THRESHOLD;
        let zr = self.triggers[1] >= TRIGGER_THRESHOLD;
        BUTTONS
            .into_iter()
            .filter(move |(flag, b)| {
                if *flag != 0 {
                    self.buttons & flag != 0
                } else {
                    (zl && b.common == ZL.common) || (zr && b.common == ZR.common)
                }
            })
            .map(|(_, b)| b)
    }
    fn sticks12(&self) -> [u16; 4] {
        self.sticks.map(stick)
    }
    /// Report 0x09's power byte: level 0-9, external power and charging.
    fn power(&self) -> u8 {
        let level = (u16::from(self.battery) * 9 / 100) as u8;
        (level << 2) | u8::from(self.external_power) | (u8::from(self.charging) << 1)
    }
    /// The battery voltage report 0x05 carries, from 3.5 V empty to 4.1 V full.
    fn millivolts(&self) -> u16 {
        3500 + u16::from(self.battery) * 6
    }
    /// Report 0x05, the one SDL selects: buttons, sticks, battery and, with
    /// the IMU feature on, timestamped motion.
    pub fn common_report(&self, counter: u32, motion: Option<u32>) -> [u8; 64] {
        let mut r = [0u8; 64];
        r[0] = REPORT_COMMON;
        r[1..5].copy_from_slice(&counter.to_le_bytes());
        for b in self.pressed() {
            r[5 + b.common.0] |= b.common.1;
        }
        let [lx, ly, rx, ry] = self.sticks12();
        pack12(&mut r[11..14], lx, ly);
        pack12(&mut r[14..17], rx, ry);
        r[0x20..0x22].copy_from_slice(&self.millivolts().to_le_bytes());
        r[0x22] = if self.charging { 0x34 } else { 0x20 };
        r[0x2A] = 0x01;
        if let Some(timestamp) = motion {
            r[0x2B..0x2F].copy_from_slice(&timestamp.to_le_bytes());
            for (i, v) in self.accel.iter().chain(&self.gyro).enumerate() {
                r[0x31 + 2 * i..0x33 + 2 * i].copy_from_slice(&v.to_le_bytes());
            }
        }
        r
    }
    /// Report 0x09, the controller's default before a driver selects another.
    pub fn pro_report(&self, counter: u8, rumble: bool) -> [u8; 64] {
        let mut r = [0u8; 64];
        r[0] = REPORT_PRO;
        r[1] = counter;
        r[2] = self.power();
        for b in self.pressed() {
            r[3 + b.pro.0] |= b.pro.1;
        }
        let [lx, ly, rx, ry] = self.sticks12();
        pack12(&mut r[6..9], lx, ly);
        pack12(&mut r[9..12], rx, ry);
        r[12] = if rumble { 0x38 } else { 0x30 };
        r
    }
}

/// The larger of an HD rumble actuator's two amplitudes, as SDL encodes
/// them (`EncodeHDRumble`): `(low, high)` frequency band.
fn actuator_amplitudes(actuator: &[u8]) -> (u16, u16) {
    // Byte 0 is the sequence counter; bytes 1-5 the encoded rumble.
    let d = &actuator[1..6];
    let high = (u32::from(d[2] & 0x0F) << 12) | (u32::from(d[1] & 0xFC) << 4);
    let low = (u32::from(d[4]) << 8) | u32::from(d[3] & 0xC0);
    let scale = |amp: u32| (amp as f32 * 65535.0 / RUMBLE_AMP_MAX).clamp(0.0, 65535.0) as u16;
    (scale(low), scale(high))
}
/// The motor speeds (low frequency, high frequency) an HD rumble output
/// report asks for: the stronger of the two actuators in each band. SDL
/// sends both actuators the same rumble.
pub fn rumble(report: &[u8]) -> Option<(u16, u16)> {
    let payload = match report.first() {
        Some(&REPORT_OUTPUT) => &report[1..],
        _ if report.len() == 32 => report,
        _ => return None,
    };
    if payload.len() < 32 {
        return None;
    }
    let (left_low, left_high) = actuator_amplitudes(&payload[..16]);
    let (right_low, right_high) = actuator_amplitudes(&payload[16..32]);
    Some((left_low.max(right_low), left_high.max(right_high)))
}

/// The virtual controller: its state, the command protocol's state, bulk
/// replies waiting for the host to read them, and rumble the host set.
pub struct Switch2Pro {
    pub state: State,
    serial: String,
    reports_enabled: bool,
    report_id: u8,
    feature_mask: u8,
    features: u8,
    counter32: u32,
    counter8: u8,
    /// IMU reports since the IMU feature was turned on.
    imu_reports: u32,
    /// The motion timestamp of the last IMU report, µs.
    motion_timestamp: u32,
    /// When calibration ended and the timestamp it had then.
    motion_clock: Option<(Instant, u32)>,
    bulk: VecDeque<Vec<u8>>,
    rumble: Option<(u16, u16)>,
    player_leds: Option<u8>,
}
impl Switch2Pro {
    /// `serial` is what Windows and Steam tell two controllers apart by; at
    /// most 16 characters are reported.
    pub fn new(serial: &str) -> Self {
        Self {
            state: State::default(),
            serial: serial.chars().filter(char::is_ascii).take(16).collect(),
            reports_enabled: false,
            report_id: REPORT_PRO,
            feature_mask: 0,
            features: FEATURE_BUTTONS | FEATURE_STICKS,
            counter32: 0,
            counter8: 0,
            imu_reports: 0,
            motion_timestamp: 0,
            motion_clock: None,
            bulk: VecDeque::new(),
            rumble: None,
            player_leds: None,
        }
    }
    /// The motor speeds (low frequency, high frequency) the host last set, once.
    pub fn take_rumble(&mut self) -> Option<(u16, u16)> {
        self.rumble.take()
    }
    /// The player lights the host last set, once.
    pub fn take_player_leds(&mut self) -> Option<u8> {
        self.player_leds.take()
    }
    /// Whether a new state should be reported at once rather than at the next
    /// 4 ms report: not while SDL calibrates its motion timestamps against
    /// the report rate.
    pub fn immediate(&self) -> bool {
        self.features & FEATURE_IMU == 0 || self.imu_reports >= CALIBRATION_REPORTS
    }
    /// Whether the host has started the input reports.
    pub fn reporting(&self) -> bool {
        self.reports_enabled
    }
    fn next_motion_timestamp(&mut self) -> u32 {
        if self.imu_reports < CALIBRATION_REPORTS {
            self.imu_reports += 1;
            self.motion_timestamp = self.motion_timestamp.wrapping_add(4000);
            return self.motion_timestamp;
        }
        let (start, base) = *self
            .motion_clock
            .get_or_insert((Instant::now(), self.motion_timestamp));
        // Never backwards, and never the same twice: SDL sends sensor
        // events only for a new timestamp.
        let now = base.wrapping_add(start.elapsed().as_micros() as u32);
        self.motion_timestamp = if (now.wrapping_sub(self.motion_timestamp) as i32) > 0 {
            now
        } else {
            self.motion_timestamp.wrapping_add(1)
        };
        self.motion_timestamp
    }
    fn report(&mut self, id: u8) -> Vec<u8> {
        let id = if id == 0 { self.report_id } else { id };
        if id == REPORT_COMMON {
            self.counter32 = self.counter32.wrapping_add(1);
            let motion = (self.features & FEATURE_IMU != 0).then(|| self.next_motion_timestamp());
            self.state.common_report(self.counter32, motion).to_vec()
        } else {
            self.counter8 = self.counter8.wrapping_add(1);
            let rumble = self.features & FEATURE_RUMBLE != 0;
            self.state.pro_report(self.counter8, rumble).to_vec()
        }
    }
    fn masked(&self, flags: u8) -> u8 {
        if self.feature_mask == 0 {
            flags
        } else {
            flags & self.feature_mask
        }
    }
    /// One command on the bulk OUT endpoint: `[command, 0x91, sequence,
    /// subcommand, 0, length, 0, 0, payload...]`. Every command is answered
    /// on the bulk IN endpoint.
    pub fn command(&mut self, out: &[u8]) {
        if out.len() < 8 {
            return;
        }
        let (cmd, seq, sub) = (out[0], out[2], out[3]);
        let arg = out.get(8).copied().unwrap_or(0);
        let header = [cmd, 0x01, seq, sub, 0x10, 0x78, 0x00, 0x00];
        let mut reply = header.to_vec();
        match (cmd, sub) {
            (CMD_FLASH, 0x01) if out.len() >= 16 => {
                let address = u32::from_le_bytes(out[12..16].try_into().unwrap());
                reply.resize(16, 0);
                reply[8] = FLASH_BLOCK as u8;
                reply[12..16].copy_from_slice(&address.to_le_bytes());
                reply.extend_from_slice(&self.flash(address));
            }
            (CMD_USB, 0x03) => {
                if out.len() >= 9 {
                    self.reports_enabled = arg != 0;
                }
                reply.extend_from_slice(&[1, 0, 0, 0]);
            }
            (CMD_USB, 0x0A) => {
                if matches!(arg, REPORT_COMMON | REPORT_PRO) {
                    self.report_id = arg;
                }
            }
            (CMD_USB, 0x0D) => {
                self.reports_enabled = true;
                reply.extend_from_slice(&[1, 0, 0, 0]);
            }
            (CMD_USB, _) => {}
            (CMD_FEATURE, 0x01) => {
                let mut payload = [0u8; 12];
                for (feature, index, value) in [
                    (FEATURE_BUTTONS, 0, 0x07),
                    (FEATURE_STICKS, 1, 0x07),
                    (FEATURE_IMU, 2, 0x01),
                    (FEATURE_MOUSE, 4, 0x03),
                    (FEATURE_RUMBLE, 5, 0x03),
                ] {
                    if arg & feature != 0 {
                        payload[4 + index] = value;
                    }
                }
                reply.extend_from_slice(&payload);
            }
            (CMD_FEATURE, sub) => {
                match sub {
                    0x02 => self.feature_mask = arg,
                    0x03 => {
                        self.feature_mask = 0;
                        self.features = 0;
                    }
                    0x04 => self.features |= self.masked(arg),
                    0x05 => self.features &= !self.masked(arg),
                    _ => {}
                }
                if self.features & FEATURE_IMU == 0 {
                    self.imu_reports = 0;
                    self.motion_clock = None;
                }
                reply.extend_from_slice(&[0, 0, 0, 0]);
            }
            (CMD_PLAYER_LED, 0x07) if out.len() >= 9 => {
                self.player_leds = Some(arg);
            }
            _ => {}
        }
        self.bulk.push_back(reply);
    }
    /// A 64-byte flash block: the serial number, and stick calibration that
    /// maps the full 12-bit range. Bias blocks stay zero, and the user
    /// calibration blocks lack their magic, so SDL keeps the factory values.
    fn flash(&self, address: u32) -> [u8; FLASH_BLOCK] {
        let mut block = [0u8; FLASH_BLOCK];
        match address {
            0x13000 => block[2..2 + self.serial.len()].copy_from_slice(self.serial.as_bytes()),
            0x13080 | 0x130C0 => {
                // Neutral, then the travel to the maximum and to the minimum.
                pack12(&mut block[0x28..0x2B], 2048, 2048);
                pack12(&mut block[0x2B..0x2E], 2047, 2047);
                pack12(&mut block[0x2E..0x31], 2048, 2048);
            }
            _ => {}
        }
        block
    }
    fn string(&self, index: u8) -> Option<Vec<u8>> {
        Some(match index {
            0 => vec![4, 3, 0x09, 0x04],
            1 => string_descriptor("Nintendo"),
            2 => string_descriptor("Switch 2 Pro Controller"),
            3 => string_descriptor(&self.serial),
            4 | 5 => string_descriptor("Nintendo Switch 2 Pro Controller"),
            6 => string_descriptor("Pro Controller"),
            // Microsoft OS 1.0 string: "MSFT100" and the vendor code.
            0xEE => {
                let mut d = string_descriptor("MSFT100");
                d.extend_from_slice(&[MS_OS_VENDOR_CODE, 0]);
                d[0] = d.len() as u8;
                d
            }
            _ => return None,
        })
    }
}

impl Device for Switch2Pro {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 2,
            vendor: VENDOR,
            product: PRODUCT,
            release: 0x0200,
            interfaces: vec![(3, 0, 0), (0xFF, 0, 0)],
        }
    }
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>> {
        let reply = match (setup.request_type, setup.request) {
            // GET_STATUS for the device, an interface or an endpoint.
            (0x80..=0x82, 0x00) => vec![0, 0],
            // CLEAR_FEATURE, SET_FEATURE, SET_ADDRESS, SET_CONFIGURATION,
            // SET_INTERFACE.
            (0x00..=0x02, 0x01 | 0x03) | (0x00, 0x05 | 0x09) | (0x01, 0x0b) => vec![],
            (0x80, 0x08) => vec![1],
            (0x81, 0x0a) => vec![0],
            (0x80, 0x06) => match (setup.value >> 8, setup.value as u8) {
                (1, _) => device_descriptor().to_vec(),
                (2, _) => configuration_descriptor(),
                (3, index) => self.string(index)?,
                // No device qualifier (a full-speed device), no BOS.
                _ => return None,
            },
            (0x81, 0x06) if setup.index == 0 => match setup.value >> 8 {
                0x21 => hid_descriptor().to_vec(),
                0x22 => REPORT_DESCRIPTOR.to_vec(),
                _ => return None,
            },
            // Microsoft OS 1.0 compatible ID and extended properties.
            (0xC0 | 0xC1, MS_OS_VENDOR_CODE) => match setup.index {
                4 => compatible_id(),
                5 => extended_properties(),
                _ => return None,
            },
            // HID class: GET_REPORT for an input report, GET_IDLE, GET_PROTOCOL.
            (0xa1, 0x01) => match (setup.value >> 8, setup.value as u8) {
                (1, id @ (0 | REPORT_COMMON | REPORT_PRO)) => self.report(id),
                _ => return None,
            },
            (0xa1, 0x02) => vec![0],
            (0xa1, 0x03) => vec![1],
            // SET_REPORT with the rumble output report, SET_IDLE, SET_PROTOCOL.
            (0x21, 0x09) => {
                if setup.value == (2 << 8) | u16::from(REPORT_OUTPUT) {
                    self.output(HID_ENDPOINT, data);
                }
                vec![]
            }
            (0x21, 0x0a | 0x0b) => vec![],
            _ => return None,
        };
        Some(reply)
    }
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>> {
        self.transfer_in(endpoint, BULK_PACKET)
    }
    fn transfer_in(&mut self, endpoint: u8, length: usize) -> Option<Vec<u8>> {
        match endpoint {
            // The real controller reports nothing until a driver starts it.
            HID_ENDPOINT if self.reports_enabled => Some(self.report(0)),
            BULK_ENDPOINT => {
                let reply = self.bulk.front_mut()?;
                let take = length.max(1).min(reply.len());
                let chunk: Vec<u8> = reply.drain(..take).collect();
                // A reply that ends exactly on a packet boundary is followed
                // by a zero-length packet only if the reader asked for more;
                // a drained reply is gone either way.
                if reply.is_empty() {
                    self.bulk.pop_front();
                }
                Some(chunk)
            }
            _ => None,
        }
    }
    fn output(&mut self, endpoint: u8, data: &[u8]) -> bool {
        match endpoint {
            HID_ENDPOINT => {
                if let Some(speeds) = rumble(data) {
                    self.rumble = Some(speeds);
                }
                true
            }
            BULK_ENDPOINT => {
                self.command(data);
                true
            }
            _ => false,
        }
    }
    fn retries_held(&self) -> bool {
        true
    }
}

fn device_descriptor() -> [u8; 18] {
    let [vl, vh] = VENDOR.to_le_bytes();
    let [pl, ph] = PRODUCT.to_le_bytes();
    // USB 2.0, miscellaneous class (interface association), 64-byte EP0,
    // release 2.00, strings 1, 2 and 3, one configuration.
    [
        18, 1, 0x00, 0x02, 0xEF, 0x02, 0x01, 64, vl, vh, pl, ph, 0x00, 0x02, 1, 2, 3, 1,
    ]
}
fn hid_descriptor() -> [u8; 9] {
    let [ll, lh] = (REPORT_DESCRIPTOR.len() as u16).to_le_bytes();
    [9, 0x21, 0x11, 0x01, 0, 1, 0x22, ll, lh]
}
fn configuration_descriptor() -> Vec<u8> {
    // Two interfaces, self-powered, 500 mA, configuration string 4.
    let mut d = vec![9, 2, 0, 0, 2, 1, 4, 0xC0, 0xFA];
    // Interface 0: HID with interrupt IN 0x81 and OUT 0x01, every 4 ms.
    d.extend_from_slice(&[9, 4, 0, 0, 2, 3, 0, 0, 5]);
    d.extend_from_slice(&hid_descriptor());
    d.extend_from_slice(&[7, 5, 0x80 | HID_ENDPOINT, 3, 64, 0, 4]);
    d.extend_from_slice(&[7, 5, HID_ENDPOINT, 3, 64, 0, 4]);
    // Interface 1: vendor-specific with bulk OUT 0x02 and IN 0x82.
    d.extend_from_slice(&[9, 4, 1, 0, 2, 0xFF, 0, 0, 6]);
    d.extend_from_slice(&[7, 5, BULK_ENDPOINT, 2, 64, 0, 0]);
    d.extend_from_slice(&[7, 5, 0x80 | BULK_ENDPOINT, 2, 64, 0, 0]);
    let total = (d.len() as u16).to_le_bytes();
    d[2..4].copy_from_slice(&total);
    d
}
fn string_descriptor(s: &str) -> Vec<u8> {
    let mut d = vec![0, 3];
    for unit in s.encode_utf16() {
        d.extend_from_slice(&unit.to_le_bytes());
    }
    d[0] = d.len() as u8;
    d
}
/// Microsoft OS 1.0 compatible ID descriptor: WinUSB for interface 1, so a
/// driver can claim the bulk interface without an INF.
fn compatible_id() -> Vec<u8> {
    let mut d = vec![0u8; 40];
    d[0..4].copy_from_slice(&40u32.to_le_bytes());
    d[4..6].copy_from_slice(&0x0100u16.to_le_bytes());
    d[6..8].copy_from_slice(&4u16.to_le_bytes());
    d[8] = 1;
    d[16] = 1;
    d[18..24].copy_from_slice(b"WINUSB");
    d
}
fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}
/// Microsoft OS 1.0 extended properties: the interface GUID libusb opens
/// the WinUSB interface by.
fn extended_properties() -> Vec<u8> {
    let name = utf16z("DeviceInterfaceGUID");
    let value = utf16z(DEVICE_INTERFACE_GUID);
    let section = 4 + 4 + 2 + name.len() + 4 + value.len();
    let mut d = Vec::with_capacity(10 + section);
    d.extend_from_slice(&((10 + section) as u32).to_le_bytes());
    d.extend_from_slice(&0x0100u16.to_le_bytes());
    d.extend_from_slice(&5u16.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&(section as u32).to_le_bytes());
    // REG_SZ
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&(name.len() as u16).to_le_bytes());
    d.extend_from_slice(&name);
    d.extend_from_slice(&(value.len() as u32).to_le_bytes());
    d.extend_from_slice(&value);
    d
}

/// The HID interface: vendor report 0x05 (63 bytes), gamepad report 0x09
/// (two vendor bytes, 21 buttons, four 12-bit axes, 52 vendor bytes) and
/// output report 0x02 (63 bytes), as libVIIPER declares them.
const REPORT_DESCRIPTOR: [u8; 97] = [
    0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, // Generic desktop, gamepad, application
    0x85, 0x05, 0x05, 0xFF, 0x09, 0x01, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x95, 0x3F, 0x75, 0x08, 0x81,
    0x02, // report 0x05: 63 vendor bytes
    0x85, 0x09, 0x09, 0x01, 0x95, 0x02, 0x81, 0x02, // report 0x09: two vendor bytes
    0x05, 0x09, 0x19, 0x01, 0x29, 0x15, 0x25, 0x01, 0x95, 0x15, 0x75, 0x01, 0x81,
    0x02, // 21 buttons
    0x95, 0x01, 0x75, 0x03, 0x81, 0x03, // padding
    0x05, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x09, 0x30, 0x09, 0x31, 0x09, 0x33, 0x09, 0x35, 0x26, 0xFF,
    0x0F, 0x95, 0x04, 0x75, 0x0C, 0x81, 0x02, 0xC0, // pointer: X, Y, Rx, Rz, 12 bits
    0x05, 0xFF, 0x09, 0x02, 0x26, 0xFF, 0x00, 0x95, 0x34, 0x75, 0x08, 0x81,
    0x02, // 52 vendor bytes
    0x85, 0x02, 0x09, 0x01, 0x95, 0x3F, 0x91, 0x02, // output report 0x02: 63 bytes
    0xC0,
];

#[cfg(test)]
mod tests;
