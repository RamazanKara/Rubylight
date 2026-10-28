use std::collections::HashMap;

use nightjar_core::input::{packets::touch_event, InputQueue};
use rubylight_protocol::pen::ROLL_UNKNOWN;
use sdl3_sys::everything::*;

#[derive(Default)]
pub struct Touch {
    contacts: HashMap<(u64, u64), u32>,
    pen: Option<Pen>,
}

struct Pen {
    id: SDL_PenID,
    position: (f32, f32),
    state: SDL_PenInputFlags,
    pressure: f32,
    distance: f32,
    tilt: [Option<f32>; 2],
    /// Barrel roll, degrees 0-359 clockwise, or `ROLL_UNKNOWN`.
    roll: u16,
    sent: bool,
    down: bool,
}

/// SDL's x and y tilt as Moonlight's tilt direction (degrees clockwise, the host's inverse of
/// it gives the x and y tilt back) and tilt from upright; both unknown without both axes.
fn tilt(tilt: [Option<f32>; 2]) -> (u16, u8) {
    let [Some(x), Some(y)] = tilt else { return (u16::MAX, u8::MAX) };
    let (x, y) = (x.to_radians().tan(), y.to_radians().tan());
    let rotation = (-x).atan2(y).to_degrees().rem_euclid(360.0).round() as u16 % 360;
    (rotation, x.hypot(y).atan().to_degrees().round().clamp(0.0, 90.0) as u8)
}

