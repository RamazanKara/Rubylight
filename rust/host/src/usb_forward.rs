//! USB forwarding: a device plugged into the client, attached to this host through
//! usbip-win2's virtual host controller.
//!
//! The client asks on the control stream (0x553A). The host checks the setting, the device's
//! `allow_usb`, the driver and the device limit, and answers with a token that opens the native
//! API's USB tunnel once, within 30 s, from the certificate that asked. Over the tunnel the
//! client serves the device as a USB/IP server. The host relays it to the driver on a loopback
//! port, checking the framing on the way, so a client can't hand the kernel driver malformed
//! USB/IP. An attachment ends when the client or the console detaches it, when either end of
//! the relay closes, and with its stream.

use anyhow::{Context, Result, anyhow, bail};
use butterpollo_core::{
    native_api::usb::TOKEN_TTL_SECS,
    state::Client,
    usb_forward::{TOKEN_BYTES, UsbMessage, UsbReason, UsbState},
    usbip_wire::{self as wire, Header},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

/// How long the driver may take to import the device, the client's answer included.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
/// Transfers in flight on one device; a driver keeps a few per endpoint.
const MAX_IN_FLIGHT: usize = 512;

/// The USB/IP driver; tests swap in a fake.
pub trait Vhci: Send + Sync {
    /// Whether the driver is installed.
    fn ready(&self) -> bool;
    /// Attaches `busid` from the USB/IP server on 127.0.0.1:`tcp_port` and returns the hub
    /// port. Blocks until the driver imported the device or failed.
    fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32>;
    /// Unplugs hub `port` and stops the driver attaching the device again.
    fn detach(&self, port: u32, tcp_port: u16, busid: &str) -> Result<()>;
}

/// usbip-win2, looked up on every use: installing it needs no host restart.
struct Usbip;
impl Vhci for Usbip {
    fn ready(&self) -> bool {
        butterpollo_windows::input::UsbipDriver::find().is_some()
    }
    fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32> {
        usbip()?.attach(tcp_port, busid)
    }
    fn detach(&self, port: u32, tcp_port: u16, busid: &str) -> Result<()> {
        usbip()?.detach(port, tcp_port, busid)
    }
}
fn usbip() -> Result<butterpollo_windows::input::UsbipDriver> {
    butterpollo_windows::input::UsbipDriver::find().context("usbip-win2 is not installed")
}

/// The devices clients forward to this host.
pub struct UsbForwarding {
    vhci: Mutex<Arc<dyn Vhci>>,
    registry: Mutex<Registry>,
}
impl Default for UsbForwarding {
    fn default() -> Self {
        Self {
            vhci: Mutex::new(Arc::new(Usbip)),
            registry: Mutex::default(),
        }
    }
}

#[derive(Default)]
struct Registry {
    next: u64,
    attachments: Vec<Attachment>,
    /// Messages for each session's client, which its control loop sends.
    outbox: HashMap<String, Vec<UsbMessage>>,
}
impl Registry {
    fn queue(&mut self, session: &str, messages: impl IntoIterator<Item = UsbMessage>) {
        let mut messages = messages.into_iter().peekable();
        if messages.peek().is_some() {
            self.outbox
                .entry(session.to_owned())
                .or_default()
                .extend(messages);
        }
    }
    fn index(&self, id: u64) -> Option<usize> {
        self.attachments.iter().position(|a| a.id == id)
    }
    /// Ends attachment `id`: at once while it waits for its tunnel, else through its relay.
    /// `notice` goes to the client once it is detached.
    fn end(&mut self, id: u64, notice: Vec<UsbMessage>) -> bool {
        let Some(index) = self.index(id) else {
            return false;
        };
        if matches!(self.attachments[index].stage, Stage::Pending { .. }) {
            let attachment = self.attachments.remove(index);
            tracing::info!(device = %attachment.device.name, client = %attachment.client, "USB attachment withdrawn before its tunnel opened");
            self.queue(&attachment.session, notice);
        } else {
            let attachment = &mut self.attachments[index];
            if attachment.ending.is_none() {
                attachment.ending = Some(notice);
                attachment.stop.notify_one();
            }
        }
        true
    }
}

struct Attachment {
    id: u64,
    session: String,
    /// The certificate of the device that asked: only it opens the tunnel.
    cert: String,
    client: String,
    request_id: u32,
    device: Requested,
    token: [u8; TOKEN_BYTES],
    stage: Stage,
    since: SystemTime,
    /// Wakes the relay to end the attachment.
    stop: Arc<Notify>,
    /// Set once something ended it: what to tell the client when the relay is done.
    ending: Option<Vec<UsbMessage>>,
}

/// The device as the client described it.
#[derive(Clone, Debug)]
struct Requested {
    vid: u16,
    pid: u16,
    class: u8,
    speed: u8,
    busid: String,
    name: String,
}

#[derive(Clone, Copy)]
enum Stage {
    Pending { expires: Instant },
    Opening,
    Attached { port: u32 },
}

/// An attachment as the console lists it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Listed {
    pub id: u64,
    pub client: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub class: u8,
    pub speed: u8,
    pub busid: String,
    /// `waiting` for the tunnel, `opening`, `attached` or `detaching`.
    pub state: &'static str,
    /// The driver's hub port once attached.
    pub port: Option<u32>,
    pub since_unix_ms: u64,
}

