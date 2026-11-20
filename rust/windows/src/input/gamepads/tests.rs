use super::*;
use std::collections::VecDeque;
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
#[test]
fn feedback_drains_each_pads_backlog_and_forwards_the_newest_of_each_kind() {
    // Pad 0 has a rumble and its stop pending; pad 1 a game's rumble
    // writes beyond what one poll reads.
    let mut queues = BTreeMap::from([
        (2u32, VecDeque::from([(4u16, vec![9; 8]), (4, vec![0; 8])])),
        (5, (0..10u8).map(|n| (4, vec![n])).collect()),
    ]);
    let active = BTreeMap::from([(0, 2), (1, 5)]);
    let mut last = BTreeMap::new();
    let mut failures = BTreeMap::new();
    let mut calls = 0;
    let mut poll = |slot| {
        calls += 1;
        Ok(queues.get_mut(&slot).unwrap().pop_front())
    };
    let output = poll_feedback(
        &active,
        &mut last,
        &mut failures,
        "fake",
        Instant::now(),
        &mut poll,
    );
    // The stop is the last rumble pad 0 sends, so its motors don't stick.
    assert_eq!(
        output,
        vec![(0, 4, vec![0; 8]), (1, 4, vec![FEEDBACK_DRAIN as u8 - 1])]
    );
    let output = poll_feedback(
        &active,
        &mut last,
        &mut failures,
        "fake",
        Instant::now(),
        &mut poll,
    );
    assert_eq!(output, vec![(1, 4, vec![9])]);
    // Pad 0: two reports and the empty answer, then one empty answer;
    // pad 1: a full drain, then the rest and the empty answer.
    assert_eq!(calls, 3 + FEEDBACK_DRAIN + 1 + 3);

    // A report read before a failing call still goes out, and the failure
    // counts.
    let mut answers = VecDeque::from([Ok(Some((4, vec![7]))), Err(anyhow::anyhow!("gone"))]);
    let output = poll_feedback(
        &BTreeMap::from([(0, 2)]),
        &mut last,
        &mut failures,
        "fake",
        Instant::now(),
        |_| answers.pop_front().unwrap(),
    );
    assert_eq!(output, vec![(0, 4, vec![7])]);
    assert_eq!(failures[&0].unreported, 0, "the first failure is logged");
    assert!(failures[&0].reported.is_some());
}
#[test]
fn a_rumble_only_report_does_not_hide_the_lights_and_trigger_effects_before_it() {
    // PlayStation reports: rumble in bytes 0-3, lightbar 4-6, flags in 7
    // (1 lightbar, 2 trigger effects), the effects from byte 10.
    let report = |rumble: u8, flags: u8| {
        let mut data = vec![0; 32];
        data[..4].fill(rumble);
        data[4..7].copy_from_slice(&[1, 2, 3]);
        data[7] = flags;
        data[10] = 0x21;
        (5u16, data)
    };
    let poll_once = |queue: Vec<(u16, Vec<u8>)>| {
        let mut queue = VecDeque::from(queue);
        poll_feedback(
            &BTreeMap::from([(0, 2)]),
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
            "fake",
            Instant::now(),
            |_| Ok(queue.pop_front()),
        )
    };
    let with = |(kind, data): (u16, Vec<u8>)| (0, kind, data);
    // Triggers off and lights, then a rumble-only report: both go, the
    // newest rumble last.
    assert_eq!(
        poll_once(vec![report(0xff, 3), report(0, 0)]),
        vec![with(report(0xff, 3)), with(report(0, 0))]
    );
    assert_eq!(
        poll_once(vec![report(0xff, 2), report(0x80, 1), report(0, 0)]),
        vec![
            with(report(0xff, 2)),
            with(report(0x80, 1)),
            with(report(0, 0))
        ]
    );
    // A newer report that carries everything an older one did replaces it.
    assert_eq!(
        poll_once(vec![report(0xff, 0), report(0x80, 1), report(0, 3)]),
        vec![with(report(0, 3))]
    );
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
#[test]
fn the_steam_controllers_contacts_are_its_two_pads() {
    use butterpollo_core::input_policy::VHF_STEAM_CONTROLLER as STEAM;
    let mut pointers = BTreeMap::new();
    // Two touchpads: each is its own pad at its full width.
    let left = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 1, 0), true).unwrap();
    let right = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 1, 1, 0), true).unwrap();
    assert_eq!(&left[12..16], &[0, 1, 255, 63]);
    assert_eq!(&right[12..16], &[1, 1, 255, 63]);
    let up = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 1, 2, 0), true).unwrap();
    assert_eq!(&up[12..14], &[1, 2]);
    assert_eq!(pointers, BTreeMap::from([((2, 0), 0)]));
    submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 7, 0), true).unwrap();
    assert!(pointers.is_empty());
    // One touchpad: its left half is the left pad, stretched to full width.
    let half = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 1, 9), false).unwrap();
    assert_eq!(&half[12..16], &[0, 1, 255, 127]);
    let mut right_half = touch(2, 0, 1, 10);
    if let Event::ControllerTouch { x, .. } = &mut right_half {
        *x = 0.75;
    }
    let right = submit_touch_on(&mut pointers, STEAM, 4, &right_half, false).unwrap();
    assert_eq!(&right[12..16], &[1, 1, 255, 127]);
    // A move keeps the pad the finger went down on.
    let mut moved = touch(2, 0, 3, 10);
    if let Event::ControllerTouch { x, .. } = &mut moved {
        *x = 0.25;
    }
    let moved = submit_touch_on(&mut pointers, STEAM, 4, &moved, false).unwrap();
    assert_eq!(&moved[12..16], &[1, 3, 0, 0]);
    assert!(submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 1, 1, 11), false).is_none());
    // A second finger on the held left pad is ignored, and the first one's
    // lift frees the pad.
    assert!(submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 1, 12), false).is_none());
    assert!(submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 2, 12), false).is_none());
    assert_eq!(pointers, BTreeMap::from([((2, 9), 0), ((2, 10), 1)]));
    submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 2, 9), false).unwrap();
    assert_eq!(pointers, BTreeMap::from([((2, 10), 1)]));
}
#[test]
fn steam_controller_feedback_is_never_coalesced_or_deduplicated() {
    let click = (6u16, [vec![4, 0, 0, 0, 0x82, 0, 2, 0], vec![0; 8]].concat());
    let settings = (7u16, vec![0x8e; 32]);
    let mut queue = VecDeque::from([
        click.clone(),
        click.clone(),
        settings.clone(),
        settings.clone(),
    ]);
    let mut last = BTreeMap::new();
    let output = poll_feedback(
        &BTreeMap::from([(0, 2)]),
        &mut last,
        &mut BTreeMap::new(),
        "fake",
        Instant::now(),
        |_| Ok(queue.pop_front()),
    );
    let with = |(kind, data): (u16, Vec<u8>)| (0, kind, data);
    assert_eq!(
        output,
        vec![
            with(click.clone()),
            with(click),
            with(settings.clone()),
            with(settings)
        ]
    );
    assert!(
        last.is_empty(),
        "a Steam report does not hide a later rumble"
    );
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
/// A Steam Controller's left grip touch and a Switch 2 Pro Controller's C
/// button share a flag; only the Steam Controller's pad may see it
/// (nightjar#125: the host masked the grip away for every pad).
#[test]
fn the_steam_controllers_left_grip_bit_reaches_its_own_pad() {
    use butterpollo_core::input_policy::VHF_STEAM_CONTROLLER;
    use butterpollo_core::switch2::BUTTON_C;
    assert_eq!(BUTTON_C, 0x0040_0000);
    assert_eq!(
        profile_button_word(Some(VHF_STEAM_CONTROLLER), BUTTON_C | 0x1000),
        BUTTON_C | 0x1000
    );
    // Every other pad has no grip sense, and its C press is not a button there.
    for profile in [None, Some(4), Some(6)] {
        assert_eq!(profile_button_word(profile, BUTTON_C | 0x1000), 0x1000);
    }
}

/// Windows retires the virtual pads when it leaves D0; the driver answers every
/// request with ERROR_DEVICE_REMOVED until its slot is created again, and no
/// other driver error is a reason to create one. The driver refusing the
/// handle itself (#138) closes the pads instead, which only its answer marks.
#[test]
fn only_the_drivers_device_removed_error_creates_the_pad_again() {
    use gamepad_backend::{driver_error, driver_lost};
    use windows::Win32::Foundation::{E_HANDLE, ERROR_DEVICE_REMOVED};
    let removed = anyhow::Error::new(windows::core::Error::from_hresult(
        ERROR_DEVICE_REMOVED.to_hresult(),
    ));
    assert!(device_removed(&removed));
    assert!(!driver_lost(&removed));
    assert!(device_removed(
        &removed.context("the gamepad driver could not report feedback")
    ));
    let refused = driver_error(windows::core::Error::from_hresult(E_HANDLE));
    assert!(!device_removed(&refused));
    assert!(driver_lost(&refused));
    assert!(format!("{refused:#}").contains("0x80070006"), "{refused:#}");
    let refused = refused.context("the gamepad driver could not plug a vhf_ds5 controller");
    assert!(driver_lost(&refused), "through added context");
    assert!(!device_removed(&refused));
    // A bare E_HANDLE, not from the driver, is neither.
    let bare = anyhow::Error::new(windows::core::Error::from_hresult(E_HANDLE));
    assert!(!device_removed(&bare));
    assert!(!driver_lost(&bare));
    assert!(!driver_lost(&driver_error(ERROR_DEVICE_REMOVED.into())));
    assert!(!device_removed(&anyhow::anyhow!("injected failure")));
    assert!(!driver_lost(&anyhow::anyhow!("injected failure")));
}

fn refused_error() -> anyhow::Error {
    gamepad_backend::driver_error(windows::core::Error::from_hresult(
        windows::Win32::Foundation::E_HANDLE,
    ))
}

/// #138: a state the driver refuses the handle for marks the pads lost for
/// the gamepad thread to close; the pad is not created again in place.
#[test]
fn a_refused_handle_marks_the_pads_lost_without_creating_the_pad_again() {
    use super::super::gamepad_thread::Pads;
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads).submits.push_back(Err(refused_error()));
    let error = Pads::apply(&mut pads, &state(0, 1)).unwrap_err();
    assert!(gamepad_backend::driver_lost(&error));
    assert!(Pads::lost(&pads));
    assert_eq!(script(&mut pads).calls, ["submit 2"], "no plug, no retry");
    assert!(!pads.recreated_at.contains_key(&0));
    // Slot 2 was never reserved here: closing must not free it.
    pads.active.clear();
    // Any other failure leaves the pads open.
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .submits
        .push_back(Err(anyhow::anyhow!("injected failure")));
    assert!(Pads::apply(&mut pads, &state(0, 1)).is_err());
    assert!(!Pads::lost(&pads));
    pads.active.clear();
}

