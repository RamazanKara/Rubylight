//! The control half: which devices the user attached, what the host made of each, and the
//! tunnel thread that serves an attached one.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rubylight_protocol::usb::{UsbMessage, UsbReason, UsbState, TOKEN_BYTES};

use super::server::{serve, UsbDevice};
use super::Tunnel;
use crate::control::{ClientMessage, SendError};
use crate::http::HostClient;

/// How many ended attachments the list keeps, newest last, so the user sees why one ended.
const ENDED_KEPT: usize = 16;

/// Where one attachment stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Asked the host; waiting for its token.
    Asking,
    /// Opening the tunnel; the host's driver imports the device.
    Opening,
    /// The host's driver has the device.
    Attached,
    /// Over: as the host said, or `Detached` with no reason when the user detached it here.
    /// `error` says what failed on this side.
    Ended {
        state: UsbState,
        reason: UsbReason,
        error: Option<String>,
    },
}

/// One device the user attached in this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub request_id: u32,
    pub busid: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub class: u8,
    pub speed: u8,
    pub phase: Phase,
}

impl Attachment {
    pub fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Ended { .. })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttachError {
    /// The session ended.
    Ended,
    /// The device is attached or being attached already.
    Busy,
    /// Its busid or name breaks the message's rules.
    Invalid,
    /// The control stream refused the request: [`SendError::Unsupported`] when the host does
    /// not forward USB devices.
    Send(SendError),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ended => f.write_str("the stream ended"),
            Self::Busy => f.write_str("the device is attached already"),
            Self::Invalid => f.write_str("the device's busid or name does not fit"),
            Self::Send(SendError::Unsupported) => f.write_str("the host does not forward USB devices"),
            Self::Send(e) => write!(f, "the request did not go out: {e:?}"),
        }
    }
}

impl std::error::Error for AttachError {}

type SendFn = dyn Fn(ClientMessage) -> Result<(), SendError> + Send + Sync;
type OpenFn = dyn Fn(&[u8; TOKEN_BYTES]) -> Result<Arc<dyn Tunnel>, String> + Send + Sync;
type ChangedFn = dyn Fn() + Send + Sync;

struct Entry {
    attachment: Attachment,
    device: Arc<dyn UsbDevice>,
    tunnel: Option<Arc<dyn Tunnel>>,
    /// A tunnel thread serves the device, and closes it when it stops.
    serving: bool,
}

#[derive(Default)]
struct Table {
    last_id: u32,
    entries: Vec<Entry>,
    ended: bool,
}

/// What ending an entry leaves to do outside the lock.
#[derive(Default)]
struct Cleanup {
    tunnel: Option<Arc<dyn Tunnel>>,
    device: Option<Arc<dyn UsbDevice>>,
}

impl Cleanup {
    fn run(self) {
        if let Some(tunnel) = self.tunnel {
            tunnel.close();
        }
        if let Some(device) = self.device {
            device.close();
        }
    }
}

impl Table {
    fn active(&mut self, request_id: u32) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|e| e.attachment.request_id == request_id && e.attachment.is_active())
    }

    fn end(&mut self, request_id: u32, state: UsbState, reason: UsbReason, error: Option<String>) -> Option<Cleanup> {
        let entry = self.active(request_id)?;
        entry.attachment.phase = Phase::Ended { state, reason, error };
        let cleanup = Cleanup {
            tunnel: entry.tunnel.take(),
            // A serving thread closes the device once its tunnel closes.
            device: (!entry.serving).then(|| entry.device.clone()),
        };
        let ended = self.entries.iter().filter(|e| !e.attachment.is_active()).count();
        if ended > ENDED_KEPT {
            if let Some(at) = self.entries.iter().position(|e| !e.attachment.is_active()) {
                self.entries.remove(at);
            }
        }
        Some(cleanup)
    }
}

struct Inner {
    send: Box<SendFn>,
    open: Box<OpenFn>,
    changed: Box<ChangedFn>,
    table: Mutex<Table>,
}

impl Inner {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Tells the host an attachment ended here. The stream may have ended first, when the
    /// host detaches everything itself.
    fn tell_detached(&self, request_id: u32) {
        let _ = (self.send)(ClientMessage::Usb(UsbMessage::Detach { request_id }));
    }

