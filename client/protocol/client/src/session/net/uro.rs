//! UDP receive coalescing on Windows (`WSARecvMsg` + `UDP_RECV_MAX_COALESCED_SIZE`).

use std::io;
use std::mem::{size_of, size_of_val};
use std::net::UdpSocket;
use std::os::windows::io::AsRawSocket;
use std::ptr::null_mut;

use windows_sys::core::GUID;
use windows_sys::Win32::Networking::WinSock::{
    setsockopt, WSAGetLastError, WSAIoctl, CMSGHDR, IPPROTO_UDP, LPFN_WSARECVMSG,
    SIO_GET_EXTENSION_FUNCTION_POINTER, SOCKET, SOCKET_ERROR, UDP_COALESCED_INFO,
    UDP_RECV_MAX_COALESCED_SIZE, WSABUF, WSAID_WSARECVMSG, WSAMSG,
};

use super::batch::{coalesced_segment, RECEIVE_BATCH};

/// The most a coalesced read holds: the largest UDP payload over IPv4, as msquic sets it.
pub const MAX_COALESCED: usize = 65527;

// The layout `coalesced_segment` reads.
const _: () = assert!(
    IPPROTO_UDP == 17
        && UDP_COALESCED_INFO == 3
        && size_of::<CMSGHDR>() == size_of::<usize>() + 8
);

