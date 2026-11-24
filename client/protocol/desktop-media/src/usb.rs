//! USB forwarding (Experimental): the devices plugged into this PC, which the user attaches to
//! the host from the stream controls, served to it through libusb. On Windows libusb opens
//! only devices that have the WinUSB driver; [`usb_winusb`](crate::usb_winusb) names the
//! driver of every other one, and the picker says so instead of offering it. On Linux the
//! devices libusb can't serve (webcams, audio) go through the kernel's own USB/IP server
//! instead ([`usb_linux`](crate::usb_linux)).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nightjar_core::control::SendError;
use nightjar_core::http::{Availability, Feature, HostCapabilities, NativeCapabilities, Reason};
use nightjar_core::usb::{AttachError, Attachment, Forwarder, Phase, UsbDevice, UsbReason, UsbState};
use rusb::{TransferType, UsbContext};

use crate::strings::{en, tr, trf};

mod libusb;

const CLASS_HUB: u8 = 9;

/// Why a device plugged in here can't be forwarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Windows has another driver on it (the driver services, as Windows names them).
    NoWinUsb(String),
    /// It has isochronous endpoints (audio, video), which this client doesn't serve (Linux
    /// serves them through usbip-host).
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Isochronous,
    /// It has isochronous endpoints, and this computer lacks what Linux's USB/IP server needs.
    #[cfg(target_os = "linux")]
    Kernel(crate::usb_linux::Missing),
}

impl Blocked {
    pub fn message(&self) -> String {
        match self {
            Self::NoWinUsb(driver) => trf("No WinUSB driver ({}). Install WinUSB for this device, for example with Zadig, to forward it. Windows can’t use it here while WinUSB has it.", &[driver]),
            Self::Isochronous => tr("It has isochronous endpoints (audio or video), which Nightjar doesn’t forward on this computer.").to_owned(),
            #[cfg(target_os = "linux")]
            Self::Kernel(missing) => missing.message(),
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
    /// Linux's usbip-host serves it, not libusb: it has isochronous endpoints.
    pub kernel: bool,
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
    #[cfg(target_os = "linux")]
    let missing = crate::usb_linux::System::real().missing();
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
        let isochronous = blocked.is_none() && device.active_config_descriptor().is_ok_and(|c| has_isochronous(&c));
        #[cfg(target_os = "linux")]
        let kernel = isochronous && missing.is_none();
        #[cfg(target_os = "linux")]
        if let (true, Some(missing)) = (isochronous, &missing) { blocked = Some(Blocked::Kernel(missing.clone())); }
        #[cfg(not(target_os = "linux"))]
        let kernel = false;
        #[cfg(not(target_os = "linux"))]
        if isochronous { blocked = Some(Blocked::Isochronous); }
        let name = name.unwrap_or_else(|| format!("USB device {vid:04x}:{pid:04x}"));
        out.push(Local { busid, vid, pid, name, blocked, kernel });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.busid.cmp(&b.busid)));
    Ok(out)
}

