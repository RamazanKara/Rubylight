//! Touchpad passthrough (the "Touchpad gestures on the PC" setting): the touchpad's raw
//! multi-finger contacts go to the PC as Foundation Sunshine's touchpad packets instead of the
//! pointer moves and scrolls the system makes of them, so the PC sees the fingers. Only a host
//! that offers the packets gets them; elsewhere the touchpad stays a mouse. The contacts come
//! from Raw Input on Windows, evdev on Linux and the trackpad's touches on macOS; the shared
//! [`Tracker`] turns their frames into what the host gets.
//!
//! Ported from Moonlight V+ PC: streaming/input/wintouchpad.cpp (Raw Input HID reports to
//! hardware frames, [`Assembler`] and `raw`).

use std::collections::BTreeMap;
use std::time::Instant;

pub use nightjar_core::input::touchpad::{Frame, RawContact, Tracker, Transport};
use nightjar_core::input::packets::touchpad;
use sdl3_sys::everything::*;

#[cfg_attr(not(windows), allow(dead_code))]
const MAX_CONTACTS: usize = touchpad::MAX_FRAME_CONTACTS;

/// What a touchpad source saw since the last pump.
/// macOS never removes its trackpad.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub enum Event {
    Frame(Frame),
    /// The touchpad with this device number went away.
    Removed(usize),
}

/// What one HID input report carried.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Report {
    pub contact_count: Option<u32>,
    pub scan_time: Option<u32>,
    pub button: Option<bool>,
    pub contacts: Vec<RawContact>,
}

#[cfg_attr(not(windows), allow(dead_code))]
struct Pending {
    count: u32,
    scan: Option<u32>,
    contacts: BTreeMap<u32, RawContact>,
}

/// Gathers a touchpad's reports into frames (`consumeReport`): a frame announces its contact
/// count, and hybrid or parallel reports bring the contacts until that many are in.
#[derive(Default)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Assembler {
    pending: Option<Pending>,
    completed_scan: Option<u32>,
    button: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Assembler {
    /// The frame `report` completes, as (contacts, button), or the button alone when the
    /// report has only that.
    pub fn consume(&mut self, report: Report) -> Option<(Option<Vec<RawContact>>, bool)> {
        if let Some(button) = report.button {
            self.button = button;
        }
        let button_only = report.button.map(|_| (None, self.button));
        if report.contact_count.is_none() && self.pending.is_none() {
            return button_only;
        }
        let pending_scan = self.pending.as_ref().and_then(|p| p.scan);
        let same_pending_scan = self.pending.is_some()
            && (report.scan_time.is_none() || pending_scan.is_none() || report.scan_time == pending_scan);
        let same_completed_scan =
            self.pending.is_none() && report.scan_time.is_some() && report.scan_time == self.completed_scan;
        let zero_continuation = report.contact_count == Some(0) && same_pending_scan;
        // A report after the five-contact limit completed this scan; a real all-up frame has a
        // new scan time.
        if report.contact_count == Some(0) && same_completed_scan {
            return button_only;
        }
        let mut starts_new = self.pending.is_none() && report.contact_count.is_some();
        if report.contact_count.is_some() && report.scan_time.is_some() && pending_scan.is_some()
            && report.scan_time != pending_scan
        {
            starts_new = true;
        }
        if starts_new {
            self.pending = Some(Pending { count: report.contact_count.unwrap_or(0), scan: report.scan_time, contacts: BTreeMap::new() });
        } else if let (Some(count), false) = (report.contact_count, zero_continuation) {
            if let Some(p) = self.pending.as_mut() { p.count = count; }
        }
        let pending = self.pending.as_mut()?;
        let expected = (pending.count as usize).min(MAX_CONTACTS);
        for contact in report.contacts {
            if pending.contacts.len() >= expected {
                break;
            }
            // IDs are unique within a frame: an unused parallel-report slot (zeros, ID 0, tip
            // clear) must not replace the real first contact, which often has ID 0 too.
            pending.contacts.entry(contact.pointer_id).or_insert(contact);
        }
        if expected != 0 && pending.contacts.len() < expected {
            return None;
        }
        let pending = self.pending.take()?;
        self.completed_scan = pending.scan;
        Some((Some(pending.contacts.into_values().take(expected).collect()), self.button))
    }
}

