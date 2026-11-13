//! Linux: a touchpad's fingers from its evdev device (multi-touch protocol B), read without
//! grabbing it, so the desktop keeps its pointer and the main loop drops what it makes of them.
//! Reading `/dev/input` needs the `input` group (or a seat ACL) on most distributions.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nightjar_core::input::touchpad::{length_mm, unit};
use sdl3_sys::everything::SDL_Window;

use super::{Event, Frame, RawContact};

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
const BTN_LEFT: u16 = 0x110;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TOOL_TYPE: u16 = 0x37;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_PRESSURE: u16 = 0x3a;
const ABS_CNT: usize = 0x40;
const INPUT_PROP_POINTER: usize = 0x00;
const INPUT_PROP_DIRECT: usize = 0x01;
const MT_TOOL_PALM: i32 = 2;
const MAX_SLOTS: usize = 16;
/// New touchpads (a keyboard cover attached) are looked for this often.
const RESCAN: Duration = Duration::from_secs(2);

/// `_IOC(_IOC_READ, 'E', nr, size)` (x86, Arm and RISC-V layout).
fn ioc_read(nr: u32, size: usize) -> libc::Ioctl {
    ((2u32 << 30) | ((size as u32 & 0x3fff) << 16) | ((b'E' as u32) << 8) | nr) as libc::Ioctl
}

/// `EVIOCGBIT(ev, len)`: which codes of an event type (0 for the types) the device has.
fn bits(file: &File, ev: u32, len: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; len];
    // SAFETY: the kernel writes at most `len` bytes into the buffer.
    let n = unsafe { libc::ioctl(file.as_raw_fd(), ioc_read(0x20 + ev, len), buffer.as_mut_ptr()) };
    (n >= 0).then_some(buffer)
}

/// `EVIOCGPROP(len)`: the device's input properties.
fn properties(file: &File) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; 4];
    // SAFETY: the kernel writes at most the buffer's length.
    let n = unsafe { libc::ioctl(file.as_raw_fd(), ioc_read(0x09, buffer.len()), buffer.as_mut_ptr()) };
    (n >= 0).then_some(buffer)
}

/// `EVIOCGABS(abs)`: an axis' range, resolution and current value.
fn absinfo(file: &File, code: u16) -> Option<libc::input_absinfo> {
    // SAFETY: zeroed is a valid input_absinfo (plain integers).
    let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the kernel writes one input_absinfo.
    let n = unsafe {
        libc::ioctl(file.as_raw_fd(), ioc_read(0x40 + u32::from(code), size_of::<libc::input_absinfo>()), &mut info as *mut libc::input_absinfo)
    };
    (n >= 0).then_some(info)
}

/// `EVIOCGMTSLOTS`: an axis' value in each slot.
fn slot_values(file: &File, code: u16, slots: usize) -> Option<Vec<i32>> {
    let mut buffer = vec![0i32; slots + 1];
    buffer[0] = i32::from(code);
    // SAFETY: the kernel reads the code and writes at most `slots` values after it.
    let n = unsafe { libc::ioctl(file.as_raw_fd(), ioc_read(0x0a, buffer.len() * 4), buffer.as_mut_ptr()) };
    (n >= 0).then(|| buffer[1..].to_vec())
}

fn has(bits: &[u8], index: usize) -> bool {
    bits.get(index / 8).is_some_and(|byte| byte & (1 << (index % 8)) != 0)
}

