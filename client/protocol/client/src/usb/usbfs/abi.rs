//! usbfs's ioctl numbers and structures (<linux/usbdevice_fs.h>), for the target's pointer
//! width. Not gated on Linux, so their test runs on any host.

use super::Urb;

/// Linux's `_IOC` on the architectures Android runs on (arm, arm64, x86, x86_64).
const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr
}
pub const NONE: u32 = 0;
pub const WRITE: u32 = 1;
pub const READ: u32 = 2;

#[repr(C)]
pub struct CtrlTransfer {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    pub timeout_ms: u32,
    pub data: *mut u8,
}

#[repr(C)]
pub struct SetInterface {
    pub interface: u32,
    pub alternate: u32,
}

#[repr(C)]
pub struct Ioctl {
    pub interface: i32,
    pub code: i32,
    pub data: *mut u8,
}

pub const UINT: usize = std::mem::size_of::<u32>();
pub const CONTROL: u32 = ioc(READ | WRITE, 0, std::mem::size_of::<CtrlTransfer>());
pub const SETINTERFACE: u32 = ioc(READ, 4, std::mem::size_of::<SetInterface>());
pub const SETCONFIGURATION: u32 = ioc(READ, 5, UINT);
pub const SUBMITURB: u32 = ioc(READ, 10, std::mem::size_of::<Urb>());
pub const DISCARDURB: u32 = ioc(NONE, 11, 0);
pub const REAPURBNDELAY: u32 = ioc(WRITE, 13, std::mem::size_of::<*mut u8>());
pub const CLAIMINTERFACE: u32 = ioc(READ, 15, UINT);
pub const RELEASEINTERFACE: u32 = ioc(READ, 16, UINT);
pub const IOCTL: u32 = ioc(READ | WRITE, 18, std::mem::size_of::<Ioctl>());
pub const RESET: u32 = ioc(NONE, 20, 0);
pub const CLEAR_HALT: u32 = ioc(READ, 21, UINT);
pub const DISCONNECT: u32 = ioc(NONE, 22, 0);
pub const CONNECT: u32 = ioc(NONE, 23, 0);
pub const GET_SPEED: u32 = ioc(NONE, 31, 0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ioctl_numbers_are_linuxs() {
        // From <linux/usbdevice_fs.h>, which differ between 32- and 64-bit only by size.
        assert_eq!(CLAIMINTERFACE, 0x8004_550f);
        assert_eq!(DISCARDURB, 0x0000_550b);
        assert_eq!(GET_SPEED, 0x0000_551f);
        if std::mem::size_of::<usize>() == 8 {
            assert_eq!(SUBMITURB, 0x8038_550a);
            assert_eq!(REAPURBNDELAY, 0x4008_550d);
            assert_eq!(CONTROL, 0xc018_5500);
            assert_eq!(IOCTL, 0xc010_5512);
        } else {
            assert_eq!(SUBMITURB, 0x802c_550a);
            assert_eq!(REAPURBNDELAY, 0x4004_550d);
            assert_eq!(CONTROL, 0xc010_5500);
            assert_eq!(IOCTL, 0xc00c_5512);
        }
    }
}
