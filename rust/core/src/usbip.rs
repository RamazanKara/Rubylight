//! A USB/IP server for one virtual USB device on the loopback interface.
//! A USB/IP client attaches it (on Windows, usbip-win2's signed UDE
//! driver), and the device then looks to the host like one plugged into a
//! USB port, with no driver of ours in the kernel.
//!
//! The protocol is the Linux kernel's (Documentation/usb/usbip_protocol):
//! big-endian `OP_REQ_IMPORT` to attach, then 48-byte `CMD_SUBMIT` and
//! `CMD_UNLINK` headers, answered by `RET_SUBMIT` and `RET_UNLINK`.
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The bus ID the device is exported under.
pub const BUS_ID: &str = "1-1";
const VERSION: u16 = 0x0111;
const OP_REQ_DEVLIST: u16 = 0x8005;
const OP_REQ_IMPORT: u16 = 0x8003;
const CMD_SUBMIT: u32 = 1;
const CMD_UNLINK: u32 = 2;
const RET_SUBMIT: u32 = 3;
const RET_UNLINK: u32 = 4;
const EPIPE: i32 = -32;
const ECONNRESET: i32 = -104;
/// Bound both HID and audio transfer buffers before allocating them.
const MAX_TRANSFER: usize = 65536;
/// How long a client may take to ask for the device once connected.
const IMPORT_WAIT: Duration = Duration::from_secs(5);
/// How often a blocked read looks for [`Export`] being dropped. Windows
/// does not wake a blocked receive when another thread shuts the socket
/// down, so reads time out and look instead.
const READ_SLICE: Duration = Duration::from_millis(100);
/// A client that stops reading is dropped after this long, so the input
/// thread never waits on it.
const WRITE_WAIT: Duration = Duration::from_secs(1);

/// A control request's setup packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}
impl Setup {
    fn parse(b: &[u8; 8]) -> Self {
        Self {
            request_type: b[0],
            request: b[1],
            value: u16::from_le_bytes([b[2], b[3]]),
            index: u16::from_le_bytes([b[4], b[5]]),
            length: u16::from_le_bytes([b[6], b[7]]),
        }
    }
}

/// What the device list and import replies say about the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbDevice {
    /// Linux USB speed: 2 = full speed, 3 = high speed.
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub release: u16,
    /// Class, subclass and protocol of each interface.
    pub interfaces: Vec<(u8, u8, u8)>,
}
impl UsbDevice {
    /// `usbip_usb_device`: a device in configuration 1 at its declared speed.
    fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; 256 + 32];
        let path = format!("/sys/devices/butterpollo/usb1/{BUS_ID}");
        b[..path.len()].copy_from_slice(path.as_bytes());
        b[256..256 + BUS_ID.len()].copy_from_slice(BUS_ID.as_bytes());
        for n in [1u32, 2, self.speed] {
            // bus 1, device 2
            b.extend_from_slice(&n.to_be_bytes());
        }
        for n in [self.vendor, self.product, self.release] {
            b.extend_from_slice(&n.to_be_bytes());
        }
        b.extend_from_slice(&[0, 0, 0, 1, 1, self.interfaces.len() as u8]);
        b
    }
}

/// A virtual USB device. Control requests are answered at once; interrupt
/// IN transfers wait for [`Device::interrupt`] to have data.
pub trait Device: Send + 'static {
    fn describe(&self) -> UsbDevice;
    /// Answers a control request on endpoint 0 with the data to return (empty
    /// for OUT requests); None stalls it. `data` is what an OUT request sent.
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>>;
    /// The next report on an interrupt IN endpoint. None holds that
    /// endpoint's transfers until the client cancels them.
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>>;
    fn output(&mut self, _endpoint: u8, _data: &[u8]) -> bool {
        false
    }
    /// Maximum packet size and frame alignment for a 1 ms isochronous endpoint.
    fn iso_layout(&self, _endpoint: u8, _input: bool) -> Option<(usize, usize)> {
        None
    }
    /// An audio deadline was missed or its transport disconnected.
    fn iso_discontinuity(&mut self) {}
    /// One validated USB frame, at its scheduled time. IN returns its data;
    /// OUT consumes data and returns an empty vector. None stalls the packet.
    fn iso(
        &mut self,
        _endpoint: u8,
        _input: bool,
        _data: &[u8],
        _length: usize,
    ) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Clone, Debug)]
