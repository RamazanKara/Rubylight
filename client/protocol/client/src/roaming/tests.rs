use super::*;
use rubylight_protocol::roaming::{vectors, Kind, PROBE_FLAG_MEDIA_REBOUND};

const NONCE: u64 = 0x1111_2222_3333_4444;

/// The host's side: seals CHALLENGE and MIGRATED under the vectors' key.
struct FakeHost {
    gcm: Aes128Gcm,
    seq: u32,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            gcm: Aes128Gcm::new(GenericArray::from_slice(&vectors::KEY)),
            seq: 0,
        }
    }

    fn send(&mut self, payload: Payload) -> [u8; DATAGRAM_BYTES] {
        self.seq += 1;
        let header = Header {
            kind: payload.kind(),
            session_tag: vectors::SESSION_TAG,
            seq: self.seq,
        };
        seal(&self.gcm, header, payload)
    }

    /// Opens a client datagram.
    fn open(&self, wire: &[u8; DATAGRAM_BYTES]) -> (Header, Payload) {
        let datagram = Datagram::decode(wire).unwrap();
        (datagram.header, open(&self.gcm, &datagram).expect("opens"))
    }
}

fn sent(roaming: &mut Roaming) -> Vec<[u8; DATAGRAM_BYTES]> {
    std::iter::from_fn(|| roaming.poll_transmit()).collect()
}

/// A started attempt that has sent its first PROBE.
fn probing(now: Instant) -> (Roaming, FakeHost) {
    let mut roaming = Roaming::new(&vectors::RIKEY, Duration::from_secs(60));
    roaming.start(now, RoamTrigger::NetworkChanged, 0, NONCE);
    roaming.poll(now);
    let host = FakeHost::new();
    let probes = sent(&mut roaming);
    assert_eq!(probes.len(), 1);
    assert_eq!(
        host.open(&probes[0]).1,
        Payload::Probe {
            nonce_c: NONCE,
            flags: 0
        }
    );
    assert_eq!(
        roaming.take_events(),
        [RoamEvent::Started {
            trigger: RoamTrigger::NetworkChanged
        }]
    );
    (roaming, host)
}

#[test]
fn key_and_tag_derive_like_the_vectors() {
    assert_eq!(
        derive(&vectors::RIKEY),
        (vectors::KEY, vectors::SESSION_TAG)
    );
}

#[test]
fn the_known_answers_run_end_to_end() {
    let now = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, MAX_ATTEMPT);
    roaming.start(
        now,
        RoamTrigger::NetworkChanged,
        vectors::PROBE_FLAGS,
        vectors::NONCE_C,
    );
    roaming.poll(now);
    assert_eq!(sent(&mut roaming), [vectors::PROBE_DATAGRAM]);
    let later = now + Duration::from_millis(40);
    roaming
        .handle_datagram(later, &vectors::CHALLENGE_DATAGRAM)
        .unwrap();
    assert_eq!(sent(&mut roaming), [vectors::CONFIRM_DATAGRAM]);
    roaming
        .handle_datagram(later, &vectors::MIGRATED_DATAGRAM)
        .unwrap();
    assert!(sent(&mut roaming).is_empty());
    assert_eq!(
        roaming.take_events(),
        [
            RoamEvent::Started {
                trigger: RoamTrigger::NetworkChanged
            },
            RoamEvent::Roamed {
                elapsed: Duration::from_millis(40)
            }
        ]
    );
    assert_eq!(roaming.trigger(), None);
    assert_eq!(roaming.poll(later), None, "nothing more to send");
}

#[test]
fn happy_path_confirms_and_roams() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let challenge = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 77,
    });
    roaming.handle_datagram(now, &challenge).unwrap();
    let confirm = sent(&mut roaming);
    let (header, payload) = host.open(&confirm[0]);
    assert_eq!((header.kind, header.seq), (Kind::Confirm, 2));
    assert_eq!(payload, Payload::Confirm { challenge: 77 });
    let migrated = host.send(Payload::Migrated { challenge: 77 });
    let at = now + Duration::from_millis(90);
    roaming.handle_datagram(at, &migrated).unwrap();
    assert_eq!(
        roaming.take_events(),
        [RoamEvent::Roamed {
            elapsed: Duration::from_millis(90)
        }]
    );
    // A late MIGRATED finds nothing to finish.
    let again = host.send(Payload::Migrated { challenge: 77 });
    assert_eq!(roaming.handle_datagram(at, &again), Err(Rejected::Idle));
    assert!(roaming.take_events().is_empty());
}

