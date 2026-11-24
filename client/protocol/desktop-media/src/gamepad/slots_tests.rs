//! SDL virtual gamepad tests for controller limits, one-player mode, and slot promotion.

use nightjar_core::input::packets::button;

use super::*;

/// A controller left out says why: SDL's error when it cannot be opened, the limit past
/// the host's 16.
#[test]
fn a_controller_that_is_left_out_says_why() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attaches.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut pads = Gamepads::default();
        match pads.added(SDL_JoystickID(0x7fff_fff0)) {
            Err(NotOpened::Failed(error)) => assert!(!error.is_empty()),
            other => panic!("{other:?}"),
        }
        assert!(pads.pads.is_empty());

        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
        desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
        desc.name = c"Nightjar test pad".as_ptr();
        let ids: Vec<_> = (0..=MAX_PADS).map(|_| SDL_AttachVirtualJoystick(&desc)).collect();
        assert!(ids.iter().all(|id| id.0 != 0), "{}", crate::sdl::error());
        for id in &ids[..MAX_PADS] {
            pads.added(*id).unwrap();
        }
        assert_eq!(pads.added(ids[MAX_PADS]), Err(NotOpened::Limit));
        assert_eq!(pads.pads.len(), MAX_PADS);
        assert!(NotOpened::Limit.message().contains("16"));

        pads.close_all();
        for id in ids {
            SDL_DetachVirtualJoystick(id);
        }
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// "Controllers" One player: every controller reaches the PC as gamepad 0 with player 1 alone
/// in the mask, and the PC's rumble for it reaches each of them; A player each keeps a gamepad
/// per controller, and Off tells a launch of none.
#[test]
fn one_player_sends_every_controller_as_player_one() {
    use crate::config::Settings;
    use std::sync::atomic::{AtomicU32, Ordering};
    static FIRST: AtomicU32 = AtomicU32::new(0);
    static SECOND: AtomicU32 = AtomicU32::new(0);
    unsafe extern "C" fn rumble_into(userdata: *mut std::ffi::c_void, low: u16, high: u16) -> bool {
        // SAFETY: `userdata` points to one of the `'static AtomicU32`s in this test.
        unsafe { (*userdata.cast::<AtomicU32>()).store((u32::from(low) << 16) | u32::from(high), Ordering::SeqCst) };
        true
    }
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptors outlive the attaches.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let attach = |rumbled: &'static AtomicU32| {
            let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
            desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
            desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
            desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
            desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
            desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
            desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
            desc.name = c"Nightjar test pad".as_ptr();
            desc.userdata = (rumbled as *const AtomicU32).cast_mut().cast();
            desc.Rumble = Some(rumble_into);
            let id = SDL_AttachVirtualJoystick(&desc);
            assert!(id.0 != 0, "{}", crate::sdl::error());
            id
        };
        let ids = [attach(&FIRST), attach(&SECOND)];
        let one = Settings { multi_controller: false, ..Default::default() };
        let each = Settings::default();
        let off = Settings { controllers: false, multi_controller: false, ..Default::default() };
        assert!(one_player(&one) && !one_player(&each) && !one_player(&off));
        let mut pads = Gamepads::default();
        pads.configure(&one);
        for id in ids { pads.added(id).unwrap(); }
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], feature_flags: 0x03, ..Default::default() });
        pads.announce_all(&mut queue);
        let numbers = |pads: &Gamepads| pads.pads.iter().map(|p| (p.index, p.sent_as)).collect::<Vec<_>>();
        assert_eq!(numbers(&pads), [(0, 0), (1, 0)], "each keeps its slot, both are player 1");
        assert_eq!((pads.active_mask(), pads.launch_mask(&one)), (1, 1));
        let listed = pads.launch_controllers(&one);
        assert!(listed.starts_with("0:") && !listed.contains(','), "{listed}");
        pads.rumble(0, 0x1111, 0x2222);
        assert_eq!((FIRST.load(Ordering::SeqCst), SECOND.load(Ordering::SeqCst)), (0x1111_2222, 0x1111_2222));
        pads.rumble(1, 0x5555, 0x6666);
        assert_eq!(SECOND.load(Ordering::SeqCst), 0x1111_2222, "no gamepad 1");

        // A player each: a gamepad per controller, and gamepad 1's rumble reaches the second alone.
        pads.configure(&each);
        assert_eq!(numbers(&pads), [(0, 0), (1, 1)]);
        assert_eq!((pads.active_mask(), pads.launch_mask(&each)), (3, 3));
        let listed = pads.launch_controllers(&each);
        assert!(listed.starts_with("0:") && listed.contains(",1:"), "{listed}");
        pads.rumble(1, 0x3333, 0x4444);
        assert_eq!((FIRST.load(Ordering::SeqCst), SECOND.load(Ordering::SeqCst)), (0x1111_2222, 0x3333_4444));
        // Off: a launch tells the PC of none.
        assert_eq!((pads.launch_mask(&off), pads.launch_controllers(&off).as_str()), (0, ""));

        // One player again: player 1 stays while a controller is sent as it, and after the last one goes.
        pads.configure(&one);
        assert_eq!(pads.removed(ids[0]), Some(0));
        pads.send_removed(0, &mut queue);
        assert_eq!(pads.active_mask(), 1);
        assert_eq!(pads.removed(ids[1]), Some(0));
        assert_eq!(pads.active_mask(), 1, "player 1 stays, as Android keeps it");

        pads.close_all();
        for id in ids {
            SDL_DetachVirtualJoystick(id);
        }
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// GFE's GameStream takes four controllers: a fifth is not announced or sent, says so once, and
/// its removal tells the host nothing. Sunshine and its forks keep all 16.
#[test]
fn a_host_that_takes_four_controllers_gets_four() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attaches.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
        desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
        desc.name = c"Nightjar test pad".as_ptr();
        let ids: Vec<_> = (0..5).map(|_| SDL_AttachVirtualJoystick(&desc)).collect();
        assert!(ids.iter().all(|id| id.0 != 0), "{}", crate::sdl::error());

        // GFE: an appversion without Sunshine's negative fourth part.
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, 0], ..Default::default() });
        let mut pads = Gamepads::default();
        for id in &ids {
            pads.added(*id).unwrap();
        }
        assert_eq!(pads.set_limit(queue.max_controllers()), 1, "the fifth controller has no slot");
        pads.announce_all(&mut queue);
        assert_eq!(pads.active_mask(), 0x0F, "the PC is told of four players");
        assert!(pads.pads[..4].iter().all(|pad| pad.announced));
        assert!(!pads.pads[4].announced, "the fifth is not announced");
        assert_eq!(pad_slots(&pads), [Some(0), Some(1), Some(2), Some(3), None]);

        // Nothing the fifth does reaches the PC; its buttons still drive Nightjar's own menu.
        let mut out = Vec::new();
        queue.poll(std::time::Instant::now(), false, &mut out);
        let joystick = SDL_GetGamepadJoystick(pads.pads[4].handle);
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
        SDL_UpdateJoysticks();
        let mut event: SDL_Event = std::mem::zeroed();
        while SDL_PollEvent(&mut event) {
            pads.handle(&event, &mut queue);
        }
        assert!(queue.is_empty(), "no state from a controller the host has no slot for");
        assert_eq!(pads.pads[4].buttons, button::A, "the controller itself still works here");

        // One toast a stream, and a removed refused controller tells the host nothing.
        assert_eq!(pads.refused_notice().as_deref(), Some("This PC takes 4 controllers"));
        assert_eq!(pads.refused_notice(), None, "said once");
        assert_eq!(pads.removed(ids[4]), None, "the PC never knew it");
        pads.send_removed(4, &mut queue);
        assert!(queue.is_empty());

        // Sunshine takes all 16: the fifth controller gets its slot on its stream.
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], ..Default::default() });
        let mut sunshine = Gamepads::default();
        for id in &ids {
            sunshine.added(*id).unwrap();
        }
        assert_eq!(sunshine.set_limit(queue.max_controllers()), 0);
        sunshine.announce_all(&mut queue);
        assert_eq!(sunshine.active_mask(), 0x1F);
        sunshine.close_all();

        pads.close_all();
        for id in ids {
            SDL_DetachVirtualJoystick(id);
        }
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

