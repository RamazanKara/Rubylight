//! A virtual USB HID mouse and a virtual Precision Touchpad, served over USB/IP
//! ([`crate::usbip`]) and attached by usbip-win2's signed driver: Windows sees a mouse or
//! touchpad plugged into a USB port, with no driver of ours in its kernel.
//!
//! The mouse has a relative collection (buttons, 16-bit X and Y, a wheel with a resolution
//! multiplier and a horizontal pan) and an absolute one (X and Y on 0-32767), and is polled
//! every millisecond. Its reports reach games as a real mouse's do: raw input from a device,
//! never flagged as injected. Foundation Sunshine's virtual mouse (a UMDF driver,
//! `src/platform/windows/virtual_mouse.cpp`) gave the idea and the per-app mouse mode; this
//! device and its descriptors are our own.
//!
//! The touchpad follows Microsoft's Precision Touchpad HID protocol: five fingers in parallel
//! reports with scan time, contact count and the click button, the device capabilities and
//! certification status feature reports, and the configuration collection through which
//! Windows switches it to touchpad input. A tablet's physical touchpad (Moonlight V+'s and
//! Foundation Sunshine's touchpad packets, 0x55000009 and 0x5500000A) replays on it, so
//! Windows' own gestures work. The certification status blob is the one open-source PTP
//! drivers send, from imbushuo's mac-precision-touchpad (`AmtPtpDeviceSpiKm/Hid.h`, MIT,
//! Copyright 2018 Bingxing Wang; `third-party/licenses/mac-precision-touchpad-LICENSE.txt`).
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::usbip::{Device, Setup, UsbDevice};

/// pid.codes' open-source vendor ID.
pub const VENDOR: u16 = 0x1209;
pub const MOUSE_PRODUCT: u16 = 0x52b1;
pub const TOUCHPAD_PRODUCT: u16 = 0x52b2;
/// Both devices' interrupt IN endpoint, polled every millisecond (`bInterval` 1 at full speed).
pub const ENDPOINT: u8 = 1;
/// How often the USB/IP server completes a waiting transfer with no change: never sooner than
/// a change, which completes at once.
pub const POLL: Duration = Duration::from_millis(1);
/// Unsent reports kept for a host that stopped polling; older ones go first.
const QUEUE_LIMIT: usize = 256;

/// The HID mouse's button bits.
pub mod button {
    pub const LEFT: u8 = 0x01;
    pub const RIGHT: u8 = 0x02;
    pub const MIDDLE: u8 = 0x04;
    pub const BACK: u8 = 0x08;
    pub const FORWARD: u8 = 0x10;
}

/// A Moonlight mouse button (1 left, 2 middle, 3 right, 4 X1, 5 X2) as the HID bit.
pub fn button_bit(button: u8) -> Option<u8> {
    Some(match button {
        1 => button::LEFT,
        2 => button::MIDDLE,
        3 => button::RIGHT,
        4 => button::BACK,
        5 => button::FORWARD,
        _ => return None,
    })
}

const RELATIVE_ID: u8 = 1;
const ABSOLUTE_ID: u8 = 2;
const MULTIPLIER_ID: u8 = 3;
/// The wheel's resolution multiplier once Windows sets it (Logitech's eight steps a notch).
pub const WHEEL_MULTIPLIER: i32 = 8;
/// The absolute collection's logical maximum on both axes.
pub const ABSOLUTE_MAX: u16 = 32767;
/// Windows' wheel delta of one notch.
const NOTCH: i32 = 120;

