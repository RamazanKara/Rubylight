//! USB forwarding (Experimental): the devices plugged into this PC, which the user attaches to
//! the host from the stream controls. On Windows non-isochronous devices go through libusb and
//! WinUSB isochronous devices go directly through Microsoft's inbox `winusb.dll` ([`winusb`]),
//! requiring no driver beyond WinUSB ([`usb_winusb`](crate::usb_winusb) names any other driver so
//! the picker explains why). On Linux isochronous devices prefer the kernel's USB/IP server
//! ([`usb_linux`](crate::usb_linux)) when available and fall back to libusb's asynchronous
//! isochronous transfers ([`iso`]) otherwise; macOS serves all devices through libusb ([`iso`]).
//! A Linux device libusb may not open goes to the kernel's USB/IP server instead when the user
//! attaches it by hand (its password prompt never comes up by itself); a Windows
//! device another driver holds goes through usbipd-win when the user asks for it
//! (`usb_windows`). A controller being forwarded pauses as a gamepad
//! ([`usb_arbitration`](crate::usb_arbitration)).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nightjar_core::control::SendError;
use nightjar_core::http::{Availability, Feature, HostCapabilities, NativeCapabilities, Reason};
use nightjar_core::usb::{AttachError, Attachment, Forwarder, Phase, UsbDevice, UsbReason, UsbState};
#[cfg(any(windows, target_os = "linux"))]
use rusb::TransferType;
use rusb::UsbContext;

use crate::strings::{en, tr, trf};

mod auto;
mod iso;
mod libusb;
mod winusb;

pub use auto::auto_key;
#[cfg(target_os = "linux")]
pub(crate) use auto::note_password_failed;
#[cfg(all(target_os = "linux", test))]
pub(crate) use auto::password_failures;

const CLASS_HUB: u8 = 9;

/// Transport backend chosen for an unblocked USB device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Backend {
    /// libusb (control, bulk, interrupt, and async isochronous transfers via [`iso`]).
    Libusb,
    /// Windows inbox `winusb.dll` (control, bulk, interrupt, and isochronous via [`winusb`]).
    WinUsbIso,
    /// Linux kernel `usbip-host` server ([`usb_linux`](crate::usb_linux)).
    LinuxKernel,
}

/// Platform capabilities relevant to choosing a USB forwarding transport backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Platform {
    Windows { uses_winusb: bool },
    Linux { has_kernel_usbip: bool },
    MacOs,
}

/// Selects the USB transport for an unblocked device. Isochronous devices are supported on all
/// desktop platforms without blocking: Windows inbox WinUSB uses [`Backend::WinUsbIso`], Linux
/// prefers `usbip-host` when available and falls back to [`Backend::Libusb`], and macOS uses
/// [`Backend::Libusb`].
#[allow(dead_code)]
pub(crate) fn select_backend(platform: Platform, isochronous: bool) -> Backend {
    match platform {
        Platform::Windows { uses_winusb } if isochronous && uses_winusb => Backend::WinUsbIso,
        Platform::Linux { has_kernel_usbip } if isochronous && has_kernel_usbip => Backend::LinuxKernel,
        _ => Backend::Libusb,
    }
}

/// Why a device plugged in here can't be forwarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Windows has another driver on it (the driver services, as Windows names them).
    NoWinUsb(String),
    /// Retained for API and localization compatibility; never returned now that every desktop
    /// platform forwards isochronous endpoints.
    #[allow(dead_code)]
    Isochronous,
    /// Retained for API and localization compatibility; Linux falls back to libusb isochronous
    /// transfers when `usbip-host` is unavailable.
    #[cfg(target_os = "linux")]
    #[allow(dead_code)]
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

/// What usbipd-win can do for a device Windows has another driver on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsbipdOffer {
    /// Nothing: the device opens without it, or usbipd-win doesn't list it.
    #[default]
    None,
    /// usbipd-win isn't installed; it would forward the device with Windows' driver left on.
    NotInstalled,
    /// usbipd-win lists it: the user may forward it through usbipd-win.
    Ready,
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
    /// Windows: what usbipd-win can do for it while another driver has it.
    pub usbipd: UsbipdOffer,
}

