//! USB forwarding through Linux's own USB/IP server, for the devices libusb can't serve
//! (webcams and audio devices, whose isochronous endpoints it doesn't forward): the kernel's
//! usbip-host takes the device while it is attached and serves it on a TCP connection Nightjar
//! makes on loopback and hands it, as usbipd would after an import; the tunnel is relayed to
//! that connection ([`nightjar_core::usb::UsbDevice::relay`]). No usbipd runs: nothing listens
//! for USB/IP, and the loopback listener that makes the connection takes exactly one and closes.
//!
//! Binding needs root: Nightjar's helper script (packaging/linux/usb) runs through pkexec under
//! its own polkit action. The .deb and .rpm install it with its policy and udev rule; anywhere
//! else (AppImage, archive) the first attach installs them, as one more password prompt. The
//! helper gets the connection's other end as its stdin and holds the device until the export
//! on it ends, so the device goes back to its driver when the attachment ends or Nightjar quits
//! or crashes (which closes Nightjar's end), and the udev rule clears the port when a forwarded
//! device is unplugged.
//!
//! Ported from Moonlight V+ (qiin2333/moonlight-qt): its Linux usbip-host backend, helper,
//! polkit policy and udev rule.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use nightjar_core::usb::{wire, Completion, TcpTunnel, Transfer, Tunnel, UsbDevice};

use crate::strings::{tr, trf};

/// The helper script, as the packages install it and as the first attach installs it elsewhere.
const SCRIPT: &str = include_str!("../packaging/linux/usb/nightjar-usb-helper");
/// The line an installed helper of this version carries.
const VERSION_MARKER: &str = "# nightjar-usb-helper-version: 2";
const HELPER: &str = "/usr/lib/nightjar/nightjar-usb-helper";
const POLICY: &str = "/usr/share/polkit-1/actions/io.github.ramazankara.nightjar.usb.policy";
const RULE: &str = "/usr/lib/udev/rules.d/90-nightjar-usb.rules";
/// Where the helper looks for usbip (its own PATH).
const SYSTEM_PATH: [&str; 4] = ["/usr/sbin", "/usr/bin", "/sbin", "/bin"];

/// What this computer lacks to forward through usbip-host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The usbip program.
    Usbip,
    /// pkexec, which asks for the password.
    Pkexec,
    /// Nightjar runs as a Flatpak, which can't reach either.
    Flatpak,
}

impl Missing {
    pub fn message(&self) -> String {
        match self {
            Self::Usbip => tr("Webcams and audio devices are forwarded by Linux’s USB/IP tools, which aren’t installed. Install your distribution’s usbip package (on Ubuntu: linux-tools-generic), then press Refresh.").to_owned(),
            Self::Pkexec => tr("Webcams and audio devices need pkexec to ask for your password, which isn’t installed. Install your distribution’s pkexec or polkit package, then press Refresh.").to_owned(),
            Self::Flatpak => tr("Webcams and audio devices can’t be forwarded from the Flatpak. Use the .deb, .rpm or AppImage of Nightjar for them.").to_owned(),
        }
    }
}

/// The machine usbip-host forwarding runs on: real paths, or a fake sysfs and helper in tests.
pub struct System {
    /// /sys/bus/usb/devices.
    sysfs: PathBuf,
    /// What runs the installed helper: pkexec and its path.
    helper: Vec<OsString>,
    /// The installed helper, policy and rule, which say whether it needs installing.
    installed: [PathBuf; 3],
}

impl System {
    pub fn real() -> Self {
        Self {
            sysfs: PathBuf::from("/sys/bus/usb/devices"),
            helper: vec!["pkexec".into(), HELPER.into()],
            installed: [HELPER.into(), POLICY.into(), RULE.into()],
        }
    }

