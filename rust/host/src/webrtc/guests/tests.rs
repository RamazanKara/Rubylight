use super::*;
use crate::webrtc::sdp::testing::spec;
use butterpollo_core::webrtc::guests::MIN_LIFETIME;
use std::net::{Ipv4Addr, Ipv6Addr};

fn owner() -> Owner {
    Owner {
        spec: spec(VideoCodec::H264, false),
        audio_layout: None,
        followers: Arc::default(),
        input: InputHandle::probe().0,
        session: Weak::new(),
    }
}

fn v4(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 168, 4, last))
}

/// Guests with an owner streaming and one invite of `permission`; returns its id and token.
fn streaming(permission: Permission) -> (Guests, String, String) {
    let guests = Guests::default();
    guests.owner_started(owner());
    let (invite, token) = guests.create(permission, MIN_LIFETIME, None).unwrap();
    (guests, invite.id, token)
}

fn join(guests: &Guests, peer: IpAddr, token: &str, now: Instant) -> Result<Admission, JoinError> {
    guests.join_at(peer, token, "Sam", now, unix_now())
}

#[test]
fn the_right_token_joins_with_its_invites_permission_and_a_wrong_one_does_not() {
    let (guests, invite, token) = streaming(Permission::Controller);
    let now = Instant::now();
    let wrong = format!("{}x", &token[..token.len() - 1]);
    assert_eq!(
        join(&guests, v4(9), &wrong, now).err(),
        Some(JoinError::InvalidLink)
    );
    assert_eq!(
        join(&guests, v4(9), "", now).err(),
        Some(JoinError::InvalidLink)
    );

    let admitted = join(&guests, v4(9), &token, now).unwrap();
    assert_eq!(admitted.permission, Permission::Controller);
    assert_eq!(admitted.slot, 0);
    assert_eq!(admitted.name, "Sam");
    let status = guests.status();
    assert_eq!(status.guests.len(), 1);
    assert_eq!(status.guests[0].invite, invite);
    assert_eq!(status.guests[0].permission, Permission::Controller);
}

#[test]
fn an_expired_link_opens_nothing() {
    let (guests, _, token) = streaming(Permission::View);
    let later = unix_now() + MIN_LIFETIME.as_secs() + 1;
    assert_eq!(
        guests
            .join_at(v4(9), &token, "Sam", Instant::now(), later)
            .err(),
        Some(JoinError::InvalidLink)
    );
    assert!(guests.status().guests.is_empty());
}

#[test]
fn wrong_tokens_lock_out_their_address_even_for_the_right_token_but_no_other() {
    let (guests, _, token) = streaming(Permission::View);
    let now = Instant::now();
    for _ in 0..Lookups::LIMIT {
        assert_eq!(
            join(&guests, v4(9), "nope", now).err(),
            Some(JoinError::InvalidLink)
        );
    }
    assert_eq!(
        join(&guests, v4(9), "nope", now).err(),
        Some(JoinError::RateLimited)
    );
    assert_eq!(
        join(&guests, v4(9), &token, now).err(),
        Some(JoinError::RateLimited)
    );
    // Another address of the same /64 is the same site; another network is not.
    let site = |last: u16| IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, last));
    for _ in 0..Lookups::LIMIT {
        let _ = join(&guests, site(1), "nope", now);
    }
    assert_eq!(
        join(&guests, site(2), &token, now).err(),
        Some(JoinError::RateLimited)
    );
    assert!(join(&guests, v4(10), &token, now).is_ok());
    // A minute later the address may try again.
    assert!(join(&guests, v4(9), &token, now + Lookups::WINDOW).is_ok());
}

#[test]
fn a_revoked_link_disconnects_its_guests_and_opens_nothing_more() {
    let guests = Guests::default();
    guests.owner_started(owner());
    let (first, first_token) = guests.create(Permission::View, MIN_LIFETIME, None).unwrap();
    let (_, second_token) = guests.create(Permission::View, MIN_LIFETIME, None).unwrap();
    let now = Instant::now();
    let mut revoked = join(&guests, v4(9), &first_token, now).unwrap();
    let mut other = join(&guests, v4(10), &second_token, now).unwrap();

    assert!(guests.revoke(&first.id));
    assert_eq!(revoked.leave.try_recv().ok(), Some(Leave::Revoked));
    assert!(other.leave.try_recv().is_err(), "the other link stays");
    assert_eq!(
        join(&guests, v4(11), &first_token, now).err(),
        Some(JoinError::InvalidLink)
    );
    // Until its guest has gone, the link stays listed, and revoking it again says it was there.
    assert!(guests.status().invites.iter().any(|i| i.id == first.id));
    assert!(guests.revoke(&first.id));
    guests.left(revoked.id);
    assert!(guests.status().invites.iter().all(|i| i.id != first.id));
    assert!(!guests.revoke(&first.id));
}