/// Why `local` can't be forwarded as it is, with what usbipd-win would change about that.
pub fn blocked_message(local: &Local) -> Option<String> {
    let blocked = local.blocked.as_ref()?.message();
    Some(match (&local.usbipd, local.blocked.as_ref()?) {
        (UsbipdOffer::NotInstalled, _) => format!("{blocked} {}", tr("Or install usbipd-win, then press Refresh, to forward it while Windows keeps its own driver.")),
        (UsbipdOffer::Ready, Blocked::NoWinUsb(driver)) => trf("Another driver has this device ({}). Forward it with usbipd-win, which leaves that driver on it, or install WinUSB for it.", &[driver]),
        _ => blocked,
    })
}

/// The hub ports in Linux's busid (`1-2.4` is 2, 4).
pub fn ports_of(busid: &str) -> Vec<u8> {
    busid.split_once('-').map(|(_, ports)| ports.split('.').filter_map(|p| p.parse().ok()).collect()).unwrap_or_default()
}

/// Linux's name for the device at `ports` on `bus`: `1-2.4`.
pub fn busid(bus: u8, ports: &[u8]) -> String {
    let ports: Vec<String> = ports.iter().map(u8::to_string).collect();
    format!("{bus}-{}", ports.join("."))
}

#[cfg(any(windows, target_os = "linux"))]
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

/// The devices plugged in here, hubs left out, without opening any of them; on Windows, with
/// what usbipd-win can do for those another driver has.
pub fn list() -> Result<Vec<Local>, String> {
    let context = rusb::Context::new().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    #[allow(unused_mut)]
    let mut local = list_with(&context)?;
    #[cfg(windows)]
    offer_usbipd(&mut local);
    Ok(local)
}

/// Marks the devices another driver has that usbipd-win could forward. Runs `usbipd state` only
/// when there is such a device and usbipd-win is installed.
#[cfg(windows)]
fn offer_usbipd(local: &mut [Local]) {
    let blocked = |l: &Local| matches!(l.blocked, Some(Blocked::NoWinUsb(_)));
    if !local.iter().any(blocked) { return; }
    let Some(cli) = crate::usb_windows::Cli::find() else {
        local.iter_mut().filter(|l| blocked(l)).for_each(|l| l.usbipd = UsbipdOffer::NotInstalled);
        return;
    };
    use crate::usb_windows::Usbipd;
    let (state, nodes) = match (cli.state(), crate::usb_winusb::nodes()) {
        (Ok(state), Ok(nodes)) => (state, nodes),
        (Err(e), _) | (_, Err(e)) => return eprintln!("nightjar: USB: {e}"),
    };
    for device in local.iter_mut().filter(|l| blocked(l)) {
        // Only a device Nightjar can tell from every other: one Windows node at its port.
        let ids = crate::usb_winusb::instance_ids(&nodes, device.vid, device.pid, &ports_of(&device.busid));
        let listed = matches!(ids.as_slice(), [id] if crate::usb_windows::find(&state, id).is_some_and(|d| d.busid().is_some()));
        if listed { device.usbipd = UsbipdOffer::Ready; }
    }
}

/// Enumerates devices using a caller-owned `rusb::Context` so background polling avoids
/// creating a fresh context and hotplug thread on every tick.
fn list_with(context: &rusb::Context) -> Result<Vec<Local>, String> {
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
        let (name, blocked) = (
            crate::usb_winusb::description(&nodes, vid, pid, &ports),
            crate::usb_winusb::blocked(&nodes, vid, pid, &ports).map(Blocked::NoWinUsb),
        );
        #[cfg(target_os = "linux")]
        let (name, blocked) = (sysfs_name(&busid), None);
        #[cfg(not(any(windows, target_os = "linux")))]
        let (name, blocked) = (None::<String>, None);
        #[cfg(target_os = "linux")]
        let kernel = {
            let isochronous = device.active_config_descriptor().is_ok_and(|c| has_isochronous(&c));
            select_backend(Platform::Linux { has_kernel_usbip: missing.is_none() }, isochronous)
                == Backend::LinuxKernel
        };
        #[cfg(not(target_os = "linux"))]
        let kernel = false;
        let name = name.unwrap_or_else(|| format!("USB device {vid:04x}:{pid:04x}"));
        out.push(Local { busid, vid, pid, name, blocked, kernel, usbipd: UsbipdOffer::None });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.busid.cmp(&b.busid)));
    Ok(out)
}