/// Whether a mouse event comes from where the system's touchpad pointer comes from: SDL's
/// global mouse on Windows. Linux and macOS do not say which mouse a touchpad's pointer is,
/// so every mouse's events right after a frame are taken as the touchpad's (touch and pen
/// aside).
pub fn pointer_from_touchpad(which: SDL_MouseID) -> bool {
    if cfg!(windows) {
        which == SDL_MouseID(0)
    } else {
        !matches!(which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID)
    }
}

/// Where the contacts come from on this system.
#[cfg(windows)]
type Source = raw::RawTouchpad;
#[cfg(target_os = "linux")]
type Source = evdev::Evdev;
#[cfg(target_os = "macos")]
type Source = trackpad::Trackpad;

/// The stream's touchpad passthrough: off until the first pump with the setting on.
#[derive(Default)]
pub struct Touchpad {
    pub tracker: Tracker,
    transport: Option<Transport>,
    started: bool,
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    source: Option<Source>,
}

impl super::Streaming {
    /// Sends what the touchpad did since the last call; called after each round of events.
    pub fn pump_touchpad(&mut self, window: *mut SDL_Window) {
        if !self.settings.native_touchpad {
            return;
        }
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
        {
            if !self.touchpad.started {
                self.touchpad.started = true;
                let transport = Transport::of(self.session.input(|q| q.feature_flags()));
                match transport {
                    None => eprintln!("nightjar: touchpad gestures stay a mouse: the PC takes no touchpad contacts"),
                    Some(transport) => match Source::install(window) {
                        Ok(source) => {
                            eprintln!("nightjar: touchpad contacts go to the PC ({transport:?} packets)");
                            self.touchpad.transport = Some(transport);
                            self.touchpad.source = Some(source);
                            self.direct_motion.touchpad_contacts();
                        }
                        Err(e) => {
                            eprintln!("nightjar: touchpad gestures stay a mouse: {e}");
                            self.show_toast(crate::strings::trf("The touchpad stays a mouse: {}", &[&e]));
                        }
                    },
                }
            }
            let (Some(transport), Some(source)) = (self.touchpad.transport, self.touchpad.source.as_mut()) else { return };
            let events = source.take();
            let live = self.captured && self.focused && !self.absolute;
            let now = Instant::now();
            let tracker = &mut self.touchpad.tracker;
            let mut outs = Vec::new();
            if !live {
                outs = tracker.cancel();
            } else {
                for event in events {
                    match event {
                        Event::Frame(frame) => outs.extend(tracker.frame(frame, now)),
                        Event::Removed(device) if device == tracker.device() => outs.extend(tracker.cancel()),
                        Event::Removed(_) => {}
                    }
                }
            }
            if !outs.is_empty() {
                let _ = self.session.input(|q| transport.send(outs, q));
            }
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = window;
            if !self.touchpad.started {
                self.touchpad.started = true;
                eprintln!("nightjar: touchpad gestures stay a mouse on this system");
            }
        }
    }

    /// Whether an event is the touchpad again, already sent as contacts: the pointer moves,
    /// scrolls and clicks the system makes of it and, on macOS, the trackpad's touches.
    pub fn touchpad_swallows(&mut self, event: &SDL_Event) -> bool {
        if self.touchpad.transport.is_none() {
            return false;
        }
        let now = Instant::now();
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            #[cfg(target_os = "macos")]
            if let Some(source) = self.touchpad.source.as_mut() {
                if source.finger(event) {
                    return true;
                }
                if matches!(kind, SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP)
                    && i32::from(event.button.button) == SDL_BUTTON_LEFT
                    && (self.touchpad.tracker.touching() || source.touching())
                {
                    // The trackpad's click comes as the mouse's: it rides on the frames.
                    source.button(event.button.down);
                }
            }
            match kind {
                SDL_EVENT_MOUSE_MOTION => self.touchpad.tracker.swallows_motion(pointer_from_touchpad(event.motion.which), now),
                SDL_EVENT_MOUSE_WHEEL => self.touchpad.tracker.swallows_motion(pointer_from_touchpad(event.wheel.which), now),
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let b = event.button;
                    self.touchpad.tracker.swallows_button(pointer_from_touchpad(b.which), b.button, b.down, self.captured, now)
                }
                _ => false,
            }
        }
    }
}

