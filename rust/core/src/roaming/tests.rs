use super::*;
use rubylight_protocol::roaming::{DATAGRAM_BYTES, MAGIC, vectors};

const OLD: &str = "192.168.1.20:50000";
const NEW: &str = "10.0.0.7:41000";
const THIRD: &str = "203.0.113.9:41000";

fn at(address: &str) -> SocketAddr {
    address.parse().unwrap()
}

/// A table holding one session "s" at [`OLD`] under the known-answer rikey, whose
/// challenges are the known-answer challenge.
fn table() -> RoamTable<&'static str> {
    let mut table = RoamTable::with_challenges(|| vectors::CHALLENGE);
    table.insert("s", &vectors::RIKEY, at(OLD));
    table
}

/// What the client sends: datagram `payload` with its own `seq`.
fn client(seq: u32, payload: Payload) -> [u8; DATAGRAM_BYTES] {
    client_for(&vectors::RIKEY, seq, payload)
}

fn client_for(rikey: &[u8; 16], seq: u32, payload: Payload) -> [u8; DATAGRAM_BYTES] {
    let keys = RoamKeys::derive(rikey);
    let header = Header {
        kind: payload.kind(),
        session_tag: keys.session_tag,
        seq,
    };
    seal(&keys.key, header, payload)
}

fn probe(seq: u32) -> [u8; DATAGRAM_BYTES] {
    client(
        seq,
        Payload::Probe {
            nonce_c: vectors::NONCE_C,
            flags: 0,
        },
    )
}

fn confirm(seq: u32, challenge: u64) -> [u8; DATAGRAM_BYTES] {
    client(seq, Payload::Confirm { challenge })
}

fn reply_to(action: Result<Action<&str>, Refusal>) -> SocketAddr {
    match action {
        Ok(Action::Reply { to, .. }) => to,
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn host_payload(datagram: &[u8]) -> Payload {
    let datagram = Datagram::decode(datagram).unwrap();
    open(&vectors::KEY, &datagram).unwrap()
}

#[test]
fn keys_and_datagrams_reproduce_the_known_answers() {
    assert_eq!(
        RoamKeys::derive(&vectors::RIKEY),
        RoamKeys {
            key: vectors::KEY,
            session_tag: vectors::SESSION_TAG
        }
    );
    let header = |kind, seq| Header {
        kind,
        session_tag: vectors::SESSION_TAG,
        seq,
    };
    let cases = [
        (
            header(Kind::Probe, 1),
            Payload::Probe {
                nonce_c: vectors::NONCE_C,
                flags: vectors::PROBE_FLAGS,
            },
            vectors::PROBE_DATAGRAM,
        ),
        (
            header(Kind::Challenge, 1),
            Payload::Challenge {
                nonce_c: vectors::NONCE_C,
                challenge: vectors::CHALLENGE,
            },
            vectors::CHALLENGE_DATAGRAM,
        ),
        (
            header(Kind::Confirm, 2),
            Payload::Confirm {
                challenge: vectors::CHALLENGE,
            },
            vectors::CONFIRM_DATAGRAM,
        ),
        (
            header(Kind::Migrated, 2),
            Payload::Migrated {
                challenge: vectors::CHALLENGE,
            },
            vectors::MIGRATED_DATAGRAM,
        ),
    ];
    for (header, payload, expected) in cases {
        assert_eq!(
            seal(&vectors::KEY, header, payload),
            expected,
            "{payload:?}"
        );
        let decoded = Datagram::decode(&expected).unwrap();
        assert_eq!(open(&vectors::KEY, &decoded), Some(payload));
    }
}

#[test]
fn the_known_exchange_moves_the_session_with_the_known_answers() {
    let mut table = table();
    let now = Instant::now();
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM),
        Ok(Action::Reply {
            to: at(NEW),
            datagram: vectors::CHALLENGE_DATAGRAM
        })
    );
    assert_eq!(table.current(&"s"), Some(at(OLD)), "a probe moves nothing");
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::CONFIRM_DATAGRAM),
        Ok(Action::Commit {
            id: "s",
            peer: at(OLD),
            old: at(OLD),
            new: at(NEW),
            media_rebound: true,
            datagram: vectors::MIGRATED_DATAGRAM
        })
    );
    assert_eq!(table.current(&"s"), Some(at(NEW)));
}