/// #138: the refusal showed first in the feedback poll, 118 times a second.
/// One poll marks the pads lost, and none is a pad to create again.
#[test]
fn a_feedback_poll_refused_the_handle_marks_the_pads_lost() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(refused_error())]));
    assert!(pads.feedback().is_empty());
    assert!(pads.lost());
    assert_eq!(script(&mut pads).calls, ["feedback 2"], "no plug");
    pads.active.clear();
}

/// Lost pads are closed without unplug requests on the refused handle, and
/// their slots are free for the pads opened next rather than stranded.
#[test]
fn lost_pads_free_their_slots_without_unplugging() {
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut pads = scripted_pads();
    pads.active.insert(0, global);
    script(&mut pads)
        .unplugs
        .push_back(Err(anyhow::anyhow!("unplug sent on a refused handle")));
    pads.lost = true;
    drop(pads);
    assert!(!STRANDED.lock().unwrap().contains(&global));
    assert!(!SLOTS.lock().unwrap()[global as usize]);
}

/// A re-creation the driver refuses keeps the pad, so its next input tries
/// again, and a pad that is not plugged has nothing to create.
#[test]
fn a_recreate_without_the_driver_keeps_the_pad_for_the_next_attempt() {
    let mut pads = broken_pads();
    assert!(pads.recreate(0).is_ok(), "no pad, nothing to create");
    pads.active.insert(0, 15);
    pads.profiles.insert(0, 4);
    assert!(
        pads.recreate(0).is_err(),
        "the broken driver reports its error"
    );
    assert_eq!(pads.active.get(&0), Some(&15));
    assert_eq!(pads.profiles.get(&0), Some(&4));
    // A ViGEmBus pad goes through the same plug: the client replaces the
    // target in the same slot if its bus ever reports the device gone.
    pads.backend = gamepad_backend::Backend::Vigem(vigem::Client::detached().unwrap());
    pads.profiles
        .insert(0, butterpollo_core::input_policy::VIGEM_X360);
    assert!(pads.recreate(0).is_err());
    assert_eq!(pads.active.get(&0), Some(&15));
    pads.active.clear();
}

