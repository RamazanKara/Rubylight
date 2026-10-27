//! A device under Linux's usbfs, the stack below Android's `UsbDeviceConnection`: every
//! transfer is a URB on the device's file descriptor, so all endpoints run at once and
//! isochronous ones work, which Java's `UsbRequest` cannot do. Submits return at once; one
//! thread per device waits for completions (`poll`), reaps them and answers the host.
//!
//! The ioctls sit behind [`Kernel`]: [`linux::Usbfs`] on Android and Linux, a fake kernel in
//! the tests, so everything above them runs on any host.
//!
//! The host's driver configures the device as if it were plugged in there. Most of that goes
//! to the device as it is; four standard requests change state the kernel keeps and go
//! through their ioctls instead, as Linux's own USB/IP host does: SET_CONFIGURATION,
//! SET_INTERFACE, CLEAR_FEATURE(ENDPOINT_HALT) and a port reset.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rubylight_protocol::usbip::{self as wire, IsoPacket};

use super::server::{Completion, Direction, Reply, Transfer, UsbDevice};

// Only the Linux backend uses it outside its test.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
mod abi;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod linux;

/// A Linux errno, positive, from an ioctl. Linux's values on every host, so the tests run
/// anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(pub i32);

pub const EINTR: Errno = Errno(4);
pub const EAGAIN: Errno = Errno(11);
pub const EBUSY: Errno = Errno(16);
pub const ENODEV: Errno = Errno(19);
pub const EINVAL: Errno = Errno(22);

impl Errno {
    /// The negated errno a reply carries.
    pub fn status(self) -> i32 {
        -self.0
    }
}

impl std::fmt::Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "errno {}", self.0)
    }
}

/// `usbdevfs_urb.type`.
pub const URB_ISO: u8 = 0;
pub const URB_INTERRUPT: u8 = 1;
pub const URB_CONTROL: u8 = 2;
pub const URB_BULK: u8 = 3;

/// `usbdevfs_urb.flags`, which share the USB/IP flags' values.
pub const URB_SHORT_NOT_OK: u32 = 0x01;
pub const URB_ISO_ASAP: u32 = 0x02;
pub const URB_ZERO_PACKET: u32 = 0x40;

/// `struct usbdevfs_urb`; as many [`IsoFrame`]s as `number_of_packets` follow it.
#[repr(C)]
#[derive(Debug)]
pub struct Urb {
    pub kind: u8,
    pub endpoint: u8,
    pub status: i32,
    pub flags: u32,
    pub buffer: *mut u8,
    pub buffer_length: i32,
    pub actual_length: i32,
    pub start_frame: i32,
    pub number_of_packets: i32,
    pub error_count: i32,
    pub signr: u32,
    pub usercontext: usize,
}

/// `struct usbdevfs_iso_packet_desc`. `status` is a negated errno in an unsigned field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IsoFrame {
    pub length: u32,
    pub actual_length: u32,
    pub status: u32,
}

/// One URB and its packets in one allocation, as the kernel reads them.
pub struct UrbBox {
    urb: std::ptr::NonNull<Urb>,
    packets: usize,
}

// The kernel and the reaper only touch the URB under the device's state lock or after it
// left the kernel.
unsafe impl Send for UrbBox {}

impl UrbBox {
    fn layout(packets: usize) -> std::alloc::Layout {
        let size = std::mem::size_of::<Urb>() + packets * std::mem::size_of::<IsoFrame>();
        std::alloc::Layout::from_size_align(size, std::mem::align_of::<Urb>())
            .expect("a URB's layout")
    }

    pub fn new(packets: usize) -> Self {
        let layout = Self::layout(packets);
        // Zeroed: a valid Urb (null buffer) and packets.
        let raw = unsafe { std::alloc::alloc_zeroed(layout) }.cast::<Urb>();
        let urb =
            std::ptr::NonNull::new(raw).unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        Self { urb, packets }
    }

    pub fn as_ptr(&self) -> *mut Urb {
        self.urb.as_ptr()
    }

    pub fn urb(&self) -> &Urb {
        unsafe { self.urb.as_ref() }
    }

    pub fn urb_mut(&mut self) -> &mut Urb {
        unsafe { self.urb.as_mut() }
    }