fn pad_slots(pads: &Gamepads) -> Vec<Option<u8>> {
    pads.pads.iter().map(Pad::slot).collect()
}

/// A controller the host refused takes the slot of one that is unplugged, and the PC hears its
/// arrival: GFE with five pads, unplug the second, and the fifth becomes player 2.
#[test]
fn a_refused_controller_takes_a_freed_slot() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attaches.
    unsafe {
        assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
        let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
        desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
        desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
        desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
        desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
        desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
        desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
        desc.name = c"Nightjar test pad".as_ptr();
        let ids: Vec<_> = (0..5).map(|_| SDL_AttachVirtualJoystick(&desc)).collect();
        assert!(ids.iter().all(|id| id.0 != 0), "{}", crate::sdl::error());

        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, 0], ..Default::default() });
        let mut pads = Gamepads::default();
        for id in &ids {
            pads.added(*id).unwrap();
        }
        pads.set_limit(queue.max_controllers());
        pads.announce_all(&mut queue);
        assert_eq!(pad_slots(&pads), [Some(0), Some(1), Some(2), Some(3), None]);

        assert_eq!(pads.removed(ids[1]), Some(1), "the PC knew the unplugged one as player 2");
        assert_eq!(pad_slots(&pads), [Some(0), Some(2), Some(3), Some(1)], "the fifth took the freed slot");
        let promoted = pads.pads.iter().find(|pad| pad.id == ids[4]).unwrap();
        assert_eq!(promoted.index, 1);
        assert!(!promoted.announced, "the PC has not heard of it yet");
        assert_eq!(pads.active_mask(), 0x0F, "all four slots are players again");
        assert!(pads.refused_notice().is_none(), "no controller is left without a slot");

        // The slot's old controller is neutralised, then the new one arrives in it.
        let mut out = Vec::new();
        queue.poll(std::time::Instant::now(), false, &mut out);
        out.clear();
        pads.send_removed(1, &mut queue);
        pads.announce_new(&mut queue);
        assert!(pads.pads.iter().find(|pad| pad.id == ids[4]).unwrap().announced, "the PC heard its arrival");
        queue.poll(std::time::Instant::now(), false, &mut out);
        assert!(!out.is_empty(), "the freed slot's controller reaches the PC");

        pads.close_all();
        for id in ids {
            SDL_DetachVirtualJoystick(id);
        }
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}
