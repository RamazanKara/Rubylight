//! USB forwarding, the client's half: a device plugged into this client, attached to the host.
//!
//! The user picks each device; nothing attaches by itself. [`Forwarder`] asks the host on the
//! control stream (0x553A, [`rubylight_protocol::usb`]), opens the native API's USB tunnel
//! with the token the host answers, and [`serve`]s the device over it as a USB/IP server: the
//! host relays the tunnel to its USB/IP driver. Platforms implement [`UsbDevice`], or the
//! simpler [`BlockingDevice`] behind [`Blocking`], for their USB stack (libusb on the desktop);
//! [`usbfs::UsbfsDevice`] runs one on Linux's usbfs, the descriptor Android's `UsbManager`
//! opens; [`fake::FakeDevice`] stands in for one in tests.

mod blocking;
pub mod fake;
mod forwarder;
mod server;
pub mod usbfs;

pub use blocking::{Blocking, BlockingDevice};
pub use forwarder::{AttachError, Attachment, Forwarder, Phase};
pub use rubylight_protocol::usb::{UsbMessage, UsbReason, UsbState};
pub use rubylight_protocol::usbip::{self as wire, IsoPacket};
pub use server::{serve, Completion, Direction, Reply, ServeError, Transfer, UsbDevice};

/// The connection a device's USB/IP runs over: [`crate::http::UsbTunnel`], or a socket in
/// tests. One thread reads while others write.
pub trait Tunnel: Send + Sync {
    /// Reads what arrived, waiting for it; 0 once the host closed.
    fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize>;
    /// Sends `bytes` whole.
    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()>;
    /// Closes the connection, waking a blocked read.
    fn close(&self);
}

impl Tunnel for crate::http::UsbTunnel {
    fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        crate::http::UsbTunnel::read(self, bytes)
    }
    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        crate::http::UsbTunnel::write_all(self, bytes)
    }
    fn close(&self) {
        crate::http::UsbTunnel::close(self)
    }
}

/// A tunnel over a plain TCP connection, for tests and loopback tools.
pub struct TcpTunnel {
    reader: std::net::TcpStream,
    writer: std::sync::Mutex<std::net::TcpStream>,
    closed: std::sync::atomic::AtomicBool,
}

/// How often a read looks for [`Tunnel::close`]: Windows doesn't wake a blocked receive on
/// a shutdown. Data still arrives as soon as it comes.
const CLOSE_POLL: std::time::Duration = std::time::Duration::from_millis(50);

impl TcpTunnel {
    pub fn new(stream: std::net::TcpStream) -> std::io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(CLOSE_POLL))?;
        Ok(Self {
            writer: std::sync::Mutex::new(stream.try_clone()?),
            reader: stream,
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

impl Tunnel for TcpTunnel {
    fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        use std::sync::atomic::Ordering;
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Ok(0);
            }
            match std::io::Read::read(&mut &self.reader, bytes) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                other => return other,
            }
        }
    }
    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::io::Write::write_all(&mut *writer, bytes)
    }
    fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self.reader.shutdown(std::net::Shutdown::Both);
    }
}

static WARN: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

/// Where forwarding's warnings go, things that don't end an attachment but should be seen:
/// stderr until the platform names its log (Android's has no stderr). The first call wins.
pub fn warn_with(warn: fn(&str)) {
    let _ = WARN.set(warn);
}

pub(crate) fn warn(message: &str) {
    match WARN.get() {
        Some(warn) => warn(message),
        None => eprintln!("nightjar: {message}"),
    }
}

/// Reads exactly `bytes`; `Ok(false)` when the peer closed before the first byte.
pub(crate) fn read_whole(tunnel: &dyn Tunnel, bytes: &mut [u8]) -> std::io::Result<bool> {
    let mut at = 0;
    while at < bytes.len() {
        match tunnel.read(&mut bytes[at..])? {
            0 if at == 0 => return Ok(false),
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => at += n,
        }
    }
    Ok(true)
}