#[test]
fn a_bad_tag_is_dropped_without_an_answer() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let mut challenge = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    challenge[DATAGRAM_BYTES - 1] ^= 1;
    assert_eq!(
        roaming.handle_datagram(now, &challenge),
        Err(Rejected::Authentication)
    );
    // Another key: the tag does not verify either.
    let mut other = FakeHost::new();
    other.gcm = Aes128Gcm::new(GenericArray::from_slice(&[9; 16]));
    other.seq = 5;
    let forged = other.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    assert_eq!(
        roaming.handle_datagram(now, &forged),
        Err(Rejected::Authentication)
    );
    assert!(sent(&mut roaming).is_empty());
    // A failed open does not use up the host's seq.
    let genuine = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(now, &genuine).unwrap();
    assert_eq!(sent(&mut roaming).len(), 1);
}

#[test]
fn other_sessions_and_client_types_are_dropped() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let mut wire = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    wire[8] ^= 1;
    assert_eq!(roaming.handle_datagram(now, &wire), Err(Rejected::Session));
    // Our own PROBE reflected back.
    assert_eq!(
        roaming.handle_datagram(now, &vectors::PROBE_DATAGRAM),
        Err(Rejected::Direction)
    );
    assert_eq!(
        roaming.handle_datagram(now, &wire[..51]),
        Err(Rejected::Decode(DecodeError::Length))
    );
    assert!(sent(&mut roaming).is_empty());
}

#[test]
fn a_wrong_challenge_or_nonce_moves_nothing() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let stale = host.send(Payload::Challenge {
        nonce_c: NONCE + 1,
        challenge: 5,
    });
    assert_eq!(roaming.handle_datagram(now, &stale), Err(Rejected::Nonce));
    assert!(
        sent(&mut roaming).is_empty(),
        "no CONFIRM for another burst"
    );
    let challenge = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(now, &challenge).unwrap();
    assert_eq!(sent(&mut roaming).len(), 1);
    let wrong = host.send(Payload::Migrated { challenge: 6 });
    assert_eq!(
        roaming.handle_datagram(now, &wrong),
        Err(Rejected::Challenge)
    );
    assert!(roaming.take_events().is_empty());
    assert_eq!(roaming.trigger(), Some(RoamTrigger::NetworkChanged));
}

#[test]
fn a_duplicate_challenge_is_confirmed_once_per_datagram() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let first = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(now, &first).unwrap();
    assert_eq!(sent(&mut roaming).len(), 1);
    // The same datagram again is a replay: no second CONFIRM.
    assert_eq!(roaming.handle_datagram(now, &first), Err(Rejected::Replay));
    assert!(sent(&mut roaming).is_empty());
    // The host re-sends the challenge, and then challenges a later PROBE anew.
    let resent = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(now, &resent).unwrap();
    let newer = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 8,
    });
    roaming.handle_datagram(now, &newer).unwrap();
    let confirms: Vec<_> = sent(&mut roaming)
        .iter()
        .map(|wire| host.open(wire))
        .collect();
    assert_eq!(confirms.len(), 2);
    assert_eq!(confirms[0].0.seq + 1, confirms[1].0.seq, "fresh seqs");
    assert_eq!(confirms[1].1, Payload::Confirm { challenge: 8 });
    // The host committed the first challenge before the second arrived.
    let migrated = host.send(Payload::Migrated { challenge: 5 });
    roaming.handle_datagram(now, &migrated).unwrap();
    assert!(matches!(
        roaming.take_events()[..],
        [RoamEvent::Roamed { .. }]
    ));
}

#[test]
fn probes_back_off_then_give_up() {
    let now = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, Duration::from_secs(60));
    assert_eq!(
        roaming.give_up, MAX_ATTEMPT,
        "a long drop tolerance is capped"
    );
    roaming.start(now, RoamTrigger::HostSilent, 0, NONCE);
    let mut probes = Vec::new();
    let mut at = now;
    while let Some(next) = roaming.poll(at) {
        if !sent(&mut roaming).is_empty() {
            probes.push(at - now);
        }
        assert!(next > at);
        at = next;
    }
    assert_eq!(at - now, MAX_ATTEMPT);
    let fast: Vec<_> = (0..FAST_PROBES).map(|i| FAST_PROBE_INTERVAL * i).collect();
    assert_eq!(probes[..FAST_PROBES as usize], fast[..]);
    let slow: Vec<_> = (0..7)
        .map(|i| Duration::from_millis(3750) + SLOW_PROBE_INTERVAL * i)
        .collect();
    assert_eq!(probes[FAST_PROBES as usize..], slow[..]);
    assert_eq!(
        roaming.take_events(),
        [
            RoamEvent::Started {
                trigger: RoamTrigger::HostSilent
            },
            RoamEvent::Failed {
                elapsed: MAX_ATTEMPT,
                rejected: Rejections::default(),
            }
        ]
    );
    assert_eq!(roaming.poll(at + MAX_ATTEMPT), None);
    assert!(sent(&mut roaming).is_empty(), "nothing after giving up");
}

