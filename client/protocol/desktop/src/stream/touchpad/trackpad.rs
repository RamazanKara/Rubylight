//! macOS: the trackpad's fingers as SDL reports them, touch events of an indirect touch device
//! on the trackpad's surface (0-1), gathered into one frame per pump. The trackpad's click comes
//! as the mouse's left button and rides on the frames.

use std::collections::BTreeMap;

use sdl3_sys::everything::*;

use super::{Event, Frame, RawContact};

/// The trackpad's fingers since the last pump.
#[derive(Default)]
pub struct Trackpad {
    /// Finger id to its contact.
    fingers: BTreeMap<u64, RawContact>,
    /// SDL finger ids to the small contact ids the host gets.
    ids: BTreeMap<u64, u32>,
    button: bool,
    device: Option<SDL_TouchID>,
    changed: bool,
}

impl Trackpad {
    pub fn install(_window: *mut SDL_Window) -> Result<Trackpad, String> {
        Ok(Trackpad::default())
    }

    /// Takes a finger event of the trackpad; false for anything else, which goes on as before.
    pub fn finger(&mut self, event: &SDL_Event) -> bool {
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            if !matches!(kind, SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_MOTION | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED) {
                return false;
            }
            let finger = event.tfinger;
            if !matches!(SDL_GetTouchDeviceType(finger.touchID), SDL_TOUCH_DEVICE_INDIRECT_ABSOLUTE | SDL_TOUCH_DEVICE_INDIRECT_RELATIVE) {
                return false;
            }
            self.device = Some(finger.touchID);
            self.touch(kind, finger.fingerID.0, finger.x, finger.y, finger.pressure);
            true
        }
    }

    fn touch(&mut self, kind: SDL_EventType, finger: u64, x: f32, y: f32, pressure: f32) {
        match kind {
            SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED => {
                self.fingers.remove(&finger);
                self.ids.remove(&finger);
            }
            _ => {
                let id = match self.ids.get(&finger) {
                    Some(id) => *id,
                    None => {
                        let Some(id) = (0..32).find(|id| !self.ids.values().any(|v| v == id)) else { return };
                        self.ids.insert(finger, id);
                        id
                    }
                };
                self.fingers.insert(finger, RawContact { pointer_id: id, x, y, pressure: pressure.clamp(0.0, 1.0), touching: true });
            }
        }
        self.changed = true;
    }

    /// The trackpad's click (the left mouse button while fingers are on it).
    pub fn button(&mut self, down: bool) {
        if self.button != down {
            self.button = down;
            self.changed = true;
        }
    }

    pub fn touching(&self) -> bool {
        !self.fingers.is_empty() || self.button
    }

    pub fn take(&mut self) -> Vec<Event> {
        if !std::mem::take(&mut self.changed) {
            return Vec::new();
        }
        let device = self.device.map_or(1, |id| (id.0 as usize).max(1));
        // SDL does not give the trackpad's size; the host uses a laptop touchpad's.
        vec![Event::Frame(Frame { device, contacts: Some(self.fingers.values().copied().collect()), button: self.button, width_mm: 0, height_mm: 0 })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingers_gather_into_one_frame_per_pump() {
        let mut t = Trackpad::default();
        assert!(t.take().is_empty());
        t.touch(SDL_EVENT_FINGER_DOWN, 900, 0.1, 0.2, 0.0);
        t.touch(SDL_EVENT_FINGER_DOWN, 901, 0.5, 0.5, 0.0);
        t.touch(SDL_EVENT_FINGER_MOTION, 900, 0.2, 0.2, 0.0);
        let frames = t.take();
        let [Event::Frame(frame)] = frames.as_slice() else { panic!("one frame") };
        let contacts = frame.contacts.as_ref().unwrap();
        assert_eq!(contacts.iter().map(|c| (c.pointer_id, c.x)).collect::<Vec<_>>(), [(0, 0.2), (1, 0.5)]);
        assert!(t.take().is_empty(), "nothing new");
        // The click rides on the frame; a lifted finger frees its id.
        t.button(true);
        t.touch(SDL_EVENT_FINGER_UP, 900, 0.2, 0.2, 0.0);
        t.touch(SDL_EVENT_FINGER_DOWN, 902, 0.9, 0.9, 0.0);
        let frames = t.take();
        let [Event::Frame(frame)] = frames.as_slice() else { panic!("one frame") };
        assert!(frame.button);
        assert_eq!(frame.contacts.as_ref().unwrap().iter().map(|c| c.pointer_id).collect::<Vec<_>>(), [1, 0]);
        assert!(t.touching());
    }
}
