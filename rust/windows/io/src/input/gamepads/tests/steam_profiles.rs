//! Driver host priority, slot allocation, ViGEm/Switch fallbacks, and Steam controls tests.

use super::*;

#[test]
fn only_a_plugged_vhf_driver_pad_raises_the_driver_host_and_only_when_asked() {
    let config =
        butterpollo_config::config::Config::parse("gamepad_driver_host_priority=true\n").unwrap();
    let policy = butterpollo_input_policy::input_policy::Policy::resolve(&config).unwrap();
    assert!(policy.driver_host_priority);
    assert!(
        !butterpollo_input_policy::input_policy::Policy::resolve(&Default::default())
            .unwrap()
            .driver_host_priority,
        "off by default"
    );
    use std::sync::atomic::{AtomicUsize, Ordering};
    static HELD: AtomicUsize = AtomicUsize::new(0);
    struct Counted;
    impl driver_host::Held for Counted {}
    impl Drop for Counted {
        fn drop(&mut self) {
            HELD.fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn counted() -> Box<dyn driver_host::Held> {
        HELD.fetch_add(1, Ordering::SeqCst);
        Box::new(Counted)
    }
    // A USB/IP DualSense's pads are not the UMDF driver's: nothing to raise.
    let mut pads = broken_pads();
    pads.driver_host_acquire = counted;
    pads.policy = policy;
    pads.active.insert(0, 15);
    pads.follow_driver_host();
    assert_eq!(HELD.load(Ordering::SeqCst), 0);
    // The VHF driver's pads: held while one is plugged, once.
    // SAFETY: a new event stands in for the driver handle, which the
    // backend closes.
    let handle =
        unsafe { windows::Win32::System::Threading::CreateEventW(None, true, false, None) }
            .unwrap();
    pads.backend = gamepad_backend::Backend::Vhf(Some(handle));
    for _ in 0..2 {
        pads.follow_driver_host();
        assert_eq!(HELD.load(Ordering::SeqCst), 1);
    }
    pads.active.clear();
    pads.follow_driver_host();
    assert_eq!(HELD.load(Ordering::SeqCst), 0, "the last pad left");
    // Off by default: the same pads raise nothing.
    pads.policy =
        butterpollo_input_policy::input_policy::Policy::resolve(&Default::default()).unwrap();
    pads.active.insert(0, 15);
    pads.follow_driver_host();
    assert_eq!(HELD.load(Ordering::SeqCst), 0);
    pads.active.clear();
}

#[test]
fn a_pad_that_will_not_unplug_at_session_end_keeps_its_slot_until_it_does() {
    let _stranded = stranded_slots();
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    // The pad owns the slot now, as `ensure` leaves it.
    std::mem::forget(slot);
    let mut pads = broken_pads();
    pads.active.insert(0, global);
    drop(pads);
    assert!(SLOTS.lock().unwrap()[global as usize], "still reserved");
    assert!(STRANDED.lock().unwrap().contains(&global));
    // Still refused: it stays reserved.
    let mut slots = *SLOTS.lock().unwrap();
    reclaim(&mut slots, |_| anyhow::bail!("busy"));
    assert!(slots[global as usize]);
    assert!(STRANDED.lock().unwrap().contains(&global));
    // Unplugged at last: free again.
    let mut slots = SLOTS.lock().unwrap();
    let mut tried = Vec::new();
    reclaim(&mut slots, |g| {
        tried.push(g);
        if g == u32::from(global) {
            Ok(())
        } else {
            anyhow::bail!("another test's")
        }
    });
    assert!(tried.contains(&u32::from(global)));
    assert!(!slots[global as usize]);
    assert!(!STRANDED.lock().unwrap().contains(&global));
}

/// #74: a Deck client gets the pad chosen by name, whatever a Deck may get
/// otherwise, and hears nothing about one.
#[test]
fn a_chosen_vigem_pad_is_never_replaced_by_a_steam_deck_pad() {
    use butterpollo_input_policy::input_policy::{
        CTYPE_STEAM, DeckPad, VIGEM_DS4, VIGEM_PROFILES, VIGEM_X360,
    };
    for profile in [VIGEM_X360, VIGEM_DS4] {
        let mut pads = broken_pads();
        pads.profile = profile;
        pads.available = VIGEM_PROFILES;
        pads.policy.steam_deck = DeckPad::SteamDeck;
        pads.arrivals.insert(0, (CTYPE_STEAM, 0));
        assert!(!pads.ensure_deck(0).unwrap());
        assert!(pads.deck_failed.is_empty(), "it never tried");
        assert!(pads.warnings.snapshot().is_empty());
    }
}

/// #108: the ViGEmBus DualShock 4 takes touch, motion and battery like the
/// VHF PlayStation pads, its contacts with their driver slot and position.
#[test]
fn a_vigem_ds4_takes_touch_motion_and_battery() {
    use butterpollo_input_policy::input_policy::{VIGEM_DS4, VIGEM_X360};
    let pointers = BTreeMap::new();
    let update = gamepad_touch_request(&pointers, VIGEM_DS4, 3, &touch(0, 0, 1, 17), false)
        .expect("a DS4 touch");
    assert_eq!(
        (update.global, update.slot, update.event, update.position),
        (3, 0, 1, [0.25, 0.5])
    );
    // A client's second touchpad lands on the right half, as on the VHF pads.
    let update = gamepad_touch_request(&pointers, VIGEM_DS4, 3, &touch(0, 1, 1, 18), true)
        .expect("a second touchpad");
    assert_eq!(update.position, [0.625, 0.5]);
    assert!(gamepad_touch_request(&pointers, VIGEM_X360, 3, &touch(0, 0, 1, 17), false).is_none());
    let mut pads = broken_pads();
    pads.profiles.insert(0, VIGEM_DS4);
    pads.profiles.insert(1, VIGEM_X360);
    assert!(pads.motion_supported(0));
    assert!(!pads.motion_supported(1));
}

/// Vibepollo's manual DS4: Back also clicks the touchpad, for controllers
/// without a touchpad button, unless the setting is off; no other pad's
/// Back changes.
#[test]
fn back_clicks_a_vigem_ds4_touchpad_unless_turned_off() {
    use butterpollo_input_policy::input_policy::{VHF_DS4, VIGEM_DS4};
    let mut pads = broken_pads();
    pads.profiles.insert(0, VIGEM_DS4);
    pads.profiles.insert(1, VHF_DS4);
    assert_eq!(pads.profile_buttons(0, 0x1020), 0x10_1020);
    assert_eq!(pads.profile_buttons(0, 0x1000), 0x1000);
    assert_eq!(pads.profile_buttons(1, 0x20), 0x20);
    pads.policy.ds4_back_as_touchpad_click = false;
    assert_eq!(pads.profile_buttons(0, 0x20), 0x20);
}

/// The Steam Controller chosen by name is every client's pad, a Deck
/// without a Steam Controller's capabilities included.
#[test]
fn a_chosen_steam_controller_is_never_replaced_by_a_steam_deck_pad() {
    use butterpollo_input_policy::input_policy::{CTYPE_STEAM, DeckPad, VHF_STEAM_CONTROLLER};
    let mut pads = broken_pads();
    pads.profile = VHF_STEAM_CONTROLLER;
    pads.policy.steam_deck = DeckPad::SteamDeck;
    pads.arrivals.insert(0, (CTYPE_STEAM, 0));
    assert!(!pads.ensure_deck(0).unwrap());
    assert!(pads.deck_failed.is_empty(), "it never tried");
    assert!(pads.warnings.snapshot().is_empty());
}

#[test]
fn a_plug_keeps_the_slots_free_and_stops_where_the_driver_says_so() {
    let slots = std::sync::Mutex::new([false; 16]);
    slots.lock().unwrap()[0] = true;
    // Not held while the driver works, and the slot is taken by then.
    let plugged = plug_in_free_slot(&slots, true, |global| {
        assert_eq!(global, 1);
        assert!(slots.try_lock().unwrap()[1], "reserved during the plug");
        Ok(())
    });
    assert_eq!(plugged.unwrap(), Some(1));
    assert!(slots.lock().unwrap()[1]);
    // A driver for which the slot doesn't matter is asked once and the slot
    // comes back; one for which it does, in every free slot.
    let mut tried = vec![];
    let refused = plug_in_free_slot(&slots, false, |global| {
        tried.push(global);
        anyhow::bail!("refused")
    });
    assert_eq!(refused.unwrap_err().to_string(), "refused");
    assert_eq!(tried, [2]);
    assert!(!slots.lock().unwrap()[2]);
    let mut tried = vec![];
    let refused = plug_in_free_slot(&slots, true, |global| {
        tried.push(global);
        anyhow::bail!("refused {global}")
    });
    assert_eq!(refused.unwrap_err().to_string(), "refused 15");
    assert_eq!(tried, (2..16).collect::<Vec<_>>());
    assert_eq!(
        slots.lock().unwrap().iter().filter(|used| **used).count(),
        2
    );
    // No free slot is not a refusal.
    *slots.lock().unwrap() = [true; 16];
    assert_eq!(
        plug_in_free_slot(&slots, true, |_| unreachable!()).unwrap(),
        None
    );
}

/// A ViGEmBus pad goes with its client's handle: its slot isn't reserved for
/// a later unplug no other client could do.
#[test]
fn a_vigem_pad_that_will_not_unplug_frees_its_slot_with_its_client() {
    let _stranded = stranded_slots();
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut pads = broken_pads();
    pads.backend = gamepad_backend::Backend::Vigem(vigem::Client::detached().unwrap());
    pads.active.insert(0, global);
    drop(pads);
    assert!(!STRANDED.lock().unwrap().contains(&global));
    assert!(!SLOTS.lock().unwrap()[global as usize]);
}

#[test]
fn a_nintendo_controller_without_the_switch_profile_is_warned_about() {
    use butterpollo_input_policy::input_policy::{
        VHF_DS4, VHF_DUALSENSE, VHF_SWITCH, VHF_XBOX_SERIES,
    };
    assert_eq!(switch_fallback(0, 3, 0x30, VHF_SWITCH), None);
    assert_eq!(
        switch_fallback(0, 1, 0x30, VHF_DUALSENSE),
        None,
        "not Nintendo"
    );
    assert_eq!(
        switch_fallback(VHF_DS4, 3, 0x30, VHF_DS4),
        None,
        "a profile chosen in the settings"
    );
    let kept = switch_fallback(0, 3, 0x30, VHF_DUALSENSE).unwrap();
    assert!(kept.contains("a DualSense, which keeps its gyro"), "{kept}");
    let lost = switch_fallback(0, 3, 0x30, VHF_XBOX_SERIES).unwrap();
    assert!(
        lost.contains("an Xbox controller, without its gyro"),
        "{lost}"
    );
    let plain = switch_fallback(0, 3, 0, VHF_XBOX_SERIES).unwrap();
    assert!(plain.contains("an Xbox controller."), "{plain}");
}

/// Automatic pads with Steam's driver profile, and the arrival of a controller carrying
/// on-screen Steam Controller controls (or not).
fn steam_controls_pads(steam_app: bool, running: fn() -> bool) -> Gamepads {
    let mut pads = scripted_pads();
    pads.profile = 0;
    pads.available |= 1 << (butterpollo_input_policy::input_policy::VHF_STEAM_CONTROLLER - 1);
    pads.policy.steam_app = steam_app;
    pads.steam_running = running;
    pads
}

fn steam_controls_arrival(id: u8, capabilities: u16) -> Event {
    Event::Arrival {
        id,
        kind: 1,
        capabilities,
        buttons: 0x0f_0000,
    }
}

/// On-screen Steam Controller controls under Automatic in an app started through Steam:
/// without Steam running an Xbox pad and the warning; whether Steam runs is asked once per
/// connection, so Steam starting mid-stream never swaps the pad. Arriving again asks again (a
/// Steam Controller now), leaving forgets the answer, and pads without the controls never ask.
#[test]
fn steam_controls_keep_the_pad_steam_gave_them_until_they_arrive_again() {
    use butterpollo_input_policy::input_policy::{VHF_STEAM_CONTROLLER, VHF_XBOX_SERIES};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    fn running() -> bool {
        ASKED.fetch_add(1, Ordering::SeqCst);
        RUNNING.load(Ordering::SeqCst)
    }
    // Its pads plug through `ensure`, whose reclaim would free another test's stranded slot.
    let _stranded = stranded_slots();
    let mut pads = steam_controls_pads(true, running);
    let arrival = steam_controls_arrival;
    pads.apply(&arrival(0, 0x600)).unwrap();
    assert_eq!(pads.profiles[&0], VHF_XBOX_SERIES);
    assert_eq!(ASKED.load(Ordering::SeqCst), 1);
    let warnings = pads.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "input_steam_controls");
    assert!(
        warnings[0].message.contains("Steam was not running"),
        "{}",
        warnings[0].message
    );
    // Steam starts mid-stream: the pad stays in its slot, and its states ask nothing.
    let slot = pads.active[&0];
    RUNNING.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        pads.apply(&state(0, 1)).unwrap();
    }
    assert_eq!(pads.profiles[&0], VHF_XBOX_SERIES);
    assert_eq!(pads.active[&0], slot);
    assert_eq!(ASKED.load(Ordering::SeqCst), 1);
    let unplugged = format!("unplug {slot}");
    assert!(
        !script(&mut pads).calls.contains(&unplugged),
        "{:?}",
        script(&mut pads).calls
    );
    // Arriving again, as a reconnect does, asks again: a Steam Controller, the warning gone.
    pads.apply(&arrival(0, 0x600)).unwrap();
    assert_eq!(pads.profiles[&0], VHF_STEAM_CONTROLLER);
    assert_eq!(pads.steam_pads(), 1);
    assert_eq!(ASKED.load(Ordering::SeqCst), 2);
    assert!(pads.warnings.snapshot().is_empty());
    // Leaving forgets the answer.
    pads.apply(&state(0, 0)).unwrap();
    assert!(pads.steam_at_arrival.is_empty());
    assert_eq!(pads.steam_pads(), 0);
    // A touchpad without the controls, and a type chosen by name, never ask.
    pads.apply(&arrival(1, 8)).unwrap();
    pads.profile = VHF_XBOX_SERIES;
    pads.apply(&arrival(2, 0x600)).unwrap();
    assert_eq!(ASKED.load(Ordering::SeqCst), 2);
    assert!(pads.steam_at_arrival.is_empty());
}