/// Whether the device's bits are a touchpad's: multi-touch protocol B positions on an indirect
/// pointer (a touchscreen is direct).
pub fn is_touchpad(abs: &[u8], props: &[u8]) -> bool {
    has(abs, usize::from(ABS_MT_SLOT))
        && has(abs, usize::from(ABS_MT_POSITION_X))
        && has(abs, usize::from(ABS_MT_POSITION_Y))
        && has(props, INPUT_PROP_POINTER)
        && !has(props, INPUT_PROP_DIRECT)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Slot {
    /// -1 when no finger is in the slot.
    tracking: i32,
    x: i32,
    y: i32,
    pressure: i32,
    palm: bool,
}

/// One touchpad's multi-touch state machine, fed evdev events.
#[derive(Debug)]
pub struct State {
    device: usize,
    slots: Vec<Slot>,
    slot: usize,
    button: bool,
    x: (i32, i32),
    y: (i32, i32),
    pressure: Option<(i32, i32)>,
    size_mm: (u16, u16),
    changed: bool,
    dropped: bool,
}

impl State {
    fn new(device: usize, slots: usize, x: (i32, i32), y: (i32, i32), pressure: Option<(i32, i32)>, size_mm: (u16, u16)) -> State {
        State {
            device,
            slots: vec![Slot { tracking: -1, ..Slot::default() }; slots.clamp(1, MAX_SLOTS)],
            slot: 0,
            button: false,
            x,
            y,
            pressure,
            size_mm,
            changed: false,
            dropped: false,
        }
    }

    /// One event; a frame at each `SYN_REPORT` that changed something. After `SYN_DROPPED`
    /// events up to the next report are lost: `resync` then reads the slots again.
    pub fn event(&mut self, kind: u16, code: u16, value: i32) -> Option<Frame> {
        if self.dropped {
            if kind == EV_SYN && code == SYN_REPORT {
                self.dropped = false;
            }
            return None;
        }
        match (kind, code) {
            (EV_SYN, SYN_REPORT) if self.changed => {
                self.changed = false;
                return Some(self.frame());
            }
            (EV_SYN, SYN_DROPPED) => self.dropped = true,
            (EV_KEY, BTN_LEFT) => {
                self.button = value != 0;
                self.changed = true;
            }
            (EV_ABS, ABS_MT_SLOT) => self.slot = usize::try_from(value).unwrap_or(usize::MAX),
            (EV_ABS, _) => {
                let slot = self.slots.get_mut(self.slot)?;
                match code {
                    ABS_MT_TRACKING_ID => {
                        slot.tracking = value;
                        slot.palm = false;
                    }
                    ABS_MT_POSITION_X => slot.x = value,
                    ABS_MT_POSITION_Y => slot.y = value,
                    ABS_MT_PRESSURE => slot.pressure = value,
                    ABS_MT_TOOL_TYPE => slot.palm = value == MT_TOOL_PALM,
                    _ => return None,
                }
                self.changed = true;
            }
            _ => {}
        }
        None
    }

    fn frame(&self) -> Frame {
        let contacts = self.slots.iter().filter(|s| s.tracking >= 0).map(|s| RawContact {
            pointer_id: s.tracking as u32,
            x: unit(s.x as f32, self.x.0 as f32, self.x.1 as f32),
            y: unit(s.y as f32, self.y.0 as f32, self.y.1 as f32),
            pressure: self.pressure.map_or(0.0, |(min, max)| unit(s.pressure as f32, min as f32, max as f32)),
            touching: !s.palm,
        }).collect();
        Frame { device: self.device, contacts: Some(contacts), button: self.button, width_mm: self.size_mm.0, height_mm: self.size_mm.1 }
    }

    /// Puts the slots read from the device in place, as after `SYN_DROPPED` or at the start.
    fn set_slots(&mut self, tracking: &[i32], x: &[i32], y: &[i32]) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            slot.tracking = tracking.get(i).copied().unwrap_or(-1);
            slot.x = x.get(i).copied().unwrap_or(slot.x);
            slot.y = y.get(i).copied().unwrap_or(slot.y);
            slot.palm = false;
        }
        self.changed = true;
    }
}

struct Pad {
    file: File,
    path: PathBuf,
    state: State,
}