    pub fn frames(&self) -> &[IsoFrame] {
        unsafe { std::slice::from_raw_parts(frames_of(self.as_ptr()), self.packets) }
    }

    pub fn frames_mut(&mut self) -> &mut [IsoFrame] {
        unsafe { std::slice::from_raw_parts_mut(frames_of(self.as_ptr()), self.packets) }
    }
}

impl Drop for UrbBox {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.urb.as_ptr().cast(), Self::layout(self.packets)) }
    }
}

/// The packets that follow `urb`.
///
/// # Safety
/// `urb` points at a URB allocated with its packets, as [`UrbBox`] does.
pub unsafe fn frames_of(urb: *mut Urb) -> *mut IsoFrame {
    urb.cast::<u8>().add(std::mem::size_of::<Urb>()).cast()
}

/// What [`Kernel::wait`] saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    /// A URB completed: reap it.
    Completed,
    /// Nothing within the timeout.
    Idle,
    /// The device was unplugged; what still completes can be reaped.
    Gone,
}

/// The usbfs ioctls of one open device.
pub trait Kernel: Send + Sync + 'static {
    /// The device descriptor and every configuration's descriptors, as `read` returns them.
    fn descriptors(&self) -> Result<Vec<u8>, Errno>;
    /// The active configuration's value, 0 when unconfigured (GET_CONFIGURATION).
    fn configuration(&self) -> Result<u8, Errno>;
    /// `enum usb_device_speed`, which USB/IP's speeds follow; 0 when the kernel does not know.
    fn speed(&self) -> Result<u32, Errno>;
    fn claim(&self, interface: u8) -> Result<(), Errno>;
    fn release(&self, interface: u8) -> Result<(), Errno>;
    /// Unbinds the kernel driver that has `interface`.
    fn disconnect_driver(&self, interface: u8) -> Result<(), Errno>;
    /// Lets the kernel bind a driver to `interface` again.
    fn connect_driver(&self, interface: u8) -> Result<(), Errno>;
    fn set_configuration(&self, value: u8) -> Result<(), Errno>;
    fn set_interface(&self, interface: u8, alternate: u8) -> Result<(), Errno>;
    /// Clears a halt on `endpoint` (an address, with the IN bit) and resets its toggle.
    fn clear_halt(&self, endpoint: u8) -> Result<(), Errno>;
    fn reset(&self) -> Result<(), Errno>;
    /// Hands `urb` to the kernel.
    ///
    /// # Safety
    /// `urb`, its packets and its buffer stay valid and in place until [`Kernel::reap`]
    /// returns it, or until [`Kernel::close`] if it never does.
    unsafe fn submit(&self, urb: *mut Urb) -> Result<(), Errno>;
    /// Cancels `urb`; it still completes, with -ENOENT, and is reaped.
    fn discard(&self, urb: *mut Urb) -> Result<(), Errno>;
    /// A completed URB, its status and lengths written, or None when none has.
    fn reap(&self) -> Result<Option<*mut Urb>, Errno>;
    fn wait(&self, timeout: Duration) -> Result<Ready, Errno>;
    /// Closes the file descriptor; every call after fails.
    fn close(&self);
}

/// One interface of a configuration, at alternate setting 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub class: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub interfaces: Vec<Interface>,
    /// Every endpoint address of every alternate setting, with its transfer type.
    pub endpoints: Vec<(u8, u8)>,
}

/// What the descriptors say about the device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Descriptors {
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configurations: Vec<Configuration>,
}

impl Descriptors {
    /// Parses what usbfs's `read` returns: the device descriptor, then each configuration's.
    pub fn parse(raw: &[u8]) -> Result<Self, String> {
        let device = raw
            .get(..18)
            .filter(|d| d[0] >= 18 && d[1] == 1)
            .ok_or("no device descriptor")?;
        let mut parsed = Self {
            bcd_device: u16::from_le_bytes([device[12], device[13]]),
            vendor: u16::from_le_bytes([device[8], device[9]]),
            product: u16::from_le_bytes([device[10], device[11]]),
            class: device[4],
            subclass: device[5],
            protocol: device[6],
            configurations: Vec::new(),
        };
        let mut at = usize::from(device[0]);
        while at < raw.len() {
            let head = raw
                .get(at..at + 9)
                .ok_or("a cut configuration descriptor")?;
            if head[1] != 2 || head[0] < 9 {
                return Err(format!(
                    "descriptor type {} where a configuration belongs",
                    head[1]
                ));
            }
            let total = usize::from(u16::from_le_bytes([head[2], head[3]]));
            let body = raw
                .get(at..at + total.max(9))
                .ok_or("a configuration longer than the descriptors")?;
            parsed.configurations.push(configuration(body)?);
            at += total.max(9);
        }
        Ok(parsed)
    }