/// Windows leaves D0: the VHF driver answers input with ERROR_DEVICE_REMOVED
/// until its slot is created again. The same slot is plugged, the state is
/// retried, and no second pad is made.
#[test]
fn a_device_removed_submit_creates_the_same_pad_again_and_retries_it() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .submits
        .push_back(Err(device_removed_error()));
    pads.apply(&state(0, 1)).unwrap();
    assert_eq!(
        script(&mut pads).calls,
        ["submit 2", "plug 2", "submit 2"],
        "the same slot is created again and the state retried once"
    );
    assert_eq!(pads.active.get(&0), Some(&2), "one pad, in its slot");
    assert_eq!(pads.active.len(), 1);
    assert_eq!(pads.profiles.get(&0), Some(&4));
    assert!(pads.recreated_at.contains_key(&0));

    // A driver that is still gone waits: no second plug within the second.
    script(&mut pads)
        .submits
        .push_back(Err(device_removed_error()));
    assert!(pads.apply(&state(0, 1)).is_err());
    assert_eq!(
        script(&mut pads)
            .calls
            .iter()
            .filter(|call| call.starts_with("plug"))
            .count(),
        1,
        "throttled"
    );
    pads.active.clear();
}

/// The one-second retry belongs to the pad, not to every pad a client has: a
/// second controller that is lost while the first waits is created again at
/// once.
#[test]
fn a_recreate_waits_a_second_for_that_pad_and_not_for_the_others() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.active.insert(1, 3);
    pads.profiles.insert(0, 4);
    pads.profiles.insert(1, 4);
    for id in [0, 1] {
        script(&mut pads)
            .submits
            .push_back(Err(device_removed_error()));
        pads.apply(&state(id, 0b11)).unwrap();
    }
    assert_eq!(
        script(&mut pads).calls,
        [
            "submit 2", "plug 2", "submit 2", "submit 3", "plug 3", "submit 3"
        ]
    );
    let now = Instant::now();
    assert!(pads.recreate_due(4, now));
    assert!(!pads.recreate_due(4, now + RECREATE_RETRY / 2));
    assert!(
        pads.recreate_due(4, now + RECREATE_RETRY),
        "asked again after the second"
    );
    pads.active.clear();
}

