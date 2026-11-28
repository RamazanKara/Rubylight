//! Regression tests for guest expiry during admission and approval (`STR-09`).

use super::*;

const BASE: u64 = 1_800_000_000;

fn playing() -> Guests {
    let mut guests = Guests::default();
    assert!(guests.follow_game(Some(("game-1", "owner"))).is_empty());
    guests
}

#[test]
fn expired_guests_do_not_block_capacity_or_duplicate_on_admit_and_approve() {
    let now = Instant::now();
    let mut guests = playing();
    let lifetime = Some(MIN_LIFETIME);
    for i in 0..MAX_GUESTS {
        let dev = format!("dev-{i}");
        let Admission::Waiting { id } = guests.admit(
            &dev,
            &dev,
            JoinPolicy::Ask,
            Permission::Controller,
            BASE,
            now,
        ) else {
            panic!("expected waiting");
        };
        assert!(guests.approve(id, Permission::Controller, lifetime, BASE));
        let _ = guests.outcome(id, now);
        assert!(guests.joined(id, &format!("launch-{i}")));
    }

    // At capacity while live.
    let end = BASE + MIN_LIFETIME.as_secs();
    assert_eq!(
        guests.admit(
            "new",
            "new",
            JoinPolicy::Allow,
            Permission::View,
            end - 1,
            now
        ),
        Admission::Refused(Refusal::Full)
    );

    // Once expired, admitting a new device succeeds without calling `expire` first,
    // and the expired launches are still returned by `expire`.
    assert!(matches!(
        guests.admit("new", "new", JoinPolicy::Allow, Permission::View, end, now),
        Admission::Granted { .. }
    ));
    assert_eq!(guests.guests().len(), 1);
    let expired = guests.expire(end);
    assert_eq!(expired.len(), MAX_GUESTS);

    // Re-admitting an expired device with JoinPolicy::Allow creates no duplicate entry.
    let mut guests = playing();
    let Admission::Waiting { id } =
        guests.admit("dup", "dup", JoinPolicy::Ask, Permission::View, BASE, now)
    else {
        panic!("expected waiting");
    };
    assert!(guests.approve(id, Permission::View, lifetime, BASE));
    let _ = guests.outcome(id, now);
    assert!(guests.joined(id, "launch-old"));
    assert!(matches!(
        guests.admit("dup", "dup", JoinPolicy::Allow, Permission::View, end, now),
        Admission::Granted { .. }
    ));
    assert_eq!(
        guests.guests().iter().filter(|g| g.device == "dup").count(),
        1
    );
    assert_eq!(guests.expire(end), vec!["launch-old".to_owned()]);

    // Approving a waiting request when MAX_GUESTS existing entries have expired succeeds.
    let mut guests = playing();
    let Admission::Waiting { id: waiting_id } = guests.admit(
        "waiter",
        "waiter",
        JoinPolicy::Ask,
        Permission::View,
        BASE,
        now,
    ) else {
        panic!("expected waiting");
    };
    for i in 0..MAX_GUESTS {
        let dev = format!("full-{i}");
        let Admission::Waiting { id } =
            guests.admit(&dev, &dev, JoinPolicy::Ask, Permission::View, BASE, now)
        else {
            panic!("expected waiting");
        };
        assert!(guests.approve(id, Permission::View, lifetime, BASE));
        assert!(guests.joined(id, &format!("full-launch-{i}")));
    }
    assert!(!guests.approve(waiting_id, Permission::View, None, end - 1));
    assert!(guests.approve(waiting_id, Permission::View, None, end));
    assert_eq!(guests.guests().len(), 1);
    assert_eq!(guests.expire(end).len(), MAX_GUESTS);
}
