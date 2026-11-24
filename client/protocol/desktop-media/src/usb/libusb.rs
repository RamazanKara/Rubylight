//! libusb-backed USB/IP device transport, standard control requests, and configuration state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nightjar_core::usb::{wire, Blocking, BlockingDevice, Direction, Reply, Transfer, UsbDevice};
use rusb::constants::{LIBUSB_ERROR_NO_DEVICE, LIBUSB_ERROR_OVERFLOW, LIBUSB_ERROR_PIPE, LIBUSB_ERROR_TIMEOUT, LIBUSB_SUCCESS};
use rusb::TransferType;

/// How long one bulk or interrupt call waits before the transfer looks at its cancel flag.
const SLICE: Duration = Duration::from_millis(200);
/// How long a control transfer may take; libusb's synchronous ones can't be cancelled.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const URB_ZERO_PACKET: u32 = 0x40;

/// Opens `device` through libusb, claims its active configuration's interfaces, and wraps it as a
/// [`UsbDevice`].
pub(super) fn open_device(
    device: &rusb::Device<rusb::Context>,
    busid: &str,
    vid: u16,
    pid: u16,
) -> rusb::Result<Arc<dyn UsbDevice>> {
    let descriptor = device.device_descriptor()?;
    let handle = device.open()?;
    // Linux: usb-storage, usbhid and the like let go while it is attached and take it back after.
    match handle.set_auto_detach_kernel_driver(true) {
        Ok(()) | Err(rusb::Error::NotSupported) => {}
        Err(e) => return Err(e),
    }
    let configuration = handle.active_configuration()?;
    let config = device.active_config_descriptor()?;
    let mut configs = HashMap::new();
    for index in 0..descriptor.num_configurations() {
        match device.config_descriptor(index) {
            Ok(c) => {
                configs.insert(
                    c.number(),
                    Config { interfaces: c.interfaces().map(|i| i.number()).collect(), endpoints: endpoints(&c) },
                );
            }
            Err(e) => eprintln!("nightjar: USB {busid}: configuration descriptor {index} could not be read, so the host can't set it: {e}"),
        }
    }
    let state = State { configuration, active: configuration, claimed: Vec::new(), endpoints: endpoints(&config), configs, closed: false };
    let class = match descriptor.class_code() {
        0 => config.interfaces().next().and_then(|i| i.descriptors().next()).map_or(0, |s| s.class_code()),
        class => class,
    };
    let version = descriptor.device_version();
    let bcd_device = (u16::from(version.0 / 10) << 12) | (u16::from(version.0 % 10) << 8) | (u16::from(version.1) << 4) | u16::from(version.2);
    let described = wire::Device {
        path: format!("/libusb/{busid}"),
        busid: busid.to_owned(),
        busnum: u32::from(device.bus_number()),
        devnum: u32::from(device.address()),
        speed: speed(device.speed()),
        vendor: vid,
        product: pid,
        bcd_device,
        class: descriptor.class_code(),
        subclass: descriptor.sub_class_code(),
        protocol: descriptor.protocol_code(),
        configuration,
        configurations: descriptor.num_configurations(),
        interfaces: config.num_interfaces(),
    };
    let opened = Libusb { handle, described, class, state: Mutex::new(state), warned_iso: AtomicBool::new(false) };
    let interfaces: Vec<u8> = config.interfaces().map(|i| i.number()).collect();
    claim(&opened.handle, &mut opened.state(), &interfaces)?;
    Ok(Arc::new(Blocking::new(opened)))
}

/// libusb's speed as USB/IP numbers it: 1 low, 2 full, 3 high, 5 super, 6 super+.
fn speed(speed: rusb::Speed) -> u32 {
    match speed {
        rusb::Speed::Low => 1,
        rusb::Speed::Full => 2,
        rusb::Speed::High => 3,
        rusb::Speed::Super => 5,
        rusb::Speed::SuperPlus => 6,
        _ => 0,
    }
}

/// Every endpoint of a configuration, any alternate setting: its type and maximum packet size.
fn endpoints(config: &rusb::ConfigDescriptor) -> HashMap<u8, (TransferType, u16)> {
    let mut out = HashMap::new();
    for interface in config.interfaces() {
        for setting in interface.descriptors() {
            for endpoint in setting.endpoint_descriptors() {
                out.insert(endpoint.address(), (endpoint.transfer_type(), endpoint.max_packet_size() & 0x7ff));
            }
        }
    }
    out
}

