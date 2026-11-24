//! Tests and in-memory fake kernel for [`super::UsbfsDevice`].

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use rubylight_protocol::usbip::{self as wire, Header, IsoPacket};

use super::*;
use crate::usb::serve;
use crate::usb::server::tests::{pipe, setup, Driver};

#[path = "usbfs_tests/open_tests.rs"]
mod open;
#[path = "usbfs_tests/transfer_tests.rs"]
mod transfer;

const PATH: &str = "/dev/bus/usb/001/005";
const EBADF: Errno = Errno(9);
const ENOENT: Errno = Errno(2);
const EXDEV: i32 = -18;

fn interface(number: u8, alternate: u8, endpoints: u8, class: u8) -> [u8; 9] {
    [9, 4, number, alternate, endpoints, class, 0, 0, 0]
}

fn endpoint(address: u8, attributes: u8) -> [u8; 7] {
    [7, 5, address, attributes, 64, 0, 1]
}

fn configuration(value: u8, interfaces: u8, body: &[u8]) -> Vec<u8> {
    let total = (9 + body.len()) as u16;
    let mut out = vec![
        9,
        2,
        total as u8,
        (total >> 8) as u8,
        interfaces,
        value,
        0,
        0x80,
        50,
    ];
    out.extend_from_slice(body);
    out
}

/// A vendor device (class 0 on the device, 0xff on interface 0) with a HID interface 1 a
/// kernel driver has, and a second configuration with one interface.
fn descriptors() -> Vec<u8> {
    let mut raw = vec![
        18, 1, 0x00, 0x02, 0, 0, 0, 64, 0x09, 0x12, 0x01, 0x00, 0x00, 0x01, 1, 2, 0, 2,
    ];
    let mut first = Vec::new();
    first.extend_from_slice(&interface(0, 0, 3, 0xff));
    first.extend_from_slice(&endpoint(0x01, 2));
    first.extend_from_slice(&endpoint(0x81, 2));
    first.extend_from_slice(&endpoint(0x82, 3));
    first.extend_from_slice(&interface(0, 1, 2, 0xff));
    first.extend_from_slice(&endpoint(0x83, 1));
    first.extend_from_slice(&endpoint(0x03, 1));
    first.extend_from_slice(&interface(1, 0, 0, 3));
    raw.extend(configuration(1, 2, &first));
    raw.extend(configuration(2, 1, &interface(0, 0, 0, 0xff)));
    raw
}

#[derive(Default)]
struct Machine {
    log: Vec<String>,
    /// Submitted URBs the device has not answered, by address.
    parked: Vec<usize>,
    completed: VecDeque<usize>,
    loopback: Vec<u8>,
    stored: Vec<u8>,
    iso_out: Vec<u8>,
    reports: VecDeque<Vec<u8>>,
    /// Interfaces a kernel driver has.
    drivers: Vec<u8>,
    claimed: Vec<u8>,
    alternate: u8,
    speed: u32,
    /// What USBDEVFS_GET_SPEED fails with, on a kernel before 4.13.
    no_speed: Option<Errno>,
    slow: bool,
    qualifier: bool,
    /// Replaces [`descriptors`].
    raw: Option<Vec<u8>>,
    gone: bool,
    closed: bool,
    /// The interface whose claim fails outright.
    refuse: Option<u8>,
}

/// usbfs in memory: answers URBs as the kernel would, writing through their pointers.
#[derive(Clone)]
struct FakeKernel {
    machine: Arc<Mutex<Machine>>,
    changed: Arc<Condvar>,
}

impl FakeKernel {
    fn new() -> Self {
        Self {
            machine: Arc::new(Mutex::new(Machine {
                drivers: vec![1],
                speed: 3,
                ..Machine::default()
            })),
            changed: Arc::new(Condvar::new()),
        }
    }