/// #138: pads opened after the driver refused the last ones' handle get each arrival
/// replayed. It keeps the answer the pads before had, so Steam starting meanwhile never makes
/// the pad a Steam Controller mid-stream; the client's own next arrival asks again.
#[test]
fn a_replayed_arrival_keeps_the_steam_answer_of_the_refused_pads() {
    use butterpollo_input_policy::input_policy::{VHF_STEAM_CONTROLLER, VHF_XBOX_SERIES};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    fn running() -> bool {
        ASKED.fetch_add(1, Ordering::SeqCst);
        RUNNING.load(Ordering::SeqCst)
    }
    let _stranded = stranded_slots();
    let mut before = steam_controls_pads(true, running);
    before.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    assert_eq!(before.profiles[&0], VHF_XBOX_SERIES);
    assert_eq!(ASKED.load(Ordering::SeqCst), 1);
    let memory = before.memory();
    let warnings = before.warnings.clone();
    drop(before);
    RUNNING.store(true, Ordering::SeqCst);
    let mut pads = steam_controls_pads(true, running);
    pads.warnings = warnings;
    pads.remember(memory);
    pads.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    assert_eq!(
        pads.profiles[&0], VHF_XBOX_SERIES,
        "no Steam Controller mid-stream"
    );
    assert_eq!(ASKED.load(Ordering::SeqCst), 1, "not asked again");
    pads.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    assert_eq!(pads.profiles[&0], VHF_STEAM_CONTROLLER);
    assert_eq!(ASKED.load(Ordering::SeqCst), 2);
}

