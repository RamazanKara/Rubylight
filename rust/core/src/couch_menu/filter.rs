//! The input filter (`docs/couch-menu-design.md` section 3.3). It runs on each
//! batch after the permission check and before the injector or WebRTC sink.
//!
//! Closed, it only reads the events, watching for a chord, and passes them on
//! unchanged and in order. Open, it consumes every event but `Arrival`,
//! `Battery`, `Haptics` and `SteamAttributes` and turns them into [`Nav`]; a
//! controller event that changes the set of pads goes on at rest.
//! After a close, whatever is still held stays masked until it is let go, so
//! the game never sees half a press.
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::chord::{
    CornerHold, MOUSE_LEFT, OpenWith, PadChord, TOUCH_CANCEL, TOUCH_CANCEL_ALL, TOUCH_DOWN,
    TOUCH_UP,
};
use super::{Nav, Shared};
use crate::input::Input;

pub const REPEAT_DELAY: Duration = Duration::from_millis(400);
pub const REPEAT_EVERY: Duration = Duration::from_millis(120);
/// Left-stick deflection that starts a direction (50%) and that ends it (30%).
const STICK_ON: i32 = 16_384;
const STICK_OFF: i32 = 9_830;

const DPAD_UP: u32 = 0x0001;
const DPAD_DOWN: u32 = 0x0002;
const DPAD_LEFT: u32 = 0x0004;
const DPAD_RIGHT: u32 = 0x0008;
const BUTTON_A: u32 = 0x1000;
const BUTTON_B: u32 = 0x2000;
const MOUSE_RIGHT: u8 = 3;
const VK_BACK: u16 = 0x08;
const VK_RETURN: u16 = 0x0d;
const VK_ESCAPE: u16 = 0x1b;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;

/// What one [`Filter::apply`] asks of the host.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// The menu just opened: release every key, button and contact the game
    /// holds (`Injector::release_all()` or `Sink::release`), once.
    pub release_game: bool,
    /// For the menu worker, in order.
    pub nav: Vec<Nav>,
    /// A chord asked to open the menu and this session refused, with the
    /// reason: log it. Reported once per filter.
    pub refused: Option<String>,
}

/// Touch contacts by Moonlight id; at most 16 are followed.
#[derive(Clone, Copy, Debug, Default)]
struct Contacts {
    ids: [u32; 16],
    len: u8,
}

impl Contacts {
    fn contains(&self, id: u32) -> bool {
        self.ids[..usize::from(self.len)].contains(&id)
    }
    fn add(&mut self, id: u32) {
        if !self.contains(id) && usize::from(self.len) < self.ids.len() {
            self.ids[usize::from(self.len)] = id;
            self.len += 1;
        }
    }
    fn remove(&mut self, id: u32) {
        let len = usize::from(self.len);
        if let Some(at) = self.ids[..len].iter().position(|held| *held == id) {
            self.ids[at] = self.ids[len - 1];
            self.len -= 1;
        }
    }
}

/// What the client holds: keys by VK, mouse buttons, each pad's buttons,
/// the pen tip and touch contacts.
#[derive(Clone, Copy, Debug, Default)]
struct Held {
    keys: [u64; 4],
    mouse: u8,
    pads: [u32; 16],
    pen: bool,
    touches: Contacts,
}

impl Held {
    fn observe(&mut self, event: &Input) {
        match *event {
            Input::Keyboard { key, down, .. } => self.set_key(key, down),
            Input::MouseButton { button, down } => {
                let bit = 1u8.checked_shl(u32::from(button)).unwrap_or(0);
                if down {
                    self.mouse |= bit;
                } else {
                    self.mouse &= !bit;
                }
            }
            Input::Controller { id, buttons, .. } => {
                if let Some(pad) = self.pads.get_mut(usize::from(id)) {
                    *pad = buttons;
                }
            }
            Input::Touch { event, id, .. } => match event {
                TOUCH_DOWN => self.touches.add(id),
                TOUCH_UP | TOUCH_CANCEL => self.touches.remove(id),
                TOUCH_CANCEL_ALL => self.touches = Contacts::default(),
                _ => {}
            },
            Input::Pen { event, .. } => match event {
                TOUCH_DOWN => self.pen = true,
                TOUCH_UP | TOUCH_CANCEL | TOUCH_CANCEL_ALL => self.pen = false,
                _ => {}
            },
            _ => {}
        }
    }
    fn has_key(&self, key: u16) -> bool {
        let key = usize::from(key & 0xff);
        self.keys[key / 64] & (1 << (key % 64)) != 0
    }
    fn set_key(&mut self, key: u16, down: bool) {
        let key = usize::from(key & 0xff);
        if down {
            self.keys[key / 64] |= 1 << (key % 64);
        } else {
            self.keys[key / 64] &= !(1 << (key % 64));
        }
    }
    fn is_empty(&self) -> bool {
        self.keys == [0; 4]
            && self.mouse == 0
            && self.pads == [0; 16]
            && !self.pen
            && self.touches.len == 0
    }
}