/// The mouse's report descriptor: a relative mouse with a high-resolution wheel (report 1,
/// multiplier feature report 3) and an absolute mouse (report 2).
pub const MOUSE_REPORT_DESCRIPTOR: &[u8] = &[
    0x05,
    0x01, // Usage Page (Generic Desktop)
    0x09,
    0x02, // Usage (Mouse)
    0xa1,
    0x01, // Collection (Application)
    0x85,
    RELATIVE_ID, //   Report ID (1)
    0x09,
    0x01, //   Usage (Pointer)
    0xa1,
    0x00, //   Collection (Physical)
    0x05,
    0x09, //     Usage Page (Button)
    0x19,
    0x01, //     Usage Minimum (1)
    0x29,
    0x05, //     Usage Maximum (5)
    0x15,
    0x00, //     Logical Minimum (0)
    0x25,
    0x01, //     Logical Maximum (1)
    0x75,
    0x01, //     Report Size (1)
    0x95,
    0x05, //     Report Count (5)
    0x81,
    0x02, //     Input (Data, Var, Abs)
    0x75,
    0x03, //     Report Size (3)
    0x95,
    0x01, //     Report Count (1)
    0x81,
    0x01, //     Input (Const)
    0x05,
    0x01, //     Usage Page (Generic Desktop)
    0x09,
    0x30, //     Usage (X)
    0x09,
    0x31, //     Usage (Y)
    0x16,
    0x01,
    0x80, //     Logical Minimum (-32767)
    0x26,
    0xff,
    0x7f, //     Logical Maximum (32767)
    0x75,
    0x10, //     Report Size (16)
    0x95,
    0x02, //     Report Count (2)
    0x81,
    0x06, //     Input (Data, Var, Rel)
    0xa1,
    0x02, //     Collection (Logical)
    0x85,
    MULTIPLIER_ID, //       Report ID (3)
    0x09,
    0x48, //       Usage (Resolution Multiplier)
    0x15,
    0x00, //       Logical Minimum (0)
    0x25,
    0x01, //       Logical Maximum (1)
    0x35,
    0x01, //       Physical Minimum (1)
    0x45,
    WHEEL_MULTIPLIER as u8, //       Physical Maximum (8)
    0x75,
    0x02, //       Report Size (2)
    0x95,
    0x01, //       Report Count (1)
    0xb1,
    0x02, //       Feature (Data, Var, Abs)
    0x75,
    0x06, //       Report Size (6)
    0xb1,
    0x01, //       Feature (Const)
    0x85,
    RELATIVE_ID, //       Report ID (1)
    0x35,
    0x00, //       Physical Minimum (0)
    0x45,
    0x00, //       Physical Maximum (0)
    0x09,
    0x38, //       Usage (Wheel)
    0x15,
    0x81, //       Logical Minimum (-127)
    0x25,
    0x7f, //       Logical Maximum (127)
    0x75,
    0x08, //       Report Size (8)
    0x95,
    0x01, //       Report Count (1)
    0x81,
    0x06, //       Input (Data, Var, Rel)
    0xc0, //     End Collection
    0x05,
    0x0c, //     Usage Page (Consumer)
    0x0a,
    0x38,
    0x02, //     Usage (AC Pan)
    0x15,
    0x81, //     Logical Minimum (-127)
    0x25,
    0x7f, //     Logical Maximum (127)
    0x75,
    0x08, //     Report Size (8)
    0x95,
    0x01, //     Report Count (1)
    0x81,
    0x06, //     Input (Data, Var, Rel)
    0xc0, //   End Collection
    0xc0, // End Collection
    0x05,
    0x01, // Usage Page (Generic Desktop)
    0x09,
    0x02, // Usage (Mouse)
    0xa1,
    0x01, // Collection (Application)
    0x85,
    ABSOLUTE_ID, //   Report ID (2)
    0x09,
    0x01, //   Usage (Pointer)
    0xa1,
    0x00, //   Collection (Physical)
    0x05,
    0x09, //     Usage Page (Button)
    0x19,
    0x01, //     Usage Minimum (1)
    0x29,
    0x05, //     Usage Maximum (5)
    0x15,
    0x00, //     Logical Minimum (0)
    0x25,
    0x01, //     Logical Maximum (1)
    0x75,
    0x01, //     Report Size (1)
    0x95,
    0x05, //     Report Count (5)
    0x81,
    0x02, //     Input (Data, Var, Abs)
    0x75,
    0x03, //     Report Size (3)
    0x95,
    0x01, //     Report Count (1)
    0x81,
    0x01, //     Input (Const)
    0x05,
    0x01, //     Usage Page (Generic Desktop)
    0x09,
    0x30, //     Usage (X)
    0x09,
    0x31, //     Usage (Y)
    0x15,
    0x00, //     Logical Minimum (0)
    0x26,
    0xff,
    0x7f, //     Logical Maximum (32767)
    0x75,
    0x10, //     Report Size (16)
    0x95,
    0x02, //     Report Count (2)
    0x81,
    0x02, //     Input (Data, Var, Abs)
    0xc0, //   End Collection
    0xc0, // End Collection
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseReport {
    Relative {
        buttons: u8,
        x: i16,
        y: i16,
        wheel: i8,
        pan: i8,
    },
    /// The absolute collection carries no buttons: they go on the relative one only, so
    /// Windows sees each press once.
    Absolute { x: u16, y: u16 },
}
impl MouseReport {
    fn encode(self) -> Vec<u8> {
        match self {
            Self::Relative {
                buttons,
                x,
                y,
                wheel,
                pan,
            } => {
                let mut r = vec![RELATIVE_ID, buttons];
                r.extend_from_slice(&x.to_le_bytes());
                r.extend_from_slice(&y.to_le_bytes());
                r.extend_from_slice(&[wheel as u8, pan as u8]);
                r
            }
            Self::Absolute { x, y } => {
                let mut r = vec![ABSOLUTE_ID, 0];
                r.extend_from_slice(&x.to_le_bytes());
                r.extend_from_slice(&y.to_le_bytes());
                r
            }
        }
    }
}

/// The virtual mouse: its button state and the reports Windows has not polled yet. Motion
/// and wheel merge into the newest unsent report; a button change always starts a new one,
/// so a click shorter than a poll still reaches Windows as a press and a release.
pub struct HidMouse {
    buttons: u8,
    queue: VecDeque<MouseReport>,
    /// Wheel and pan in 1/120 notches not sent yet.
    scroll: [i32; 2],
    /// The resolution multiplier Windows set (1 until it does).
    multiplier: i32,
    serial: String,
}
impl HidMouse {
    pub fn new(serial: &str) -> Self {
        Self {
            buttons: 0,
            queue: VecDeque::new(),
            scroll: [0; 2],
            multiplier: 1,
            serial: serial.chars().take(32).collect(),
        }
    }
    /// The buttons held now.
    pub fn buttons(&self) -> u8 {
        self.buttons
    }
    /// The wheel steps per notch Windows asked for: 1, or [`WHEEL_MULTIPLIER`].
    pub fn multiplier(&self) -> i32 {
        self.multiplier
    }
    fn push(&mut self, report: MouseReport) {
        if self.queue.len() >= QUEUE_LIMIT {
            self.queue.pop_front();
        }
        self.queue.push_back(report);
    }
    /// The newest unsent relative report, made if there is none.
    fn relative(&mut self) -> &mut MouseReport {
        if !matches!(self.queue.back(), Some(MouseReport::Relative { .. })) {
            let buttons = self.buttons;
            self.push(MouseReport::Relative {
                buttons,
                x: 0,
                y: 0,
                wheel: 0,
                pan: 0,
            });
        }
        self.queue.back_mut().unwrap()
    }
    /// Relative motion in mickeys. Returns whether a report is waiting.
    pub fn motion(&mut self, mut dx: i32, mut dy: i32) -> bool {
        while dx != 0 || dy != 0 {
            let MouseReport::Relative { x, y, .. } = self.relative() else {
                unreachable!()
            };
            let take = |sum: &mut i16, rest: &mut i32| {
                let total = (i32::from(*sum) + *rest).clamp(-32767, 32767);
                *rest -= total - i32::from(*sum);
                *sum = total as i16;
            };
            take(x, &mut dx);
            take(y, &mut dy);
            if dx != 0 || dy != 0 {
                // The report is full: the rest goes in the next one.
                let buttons = self.buttons;
                self.push(MouseReport::Relative {
                    buttons,
                    x: 0,
                    y: 0,
                    wheel: 0,
                    pan: 0,
                });
            }
        }
        !self.queue.is_empty()
    }
    /// A button pressed or released (a [`button`] bit). False when nothing changed.
    pub fn button(&mut self, bit: u8, down: bool) -> bool {
        let buttons = if down {
            self.buttons | bit
        } else {
            self.buttons & !bit
        };
        if buttons == self.buttons {
            return false;
        }
        self.buttons = buttons;
        self.push(MouseReport::Relative {
            buttons,
            x: 0,
            y: 0,
            wheel: 0,
            pan: 0,
        });
        true
    }
    /// A wheel turn in 1/120 notches (Windows' `WHEEL_DELTA` scale), positive up or right.
    /// Returns whether a report is waiting; parts of a step wait for the rest.
    pub fn scroll(&mut self, amount: i16, horizontal: bool) -> bool {
        let index = usize::from(horizontal);
        // Only the vertical wheel has a resolution multiplier.
        let step = if horizontal {
            NOTCH
        } else {
            NOTCH / self.multiplier
        };
        self.scroll[index] += i32::from(amount);
        let mut steps = self.scroll[index] / step;
        self.scroll[index] -= steps * step;
        while steps != 0 {
            let MouseReport::Relative { wheel, pan, .. } = self.relative() else {
                unreachable!()
            };
            let field = if horizontal { pan } else { wheel };
            let total = (i32::from(*field) + steps).clamp(-127, 127);
            steps -= total - i32::from(*field);
            *field = total as i8;
            if steps != 0 {
                let buttons = self.buttons;
                self.push(MouseReport::Relative {
                    buttons,
                    x: 0,
                    y: 0,
                    wheel: 0,
                    pan: 0,
                });
            }
        }
        !self.queue.is_empty()
    }
    /// The pointer at `x`, `y` on 0-[`ABSOLUTE_MAX`] of the absolute collection's surface.
    pub fn absolute(&mut self, x: u16, y: u16) {
        let report = MouseReport::Absolute {
            x: x.min(ABSOLUTE_MAX),
            y: y.min(ABSOLUTE_MAX),
        };
        match self.queue.back_mut() {
            Some(back @ MouseReport::Absolute { .. }) => *back = report,
            _ => self.push(report),
        }
    }
    /// Releases every button; returns whether one was held.
    pub fn release_all(&mut self) -> bool {
        self.scroll = [0; 2];
        if self.buttons == 0 {
            return false;
        }
        self.buttons = 0;
        self.push(MouseReport::Relative {
            buttons: 0,
            x: 0,
            y: 0,
            wheel: 0,
            pan: 0,
        });
        true
    }
    /// Drops reports Windows did not poll, for a device that is not attached.
    pub fn discard(&mut self) {
        self.queue.clear();
    }
}

impl Device for HidMouse {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 2,
            vendor: VENDOR,
            product: MOUSE_PRODUCT,
            release: 0x0100,
            interfaces: vec![(3, 0, 0)],
        }
    }
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>> {
        let strings = ["Rubylight", "Rubylight Virtual Mouse", self.serial.as_str()];
        let reply = match standard(setup, MOUSE_PRODUCT, MOUSE_REPORT_DESCRIPTOR, 16, &strings) {
            Standard::Reply(reply) => reply,
            Standard::Stall => return None,
            Standard::Class => match (setup.request_type, setup.request, setup.value) {
                // GET_REPORT: an input report as it stands, or the multiplier.
                (0xa1, 0x01, value) => match (value >> 8, value as u8) {
                    (1, RELATIVE_ID) => MouseReport::Relative {
                        buttons: self.buttons,
                        x: 0,
                        y: 0,
                        wheel: 0,
                        pan: 0,
                    }
                    .encode(),
                    (1, ABSOLUTE_ID) => MouseReport::Absolute { x: 0, y: 0 }.encode(),
                    (3, MULTIPLIER_ID) => vec![MULTIPLIER_ID, u8::from(self.multiplier > 1)],
                    _ => return None,
                },
                // SET_REPORT of the multiplier: logical 1 is physical 8.
                (0x21, 0x09, value) if value == 0x0300 | u16::from(MULTIPLIER_ID) => {
                    let logical = data.get(1).copied().unwrap_or(0) & 0x03;
                    self.multiplier = if logical >= 1 { WHEEL_MULTIPLIER } else { 1 };
                    self.scroll[0] = 0;
                    vec![]
                }
                (0x21, 0x09, _) => return None,
                _ => return hid_class(setup),
            },
        };
        Some(reply)
    }
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>> {
        (endpoint == ENDPOINT)
            .then(|| self.queue.pop_front().map(MouseReport::encode))
            .flatten()
    }
    fn interrupt_ready(&self, endpoint: u8) -> bool {
        endpoint == ENDPOINT && !self.queue.is_empty()
    }
}