/// #138: the refused pads raised the warning for a controller whose on-screen Steam
/// Controller controls got another pad, and it left before its arrival was replayed. The pads
/// opened next still clear the warning.
#[test]
fn a_steam_controls_warning_from_refused_pads_clears_when_its_controller_leaves() {
    fn not_running() -> bool {
        false
    }
    let _stranded = stranded_slots();
    let mut before = steam_controls_pads(true, not_running);
    before.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    assert_eq!(before.warnings.snapshot()[0].code, "input_steam_controls");
    let memory = before.memory();
    let warnings = before.warnings.clone();
    drop(before);
    let mut pads = steam_controls_pads(true, not_running);
    pads.warnings = warnings.clone();
    pads.remember(memory);
    pads.apply(&state(0, 0)).unwrap();
    assert!(warnings.snapshot().is_empty(), "{:?}", warnings.snapshot());
}

/// #138: pads refused again before every carried arrival was replayed hand on the answers
/// not yet replayed beside their own, their own winning.
#[test]
fn memory_keeps_carried_steam_answers_not_yet_replayed() {
    fn running() -> bool {
        true
    }
    let _stranded = stranded_slots();
    let mut pads = steam_controls_pads(true, running);
    pads.remember(PadMemory {
        steam_at_arrival: [(0, false)].into(),
        ..Default::default()
    });
    pads.apply(&steam_controls_arrival(1, 0x600)).unwrap();
    assert_eq!(
        pads.memory().steam_at_arrival,
        [(0, false), (1, true)].into()
    );
    // The pads' own answer wins over a carried one.
    pads.carried_steam.insert(1, false);
    assert!(pads.memory().steam_at_arrival[&1]);
}

