//! Input event dispatch and shared per-session injection state.

use anyhow::{Context, Result, bail};
use butterpollo_core::input::Input as Event;
use butterpollo_core::input_policy::{VHF_AUTO, gamepad_profile};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::*,
        Foundation::*,
        Storage::FileSystem::*,
        System::StationsAndDesktops::*,
        UI::{
            Controls::*,
            Input::{KeyboardAndMouse::*, Pointer::*},
            WindowsAndMessaging::*,
        },
    },
    core::{GUID, PCWSTR},
};

mod desktop;
mod dualsense_pad;
mod gamepad_backend;
#[path = "input/gamepad_thread.rs"]
mod gamepad_thread;
mod gamepads;
mod keyboard_mouse;
mod steam_deck_pad;
mod touch_pen;
mod usbip_pad;

pub use desktop::{
    follow_input_desktop, input_desktop_name, keep_on_input_desktop, on_input_desktop,
    secure_desktop_shown,
};
pub use gamepad_thread::{GamepadThread, PadReport};
pub use gamepads::{Gamepads, open_interface};
use keyboard_mouse::{
    EXPLICIT_EXTENDED_KEY, inject, is_modifier, keyboard_identity, mapped_keyboard_identity,
};
use touch_pen::{inject_pointer, refresh_due};
pub use usbip_pad::UsbipDriver;

static HELD: std::sync::Mutex<BTreeMap<(bool, u32), usize>> =
    std::sync::Mutex::new(BTreeMap::new());

