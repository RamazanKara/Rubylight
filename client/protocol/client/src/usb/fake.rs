//! A USB device in memory, for tests: standard requests on the control endpoint, a bulk
//! loopback, an interrupt endpoint that reports what the test pushes, and an isochronous IN
//! endpoint.
//!
//! | Endpoint | Type | Does |
//! |---|---|---|
//! | 0 | control | GET_DESCRIPTOR (device, configuration, strings), GET/SET_CONFIGURATION, SET_INTERFACE, GET_STATUS, CLEAR_FEATURE; vendor request 1 stores OUT data and reads it back IN; anything else stalls |
//! | 0x01 | bulk OUT | appends to the loopback |
//! | 0x81 | bulk IN | reads the loopback, waiting for data |
//! | 0x82 | interrupt IN | the next pushed report, waiting for one |
//! | 0x83 | isochronous IN (alternate setting 1) | packet `i` carries up to 4 bytes of value `i` |

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rubylight_protocol::usbip::{self as wire, IsoPacket};

use super::blocking::BlockingDevice;
use super::server::{Direction, Reply, Transfer};

/// pid.codes' test vendor and product.
pub const VENDOR: u16 = 0x1209;
pub const PRODUCT: u16 = 0x0001;
pub const BULK: u8 = 1;
pub const INTERRUPT: u8 = 2;
pub const ISOCHRONOUS: u8 = 3;
/// The vendor request that stores and reads back data on the control endpoint.
pub const VENDOR_REQUEST: u8 = 1;
pub const MANUFACTURER: &str = "Rubylight";
pub const PRODUCT_NAME: &str = "Fake USB device";

/// How long a waiting transfer sleeps before it looks at its cancel flag again.
const SLICE: Duration = Duration::from_millis(20);

pub const DEVICE_DESCRIPTOR: [u8; 18] = [
    18, 1, 0x00, 0x02, 0, 0, 0, 64, 0x09, 0x12, 0x01, 0x00, 0x00, 0x01, 1, 2, 0, 1,
];

const fn endpoint(address: u8, attributes: u8, size: u16, interval: u8) -> [u8; 7] {
    [
        7,
        5,
        address,
        attributes,
        size as u8,
        (size >> 8) as u8,
        interval,
    ]
}

/// Configuration 1: interface 0, alternate settings 0 (bulk and interrupt) and 1 (and the
/// isochronous endpoint).
pub fn configuration_descriptor() -> Vec<u8> {
    let bulk_out = endpoint(BULK, 2, 512, 0);
    let bulk_in = endpoint(0x80 | BULK, 2, 512, 0);
    let interrupt = endpoint(0x80 | INTERRUPT, 3, 8, 1);
    let iso = endpoint(0x80 | ISOCHRONOUS, 1, 64, 1);
    let mut out = vec![9, 2, 0, 0, 1, 1, 0, 0x80, 50];
    out.extend_from_slice(&[9, 4, 0, 0, 3, 0xff, 0, 0, 0]);
    for ep in [bulk_out, bulk_in, interrupt] {
        out.extend_from_slice(&ep);
    }
    out.extend_from_slice(&[9, 4, 0, 1, 4, 0xff, 0, 0, 0]);
    for ep in [bulk_out, bulk_in, interrupt, iso] {
        out.extend_from_slice(&ep);
    }
    let total = out.len() as u16;
    out[2..4].copy_from_slice(&total.to_le_bytes());
    out
}

fn string_descriptor(text: &str) -> Vec<u8> {
    let mut out = vec![0, 3];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out[0] = out.len() as u8;
    out
}

#[derive(Default)]
struct State {
    loopback: VecDeque<u8>,
    reports: VecDeque<Vec<u8>>,
    vendor: Vec<u8>,
    configuration: u8,
    alternate: u8,
    /// Endpoint, direction and length of every transfer that ran, in order.
    seen: Vec<(u8, Direction, u32)>,
    closes: u32,
    unplugged: bool,
}

struct Fake {
    busid: String,
    speed: u32,
    state: Mutex<State>,
    changed: Condvar,
}

/// The fake. Clones share it, so a test keeps one while the forwarder serves another.
#[derive(Clone)]
pub struct FakeDevice {
    inner: Arc<Fake>,
}