#[test]
fn a_bad_gcm_tag_or_any_changed_bit_is_dropped() {
    let now = Instant::now();
    for bit in 0..DATAGRAM_BYTES * 8 {
        let mut altered = vectors::PROBE_DATAGRAM;
        altered[bit / 8] ^= 1 << (bit % 8);
        let refusal = table().on_datagram(now, at(NEW), &altered).unwrap_err();
        assert!(
            matches!(
                refusal,
                Refusal::Authentication
                    | Refusal::Decode(_)
                    | Refusal::Direction
                    | Refusal::UnknownSession
            ),
            "bit {bit}: {refusal:?}"
        );
    }
    let mut table = table();
    let mut forged = vectors::PROBE_DATAGRAM;
    forged[20] ^= 0x80;
    assert_eq!(
        table.on_datagram(now, at(NEW), &forged),
        Err(Refusal::Authentication)
    );
    // A forgery does not consume the seq: the real probe is still taken.
    reply_to(table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM));
    // Sealed under another launch's key but carrying this session's tag.
    let other = RoamKeys::derive(&[7; 16]);
    let header = Header {
        kind: Kind::Probe,
        session_tag: vectors::SESSION_TAG,
        seq: 9,
    };
    let foreign = seal(
        &other.key,
        header,
        Payload::Probe {
            nonce_c: 1,
            flags: 0,
        },
    );
    assert_eq!(
        table.on_datagram(now, at(NEW), &foreign),
        Err(Refusal::Authentication)
    );
}

#[test]
fn a_replayed_seq_is_dropped() {
    let mut table = table();
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(5)));
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(5)),
        Err(Refusal::Replay)
    );
    assert_eq!(
        table.on_datagram(now, at(THIRD), &probe(4)),
        Err(Refusal::Replay),
        "an older probe replayed from elsewhere"
    );
    // Seq is shared by PROBE and CONFIRM.
    assert_eq!(
        table.on_datagram(now, at(NEW), &confirm(5, vectors::CHALLENGE)),
        Err(Refusal::Replay)
    );
    assert!(matches!(
        table.on_datagram(now, at(NEW), &confirm(6, vectors::CHALLENGE)),
        Ok(Action::Commit { .. })
    ));
    // The committed exchange replayed changes nothing.
    assert_eq!(
        table.on_datagram(now, at(NEW), &confirm(6, vectors::CHALLENGE)),
        Err(Refusal::Replay)
    );
    assert_eq!(table.current(&"s"), Some(at(NEW)));
}

#[test]
fn a_wrong_challenge_moves_nothing() {
    let mut table = table();
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(1)));
    assert_eq!(
        table.on_datagram(now, at(NEW), &confirm(2, vectors::CHALLENGE ^ 1)),
        Err(Refusal::Challenge)
    );
    assert_eq!(table.current(&"s"), Some(at(OLD)));
    assert!(matches!(
        table.on_datagram(now, at(NEW), &confirm(3, vectors::CHALLENGE)),
        Ok(Action::Commit { .. })
    ));
}

#[test]
fn a_confirm_from_a_third_address_moves_nothing() {
    let mut table = table();
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(1)));
    assert_eq!(
        table.on_datagram(now, at(THIRD), &confirm(2, vectors::CHALLENGE)),
        Err(Refusal::Address)
    );
    // Same IP, another port is another address too.
    let port = SocketAddr::new(at(NEW).ip(), 41001);
    assert_eq!(
        table.on_datagram(now, port, &confirm(3, vectors::CHALLENGE)),
        Err(Refusal::Address)
    );
    assert_eq!(table.current(&"s"), Some(at(OLD)));
}

#[test]
fn a_newer_probe_replaces_the_pending_address_and_a_pending_one_expires() {
    let mut challenges = 0u64;
    let mut table = RoamTable::with_challenges(move || {
        challenges += 1;
        challenges
    });
    table.insert("s", &vectors::RIKEY, at(OLD));
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(THIRD), &probe(1)));
    reply_to(table.on_datagram(now, at(NEW), &probe(2)));
    assert_eq!(
        table.on_datagram(now, at(THIRD), &confirm(3, 1)),
        Err(Refusal::Address)
    );
    assert_eq!(
        table.on_datagram(now, at(NEW), &confirm(4, 1)),
        Err(Refusal::Challenge),
        "THIRD's challenge, which NEW never heard"
    );
    reply_to(table.on_datagram(now, at(NEW), &probe(5)));
    assert_eq!(
        table.on_datagram(now + PENDING_LIFETIME, at(NEW), &confirm(6, 3)),
        Err(Refusal::NoPending)
    );
    assert_eq!(table.current(&"s"), Some(at(OLD)));
}