#[test]
fn a_short_drop_tolerance_gives_up_sooner() {
    let now = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, Duration::from_secs(2));
    roaming.start(now, RoamTrigger::NetworkChanged, 0, NONCE);
    assert!(roaming.poll(now).is_some());
    assert_eq!(roaming.poll(now + Duration::from_secs(2)), None);
    assert!(matches!(
        roaming.take_events()[..],
        [
            RoamEvent::Started { .. },
            RoamEvent::Failed { elapsed, .. }
        ] if elapsed == Duration::from_secs(2)
    ));
}

#[test]
fn a_new_network_restarts_the_probes_under_a_new_nonce() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let later = now + Duration::from_secs(4);
    roaming.poll(later);
    sent(&mut roaming);
    let garbage = [0x0F, 0xFE, 0x52, 0x4D, 1];
    assert!(roaming.handle_datagram(later, &garbage).is_err());
    roaming.start(
        later,
        RoamTrigger::NetworkChanged,
        PROBE_FLAG_MEDIA_REBOUND,
        NONCE + 1,
    );
    assert_eq!(
        roaming.poll(later),
        Some(later + FAST_PROBE_INTERVAL),
        "fast again"
    );
    let probe = sent(&mut roaming);
    assert_eq!(
        host.open(&probe[0]).1,
        Payload::Probe {
            nonce_c: NONCE + 1,
            flags: PROBE_FLAG_MEDIA_REBOUND
        }
    );
    assert_eq!(
        roaming.take_events(),
        [RoamEvent::Started {
            trigger: RoamTrigger::NetworkChanged
        }],
        "an attempt on the new network"
    );
    let old = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 1,
    });
    assert_eq!(roaming.handle_datagram(later, &old), Err(Rejected::Nonce));
    // The give-up time counts from the change.
    assert!(roaming
        .poll(later + MAX_ATTEMPT - FAST_PROBE_INTERVAL)
        .is_some());
    assert_eq!(roaming.poll(later + MAX_ATTEMPT), None);
    assert_eq!(
        roaming.take_events(),
        [RoamEvent::Failed {
            elapsed: MAX_ATTEMPT,
            // Only what the attempt on the new network dropped.
            rejected: Rejections {
                nonce: 1,
                ..Rejections::default()
            },
        }]
    );
}

#[test]
fn a_failed_attempt_counts_what_it_dropped() {
    let now = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, Duration::from_secs(2));
    let mut host = FakeHost::new();
    // Idle: nothing counts toward the next attempt.
    let idle = host.send(Payload::Migrated { challenge: 1 });
    assert_eq!(roaming.handle_datagram(now, &idle), Err(Rejected::Idle));
    roaming.start(now, RoamTrigger::HostSilent, 0, NONCE);
    roaming.poll(now);
    let stale = host.send(Payload::Challenge {
        nonce_c: NONCE + 1,
        challenge: 2,
    });
    assert_eq!(roaming.handle_datagram(now, &stale), Err(Rejected::Nonce));
    assert_eq!(roaming.handle_datagram(now, &stale), Err(Rejected::Replay));
    assert_eq!(roaming.handle_datagram(now, &idle), Err(Rejected::Replay));
    let unconfirmed = host.send(Payload::Migrated { challenge: 3 });
    assert_eq!(
        roaming.handle_datagram(now, &unconfirmed),
        Err(Rejected::Challenge)
    );
    let mut forged = host.send(Payload::Migrated { challenge: 3 });
    forged[DATAGRAM_BYTES - 1] ^= 1;
    assert_eq!(
        roaming.handle_datagram(now, &forged),
        Err(Rejected::Authentication)
    );
    assert_eq!(roaming.poll(now + Duration::from_secs(2)), None);
    let events = roaming.take_events();
    let rejected = Rejections {
        replay: 2,
        authentication: 1,
        nonce: 1,
        challenge: 1,
        ..Rejections::default()
    };
    assert_eq!(
        events[1],
        RoamEvent::Failed {
            elapsed: Duration::from_secs(2),
            rejected,
        }
    );
    assert_eq!(
        events[1].to_string(),
        "Roaming: the PC did not confirm a move within 2000 ms; dropped 2 replayed, \
             1 unauthenticated, 1 for another nonce, 1 for an unconfirmed challenge"
    );
}