/// A pad's navigation state while the menu is open.
#[derive(Clone, Copy, Debug, Default)]
struct NavPad {
    buttons: u32,
    /// The latched left-stick direction (hysteresis).
    stick: Option<Nav>,
    /// The direction held now, from the D-pad or the stick.
    dir: Option<Nav>,
}

/// One control peer's filter. Owns no thread and no allocation while closed.
pub struct Filter {
    shared: Arc<Shared>,
    open_with: OpenWith,
    /// Whether this filter has seen the menu open (it follows `shared`).
    open: bool,
    seen_opens: u64,
    pads: PadChord,
    corner: CornerHold,
    held: Held,
    /// Held at the last close and not yet let go.
    masked: Held,
    masking: bool,
    nav_pads: [NavPad; 16],
    repeat: Option<(u16, Nav, Instant)>,
    refused_reported: bool,
    /// The pads the game knows: the `active` mask of the last controller
    /// event passed on.
    active: u16,
}

impl Filter {
    /// For a stream of `width`×`height` target pixels, opened the ways and
    /// with the chords `open_with` sets.
    pub fn new(shared: Arc<Shared>, open_with: OpenWith, width: u32, height: u32) -> Self {
        Self {
            shared,
            open_with,
            open: false,
            seen_opens: 0,
            pads: PadChord::new(open_with.pad_chord, open_with.pad_hold),
            corner: CornerHold::new(width, height, open_with.corner_hold),
            held: Held::default(),
            masked: Held::default(),
            masking: false,
            nav_pads: [NavPad::default(); 16],
            repeat: None,
            refused_reported: false,
            active: 0,
        }
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    /// Follows a new stream size (0x5532): the corner box keeps its share of
    /// the shorter side. A corner hold in progress starts over.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.corner = CornerHold::new(width, height, self.open_with.corner_hold);
    }
    /// When the control loop must call [`Filter::apply`] again without new
    /// input: a hold completing or a direction repeating. `None` while
    /// closed with nothing pending.
    pub fn next_due(&self) -> Option<Instant> {
        [
            self.pads.next_due(),
            self.corner.next_due(),
            self.repeat.map(|(.., at)| at),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    /// Filters one batch in place, received at `now` (call it with an empty
    /// batch when [`Filter::next_due`] passes or the control wake fires).
    /// `pointer` is the host cursor's stream position (`0..=1`), asked only
    /// when a relative-mouse hold completes or a relative mouse clicks in the
    /// open menu.
    pub fn apply(
        &mut self,
        now: Instant,
        events: &mut Vec<Input>,
        mut pointer: impl FnMut() -> Option<(f32, f32)>,
    ) -> Outcome {
        let mut out = Outcome::default();
        self.sync(&mut out);
        self.fire_due(now, &mut pointer, &mut out);
        let mut kept = 0;
        for at in 0..events.len() {
            if self.event(now, &mut events[at], &mut pointer, &mut out) {
                events.swap(kept, at);
                kept += 1;
            }
        }
        events.truncate(kept);
        out
    }
    /// Follows an open or close made elsewhere (native API, menu worker).
    fn sync(&mut self, out: &mut Outcome) {
        if Shared::is_open(self.shared.load()) {
            if self.open && self.shared.opens() != self.seen_opens {
                // Closed and opened again since the last pass.
                self.closed(out);
            }
            if !self.open {
                self.opened(out);
            }
        } else if self.open {
            self.closed(out);
        }
    }
    fn opened(&mut self, out: &mut Outcome) {
        self.open = true;
        self.seen_opens = self.shared.opens();
        out.release_game = true;
        out.nav.push(Nav::Opened);
        // The game is released: nothing from before needs masking any more,
        // and the close masks what is held then.
        self.masked = Held::default();
        self.masking = false;
        self.pads.cancel();
        self.corner.cancel();
        for (nav, buttons) in self.nav_pads.iter_mut().zip(self.held.pads) {
            *nav = NavPad {
                buttons,
                ..NavPad::default()
            };
        }
        self.repeat = None;
    }
    fn closed(&mut self, out: &mut Outcome) {
        self.open = false;
        out.nav.push(Nav::Closed);
        self.masked = self.held;
        self.masking = !self.masked.is_empty();
        self.pads.cancel();
        self.corner.cancel();
        self.repeat = None;
    }
    fn request_open(&mut self, out: &mut Outcome) -> bool {
        match self.shared.open() {
            Ok(_) => {
                self.opened(out);
                true
            }
            Err(reason) => {
                if !self.refused_reported {
                    self.refused_reported = true;
                    out.refused = Some(reason);
                }
                false
            }
        }
    }
    fn request_close(&mut self, out: &mut Outcome) {
        self.shared.close();
        self.closed(out);
    }
    fn fire_due(
        &mut self,
        now: Instant,
        pointer: &mut dyn FnMut() -> Option<(f32, f32)>,
        out: &mut Outcome,
    ) {
        if self.pads.fire(now) {
            if self.open {
                self.request_close(out);
            } else {
                self.request_open(out);
            }
        }
        if !self.open && self.corner.fire(now, pointer) {
            self.request_open(out);
        }
        if let Some((pad, dir, at)) = self.repeat
            && at <= now
        {
            out.nav.push(dir);
            let next = at + REPEAT_EVERY;
            self.repeat = Some((
                pad,
                dir,
                if next <= now {
                    now + REPEAT_EVERY
                } else {
                    next
                },
            ));
        }
    }
    /// Whether `event` goes on to the game.
    fn event(
        &mut self,
        now: Instant,
        event: &mut Input,
        pointer: &mut dyn FnMut() -> Option<(f32, f32)>,
        out: &mut Outcome,
    ) -> bool {
        // A key the client already holds is an auto-repeat: only the first
        // key-down of the keyboard chord opens or closes the menu.
        let key_chord = self.open_with.key_chord.matches(event)
            && !matches!(*event, Input::Keyboard { key, .. } if self.held.has_key(key));
        self.held.observe(event);
        if let Input::Controller { id, buttons, .. } = *event
            && (self.open || self.open_with.controller)
        {
            self.pads.update(id, buttons, now);
        }
        self.corner
            .observe(event, now, !self.open && self.open_with.touch);
        if self.open {
            self.navigate(now, event, key_chord, pointer, out);
            return match event {
                // A pad joined or left: the game sees the new set at once, at
                // rest. Consumed, a pad unplugged while the menu is open
                // stayed plugged on the host.
                Input::Controller {
                    active,
                    buttons,
                    left_trigger,
                    right_trigger,
                    sticks,
                    ..
                } if *active != self.active => {
                    self.active = *active;
                    (*buttons, *left_trigger, *right_trigger, *sticks) = (0, 0, 0, [0; 4]);
                    true
                }
                _ => matches!(
                    event,
                    Input::Arrival { .. }
                        | Input::Battery { .. }
                        | Input::Haptics(_)
                        | Input::SteamAttributes(_)
                ),
            };
        }
        if self.open_with.keyboard && key_chord && self.request_open(out) {
            return false;
        }
        if let Input::Controller { active, .. } = *event {
            self.active = active;
        }
        !self.masking || self.unmask(event)
    }
    fn navigate(
        &mut self,
        now: Instant,
        event: &Input,
        key_chord: bool,
        pointer: &mut dyn FnMut() -> Option<(f32, f32)>,
        out: &mut Outcome,
    ) {
        match *event {
            Input::Controller {
                id,
                buttons,
                sticks,
                ..
            } => self.pad_nav(id, buttons, sticks, now, out),
            Input::Keyboard {
                key, down: true, ..
            } => {
                // A repeat of the chord's key does nothing, not Back.
                if self.open_with.key_chord.matches(event) {
                    if key_chord {
                        self.request_close(out);
                    }
                    return;
                }
                let nav = match key {
                    VK_UP => Nav::Up,
                    VK_DOWN => Nav::Down,
                    VK_LEFT => Nav::Left,
                    VK_RIGHT => Nav::Right,
                    VK_RETURN | VK_SPACE => Nav::Accept,
                    VK_ESCAPE | VK_BACK => Nav::Back,
                    _ => return,
                };
                out.nav.push(nav);
            }
            Input::MouseButton {
                button: MOUSE_LEFT,
                down: true,
            } => {
                if let Some((x, y)) = self.corner.last_absolute().or_else(&mut *pointer) {
                    out.nav.push(Nav::Point { x, y });
                }
            }
            Input::MouseButton {
                button: MOUSE_RIGHT,
                down: true,
            } => out.nav.push(Nav::Back),
            Input::Scroll {
                amount,
                horizontal: false,
            } if amount != 0 => out.nav.push(if amount > 0 { Nav::Up } else { Nav::Down }),
            Input::Touch {
                event: TOUCH_DOWN,
                x,
                y,
                ..
            }
            | Input::Pen {
                event: TOUCH_DOWN,
                x,
                y,
                ..
            } => out.nav.push(Nav::Point { x, y }),
            _ => {}
        }
    }
    fn pad_nav(
        &mut self,
        id: u16,
        buttons: u32,
        sticks: [i16; 4],
        now: Instant,
        out: &mut Outcome,
    ) {
        let Some(pad) = self.nav_pads.get_mut(usize::from(id)) else {
            return;
        };
        let pressed = buttons & !pad.buttons;
        pad.buttons = buttons;
        if pressed & BUTTON_A != 0 {
            out.nav.push(Nav::Accept);
        }
        if pressed & BUTTON_B != 0 {
            out.nav.push(Nav::Back);
        }
        let (x, y) = (i32::from(sticks[0]), i32::from(sticks[1]));
        pad.stick = match pad.stick {
            Some(dir) if along(dir, x, y) >= STICK_OFF => Some(dir),
            _ if x.abs().max(y.abs()) <= STICK_ON => None,
            _ if x.abs() > y.abs() => Some(if x > 0 { Nav::Right } else { Nav::Left }),
            // Moonlight's stick Y is positive up.
            _ => Some(if y > 0 { Nav::Up } else { Nav::Down }),
        };
        let dpad = [
            (DPAD_UP, Nav::Up),
            (DPAD_DOWN, Nav::Down),
            (DPAD_LEFT, Nav::Left),
            (DPAD_RIGHT, Nav::Right),
        ]
        .into_iter()
        .find_map(|(bit, dir)| (buttons & bit != 0).then_some(dir));
        let dir = dpad.or(pad.stick);
        if dir == pad.dir {
            return;
        }
        pad.dir = dir;
        match dir {
            Some(dir) => {
                out.nav.push(dir);
                self.repeat = Some((id, dir, now + REPEAT_DELAY));
            }
            None if self.repeat.is_some_and(|(pad, ..)| pad == id) => self.repeat = None,
            None => {}
        }
    }
    /// After a close: drops or trims events of what was held at the close
    /// until it is let go. Returns whether the event goes on.
    fn unmask(&mut self, event: &mut Input) -> bool {
        let masked = &mut self.masked;
        let keep = match event {
            Input::Keyboard { key, down, .. } => {
                if masked.has_key(*key) {
                    if !*down {
                        masked.set_key(*key, false);
                    }
                    false
                } else {
                    true
                }
            }
            Input::MouseButton { button, down } => {
                let bit = 1u8.checked_shl(u32::from(*button)).unwrap_or(0);
                if masked.mouse & bit != 0 {
                    if !*down {
                        masked.mouse &= !bit;
                    }
                    false
                } else {
                    true
                }
            }
            Input::Controller { id, buttons, .. } => {
                if let Some(mask) = masked.pads.get_mut(usize::from(*id)) {
                    let raw = *buttons;
                    *buttons &= !*mask;
                    *mask &= raw;
                }
                true
            }
            Input::Touch { event, id, .. } => {
                if *event == TOUCH_CANCEL_ALL {
                    masked.touches = Contacts::default();
                    true
                } else if masked.touches.contains(*id) {
                    if matches!(*event, TOUCH_DOWN | TOUCH_UP | TOUCH_CANCEL) {
                        masked.touches.remove(*id);
                    }
                    *event == TOUCH_DOWN
                } else {
                    true
                }
            }
            Input::Pen { event, .. } if masked.pen => {
                if matches!(*event, TOUCH_UP | TOUCH_CANCEL | TOUCH_CANCEL_ALL) {
                    masked.pen = false;
                }
                false
            }
            _ => true,
        };
        self.masking = !self.masked.is_empty();
        keep
    }
}

/// How far the stick points along `dir`.
fn along(dir: Nav, x: i32, y: i32) -> i32 {
    match dir {
        Nav::Up => y,
        Nav::Down => -y,
        Nav::Left => -x,
        Nav::Right => x,
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