/// The certification status feature report's blob (Usage 0xC5 of page 0xFF00), as
/// open-source Precision Touchpad drivers send it (mac-precision-touchpad, MIT).
pub const PTP_CERTIFICATION_BLOB: [u8; 256] = [
    0xfc, 0x28, 0xfe, 0x84, 0x40, 0xcb, 0x9a, 0x87, 0x0d, 0xbe, 0x57, 0x3c, 0xb6, 0x70, 0x09, 0x88,
    0x07, 0x97, 0x2d, 0x2b, 0xe3, 0x38, 0x34, 0xb6, 0x6c, 0xed, 0xb0, 0xf7, 0xe5, 0x9c, 0xf6, 0xc2,
    0x2e, 0x84, 0x1b, 0xe8, 0xb4, 0x51, 0x78, 0x43, 0x1f, 0x28, 0x4b, 0x7c, 0x2d, 0x53, 0xaf, 0xfc,
    0x47, 0x70, 0x1b, 0x59, 0x6f, 0x74, 0x43, 0xc4, 0xf3, 0x47, 0x18, 0x53, 0x1a, 0xa2, 0xa1, 0x71,
    0xc7, 0x95, 0x0e, 0x31, 0x55, 0x21, 0xd3, 0xb5, 0x1e, 0xe9, 0x0c, 0xba, 0xec, 0xb8, 0x89, 0x19,
    0x3e, 0xb3, 0xaf, 0x75, 0x81, 0x9d, 0x53, 0xb9, 0x41, 0x57, 0xf4, 0x6d, 0x39, 0x25, 0x29, 0x7c,
    0x87, 0xd9, 0xb4, 0x98, 0x45, 0x7d, 0xa7, 0x26, 0x9c, 0x65, 0x3b, 0x85, 0x68, 0x89, 0xd7, 0x3b,
    0xbd, 0xff, 0x14, 0x67, 0xf2, 0x2b, 0xf0, 0x2a, 0x41, 0x54, 0xf0, 0xfd, 0x2c, 0x66, 0x7c, 0xf8,
    0xc0, 0x8f, 0x33, 0x13, 0x03, 0xf1, 0xd3, 0xc1, 0x0b, 0x89, 0xd9, 0x1b, 0x62, 0xcd, 0x51, 0xb7,
    0x80, 0xb8, 0xaf, 0x3a, 0x10, 0xc1, 0x8a, 0x5b, 0xe8, 0x8a, 0x56, 0xf0, 0x8c, 0xaa, 0xfa, 0x35,
    0xe9, 0x42, 0xc4, 0xd8, 0x55, 0xc3, 0x38, 0xcc, 0x2b, 0x53, 0x5c, 0x69, 0x52, 0xd5, 0xc8, 0x73,
    0x02, 0x38, 0x7c, 0x73, 0xb6, 0x41, 0xe7, 0xff, 0x05, 0xd8, 0x2b, 0x79, 0x9a, 0xe2, 0x34, 0x60,
    0x8f, 0xa3, 0x32, 0x1f, 0x09, 0x78, 0x62, 0xbc, 0x80, 0xe3, 0x0f, 0xbd, 0x65, 0x20, 0x08, 0x13,
    0xc1, 0xe2, 0xee, 0x53, 0x2d, 0x86, 0x7e, 0xa7, 0x5a, 0xc5, 0xd3, 0x7d, 0x98, 0xbe, 0x31, 0x48,
    0x1f, 0xfb, 0xda, 0xaf, 0xa2, 0xa8, 0x6a, 0x89, 0xd6, 0xbf, 0xf2, 0xd3, 0x32, 0x2a, 0x9a, 0xe4,
    0xcf, 0x17, 0xb7, 0xb8, 0xf4, 0xe1, 0x33, 0x08, 0x24, 0x8b, 0xc4, 0x43, 0xa5, 0xe5, 0x24, 0xc2,
];