pub fn capabilities(config: &butterpollo_core::config::Config) -> u32 {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let mut flags = 0;
    if config.boolean("mouse", true)
        && config.boolean("native_pen_touch", true)
        // SAFETY: The module and symbol names are terminated static strings; the returned address is only checked for presence.
        && unsafe {
            GetModuleHandleW(windows::core::w!("user32.dll")).is_ok_and(|module| {
                GetProcAddress(module, windows::core::s!("CreateSyntheticPointerDevice")).is_some()
            })
        }
    {
        flags |= 1;
    }
    if config.boolean("controller", true)
        && !matches!(
            config.get("gamepad", "auto"),
            "vhf_xbox" | "vhf_xbox_one" | "vhf_switch"
        )
    {
        // Moonlight has one general controller-touch feature bit; there is
        // no separate host bit for dual touchpads. The virtual DualShock 4 and
        // DualSense have one touch surface, so a client's second touchpad
        // (a Steam Deck's right trackpad) is placed on its right half; apply()
        // ignores any other touchpad.
        flags |= 2;
    }
    flags
}
pub struct Injector {
    keys: BTreeSet<u32>,
    key_sources: BTreeMap<u32, u32>,
    buttons: BTreeSet<u8>,
    touches: BTreeMap<u32, POINTER_TOUCH_INFO>,
    /// Touches on their way to mouse input, with a mouse or trackpad touch profile.
    touch_profile: butterpollo_core::touch_profile::TouchProfile,
    touch_device: Option<HSYNTHETICPOINTERDEVICE>,
    pen_device: Option<HSYNTHETICPOINTERDEVICE>,
    pen: POINTER_PEN_INFO,
    /// When touch contacts and the pen were last injected.
    touch_refreshed: std::time::Instant,
    pen_refreshed: std::time::Instant,
    gamepads: std::sync::Arc<GamepadThread>,
    /// The display's desktop rectangle, None until the display exists.
    rect: Option<RECT>,
    policy: butterpollo_core::input_policy::Policy,
    /// The layout that gives keys the client did not normalize their scan codes (None:
    /// the host's), and the one asked for since. It takes over once no key is held, so
    /// a release sends its press's scan code.
    layout: Option<crate::keylayout::Layout>,
    next_layout: Option<crate::keylayout::Layout>,
    key_flags: BTreeMap<u32, u8>,
    /// Repeating key, its flags, the modifiers pressed around it at key-down
    /// and when it repeats.
    repeat: Option<(u32, u8, u8, std::time::Instant)>,
    scroll: [i32; 2],
    haptics: bool,
    /// The display absolute input maps onto, and when its rectangle was read.
    output: String,
    /// The stream's size: clients place absolute input on the whole stream,
    /// including the bars around a display of another shape.
    stream: Option<(u32, u32)>,
    rect_read: std::time::Instant,
    lookup: Option<Lookup>,
    /// When absolute input was last reported ignored for a missing display.
    display_warned: Option<std::time::Instant>,
    /// Whether the client last moved the mouse by absolute position, and a
    /// left-button release held back meanwhile.
    absolute: bool,
    left_release: Option<std::time::Instant>,
    pending: Pending,
    /// SendInput, or a stand-in in tests.
    inject: fn(&[INPUT]) -> usize,
}
/// Keyboard and mouse input of one pass of the control loop, sent in one
/// SendInput call: a call cost 31-36 us whether it carried one input or
/// eight.
#[derive(Default)]
struct Pending {
    inputs: Vec<INPUT>,
    /// Each event's end in `inputs`, and its press if it is one.
    events: Vec<(usize, Option<Press>)>,
    /// Why events of the pass failed.
    errors: Vec<anyhow::Error>,
}
/// A display looked up on its own thread.
struct Lookup {
    output: String,
    started: std::time::Instant,
    /// None once its result was used.
    thread: Option<std::thread::JoinHandle<Result<RECT>>>,
}
/// A key or button press, recorded as held once Windows takes it.
struct Press {
    keyboard: bool,
    id: u32,
    /// Unmapped keyboard identity; unused for mouse presses.
    source: u32,
    flags: u8,
    /// Whether it was counted in HELD, to count it out if Windows refuses it.
    counted: bool,
    /// The modifiers pressed around a key that repeats.
    repeat: Option<u8>,
}
/// How long a left release waits after absolute input, as in Sunshine.
const LEFT_RELEASE_DELAY: std::time::Duration = std::time::Duration::from_millis(10);
/// How often a display's rectangle is read again, and a missing display
/// looked up again: the lookup enumerates every display.
const RECT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// How often absolute input ignored for a missing display is reported.
const MISSING_DISPLAY_WARNING: std::time::Duration = std::time::Duration::from_secs(10);
/// The desktop rectangle of `output`: a GDI name, a device ID, or empty for
/// the first display.
fn display_rect(output: &str) -> Result<RECT> {
    // Before capture publishes its GDI name, the configured display is
    // often a device ID ({...}); input arriving then was dropped.
    let name = crate::display::monitors()
        .ok()
        .and_then(|all| all.into_iter().find(|m| m.matches(output)))
        .map_or_else(|| output.to_owned(), |m| m.display_name);
    let d = crate::capture::displays()?
        .into_iter()
        .find(|d| output.is_empty() || d.display_name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| anyhow::anyhow!("input display missing"))?;
    Ok(RECT {
        left: d.x,
        top: d.y,
        right: d.x + d.width as i32,
        bottom: d.y + d.height as i32,
    })
}
/// The current rectangle of a display by GDI name, read cheaply from its mode.
fn current_rect(name: &str) -> Option<RECT> {
    if !name.starts_with(r"\\.\") {
        return None;
    }
    let mode = crate::display::mode(name).ok()?;
    // SAFETY: display::mode returns a display DEVMODEW whose display position union field is initialized.
    let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
    Some(RECT {
        left: position.x,
        top: position.y,
        right: position.x + mode.dmPelsWidth as i32,
        bottom: position.y + mode.dmPelsHeight as i32,
    })
}
/// A width and height, and a rectangle (left, top, width, height) in it.
type Size = (u32, u32);
type Picture = (u32, u32, u32, u32);
/// A desktop point as fractions of the stream, when it is on the display at
/// `rect` whose picture sits at `picture` in the stream (None: it fills it).
fn cursor_in_stream(
    point: POINT,
    rect: RECT,
    picture: Option<(Size, Picture)>,
) -> Option<(f32, f32)> {
    let on = |value: i32, start: i32, end: i32| {
        (start < end && (start..end).contains(&value))
            .then(|| f64::from(value - start) / f64::from(end - start))
    };
    let (x, y) = (
        on(point.x, rect.left, rect.right)?,
        on(point.y, rect.top, rect.bottom)?,
    );
    let (x, y) = match picture {
        Some((stream, (left, top, width, height))) => (
            (f64::from(left) + x * f64::from(width)) / f64::from(stream.0),
            (f64::from(top) + y * f64::from(height)) / f64::from(stream.1),
        ),
        None => (x, y),
    };
    Some((x as f32, y as f32))
}
impl Injector {
    pub fn new(output: &str, profile: &str) -> Result<Self> {
        Self::new_options(
            output,
            profile,
            &butterpollo_core::config::Config::default(),
        )
    }
    pub fn new_options(
        output: &str,
        profile: &str,
        config: &butterpollo_core::config::Config,
    ) -> Result<Self> {
        Self::new_options_reported(output, profile, config, Default::default())
    }
    pub fn new_options_reported(
        output: &str,
        profile: &str,
        config: &butterpollo_core::config::Config,
        warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
    ) -> Result<Self> {
        let profile = gamepad_profile(profile);
        let policy = butterpollo_core::input_policy::Policy::resolve(config)?;
        // Keyboard, relative mouse and controllers must work before the
        // stream's display exists: a controller's arrival is sent only once.
        let mut injector = Self {
            keys: BTreeSet::new(),
            key_sources: BTreeMap::new(),
            buttons: BTreeSet::new(),
            touches: BTreeMap::new(),
            touch_profile: Default::default(),
            touch_device: None,
            pen_device: None,
            pen: Default::default(),
            touch_refreshed: std::time::Instant::now(),
            pen_refreshed: std::time::Instant::now(),
            gamepads: std::sync::Arc::new(GamepadThread::new(profile, policy.clone(), warnings)?),
            policy,
            layout: None,
            next_layout: None,
            key_flags: BTreeMap::new(),
            repeat: None,
            scroll: [0; 2],
            haptics: true,
            rect: None,
            output: output.to_owned(),
            stream: None,
            rect_read: std::time::Instant::now(),
            lookup: None,
            display_warned: None,
            absolute: false,
            left_release: None,
            pending: Pending::default(),
            inject,
        };
        injector.look_up(output);
        Ok(injector)
    }
    /// The keyboard layout keys the client did not normalize go with (None: the host's),
    /// read by the control loop from the device's report (0x553E) and `keyboard_layout`.
    /// It takes over at [`Self::follow_keyboard_layout`].
    pub fn set_keyboard_layout(&mut self, layout: Option<crate::keylayout::Layout>) {
        self.next_layout = layout;
    }
    /// Each pass: the layout set last takes over once no key is held, so a release sends
    /// its press's scan code.
    pub fn follow_keyboard_layout(&mut self) {
        let klid = |layout: &Option<crate::keylayout::Layout>| layout.as_ref().map(|l| l.klid());
        if self.keys.is_empty() && klid(&self.layout) != klid(&self.next_layout) {
            self.layout = self.next_layout.clone();
            tracing::info!(layout = ?self.layout, "keys follow a keyboard layout");
        }
    }
    /// The stream's size, so absolute input skips the bars around a display of
    /// another shape, as the encoder adds them.
    pub fn set_stream_size(&mut self, width: u32, height: u32) {
        self.stream = (width > 0 && height > 0).then_some((width, height));
    }
    /// Where the display's picture sits in the stream, when it does not fill it.
    fn picture(&self) -> Option<(Size, Picture)> {
        let stream = self.stream?;
        let rect = self.rect?;
        let display = (
            u32::try_from(rect.right - rect.left).ok()?,
            u32::try_from(rect.bottom - rect.top).ok()?,
        );
        butterpollo_core::display_policy::picture(display, stream).map(|picture| (stream, picture))
    }
    /// A point on the stream as fractions of the display.
    fn on_display(&self, x: f32, y: f32) -> (f32, f32) {
        match self.picture() {
            Some((stream, picture)) => {
                let (x, y) = butterpollo_core::display_policy::stream_to_picture(
                    (f64::from(x), f64::from(y)),
                    stream,
                    picture,
                );
                (x as f32, y as f32)
            }
            None => (x, y),
        }
    }
    /// The host cursor as fractions of the whole stream, bars included, or
    /// None off the stream's display or before it is known. The couch menu
    /// reads it only when a relative mouse's hold or click needs a position
    /// (docs/couch-menu-design.md 3.2).
    pub fn pointer_in_stream(&mut self) -> Option<(f32, f32)> {
        self.settle_lookup(false);
        let rect = self.rect?;
        let mut point = POINT::default();
        // SAFETY: `point` is a live POINT for the call to write.
        if let Err(error) = unsafe { GetCursorPos(&mut point) } {
            tracing::warn!(%error, "couch menu: the cursor position is unavailable");
            return None;
        }
        cursor_in_stream(point, rect, self.picture())
    }
    /// Map absolute input onto `output` from now on: the stream's display can
    /// be created, recreated or renamed after input began.
    pub fn set_output(&mut self, output: &str) {
        if output.is_empty() {
            return;
        }
        if self
            .lookup
            .as_ref()
            .is_some_and(|lookup| !lookup.output.eq_ignore_ascii_case(output))
        {
            self.lookup = None;
        }
        self.settle_lookup(false);
        let renamed = !output.eq_ignore_ascii_case(&self.output);
        if self.rect.is_none() {
            // Until the display exists, follow the session's name, and look
            // the same name up again only every RECT_INTERVAL.
            if renamed || self.rect_read.elapsed() >= RECT_INTERVAL {
                self.output = output.to_owned();
                self.resolve_rect();
            }
            return;
        }
        // The rectangle and the name change together once it is found; a
        // name not found is looked up again every RECT_INTERVAL.
        if renamed
            && self.lookup.as_ref().is_none_or(|lookup| {
                !lookup.output.eq_ignore_ascii_case(output)
                    || lookup.started.elapsed() >= RECT_INTERVAL
            })
        {
            self.look_up(output);
        }
    }
    /// Look up a display that did not exist yet.
    fn resolve_rect(&mut self) {
        self.rect_read = std::time::Instant::now();
        let output = self.output.clone();
        self.look_up(&output);
    }
    /// Look `output` up on another thread, unless it is being looked up: the
    /// lookup enumerates every display, which took 1.5-2.5 ms and once 18 ms
    /// on the input thread.
    fn look_up(&mut self, output: &str) {
        if self.lookup.as_ref().is_some_and(|lookup| {
            lookup.thread.is_some() && lookup.output.eq_ignore_ascii_case(output)
        }) {
            return;
        }
        let name = output.to_owned();
        let thread = std::thread::Builder::new()
            .name("display lookup".into())
            .spawn(move || display_rect(&name));
        self.lookup = Some(Lookup {
            output: output.to_owned(),
            started: std::time::Instant::now(),
            thread: None,
        });
        match thread {
            Ok(thread) => self.lookup.as_mut().unwrap().thread = Some(thread),
            Err(_) => self.found(output.to_owned(), display_rect(output)),
        }
    }
    /// Use the result of a lookup that finished; with `wait`, of one still
    /// running.
    fn settle_lookup(&mut self, wait: bool) {
        let Some(lookup) = &mut self.lookup else {
            return;
        };
        let Some(thread) = lookup.thread.take_if(|thread| wait || thread.is_finished()) else {
            return;
        };
        let output = lookup.output.clone();
        if let Ok(rect) = thread.join() {
            self.found(output, rect);
        }
    }
    fn found(&mut self, output: String, rect: Result<RECT>) {
        match rect {
            Ok(rect) => {
                tracing::debug!(output, "input display found");
                self.rect = Some(rect);
                self.output = output;
                self.rect_read = std::time::Instant::now();
            }
            Err(error) => tracing::debug!(%error, output, "input display unavailable"),
        }
    }
    /// The display's rectangle. Until the display exists, absolute mouse,
    /// touch and pen input is ignored, and that is reported now and then.
    fn display(&mut self) -> Option<RECT> {
        // Absolute input maps onto the display a running lookup is finding,
        // as when the lookup ran here; other input never waits for it.
        self.settle_lookup(true);
        if self.rect.is_none()
            && self
                .display_warned
                .is_none_or(|at| at.elapsed() >= MISSING_DISPLAY_WARNING)
        {
            self.display_warned = Some(std::time::Instant::now());
            tracing::warn!(
                output = %self.output,
                "input display not found yet; absolute mouse, touch and pen input is ignored"
            );
        }
        self.rect
    }
    pub fn refresh(&mut self) -> Result<()> {
        // Every step runs even when an earlier one fails; the first error is
        // reported. A failed step is not retried before its next due time.
        let mut first = None;
        let mut keep = |result: Result<()>| {
            if let Err(error) = result {
                first.get_or_insert(error);
            }
        };
        keep(self.due());
        let now = std::time::Instant::now();
        // A game can change its display's resolution or position mid-stream,
        // and the stream's display can appear after input began.
        self.settle_lookup(false);
        if self.rect_read.elapsed() >= RECT_INTERVAL {
            if self.rect.is_none() {
                self.resolve_rect();
            } else {
                self.rect_read = now;
                if let Some(rect) = current_rect(&self.output) {
                    self.rect = Some(rect);
                }
            }
        }
        if refresh_due(&mut self.touch_refreshed, now) {
            keep(self.inject_touches());
        }
        if refresh_due(&mut self.pen_refreshed, now)
            && self.pen.pointerInfo.pointerFlags != POINTER_FLAG_NONE
            && let Some(device) = self.pen_device
        {
            // SAFETY: The injector still owns the pen device and the borrowed frame is initialized as pen input.
            keep(unsafe {
                inject_pointer(
                    device,
                    &[POINTER_TYPE_INFO {
                        r#type: PT_PEN,
                        Anonymous: POINTER_TYPE_INFO_0 { penInfo: self.pen },
                    }],
                )
            });
        }
        first.map_or(Ok(()), Err)
    }
    pub fn apply(&mut self, e: &Event) -> Result<()> {
        let mut errors = self.apply_all(std::slice::from_ref(e));
        errors.pop().map_or(Ok(()), Err)
    }
    /// Apply a pass of events in order, with one SendInput call where the
    /// order allows; returns why events failed.
    pub fn apply_all(&mut self, events: &[Event]) -> Vec<anyhow::Error> {
        for e in events {
            if let Err(error) = self.stage(e) {
                self.pending.errors.push(error);
            }
        }
        self.flush();
        std::mem::take(&mut self.pending.errors)
    }
    /// Add an event's keyboard and mouse input to the pass, or apply it.
    fn stage(&mut self, e: &Event) -> Result<()> {
        if !self.policy.allows(e) {
            return Ok(());
        }
        use Event::*;
        if let Touch {
            event, id, x, y, ..
        } = *e
            && self.policy.touch.translates()
        {
            // Relative movement and scrolling are measured in stream pixels; the stream sets
            // its size before input, so 1080p stands in only for a touch that comes earlier.
            let size = self.stream.or_else(|| {
                let rect = self.rect?;
                Some((
                    u32::try_from(rect.right - rect.left).ok()?,
                    u32::try_from(rect.bottom - rect.top).ok()?,
                ))
            });
            let inputs = self.touch_profile.translate(
                &self.policy.touch,
                event,
                id,
                [x, y],
                size.unwrap_or((1920, 1080)),
                std::time::Instant::now(),
            );
            for input in &inputs {
                if let Err(error) = self.stage(input) {
                    self.pending.errors.push(error);
                }
            }
            return Ok(());
        }
        match e {
            // How a key or button is sent depends on the keys or buttons held,
            // including those pressed earlier in the pass.
            Keyboard { .. } if self.press_pending(true) => self.flush(),
            MouseButton { .. } if self.press_pending(false) => self.flush(),
            // Finish the absolute-mode click before relative input can press again.
            Relative { .. } if self.left_release.take().is_some() => {
                self.flush();
                if let Err(error) = self.held(false, 1, false, true, Self::button(1, false)) {
                    self.pending.errors.push(error);
                }
            }
            // Touch and pen go through another call: keep them in order.
            Touch { .. } | Pen { .. } => self.flush(),
            _ => {}
        }
        let start = self.pending.inputs.len();
        let press = self.stage_event(e)?;
        let end = self.pending.inputs.len();
        if end > start {
            self.pending.events.push((end, press));
        } else if let Some(press) = press {
            // Nothing to send, such as a key another client holds.
            self.hold(press);
        }
        Ok(())
    }
    fn stage_event(&mut self, e: &Event) -> Result<Option<Press>> {
        use Event::*;
        match e {
            Relative { x, y } => {
                self.absolute = false;
                self.pending.inputs.push(Self::mouse(
                    i32::from(*x),
                    i32::from(*y),
                    0,
                    MOUSEEVENTF_MOVE,
                ));
            }
            Absolute {
                x,
                y,
                width,
                height,
                // SAFETY: GetSystemMetrics uses valid metric constants and does not borrow any pointers.
            } => unsafe {
                self.absolute = true;
                let Some(rect) = self.display() else {
                    return Ok(None);
                };
                let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
                let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
                let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
                // The client's far edge is the display's last pixel, not the
                // first pixel of its neighbour.
                let (width, height) = (i64::from(*width).max(1), i64::from(*height).max(1));
                let (span_x, span_y) = (
                    i64::from((rect.right - rect.left - 1).max(0)),
                    i64::from((rect.bottom - rect.top - 1).max(0)),
                );
                let (px, py) = match self.picture() {
                    // The client's size is the stream's picture with its bars.
                    Some((stream, picture)) => {
                        let (fx, fy) = butterpollo_core::display_policy::stream_to_picture(
                            (
                                i64::from(*x).clamp(0, width) as f64 / width as f64,
                                i64::from(*y).clamp(0, height) as f64 / height as f64,
                            ),
                            stream,
                            picture,
                        );
                        (
                            rect.left + (fx * span_x as f64) as i32,
                            rect.top + (fy * span_y as f64) as i32,
                        )
                    }
                    None => (
                        rect.left + (i64::from(*x).clamp(0, width) * span_x / width) as i32,
                        rect.top + (i64::from(*y).clamp(0, height) * span_y / height) as i32,
                    ),
                };
                self.pending.inputs.push(Self::mouse(
                    ((i64::from(px - left) * 65535) / i64::from((w - 1).max(1))) as i32,
                    ((i64::from(py - top) * 65535) / i64::from((h - 1).max(1))) as i32,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                ));
            },
            MouseButton { button, down } => {
                // With absolute input (touch, pen, a tablet), Moonlight sends a
                // right press right after a left release for a long press: the
                // left click would land first and open a link. Hold the left
                // release back briefly and click right meanwhile, as Sunshine does.
                if *button == 1 && self.absolute {
                    if !*down && self.buttons.contains(button) {
                        self.buttons.remove(button);
                        self.left_release = Some(std::time::Instant::now() + LEFT_RELEASE_DELAY);
                        return Ok(None);
                    }
                    if *down && self.left_release.take().is_some() {
                        // Still down in Windows: its release never went out.
                        self.buttons.insert(*button);
                        return Ok(None);
                    }
                }
                if *button == 3 && *down && self.left_release.is_some() {
                    self.pending
                        .inputs
                        .extend([Self::button(3, true), Self::button(3, false)]);
                    return Ok(None);
                }
                let owned = self.buttons.contains(button);
                let inputs = &mut self.pending.inputs;
                Self::update_held(
                    &mut HELD.lock().unwrap(),
                    (false, u32::from(*button)),
                    *down,
                    owned,
                    Self::button(*button, *down),
                    |input| {
                        inputs.extend_from_slice(input);
                        Ok(())
                    },
                )?;
                if *down {
                    return Ok(Some(Press {
                        keyboard: false,
                        id: u32::from(*button),
                        source: 0,
                        flags: 0,
                        counted: !owned,
                        repeat: None,
                    }));
                }
                // A release is recorded even when Windows refuses it.
                self.buttons.remove(button);
            }
            Scroll { amount, horizontal } => {
                let index = usize::from(*horizontal);
                let amount = if self.policy.high_resolution_scrolling {
                    i32::from(*amount)
                } else {
                    self.scroll[index] += i32::from(*amount);
                    let amount = self.scroll[index] / 120 * 120;
                    self.scroll[index] -= amount;
                    amount
                };
                if amount == 0 {
                    return Ok(None);
                }
                self.pending.inputs.push(Self::mouse(
                    0,
                    0,
                    amount as u32,
                    if *horizontal {
                        MOUSEEVENTF_HWHEEL
                    } else {
                        MOUSEEVENTF_WHEEL
                    },
                ));
            }
            Keyboard {
                key,
                down,
                flags,
                modifiers,
            } => {
                let mut source = keyboard_identity(*key, *modifiers);
                // A client can set the extended-key bit on the press but not
                // on the release: release the key that is held, or it stays
                // down and keeps repeating.
                if !*down
                    && !self.key_sources.contains_key(&source)
                    && self
                        .key_sources
                        .contains_key(&(source ^ EXPLICIT_EXTENDED_KEY))
                {
                    source ^= EXPLICIT_EXTENDED_KEY;
                }
                let key = if *down {
                    if self.key_sources.contains_key(&source) {
                        return Ok(None);
                    }
                    let key = mapped_keyboard_identity(&self.policy, *key, *modifiers);
                    if self.keys.contains(&key) {
                        self.key_sources.insert(source, key);
                        return Ok(None);
                    }
                    key
                } else {
                    let Some(key) = self.key_sources.remove(&source) else {
                        return Ok(None);
                    };
                    // Two client keys may map to one host key. It stays down
                    // until the last source lets go, without restarting repeat.
                    if self.key_sources.values().any(|mapped| *mapped == key) {
                        return Ok(None);
                    }
                    key
                };
                let owned = self.keys.contains(&key);
                let flags = if *down {
                    *flags
                } else {
                    self.key_flags.get(&key).copied().unwrap_or(*flags)
                };
                let modifiers = if *down && !owned {
                    self.synthetic_modifiers(key, *modifiers)
                } else {
                    0
                };
                let input = self.key_scan(key, *down, flags);
                let always_send_scancodes = self.policy.always_send_scancodes;
                let inputs = &mut self.pending.inputs;
                if modifiers == 0 {
                    Self::update_held(
                        &mut HELD.lock().unwrap(),
                        (true, key),
                        *down,
                        owned,
                        input,
                        |input| {
                            inputs.extend_from_slice(input);
                            Ok(())
                        },
                    )?;
                } else {
                    // The client reported a modifier it never pressed as a
                    // key: press it around this key only, as Vibepollo does.
                    let mut held = HELD.lock().unwrap();
                    let count = held.get(&(true, key)).copied().unwrap_or(0);
                    if count == 0 {
                        inputs.extend(Self::with_modifiers(
                            input,
                            modifiers,
                            always_send_scancodes,
                        ));
                    }
                    held.insert((true, key), count + 1);
                }
                // A press Windows refuses is not held; a release is recorded
                // even when Windows refuses it, or the key keeps repeating.
                if *down {
                    return Ok(Some(Press {
                        keyboard: true,
                        id: key,
                        source,
                        flags,
                        counted: !owned,
                        repeat: (!owned && !is_modifier(key)).then_some(modifiers),
                    }));
                }
                self.keys.remove(&key);
                self.key_flags.remove(&key);
                if self.repeat.is_some_and(|(repeating, ..)| repeating == key) {
                    self.repeat = None;
                }
            }
            Text(s) => {
                for v in s.encode_utf16() {
                    self.pending
                        .inputs
                        .extend([Self::key(v, true, true), Self::key(v, false, true)]);
                }
            }
            Touch {
                event,
                id,
                x,
                y,
                pressure,
                major,
                minor,
                rotation,
            } => self.touch(*event, *id, *x, *y, *pressure, *major, *minor, *rotation)?,
            Pen {
                event,
                tool,
                buttons,
                x,
                y,
                pressure,
                rotation,
                tilt,
                roll,
            } => self.pen(
                *event, *tool, *buttons, *x, *y, *pressure, *rotation, *tilt, *roll,
            )?,
            Haptics(enabled) => {
                self.haptics = *enabled;
                self.gamepads.set_haptics(*enabled);
            }
            // Only the VHF Steam Controller profile takes them, and no pinned
            // driver has it yet.
            SteamAttributes(_) => {}
            Controller { .. }
            | Arrival { .. }
            | ControllerTouch { .. }
            | Motion { .. }
            | Battery { .. } => self.gamepads.send(e.clone()),
        }
        Ok(None)
    }
    /// Feedback and motion-sensor requests from the virtual gamepads for the
    /// client, collected since the last call.
    pub fn controller_audio(&self, id: u16, kind: u16, payload: &[u8]) {
        self.gamepads.audio(id, kind, payload);
    }
    pub fn controller_audio_ready(&self, id: u16) -> bool {
        self.gamepads.controller_audio_ready(id)
    }
    pub fn gamepad_reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        self.gamepads.reports()
    }
    /// Wake the control socket wait when the driver posts feedback. The socket
    /// must be connected and nonblocking before accepting controller input.
    pub fn set_feedback_waker(&self, waker: std::sync::Arc<std::net::UdpSocket>) {
        self.gamepads.set_feedback_waker(waker);
    }
    pub fn feedback_allowed(&self, kind: u16) -> bool {
        !matches!(kind, 0x010b | 0x5500 | 0x5503 | 0x5601)
            || (self.policy.forward_rumble && self.haptics)
    }
    /// The driver worker keeps ownership of its slots until unplug completes.
    /// Disconnecting a peer must not wait for that driver on the control thread.
    pub fn use_gamepads(&mut self, gamepads: std::sync::Arc<GamepadThread>) {
        drop(self.gamepads.stop());
        self.gamepads = gamepads;
        self.gamepads.activate();
    }
    pub fn detach_gamepads(&mut self) {
        self.gamepads.park();
        if std::sync::Arc::strong_count(&self.gamepads) == 1 {
            drop(self.gamepads.stop());
        }
    }
    pub fn release_all(&mut self) {
        self.gamepads.release_all();
        if let Err(error) = self.release_pointers() {
            tracing::warn!(%error, "input cleanup incomplete");
        }
    }
    fn release_pointers(&mut self) -> Result<()> {
        self.flush();
        let mut failures: Vec<_> = std::mem::take(&mut self.pending.errors)
            .into_iter()
            .map(|error| format!("pending input: {error:#}"))
            .collect();
        let mut keep = |what: &str, result: Result<()>| {
            if let Err(error) = result {
                failures.push(format!("{what}: {error:#}"));
            }
        };
        self.repeat = None;
        self.scroll = [0; 2];
        // The buttons a touch held are released below with the rest.
        self.touch_profile = Default::default();
        if self.left_release.take().is_some() {
            keep(
                "deferred left button",
                self.release_owned(false, 1, Self::button(1, false)),
            );
        }
        for key in &self.keys {
            keep(
                &format!("key {key}"),
                self.release_owned(
                    true,
                    *key,
                    self.key_scan(*key, false, self.key_flags.get(key).copied().unwrap_or(0)),
                ),
            );
        }
        for button in &self.buttons {
            keep(
                &format!("mouse button {button}"),
                self.release_owned(false, u32::from(*button), Self::button(*button, false)),
            );
        }
        // Lift contacts that are still down before their devices go away,
        // attempting every release even if an earlier device refused it.
        keep("touch contacts", self.cancel_touches());
        keep("pen", self.cancel_pen());
        self.keys.clear();
        self.key_sources.clear();
        self.buttons.clear();
        self.key_flags.clear();
        if !failures.is_empty() {
            bail!("{}", failures.join("; "));
        }
        Ok(())
    }
}
impl Drop for Injector {
    fn drop(&mut self) {
        if let Err(error) = self.release_pointers() {
            tracing::warn!(%error, "input cleanup incomplete during teardown");
        }
        // SAFETY: These synthetic device handles are owned by the injector and taken so each is destroyed exactly once.
        unsafe {
            if let Some(device) = self.touch_device.take() {
                DestroySyntheticPointerDevice(device);
            }
            if let Some(device) = self.pen_device.take() {
                DestroySyntheticPointerDevice(device);
            }
        }
    }
}
#[cfg(test)]
#[path = "input/native_touch_tests.rs"]
mod native_touch_tests;

#[cfg(test)]
mod tests;
