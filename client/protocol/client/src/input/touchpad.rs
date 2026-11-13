//! A physical touchpad's fingers for a host that takes Foundation Sunshine's touchpad packets
//! (the "Touchpad gestures on the PC" setting): whole frames of the touchpad's contacts become
//! the contact changes the host replays on its own touchpad, so the PC's gestures work. Shared by
//! desktop Nightjar (Windows Raw Input, Linux evdev, macOS trackpad touches) and Android
//! (`SOURCE_TOUCHPAD` with pointer capture).
//!
//! Ported from Moonlight V+ PC: streaming/input/touchpad.cpp (frames to contact changes and
//! the mouse events the system promotes from them, [`Tracker`]).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::control::negotiation::feature;
use crate::input::packets::{mouse_button, touch_event, touchpad, TouchpadContact};
use crate::input::{InputError, InputQueue};

const MAX_CONTACTS: usize = touchpad::MAX_FRAME_CONTACTS;
/// The system's promoted pointer moves, clicks and scrolls are dropped this long after a frame,
/// so a lost last report cannot leave the mouse off.
const MOUSE_SUPPRESSION: Duration = Duration::from_millis(500);

/// One finger as the touchpad gives it, on its surface (0-1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawContact {
    pub pointer_id: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    /// On the surface and not a palm; a contact without it is lifting.
    pub touching: bool,
}

/// A whole hardware frame of one touchpad, or only its button (`contacts: None`).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Tells touchpads apart; never 0, which is no touchpad.
    pub device: usize,
    pub contacts: Option<Vec<RawContact>>,
    pub button: bool,
    /// The surface's size, 0 when unknown (the host then uses a laptop touchpad's).
    pub width_mm: u16,
    pub height_mm: u16,
}

/// What the tracker sends.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Contacts { contacts: Vec<TouchpadContact>, buttons: u8, width_mm: u16, height_mm: u16 },
    /// The touchpad's button pressed with no finger on it: the host has no contact to carry
    /// it, so it goes as the left mouse button.
    MouseButton(bool),
}

/// Turns frames into contact changes (`handleWindowsTouchpadFrame`) and decides which of the
/// mouse events the system makes of the touchpad are dropped.
#[derive(Default)]
pub struct Tracker {
    active: BTreeMap<u32, TouchpadContact>,
    device: usize,
    button_down: bool,
    mouse_fallback: bool,
    size: (u16, u16),
    last_frame: Option<Instant>,
    /// Mouse buttons (1 << index) whose press was dropped, so their release is too.
    suppressed_buttons: u32,
}

impl Tracker {
    fn has_state(&self) -> bool {
        !self.active.is_empty() || self.button_down || self.mouse_fallback
    }

    pub fn frame(&mut self, frame: Frame, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        let has_current = frame.contacts.as_ref().is_some_and(|c| c.iter().take(MAX_CONTACTS).any(|c| c.touching));
        let frame_active = has_current || frame.button;
        if self.has_state() && self.device != frame.device {
            // An empty or late frame of another touchpad must not take over.
            if !frame_active {
                return out;
            }
            out.extend(self.cancel());
        }
        self.device = frame.device;
        self.size = (frame.width_mm, frame.height_mm);
        self.last_frame = Some(now);
        let button_changed = frame.button != self.button_down;
        let mut fallback_changed = false;
        if self.mouse_fallback && !frame.button {
            out.push(Out::MouseButton(false));
            self.mouse_fallback = false;
            fallback_changed = true;
        } else if !self.mouse_fallback && frame.button
            && !(if frame.contacts.is_some() { has_current } else { !self.active.is_empty() })
        {
            out.push(Out::MouseButton(true));
            self.mouse_fallback = true;
            fallback_changed = true;
        }
        let buttons = if frame.button && !self.mouse_fallback { touchpad::BUTTON_PRIMARY } else { 0 };
        let Some(contacts) = frame.contacts else {
            if button_changed && !fallback_changed {
                let moved: Vec<_> = self.active.values().map(|c| TouchpadContact { event: touch_event::MOVE, ..*c }).collect();
                self.push(&mut out, moved, buttons);
            }
            self.button_down = frame.button;
            if self.active.is_empty() && !frame.button {
                self.device = 0;
            }
            return out;
        };
        let mut current = BTreeMap::new();
        for c in contacts.iter().take(MAX_CONTACTS).filter(|c| c.touching) {
            let event = if self.active.contains_key(&c.pointer_id) { touch_event::MOVE } else { touch_event::DOWN };
            current.insert(c.pointer_id, TouchpadContact {
                event,
                pointer_id: c.pointer_id,
                x: c.x.clamp(0.0, 1.0),
                y: c.y.clamp(0.0, 1.0),
                pressure: c.pressure.clamp(0.0, 1.0),
            });
        }
        // Lifted contacts go first, so the host has a free slot when a full frame replaces one.
        let lifted: Vec<_> = self.active.iter().filter(|(id, _)| !current.contains_key(id))
            .map(|(_, c)| TouchpadContact { event: touch_event::UP, ..*c }).collect();
        self.push(&mut out, lifted, buttons);
        self.push(&mut out, current.values().copied().collect(), buttons);
        self.active = current;
        self.button_down = frame.button;
        if self.active.is_empty() && !frame.button {
            self.device = 0;
        }
        out
    }