/// A client probes every 250 ms; over a longer round trip each CONFIRM answers an earlier
/// probe, so a repeated probe must not replace the challenge it confirms.
#[test]
fn a_repeated_probe_hears_the_pending_challenge_until_the_pending_address_expires() {
    let mut challenges = 0u64;
    let mut table = RoamTable::with_challenges(move || {
        challenges += 1;
        challenges
    });
    table.insert("s", &vectors::RIKEY, at(OLD));
    let probe_with = |seq, nonce_c| client(seq, Payload::Probe { nonce_c, flags: 0 });
    let challenge_of = |action: Result<Action<&str>, Refusal>| match action {
        Ok(Action::Reply { datagram, .. }) => match host_payload(&datagram) {
            Payload::Challenge { challenge, .. } => challenge,
            other => panic!("expected a challenge, got {other:?}"),
        },
        other => panic!("expected a reply, got {other:?}"),
    };
    let now = Instant::now();
    // PROBE(1), PROBE(2) and CONFIRM(c1) from one address commit.
    let c1 = challenge_of(table.on_datagram(now, at(NEW), &probe_with(1, 11)));
    let later = now + Duration::from_millis(250);
    assert_eq!(
        challenge_of(table.on_datagram(later, at(NEW), &probe_with(2, 11))),
        c1
    );
    assert!(matches!(
        table.on_datagram(later, at(NEW), &confirm(3, c1)),
        Ok(Action::Commit { .. })
    ));
    assert_eq!(table.current(&"s"), Some(at(NEW)));

    // A new nonce replaces the pending challenge, and the older one is then stale.
    let now = now + COMMIT_INTERVAL;
    let c2 = challenge_of(table.on_datagram(now, at(THIRD), &probe_with(4, 21)));
    let c3 = challenge_of(table.on_datagram(now, at(THIRD), &probe_with(5, 22)));
    assert_ne!(c2, c3);
    assert_eq!(
        table.on_datagram(now, at(THIRD), &confirm(6, c2)),
        Err(Refusal::Challenge)
    );
    // So does the same nonce from another address.
    let c4 = challenge_of(table.on_datagram(now, at(OLD), &probe_with(7, 22)));
    assert_ne!(c3, c4);
    assert_eq!(
        table.on_datagram(now, at(THIRD), &confirm(8, c3)),
        Err(Refusal::Address)
    );

    // Repeats do not keep a pending address alive: it expires after its first probe.
    let mut seq = 9;
    let mut at_time = now;
    while at_time < now + PENDING_LIFETIME {
        assert_eq!(
            challenge_of(table.on_datagram(at_time, at(OLD), &probe_with(seq, 22))),
            c4
        );
        seq += 1;
        at_time += Duration::from_millis(500);
    }
    assert_eq!(
        table.on_datagram(now + PENDING_LIFETIME, at(OLD), &confirm(seq, c4)),
        Err(Refusal::NoPending)
    );
    assert_eq!(table.current(&"s"), Some(at(NEW)));
}

#[test]
fn a_stopping_session_refuses_everything() {
    let mut table = table();
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(1)));
    table.set_stopping(&"s");
    assert_eq!(
        table.on_datagram(now, at(NEW), &confirm(2, vectors::CHALLENGE)),
        Err(Refusal::Stopping)
    );
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(3)),
        Err(Refusal::Stopping)
    );
    assert_eq!(table.current(&"s"), Some(at(OLD)));
    table.insert("s", &vectors::RIKEY, at(OLD));
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(4)),
        Err(Refusal::Stopping),
        "a late reconnect does not revive it"
    );
    table.remove(&"s");
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(4)),
        Err(Refusal::UnknownSession)
    );
    assert_eq!(table.current(&"s"), None);
}