/// Why a tunnel token opens nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Unknown, expired, or another device's.
    Unknown,
    /// Already used.
    Used,
}

/// A claimed token: the attachment whose relay the tunnel carries.
#[derive(Debug)]
pub struct Opened {
    id: u64,
    request_id: u32,
    device: Requested,
    client: String,
    stop: Arc<Notify>,
}

fn status(request_id: u32, state: UsbState, reason: UsbReason) -> UsbMessage {
    UsbMessage::Status {
        request_id,
        state,
        reason,
    }
}

/// Compares tokens in constant time.
fn same(a: &[u8; TOKEN_BYTES], b: &[u8; TOKEN_BYTES]) -> bool {
    a.iter().zip(b).fold(0, |differ, (a, b)| differ | (a ^ b)) == 0
}

impl UsbForwarding {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn vhci(&self) -> Arc<dyn Vhci> {
        self.vhci
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    #[cfg(test)]
    pub fn set_vhci(&self, vhci: Arc<dyn Vhci>) {
        *self.vhci.lock().unwrap() = vhci;
    }

    /// Whether the USB/IP driver is installed.
    pub fn ready(&self) -> bool {
        self.vhci().ready()
    }

    /// A 0x553A message from the client of `session`; `enabled` and `limit` are the host's
    /// `usb_forwarding` and `usb_max_devices`.
    pub fn on_message(
        &self,
        session: &str,
        client: &Client,
        enabled: bool,
        limit: u8,
        message: UsbMessage,
    ) {
        match message {
            UsbMessage::AttachRequest {
                request_id,
                vid,
                pid,
                class,
                speed,
                busid,
                name,
            } => {
                let refusal = if !enabled {
                    Some(UsbReason::Disabled)
                } else if !client.allows_usb() {
                    Some(UsbReason::Permission)
                } else if !wire::valid_busid(&busid) {
                    Some(UsbReason::DeviceError)
                } else if !self.ready() {
                    Some(UsbReason::Driver)
                } else {
                    None
                };
                let mut registry = self.registry();
                if registry
                    .attachments
                    .iter()
                    .any(|a| a.session == session && a.request_id == request_id)
                {
                    tracing::warn!(client = %client.name, request_id, "USB attach request repeats a request id; ignored");
                    return;
                }
                let refusal = refusal.or_else(|| {
                    (registry.attachments.len() >= usize::from(limit)).then_some(UsbReason::Limit)
                });
                if let Some(reason) = refusal {
                    tracing::warn!(client = %client.name, device = %name, vid = format_args!("{vid:04x}"), pid = format_args!("{pid:04x}"), ?reason, "USB attach refused");
                    registry.queue(session, [status(request_id, UsbState::Refused, reason)]);
                    return;
                }
                tracing::info!(client = %client.name, device = %name, vid = format_args!("{vid:04x}"), pid = format_args!("{pid:04x}"), %busid, "USB attach requested");
                let token: [u8; TOKEN_BYTES] = rand::random();
                registry.next += 1;
                let id = registry.next;
                registry.attachments.push(Attachment {
                    id,
                    session: session.to_owned(),
                    cert: client.cert.clone(),
                    client: client.name.clone(),
                    request_id,
                    device: Requested {
                        vid,
                        pid,
                        class,
                        speed,
                        busid,
                        name,
                    },
                    token,
                    stage: Stage::Pending {
                        expires: Instant::now() + Duration::from_secs(TOKEN_TTL_SECS),
                    },
                    since: SystemTime::now(),
                    stop: Arc::default(),
                    ending: None,
                });
                registry.queue(session, [UsbMessage::TunnelReady { request_id, token }]);
            }
            UsbMessage::Detach { request_id } => {
                let mut registry = self.registry();
                let found = registry
                    .attachments
                    .iter()
                    .find(|a| a.session == session && a.request_id == request_id)
                    .map(|a| a.id);
                match found {
                    Some(id) => {
                        tracing::info!(client = %client.name, request_id, "USB detach requested by the client");
                        registry.end(
                            id,
                            vec![status(request_id, UsbState::Detached, UsbReason::None)],
                        );
                    }
                    None => tracing::debug!(request_id, "USB detach for no attachment"),
                }
            }
            other => tracing::debug!(?other, "USB message only a host sends"),
        }
    }