/// A touchpad axis' physical length in millimetres from its HID unit (`physicalDimensionMm`):
/// centimetres or inches, else 0 for unknown.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn physical_mm(physical_min: i32, physical_max: i32, units: u32, units_exp: u32) -> u16 {
    let extent = i64::from(physical_max) - i64::from(physical_min);
    let system = units & 0xF;
    let length_exponent = (units >> 4) & 0xF;
    if extent <= 0 || !(system == 1 || system == 3) || length_exponent != 1 || units & !0xFF != 0 {
        return 0;
    }
    let mut exponent = (units_exp & 0xF) as i32;
    if exponent & 0x8 != 0 {
        exponent -= 16;
    }
    let per_unit = if system == 1 { 10.0 } else { 25.4 };
    let mm = extent as f64 * 10f64.powi(exponent) * per_unit;
    if !mm.is_finite() || mm <= 0.0 {
        return 0;
    }
    mm.clamp(1.0, f64::from(u16::MAX)).round() as u16
}

/// A HID value on its logical range as 0-1.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn normalize(value: u32, min: i32, max: i32) -> f32 {
    if max <= min {
        return 0.0;
    }
    ((f64::from(value) - f64::from(min)) / (f64::from(max) - f64::from(min))).clamp(0.0, 1.0) as f32
}

#[cfg(target_os = "linux")]
mod evdev;
#[cfg(target_os = "macos")]
mod trackpad;