#[cfg(test)]
thread_local! {
    /// Makes this thread's sockets refuse coalescing, as a system without it.
    pub static REFUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn last_error() -> io::Error {
    // SAFETY: no preconditions.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}

fn raw(socket: &UdpSocket) -> SOCKET {
    socket.as_raw_socket() as SOCKET
}

/// A socket's `WSARecvMsg` and the coalescing turned on, for the receiver's socket and
/// the roam's replacements.
pub struct Coalesce {
    recv_msg: LPFN_WSARECVMSG,
    /// The socket the reads are on.
    socket: SOCKET,
    /// The option's value: a call's worth of the reader's datagrams.
    size: u32,
    /// Whether coalescing is on: until [`Coalesce::turn_off`], which also keeps it off on a
    /// roam's new socket.
    on: bool,
    /// The control messages of a read: one `UDP_COALESCED_INFO` takes 24 bytes.
    control: [u64; 16],
}

impl Coalesce {
    /// Coalescing on `socket` up to [`RECEIVE_BATCH`] datagrams of `max_datagram` bytes
    /// (more of smaller ones, which the receiver takes over several receives), or why the
    /// system refused it.
    pub fn new(socket: &UdpSocket, max_datagram: usize) -> io::Result<Self> {
        let size = (max_datagram.max(1) * RECEIVE_BATCH).min(MAX_COALESCED) as u32;
        let recv_msg = enable(raw(socket), Some(size))?;
        Ok(Self {
            recv_msg,
            socket: raw(socket),
            size,
            on: true,
            control: [0; 16],
        })
    }

    /// Whether coalescing is on.
    pub fn on(&self) -> bool {
        self.on
    }

    /// Moves the reads to `socket` when it is not the one they are on (a roam's new
    /// socket), with coalescing on there while it is on, or says why the system refused it.
    pub fn follow(&mut self, socket: &UdpSocket) -> io::Result<()> {
        if raw(socket) != self.socket {
            self.recv_msg = enable(raw(socket), self.on.then_some(self.size))?;
            self.socket = raw(socket);
        }
        Ok(())
    }

    /// Turns coalescing off on the socket and for the roam's next ones. The reads go on
    /// through `WSARecvMsg`: those coalesced before still come with their size.
    pub fn turn_off(&mut self) -> io::Result<()> {
        self.on = false;
        set_size(self.socket, 0)
    }

    /// One read into `buffer`: its bytes and the datagram size of a coalesced read. The
    /// socket's poll timeout applies as to `recv`. The sender's address is not asked for:
    /// as with `recv` and `recvmmsg`, the unconnected socket takes any.
    pub fn receive(
        &mut self,
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> io::Result<(usize, Option<usize>)> {
        let Some(recv_msg) = self.recv_msg else {
            return Err(io::ErrorKind::Unsupported.into());
        };
        let mut data = WSABUF {
            len: buffer.len().min(u32::MAX as usize) as u32,
            buf: buffer.as_mut_ptr(),
        };
        let mut message = WSAMSG {
            name: null_mut(),
            namelen: 0,
            lpBuffers: &mut data,
            dwBufferCount: 1,
            Control: WSABUF {
                len: size_of_val(&self.control) as u32,
                buf: self.control.as_mut_ptr().cast(),
            },
            dwFlags: 0,
        };
        let mut received = 0u32;
        // SAFETY: the message points at `buffer` and `control` for their lengths, both
        // live across the call; no overlapped I/O.
        let result =
            unsafe { recv_msg(raw(socket), &mut message, &mut received, null_mut(), None) };
        if result == SOCKET_ERROR {
            return Err(last_error());
        }
        // On return Control.len is the length of the control messages.
        let length = (message.Control.len as usize).min(size_of_val(&self.control));
        // SAFETY: plain integers seen as their bytes, within the array.
        let control =
            unsafe { std::slice::from_raw_parts(self.control.as_ptr().cast::<u8>(), length) };
        Ok((received as usize, coalesced_segment(control)))
    }
}

/// `socket`'s `WSARecvMsg`, then coalescing up to `size` bytes on it when that is Some: in
/// this order, so a socket without the function never coalesces.
fn enable(socket: SOCKET, size: Option<u32>) -> io::Result<LPFN_WSARECVMSG> {
    #[cfg(test)]
    if REFUSED.get() {
        return Err(io::Error::from_raw_os_error(
            windows_sys::Win32::Networking::WinSock::WSAENOPROTOOPT,
        ));
    }
    let mut recv_msg: LPFN_WSARECVMSG = None;
    let mut returned = 0u32;
    // SAFETY: the in and out buffers are valid for the sizes given; no overlapped I/O.
    let result = unsafe {
        WSAIoctl(
            socket,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            (&WSAID_WSARECVMSG as *const GUID).cast(),
            size_of::<GUID>() as u32,
            (&mut recv_msg as *mut LPFN_WSARECVMSG).cast(),
            size_of::<LPFN_WSARECVMSG>() as u32,
            &mut returned,
            null_mut(),
            None,
        )
    };
    if result == SOCKET_ERROR {
        return Err(last_error());
    }
    if recv_msg.is_none() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "no WSARecvMsg"));
    }
    if let Some(size) = size {
        set_size(socket, size)?;
    }
    Ok(recv_msg)
}

/// The coalesced reads' maximum on `socket`: 0 turns coalescing off.
fn set_size(socket: SOCKET, size: u32) -> io::Result<()> {
    // SAFETY: a DWORD option of its size.
    let result = unsafe {
        setsockopt(
            socket,
            IPPROTO_UDP,
            UDP_RECV_MAX_COALESCED_SIZE,
            (&size as *const u32).cast(),
            size_of::<u32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        return Err(last_error());
    }
    Ok(())
}

/// The socket's coalesced reads' maximum, 0 while coalescing is off.
#[cfg(test)]
pub fn size_on(socket: SOCKET) -> io::Result<u32> {
    use windows_sys::Win32::Networking::WinSock::getsockopt;
    let mut size = 0u32;
    let mut length = size_of::<u32>() as i32;
    // SAFETY: a DWORD option into a DWORD of its size.
    let result = unsafe {
        getsockopt(
            socket,
            IPPROTO_UDP,
            UDP_RECV_MAX_COALESCED_SIZE,
            (&mut size as *mut u32).cast(),
            &mut length,
        )
    };
    if result == SOCKET_ERROR {
        return Err(last_error());
    }
    Ok(size)
}

#[cfg(test)]
pub fn socket_of(coalesce: &Coalesce) -> SOCKET {
    coalesce.socket
}
