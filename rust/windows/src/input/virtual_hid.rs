//! The virtual USB mouse and Precision Touchpad (`butterpollo_core::virtual_hid`), plugged in
//! through usbip-win2 while a stream wants them and unplugged when the last one ends.
//!
//! Mouse input keeps its whole injection logic (button ownership across clients, the
//! absolute-mode left release, scroll steps): only the last step changes. Where SendInput
//! would take a mouse `INPUT`, [`hid_ops`] turns it into the virtual mouse's reports, so a
//! game reads it as a USB mouse's raw input instead of injected input.
use super::usbip_pad::{UsbPad, usbip_exe};
use butterpollo_core::{
    usbip::Device,
    virtual_hid::{self, HidMouse, POLL, PrecisionTouchpad},
};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use windows::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};

/// How long a device that failed to plug waits before a stream may try again.
const RETRY: Duration = Duration::from_secs(30);

/// Where a device stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready,
    Plugging,
    /// usbip-win2 is not installed.
    NoDriver,
    /// Plugging it failed; the reason.
    Failed(String),
}

enum State<D: Device> {
    Unplugged,
    Plugging,
    Plugged(UsbPad<D>),
    Failed { at: Instant, error: String },
}

struct Inner<D: Device> {
    state: State<D>,
    /// Streams that want the device.
    users: usize,
}

/// One virtual device of the host, shared by every stream.
pub struct Slot<D: Device> {
    name: &'static str,
    inner: Mutex<Inner<D>>,
}

pub static MOUSE: Slot<HidMouse> = Slot::new("virtual mouse");
pub static TOUCHPAD: Slot<PrecisionTouchpad> = Slot::new("virtual touchpad");

impl<D: Device> Slot<D> {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            inner: Mutex::new(Inner {
                state: State::Unplugged,
                users: 0,
            }),
        }
    }
    /// A stream wants the device; [`Self::release`] when it ends.
    pub fn acquire(&self) {
        self.inner.lock().unwrap().users += 1;
    }
    /// A stream no longer wants it: the last one unplugs it, off the caller's thread.
    pub fn release(&'static self) {
        let mut inner = self.inner.lock().unwrap();
        inner.users = inner.users.saturating_sub(1);
        if inner.users > 0 {
            return;
        }
        if let State::Plugged(pad) = std::mem::replace(&mut inner.state, State::Unplugged) {
            drop(inner);
            let name = self.name;
            let unplug = move || {
                drop(pad);
                tracing::info!(device = name, "unplugged");
            };
            if let Err(error) = std::thread::Builder::new()
                .name("virtual HID unplug".into())
                .spawn(unplug)
            {
                tracing::warn!(device = name, %error, "the unplug thread did not start");
            }
        }
    }
    /// Whether the device is plugged in and Windows imported it.
    pub fn ready(&self) -> bool {
        matches!(&self.inner.lock().unwrap().state, State::Plugged(pad)
            if pad.export.as_ref().is_some_and(|export| export.imported()))
    }
    /// Plugs in the device `make` builds, on a thread of its own, unless it is plugged in,
    /// on its way, or failed less than [`RETRY`] ago. Returns where it stands.
    pub fn plug(&'static self, make: impl FnOnce() -> D + Send + 'static) -> Availability {
        let mut inner = self.inner.lock().unwrap();
        match &inner.state {
            State::Plugged(_) => return Availability::Ready,
            State::Plugging => return Availability::Plugging,
            State::Failed { at, error } if at.elapsed() < RETRY => {
                return Availability::Failed(error.clone());
            }
            _ => {}
        }
        let Some(exe) = usbip_exe() else {
            inner.state = State::Unplugged;
            return Availability::NoDriver;
        };
        inner.state = State::Plugging;
        drop(inner);
        let name = self.name;
        let plug = move || {
            let started = Instant::now();
            let result = UsbPad::plug(&exe, make(), POLL);
            let mut inner = self.inner.lock().unwrap();
            match result {
                Ok(pad) if inner.users > 0 => {
                    tracing::info!(
                        device = name,
                        port = pad.port(),
                        ms = started.elapsed().as_millis() as u64,
                        "plugged in"
                    );
                    inner.state = State::Plugged(pad);
                }
                Ok(pad) => {
                    // The streams that wanted it ended meanwhile.
                    inner.state = State::Unplugged;
                    drop(inner);
                    drop(pad);
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    tracing::warn!(device = name, error, "could not be plugged in");
                    inner.state = State::Failed {
                        at: Instant::now(),
                        error,
                    };
                }
            }
        };
        if let Err(error) = std::thread::Builder::new()
            .name("virtual HID plug".into())
            .spawn(plug)
        {
            let error = format!("the plug thread did not start: {error}");
            self.inner.lock().unwrap().state = State::Failed {
                at: Instant::now(),
                error: error.clone(),
            };
            return Availability::Failed(error);
        }
        Availability::Plugging
    }
    /// Runs `f` on the plugged-in device and tells the USB/IP server it changed; None when it
    /// is not plugged in.
    pub fn with<R>(&self, f: impl FnOnce(&mut D) -> R) -> Option<R> {
        let inner = self.inner.lock().unwrap();
        let State::Plugged(pad) = &inner.state else {
            return None;
        };
        let export = pad.export.as_ref()?;
        if !export.imported() {
            return None;
        }
        let result = export.with(f);
        export.changed();
        Some(result)
    }
}