#[test]
fn probes_are_rate_limited_per_session_and_overall() {
    let mut table = table();
    let start = Instant::now();
    for seq in 1..=SESSION_PER_SECOND {
        reply_to(table.on_datagram(start, at(NEW), &probe(seq)));
    }
    let over = SESSION_PER_SECOND + 1;
    assert_eq!(
        table.on_datagram(start, at(NEW), &probe(over)),
        Err(Refusal::SessionRate)
    );
    let later = start + Duration::from_secs(1);
    reply_to(table.on_datagram(later, at(NEW), &probe(over + 1)));

    // Many sessions, each within its own share, still meet the global cap.
    let mut table = RoamTable::with_challenges(|| 1);
    let sessions = GLOBAL_PER_SECOND / SESSION_PER_SECOND + 1;
    let rikeys: Vec<[u8; 16]> = (0..sessions as u128).map(u128::to_le_bytes).collect();
    for (n, rikey) in rikeys.iter().enumerate() {
        table.insert(n, rikey, at(OLD));
    }
    let mut answered = 0;
    let mut capped = 0;
    for seq in 1..=SESSION_PER_SECOND {
        for rikey in &rikeys {
            let datagram = client_for(
                rikey,
                seq,
                Payload::Probe {
                    nonce_c: 1,
                    flags: 0,
                },
            );
            match table.on_datagram(start, at(NEW), &datagram) {
                Ok(Action::Reply { .. }) => answered += 1,
                Err(Refusal::GlobalRate) => capped += 1,
                other => panic!("{other:?}"),
            }
        }
    }
    assert_eq!(answered, GLOBAL_PER_SECOND);
    assert_eq!(capped, sessions * SESSION_PER_SECOND - GLOBAL_PER_SECOND);
}

#[test]
fn junk_under_one_tag_does_not_refuse_another_session() {
    let mut table = table();
    let other = [3; 16];
    table.insert("t", &other, at(THIRD));
    let now = Instant::now();
    let mut junk = probe(1);
    junk[DATAGRAM_BYTES - 1] ^= 1;
    let mut refused = Vec::new();
    for _ in 0..GLOBAL_PER_SECOND {
        refused.push(table.on_datagram(now, at(NEW), &junk).unwrap_err());
    }
    assert!(
        refused[..PREAUTH_PER_SECOND as usize]
            .iter()
            .all(|refusal| *refusal == Refusal::Authentication)
    );
    assert!(
        refused[PREAUTH_PER_SECOND as usize..]
            .iter()
            .all(|refusal| *refusal == Refusal::SessionRate)
    );
    let probe_t = client_for(
        &other,
        1,
        Payload::Probe {
            nonce_c: 1,
            flags: 0,
        },
    );
    assert_eq!(reply_to(table.on_datagram(now, at(NEW), &probe_t)), at(NEW));
    // The flooded session waits out its second.
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(2)),
        Err(Refusal::SessionRate)
    );
    reply_to(table.on_datagram(now + Duration::from_secs(1), at(NEW), &probe(3)));
}

#[test]
fn a_session_moves_at_most_once_a_second() {
    let mut table = table();
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(1)));
    assert!(matches!(
        table.on_datagram(now, at(NEW), &confirm(2, vectors::CHALLENGE)),
        Ok(Action::Commit { .. })
    ));
    reply_to(table.on_datagram(now, at(THIRD), &probe(3)));
    assert_eq!(
        table.on_datagram(now, at(THIRD), &confirm(4, vectors::CHALLENGE)),
        Err(Refusal::CommitRate)
    );
    // The refused confirm keeps the pending address, so its retry a second later moves.
    let later = now + COMMIT_INTERVAL;
    match table.on_datagram(later, at(THIRD), &confirm(5, vectors::CHALLENGE)) {
        Ok(Action::Commit { peer, old, new, .. }) => assert_eq!(
            (peer, old, new),
            (at(OLD), at(NEW), at(THIRD)),
            "ENet still knows the peer by its first address"
        ),
        other => panic!("{other:?}"),
    }
    // A reconnect makes its address the ENet one.
    table.insert("s", &vectors::RIKEY, at(NEW));
    reply_to(table.on_datagram(later, at(OLD), &probe(6)));
    match table.on_datagram(
        later + COMMIT_INTERVAL,
        at(OLD),
        &confirm(7, vectors::CHALLENGE),
    ) {
        Ok(Action::Commit { peer, old, new, .. }) => {
            assert_eq!((peer, old, new), (at(NEW), at(NEW), at(OLD)))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_host_refuses_rather_than_wrap_its_seq() {
    let mut table = table();
    let now = Instant::now();
    table.entries[0].host_seq = u32::MAX - 1;
    let Ok(Action::Reply { datagram, .. }) = table.on_datagram(now, at(NEW), &probe(1)) else {
        panic!("the last seq is still used");
    };
    assert_eq!(Datagram::decode(&datagram).unwrap().header.seq, u32::MAX);
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(2)),
        Err(Refusal::SeqExhausted)
    );
    assert_eq!(
        table.on_datagram(now, at(NEW), &confirm(3, vectors::CHALLENGE)),
        Err(Refusal::SeqExhausted),
        "nor does it move without its MIGRATED"
    );
    assert_eq!(table.current(&"s"), Some(at(OLD)));
}