/// Fingers the touchpad reports at once, as Moonlight V+'s frame packet carries.
pub const MAX_CONTACTS: usize = 5;
/// The size a client that reports none gets: a typical tablet keyboard cover's touchpad.
pub const DEFAULT_SIZE_MM: (u16, u16) = (105, 70);
const INPUT_MODE_ID: u8 = 3;
const TOUCH_ID: u8 = 4;
const CAPABILITIES_ID: u8 = 5;
const CERTIFICATION_ID: u8 = 6;
const FUNCTION_SWITCH_ID: u8 = 7;
/// Input mode Windows sets to take touchpad reports instead of a mouse's.
pub const INPUT_MODE_TOUCHPAD: u8 = 3;
/// One report: ID, five fingers of five bytes, scan time, contact count and the button.
const TOUCH_REPORT_BYTES: usize = 1 + MAX_CONTACTS * 5 + 2 + 1 + 1;

/// The touchpad's report descriptor for a surface of `width_mm` by `height_mm`: one logical
/// unit is 0.1 mm.
pub fn touchpad_report_descriptor(width_mm: u16, height_mm: u16) -> Vec<u8> {
    let (x, y) = touchpad_extent(width_mm, height_mm);
    let [xl, xh] = x.to_le_bytes();
    let [yl, yh] = y.to_le_bytes();
    let mut d = vec![
        0x05, 0x0d, // Usage Page (Digitizer)
        0x09, 0x05, // Usage (Touch Pad)
        0xa1, 0x01, // Collection (Application)
        0x85, TOUCH_ID, //   Report ID (4)
    ];
    for _ in 0..MAX_CONTACTS {
        d.extend_from_slice(&[
            0x05, 0x0d, //   Usage Page (Digitizer)
            0x09, 0x22, //   Usage (Finger)
            0xa1, 0x02, //   Collection (Logical)
            0x15, 0x00, //     Logical Minimum (0)
            0x25, 0x01, //     Logical Maximum (1)
            0x75, 0x01, //     Report Size (1)
            0x95, 0x01, //     Report Count (1)
            0x09, 0x47, //     Usage (Confidence)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x09, 0x42, //     Usage (Tip Switch)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x25, 0x07, //     Logical Maximum (7)
            0x75, 0x03, //     Report Size (3)
            0x09, 0x51, //     Usage (Contact Identifier)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x81, 0x03, //     Input (Const): 3 bits
            0x05, 0x01, //     Usage Page (Generic Desktop)
            0x15, 0x00, //     Logical Minimum (0)
            0x26, xl, xh, //     Logical Maximum (width in 0.1 mm)
            0x75, 0x10, //     Report Size (16)
            0x55, 0x0e, //     Unit Exponent (-2)
            0x65, 0x11, //     Unit (SI Linear: centimetre)
            0x09, 0x30, //     Usage (X)
            0x35, 0x00, //     Physical Minimum (0)
            0x46, xl, xh, //     Physical Maximum (width)
            0x95, 0x01, //     Report Count (1)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x26, yl, yh, //     Logical Maximum (height in 0.1 mm)
            0x46, yl, yh, //     Physical Maximum (height)
            0x09, 0x31, //     Usage (Y)
            0x81, 0x02, //     Input (Data, Var, Abs)
            0x45, 0x00, //     Physical Maximum (0)
            0x55, 0x00, //     Unit Exponent (0)
            0x65, 0x00, //     Unit (None)
            0xc0, //   End Collection
        ]);
    }
    d.extend_from_slice(&[
        0x05,
        0x0d, //   Usage Page (Digitizer)
        0x55,
        0x0c, //   Unit Exponent (-4)
        0x66,
        0x01,
        0x10, //   Unit (SI Linear: second)
        0x47,
        0xff,
        0xff,
        0x00,
        0x00, //   Physical Maximum (65535)
        0x27,
        0xff,
        0xff,
        0x00,
        0x00, //   Logical Maximum (65535)
        0x75,
        0x10, //   Report Size (16)
        0x95,
        0x01, //   Report Count (1)
        0x09,
        0x56, //   Usage (Scan Time)
        0x81,
        0x02, //   Input (Data, Var, Abs)
        0x45,
        0x00, //   Physical Maximum (0)
        0x55,
        0x00, //   Unit Exponent (0)
        0x65,
        0x00, //   Unit (None)
        0x09,
        0x54, //   Usage (Contact Count)
        0x25,
        0x7f, //   Logical Maximum (127)
        0x75,
        0x08, //   Report Size (8)
        0x81,
        0x02, //   Input (Data, Var, Abs)
        0x05,
        0x09, //   Usage Page (Button)
        0x09,
        0x01, //   Usage (Button 1)
        0x25,
        0x01, //   Logical Maximum (1)
        0x75,
        0x01, //   Report Size (1)
        0x81,
        0x02, //   Input (Data, Var, Abs)
        0x95,
        0x07, //   Report Count (7)
        0x81,
        0x03, //   Input (Const)
        0x05,
        0x0d, //   Usage Page (Digitizer)
        0x85,
        CAPABILITIES_ID, //   Report ID (5)
        0x09,
        0x55, //   Usage (Contact Count Maximum)
        0x09,
        0x59, //   Usage (Pad Type)
        0x25,
        0x0f, //   Logical Maximum (15)
        0x75,
        0x04, //   Report Size (4)
        0x95,
        0x02, //   Report Count (2)
        0xb1,
        0x02, //   Feature (Data, Var, Abs)
        0x06,
        0x00,
        0xff, //   Usage Page (Vendor 0xFF00)
        0x85,
        CERTIFICATION_ID, //   Report ID (6)
        0x09,
        0xc5, //   Usage (Certification Status)
        0x15,
        0x00, //   Logical Minimum (0)
        0x26,
        0xff,
        0x00, //   Logical Maximum (255)
        0x75,
        0x08, //   Report Size (8)
        0x96,
        0x00,
        0x01, //   Report Count (256)
        0xb1,
        0x02, //   Feature (Data, Var, Abs)
        0xc0, // End Collection
        0x05,
        0x0d, // Usage Page (Digitizer)
        0x09,
        0x0e, // Usage (Device Configuration)
        0xa1,
        0x01, // Collection (Application)
        0x85,
        INPUT_MODE_ID, //   Report ID (3)
        0x09,
        0x22, //   Usage (Finger)
        0xa1,
        0x02, //   Collection (Logical)
        0x09,
        0x52, //     Usage (Input Mode)
        0x15,
        0x00, //     Logical Minimum (0)
        0x25,
        0x0a, //     Logical Maximum (10)
        0x75,
        0x08, //     Report Size (8)
        0x95,
        0x01, //     Report Count (1)
        0xb1,
        0x02, //     Feature (Data, Var, Abs)
        0xc0, //   End Collection
        0x09,
        0x22, //   Usage (Finger)
        0xa1,
        0x00, //   Collection (Physical)
        0x85,
        FUNCTION_SWITCH_ID, //     Report ID (7)
        0x09,
        0x57, //     Usage (Surface Switch)
        0x09,
        0x58, //     Usage (Button Switch)
        0x25,
        0x01, //     Logical Maximum (1)
        0x75,
        0x01, //     Report Size (1)
        0x95,
        0x02, //     Report Count (2)
        0xb1,
        0x02, //     Feature (Data, Var, Abs)
        0x95,
        0x06, //     Report Count (6)
        0xb1,
        0x03, //     Feature (Const)
        0xc0, //   End Collection
        0xc0, // End Collection
    ]);
    d
}

