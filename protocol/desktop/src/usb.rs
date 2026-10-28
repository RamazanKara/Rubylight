//! USB forwarding (Experimental): the devices plugged into this PC, which the user attaches to
//! the host from the stream controls, served to it through libusb. On Windows libusb opens
//! only devices that have the WinUSB driver; [`usb_winusb`](crate::usb_winusb) names the
//! driver of every other one, and the picker says so instead of offering it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nightjar_core::control::SendError;
use nightjar_core::usb::{wire, AttachError, Attachment, Blocking, BlockingDevice, Direction, Forwarder, Phase, Reply, Transfer, UsbDevice, UsbReason, UsbState};
use rusb::constants::{LIBUSB_ERROR_NO_DEVICE, LIBUSB_ERROR_OVERFLOW, LIBUSB_ERROR_PIPE, LIBUSB_ERROR_TIMEOUT, LIBUSB_SUCCESS};
use rusb::{TransferType, UsbContext};

use nightjar_core::http::{Availability, Feature, HostCapabilities, NativeCapabilities, Reason};

use crate::strings::{en, tr, trf};

/// How long one bulk or interrupt call waits before the transfer looks at its cancel flag.
const SLICE: Duration = Duration::from_millis(200);
/// How long a control transfer may take; libusb's synchronous ones can't be cancelled.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const URB_ZERO_PACKET: u32 = 0x40;
const CLASS_HUB: u8 = 9;

/// Why a device plugged in here can't be forwarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Windows has another driver on it (the driver services, as Windows names them).
    NoWinUsb(String),
    /// It has isochronous endpoints (audio, video), which this client doesn't serve.
    Isochronous,
}

impl Blocked {
    pub fn message(&self) -> String {
        match self {
            Self::NoWinUsb(driver) => trf("No WinUSB driver ({}). Install WinUSB for this device, for example with Zadig, to forward it. Windows can't use it here while WinUSB has it.", &[driver]),
            Self::Isochronous => tr("It has isochronous endpoints (audio or video), which Nightjar for Windows and Linux doesn't forward.").to_owned(),
        }
    }
}

/// A device plugged in here, as the picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub busid: String,
    pub vid: u16,
    pub pid: u16,
    pub name: String,
    pub blocked: Option<Blocked>,
}

/// Linux's name for the device at `ports` on `bus`: `1-2.4`.
pub fn busid(bus: u8, ports: &[u8]) -> String {
    let ports: Vec<String> = ports.iter().map(u8::to_string).collect();
    format!("{bus}-{}", ports.join("."))
}

fn has_isochronous(config: &rusb::ConfigDescriptor) -> bool {
    config.interfaces().flat_map(|i| i.descriptors().collect::<Vec<_>>()).any(|setting| setting.endpoint_descriptors().any(|e| e.transfer_type() == TransferType::Isochronous))
}

#[cfg(target_os = "linux")]
fn sysfs_name(busid: &str) -> Option<String> {
    let read = |field: &str| std::fs::read_to_string(format!("/sys/bus/usb/devices/{busid}/{field}")).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    match (read("manufacturer"), read("product")) {
        (Some(maker), Some(product)) if !product.starts_with(&maker) => Some(format!("{maker} {product}")),
        (_, Some(product)) => Some(product),
        (maker, None) => maker,
    }
}

/// The devices plugged in here, hubs left out, without opening any of them.
pub fn list() -> Result<Vec<Local>, String> {
    let context = rusb::Context::new().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let devices = context.devices().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    #[cfg(windows)]
    let nodes = crate::usb_winusb::nodes()?;
    let mut out = Vec::new();
    for device in devices.iter() {
        let Ok(descriptor) = device.device_descriptor() else { continue };
        let ports = device.port_numbers().unwrap_or_default();
        if descriptor.class_code() == CLASS_HUB || ports.is_empty() { continue; }
        let (vid, pid) = (descriptor.vendor_id(), descriptor.product_id());
        let busid = busid(device.bus_number(), &ports);
        #[cfg(windows)]
        let (name, mut blocked) = (
            crate::usb_winusb::description(&nodes, vid, pid, &ports),
            crate::usb_winusb::blocked(&nodes, vid, pid, &ports).map(Blocked::NoWinUsb),
        );
        #[cfg(target_os = "linux")]
        let (name, mut blocked) = (sysfs_name(&busid), None);
        #[cfg(not(any(windows, target_os = "linux")))]
        let (name, mut blocked) = (None::<String>, None);
        if blocked.is_none() && device.active_config_descriptor().is_ok_and(|c| has_isochronous(&c)) {
            blocked = Some(Blocked::Isochronous);
        }
        let name = name.unwrap_or_else(|| format!("USB device {vid:04x}:{pid:04x}"));
        out.push(Local { busid, vid, pid, name, blocked });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.busid.cmp(&b.busid)));
    Ok(out)
}

