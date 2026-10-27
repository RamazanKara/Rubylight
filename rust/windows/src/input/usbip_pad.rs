//! Loopback USB/IP attachment shared by Steam Deck and DualSense.
use anyhow::{Context, Result, bail};
use butterpollo_core::usbip::{BUS_ID, Device, Export};
use std::{
    ffi::OsStr,
    io::Read,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// usbip.exe returns once the device is plugged in or the attach failed.
const COMMAND_WAIT: Duration = Duration::from_secs(15);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// usbip-win2's client: registered install folder, default folder, then PATH.
pub(super) fn usbip_exe() -> Option<PathBuf> {
    let mut folders: Vec<PathBuf> = [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1",
    ]
    .into_iter()
    .filter_map(|key| {
        crate::steam::registry_string(
            windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
            key,
            "InstallLocation",
        )
    })
    .filter(|folder| !folder.is_empty())
    .map(PathBuf::from)
    .collect();
    folders.extend(
        ["ProgramW6432", "ProgramFiles"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|folder| PathBuf::from(folder).join("USBip")),
    );
    if let Some(path) = std::env::var_os("PATH") {
        folders.extend(std::env::split_paths(&path));
    }
    folders
        .into_iter()
        .map(|folder| folder.join("usbip.exe"))
        .find(|exe| exe.is_file())
}

/// Runs usbip.exe and returns what it printed.
fn usbip(exe: &Path, args: &[&str]) -> Result<String> {
    let mut child = Command::new(exe)
        .args(args.iter().map(OsStr::new))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let deadline = Instant::now() + COMMAND_WAIT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("usbip.exe {} did not finish", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    if !status.success() {
        bail!(
            "usbip.exe {} failed ({status}): {}",
            args.join(" "),
            err.trim()
        );
    }
    Ok(out)
}

/// An attached device; dropping it stops and detaches its own port.
pub(super) struct UsbPad<D: Device> {
    pub(super) export: Option<Export<D>>,
    exe: PathBuf,
    /// The USB/IP server's TCP port.
    tcp: String,
    /// usbip-win2's hub port the controller is plugged into.
    port: u32,
}
impl<D: Device> UsbPad<D> {
    /// Serve a device on loopback and attach it with usbip-win2.
    pub(super) fn plug(exe: &Path, device: D, period: Duration) -> Result<Self> {
        let export = Export::listen(device, period).context("starting the USB/IP server")?;
        let tcp = export.port().to_string();
        let port = attach(exe, &tcp, BUS_ID)?;
        Ok(Self {
            export: Some(export),
            exe: exe.to_owned(),
            tcp,
            port,
        })
    }
    pub(super) fn port(&self) -> u32 {
        self.port
    }
}
impl<D: Device> Drop for UsbPad<D> {
    fn drop(&mut self) {
        let port = self.port.to_string();
        if let Err(error) = usbip(&self.exe, &["detach", "--port", &port]) {
            tracing::warn!(error = format!("{error:#}"), "USB controller detach failed");
        }
        // Closing the server would otherwise leave the driver trying to
        // attach the controller again.
        drop(self.export.take());
        let _ = stop(&self.exe, &self.tcp, BUS_ID);
    }
}

/// Attaches `busid` from the USB/IP server on loopback port `tcp`; returns the hub port.
fn attach(exe: &Path, tcp: &str, busid: &str) -> Result<u32> {
    let out = usbip(
        exe,
        &[
            "--tcp-port",
            tcp,
            "attach",
            "--remote",
            "127.0.0.1",
            "--bus-id",
            busid,
            "--terse",
            "--once",
        ],
    )?;
    out.lines()
        .rev()
        .find_map(|line| line.trim().parse().ok())
        .with_context(|| format!("usbip.exe attach printed no port: {}", out.trim()))
}

/// Stops the driver attaching `busid` from loopback port `tcp` again.
fn stop(exe: &Path, tcp: &str, busid: &str) -> Result<String> {
    usbip(
        exe,
        &[
            "--tcp-port",
            tcp,
            "attach",
            "--remote",
            "127.0.0.1",
            "--bus-id",
            busid,
            "--stop",
        ],
    )
}

/// usbip-win2 for USB forwarding: attaches a client's device that the host relays on a
/// loopback port.
pub struct UsbipDriver {
    exe: PathBuf,
}
impl UsbipDriver {
    /// None when usbip-win2 is not installed.
    pub fn find() -> Option<Self> {
        usbip_exe().map(|exe| Self { exe })
    }
    /// Attaches `busid`, served on 127.0.0.1:`tcp_port`; returns the hub port. Blocks until the
    /// driver imported the device or failed, at most 15 s.
    pub fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32> {
        attach(&self.exe, &tcp_port.to_string(), busid)
    }
    /// Unplugs hub `port` and stops the driver attaching the device again.
    pub fn detach(&self, port: u32, tcp_port: u16, busid: &str) -> Result<()> {
        let detached = usbip(&self.exe, &["detach", "--port", &port.to_string()]);
        stop(&self.exe, &tcp_port.to_string(), busid)?;
        detached.map(drop)
    }
    /// Stops the driver attaching `busid` from 127.0.0.1:`tcp_port` again, for an attach whose
    /// hub port is unknown. Succeeds when there was nothing to stop.
    pub fn stop(&self, tcp_port: u16, busid: &str) -> Result<()> {
        stop(&self.exe, &tcp_port.to_string(), busid).map(drop)
    }
    /// Whether the connection from `peer` to the USB/IP server at `server` is the driver's.
    /// usbip-win2's vhci connects from the kernel (Winsock Kernel), so its connections belong
    /// to System; any process on the PC could reach a loopback port first.
    pub fn connected(peer: std::net::SocketAddr, server: std::net::SocketAddr) -> Result<bool> {
        const SYSTEM: u32 = 4;
        let (std::net::SocketAddr::V4(peer), std::net::SocketAddr::V4(server)) = (peer, server)
        else {
            bail!("the USB/IP server listens on IPv4 only, not {server} from {peer}");
        };
        let owner = crate::net::tcp_owner(peer, server)?;
        if owner.is_none() {
            tracing::warn!(%peer, "no TCP connection from this peer to the USB/IP server");
        }
        Ok(owner == Some(SYSTEM))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_reaching_the_usbip_port_is_not_the_driver() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let stranger = std::net::TcpStream::connect(address).unwrap();
        let (_accepted, peer) = server.accept().unwrap();
        assert_eq!(peer, stranger.local_addr().unwrap());
        assert!(!UsbipDriver::connected(peer, address).unwrap());
        let v6 = "[::1]:1".parse().unwrap();
        assert!(UsbipDriver::connected(v6, address).is_err());
    }
}