/// The logical (and physical) maximum of each axis, in 0.1 mm: the client's size, or
/// [`DEFAULT_SIZE_MM`] for one it did not report, at most 3 m.
fn touchpad_extent(width_mm: u16, height_mm: u16) -> (u16, u16) {
    let (width_mm, height_mm) = if width_mm == 0 || height_mm == 0 {
        DEFAULT_SIZE_MM
    } else {
        (width_mm, height_mm)
    };
    (
        width_mm.clamp(10, 3000) * 10,
        height_mm.clamp(10, 3000) * 10,
    )
}

/// One finger of a touchpad packet, as `butterpollo_core::input` decodes it: a Moonlight touch
/// event (0 hover, 1 down, 2 up, 3 move, 4 cancel, 5 button only, 6 hover leave, 7 cancel
/// all), the client's pointer id and its place on the surface (0-1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub event: u8,
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Finger {
    client: u32,
    x: u16,
    y: u16,
}

/// The virtual Precision Touchpad: fingers on their slots (the HID contact ids), the click
/// button, what Windows configured and the reports it has not polled yet.
pub struct PrecisionTouchpad {
    extent: (u16, u16),
    size_mm: (u16, u16),
    slots: [Option<Finger>; MAX_CONTACTS],
    button: bool,
    queue: VecDeque<[u8; TOUCH_REPORT_BYTES]>,
    input_mode: u8,
    surface: bool,
    buttons: bool,
    epoch: Instant,
    last_scan: Option<u16>,
    serial: String,
    /// Contacts that came before Windows switched the device to touchpad input.
    pub early_contacts: u64,
}
impl PrecisionTouchpad {
    /// A touchpad of the client's size (0 for unknown), whose scan times start at `epoch`.
    pub fn new(width_mm: u16, height_mm: u16, serial: &str, epoch: Instant) -> Self {
        Self {
            extent: touchpad_extent(width_mm, height_mm),
            size_mm: (width_mm, height_mm),
            slots: [None; MAX_CONTACTS],
            button: false,
            queue: VecDeque::new(),
            input_mode: 0,
            surface: true,
            buttons: true,
            epoch,
            last_scan: None,
            serial: serial.chars().take(32).collect(),
            early_contacts: 0,
        }
    }
    /// The size the device was made with, as the client reported it.
    pub fn size_mm(&self) -> (u16, u16) {
        self.size_mm
    }
    /// Whether Windows took the device as a Precision Touchpad (input mode 3).
    pub fn active(&self) -> bool {
        self.input_mode == INPUT_MODE_TOUCHPAD
    }
    /// Fingers down now.
    pub fn fingers(&self) -> usize {
        self.slots.iter().flatten().count()
    }
    /// Applies the contacts of one packet (a frame, or one contact) with the touchpad's
    /// buttons (bit 0 the click) at `now`, and queues the report Windows reads. Contacts the
    /// packet does not name stay where they were. Returns whether a report was queued.
    pub fn apply(&mut self, contacts: &[Contact], buttons: u8, now: Instant) -> bool {
        // Slots in this report: (slot, finger, tip, confidence).
        let mut lifted: Vec<(usize, Finger, bool)> = Vec::new();
        for contact in contacts.iter().take(MAX_CONTACTS) {
            let slot = self
                .slots
                .iter()
                .position(|s| s.is_some_and(|f| f.client == contact.id));
            match contact.event {
                // Down, or a move of a finger whose down was lost.
                1 | 3 => {
                    let finger = Finger {
                        client: contact.id,
                        x: scale(contact.x, self.extent.0),
                        y: scale(contact.y, self.extent.1),
                    };
                    match slot.or_else(|| self.slots.iter().position(Option::is_none)) {
                        Some(slot) => self.slots[slot] = Some(finger),
                        // A sixth finger has no slot.
                        None => continue,
                    }
                }
                // Up lifts the finger; cancel lifts it as not a touch (a palm).
                2 | 4 => {
                    if let Some(slot) = slot {
                        let mut finger = self.slots[slot].take().unwrap();
                        if contact.x.is_finite() && contact.y.is_finite() && contact.event == 2 {
                            finger.x = scale(contact.x, self.extent.0);
                            finger.y = scale(contact.y, self.extent.1);
                        }
                        lifted.push((slot, finger, contact.event == 2));
                    }
                }
                7 => {
                    for slot in 0..MAX_CONTACTS {
                        if let Some(finger) = self.slots[slot].take() {
                            lifted.push((slot, finger, false));
                        }
                    }
                }
                // Hover, hover leave and button-only events move no finger.
                _ => {}
            }
        }
        let button = buttons & 1 != 0;
        let button_changed = button != self.button;
        self.button = button;
        if !self.active() {
            self.early_contacts += contacts.len() as u64;
            return false;
        }
        if contacts.is_empty() && !button_changed {
            return false;
        }
        let mut report = [0u8; TOUCH_REPORT_BYTES];
        report[0] = TOUCH_ID;
        let mut count = 0usize;
        if self.surface {
            let down = (0..MAX_CONTACTS)
                .filter_map(|slot| self.slots[slot].map(|f| (slot, f, true, true)));
            let up = lifted
                .iter()
                .map(|&(slot, finger, confident)| (slot, finger, false, confident));
            for (slot, finger, tip, confident) in up.chain(down).take(MAX_CONTACTS) {
                let at = 1 + count * 5;
                report[at] = u8::from(confident) | u8::from(tip) << 1 | (slot as u8) << 2;
                report[at + 1..at + 3].copy_from_slice(&finger.x.to_le_bytes());
                report[at + 3..at + 5].copy_from_slice(&finger.y.to_le_bytes());
                count += 1;
            }
        }
        if count == 0 && !(button_changed && self.buttons) {
            return false;
        }
        let at = 1 + MAX_CONTACTS * 5;
        let scan = self.scan_time(now);
        report[at..at + 2].copy_from_slice(&scan.to_le_bytes());
        report[at + 2] = count as u8;
        report[at + 3] = u8::from(self.buttons && self.button);
        if self.queue.len() >= QUEUE_LIMIT {
            self.queue.pop_front();
        }
        self.queue.push_back(report);
        true
    }
    /// Lifts every finger and releases the button (the client left or its touchpad went
    /// away). Returns whether a report was queued.
    pub fn release_all(&mut self, now: Instant) -> bool {
        let cancel = Contact {
            event: 7,
            id: 0,
            x: 0.,
            y: 0.,
        };
        self.apply(&[cancel], 0, now)
    }
    /// Scan time in 100 us units from `epoch`, wrapping, and never the same twice in a row.
    fn scan_time(&mut self, now: Instant) -> u16 {
        let ticks = (now.saturating_duration_since(self.epoch).as_micros() / 100) as u64;
        let mut scan = (ticks % 65536) as u16;
        if self.last_scan == Some(scan) {
            scan = scan.wrapping_add(1);
        }
        self.last_scan = Some(scan);
        scan
    }
    pub fn discard(&mut self) {
        self.queue.clear();
    }
    fn current(&mut self) -> Vec<u8> {
        let mut report = [0u8; TOUCH_REPORT_BYTES];
        report[0] = TOUCH_ID;
        report.to_vec()
    }
}