    /// What to send the client of `session` now. Reports tokens that ran out first.
    pub fn take(&self, session: &str, now: Instant) -> Vec<UsbMessage> {
        let mut registry = self.registry();
        if registry.attachments.is_empty() && registry.outbox.is_empty() {
            return Vec::new();
        }
        let mut expired = Vec::new();
        registry.attachments.retain(|a| {
            let gone = a.session == session
                && matches!(a.stage, Stage::Pending { expires } if expires <= now);
            if gone {
                tracing::warn!(client = %a.client, device = %a.device.name, "USB tunnel not opened in time");
                expired.push(status(
                    a.request_id,
                    UsbState::Failed,
                    UsbReason::TunnelTimeout,
                ));
            }
            !gone
        });
        registry.queue(session, expired);
        registry.outbox.remove(session).unwrap_or_default()
    }

    /// `session` ended or disconnected: detaches everything it attached and returns what to
    /// tell its client, if it is still there.
    pub fn end_session(&self, session: &str) -> Vec<UsbMessage> {
        let mut registry = self.registry();
        let mut messages = registry.outbox.remove(session).unwrap_or_default();
        registry.attachments.retain_mut(|a| {
            if a.session != session {
                return true;
            }
            tracing::info!(client = %a.client, device = %a.device.name, "USB device detached: the stream ended");
            match a.ending.replace(Vec::new()) {
                Some(notice) => messages.extend(notice),
                None => messages.extend([
                    UsbMessage::Detach {
                        request_id: a.request_id,
                    },
                    status(a.request_id, UsbState::Detached, UsbReason::SessionEnd),
                ]),
            }
            if matches!(a.stage, Stage::Pending { .. }) {
                return false;
            }
            a.stop.notify_one();
            true
        });
        messages
    }