    /// What keeps forwarding through usbip-host from working here, if anything.
    pub fn missing(&self) -> Option<Missing> {
        if Path::new("/.flatpak-info").exists() { return Some(Missing::Flatpak); }
        let in_system = |program: &str| SYSTEM_PATH.iter().any(|dir| Path::new(dir).join(program).is_file());
        if !in_system("usbip") { return Some(Missing::Usbip); }
        let in_path = std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("pkexec").is_file()));
        if !in_path && !in_system("pkexec") { return Some(Missing::Pkexec); }
        None
    }

    fn read(&self, busid: &str, field: &str) -> Option<String> {
        std::fs::read_to_string(self.sysfs.join(busid).join(field)).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
    }

    /// vendor:product:serial as the helper reads it: lowercase hex, whitespace left out.
    pub fn identity(&self, busid: &str) -> Option<String> {
        let strip = |s: String| s.chars().filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r')).collect::<String>();
        let vid = strip(self.read(busid, "idVendor")?).to_ascii_lowercase();
        let pid = strip(self.read(busid, "idProduct")?).to_ascii_lowercase();
        let serial = self.read(busid, "serial").map(strip).unwrap_or_default();
        Some(format!("{vid}:{pid}:{serial}"))
    }

    /// Whether usbip-host has the device.
    pub fn bound(&self, busid: &str) -> bool {
        std::fs::read_link(self.sysfs.join(busid).join("driver")).is_ok_and(|target| target.file_name().is_some_and(|name| name == "usbip-host"))
    }

    /// Whether usbip-host serves the device on a connection now (usbip_status 2, in use).
    fn exported(&self, busid: &str) -> bool {
        self.read(busid, "usbip_status").as_deref() == Some("2")
    }

    /// The device as the attach request and the picker describe it.
    fn describe(&self, busid: &str) -> Option<wire::Device> {
        let number = |field: &str| self.read(busid, field).and_then(|s| s.parse::<u32>().ok());
        let hex = |field: &str| self.read(busid, field).and_then(|s| u16::from_str_radix(&s, 16).ok());
        let byte = |field: &str| hex(field).and_then(|v| u8::try_from(v).ok());
        let speed = match self.read(busid, "speed").as_deref() {
            Some("1.5") => 1,
            Some("12") => 2,
            Some("480") => 3,
            Some("53.3") => 4,
            Some("5000") => 5,
            Some("10000" | "20000") => 6,
            _ => 0,
        };
        Some(wire::Device {
            path: format!("/sys/bus/usb/devices/{busid}"),
            busid: busid.to_owned(),
            busnum: number("busnum")?,
            devnum: number("devnum")?,
            speed,
            vendor: hex("idVendor")?,
            product: hex("idProduct")?,
            bcd_device: hex("bcdDevice").unwrap_or(0),
            class: byte("bDeviceClass").unwrap_or(0),
            subclass: byte("bDeviceSubClass").unwrap_or(0),
            protocol: byte("bDeviceProtocol").unwrap_or(0),
            configuration: self.read(busid, "bConfigurationValue").and_then(|s| s.parse().ok()).unwrap_or(0),
            configurations: self.read(busid, "bNumConfigurations").and_then(|s| s.parse().ok()).unwrap_or(1),
            interfaces: self.read(busid, "bNumInterfaces").and_then(|s| s.parse().ok()).unwrap_or(0),
        })
    }

    /// bDeviceClass, or the first interface's class when that is 0.
    fn class(&self, described: &wire::Device) -> u8 {
        if described.class != 0 { return described.class; }
        let first = format!("{}:{}.0", described.busid, described.configuration.max(1));
        self.read(&first, "bInterfaceClass").and_then(|s| u8::from_str_radix(&s, 16).ok()).unwrap_or(0)
    }

    fn is_installed(&self) -> bool {
        let [helper, policy, rule] = &self.installed;
        std::fs::read_to_string(helper).is_ok_and(|s| s.contains(VERSION_MARKER)) && policy.is_file() && rule.is_file()
    }

    /// Installs the helper, its policy and its udev rule: one password prompt.
    fn install(&self) -> Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("nightjar-usb-{}-{nanos}", std::process::id()));
        let failed = |e: std::io::Error| trf("The USB helper could not be written to a temporary file: {}", &[&e]);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(failed)?;
        let script = dir.join("nightjar-usb-helper");
        let written = std::fs::write(&script, SCRIPT).and_then(|()| std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)));
        let result = written.map_err(failed).and_then(|()| {
            let mut command = Command::new("pkexec");
            crate::amd_decode::for_child(&mut command).arg("/bin/sh").arg(&script).arg("install");
            let output = run(command)?;
            if output.code == Some(0) && self.is_installed() { return Ok(()); }
            Err(match output.code {
                Some(7) => tr("Setting up USB forwarding failed: the rule that gives a device back after it’s unplugged could not be loaded. Try again.").to_owned(),
                code => failure(code, &output.stderr),
            })
        });
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// Binds `busid`, which must still be the device the user picked, to usbip-host and holds
    /// it until the returned device closes or drops. Blocks while the password is asked for.
    pub fn open(&self, busid: &str, vid: u16, pid: u16) -> Result<Arc<Kernel>, String> {
        let unplugged = || tr("The device was unplugged.").to_owned();
        let described = self.describe(busid).ok_or_else(unplugged)?;
        if (described.vendor, described.product) != (vid, pid) { return Err(unplugged()); }
        let identity = self.identity(busid).ok_or_else(unplugged)?;
        // Exported already: binding it again would end that export. The helper checks this as
        // well; checking here spares the password prompt.
        if self.exported(busid) { return Err(busy()); }
        if self.bound(busid) { eprintln!("nightjar: USB {busid}: usbip-host has it already but doesn't export it (left bound by another program or a helper that was killed); exporting it and giving it back to its driver afterwards"); }
        if !self.is_installed() { self.install()?; }
        let failed = |e: std::io::Error| trf("The device could not be opened: {}", &[&e]);
        let (theirs, ours) = loopback_pair().map_err(failed)?;
        let connection = Arc::new(TcpTunnel::new(ours).map_err(failed)?);
        let (program, args) = self.helper.split_first().expect("the helper command names a program");
        let mut command = Command::new(program);
        crate::amd_decode::for_child(&mut command).args(args).arg("hold").arg(busid).arg(&identity);
        // The helper hands its end to usbip-host; Command closes this process's copy.
        command.stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs))).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Its own process group: a Ctrl-C in Nightjar's terminal doesn't reach it, and its
        // watchdog kills only it.
        command.process_group(0);
        let mut child = command.spawn().map_err(spawn_error)?;
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let mut line = String::new();
        let bound = BufReader::new(stdout).read_line(&mut line).is_ok() && line.trim() == "bound";
        if !bound {
            connection.close();
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            let code = child.wait().ok().and_then(|status| status.code());
            eprintln!("nightjar: USB {busid}: the helper did not bind it (exit {code:?}): {}", text.trim());
            return Err(failure(code, &text));
        }
        // What it says while it holds the device and when it gives it back goes to the log.
        let name = busid.to_owned();
        let _ = std::thread::Builder::new().name("usb-helper-log".into()).spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() { eprintln!("nightjar: USB {name}: helper: {line}"); }
            }
        });
        eprintln!("nightjar: USB {busid}: usbip-host has it");
        let class = self.class(&described);
        Ok(Arc::new(Kernel { described, class, connection, relayed: AtomicBool::new(false), held: Mutex::new(Some(child)) }))
    }
}

