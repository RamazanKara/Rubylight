//! Registered I/O (Windows 8), measured for the video socket and not taken: receives posted
//! ahead of time into one registered buffer, their completions dequeued in batches without a
//! system call, and an event wait only when none is there. Against one `recv` per datagram it
//! cuts the receive thread's CPU per packet by about 40% at PyroWave's rates and hands each
//! datagram over no later (`receive_bench`). But a socket made for it refuses `send`, `recv`
//! and `try_clone` (`a_registered_socket_refuses_plain_calls`), and the video socket is cloned
//! for the receive thread and sends the pings, which would have to go through `RIOSend` on
//! the receive thread's queue. The posted receives are the socket's buffer: a datagram that
//! finds none is dropped, so there are as many as the receive buffer holds.

use std::ffi::c_void;
use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::os::windows::io::{AsRawSocket, FromRawSocket, RawSocket};
use std::sync::Once;

use socket2::{Domain, SockRef, Socket};

use super::{failure, Failure, RECEIVE_BATCH, UDP_RECV_POLL_TIMEOUT};

type Handle = *mut c_void;

/// Posted receives at least and at most, whatever the receive buffer.
const SLOTS_MIN: usize = 512;
const SLOTS_MAX: usize = 32768;
const RIO_EVENT_COMPLETION: i32 = 1;
const RIO_MSG_DEFER: u32 = 2;
const RIO_MSG_COMMIT_ONLY: u32 = 8;
const RIO_CORRUPT_CQ: u32 = 0xFFFF_FFFF;
const RIO_INVALID_BUFFERID: usize = 0xFFFF_FFFF;
const SIO_GET_MULTIPLE_EXTENSION_FUNCTION_POINTER: u32 = 0xC800_0024;
const WSA_FLAG_OVERLAPPED: u32 = 0x01;
const WSA_FLAG_NO_HANDLE_INHERIT: u32 = 0x80;
const WSA_FLAG_REGISTERED_IO: u32 = 0x100;
const INVALID_SOCKET: usize = usize::MAX;
const SOCK_DGRAM: i32 = 2;
const IPPROTO_UDP: i32 = 17;
const WSAEALREADY: i32 = 10037;
const WSAEMSGSIZE: i32 = 10040;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x102;

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

const WSAID_MULTIPLE_RIO: Guid = Guid(
    0x8509_e081,
    0x96dd,
    0x4005,
    [0xb1, 0x65, 0x9e, 0x2e, 0xe8, 0xc7, 0x9e, 0x3f],
);

#[repr(C)]
struct RioBuf {
    id: Handle,
    offset: u32,
    length: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RioResult {
    status: i32,
    bytes: u32,
    socket_context: u64,
    request_context: u64,
}

/// `RIO_NOTIFICATION_COMPLETION` of the event kind; the union's larger IOCP member pads it.
#[repr(C)]
struct Notification {
    kind: i32,
    event: Handle,
    notify_reset: i32,
    _iocp: [usize; 2],
}

/// `RIO_EXTENSION_FUNCTION_TABLE`; the members this module does not call are opaque.
#[repr(C)]
#[derive(Clone, Copy)]
struct Table {
    size: u32,
    receive: unsafe extern "system" fn(Handle, *const RioBuf, u32, u32, *mut c_void) -> i32,
    receive_ex: usize,
    send: usize,
    send_ex: usize,
    close_cq: unsafe extern "system" fn(Handle),
    create_cq: unsafe extern "system" fn(u32, *const Notification) -> Handle,
    #[allow(clippy::type_complexity)]
    create_rq:
        unsafe extern "system" fn(usize, u32, u32, u32, u32, Handle, Handle, *mut c_void) -> Handle,
    dequeue: unsafe extern "system" fn(Handle, *mut RioResult, u32) -> u32,
    deregister: unsafe extern "system" fn(Handle),
    notify: unsafe extern "system" fn(Handle) -> i32,
    register: unsafe extern "system" fn(*mut u8, u32) -> Handle,
    resize_cq: usize,
    resize_rq: usize,
}

#[link(name = "ws2_32")]
extern "system" {
    fn WSASocketW(
        af: i32,
        ty: i32,
        protocol: i32,
        info: *mut c_void,
        group: u32,
        flags: u32,
    ) -> usize;
    #[allow(clippy::too_many_arguments)]
    fn WSAIoctl(
        socket: usize,
        code: u32,
        input: *const c_void,
        input_length: u32,
        output: *mut c_void,
        output_length: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
        routine: *mut c_void,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateEventW(attributes: *mut c_void, manual: i32, initial: i32, name: *const u16)
        -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
}

/// A UDP socket that Registered I/O can receive on (`WSA_FLAG_REGISTERED_IO`); every other
/// socket call takes it as usual.
pub fn socket(domain: Domain) -> io::Result<Socket> {
    // Any std socket starts Winsock, as socket2 does.
    static WINSOCK: Once = Once::new();
    WINSOCK.call_once(|| drop(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))));
    let flags = WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT | WSA_FLAG_REGISTERED_IO;
    // SAFETY: plain arguments; the handle is checked.
    let raw = unsafe {
        WSASocketW(
            i32::from(domain),
            SOCK_DGRAM,
            IPPROTO_UDP,
            std::ptr::null_mut(),
            0,
            flags,
        )
    };
    if raw == INVALID_SOCKET {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a new socket nothing else owns.
    Ok(unsafe { Socket::from_raw_socket(raw as RawSocket) })
}

/// What a receive queue holds in the kernel: freed only once no posted receive is left.
struct Registered {
    table: Table,
    cq: Handle,
    event: Handle,
    id: Handle,
    buffer: Vec<u8>,
    /// Receives posted and not dequeued yet.
    outstanding: usize,
}

// SAFETY: the handles are process-wide; only the thread that holds it uses it.
unsafe impl Send for Registered {}

impl Registered {
    /// Frees everything, which the kernel no longer writes to.
    fn free(self) {
        // SAFETY: handles this queue created; no receive into the buffer is posted.
        unsafe {
            if !self.cq.is_null() {
                (self.table.close_cq)(self.cq);
            }
            if self.id as usize != RIO_INVALID_BUFFERID {
                (self.table.deregister)(self.id);
            }
            CloseHandle(self.event);
        }
    }