    pub fn configuration(&self, value: u8) -> Option<&Configuration> {
        self.configurations.iter().find(|c| c.value == value)
    }
}

fn configuration(body: &[u8]) -> Result<Configuration, String> {
    let mut parsed = Configuration {
        value: body[5],
        ..Configuration::default()
    };
    let mut at = usize::from(body[0]);
    while at + 2 <= body.len() {
        let length = usize::from(body[at]);
        if length < 2 || at + length > body.len() {
            return Err(format!(
                "a descriptor of {length} bytes in configuration {}",
                parsed.value
            ));
        }
        let descriptor = &body[at..at + length];
        match descriptor[1] {
            4 if length >= 9 => {
                if !parsed.interfaces.iter().any(|i| i.number == descriptor[2]) {
                    parsed.interfaces.push(Interface {
                        number: descriptor[2],
                        class: descriptor[5],
                    });
                }
            }
            5 if length >= 7
                && !parsed
                    .endpoints
                    .iter()
                    .any(|(address, _)| *address == descriptor[2]) =>
            {
                parsed.endpoints.push((descriptor[2], descriptor[3] & 3));
            }
            _ => {}
        }
        at += length;
    }
    Ok(parsed)
}

/// Bus and device number of an Android or Linux device node, `/dev/bus/usb/001/005`.
pub fn bus_and_device(path: &str) -> Option<(u32, u32)> {
    let mut parts = path.rsplit('/');
    let device = parts.next()?.parse().ok()?;
    let bus = parts.next()?.parse().ok()?;
    Some((bus, device))
}

/// How long a reaper sleeps when nothing completes; only closing waits for it.
const POLL: Duration = Duration::from_millis(100);
/// How long closing waits for discarded URBs to come back before it closes the descriptor.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// What a completed URB answers.
enum Shape {
    Control,
    Plain,
    /// The packets asked, in the order asked.
    Iso(Vec<IsoPacket>),
}

/// A URB the kernel has.
struct Flight {
    seqnum: u32,
    urb: UrbBox,
    buffer: Vec<u8>,
    direction: Direction,
    length: u32,
    shape: Shape,
    done: Completion,
}

impl Flight {
    fn reply(&self) -> Reply {
        let urb = self.urb.urb();
        let actual = usize::try_from(urb.actual_length).unwrap_or(0);
        let mut reply = Reply {
            status: urb.status,
            start_frame: urb.start_frame,
            ..Reply::default()
        };
        match &self.shape {
            Shape::Control | Shape::Plain => {
                // A control URB's data follows its setup packet.
                let skip = if matches!(self.shape, Shape::Control) {
                    8
                } else {
                    0
                };
                let actual = actual.min(self.length as usize);
                match self.direction {
                    Direction::In => {
                        reply.data =
                            self.buffer[skip..(skip + actual).min(self.buffer.len())].to_vec()
                    }
                    Direction::Out => reply.sent = actual as u32,
                }
            }
            Shape::Iso(asked) => {
                let mut at = 0usize;
                let mut sent = 0u32;
                for (packet, frame) in asked.iter().zip(self.urb.frames()) {
                    let actual = frame.actual_length.min(packet.length);
                    if self.direction == Direction::In {
                        let end = (at + actual as usize).min(self.buffer.len());
                        reply.data.extend_from_slice(&self.buffer[at.min(end)..end]);
                    }
                    sent += actual;
                    reply.iso.push(IsoPacket {
                        actual_length: actual,
                        status: frame.status as i32,
                        ..*packet
                    });
                    at += packet.length as usize;
                }
                if self.direction == Direction::Out {
                    reply.sent = sent;
                }
                reply.error_count = u32::try_from(urb.error_count).unwrap_or(0);
            }
        }
        reply
    }
}