/// A standard request the driver sends that changes the device's state, which libusb has to
/// do itself (Windows refuses them as plain control transfers).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Standard {
    SetConfiguration(u8),
    SetInterface { interface: u8, setting: u8 },
    ClearHalt(u8),
    /// SET_FEATURE(PORT_RESET) to the device's port, which the host's driver sends to recover
    /// it, as Linux's USB/IP host takes it: the device is reset here.
    PortReset,
}

pub(super) fn standard(setup: &[u8; 8]) -> Option<Standard> {
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    match (setup[0], setup[1]) {
        (0x00, 9) => Some(Standard::SetConfiguration(value as u8)),
        (0x01, 11) => Some(Standard::SetInterface { interface: index as u8, setting: value as u8 }),
        (0x02, 1) if value == 0 => Some(Standard::ClearHalt(index as u8)),
        (0x23, 3) if value == 4 => Some(Standard::PortReset),
        _ => None,
    }
}

/// A libusb return code as USB/IP's negated errno.
pub(super) fn errno(code: i32) -> i32 {
    match code {
        LIBUSB_ERROR_PIPE => wire::EPIPE,
        LIBUSB_ERROR_NO_DEVICE => wire::ENODEV,
        LIBUSB_ERROR_OVERFLOW => wire::EOVERFLOW,
        LIBUSB_ERROR_TIMEOUT => wire::ETIMEDOUT,
        _ => wire::EPROTO,
    }
}

pub(super) fn rusb_errno(e: rusb::Error) -> i32 {
    match e {
        rusb::Error::Pipe => wire::EPIPE,
        rusb::Error::NoDevice => wire::ENODEV,
        rusb::Error::Overflow => wire::EOVERFLOW,
        rusb::Error::Timeout => wire::ETIMEDOUT,
        _ => wire::EPROTO,
    }
}

/// Reads up to `length` bytes in [`SLICE`]s until the transfer ends or is cancelled. A slice
/// that runs out has only read whole packets (a short one would have ended it), so the next
/// slice goes on where it stopped.
pub(super) fn read_sliced(length: usize, cancelled: &AtomicBool, mut call: impl FnMut(&mut [u8]) -> (i32, usize)) -> Reply {
    let mut buffer = vec![0; length];
    let mut at = 0;
    loop {
        if cancelled.load(Ordering::SeqCst) { return Reply::failed(wire::ECONNRESET); }
        let (code, n) = call(&mut buffer[at..]);
        at = (at + n).min(length);
        match code {
            LIBUSB_SUCCESS => break,
            LIBUSB_ERROR_TIMEOUT if at < length => {}
            LIBUSB_ERROR_TIMEOUT => break,
            code => return Reply::failed(errno(code)),
        }
    }
    buffer.truncate(at);
    Reply::read(buffer)
}

/// Sends `data` in [`SLICE`]s, then a zero-length packet when `zero_packet` asks for one. An
/// empty transfer is a zero-length packet itself.
pub(super) fn write_sliced(data: &[u8], zero_packet: bool, cancelled: &AtomicBool, mut call: impl FnMut(&[u8]) -> (i32, usize)) -> Reply {
    let mut at = 0;
    let mut zero = zero_packet || data.is_empty();
    while at < data.len() || zero {
        if cancelled.load(Ordering::SeqCst) { return Reply::failed(wire::ECONNRESET); }
        let (code, n) = call(&data[at..]);
        at = (at + n).min(data.len());
        match code {
            LIBUSB_SUCCESS if at >= data.len() && zero && n == 0 => zero = false,
            LIBUSB_SUCCESS | LIBUSB_ERROR_TIMEOUT => {}
            code => return Reply::failed(errno(code)),
        }
    }
    Reply::sent(at as u32)
}

/// One of the device's configurations: its interfaces and endpoints.
#[derive(Clone)]
pub(super) struct Config {
    pub(super) interfaces: Vec<u8>,
    pub(super) endpoints: HashMap<u8, (TransferType, u16)>,
}

pub(super) struct State {
    /// The configuration the host sees; 0 after it unconfigured the device.
    pub(super) configuration: u8,
    /// The configuration the device is in: unconfiguring only hides it (see [`set_configuration`]).
    pub(super) active: u8,
    pub(super) claimed: Vec<u8>,
    pub(super) endpoints: HashMap<u8, (TransferType, u16)>,
    pub(super) configs: HashMap<u8, Config>,
    pub(super) closed: bool,
}

/// What changing the configuration and resetting the device need of a libusb handle.
pub(super) trait Configure {
    fn claim_interface(&self, interface: u8) -> rusb::Result<()>;
    fn release_interface(&self, interface: u8) -> rusb::Result<()>;
    fn set_auto_detach_kernel_driver(&self, on: bool) -> rusb::Result<()>;
    fn set_active_configuration(&self, value: u8) -> rusb::Result<()>;
    fn reset(&self) -> rusb::Result<()>;
}