    /// Dequeues the completions of receives still posted until there are none, then frees
    /// everything: a receive stays posted until its socket closes, which may outlast this
    /// thread's handle (the session and the pings hold their own), and the kernel writes
    /// into the buffer until then.
    fn free_once_drained(self) {
        if self.outstanding == 0 {
            return self.free();
        }
        // Without a thread to wait for the socket the buffer stays allocated, which a spawn
        // that fails leaves (the closure is dropped, not run).
        let held = std::mem::ManuallyDrop::new(self);
        let _ = std::thread::Builder::new()
            .name("VideoRioFree".into())
            .spawn(move || std::mem::ManuallyDrop::into_inner(held).drain_and_free());
    }

    fn drain_and_free(mut self) {
        let mut results = [RioResult::default(); RECEIVE_BATCH];
        while self.outstanding > 0 {
            // SAFETY: `results` holds RECEIVE_BATCH entries.
            let count = unsafe {
                (self.table.dequeue)(self.cq, results.as_mut_ptr(), RECEIVE_BATCH as u32)
            };
            if count == RIO_CORRUPT_CQ {
                // The kernel may still write to it: never freed.
                std::mem::forget(self);
                return;
            }
            self.outstanding = self.outstanding.saturating_sub(count as usize);
            if count == 0 {
                // SAFETY: arms the event of a live queue and waits on it.
                unsafe {
                    (self.table.notify)(self.cq);
                    WaitForSingleObject(self.event, 1000);
                }
            }
        }
        self.free();
    }