#[test]
fn a_network_change_during_a_silence_takes_over_the_attempt() {
    let now = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, MAX_ATTEMPT);
    let mut host = FakeHost::new();
    roaming.start(now, RoamTrigger::HostSilent, 0, NONCE);
    roaming.poll(now);
    sent(&mut roaming);
    let challenge = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(now, &challenge).unwrap();
    sent(&mut roaming);
    let change = now + Duration::from_secs(2);
    roaming.start(change, RoamTrigger::NetworkChanged, 0, NONCE + 1);
    assert_eq!(roaming.trigger(), Some(RoamTrigger::NetworkChanged));
    assert_eq!(
        roaming.take_events(),
        [
            RoamEvent::Started {
                trigger: RoamTrigger::HostSilent
            },
            RoamEvent::Started {
                trigger: RoamTrigger::NetworkChanged
            }
        ]
    );
    // The silence's challenge was for the old network.
    let stale = host.send(Payload::Migrated { challenge: 5 });
    assert_eq!(
        roaming.handle_datagram(change, &stale),
        Err(Rejected::Challenge)
    );
    roaming.poll(change);
    assert_eq!(
        opened(&host, &sent(&mut roaming)),
        [Payload::Probe {
            nonce_c: NONCE + 1,
            flags: 0
        }],
        "probing, not confirming the old challenge"
    );
    let challenge = host.send(Payload::Challenge {
        nonce_c: NONCE + 1,
        challenge: 9,
    });
    let answered = change + Duration::from_millis(60);
    roaming.handle_datagram(answered, &challenge).unwrap();
    let migrated = host.send(Payload::Migrated { challenge: 9 });
    roaming.handle_datagram(answered, &migrated).unwrap();
    assert_eq!(
        roaming.take_events(),
        [RoamEvent::Roamed {
            elapsed: Duration::from_millis(60)
        }],
        "timed from the change"
    );
}

#[test]
fn a_new_nonce_forgets_the_confirmed_challenges() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let challenge = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(now, &challenge).unwrap();
    roaming.start(now, RoamTrigger::HostSilent, 0, NONCE + 1);
    let stale = host.send(Payload::Migrated { challenge: 5 });
    assert_eq!(
        roaming.handle_datagram(now, &stale),
        Err(Rejected::Challenge)
    );
    assert_eq!(roaming.trigger(), Some(RoamTrigger::NetworkChanged));
}

#[test]
fn cancel_reports_the_old_path_and_stops() {
    let now = Instant::now();
    let (mut roaming, _) = probing(now);
    roaming.cancel(now + Duration::from_millis(300));
    assert_eq!(
        roaming.take_events(),
        [RoamEvent::Unneeded {
            elapsed: Duration::from_millis(300)
        }]
    );
    assert_eq!(roaming.poll(now + Duration::from_secs(1)), None);
    assert!(sent(&mut roaming).is_empty());
    roaming.cancel(now);
    assert!(roaming.take_events().is_empty());
}

#[test]
fn an_exhausted_seq_fails_the_attempt() {
    let now = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, MAX_ATTEMPT);
    roaming.sent_seq.0.store(u32::MAX, Ordering::Relaxed);
    roaming.start(now, RoamTrigger::NetworkChanged, 0, NONCE);
    assert_eq!(roaming.poll(now), None);
    assert!(sent(&mut roaming).is_empty());
    assert!(matches!(
        roaming.take_events()[..],
        [RoamEvent::Started { .. }, RoamEvent::Failed { .. }]
    ));
}

