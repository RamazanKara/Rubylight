//! [`Kernel`] on a usbfs file descriptor: Android's `UsbDeviceConnection.getFileDescriptor()`
//! or a `/dev/bus/usb` node opened on Linux.

use std::os::fd::RawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use super::abi::*;
use super::{Errno, Kernel, Ready, Urb, EINTR};

/// A usbfs device, on a descriptor of its own.
pub struct Usbfs {
    fd: AtomicI32,
}

fn errno() -> Errno {
    Errno(
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO),
    )
}

impl Usbfs {
    /// Duplicates `fd`, so the caller may close its own (Java's connection) at once.
    pub fn from_fd(fd: RawFd) -> Result<Self, Errno> {
        let own = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if own < 0 {
            return Err(errno());
        }
        Ok(Self {
            fd: AtomicI32::new(own),
        })
    }

    fn fd(&self) -> RawFd {
        self.fd.load(Ordering::SeqCst)
    }

    /// The ioctl's result, retried on EINTR.
    fn ioctl(&self, request: u32, arg: *mut libc::c_void) -> Result<i32, Errno> {
        loop {
            let result = unsafe { libc::ioctl(self.fd(), request as _, arg) };
            if result >= 0 {
                return Ok(result);
            }
            let e = errno();
            if e != EINTR {
                return Err(e);
            }
        }
    }

    fn with_uint(&self, request: u32, value: u32) -> Result<(), Errno> {
        let mut value = value;
        self.ioctl(request, (&mut value as *mut u32).cast())
            .map(drop)
    }

    fn driver(&self, interface: u8, code: u32) -> Result<(), Errno> {
        let mut command = Ioctl {
            interface: i32::from(interface),
            code: code as i32,
            data: std::ptr::null_mut(),
        };
        self.ioctl(IOCTL, (&mut command as *mut Ioctl).cast())
            .map(drop)
    }
}

impl Kernel for Usbfs {
    fn descriptors(&self) -> Result<Vec<u8>, Errno> {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::pread(
                    self.fd(),
                    chunk.as_mut_ptr().cast(),
                    chunk.len(),
                    raw.len() as libc::off_t,
                )
            };
            match n {
                0 => return Ok(raw),
                n if n > 0 => raw.extend_from_slice(&chunk[..n as usize]),
                _ if errno() == EINTR => {}
                _ => return Err(errno()),
            }
        }
    }

    fn configuration(&self) -> Result<u8, Errno> {
        let mut value = 0u8;
        let mut transfer = CtrlTransfer {
            request_type: 0x80,
            request: 8,
            value: 0,
            index: 0,
            length: 1,
            timeout_ms: 1000,
            data: &mut value,
        };
        match self.ioctl(CONTROL, (&mut transfer as *mut CtrlTransfer).cast())? {
            1 => Ok(value),
            _ => Err(super::EINVAL),
        }
    }

    fn speed(&self) -> Result<u32, Errno> {
        self.ioctl(GET_SPEED, std::ptr::null_mut())
            .map(|speed| speed as u32)
    }

    fn claim(&self, interface: u8) -> Result<(), Errno> {
        self.with_uint(CLAIMINTERFACE, u32::from(interface))
    }

    fn release(&self, interface: u8) -> Result<(), Errno> {
        self.with_uint(RELEASEINTERFACE, u32::from(interface))
    }

    fn disconnect_driver(&self, interface: u8) -> Result<(), Errno> {
        self.driver(interface, DISCONNECT)
    }

    fn connect_driver(&self, interface: u8) -> Result<(), Errno> {
        self.driver(interface, CONNECT)
    }

    fn set_configuration(&self, value: u8) -> Result<(), Errno> {
        self.with_uint(SETCONFIGURATION, u32::from(value))
    }

    fn set_interface(&self, interface: u8, alternate: u8) -> Result<(), Errno> {
        let mut setting = SetInterface {
            interface: u32::from(interface),
            alternate: u32::from(alternate),
        };
        self.ioctl(SETINTERFACE, (&mut setting as *mut SetInterface).cast())
            .map(drop)
    }

    fn clear_halt(&self, endpoint: u8) -> Result<(), Errno> {
        self.with_uint(CLEAR_HALT, u32::from(endpoint))
    }

    fn reset(&self) -> Result<(), Errno> {
        self.ioctl(RESET, std::ptr::null_mut()).map(drop)
    }

    unsafe fn submit(&self, urb: *mut Urb) -> Result<(), Errno> {
        self.ioctl(SUBMITURB, urb.cast()).map(drop)
    }

    fn discard(&self, urb: *mut Urb) -> Result<(), Errno> {
        self.ioctl(DISCARDURB, urb.cast()).map(drop)
    }

    fn reap(&self) -> Result<Option<*mut Urb>, Errno> {
        let mut urb: *mut Urb = std::ptr::null_mut();
        match self.ioctl(REAPURBNDELAY, (&mut urb as *mut *mut Urb).cast()) {
            Ok(_) => Ok(Some(urb)),
            Err(e) if e.0 == libc::EAGAIN => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn wait(&self, timeout: Duration) -> Result<Ready, Errno> {
        let mut poll = libc::pollfd {
            fd: self.fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        match unsafe { libc::poll(&mut poll, 1, ms) } {
            0 => Ok(Ready::Idle),
            n if n > 0 => {
                // usbfs says POLLHUP or POLLERR once the device is unplugged, POLLOUT while a
                // completed URB waits.
                if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                    Ok(Ready::Gone)
                } else {
                    Ok(Ready::Completed)
                }
            }
            _ => Err(errno()),
        }
    }

    fn close(&self) {
        let fd = self.fd.swap(-1, Ordering::SeqCst);
        if fd >= 0 {
            unsafe { libc::close(fd) };
        }
    }
}

impl Drop for Usbfs {
    fn drop(&mut self) {
        self.close();
    }
}