/// Opens `local` and claims every interface, for [`nightjar_core::usb::Forwarder::attach`].
pub fn open(local: &Local) -> Result<Arc<dyn UsbDevice>, String> {
    if let Some(blocked) = &local.blocked { return Err(blocked.message()); }
    let context = rusb::Context::new().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let devices = context.devices().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let device = devices.iter()
        .find(|d| busid(d.bus_number(), &d.port_numbers().unwrap_or_default()) == local.busid
            && d.device_descriptor().is_ok_and(|desc| (desc.vendor_id(), desc.product_id()) == (local.vid, local.pid)))
        .ok_or_else(|| tr("The device was unplugged.").to_owned())?;
    let descriptor = device.device_descriptor().map_err(open_error)?;
    let handle = device.open().map_err(open_error)?;
    // Linux: usb-storage, usbhid and the like let go while it is attached and take it back after.
    match handle.set_auto_detach_kernel_driver(true) {
        Ok(()) | Err(rusb::Error::NotSupported) => {}
        Err(e) => return Err(open_error(e)),
    }
    let configuration = handle.active_configuration().map_err(open_error)?;
    let config = device.active_config_descriptor().map_err(open_error)?;
    let mut configs = HashMap::new();
    for index in 0..descriptor.num_configurations() {
        match device.config_descriptor(index) {
            Ok(c) => { configs.insert(c.number(), Config { interfaces: c.interfaces().map(|i| i.number()).collect(), endpoints: endpoints(&c) }); }
            Err(e) => eprintln!("nightjar: USB {}: configuration descriptor {index} could not be read, so the host can't set it: {e}", local.busid),
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
        path: format!("/libusb/{}", local.busid),
        busid: local.busid.clone(),
        busnum: u32::from(device.bus_number()),
        devnum: u32::from(device.address()),
        speed: speed(device.speed()),
        vendor: local.vid,
        product: local.pid,
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
    claim(&opened.handle, &mut opened.state(), &interfaces).map_err(open_error)?;
    Ok(Arc::new(Blocking::new(opened)))
}

fn open_error(e: rusb::Error) -> String {
    match e {
        rusb::Error::Access if cfg!(windows) => tr("Windows refused to open the device; another program may be using it.").to_owned(),
        rusb::Error::Access => tr("No permission to open the device. Add a udev rule that gives your user access to it.").to_owned(),
        rusb::Error::Busy => tr("Another driver or program holds the device.").to_owned(),
        rusb::Error::NoDevice | rusb::Error::NotFound => tr("The device was unplugged.").to_owned(),
        rusb::Error::NotSupported if cfg!(windows) => Blocked::NoWinUsb(tr("another driver").to_owned()).message(),
        e => trf("The device could not be opened: {}", &[&e]),
    }
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
enum Standard {
    SetConfiguration(u8),
    SetInterface { interface: u8, setting: u8 },
    ClearHalt(u8),
    /// SET_FEATURE(PORT_RESET) to the device's port, which the host's driver sends to recover
    /// it, as Linux's USB/IP host takes it: the device is reset here.
    PortReset,
}

fn standard(setup: &[u8; 8]) -> Option<Standard> {
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
fn errno(code: i32) -> i32 {
    match code {
        LIBUSB_ERROR_PIPE => wire::EPIPE,
        LIBUSB_ERROR_NO_DEVICE => wire::ENODEV,
        LIBUSB_ERROR_OVERFLOW => wire::EOVERFLOW,
        LIBUSB_ERROR_TIMEOUT => wire::ETIMEDOUT,
        _ => wire::EPROTO,
    }
}

fn rusb_errno(e: rusb::Error) -> i32 {
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
fn read_sliced(length: usize, cancelled: &AtomicBool, mut call: impl FnMut(&mut [u8]) -> (i32, usize)) -> Reply {
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
fn write_sliced(data: &[u8], zero_packet: bool, cancelled: &AtomicBool, mut call: impl FnMut(&[u8]) -> (i32, usize)) -> Reply {
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
struct Config {
    interfaces: Vec<u8>,
    endpoints: HashMap<u8, (TransferType, u16)>,
}

struct State {
    /// The configuration the host sees; 0 after it unconfigured the device.
    configuration: u8,
    /// The configuration the device is in: unconfiguring only hides it (see [`set_configuration`]).
    active: u8,
    claimed: Vec<u8>,
    endpoints: HashMap<u8, (TransferType, u16)>,
    configs: HashMap<u8, Config>,
    closed: bool,
}

/// What changing the configuration and resetting the device need of a libusb handle.
trait Configure {
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
fn set_configuration(handle: &impl Configure, state: &mut State, value: u8, busid: &str) -> Reply {
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
fn reset_port(handle: &impl Configure, state: &mut State, busid: &str) -> Reply {
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
                let zero = transfer.flags & URB_ZERO_PACKET != 0 && !data.is_empty() && max_packet > 0 && data.len() % usize::from(max_packet) == 0;
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

/// The stream controls' USB section: the devices plugged in here, and what the PC made of
/// each one the user attached.
pub struct Panel {
    forwarder: Forwarder,
    /// Listed when the section is first drawn and on Refresh.
    local: Option<Vec<Local>>,
    /// The last thing that failed, until the next action.
    pub error: Option<String>,
}

impl Panel {
    pub fn new(forwarder: Forwarder) -> Self {
        Self { forwarder, local: None, error: None }
    }

    /// The devices here, listed now if they haven't been.
    pub fn local(&mut self) -> &[Local] {
        if self.local.is_none() { self.refresh(); }
        self.local.as_deref().unwrap_or_default()
    }

    pub fn refresh(&mut self) {
        match list() {
            Ok(local) => self.local = Some(local),
            Err(e) => {
                eprintln!("nightjar: USB: {e}");
                self.local = Some(Vec::new());
                self.error = Some(e);
            }
        }
    }

    pub fn attachments(&self) -> Vec<Attachment> {
        self.forwarder.list()
    }

    /// The attachment of `local` that is still going, if any.
    pub fn active(&self, local: &Local) -> Option<Attachment> {
        self.attachments().into_iter().find(|a| a.is_active() && a.busid == local.busid)
    }

    fn active_busid(&self, busid: &str) -> bool {
        self.attachments().iter().any(|a| a.is_active() && a.busid == busid)
    }

    pub fn attach(&mut self, local: &Local) {
        let attached = open(local).and_then(|device| self.forwarder.attach(device, &local.name).map_err(|e| attach_error(&e)));
        match attached {
            Ok(request_id) => {
                eprintln!("nightjar: USB {}: asked the PC to attach {} ({:04x}:{:04x}), request {request_id}", local.busid, local.name, local.vid, local.pid);
                self.error = None;
            }
            Err(e) => {
                eprintln!("nightjar: USB {}: attaching {} failed: {e}", local.busid, local.name);
                self.error = Some(e);
            }
        }
    }

    pub fn detach(&mut self, request_id: u32) {
        self.error = None;
        if !self.forwarder.detach(request_id) { self.error = Some(tr("The device was no longer attached.").to_owned()); }
    }

    pub fn on_message(&self, message: nightjar_core::usb::UsbMessage) {
        self.forwarder.on_message(message);
    }
}

/// Whether the PC forwards USB devices and lets this device attach them. `native` is the PC's
/// native capabilities, None when there was nothing to ask (a host without the native API).
pub fn availability(caps: &HostCapabilities, native: Option<Result<NativeCapabilities, String>>) -> Availability {
    match native {
        None => caps.availability(Feature::UsbForwarding),
        Some(Ok(native)) => caps.with_native(&native).availability(Feature::UsbForwarding),
        Some(Err(e)) => {
            eprintln!("nightjar: USB: the PC's capabilities could not be read: {e}");
            Availability::Unknown
        }
    }
}

/// Why the stream controls offer no USB devices although the setting is on, or None when the
/// PC forwards them and lets this device attach them.
pub fn refusal(availability: Availability) -> Option<&'static str> {
    match availability {
        Availability::Supported => None,
        Availability::Unsupported(Some(Reason::UsbPermission)) => Some(en(
            "This PC has not allowed USB devices for this device. On the PC, open the web UI's Devices page and allow USB devices for it.",
        )),
        // A host without the native API is a 2.6.x one or another family: it doesn't offer it.
        Availability::Unsupported(_) => Some(en("The PC does not forward USB devices.")),
        Availability::Unknown => Some(en("Could not ask the PC whether it forwards USB devices.")),
    }
}

/// The stream controls' USB section when the setting is on but the PC offers nothing: why.
pub fn show_refusal(ui: &mut egui::Ui, why: &str) {
    ui.label(tr("USB devices"));
    ui.colored_label(crate::theme::ERROR, tr(why));
}

/// The stream controls' USB section: each device plugged in here with Attach or Detach and
/// where it stands, a device that can't be forwarded with why, and what the PC made of the
/// ones that ended.
pub fn show(ui: &mut egui::Ui, panel: &mut Panel) {
    ui.horizontal(|ui| {
        ui.label(tr("USB devices"));
        if ui.button(tr("Refresh")).clicked() {
            panel.error = None;
            panel.refresh();
        }
    });
    let local = panel.local().to_vec();
    if local.is_empty() { ui.weak(tr("No USB devices are plugged in here.")); }
    let mut attach = None;
    let mut detach = None;
    for device in &local {
        ui.horizontal(|ui| {
            let active = panel.active(device);
            ui.label(format!("{} ({:04x}:{:04x}, {})", device.name, device.vid, device.pid, device.busid));
            match (&active, &device.blocked) {
                (Some(attachment), _) => {
                    ui.weak(phase_label(&attachment.phase));
                    if ui.button(tr("Detach")).clicked() { detach = Some(attachment.request_id); }
                }
                (None, Some(_)) => { ui.add_enabled(false, egui::Button::new(tr("Attach"))); }
                (None, None) => if ui.button(tr("Attach")).clicked() { attach = Some(device.clone()); },
            }
        });
        if let Some(blocked) = &device.blocked { ui.colored_label(crate::theme::ERROR, blocked.message()); }
    }
    // What the PC made of the ones that ended, newest first, while their device is still listed.
    let mut shown = std::collections::HashSet::new();
    for attachment in panel.attachments().iter().rev() {
        if attachment.is_active() || !phase_failed(&attachment.phase) || !shown.insert(attachment.busid.clone()) { continue; }
        if panel.active_busid(&attachment.busid) { continue; }
        ui.colored_label(crate::theme::ERROR, format!("{}: {}", attachment.name, phase_label(&attachment.phase)));
    }
    if let Some(error) = &panel.error { ui.colored_label(crate::theme::ERROR, error); }
    if let Some(device) = attach { panel.attach(&device); }
    if let Some(request_id) = detach { panel.detach(request_id); }
}

fn attach_error(e: &AttachError) -> String {
    match e {
        AttachError::Ended => tr("The stream ended.").to_owned(),
        AttachError::Busy => tr("The device is attached already.").to_owned(),
        AttachError::Invalid => tr("The device's name or bus ID can't be sent to the PC.").to_owned(),
        AttachError::Send(SendError::Unsupported) => tr("The PC does not forward USB devices.").to_owned(),
        AttachError::Send(e) => trf("The request did not reach the PC: {}", &[&format!("{e:?}")]),
    }
}

/// What the user sees of an attachment's phase.
pub fn phase_label(phase: &Phase) -> String {
    match phase {
        Phase::Asking => tr("Asking the PC").to_owned(),
        Phase::Opening => tr("Connecting").to_owned(),
        Phase::Attached => tr("Attached").to_owned(),
        Phase::Ended { error: Some(error), .. } => trf("Failed: {}", &[error]),
        Phase::Ended { state, reason, error: None } => match (state, reason) {
            (_, UsbReason::Permission) => tr("The PC has not allowed USB devices for this client.").to_owned(),
            (_, UsbReason::Disabled) => tr("USB forwarding is off on the PC.").to_owned(),
            (_, UsbReason::Limit) => tr("The PC has as many USB devices attached as it allows.").to_owned(),
            (_, UsbReason::Driver) => tr("The PC's USB/IP driver is missing or failed.").to_owned(),
            (_, UsbReason::TunnelTimeout) => tr("The connection to the PC timed out.").to_owned(),
            (_, UsbReason::DeviceError) => tr("The device stopped answering.").to_owned(),
            (_, UsbReason::SessionEnd) => tr("Detached when the stream ended.").to_owned(),
            (UsbState::Refused, _) => tr("The PC refused the device.").to_owned(),
            (UsbState::Failed, _) => tr("Attaching failed on the PC.").to_owned(),
            _ => tr("Detached").to_owned(),
        },
    }
}

/// Whether the phase is an ending the user should notice.
pub fn phase_failed(phase: &Phase) -> bool {
    matches!(phase, Phase::Ended { error: Some(_), .. } | Phase::Ended { state: UsbState::Refused | UsbState::Failed, .. })
        || matches!(phase, Phase::Ended { reason, .. } if !matches!(reason, UsbReason::None | UsbReason::SessionEnd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busids_follow_linux() {
        assert_eq!(busid(1, &[2]), "1-2");
        assert_eq!(busid(3, &[1, 4, 2]), "3-1.4.2");
        assert!(wire::valid_busid(&busid(255, &[15, 15, 15, 15, 15, 15, 15])));
    }

    /// The native answer with these features and the usb permission.
    fn native(features: &str, usb: bool) -> NativeCapabilities {
        serde_json::from_str(&format!(
            "{{\"api_version\":1,\"host\":{{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"uid\"}},\"features\":[{features}],\
             \"permissions\":{{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false,\"usb\":{usb}}}}}"
        ))
        .expect("the answer parses")
    }

    #[test]
    fn the_stream_controls_offer_usb_devices_only_when_the_pc_does() {
        let caps = crate::ui::files::tests::native_answer().capabilities;
        let offered = |native| refusal(availability(&caps, Some(native)));
        assert_eq!(offered(Ok(native("\"usb\"", true))), None);
        assert_eq!(
            offered(Ok(native("\"usb\"", false))),
            Some("This PC has not allowed USB devices for this device. On the PC, open the web UI's Devices page and allow USB devices for it.")
        );
        // The permission alone is no offer: the host turned USB forwarding off.
        assert_eq!(offered(Ok(native("\"files.upload\"", true))), Some("The PC does not forward USB devices."));
        assert_eq!(offered(Err("timed out".into())), Some("Could not ask the PC whether it forwards USB devices."));
        // A host without the native API is not asked, and offers nothing.
        let old = crate::host_page::tests::info("SUNSHINE_SERVER_FREE", "<RustHostVersion>2.6.4</RustHostVersion>").capabilities;
        assert_eq!(refusal(availability(&old, None)), Some("The PC does not forward USB devices."));
        assert!(!old.has(Feature::NativeApi));
    }

    #[test]
    fn state_changing_requests_go_to_libusb() {
        assert_eq!(standard(&[0x00, 9, 1, 0, 0, 0, 0, 0]), Some(Standard::SetConfiguration(1)));
        assert_eq!(standard(&[0x01, 11, 1, 0, 2, 0, 0, 0]), Some(Standard::SetInterface { interface: 2, setting: 1 }));
        assert_eq!(standard(&[0x02, 1, 0, 0, 0x81, 0, 0, 0]), Some(Standard::ClearHalt(0x81)));
        assert_eq!(standard(&[0x23, 3, 4, 0, 1, 0, 0, 0]), Some(Standard::PortReset));
        // GET_DESCRIPTOR, a class request, CLEAR_FEATURE and SET_FEATURE of other features pass through.
        assert_eq!(standard(&[0x80, 6, 0, 1, 0, 0, 18, 0]), None);
        assert_eq!(standard(&[0x21, 9, 0, 2, 0, 0, 8, 0]), None);
        assert_eq!(standard(&[0x02, 1, 1, 0, 0x81, 0, 0, 0]), None);
        assert_eq!(standard(&[0x23, 3, 8, 0, 1, 0, 0, 0]), None);
    }

    /// A handle that logs what it is asked, and fails what the test says.
    #[derive(Default)]
    struct Fake {
        log: std::cell::RefCell<Vec<String>>,
        set: Option<rusb::Error>,
        reset: Option<rusb::Error>,
    }

    impl Fake {
        fn note(&self, call: String, result: Option<rusb::Error>) -> rusb::Result<()> {
            self.log.borrow_mut().push(call);
            result.map_or(Ok(()), Err)
        }
        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut self.log.borrow_mut())
        }
    }

    impl Configure for Fake {
        fn claim_interface(&self, interface: u8) -> rusb::Result<()> { self.note(format!("claim {interface}"), None) }
        fn release_interface(&self, interface: u8) -> rusb::Result<()> { self.note(format!("release {interface}"), None) }
        fn set_auto_detach_kernel_driver(&self, on: bool) -> rusb::Result<()> { self.note(format!("detach {on}"), None) }
        fn set_active_configuration(&self, value: u8) -> rusb::Result<()> { self.note(format!("set {value}"), self.set) }
        fn reset(&self) -> rusb::Result<()> { self.note("reset".into(), self.reset) }
    }

    /// In configuration 1 (interfaces 0 and 1, endpoint 0x81) with both claimed; 2 has interface 0
    /// and endpoint 0x82.
    fn configured() -> State {
        let config = |interfaces: &[u8], endpoint| Config { interfaces: interfaces.to_vec(), endpoints: HashMap::from([(endpoint, (TransferType::Interrupt, 8))]) };
        let configs = HashMap::from([(1, config(&[0, 1], 0x81)), (2, config(&[0], 0x82))]);
        State { configuration: 1, active: 1, claimed: vec![0, 1], endpoints: configs[&1].endpoints.clone(), configs, closed: false }
    }

    #[test]
    fn a_configuration_the_device_refuses_leaves_the_old_one_claimed() {
        let (fake, mut state) = (Fake { set: Some(rusb::Error::Pipe), ..Fake::default() }, configured());
        assert_eq!(set_configuration(&fake, &mut state, 2, "1-1").status, wire::EPIPE);
        // The kernel's drivers stay off the interfaces in between.
        assert_eq!(fake.calls(), ["detach false", "release 0", "release 1", "set 2", "detach true", "claim 0", "claim 1"]);
        assert_eq!((state.configuration, state.active, &state.claimed[..]), (1, 1, &[0, 1][..]));
        assert!(state.endpoints.contains_key(&0x81));
    }

    #[test]
    fn a_new_configuration_claims_its_own_interfaces() {
        let (fake, mut state) = (Fake::default(), configured());
        assert_eq!(set_configuration(&fake, &mut state, 2, "1-1").status, 0);
        assert_eq!(fake.calls(), ["detach false", "release 0", "release 1", "set 2", "detach true", "claim 0"]);
        assert_eq!((state.configuration, state.active, &state.claimed[..]), (2, 2, &[0][..]));
        assert!(state.endpoints.contains_key(&0x82) && !state.endpoints.contains_key(&0x81));
        // One the device doesn't have is refused before anything changes.
        assert_eq!(set_configuration(&fake, &mut state, 3, "1-1").status, wire::EINVAL);
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn unconfiguring_hides_the_configuration_until_the_host_sets_it_again() {
        let (fake, mut state) = (Fake::default(), configured());
        assert_eq!(set_configuration(&fake, &mut state, 0, "1-1").status, 0);
        assert_eq!((state.configuration, state.active, &state.claimed[..]), (0, 1, &[0, 1][..]));
        assert!(state.endpoints.is_empty());
        assert_eq!(set_configuration(&fake, &mut state, 1, "1-1").status, 0);
        assert_eq!(state.configuration, 1);
        assert!(state.endpoints.contains_key(&0x81));
        // The device never left configuration 1, and its interfaces stayed claimed.
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_port_reset_resets_the_device_and_claims_its_interfaces_again() {
        let (fake, mut state) = (Fake::default(), configured());
        assert_eq!(reset_port(&fake, &mut state, "1-1").status, 0);
        assert_eq!(fake.calls(), ["reset", "claim 0", "claim 1"]);
        assert_eq!(&state.claimed[..], [0, 1]);
        // A device that came back as another one is gone.
        let fake = Fake { reset: Some(rusb::Error::NotFound), ..Fake::default() };
        assert_eq!(reset_port(&fake, &mut state, "1-1").status, wire::ENODEV);
        assert_eq!(fake.calls(), ["reset"]);
    }

    #[test]
    fn libusb_errors_become_usbip_ones() {
        assert_eq!(errno(LIBUSB_ERROR_PIPE), wire::EPIPE);
        assert_eq!(errno(LIBUSB_ERROR_NO_DEVICE), wire::ENODEV);
        assert_eq!(errno(rusb::constants::LIBUSB_ERROR_IO), wire::EPROTO);
        assert_eq!(rusb_errno(rusb::Error::Pipe), wire::EPIPE);
        assert_eq!(rusb_errno(rusb::Error::Timeout), wire::ETIMEDOUT);
    }

    #[test]
    fn a_read_keeps_what_each_slice_brought() {
        // 64-byte packets: two slices run out after one packet each, the third ends short.
        let mut slices = vec![(LIBUSB_ERROR_TIMEOUT, 64), (LIBUSB_ERROR_TIMEOUT, 64), (LIBUSB_SUCCESS, 10)].into_iter();
        let mut offered = Vec::new();
        let reply = read_sliced(512, &AtomicBool::new(false), |buffer| {
            offered.push(buffer.len());
            let (code, n) = slices.next().unwrap();
            buffer[..n].fill(offered.len() as u8);
            (code, n)
        });
        assert_eq!(offered, [512, 448, 384]);
        assert_eq!(reply.status, 0);
        assert_eq!(reply.data.len(), 138);
        assert_eq!((reply.data[0], reply.data[64], reply.data[137]), (1, 2, 3));
    }

    #[test]
    fn a_waiting_read_ends_when_cancelled_or_failed() {
        let cancelled = AtomicBool::new(false);
        let mut calls = 0;
        let reply = read_sliced(8, &cancelled, |_| {
            calls += 1;
            if calls == 3 { cancelled.store(true, Ordering::SeqCst); }
            (LIBUSB_ERROR_TIMEOUT, 0)
        });
        assert_eq!((reply.status, calls), (wire::ECONNRESET, 3));
        assert_eq!(read_sliced(8, &AtomicBool::new(false), |_| (LIBUSB_ERROR_PIPE, 0)).status, wire::EPIPE);
    }

    #[test]
    fn a_write_goes_on_after_a_slice_and_ends_with_the_zero_packet_asked_for() {
        let mut sent = Vec::new();
        let mut slices = vec![(LIBUSB_ERROR_TIMEOUT, 64), (LIBUSB_SUCCESS, 64), (LIBUSB_SUCCESS, 0)].into_iter();
        let reply = write_sliced(&[7; 128], true, &AtomicBool::new(false), |rest| {
            sent.push(rest.len());
            slices.next().unwrap()
        });
        assert_eq!((reply.status, reply.sent), (0, 128));
        assert_eq!(sent, [128, 64, 0]);
        let mut calls = 0;
        let reply = write_sliced(&[7; 10], false, &AtomicBool::new(false), |rest| { calls += 1; (LIBUSB_SUCCESS, rest.len()) });
        assert_eq!((reply.sent, calls), (10, 1));
        let mut calls = 0;
        let reply = write_sliced(&[], false, &AtomicBool::new(false), |rest| { calls += 1; (LIBUSB_SUCCESS, rest.len()) });
        assert_eq!((reply.sent, calls), (0, 1));
    }

    #[test]
    fn refusals_and_failures_say_why() {
        let ended = |state, reason| Phase::Ended { state, reason, error: None };
        let refused = ended(UsbState::Refused, UsbReason::Permission);
        assert_eq!(phase_label(&refused), "The PC has not allowed USB devices for this client.");
        assert!(phase_failed(&refused));
        assert!(phase_failed(&ended(UsbState::Failed, UsbReason::None)));
        assert!(phase_failed(&ended(UsbState::Detached, UsbReason::DeviceError)));
        let error = Phase::Ended { state: UsbState::Failed, reason: UsbReason::DeviceError, error: Some("the USB tunnel failed".into()) };
        assert_eq!(phase_label(&error), "Failed: the USB tunnel failed");
        // The user's own detach and the stream's end are no failures.
        assert!(!phase_failed(&ended(UsbState::Detached, UsbReason::None)));
        assert!(!phase_failed(&ended(UsbState::Detached, UsbReason::SessionEnd)));
        assert_eq!(attach_error(&AttachError::Send(SendError::Unsupported)), "The PC does not forward USB devices.");
    }

    #[test]
    fn blocked_devices_say_why() {
        assert!(Blocked::NoWinUsb("USBSTOR".into()).message().contains("USBSTOR"));
        assert!(Blocked::Isochronous.message().contains("isochronous"));
    }
}
