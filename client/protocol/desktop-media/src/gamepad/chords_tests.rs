//! SDL virtual gamepad tests for round-trip input, rumble, quit combos, Guide hold, and Back-as-Guide.

use nightjar_core::input::packets::{button, capability};

use super::*;

static RUMBLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TRIGGERS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn record_rumble(_: *mut std::ffi::c_void, low: u16, high: u16) -> bool {
    RUMBLE.store((u32::from(low) << 16) | u32::from(high), std::sync::atomic::Ordering::SeqCst);
    true
}

unsafe extern "C" fn record_triggers(_: *mut std::ffi::c_void, left: u16, right: u16) -> bool {
    TRIGGERS.store((u32::from(left) << 16) | u32::from(right), std::sync::atomic::Ordering::SeqCst);
    true
}

/// An SDL virtual gamepad: arrival, a button press and stick motion reach the host's
/// input queue, and the host's rumble reaches the device.
#[test]
fn virtual_gamepad_round_trip() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
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
        desc.Rumble = Some(record_rumble);
        desc.RumbleTriggers = Some(record_triggers);
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());

        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        assert_eq!(pads.pads.len(), 1, "{}", crate::sdl::error());
        let mut queue = InputQueue::new(&HostCapabilities {
            app_version: [7, 1, 431, -1],
            feature_flags: 0x03,
            ..Default::default()
        });
        let before = queue.len();
        pads.announce_all(&mut queue);
        assert!(queue.len() > before, "arrival queued");

        let joystick = SDL_GetGamepadJoystick(pads.pads[0].handle);
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
        SDL_SetJoystickVirtualAxis(joystick, SDL_GAMEPAD_AXIS_LEFTY.0, -32768);
        SDL_UpdateJoysticks();
        let queued = queue.len();
        let mut event: SDL_Event = std::mem::zeroed();
        while SDL_PollEvent(&mut event) {
            pads.handle(&event, &mut queue);
        }
        assert_eq!(pads.pads[0].buttons, button::A);
        assert_eq!(pads.pads[0].ls.1, 32767, "stick up is positive for the host");
        assert!(queue.len() > queued, "controller state queued");

        pads.pads[0].mapping.buttons[0] = Some(button::X);
        pads.pads[0].mapping.buttons[1] = Some(button::X);
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_EAST.0, true);
        SDL_UpdateJoysticks();
        while SDL_PollEvent(&mut event) { pads.handle(&event, &mut queue); }
        assert_eq!(pads.pads[0].buttons, button::X);
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_SOUTH.0, false);
        SDL_UpdateJoysticks();
        while SDL_PollEvent(&mut event) { pads.handle(&event, &mut queue); }
        assert_eq!(pads.pads[0].buttons, button::X, "a second physical button still holds the target");
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_EAST.0, false);
        SDL_UpdateJoysticks();
        while SDL_PollEvent(&mut event) { pads.handle(&event, &mut queue); }
        assert_eq!(pads.pads[0].buttons, 0);

        pads.rumble(0, 0x1234, 0x5678);
        assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);
        // A feel reaches the motors through its curve: Strong lifts a weak level and keeps full
        // strength, Subtle caps it.
        pads.rumble_feel = nightjar_policy::rumble::RumbleFeel::Strong;
        pads.rumble(0, 0x1000, 0xFFFF);
        let strong = RUMBLE.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(strong >> 16, u32::from(nightjar_policy::rumble::shape(0x1000, nightjar_policy::rumble::RumbleFeel::Strong)));
        assert!(strong >> 16 > 0x1000, "Strong lifts a weak level");
        assert_eq!(strong & 0xFFFF, 0xFFFF);
        pads.rumble_feel = nightjar_policy::rumble::RumbleFeel::Subtle;
        pads.rumble(0, 0xFFFF, 0xFFFF);
        assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), (39_321 << 16) | 39_321);
        pads.rumble_feel = nightjar_policy::rumble::RumbleFeel::Standard;
        pads.rumble(0, 0x1234, 0x5678);
        assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);
        pads.disable_rumble = true;
        pads.rumble(0, 0x4321, 0x8765);
        assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);

        // A pad with trigger motors tells the host so, and the host's trigger
        // rumble (0x5500) reaches SDL's own trigger call, not the grip motors.
        assert_ne!(
            describe(&pads.pads[0]).2 & capability::TRIGGER_RUMBLE,
            0,
            "the pad's arrival says it has trigger rumble"
        );
        pads.disable_rumble = false;
        pads.rumble_triggers(0, 0x1234, 0x5678);
        assert_eq!(TRIGGERS.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);
        pads.disable_rumble = true;
        pads.rumble_triggers(0, 0x4321, 0x8765);
        assert_eq!(
            TRIGGERS.load(std::sync::atomic::Ordering::SeqCst),
            0x1234_5678,
            "Turn off vibration stops trigger rumble too"
        );
        pads.disable_rumble = false;

        pads.release(&mut queue);
        assert_eq!(pads.pads[0].buttons, 0);
        assert_eq!(pads.pads[0].ls, (0, 0));
        pads.pads[0].mapping.buttons[SDL_GAMEPAD_BUTTON_START.0 as usize] = Some(0);
        pads.pads[0].mapping.buttons[SDL_GAMEPAD_BUTTON_BACK.0 as usize] = Some(0);
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_START.0, true);
        SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_BACK.0, true);
        SDL_UpdateJoysticks();
        let mut menu = false;
        while SDL_PollEvent(&mut event) {
            menu |= pads.handle(&event, &mut queue) == Some(Action::Menu);
        }
        assert!(menu);
        assert_eq!(pads.pads[0].buttons, 0);
        pads.back_opens_menu = true;
        let event = SDL_Event { gbutton: SDL_GamepadButtonEvent { r#type: SDL_EVENT_GAMEPAD_BUTTON_DOWN,
            which: id, button: SDL_GAMEPAD_BUTTON_BACK.0 as u8, down: true, ..Default::default() } };
        assert_eq!(pads.handle(&event, &mut queue), Some(Action::Menu));
        assert_eq!(pads.pads[0].buttons, 0);

        pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// A chord the host may claim (its menu opens on L3+R3 held by default, and the chord can