#[test]
fn a_repeated_confirm_hears_migrated_again_without_moving() {
    let mut table = table();
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(1)));
    assert!(matches!(
        table.on_datagram(now, at(NEW), &confirm(2, vectors::CHALLENGE)),
        Ok(Action::Commit { .. })
    ));
    let Ok(Action::Reply { to, datagram }) =
        table.on_datagram(now, at(NEW), &confirm(3, vectors::CHALLENGE))
    else {
        panic!("a repeated confirm is answered");
    };
    assert_eq!(to, at(NEW));
    assert_eq!(
        host_payload(&datagram),
        Payload::Migrated {
            challenge: vectors::CHALLENGE
        }
    );
    assert_eq!(
        Datagram::decode(&datagram).unwrap().header.seq,
        3,
        "the host's own seq goes on"
    );
    // From the old address it is a confirm without a probe.
    assert_eq!(
        table.on_datagram(now, at(OLD), &confirm(4, vectors::CHALLENGE)),
        Err(Refusal::NoPending)
    );
}

#[test]
fn a_probe_from_the_current_address_is_answered_but_moves_nothing() {
    let mut table = table();
    let now = Instant::now();
    let Ok(Action::Reply { to, datagram }) = table.on_datagram(now, at(OLD), &probe(1)) else {
        panic!("a probe from the current address is answered");
    };
    assert_eq!(to, at(OLD));
    assert_eq!(
        host_payload(&datagram),
        Payload::Challenge {
            nonce_c: vectors::NONCE_C,
            challenge: vectors::CHALLENGE
        }
    );
    let Ok(Action::Reply { to, datagram }) =
        table.on_datagram(now, at(OLD), &confirm(2, vectors::CHALLENGE))
    else {
        panic!("its confirm hears MIGRATED and commits nothing");
    };
    assert_eq!(to, at(OLD));
    assert!(matches!(host_payload(&datagram), Payload::Migrated { .. }));
    assert_eq!(table.current(&"s"), Some(at(OLD)));
    // Nothing moved, so a real move straight after is not held back a second.
    reply_to(table.on_datagram(now, at(NEW), &probe(3)));
    assert!(matches!(
        table.on_datagram(now, at(NEW), &confirm(4, vectors::CHALLENGE)),
        Ok(Action::Commit { .. })
    ));
}

#[test]
fn a_probe_from_the_current_address_leaves_a_pending_address_elsewhere_in_place() {
    let mut challenges = 0u64;
    let mut table = RoamTable::with_challenges(move || {
        challenges += 1;
        challenges
    });
    table.insert("s", &vectors::RIKEY, at(OLD));
    let now = Instant::now();
    reply_to(table.on_datagram(now, at(NEW), &probe(1)));
    // The old path still works now and then, so the client's probes reach it too.
    reply_to(table.on_datagram(now, at(OLD), &probe(2)));
    reply_to(table.on_datagram(now, at(OLD), &probe(3)));
    let Ok(Action::Reply { datagram, .. }) = table.on_datagram(now, at(OLD), &confirm(4, 2)) else {
        panic!("the current address's confirm is answered");
    };
    assert!(matches!(host_payload(&datagram), Payload::Migrated { .. }));
    match table.on_datagram(now, at(NEW), &confirm(5, 1)) {
        Ok(Action::Commit { old, new, .. }) => assert_eq!((old, new), (at(OLD), at(NEW))),
        other => panic!("the new address still commits, got {other:?}"),
    }
}

