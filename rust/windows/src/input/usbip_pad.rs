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

/// usbip-win2's command-line client: its installer's folder, then PATH.
pub(super) fn usbip_exe() -> Option<PathBuf> {
    let mut folders: Vec<PathBuf> = ["ProgramW6432", "ProgramFiles"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|folder| PathBuf::from(folder).join("USBip"))
        .collect();
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
        let out = usbip(
            exe,
            &[
                "--tcp-port",
                &tcp,
                "attach",
                "--remote",
                "127.0.0.1",
                "--bus-id",
                BUS_ID,
                "--terse",
                "--once",
            ],
        )?;
        let port = out
            .lines()
            .rev()
            .find_map(|line| line.trim().parse().ok())
            .with_context(|| format!("usbip.exe attach printed no port: {}", out.trim()))?;
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
        let _ = usbip(
            &self.exe,
            &[
                "--tcp-port",
                &self.tcp,
                "attach",
                "--remote",
                "127.0.0.1",
                "--bus-id",
                BUS_ID,
                "--stop",
            ],
        );
    }
}
