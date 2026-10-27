use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use rubylight_protocol::usbip::{self as wire, Header, IsoPacket};

use super::*;
use crate::usb::serve;
use crate::usb::server::tests::{pipe, setup, Driver};

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
        Ok(descriptors())
    }

    fn configuration(&self) -> Result<u8, Errno> {
        Ok(1)
    }

    fn speed(&self) -> Result<u32, Errno> {
        Ok(self.machine().speed)
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
        let (kind, address) = ((*urb).kind, (*urb).endpoint);
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

#[test]
fn the_descriptors_name_the_device_its_configurations_and_interfaces() {
    let parsed = Descriptors::parse(&descriptors()).unwrap();
    assert_eq!(
        (parsed.vendor, parsed.product, parsed.bcd_device),
        (0x1209, 0x0001, 0x0100)
    );
    assert_eq!(parsed.configurations.len(), 2);
    let first = parsed.configuration(1).unwrap();
    assert_eq!(
        first.interfaces,
        vec![
            Interface {
                number: 0,
                class: 0xff
            },
            Interface {
                number: 1,
                class: 3
            }
        ]
    );
    assert_eq!(
        first.endpoints,
        vec![(0x01, 2), (0x81, 2), (0x82, 3), (0x83, 1), (0x03, 1)]
    );
    // Cut anywhere inside, they fail rather than describe half a device.
    let raw = descriptors();
    for cut in [10, 20, 30, raw.len() - 3] {
        assert!(Descriptors::parse(&raw[..cut]).is_err(), "cut at {cut}");
    }
    let mut zero = raw.clone();
    zero[18 + 9] = 0;
    assert!(Descriptors::parse(&zero).is_err());
    assert_eq!(bus_and_device(PATH), Some((1, 5)));
    assert_eq!(bus_and_device("/dev/bus/usb/x/5"), None);
}

#[test]
fn opening_claims_every_interface_and_closing_gives_them_back_to_their_drivers() {
    let kernel = FakeKernel::new();
    let device = UsbfsDevice::open(kernel.clone(), PATH).unwrap();
    let described = device.describe();
    assert_eq!(
        described,
        wire::Device {
            path: PATH.into(),
            busid: "1-5".into(),
            busnum: 1,
            devnum: 5,
            speed: 3,
            vendor: 0x1209,
            product: 0x0001,
            bcd_device: 0x0100,
            class: 0,
            subclass: 0,
            protocol: 0,
            configuration: 1,
            configurations: 2,
            interfaces: 2,
        }
    );
    // Class 0 on the device: the first interface's.
    assert_eq!(device.class(), 0xff);
    assert_eq!(
        kernel.log(),
        ["claim 0", "claim 1", "disconnect 1", "claim 1"]
    );
    device.close();
    device.close();
    assert_eq!(
        kernel.log()[4..],
        ["release 0", "release 1", "connect 1", "close"]
    );
}

#[test]
fn a_device_that_cannot_be_opened_fails_loudly_and_closes() {
    let kernel = FakeKernel::new();
    kernel.machine().speed = 0;
    let error = UsbfsDevice::open(kernel.clone(), PATH).err().unwrap();
    assert!(error.contains("speed"), "{error}");
    assert_eq!(kernel.log(), ["close"]);

    let kernel = FakeKernel::new();
    let error = UsbfsDevice::open(kernel.clone(), "/proc/self/fd/9")
        .err()
        .unwrap();
    assert!(error.contains("not a USB device node"), "{error}");

    let kernel = FakeKernel::new();
    kernel.machine().refuse = Some(1);
    let error = UsbfsDevice::open(kernel.clone(), PATH).err().unwrap();
    assert!(error.contains("interface 1"), "{error}");
    assert_eq!(kernel.log(), ["claim 0", "claim 1", "release 0", "close"]);
}

#[test]
fn control_bulk_and_interrupt_transfers_run_as_urbs_of_their_type() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);

    let descriptor = driver.submit(0, wire::DIR_IN, 18, setup(0x80, 6, 0x0100, 18), &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!(
        (status_of(&header), data.as_slice()),
        ((descriptor, 0), &descriptors()[..18])
    );

    let stored = driver.submit(0, wire::DIR_OUT, 3, setup(0x40, 1, 0, 3), &[1, 2, 3], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (stored, 0));
    let read = driver.submit(0, wire::DIR_IN, 8, setup(0xc0, 1, 0, 8), &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((read, 0), vec![1, 2, 3]));

    let stall = driver.submit(0, wire::DIR_IN, 2, setup(0x80, 0x55, 0, 2), &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (stall, wire::EPIPE));

    // A bulk IN waits in the kernel while the OUT that feeds it runs beside it.
    let waiting = driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    let sent = driver.submit(1, wire::DIR_OUT, 5, [0; 8], b"hello", &[]);
    // The OUT completes first and feeds the IN.
    assert!(
        matches!(driver.reply(false).0, Header::RetSubmit { base, length: 5, .. } if base.seqnum == sent)
    );
    let (header, data, _) = driver.reply(true);
    assert_eq!(
        (status_of(&header), data.as_slice()),
        ((waiting, 0), &b"hello"[..])
    );

    let report = driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    kernel.push_report(&[7, 7]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((report, 0), vec![7, 7]));

    let unknown = driver.submit(4, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (unknown, -2));

    assert_eq!(
        submits(&kernel),
        [
            "submit 2 0x00",
            "submit 2 0x00",
            "submit 2 0x00",
            "submit 2 0x00",
            "submit 3 0x81",
            "submit 3 0x01",
            "submit 1 0x82",
            "submit 3 0x84",
        ]
    );
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn an_unlinked_urb_is_discarded_and_its_completion_dropped() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let waiting = driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    let unlink = driver.unlink(waiting);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    // The answer goes out before the device hears of it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !kernel.log().contains(&"discard".to_string()) {
        assert!(Instant::now() < deadline, "the URB was never discarded");
        std::thread::sleep(Duration::from_millis(2));
    }
    // The next report goes to the next transfer; nothing answers the discarded one.
    let next = driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    kernel.push_report(&[9]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((next, 0), vec![9]));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn set_interface_goes_through_its_ioctl_and_isochronous_packets_pack_both_ways() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let alternate = driver.submit(0, wire::DIR_OUT, 0, setup(0x01, 11, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (alternate, 0));
    assert!(kernel.log().contains(&"set interface 0 1".to_string()));

    let asked: Vec<IsoPacket> = (0..4)
        .map(|i| IsoPacket {
            offset: i * 8,
            length: 8,
            actual_length: 0,
            status: 0,
        })
        .collect();
    let iso = driver.submit(3, wire::DIR_IN, 32, [0; 8], &[], &asked);
    let (header, data, packets) = driver.reply(true);
    assert_eq!(status_of(&header), (iso, 0));
    assert!(matches!(
        header,
        Header::RetSubmit {
            errors: 1,
            length: 9,
            ..
        }
    ));
    assert_eq!(data, [0, 0, 0, 1, 1, 1, 3, 3, 3]);
    let got: Vec<(u32, u32, u32, i32)> = packets
        .iter()
        .map(|p| (p.offset, p.length, p.actual_length, p.status))
        .collect();
    assert_eq!(
        got,
        [(0, 8, 3, 0), (8, 8, 3, 0), (16, 8, 0, EXDEV), (24, 8, 3, 0)]
    );

    // OUT packets sit at their offsets in USB/IP and back to back in usbfs.
    let out = [
        IsoPacket {
            offset: 0,
            length: 3,
            actual_length: 0,
            status: 0,
        },
        IsoPacket {
            offset: 8,
            length: 3,
            actual_length: 0,
            status: 0,
        },
    ];
    let data: Vec<u8> = (0..16).collect();
    let sent = driver.submit(3, wire::DIR_OUT, 16, [0; 8], &data, &out);
    let (header, _, packets) = driver.reply(false);
    assert_eq!(status_of(&header), (sent, 0));
    assert!(matches!(header, Header::RetSubmit { length: 6, .. }));
    assert_eq!(packets.len(), 2);
    assert_eq!(kernel.machine().iso_out, [0, 1, 2, 8, 9, 10]);
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn configuration_halts_and_resets_change_the_kernels_state_not_the_devices() {
    let kernel = FakeKernel::new();
    let device = Arc::new(UsbfsDevice::open(kernel.clone(), PATH).unwrap());
    let (tunnel, mut driver) = pipe();
    let serving = device.clone();
    let served = std::thread::spawn(move || serve(tunnel, serving));
    driver.import("1-5").unwrap();
    let opened = kernel.log().len();

    // The configuration Android chose already: nothing to do.
    let same = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (same, 0));
    assert_eq!(kernel.log().len(), opened);

    let halt = driver.submit(
        0,
        wire::DIR_OUT,
        0,
        with_index(setup(0x02, 1, 0, 0), 0x81),
        &[],
        &[],
    );
    assert_eq!(status_of(&driver.reply(false).0), (halt, 0));

    let other = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 2, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (other, 0));
    assert_eq!(
        (
            device.describe().configuration,
            device.describe().interfaces
        ),
        (2, 1)
    );

    let reset = driver.submit(
        0,
        wire::DIR_OUT,
        0,
        with_index(setup(0x23, 3, 4, 0), 1),
        &[],
        &[],
    );
    assert_eq!(status_of(&driver.reply(false).0), (reset, 0));
    assert_eq!(
        kernel.log()[opened..],
        [
            "clear halt 0x81",
            "release 0",
            "release 1",
            "set configuration 2",
            "claim 0",
            "reset",
            "claim 0",
        ]
    );
    assert!(submits(&kernel).is_empty());
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn an_unplugged_device_fails_what_it_had_and_everything_after() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    let waiting = driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    kernel.wait_parked(1);
    kernel.unplug();
    let (seqnum, status) = status_of(&driver.reply(true).0);
    assert_eq!(seqnum, waiting);
    assert!(status == -108 || status == wire::ENODEV, "{status}");
    // Once the reaper saw it go, nothing reaches the kernel.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let before = submits(&kernel).len();
        let after = driver.submit(1, wire::DIR_OUT, 1, [0; 8], &[1], &[]);
        let (seqnum, status) = status_of(&driver.reply(false).0);
        assert_eq!(seqnum, after);
        assert_eq!(status, wire::ENODEV);
        if submits(&kernel).len() == before {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn closing_discards_what_runs_before_the_descriptor_closes() {
    let kernel = FakeKernel::new();
    let (mut driver, served) = serving(&kernel);
    driver.submit(1, wire::DIR_IN, 64, [0; 8], &[], &[]);
    driver.submit(2, wire::DIR_IN, 8, [0; 8], &[], &[]);
    kernel.wait_parked(2);
    // The host closes the tunnel: serving ends and closes the device.
    drop(driver);
    served.join().unwrap().unwrap();
    let log = kernel.log();
    let close = log.iter().position(|e| e == "close").unwrap();
    assert_eq!(
        log[close - 5..close],
        ["discard", "discard", "release 0", "release 1", "connect 1"]
    );
    assert!(kernel.machine().parked.is_empty());
}