#[test]
fn no_answer_is_larger_than_its_request_and_only_requests_of_52_bytes_are_answered() {
    let mut table = table();
    let now = Instant::now();
    let requests: Vec<(SocketAddr, Vec<u8>)> = vec![
        (at(NEW), probe(1).to_vec()),
        (at(NEW), confirm(2, vectors::CHALLENGE).to_vec()),
        (at(NEW), confirm(3, vectors::CHALLENGE).to_vec()),
        (at(OLD), probe(4).to_vec()),
        (at(THIRD), probe(5).to_vec()),
        (at(THIRD), [probe(6).as_slice(), &[0; 100]].concat()),
        (at(THIRD), probe(7)[..36].to_vec()),
        (at(THIRD), MAGIC.to_vec()),
        (at(THIRD), vec![]),
    ];
    let mut answered = 0;
    for (from, request) in requests {
        let datagram = match table.on_datagram(now, from, &request) {
            Ok(Action::Reply { datagram, .. } | Action::Commit { datagram, .. }) => datagram,
            Err(_) => continue,
        };
        answered += 1;
        assert!(datagram.len() <= request.len(), "{} bytes", request.len());
        assert_eq!(request.len(), DATAGRAM_BYTES);
    }
    assert_eq!(answered, 5);
}

#[test]
fn host_bound_types_only() {
    let mut table = table();
    let now = Instant::now();
    for datagram in [vectors::CHALLENGE_DATAGRAM, vectors::MIGRATED_DATAGRAM] {
        assert_eq!(
            table.on_datagram(now, at(NEW), &datagram),
            Err(Refusal::Direction)
        );
    }
    let mut unknown = RoamTable::<&str>::new();
    assert_eq!(
        unknown.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM),
        Err(Refusal::UnknownSession)
    );
}

#[test]
fn a_removed_peer_that_reconnects_keeps_both_seqs() {
    let mut table = table();
    let now = Instant::now();
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM),
        Ok(Action::Reply {
            to: at(NEW),
            datagram: vectors::CHALLENGE_DATAGRAM
        })
    );
    // The ENet peer disconnects and comes back under the same launch.
    table.remove(&"s");
    assert_eq!(table.current(&"s"), None);
    table.insert("s", &vectors::RIKEY, at(THIRD));
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM),
        Err(Refusal::Replay)
    );
    let Ok(Action::Reply { datagram, .. }) = table.on_datagram(now, at(NEW), &probe(2)) else {
        panic!("a fresh probe is answered");
    };
    assert_eq!(
        Datagram::decode(&datagram).unwrap().header.seq,
        2,
        "the host's seq goes on"
    );
    // A stopping launch stays stopping too.
    table.set_stopping(&"s");
    table.remove(&"s");
    table.insert("s", &vectors::RIKEY, at(OLD));
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(3)),
        Err(Refusal::Stopping)
    );
    // Only the newest removals are remembered.
    table.remove(&"s");
    for n in 1..=RETIRED as u128 {
        table.insert("other", &n.to_le_bytes(), at(OLD));
        table.remove(&"other");
    }
    table.insert("s", &vectors::RIKEY, at(OLD));
    reply_to(table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM));
}

#[test]
fn a_reconnect_keeps_both_seqs_so_no_nonce_repeats() {
    let mut table = table();
    let now = Instant::now();
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM),
        Ok(Action::Reply {
            to: at(NEW),
            datagram: vectors::CHALLENGE_DATAGRAM
        })
    );
    // The ENet peer reconnects from THIRD under the same launch.
    table.insert("s", &vectors::RIKEY, at(THIRD));
    assert_eq!(table.current(&"s"), Some(at(THIRD)));
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::PROBE_DATAGRAM),
        Err(Refusal::Replay)
    );
    assert_eq!(
        table.on_datagram(now, at(NEW), &vectors::CONFIRM_DATAGRAM),
        Err(Refusal::NoPending),
        "a reconnect drops the pending address"
    );
    let Ok(Action::Reply { datagram, .. }) = table.on_datagram(now, at(NEW), &probe(3)) else {
        panic!("a fresh probe is answered");
    };
    assert_eq!(Datagram::decode(&datagram).unwrap().header.seq, 2);
    // A new launch under the same id starts over.
    table.insert("s", &[9; 16], at(OLD));
    let fresh = client_for(
        &[9; 16],
        1,
        Payload::Probe {
            nonce_c: 1,
            flags: 0,
        },
    );
    reply_to(table.on_datagram(now, at(NEW), &fresh));
    assert_eq!(
        table.on_datagram(now, at(NEW), &probe(4)),
        Err(Refusal::UnknownSession),
        "the old launch's key is gone"
    );
}