/// Opens `local` and claims every interface, for [`nightjar_core::usb::Forwarder::attach`].
/// `by_hand`: the user started this attach, so Linux's USB/IP server, behind a password prompt,
/// may take a device that goes there first ([`kernel_by_hand`]) or one libusb is refused
/// ([`open_or_kernel`]).
pub fn open(local: &Local, by_hand: bool) -> Result<Arc<dyn UsbDevice>, String> {
    if let Some(blocked) = &local.blocked { return Err(blocked.message()); }
    #[cfg(target_os = "linux")]
    if local.kernel {
        if !by_hand {
            eprintln!("nightjar: USB {}: Linux's USB/IP server needs the password, which only an attach by hand asks for", local.busid);
        }
        return kernel_by_hand(!by_hand, || {
            crate::usb_linux::System::real().open(&local.busid, local.vid, local.pid).map(|device| device as Arc<dyn UsbDevice>)
        });
    }
    let context = rusb::Context::new().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let devices = context.devices().map_err(|e| trf("USB devices could not be listed: {}", &[&e]))?;
    let device = devices.iter()
        .find(|d| busid(d.bus_number(), &d.port_numbers().unwrap_or_default()) == local.busid
            && d.device_descriptor().is_ok_and(|desc| (desc.vendor_id(), desc.product_id()) == (local.vid, local.pid)))
        .ok_or_else(|| tr("The device was unplugged.").to_owned())?;
    #[cfg(windows)]
    {
        let isochronous = device.active_config_descriptor().is_ok_and(|c| has_isochronous(&c));
        if isochronous {
            let ports = device.port_numbers().unwrap_or_default();
            let nodes = crate::usb_winusb::nodes()?;
            let uses_winusb = crate::usb_winusb::uses_winusb(&nodes, local.vid, local.pid, &ports);
            if select_backend(Platform::Windows { uses_winusb }, true) == Backend::WinUsbIso {
                let paths = crate::usb_winusb::interface_paths(local.vid, local.pid, &ports)?;
                return winusb::open_device(&device, &local.busid, local.vid, local.pid, &paths);
            }
        }
    }
    let opened = libusb::open_device(&device, &local.busid, local.vid, local.pid);
    #[cfg(target_os = "linux")]
    let kernel = || {
        let system = crate::usb_linux::System::real();
        if let Some(missing) = system.missing() {
            eprintln!("nightjar: USB {}: libusb may not open it and Linux's USB/IP server can't take over: {missing:?}", local.busid);
            return None;
        }
        eprintln!("nightjar: USB {}: libusb may not open it; handing it to Linux's USB/IP server", local.busid);
        Some(system.open(&local.busid, local.vid, local.pid).map(|device| device as Arc<dyn UsbDevice>))
    };
    #[cfg(not(target_os = "linux"))]
    let kernel = || None;
    // Only Linux's fallback asks for a password, which an automatic attach never brings up.
    let unattended = cfg!(target_os = "linux") && !by_hand;
    open_or_kernel(opened, unattended, kernel)
}

/// Opens `local` through usbipd-win, which the user asked for: Windows keeps its own driver on
/// it, and usbipd-win lends it while it is attached ([`crate::usb_windows`]).
#[cfg(windows)]
fn open_usbipd(local: &Local) -> Result<Arc<dyn UsbDevice>, String> {
    let cli = crate::usb_windows::Cli::find().ok_or_else(|| tr("usbipd-win isn’t installed. Install it, then press Refresh.").to_owned())?;
    let journal = crate::usb_windows::Journal::real()?;
    let nodes = crate::usb_winusb::nodes()?;
    let instance_id = match crate::usb_winusb::instance_ids(&nodes, local.vid, local.pid, &ports_of(&local.busid)).as_slice() {
        [] => return Err(tr("The device was unplugged.").to_owned()),
        [id] => id.clone(),
        _ => return Err(tr("Two identical devices are on the same port number of different USB controllers, so Nightjar can’t tell them apart for usbipd-win. Plug one into another port, then press Refresh.").to_owned()),
    };
    let picked = crate::usb_windows::Picked { busid: local.busid.clone(), vid: local.vid, pid: local.pid, instance_id, class: device_class(local) };
    crate::usb_windows::open_with(Arc::new(cli), journal, crate::usb_windows::LOOPBACK, &picked).map(|device| device as Arc<dyn UsbDevice>)
}

#[cfg(not(windows))]
fn open_usbipd(_: &Local) -> Result<Arc<dyn UsbDevice>, String> {
    Err(trf("The device could not be opened: {}", &[&"usbipd-win"]))
}

