//! The USB/IP server one forwarded device runs: the driver's import, then its submits and
//! unlinks, answered as the device completes them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rubylight_protocol::usbip::{self as wire, Base, Header, IsoPacket};

use super::{read_whole, Tunnel};

/// Which way a transfer's data goes, from the host's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Out,
    In,
}

/// One transfer the host's driver submitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub seqnum: u32,
    /// The endpoint number, 0-15; 0 is the control endpoint.
    pub endpoint: u8,
    pub direction: Direction,
    /// Linux's URB transfer flags (`URB_SHORT_NOT_OK` 0x1, `URB_ISO_ASAP` 0x2,
    /// `URB_ZERO_PACKET` 0x40).
    pub flags: u32,
    /// The IN buffer's size, or the OUT data's.
    pub length: u32,
    /// A control transfer's setup packet.
    pub setup: [u8; 8],
    /// The OUT data.
    pub data: Vec<u8>,
    pub interval: u32,
    pub start_frame: i32,
    /// An isochronous transfer's packets; empty for any other.
    pub iso: Vec<IsoPacket>,
}

impl Transfer {
    pub fn is_control(&self) -> bool {
        self.endpoint == 0
    }

    pub fn is_isochronous(&self) -> bool {
        !self.iso.is_empty()
    }
}

/// How a transfer ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// 0, or a negated Linux errno: [`wire::EPIPE`] for a stall, [`wire::ENODEV`] for a
    /// device that went away, [`wire::EPROTO`] for any other failure.
    pub status: i32,
    /// The IN data, at most the transfer's length. For an isochronous transfer, the packets'
    /// data back to back, as their actual lengths say.
    pub data: Vec<u8>,
    /// The bytes an OUT transfer sent; ignored for IN, where `data` says.
    pub sent: u32,
    pub start_frame: i32,
    /// An isochronous transfer's packets, in the order asked, with their actual lengths and
    /// statuses. Left empty on a failure, every packet fails with `status`.
    pub iso: Vec<IsoPacket>,
    pub error_count: u32,
}

impl Reply {
    /// An IN transfer that read `data`.
    pub fn read(data: Vec<u8>) -> Self {
        Self {
            data,
            ..Self::default()
        }
    }

    /// An OUT transfer that sent `sent` bytes.
    pub fn sent(sent: u32) -> Self {
        Self {
            sent,
            ..Self::default()
        }
    }

    /// A transfer that failed with negated errno `status`.
    pub fn failed(status: i32) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }
}

/// A device the client serves. Submits come from the server's thread and must return at
/// once; [`Blocking`](super::Blocking) runs a device that can only block.
pub trait UsbDevice: Send + Sync {
    /// The device as the import reply describes it. Its busid, vendor and product are the
    /// ones the attach request named.
    fn describe(&self) -> wire::Device;
    /// bDeviceClass, or the first interface's class when that is 0.
    fn class(&self) -> u8 {
        self.describe().class
    }
    /// Starts `transfer`. `done` completes it from any thread, once; dropping it fails the
    /// transfer with [`wire::ESHUTDOWN`].
    fn submit(&self, transfer: Transfer, done: Completion);
    /// The driver gave up on `seqnum`: stop it if it still runs, so that it takes nothing more
    /// from the device once this returns; the server answers the unlink after. Its completion
    /// is ignored.
    fn unlink(&self, seqnum: u32);
    /// The attachment ended: stop every transfer and let go of the device.
    fn close(&self);
    /// A device a USB/IP server on this machine serves whole (Linux's usbip-host behind
    /// usbipd): a connection to that server, which the tunnel is relayed to byte for byte once
    /// the driver's import names this device. None for a device served transfer by transfer.
    fn relay(&self) -> Option<std::io::Result<Arc<dyn Tunnel>>> {
        None
    }
}