    fn push(&self, out: &mut Vec<Out>, contacts: Vec<TouchpadContact>, buttons: u8) {
        for chunk in contacts.chunks(MAX_CONTACTS) {
            out.push(Out::Contacts { contacts: chunk.to_vec(), buttons, width_mm: self.size.0, height_mm: self.size.1 });
        }
    }

    /// Lifts everything this touchpad holds on the host (focus or capture lost, the touchpad
    /// removed or another taking over).
    pub fn cancel(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        let cancelled: Vec<_> = self.active.values().map(|c| TouchpadContact { event: touch_event::CANCEL, ..*c }).collect();
        self.push(&mut out, cancelled, 0);
        if self.mouse_fallback {
            out.push(Out::MouseButton(false));
        }
        let suppressed = self.suppressed_buttons;
        *self = Tracker { suppressed_buttons: suppressed, ..Tracker::default() };
        out
    }

    pub fn device(&self) -> usize {
        self.device
    }

    /// Whether fingers or the click are on the touchpad now.
    pub fn touching(&self) -> bool {
        self.has_state()
    }

    /// Pointer moves and scrolls right after a frame are the touchpad again, when they come
    /// from where the system's touchpad pointer comes from (`from_touchpad`).
    pub fn swallows_motion(&self, from_touchpad: bool, now: Instant) -> bool {
        from_touchpad && self.last_frame.is_some_and(|t| now.saturating_duration_since(t) <= MOUSE_SUPPRESSION)
    }

    /// A click the system promotes from the touchpad: the frames carry it (or the mouse
    /// fallback already sent it), and its release goes with it. `button` is the mouse button
    /// (`mouse_button::LEFT` to `X2`, as SDL numbers them too).
    pub fn swallows_button(&mut self, from_touchpad: bool, button: u8, down: bool, captured: bool, now: Instant) -> bool {
        if !from_touchpad || !(mouse_button::LEFT..=mouse_button::X2).contains(&button) {
            return false;
        }
        let mask = 1u32 << button;
        if !down {
            let swallowed = self.suppressed_buttons & mask != 0;
            self.suppressed_buttons &= !mask;
            return swallowed;
        }
        self.suppressed_buttons &= !mask;
        if captured && ((self.mouse_fallback && button == mouse_button::LEFT) || self.swallows_motion(from_touchpad, now)) {
            self.suppressed_buttons |= mask;
            return true;
        }
        false
    }
}

/// How the host takes touchpad contacts: a frame per packet, or one contact per packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Frame,
    Individual,
}

impl Transport {
    pub fn of(feature_flags: u32) -> Option<Transport> {
        if feature_flags & feature::TOUCHPAD_FRAME_EVENTS != 0 {
            Some(Transport::Frame)
        } else if feature_flags & feature::TOUCHPAD_EVENTS != 0 {
            Some(Transport::Individual)
        } else {
            None
        }
    }