/// change) is ordinary input here: both stick clicks reach the host for as long as they are
/// held, and no Nightjar action comes of it.
#[test]
fn stick_clicks_held_together_reach_the_host_and_are_no_action() {
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
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
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        let mut queue = InputQueue::new(&HostCapabilities {
            app_version: [7, 1, 431, -1],
            feature_flags: 0x03,
            ..Default::default()
        });
        pads.announce_all(&mut queue);
        let joystick = SDL_GetGamepadJoystick(pads.pads[0].handle);
        let mut event: SDL_Event = std::mem::zeroed();
        for stick in [SDL_GAMEPAD_BUTTON_LEFT_STICK, SDL_GAMEPAD_BUTTON_RIGHT_STICK] {
            SDL_SetJoystickVirtualButton(joystick, stick.0, true);
            SDL_UpdateJoysticks();
            let queued = queue.len();
            while SDL_PollEvent(&mut event) {
                assert_eq!(pads.handle(&event, &mut queue), None);
            }
            assert!(queue.len() > queued, "each click is sent");
        }
        assert_eq!(pads.pads[0].buttons, button::LS_CLK | button::RS_CLK);
        // Held for two seconds: nothing local fires, and both stay down for the host.
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(!pads.update_mouse(&mut queue));
        assert_eq!(pads.pads[0].buttons, button::LS_CLK | button::RS_CLK);
        for stick in [SDL_GAMEPAD_BUTTON_LEFT_STICK, SDL_GAMEPAD_BUTTON_RIGHT_STICK] {
            SDL_SetJoystickVirtualButton(joystick, stick.0, false);
        }
        SDL_UpdateJoysticks();
        while SDL_PollEvent(&mut event) {
            assert_eq!(pads.handle(&event, &mut queue), None);
        }
        assert_eq!(pads.pads[0].buttons, 0);

        pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// Every quit combo in the settings leaves the stream when its last button goes down, and only
/// the chosen one does; the menu chord and the stream menu's mouse switch keep working.
#[test]
fn the_chosen_quit_combo_leaves_the_stream() {
    use crate::config::GamepadQuitCombo;
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
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
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], feature_flags: 0x03, ..Default::default() });
        pads.announce_all(&mut queue);
        let press = |pads: &mut Gamepads, queue: &mut InputQueue, button: i32, down: bool| {
            let event = SDL_Event { gbutton: SDL_GamepadButtonEvent { r#type: if down { SDL_EVENT_GAMEPAD_BUTTON_DOWN } else { SDL_EVENT_GAMEPAD_BUTTON_UP },
                which: id, button: button as u8, down, ..Default::default() } };
            pads.handle(&event, queue)
        };
        // Shoulders first so Start+Select alone (the menu chord) is never all that is held.
        let order = |mask: u32| {
            let mut buttons: Vec<i32> = (0..32).filter(|b| mask & 1 << b != 0).collect();
            buttons.sort_by_key(|b| ![SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0].contains(b));
            buttons
        };
        for chosen in GamepadQuitCombo::ALL {
            pads.quit_combo = chosen;
            for combo in GamepadQuitCombo::ALL {
                let buttons = order(quit_mask(combo));
                let mut actions = Vec::new();
                for button in &buttons { actions.push(press(&mut pads, &mut queue, *button, true)); }
                for button in &buttons { press(&mut pads, &mut queue, *button, false); }
                let quit = actions.last() == Some(&Some(Action::Quit));
                assert_eq!(quit, combo == chosen, "{combo:?} held with {chosen:?} chosen");
                assert!(actions[..actions.len() - 1].iter().all(Option::is_none), "{combo:?}: {actions:?}");
                assert_eq!(pads.pads[0].raw_buttons, 0);
            }
            assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, true), None);
            assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_BACK.0, true), Some(Action::Menu), "{chosen:?}");
            press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, false);
            press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_BACK.0, false);
        }
        // With Back opening the stream controls, a combo with Select still leaves the stream,
        // Select before or after its other button, and Back never reaches the PC; Back alone
        // opens the controls on press, Back over a held shoulder on release.
        pads.back_opens_menu = true;
        let back = SDL_GAMEPAD_BUTTON_BACK.0;
        for chosen in [GamepadQuitCombo::StartSelectLbRb, GamepadQuitCombo::SelectLbRbY] {
            pads.quit_combo = chosen;
            for reversed in [false, true] {
                let mut buttons = order(quit_mask(chosen));
                if reversed { buttons[2..].reverse(); }
                let mut actions = Vec::new();
                for button in &buttons {
                    actions.push(press(&mut pads, &mut queue, *button, true));
                    assert_eq!(pads.pads[0].buttons & button::BACK, 0, "Back stays Nightjar's");
                }
                for button in &buttons { actions.push(press(&mut pads, &mut queue, *button, false)); }
                assert_eq!(actions[buttons.len() - 1], Some(Action::Quit), "{chosen:?} {buttons:?}: {actions:?}");
                assert_eq!(actions.iter().filter(|a| a.is_some()).count(), 1, "{chosen:?} {buttons:?}: {actions:?}");
                assert_eq!(pads.pads[0].raw_buttons, 0);
            }
            assert_eq!(press(&mut pads, &mut queue, back, true), Some(Action::Menu), "{chosen:?}");
            assert_eq!(press(&mut pads, &mut queue, back, false), None);
            assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, true), None);
            assert_eq!(press(&mut pads, &mut queue, back, true), None, "{chosen:?}: Back waits over a shoulder");
            assert_eq!(press(&mut pads, &mut queue, back, false), Some(Action::Menu), "{chosen:?}");
            assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, false), None);
        }
        // A combo without Select: Back opens the controls on press even over its buttons.
        pads.quit_combo = GamepadQuitCombo::StartLbRbA;
        assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, true), None);
        assert_eq!(press(&mut pads, &mut queue, back, true), Some(Action::Menu));
        assert_eq!(press(&mut pads, &mut queue, back, false), None);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, false);
        assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, true), None);
        assert_eq!(press(&mut pads, &mut queue, back, true), Some(Action::Menu), "the menu chord still opens the controls");
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, false);
        press(&mut pads, &mut queue, back, false);
        pads.back_opens_menu = false;

        assert_eq!(quit_mask(GamepadQuitCombo::StartSelectLbRb), 1 << SDL_GAMEPAD_BUTTON_START.0 | 1 << SDL_GAMEPAD_BUTTON_BACK.0
            | 1 << SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0 | 1 << SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0, "the default stays moonlight-qt's");

        assert!(!pads.mouse_active());
        assert_eq!(pads.toggle_mouse(&mut queue), Some(true));
        assert!(pads.mouse_active());
        assert_eq!(pads.toggle_mouse(&mut queue), Some(false));
        assert!(!pads.mouse_active());

        pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
        assert_eq!(Gamepads::default().toggle_mouse(&mut queue), None, "no controller, no switch");
    }
}