    /// Dequeues without posting again until no receive is left or `timeout` passed: the
    /// receives still posted.
    fn drain_for(&mut self, timeout: std::time::Duration) -> usize {
        let deadline = std::time::Instant::now() + timeout;
        let mut results = [RioResult::default(); RECEIVE_BATCH];
        while self.outstanding > 0 && std::time::Instant::now() < deadline {
            // SAFETY: `results` holds RECEIVE_BATCH entries.
            let count = unsafe {
                (self.table.dequeue)(self.cq, results.as_mut_ptr(), RECEIVE_BATCH as u32)
            };
            assert_ne!(count, RIO_CORRUPT_CQ);
            self.outstanding -= count as usize;
            if count == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        self.outstanding
    }
}

/// Registered I/O receives on one socket.
pub struct Rio {
    /// None only while dropping.
    registered: Option<Registered>,
    rq: Handle,
    slot: usize,
    results: [RioResult; RECEIVE_BATCH],
    /// Slot and length of each datagram of the last receive, posted again with the next.
    ready: [(usize, usize); RECEIVE_BATCH],
    count: usize,
    /// A failure that came with datagrams, reported by the next receive.
    pending: Option<io::Error>,
}

impl Rio {
    /// Posts receives of `max_datagram` (rounded up) on `socket`, as many as its receive
    /// buffer holds. Fails where the socket lacks `WSA_FLAG_REGISTERED_IO` or the system
    /// Registered I/O.
    pub fn new(socket: &UdpSocket, max_datagram: usize) -> io::Result<Self> {
        let raw = socket.as_raw_socket();
        let slot = max_datagram.max(1).next_multiple_of(64);
        let slots = (SockRef::from(socket).recv_buffer_size()? / slot).clamp(SLOTS_MIN, SLOTS_MAX);
        // SAFETY: every call gets pointers of the sizes it is given; each handle is checked
        // before use and freed with `registered` on failure.
        unsafe {
            let mut table = std::mem::MaybeUninit::<Table>::zeroed();
            let mut returned = 0;
            if WSAIoctl(
                raw as usize,
                SIO_GET_MULTIPLE_EXTENSION_FUNCTION_POINTER,
                (&WSAID_MULTIPLE_RIO as *const Guid).cast(),
                std::mem::size_of::<Guid>() as u32,
                table.as_mut_ptr().cast(),
                std::mem::size_of::<Table>() as u32,
                &mut returned,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            let table = table.assume_init();
            let event = CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null());
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut registered = Registered {
                table,
                cq: std::ptr::null_mut(),
                event,
                id: RIO_INVALID_BUFFERID as Handle,
                buffer: vec![0u8; slot * slots],
                outstanding: 0,
            };
            registered.id = (table.register)(
                registered.buffer.as_mut_ptr(),
                registered.buffer.len() as u32,
            );
            if registered.id as usize == RIO_INVALID_BUFFERID {
                let e = io::Error::last_os_error();
                registered.free();
                return Err(e);
            }
            let notification = Notification {
                kind: RIO_EVENT_COMPLETION,
                event,
                notify_reset: 1,
                _iocp: [0; 2],
            };
            // Room for every receive and the one send the queue must allow.
            registered.cq = (table.create_cq)(slots as u32 + 1, &notification);
            if registered.cq.is_null() {
                let e = io::Error::last_os_error();
                registered.free();
                return Err(e);
            }
            let rq = (table.create_rq)(
                raw as usize,
                slots as u32,
                1,
                1,
                1,
                registered.cq,
                registered.cq,
                std::ptr::null_mut(),
            );
            if rq.is_null() {
                let e = io::Error::last_os_error();
                registered.free();
                return Err(e);
            }
            // From here the queue lives as long as the socket; dropping waits for it.
            let mut rio = Self {
                registered: Some(registered),
                rq,
                slot,
                results: [RioResult::default(); RECEIVE_BATCH],
                ready: [(0, 0); RECEIVE_BATCH],
                count: 0,
                pending: None,
            };
            for index in 0..slots {
                rio.post(index)?;
            }
            rio.commit()?;
            Ok(rio)
        }
    }

    fn registered(&mut self) -> &mut Registered {
        self.registered.as_mut().expect("registered until dropped")
    }