/// Why serving a device ended other than by the host closing the tunnel.
#[derive(Debug)]
pub enum ServeError {
    Io(std::io::Error),
    /// The host broke USB/IP, or asked for another device.
    Protocol(String),
    /// The device answered a transfer with a reply that does not fit it.
    Device(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "the USB tunnel failed: {e}"),
            Self::Protocol(e) => write!(f, "the host broke USB/IP: {e}"),
            Self::Device(e) => write!(f, "the device answered badly: {e}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<std::io::Error> for ServeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

struct Shared {
    tunnel: Arc<dyn Tunnel>,
    state: Mutex<State>,
}

/// The transfers one device may have in flight. The driver queues a few per endpoint; one
/// that queues more than this would pile up buffers on the client.
pub(crate) const MAX_IN_FLIGHT: usize = 1024;
/// The bytes those transfers may ask for together, each up to [`wire::MAX_TRANSFER`].
pub(crate) const MAX_BUFFERED: u64 = 64 << 20;

#[derive(Default)]
struct State {
    /// Submits the device has yet to complete, with the bytes each asked for; a completion for
    /// any other is dropped.
    pending: HashMap<u32, u64>,
    /// The sum of `pending`'s bytes.
    buffered: u64,
    /// A submit was refused for going past [`MAX_IN_FLIGHT`] or [`MAX_BUFFERED`]: said once.
    refused: bool,
    /// Set once serving ends: late completions write nothing.
    closed: bool,
    /// What made a completion close the tunnel.
    failure: Option<ServeError>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// Takes `seqnum` out of `pending`: whether it was there.
    fn settle(&mut self, seqnum: u32) -> bool {
        match self.pending.remove(&seqnum) {
            Some(bytes) => {
                self.buffered -= bytes;
                true
            }
            None => false,
        }
    }
}

impl Shared {
    /// Ends serving with `error` unless it already ended; the read loop wakes and returns it.
    fn fail(&self, state: &mut State, error: ServeError) {
        if !state.closed && state.failure.is_none() {
            state.failure = Some(error);
        }
        self.tunnel.close();
    }
}

/// What a completion must fit: the submit's direction, size and packets.
struct Asked {
    seqnum: u32,
    direction: Direction,
    length: u32,
    /// The submit's packets field as sent, [`wire::NOT_ISO`] or 0 for most transfers.
    packets: u32,
    iso: Vec<IsoPacket>,
}

/// Completes one transfer. See [`UsbDevice::submit`].
pub struct Completion {
    shared: Arc<Shared>,
    asked: Option<Asked>,
}

impl Completion {
    pub fn seqnum(&self) -> u32 {
        self.asked.as_ref().map_or(0, |asked| asked.seqnum)
    }

    /// Sends `reply` for the transfer, unless the driver unlinked it or serving ended.
    pub fn complete(mut self, reply: Reply) {
        if let Some(asked) = self.asked.take() {
            finish(&self.shared, &asked, reply);
        }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if let Some(asked) = self.asked.take() {
            finish(&self.shared, &asked, Reply::failed(wire::ESHUTDOWN));
        }
    }
}

fn finish(shared: &Shared, asked: &Asked, reply: Reply) {
    let mut state = shared.state();
    if state.closed || !state.settle(asked.seqnum) {
        return;
    }
    let message = match ret_submit(asked, reply) {
        Ok(message) => message,
        Err(e) => return shared.fail(&mut state, ServeError::Device(e)),
    };
    // Written under the lock, so an unlink's answer never overtakes it or goes out after it.
    if let Err(e) = shared.tunnel.write_all(&message) {
        shared.fail(&mut state, ServeError::Io(e));
    }
}

/// The `RET_SUBMIT` for `reply`, or why it does not fit the submit.
fn ret_submit(asked: &Asked, mut reply: Reply) -> Result<Vec<u8>, String> {
    let seqnum = asked.seqnum;
    if reply.status != 0 && reply.iso.is_empty() {
        // A failure moves no data, and every packet fails with it.
        reply.data.clear();
        reply.sent = 0;
        reply.iso = asked
            .iso
            .iter()
            .map(|packet| IsoPacket {
                actual_length: 0,
                status: reply.status,
                ..*packet
            })
            .collect();
        reply.error_count = asked.iso.len() as u32;
    }
    let length = match asked.direction {
        Direction::In => u32::try_from(reply.data.len()).map_err(|_| "too much data")?,
        Direction::Out => {
            reply.data.clear();
            reply.sent
        }
    };
    if length > asked.length {
        return Err(format!(
            "{length} bytes for transfer {seqnum} of {}",
            asked.length
        ));
    }
    if !asked.iso.is_empty() {
        if !wire::iso_reply_fits(&asked.iso, reply.iso.iter().copied(), length) {
            return Err(format!(
                "isochronous packets for transfer {seqnum} that do not fit the {length} bytes or the packets asked"
            ));
        }
    } else if !reply.iso.is_empty() {
        return Err(format!(
            "isochronous packets for transfer {seqnum}, which has none"
        ));
    }
    let header = Header::RetSubmit {
        base: Base {
            seqnum,
            ..Base::default()
        },
        status: reply.status,
        length,
        start_frame: reply.start_frame,
        // As many as asked, in the submit's own form.
        packets: asked.packets,
        errors: reply.error_count,
    };
    let mut message = Vec::with_capacity(
        wire::HEADER_BYTES + reply.data.len() + reply.iso.len() * wire::ISO_PACKET_BYTES,
    );
    message.extend_from_slice(&header.encode());
    message.extend_from_slice(&reply.data);
    for packet in &reply.iso {
        message.extend_from_slice(&packet.encode());
    }
    Ok(message)
}

/// Serves `device` over `tunnel` until the host closes it (`Ok`) or something breaks. Answers
/// the driver's import with the device, then hands every submit to the device and answers
/// each unlink. When it returns the tunnel is closed and the device too.
pub fn serve(tunnel: Arc<dyn Tunnel>, device: Arc<dyn UsbDevice>) -> Result<(), ServeError> {
    let shared = Arc::new(Shared {
        tunnel: tunnel.clone(),
        state: Mutex::new(State::default()),
    });
    let result = run(&shared, device.as_ref());
    let failure = {
        let mut state = shared.state();
        state.closed = true;
        state.pending.clear();
        state.buffered = 0;
        state.failure.take()
    };
    tunnel.close();
    device.close();
    match failure {
        // A completion's failure is why the read broke.
        Some(failure) => Err(failure),
        None => result,
    }
}

/// Hands the driver's import `request` to `peer`, a USB/IP server here, and passes its answer
/// back: a refusal ends it, else every byte goes both ways until either side closes. The side
/// that closed first says how it ended.
fn relay(
    tunnel: &Arc<dyn Tunnel>,
    peer: Arc<dyn Tunnel>,
    request: &[u8; wire::IMPORT_REQUEST_BYTES],
) -> Result<(), ServeError> {
    let answered = (|| -> std::io::Result<wire::OpHeader> {
        peer.write_all(request)?;
        let mut head = [0; wire::OP_HEADER_BYTES];
        if !read_whole(peer.as_ref(), &mut head)? {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        tunnel.write_all(&head)?;
        Ok(wire::OpHeader::decode(&head))
    })();
    let head = match answered {
        Ok(head) => head,
        Err(e) => {
            peer.close();
            return Err(ServeError::Device(format!(
                "the USB/IP server here did not answer the import: {e}"
            )));
        }
    };
    if head.code != wire::OP_REP_IMPORT || head.status != 0 {
        peer.close();
        return Err(ServeError::Device(format!(
            "the USB/IP server here refused the device (code {:#06x}, status {})",
            head.code, head.status
        )));
    }
    let first = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let back = {
        let (tunnel, peer, first) = (tunnel.clone(), peer.clone(), first.clone());
        std::thread::Builder::new()
            .name("usb-relay".into())
            .spawn(move || {
                let pumped = pump(peer.as_ref(), tunnel.as_ref());
                let was_first = !first.swap(true, std::sync::atomic::Ordering::SeqCst);
                tunnel.close();
                (pumped, was_first)
            })
    };
    let back = match back {
        Ok(back) => back,
        Err(e) => {
            peer.close();
            return Err(ServeError::Io(e));
        }
    };
    let forth = pump(tunnel.as_ref(), peer.as_ref());
    let forth_first = !first.swap(true, std::sync::atomic::Ordering::SeqCst);
    peer.close();
    let (back, back_first) = back
        .join()
        .unwrap_or_else(|_| (Err(std::io::Error::other("the relay thread panicked")), true));
    match (forth_first, back_first) {
        (true, _) => forth.map_err(ServeError::Io),
        _ => back.map_err(ServeError::Io),
    }
}

/// Copies what `from` reads to `to` until `from` closes.
fn pump(from: &dyn Tunnel, to: &dyn Tunnel) -> std::io::Result<()> {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        match from.read(&mut buffer)? {
            0 => return Ok(()),
            n => to.write_all(&buffer[..n])?,
        }
    }
}

fn run(shared: &Arc<Shared>, device: &dyn UsbDevice) -> Result<(), ServeError> {
    let tunnel = shared.tunnel.as_ref();
    let described = device.describe();
    let mut request = [0; wire::IMPORT_REQUEST_BYTES];
    if !read_whole(tunnel, &mut request)? {
        return Ok(());
    }
    match wire::decode_import_request(&request) {
        Some(busid) if busid == described.busid => {}
        other => {
            tunnel.write_all(&wire::import_reply(1))?;
            return Err(ServeError::Protocol(format!(
                "the driver asked for {other:?}, not {}",
                described.busid
            )));
        }
    }
    if let Some(peer) = device.relay() {
        let peer = peer.map_err(|e| {
            ServeError::Device(format!("the USB/IP server here did not answer: {e}"))
        })?;
        return relay(&shared.tunnel, peer, &request);
    }
    let encoded = described.encode().ok_or_else(|| {
        ServeError::Device(format!(
            "busid {:?} or path {:?} does not fit",
            described.busid, described.path
        ))
    })?;
    let mut reply = wire::import_reply(0).to_vec();
    reply.extend_from_slice(&encoded);
    tunnel.write_all(&reply)?;

    loop {
        let mut head = [0; wire::HEADER_BYTES];
        if !read_whole(tunnel, &mut head)? {
            return Ok(());
        }
        match Header::decode(&head) {
            Some(Header::Submit {
                base,
                flags,
                length,
                start_frame,
                packets,
                interval,
                setup,
            }) => {
                let (direction, endpoint) = match (base.direction, u8::try_from(base.ep)) {
                    (wire::DIR_OUT, Ok(ep @ 0..=15)) => (Direction::Out, ep),
                    (wire::DIR_IN, Ok(ep @ 0..=15)) => (Direction::In, ep),
                    _ => {
                        return Err(ServeError::Protocol(format!(
                            "a submit to endpoint {} direction {}",
                            base.ep, base.direction
                        )))
                    }
                };
                let payload =
                    wire::submit_payload(base.direction, length, packets).ok_or_else(|| {
                        ServeError::Protocol(format!(
                            "a submit of {length} bytes and {packets} packets"
                        ))
                    })?;
                let mut body = vec![0; payload];
                if !read_whole(tunnel, &mut body)? && payload > 0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
                }
                let iso: Vec<IsoPacket> = wire::iso_descriptors(&body, packets)
                    .ok_or_else(|| ServeError::Protocol("short packet descriptors".into()))?
                    .collect();
                body.truncate(body.len() - iso.len() * wire::ISO_PACKET_BYTES);
                let asked = Asked {
                    seqnum: base.seqnum,
                    direction,
                    length,
                    packets,
                    iso: iso.clone(),
                };
                let bytes = u64::from(length);
                let mut state = shared.state();
                if state.pending.contains_key(&base.seqnum) {
                    return Err(ServeError::Protocol(format!(
                        "seqnum {} reused while in flight",
                        base.seqnum
                    )));
                }
                if state.pending.len() >= MAX_IN_FLIGHT || state.buffered + bytes > MAX_BUFFERED {
                    // Refused as Linux refuses a submit it has no memory for. Answered under
                    // the lock, like every completion.
                    let first = !std::mem::replace(&mut state.refused, true);
                    let (count, buffered) = (state.pending.len(), state.buffered);
                    let answer = ret_submit(&asked, Reply::failed(wire::ENOMEM))
                        .map_err(ServeError::Device)?;
                    tunnel.write_all(&answer)?;
                    drop(state);
                    if first {
                        super::warn(&format!(
                            "USB {}: the host's driver has {count} transfers ({} KiB) in flight; \
                             a device takes at most {MAX_IN_FLIGHT} or {} KiB, so more fail with \
                             ENOMEM until some complete",
                            described.busid,
                            buffered >> 10,
                            MAX_BUFFERED >> 10
                        ));
                    }
                    continue;
                }
                state.pending.insert(base.seqnum, bytes);
                state.buffered += bytes;
                drop(state);
                let done = Completion {
                    shared: shared.clone(),
                    asked: Some(asked),
                };
                device.submit(
                    Transfer {
                        seqnum: base.seqnum,
                        endpoint,
                        direction,
                        flags,
                        length,
                        setup,
                        data: body,
                        interval,
                        start_frame,
                        iso,
                    },
                    done,
                );
            }
            Some(Header::Unlink { base, unlink }) => {
                // Out of `pending` first, so its completion, however close, is dropped.
                let cancelled = shared.state().settle(unlink);
                // Stopped before it is answered, as Linux's own USB/IP host does: once the
                // driver hears, the endpoint's next transfer gets what the device sends next,
                // not the transfer it gave up on. Out of the lock, because stopping it may
                // complete it at once.
                if cancelled {
                    device.unlink(unlink);
                }
                let status = if cancelled { wire::ECONNRESET } else { 0 };
                let answer = Header::RetUnlink {
                    base: Base {
                        seqnum: base.seqnum,
                        ..Base::default()
                    },
                    status,
                };
                // Under the lock, like every completion, so the two never interleave.
                let state = shared.state();
                tunnel.write_all(&answer.encode())?;
                drop(state);
            }
            other => return Err(ServeError::Protocol(format!("the driver sent {other:?}"))),
        }
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
pub(crate) mod tests;