/// bDeviceClass of `local`, or its first interface's class when that is 0, read from the
/// descriptors libusb lists without opening it; 0 when it can't tell.
#[cfg(windows)]
fn device_class(local: &Local) -> u8 {
    let Ok(context) = rusb::Context::new() else { return 0 };
    let Ok(devices) = context.devices() else { return 0 };
    let device = devices.iter().find(|d| busid(d.bus_number(), &d.port_numbers().unwrap_or_default()) == local.busid
        && d.device_descriptor().is_ok_and(|desc| (desc.vendor_id(), desc.product_id()) == (local.vid, local.pid)));
    device.map_or(0, |device| match device.device_descriptor().map(|d| d.class_code()) {
        Ok(class) if class != 0 => class,
        _ => device.active_config_descriptor().ok()
            .and_then(|config| config.interfaces().next().and_then(|i| i.descriptors().next()).map(|s| s.class_code()))
            .unwrap_or(0),
    })
}

/// How a device is opened for an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// libusb, WinUSB or Linux's usbip-host, as [`open`] chooses.
    Direct,
    /// usbipd-win, which the user asked for ([`open_usbipd`]).
    Usbipd,
}

/// libusb's answer for a device, with Linux's own USB/IP server ([`usb_linux`](crate::usb_linux))
/// taking over when libusb was refused permission (`EACCES`, no udev rule gives the user the
/// device): `kernel` opens it there, or is None when that can't run on this computer. The udev
/// advice stays when there is no such fallback, and follows the fallback's own failure. That
/// server asks for the password (polkit's `auth_admin`, every time), so an `unattended` attach
/// (the automatic one) never starts it: it keeps the udev advice and counts as a password that
/// did not go through, which leaves the device to an attach by hand ([`auto`]).
fn open_or_kernel<T>(opened: Result<T, rusb::Error>, unattended: bool, kernel: impl FnOnce() -> Option<Result<T, String>>) -> Result<T, String> {
    match opened {
        Err(rusb::Error::Access) if unattended => {
            auto::note_password_failed();
            Err(open_error(rusb::Error::Access))
        }
        Err(rusb::Error::Access) => match kernel() {
            Some(Ok(device)) => Ok(device),
            Some(Err(why)) => Err(format!("{why} {}", open_error(rusb::Error::Access))),
            None => Err(open_error(rusb::Error::Access)),
        },
        other => other.map_err(open_error),
    }
}

/// Linux's USB/IP server for a device that goes there first (`Local::kernel`: a webcam or an
/// audio device). It asks for the password (polkit's `auth_admin`, every time), so an
/// `unattended` attach (the automatic one) never starts `kernel`: it says to press Attach and
/// counts as a password that did not go through, which leaves the device to an attach by hand
/// ([`auto`]). A webcam on the automatic list never prompts on a plug-in by itself.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn kernel_by_hand<T>(unattended: bool, kernel: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    if unattended {
        auto::note_password_failed();
        return Err(tr("Forwarding this device needs your password, so it isn’t attached automatically. Press Attach to forward it.").to_owned());
    }
    kernel()
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
type ListedResult = Option<Result<Vec<Local>, String>>;

/// Local USB devices the user attaches and detaches from the stream controls.
pub struct Panel {
    forwarder: Arc<Forwarder>,
    /// Listed in the background when the section is first drawn and on Refresh.
    local: Option<Vec<Local>>,
    /// The last thing that failed, until the next action.
    pub error: Option<String>,
    /// Devices being opened on their own thread, and what failed there, until the next draw shows it.
    opening: Arc<Mutex<Opening>>,
    /// Background device enumeration in flight (`refresh` / `local`).
    listing: Arc<AtomicBool>,
    listed: Arc<Mutex<ListedResult>>,
    /// The devices (by [`auto_key`]) attached whenever they are plugged in during a stream.
    auto: Arc<Mutex<Vec<String>>>,
    /// Set when the user changed `auto`, until the settings are saved.
    auto_changed: bool,
    /// Dropped with the panel, which ends the thread that attaches `auto`'s devices.
    auto_thread: Option<std::sync::mpsc::Sender<()>>,
    /// The devices (by [`auto_key`]) whose password prompt was cancelled: the automatic attach
    /// leaves them alone until the user attaches one by hand or ticks Automatic again.
    quiet: Arc<Mutex<Vec<String>>>,
    /// What this stream's forwarded devices are claimed as ([`crate::usb_arbitration`]), so
    /// the panel's end gives every controller among them back.
    owner: u64,
    /// The shares Nightjar left in usbipd-win when it stopped, as the last list found them.
    usbipd_left: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Opening {
    busids: Vec<String>,
    failed: Option<String>,
}

impl Opening {
    /// Marks `busid` as currently opening; returns `false` if already in flight.
    fn try_start(&mut self, busid: &str) -> bool {
        if self.busids.iter().any(|b| b == busid) {
            return false;
        }
        self.busids.push(busid.to_owned());
        true
    }