    /// The console's list.
    pub fn list(&self) -> Vec<Listed> {
        self.registry()
            .attachments
            .iter()
            .map(|a| Listed {
                id: a.id,
                client: a.client.clone(),
                name: a.device.name.clone(),
                vid: a.device.vid,
                pid: a.device.pid,
                class: a.device.class,
                speed: a.device.speed,
                busid: a.device.busid.clone(),
                state: match (a.stage, a.ending.is_some()) {
                    (_, true) => "detaching",
                    (Stage::Pending { .. }, _) => "waiting",
                    (Stage::Opening, _) => "opening",
                    (Stage::Attached { .. }, _) => "attached",
                },
                port: match a.stage {
                    Stage::Attached { port } => Some(port),
                    _ => None,
                },
                since_unix_ms: a
                    .since
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64),
            })
            .collect()
    }

    /// The console detaches attachment `id`; false when there is none.
    pub fn detach(&self, id: u64) -> bool {
        let mut registry = self.registry();
        let Some(request_id) = registry
            .index(id)
            .map(|i| registry.attachments[i].request_id)
        else {
            return false;
        };
        tracing::info!(id, "USB device detached from the console");
        registry.end(
            id,
            vec![
                UsbMessage::Detach { request_id },
                status(request_id, UsbState::Detached, UsbReason::None),
            ],
        )
    }

    /// Takes the tunnel token of the device with certificate `cert`.
    pub fn claim(
        &self,
        token: &[u8; TOKEN_BYTES],
        cert: &str,
        now: Instant,
    ) -> Result<Opened, Refused> {
        let mut registry = self.registry();
        let attachment = registry
            .attachments
            .iter_mut()
            .find(|a| same(&a.token, token) && a.cert == cert)
            .ok_or(Refused::Unknown)?;
        match attachment.stage {
            Stage::Pending { expires } if expires > now && attachment.ending.is_none() => {}
            Stage::Pending { .. } => return Err(Refused::Unknown),
            Stage::Opening | Stage::Attached { .. } => return Err(Refused::Used),
        }
        attachment.stage = Stage::Opening;
        Ok(Opened {
            id: attachment.id,
            request_id: attachment.request_id,
            device: attachment.device.clone(),
            client: attachment.client.clone(),
            stop: attachment.stop.clone(),
        })
    }

    /// The upgrade of a claimed tunnel failed.
    pub fn abandon(&self, opened: Opened, error: impl std::fmt::Display) {
        tracing::warn!(client = %opened.client, device = %opened.device.name, %error, "USB tunnel upgrade failed");
        self.finish(
            &opened,
            vec![status(
                opened.request_id,
                UsbState::Failed,
                UsbReason::DeviceError,
            )],
        );
    }

    /// Relays a claimed tunnel to the driver until either end closes or something detaches it.
    pub async fn serve<T>(&self, opened: Opened, mut tunnel: T)
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let vhci = self.vhci();
        let busid = opened.device.busid.clone();
        let (driver, tcp_port, port) = match open(vhci.clone(), &mut tunnel, &opened).await {
            Ok(opened) => opened,
            Err((reason, error)) => {
                tracing::warn!(client = %opened.client, device = %opened.device.name, error = format!("{error:#}"), ?reason, "USB attach failed");
                return self.finish(
                    &opened,
                    vec![status(opened.request_id, UsbState::Failed, reason)],
                );
            }
        };
        self.attached(&opened, port);
        let outcome = pump(driver, tunnel, &opened.stop).await;
        let detached =
            tokio::task::spawn_blocking(move || vhci.detach(port, tcp_port, &busid)).await;
        match detached {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(port, error = format!("{error:#}"), "USB detach failed")
            }
            Err(error) => tracing::warn!(port, %error, "USB detach failed"),
        }
        let natural = match outcome {
            Ok(Closed::Requested) => Vec::new(),
            Ok(Closed::Device) => {
                tracing::info!(client = %opened.client, device = %opened.device.name, "USB device detached: the client closed its tunnel");
                vec![status(
                    opened.request_id,
                    UsbState::Detached,
                    UsbReason::None,
                )]
            }
            Ok(Closed::Driver) => {
                tracing::warn!(client = %opened.client, device = %opened.device.name, "USB device detached: the driver closed it");
                vec![status(
                    opened.request_id,
                    UsbState::Failed,
                    UsbReason::Driver,
                )]
            }
            Err(error) => {
                tracing::warn!(client = %opened.client, device = %opened.device.name, error = format!("{error:#}"), "USB device detached: the relay failed");
                vec![status(
                    opened.request_id,
                    UsbState::Failed,
                    UsbReason::DeviceError,
                )]
            }
        };
        self.finish(&opened, natural);
    }

    fn attached(&self, opened: &Opened, port: u32) {
        let mut registry = self.registry();
        let Some(index) = registry.index(opened.id) else {
            return;
        };
        let attachment = &mut registry.attachments[index];
        attachment.stage = Stage::Attached { port };
        if attachment.ending.is_none() {
            tracing::info!(client = %opened.client, device = %opened.device.name, port, "USB device attached");
            let session = attachment.session.clone();
            registry.queue(
                &session,
                [status(
                    opened.request_id,
                    UsbState::Attached,
                    UsbReason::None,
                )],
            );
        }
    }

    /// Forgets a relayed attachment and tells its client what ended it, unless that was said.
    fn finish(&self, opened: &Opened, natural: Vec<UsbMessage>) {
        let mut registry = self.registry();
        let Some(index) = registry.index(opened.id) else {
            return;
        };
        let attachment = registry.attachments.remove(index);
        let notice = attachment.ending.unwrap_or(natural);
        registry.queue(&attachment.session, notice);
    }
}

