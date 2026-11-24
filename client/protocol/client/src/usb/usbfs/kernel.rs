//! Kernel-facing usbfs types, URB allocation wrapper, and [`Kernel`] ioctl trait.

use std::time::Duration;

/// A Linux errno, positive, from an ioctl. Linux's values on every host, so the tests run
/// anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(pub i32);

pub const EINTR: Errno = Errno(4);
pub const EAGAIN: Errno = Errno(11);
pub const EBUSY: Errno = Errno(16);
pub const ENODEV: Errno = Errno(19);
pub const EINVAL: Errno = Errno(22);

impl Errno {
    /// The negated errno a reply carries.
    pub fn status(self) -> i32 {
        -self.0
    }
}

impl std::fmt::Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "errno {}", self.0)
    }
}

/// `usbdevfs_urb.type`.
pub const URB_ISO: u8 = 0;
pub const URB_INTERRUPT: u8 = 1;
pub const URB_CONTROL: u8 = 2;
pub const URB_BULK: u8 = 3;

/// `usbdevfs_urb.flags`, which share the USB/IP flags' values.
pub const URB_SHORT_NOT_OK: u32 = 0x01;
pub const URB_ISO_ASAP: u32 = 0x02;
pub const URB_ZERO_PACKET: u32 = 0x40;

/// `struct usbdevfs_urb`; as many [`IsoFrame`]s as `number_of_packets` follow it.
#[repr(C)]
#[derive(Debug)]
pub struct Urb {
    pub kind: u8,
    pub endpoint: u8,
    pub status: i32,
    pub flags: u32,
    pub buffer: *mut u8,
    pub buffer_length: i32,
    pub actual_length: i32,
    pub start_frame: i32,
    pub number_of_packets: i32,
    pub error_count: i32,
    pub signr: u32,
    pub usercontext: usize,
}

/// `struct usbdevfs_iso_packet_desc`. `status` is a negated errno in an unsigned field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IsoFrame {
    pub length: u32,
    pub actual_length: u32,
    pub status: u32,
}

/// One URB and its packets in one allocation, as the kernel reads them.
pub struct UrbBox {
    urb: std::ptr::NonNull<Urb>,
    packets: usize,
}

// The kernel and the reaper only touch the URB under the device's state lock or after it
// left the kernel.
unsafe impl Send for UrbBox {}

impl UrbBox {
    fn layout(packets: usize) -> std::alloc::Layout {
        let size = std::mem::size_of::<Urb>() + packets * std::mem::size_of::<IsoFrame>();
        std::alloc::Layout::from_size_align(size, std::mem::align_of::<Urb>())
            .expect("a URB's layout")
    }

    pub fn new(packets: usize) -> Self {
        let layout = Self::layout(packets);
        // Zeroed: a valid Urb (null buffer) and packets.
        let raw = unsafe { std::alloc::alloc_zeroed(layout) }.cast::<Urb>();
        let urb =
            std::ptr::NonNull::new(raw).unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        Self { urb, packets }
    }

    pub fn as_ptr(&self) -> *mut Urb {
        self.urb.as_ptr()
    }

    pub fn urb(&self) -> &Urb {
        unsafe { self.urb.as_ref() }
    }

    pub fn urb_mut(&mut self) -> &mut Urb {
        unsafe { self.urb.as_mut() }
    }

    pub fn frames(&self) -> &[IsoFrame] {
        unsafe { std::slice::from_raw_parts(frames_of(self.as_ptr()), self.packets) }
    }

    pub fn frames_mut(&mut self) -> &mut [IsoFrame] {
        unsafe { std::slice::from_raw_parts_mut(frames_of(self.as_ptr()), self.packets) }
    }
}

impl Drop for UrbBox {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.urb.as_ptr().cast(), Self::layout(self.packets)) }
    }
}

/// The packets that follow `urb`.
///
/// # Safety
/// `urb` points at a URB allocated with its packets, as [`UrbBox`] does.
pub unsafe fn frames_of(urb: *mut Urb) -> *mut IsoFrame {
    // SAFETY: caller guarantees `urb` was allocated with trailing `IsoFrame` slots.
    unsafe { urb.cast::<u8>().add(std::mem::size_of::<Urb>()).cast() }
}

/// What [`Kernel::wait`] saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    /// A URB completed: reap it.
    Completed,
    /// Nothing within the timeout.
    Idle,
    /// The device was unplugged; what still completes can be reaped.
    Gone,
}

/// The usbfs ioctls of one open device.
pub trait Kernel: Send + Sync + 'static {
    /// The device descriptor and every configuration's descriptors, as `read` returns them.
    fn descriptors(&self) -> Result<Vec<u8>, Errno>;
    /// The active configuration's value, 0 when unconfigured (GET_CONFIGURATION).
    fn configuration(&self) -> Result<u8, Errno>;
    /// `enum usb_device_speed`, which USB/IP's speeds follow; 0 when the kernel does not know.
    /// Kernels before Linux 4.13 (many Android TV boxes) cannot say: `guessed_speed` then.
    fn speed(&self) -> Result<u32, Errno>;
    /// True for a low-speed device (USBDEVFS_CONNECTINFO, which every kernel answers).
    fn low_speed(&self) -> Result<bool, Errno>;
    /// A control read from the device; the bytes it answered.
    fn control_in(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &mut [u8],
    ) -> Result<usize, Errno>;
    fn claim(&self, interface: u8) -> Result<(), Errno>;
    fn release(&self, interface: u8) -> Result<(), Errno>;
    /// Unbinds the kernel driver that has `interface`.
    fn disconnect_driver(&self, interface: u8) -> Result<(), Errno>;
    /// Lets the kernel bind a driver to `interface` again.
    fn connect_driver(&self, interface: u8) -> Result<(), Errno>;
    fn set_configuration(&self, value: u8) -> Result<(), Errno>;
    fn set_interface(&self, interface: u8, alternate: u8) -> Result<(), Errno>;
    /// Clears a halt on `endpoint` (an address, with the IN bit) and resets its toggle.
    fn clear_halt(&self, endpoint: u8) -> Result<(), Errno>;
    fn reset(&self) -> Result<(), Errno>;
    /// Hands `urb` to the kernel.
    ///
    /// # Safety
    /// `urb`, its packets and its buffer stay valid and in place until [`Kernel::reap`]
    /// returns it, or until [`Kernel::close`] if it never does.
    unsafe fn submit(&self, urb: *mut Urb) -> Result<(), Errno>;
    /// Cancels `urb`; it still completes, with -ENOENT, and is reaped.
    fn discard(&self, urb: *mut Urb) -> Result<(), Errno>;
    /// A completed URB, its status and lengths written, or None when none has.
    fn reap(&self) -> Result<Option<*mut Urb>, Errno>;
    fn wait(&self, timeout: Duration) -> Result<Ready, Errno>;
    /// Closes the file descriptor; every call after fails.
    fn close(&self);
}