fn position(x: f32, y: f32, rect: SDL_FRect, clamp: bool) -> Option<(f32, f32)> {
    if !x.is_finite() || !y.is_finite() || rect.w <= 0.0 || rect.h <= 0.0 { return None; }
    let (x, y) = ((x - rect.x) / rect.w, (y - rect.y) / rect.h);
    if !clamp && (!(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y)) { return None; }
    Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

impl Pen {
    fn new(id: SDL_PenID) -> Self {
        Self { id, position: (0.0, 0.0), state: SDL_PenInputFlags(0), pressure: 0.0,
            distance: 0.0, tilt: [None, None], roll: ROLL_UNKNOWN, sent: false, down: false }
    }

    fn send(&mut self, event: u8, queue: &mut InputQueue) {
        let tool = if self.state & SDL_PEN_INPUT_ERASER_TIP != SDL_PenInputFlags(0) { 2 } else { 1 };
        let buttons = ((self.state.0 >> 1) & 7) as u8;
        let (rotation, tilt) = tilt(self.tilt);
        if queue.send_pen(event, tool, buttons, self.position.0, self.position.1,
            if self.down { self.pressure } else { self.distance }, 0.0, 0.0, rotation, tilt, self.roll).is_ok() { self.sent = true; }
    }
}

impl Touch {
    pub fn release(&mut self, queue: &mut InputQueue) {
        if !self.contacts.is_empty() {
            let _ = queue.send_touch(touch_event::CANCEL_ALL, 0, 0.0, 0.0, 0.0, 0.0, 0.0, u16::MAX);
            self.contacts.clear();
        }
        if let Some(mut pen) = self.pen.take() {
            if pen.sent {
                pen.state = SDL_PenInputFlags(0);
                pen.send(touch_event::CANCEL, queue);
                pen.send(touch_event::HOVER_LEAVE, queue);
            }
        }
    }

    pub fn handle(&mut self, event: &SDL_Event, rect: SDL_FRect, area: (i32, i32), density: f32, queue: &mut InputQueue) {
        // SAFETY: each union member is selected by its SDL event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            if matches!(kind, SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_MOTION | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED) {
                let finger = event.tfinger;
                let key = (finger.touchID.0, finger.fingerID.0);
                let down = kind == SDL_EVENT_FINGER_DOWN;
                let Some((x, y)) = position(finger.x * area.0 as f32, finger.y * area.1 as f32, rect, !down) else { return; };
                let id = if down {
                    if self.contacts.contains_key(&key) { return; }
                    let Some(id) = (0..32).find(|id| !self.contacts.values().any(|value| value == id)) else { return; };
                    id
                } else {
                    let Some(id) = self.contacts.get(&key).copied() else { return; };
                    id
                };
                let action = match kind {
                    SDL_EVENT_FINGER_DOWN => touch_event::DOWN,
                    SDL_EVENT_FINGER_UP => touch_event::UP,
                    SDL_EVENT_FINGER_CANCELED => touch_event::CANCEL,
                    _ => touch_event::MOVE,
                };
                if queue.send_touch(action, id, x, y, finger.pressure.clamp(0.0, 1.0), 0.0, 0.0, u16::MAX).is_ok() {
                    if down { self.contacts.insert(key, id); }
                    else if matches!(action, touch_event::UP | touch_event::CANCEL) { self.contacts.remove(&key); }
                }
                return;
            }
            if kind == SDL_EVENT_PEN_PROXIMITY_OUT {
                if self.pen.as_ref().is_some_and(|p| p.id == event.pproximity.which) {
                    if let Some(mut pen) = self.pen.take() {
                        pen.state = SDL_PenInputFlags(0);
                        if pen.sent {
                            if pen.down { pen.send(touch_event::CANCEL, queue); }
                            pen.send(touch_event::HOVER_LEAVE, queue);
                        }
                    }
                }
                return;
            }
            let (id, state, x, y) = match kind {
                SDL_EVENT_PEN_MOTION => { let e = event.pmotion; (e.which, e.pen_state, e.x, e.y) }
                SDL_EVENT_PEN_DOWN | SDL_EVENT_PEN_UP => { let e = event.ptouch; (e.which, e.pen_state, e.x, e.y) }
                SDL_EVENT_PEN_BUTTON_DOWN | SDL_EVENT_PEN_BUTTON_UP => { let e = event.pbutton; (e.which, e.pen_state, e.x, e.y) }
                SDL_EVENT_PEN_AXIS => { let e = event.paxis; (e.which, e.pen_state, e.x, e.y) }
                _ => return,
            };
            let pen = self.pen.get_or_insert_with(|| Pen::new(id));
            // The wire carries one pen; a second tablet must not release the first pen's stroke.
            if pen.id != id { return; }
            if kind == SDL_EVENT_PEN_AXIS && event.paxis.value.is_finite() {
                let value = event.paxis.value;
                match event.paxis.axis {
                    SDL_PEN_AXIS_PRESSURE => pen.pressure = value.clamp(0.0, 1.0),
                    SDL_PEN_AXIS_DISTANCE => pen.distance = value.clamp(0.0, 1.0),
                    SDL_PEN_AXIS_XTILT => pen.tilt[0] = Some(value.clamp(-90.0, 90.0)),
                    SDL_PEN_AXIS_YTILT => pen.tilt[1] = Some(value.clamp(-90.0, 90.0)),
                    // SDL's rotation is the barrel roll, -180 to 180 clockwise from up.
                    SDL_PEN_AXIS_ROTATION => pen.roll = value.rem_euclid(360.0).round() as u16 % 360,
                    _ => return,
                }
            }
            let Some(position) = position(x * density, y * density, rect, pen.down) else {
                if pen.sent { pen.send(touch_event::HOVER_LEAVE, queue); pen.sent = false; }
                return;
            };
            pen.position = position;
            pen.state = state;
            let action = match kind {
                SDL_EVENT_PEN_DOWN => { pen.down = true; touch_event::DOWN }
                SDL_EVENT_PEN_UP if pen.down => { pen.down = false; touch_event::UP }
                SDL_EVENT_PEN_UP => return,
                SDL_EVENT_PEN_BUTTON_DOWN | SDL_EVENT_PEN_BUTTON_UP => touch_event::BUTTON_ONLY,
                _ if pen.down => touch_event::MOVE,
                _ if state & SDL_PEN_INPUT_DOWN != SDL_PenInputFlags(0) => return,
                _ => touch_event::HOVER,
            };
            pen.send(action, queue);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::present::letterbox;
    use nightjar_core::control::HostCapabilities;

    #[test]
    fn touch_coordinates_follow_cropping_stretching_and_top_alignment() {
        use crate::config::VideoScaling::*;
        for mode in [Fit, Fill, Stretch, Native] {
            let rect = super::super::present::viewport((1920, 1080), (1280, 1024), mode, false);
            assert_eq!(position(640.0, 512.0, rect, false), Some((0.5, 0.5)));
        }
        let rect = super::super::present::viewport((1920, 1080), (1280, 1024), Native, true);
        assert_eq!(position(0.0, 0.0, rect, false), Some((1.0 / 6.0, 0.0)));
        let rect = super::super::present::viewport((1920, 1080), (1280, 1024), Fit, true);
        assert_eq!(position(640.0, 0.0, rect, false), Some((0.5, 0.0)));
        assert_eq!(position(640.0, 900.0, rect, false), None);
        assert_eq!(position(640.0, 900.0, rect, true), Some((0.5, 1.0)));
    }

    #[test]
    fn contacts_respect_bars_and_device_ids_and_cancel_on_release() {
        let mut touch = Touch::default();
        let mut q = InputQueue::new(&HostCapabilities { feature_flags: 1, ..Default::default() });
        let mut event = SDL_TouchFingerEvent { r#type: SDL_EVENT_FINGER_DOWN, touchID: SDL_TouchID(1), fingerID: SDL_FingerID(3), x: 0.5, y: 0.0, pressure: 0.4, ..Default::default() };
        touch.handle(&SDL_Event { tfinger: event }, letterbox((1920, 1080), (2560, 1600)), (2560, 1600), 2.0, &mut q);
        assert!(touch.contacts.is_empty());
        event.y = 0.5;
        for device in [1, 2] {
            event.touchID = SDL_TouchID(device);
            touch.handle(&SDL_Event { tfinger: event }, letterbox((1920, 1080), (2560, 1600)), (2560, 1600), 2.0, &mut q);
        }
        assert_eq!(touch.contacts.len(), 2);
        assert_ne!(touch.contacts[&(1, 3)], touch.contacts[&(2, 3)]);
        touch.release(&mut q);
        assert!(touch.contacts.is_empty());
        let mut packets = Vec::new();
        q.drain(std::time::Instant::now(), false, &mut packets);
        assert_eq!(packets.last().unwrap().data[8], touch_event::CANCEL_ALL);
        assert_eq!(position(1280.0, 800.0, letterbox((1920, 1080), (2560, 1600)), false), Some((0.5, 0.5)));
        assert_eq!(position(f32::NAN, 0.0, letterbox((1920, 1080), (2560, 1600)), true), None);
    }

    #[test]
    fn pen_release_clears_buttons_and_ends_stroke() {
        let mut touch = Touch::default();
        let mut q = InputQueue::new(&HostCapabilities { feature_flags: 1, ..Default::default() });
        let event = SDL_PenTouchEvent { r#type: SDL_EVENT_PEN_DOWN, which: SDL_PenID(7), x: 50.0, y: 50.0,
            pen_state: SDL_PEN_INPUT_DOWN | SDL_PEN_INPUT_BUTTON_1 | SDL_PEN_INPUT_ERASER_TIP, down: true, ..Default::default() };
        touch.handle(&SDL_Event { ptouch: event }, letterbox((100, 100), (100, 100)), (100, 100), 1.0, &mut q);
        assert!(touch.pen.as_ref().unwrap().down);
        touch.release(&mut q);
        let mut packets = Vec::new();
        q.drain(std::time::Instant::now(), false, &mut packets);
        assert_eq!(packets[packets.len() - 2].data[8], touch_event::CANCEL);
        assert_eq!(packets.last().unwrap().data[8], touch_event::HOVER_LEAVE);
        assert!(touch.pen.is_none());
    }

    /// SDL's rotation axis is the barrel roll: it goes as 0x5500000B to a host that takes it
    /// and is never sent as the tilt's direction, which comes from the x and y tilt.
    #[test]
    fn pen_rotation_is_the_barrel_roll_and_tilt_keeps_its_direction() {
        let packet = |offered: bool| {
            let mut touch = Touch::default();
            let mut q = InputQueue::new(&HostCapabilities { feature_flags: 1, ..Default::default() });
            q.set_pen_barrel_roll(offered);
            for (axis, value) in [(SDL_PEN_AXIS_XTILT, 20.0), (SDL_PEN_AXIS_YTILT, -10.0), (SDL_PEN_AXIS_ROTATION, -90.0)] {
                let event = SDL_PenAxisEvent { r#type: SDL_EVENT_PEN_AXIS, which: SDL_PenID(7), x: 50.0, y: 50.0, axis, value, ..Default::default() };
                touch.handle(&SDL_Event { paxis: event }, letterbox((100, 100), (100, 100)), (100, 100), 1.0, &mut q);
            }
            let mut packets = Vec::new();
            q.drain(std::time::Instant::now(), false, &mut packets);
            packets.pop().unwrap().data
        };
        let stock = packet(false);
        assert_eq!(stock.len(), 36);
        let rolled = packet(true);
        let rolled = rubylight_protocol::pen::PenBarrelRoll::decode(&rolled).expect("0x5500000B");
        assert_eq!(rolled.roll(), 270);
        assert_eq!(rolled.pen()[..], stock[..]);
        // The host turns the direction and tilt back into Windows' x and y tilt.
        let rotation = f32::from(u16::from_le_bytes([stock[24], stock[25]])).to_radians();
        let angle = f32::from(stock[26]).to_radians();
        let x = ((-rotation).sin() * angle.sin()).atan2(angle.cos()).to_degrees();
        let y = ((-rotation).cos() * angle.sin()).atan2(angle.cos()).to_degrees();
        assert!((x.to_radians().tan() / y.to_radians().tan() - 20f32.to_radians().tan() / -10f32.to_radians().tan()).abs() < 0.05, "{x} {y}");
        assert!(x > 0.0 && y < 0.0, "{x} {y}");
        assert_eq!(tilt(super::Pen::new(SDL_PenID(1)).tilt), (u16::MAX, u8::MAX));
    }
}