/// What the helper made of a run, as the user reads it.
fn failure(code: Option<i32>, stderr: &str) -> String {
    match code {
        _ if stderr.contains("operation_timeout") => tr("Linux’s USB/IP tools didn’t answer in time. Unplug the device, plug it in again and retry.").to_owned(),
        _ if stderr.contains("authentication agent") => tr("No password prompt could be shown: no polkit authentication agent runs in this desktop session.").to_owned(),
        Some(126 | 127) => tr("Forwarding this device needs your password, and the prompt was cancelled or failed.").to_owned(),
        Some(3) => Missing::Usbip.message(),
        Some(4) => tr("Linux’s USB/IP module (usbip-host) could not be loaded. Install your distribution’s extra kernel modules (on Ubuntu: linux-modules-extra), then retry.").to_owned(),
        Some(5) => tr("Another device was plugged in there meanwhile. Press Refresh and attach again.").to_owned(),
        Some(8) => tr("The device’s last connection didn’t let go. Unplug it, plug it in again and retry.").to_owned(),
        Some(10) => busy(),
        code => {
            let detail = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).map_or_else(|| format!("exit {code:?}"), str::to_owned);
            trf("Linux’s USB/IP tools failed: {}", &[&detail])
        }
    }
}

fn busy() -> String {
    tr("Another program forwards this device already. Stop it there, then retry.").to_owned()
}

