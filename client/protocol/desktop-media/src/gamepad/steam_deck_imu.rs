//! Direct read-only Steam Deck IMU reader for SteamOS Game Mode where Steam Virtual Gamepad
//! (`28de:11ff`) hides SDL motion sensors on the underlying controller (`28de:1205`).
//!
//! Input report layout (`SteamDeckStatePacket_t`) and sensor axis orientation follow SDL3's
//! `SDL_hidapi_steamdeck.c` (zlib) and `rust/core/input-policy/src/steam_deck.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
use nightjar_core::input::InputQueue;
use nightjar_core::input::packets::motion_type;
use nightjar_core::session::InputHandle;
use sdl3_sys::everything::*;

use crate::gamepad_types::Mapping;

use super::{Gamepads, Pad};

pub(super) const VALVE_VENDOR_ID: u16 = 0x28de;
pub(super) const STEAM_DECK_PRODUCT_ID: u16 = 0x1205;
pub(super) const STEAM_VIRTUAL_GAMEPAD_PRODUCT_ID: u16 = 0x11ff;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) const ACCEL_SCALE: f32 = 2.0 * 9.80665 / 32768.0;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) const GYRO_SCALE: f32 = 2000.0 / 32768.0;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DeckImuSample {
    pub accel_ms2: [f32; 3],
    pub gyro_dps: [f32; 3],
}

/// Decodes a 64-byte `ID_CONTROLLER_DECK_STATE` report (or 65-byte with leading `0x00` report ID).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn parse_deck_state_report(report: &[u8]) -> Option<DeckImuSample> {
    let payload = match report.len() {
        64 => report,
        65 if report[0] == 0x00 => &report[1..],
        _ => return None,
    };
    if payload[..4] != [0x01, 0x00, 0x09, 64] {
        return None;
    }
    let word = |at: usize| f32::from(i16::from_le_bytes([payload[at], payload[at + 1]]));
    let (ax, ay, az) = (word(24), word(26), word(28));
    let (gx, gy, gz) = (word(30), word(32), word(34));
    Some(DeckImuSample {
        accel_ms2: [ax * ACCEL_SCALE, az * ACCEL_SCALE, -ay * ACCEL_SCALE],
        gyro_dps: [gx * GYRO_SCALE, gz * GYRO_SCALE, -gy * GYRO_SCALE],
    })
}

