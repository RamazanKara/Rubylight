use super::*;

#[test]
fn stick_y_inverts_without_overflow() {
    assert_eq!(invert(-32768), 32767);
    assert_eq!(invert(32767), -32767);
    assert_eq!(invert(0), 0);
}

static RUMBLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn record_rumble(_: *mut std::ffi::c_void, low: u16, high: u16) -> bool {
    RUMBLE.store((u32::from(low) << 16) | u32::from(high), std::sync::atomic::Ordering::SeqCst);
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
        pads.disable_rumble = true;
        pads.rumble(0, 0x4321, 0x8765);
        assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);

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

#[test]
fn button_map_follows_sdl_order() {
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_SOUTH.0 as usize], button::A);
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_START.0 as usize], button::PLAY);
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_DPAD_RIGHT.0 as usize], button::RIGHT);
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_TOUCHPAD.0 as usize], button::TOUCHPAD);
}

#[test]
fn mappings_apply_once_preserve_explicit_bindings_and_scale_dead_zones() {
    let mut mapping = Mapping::default();
    assert_eq!(mapping.map_buttons(1, true), button::B);
    mapping.buttons[0] = Some(button::X);
    mapping.buttons[2] = Some(button::Y);
    assert_eq!(mapping.map_buttons(1, true), button::X);
    assert_eq!(mapping.map_buttons(1 | 4, false), button::X | button::Y);
    mapping.axes = [-4, 3, 0, -1, 6, 0];
    let report = mapping.apply(0, [-32768, 1000, -2000, 32767, 0, 32767], PadSettings::default());
    assert_eq!(report.ls, (-32767, 2000));
    assert_eq!(report.rs, (0, -32767));
    assert_eq!((report.lt, report.rt), (255, 0));
    assert_eq!(deadzone((3000, 3000), 20), (0, 0));
    assert_eq!((trigger(1, 0), trigger(-5, 0), trigger(32767, 0)), (0, 0, 255));
    assert_eq!(trigger(3300, 0), 25, "no dead zone by default: a 10% press goes through");
    assert_eq!((trigger(3276, 10), trigger(3400, 10), trigger(32767, 10)), (0, 1, 255));
    assert_eq!(trigger(19660, 20), 127, "60% raw is 50% of the travel past a 20% dead zone");
    mapping.trigger_deadzone = 20;
    // lt reads source 6 here
    assert_eq!(mapping.apply(0, [0, 0, 0, 0, 0, 6000], PadSettings::default()).lt, 0);
    assert_eq!(mapping.apply(0, [0, 0, 0, 0, 0, 19660], PadSettings::default()).lt, 127);
    assert!(mapping.valid());
    mapping.trigger_deadzone = MAX_TRIGGER_DEADZONE + 1;
    assert!(!mapping.valid());
    mapping.trigger_deadzone = 0;
    assert_eq!(deadzone((32767, 0), 20), (32767, 0));
    let middle = deadzone((20000, 0), 20);
    assert!((16000..18000).contains(&middle.0));
    assert_eq!(middle.1, 0);
    assert!(mapping.valid());
    mapping.axes[4] = -6;
    assert!(!mapping.valid());
    mapping.axes[4] = 6;
    mapping.buttons[0] = Some(button::A | button::B);
    assert!(!mapping.valid());
}