/// A position on the surface (0-1) on a logical axis of `max`.
fn scale(v: f32, max: u16) -> u16 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * f32::from(max)).round() as u16
}

impl Device for PrecisionTouchpad {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 2,
            vendor: VENDOR,
            product: TOUCHPAD_PRODUCT,
            release: 0x0100,
            interfaces: vec![(3, 0, 0)],
        }
    }
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>> {
        let descriptor = touchpad_report_descriptor(self.extent.0 / 10, self.extent.1 / 10);
        let strings = [
            "Rubylight",
            "Rubylight Virtual Touchpad",
            self.serial.as_str(),
        ];
        let reply = match standard(setup, TOUCHPAD_PRODUCT, &descriptor, 64, &strings) {
            Standard::Reply(reply) => reply,
            Standard::Stall => return None,
            Standard::Class => match (setup.request_type, setup.request) {
                (0xa1, 0x01) => match (setup.value >> 8, setup.value as u8) {
                    (1, TOUCH_ID) => self.current(),
                    (3, CAPABILITIES_ID) => {
                        // Five contacts; pad type 0, a click pad (its whole surface clicks).
                        vec![CAPABILITIES_ID, MAX_CONTACTS as u8]
                    }
                    (3, CERTIFICATION_ID) => {
                        let mut r = vec![CERTIFICATION_ID];
                        r.extend_from_slice(&PTP_CERTIFICATION_BLOB);
                        r
                    }
                    (3, INPUT_MODE_ID) => vec![INPUT_MODE_ID, self.input_mode],
                    (3, FUNCTION_SWITCH_ID) => vec![
                        FUNCTION_SWITCH_ID,
                        u8::from(self.surface) | u8::from(self.buttons) << 1,
                    ],
                    _ => return None,
                },
                (0x21, 0x09) => match (setup.value >> 8, setup.value as u8) {
                    (3, INPUT_MODE_ID) => {
                        self.input_mode = data.get(1).copied().unwrap_or(0);
                        if !self.active() {
                            self.queue.clear();
                        }
                        vec![]
                    }
                    (3, FUNCTION_SWITCH_ID) => {
                        let switches = data.get(1).copied().unwrap_or(0b11);
                        self.surface = switches & 1 != 0;
                        self.buttons = switches & 2 != 0;
                        vec![]
                    }
                    _ => return None,
                },
                _ => return hid_class(setup),
            },
        };
        Some(reply)
    }
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>> {
        (endpoint == ENDPOINT)
            .then(|| self.queue.pop_front().map(|r| r.to_vec()))
            .flatten()
    }
    fn interrupt_ready(&self, endpoint: u8) -> bool {
        endpoint == ENDPOINT && !self.queue.is_empty()
    }
}