    fn tunnel(self: &Arc<Self>, request_id: u32, token: [u8; TOKEN_BYTES]) {
        let failed = |inner: &Inner, error: String| {
            let cleanup = inner
                .table()
                .end(request_id, UsbState::Failed, UsbReason::None, Some(error));
            if let Some(cleanup) = cleanup {
                cleanup.run();
                inner.tell_detached(request_id);
            }
            (inner.changed)();
        };
        let tunnel = match (self.open)(&token) {
            Ok(tunnel) => tunnel,
            Err(e) => return failed(self, format!("the USB tunnel did not open: {e}")),
        };
        let device = {
            let mut table = self.table();
            match table.active(request_id) {
                Some(entry) => {
                    entry.tunnel = Some(tunnel.clone());
                    entry.serving = true;
                    entry.device.clone()
                }
                // Detached while it opened.
                None => {
                    drop(table);
                    return tunnel.close();
                }
            }
        };
        // Closes the tunnel and the device when it returns.
        let served = serve(tunnel, device);
        let cleanup = {
            let mut table = self.table();
            match served {
                // The host closed the tunnel; its status says why.
                Ok(()) => table.end(request_id, UsbState::Detached, UsbReason::None, None),
                Err(e) => table.end(request_id, UsbState::Failed, UsbReason::DeviceError, Some(e.to_string())),
            }
        };
        if let Some(cleanup) = cleanup {
            cleanup.run();
            self.tell_detached(request_id);
        }
        (self.changed)();
    }
}

/// One session's USB forwarding. Send what it makes on the control stream, hand it every
/// [`ControlEvent::Usb`](crate::control::ControlEvent::Usb), and call [`Forwarder::end`] when
/// the stream ends or drops (dropping it does too).
pub struct Forwarder {
    inner: Arc<Inner>,
}

