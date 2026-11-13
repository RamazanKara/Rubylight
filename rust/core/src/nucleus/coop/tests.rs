use super::*;
use serde_json::json;

fn coop(pads: &[u8]) -> Session {
    Session::new(1, Kind::Coop, "Racer", "Racer", pads, ("owner", "Phone")).unwrap()
}

#[test]
fn a_session_needs_one_pad_per_player() {
    assert!(Session::new(1, Kind::Coop, "g", "G", &[], ("a", "A")).is_err());
    assert!(Session::new(1, Kind::Coop, "g", "G", &[0, 1, 2, 3, 0], ("a", "A")).is_err());
    assert!(Session::new(1, Kind::Coop, "g", "G", &[1, 1], ("a", "A")).is_err());
    assert!(Session::new(1, Kind::Coop, "g", "G", &[4], ("a", "A")).is_err());
    assert!(Session::new(1, Kind::Background, "g", "G", &[0, 1], ("a", "A")).is_err());
    let s = coop(&[1, 2, 3]);
    assert_eq!(s.slot_of("owner"), Some(0));
    assert_eq!(s.phase, Phase::Starting);
    assert_eq!(s.route("owner"), Some(1));
}

#[test]
fn devices_join_free_slots_and_each_reaches_only_its_own_pad() {
    let mut s = coop(&[1, 2, 3]);
    assert_eq!(s.join("laptop", "Laptop", true), Ok(Joined::Seated(1)));
    assert_eq!(s.join("tv", "TV", true), Ok(Joined::Seated(2)));
    assert_eq!(
        s.join("fourth", "Fourth", true),
        Err(Refused::Full {
            title: "Racer".into()
        })
    );
    assert_eq!(s.route("owner"), Some(1));
    assert_eq!(s.route("laptop"), Some(2));
    assert_eq!(s.route("tv"), Some(3));
    assert_eq!(s.route("stranger"), None);
}

#[test]
fn joining_needs_the_permission_and_only_the_owner_returns_without_it() {
    let mut s = coop(&[0, 1]);
    assert_eq!(
        s.join("laptop", "Laptop", false),
        Err(Refused::NotPermitted)
    );
    assert_eq!(s.join("owner", "Phone", false), Ok(Joined::Returned(0)));
    // A joined player whose permission was taken away cannot come back,
    // and an ended session's title stays hidden from a device without it.
    assert_eq!(s.join("laptop", "Laptop", true), Ok(Joined::Seated(1)));
    s.leave("laptop", Instant::now());
    assert_eq!(
        s.join("laptop", "Laptop", false),
        Err(Refused::NotPermitted)
    );
    assert_eq!(s.join("laptop", "Laptop", true), Ok(Joined::Returned(1)));
    s.end(End::Ended);
    assert_eq!(s.join("tv", "TV", false), Err(Refused::NotPermitted));
    let extra = |value| serde_json::from_value(json!({ "coop_join": value })).unwrap();
    assert!(may_join(&extra(json!(true)), true));
    assert!(may_join(&extra(json!("true")), true));
    // false keeps out a device that launches apps, and so does any other value.
    assert!(!may_join(&extra(json!(false)), true));
    assert!(!may_join(&extra(json!(1)), true));
    // Never set, or cleared with null: joining follows Launch apps.
    assert!(may_join(&BTreeMap::new(), true));
    assert!(may_join(&extra(json!(null)), true));
    // Joining launches the co-op entry: without Launch apps nothing lets it in.
    for value in [json!(true), json!("true"), json!(null), json!(false)] {
        assert!(!may_join(&extra(value), false));
    }
    assert!(!may_join(&BTreeMap::new(), false));
}

#[test]
fn a_background_stream_is_private() {
    let mut s = Session::new(
        1,
        Kind::Background,
        "Racer",
        "Racer",
        &[0],
        ("owner", "Phone"),
    )
    .unwrap();
    assert!(matches!(
        s.join("laptop", "Laptop", true),
        Err(Refused::Private { .. })
    ));
    assert_eq!(s.join("owner", "Phone", true), Ok(Joined::Returned(0)));
}

#[test]
fn a_dropped_device_keeps_its_slot_for_the_grace_period() {
    let start = Instant::now();
    let mut s = coop(&[0, 1]);
    s.join("laptop", "Laptop", true).unwrap();
    assert_eq!(s.rejoin_left(1, start), None, "streaming");
    assert_eq!(s.rejoin_left(3, start), None, "no such slot");
    s.leave("laptop", start);
    // Away: no input reaches its pad, and its slot is not free.
    assert_eq!(s.route("laptop"), None);
    assert_eq!(
        s.rejoin_left(1, start + Duration::from_secs(15)),
        Some(GRACE - Duration::from_secs(15))
    );
    assert_eq!(
        s.rejoin_left(1, start + GRACE + GRACE),
        Some(Duration::ZERO)
    );
    assert!(matches!(
        s.join("tv", "TV", true),
        Err(Refused::Full { .. })
    ));
    assert!(s.tick(start + GRACE - Duration::from_secs(1)).is_empty());
    assert_eq!(s.join("laptop", "Laptop", true), Ok(Joined::Returned(1)));
    assert_eq!(s.route("laptop"), Some(1));
    s.leave("laptop", start + Duration::from_secs(10));
    assert_eq!(
        s.tick(start + Duration::from_secs(10) + GRACE),
        vec![Event::Freed {
            slot: 1,
            device: "laptop".into()
        }]
    );
    assert_eq!(s.join("tv", "TV", true), Ok(Joined::Seated(1)));
}