/// A game's rumble poll can find the pad gone before the player touches
/// anything; the slot is created again before the next controller state.
#[test]
fn a_feedback_poll_that_finds_the_device_gone_creates_the_pad_again() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(device_removed_error())]));
    assert!(pads.feedback().is_empty());
    assert_eq!(script(&mut pads).calls, ["feedback 2", "plug 2"]);
    assert_eq!(pads.active.get(&0), Some(&2));
    // The poll is throttled like the input path while the driver stays gone.
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(device_removed_error())]));
    assert!(pads.feedback().is_empty());
    assert_eq!(
        script(&mut pads)
            .calls
            .iter()
            .filter(|call| call.starts_with("plug"))
            .count(),
        1,
        "no plug within the second"
    );
    pads.active.clear();
}

/// A pad the client removed whose unplug the driver refused is never created
/// again by a feedback poll; a sleep that takes the device lets the deferred
/// unplug finish instead.
#[test]
fn a_feedback_poll_never_brings_back_a_pad_the_client_removed() {
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut pads = scripted_pads();
    pads.active.insert(0, global);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .unplugs
        .push_back(Err(anyhow::anyhow!("the driver is busy")));
    // The client's mask no longer has the controller: its removal is deferred.
    pads.apply(&state(0, 0)).unwrap();
    assert_eq!(pads.active.get(&0), Some(&global));
    assert!(pads.dropped.contains(&0));
    // The poll finds the device gone and the unplug now works: it is never
    // plugged again.
    script(&mut pads).feedback.insert(
        u32::from(global),
        VecDeque::from([Err(device_removed_error())]),
    );
    assert!(pads.feedback().is_empty());
    assert!(!pads.active.contains_key(&0), "the pad is gone");
    assert!(!pads.dropped.contains(&0));
    assert!(
        !SLOTS.lock().unwrap()[global as usize],
        "its slot is free again"
    );
    assert_eq!(
        script(&mut pads).calls,
        [
            format!("unplug {global}"),
            format!("feedback {global}"),
            format!("unplug {global}")
        ]
    );
}