impl Forwarder {
    /// `send` puts a message on the control stream, `open` opens the USB tunnel for a token,
    /// and `changed` runs after every change to [`Forwarder::list`], from any thread.
    pub fn new(
        send: impl Fn(ClientMessage) -> Result<(), SendError> + Send + Sync + 'static,
        open: impl Fn(&[u8; TOKEN_BYTES]) -> Result<Arc<dyn Tunnel>, String> + Send + Sync + 'static,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                send: Box::new(send),
                open: Box::new(open),
                changed: Box::new(changed),
                table: Mutex::new(Table::default()),
            }),
        }
    }

    /// Opens tunnels to `host`, the host the session streams from.
    pub fn for_host(
        host: Arc<HostClient>,
        send: impl Fn(ClientMessage) -> Result<(), SendError> + Send + Sync + 'static,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let open = move |token: &[u8; TOKEN_BYTES]| {
            host.open_usb_tunnel(token)
                .map(|tunnel| Arc::new(tunnel) as Arc<dyn Tunnel>)
                .map_err(|e| e.to_string())
        };
        Self::new(send, open, changed)
    }

    /// Asks the host to attach `device`, which the user picked, and returns its request id.
    /// The list follows what becomes of it.
    pub fn attach(&self, device: Arc<dyn UsbDevice>, name: &str) -> Result<u32, AttachError> {
        let described = device.describe();
        let (request_id, message) = {
            let mut table = self.inner.table();
            if table.ended {
                return Err(AttachError::Ended);
            }
            if table
                .entries
                .iter()
                .any(|e| e.attachment.is_active() && e.attachment.busid == described.busid)
            {
                return Err(AttachError::Busy);
            }
            let mut request_id = table.last_id.wrapping_add(1);
            if request_id == 0 {
                request_id = 1;
            }
            let attachment = Attachment {
                request_id,
                busid: described.busid.clone(),
                name: name.to_owned(),
                vid: described.vendor,
                pid: described.product,
                class: device.class(),
                speed: u8::try_from(described.speed).unwrap_or(0),
                phase: Phase::Asking,
            };
            let message = ClientMessage::Usb(UsbMessage::AttachRequest {
                request_id,
                vid: attachment.vid,
                pid: attachment.pid,
                class: attachment.class,
                speed: attachment.speed,
                busid: attachment.busid.clone(),
                name: attachment.name.clone(),
            });
            if !message.is_valid() || !rubylight_protocol::usbip::valid_busid(&attachment.busid) {
                return Err(AttachError::Invalid);
            }
            table.last_id = request_id;
            // In the table before it goes out, so the host's answer finds it.
            table.entries.push(Entry {
                attachment,
                device,
                tunnel: None,
                serving: false,
            });
            (request_id, message)
        };
        if let Err(e) = (self.inner.send)(message) {
            self.inner
                .table()
                .entries
                .retain(|entry| entry.attachment.request_id != request_id);
            return Err(AttachError::Send(e));
        }
        (self.inner.changed)();
        Ok(request_id)
    }

    /// Detaches `request_id` because the user asked or unplugged it; false when it is not
    /// attached. The host hears so.
    pub fn detach(&self, request_id: u32) -> bool {
        let cleanup = self
            .inner
            .table()
            .end(request_id, UsbState::Detached, UsbReason::None, None);
        let Some(cleanup) = cleanup else {
            return false;
        };
        self.inner.tell_detached(request_id);
        cleanup.run();
        (self.inner.changed)();
        true
    }

    /// Handles a USB message from the host.
    pub fn on_message(&self, message: UsbMessage) {
        let request_id = message.request_id();
        let mut table = self.inner.table();
        let cleanup = match message {
            UsbMessage::TunnelReady { token, .. } => {
                match table.active(request_id) {
                    Some(entry) if entry.attachment.phase == Phase::Asking => {
                        entry.attachment.phase = Phase::Opening;
                    }
                    _ => return,
                }
                drop(table);
                let inner = self.inner.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("usb-tunnel-{request_id}"))
                    .spawn(move || inner.tunnel(request_id, token));
                if let Err(e) = spawned {
                    let cleanup = self.inner.table().end(
                        request_id,
                        UsbState::Failed,
                        UsbReason::None,
                        Some(format!("no tunnel thread: {e}")),
                    );
                    if let Some(cleanup) = cleanup {
                        cleanup.run();
                        self.inner.tell_detached(request_id);
                    }
                }
                (self.inner.changed)();
                return;
            }
            UsbMessage::Status {
                state: UsbState::Attached,
                ..
            } => {
                match table.active(request_id) {
                    Some(entry) if entry.attachment.phase == Phase::Opening => {
                        entry.attachment.phase = Phase::Attached;
                    }
                    _ => return,
                }
                None
            }
            UsbMessage::Status { state, reason, .. } => {
                match table.end(request_id, state, reason, None) {
                    Some(cleanup) => Some(cleanup),
                    None => {
                        // A status after the host's own detach says why.
                        let ended = table.entries.iter_mut().rev().find(|e| {
                            e.attachment.request_id == request_id
                                && matches!(e.attachment.phase, Phase::Ended { error: None, .. })
                        });
                        match ended {
                            Some(entry) => {
                                entry.attachment.phase = Phase::Ended {
                                    state,
                                    reason,
                                    error: None,
                                };
                                None
                            }
                            None => return,
                        }
                    }
                }
            }
            UsbMessage::Detach { .. } => {
                match table.end(request_id, UsbState::Detached, UsbReason::None, None) {
                    Some(cleanup) => Some(cleanup),
                    None => return,
                }
            }
            UsbMessage::AttachRequest { .. } => return,
        };
        drop(table);
        if let Some(cleanup) = cleanup {
            cleanup.run();
        }
        (self.inner.changed)();
    }

    /// Every attachment of this session, the ended ones last kept included, oldest first.
    pub fn list(&self) -> Vec<Attachment> {
        self.inner
            .table()
            .entries
            .iter()
            .map(|e| e.attachment.clone())
            .collect()
    }

    /// The stream ended or dropped: everything detaches here (the host detaches its side
    /// itself) and nothing attaches any more.
    pub fn end(&self) {
        let cleanups: Vec<Cleanup> = {
            let mut table = self.inner.table();
            if table.ended {
                return;
            }
            table.ended = true;
            let active: Vec<u32> = table
                .entries
                .iter()
                .filter(|e| e.attachment.is_active())
                .map(|e| e.attachment.request_id)
                .collect();
            active
                .into_iter()
                .filter_map(|id| table.end(id, UsbState::Detached, UsbReason::SessionEnd, None))
                .collect()
        };
        for cleanup in cleanups {
            cleanup.run();
        }
        (self.inner.changed)();
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
#[path = "forwarder_tests.rs"]
mod tests;
