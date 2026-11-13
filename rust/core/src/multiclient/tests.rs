use super::*;

const NOW: u64 = 1_800_000_000;

fn playing() -> Guests {
    let mut guests = Guests::default();
    assert!(guests.follow_game(Some(("game-1", "owner"))).is_empty());
    guests
}

fn ask(guests: &mut Guests, device: &str, at: Instant) -> u32 {
    match guests.admit(
        device,
        device,
        JoinPolicy::Ask,
        Permission::Controller,
        NOW,
        at,
    ) {
        Admission::Waiting { id } => id,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_policy_reads_its_setting_and_asks_when_it_cannot() {
    for (text, policy) in [
        ("off", JoinPolicy::Off),
        ("ask", JoinPolicy::Ask),
        ("allow", JoinPolicy::Allow),
    ] {
        assert_eq!(JoinPolicy::parse(text), Some(policy));
        assert_eq!(policy.as_str(), text);
        let config = Config::parse(&format!("{JOIN_SETTING} = {text}\n")).unwrap();
        assert_eq!(JoinPolicy::from_config(&config), policy);
    }
    assert_eq!(JoinPolicy::from_config(&Config::default()), JoinPolicy::Ask);
    let odd = Config::parse(&format!("{JOIN_SETTING} = maybe\n")).unwrap();
    assert_eq!(JoinPolicy::from_config(&odd), JoinPolicy::Ask);
    assert_eq!(
        default_permission(&Config::default()),
        Permission::Controller
    );
    let view = Config::parse(&format!("{PERMISSION_SETTING} = view\n")).unwrap();
    assert_eq!(default_permission(&view), Permission::View);
}

#[test]
fn a_join_waits_for_the_owner_and_streams_once_let_in() {
    let start = Instant::now();
    let mut guests = playing();
    let id = ask(&mut guests, "phone", start);
    assert_eq!(guests.waiting().count(), 1);
    assert_eq!(guests.outcome(id, start), Admission::Waiting { id });
    // Asking again while waiting is the same request.
    assert_eq!(ask(&mut guests, "phone", start), id);
    assert!(guests.approve(id, Permission::ControllerKeyboardMouse, None, NOW));
    assert!(
        !guests.approve(id, Permission::View, None, NOW),
        "answered once"
    );
    assert_eq!(
        guests.outcome(id, start),
        Admission::Granted {
            id,
            permission: Permission::ControllerKeyboardMouse
        }
    );
    assert_eq!(guests.waiting().count(), 0);
    assert!(!guests.streaming());
    assert!(guests.joined(id, "launch-a"));
    assert!(guests.streaming());
    assert_eq!(guests.of_launch("launch-a").map(|g| g.id), Some(id));
}

#[test]
fn a_denied_or_unanswered_join_is_refused() {
    let start = Instant::now();
    let mut guests = playing();
    let denied = ask(&mut guests, "phone", start);
    assert!(guests.deny(denied));
    assert_eq!(
        guests.outcome(denied, start),
        Admission::Refused(Refusal::Denied)
    );
    assert!(guests.guests().is_empty());
    let unanswered = ask(&mut guests, "tablet", start);
    assert_eq!(
        guests.outcome(unanswered, start + APPROVAL_WAIT - Duration::from_millis(1)),
        Admission::Waiting { id: unanswered }
    );
    assert_eq!(
        guests.outcome(unanswered, start + APPROVAL_WAIT),
        Admission::Refused(Refusal::Unanswered)
    );
    // Too late to approve.
    assert!(!guests.approve(unanswered, Permission::View, None, NOW));
}

#[test]
fn allow_lets_in_at_once_with_the_default_permission() {
    let mut guests = playing();
    let Admission::Granted { id, permission } = guests.admit(
        "phone",
        "Phone",
        JoinPolicy::Allow,
        Permission::View,
        NOW,
        Instant::now(),
    ) else {
        panic!("not granted");
    };
    assert_eq!(permission, Permission::View);
    assert_eq!(guests.get(id).unwrap().name, "Phone");
    assert_eq!(guests.waiting().count(), 0);
}

#[test]
fn a_guest_with_a_place_comes_back_without_asking() {
    let start = Instant::now();
    let mut guests = playing();
    let id = ask(&mut guests, "phone", start);
    assert!(guests.approve(id, Permission::Controller, None, NOW));
    let _ = guests.outcome(id, start);
    assert!(guests.joined(id, "launch-a"));
    guests.left("launch-a");
    assert!(!guests.streaming());
    assert_eq!(
        guests.admit(
            "phone",
            "phone",
            JoinPolicy::Ask,
            Permission::View,
            NOW,
            start
        ),
        Admission::Granted {
            id,
            permission: Permission::Controller
        }
    );
}

#[test]
fn kick_ends_the_place_and_revoke_refuses_the_device_for_the_game() {
    let start = Instant::now();
    let mut guests = playing();
    let id = ask(&mut guests, "phone", start);
    assert!(guests.approve(id, Permission::Controller, None, NOW));
    assert!(guests.joined(id, "launch-a"));
    assert_eq!(guests.kick(id), Ok(Some("launch-a".into())));
    assert!(guests.get(id).is_none());
    assert_eq!(guests.kick(id), Err(NoSuchGuest));
    // A kicked device asks again.
    let again = ask(&mut guests, "phone", start);
    assert!(guests.approve(again, Permission::Controller, None, NOW));
    assert!(guests.joined(again, "launch-b"));
    assert_eq!(guests.revoke(again), Ok(Some("launch-b".into())));
    assert_eq!(guests.revoke(again), Err(NoSuchGuest));
    // A revoked device is refused at once for the rest of the game.
    assert_eq!(
        guests.admit(
            "phone",
            "phone",
            JoinPolicy::Allow,
            Permission::View,
            NOW,
            start
        ),
        Admission::Refused(Refusal::Revoked)
    );
    assert_eq!(guests.revoked(), ["phone".to_owned()]);
    assert!(guests.forgive("phone"));
    assert!(!guests.forgive("phone"));
    assert!(matches!(
        guests.admit(
            "phone",
            "phone",
            JoinPolicy::Ask,
            Permission::View,
            NOW,
            start
        ),
        Admission::Waiting { .. }
    ));
    // Revoking a waiting request turns it down and refuses its device.
    let waiting = ask(&mut guests, "tablet", start);
    assert_eq!(guests.revoke(waiting), Ok(None));
    assert_eq!(
        guests.outcome(waiting, start),
        Admission::Refused(Refusal::Denied)
    );
    assert_eq!(
        guests.admit(
            "tablet",
            "tablet",
            JoinPolicy::Ask,
            Permission::View,
            NOW,
            start
        ),
        Admission::Refused(Refusal::Revoked)
    );
    // A new game forgets who was refused.
    let _ = guests.follow_game(Some(("game-2", "owner")));
    assert!(guests.revoked().is_empty());
}

#[test]
fn a_guests_time_runs_out() {
    let start = Instant::now();
    let mut guests = playing();
    let id = ask(&mut guests, "phone", start);
    assert!(
        !guests.approve(
            id,
            Permission::View,
            Some(MIN_LIFETIME - Duration::from_secs(1)),
            NOW
        ),
        "too short"
    );
    assert!(!guests.approve(id, Permission::View, Some(MAX_LIFETIME * 2), NOW));
    assert!(guests.approve(id, Permission::View, Some(MIN_LIFETIME), NOW));
    assert!(guests.joined(id, "launch-a"));
    let end = NOW + MIN_LIFETIME.as_secs();
    assert!(guests.expire(end - 1).is_empty());
    // Expired, it is no guest when it comes back either.
    assert!(matches!(
        guests.admit(
            "phone",
            "phone",
            JoinPolicy::Ask,
            Permission::View,
            end,
            start
        ),
        Admission::Waiting { .. }
    ));
    assert_eq!(guests.expire(end), vec!["launch-a".to_owned()]);
    assert!(guests.get(id).is_none());
}

#[test]
fn a_new_game_ends_every_guest_and_a_new_owner_keeps_them() {
    let start = Instant::now();
    let mut guests = playing();
    let phone = ask(&mut guests, "phone", start);
    assert!(guests.approve(phone, Permission::Controller, None, NOW));
    assert!(guests.joined(phone, "launch-a"));
    let tablet = ask(&mut guests, "tablet", start);
    assert!(guests.approve(tablet, Permission::Controller, None, NOW));
    let _ = ask(&mut guests, "tv", start);
    // Continue here from the tablet: it owns the game now, the phone stays a guest.
    assert!(guests.follow_game(Some(("game-1", "tablet"))).is_empty());
    assert_eq!(guests.owner(), Some("tablet"));
    assert!(guests.get(tablet).is_none());
    assert!(guests.get(phone).is_some());
    assert_eq!(guests.waiting().count(), 1);
    // Another game: everyone goes.
    assert_eq!(
        guests.follow_game(Some(("game-2", "tablet"))),
        vec!["launch-a".to_owned()]
    );
    assert!(guests.guests().is_empty());
    assert_eq!(guests.waiting().count(), 0);
    assert!(guests.follow_game(None).is_empty());
    assert_eq!(
        guests.admit(
            "phone",
            "phone",
            JoinPolicy::Allow,
            Permission::View,
            NOW,
            start
        ),
        Admission::Refused(Refusal::GameEnded)
    );
    // A request forgotten with its game is refused for that.
    let mut guests = playing();
    let id = ask(&mut guests, "phone", start);
    let _ = guests.follow_game(Some(("game-2", "owner")));
    assert_eq!(
        guests.outcome(id, start),
        Admission::Refused(Refusal::GameEnded)
    );
}

#[test]
fn guests_and_requests_are_bounded() {
    let start = Instant::now();
    let mut guests = playing();
    for i in 0..MAX_GUESTS {
        assert!(matches!(
            guests.admit(
                &format!("device-{i}"),
                "d",
                JoinPolicy::Allow,
                Permission::View,
                NOW,
                start
            ),
            Admission::Granted { .. }
        ));
    }
    assert_eq!(
        guests.admit(
            "one-more",
            "d",
            JoinPolicy::Allow,
            Permission::View,
            NOW,
            start
        ),
        Admission::Refused(Refusal::Full)
    );
    let mut guests = playing();
    for i in 0..MAX_REQUESTS {
        ask(&mut guests, &format!("device-{i}"), start);
    }
    assert_eq!(
        guests.admit(
            "one-more",
            "d",
            JoinPolicy::Ask,
            Permission::View,
            NOW,
            start
        ),
        Admission::Refused(Refusal::Full)
    );
    // Unanswered requests make room once they lapse.
    assert!(matches!(
        guests.admit(
            "one-more",
            "d",
            JoinPolicy::Ask,
            Permission::View,
            NOW,
            start + APPROVAL_WAIT
        ),
        Admission::Waiting { .. }
    ));
}

#[test]
fn changes_bump_the_version_and_permissions_change_in_place() {
    let mut guests = playing();
    let before = guests.version();
    let id = ask(&mut guests, "phone", Instant::now());
    assert_ne!(guests.version(), before);
    assert!(guests.approve(id, Permission::View, None, NOW));
    let approved = guests.version();
    assert!(guests.set_permission(id, Permission::View));
    assert_eq!(guests.version(), approved, "no change, no new version");
    assert!(guests.set_permission(id, Permission::ControllerKeyboardMouse));
    assert_ne!(guests.version(), approved);
    assert_eq!(
        guests.get(id).unwrap().permission,
        Permission::ControllerKeyboardMouse
    );
    assert!(!guests.set_permission(id + 1, Permission::View));
}

#[test]
fn guest_permissions_gate_input_kinds() {
    use crate::input::Input;
    let pad = Input::Controller {
        id: 0,
        active: 1,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    let key = Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true,
    };
    let mouse = Input::Relative { x: 1, y: 1 };
    let touch = Input::Touch {
        event: 0,
        id: 0,
        x: 0.0,
        y: 0.0,
        pressure: 0.0,
        major: 0.0,
        minor: 0.0,
        rotation: 0,
    };
    let allows = |p: Permission, e: &Input| guest_allows(p, e.required_permission());
    for event in [&pad, &key, &mouse, &touch] {
        assert!(!allows(Permission::View, event));
    }
    assert!(allows(Permission::Controller, &pad));
    assert!(!allows(Permission::Controller, &key));
    assert!(!allows(Permission::Controller, &mouse));
    assert!(allows(Permission::ControllerKeyboardMouse, &key));
    assert!(allows(Permission::ControllerKeyboardMouse, &mouse));
    // Touch and pen move the PC's pointer: never a guest's.
    assert!(!allows(Permission::ControllerKeyboardMouse, &touch));
    assert!(is_keyboard_mouse(key.required_permission()));
    assert!(is_keyboard_mouse(mouse.required_permission()));
    assert!(!is_keyboard_mouse(pad.required_permission()));
}

#[test]
fn one_holder_has_the_keyboard_and_mouse_and_the_owner_takes_them_back() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut kbm = Kbm::default();
    // Nobody used them yet: the first guest picks them up.
    assert_eq!(
        kbm.input(Source::Guest(1), at(0)),
        Verdict::Took {
            from: Source::Owner
        }
    );
    assert_eq!(kbm.input(Source::Guest(1), at(100)), Verdict::Accept);
    // Another guest waits until they were left alone.
    assert_eq!(kbm.input(Source::Guest(2), at(500)), Verdict::Drop);
    // The owner takes them back at once by moving the mouse.
    assert_eq!(
        kbm.input(Source::Owner, at(600)),
        Verdict::Took {
            from: Source::Guest(1)
        }
    );
    assert_eq!(kbm.input(Source::Guest(1), at(700)), Verdict::Drop);
    assert_eq!(kbm.input(Source::Owner, at(1000)), Verdict::Accept);
    // Left alone long enough, a guest picks them up again.
    let idle = at(1000) + HANDOFF_IDLE;
    assert_eq!(
        kbm.input(Source::Guest(2), idle - Duration::from_millis(1)),
        Verdict::Drop
    );
    assert_eq!(
        kbm.input(Source::Guest(2), idle),
        Verdict::Took {
            from: Source::Owner
        }
    );
    assert_eq!(kbm.holder(), Source::Guest(2));
}

#[test]
fn the_owner_hands_them_over_and_a_leaving_guest_gives_them_back() {
    let start = Instant::now();
    let mut kbm = Kbm::default();
    assert_eq!(kbm.input(Source::Owner, start), Verdict::Accept);
    assert_eq!(kbm.give(Source::Guest(3), start), Some(Source::Owner));
    assert_eq!(kbm.give(Source::Guest(3), start), None);
    assert_eq!(kbm.input(Source::Guest(3), start), Verdict::Accept);
    kbm.forget(4);
    assert_eq!(kbm.holder(), Source::Guest(3));
    kbm.forget(3);
    assert_eq!(kbm.holder(), Source::Owner);
}

#[test]
fn refusals_have_their_client_text_and_status() {
    for refusal in [
        Refusal::Full,
        Refusal::Denied,
        Refusal::Unanswered,
        Refusal::GameEnded,
        Refusal::Revoked,
    ] {
        assert!(!refusal.message().is_empty());
        assert!((400..600).contains(&refusal.status()));
    }
    let ended = [
        Ended::Kicked,
        Ended::Revoked,
        Ended::Expired,
        Ended::GameEnded,
    ]
    .map(Ended::message);
    for (i, a) in ended.iter().enumerate() {
        assert!(ended[i + 1..].iter().all(|b| a != b), "{a}");
    }
}