/// A client that sends the controller active again says the removal is over:
/// the pad must not be forgotten as removed by the next feedback poll.
#[test]
fn a_state_that_turns_a_pad_back_on_clears_its_deferred_removal() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .unplugs
        .push_back(Err(anyhow::anyhow!("the driver is busy")));
    // The client's mask no longer has the controller: its removal is deferred.
    pads.apply(&state(0, 0)).unwrap();
    assert!(pads.dropped.contains(&0));
    // The next state has it back before the poll.
    pads.apply(&state(0, 1)).unwrap();
    assert!(
        !pads.dropped.contains(&0),
        "a pad the client turns on again is not being removed"
    );
    script(&mut pads).calls.clear();
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(device_removed_error())]));
    assert!(pads.feedback().is_empty());
    assert_eq!(
        script(&mut pads).calls,
        ["feedback 2", "plug 2"],
        "the poll re-creates the active pad instead of forgetting it"
    );
    pads.active.clear();
}

/// A back-button timeout due on two pads: one pad's failed submit must not
/// skip the other's update, and the last error is reported.
#[test]
fn refresh_updates_every_pad_and_returns_the_last_failure() {
    let mut pads = scripted_pads();
    pads.policy.back_button_timeout = Some(Duration::ZERO);
    for id in [0u16, 1] {
        pads.active.insert(id, id + 2);
        pads.profiles.insert(id, 4);
        pads.apply(&Event::Controller {
            id,
            active: 0b11,
            buttons: 0x20,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        })
        .unwrap();
    }
    // The timeout has passed: both pads' back buttons are due.
    std::thread::sleep(Duration::from_millis(150));
    script(&mut pads).calls.clear();
    script(&mut pads).submits = VecDeque::from([
        Err(anyhow::anyhow!("first pad failed")),
        Err(anyhow::anyhow!("second pad failed")),
    ]);
    let error = pads.refresh().unwrap_err();
    assert!(format!("{error:#}").contains("second pad failed"));
    assert_eq!(
        script(&mut pads).calls,
        ["submit 2", "submit 3"],
        "the second pad was updated after the first failed"
    );
    pads.active.clear();
}