struct State {
    flights: HashMap<usize, Flight>,
    seqnums: HashMap<u32, usize>,
    next: usize,
    device: wire::Device,
    claimed: Vec<u8>,
    /// Interfaces whose kernel driver was unbound to claim them; bound again on close.
    disconnected: Vec<u8>,
    closed: bool,
    /// Unplugged: nothing submits any more.
    gone: bool,
}

struct Inner<K> {
    kernel: K,
    descriptors: Descriptors,
    state: Mutex<State>,
    reaper: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl<K> Inner<K> {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A [`UsbDevice`] on usbfs.
pub struct UsbfsDevice<K: Kernel> {
    inner: Arc<Inner<K>>,
}

impl<K: Kernel> UsbfsDevice<K> {
    /// Takes over the device `kernel` has open at `path` (its device node, which names its
    /// bus and address): reads its descriptors and speed, claims every interface of the active
    /// configuration, unbinding kernel drivers that have one, and starts its reaper. Closes
    /// `kernel` when it fails.
    pub fn open(kernel: K, path: &str) -> Result<Self, String> {
        match Self::try_open(&kernel, path) {
            Ok((descriptors, device)) => {
                let inner = Arc::new(Inner {
                    kernel,
                    descriptors,
                    state: Mutex::new(State {
                        flights: HashMap::new(),
                        seqnums: HashMap::new(),
                        next: 1,
                        device,
                        claimed: Vec::new(),
                        disconnected: Vec::new(),
                        closed: false,
                        gone: false,
                    }),
                    reaper: Mutex::new(None),
                });
                let configuration = inner.state().device.configuration;
                if let Err(e) = claim_all(&inner, configuration) {
                    release_all(&inner);
                    inner.kernel.close();
                    return Err(e);
                }
                let reaping = inner.clone();
                let spawned = std::thread::Builder::new()
                    .name("usb-reaper".into())
                    .spawn(move || reap_loop(&reaping));
                match spawned {
                    Ok(thread) => {
                        *inner.reaper.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread)
                    }
                    Err(e) => {
                        release_all(&inner);
                        inner.kernel.close();
                        return Err(format!("no reaper thread: {e}"));
                    }
                }
                Ok(Self { inner })
            }
            Err(e) => {
                kernel.close();
                Err(e)
            }
        }
    }

    fn try_open(kernel: &K, path: &str) -> Result<(Descriptors, wire::Device), String> {
        let raw = kernel
            .descriptors()
            .map_err(|e| format!("its descriptors did not read: {e}"))?;
        let descriptors =
            Descriptors::parse(&raw).map_err(|e| format!("its descriptors are broken: {e}"))?;
        let configuration = kernel
            .configuration()
            .map_err(|e| format!("its configuration did not read: {e}"))?;
        let speed = match kernel.speed() {
            Ok(0) => return Err("the kernel does not know its speed".into()),
            Ok(speed) => speed,
            Err(e) => return Err(format!("its speed did not read: {e}")),
        };
        let (busnum, devnum) =
            bus_and_device(path).ok_or_else(|| format!("{path:?} is not a USB device node"))?;
        let device = wire::Device {
            path: path.to_owned(),
            busid: format!("{busnum}-{devnum}"),
            busnum,
            devnum,
            speed,
            vendor: descriptors.vendor,
            product: descriptors.product,
            bcd_device: descriptors.bcd_device,
            class: descriptors.class,
            subclass: descriptors.subclass,
            protocol: descriptors.protocol,
            configuration,
            configurations: descriptors.configurations.len().min(255) as u8,
            interfaces: interface_count(&descriptors, configuration),
        };
        Ok((descriptors, device))
    }

    /// The endpoint's transfer type from the descriptors; bulk when they don't say.
    fn kind_of(&self, address: u8) -> u8 {
        let mut types = self
            .inner
            .descriptors
            .configurations
            .iter()
            .flat_map(|c| c.endpoints.iter());
        match types.find(|(a, _)| *a == address) {
            Some((_, 1)) => URB_ISO,
            Some((_, 3)) => URB_INTERRUPT,
            _ => URB_BULK,
        }
    }

    /// A standard request that changes the kernel's state, run through its ioctl; None for
    /// any other, which goes to the device.
    fn intercept(&self, transfer: &Transfer) -> Option<Reply> {
        let setup = transfer.setup;
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let index = u16::from_le_bytes([setup[4], setup[5]]);
        let kernel = &self.inner.kernel;
        let result = match (setup[0], setup[1]) {
            // SET_CONFIGURATION.
            (0x00, 9) => self.set_configuration(value as u8),
            // SET_INTERFACE.
            (0x01, 11) => kernel.set_interface(index as u8, value as u8),
            // CLEAR_FEATURE(ENDPOINT_HALT).
            (0x02, 1) if value == 0 => kernel.clear_halt(index as u8),
            // SET_FEATURE(PORT_RESET) to the port the device sits on.
            (0x23, 3) if value == 4 => self.reset(),
            _ => return None,
        };
        Some(match result {
            Ok(()) => Reply::default(),
            Err(e) => Reply::failed(e.status()),
        })
    }

    fn set_configuration(&self, value: u8) -> Result<(), Errno> {
        let current = self.inner.state().device.configuration;
        // Android configured the device already, and the host's driver picks the same one:
        // nothing changes, and the kernel would refuse it while interfaces are claimed.
        if value == current {
            return Ok(());
        }
        release_all(&self.inner);
        if let Err(e) = self.inner.kernel.set_configuration(value) {
            // Still in the old one.
            let _ = claim_all(&self.inner, current);
            return Err(e);
        }
        {
            let mut state = self.inner.state();
            state.device.configuration = value;
            state.device.interfaces = interface_count(&self.inner.descriptors, value);
        }
        claim_all(&self.inner, value).map_err(|_| EBUSY)
    }

    fn reset(&self) -> Result<(), Errno> {
        self.inner.kernel.reset()?;
        // A reset unbinds usbfs from the interfaces; take them back.
        let configuration = self.inner.state().device.configuration;
        self.inner.state().claimed.clear();
        claim_all(&self.inner, configuration).map_err(|_| EBUSY)
    }

    /// The URB and buffer for `transfer`, or the status it fails with.
    fn build(&self, transfer: &Transfer) -> Result<(UrbBox, Vec<u8>, Shape), i32> {
        let address = transfer.endpoint
            | if transfer.direction == Direction::In {
                0x80
            } else {
                0
            };
        let length = transfer.length as usize;
        let (mut urb, buffer, shape) = if transfer.is_control() {
            let mut buffer = Vec::with_capacity(8 + length);
            buffer.extend_from_slice(&transfer.setup);
            match transfer.direction {
                Direction::Out => buffer.extend_from_slice(&transfer.data),
                Direction::In => buffer.resize(8 + length, 0),
            }
            let mut urb = UrbBox::new(0);
            urb.urb_mut().kind = URB_CONTROL;
            (urb, buffer, Shape::Control)
        } else if transfer.is_isochronous() {
            let total: usize = transfer.iso.iter().map(|p| p.length as usize).sum();
            if total > length {
                return Err(wire::EINVAL);
            }
            let mut buffer = Vec::with_capacity(total);
            match transfer.direction {
                // usbfs takes the packets back to back; USB/IP places them at their offsets.
                Direction::Out => {
                    for packet in &transfer.iso {
                        let (start, end) = (
                            packet.offset as usize,
                            (packet.offset + packet.length) as usize,
                        );
                        let data = transfer.data.get(start..end).ok_or(wire::EINVAL)?;
                        buffer.extend_from_slice(data);
                    }
                }
                Direction::In => buffer.resize(total, 0),
            }
            let mut urb = UrbBox::new(transfer.iso.len());
            for (frame, packet) in urb.frames_mut().iter_mut().zip(&transfer.iso) {
                frame.length = packet.length;
            }
            let raw = urb.urb_mut();
            raw.kind = URB_ISO;
            raw.flags = URB_ISO_ASAP;
            raw.number_of_packets = transfer.iso.len() as i32;
            (urb, buffer, Shape::Iso(transfer.iso.clone()))
        } else {
            let buffer = match transfer.direction {
                Direction::Out => transfer.data.clone(),
                Direction::In => vec![0; length],
            };
            let mut urb = UrbBox::new(0);
            let raw = urb.urb_mut();
            raw.kind = match self.kind_of(address) {
                URB_INTERRUPT => URB_INTERRUPT,
                _ => URB_BULK,
            };
            raw.flags = match transfer.direction {
                Direction::In => transfer.flags & URB_SHORT_NOT_OK,
                Direction::Out => transfer.flags & URB_ZERO_PACKET,
            };
            (urb, buffer, Shape::Plain)
        };
        let raw = urb.urb_mut();
        raw.endpoint = if transfer.is_control() { 0 } else { address };
        raw.buffer_length = i32::try_from(buffer.len()).map_err(|_| wire::EINVAL)?;
        Ok((urb, buffer, shape))
    }
}

fn interface_count(descriptors: &Descriptors, configuration: u8) -> u8 {
    descriptors
        .configuration(configuration)
        .map_or(0, |c| c.interfaces.len().min(255) as u8)
}

/// Claims every interface of `configuration`, unbinding the kernel's driver from one it has.
fn claim_all<K: Kernel>(inner: &Inner<K>, configuration: u8) -> Result<(), String> {
    let Some(config) = inner.descriptors.configuration(configuration) else {
        return Ok(());
    };
    for interface in config.interfaces.iter().map(|i| i.number) {
        if inner.state().claimed.contains(&interface) {
            continue;
        }
        match inner.kernel.claim(interface) {
            Ok(()) => {}
            Err(EBUSY) => {
                inner.kernel.disconnect_driver(interface).map_err(|e| {
                    format!("interface {interface} is in use and its driver did not let go: {e}")
                })?;
                let mut state = inner.state();
                if !state.disconnected.contains(&interface) {
                    state.disconnected.push(interface);
                }
                drop(state);
                inner
                    .kernel
                    .claim(interface)
                    .map_err(|e| format!("interface {interface} did not claim: {e}"))?;
            }
            Err(e) => return Err(format!("interface {interface} did not claim: {e}")),
        }
        inner.state().claimed.push(interface);
    }
    Ok(())
}

fn release_all<K: Kernel>(inner: &Inner<K>) {
    let claimed = std::mem::take(&mut inner.state().claimed);
    for interface in claimed {
        let _ = inner.kernel.release(interface);
    }
}

fn reap_loop<K: Kernel>(inner: &Inner<K>) {
    let mut deadline = None;
    loop {
        {
            let state = inner.state();
            if state.flights.is_empty() && (state.closed || state.gone) {
                return;
            }
            if state.closed
                && *deadline.get_or_insert_with(|| Instant::now() + CLOSE_WAIT) < Instant::now()
            {
                // What is left stays with the state; closing frees it after the descriptor.
                return;
            }
        }
        match inner.kernel.wait(POLL) {
            Ok(Ready::Completed) => {
                if !drain(inner) {
                    return unplugged(inner);
                }
            }
            Ok(Ready::Idle) | Err(EINTR) => {}
            Ok(Ready::Gone) | Err(_) => {
                drain(inner);
                return unplugged(inner);
            }
        }
    }
}

/// Reaps and answers every completed URB; false once the device is gone.
fn drain<K: Kernel>(inner: &Inner<K>) -> bool {
    loop {
        match inner.kernel.reap() {
            Ok(Some(urb)) => complete(inner, urb),
            Ok(None) | Err(EAGAIN) => return true,
            Err(EINTR) => {}
            Err(_) => return false,
        }
    }
}

fn complete<K: Kernel>(inner: &Inner<K>, urb: *mut Urb) {
    let flight = {
        let mut state = inner.state();
        // The id the URB was submitted with; the kernel leaves it alone.
        let id = unsafe { (*urb).usercontext };
        let Some(flight) = state.flights.remove(&id) else {
            return;
        };
        state.seqnums.remove(&flight.seqnum);
        flight
    };
    let reply = flight.reply();
    flight.done.complete(reply);
}

/// The device was unplugged: nothing reaps any more, so what the kernel still has fails here.
fn unplugged<K: Kernel>(inner: &Inner<K>) {
    let left: Vec<Flight> = {
        let mut state = inner.state();
        state.gone = true;
        state.seqnums.clear();
        state.flights.drain().map(|(_, f)| f).collect()
    };
    for flight in left {
        flight.done.complete(Reply::failed(wire::ENODEV));
        // Its memory goes with it: no reap will hand the URB back.
        drop(flight.urb);
    }
}

impl<K: Kernel> UsbDevice for UsbfsDevice<K> {
    fn describe(&self) -> wire::Device {
        self.inner.state().device.clone()
    }

