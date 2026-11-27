//! Controller audio with an SDL virtual gamepad: Bluetooth audio whose reports stopped is
//! dropped, and the controller goes to the PC again without the PCM capability.

use std::time::{Duration, Instant};

use nightjar_core::control::haptics::{Pcm, CAPABILITY};

use super::super::protocol::describe;
use super::super::*;
use crate::dualsense_bt_audio::{BtAudio, Output};

/// Takes the probe, then no report.
struct Refusing(bool);

impl Output for Refusing {
    fn write(&mut self, _: &[u8]) -> bool {
        std::mem::replace(&mut self.0, false)
    }
}

#[test]
fn bluetooth_audio_whose_reports_stopped_is_dropped_and_the_controller_announced_again() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    // SAFETY: SDL calls on this test's thread, which holds the SDL test lock.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.name = c"Nightjar audio pad".as_ptr();
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        pads.pads[0].bt_audio = Some(BtAudio::start(Refusing(true)).unwrap());
        pads.pads[0].announced = true;
        assert_ne!(describe(&pads.pads[0]).2 & CAPABILITY, 0, "it plays PCM");
        assert!(!pads.reap_bt_audio(), "nothing stopped yet");
        // Sound arrives and every report after the probe is refused: the worker stops.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sequence = 0;
        while let Some(audio) = pads.pads[0].bt_audio.as_ref().filter(|audio| audio.alive())
            && Instant::now() < deadline
        {
            audio.offer(&Pcm { controller: 0, sequence, samples: vec![0x10; 960] }, true);
            sequence += 1;
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pads.reap_bt_audio(), "its stopped audio is noticed");
        let pad = &pads.pads[0];
        assert!(pad.bt_audio.is_none() && !pad.announced, "dropped, and to be announced again");
        assert_eq!(describe(pad).2 & CAPABILITY, 0, "the new arrival asks for no PCM");
        assert!(!pads.reap_bt_audio());
        pads.removed(id);
        SDL_DetachVirtualJoystick(id);
    }
}