/// The Desktop or an app that starts a program itself: Steam would give such a program its
/// desktop layout, so the controls keep an Xbox-type pad, though Steam runs, and nobody is
/// warned or asks whether Steam runs.
#[test]
fn steam_controls_outside_a_steam_app_keep_an_xbox_pad() {
    use butterpollo_input_policy::input_policy::VHF_XBOX_SERIES;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    fn running() -> bool {
        ASKED.fetch_add(1, Ordering::SeqCst);
        true
    }
    let _stranded = stranded_slots();
    let mut pads = steam_controls_pads(false, running);
    pads.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    pads.apply(&steam_controls_arrival(1, 0x608)).unwrap();
    assert_eq!(pads.profiles[&0], VHF_XBOX_SERIES);
    assert_eq!(pads.profiles[&1], VHF_XBOX_SERIES);
    assert_eq!(ASKED.load(Ordering::SeqCst), 0);
    assert!(pads.warnings.snapshot().is_empty());
}

/// The warning lasts while any controller's controls lack their Steam Controller: one leaving
/// keeps it for the other, an arrival without the controls clears it though its pad stays,
/// and the last one leaving clears it too.
#[test]
fn the_steam_controls_warning_lasts_while_a_controller_has_it() {
    let _stranded = stranded_slots();
    let mut pads = steam_controls_pads(true, || false);
    let warned = |pads: &Gamepads| {
        pads.warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "input_steam_controls")
    };
    pads.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    pads.apply(&steam_controls_arrival(1, 0x600)).unwrap();
    assert!(warned(&pads));
    let xbox = butterpollo_input_policy::input_policy::VHF_XBOX_SERIES;
    assert_eq!((pads.profiles[&0], pads.profiles[&1]), (xbox, xbox));
    pads.apply(&state(0, 0b01)).unwrap();
    assert!(warned(&pads), "controller 0 still lacks it");
    let slot = pads.active[&0];
    pads.apply(&steam_controls_arrival(0, 0)).unwrap();
    assert_eq!(pads.active[&0], slot, "the same Xbox pad");
    assert!(!warned(&pads), "no controls, no warning");
    pads.apply(&steam_controls_arrival(0, 0x600)).unwrap();
    assert!(warned(&pads));
    pads.apply(&state(0, 0)).unwrap();
    assert!(!warned(&pads), "the last one left");
    assert!(pads.steam_fallbacks.is_empty());
}

