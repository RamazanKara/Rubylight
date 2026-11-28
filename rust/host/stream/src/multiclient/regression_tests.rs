//! Regression tests for `DeviceGuests` lock ordering (`STR-01`), owner KBM release fast-path
//! restoration & cleanup (`STR-04`), and eager guest expiration (`STR-09`).

use super::*;
use butterpollo_session::session::Role;

#[test]
fn sweep_unplugs_guest_pads_outside_state_lock() {
    let f = crate::state::test_support::Fixture::new();
    let guests = &f.host.device_guests;
    {
        let mut state = locked(&guests.state);
        let _ = state.follow_game(Some(("gen-1", "owner")));
        let Admission::Granted { id, .. } = state.admit(
            "guest-dev",
            "Guest",
            JoinPolicy::Allow,
            Permission::View,
            unix_now(),
            Instant::now(),
        ) else {
            panic!("expected immediate grant");
        };
        assert!(state.joined(id, "ended-guest-launch"));
        locked(&guests.pads).insert(id, "guest-pad-key".into());
    }

    // Stream is no longer in `h.sessions`, so `sweep` marks it left and unplugs its pad key.
    sweep(&f.host);
    assert!(locked(&guests.pads).is_empty());

    // Structural regression guard: `controller_sessions.unplug` must happen after the
    // `guests.state` lock scope ends in `sweep`.
    let source = include_str!("../multiclient.rs").replace("\r\n", "\n");
    let sweep_fn = &source[source.find("pub(crate) fn sweep(").unwrap()..];
    let sweep_end = sweep_fn.find("\n/// Watches the PC's own").unwrap();
    let body = &sweep_fn[..sweep_end];
    let state_lock = body.find("locked(&guests.state)").unwrap();
    let reconcile = body.find("guests.reconcile_releases(").unwrap();
    let unplug = body.find("h.controller_sessions.unplug(").unwrap();
    assert!(
        state_lock < reconcile && reconcile < unplug,
        "sweep must drop guests.state before calling controller_sessions.unplug"
    );
}

#[test]
fn owner_release_restores_atomic_fast_path_prunes_seen_and_preserves_concurrent_handoff() {
    let f = crate::state::test_support::Fixture::new();
    let guests = &f.host.device_guests;
    let owner1 = f.session(f.client(u32::MAX), Role::Stream);

    // Owner loses KBM to a guest: releasing becomes true, owner1 receives one release.
    guests.release_later(Source::Owner);
    assert!(guests.releasing.load(Ordering::Acquire));
    assert!(guests.take_release(&owner1));
    assert!(!guests.take_release(&owner1));

    // Sweep reconciles active owners, clears pending + seen, and restores the atomic fast path.
    sweep(&f.host);
    assert!(!guests.releasing.load(Ordering::Acquire));
    assert!(locked(&guests.owner_release).seen.is_empty());

    // A later solo owner session hits the lock-free fast path and never gets a stale release.
    let owner2 = f.session(f.client(u32::MAX), Role::Stream);
    assert!(!guests.take_release(&owner2));

    // Concurrent KBM handoff race: if a new `release_later(Source::Owner)` arrives after
    // `sweep` snapshotted `owner_gen_before` while guests are still streaming, `reconcile_releases`
    // must NOT clear `owner.pending` or `releasing`.
    guests.release_later(Source::Owner);
    let gen_before = locked(&guests.owner_release).generation;
    assert!(guests.take_release(&owner1));
    assert!(guests.take_release(&owner2));
    // Simulate a second concurrent KBM handoff bumping generation past `gen_before`.
    guests.release_later(Source::Owner);
    let live_guests: BTreeSet<u32> = [1].into_iter().collect();
    let live_owners: BTreeSet<String> = [owner1.launch.id.clone(), owner2.launch.id.clone()]
        .into_iter()
        .collect();
    guests.reconcile_releases(&live_guests, &live_owners, true, gen_before);
    assert!(guests.releasing.load(Ordering::Acquire));
    assert!(locked(&guests.owner_release).pending);
    assert!(guests.take_release(&owner1));
    assert!(guests.take_release(&owner2));

    // Once reconciled at the latest generation with all live owners accounted for, fast path returns.
    let gen_latest = locked(&guests.owner_release).generation;
    guests.reconcile_releases(&live_guests, &live_owners, true, gen_latest);
    assert!(!guests.releasing.load(Ordering::Acquire));
    assert!(locked(&guests.owner_release).seen.is_empty());
}

#[test]
fn expired_guests_are_evicted_before_approve_and_stopped_by_guest_live() {
    let f = crate::state::test_support::Fixture::new();
    let guests = &f.host.device_guests;
    let now = Instant::now();
    let old_unix = 1_000_000;
    let lifetime = Some(policy::MIN_LIFETIME);

    let guest_session = f.session(f.client(u32::MAX), Role::Stream);
    let expired_id = {
        let mut state = locked(&guests.state);
        let _ = state.follow_game(Some(("gen-1", "owner")));
        let Admission::Waiting { id } = state.admit(
            "guest-0",
            "Guest 0",
            JoinPolicy::Ask,
            Permission::Controller,
            old_unix,
            now,
        ) else {
            panic!("expected waiting");
        };
        assert!(state.approve(id, Permission::Controller, lifetime, old_unix));
        let _ = state.outcome(id, now);
        assert!(state.joined(id, &guest_session.launch.id));
        id
    };

    // `guest_live` expires the timed-out guest and stops its active stream outside the lock.
    assert!(!guests.guest_live(&f.host, expired_id));
    assert!(guest_session.stopping());

    // Fill MAX_GUESTS with already-expired entries and verify `DeviceGuests::approve` succeeds
    // for a waiting request without needing `sweep` to run first.
    let waiter_id = {
        let mut state = locked(&guests.state);
        let _ = state.follow_game(Some(("gen-2", "owner")));
        let Admission::Waiting { id: waiter } = state.admit(
            "waiter",
            "Waiter",
            JoinPolicy::Ask,
            Permission::View,
            unix_now(),
            now,
        ) else {
            panic!("expected waiting");
        };
        for i in 0..policy::MAX_GUESTS {
            let dev = format!("expired-{i}");
            let Admission::Waiting { id } =
                state.admit(&dev, &dev, JoinPolicy::Ask, Permission::View, old_unix, now)
            else {
                panic!("expected waiting");
            };
            assert!(state.approve(id, Permission::View, lifetime, old_unix));
            assert!(state.joined(id, &format!("expired-launch-{i}")));
        }
        waiter
    };

    assert_eq!(guests.approve(waiter_id, Permission::View, None), Ok(()));
    assert_eq!(locked(&guests.state).guests().len(), 1);
}