    fn class(&self) -> u8 {
        let device = self.describe();
        if device.class != 0 {
            return device.class;
        }
        self.inner
            .descriptors
            .configuration(device.configuration)
            .or(self.inner.descriptors.configurations.first())
            .and_then(|c| c.interfaces.first())
            .map_or(0, |i| i.class)
    }

    fn submit(&self, transfer: Transfer, done: Completion) {
        {
            let state = self.inner.state();
            if state.closed {
                drop(state);
                return done.complete(Reply::failed(wire::ESHUTDOWN));
            }
            if state.gone {
                drop(state);
                return done.complete(Reply::failed(wire::ENODEV));
            }
        }
        if transfer.is_control() {
            if let Some(reply) = self.intercept(&transfer) {
                return done.complete(reply);
            }
        }
        let (urb, buffer, shape) = match self.build(&transfer) {
            Ok(built) => built,
            Err(status) => return done.complete(Reply::failed(status)),
        };
        let mut state = self.inner.state();
        let id = state.next;
        state.next = state.next.wrapping_add(1).max(1);
        let mut flight = Flight {
            seqnum: transfer.seqnum,
            urb,
            buffer,
            direction: transfer.direction,
            length: transfer.length,
            shape,
            done,
        };
        let raw = flight.urb.as_ptr();
        {
            let urb = flight.urb.urb_mut();
            urb.buffer = flight.buffer.as_mut_ptr();
            urb.usercontext = id;
        }
        // In the table first: the reaper may reap it before the submit returns, and waits for
        // this lock to find it.
        state.flights.insert(id, flight);
        state.seqnums.insert(transfer.seqnum, id);
        // The flight owns the URB and its buffer until it is reaped, and closing frees them only
        // after the descriptor closed.
        if let Err(e) = unsafe { self.inner.kernel.submit(raw) } {
            state.seqnums.remove(&transfer.seqnum);
            let flight = state.flights.remove(&id);
            drop(state);
            if let Some(flight) = flight {
                flight.done.complete(Reply::failed(e.status()));
            }
        }
    }