#[test]
fn an_expired_link_with_a_guest_watching_stays_listed_and_can_be_revoked() {
    let (guests, id, token) = streaming(Permission::Controller);
    let mut watching = join(&guests, v4(9), &token, Instant::now()).unwrap();
    // The link expires: it lets nobody new in, and its guest stays.
    guests.inner.lock().unwrap().invites.prune(u64::MAX);
    assert!(
        guests
            .inner
            .lock()
            .unwrap()
            .invites
            .live(0)
            .next()
            .is_none()
    );
    let listed = guests.status();
    assert_eq!(listed.invites.len(), 1);
    assert_eq!(listed.invites[0].id, id);
    assert_eq!(listed.guests[0].invite, id);
    assert!(guests.open(), "a guest is watching");
    assert!(guests.revoke(&id));
    assert_eq!(watching.leave.try_recv().ok(), Some(Leave::Revoked));
    guests.left(watching.id);
    assert!(guests.status().invites.is_empty());
    assert!(
        !guests.open(),
        "no link and nobody watching: the guest port answers nothing"
    );
}

#[test]
fn a_link_carries_the_owners_address_to_its_guests() {
    let guests = Guests::default();
    assert!(!guests.open());
    guests.owner_started(owner());
    let (with, with_token) = guests
        .create(
            Permission::View,
            MIN_LIFETIME,
            Some("home.example.org:47991".into()),
        )
        .unwrap();
    let (_, without_token) = guests.create(Permission::View, MIN_LIFETIME, None).unwrap();
    assert!(guests.open());
    let now = Instant::now();
    assert_eq!(
        join(&guests, v4(9), &with_token, now)
            .unwrap()
            .address
            .as_deref(),
        Some("home.example.org:47991")
    );
    assert_eq!(
        join(&guests, v4(10), &without_token, now).unwrap().address,
        None
    );
    // Revoked, its address is forgotten with it.
    guests.revoke(&with.id);
    assert!(
        !guests
            .inner
            .lock()
            .unwrap()
            .addresses
            .contains_key(&with.id)
    );
}

#[test]
fn a_kicked_guest_is_told_to_leave_and_its_seat_frees_when_it_has() {
    let (guests, _, token) = streaming(Permission::View);
    let now = Instant::now();
    let mut kicked = join(&guests, v4(9), &token, now).unwrap();
    let mut stays = join(&guests, v4(10), &token, now).unwrap();
    assert!(guests.kick(kicked.id));
    assert_eq!(kicked.leave.try_recv().ok(), Some(Leave::Kicked));
    assert!(stays.leave.try_recv().is_err());
    guests.left(kicked.id);
    assert!(!guests.kick(kicked.id));
    assert_eq!(guests.status().guests.len(), 1);
    // Its link still works.
    assert_eq!(join(&guests, v4(9), &token, now).unwrap().slot, kicked.slot);
}

#[test]
fn no_guest_joins_without_the_owners_stream_and_that_is_no_wrong_token() {
    let guests = Guests::default();
    let (_, token) = guests.create(Permission::View, MIN_LIFETIME, None).unwrap();
    let now = Instant::now();
    for _ in 0..=Lookups::LIMIT {
        assert_eq!(
            join(&guests, v4(9), &token, now).err(),
            Some(JoinError::NotStreaming)
        );
    }
    guests.owner_started(owner());
    assert!(join(&guests, v4(9), &token, now).is_ok());
}

#[test]
fn the_guests_are_limited_and_a_seat_that_frees_is_taken_again() {
    let (guests, _, token) = streaming(Permission::View);
    let now = Instant::now();
    let seated: Vec<_> = (0..MAX_GUESTS)
        .map(|n| join(&guests, v4(n as u8 + 1), &token, now).unwrap())
        .collect();
    let slots: Vec<_> = seated.iter().map(|a| a.slot).collect();
    assert_eq!(slots, (0..MAX_GUESTS).collect::<Vec<_>>());
    assert_eq!(
        join(&guests, v4(99), &token, now).err(),
        Some(JoinError::Full)
    );
    guests.left(seated[1].id);
    assert_eq!(join(&guests, v4(99), &token, now).unwrap().slot, 1);
}

