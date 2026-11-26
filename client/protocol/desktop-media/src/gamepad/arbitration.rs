//! Gamepads and USB forwarding ([`crate::usb_arbitration`]): a controller forwarded over USB to
//! the host is closed here, so the host does not get it twice, and reopened as soon as the
//! forwarding stops.

use nightjar_core::input::InputQueue;
use sdl3_sys::everything::*;

use super::{Gamepads, NotOpened};
use crate::usb_arbitration::{self, PadIdentity};

/// What SDL tells of a gamepad before it is opened: vendor, product and the busid behind its path.
fn pad_identity(id: SDL_JoystickID) -> PadIdentity {
    // SAFETY: SDL queries by instance id on the main thread; the path is copied at once.
    unsafe {
        let path = SDL_GetGamepadPathForID(id);
        let path = (!path.is_null()).then(|| std::ffi::CStr::from_ptr(path).to_string_lossy().into_owned());
        PadIdentity {
            vid: SDL_GetGamepadVendorForID(id),
            pid: SDL_GetGamepadProductForID(id),
            busid: path.as_deref().and_then(usb_arbitration::busid_of_path),
        }
    }
}

impl Gamepads {
    /// The open pads with what SDL tells of them.
    fn open_identities(&self) -> Vec<(SDL_JoystickID, PadIdentity)> {
        self.pads.iter().map(|pad| (pad.id, pad_identity(pad.id))).collect()
    }

    /// Whether `id`, being added, is a controller forwarded over USB: it is kept paused and
    /// left closed.
    pub(super) fn forwarded_here(&mut self, id: SDL_JoystickID) -> bool {
        let forwarded = self.arbitration.forwarded();
        if forwarded.is_empty() { return false; }
        let identity = pad_identity(id);
        let mut open = self.open_identities();
        open.push((id, identity.clone()));
        let paused: Vec<_> = self.paused.iter().filter(|(p, _)| *p != id).cloned().collect();
        if !usb_arbitration::plan(&open, &paused, &forwarded).pause.contains(&id) { return false; }
        eprintln!("nightjar: controller {}: forwarded over USB to the PC; not read here until that stops", id.0);
        self.paused.retain(|(p, _)| *p != id);
        self.paused.push((id, identity));
        true
    }

    /// Whether USB forwarding changed since [`Self::arbitrate`] last ran: one atomic load.
    pub fn arbitration_due(&self) -> bool {
        self.arbitration.generation() != self.arbitrated
    }

    /// Pauses the open controllers that are now forwarded over USB and reopens the paused ones
    /// that no longer are. `queue`, during a stream that sends controllers, gets each paused
    /// one's release and removal and the reopened ones' arrival. What could not be reopened,
    /// for the user.
    pub fn arbitrate(&mut self, mut queue: Option<&mut InputQueue>) -> Vec<NotOpened> {
        let generation = self.arbitration.generation();
        if generation == self.arbitrated { return Vec::new(); }
        self.arbitrated = generation;
        let forwarded = self.arbitration.forwarded();
        let open = self.open_identities();
        let plan = usb_arbitration::plan(&open, &self.paused, &forwarded);
        for id in plan.pause {
            let Some((_, identity)) = open.iter().find(|(p, _)| *p == id).cloned() else { continue };
            eprintln!("nightjar: controller {}: forwarded over USB to the PC; paused here", id.0);
            if let Some(queue) = queue.as_deref_mut() { self.release_mouse(id, queue); }
            let slot = self.removed(id);
            if let (Some(slot), Some(queue)) = (slot, queue.as_deref_mut()) { self.send_removed(slot, queue); }
            self.paused.push((id, identity));
        }
        let mut failed = Vec::new();
        for id in plan.resume {
            self.paused.retain(|(p, _)| *p != id);
            // SAFETY: an SDL query by instance id. One unplugged meanwhile has nothing to reopen.
            if !unsafe { SDL_IsGamepad(id) } { continue; }
            eprintln!("nightjar: controller {}: no longer forwarded over USB; reading it here again", id.0);
            if let Err(why) = self.added(id) { failed.push(why); }
        }
        if let Some(queue) = queue { self.announce_new(queue); }
        self.arbitration.applied(generation);
        failed
    }

    /// The controllers paused while forwarded over USB.
    pub fn paused_count(&self) -> usize {
        self.paused.len()
    }
}

#[cfg(test)]
mod tests;
