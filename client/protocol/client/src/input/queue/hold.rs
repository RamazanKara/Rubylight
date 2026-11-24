//! Network-grace state holding: captures the latest key, button, controller and contact state
//! while the host is silent, then replays it once contact returns.

use super::poll::note_sent;
use super::{InputPacket, InputQueue, Pending, MAX_MOTION_TYPES};
use crate::control::wire::Delivery;
use crate::input::packets::{touch_event, InputEvent, MAX_CONTROLLERS};

const HELD_KEYS: usize = 256;
const HELD_MOUSE_BUTTONS: usize = 8;

/// The latest state of what the host must not lose while it is silent.
pub(super) struct Held {
    keyboard: Vec<Option<Pending>>,
    mouse_buttons: [Option<Pending>; HELD_MOUSE_BUTTONS],
    controllers: [Option<Pending>; MAX_CONTROLLERS as usize],
    arrivals: [Option<Pending>; MAX_CONTROLLERS as usize],
    identities: [Option<Pending>; MAX_CONTROLLERS as usize],
    motion: [[Option<Pending>; MAX_MOTION_TYPES]; MAX_CONTROLLERS as usize],
    batteries: [Option<Pending>; MAX_CONTROLLERS as usize],
    touch: Option<Pending>,
    pen: Option<Pending>,
    dropped: u32,
}

impl Default for Held {
    fn default() -> Self {
        Self {
            keyboard: vec![None; HELD_KEYS],
            mouse_buttons: Default::default(),
            controllers: Default::default(),
            arrivals: Default::default(),
            identities: Default::default(),
            motion: Default::default(),
            batteries: Default::default(),
            touch: None,
            pen: None,
            dropped: 0,
        }
    }
}

impl InputQueue {
    /// The number of packets held back since the host went silent.
    pub fn held_count(&self) -> u32 {
        self.held.dropped
    }

    pub(super) fn note_held(&mut self) {
        self.holding = true;
        self.held.dropped = self.held.dropped.wrapping_add(1);
    }

    pub(super) fn hold(&mut self, mut pending: Pending) {
        self.note_held();
        let held = &mut self.held;
        match pending.event {
            InputEvent::Keyboard { key_code, .. } => {
                held.keyboard[usize::from(key_code & 0xFF)] = Some(pending)
            }
            InputEvent::MouseButton { button, .. } => {
                if let Some(slot) = held.mouse_buttons.get_mut(usize::from(button)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::Controller { number, .. } => {
                if let Some(slot) = held.controllers.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::ControllerArrival { number, .. } => {
                if let Some(slot) = held.arrivals.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::ControllerIdentity { number, .. } => {
                if let Some(slot) = held.identities.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::ControllerMotion {
                number,
                motion_type,
                ..
            } => {
                if (1..=MAX_MOTION_TYPES as u8).contains(&motion_type)
                    && let Some(slots) = held.motion.get_mut(usize::from(number))
                {
                    slots[usize::from(motion_type - 1)] = Some(pending);
                }
            }
            InputEvent::ControllerBattery { number, .. } => {
                if let Some(slot) = held.batteries.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            // Contacts from the silence are cancelled once the host is back.
            InputEvent::Touch { ref mut event, .. } => {
                *event = touch_event::CANCEL_ALL;
                pending.delivery = Delivery::Reliable;
                held.touch = Some(pending);
            }
            InputEvent::Pen { ref mut event, .. } => {
                *event = touch_event::CANCEL_ALL;
                pending.delivery = Delivery::Reliable;
                held.pen = Some(pending);
            }
            // Motion, scrolling, text and the rest from the silence are dropped.
            _ => {}
        }
    }

    /// Sends the latest held state: identities and arrivals, then releases before presses, then controller
    /// state, then touch and pen cancels.
    pub(super) fn release_held(&mut self, out: &mut Vec<InputPacket>) {
        let held = std::mem::take(&mut self.held);
        self.holding = false;
        let frame_warp = self.frame_warp.clone();
        let send = |pending: &Option<Pending>, out: &mut Vec<InputPacket>| {
            if let Some(p) = pending {
                note_sent(frame_warp.as_deref(), &p.event);
                out.push(InputPacket {
                    data: p.event.encode(self.sunshine),
                    channel: p.channel,
                    delivery: p.delivery,
                });
            }
        };
        let is_down = |p: &Option<Pending>| {
            matches!(
                p,
                Some(Pending {
                    event: InputEvent::Keyboard { down: true, .. }
                        | InputEvent::MouseButton { down: true, .. },
                    ..
                })
            )
        };
        // Each identity goes before the arrivals: the host picks the pad on arrival.
        for slot in held.identities.iter().chain(&held.arrivals) {
            send(slot, out);
        }
        for slot in held
            .keyboard
            .iter()
            .chain(&held.mouse_buttons)
            .filter(|p| !is_down(p))
        {
            send(slot, out);
        }
        for slot in held
            .keyboard
            .iter()
            .chain(&held.mouse_buttons)
            .filter(|p| is_down(p))
        {
            send(slot, out);
        }
        for number in 0..usize::from(MAX_CONTROLLERS) {
            send(&held.controllers[number], out);
            send(&held.motion[number][0], out);
            send(&held.motion[number][1], out);
            send(&held.batteries[number], out);
        }
        send(&held.touch, out);
        send(&held.pen, out);
    }
}