impl Configure for rusb::DeviceHandle<rusb::Context> {
    fn claim_interface(&self, interface: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::claim_interface(self, interface)
    }
    fn release_interface(&self, interface: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::release_interface(self, interface)
    }
    fn set_auto_detach_kernel_driver(&self, on: bool) -> rusb::Result<()> {
        rusb::DeviceHandle::set_auto_detach_kernel_driver(self, on)
    }
    fn set_active_configuration(&self, value: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::set_active_configuration(self, value)
    }
    fn reset(&self) -> rusb::Result<()> {
        rusb::DeviceHandle::reset(self)
    }
}

/// Claims each of `interfaces` not claimed yet; on a failure lets go of every one.
fn claim(handle: &impl Configure, state: &mut State, interfaces: &[u8]) -> rusb::Result<()> {
    for &interface in interfaces {
        if state.claimed.contains(&interface) { continue; }
        if let Err(e) = handle.claim_interface(interface) {
            for &claimed in &state.claimed { let _ = handle.release_interface(claimed); }
            state.claimed.clear();
            return Err(e);
        }
        state.claimed.push(interface);
    }
    Ok(())
}

fn release(handle: &impl Configure, state: &mut State, busid: &str) {
    for interface in std::mem::take(&mut state.claimed) {
        if let Err(e) = handle.release_interface(interface) {
            eprintln!("nightjar: USB {busid}: releasing interface {interface} failed: {e}");
        }
    }
}

/// libusb puts the kernel's drivers back on interfaces it lets go of (Linux); `on` false keeps
/// them off.
fn auto_detach(handle: &impl Configure, on: bool, busid: &str) {
    match handle.set_auto_detach_kernel_driver(on) {
        Ok(()) | Err(rusb::Error::NotSupported) => {}
        Err(e) => eprintln!("nightjar: USB {busid}: turning the kernel driver's detach {} failed: {e}", if on { "on" } else { "off" }),
    }
}

/// SET_CONFIGURATION from the host. Unconfiguring (0) only hides the configuration until the
/// host sets one again: WinUSB can't unconfigure, and Linux won't while interfaces are
/// claimed. A real change lets go of the old interfaces without giving them back to the
/// kernel's drivers (Linux refuses the change while a driver is bound), and when the device
/// refuses it claims them again, so the old configuration still works.
pub(super) fn set_configuration(handle: &impl Configure, state: &mut State, value: u8, busid: &str) -> Reply {
    if value == state.configuration { return Reply::sent(0); }
    if value == 0 {
        state.configuration = 0;
        state.endpoints.clear();
        return Reply::sent(0);
    }
    let Some(config) = state.configs.get(&value).cloned() else { return Reply::failed(wire::EINVAL) };
    if value != state.active {
        auto_detach(handle, false, busid);
        let old = state.claimed.clone();
        release(handle, state, busid);
        let set = handle.set_active_configuration(value);
        auto_detach(handle, true, busid);
        if let Err(e) = set {
            eprintln!("nightjar: USB {busid}: configuration {value} failed: {e}; staying in {}", state.active);
            if let Err(again) = claim(handle, state, &old) {
                eprintln!("nightjar: USB {busid}: configuration {}'s interfaces could not be claimed again: {again}", state.active);
            }
            return Reply::failed(rusb_errno(e));
        }
        state.active = value;
    }
    state.configuration = value;
    state.endpoints = config.endpoints;
    match claim(handle, state, &config.interfaces) {
        Ok(()) => Reply::sent(0),
        Err(e) => Reply::failed(rusb_errno(e)),
    }
}

/// A port reset from the host. libusb claims the interfaces again after a reset; claiming
/// them here too (a no-op for one claimed already) checks that it did.
pub(super) fn reset_port(handle: &impl Configure, state: &mut State, busid: &str) -> Reply {
    match handle.reset() {
        Ok(()) => {}
        // The device came back as another one, which this handle no longer reaches.
        Err(rusb::Error::NotFound | rusb::Error::NoDevice) => {
            eprintln!("nightjar: USB {busid}: the device went away when it was reset");
            return Reply::failed(wire::ENODEV);
        }
        Err(e) => {
            eprintln!("nightjar: USB {busid}: resetting the device failed: {e}");
            return Reply::failed(rusb_errno(e));
        }
    }
    let interfaces = state.configs.get(&state.active).map(|c| c.interfaces.clone()).unwrap_or_default();
    state.claimed.clear();
    match claim(handle, state, &interfaces) {
        Ok(()) => Reply::sent(0),
        Err(e) => {
            eprintln!("nightjar: USB {busid}: the interfaces could not be claimed again after a reset: {e}");
            Reply::failed(rusb_errno(e))
        }
    }
}