/// Imports the device into the driver: the driver's import request goes to the client and its
/// answer back, while the driver attaches. Returns the driver's connection, the loopback port
/// and the hub port.
async fn open<T>(
    vhci: Arc<dyn Vhci>,
    tunnel: &mut T,
    opened: &Opened,
) -> Result<(TcpStream, u16, u32), (UsbReason, anyhow::Error)>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("binding the USB/IP relay")
        .map_err(|e| (UsbReason::Driver, e))?;
    let tcp_port = listener
        .local_addr()
        .context("the USB/IP relay's port")
        .map_err(|e| (UsbReason::Driver, e))?
        .port();
    let busid = opened.device.busid.clone();
    let attaching = vhci.clone();
    let mut attach = std::pin::pin!(async move {
        tokio::task::spawn_blocking(move || attaching.attach(tcp_port, &busid))
            .await
            .context("the driver's attach stopped")?
    });
    let (imported, attached) = {
        let mut import = std::pin::pin!(tokio::time::timeout(
            OPEN_TIMEOUT,
            import(listener, tunnel, &opened.device)
        ));
        let (mut imported, mut attached) = (None, None);
        while imported.is_none() || attached.is_none() {
            tokio::select! {
                result = &mut import, if imported.is_none() => imported = Some(result.unwrap_or_else(|_| {
                    Err((UsbReason::Driver, anyhow!("the driver did not import the device within {OPEN_TIMEOUT:?}")))
                })),
                result = &mut attach, if attached.is_none() => {
                    // A failed attach never imports: stop waiting for it.
                    if result.is_err() && imported.is_none() {
                        imported = Some(Err((UsbReason::Driver, anyhow!("the driver did not import the device"))));
                    }
                    attached = Some(result);
                }
            }
        }
        (imported.unwrap(), attached.unwrap())
    };
    match (imported, attached) {
        (Ok(driver), Ok(port)) => Ok((driver, tcp_port, port)),
        (Err(failure), Ok(port)) => {
            let busid = opened.device.busid.clone();
            let _ = tokio::task::spawn_blocking(move || vhci.detach(port, tcp_port, &busid)).await;
            Err(failure)
        }
        // The client's answer explains a failed attach better than usbip.exe does.
        (Err((UsbReason::DeviceError, error)), Err(_)) => Err((UsbReason::DeviceError, error)),
        (_, Err(error)) => Err((UsbReason::Driver, error)),
    }
}

