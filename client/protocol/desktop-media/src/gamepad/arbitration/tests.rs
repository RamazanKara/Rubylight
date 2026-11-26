//! SDL virtual gamepad tests for USB forwarding arbitration: a forwarded controller is paused
//! here and comes back on every way the forwarding ends.

use nightjar_core::control::HostCapabilities;

use super::super::*;
use crate::usb_arbitration::{Forwarded, Registry};

const DUALSENSE: (u16, u16) = (0x054c, 0x0ce6);
const XBOX: (u16, u16) = (0x045e, 0x0b12);

/// A virtual gamepad SDL reports with `vid:pid` and no path (so no busid).
unsafe fn virtual_pad((vid, pid): (u16, u16)) -> SDL_JoystickID {
    // SAFETY: the caller holds the SDL test lock; the descriptor outlives the attach.
    unsafe {
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.vendor_id = vid;
        desc.product_id = pid;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
        desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
        desc.name = c"Nightjar arbitration pad".as_ptr();
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        id
    }
}

fn forwarded((vid, pid): (u16, u16)) -> Forwarded {
    Forwarded { vid, pid, busid: "1-2".into() }
}

fn open_ids(pads: &Gamepads) -> Vec<u32> {
    pads.pads.iter().map(|p| p.id.0).collect()
}

/// Paused while forwarded, closed for SDL's re-adds too, and reopened by the claim's release
/// (a detach), by the end of the stream's panel, and never left paused once unplugged.
#[test]
fn a_forwarded_controller_pauses_and_comes_back_whenever_forwarding_stops() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    // SAFETY: SDL calls on this test's thread.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let registry = Registry::new();
        let mut pads = Gamepads { arbitration: registry.clone(), ..Default::default() };
        let dualsense = virtual_pad(DUALSENSE);
        let xbox = virtual_pad(XBOX);
        pads.added(dualsense).unwrap();
        pads.added(xbox).unwrap();
        assert!(!pads.arbitration_due());
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], feature_flags: 0x03, ..Default::default() });
        pads.announce_all(&mut queue);

        // Detach: the claim is released.
        let claim = registry.claim(registry.owner(), forwarded(DUALSENSE));
        assert!(pads.arbitration_due());
        assert!(pads.arbitrate(Some(&mut queue)).is_empty());
        assert!(!pads.arbitration_due());
        assert_eq!(open_ids(&pads), vec![xbox.0], "only the forwarded controller pauses");
        assert_eq!(pads.paused_count(), 1);
        pads.added(dualsense).unwrap();
        assert_eq!(open_ids(&pads), vec![xbox.0], "SDL adding it again keeps it closed while forwarded");
        claim.release();
        assert!(pads.arbitrate(Some(&mut queue)).is_empty());
        assert_eq!(pads.paused_count(), 0);
        let back = pads.pads.iter().find(|p| p.id == dualsense).expect("reopened once forwarding stopped");
        assert!(back.announced, "the PC is told it arrived again");

        // The stream ends: its panel releases everything it claimed, whatever holds the devices.
        let owner = registry.owner();
        let held = registry.claim(owner, forwarded(DUALSENSE));
        pads.arbitrate(None);
        assert_eq!(open_ids(&pads), vec![xbox.0]);
        registry.release_owner(owner);
        pads.arbitrate(None);
        assert!(open_ids(&pads).contains(&dualsense.0), "back after the stream's end");
        drop(held);

        // Unplugged while paused: nothing stays paused, and nothing fails to reopen.
        let claim = registry.claim(registry.owner(), forwarded(DUALSENSE));
        pads.arbitrate(None);
        assert_eq!(pads.paused_count(), 1);
        SDL_DetachVirtualJoystick(dualsense);
        assert_eq!(pads.removed(dualsense), None, "a paused controller has no slot to free");
        assert_eq!(pads.paused_count(), 0);
        drop(claim);
        assert!(pads.arbitrate(None).is_empty());
        assert_eq!(open_ids(&pads), vec![xbox.0]);

        // A forwarded device of another model leaves every controller alone.
        let _webcam = registry.claim(registry.owner(), forwarded((0x046d, 0x085e)));
        pads.arbitrate(None);
        assert_eq!(open_ids(&pads), vec![xbox.0]);

        pads.close_all();
        SDL_DetachVirtualJoystick(xbox);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}