struct IsoPacket {
    offset: usize,
    length: usize,
}
struct IsoTransfer {
    seqnum: u32,
    endpoint: u8,
    input: bool,
    length: usize,
    packets: Vec<IsoPacket>,
    data: Vec<u8>,
    due: Instant,
    start_frame: u32,
}
impl IsoTransfer {
    fn reply(&self, status: i32, data: &[u8], lengths: &[usize]) -> Vec<u8> {
        let mut b = ret(
            RET_SUBMIT,
            self.seqnum,
            status,
            self.input.then_some(data),
            if status != 0 {
                0
            } else if self.input {
                data.len()
            } else {
                self.length
            },
        );
        b[28..32].copy_from_slice(&self.start_frame.to_be_bytes());
        b[32..36].copy_from_slice(&(self.packets.len() as u32).to_be_bytes());
        b[36..40].copy_from_slice(
            &(if status == 0 {
                0
            } else {
                self.packets.len() as u32
            })
            .to_be_bytes(),
        );
        for (i, p) in self.packets.iter().enumerate() {
            for n in [
                p.offset as u32,
                p.length as u32,
                if status != 0 {
                    0
                } else if self.input {
                    lengths[i] as u32
                } else {
                    p.length as u32
                },
                status as u32,
            ] {
                b.extend_from_slice(&n.to_be_bytes());
            }
        }
        b
    }
}

/// An interrupt IN transfer waiting for data.
#[derive(Clone, Copy, Debug)]
struct Transfer {
    seqnum: u32,
    endpoint: u8,
    length: usize,
}
#[derive(Default)]
struct Link {
    /// The connected client, for replies.
    stream: Option<TcpStream>,
    /// Whether the client imported the device and URBs flow.
    imported: bool,
    /// Transfers on endpoints that report, oldest first.
    pending: VecDeque<Transfer>,
    /// Transfers on endpoints that never report, kept until unlinked.
    held: Vec<Transfer>,
    iso: VecDeque<IsoTransfer>,
    iso_next: [Option<Instant>; 16],
    /// Whether the device changed since its last report.
    changed: bool,
}
impl Link {
    /// Sends a reply; a client that cannot take it is disconnected.
    fn send(&mut self, b: &[u8]) {
        if let Some(stream) = &mut self.stream
            && stream.write_all(b).is_err()
        {
            let _ = stream.shutdown(Shutdown::Both);
            self.stream = None;
            self.imported = false;
        }
    }
}
struct Shared<D> {
    device: Mutex<D>,
    link: Mutex<Link>,
    wake: Condvar,
    stop: AtomicBool,
    period: Duration,
    epoch: Instant,
    /// How late an OUT transfer may complete and still play its audio.
    late: Duration,
}