/// Takes the driver's connection and passes its import request and the client's reply,
/// checking that the reply is the device the client asked to attach.
async fn import<T>(
    listener: TcpListener,
    tunnel: &mut T,
    device: &Requested,
) -> Result<TcpStream, (UsbReason, anyhow::Error)>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let driver_error = |e: anyhow::Error| (UsbReason::Driver, e);
    let device_error = |e: anyhow::Error| (UsbReason::DeviceError, e);
    let (mut driver, _) = listener
        .accept()
        .await
        .context("accepting the driver")
        .map_err(driver_error)?;
    drop(listener);
    let _ = driver.set_nodelay(true);
    let mut request = [0; wire::IMPORT_REQUEST_BYTES];
    driver
        .read_exact(&mut request)
        .await
        .context("reading the driver's import request")
        .map_err(driver_error)?;
    match wire::decode_import_request(&request) {
        Some(busid) if busid == device.busid => {}
        other => {
            return Err(driver_error(anyhow!(
                "the driver asked for {other:?}, not {}",
                device.busid
            )));
        }
    }
    let mut reply = [0; wire::OP_HEADER_BYTES + wire::DEVICE_BYTES];
    let result: Result<usize> = async {
        tunnel.write_all(&request).await?;
        tunnel.flush().await?;
        tunnel
            .read_exact(&mut reply[..wire::OP_HEADER_BYTES])
            .await
            .context("the client closed the tunnel")?;
        let header = wire::OpHeader::decode(reply[..wire::OP_HEADER_BYTES].try_into()?);
        if header.code != wire::OP_REP_IMPORT {
            bail!("the client answered op {:#06x}", header.code);
        }
        if header.status != 0 {
            return Ok(wire::OP_HEADER_BYTES);
        }
        tunnel
            .read_exact(&mut reply[wire::OP_HEADER_BYTES..])
            .await
            .context("the client closed the tunnel")?;
        let described = wire::Device::decode(reply[wire::OP_HEADER_BYTES..].try_into()?)
            .context("the client described the device badly")?;
        if described.busid != device.busid
            || described.vendor != device.vid
            || described.product != device.pid
        {
            bail!(
                "the client served {} {:04x}:{:04x}, not {} {:04x}:{:04x}",
                described.busid,
                described.vendor,
                described.product,
                device.busid,
                device.vid,
                device.pid
            );
        }
        Ok(reply.len())
    }
    .await;
    let length = result.map_err(device_error)?;
    driver
        .write_all(&reply[..length])
        .await
        .context("answering the driver's import request")
        .map_err(driver_error)?;
    if length == wire::OP_HEADER_BYTES {
        return Err(device_error(anyhow!("the client refused the import")));
    }
    Ok(driver)
}

/// Which end closed a relay cleanly.
#[derive(Debug, PartialEq, Eq)]
enum Closed {
    Requested,
    Device,
    Driver,
}

#[derive(Clone, Copy)]
struct Submitted {
    direction: u32,
    length: u32,
    packets: u32,
}

/// The driver's transfers the client has yet to answer.
#[derive(Default)]
struct InFlight {
    submits: HashMap<u32, Submitted>,
    /// Unlink seqnum to the submit it cancels.
    unlinks: HashMap<u32, u32>,
}

/// Passes USB/IP both ways until an end closes or `stop` is notified.
async fn pump<T>(driver: TcpStream, tunnel: T, stop: &Notify) -> Result<Closed>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (mut from_driver, mut to_driver) = driver.into_split();
    let (mut from_client, mut to_client) = tokio::io::split(tunnel);
    let flight = Mutex::new(InFlight::default());
    tokio::select! {
        result = commands(&mut from_driver, &mut to_client, &flight) => result.map(|()| Closed::Driver),
        result = replies(&mut from_client, &mut to_driver, &flight) => result.map(|()| Closed::Device),
        () = stop.notified() => Ok(Closed::Requested),
    }
}