impl FakeDevice {
    /// A high-speed device at `busid`.
    pub fn new(busid: &str) -> Self {
        Self {
            inner: Arc::new(Fake {
                busid: busid.to_owned(),
                speed: 3,
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues an interrupt report.
    pub fn push_report(&self, report: Vec<u8>) {
        self.state().reports.push_back(report);
        self.inner.changed.notify_all();
    }

    /// Unplugs it: every transfer from now fails with `ENODEV`.
    pub fn unplug(&self) {
        self.state().unplugged = true;
        self.inner.changed.notify_all();
    }

    /// How often the attachment let go of it.
    pub fn closes(&self) -> u32 {
        self.state().closes
    }

    pub fn configuration(&self) -> u8 {
        self.state().configuration
    }

    /// Endpoint, direction and length of every transfer that ran, in order.
    pub fn seen(&self) -> Vec<(u8, Direction, u32)> {
        self.state().seen.clone()
    }

    /// Waits for data or a report until `cancelled`. A cancelled transfer takes nothing, so
    /// what arrives after an unlink goes to the next one.
    fn wait<T>(
        &self,
        cancelled: &AtomicBool,
        mut take: impl FnMut(&mut State) -> Option<T>,
    ) -> Result<T, i32> {
        let mut state = self.state();
        loop {
            if state.unplugged {
                return Err(wire::ENODEV);
            }
            if cancelled.load(Ordering::SeqCst) {
                return Err(wire::ECONNRESET);
            }
            if let Some(value) = take(&mut state) {
                return Ok(value);
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, SLICE)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn control(&self, transfer: &Transfer) -> Reply {
        let setup = transfer.setup;
        let (request_type, request) = (setup[0], setup[1]);
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let length = usize::from(u16::from_le_bytes([setup[6], setup[7]]));
        let read = |data: Vec<u8>| {
            let mut data = data;
            data.truncate(length.min(transfer.length as usize));
            Reply::read(data)
        };
        let mut state = self.state();
        match (request_type, request) {
            // GET_STATUS
            (0x80, 0) => read(vec![0, 0]),
            // CLEAR_FEATURE (endpoint halt)
            (0x02, 1) => Reply::sent(0),
            // GET_DESCRIPTOR
            (0x80, 6) => match ((value >> 8) as u8, value as u8) {
                (1, _) => read(DEVICE_DESCRIPTOR.to_vec()),
                (2, 0) => read(configuration_descriptor()),
                (3, 0) => read(vec![4, 3, 0x09, 0x04]),
                (3, 1) => read(string_descriptor(MANUFACTURER)),
                (3, 2) => read(string_descriptor(PRODUCT_NAME)),
                _ => Reply::failed(wire::EPIPE),
            },
            // GET_CONFIGURATION
            (0x80, 8) => read(vec![state.configuration]),
            // SET_CONFIGURATION
            (0x00, 9) if value <= 1 => {
                state.configuration = value as u8;
                Reply::sent(0)
            }
            // SET_INTERFACE
            (0x01, 11) if value <= 1 => {
                state.alternate = value as u8;
                Reply::sent(0)
            }
            (0x40, VENDOR_REQUEST) => {
                state.vendor = transfer.data.clone();
                Reply::sent(transfer.data.len() as u32)
            }
            (0xc0, VENDOR_REQUEST) => read(state.vendor.clone()),
            _ => Reply::failed(wire::EPIPE),
        }
    }

    fn isochronous(&self, transfer: &Transfer) -> Reply {
        let mut data = Vec::new();
        let iso = transfer
            .iso
            .iter()
            .enumerate()
            .map(|(at, packet)| {
                let actual = packet.length.min(4);
                data.extend(std::iter::repeat(at as u8).take(actual as usize));
                IsoPacket {
                    actual_length: actual,
                    status: 0,
                    ..*packet
                }
            })
            .collect();
        Reply {
            data,
            start_frame: transfer.start_frame,
            iso,
            ..Reply::default()
        }
    }
}

impl BlockingDevice for FakeDevice {
    fn describe(&self) -> wire::Device {
        wire::Device {
            path: format!("/fake/{}", self.inner.busid),
            busid: self.inner.busid.clone(),
            busnum: 1,
            devnum: 2,
            speed: self.inner.speed,
            vendor: VENDOR,
            product: PRODUCT,
            bcd_device: 0x0100,
            class: 0,
            subclass: 0,
            protocol: 0,
            configuration: self.configuration(),
            configurations: 1,
            interfaces: 1,
        }
    }

    fn class(&self) -> u8 {
        0xff
    }

    fn transfer(&self, transfer: &Transfer, cancelled: &AtomicBool) -> Reply {
        {
            let mut state = self.state();
            state
                .seen
                .push((transfer.endpoint, transfer.direction, transfer.length));
            if state.unplugged {
                return Reply::failed(wire::ENODEV);
            }
        }
        let limit = transfer.length as usize;
        let result = match (transfer.endpoint, transfer.direction) {
            (0, _) => return self.control(transfer),
            (BULK, Direction::Out) => {
                self.state().loopback.extend(&transfer.data);
                self.inner.changed.notify_all();
                return Reply::sent(transfer.data.len() as u32);
            }
            (BULK, Direction::In) => self.wait(cancelled, |state| {
                let n = limit.min(state.loopback.len());
                (n > 0).then(|| state.loopback.drain(..n).collect())
            }),
            (INTERRUPT, Direction::In) => self.wait(cancelled, |state| {
                state.reports.pop_front().map(|mut report| {
                    report.truncate(limit);
                    report
                })
            }),
            (ISOCHRONOUS, Direction::In) if transfer.is_isochronous() => {
                return self.isochronous(transfer)
            }
            _ => Err(wire::EPIPE),
        };
        match result {
            Ok(data) => Reply::read(data),
            Err(status) => Reply::failed(status),
        }
    }

    fn close(&self) {
        self.state().closes += 1;
        self.inner.changed.notify_all();
    }
}