pub(super) fn is_deck_imu_candidate(usb_id: (u16, u16), name: &str, has_sdl_sensors: bool) -> bool {
    !has_sdl_sensors
        && (usb_id == (VALVE_VENDOR_ID, STEAM_VIRTUAL_GAMEPAD_PRODUCT_ID)
            || usb_id == (VALVE_VENDOR_ID, STEAM_DECK_PRODUCT_ID)
            || name.eq_ignore_ascii_case("Steam Virtual Gamepad"))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn uevent_matches_steam_deck(uevent: &str) -> bool {
    uevent.lines().any(|line| {
        let Some((key, value)) = line.trim().split_once('=') else { return false };
        if !key.eq_ignore_ascii_case("HID_ID") {
            return false;
        }
        let mut p = value.split(':');
        matches!(
            (p.next(), p.next(), p.next(), p.next()),
            (Some(bus), Some(vid), Some(pid), None)
                if !bus.is_empty() && vid.eq_ignore_ascii_case("000028DE") && pid.eq_ignore_ascii_case("00001205")
        )
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn is_controller_interface(interface_num: &str) -> bool {
    u8::from_str_radix(interface_num.trim(), 16) == Ok(2)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Default)]
pub(super) struct RateGate {
    last_slot_us: Option<u64>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl RateGate {
    pub(super) fn should_emit(&mut self, now_us: u64, rate_hz: u16) -> bool {
        if rate_hz == 0 {
            self.last_slot_us = None;
            return false;
        }
        let interval_us = 1_000_000 / u64::from(rate_hz.clamp(1, 1000));
        match self.last_slot_us {
            Some(last) => {
                let elapsed = now_us.saturating_sub(last);
                if elapsed < interval_us {
                    return false;
                }
                // Advance by one interval to preserve cadence phase (e.g. 250 Hz hardware at a
                // 200 Hz host request emits 4/5 packets = 200 Hz instead of halving to 125 Hz),
                // snapping to `now_us` after an idle gap so buffered reads never burst.
                self.last_slot_us = Some(if elapsed >= interval_us * 2 {
                    now_us
                } else {
                    last.saturating_add(interval_us)
                });
                true
            }
            None => {
                self.last_slot_us = Some(now_us);
                true
            }
        }
    }
}

#[derive(Default)]
pub(super) struct DeckImuShared {
    stop: AtomicBool,
    sending: AtomicBool,
    refused: AtomicBool,
    sent_as: AtomicU8,
    accel_rate_hz: AtomicU16,
    gyro_rate_hz: AtomicU16,
    mapping: Mutex<Mapping>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
type ReadyMotion = (u8, Option<[f32; 3]>, Option<[f32; 3]>);

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn send_motion(queue: &mut InputQueue, (sent_as, accel, gyro): ReadyMotion) {
    if let Some([ax, ay, az]) = accel {
        let _ = queue.send_controller_motion(sent_as, motion_type::ACCEL, ax, ay, az);
    }
    if let Some([gx, gy, gz]) = gyro {
        let _ = queue.send_controller_motion(sent_as, motion_type::GYRO, gx, gy, gz);
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl DeckImuShared {
    fn prepare_motion(
        &self,
        report: &[u8],
        now_us: u64,
        accel_gate: &mut RateGate,
        gyro_gate: &mut RateGate,
    ) -> Option<ReadyMotion> {
        let sample = parse_deck_state_report(report)?;
        if !self.sending.load(Ordering::Acquire) || self.refused.load(Ordering::Acquire) {
            return None;
        }
        let mapping = self
            .mapping
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if mapping.drops_motion_and_touchpad() {
            return None;
        }
        let accel = accel_gate
            .should_emit(now_us, self.accel_rate_hz.load(Ordering::Acquire))
            .then_some(sample.accel_ms2);
        let gyro = gyro_gate
            .should_emit(now_us, self.gyro_rate_hz.load(Ordering::Acquire))
            .then(|| mapping.map_gyro(sample.gyro_dps));
        (accel.is_some() || gyro.is_some())
            .then(|| (self.sent_as.load(Ordering::Acquire), accel, gyro))
    }
}

pub(super) struct DeckImu {
    device_path: PathBuf,
    shared: Arc<DeckImuShared>,
    worker: Option<JoinHandle<()>>,
}

impl DeckImu {
    pub(super) fn new(device_path: PathBuf) -> Self {
        Self {
            device_path,
            shared: Arc::new(DeckImuShared::default()),
            worker: None,
        }
    }

    pub(super) fn sync(&self, sent_as: u8, refused: bool, mapping: &Mapping) {
        self.shared.sent_as.store(sent_as, Ordering::Release);
        self.shared.refused.store(refused, Ordering::Release);
        let mut guard = self
            .shared
            .mapping
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if *guard != *mapping {
            *guard = mapping.clone();
        }
    }

    pub(super) fn set_sending(&self, sending: bool) {
        self.shared.sending.store(sending, Ordering::Release);
    }

    pub(super) fn set_rate(&mut self, kind: u8, rate_hz: u16, input: Option<&InputHandle>) {
        match kind {
            motion_type::ACCEL => self.shared.accel_rate_hz.store(rate_hz, Ordering::Release),
            motion_type::GYRO => self.shared.gyro_rate_hz.store(rate_hz, Ordering::Release),
            _ => return,
        }
        if self.shared.accel_rate_hz.load(Ordering::Acquire) == 0
            && self.shared.gyro_rate_hz.load(Ordering::Acquire) == 0
        {
            self.stop();
            return;
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
        if self.worker.is_none()
            && let Some(input) = input
        {
            self.shared.stop.store(false, Ordering::Release);
            let (path, shared, input) =
                (self.device_path.clone(), self.shared.clone(), input.clone());
            match std::thread::Builder::new()
                .name("nightjar-deck-imu".into())
                .spawn(move || run_reader(path, shared, input))
            {
                Ok(handle) => self.worker = Some(handle),
                Err(error) => eprintln!("nightjar: Steam Deck IMU thread failed: {error}"),
            }
        }
    }

    pub(super) fn stop(&mut self) {
        self.shared.accel_rate_hz.store(0, Ordering::Release);
        self.shared.gyro_rate_hz.store(0, Ordering::Release);
        self.shared.sending.store(false, Ordering::Release);
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DeckImu {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pad_has_sdl_sensors(pad: &Pad) -> bool {
    // SAFETY: `pad.handle` is an open `SDL_Gamepad` owned by `Pad`.
    !pad.handle.is_null()
        && unsafe {
            SDL_GamepadHasSensor(pad.handle, SDL_SENSOR_ACCEL)
                || SDL_GamepadHasSensor(pad.handle, SDL_SENSOR_GYRO)
        }
}

impl Gamepads {
    pub(super) fn refresh_deck_imu(&mut self) {
        let native = self.pads.iter().any(|p| {
            p.usb_id == (VALVE_VENDOR_ID, STEAM_DECK_PRODUCT_ID) && pad_has_sdl_sensors(p)
        });
        if native {
            for pad in &mut self.pads {
                if pad.deck_imu.take().is_some() {
                    pad.announced = false;
                }
            }
            return;
        }
        if !self.pads.iter().any(|p| p.deck_imu.is_some())
            && let Some(idx) = self
                .pads
                .iter()
                .position(|p| is_deck_imu_candidate(p.usb_id, &p.name, pad_has_sdl_sensors(p)))
            && let Some(path) = find_steam_deck_hidraw()
        {
            eprintln!(
                "nightjar: controller {} ({}): reading Steam Deck IMU directly from {}",
                self.pads[idx].sent_as,
                self.pads[idx].name,
                path.display()
            );
            self.pads[idx].deck_imu = Some(DeckImu::new(path));
            self.pads[idx].announced = false;
        }
        for pad in &self.pads {
            if let Some(imu) = &pad.deck_imu {
                imu.sync(pad.sent_as, pad.refused, &pad.mapping);
            }
        }
    }

    pub(crate) fn set_deck_imu_sending(&self, sending: bool) {
        for pad in &self.pads {
            if let Some(imu) = &pad.deck_imu {
                imu.set_sending(sending);
            }
        }
    }

    pub fn apply_pad_mapping(&mut self, key: &str, mapping: &Mapping) {
        for pad in &mut self.pads {
            if pad.key == key {
                pad.mapping = mapping.clone();
            }
        }
        self.mappings.insert(key.to_owned(), mapping.clone());
        self.refresh_deck_imu();
    }
}

#[cfg(not(target_os = "linux"))]
fn find_steam_deck_hidraw() -> Option<PathBuf> {
    None
}

#[cfg(not(target_os = "linux"))]
fn run_reader(_device_path: PathBuf, _shared: Arc<DeckImuShared>, _input: InputHandle) {}

#[cfg(target_os = "linux")]
#[repr(C)]
struct HidrawDevinfo {
    #[allow(dead_code)]
    bustype: u32,
    vendor: i16,
    product: i16,
}

#[cfg(target_os = "linux")]
struct RawFd(libc::c_int);

#[cfg(target_os = "linux")]
impl Drop for RawFd {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an open file descriptor owned by `RawFd`.
        unsafe { libc::close(self.0) };
    }
}

#[cfg(target_os = "linux")]
fn open_readonly(path: &std::path::Path) -> Option<RawFd> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let flags = libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC;
    loop {
        // SAFETY: `c_path` is NUL-terminated and opened strictly read-only non-blocking.
        let fd = unsafe { libc::open(c_path.as_ptr(), flags) };
        if fd >= 0 {
            return Some(RawFd(fd));
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return None;
        }
    }
}

#[cfg(target_os = "linux")]
fn poll_and_drain(
    fd: &RawFd,
    timeout_ms: libc::c_int,
    buf: &mut [u8; 128],
    mut on_packet: impl FnMut(&[u8]) -> bool,
) -> Option<bool> {
    let mut pfd = libc::pollfd {
        fd: fd.0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pfd` points to one valid `pollfd` for `fd.0`.
    let prc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if prc < 0 {
        return (std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR))
            .then_some(false);
    }
    if prc == 0 {
        return Some(false);
    }
    if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
        && pfd.revents & libc::POLLIN == 0
    {
        return None;
    }
    loop {
        // SAFETY: `fd.0` is open and `buf` is valid for `buf.len()` bytes.
        let n = unsafe { libc::read(fd.0, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            if on_packet(&buf[..n as usize]) {
                return Some(true);
            }
            continue;
        }
        if n == 0 {
            return None;
        }
        match std::io::Error::last_os_error().raw_os_error().unwrap_or(0) {
            libc::EINTR => continue,
            libc::EAGAIN => return Some(false),
            _ => return None,
        }
    }
}

#[cfg(target_os = "linux")]
fn sorted_hidraw_names(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("hidraw"))
        .collect();
    names.sort_by(|a, b| {
        let num = |s: &str| s.strip_prefix("hidraw").and_then(|n| n.parse::<u32>().ok());
        (num(a).unwrap_or(u32::MAX), a).cmp(&(num(b).unwrap_or(u32::MAX), b))
    });
    names
}

#[cfg(target_os = "linux")]
fn find_steam_deck_hidraw() -> Option<PathBuf> {
    const HIDIOCGRAWINFO: libc::Ioctl = 0x8008_4803_u32 as libc::Ioctl;
    let (mut saw_iface, mut missing_iface) = (false, false);
    for name in sorted_hidraw_names(std::path::Path::new("/sys/class/hidraw")) {
        let sys_dev = PathBuf::from("/sys/class/hidraw")
            .join(&name)
            .join("device");
        let Ok(uevent) = std::fs::read_to_string(sys_dev.join("uevent")) else {
            continue;
        };
        if !uevent_matches_steam_deck(&uevent) {
            continue;
        }
        let Ok(iface) = std::fs::read_to_string(sys_dev.join("..").join("bInterfaceNumber")) else {
            missing_iface = true;
            continue;
        };
        saw_iface = true;
        if is_controller_interface(&iface) {
            let dev_path = PathBuf::from("/dev").join(&name);
            if open_readonly(&dev_path).is_some() {
                return Some(dev_path);
            }
        }
    }
    if saw_iface && !missing_iface {
        return None;
    }
    for name in sorted_hidraw_names(std::path::Path::new("/dev")) {
        let dev_path = PathBuf::from("/dev").join(&name);
        let Some(fd) = open_readonly(&dev_path) else {
            continue;
        };
        let mut info = HidrawDevinfo {
            bustype: 0,
            vendor: 0,
            product: 0,
        };
        // SAFETY: `fd.0` is an open hidraw fd and `info` matches `struct hidraw_devinfo`.
        let rc = unsafe { libc::ioctl(fd.0, HIDIOCGRAWINFO, &mut info) };
        if rc < 0
            || (info.vendor as u16, info.product as u16) != (VALVE_VENDOR_ID, STEAM_DECK_PRODUCT_ID)
        {
            continue;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(30);
        let mut buf = [0u8; 128];
        while let Some(rem) = deadline.checked_duration_since(std::time::Instant::now()) {
            let ms = rem.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
            match poll_and_drain(&fd, ms, &mut buf, |p| parse_deck_state_report(p).is_some()) {
                Some(true) => return Some(dev_path),
                Some(false) => continue,
                None => break,
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn run_reader(device_path: PathBuf, shared: Arc<DeckImuShared>, input: InputHandle) {
    let Some(fd) = open_readonly(&device_path) else {
        eprintln!(
            "nightjar: Steam Deck IMU: could not open {}",
            device_path.display()
        );
        return;
    };
    let (mut accel_gate, mut gyro_gate) = (RateGate::default(), RateGate::default());
    let mut buf = [0u8; 128];
    while !shared.stop.load(Ordering::Acquire) {
        let cont = poll_and_drain(&fd, 50, &mut buf, |pkt| {
            if shared.stop.load(Ordering::Acquire) {
                return true;
            }
            if let Some(m) = shared.prepare_motion(
                pkt,
                nightjar_core::session::now_us(),
                &mut accel_gate,
                &mut gyro_gate,
            ) {
                input.input(|q| send_motion(q, m));
            }
            false
        });
        if !matches!(cont, Some(false)) {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
