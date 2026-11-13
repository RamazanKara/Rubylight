use super::*;
use rand::{SeedableRng, rngs::StdRng};

fn rng() -> StdRng {
    StdRng::seed_from_u64(7)
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

// Computed with Python's hmac/hashlib: secret = bytes 0x00..0x1f,
// nonce = bytes 0xa0..0xbf in hex.
const SECRET: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const NONCE: &str = "a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf";
const PROOF: &str = "6cc53d0e9647fd7c04a198c1c2f1bbb425ecf7957567b61208033dfd029c9a63";

#[test]
fn proof_has_a_fixed_vector() {
    assert_eq!(proof_hex(SECRET, NONCE).as_deref(), Some(PROOF));
    // A different nonce gives a different proof.
    let other = NONCE.replace("a0a1", "a1a0");
    assert_ne!(proof_hex(SECRET, &other).as_deref(), Some(PROOF));
}

#[test]
fn proof_needs_a_well_formed_secret() {
    assert_eq!(proof_hex("", NONCE), None);
    assert_eq!(proof_hex("zz", NONCE), None);
    assert_eq!(proof_hex(&SECRET[..62], NONCE), None);
    assert_eq!(proof_hex(&SECRET.to_uppercase(), NONCE), None);
}

#[test]
fn nonces_are_256_bit_hex_and_differ() {
    let mut rng = rng();
    let a = generate_nonce(&mut rng);
    let b = generate_nonce(&mut rng);
    assert!(is_lower_hex(&a, 32));
    assert!(is_lower_hex(&b, 32));
    assert_ne!(a, b);
    // The OS RNG works too.
    assert!(is_lower_hex(&generate_nonce(&mut rand::rngs::OsRng), 32));
}

#[test]
fn credentials_have_the_documented_sizes() {
    let mut rng = rng();
    let a = Credentials::generate(&mut rng);
    let b = Credentials::generate(&mut rng);
    assert!(is_lower_hex(&a.client_id, 16));
    assert!(is_lower_hex(&a.secret, 32));
    assert_ne!(a, b);
    assert!(!format!("{a:?}").contains(&a.secret));
}

#[test]
fn challenge_accepts_the_right_proof_once() {
    let mut challenge = Challenge::new(&mut rng());
    let nonce = challenge.nonce().unwrap().to_string();
    let proof = proof_hex(SECRET, &nonce).unwrap();
    assert!(challenge.verify(Some(SECRET), &proof));
    // Single use: the same proof fails now, and there is no nonce left.
    assert_eq!(challenge.nonce(), None);
    assert!(!challenge.verify(Some(SECRET), &proof));
}

#[test]
fn a_failed_attempt_spends_the_nonce_too() {
    let mut challenge = Challenge::new(&mut rng());
    let nonce = challenge.nonce().unwrap().to_string();
    let proof = proof_hex(SECRET, &nonce).unwrap();
    assert!(!challenge.verify(Some(SECRET), &"0".repeat(64)));
    assert!(!challenge.verify(Some(SECRET), &proof));
}

#[test]
fn challenge_refuses_wrong_secret_unknown_client_and_malformed_proofs() {
    let other = "ff".repeat(32);
    for (secret, proof) in [
        (Some(other.as_str()), None),
        (None, None),
        (Some(SECRET), Some("")),
        (Some(SECRET), Some("zz")),
    ] {
        let mut challenge = Challenge::new(&mut rng());
        let nonce = challenge.nonce().unwrap().to_string();
        let proof = proof
            .map(str::to_string)
            .unwrap_or_else(|| proof_hex(SECRET, &nonce).unwrap());
        assert!(!challenge.verify(secret, &proof));
    }
}

#[test]
fn challenge_accepts_an_upper_case_proof() {
    let mut challenge = Challenge::new(&mut rng());
    let nonce = challenge.nonce().unwrap().to_string();
    let proof = proof_hex(SECRET, &nonce).unwrap().to_uppercase();
    assert!(challenge.verify(Some(SECRET), &proof));
}

#[test]
fn discarding_the_challenge_spends_it() {
    let mut challenge = Challenge::new(&mut rng());
    let nonce = challenge.nonce().unwrap().to_string();
    challenge.discard();
    assert!(!challenge.verify(Some(SECRET), &proof_hex(SECRET, &nonce).unwrap()));
}

#[test]
fn constant_time_eq_compares_whole_strings() {
    assert!(constant_time_eq("1234", "1234"));
    assert!(!constant_time_eq("1234", "1235"));
    assert!(!constant_time_eq("1234", "123"));
    assert!(!constant_time_eq("", "0"));
    assert!(constant_time_eq("", ""));
}

#[test]
fn pending_pin_allows_one_attempt() {
    let mut pin = PendingPin::new(&mut rng(), "0421", "Samsung TV", secs(10));
    assert!(is_lower_hex(pin.request_id(), 16));
    assert_eq!(pin.client_name(), "Samsung TV");
    assert_eq!(
        pin.attempt("1111", secs(20)),
        Err(PairingFailure::IncorrectPin)
    );
    // The right PIN no longer works: one attempt.
    assert_eq!(pin.attempt("0421", secs(21)), Err(PairingFailure::Failed));
    let mut pin = PendingPin::new(&mut rng(), "0421", "TV", secs(0));
    assert_eq!(pin.attempt("0421", secs(299)), Ok(()));
}

#[test]
fn pending_pin_expires_after_five_minutes() {
    let mut pin = PendingPin::new(&mut rng(), "0421", "TV", secs(100));
    assert!(!pin.is_expired(secs(399)));
    assert!(pin.is_expired(secs(400)));
    assert_eq!(pin.attempt("0421", secs(400)), Err(PairingFailure::Expired));
    // A clock that steps back is not an expiry.
    assert!(!pin.is_expired(secs(0)));
}

#[test]
fn pairing_failures_use_the_protocol_codes() {
    assert_eq!(PairingFailure::Expired.code().as_str(), "pairing-expired");
    assert_eq!(
        PairingFailure::IncorrectPin.code().as_str(),
        "incorrect-pin"
    );
    assert_eq!(
        PairingFailure::Cancelled.code().as_str(),
        "pairing-cancelled"
    );
    assert_eq!(PairingFailure::Failed.code().as_str(), "pairing-failed");
}

#[test]
fn request_ids_are_unique_and_unguessable() {
    let mut rng = rng();
    let a = PendingPin::new(&mut rng, "0001", "a", secs(0));
    let b = PendingPin::new(&mut rng, "0001", "a", secs(0));
    assert_ne!(a.request_id(), b.request_id());
    assert_eq!(a.request_id().len(), 32);
}

#[test]
fn approving_a_request_consumes_it() {
    let mut rng = rng();
    let mut pins = PendingPins::new();
    let a = PendingPin::new(&mut rng, "1111", "a", secs(0));
    let b = PendingPin::new(&mut rng, "2222", "b", secs(0));
    let (id_a, id_b) = (a.request_id().to_string(), b.request_id().to_string());
    pins.add(a);
    pins.add(b);
    assert_eq!(pins.waiting(secs(1)).count(), 2);
    // A wrong PIN ends the request.
    assert_eq!(
        pins.approve(&id_a, "9999", secs(1)).unwrap_err(),
        PairingFailure::IncorrectPin
    );
    assert_eq!(
        pins.approve(&id_a, "1111", secs(1)).unwrap_err(),
        PairingFailure::Failed
    );
    let done = pins.approve(&id_b, "2222", secs(2)).unwrap();
    assert_eq!(done.client_name(), "b");
    assert!(pins.is_empty());
    assert_eq!(
        pins.approve("nope", "1", secs(0)).unwrap_err(),
        PairingFailure::Failed
    );
}

#[test]
fn expired_requests_are_not_listed_and_fail_to_approve() {
    let mut rng = rng();
    let mut pins = PendingPins::new();
    let a = PendingPin::new(&mut rng, "1111", "a", secs(0));
    let id = a.request_id().to_string();
    pins.add(a);
    assert_eq!(pins.waiting(secs(300)).count(), 0);
    assert_eq!(
        pins.approve(&id, "1111", secs(300)).unwrap_err(),
        PairingFailure::Expired
    );
    let b = PendingPin::new(&mut rng, "1111", "b", secs(0));
    pins.add(b);
    pins.prune(secs(300));
    assert!(pins.is_empty());
}

#[test]
fn cancel_removes_a_request() {
    let mut rng = rng();
    let mut pins = PendingPins::new();
    let a = PendingPin::new(&mut rng, "1111", "a", secs(0));
    let id = a.request_id().to_string();
    pins.add(a);
    assert!(pins.cancel(&id).is_some());
    assert!(pins.cancel(&id).is_none());
    assert_eq!(
        pins.approve(&id, "1111", secs(0)).unwrap_err(),
        PairingFailure::Failed
    );
}

#[test]
fn pending_requests_are_bounded_and_the_oldest_goes() {
    let mut rng = rng();
    let mut pins = PendingPins::new();
    let first = PendingPin::new(&mut rng, "0000", "first", secs(0));
    let first_id = first.request_id().to_string();
    assert!(pins.add(first).is_none());
    for n in 1..MAX_PENDING_PINS {
        assert!(
            pins.add(PendingPin::new(
                &mut rng,
                "0000",
                format!("{n}"),
                secs(n as u64)
            ))
            .is_none()
        );
    }
    let dropped = pins
        .add(PendingPin::new(&mut rng, "0000", "extra", secs(99)))
        .unwrap();
    assert_eq!(dropped.request_id(), first_id);
    assert_eq!(pins.len(), MAX_PENDING_PINS);
}

#[test]
fn nine_unauthenticated_connections_drop_the_oldest() {
    let mut table = ConnectionTable::default();
    for id in 1..=8 {
        assert!(table.open(id).is_empty());
    }
    assert_eq!(
        table.open(9),
        vec![Dropped {
            id: 1,
            was_streaming: false
        }]
    );
    assert_eq!(table.unauthenticated(), 8);
    assert!(table.state(1).is_none());
    assert_eq!(
        table.open(10),
        vec![Dropped {
            id: 2,
            was_streaming: false
        }]
    );
}

#[test]
fn unauthenticated_connections_never_displace_a_stream() {
    let mut table = ConnectionTable::default();
    table.open(1);
    table.authenticate(1, "tv").unwrap();
    table.set_streaming(1, true);
    table.open(2);
    table.authenticate(2, "other").unwrap();
    // Fill and overflow with strangers, many times over.
    let mut dropped = Vec::new();
    for id in 100..140 {
        dropped.extend(table.open(id));
    }
    assert!(dropped.iter().all(|d| d.id >= 100 && !d.was_streaming));
    assert_eq!(
        table.state(1),
        Some(&ConnectionState::Streaming {
            client_id: "tv".into()
        })
    );
    assert!(matches!(
        table.state(2),
        Some(ConnectionState::Authenticated { .. })
    ));
    assert_eq!(table.unauthenticated(), 8);
}

#[test]
fn authenticated_connections_free_an_unauthenticated_slot() {
    let mut table = ConnectionTable::default();
    for id in 1..=8 {
        table.open(id);
    }
    table.authenticate(8, "tv").unwrap();
    assert_eq!(table.unauthenticated(), 7);
    assert!(table.open(9).is_empty());
}

#[test]
fn a_connection_authenticated_from_the_start_takes_no_waiting_slot_and_replaces_its_own() {
    let mut table = ConnectionTable::default();
    for id in 1..=8 {
        table.open(id);
    }
    // The unauthenticated slots are full; none of the waiting TVs goes.
    assert!(table.open_authenticated(9, "console").is_empty());
    assert_eq!(table.unauthenticated(), 8);
    assert!(matches!(
        table.state(9),
        Some(ConnectionState::Authenticated { .. })
    ));
    table.set_streaming(9, true);
    // A second tab of the same console replaces the first, stream and all.
    assert_eq!(
        table.open_authenticated(10, "console"),
        vec![Dropped {
            id: 9,
            was_streaming: true
        }]
    );
    assert!(table.state(9).is_none());
    assert_eq!(table.len(), 9);
}

#[test]
fn a_reconnecting_client_replaces_its_old_connection() {
    let mut table = ConnectionTable::default();
    table.open(1);
    table.authenticate(1, "tv").unwrap();
    table.set_streaming(1, true);
    table.open(2);
    let dropped = table.authenticate(2, "tv").unwrap();
    assert_eq!(
        dropped,
        vec![Dropped {
            id: 1,
            was_streaming: true
        }]
    );
    assert!(table.state(1).is_none());
    assert!(matches!(
        table.state(2),
        Some(ConnectionState::Authenticated { .. })
    ));
}

#[test]
fn disabling_or_unpairing_drops_every_connection_of_the_client() {
    let mut table = ConnectionTable::default();
    for id in 1..=3 {
        table.open(id);
    }
    table.authenticate(1, "tv").unwrap();
    table.set_streaming(1, true);
    table.authenticate(2, "phone").unwrap();
    assert_eq!(
        table.drop_client("tv"),
        vec![Dropped {
            id: 1,
            was_streaming: true
        }]
    );
    assert!(table.drop_client("tv").is_empty());
    assert_eq!(table.len(), 2);
    table.close(2);
    table.close(3);
    assert!(table.is_empty());
}

#[test]
fn authenticating_twice_leaves_the_connection_as_it_was() {
    let mut table = ConnectionTable::default();
    table.open(1);
    table.authenticate(1, "tv").unwrap();
    table.set_streaming(1, true);
    // A second authenticate, as the same or another client, is refused
    // and the stream state survives.
    for client in ["tv", "other"] {
        assert_eq!(
            table.authenticate(1, client),
            Err(ErrorCode::AlreadyAuthenticated)
        );
        assert_eq!(
            table.state(1),
            Some(&ConnectionState::Streaming {
                client_id: "tv".into()
            })
        );
    }
    table.set_streaming(1, false);
    assert_eq!(
        table.authenticate(1, "other"),
        Err(ErrorCode::AlreadyAuthenticated)
    );
    assert_eq!(
        table.state(1),
        Some(&ConnectionState::Authenticated {
            client_id: "tv".into()
        })
    );
}

#[test]
fn streaming_flag_only_applies_after_authentication() {
    let mut table = ConnectionTable::default();
    table.open(1);
    table.set_streaming(1, true);
    assert_eq!(table.state(1), Some(&ConnectionState::Unauthenticated));
    table.authenticate(1, "tv").unwrap();
    table.set_streaming(1, true);
    table.set_streaming(1, false);
    assert_eq!(
        table.state(1),
        Some(&ConnectionState::Authenticated {
            client_id: "tv".into()
        })
    );
    assert!(table.authenticate(99, "x").unwrap().is_empty());
}

#[test]
fn the_roster_keeps_32_and_drops_the_oldest() {
    let mut roster = PairedRoster::new();
    for n in 0..MAX_PAIRED as u64 {
        assert!(roster.insert(&format!("client-{n}"), n).is_empty());
    }
    assert_eq!(roster.len(), 32);
    assert_eq!(roster.insert("newest", 100), vec!["client-0".to_string()]);
    assert_eq!(roster.insert("newer", 101), vec!["client-1".to_string()]);
    assert_eq!(roster.len(), 32);
    assert!(!roster.contains("client-0"));
    assert!(roster.contains("newest"));
}

#[test]
fn re_pairing_a_client_makes_it_newest_without_a_duplicate() {
    let mut roster = PairedRoster::new();
    for n in 0..MAX_PAIRED as u64 {
        roster.insert(&format!("client-{n}"), n);
    }
    assert!(roster.insert("client-0", 500).is_empty());
    assert_eq!(roster.len(), 32);
    // client-1 is the oldest now.
    assert_eq!(roster.insert("fresh", 501), vec!["client-1".to_string()]);
    assert!(roster.remove("client-0"));
    assert!(!roster.remove("client-0"));
}

#[test]
fn a_stored_roster_over_the_limit_is_trimmed_oldest_first() {
    let entries = (0..35u64).rev().map(|n| (format!("c{n}"), n));
    let (roster, surplus) = PairedRoster::from_entries(entries);
    assert_eq!(roster.len(), 32);
    assert_eq!(surplus, vec!["c0", "c1", "c2"]);
}