/// Holding Guide for the chosen time opens the stream menu and the PC never sees it; a short
/// press still reaches the PC whole, Guide in a chord goes through, and Off sends it on press
/// (nightjar#63).
#[test]
fn holding_guide_opens_the_menu_and_a_short_press_reaches_the_pc() {
    use crate::config::GamepadQuitCombo;
    use std::time::{Duration, Instant};
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
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
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        let mut pads = Gamepads::default();
        pads.added(id).unwrap();
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], feature_flags: 0x03, ..Default::default() });
        pads.announce_all(&mut queue);
        let press = |pads: &mut Gamepads, queue: &mut InputQueue, button: i32, down: bool| {
            let event = SDL_Event { gbutton: SDL_GamepadButtonEvent { r#type: if down { SDL_EVENT_GAMEPAD_BUTTON_DOWN } else { SDL_EVENT_GAMEPAD_BUTTON_UP },
                which: id, button: button as u8, down, ..Default::default() } };
            pads.handle(&event, queue)
        };
        let guide = SDL_GAMEPAD_BUTTON_GUIDE.0;

        // Off: Guide reaches the PC on press, as before.
        assert_eq!(press(&mut pads, &mut queue, guide, true), None);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL);
        press(&mut pads, &mut queue, guide, false);
        assert_eq!(pads.pads[0].buttons, 0);
        assert!(!pads.guide_tick(Instant::now() + Duration::from_secs(5), &mut queue));

        pads.guide_hold = Some(Duration::from_secs(1));
        // A short press: held back on press, sent on release, let go after the tap time.
        let queued = queue.len();
        assert_eq!(press(&mut pads, &mut queue, guide, true), None);
        assert_eq!(pads.pads[0].buttons, 0);
        assert_eq!(queue.len(), queued, "nothing goes out while it may still be a hold");
        assert!(pads.guide_waiting());
        assert!(!pads.guide_tick(Instant::now(), &mut queue));
        press(&mut pads, &mut queue, guide, false);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL, "the short press reaches the PC");
        assert!(queue.len() > queued);
        assert!(!pads.guide_tick(Instant::now(), &mut queue));
        assert_eq!(pads.pads[0].buttons, button::SPECIAL, "held long enough for the PC to see");
        assert!(!pads.guide_tick(Instant::now() + GUIDE_TAP, &mut queue));
        assert_eq!(pads.pads[0].buttons, 0);
        assert!(!pads.guide_waiting());

        // A hold: the tick opens the menu once, and the release is swallowed.
        let queued = queue.len();
        press(&mut pads, &mut queue, guide, true);
        assert!(!pads.guide_tick(Instant::now() + Duration::from_millis(900), &mut queue));
        assert!(pads.guide_tick(Instant::now() + Duration::from_millis(1100), &mut queue));
        assert!(!pads.guide_tick(Instant::now() + Duration::from_millis(1200), &mut queue), "opens once");
        assert_eq!(press(&mut pads, &mut queue, guide, false), None);
        assert_eq!(pads.pads[0].buttons, 0);
        assert_eq!(queue.len(), queued, "the PC never sees a held Guide");
        assert!(!pads.guide_waiting());

        // Guide then A: a chord for the PC, Guide first.
        press(&mut pads, &mut queue, guide, true);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL | button::A);
        assert!(!pads.guide_tick(Instant::now() + Duration::from_secs(2), &mut queue), "a chord never opens the menu");
        press(&mut pads, &mut queue, guide, false);
        assert_eq!(pads.pads[0].buttons, button::A);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, false);
        // A then Guide: Guide joins the held button right away.
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
        press(&mut pads, &mut queue, guide, true);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL | button::A);
        press(&mut pads, &mut queue, guide, false);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, false);
        assert_eq!(pads.pads[0].raw_buttons, 0);

        // The quit combos and the menu chord still work.
        for combo in GamepadQuitCombo::ALL {
            pads.quit_combo = combo;
            let mut buttons: Vec<i32> = (0..32).filter(|b| quit_mask(combo) & 1 << b != 0).collect();
            buttons.sort_by_key(|b| ![SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0].contains(b));
            let actions: Vec<_> = buttons.iter().map(|b| press(&mut pads, &mut queue, *b, true)).collect();
            for b in &buttons { press(&mut pads, &mut queue, *b, false); }
            assert_eq!(actions.last(), Some(&Some(Action::Quit)), "{combo:?}");
        }
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, true);
        assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_BACK.0, true), Some(Action::Menu));
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, false);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_BACK.0, false);

        // Opening the menu releases everything, a held Guide too.
        press(&mut pads, &mut queue, guide, true);
        pads.release(&mut queue);
        assert!(!pads.guide_waiting());

        // Turned off mid-press: the held Guide is dropped, not chorded into the next button.
        // The default is 1 s, as on Android.
        pads.configure(&crate::config::Settings::default());
        assert_eq!(pads.guide_hold, Some(Duration::from_secs(1)));
        press(&mut pads, &mut queue, guide, true);
        pads.configure(&crate::config::Settings { guide_hold_ms: 0, ..Default::default() });
        assert!(!pads.guide_waiting());
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
        assert_eq!(pads.pads[0].buttons, button::A);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, false);
        press(&mut pads, &mut queue, guide, false);
        assert_eq!(pads.pads[0].buttons, 0);

        pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}