#[test]
fn a_reconnect_under_the_same_key_continues_the_seqs() {
    let now = Instant::now();
    let seq = LaunchSeq::default();
    let host = FakeHost::new();
    let mut seqs = Vec::new();
    for _ in 0..2 {
        let mut roaming = Roaming::with_seq(&vectors::RIKEY, MAX_ATTEMPT, seq.clone());
        roaming.start(now, RoamTrigger::NetworkChanged, 0, NONCE);
        roaming.poll(now);
        roaming.poll(now + FAST_PROBE_INTERVAL);
        seqs.extend(sent(&mut roaming).iter().map(|wire| host.open(wire).0.seq));
    }
    assert_eq!(seqs, [1, 2, 3, 4]);
    assert_eq!(seq.last(), 4);
    // A fresh launch's seqs start over.
    let mut roaming = Roaming::new(&vectors::RIKEY, MAX_ATTEMPT);
    roaming.start(now, RoamTrigger::NetworkChanged, 0, NONCE);
    roaming.poll(now);
    assert_eq!(host.open(&sent(&mut roaming)[0]).0.seq, 1);
}

/// The payloads of `wires`, in order.
fn opened(host: &FakeHost, wires: &[[u8; DATAGRAM_BYTES]]) -> Vec<Payload> {
    wires.iter().map(|wire| host.open(wire).1).collect()
}

#[test]
fn confirms_are_resent_for_the_newest_challenge_then_probing_resumes() {
    let now = Instant::now();
    let (mut roaming, mut host) = probing(now);
    let at = |ms| now + Duration::from_millis(ms);
    let first = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 5,
    });
    roaming.handle_datagram(at(100), &first).unwrap();
    assert_eq!(
        opened(&host, &sent(&mut roaming)),
        [Payload::Confirm { challenge: 5 }]
    );
    // The next timer resends the CONFIRM, not a PROBE.
    assert_eq!(roaming.poll(at(100)), Some(at(350)));
    assert!(sent(&mut roaming).is_empty());
    assert_eq!(roaming.poll(at(350)), Some(at(600)));
    assert_eq!(
        opened(&host, &sent(&mut roaming)),
        [Payload::Confirm { challenge: 5 }]
    );
    // A newer challenge (a PROBE still in flight) is confirmed from then on.
    let newer = host.send(Payload::Challenge {
        nonce_c: NONCE,
        challenge: 8,
    });
    roaming.handle_datagram(at(400), &newer).unwrap();
    assert_eq!(
        opened(&host, &sent(&mut roaming)),
        [Payload::Confirm { challenge: 8 }]
    );
    let mut t = at(400);
    let mut resent = Vec::new();
    loop {
        let next = roaming.poll(t).unwrap();
        resent.extend(opened(&host, &sent(&mut roaming)));
        if next >= at(400) + CONFIRM_WINDOW {
            break;
        }
        t = next;
    }
    assert_eq!(resent.len(), 11, "every 250 ms for 3 s: {resent:?}");
    assert!(resent
        .iter()
        .all(|p| *p == Payload::Confirm { challenge: 8 }));
    // No MIGRATED within the window: probing resumes.
    let resumed = at(400) + CONFIRM_WINDOW;
    assert_eq!(roaming.poll(resumed), Some(resumed + FAST_PROBE_INTERVAL));
    assert_eq!(
        opened(&host, &sent(&mut roaming)),
        [Payload::Probe {
            nonce_c: NONCE,
            flags: 0
        }]
    );
    // A late MIGRATED for a confirmed challenge still completes the attempt.
    let migrated = host.send(Payload::Migrated { challenge: 5 });
    roaming.handle_datagram(resumed, &migrated).unwrap();
    assert!(matches!(
        roaming.take_events()[..],
        [RoamEvent::Roamed { .. }]
    ));
}

/// The challenge a [`SimHost`] gives a PROBE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostRule {
    /// A repeated PROBE with the pending `nonce_c` gets the pending challenge again.
    SameChallenge,
    /// Every PROBE gets a fresh challenge that replaces the pending one (the first host).
    FreshChallenge,
}

/// The host's rules for one candidate address: CHALLENGE per PROBE, MIGRATED for a
/// CONFIRM of the pending challenge, and again for one already committed.
struct SimHost {
    host: FakeHost,
    rule: HostRule,
    client_seq: u32,
    /// `nonce_c` and challenge.
    pending: Option<(u64, u64)>,
    committed: Option<u64>,
    challenges: u64,
}

impl SimHost {
    fn new(rule: HostRule) -> Self {
        Self {
            host: FakeHost::new(),
            rule,
            client_seq: 0,
            pending: None,
            committed: None,
            challenges: 0,
        }
    }