/// One device served on `127.0.0.1` at [`Export::port`] under [`BUS_ID`].
/// A client that disconnects may attach again. Dropping it disconnects the
/// client and stops the server.
pub struct Export<D: Device> {
    shared: Arc<Shared<D>>,
    port: u16,
    threads: Vec<JoinHandle<()>>,
}
impl<D: Device> Export<D> {
    /// Serves `device`, completing an interrupt transfer at least every
    /// `period` while the client waits for one, and at once after
    /// [`Export::changed`].
    pub fn listen(device: D, period: Duration) -> io::Result<Self> {
        Self::listen_with(device, period, crate::haptics::MAX_AGE)
    }
    /// As [`Export::listen`], playing the audio of OUT transfers that complete at most `late`
    /// after they were due.
    fn listen_with(device: D, period: Duration, late: Duration) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            device: Mutex::new(device),
            link: Mutex::default(),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            period,
            epoch: Instant::now(),
            late,
        });
        let mut threads = vec![];
        let accept = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("usbip-accept".into())
                .spawn(move || accept_clients(&accept, &listener))?,
        );
        let tick = shared.clone();
        let ticker = std::thread::Builder::new()
            .name("usbip-reports".into())
            .spawn(move || report(&tick));
        let mut export = Self {
            shared,
            port,
            threads,
        };
        export.threads.push(ticker?);
        Ok(export)
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    /// Whether a client has the device attached.
    pub fn imported(&self) -> bool {
        self.shared.link.lock().unwrap().imported
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut D) -> R) -> R {
        f(&mut self.shared.device.lock().unwrap())
    }
    /// Reports the device's new state without waiting for the period.
    pub fn changed(&self) {
        self.shared.link.lock().unwrap().changed = true;
        self.shared.wake.notify_all();
    }
}
impl<D: Device> Drop for Export<D> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        {
            let mut link = self.shared.link.lock().unwrap();
            if let Some(stream) = link.stream.take() {
                let _ = stream.shutdown(Shutdown::Both);
            }
            link.imported = false;
        }
        self.shared.wake.notify_all();
        // Shutdown must not depend on another loopback connection succeeding.
        // An unpark token also wakes an accept worker that has not parked yet.
        for thread in &self.threads {
            thread.thread().unpark();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn accept_clients<D: Device>(shared: &Shared<D>, listener: &TcpListener) {
    while !shared.stop.load(Ordering::Acquire) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if error.kind() != io::ErrorKind::WouldBlock {
                    tracing::debug!(%error, "USB/IP accept failed");
                }
                // Only idle attachment waits here; connected input and audio
                // retain their own report deadlines and blocking socket I/O.
                std::thread::park_timeout(READ_SLICE);
                continue;
            }
        };
        if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
            continue;
        }
        if let Err(error) = serve(shared, stream) {
            tracing::debug!(%error, "USB/IP client disconnected");
        }
        let mut link = shared.link.lock().unwrap();
        link.stream = None;
        link.imported = false;
        link.pending.clear();
        link.held.clear();
        link.iso.clear();
        link.iso_next = [None; 16];
        shared.device.lock().unwrap().iso_discontinuity();
    }
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
fn op_reply(code: u16, status: u32) -> Vec<u8> {
    let mut b = VERSION.to_be_bytes().to_vec();
    b.extend_from_slice(&code.to_be_bytes());
    b.extend_from_slice(&status.to_be_bytes());
    b
}
/// `RET_SUBMIT` or `RET_UNLINK`: 48 bytes, then any IN data.
fn ret(command: u32, seqnum: u32, status: i32, data: Option<&[u8]>, length: usize) -> Vec<u8> {
    let mut b = Vec::with_capacity(48 + data.map_or(0, <[u8]>::len));
    b.extend_from_slice(&command.to_be_bytes());
    b.extend_from_slice(&seqnum.to_be_bytes());
    b.extend_from_slice(&[0; 12]);
    b.extend_from_slice(&status.to_be_bytes());
    if command == RET_SUBMIT {
        b.extend_from_slice(&(length as u32).to_be_bytes());
        // start frame, number of packets (none: not isochronous), errors
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&u32::MAX.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
    }
    b.resize(48, 0);
    if let Some(data) = data {
        b.extend_from_slice(data);
    }
    b
}