/// With "Back button as Guide" a held Back opens the stream menu as a held Guide does, a short
/// press reaches the PC as Guide, and the quit combo with Select still leaves the stream.
#[test]
fn back_as_guide_holds_for_the_menu_and_keeps_the_quit_combo() {
    use crate::config::{GamepadQuitCombo, Settings};
    use std::time::{Duration, Instant};
    let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
    use nightjar_core::control::HostCapabilities;
    // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
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
        let id = SDL_AttachVirtualJoystick(&desc);
        assert!(id.0 != 0, "{}", crate::sdl::error());
        let mut pads = Gamepads::default();
        // Guide held for 1 s opens the menu by default.
        pads.configure(&Settings { back_as_guide: true, controller_mouse: false, ..Default::default() });
        pads.added(id).unwrap();
        let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], feature_flags: 0x03, ..Default::default() });
        pads.announce_all(&mut queue);
        let press = |pads: &mut Gamepads, queue: &mut InputQueue, button: i32, down: bool| {
            let event = SDL_Event { gbutton: SDL_GamepadButtonEvent { r#type: if down { SDL_EVENT_GAMEPAD_BUTTON_DOWN } else { SDL_EVENT_GAMEPAD_BUTTON_UP },
                which: id, button: button as u8, down, ..Default::default() } };
            pads.handle(&event, queue)
        };
        let back = SDL_GAMEPAD_BUTTON_BACK.0;

        // A short press: kept back while it may be a hold, then Guide for the PC, let go after the tap time.
        assert_eq!(press(&mut pads, &mut queue, back, true), None);
        assert_eq!(pads.pads[0].buttons, 0);
        press(&mut pads, &mut queue, back, false);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL);
        assert!(!pads.guide_tick(Instant::now() + GUIDE_TAP, &mut queue));
        assert_eq!((pads.pads[0].buttons, pads.pads[0].raw_buttons), (0, 0));
        // A hold opens the menu, and the PC never sees it.
        press(&mut pads, &mut queue, back, true);
        assert!(pads.guide_tick(Instant::now() + Duration::from_millis(1100), &mut queue));
        assert_eq!(press(&mut pads, &mut queue, back, false), None);
        assert_eq!(pads.pads[0].buttons, 0);
        // Over a held button Back goes to the PC as Guide at once.
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
        press(&mut pads, &mut queue, back, true);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL | button::A);
        press(&mut pads, &mut queue, back, false);
        press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_SOUTH.0, false);
        assert_eq!((pads.pads[0].buttons, pads.pads[0].raw_buttons), (0, 0));

        // The quit combo with Select still leaves the stream, Select first or last.
        pads.quit_combo = GamepadQuitCombo::StartSelectLbRb;
        let shoulders = [SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0];
        for order in [[back, shoulders[0], shoulders[1], SDL_GAMEPAD_BUTTON_START.0], [shoulders[0], shoulders[1], SDL_GAMEPAD_BUTTON_START.0, back]] {
            let actions: Vec<_> = order.iter().map(|b| press(&mut pads, &mut queue, *b, true)).collect();
            for b in &order { press(&mut pads, &mut queue, *b, false); }
            assert_eq!(actions.last(), Some(&Some(Action::Quit)), "{order:?}: {actions:?}");
            assert!(actions[..3].iter().all(Option::is_none), "{order:?}: {actions:?}");
            assert_eq!(pads.pads[0].raw_buttons, 0);
            assert!(!pads.guide_waiting());
        }

        // Without the hold Back reaches the PC as Guide on press.
        pads.configure(&Settings { back_as_guide: true, guide_hold_ms: 0, controller_mouse: false, ..Default::default() });
        assert_eq!(press(&mut pads, &mut queue, back, true), None);
        assert_eq!(pads.pads[0].buttons, button::SPECIAL);
        press(&mut pads, &mut queue, back, false);
        assert_eq!(pads.pads[0].buttons, 0);
        // "Back opens the stream menu" wins: Back is Nightjar's and the PC gets nothing.
        pads.configure(&Settings { back_as_guide: true, back_opens_menu: true, controller_mouse: false, ..Default::default() });
        assert_eq!(press(&mut pads, &mut queue, back, true), Some(Action::Menu));
        assert_eq!(pads.pads[0].buttons, 0);
        press(&mut pads, &mut queue, back, false);

        pads.close_all();
        SDL_DetachVirtualJoystick(id);
        SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
    }
}