    fn finish(&mut self, busid: &str) {
        self.busids.retain(|b| b != busid);
    }
}

/// Ensures `busid` is removed from `Opening::busids` even if `open`/`attach` panics.
struct OpeningGuard<'a> {
    opening: &'a Mutex<Opening>,
    busid: String,
}

impl Drop for OpeningGuard<'_> {
    fn drop(&mut self) {
        self.opening.lock().unwrap_or_else(PoisonError::into_inner).finish(&self.busid);
    }
}

/// Opens and attaches `local`, which the caller marked as being opened (`by_hand`: the user
/// started it, not the automatic attach); returns whether attach succeeded, and records any
/// failure on the panel. A failure behind the password puts `local` on `quiet`
/// ([`auto::hushed_on_password_failure`]).
fn attach_tracked(forwarder: &Forwarder, owner: u64, (route, by_hand): (Route, bool), opening: &Mutex<Opening>, quiet: &Mutex<Vec<String>>, local: &Local) -> bool {
    let _guard = OpeningGuard { opening, busid: local.busid.clone() };
    match auto::hushed_on_password_failure(quiet, local, || attach_now(forwarder, owner, route, by_hand, local)) {
        Ok(_) => true,
        Err(e) => {
            opening.lock().unwrap_or_else(PoisonError::into_inner).failed = Some(e);
            false
        }
    }
}

impl Panel {
    /// `auto`: the devices to attach whenever they are plugged in (the `usb_auto` setting).
    pub fn new(forwarder: Forwarder, auto: Vec<String>) -> Self {
        let mut panel = Self {
            forwarder: Arc::new(forwarder), local: None, error: None, opening: Arc::default(),
            listing: Arc::new(AtomicBool::new(false)), listed: Arc::default(),
            auto: Arc::new(Mutex::new(auto)), auto_changed: false, auto_thread: None, quiet: Arc::default(),
            owner: crate::usb_arbitration::Registry::global().owner(), usbipd_left: Arc::default(),
        };
        panel.start_auto();
        panel
    }

    fn opening(&self) -> MutexGuard<'_, Opening> {
        self.opening.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `local` is being opened.
    pub fn is_opening(&self, local: &Local) -> bool {
        self.opening().busids.contains(&local.busid)
    }