    fn machine(&self) -> MutexGuard<'_, Machine> {
        self.machine.lock().unwrap()
    }

    fn log(&self) -> Vec<String> {
        self.machine().log.clone()
    }

    fn note(machine: &mut Machine, entry: String) {
        machine.log.push(entry);
    }

    fn finish(&self, machine: &mut Machine, urb: *mut Urb, status: i32, actual: usize) {
        unsafe {
            (*urb).status = status;
            (*urb).actual_length = actual as i32;
        }
        machine.completed.push_back(urb as usize);
        self.changed.notify_all();
    }

    fn push_report(&self, report: &[u8]) {
        let mut machine = self.machine();
        machine.reports.push_back(report.to_vec());
        self.serve_parked(&mut machine);
    }

    fn unplug(&self) {
        let mut machine = self.machine();
        machine.gone = true;
        for urb in std::mem::take(&mut machine.parked) {
            self.finish(&mut machine, urb as *mut Urb, -108, 0);
        }
    }

    fn wait_parked(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.machine().parked.len() < count {
            assert!(
                Instant::now() < deadline,
                "the URB never reached the kernel"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Answers parked IN URBs that data has come for.
    fn serve_parked(&self, machine: &mut Machine) {
        for urb in std::mem::take(&mut machine.parked) {
            if !self.run_in(machine, urb as *mut Urb) {
                machine.parked.push(urb);
            }
        }
    }

    fn buffer(urb: *mut Urb) -> &'static mut [u8] {
        unsafe { std::slice::from_raw_parts_mut((*urb).buffer, (*urb).buffer_length as usize) }
    }

    /// Runs a bulk or interrupt IN; false when it waits for data.
    fn run_in(&self, machine: &mut Machine, urb: *mut Urb) -> bool {
        let buffer = Self::buffer(urb);
        let source = match unsafe { (*urb).endpoint } {
            0x81 if !machine.loopback.is_empty() => {
                let n = buffer.len().min(machine.loopback.len());
                machine.loopback.drain(..n).collect::<Vec<u8>>()
            }
            0x82 => match machine.reports.pop_front() {
                Some(report) => report,
                None => return false,
            },
            _ => return false,
        };
        let n = source.len().min(buffer.len());
        buffer[..n].copy_from_slice(&source[..n]);
        self.finish(machine, urb, 0, n);
        true
    }

    fn control(&self, machine: &mut Machine, urb: *mut Urb) {
        let buffer = Self::buffer(urb);
        let (head, data) = buffer.split_at_mut(8);
        let length = usize::from(u16::from_le_bytes([head[6], head[7]]));
        match (head[0], head[1], head[3]) {
            (0x80, 6, 1) => {
                let device = &descriptors()[..18];
                let n = length.min(18);
                data[..n].copy_from_slice(&device[..n]);
                self.finish(machine, urb, 0, n);
            }
            (0x40, 1, _) => {
                machine.stored = data[..length].to_vec();
                self.finish(machine, urb, 0, length);
            }
            (0xc0, 1, _) => {
                let n = length.min(machine.stored.len());
                data[..n].copy_from_slice(&machine.stored[..n]);
                self.finish(machine, urb, 0, n);
            }
            _ => self.finish(machine, urb, wire::EPIPE, 0),
        }
    }

    fn iso(&self, machine: &mut Machine, urb: *mut Urb) {
        let packets = unsafe { (*urb).number_of_packets } as usize;
        let frames = unsafe { std::slice::from_raw_parts_mut(frames_of(urb), packets) };
        let buffer = Self::buffer(urb);
        let mut at = 0;
        let mut errors = 0;
        for (i, frame) in frames.iter_mut().enumerate() {
            let slot = &mut buffer[at..at + frame.length as usize];
            if unsafe { (*urb).endpoint } == 0x03 {
                machine.iso_out.extend_from_slice(slot);
                frame.actual_length = frame.length;
            } else if i == 2 {
                frame.actual_length = 0;
                frame.status = EXDEV as u32;
                errors += 1;
            } else {
                let n = slot.len().min(3);
                slot[..n].fill(i as u8);
                frame.actual_length = n as u32;
            }
            at += frame.length as usize;
        }
        unsafe { (*urb).error_count = errors };
        self.finish(machine, urb, 0, 0);
    }
}

impl Kernel for FakeKernel {
    fn descriptors(&self) -> Result<Vec<u8>, Errno> {
        Ok(self.machine().raw.clone().unwrap_or_else(descriptors))
    }

    fn configuration(&self) -> Result<u8, Errno> {
        Ok(1)
    }

    fn speed(&self) -> Result<u32, Errno> {
        let machine = self.machine();
        machine.no_speed.map_or(Ok(machine.speed), Err)
    }

    fn low_speed(&self) -> Result<bool, Errno> {
        Ok(self.machine().slow)
    }

    fn control_in(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        _: u16,
        data: &mut [u8],
    ) -> Result<usize, Errno> {
        // Only the device qualifier is asked for this way; a full-speed device stalls it.
        assert_eq!((request_type, request, value), (0x80, 6, 0x0600));
        if !self.machine().qualifier {
            return Err(Errno(32));
        }
        data.copy_from_slice(&[10, 6, 0x00, 0x02, 0, 0, 0, 64, 1, 0]);
        Ok(10)
    }

    fn claim(&self, interface: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, format!("claim {interface}"));
        if machine.refuse == Some(interface) {
            return Err(Errno(1));
        }
        if machine.drivers.contains(&interface) {
            return Err(EBUSY);
        }
        machine.claimed.push(interface);
        Ok(())
    }

    fn release(&self, interface: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, format!("release {interface}"));
        machine.claimed.retain(|i| *i != interface);
        Ok(())
    }

    fn disconnect_driver(&self, interface: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, format!("disconnect {interface}"));
        machine.drivers.retain(|i| *i != interface);
        Ok(())
    }

    fn connect_driver(&self, interface: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, format!("connect {interface}"));
        machine.drivers.push(interface);
        Ok(())
    }

    fn set_configuration(&self, value: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, format!("set configuration {value}"));
        if !machine.claimed.is_empty() {
            return Err(EBUSY);
        }
        Ok(())
    }

    fn set_interface(&self, interface: u8, alternate: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(
            &mut machine,
            format!("set interface {interface} {alternate}"),
        );
        machine.alternate = alternate;
        Ok(())
    }

    fn clear_halt(&self, endpoint: u8) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, format!("clear halt {endpoint:#04x}"));
        Ok(())
    }

    fn reset(&self) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, "reset".into());
        // usbfs loses its interfaces.
        machine.claimed.clear();
        Ok(())
    }

    unsafe fn submit(&self, urb: *mut Urb) -> Result<(), Errno> {
        let mut machine = self.machine();
        // SAFETY: caller guarantees `urb` points to a valid `Urb`.
        let (kind, address) = unsafe { ((*urb).kind, (*urb).endpoint) };
        Self::note(&mut machine, format!("submit {kind} {address:#04x}"));
        if machine.closed {
            return Err(EBADF);
        }
        if machine.gone {
            return Err(ENODEV);
        }
        match (kind, address) {
            (URB_CONTROL, 0) => self.control(&mut machine, urb),
            (URB_BULK, 0x01) => {
                let data = Self::buffer(urb).to_vec();
                machine.loopback.extend_from_slice(&data);
                self.finish(&mut machine, urb, 0, data.len());
                self.serve_parked(&mut machine);
            }
            (URB_BULK, 0x81) | (URB_INTERRUPT, 0x82) => {
                if !self.run_in(&mut machine, urb) {
                    machine.parked.push(urb as usize);
                }
            }
            (URB_ISO, 0x83 | 0x03) if machine.alternate == 1 => self.iso(&mut machine, urb),
            _ => return Err(ENOENT),
        }
        Ok(())
    }

    fn discard(&self, urb: *mut Urb) -> Result<(), Errno> {
        let mut machine = self.machine();
        Self::note(&mut machine, "discard".into());
        let Some(at) = machine.parked.iter().position(|u| *u == urb as usize) else {
            return Err(EINVAL);
        };
        machine.parked.remove(at);
        self.finish(&mut machine, urb, -2, 0);
        Ok(())
    }

    fn reap(&self) -> Result<Option<*mut Urb>, Errno> {
        let mut machine = self.machine();
        if machine.closed {
            return Err(EBADF);
        }
        match machine.completed.pop_front() {
            Some(urb) => Ok(Some(urb as *mut Urb)),
            None if machine.gone => Err(ENODEV),
            None => Ok(None),
        }
    }

    fn wait(&self, timeout: Duration) -> Result<Ready, Errno> {
        let machine = self.machine();
        let (machine, _) = self
            .changed
            .wait_timeout_while(machine, timeout, |m| {
                m.completed.is_empty() && !m.gone && !m.closed
            })
            .unwrap();
        if machine.closed {
            Err(EBADF)
        } else if !machine.completed.is_empty() {
            Ok(Ready::Completed)
        } else if machine.gone {
            Ok(Ready::Gone)
        } else {
            Ok(Ready::Idle)
        }
    }

    fn close(&self) {
        let mut machine = self.machine();
        Self::note(&mut machine, "close".into());
        machine.closed = true;
        self.changed.notify_all();
    }
}

fn status_of(header: &Header) -> (u32, i32) {
    match *header {
        Header::RetSubmit { base, status, .. } | Header::RetUnlink { base, status } => {
            (base.seqnum, status)
        }
        _ => panic!("not a reply: {header:?}"),
    }
}

fn serving(
    kernel: &FakeKernel,
) -> (
    Driver,
    std::thread::JoinHandle<Result<(), crate::usb::ServeError>>,
) {
    let device = Arc::new(UsbfsDevice::open(kernel.clone(), PATH).unwrap());
    let (tunnel, mut driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, device));
    driver.import("1-5").unwrap();
    (driver, served)
}

fn with_index(mut setup: [u8; 8], index: u16) -> [u8; 8] {
    setup[4..6].copy_from_slice(&index.to_le_bytes());
    setup
}

fn submits(kernel: &FakeKernel) -> Vec<String> {
    kernel
        .log()
        .into_iter()
        .filter(|e| e.starts_with("submit"))
        .collect()
}