    /// Posts a receive into `index`, deferred to the next commit.
    fn post(&mut self, index: usize) -> io::Result<()> {
        let (rq, slot) = (self.rq, self.slot);
        let registered = self.registered();
        let buffer = RioBuf {
            id: registered.id,
            offset: (index * slot) as u32,
            length: slot as u32,
        };
        // SAFETY: a slot of the registered buffer; the request context is its index.
        if unsafe {
            (registered.table.receive)(rq, &buffer, 1, RIO_MSG_DEFER, index as *mut c_void)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        registered.outstanding += 1;
        Ok(())
    }

    fn commit(&mut self) -> io::Result<()> {
        let rq = self.rq;
        let table = self.registered().table;
        // SAFETY: commits the deferred receives of a live queue.
        if unsafe {
            (table.receive)(
                rq,
                std::ptr::null(),
                0,
                RIO_MSG_COMMIT_ONLY,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits up to the poll timeout for a datagram and takes it with every other one already
    /// there, up to [`RECEIVE_BATCH`]: their count, 0 on the timeout. The slots of the last
    /// receive are posted again first. ICMP port-unreachable reports are skipped.
    pub fn receive(&mut self) -> io::Result<usize> {
        if self.count > 0 {
            for i in 0..std::mem::take(&mut self.count) {
                self.post(self.ready[i].0)?;
            }
            self.commit()?;
        }
        if let Some(e) = self.pending.take() {
            return Err(e);
        }
        loop {
            let (cq, event, table) = {
                let registered = self.registered();
                (registered.cq, registered.event, registered.table)
            };
            // SAFETY: `results` holds RECEIVE_BATCH entries.
            let dequeued =
                unsafe { (table.dequeue)(cq, self.results.as_mut_ptr(), RECEIVE_BATCH as u32) };
            if dequeued == RIO_CORRUPT_CQ {
                return Err(io::Error::other(
                    "the Registered I/O completion queue is corrupt",
                ));
            }
            if dequeued > 0 {
                self.registered().outstanding -= dequeued as usize;
                let mut reposted = false;
                for i in 0..dequeued as usize {
                    let RioResult {
                        status,
                        bytes,
                        request_context,
                        ..
                    } = self.results[i];
                    let index = request_context as usize;
                    match status {
                        // A longer datagram is cut to the slot, as the reader cuts it anyway.
                        0 | WSAEMSGSIZE => {
                            self.ready[self.count] = (index, bytes as usize);
                            self.count += 1;
                        }
                        status => {
                            self.post(index)?;
                            reposted = true;
                            if let Failure::Fatal(e) = failure(io::Error::from_raw_os_error(status))
                            {
                                self.pending.get_or_insert(e);
                            }
                        }
                    }
                }
                if reposted {
                    self.commit()?;
                }
                if self.count > 0 {
                    return Ok(self.count);
                }
                if let Some(e) = self.pending.take() {
                    return Err(e);
                }
                continue;
            }
            // SAFETY: arms the event of a live queue, which fires at once when a completion
            // came meanwhile, then waits on it.
            unsafe {
                let armed = (table.notify)(cq);
                if armed != 0 && armed != WSAEALREADY {
                    return Err(io::Error::from_raw_os_error(armed));
                }
                match WaitForSingleObject(event, UDP_RECV_POLL_TIMEOUT.as_millis() as u32) {
                    WAIT_OBJECT_0 => {}
                    WAIT_TIMEOUT => return Ok(0),
                    _ => return Err(io::Error::last_os_error()),
                }
            }
        }
    }

    /// Datagram `index` of the last receive.
    pub fn datagram(&self, index: usize) -> &[u8] {
        let (slot, length) = self.ready[index];
        let start = slot * self.slot;
        let buffer = &self
            .registered
            .as_ref()
            .expect("registered until dropped")
            .buffer;
        &buffer[start..start + length]
    }

    pub fn length(&self, index: usize) -> usize {
        self.ready[index].1
    }

    /// Receives still posted.
    pub fn outstanding(&self) -> usize {
        self.registered.as_ref().map_or(0, |r| r.outstanding)
    }

    /// [`Registered::drain_for`].
    pub fn drain_for(&mut self, timeout: std::time::Duration) -> usize {
        self.registered().drain_for(timeout)
    }
}

impl Drop for Rio {
    fn drop(&mut self) {
        if let Some(registered) = self.registered.take() {
            registered.free_once_drained();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, UdpSocket};
    use std::time::Duration;

    use super::*;

    /// A registered socket bound to loopback, with room for `buffer` bytes.
    fn registered(buffer: usize) -> UdpSocket {
        let socket = socket(Domain::IPV4).unwrap();
        socket
            .bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())
            .unwrap();
        socket.set_recv_buffer_size(buffer).unwrap();
        socket.into()
    }

    /// Why the video socket cannot be one: it is cloned, and the pings go out of it.
    #[test]
    fn a_registered_socket_refuses_plain_calls() {
        let socket = registered(1 << 20);
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.send_to(b"hello", socket.local_addr().unwrap())
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(socket.recv(&mut [0; 64]).is_err());
        assert!(socket.send_to(b"ping", peer.local_addr().unwrap()).is_err());
        assert!(socket.try_clone().is_err());
    }

    #[test]
    fn registered_receives_come_in_order_and_time_out() {
        let socket = registered(1 << 20);
        let mut rio = Rio::new(&socket, 1500).unwrap();
        assert_eq!(rio.outstanding(), (1 << 20) / 1536);
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        for n in 0..100u8 {
            peer.send_to(&[n; 1392], socket.local_addr().unwrap())
                .unwrap();
        }
        let mut seen = Vec::new();
        while seen.len() < 100 {
            let count = rio.receive().unwrap();
            assert!(count > 0 && count <= RECEIVE_BATCH, "{count}");
            seen.extend((0..count).map(|i| (rio.datagram(i)[0], rio.length(i))));
        }
        assert_eq!(seen, (0..100u8).map(|n| (n, 1392)).collect::<Vec<_>>());
        assert_eq!(rio.receive().unwrap(), 0, "then the poll timeout");
    }

    /// The buffer is freed only once no receive is posted: closing the socket completes them.
    #[test]
    fn closing_the_socket_completes_every_posted_receive() {
        let socket = registered(1 << 20);
        let mut rio = Rio::new(&socket, 1500).unwrap();
        assert!(rio.outstanding() > 0);
        drop(socket);
        assert_eq!(rio.drain_for(Duration::from_secs(10)), 0);
    }
}