/// "Stick dead zone" and "Trigger dead zone" in the settings apply to a controller whose
/// buttons page sets none of its own; a mapping's own value wins, each on its own.
#[test]
fn the_settings_dead_zones_fill_in_for_a_mapping_without_its_own() {
    let settings = crate::config::Settings { deadzone: 20, trigger_deadzone: 10, ..Default::default() };
    let applied = PadSettings::from(&settings);
    assert_eq!(applied.dead_zones, DeadZones { stick: 20, trigger: 10 });
    let plain = Mapping::default();
    assert_eq!(plain.dead_zones(applied.dead_zones), DeadZones { stick: 20, trigger: 10 });
    // Inside the settings' 20 % stick and 10 % trigger dead zones nothing reaches the PC.
    let movement = [3000, -3000, 0, 0, 3000, 3000];
    let report = plain.apply(0, movement, applied);
    assert_eq!((report.ls, report.lt, report.rt), ((0, 0), 0, 0));
    // Without them the same movement goes through.
    let raw = plain.apply(0, movement, PadSettings::default());
    assert_eq!(raw.ls, (3000, 3000));
    assert!(raw.lt > 0 && raw.rt > 0);
    // A mapping's own value wins over the settings'.
    let own_stick = Mapping { deadzone: 5, ..Mapping::default() };
    assert_eq!(own_stick.dead_zones(applied.dead_zones), DeadZones { stick: 5, trigger: 10 });
    assert_eq!(own_stick.apply(0, [3000, 0, 0, 0, 0, 0], applied).ls, deadzone((3000, 0), 5));
    let own_trigger = Mapping { trigger_deadzone: 30, ..Mapping::default() };
    assert_eq!(own_trigger.dead_zones(applied.dead_zones), DeadZones { stick: 20, trigger: 30 });
    // Settings past a mapping's range are held to it, and the defaults change nothing.
    let wide = PadSettings::from(&crate::config::Settings { deadzone: 90, trigger_deadzone: 90, ..Default::default() });
    assert_eq!(wide.dead_zones, DeadZones { stick: MAX_STICK_DEADZONE, trigger: MAX_TRIGGER_DEADZONE });
    assert_eq!(PadSettings::from(&crate::config::Settings::default()), PadSettings::default());
    assert_eq!(PadSettings::from(true), PadSettings { swap: true, ..PadSettings::default() });
}