    fn unlink(&self, seqnum: u32) {
        let state = self.inner.state();
        let Some(flight) = state
            .seqnums
            .get(&seqnum)
            .and_then(|id| state.flights.get(id))
        else {
            return;
        };
        // Still ours under the lock: the reaper removes it only after taking it. It comes back
        // with -ENOENT, which the server drops, or completed already (EINVAL here).
        let _ = self.inner.kernel.discard(flight.urb.as_ptr());
    }

    fn close(&self) {
        let urbs: Vec<*mut Urb> = {
            let mut state = self.inner.state();
            if state.closed {
                return;
            }
            state.closed = true;
            state.flights.values().map(|f| f.urb.as_ptr()).collect()
        };
        for urb in urbs {
            let _ = self.inner.kernel.discard(urb);
        }
        let reaper = self
            .inner
            .reaper
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(reaper) = reaper {
            if reaper.thread().id() != std::thread::current().id() {
                let _ = reaper.join();
            }
        }
        release_all(&self.inner);
        let disconnected = std::mem::take(&mut self.inner.state().disconnected);
        for interface in disconnected {
            let _ = self.inner.kernel.connect_driver(interface);
        }
        self.inner.kernel.close();
        // Whatever the kernel never gave back goes now that it can't write to it.
        let left: Vec<Flight> = {
            let mut state = self.inner.state();
            state.seqnums.clear();
            state.flights.drain().map(|(_, f)| f).collect()
        };
        drop(left);
    }
}

#[cfg(test)]
#[path = "usbfs_tests.rs"]
mod tests;