    /// Queues what the tracker sends; the first error other than a stopped stream.
    pub fn send(self, outs: Vec<Out>, queue: &mut InputQueue) -> Result<(), InputError> {
        let mut first = Ok(());
        for out in outs {
            let sent = match out {
                Out::MouseButton(down) => queue.send_mouse_button(mouse_button::LEFT, down),
                Out::Contacts { contacts, buttons, width_mm, height_mm } => match self {
                    Transport::Frame => queue.send_touchpad_frame(&contacts, buttons, width_mm, height_mm),
                    Transport::Individual => contacts.iter().try_for_each(|c| queue.send_touchpad(*c, buttons, width_mm, height_mm)),
                },
            };
            match sent {
                Ok(()) | Err(InputError::Stopped) => {}
                Err(e) => {
                    eprintln!("nightjar: touchpad input not sent: {e:?}");
                    if first.is_ok() {
                        first = Err(e);
                    }
                }
            }
        }
        first
    }
}

/// A touchpad axis' length in millimetres from its range and resolution in units per
/// millimetre (evdev's `input_absinfo`, Android's motion range), 0 when unknown.
pub fn length_mm(min: i32, max: i32, units_per_mm: f32) -> u16 {
    let extent = f64::from(max) - f64::from(min);
    if extent <= 0.0 || !units_per_mm.is_finite() || units_per_mm <= 0.0 {
        return 0;
    }
    (extent / f64::from(units_per_mm)).clamp(1.0, f64::from(u16::MAX)).round() as u16
}