#[cfg(windows)]
mod raw {
    //! Raw Input from the touchpad's HID digitizer collection, read in SDL's message hook.

    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};

    use sdl3_sys::everything::*;
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::UI::Input::*;
    use windows::Win32::UI::WindowsAndMessaging::{GIDC_REMOVAL, MSG, WM_INPUT, WM_INPUT_DEVICE_CHANGE};

    use super::{normalize, physical_mm, Assembler, Event, Frame, RawContact, Report};

    const USAGE_CONTACT_IDENTIFIER: u16 = 0x51;
    const USAGE_CONTACT_COUNT: u16 = 0x54;
    const USAGE_SCAN_TIME: u16 = 0x56;
    const USAGE_CONFIDENCE: u16 = 0x47;
    const USAGE_BUTTON_1: u16 = 0x01;

    struct Device {
        preparsed: Vec<u8>,
        buttons: Vec<HIDP_BUTTON_CAPS>,
        values: Vec<HIDP_VALUE_CAPS>,
        width_mm: u16,
        height_mm: u16,
        assembler: Assembler,
    }

    impl Device {
        fn preparsed(&self) -> PHIDP_PREPARSED_DATA {
            PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
        }
    }

    struct Hook {
        hwnd: HWND,
        /// Failures too, so a device that is not a touchpad is not queried on every report.
        devices: HashMap<usize, Option<Device>>,
        events: Vec<Event>,
    }

    pub struct RawTouchpad {
        hook: Box<RefCell<Hook>>,
    }

    fn device_spec(flags: RAWINPUTDEVICE_FLAGS, hwnd: HWND) -> RAWINPUTDEVICE {
        RAWINPUTDEVICE { usUsagePage: HID_USAGE_PAGE_DIGITIZER, usUsage: HID_USAGE_DIGITIZER_TOUCH_PAD, dwFlags: flags, hwndTarget: hwnd }
    }

    impl RawTouchpad {
        pub fn install(window: *mut SDL_Window) -> Result<RawTouchpad, String> {
            // SAFETY: the window lives as long as the stream; SDL's property of it.
            let hwnd = unsafe {
                SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut())
            };
            if hwnd.is_null() {
                return Err("the window has no HWND".into());
            }
            let hwnd = HWND(hwnd);
            // SAFETY: a plain registration for this window.
            unsafe { RegisterRawInputDevices(&[device_spec(RIDEV_DEVNOTIFY, hwnd)], size_of::<RAWINPUTDEVICE>() as u32) }
                .map_err(|e| format!("touchpad Raw Input registration failed: {e}"))?;
            let hook = Box::new(RefCell::new(Hook { hwnd, devices: HashMap::new(), events: Vec::new() }));
            // SAFETY: the box outlives the hook, which Drop removes first.
            unsafe { SDL_SetWindowsMessageHook(Some(message_hook), &*hook as *const RefCell<Hook> as *mut _) };
            Ok(RawTouchpad { hook })
        }

        pub fn take(&self) -> Vec<Event> {
            std::mem::take(&mut self.hook.borrow_mut().events)
        }
    }

    impl Drop for RawTouchpad {
        fn drop(&mut self) {
            // SAFETY: undoes install.
            unsafe {
                SDL_SetWindowsMessageHook(None, std::ptr::null_mut());
                if let Err(e) = RegisterRawInputDevices(&[device_spec(RIDEV_REMOVE, HWND::default())], size_of::<RAWINPUTDEVICE>() as u32) {
                    eprintln!("nightjar: touchpad Raw Input removal failed: {e}");
                }
            }
        }
    }

    /// SDL calls this for every message it takes off the queue, on the main thread; returning
    /// true lets SDL handle the message as usual.
    unsafe extern "C" fn message_hook(userdata: *mut std::ffi::c_void, msg: *mut sdl3_sys::everything::MSG) -> bool {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: install's box, alive while the hook is set; SDL's message.
            let (hook, msg) = unsafe { (&*(userdata as *const RefCell<Hook>), &*(msg as *const MSG)) };
            let Ok(mut hook) = hook.try_borrow_mut() else { return };
            if msg.hwnd != hook.hwnd {
                return;
            }
            match msg.message {
                WM_INPUT => hook.raw_input(HRAWINPUT(msg.lParam.0 as *mut _)),
                WM_INPUT_DEVICE_CHANGE => {
                    let device = msg.lParam.0 as usize;
                    // A handle may come back for another device: forget what it was.
                    hook.devices.remove(&device);
                    // Arrival needs nothing more: the next report opens the device anew.
                    if msg.wParam.0 as u32 == GIDC_REMOVAL {
                        hook.events.push(Event::Removed(device));
                    }
                }
                _ => {}
            }
        }));
        true
    }

    fn button_has(cap: &HIDP_BUTTON_CAPS, usage: u16) -> bool {
        // SAFETY: the union member IsRange names.
        unsafe {
            if cap.IsRange {
                (cap.Anonymous.Range.UsageMin..=cap.Anonymous.Range.UsageMax).contains(&usage)
            } else {
                cap.Anonymous.NotRange.Usage == usage
            }
        }
    }

    fn value_has(cap: &HIDP_VALUE_CAPS, usage: u16) -> bool {
        // SAFETY: the union member IsRange names.
        unsafe {
            if cap.IsRange {
                (cap.Anonymous.Range.UsageMin..=cap.Anonymous.Range.UsageMax).contains(&usage)
            } else {
                cap.Anonymous.NotRange.Usage == usage
            }
        }
    }

    fn open(handle: HANDLE) -> Option<Device> {
        // SAFETY: Raw Input queries into buffers of the sizes they report.
        unsafe {
            let mut size = 0u32;
            if GetRawInputDeviceInfoW(Some(handle), RIDI_PREPARSEDDATA, None, &mut size) == u32::MAX || size == 0 {
                return None;
            }
            let mut preparsed = vec![0u8; size as usize];
            let mut copied_size = size;
            let copied = GetRawInputDeviceInfoW(Some(handle), RIDI_PREPARSEDDATA, Some(preparsed.as_mut_ptr().cast()), &mut copied_size);
            if copied == u32::MAX || copied == 0 || copied as usize > preparsed.len() {
                return None;
            }
            preparsed.truncate(copied as usize);
            let data = PHIDP_PREPARSED_DATA(preparsed.as_ptr() as isize);
            let mut caps = HIDP_CAPS::default();
            if HidP_GetCaps(data, &mut caps) != HIDP_STATUS_SUCCESS
                || caps.UsagePage != HID_USAGE_PAGE_DIGITIZER || caps.Usage != HID_USAGE_DIGITIZER_TOUCH_PAD
            {
                return None;
            }
            let mut buttons = vec![HIDP_BUTTON_CAPS::default(); usize::from(caps.NumberInputButtonCaps)];
            if !buttons.is_empty() {
                let mut length = caps.NumberInputButtonCaps;
                if HidP_GetButtonCaps(HidP_Input, buttons.as_mut_ptr(), &mut length, data) != HIDP_STATUS_SUCCESS {
                    return None;
                }
                buttons.truncate(usize::from(length));
            }
            let mut length = caps.NumberInputValueCaps;
            if length == 0 {
                return None;
            }
            let mut values = vec![HIDP_VALUE_CAPS::default(); usize::from(length)];
            if HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut length, data) != HIDP_STATUS_SUCCESS {
                return None;
            }
            values.truncate(usize::from(length));
            let (mut width_mm, mut height_mm) = (0, 0);
            for cap in values.iter().filter(|c| c.UsagePage == HID_USAGE_PAGE_GENERIC && c.LinkCollection != 0) {
                if width_mm == 0 && value_has(cap, HID_USAGE_GENERIC_X) {
                    width_mm = physical_mm(cap.PhysicalMin, cap.PhysicalMax, cap.Units, cap.UnitsExp);
                }
                if height_mm == 0 && value_has(cap, HID_USAGE_GENERIC_Y) {
                    height_mm = physical_mm(cap.PhysicalMin, cap.PhysicalMax, cap.Units, cap.UnitsExp);
                }
            }
            Some(Device { preparsed, buttons, values, width_mm, height_mm, assembler: Assembler::default() })
        }
    }

    #[derive(Default)]
    struct Partial {
        tip: Option<bool>,
        confidence: Option<bool>,
        id: Option<u32>,
        x: Option<f32>,
        y: Option<f32>,
        pressure: Option<f32>,
    }

    fn parse(device: &Device, report: &mut [u8]) -> Option<Report> {
        let mut parsed = Report::default();
        let mut partials: BTreeMap<u16, Partial> = BTreeMap::new();
        let mut any = false;
        let report_id = report.first().copied();
        for cap in &device.buttons {
            if cap.ReportID != 0 && report_id != Some(cap.ReportID) {
                continue;
            }
            let is_button = cap.UsagePage == HID_USAGE_PAGE_BUTTON && button_has(cap, USAGE_BUTTON_1);
            let is_tip = cap.UsagePage == HID_USAGE_PAGE_DIGITIZER && cap.LinkCollection != 0 && button_has(cap, HID_USAGE_DIGITIZER_TIP_SWITCH);
            let is_confidence = cap.UsagePage == HID_USAGE_PAGE_DIGITIZER && cap.LinkCollection != 0 && button_has(cap, USAGE_CONFIDENCE);
            if !is_button && !is_tip && !is_confidence {
                continue;
            }
            let mut usages = [0u16; 16];
            let mut count = usages.len() as u32;
            // SAFETY: the device's own preparsed data and report.
            let status = unsafe { HidP_GetUsages(HidP_Input, cap.UsagePage, Some(cap.LinkCollection), usages.as_mut_ptr(), &mut count, device.preparsed(), report) };
            if status != HIDP_STATUS_SUCCESS {
                continue;
            }
            let pressed = |usage: u16| usages[..count as usize].contains(&usage);
            any = true;
            if is_button {
                parsed.button = Some(pressed(USAGE_BUTTON_1));
            }
            if is_tip {
                partials.entry(cap.LinkCollection).or_default().tip = Some(pressed(HID_USAGE_DIGITIZER_TIP_SWITCH));
            }
            if is_confidence {
                partials.entry(cap.LinkCollection).or_default().confidence = Some(pressed(USAGE_CONFIDENCE));
            }
        }
        for cap in &device.values {
            if cap.ReportID != 0 && report_id != Some(cap.ReportID) {
                continue;
            }
            let usages: &[u16] = match (cap.UsagePage, cap.LinkCollection) {
                (HID_USAGE_PAGE_DIGITIZER, 0) => &[USAGE_CONTACT_COUNT, USAGE_SCAN_TIME],
                (HID_USAGE_PAGE_DIGITIZER, _) => &[USAGE_CONTACT_IDENTIFIER, HID_USAGE_DIGITIZER_TIP_PRESSURE],
                (HID_USAGE_PAGE_GENERIC, lc) if lc != 0 => &[HID_USAGE_GENERIC_X, HID_USAGE_GENERIC_Y],
                _ => &[],
            };
            for &usage in usages.iter().filter(|u| value_has(cap, **u)) {
                let mut value = 0u32;
                // SAFETY: the device's own preparsed data and report.
                let status = unsafe { HidP_GetUsageValue(HidP_Input, cap.UsagePage, Some(cap.LinkCollection), usage, &mut value, device.preparsed(), report) };
                if status != HIDP_STATUS_SUCCESS {
                    continue;
                }
                any = true;
                let unit = normalize(value, cap.LogicalMin, cap.LogicalMax);
                match (cap.UsagePage, usage) {
                    (HID_USAGE_PAGE_DIGITIZER, USAGE_CONTACT_COUNT) => parsed.contact_count = Some(value),
                    (HID_USAGE_PAGE_DIGITIZER, USAGE_SCAN_TIME) => parsed.scan_time = Some(value),
                    (HID_USAGE_PAGE_DIGITIZER, USAGE_CONTACT_IDENTIFIER) => partials.entry(cap.LinkCollection).or_default().id = Some(value),
                    (HID_USAGE_PAGE_DIGITIZER, _) => partials.entry(cap.LinkCollection).or_default().pressure = Some(unit),
                    (_, HID_USAGE_GENERIC_X) => partials.entry(cap.LinkCollection).or_default().x = Some(unit),
                    _ => partials.entry(cap.LinkCollection).or_default().y = Some(unit),
                }
            }
        }
        for p in partials.into_values() {
            let (Some(pointer_id), Some(x), Some(y)) = (p.id, p.x, p.y) else { continue };
            // A contact without tip or confidence still counts towards the frame, so it is
            // sent lifted.
            let touching = p.tip.unwrap_or(true) && p.confidence.unwrap_or(true);
            parsed.contacts.push(RawContact { pointer_id, x, y, pressure: p.pressure.unwrap_or(0.0), touching });
        }
        any.then_some(parsed)
    }

    impl Hook {
        fn raw_input(&mut self, input: HRAWINPUT) {
            // SAFETY: Raw Input reads into buffers of the sizes it reports.
            unsafe {
                let mut header = RAWINPUTHEADER::default();
                let mut size = size_of::<RAWINPUTHEADER>() as u32;
                let header_size = size;
                if GetRawInputData(input, RID_HEADER, Some((&mut header as *mut RAWINPUTHEADER).cast()), &mut size, header_size) != header_size
                    || header.dwType != RIM_TYPEHID.0
                {
                    return;
                }
                let key = header.hDevice.0 as usize;
                let device = self.devices.entry(key).or_insert_with(|| open(header.hDevice));
                let Some(device) = device.as_mut() else { return };
                let mut size = 0u32;
                if GetRawInputData(input, RID_INPUT, None, &mut size, header_size) != 0 || size == 0 {
                    return;
                }
                let mut data = vec![0u64; (size as usize).div_ceil(8)];
                let mut copied_size = size;
                let copied = GetRawInputData(input, RID_INPUT, Some(data.as_mut_ptr().cast()), &mut copied_size, header_size);
                if copied == u32::MAX || copied != copied_size {
                    return;
                }
                let offset = std::mem::offset_of!(RAWINPUT, data) + std::mem::offset_of!(RAWHID, bRawData);
                if (copied as usize) < offset {
                    return;
                }
                let raw = &*(data.as_ptr() as *const RAWINPUT);
                let (report_size, count) = (raw.data.hid.dwSizeHid as usize, raw.data.hid.dwCount as usize);
                if raw.header.dwType != RIM_TYPEHID.0 || report_size == 0 || count == 0 || count > (copied as usize - offset) / report_size {
                    return;
                }
                let bytes = std::slice::from_raw_parts(data.as_ptr().cast::<u8>().add(offset), report_size * count);
                for chunk in bytes.chunks_exact(report_size) {
                    let mut report = chunk.to_vec();
                    let Some(parsed) = parse(device, &mut report) else { continue };
                    if let Some((contacts, button)) = device.assembler.consume(parsed) {
                        self.events.push(Event::Frame(Frame { device: key, contacts, button, width_mm: device.width_mm, height_mm: device.height_mm }));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: u32, x: f32, touching: bool) -> RawContact {
        RawContact { pointer_id: id, x, y: 0.5, pressure: 0.0, touching }
    }

    fn report(count: Option<u32>, scan: u32, contacts: Vec<RawContact>) -> Report {
        Report { contact_count: count, scan_time: Some(scan), button: Some(false), contacts }
    }

    #[test]
    fn hybrid_reports_gather_into_one_frame() {
        let mut a = Assembler::default();
        // Three fingers, two in the first report and one in the next of the same scan.
        assert_eq!(a.consume(report(Some(3), 10, vec![contact(0, 0.1, true), contact(1, 0.2, true)])), None);
        let (contacts, button) = a.consume(report(Some(0), 10, vec![contact(2, 0.3, true), contact(0, 0.9, false)])).unwrap();
        assert_eq!(contacts.unwrap().iter().map(|c| (c.pointer_id, c.x)).collect::<Vec<_>>(), [(0, 0.1), (1, 0.2), (2, 0.3)]);
        assert!(!button);
        // A late zero-count report of the completed scan is only the button.
        assert_eq!(a.consume(report(Some(0), 10, vec![])), Some((None, false)));
        // All fingers up is a frame of its own, with a new scan time.
        assert_eq!(a.consume(report(Some(0), 11, vec![])), Some((Some(vec![]), false)));
    }

    #[test]
    fn a_parallel_report_keeps_five_contacts_and_the_first_of_each_id() {
        let mut a = Assembler::default();
        let mut contacts: Vec<_> = (0..6).map(|i| contact(i, 0.1, true)).collect();
        // The unused slot of a parallel report: ID 0, tip clear.
        contacts.insert(1, contact(0, 0.0, false));
        let (frame, _) = a.consume(report(Some(6), 1, contacts)).unwrap();
        let frame = frame.unwrap();
        assert_eq!(frame.len(), 5);
        assert!(frame[0].touching && frame[0].x == 0.1);
    }

    #[test]
    fn hid_units_give_the_touchpad_size() {
        // 0-1050 in 10^-2 cm (SI linear, length^1): 105 mm.
        assert_eq!(physical_mm(0, 1050, 0x11, 0xE), 105);
        // 0-400 in 10^-2 inch (English linear): 101.6 mm.
        assert_eq!(physical_mm(0, 400, 0x13, 0xE), 102);
        assert_eq!(physical_mm(0, 400, 0x00, 0xE), 0);
        assert_eq!(normalize(50, 0, 100), 0.5);
        assert_eq!(normalize(500, 0, 100), 1.0);
        assert_eq!(normalize(5, 10, 10), 0.0);
    }
}