struct Libusb {
    handle: rusb::DeviceHandle<rusb::Context>,
    described: wire::Device,
    class: u8,
    state: Mutex<State>,
    warned_iso: AtomicBool,
}

impl Libusb {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn control(&self, transfer: &Transfer) -> Reply {
        let setup = &transfer.setup;
        let done = |result: rusb::Result<()>| result.map_or_else(|e| Reply::failed(rusb_errno(e)), |()| Reply::sent(0));
        let busid = &self.described.busid;
        match standard(setup) {
            Some(Standard::SetConfiguration(value)) => return set_configuration(&self.handle, &mut self.state(), value, busid),
            Some(Standard::PortReset) => return reset_port(&self.handle, &mut self.state(), busid),
            Some(Standard::SetInterface { interface, setting }) => return done(self.handle.set_alternate_setting(interface, setting)),
            Some(Standard::ClearHalt(endpoint)) => return done(self.handle.clear_halt(endpoint)),
            None => {}
        }
        let (request_type, request) = (setup[0], setup[1]);
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let index = u16::from_le_bytes([setup[4], setup[5]]);
        let length = usize::from(u16::from_le_bytes([setup[6], setup[7]]));
        if request_type & 0x80 != 0 {
            let mut buffer = vec![0; length.min(transfer.length as usize)];
            match self.handle.read_control(request_type, request, value, index, &mut buffer, CONTROL_TIMEOUT) {
                Ok(n) => {
                    buffer.truncate(n);
                    Reply::read(buffer)
                }
                Err(e) => Reply::failed(rusb_errno(e)),
            }
        } else {
            match self.handle.write_control(request_type, request, value, index, &transfer.data, CONTROL_TIMEOUT) {
                Ok(n) => Reply::sent(n as u32),
                Err(e) => Reply::failed(rusb_errno(e)),
            }
        }
    }

    /// One bulk or interrupt call of at most [`SLICE`]: libusb's code and the bytes moved.
    fn call(&self, address: u8, kind: TransferType, data: *mut u8, length: usize) -> (i32, usize) {
        let Ok(length) = i32::try_from(length) else { return (rusb::constants::LIBUSB_ERROR_INVALID_PARAM, 0) };
        let mut moved = 0;
        let run = if kind == TransferType::Interrupt { rusb::ffi::libusb_interrupt_transfer } else { rusb::ffi::libusb_bulk_transfer };
        // SAFETY: the handle is open while self lives; `data` holds `length` bytes, which libusb
        // only reads for an OUT endpoint.
        let code = unsafe { run(self.handle.as_raw(), address, data, length, &mut moved, SLICE.as_millis() as u32) };
        (code, usize::try_from(moved).unwrap_or(0))
    }
}

impl BlockingDevice for Libusb {
    fn describe(&self) -> wire::Device {
        wire::Device { configuration: self.state().configuration, ..self.described.clone() }
    }

    fn class(&self) -> u8 {
        self.class
    }

    fn transfer(&self, transfer: &Transfer, cancelled: &AtomicBool) -> Reply {
        if self.state().closed { return Reply::failed(wire::ESHUTDOWN); }
        if transfer.is_control() { return self.control(transfer); }
        if transfer.is_isochronous() {
            if !self.warned_iso.swap(true, Ordering::SeqCst) {
                eprintln!("nightjar: USB {}: the host asked for isochronous transfers, which this client doesn't serve", self.described.busid);
            }
            return Reply::failed(wire::EPROTO);
        }
        let address = transfer.endpoint | if transfer.direction == Direction::In { 0x80 } else { 0 };
        let Some((kind, max_packet)) = self.state().endpoints.get(&address).copied() else { return Reply::failed(wire::EPIPE) };
        match transfer.direction {
            Direction::In => read_sliced(transfer.length as usize, cancelled, |buffer| self.call(address, kind, buffer.as_mut_ptr(), buffer.len())),
            Direction::Out => {
                let data = &transfer.data;
                let zero = transfer.flags & URB_ZERO_PACKET != 0 && !data.is_empty() && max_packet > 0 && data.len().is_multiple_of(usize::from(max_packet));
                write_sliced(data, zero, cancelled, |rest| self.call(address, kind, rest.as_ptr().cast_mut(), rest.len()))
            }
        }
    }

    fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        release(&self.handle, &mut state, &self.described.busid);
    }
}
