//! A physical touchpad's fingers (`SOURCE_TOUCHPAD` with pointer capture, "Touchpad gestures
//! on the PC") for a host that takes touchpad packets: the app hands over each motion event's
//! fingers as a frame, and nightjar-core's tracker turns the frames into contact changes.

use std::sync::Mutex;
use std::time::Instant;

use nightjar_core::input::touchpad::{Frame, Out, RawContact, Tracker, Transport};
use nightjar_core::input::InputError;

/// The tracker of the session it belongs to: a new session starts with none held.
static TRACKER: Mutex<Option<(u64, Tracker)>> = Mutex::new(None);

/// `RustBridge.TOUCHPAD_*`: how the host takes touchpad contacts.
pub const TRANSPORT_NONE: i32 = 0;
pub const TRANSPORT_INDIVIDUAL: i32 = 1;
pub const TRANSPORT_FRAME: i32 = 2;

pub fn transport_code(feature_flags: u32) -> i32 {
    match Transport::of(feature_flags) {
        None => TRANSPORT_NONE,
        Some(Transport::Individual) => TRANSPORT_INDIVIDUAL,
        Some(Transport::Frame) => TRANSPORT_FRAME,
    }
}

/// The fingers of one motion event, positions already on the touchpad's surface (0-1).
pub fn contacts(ids: &[i32], xs: &[f32], ys: &[f32], pressures: &[f32]) -> Vec<RawContact> {
    ids.iter().zip(xs).zip(ys).zip(pressures).map(|(((&id, &x), &y), &pressure)| RawContact {
        pointer_id: id.max(0) as u32,
        x,
        y,
        pressure,
        touching: true,
    }).collect()
}

/// What the session's tracker sends for `f`; a tracker of another session is dropped first.
pub fn track(session: u64, f: impl FnOnce(&mut Tracker) -> Vec<Out>) -> Vec<Out> {
    let mut guard = TRACKER.lock().unwrap_or_else(|p| p.into_inner());
    if guard.as_ref().map_or(true, |(id, _)| *id != session) {
        *guard = Some((session, Tracker::default()));
    }
    let (_, tracker) = guard.as_mut().expect("set above");
    f(tracker)
}

/// The tracker's output for one frame of `device`.
pub fn frame(session: u64, device: i32, contacts: Vec<RawContact>, button: bool, width_mm: u16, height_mm: u16) -> Vec<Out> {
    let frame = Frame { device: device.max(1) as usize, contacts: Some(contacts), button, width_mm, height_mm };
    track(session, |t| t.frame(frame, Instant::now()))
}

/// Sends the tracker's output on the session's input queue.
pub fn send(outs: Vec<Out>, queue: &mut nightjar_core::input::InputQueue) -> Result<(), InputError> {
    if outs.is_empty() {
        return Ok(());
    }
    match Transport::of(queue.feature_flags()) {
        Some(transport) => transport.send(outs, queue),
        None => Err(InputError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::control::negotiation::feature;
    use nightjar_core::input::packets::touch_event;

    #[test]
    fn fingers_become_contacts_and_a_new_session_starts_clean() {
        let c = contacts(&[3, 4, 5], &[0.1, 0.2], &[0.5, 0.6, 0.7], &[0.0, 1.0, 1.0]);
        assert_eq!(c.len(), 2, "arrays of different lengths give the shortest");
        assert_eq!((c[1].pointer_id, c[1].x, c[1].y, c[1].pressure), (4, 0.2, 0.6, 1.0));
        let downs = |outs: &[Out]| outs.iter().map(|o| match o {
            Out::Contacts { contacts, .. } => contacts.iter().filter(|c| c.event == touch_event::DOWN).count(),
            Out::MouseButton(_) => 0,
        }).sum::<usize>();
        assert_eq!(downs(&frame(901, 7, c.clone(), false, 100, 60)), 2);
        assert_eq!(downs(&frame(901, 7, c.clone(), false, 100, 60)), 0, "the same fingers move");
        assert_eq!(downs(&frame(902, 7, c, false, 100, 60)), 2, "another session holds nothing yet");
        assert_eq!(transport_code(0), TRANSPORT_NONE);
        assert_eq!(transport_code(feature::TOUCHPAD_EVENTS), TRANSPORT_INDIVIDUAL);
        assert_eq!(transport_code(feature::TOUCHPAD_EVENTS | feature::TOUCHPAD_FRAME_EVENTS), TRANSPORT_FRAME);
    }
}