/// Reads one header; false when the peer closed between messages.
async fn header<R: AsyncRead + Unpin>(
    from: &mut R,
    header: &mut [u8; wire::HEADER_BYTES],
) -> Result<bool> {
    let first = from.read(header).await?;
    if first == 0 {
        return Ok(false);
    }
    from.read_exact(&mut header[first..]).await?;
    Ok(true)
}

/// The driver's submits and unlinks, to the client.
async fn commands<R, W>(from: &mut R, to: &mut W, flight: &Mutex<InFlight>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut message = Vec::new();
    loop {
        let mut head = [0; wire::HEADER_BYTES];
        if !header(from, &mut head)
            .await
            .context("reading the driver")?
        {
            return Ok(());
        }
        let payload = match Header::decode(&head) {
            Some(Header::Submit {
                base,
                length,
                packets,
                ..
            }) => {
                let payload =
                    wire::submit_payload(base.direction, length, packets).with_context(|| {
                        format!("the driver submitted {length} bytes, {packets} packets")
                    })?;
                let mut flight = flight.lock().unwrap();
                if flight.submits.len() >= MAX_IN_FLIGHT {
                    bail!("the driver has more than {MAX_IN_FLIGHT} transfers in flight");
                }
                let submitted = Submitted {
                    direction: base.direction,
                    length,
                    packets,
                };
                if flight.submits.insert(base.seqnum, submitted).is_some() {
                    bail!("the driver reused seqnum {}", base.seqnum);
                }
                payload
            }
            Some(Header::Unlink { base, unlink }) => {
                flight.lock().unwrap().unlinks.insert(base.seqnum, unlink);
                0
            }
            other => bail!("the driver sent {other:?}"),
        };
        message.clear();
        message.extend_from_slice(&head);
        message.resize(wire::HEADER_BYTES + payload, 0);
        from.read_exact(&mut message[wire::HEADER_BYTES..])
            .await
            .context("reading the driver")?;
        to.write_all(&message).await.context("writing the tunnel")?;
        to.flush().await.context("writing the tunnel")?;
    }
}

/// The client's replies, to the driver, each checked against what the driver asked.
async fn replies<R, W>(from: &mut R, to: &mut W, flight: &Mutex<InFlight>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut message = Vec::new();
    loop {
        let mut head = [0; wire::HEADER_BYTES];
        if !header(from, &mut head)
            .await
            .context("reading the tunnel")?
        {
            return Ok(());
        }
        let payload = match Header::decode(&head) {
            Some(Header::RetSubmit {
                base,
                length,
                packets,
                ..
            }) => {
                let submitted = flight
                    .lock()
                    .unwrap()
                    .submits
                    .remove(&base.seqnum)
                    .with_context(|| {
                        format!(
                            "the client answered seqnum {}, which is not in flight",
                            base.seqnum
                        )
                    })?;
                wire::reply_payload(
                    submitted.direction,
                    submitted.length,
                    submitted.packets,
                    length,
                    packets,
                )
                .with_context(|| {
                    format!(
                        "the client answered {} bytes and {packets} packets to {} bytes and {} packets",
                        length, submitted.length, submitted.packets
                    )
                })?
            }
            Some(Header::RetUnlink { base, status }) => {
                let mut flight = flight.lock().unwrap();
                let unlinked = flight.unlinks.remove(&base.seqnum).with_context(|| {
                    format!(
                        "the client answered unlink {}, which is not in flight",
                        base.seqnum
                    )
                })?;
                if status == wire::ECONNRESET {
                    flight.submits.remove(&unlinked);
                }
                0
            }
            other => bail!("the client sent {other:?}"),
        };
        message.clear();
        message.extend_from_slice(&head);
        message.resize(wire::HEADER_BYTES + payload, 0);
        from.read_exact(&mut message[wire::HEADER_BYTES..])
            .await
            .context("reading the tunnel")?;
        to.write_all(&message).await.context("writing the driver")?;
    }
}

#[cfg(test)]
pub(crate) mod tests;