#[test]
fn the_owners_stream_ending_disconnects_every_guest_but_an_old_streams_end_does_not() {
    let guests = Guests::default();
    let first = guests.owner_started(owner());
    let (_, token) = guests.create(Permission::View, MIN_LIFETIME, None).unwrap();
    let now = Instant::now();
    let mut old = join(&guests, v4(9), &token, now).unwrap();

    // The owner switched streams: the old stream's guests go, its end changes nothing after.
    let second = guests.owner_started(owner());
    assert_ne!(first, second);
    assert_eq!(old.leave.try_recv().ok(), Some(Leave::OwnerEnded));
    guests.left(old.id);
    let mut current = join(&guests, v4(9), &token, now).unwrap();
    guests.owner_ended(first);
    assert!(current.leave.try_recv().is_err());
    assert!(guests.status().streaming);

    guests.owner_ended(second);
    assert_eq!(current.leave.try_recv().ok(), Some(Leave::OwnerEnded));
    assert!(!guests.status().streaming);
    assert_eq!(
        join(&guests, v4(10), &token, now).err(),
        Some(JoinError::NotStreaming)
    );
}

#[test]
fn the_status_never_shows_a_token() {
    let (guests, _, token) = streaming(Permission::ControllerKeyboardMouse);
    join(&guests, v4(9), &token, Instant::now()).unwrap();
    let shown = serde_json::to_string(&guests.status()).unwrap();
    assert!(!shown.contains(&token), "{shown}");
    assert!(shown.contains("\"streaming\":true"), "{shown}");
}

#[test]
fn no_keyframe_is_asked_for_without_the_owners_session() {
    let guests = Guests::default();
    assert!(!guests.keyframe(&Weak::new(), Instant::now()));
    guests.owner_started(owner());
    assert!(!guests.keyframe(&owner().session, Instant::now()));
    // Nor is the lock taken: the owner's frame thread asks while the panel lists.
    let held = guests.inner.lock().unwrap();
    assert!(!guests.keyframe(&Weak::new(), Instant::now()));
    drop(held);
}

#[test]
fn sockets_that_have_not_joined_are_limited_per_address_and_in_all() {
    let guests = Guests::default();
    let now = Instant::now();
    let open = |peer| guests.open_socket(peer, now);
    for _ in 0..MAX_SOCKETS_PER_PEER {
        assert_eq!(open(v4(9)), Ok(()));
    }
    assert_eq!(open(v4(9)), Err(JoinError::Busy));
    guests.close_socket(v4(9));
    assert_eq!(open(v4(9)), Ok(()));

    let site = |last: u16| IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, last));
    for n in 0..MAX_SOCKETS_PER_PEER {
        assert_eq!(open(site(n as u16)), Ok(()));
    }
    assert_eq!(
        open(site(99)),
        Err(JoinError::Busy),
        "one /64 is one address"
    );

    let mut last = 10;
    while guests.inner.lock().unwrap().pending.values().sum::<usize>() < MAX_SOCKETS {
        assert_eq!(open(v4(last)), Ok(()));
        last += 1;
    }
    // A refusal leaves nothing behind: addresses rotating through refusals cannot grow the map.
    let counted = guests.inner.lock().unwrap().pending.len();
    for n in 0..1000u16 {
        assert_eq!(open(site(1000 + n)), Err(JoinError::Busy));
        let rotating = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, n, 0, 0, 0, 1));
        assert_eq!(open(rotating), Err(JoinError::Busy));
    }
    assert_eq!(guests.inner.lock().unwrap().pending.len(), counted);
    assert_eq!(open(v4(200)), Err(JoinError::Busy));
    // A socket that joins no longer counts: the next page gets in.
    guests.close_socket(v4(9));
    assert_eq!(open(v4(200)), Ok(()));
    // Closed down to none, an address has no entry left.
    for _ in 0..MAX_SOCKETS_PER_PEER {
        guests.close_socket(v4(9));
    }
    assert!(!guests.inner.lock().unwrap().pending.contains_key(&v4(9)));
}

#[test]
fn a_page_that_sends_no_token_counts_as_a_wrong_one() {
    let (guests, _, token) = streaming(Permission::View);
    let now = Instant::now();
    for _ in 0..5 {
        assert_eq!(guests.open_socket(v4(9), now), Ok(()));
        guests.unanswered(v4(9), now);
        guests.close_socket(v4(9));
    }
    // That address now waits before it may open another socket, and its right token waits too.
    assert_eq!(guests.open_socket(v4(9), now), Err(JoinError::RateLimited));
    assert_eq!(
        join(&guests, v4(9), &token, now).err(),
        Some(JoinError::RateLimited)
    );
    assert!(!guests.inner.lock().unwrap().pending.contains_key(&v4(9)));
    // Another address is not held up by it.
    assert_eq!(guests.open_socket(v4(10), now), Ok(()));
}

#[test]
fn a_hosts_name_is_its_address_without_the_port() {
    assert_eq!(host_name("192.168.4.69:47990"), "192.168.4.69");
    assert_eq!(host_name("[2001:db8::1]:47990"), "2001:db8::1");
    assert_eq!(host_name("home.example.org"), "home.example.org");
}