fn spawn_error(e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => Missing::Pkexec.message(),
        _ => trf("The USB helper could not be started: {}", &[&e]),
    }
}

struct Output {
    code: Option<i32>,
    stderr: String,
}

fn run(mut command: Command) -> Result<Output, String> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).process_group(0);
    let output = command.output().map_err(spawn_error)?;
    Ok(Output { code: output.status.code(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() })
}

/// A connected TCP pair on loopback: the end usbip-host gets, and Nightjar's. The listener
/// takes exactly one connection, the one made here (checked by address, so another local
/// program racing to it is refused), and closes before this returns.
fn loopback_pair() -> std::io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let ours = TcpStream::connect(listener.local_addr()?)?;
    let (theirs, from) = listener.accept()?;
    drop(listener);
    if from != ours.local_addr()? {
        return Err(std::io::Error::other(format!("another program ({from}) connected to the USB connection's loopback port")));
    }
    Ok((theirs, ours))
}

/// A device usbip-host has, which it serves to the host on the connection relayed to the tunnel.
pub struct Kernel {
    described: wire::Device,
    class: u8,
    /// Nightjar's end of the connection usbip-host serves the device on.
    connection: Arc<TcpTunnel>,
    relayed: AtomicBool,
    /// The helper, until it gave the device back.
    held: Mutex<Option<Child>>,
}

impl Kernel {
    /// Gives the device back: closing Nightjar's end ends usbip-host's export, and the helper
    /// unbinds. `wait` waits for it (tests); else a thread does and logs how it went.
    fn release(&self, wait: bool) -> Option<std::process::ExitStatus> {
        self.connection.close();
        let mut child = self.held.lock().unwrap_or_else(PoisonError::into_inner).take()?;
        let busid = self.described.busid.clone();
        let mut reap = move || match child.wait() {
            Ok(status) if status.success() => {
                eprintln!("nightjar: USB {busid}: back with its own driver");
                Some(status)
            }
            Ok(status) => {
                eprintln!("nightjar: USB {busid}: the helper could not give it back to its driver ({status}); unplug it and plug it in again");
                Some(status)
            }
            Err(e) => {
                eprintln!("nightjar: USB {busid}: waiting for the helper failed: {e}");
                None
            }
        };
        if wait { return reap(); }
        if let Err(e) = std::thread::Builder::new().name("usb-helper-wait".into()).spawn(move || { reap(); }) {
            eprintln!("nightjar: USB {}: no thread to wait for the helper: {e}", self.described.busid);
        }
        None
    }
}

impl UsbDevice for Kernel {
    fn describe(&self) -> wire::Device {
        self.described.clone()
    }

    fn class(&self) -> u8 {
        self.class
    }

    fn submit(&self, _: Transfer, done: Completion) {
        // Never called: the server relays the tunnel to usbip-host instead.
        done.complete(nightjar_core::usb::Reply::failed(wire::EPROTO));
    }

    fn unlink(&self, _: u32) {}

    fn close(&self) {
        self.release(false);
    }

    fn relay(&self) -> Option<std::io::Result<Arc<dyn Tunnel>>> {
        if self.relayed.swap(true, Ordering::SeqCst) {
            return Some(Err(std::io::Error::other("the device was served once already")));
        }
        Some(Ok(self.connection.clone()))
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        self.release(false);
    }
}

#[cfg(test)]
mod tests;