    fn apply_listed(&mut self) {
        if let Some(result) = self.listed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            match result {
                Ok(local) => self.local = Some(local),
                Err(e) => {
                    eprintln!("nightjar: USB: {e}");
                    self.local.get_or_insert_with(Vec::new);
                    self.error = Some(e);
                }
            }
        }
    }

    /// The devices here, triggering a background list if they haven't been listed yet.
    pub fn local(&mut self) -> &[Local] {
        self.apply_listed();
        if self.local.is_none() { self.refresh(); }
        self.local.as_deref().unwrap_or_default()
    }

    /// Starts enumerating plugged-in USB devices on a background thread (coalescing concurrent clicks).
    pub fn refresh(&mut self) {
        self.apply_listed();
        if self.listing.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let (listing, listed, usbipd_left) = (self.listing.clone(), self.listed.clone(), self.usbipd_left.clone());
        let spawned = std::thread::Builder::new().name("usb-list".into()).spawn(move || {
            struct Clear<'a>(&'a AtomicBool);
            impl Drop for Clear<'_> {
                fn drop(&mut self) { self.0.store(false, Ordering::Release); }
            }
            let _clear = Clear(&listing);
            let result = list();
            *listed.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            // After the list, which shouldn't wait for usbipd-win's answer.
            #[cfg(windows)]
            usbipd_left.store(crate::usb_windows::leftovers(), Ordering::Relaxed);
            #[cfg(not(windows))]
            let _ = &usbipd_left;
        });
        if let Err(e) = spawned {
            self.listing.store(false, Ordering::Release);
            let message = trf("USB devices could not be listed: {}", &[&e]);
            eprintln!("nightjar: USB: {message}");
            self.local.get_or_insert_with(Vec::new);
            self.error = Some(message);
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

    /// Forwards `local` through usbipd-win, as the user asked (Windows has another driver on it).
    pub fn attach_usbipd_busid(&mut self, busid: &str) {
        if let Some(device) = self.local().iter().find(|d| d.busid == busid).cloned() {
            self.start_attach(&device, Route::Usbipd);
        }
    }

    /// Attaches `local`, as the user asked: its password may be asked for again.
    pub fn attach(&mut self, local: &Local) {
        self.unhush(local);
        self.start_attach(local, Route::Direct);
    }

    /// The shares Nightjar left in usbipd-win when it stopped (as the last list found them), and
    /// whether they are being given back. Picks up how the last giving back went: a failure is
    /// the panel's error, and the list is read again.
    pub fn usbipd_left(&mut self) -> (usize, bool) {
        #[cfg(windows)]
        if let Some(given) = crate::usb_windows::take_given_back() {
            if let Err(e) = given { self.error = Some(e); }
            self.refresh();
        }
        #[cfg(windows)]
        let giving = crate::usb_windows::giving_back();
        #[cfg(not(windows))]
        let giving = false;
        (self.usbipd_left.load(Ordering::Relaxed), giving)
    }

    /// Gives back what Nightjar left shared in usbipd-win, as the user asked (Windows asks for
    /// administrator rights).
    pub fn give_back_usbipd(&mut self) {
        self.error = None;
        #[cfg(windows)]
        crate::usb_windows::give_back_in_background();
    }

    fn start_attach(&mut self, local: &Local, route: Route) {
        self.error = None;
        if !self.opening().try_start(&local.busid) { return; }
        // Opening devices (SetupAPI/libusb string reads, WinUSB handles, usbip-host bind) runs off the drawing thread.
        let busid = local.busid.clone();
        let (forwarder, opening, quiet, local, owner) = (self.forwarder.clone(), self.opening.clone(), self.quiet.clone(), local.clone(), self.owner);
        let spawned = std::thread::Builder::new().name("usb-open".into()).spawn(move || {
            let _ = attach_tracked(&forwarder, owner, (route, true), &opening, &quiet, &local);
        });
        if let Err(e) = spawned {
            self.opening().finish(&busid);
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
    /// The stream is over: a device still being opened finds the forwarder ended, and every
    /// controller forwarded during it is read here again.
    fn drop(&mut self) {
        self.forwarder.end();
        crate::usb_arbitration::Registry::global().release_owner(self.owner);
    }
}

/// How long opening a device waits for the gamepads to let go of it, should it be one.
const ARBITRATION_WAIT: Duration = Duration::from_millis(500);

/// Opens `local` and asks the PC to attach it; what failed, for the user. A controller among
/// them is paused here from now until the attachment ends ([`crate::usb_arbitration`]).
fn attach_now(forwarder: &Forwarder, owner: u64, route: Route, by_hand: bool, local: &Local) -> Result<u32, String> {
    use crate::usb_arbitration::{Arbitrated, Forwarded, Registry};
    let claim = Registry::global().claim(owner, Forwarded::new(local.vid, local.pid, &local.busid));
    if !claim.wait_applied(ARBITRATION_WAIT) {
        eprintln!("nightjar: USB {}: the controllers did not let go in time; opening it anyway", local.busid);
    }
    let opened = match route {
        Route::Direct => open(local, by_hand),
        Route::Usbipd => open_usbipd(local),
    };
    let attached = opened.and_then(|device| forwarder.attach(Arbitrated::wrap(device, claim), &local.name).map_err(|e| attach_error(&e)));
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

#[cfg(test)]
mod opening_tests {
    use super::*;

    #[test]
    fn opening_dedups_concurrent_opens_and_guard_releases_busid() {
        let opening = Mutex::new(Opening::default());
        assert!(opening.lock().unwrap().try_start("1-2"));
        assert!(!opening.lock().unwrap().try_start("1-2"), "concurrent open for same busid rejected");
        assert!(opening.lock().unwrap().try_start("1-3"), "distinct busid allowed");
        {
            let _guard = OpeningGuard { opening: &opening, busid: "1-2".into() };
            assert!(opening.lock().unwrap().busids.contains(&"1-2".to_owned()));
        }
        assert!(!opening.lock().unwrap().busids.contains(&"1-2".to_owned()), "guard releases busid on drop");
        assert!(opening.lock().unwrap().try_start("1-2"), "busid can be opened again after release");
        opening.lock().unwrap().finish("1-2");
        opening.lock().unwrap().finish("1-3");
        assert!(opening.lock().unwrap().busids.is_empty());
    }
}