/// A value on its range as 0-1.
pub fn unit(value: f32, min: f32, max: f32) -> f32 {
    if max <= min || !value.is_finite() {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: u32, x: f32, touching: bool) -> RawContact {
        RawContact { pointer_id: id, x, y: 0.5, pressure: 0.0, touching }
    }

    fn frame(contacts: &[(u32, f32)], button: bool) -> Frame {
        Frame { device: 7, contacts: Some(contacts.iter().map(|&(id, x)| contact(id, x, true)).collect()), button, width_mm: 100, height_mm: 60 }
    }

    fn events(out: &[Out]) -> Vec<(Vec<(u8, u32)>, u8)> {
        out.iter().filter_map(|o| match o {
            Out::Contacts { contacts, buttons, .. } => Some((contacts.iter().map(|c| (c.event, c.pointer_id)).collect(), *buttons)),
            Out::MouseButton(_) => None,
        }).collect()
    }

    #[test]
    fn fingers_go_down_move_and_lift_with_the_click_on_them() {
        let t = Instant::now();
        let mut tr = Tracker::default();
        assert_eq!(events(&tr.frame(frame(&[(1, 0.2)], false), t)), [(vec![(touch_event::DOWN, 1)], 0)]);
        assert!(tr.touching());
        assert_eq!(events(&tr.frame(frame(&[(1, 0.3), (2, 0.6)], false), t)),
            [(vec![(touch_event::MOVE, 1), (touch_event::DOWN, 2)], 0)]);
        // A click with fingers on it rides on their contacts.
        assert_eq!(events(&tr.frame(frame(&[(1, 0.3), (2, 0.6)], true), t)),
            [(vec![(touch_event::MOVE, 1), (touch_event::MOVE, 2)], touchpad::BUTTON_PRIMARY)]);
        // Lifted fingers go first, in their own packet.
        assert_eq!(events(&tr.frame(frame(&[(2, 0.6), (3, 0.1)], false), t)),
            [(vec![(touch_event::UP, 1)], 0), (vec![(touch_event::MOVE, 2), (touch_event::DOWN, 3)], 0)]);
        assert_eq!(events(&tr.cancel()), [(vec![(touch_event::CANCEL, 2), (touch_event::CANCEL, 3)], 0)]);
        assert!(tr.cancel().is_empty());
        assert!(!tr.touching());
    }

    #[test]
    fn a_click_without_a_finger_is_the_left_mouse_button() {
        let t = Instant::now();
        let mut tr = Tracker::default();
        let out = tr.frame(Frame { device: 7, contacts: None, button: true, width_mm: 0, height_mm: 0 }, t);
        assert_eq!(out, [Out::MouseButton(true)]);
        // The system's own promoted left click is dropped, and so is its release.
        assert!(tr.swallows_button(true, mouse_button::LEFT, true, true, t));
        assert!(tr.swallows_button(true, mouse_button::LEFT, false, true, t));
        let out = tr.frame(Frame { device: 7, contacts: None, button: false, width_mm: 0, height_mm: 0 }, t);
        assert_eq!(out, [Out::MouseButton(false)]);
        // A held fallback click is released when the touchpad is cancelled.
        tr.frame(Frame { device: 7, contacts: None, button: true, width_mm: 0, height_mm: 0 }, t);
        assert_eq!(tr.cancel(), [Out::MouseButton(false)]);
    }

    #[test]
    fn the_pointer_is_dropped_only_right_after_a_frame_and_only_from_the_touchpad() {
        let t = Instant::now();
        let mut tr = Tracker::default();
        assert!(!tr.swallows_motion(true, t));
        tr.frame(frame(&[(1, 0.2)], false), t);
        assert!(tr.swallows_motion(true, t + Duration::from_millis(500)));
        assert!(!tr.swallows_motion(true, t + Duration::from_millis(501)));
        // Another mouse keeps working.
        assert!(!tr.swallows_motion(false, t));
        assert!(!tr.swallows_button(false, mouse_button::LEFT, true, true, t));
        // A release whose press went through is not dropped.
        assert!(!tr.swallows_button(true, mouse_button::RIGHT, false, true, t));
        tr.cancel();
        assert!(!tr.swallows_motion(true, t));
    }

    #[test]
    fn another_touchpad_takes_over_only_with_a_finger_or_click() {
        let t = Instant::now();
        let mut tr = Tracker::default();
        tr.frame(frame(&[(1, 0.2)], false), t);
        let other = |contacts: Option<Vec<RawContact>>| Frame { device: 9, contacts, button: false, width_mm: 0, height_mm: 0 };
        assert!(tr.frame(other(Some(vec![])), t).is_empty());
        assert_eq!(events(&tr.frame(other(Some(vec![contact(4, 0.5, true)])), t)),
            [(vec![(touch_event::CANCEL, 1)], 0), (vec![(touch_event::DOWN, 4)], 0)]);
        assert_eq!(tr.device(), 9);
    }

    #[test]
    fn the_host_picks_frames_over_single_contacts() {
        assert_eq!(Transport::of(feature::PEN_TOUCH_EVENTS), None);
        assert_eq!(Transport::of(feature::TOUCHPAD_EVENTS), Some(Transport::Individual));
        assert_eq!(Transport::of(feature::TOUCHPAD_EVENTS | feature::TOUCHPAD_FRAME_EVENTS), Some(Transport::Frame));
    }

    #[test]
    fn ranges_give_the_surface_size_and_position() {
        // evdev: 0-1200 at 12 units per mm.
        assert_eq!(length_mm(0, 1200, 12.0), 100);
        assert_eq!(length_mm(-600, 600, 12.0), 100);
        assert_eq!(length_mm(0, 1200, 0.0), 0);
        assert_eq!(length_mm(5, 5, 10.0), 0);
        assert_eq!(unit(600.0, 0.0, 1200.0), 0.5);
        assert_eq!(unit(-5.0, 0.0, 1200.0), 0.0);
        assert_eq!(unit(5.0, 3.0, 3.0), 0.0);
    }

    #[test]
    fn frames_reach_the_host_as_touchpad_packets() {
        use crate::control::negotiation::HostCapabilities;
        let caps = |feature_flags| HostCapabilities { app_version: [7, 1, 431, -1], feature_flags, ..HostCapabilities::default() };
        let mut q = InputQueue::new(&caps(feature::TOUCHPAD_EVENTS | feature::TOUCHPAD_FRAME_EVENTS));
        let mut tr = Tracker::default();
        let outs = tr.frame(frame(&[(1, 0.2), (2, 0.4)], false), Instant::now());
        assert_eq!(Transport::Frame.send(outs, &mut q), Ok(()));
        let mut without = InputQueue::new(&caps(0));
        let outs = tr.frame(frame(&[(1, 0.3)], false), Instant::now());
        assert_eq!(Transport::Frame.send(outs, &mut without), Err(InputError::Unsupported));
    }
}
