//! Controller audio with an SDL virtual gamepad: Bluetooth audio whose reports stopped is
//! dropped, the controller goes to the PC again without the PCM capability, and the audio stays
//! off until the controller reconnects or the next stream.

use std::time::{Duration, Instant};

use nightjar_core::control::haptics::{CAPABILITY, Pcm};

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
            audio.offer(
                &Pcm {
                    controller: 0,
                    sequence,
                    samples: vec![0x10; 960],
                },
                true,
            );
            sequence += 1;
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pads.reap_bt_audio(), "its stopped audio is noticed");
        let pad = &pads.pads[0];
        assert!(
            pad.bt_audio.is_none() && !pad.announced,
            "dropped, and to be announced again"
        );
        assert_eq!(
            describe(pad).2 & CAPABILITY,
            0,
            "the new arrival asks for no PCM"
        );
        assert!(!pads.reap_bt_audio());
        // Controller and audio device events, and a controller back from USB forwarding, run
        // auto_audio: it leaves this one off rather than fail and say so again a second later.
        assert!(pads.bt_audio_failed(id), "remembered");
        pads.auto_audio();
        assert!(
            pads.pads[0].bt_audio.is_none() && pads.bt_audio_failed(id),
            "still off"
        );
        // Reconnecting (a new instance id) or the next stream tries again.
        pads.removed(id);
        assert!(!pads.bt_audio_failed(id), "forgotten once removed");
        pads.bt_audio_failed.push(id);
        pads.prepare_audio(false);
        assert!(!pads.bt_audio_failed(id), "forgotten at the next stream");
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// Takes every report without failing.
struct Accepting;

impl Output for Accepting {
    fn write(&mut self, _: &[u8]) -> bool {
        true
    }
}

static BT_RUMBLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static BT_RUMBLE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "C" fn record_bt_rumble(_: *mut std::ffi::c_void, low: u16, high: u16) -> bool {
    BT_RUMBLE.store(
        (u32::from(low) << 16) | u32::from(high),
        std::sync::atomic::Ordering::SeqCst,
    );
    BT_RUMBLE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    true
}

#[test]
fn bluetooth_haptics_zeroes_and_suppresses_sdl_rumble_while_active() {
    use std::sync::atomic::Ordering;
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    BT_RUMBLE.store(0, Ordering::SeqCst);
    BT_RUMBLE_CALLS.store(0, Ordering::SeqCst);
    // SAFETY: SDL calls on this test's thread, which holds the SDL test lock.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.name = c"Nightjar BT haptics pad".as_ptr();
        desc.Rumble = Some(record_bt_rumble);
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());

        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        pads.pads[0].bt_audio = Some(BtAudio::start(Accepting).unwrap());

        // Before BT haptics activates, SDL rumble goes through normally.
        pads.rumble(0, 0x4000, 0x8000);
        assert_eq!(BT_RUMBLE.load(Ordering::SeqCst), 0x4000_8000);
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 1);

        // Speaker audio alone neither zeroes nor suppresses SDL rumble.
        pads.pcm(
            Pcm {
                controller: 0,
                sequence: 0,
                samples: vec![0x10; 960],
            },
            true,
        );
        assert_eq!(BT_RUMBLE.load(Ordering::SeqCst), 0x4000_8000);
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 1);
        pads.rumble(0, 0x3000, 0x5000);
        assert_eq!(BT_RUMBLE.load(Ordering::SeqCst), 0x3000_5000);
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 2);

        // Activating BT haptics zeroes active SDL rumble once and suppresses further SDL rumble.
        pads.pcm(
            Pcm {
                controller: 0,
                sequence: 0,
                samples: vec![0x10; 960],
            },
            false,
        );
        assert_eq!(BT_RUMBLE.load(Ordering::SeqCst), 0);
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 3);
        pads.rumble(0, 0x7000, 0x9000);
        assert_eq!(BT_RUMBLE.load(Ordering::SeqCst), 0);
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 3);

        // Subsequent haptics packets while active do not spam SDL_RumbleGamepad(0, 0, 0).
        pads.pcm(
            Pcm {
                controller: 0,
                sequence: 1,
                samples: vec![0x20; 960],
            },
            false,
        );
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 3);

        // Once BT haptics goes idle (> 100 ms), ordinary SDL rumble works again.
        std::thread::sleep(Duration::from_millis(130));
        pads.rumble(0, 0x2000, 0x1000);
        assert_eq!(BT_RUMBLE.load(Ordering::SeqCst), 0x2000_1000);
        assert_eq!(BT_RUMBLE_CALLS.load(Ordering::SeqCst), 4);

        pads.removed(id);
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
    crate::dualsense_bt_audio::settle(Duration::from_secs(2));
}

#[test]
fn bt_audio_open_failure_and_refused_probe_are_recorded_and_not_retried() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    // SAFETY: SDL calls on this test's thread, which holds the SDL test lock.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.vendor_id = 0x054c;
        desc.product_id = 0x0ce6;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.name = c"DualSense BT test pad".as_ptr();
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());

        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        // Force BT capability check if SDL virtual joystick exposes USB/unknown connection.
        // Test both synchronous open failure and async probe failure via `reap_bt_audio`.
        if pads.pads[0].supports_bt_audio() {
            let mut attempts = 0usize;
            pads.open_bt_audio_with(|_| {
                attempts += 1;
                Err("no HID device".into())
            });
            assert_eq!(attempts, 1);
            assert!(pads.bt_audio_failed(id), "open failure must be recorded in bt_audio_failed");
            pads.open_bt_audio_with(|_| {
                attempts += 1;
                Err("should not retry".into())
            });
            assert_eq!(attempts, 1, "failed pad must not be retried on subsequent auto_audio calls");
            pads.bt_audio_failed.clear();
        }

        // Async probe failure on worker thread is reaped and recorded in bt_audio_failed.
        pads.pads[0].bt_audio = Some(BtAudio::start(Refusing(false)).unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        while pads.pads[0].bt_audio.as_ref().is_some_and(BtAudio::alive) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pads.reap_bt_audio(), "refused async probe is reaped");
        assert!(pads.pads[0].bt_audio.is_none());
        assert!(pads.bt_audio_failed(id), "reaped probe failure is recorded");

        pads.removed(id);
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
    crate::dualsense_bt_audio::settle(Duration::from_secs(2));
}