#[test]
fn a_session_everyone_left_ends_after_the_grace_period() {
    let start = Instant::now();
    let mut s = coop(&[0, 1]);
    s.join("laptop", "Laptop", true).unwrap();
    s.leave("owner", start);
    assert!(
        s.tick(start + GRACE).len() == 1,
        "the owner's slot is freed"
    );
    assert!(!s.ended(), "the laptop still plays");
    s.leave("laptop", start + GRACE);
    let later = start + GRACE + GRACE;
    let events = s.tick(later);
    assert!(events.contains(&Event::Ended(End::Empty)), "{events:?}");
    assert_eq!(s.phase, Phase::Ended(End::Empty));
    assert!(s.tick(later + GRACE).is_empty());
    assert_eq!(s.route("laptop"), None);
    assert!(matches!(
        s.join("owner", "Phone", true),
        Err(Refused::Ended { .. })
    ));
}

#[test]
fn a_kicked_device_cannot_rejoin_and_the_first_end_reason_stays() {
    let mut s = coop(&[0, 1]);
    s.join("laptop", "Laptop", true).unwrap();
    assert_eq!(s.kick(1), Some("laptop".into()));
    assert_eq!(s.kick(1), None);
    assert_eq!(s.kicked().collect::<Vec<_>>(), ["laptop"]);
    assert_eq!(s.route("laptop"), None);
    assert!(matches!(
        s.join("laptop", "Laptop", true),
        Err(Refused::Kicked { .. })
    ));
    assert_eq!(s.join("tv", "TV", true), Ok(Joined::Seated(1)));
    s.running(&[Some(10), Some(11)]);
    assert_eq!(s.phase, Phase::Running);
    assert_eq!(s.slots[1].window, Some(11));
    assert!(s.end(End::Crashed("Racer.exe exited".into())));
    assert!(!s.end(End::Ended));
    assert_eq!(
        s.phase,
        Phase::Ended(End::Crashed("Racer.exe exited".into()))
    );
    // A late start report does not revive it.
    s.running(&[Some(12)]);
    assert!(s.ended());
    assert_eq!(
        End::Crashed("Racer.exe exited".into()).to_string(),
        "the game stopped: Racer.exe exited"
    );
}

#[test]
fn nucleus_runs_one_game_and_only_co_op_of_the_same_game_joins() {
    let s = coop(&[0, 1]);
    assert_eq!(admit(None, "Racer", Kind::Coop, "tv"), Admit::Start);
    assert_eq!(admit(Some(&s), "Racer", Kind::Coop, "tv"), Admit::Join(1));
    let busy = admit(Some(&s), "Shooter", Kind::Coop, "tv");
    assert!(
        matches!(&busy, Admit::Busy(text) if text.starts_with("Nucleus Co-op is running Racer for a co-op session")),
        "{busy:?}"
    );
    assert!(matches!(
        admit(Some(&s), "Racer", Kind::Background, "owner"),
        Admit::Busy(_)
    ));
    let mut ended = coop(&[0]);
    ended.end(End::Ended);
    assert_eq!(
        admit(Some(&ended), "Shooter", Kind::Background, "owner"),
        Admit::Start
    );
}

#[test]
fn only_the_owner_returns_to_a_background_stream() {
    let s = Session::new(
        1,
        Kind::Background,
        "Racer",
        "Racer",
        &[0],
        ("owner", "Phone"),
    )
    .unwrap();
    assert_eq!(
        admit(Some(&s), "Racer", Kind::Background, "owner"),
        Admit::Join(1)
    );
    for (game, kind, device) in [
        ("Racer", Kind::Background, "tv"),
        ("Racer", Kind::Coop, "owner"),
        ("Shooter", Kind::Background, "owner"),
    ] {
        let busy = admit(Some(&s), game, kind, device);
        assert!(
            matches!(&busy, Admit::Busy(text) if text.contains("for a background stream")),
            "{game} {kind:?} {device}: {busy:?}"
        );
    }
}

#[test]
fn players_follow_the_handler_the_setting_and_free_pads() {
    assert_eq!(players(4, 4, 4), Ok(4));
    assert_eq!(players(8, 4, 4), Ok(4));
    assert_eq!(players(4, 2, 4), Ok(2));
    assert_eq!(players(4, 4, 3), Ok(3));
    assert_eq!(players(4, 0, 4), Ok(1));
    assert_eq!(players(4, 9, 4), Ok(4));
    assert!(
        players(4, 4, 0)
            .unwrap_err()
            .contains("XInput controller slots")
    );
}

#[test]
fn a_new_pad_is_the_one_index_that_appeared() {
    assert_eq!(
        new_pad([true, false, false, false], [true, true, false, false]),
        Some(1)
    );
    assert_eq!(new_pad([false; 4], [false; 4]), None);
    // A PC controller plugged in at the same moment: unknown, not a guess.
    assert_eq!(new_pad([false; 4], [true, true, false, false]), None);
}