#[test]
fn steam_controls_say_why_they_got_no_steam_controller() {
    use butterpollo_input_policy::input_policy::{
        CTYPE_STEAM, VHF_DUALSENSE, VHF_STEAM_CONTROLLER, VHF_XBOX_SERIES,
    };
    let driver = (1 << (VHF_XBOX_SERIES - 1)) | (1 << (VHF_DUALSENSE - 1));
    let with_steam = driver | (1 << (VHF_STEAM_CONTROLLER - 1));
    // The Desktop or a program started directly: an Xbox pad, as meant, without a warning.
    assert_eq!(
        steam_controls_fallback(0, 1, 0x600, false, with_steam, VHF_XBOX_SERIES),
        None
    );
    let no_steam = steam_controls_fallback(0, 1, 0x600, true, with_steam, VHF_XBOX_SERIES).unwrap();
    assert!(no_steam.contains("Steam was not running"), "{no_steam}");
    assert!(no_steam.contains("an Xbox controller"), "{no_steam}");
    assert!(
        no_steam.contains("Start Steam, then reconnect."),
        "{no_steam}"
    );
    let old_driver = steam_controls_fallback(0, 0, 0x608, true, driver, VHF_XBOX_SERIES).unwrap();
    assert!(
        old_driver.contains("no Steam Controller profile"),
        "{old_driver}"
    );
    assert!(
        old_driver.contains("Run Rubylight setup again"),
        "{old_driver}"
    );
    // Motion kept a DualSense: the warning names the pad it got.
    let gyro = steam_controls_fallback(0, 1, 0x630, true, with_steam, VHF_DUALSENSE).unwrap();
    assert!(gyro.contains("a DualSense"), "{gyro}");
    // A Steam Controller, a type chosen by name, other kinds and other pads say nothing.
    for (configured, kind, caps, profile) in [
        (0, 1, 0x600, VHF_STEAM_CONTROLLER),
        (VHF_XBOX_SERIES, 1, 0x600, VHF_XBOX_SERIES),
        (0, CTYPE_STEAM, 0x608, VHF_DUALSENSE),
        (0, 2, 0x608, VHF_DUALSENSE),
        (0, 1, 8, VHF_DUALSENSE),
    ] {
        assert_eq!(
            steam_controls_fallback(configured, kind, caps, true, with_steam, profile),
            None
        );
    }
}