/// Fills `buf`, giving up when the export stops or `deadline` passes.
fn read_full<D: Device>(
    shared: &Shared<D>,
    mut stream: &TcpStream,
    buf: &mut [u8],
    deadline: Option<Instant>,
) -> io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if shared.stop.load(Ordering::Acquire)
                    || deadline.is_some_and(|d| Instant::now() >= d)
                {
                    return Err(e);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn serve<D: Device>(shared: &Shared<D>, stream: TcpStream) -> io::Result<()> {
    // Accepted sockets may inherit the nonblocking listener's mode.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_WAIT))?;
    stream.set_read_timeout(Some(READ_SLICE))?;
    {
        let mut link = shared.link.lock().unwrap();
        if shared.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        link.stream = Some(stream.try_clone()?);
    }
    let deadline = Some(Instant::now() + IMPORT_WAIT);
    let read =
        |buf: &mut [u8], deadline: Option<Instant>| read_full(shared, &stream, buf, deadline);
    let mut head = [0u8; 8];
    read(&mut head, deadline)?;
    let code = u16::from_be_bytes([head[2], head[3]]);
    let describe = || shared.device.lock().unwrap().describe();
    match code {
        OP_REQ_DEVLIST => {
            let device = describe();
            let mut b = op_reply(0x0005, 0);
            b.extend_from_slice(&1u32.to_be_bytes());
            b.extend_from_slice(&device.encode());
            for (class, subclass, protocol) in device.interfaces {
                b.extend_from_slice(&[class, subclass, protocol, 0]);
            }
            (&stream).write_all(&b)?;
            return Ok(());
        }
        OP_REQ_IMPORT => {
            let mut busid = [0u8; 32];
            read(&mut busid, deadline)?;
            let len = busid.iter().position(|b| *b == 0).unwrap_or(32);
            if &busid[..len] != BUS_ID.as_bytes() {
                (&stream).write_all(&op_reply(0x0003, 1))?;
                return Ok(());
            }
            let mut b = op_reply(0x0003, 0);
            b.extend_from_slice(&describe().encode());
            (&stream).write_all(&b)?;
        }
        _ => {
            return Err(io::Error::other(format!(
                "unknown USB/IP request {code:#x}"
            )));
        }
    }
    shared.link.lock().unwrap().imported = true;
    tracing::debug!("USB/IP client attached the device");
    loop {
        let mut h = [0u8; 48];
        read(&mut h, None)?;
        let (command, seqnum, direction, endpoint) =
            (be32(&h, 0), be32(&h, 4), be32(&h, 12), be32(&h, 16));
        if direction > 1 || endpoint > 15 {
            return Err(io::Error::other("invalid USB/IP endpoint or direction"));
        }
        match command {
            CMD_SUBMIT => {
                let length = be32(&h, 24) as i32;
                let packets = be32(&h, 32) as i32;
                if !(-1..=1024).contains(&packets) {
                    return Err(io::Error::other("invalid USB/IP packet count"));
                }
                let length = usize::try_from(length)
                    .ok()
                    .filter(|n| *n <= MAX_TRANSFER)
                    .ok_or_else(|| io::Error::other("USB/IP transfer too large"))?;
                let mut data = vec![];
                if direction == 0 {
                    data.resize(length, 0);
                    read(&mut data, None)?;
                }
                if packets > 0 {
                    let mut wire = vec![0; 16 * packets as usize];
                    read(&mut wire, None)?;
                    let mut descriptors = Vec::new();
                    let mut end = 0;
                    for p in wire.as_chunks::<16>().0 {
                        let offset = be32(p, 0) as usize;
                        let size = be32(p, 4) as usize;
                        if offset < end || offset > length || size > length - offset {
                            return Err(io::Error::other("invalid USB/IP isochronous offsets"));
                        }
                        end = offset + size;
                        descriptors.push(IsoPacket {
                            offset,
                            length: size,
                        });
                    }
                    let layout = shared
                        .device
                        .lock()
                        .unwrap()
                        .iso_layout(endpoint as u8, direction == 1);
                    let valid = layout.is_some_and(|(max, align)| {
                        align != 0
                            && descriptors
                                .iter()
                                .all(|p| p.length <= max && p.length.is_multiple_of(align))
                    });
                    let mut link = shared.link.lock().unwrap();
                    let now = Instant::now();
                    let cursor = &mut link.iso_next[endpoint as usize];
                    let start = cursor
                        .filter(|last| {
                            now.saturating_duration_since(*last) <= Duration::from_millis(50)
                        })
                        .unwrap_or(now);
                    let due = start + Duration::from_millis(packets as u64);
                    let transfer = IsoTransfer {
                        seqnum,
                        endpoint: endpoint as u8,
                        input: direction == 1,
                        length,
                        packets: descriptors,
                        data,
                        due,
                        start_frame: due.duration_since(shared.epoch).as_millis() as u32,
                    };
                    if !valid || link.iso.len() >= 64 {
                        link.send(&transfer.reply(if valid { -12 } else { EPIPE }, &[], &[]));
                    } else {
                        link.iso_next[endpoint as usize] = Some(due);
                        link.iso.push_back(transfer);
                        shared.wake.notify_all();
                    }
                    continue;
                }
                let reply = if endpoint == 0 {
                    let setup = Setup::parse(h[40..48].try_into().unwrap());
                    let answer = if u32::from(setup.request_type >> 7) != direction
                        || usize::from(setup.length) > length
                    {
                        None
                    } else {
                        shared.device.lock().unwrap().control(&setup, &data)
                    };
                    if answer.is_some() {
                        let reset = (setup.request_type == 0 && setup.request == 9)
                            || (setup.request_type == 0x23
                                && setup.request == 3
                                && setup.value == 4);
                        let stop_audio = setup.request_type == 1
                            && setup.request == 11
                            && setup.value == 0
                            && matches!(setup.index, 1 | 2);
                        if reset || stop_audio {
                            let mut link = shared.link.lock().unwrap();
                            cancel_iso(&mut link, |t| {
                                reset || u16::from(t.endpoint) == setup.index
                            });
                            if reset {
                                let mut pending: Vec<_> = link.pending.drain(..).collect();
                                pending.append(&mut link.held);
                                for t in pending {
                                    link.send(&ret(RET_SUBMIT, t.seqnum, ECONNRESET, None, 0));
                                }
                                link.iso_next = [None; 16];
                            } else {
                                link.iso_next[setup.index as usize] = None;
                            }
                        }
                    }
                    match answer {
                        Some(mut answer) if direction == 1 => {
                            answer.truncate(length.min(usize::from(setup.length)));
                            ret(RET_SUBMIT, seqnum, 0, Some(&answer), answer.len())
                        }
                        Some(_) => ret(RET_SUBMIT, seqnum, 0, None, length),
                        None => ret(RET_SUBMIT, seqnum, EPIPE, None, 0),
                    }
                } else if direction == 1 {
                    let mut link = shared.link.lock().unwrap();
                    if link.pending.len() + link.held.len() >= 64 {
                        link.send(&ret(RET_SUBMIT, seqnum, -12, None, 0));
                        continue;
                    }
                    link.pending.push_back(Transfer {
                        seqnum,
                        endpoint: endpoint as u8,
                        length,
                    });
                    drop(link);
                    shared.wake.notify_all();
                    continue;
                } else {
                    let accepted = shared.device.lock().unwrap().output(endpoint as u8, &data);
                    ret(
                        RET_SUBMIT,
                        seqnum,
                        if accepted { 0 } else { EPIPE },
                        None,
                        if accepted { length } else { 0 },
                    )
                };
                shared.link.lock().unwrap().send(&reply);
            }
            CMD_UNLINK => {
                let victim = be32(&h, 20);
                let mut link = shared.link.lock().unwrap();
                let before = link.pending.len() + link.held.len() + link.iso.len();
                link.pending.retain(|t| t.seqnum != victim);
                link.held.retain(|t| t.seqnum != victim);
                link.iso.retain(|t| t.seqnum != victim);
                for endpoint in 0..16 {
                    link.iso_next[endpoint] = link
                        .iso
                        .iter()
                        .rev()
                        .find(|t| usize::from(t.endpoint) == endpoint)
                        .map(|t| t.due);
                }
                // A transfer already completed is not cancelled: status 0.
                let found = link.pending.len() + link.held.len() + link.iso.len() != before;
                let status = if found { ECONNRESET } else { 0 };
                link.send(&ret(RET_UNLINK, seqnum, status, None, 0));
            }
            _ => {
                return Err(io::Error::other(format!(
                    "unknown USB/IP command {command}"
                )));
            }
        }
    }
}

fn cancel_iso(link: &mut Link, matches: impl Fn(&IsoTransfer) -> bool) {
    let mut i = 0;
    while i < link.iso.len() {
        if matches(&link.iso[i]) {
            let transfer = link.iso.remove(i).unwrap();
            link.send(&transfer.reply(ECONNRESET, &[], &[]));
        } else {
            i += 1;
        }
    }
}

/// Completes interrupt transfers: one per endpoint each period, and at once
/// when the device changed.
fn report<D: Device>(shared: &Shared<D>) {
    let mut next = Instant::now();
    let mut link = shared.link.lock().unwrap();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let waiting = link.imported && (!link.pending.is_empty() || !link.iso.is_empty());
        let now = Instant::now();
        if !waiting {
            link = shared.wake.wait(link).unwrap();
            continue;
        }
        let due = link
            .iso
            .iter()
            .map(|t| t.due)
            .chain((!link.pending.is_empty()).then_some(if link.changed { now } else { next }))
            .min()
            .unwrap_or(next);
        if now < due {
            link = shared.wake.wait_timeout(link, due - now).unwrap().0;
            continue;
        }
        let mut device = shared.device.lock().unwrap();
        let mut iso_index = 0;
        while iso_index < link.iso.len() {
            if link.iso[iso_index].due > now {
                iso_index += 1;
                continue;
            }
            let transfer = link.iso.remove(iso_index).unwrap();
            // A delayed worker completes old USB requests but must not play
            // their audio at the current time or join it to a fresh PCM block.
            if !transfer.input && now.saturating_duration_since(transfer.due) > shared.late {
                device.iso_discontinuity();
                link.send(&transfer.reply(0, &[], &[]));
                continue;
            }
            let mut data = vec![];
            let mut lengths = vec![];
            let mut status = 0;
            for packet in &transfer.packets {
                let out = if transfer.input {
                    &[][..]
                } else {
                    &transfer.data[packet.offset..packet.offset + packet.length]
                };
                match device.iso(transfer.endpoint, transfer.input, out, packet.length) {
                    Some(bytes) if bytes.len() <= packet.length => {
                        lengths.push(bytes.len());
                        data.extend_from_slice(&bytes);
                    }
                    _ => {
                        status = EPIPE;
                        data.clear();
                        break;
                    }
                }
            }
            link.send(&transfer.reply(status, &data, &lengths));
        }
        let mut served = 0u32;
        let mut sent = false;
        let mut i = 0;
        while (link.changed || now >= next) && i < link.pending.len() {
            let transfer = link.pending[i];
            let bit = 1u32 << (transfer.endpoint & 31);
            if served & bit != 0 {
                i += 1;
                continue;
            }
            served |= bit;
            link.pending.remove(i);
            match device.interrupt(transfer.endpoint) {
                Some(mut data) => {
                    data.truncate(transfer.length);
                    let reply = ret(RET_SUBMIT, transfer.seqnum, 0, Some(&data), data.len());
                    link.send(&reply);
                    sent = true;
                }
                None => link.held.push(transfer),
            }
        }
        drop(device);
        if sent {
            link.changed = false;
            next = now + shared.period;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steam_deck::{REPORT_PERIOD, SteamDeck};

    #[test]
    fn dropping_an_idle_export_does_not_need_a_loopback_connection() {
        for idle in [Duration::ZERO, READ_SLICE * 2] {
            let mut export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
            // An unreachable advertised port makes the old self-connect wake-up
            // fail on unrestricted machines too. Shutdown must use no connection.
            export.port = 0;
            std::thread::sleep(idle);
            let (done, finished) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                drop(export);
                done.send(()).unwrap();
            });
            finished.recv_timeout(Duration::from_secs(2)).unwrap();
            worker.join().unwrap();
        }
    }

    #[test]
    fn composite_audio_round_trips_packet_descriptors_and_actuator_pcm() {
        let mut pad = crate::dualsense::DualSense::new(0, 3);
        pad.capabilities(crate::haptics::CAPABILITY);
        // A loaded machine can wake the report thread more than `MAX_AGE` late, and `listen`
        // then drops the audio as stale. This test is about the descriptors and the PCM.
        let export = Export::listen_with(pad, REPORT_PERIOD, Duration::from_secs(60)).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = op_reply(OP_REQ_IMPORT, 0);
        request.extend_from_slice(b"1-1");
        request.resize(40, 0);
        client.write_all(&request).unwrap();
        let mut imported = [0; 320];
        client.read_exact(&mut imported).unwrap();
        assert_eq!(be32(&imported, 8 + 296), 3, "high speed");
        assert_eq!(&imported[8 + 300..8 + 304], &[5, 0x4c, 0x0c, 0xe6]);
        // Five 1 ms packets with deliberate padding between packets. USB/IP
        // offsets describe the transfer buffer, not concatenated audio.
        let packet_size = 384;
        let stride = 392;
        let length = stride * 5;
        let mut wire = vec![];
        for n in [CMD_SUBMIT, 7, 0x0001_0002, 0, 1, 0, length, 0, 5, 1] {
            wire.extend_from_slice(&n.to_be_bytes());
        }
        wire.extend_from_slice(&[0; 8]);
        let mut audio = vec![0xcc; length as usize];
        for packet in 0..5 {
            for frame in audio[packet * stride as usize..packet * stride as usize + packet_size]
                .as_chunks_mut::<8>()
                .0
            {
                frame.copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 1, 0x80, 2, 0x7f]);
            }
        }
        wire.extend_from_slice(&audio);
        for p in 0..5 {
            for n in [p * stride, packet_size as u32, 0, 0] {
                wire.extend_from_slice(&n.to_be_bytes());
            }
        }
        // The export stamps the PCM after this; draining as of now keeps a
        // loaded test machine's delay out of the queue's 20 ms freshness limit.
        let submitted = std::time::Instant::now();
        client.write_all(&wire).unwrap();
        let mut reply = [0; 48 + 5 * 16];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply, 0), RET_SUBMIT);
        assert_eq!(be32(&reply, 4), 7);
        assert_eq!(be32(&reply, 20), 0);
        assert_eq!(be32(&reply, 24), length);
        assert_eq!(be32(&reply, 32), 5);
        for (i, descriptor) in reply[48..].as_chunks::<16>().0.iter().enumerate() {
            assert_eq!(be32(descriptor, 0), i as u32 * stride);
            assert_eq!(be32(descriptor, 8), packet_size as u32);
            assert_eq!(be32(descriptor, 12), 0);
        }
        let frames = export.with(|pad| pad.take_pcm_at(submitted));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].samples, [1, 0x80, 2, 0x7f].repeat(240));
        // The next HID control request still parses after isochronous metadata.
        submit(
            &mut client,
            8,
            0,
            true,
            3,
            [0xa1, 1, 0x85, 3, 3, 0, 3, 0],
            &[],
        );
        assert_eq!(
            self::reply(&mut client),
            (RET_SUBMIT, 8, 0, vec![0x85, 0, 0])
        );
    }

    fn import(port: u16) -> TcpStream {
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = op_reply(OP_REQ_IMPORT, 0);
        let mut busid = [0u8; 32];
        busid[..3].copy_from_slice(b"1-1");
        request.extend_from_slice(&busid);
        client.write_all(&request).unwrap();
        let mut reply = [0u8; 8 + 312];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(&reply[..8], &op_reply(0x0003, 0)[..]);
        // idVendor and idProduct, big-endian, after path, busid and three u32s.
        assert_eq!(&reply[8 + 300..8 + 304], &[0x28, 0xde, 0x12, 0x05]);
        assert_eq!(reply[8 + 311], 3, "three interfaces");
        client
    }
    fn submit(
        client: &mut TcpStream,
        seqnum: u32,
        endpoint: u32,
        inward: bool,
        length: u32,
        setup: [u8; 8],
        data: &[u8],
    ) {
        let mut b = vec![];
        for n in [
            CMD_SUBMIT,
            seqnum,
            0x0001_0002,
            u32::from(inward),
            endpoint,
            0,
            length,
            0,
            0,
            0,
        ] {
            b.extend_from_slice(&n.to_be_bytes());
        }
        b.extend_from_slice(&setup);
        b.extend_from_slice(data);
        client.write_all(&b).unwrap();
    }
    /// A reply's command, seqnum, status and data.
    fn reply(client: &mut TcpStream) -> (u32, u32, i32, Vec<u8>) {
        let mut h = [0u8; 48];
        client.read_exact(&mut h).unwrap();
        let command = be32(&h, 0);
        let length = if command == RET_SUBMIT {
            be32(&h, 24) as usize
        } else {
            0
        };
        let mut data = vec![0; length];
        // Only IN replies carry data; these tests only ask for IN data.
        client.read_exact(&mut data).unwrap();
        (command, be32(&h, 4), be32(&h, 20) as i32, data)
    }

    #[test]
    fn a_client_imports_the_deck_reads_its_descriptors_and_gets_reports() {
        let export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
        let mut client = import(export.port());
        // GET_DESCRIPTOR(device), 18 bytes.
        submit(
            &mut client,
            1,
            0,
            true,
            18,
            [0x80, 6, 0, 1, 0, 0, 18, 0],
            &[],
        );
        let (command, seqnum, status, data) = reply(&mut client);
        assert_eq!((command, seqnum, status), (RET_SUBMIT, 1, 0));
        assert_eq!(&data[8..12], &[0xde, 0x28, 0x05, 0x12]);
        // A device qualifier stalls.
        submit(
            &mut client,
            2,
            0,
            true,
            10,
            [0x80, 6, 0, 6, 0, 0, 10, 0],
            &[],
        );
        assert_eq!(reply(&mut client).2, EPIPE);
        // SET_REPORT carries data out and returns none.
        let mut ask = [0u8; 64];
        ask[0] = 0xae;
        submit(
            &mut client,
            3,
            0,
            false,
            64,
            [0x21, 9, 0, 3, 2, 0, 64, 0],
            &ask,
        );
        let mut h = [0u8; 48];
        client.read_exact(&mut h).unwrap();
        assert_eq!((be32(&h, 4), be32(&h, 20), be32(&h, 24)), (3, 0, 64));
        assert!(export.imported());

        // The mouse endpoint never reports; the controller reports on its own.
        submit(&mut client, 4, 1, true, 8, [0; 8], &[]);
        submit(&mut client, 5, 3, true, 64, [0; 8], &[]);
        let (_, seqnum, status, data) = reply(&mut client);
        assert_eq!((seqnum, status, data.len()), (5, 0, 64));
        assert_eq!(&data[..4], &[1, 0, 9, 64]);

        // A change goes out at once.
        submit(&mut client, 6, 3, true, 64, [0; 8], &[]);
        let start = Instant::now();
        export.with(|deck| {
            deck.state.apply(&crate::input::Input::Controller {
                id: 0,
                active: 1,
                buttons: 0x1000,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4],
            })
        });
        export.changed();
        let (_, seqnum, _, data) = reply(&mut client);
        assert_eq!(seqnum, 6);
        assert!(start.elapsed() < Duration::from_secs(1));
        // A periodic report may have raced the change; the next has it.
        let pressed = |data: &[u8]| data[8] & 0x80 != 0;
        if !pressed(&data) {
            submit(&mut client, 7, 3, true, 64, [0; 8], &[]);
            assert!(pressed(&reply(&mut client).3));
        }

        // Unlinking the held mouse transfer cancels it; an unknown one is gone already.
        let mut unlink = vec![];
        for n in [CMD_UNLINK, 8, 0x0001_0002, 0, 1, 4] {
            unlink.extend_from_slice(&n.to_be_bytes());
        }
        unlink.resize(48, 0);
        client.write_all(&unlink).unwrap();
        assert_eq!(reply(&mut client), (RET_UNLINK, 8, ECONNRESET, vec![]));
        unlink[20..24].copy_from_slice(&99u32.to_be_bytes());
        client.write_all(&unlink).unwrap();
        assert_eq!(reply(&mut client).2, 0);

        // Dropping the export disconnects the client promptly, though the
        // client keeps its end open.
        let start = Instant::now();
        drop(export);
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut rest = vec![];
        match client.read_to_end(&mut rest) {
            Ok(_) => assert!(rest.is_empty()),
            Err(error) => assert!(!matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            )),
        }
    }

    #[test]
    fn the_device_can_be_listed_and_attached_again_after_a_disconnect() {
        let export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
        let mut list = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
        list.write_all(&op_reply(OP_REQ_DEVLIST, 0)).unwrap();
        let mut reply = vec![];
        list.read_to_end(&mut reply).unwrap();
        assert_eq!(reply.len(), 8 + 4 + 312 + 3 * 4);
        assert_eq!(&reply[8..12], &1u32.to_be_bytes());

        let first = import(export.port());
        drop(first);
        let mut second = import(export.port());
        submit(&mut second, 1, 3, true, 64, [0; 8], &[]);
        assert_eq!(reply_len(&mut second), 64);

        // A wrong bus ID is refused.
        let mut wrong = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
        let mut request = op_reply(OP_REQ_IMPORT, 0);
        request.extend_from_slice(&[b'9'; 32]);
        wrong.write_all(&request).unwrap();
    }
    fn reply_len(client: &mut TcpStream) -> usize {
        reply(client).3.len()
    }
}
