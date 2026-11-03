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
    let (invite, token) = guests.create(permission, MIN_LIFETIME).unwrap();
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
    let (first, first_token) = guests.create(Permission::View, MIN_LIFETIME).unwrap();
    let (_, second_token) = guests.create(Permission::View, MIN_LIFETIME).unwrap();
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
    assert!(guests.status().invites.iter().all(|i| i.id != first.id));
    assert!(!guests.revoke(&first.id));
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
    let (_, token) = guests.create(Permission::View, MIN_LIFETIME).unwrap();
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
    let (_, token) = guests.create(Permission::View, MIN_LIFETIME).unwrap();
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
    assert!(!guests.keyframe(Instant::now()));
    guests.owner_started(owner());
    assert!(!guests.keyframe(Instant::now()));
}

#[test]
fn guest_sockets_are_limited_per_address_and_in_all() {
    let guests = Guests::default();
    for _ in 0..MAX_SOCKETS_PER_PEER {
        assert!(guests.open_socket(v4(9)));
    }
    assert!(!guests.open_socket(v4(9)));
    guests.close_socket(v4(9));
    assert!(guests.open_socket(v4(9)));

    let site = |last: u16| IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, last));
    for n in 0..MAX_SOCKETS_PER_PEER {
        assert!(guests.open_socket(site(n as u16)));
    }
    assert!(!guests.open_socket(site(99)), "one /64 is one address");

    let mut last = 10;
    while guests.inner.lock().unwrap().sockets.values().sum::<usize>() < MAX_SOCKETS {
        assert!(guests.open_socket(v4(last)));
        last += 1;
    }
    assert!(!guests.open_socket(v4(200)));
    guests.close_socket(v4(9));
    assert!(guests.open_socket(v4(200)));
}

#[test]
fn a_hosts_name_is_its_address_without_the_port() {
    assert_eq!(host_name("192.168.4.69:47990"), "192.168.4.69");
    assert_eq!(host_name("[2001:db8::1]:47990"), "2001:db8::1");
    assert_eq!(host_name("home.example.org"), "home.example.org");
}
