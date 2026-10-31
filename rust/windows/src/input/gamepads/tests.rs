use super::*;
#[test]
fn feedback_failures_warn_per_pad_without_hiding_healthy_reports_or_flooding_logs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("feedback.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let active = BTreeMap::from([(0, 2), (1, 5)]);
        let mut last = BTreeMap::new();
        let mut failures = BTreeMap::new();
        let now = Instant::now();
        for millis in [0, 1, 4999, 5000] {
            let output = poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(millis),
                |slot| {
                    if slot == 2 || millis >= 4999 {
                        bail!("injected poll failure");
                    }
                    Ok(Some((1, vec![7, 8])))
                },
            );
            assert_eq!(
                output,
                if millis == 0 {
                    vec![(1, 1, vec![7, 8])]
                } else {
                    vec![]
                }
            );
        }
        assert_eq!(
            poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(5001),
                |_| Ok(Some((1, vec![7, 8])))
            ),
            vec![(0, 1, vec![7, 8])]
        );
        assert!(
            poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(5002),
                |_| bail!("intermittent poll failure")
            )
            .is_empty()
        );
        let log = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 3, "{log}");
        for line in &lines {
            assert!(line.contains("WARN"));
            assert!(line.contains("controller feedback poll failed"));
            assert!(line.contains("backend=\"fake\""));
            assert!(line.contains("injected poll failure"));
        }
        assert!(lines[0].contains("controller=0 slot=2"));
        assert!(lines[0].contains("failures=1"));
        assert!(lines[1].contains("controller=1 slot=5"));
        assert!(lines[1].contains("failures=1"));
        assert!(lines[2].contains("controller=0 slot=2"));
        assert!(lines[2].contains("failures=3"));

        poll_feedback(
            &BTreeMap::new(),
            &mut last,
            &mut failures,
            "fake",
            now + Duration::from_millis(5003),
            |_| unreachable!(),
        );
        assert!(last.is_empty());
        assert!(failures.is_empty());
        poll_feedback(
            &active,
            &mut last,
            &mut failures,
            "fake",
            now + Duration::from_millis(5004),
            |_| bail!("replugged pad failure"),
        );
        let log = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 5, "{log}");
        assert!(lines[3..].iter().all(|line| line.contains("failures=1")));

        // Failures held back by the interval are reported once the pads
        // recover, and only once.
        for (millis, result) in [(5005, false), (5006, true), (10_004, true), (10_005, true)] {
            poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(millis),
                |_| {
                    if result {
                        Ok(None)
                    } else {
                        bail!("held back failure")
                    }
                },
            );
        }
        let log = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 7, "{log}");
        assert!(lines[5].contains("controller=0 slot=2"));
        assert!(lines[6].contains("controller=1 slot=5"));
        assert!(lines[5..].iter().all(|line| {
            line.contains("controller feedback polls recovered after failing")
                && line.contains("failures=1")
        }));
        assert!(failures.values().all(|failure| failure.unreported == 0));
    });
}
fn submit_touch(
    pointers: &mut BTreeMap<(u8, u32), u8>,
    profile: u16,
    global: u16,
    input: &Event,
) -> Option<Vec<u8>> {
    submit_touch_on(pointers, profile, global, input, false)
}
fn submit_touch_on(
    pointers: &mut BTreeMap<(u8, u32), u8>,
    profile: u16,
    global: u16,
    input: &Event,
    dual: bool,
) -> Option<Vec<u8>> {
    let update = gamepad_touch_request(pointers, profile, global, input, dual)?;
    let packet = update.packet.clone();
    update.submitted(pointers);
    Some(packet)
}
#[test]
fn failed_touch_submission_does_not_acquire_or_release_contact_slots() {
    let mut pointers = BTreeMap::new();
    drop(gamepad_touch_request(&pointers, 6, 2, &touch(2, 0, 1, 17), false).unwrap());
    assert!(pointers.is_empty());
    submit_touch(&mut pointers, 6, 2, &touch(2, 0, 1, 17)).unwrap();
    for event in [2, 4, 6, 7] {
        drop(gamepad_touch_request(&pointers, 6, 2, &touch(2, 0, event, 17), false).unwrap());
        assert_eq!(pointers, BTreeMap::from([((2, 17), 0)]));
    }
}
#[test]
fn ds4_keeps_primary_contact_slots_and_ignores_secondary_surfaces() {
    let mut pointers = BTreeMap::new();
    for pointer in [17, 18] {
        submit_touch(&mut pointers, 5, 2, &touch(2, 0, 1, pointer)).unwrap();
    }
    assert!(submit_touch(&mut pointers, 5, 2, &touch(2, 0, 1, 19)).is_none());
    assert!(submit_touch(&mut pointers, 5, 2, &touch(2, 1, 7, 17)).is_none());
    assert_eq!(pointers.len(), 2);
    submit_touch(&mut pointers, 5, 2, &touch(2, 0, 7, 17)).unwrap();
    assert!(pointers.is_empty());
}
#[test]
fn two_announced_touchpads_share_the_surface_side_by_side() {
    let mut pointers = BTreeMap::new();
    // A Steam Deck's trackpads: the same finger id on each, both down.
    let left = submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 1, 0), true).unwrap();
    let right = submit_touch_on(&mut pointers, 6, 5, &touch(2, 1, 1, 0), true).unwrap();
    assert_eq!(&left[12..14], &[0, 1]);
    assert_eq!(&right[12..14], &[1, 1]);
    // x = 0.25 of each half is 1/8 of the surface and 5/8 of it.
    assert_eq!(&left[14..16], &(8191u16).to_le_bytes());
    assert_eq!(&right[14..16], &(40959u16).to_le_bytes());
    assert_eq!(pointers.len(), 2);
    // Lifting one finger frees only its contact.
    let up = submit_touch_on(&mut pointers, 6, 5, &touch(2, 1, 2, 0), true).unwrap();
    assert_eq!(&up[12..14], &[1, 2]);
    assert_eq!(pointers, BTreeMap::from([((2, 0), 0)]));
    let moved = submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 3, 0), true).unwrap();
    assert_eq!(&moved[12..14], &[0, 3]);
    // Neither a third finger nor a third touchpad finds a slot or a place.
    submit_touch_on(&mut pointers, 6, 5, &touch(2, 1, 1, 1), true).unwrap();
    assert!(submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 1, 1), true).is_none());
    assert!(submit_touch_on(&mut pointers, 6, 5, &touch(2, 2, 1, 2), true).is_none());
    // Cancelling clears both touchpads' contacts.
    submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 7, 0), true).unwrap();
    assert!(pointers.is_empty());
}
#[test]
fn the_right_touchpad_stays_ignored_when_the_client_did_not_announce_two() {
    let mut pointers = BTreeMap::new();
    assert!(submit_touch_on(&mut pointers, 5, 2, &touch(2, 1, 1, 0), false).is_none());
    let full = submit_touch_on(&mut pointers, 5, 2, &touch(2, 0, 1, 0), false).unwrap();
    // A single touchpad keeps the whole surface.
    assert_eq!(&full[14..16], &(16383u16).to_le_bytes());
    assert_eq!(pointers.len(), 1);
    // Pads without a touch surface take no touch even when announced dual.
    for profile in [3, 4, 7] {
        assert!(submit_touch_on(&mut pointers, profile, 2, &touch(2, 1, 1, 5), true).is_none());
    }
}
fn touch(id: u8, touchpad: u8, event: u8, pointer: u32) -> Event {
    Event::ControllerTouch {
        id,
        touchpad,
        event,
        pointer,
        x: 0.25,
        y: 0.5,
        pressure: 1.0,
    }
}
#[test]
fn secondary_touchpad_cannot_move_release_or_cancel_primary_contacts() {
    let mut pointers = BTreeMap::new();
    let first = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 17)).unwrap();
    assert_eq!(first.len(), 22);
    assert_eq!(&first[8..12], &5u32.to_le_bytes());
    assert_eq!(&first[12..], &[0, 1, 255, 63, 255, 127, 255, 255, 0, 0]);
    for index in [1, 2, 255] {
        for event in 0..=7 {
            let before = pointers.clone();
            assert!(submit_touch(&mut pointers, 6, 5, &touch(2, index, event, 17)).is_none());
            assert_eq!(pointers, before);
        }
    }
    let moved = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 3, 17)).unwrap();
    assert_eq!(&moved[12..14], &[0, 3]);
    let second = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 18)).unwrap();
    assert_eq!(&second[12..14], &[1, 1]);
    assert!(submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 19)).is_none());
    let released = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 2, 17)).unwrap();
    assert_eq!(&released[12..14], &[0, 2]);
    let reused = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 19)).unwrap();
    assert_eq!(&reused[12..14], &[0, 1]);
}
#[test]
fn virtual_touch_mapping_keeps_controller_lifecycles_and_profiles_separate() {
    let mut pointers = BTreeMap::new();
    for id in [2, 3] {
        submit_touch(&mut pointers, 5, u16::from(id), &touch(id, 0, 1, 17)).unwrap();
    }
    let cancel = submit_touch(&mut pointers, 5, 2, &touch(2, 0, 7, 0)).unwrap();
    assert_eq!(&cancel[12..14], &[0, 5]);
    assert_eq!(pointers, BTreeMap::from([((3, 17), 0)]));
    for profile in [1, 2, 3, 4, 7, 8] {
        assert!(submit_touch(&mut pointers, profile, 3, &touch(3, 0, 7, 0)).is_none());
        assert_eq!(pointers, BTreeMap::from([((3, 17), 0)]));
    }
    let cancel_contact = submit_touch(&mut pointers, 6, 3, &touch(3, 0, 6, 17)).unwrap();
    assert_eq!(&cancel_contact[12..14], &[0, 4]);
    assert!(pointers.is_empty());
}
#[test]
fn release_skips_a_pad_that_holds_nothing_and_stays_loud_for_one_that_does() {
    // A backend without a driver handle: every report to it fails.
    let mut pads = Gamepads::open_options(
        butterpollo_core::input_policy::USBIP_DUALSENSE,
        butterpollo_core::input_policy::Policy::resolve(&Default::default()).unwrap(),
        Default::default(),
    )
    .unwrap();
    pads.profile = 0;
    // Plugged by a launch's controller preparation, then released at park.
    pads.active.insert(0, 15);
    pads.profiles.insert(0, 1);
    pads.release_all().unwrap();
    assert_eq!(pads.active.get(&0), Some(&15), "still plugged");

    let neutral = Event::Controller {
        id: 0,
        active: 1,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    pads.states.insert(0, (neutral, Default::default()));
    assert!(
        pads.release_all().is_err(),
        "a pad with a state is released"
    );
    pads.states.clear();
    pads.pointers.insert((0, 17), 0);
    assert!(
        pads.release_all().is_err(),
        "a pad with a touch is released"
    );
    // Drop would unplug through the missing handle and strand slot 15.
    pads.active.clear();
}
/// Gamepads whose driver handle is missing: every plug and unplug fails.
fn broken_pads() -> Gamepads {
    let mut pads = Gamepads::open_options(
        butterpollo_core::input_policy::USBIP_DUALSENSE,
        butterpollo_core::input_policy::Policy::resolve(&Default::default()).unwrap(),
        Default::default(),
    )
    .unwrap();
    pads.profile = 0;
    pads
}
#[test]
fn a_pad_that_will_not_unplug_at_session_end_keeps_its_slot_until_it_does() {
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
#[test]
fn a_nintendo_controller_without_the_switch_profile_is_warned_about() {
    use butterpollo_core::input_policy::{VHF_DS4, VHF_DUALSENSE, VHF_SWITCH, VHF_XBOX_SERIES};
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
#[test]
fn a_failed_plug_reports_the_drivers_error_not_full_slots() {
    let mut pads = broken_pads();
    pads.available = !0;
    let error = format!("{:#}", pads.ensure(0).unwrap_err());
    assert!(error.contains("could not plug"), "{error}");
    assert!(!error.contains("no free"), "{error}");
    assert!(pads.active.is_empty());
}