/// One step of a mouse `INPUT` on the virtual mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Motion(i32, i32),
    /// On the absolute collection, 0-[`virtual_hid::ABSOLUTE_MAX`].
    Absolute(u16, u16),
    Button(u8, bool),
    Scroll(i16, bool),
}

/// The virtual mouse's steps for a SendInput mouse input, or None for one it cannot carry: an
/// absolute move while the desktop spans more than one display (Windows maps an absolute
/// USB mouse onto the primary display only).
pub fn hid_ops(mi: &MOUSEINPUT, single_display: bool) -> Option<Vec<Op>> {
    let flags = mi.dwFlags;
    let mut ops = Vec::new();
    if flags.contains(MOUSEEVENTF_MOVE) {
        if flags.contains(MOUSEEVENTF_ABSOLUTE) {
            if !single_display {
                return None;
            }
            // SendInput's 0-65535 over the desktop, which is the primary display here, rounded
            // to the nearest point of the virtual mouse's 0-ABSOLUTE_MAX. Never 0: Windows
            // ignores an absolute (0, 0) from a HID mouse (the pointer stayed put), and 1 still
            // lands on the first pixel; the far edge, ABSOLUTE_MAX, lands on the last.
            let scale = |v: i32| {
                (((i64::from(v.clamp(0, 65535)) * i64::from(virtual_hid::ABSOLUTE_MAX) + 65535 / 2)
                    / 65535) as u16)
                    .max(1)
            };
            ops.push(Op::Absolute(scale(mi.dx), scale(mi.dy)));
        } else {
            ops.push(Op::Motion(mi.dx, mi.dy));
        }
    }
    for (down, up, bit) in [
        (
            MOUSEEVENTF_LEFTDOWN,
            MOUSEEVENTF_LEFTUP,
            virtual_hid::button::LEFT,
        ),
        (
            MOUSEEVENTF_RIGHTDOWN,
            MOUSEEVENTF_RIGHTUP,
            virtual_hid::button::RIGHT,
        ),
        (
            MOUSEEVENTF_MIDDLEDOWN,
            MOUSEEVENTF_MIDDLEUP,
            virtual_hid::button::MIDDLE,
        ),
    ] {
        if flags.contains(down) {
            ops.push(Op::Button(bit, true));
        }
        if flags.contains(up) {
            ops.push(Op::Button(bit, false));
        }
    }
    let x = |data: u32| {
        [
            (XBUTTON1 as u32, virtual_hid::button::BACK),
            (XBUTTON2 as u32, virtual_hid::button::FORWARD),
        ]
        .into_iter()
        .filter(move |(mask, _)| data & mask != 0)
        .map(|(_, bit)| bit)
    };
    if flags.contains(MOUSEEVENTF_XDOWN) {
        ops.extend(x(mi.mouseData).map(|bit| Op::Button(bit, true)));
    }
    if flags.contains(MOUSEEVENTF_XUP) {
        ops.extend(x(mi.mouseData).map(|bit| Op::Button(bit, false)));
    }
    let amount = (mi.mouseData as i32).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    if flags.contains(MOUSEEVENTF_WHEEL) {
        ops.push(Op::Scroll(amount, false));
    }
    if flags.contains(MOUSEEVENTF_HWHEEL) {
        ops.push(Op::Scroll(amount, true));
    }
    Some(ops)
}

