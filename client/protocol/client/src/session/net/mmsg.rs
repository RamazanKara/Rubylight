//! Linux and Android `recvmmsg` batching, `SO_TIMESTAMPNS` arrival timestamps and `UDP_GRO`
//! socket option control.

use std::io;
use std::net::UdpSocket;
use std::os::fd::AsRawFd;

use super::batch::{cmsg_int, cmsg_timestamp_ns, RECEIVE_BATCH, SOL_SOCKET, SO_TIMESTAMPNS};

/// Control bytes per header: room for a UDP_GRO message (a `cmsghdr` and an int, 24 bytes on
/// 64-bit Linux) and a receive time (a `cmsghdr` and a `timespec`, 32 bytes).
const CONTROL: usize = 128;

/// `recvmmsg` headers, one per slot of a buffer they point into, each with its control
/// messages.
pub struct Batch {
    headers: Box<[libc::mmsghdr]>,
    _iovecs: Box<[libc::iovec]>,
    controls: Box<[u8]>,
    /// The socket `SO_TIMESTAMPNS` was set on, or -1.
    stamped: std::os::fd::RawFd,
    /// The kernel refused `SO_TIMESTAMPNS`: datagrams come without a receive time.
    stamps_refused: bool,
}

#[cfg(test)]
thread_local! {
    /// Makes this thread's `recvmmsg` calls fail with `ENOSYS`, as a kernel without it.
    pub static REFUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn refused() -> io::Result<()> {
    if REFUSED.get() {
        return Err(io::Error::from_raw_os_error(libc::ENOSYS));
    }
    Ok(())
}

impl Batch {
    /// The headers for `buffer`'s slots of `slot` bytes. They hold pointers into it: the
    /// caller keeps it alive and its allocation in place while they are used.
    pub fn new(buffer: &mut [u8], slot: usize) -> Self {
        let mut iovecs: Box<[libc::iovec]> = buffer
            .chunks_exact_mut(slot)
            .take(RECEIVE_BATCH)
            .map(|slot| libc::iovec {
                iov_base: slot.as_mut_ptr().cast(),
                iov_len: slot.len(),
            })
            .collect();
        let mut controls = vec![0u8; CONTROL * iovecs.len()].into_boxed_slice();
        let control_base = controls.as_mut_ptr();
        let headers = iovecs
            .iter_mut()
            .enumerate()
            .map(|(index, iovec)| {
                // SAFETY: mmsghdr is plain data; all zero is an empty header.
                let mut header: libc::mmsghdr = unsafe { std::mem::zeroed() };
                header.msg_hdr.msg_iov = iovec;
                header.msg_hdr.msg_iovlen = 1;
                header.msg_hdr.msg_control = control_base.wrapping_add(index * CONTROL).cast();
                header.msg_hdr.msg_controllen = CONTROL as _;
                header
            })
            .collect();
        Self {
            headers,
            _iovecs: iovecs,
            controls,
            stamped: -1,
            stamps_refused: false,
        }
    }

    /// Up to a batch: blocks for the first datagram only (`MSG_WAITFORONE`). `MSG_TRUNC`
    /// reports a cut datagram's length on the wire.
    pub fn receive(&mut self, socket: &UdpSocket) -> io::Result<usize> {
        #[cfg(test)]
        refused()?;
        self.ask_for_stamps(socket);
        for header in self.headers.iter_mut() {
            // The kernel leaves the length of the control messages it wrote; each call
            // needs the whole control buffer again.
            header.msg_hdr.msg_controllen = CONTROL as _;
        }
        // SAFETY: the headers point at live iovecs, slots and control buffers of their lengths.
        let count = unsafe {
            libc::recvmmsg(
                socket.as_raw_fd(),
                self.headers.as_mut_ptr(),
                self.headers.len() as libc::c_uint,
                libc::MSG_WAITFORONE | libc::MSG_TRUNC,
                std::ptr::null_mut(),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }

    pub fn length(&self, index: usize) -> usize {
        self.headers[index].msg_len as usize
    }

    /// Has the kernel stamp each datagram's arrival (`SO_TIMESTAMPNS`) on `socket`, once
    /// per socket (the roam's new one included). A refusal is final: the datagrams then
    /// come without a stamp.
    fn ask_for_stamps(&mut self, socket: &UdpSocket) {
        let fd = socket.as_raw_fd();
        if self.stamps_refused || self.stamped == fd {
            return;
        }
        let value = libc::c_int::from(true);
        // SAFETY: the option is one int, read for its size.
        let result = unsafe {
            libc::setsockopt(
                fd,
                SOL_SOCKET,
                SO_TIMESTAMPNS,
                (&value as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if result < 0 {
            self.stamps_refused = true;
        } else {
            self.stamped = fd;
        }
    }

    /// Asks again at the next receive, as for a socket not seen before: a batch came with
    /// no stamps (a closed socket's descriptor number may be a new socket's).
    pub fn forget_stamps(&mut self) {
        self.stamped = -1;
    }

    /// The kernel's receive time of datagram `index`, nanoseconds on `CLOCK_REALTIME`, from
    /// its control messages. A UDP_GRO message has one for all its datagrams.
    pub fn stamp_ns(&self, index: usize) -> Option<u64> {
        // `msg_controllen` is `usize` on glibc and Android but `socklen_t` elsewhere.
        #[allow(clippy::unnecessary_cast)]
        let used = (self.headers[index].msg_hdr.msg_controllen as usize).min(CONTROL);
        let start = index * CONTROL;
        cmsg_timestamp_ns(&self.controls[start..start + used])
    }

    /// `CLOCK_REALTIME` now, nanoseconds.
    pub fn realtime_now_ns() -> Option<u64> {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `now` is a valid timespec to write.
        if unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) } < 0 || now.tv_sec < 0 {
            return None;
        }
        Some(now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64)
    }

    /// The segment size of datagram `index` when UDP_GRO coalesced it, from its control
    /// messages.
    pub fn gro(&self, index: usize) -> Option<usize> {
        // `msg_controllen` is `usize` on glibc and Android but `socklen_t` elsewhere.
        #[allow(clippy::unnecessary_cast)]
        let used = (self.headers[index].msg_hdr.msg_controllen as usize).min(CONTROL);
        let start = index * CONTROL;
        cmsg_int(&self.controls[start..start + used], gro::SOL_UDP, gro::UDP_GRO)
    }
}

/// Whether the kernel can take batches: `recvmmsg` of no datagrams returns at once, or
/// fails where it is missing (before Linux 2.6.33) or filtered out.
pub fn probe(socket: &UdpSocket) -> io::Result<()> {
    #[cfg(test)]
    refused()?;
    // SAFETY: no headers are read or written with a length of 0.
    let result = unsafe {
        libc::recvmmsg(
            socket.as_raw_fd(),
            std::ptr::null_mut(),
            0,
            libc::MSG_DONTWAIT,
            std::ptr::null_mut(),
        )
    };
    if result < 0 {
        let e = io::Error::last_os_error();
        // Any other error is a pending socket error (an ICMP report): batches work.
        if unsupported(&e) {
            return Err(e);
        }
    }
    Ok(())
}

/// An error that means `recvmmsg` itself does not work here.
pub fn unsupported(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOSYS | libc::EPERM | libc::EOPNOTSUPP | libc::EINVAL)
    )
}

/// One datagram; its length on the wire, even when `buffer` cut it (`MSG_TRUNC`).
pub fn receive_one(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `buffer` is valid for its length.
    let length = unsafe {
        libc::recv(
            socket.as_raw_fd(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            libc::MSG_TRUNC,
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(length as usize)
}

/// UDP_GRO, which has the kernel coalesce consecutive datagrams of one sender into one read, with
/// their segment size in a control message, so one batch takes more datagrams per call.
pub mod gro {
    use std::io;
    use std::net::UdpSocket;
    use std::os::fd::AsRawFd;

    /// `SOL_UDP` and `UDP_GRO`, as the kernel defines them.
    pub const SOL_UDP: libc::c_int = 17;
    pub const UDP_GRO: libc::c_int = 104;
    /// Bytes per slot while UDP_GRO is on: the largest read a coalesced message can be.
    pub const SLOT: usize = 65536;

    /// Turns UDP_GRO on or off for `socket`.
    pub fn set(socket: &UdpSocket, on: bool) -> io::Result<()> {
        let value = libc::c_int::from(on);
        // SAFETY: the option is one int, read for its size.
        let result = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                SOL_UDP,
                UDP_GRO,
                (&value as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