#[test]
fn a_failed_plug_reports_the_drivers_error_not_full_slots() {
    let _stranded = stranded_slots();
    let mut pads = broken_pads();
    pads.available = !0;
    let error = format!("{:#}", pads.ensure(0).unwrap_err());
    assert!(error.contains("could not plug"), "{error}");
    assert!(!error.contains("no free"), "{error}");
    assert!(pads.active.is_empty());
}

#[test]
fn usbip_attach_offloads_off_calling_thread_and_strands_unpluggable_temporary_pad() {
    use butterpollo_input_policy::input_policy::CTYPE_STEAM;
    use std::sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    let _stranded = stranded_slots();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let woke = Arc::new(AtomicBool::new(false));

    let mut pads = scripted_pads();
    pads.profile = 0;
    pads.set_wake({
        let woke = Arc::clone(&woke);
        Arc::new(move || woke.store(true, Ordering::SeqCst))
    });
    pads.test_attach_hook = Some({
        let gate = Arc::clone(&gate);
        Arc::new(move |_kind, _id| {
            let (lock, cvar) = &*gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cvar.wait(open).unwrap();
            }
            Ok(())
        })
    });

    // `ensure_deck` returns immediately without blocking on the slow attach hook.
    let arrival = Event::Arrival {
        id: 0,
        kind: CTYPE_STEAM,
        capabilities: 0x608,
        buttons: 0x0f_0000,
    };
    pads.apply(&arrival).unwrap();
    assert!(
        pads.attaching.contains_key(&(attach::AttachKind::Deck, 0)),
        "USB/IP attach should run in background while apply returns immediately"
    );
    assert!(!pads.attach_ready());
    assert!(!woke.load(Ordering::SeqCst));

    // Unblock background attach and wait for completion + wake.
    {
        let (lock, cvar) = &*gate;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !pads.attach_ready() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pads.attach_ready());
    assert!(woke.load(Ordering::SeqCst));
    pads.finish_attaches().unwrap();
    assert!(!pads.attaching.contains_key(&(attach::AttachKind::Deck, 0)));
    assert!(pads.deck_failed.contains(&0));
    let temp_global = pads.active.get(&0).copied();
    drop(pads);
    if let Some(temp_global) = temp_global {
        assert!(!SLOTS.lock().unwrap()[temp_global as usize]);
    }

    // When a temporary VHF pad cannot be unplugged while switching to USB/IP,
    // `retire_vhf_for_usbip` retires it from `active` and strands its slot instead of failing.
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut broken = broken_pads();
    broken.active.insert(0, global);
    broken.profiles.insert(0, 6);
    broken.retire_vhf_for_usbip(0);
    assert!(!broken.active.contains_key(&0));
    assert!(SLOTS.lock().unwrap()[global as usize]);
    assert!(STRANDED.lock().unwrap().contains(&global));
    STRANDED.lock().unwrap().remove(&global);
    SLOTS.lock().unwrap()[global as usize] = false;
}

#[test]
fn async_attach_motion_requests_drain_once_and_clear_on_disconnect() {
    let mut pads = scripted_pads();
    // Motion-capable arrival queues a request when async attach completes.
    pads.arrivals.insert(0, (3, 0x30));
    pads.queue_motion_request(0);
    pads.queue_motion_request(0);
    // Arrival without motion flags (0x30) queues nothing.
    pads.arrivals.insert(1, (1, 0x0f));
    pads.queue_motion_request(1);

    assert_eq!(pads.motion_requests(), vec![(0, 0x30)]);
    assert!(pads.motion_requests().is_empty(), "drained once");

    // Disconnecting the controller before the worker drains drops its queued request.
    pads.queue_motion_request(0);
    pads.apply(&state(0, 0)).unwrap();
    assert!(pads.motion_requests().is_empty());
}