/// The back-button update takes the same device-removed path as a controller
/// state: the slot is created again and the update retried.
#[test]
fn refresh_recreates_a_pad_whose_driver_device_was_removed() {
    let mut pads = scripted_pads();
    pads.policy.back_button_timeout = Some(Duration::ZERO);
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    pads.apply(&Event::Controller {
        id: 0,
        active: 1,
        buttons: 0x20,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    script(&mut pads).calls.clear();
    script(&mut pads).submits = VecDeque::from([Err(device_removed_error()), Ok(())]);
    pads.refresh().unwrap();
    assert_eq!(
        script(&mut pads).calls,
        ["submit 2", "plug 2", "submit 2"],
        "the slot was created again and the update retried"
    );
    pads.active.clear();
}

/// A driver whose answers a test scripts, for the paths that only a real
/// sleep can otherwise reach.
fn scripted_pads() -> Gamepads {
    let mut pads = broken_pads();
    pads.profile = butterpollo_core::input_policy::VHF_XBOX_SERIES;
    pads.available = 1 << (butterpollo_core::input_policy::VHF_XBOX_SERIES - 1);
    pads.backend = gamepad_backend::Backend::Fake(Default::default());
    pads
}

fn script(pads: &mut Gamepads) -> &mut gamepad_backend::fake::Driver {
    match &mut pads.backend {
        gamepad_backend::Backend::Fake(driver) => driver,
        _ => unreachable!("scripted pads"),
    }
}

fn state(id: u16, active: u16) -> Event {
    Event::Controller {
        id,
        active,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

fn device_removed_error() -> anyhow::Error {
    anyhow::Error::new(windows::core::Error::from_hresult(
        windows::Win32::Foundation::ERROR_DEVICE_REMOVED.to_hresult(),
    ))
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
fn only_a_plugged_vhf_driver_pad_raises_the_driver_host_and_only_when_asked() {
    let config =
        butterpollo_core::config::Config::parse("gamepad_driver_host_priority=true\n").unwrap();
    let policy = butterpollo_core::input_policy::Policy::resolve(&config).unwrap();
    assert!(policy.driver_host_priority);
    assert!(
        !butterpollo_core::input_policy::Policy::resolve(&Default::default())
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
    pads.policy = butterpollo_core::input_policy::Policy::resolve(&Default::default()).unwrap();
    pads.active.insert(0, 15);
    pads.follow_driver_host();
    assert_eq!(HELD.load(Ordering::SeqCst), 0);
    pads.active.clear();
}
/// Held by the tests that check a slot stays stranded, and by those whose driver unplugs any
/// slot: their `ensure` reclaims every stranded slot, another test's too.
static STRANDED_SLOTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn stranded_slots() -> std::sync::MutexGuard<'static, ()> {
    STRANDED_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    use butterpollo_core::input_policy::{
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
    use butterpollo_core::input_policy::{VIGEM_DS4, VIGEM_X360};
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
    use butterpollo_core::input_policy::{VHF_DS4, VIGEM_DS4};
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
    use butterpollo_core::input_policy::{CTYPE_STEAM, DeckPad, VHF_STEAM_CONTROLLER};
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
/// Automatic pads with Steam's driver profile, and the arrival of a controller carrying
/// on-screen Steam Controller controls (or not).
fn steam_controls_pads(steam_app: bool, running: fn() -> bool) -> Gamepads {
    let mut pads = scripted_pads();
    pads.profile = 0;
    pads.available |= 1 << (butterpollo_core::input_policy::VHF_STEAM_CONTROLLER - 1);
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
    use butterpollo_core::input_policy::{VHF_STEAM_CONTROLLER, VHF_XBOX_SERIES};
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
    use butterpollo_core::input_policy::{VHF_STEAM_CONTROLLER, VHF_XBOX_SERIES};
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
/// The Desktop or an app that starts a program itself: Steam would give such a program its
/// desktop layout, so the controls keep an Xbox-type pad, though Steam runs, and nobody is
/// warned or asks whether Steam runs.
#[test]
fn steam_controls_outside_a_steam_app_keep_an_xbox_pad() {
    use butterpollo_core::input_policy::VHF_XBOX_SERIES;
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
    let xbox = butterpollo_core::input_policy::VHF_XBOX_SERIES;
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
    use butterpollo_core::input_policy::{
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
    let mut pads = broken_pads();
    pads.available = !0;
    let error = format!("{:#}", pads.ensure(0).unwrap_err());
    assert!(error.contains("could not plug"), "{error}");
    assert!(!error.contains("no free"), "{error}");
    assert!(pads.active.is_empty());
}