/// Opens `local` and claims every interface, for [`nightjar_core::usb::Forwarder::attach`].
pub fn open(local: &Local) -> Result<Arc<dyn UsbDevice>, String> {
    if let Some(blocked) = &local.blocked { return Err(blocked.message()); }
    #[cfg(target_os = "linux")]
    if local.kernel {
        return crate::usb_linux::System::real().open(&local.busid, local.vid, local.pid).map(|device| device as Arc<dyn UsbDevice>);
    }
    let context = rusb::Context::new().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let devices = context.devices().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let device = devices.iter()
        .find(|d| busid(d.bus_number(), &d.port_numbers().unwrap_or_default()) == local.busid
            && d.device_descriptor().is_ok_and(|desc| (desc.vendor_id(), desc.product_id()) == (local.vid, local.pid)))
        .ok_or_else(|| tr("The device was unplugged.").to_owned())?;
    libusb::open_device(&device, &local.busid, local.vid, local.pid).map_err(open_error)
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

/// The stream controls' USB section: the devices plugged in here, and what the PC made of
/// each one the user attached.
pub struct Panel {
    forwarder: Arc<Forwarder>,
    /// Listed when the section is first drawn and on Refresh.
    local: Option<Vec<Local>>,
    /// The last thing that failed, until the next action.
    pub error: Option<String>,
    /// Devices being opened on their own thread (Linux's usbip-host asks for the password),
    /// and what failed there, until the next draw shows it.
    opening: Arc<Mutex<Opening>>,
    /// The devices (by [`auto_key`]) attached whenever they are plugged in during a stream.
    auto: Arc<Mutex<Vec<String>>>,
    /// Set when the user changed `auto`, until the settings are saved.
    auto_changed: bool,
    /// Dropped with the panel, which ends the thread that attaches `auto`'s devices.
    auto_thread: Option<std::sync::mpsc::Sender<()>>,
}

#[derive(Default)]
struct Opening {
    busids: Vec<String>,
    failed: Option<String>,
}

/// How often the automatic attach looks for newly plugged-in devices.
const AUTO_POLL: Duration = Duration::from_secs(2);

/// What the automatic list remembers a device by: its vendor and product id, so that it
/// matches on any port (Android's UsbForwarding.autoKey writes the same).
pub fn auto_key(vid: u16, pid: u16) -> String {
    format!("{vid:04x}:{pid:04x}")
}

/// The devices of `local` to attach automatically now: on the list, forwardable, not attached
/// or being opened, and not tried since they were plugged in. `tried` forgets the unplugged.
fn auto_due(local: &[Local], auto: &[String], busy: impl Fn(&str) -> bool, tried: &mut Vec<String>) -> Vec<Local> {
    tried.retain(|busid| local.iter().any(|l| l.busid == *busid));
    let mut due = Vec::new();
    for device in local {
        if device.blocked.is_some() || !auto.contains(&auto_key(device.vid, device.pid)) { continue; }
        if tried.contains(&device.busid) || busy(&device.busid) { continue; }
        tried.push(device.busid.clone());
        due.push(device.clone());
    }
    due
}

/// Opens and attaches `local`, which the caller marked as being opened; a failure shows on the panel.
fn attach_tracked(forwarder: &Forwarder, opening: &Mutex<Opening>, local: &Local) {
    let lock = || opening.lock().unwrap_or_else(PoisonError::into_inner);
    let attached = attach_now(forwarder, local);
    let mut opening = lock();
    opening.busids.retain(|b| *b != local.busid);
    if let Err(e) = attached { opening.failed = Some(e); }
}

impl Panel {
    /// `auto`: the devices to attach whenever they are plugged in (the `usb_auto` setting).
    pub fn new(forwarder: Forwarder, auto: Vec<String>) -> Self {
        let mut panel = Self {
            forwarder: Arc::new(forwarder), local: None, error: None, opening: Arc::default(),
            auto: Arc::new(Mutex::new(auto)), auto_changed: false, auto_thread: None,
        };
        panel.start_auto();
        panel
    }

    fn auto(&self) -> MutexGuard<'_, Vec<String>> {
        self.auto.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `local` is attached automatically when it is plugged in.
    pub fn is_auto(&self, local: &Local) -> bool {
        self.auto().contains(&auto_key(local.vid, local.pid))
    }

    /// Adds `local` to the automatic list or takes it off; the thread attaches an added one.
    pub fn set_auto(&mut self, local: &Local, on: bool) {
        let key = auto_key(local.vid, local.pid);
        {
            let mut auto = self.auto();
            if on == auto.contains(&key) { return; }
            if on { auto.push(key); } else { auto.retain(|k| *k != key); }
        }
        self.auto_changed = true;
        if on { self.start_auto(); }
    }

    /// The automatic list after the user changed it, once, for saving.
    pub fn take_auto_change(&mut self) -> Option<Vec<String>> {
        std::mem::take(&mut self.auto_changed).then(|| self.auto().clone())
    }

    /// Starts the thread that attaches the automatic list's devices, while the list has any.
    fn start_auto(&mut self) {
        if self.auto_thread.is_some() || self.auto().is_empty() { return; }
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let (forwarder, opening, auto) = (self.forwarder.clone(), self.opening.clone(), self.auto.clone());
        let spawned = std::thread::Builder::new().name("usb-auto".into()).spawn(move || {
            let mut tried = Vec::new();
            loop {
                let wanted = auto.lock().unwrap_or_else(PoisonError::into_inner).clone();
                match list() {
                    Ok(local) => {
                        let busy = |busid: &str| {
                            opening.lock().unwrap_or_else(PoisonError::into_inner).busids.iter().any(|b| b == busid)
                                || forwarder.list().iter().any(|a| a.is_active() && a.busid == busid)
                        };
                        for device in auto_due(&local, &wanted, busy, &mut tried) {
                            eprintln!("nightjar: USB {}: attaching {} automatically", device.busid, device.name);
                            opening.lock().unwrap_or_else(PoisonError::into_inner).busids.push(device.busid.clone());
                            attach_tracked(&forwarder, &opening, &device);
                        }
                    }
                    Err(e) => eprintln!("nightjar: USB: {e}"),
                }
                // The channel closes with the panel: the stream ended.
                if !matches!(stopped.recv_timeout(AUTO_POLL), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) { return; }
            }
        });
        match spawned {
            Ok(_) => self.auto_thread = Some(stop),
            Err(e) => self.error = Some(trf("USB devices can't be attached automatically: {}", &[&e])),
        }
    }

    fn opening(&self) -> MutexGuard<'_, Opening> {
        self.opening.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `local` is being opened.
    pub fn is_opening(&self, local: &Local) -> bool {
        self.opening().busids.contains(&local.busid)
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

    pub fn active_busid(&self, busid: &str) -> bool {
        self.attachments().iter().any(|a| a.is_active() && a.busid == busid)
    }

    pub fn take_opening_failed(&mut self) -> Option<String> {
        self.opening().failed.take()
    }

    pub fn attach_busid(&mut self, busid: &str) {
        if let Some(device) = self.local().iter().find(|d| d.busid == busid).cloned() {
            self.attach(&device);
        }
    }

    pub fn set_auto_busid(&mut self, busid: &str, on: bool) {
        if let Some(device) = self.local().iter().find(|d| d.busid == busid).cloned() {
            self.set_auto(&device, on);
        }
    }

    pub fn attach(&mut self, local: &Local) {
        self.error = None;
        if !local.kernel {
            if let Err(e) = attach_now(&self.forwarder, local) { self.error = Some(e); }
            return;
        }
        // Binding waits for the password: off the drawing thread.
        if self.is_opening(local) { return; }
        self.opening().busids.push(local.busid.clone());
        let busid = local.busid.clone();
        let (forwarder, opening, local) = (self.forwarder.clone(), self.opening.clone(), local.clone());
        let spawned = std::thread::Builder::new().name("usb-open".into()).spawn(move || attach_tracked(&forwarder, &opening, &local));
        if let Err(e) = spawned {
            self.opening().busids.retain(|b| *b != busid);
            self.error = Some(trf("The device could not be opened: {}", &[&e]));
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
            "This PC has not allowed USB devices for this device. On the PC, open the web UI’s Devices page and allow USB devices for it.",
        )),
        // A host without the native API is a 2.6.x one or another family: it doesn't offer it.
        Availability::Unsupported(_) => Some(en("The PC does not forward USB devices.")),
        Availability::Unknown => Some(en("Could not ask the PC whether it forwards USB devices.")),
    }
}

impl Drop for Panel {
    /// The stream is over: a device still being opened finds the forwarder ended.
    fn drop(&mut self) {
        self.forwarder.end();
    }
}

/// Opens `local` and asks the PC to attach it; what failed, for the user.
fn attach_now(forwarder: &Forwarder, local: &Local) -> Result<u32, String> {
    let attached = open(local).and_then(|device| forwarder.attach(device, &local.name).map_err(|e| attach_error(&e)));
    match &attached {
        Ok(request_id) => eprintln!("nightjar: USB {}: asked the PC to attach {} ({:04x}:{:04x}), request {request_id}", local.busid, local.name, local.vid, local.pid),
        Err(e) => eprintln!("nightjar: USB {}: attaching {} failed: {e}", local.busid, local.name),
    }
    attached
}

fn attach_error(e: &AttachError) -> String {
    match e {
        AttachError::Ended => tr("The stream ended.").to_owned(),
        AttachError::Busy => tr("The device is attached already.").to_owned(),
        AttachError::Invalid => tr("The device’s name or bus ID can’t be sent to the PC.").to_owned(),
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
            (_, UsbReason::Driver) => tr("The PC’s USB/IP driver is missing or failed.").to_owned(),
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
mod tests;