    fn receive(&mut self, wire: &[u8; DATAGRAM_BYTES]) -> Option<[u8; DATAGRAM_BYTES]> {
        let (header, payload) = self.host.open(wire);
        if header.seq <= self.client_seq {
            return None;
        }
        self.client_seq = header.seq;
        match payload {
            Payload::Probe { nonce_c, .. } => {
                let challenge = match self.pending {
                    Some((pending, challenge))
                        if self.rule == HostRule::SameChallenge && pending == nonce_c =>
                    {
                        challenge
                    }
                    _ => {
                        self.challenges += 1;
                        self.challenges
                    }
                };
                self.pending = Some((nonce_c, challenge));
                Some(self.host.send(Payload::Challenge { nonce_c, challenge }))
            }
            Payload::Confirm { challenge } => {
                if self
                    .pending
                    .is_some_and(|(_, pending)| pending == challenge)
                {
                    self.pending = None;
                    self.committed = Some(challenge);
                }
                (self.committed == Some(challenge))
                    .then(|| self.host.send(Payload::Migrated { challenge }))
            }
            other => panic!("the client sent {other:?}"),
        }
    }
}

/// Runs a network-change attempt against a [`SimHost`] over a path with round trip `rtt`,
/// on a simulated clock, until it ends. Returns how it ended and what the client sent.
fn simulate(rule: HostRule, rtt: Duration) -> (RoamEvent, Vec<Kind>) {
    let start = Instant::now();
    let mut roaming = Roaming::new(&vectors::RIKEY, MAX_ATTEMPT);
    let mut host = SimHost::new(rule);
    let one_way = rtt / 2;
    // In flight: arrival, whether toward the host, datagram.
    let mut flight: Vec<(Instant, bool, [u8; DATAGRAM_BYTES])> = Vec::new();
    let mut sent_kinds = Vec::new();
    roaming.start(start, RoamTrigger::NetworkChanged, 0, NONCE);
    let mut now = start;
    let mut timer = roaming.poll(now);
    loop {
        while let Some(wire) = roaming.poll_transmit() {
            sent_kinds.push(Datagram::decode(&wire).unwrap().header.kind);
            flight.push((now + one_way, true, wire));
        }
        let events = roaming.take_events();
        if let Some(end) = events
            .into_iter()
            .find(|e| !matches!(e, RoamEvent::Started { .. }))
        {
            return (end, sent_kinds);
        }
        let arrival = flight.iter().map(|(at, ..)| *at).min();
        now = timer
            .into_iter()
            .chain(arrival)
            .min()
            .expect("a timer runs");
        loop {
            flight.sort_by_key(|(at, ..)| *at);
            if !flight.first().is_some_and(|(at, ..)| *at <= now) {
                break;
            }
            let (_, toward_host, wire) = flight.remove(0);
            if toward_host {
                if let Some(answer) = host.receive(&wire) {
                    flight.push((now + one_way, false, answer));
                }
            } else {
                let _ = roaming.handle_datagram(now, &wire);
            }
        }
        timer = roaming.poll(now);
    }
}

fn roamed_within(end: RoamEvent, limit: Duration) {
    match end {
        RoamEvent::Roamed { elapsed } => assert!(elapsed <= limit, "{elapsed:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_400_ms_round_trip_roams_within_about_a_second() {
    let (end, sent) = simulate(HostRule::SameChallenge, Duration::from_millis(400));
    roamed_within(end, Duration::from_millis(1200));
    // Once the client confirmed, it stopped asking for challenges.
    let first_confirm = sent.iter().position(|k| *k == Kind::Confirm).unwrap();
    assert!(!sent[first_confirm..].contains(&Kind::Probe), "{sent:?}");
}

#[test]
fn a_1500_ms_round_trip_roams() {
    let (end, _) = simulate(HostRule::SameChallenge, Duration::from_millis(1500));
    // The first CHALLENGE after one round trip, MIGRATED after the second.
    roamed_within(end, Duration::from_millis(3250));
}

#[test]
fn a_host_that_challenges_every_probe_anew_still_roams() {
    // Without the CONFIRM resends the probes kept replacing the pending challenge.
    let (end, sent) = simulate(HostRule::FreshChallenge, Duration::from_millis(400));
    roamed_within(end, Duration::from_millis(1500));
    assert!(sent.contains(&Kind::Confirm));
    let (end, _) = simulate(HostRule::FreshChallenge, Duration::from_millis(1500));
    roamed_within(end, Duration::from_millis(5000));
}