enum Standard {
    Reply(Vec<u8>),
    Stall,
    /// A HID class request for the device to answer.
    Class,
}

/// The standard USB requests of a one-interface HID device whose interrupt IN endpoint 0x81
/// carries reports of up to `packet` bytes, polled every millisecond. `strings` are the
/// manufacturer, product and serial.
fn standard(
    setup: &Setup,
    product: u16,
    report: &[u8],
    packet: u8,
    strings: &[&str; 3],
) -> Standard {
    let hid = {
        let [ll, lh] = (report.len() as u16).to_le_bytes();
        [9, 0x21, 0x11, 0x01, 0, 1, 0x22, ll, lh]
    };
    Standard::Reply(match (setup.request_type, setup.request) {
        // GET_STATUS for the device, an interface or an endpoint.
        (0x80..=0x82, 0x00) => vec![0, 0],
        // CLEAR_FEATURE, SET_FEATURE, SET_ADDRESS, SET_CONFIGURATION, SET_INTERFACE.
        (0x00..=0x02, 0x01 | 0x03) | (0x00, 0x05 | 0x09) | (0x01, 0x0b) => vec![],
        (0x80, 0x08) => vec![1],
        (0x81, 0x0a) => vec![0],
        (0x80, 0x06) => match (setup.value >> 8, setup.value as u8) {
            (1, _) => {
                let [vl, vh] = VENDOR.to_le_bytes();
                let [pl, ph] = product.to_le_bytes();
                // USB 2.0, class per interface, 64-byte EP0, release 1.00, strings 1-3, one
                // configuration.
                vec![
                    18, 1, 0x00, 0x02, 0, 0, 0, 64, vl, vh, pl, ph, 0x00, 0x01, 1, 2, 3, 1,
                ]
            }
            (2, _) => {
                let mut d = vec![9, 2, 0, 0, 1, 1, 0, 0xa0, 50];
                d.extend_from_slice(&[9, 4, 0, 0, 1, 3, 0, 0, 0]);
                d.extend_from_slice(&hid);
                d.extend_from_slice(&[7, 5, 0x80 | ENDPOINT, 3, packet, 0, 1]);
                let total = (d.len() as u16).to_le_bytes();
                d[2..4].copy_from_slice(&total);
                d
            }
            (3, 0) => vec![4, 3, 0x09, 0x04],
            (3, n @ 1..=3) => string_descriptor(strings[usize::from(n) - 1]),
            // No other strings, no device qualifier (a full-speed device), no BOS (USB 2.0).
            _ => return Standard::Stall,
        },
        (0x81, 0x06) => match setup.value >> 8 {
            0x21 => hid.to_vec(),
            0x22 => report.to_vec(),
            _ => return Standard::Stall,
        },
        _ => return Standard::Class,
    })
}

/// GET_IDLE, GET_PROTOCOL (report protocol), SET_IDLE and SET_PROTOCOL; anything else stalls.
fn hid_class(setup: &Setup) -> Option<Vec<u8>> {
    match (setup.request_type, setup.request) {
        (0xa1, 0x02) => Some(vec![0]),
        (0xa1, 0x03) => Some(vec![1]),
        (0x21, 0x0a | 0x0b) => Some(vec![]),
        _ => None,
    }
}

fn string_descriptor(s: &str) -> Vec<u8> {
    let mut d = vec![0, 3];
    for unit in s.encode_utf16().take(126) {
        d.extend_from_slice(&unit.to_le_bytes());
    }
    d[0] = d.len() as u8;
    d
}

#[cfg(test)]
mod tests;