/// Applies `ops` to the virtual mouse; false when it is not plugged in.
pub fn apply(ops: &[Op]) -> bool {
    MOUSE
        .with(|mouse| {
            for op in ops {
                match *op {
                    Op::Motion(dx, dy) => {
                        mouse.motion(dx, dy);
                    }
                    Op::Absolute(x, y) => mouse.absolute(x, y),
                    Op::Button(bit, down) => {
                        mouse.button(bit, down);
                    }
                    Op::Scroll(amount, horizontal) => {
                        mouse.scroll(amount, horizontal);
                    }
                }
            }
        })
        .is_some()
}

/// Whether the desktop is one display, so an absolute USB mouse covers all of it.
pub fn single_display() -> bool {
    // SAFETY: GetSystemMetrics takes a metric constant and borrows nothing.
    unsafe { GetSystemMetrics(SM_CMONITORS) == 1 }
}

/// The device serial: the host's process, so a restarted host is a new device to Windows'
/// HID stack but the same one across streams.
pub fn serial() -> String {
    format!("RUBYLIGHT{}", std::process::id())
}

/// QueryPerformanceCounter now, for the input timing log (`input_timing` at trace level)
/// that `input_latency_probe` pairs with the WM_INPUT it sees.
pub fn qpc() -> i64 {
    let mut now = 0;
    // SAFETY: QueryPerformanceCounter writes only to the live local.
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    now
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: u32) -> MOUSEINPUT {
        MOUSEINPUT {
            dx,
            dy,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        }
    }

    #[test]
    fn injected_mouse_inputs_become_virtual_mouse_steps() {
        assert_eq!(
            hid_ops(&input(MOUSEEVENTF_MOVE, -3, 4, 0), false),
            Some(vec![Op::Motion(-3, 4)])
        );
        assert_eq!(
            hid_ops(&input(MOUSEEVENTF_LEFTDOWN, 0, 0, 0), false),
            Some(vec![Op::Button(virtual_hid::button::LEFT, true)])
        );
        assert_eq!(
            hid_ops(&input(MOUSEEVENTF_RIGHTUP, 0, 0, 0), false),
            Some(vec![Op::Button(virtual_hid::button::RIGHT, false)])
        );
        assert_eq!(
            hid_ops(&input(MOUSEEVENTF_XDOWN, 0, 0, XBUTTON2 as u32), false),
            Some(vec![Op::Button(virtual_hid::button::FORWARD, true)])
        );
        assert_eq!(
            hid_ops(&input(MOUSEEVENTF_WHEEL, 0, 0, (-120i32) as u32), false),
            Some(vec![Op::Scroll(-120, false)])
        );
        assert_eq!(
            hid_ops(&input(MOUSEEVENTF_HWHEEL, 0, 0, 30), false),
            Some(vec![Op::Scroll(30, true)])
        );
    }

    #[test]
    fn absolute_moves_take_the_virtual_mouse_only_on_a_single_display() {
        let absolute = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        assert_eq!(hid_ops(&input(absolute, 65535, 0, 0), false), None);
        assert_eq!(
            hid_ops(&input(absolute, 65535, 32768, 0), true),
            Some(vec![Op::Absolute(32767, 16384)])
        );
        // The top left corner is 1, not the 0 Windows ignores; the right edge stays the last.
        assert_eq!(
            hid_ops(&input(absolute, 0, 0, 0), true),
            Some(vec![Op::Absolute(1, 1)])
        );
        assert_eq!(
            hid_ops(&input(absolute, -5, 70000, 0), true),
            Some(vec![Op::Absolute(1, 32767)])
        );
    }

    #[test]
    fn a_device_that_is_not_plugged_in_takes_nothing() {
        static UNUSED: Slot<HidMouse> = Slot::new("test mouse");
        assert!(!UNUSED.ready());
        assert_eq!(UNUSED.with(|mouse| mouse.motion(1, 1)), None);
        UNUSED.acquire();
        UNUSED.release();
        UNUSED.release();
        assert!(!UNUSED.ready());
    }
}