/// "Back button as Guide": the physical Back reaches the PC as Guide, through what the mapping
/// sends for Guide; "Back opens the stream menu" keeps Back for Nightjar instead, as on Android.
#[test]
fn back_as_guide_sends_guide_for_back() {
    let back = 1 << SDL_GAMEPAD_BUTTON_BACK.0;
    let on = PadSettings { back_as_guide: true, ..PadSettings::default() };
    let mapping = Mapping::default();
    assert_eq!(mapping.apply(back, [0; 6], PadSettings::default()).buttons, button::BACK);
    assert_eq!(mapping.apply(back, [0; 6], on).buttons, button::SPECIAL);
    assert_eq!(mapping.apply(back | 1, [0; 6], on).buttons, button::SPECIAL | button::A, "other buttons stay");
    assert_eq!(mapping.apply(1 << SDL_GAMEPAD_BUTTON_GUIDE.0, [0; 6], on).buttons, button::SPECIAL, "Guide stays Guide");
    let mut guide_bound = Mapping::default();
    guide_bound.buttons[SDL_GAMEPAD_BUTTON_GUIDE.0 as usize] = Some(button::MISC);
    assert_eq!(guide_bound.apply(back, [0; 6], on).buttons, button::MISC, "Guide's binding applies");
    let from = |back_as_guide, back_opens_menu| {
        PadSettings::from(&crate::config::Settings { back_as_guide, back_opens_menu, ..Default::default() }).back_as_guide
    };
    assert!(from(true, false));
    assert!(!from(true, true), "Back opens the stream menu wins");
    assert!(!from(false, false) && !from(false, true));
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
        (*userdata.cast::<AtomicU32>()).store((u32::from(low) << 16) | u32::from(high), Ordering::SeqCst);
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
        let numbers = |pads: &Gamepads| pads.pads.iter().map(|p| (p.index, p.sent_as())).collect::<Vec<_>>();
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

#[test]
fn a_pinned_type_replaces_the_detected_one_and_automatic_keeps_it() {
    let mut mapping = Mapping::default();
    assert_eq!(mapping.connect_as, CTYPE_UNKNOWN);
    for detected in [CTYPE_UNKNOWN, CTYPE_XBOX, CTYPE_PS, CTYPE_NINTENDO] {
        assert_eq!(mapping.arrival_type(detected), detected);
    }
    // #53: a pad SDL reports as PlayStation can be told to the host as Xbox.
    mapping.connect_as = CTYPE_XBOX;
    assert_eq!(mapping.arrival_type(CTYPE_PS), CTYPE_XBOX);
    assert_eq!(mapping.arrival_type(CTYPE_UNKNOWN), CTYPE_XBOX);
    mapping.connect_as = CTYPE_PS;
    assert_eq!(mapping.arrival_type(CTYPE_XBOX), CTYPE_PS);
    mapping.connect_as = CTYPE_NINTENDO;
    assert_eq!(mapping.arrival_type(CTYPE_XBOX), CTYPE_NINTENDO);
    assert!(mapping.valid());
    // A value this version does not know is Automatic and keeps the mapping valid.
    mapping.connect_as = 9;
    assert_eq!(mapping.arrival_type(CTYPE_PS), CTYPE_PS);
    assert!(mapping.valid());
    assert_eq!(CONNECT_AS.map(|(kind, _)| kind), [0, 1, 2, 3]);
}

#[test]
fn an_xbox_pin_arrives_without_motion_or_touchpad() {
    let all = capability::ANALOG_TRIGGERS | capability::RUMBLE | capability::TOUCHPAD
        | capability::DUAL_TOUCHPAD | capability::ACCEL | capability::GYRO | capability::BATTERY_STATE;
    let mut mapping = Mapping::default();
    // Automatic and the other types keep what the pad has.
    assert_eq!(mapping.arrival_capabilities(all), all);
    for kind in [CTYPE_PS, CTYPE_NINTENDO] {
        mapping.connect_as = kind;
        assert_eq!(mapping.arrival_capabilities(all), all);
        assert!(!mapping.drops_motion_and_touchpad());
    }
    // Rubylight turns an Xbox pad with motion or a touchpad into a DualSense.
    mapping.connect_as = CTYPE_XBOX;
    assert!(mapping.drops_motion_and_touchpad());
    assert_eq!(mapping.arrival_capabilities(all),
        capability::ANALOG_TRIGGERS | capability::RUMBLE | capability::BATTERY_STATE);
}

#[test]
fn select_lb_sends_the_first_button_then_clicks_the_touchpad_of_a_pad_pinned_as_playstation() {
    let mut mapping = Mapping { connect_as: CTYPE_PS, ..Default::default() };
    assert!(mapping.emulates_clickpad(false));
    assert!(!mapping.emulates_clickpad(true), "a pad with a touchpad click keeps it");
    mapping.buttons[0] = Some(button::TOUCHPAD);
    assert!(!mapping.emulates_clickpad(false), "a button mapped to the touchpad replaces the combo");
    assert!(!Mapping::default().emulates_clickpad(false));
    assert!(!Mapping { connect_as: CTYPE_XBOX, ..Default::default() }.emulates_clickpad(false));

    let (back, lb) = (button::BACK, button::LB);
    let mut swallowed = 0;
    // The first button of the combo goes through: holding it back would add latency.
    assert_eq!(clickpad(back, &mut swallowed), back);
    assert_eq!(clickpad(back | lb, &mut swallowed), button::TOUCHPAD);
    assert_eq!(clickpad(back | lb | button::A, &mut swallowed), button::TOUCHPAD | button::A);
    // Letting go of LB lifts the click; Back stays silent until it is let go too.
    assert_eq!(clickpad(back, &mut swallowed), 0);
    assert_eq!(clickpad(back | lb, &mut swallowed), lb);
    assert_eq!(clickpad(0, &mut swallowed), 0);
    assert_eq!(swallowed, 0);
    // Select+LB as part of a larger chord is left alone.
    assert_eq!(clickpad(back | lb | button::PLAY, &mut swallowed), back | lb | button::PLAY);
}

#[test]
fn dualsense_effect_changes_only_requested_triggers() {
    let effect = trigger_effect(0xFF, 0x21, 0x26, &[3; 10], &[7; 10]);
    assert_eq!(effect[0], 0x0C);
    assert_eq!(&effect[1..10], &[0; 9]);
    assert_eq!(effect[10], 0x26);
    assert_eq!(&effect[11..21], &[7; 10]);
    assert_eq!(effect[21], 0x21);
    assert_eq!(&effect[22..32], &[3; 10]);
    assert_eq!(&effect[32..], &[0; 15]);
}