impl Pad {
    fn open(path: &Path, device: usize) -> Result<Option<Pad>, std::io::Error> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(path)?;
        let (Some(abs), Some(props)) = (bits(&file, u32::from(EV_ABS), ABS_CNT / 8), properties(&file)) else { return Ok(None) };
        if !is_touchpad(&abs, &props) {
            return Ok(None);
        }
        let (Some(x), Some(y), Some(slot)) = (absinfo(&file, ABS_MT_POSITION_X), absinfo(&file, ABS_MT_POSITION_Y), absinfo(&file, ABS_MT_SLOT)) else {
            return Ok(None);
        };
        let pressure = has(&abs, usize::from(ABS_MT_PRESSURE)).then(|| absinfo(&file, ABS_MT_PRESSURE)).flatten().map(|p| (p.minimum, p.maximum));
        let size_mm = (length_mm(x.minimum, x.maximum, x.resolution as f32), length_mm(y.minimum, y.maximum, y.resolution as f32));
        let slots = usize::try_from(slot.maximum).map_or(1, |max| max + 1);
        let mut state = State::new(device, slots, (x.minimum, x.maximum), (y.minimum, y.maximum), pressure, size_mm);
        state.slot = usize::try_from(slot.value).unwrap_or(0);
        let mut pad = Pad { file, path: path.to_owned(), state };
        pad.resync();
        Ok(Some(pad))
    }

    /// Reads which fingers are down now: at the start, or after the kernel dropped events.
    fn resync(&mut self) {
        let n = self.state.slots.len();
        if let (Some(tracking), Some(x), Some(y)) = (
            slot_values(&self.file, ABS_MT_TRACKING_ID, n),
            slot_values(&self.file, ABS_MT_POSITION_X, n),
            slot_values(&self.file, ABS_MT_POSITION_Y, n),
        ) {
            self.state.set_slots(&tracking, &x, &y);
        }
        // BTN_LEFT's state (EVIOCGKEY) is left to the next press; a click across a drop is rare.
    }

    /// The frames since the last call; Err when the touchpad is gone.
    fn read(&mut self, out: &mut Vec<Event>) -> Result<(), ()> {
        const SIZE: usize = size_of::<libc::input_event>();
        let mut buffer = [0u8; SIZE * 64];
        loop {
            match self.file.read(&mut buffer) {
                Ok(0) => return Err(()),
                Ok(n) => {
                    for chunk in buffer[..n].as_chunks::<SIZE>().0 {
                        // SAFETY: a whole input_event the kernel wrote; read unaligned.
                        let event: libc::input_event = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
                        let was_dropped = self.state.dropped;
                        if let Some(frame) = self.state.event(event.type_, event.code, event.value) {
                            out.push(Event::Frame(frame));
                        }
                        if was_dropped && !self.state.dropped {
                            self.resync();
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(()),
            }
        }
    }
}

/// The touchpads' evdev devices.
pub struct Evdev {
    pads: Vec<Pad>,
    next_device: usize,
    scanned: Instant,
}

impl Evdev {
    pub fn install(_window: *mut SDL_Window) -> Result<Evdev, String> {
        let mut evdev = Evdev { pads: Vec::new(), next_device: 1, scanned: Instant::now() };
        let denied = evdev.scan();
        if evdev.pads.is_empty() {
            return Err(if denied {
                "Nightjar may not read the touchpad. Add your user to the input group, then sign in again.".into()
            } else {
                "no touchpad found".into()
            });
        }
        Ok(evdev)
    }

    /// Opens touchpads not open yet; true when a device could not be opened for lack of rights.
    fn scan(&mut self) -> bool {
        self.scanned = Instant::now();
        let mut denied = false;
        let Ok(entries) = std::fs::read_dir("/dev/input") else { return false };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_event = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event"));
            if !is_event || self.pads.iter().any(|p| p.path == path) {
                continue;
            }
            match Pad::open(&path, self.next_device) {
                Ok(Some(pad)) => {
                    eprintln!("nightjar: touchpad {} ({} x {} mm)", path.display(), pad.state.size_mm.0, pad.state.size_mm.1);
                    self.next_device += 1;
                    self.pads.push(pad);
                }
                Ok(None) => {}
                Err(e) if e.kind() == ErrorKind::PermissionDenied => denied = true,
                Err(_) => {}
            }
        }
        denied
    }

    pub fn take(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        self.pads.retain_mut(|pad| match pad.read(&mut out) {
            Ok(()) => true,
            Err(()) => {
                out.push(Event::Removed(pad.state.device));
                false
            }
        });
        if self.scanned.elapsed() >= RESCAN {
            self.scan();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State::new(1, 5, (0, 1000), (0, 500), Some((0, 255)), (100, 50))
    }

    fn ids(frame: &Frame) -> Vec<(u32, f32, f32, bool)> {
        frame.contacts.as_ref().unwrap().iter().map(|c| (c.pointer_id, c.x, c.y, c.touching)).collect()
    }

    #[test]
    fn protocol_b_slots_become_frames_at_each_report() {
        let mut s = state();
        assert_eq!(s.event(EV_ABS, ABS_MT_SLOT, 0), None);
        s.event(EV_ABS, ABS_MT_TRACKING_ID, 40);
        s.event(EV_ABS, ABS_MT_POSITION_X, 500);
        s.event(EV_ABS, ABS_MT_POSITION_Y, 250);
        let f = s.event(EV_SYN, SYN_REPORT, 0).unwrap();
        assert_eq!(ids(&f), [(40, 0.5, 0.5, true)]);
        assert_eq!((f.device, f.width_mm, f.height_mm, f.button), (1, 100, 50, false));
        // A second finger in slot 1 and the click.
        s.event(EV_ABS, ABS_MT_SLOT, 1);
        s.event(EV_ABS, ABS_MT_TRACKING_ID, 41);
        s.event(EV_ABS, ABS_MT_POSITION_X, 1000);
        s.event(EV_KEY, BTN_LEFT, 1);
        let f = s.event(EV_SYN, SYN_REPORT, 0).unwrap();
        assert_eq!(ids(&f), [(40, 0.5, 0.5, true), (41, 1.0, 0.0, true)]);
        assert!(f.button);
        // A report without changes is no frame.
        assert_eq!(s.event(EV_SYN, SYN_REPORT, 0), None);
        // The first finger lifts; the second turns out to be a palm.
        s.event(EV_ABS, ABS_MT_SLOT, 0);
        s.event(EV_ABS, ABS_MT_TRACKING_ID, -1);
        s.event(EV_ABS, ABS_MT_SLOT, 1);
        s.event(EV_ABS, ABS_MT_TOOL_TYPE, MT_TOOL_PALM);
        let f = s.event(EV_SYN, SYN_REPORT, 0).unwrap();
        assert_eq!(ids(&f), [(41, 1.0, 0.0, false)]);
    }

    #[test]
    fn dropped_events_wait_for_the_report_and_the_slots_read_again() {
        let mut s = state();
        s.event(EV_ABS, ABS_MT_TRACKING_ID, 7);
        s.event(EV_SYN, SYN_REPORT, 0).unwrap();
        s.event(EV_SYN, SYN_DROPPED, 0);
        assert_eq!(s.event(EV_ABS, ABS_MT_TRACKING_ID, -1), None);
        assert_eq!(s.event(EV_SYN, SYN_REPORT, 0), None);
        assert!(!s.dropped);
        s.set_slots(&[-1, 9], &[0, 250], &[0, 125]);
        let f = s.event(EV_SYN, SYN_REPORT, 0).unwrap();
        assert_eq!(ids(&f), [(9, 0.25, 0.25, true)]);
        // A slot past the device's is ignored.
        s.event(EV_ABS, ABS_MT_SLOT, 99);
        assert_eq!(s.event(EV_ABS, ABS_MT_POSITION_X, 5), None);
    }

    #[test]
    fn only_an_indirect_multi_touch_pointer_is_a_touchpad() {
        let mut abs = vec![0u8; ABS_CNT / 8];
        for code in [ABS_MT_SLOT, ABS_MT_POSITION_X, ABS_MT_POSITION_Y] {
            abs[usize::from(code) / 8] |= 1 << (code % 8);
        }
        assert!(is_touchpad(&abs, &[1 << INPUT_PROP_POINTER]));
        assert!(!is_touchpad(&abs, &[(1 << INPUT_PROP_POINTER) | (1 << INPUT_PROP_DIRECT)]), "a touchscreen");
        assert!(!is_touchpad(&abs, &[0]));
        assert!(!is_touchpad(&[0; ABS_CNT / 8], &[1 << INPUT_PROP_POINTER]));
        // EVIOCGABS(ABS_MT_POSITION_X) as the kernel headers spell it.
        assert_eq!(ioc_read(0x40 + 0x35, 24) as u32, 0x8018_4575);
    }
}
