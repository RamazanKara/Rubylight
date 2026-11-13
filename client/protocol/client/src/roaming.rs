//! Client roaming without sockets: the client's half of the path validation in
//! [`rubylight_protocol::roaming`] (`docs/protocol/client-roaming.md`).
//!
//! [`Roaming`] is driven by triggers ([`Roaming::start`]), the roaming datagrams the control
//! socket set aside ([`Roaming::handle_datagram`]) and its timer ([`Roaming::poll`]). It
//! queues datagrams for the host's control port ([`Roaming::poll_transmit`]) and reports
//! [`RoamEvent`]s ([`Roaming::take_events`]). The control driver runs it only when the host
//! advertised roaming and the client's setting is on.
//!
//! An attempt sends PROBE every 250 ms twelve times, then once a second; answers each
//! CHALLENGE that echoes its `nonce_c` with CONFIRM; and ends on a MIGRATED that names a
//! challenge it confirmed. Once it confirmed, it resends the CONFIRM for the newest challenge
//! every 250 ms instead of probing, so a round trip longer than the probe interval does not
//! keep asking the host for new challenges; after 3 s without a MIGRATED it probes again.
//! Without a MIGRATED it gives up after the drop tolerance (at most 10 s) and leaves the
//! stream to its usual paths: the control peer's timeout ends it, or bounded recovery
//! reconnects.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::aes::cipher::BlockEncrypt;
use aes_gcm::aes::Aes128;
use aes_gcm::Aes128Gcm;
use rubylight_protocol::roaming::{
    Datagram, DecodeError, Direction, Header, Payload, DATAGRAM_BYTES, KEY_BLOCK,
    SESSION_TAG_BYTES, TAG_BLOCK,
};

use crate::crypto::Key;

/// PROBE interval at the start of an attempt.
pub const FAST_PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// PROBEs sent at [`FAST_PROBE_INTERVAL`] before [`SLOW_PROBE_INTERVAL`].
pub const FAST_PROBES: u32 = 12;
/// PROBE interval once the fast ones went unanswered.
pub const SLOW_PROBE_INTERVAL: Duration = Duration::from_secs(1);
/// The longest an attempt runs.
pub const MAX_ATTEMPT: Duration = Duration::from_secs(10);
/// How long CONFIRM is resent for the newest challenge, at [`FAST_PROBE_INTERVAL`], before
/// the attempt probes again; the host's pending candidate lasts as long.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(3);
/// Confirmed challenges an attempt remembers for the MIGRATED.
const MAX_CONFIRMED: usize = 8;
/// Datagrams queued for the socket at most; an attempt sends one per timer or CHALLENGE.
const MAX_QUEUED: usize = 8;

/// What started an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoamTrigger {
    /// The platform or the route toward the host reported a new network.
    NetworkChanged,
    /// The host has been silent for a second (a NAT rebinding).
    HostSilent,
}

/// An attempt's progress, for the UI and the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoamEvent {
    Started {
        trigger: RoamTrigger,
    },
    /// The host moved the session to the client's current address.
    Roamed {
        elapsed: Duration,
    },
    /// No MIGRATED in time; the stream's timeout or recovery takes over.
    Failed {
        elapsed: Duration,
        /// The roaming datagrams the attempt dropped, by reason.
        rejected: Rejections,
    },
    /// The host was heard again on the old path before it confirmed a move.
    Unneeded {
        elapsed: Duration,
    },
    /// The client may not roam onto its network (a metered one without the opt-in): a running
    /// attempt ended and none starts until the hold lifts. Not a failure.
    Held,
}

impl fmt::Display for RoamEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started {
                trigger: RoamTrigger::NetworkChanged,
            } => f.write_str("Roaming: the network changed, moving the stream"),
            Self::Started {
                trigger: RoamTrigger::HostSilent,
            } => f.write_str("Roaming: the PC went silent, checking the path"),
            Self::Roamed { elapsed } => {
                write!(f, "Roaming: moved the stream in {} ms", elapsed.as_millis())
            }
            Self::Failed { elapsed, rejected } => {
                write!(
                    f,
                    "Roaming: the PC did not confirm a move within {} ms",
                    elapsed.as_millis()
                )?;
                if *rejected != Rejections::default() {
                    write!(f, "; dropped {rejected}")?;
                }
                Ok(())
            }
            Self::Unneeded { elapsed } => write!(
                f,
                "Roaming: the PC answered on the old path after {} ms",
                elapsed.as_millis()
            ),
            Self::Held => f.write_str(
                "Roaming: held on a metered network without \"Continue on mobile data\"",
            ),
        }
    }
}

/// Why a roaming datagram was dropped. None is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    Decode(DecodeError),
    /// A client-to-host type.
    Direction,
    /// Another session's tag.
    Session,
    /// Seq not above the host's last.
    Replay,
    /// The GCM tag did not verify, or the payload's padding was not zero.
    Authentication,
    /// No attempt is running.
    Idle,
    /// A CHALLENGE for another probe burst.
    Nonce,
    /// A MIGRATED for a challenge this attempt did not confirm.
    Challenge,
}

/// The datagrams an attempt dropped, by [`Rejected`] reason, so a failed attempt's log line
/// tells a host that never answered from answers that did not fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rejections {
    pub decode: u32,
    pub direction: u32,
    pub session: u32,
    pub replay: u32,
    pub authentication: u32,
    pub nonce: u32,
    pub challenge: u32,
}

impl Rejections {
    fn count(&mut self, rejected: Rejected) {
        let count = match rejected {
            Rejected::Decode(_) => &mut self.decode,
            Rejected::Direction => &mut self.direction,
            Rejected::Session => &mut self.session,
            Rejected::Replay => &mut self.replay,
            Rejected::Authentication => &mut self.authentication,
            Rejected::Nonce => &mut self.nonce,
            Rejected::Challenge => &mut self.challenge,
            // Only counted while an attempt runs.
            Rejected::Idle => return,
        };
        *count = count.saturating_add(1);
    }
}

impl fmt::Display for Rejections {
    /// The non-zero counts, such as `2 replayed, 1 for another nonce`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let counts = [
            (self.decode, "malformed"),
            (self.direction, "in the wrong direction"),
            (self.session, "for another session"),
            (self.replay, "replayed"),
            (self.authentication, "unauthenticated"),
            (self.nonce, "for another nonce"),
            (self.challenge, "for an unconfirmed challenge"),
        ];
        let mut first = true;
        for (count, reason) in counts {
            if count > 0 {
                write!(f, "{}{count} {reason}", if first { "" } else { ", " })?;
                first = false;
            }
        }
        Ok(())
    }
}

/// The roaming key and session tag of a launch's `rikey`.
pub fn derive(rikey: &Key) -> (Key, [u8; SESSION_TAG_BYTES]) {
    let cipher = Aes128::new(GenericArray::from_slice(rikey));
    let block = |input: &[u8; 16]| {
        let mut block = GenericArray::clone_from_slice(input);
        cipher.encrypt_block(&mut block);
        <[u8; 16]>::from(block)
    };
    let mut tag = [0; SESSION_TAG_BYTES];
    tag.copy_from_slice(&block(&TAG_BLOCK)[..SESSION_TAG_BYTES]);
    (block(&KEY_BLOCK), tag)
}

/// Seals `payload` under `header`, whose kind must match.
pub fn seal(gcm: &Aes128Gcm, header: Header, payload: Payload) -> [u8; DATAGRAM_BYTES] {
    debug_assert_eq!(header.kind, payload.kind());
    let mut ciphertext = payload.encode();
    let gcm_tag = gcm
        .encrypt_in_place_detached(
            GenericArray::from_slice(&header.nonce()),
            &header.aad(),
            &mut ciphertext,
        )
        .expect("a 16-byte payload always seals");
    Datagram {
        header,
        gcm_tag: gcm_tag.into(),
        ciphertext,
    }
    .encode()
}

/// Opens a datagram already decoded.
pub fn open(gcm: &Aes128Gcm, datagram: &Datagram) -> Option<Payload> {
    let mut plaintext = datagram.ciphertext;
    gcm.decrypt_in_place_detached(
        GenericArray::from_slice(&datagram.header.nonce()),
        &datagram.header.aad(),
        &mut plaintext,
        GenericArray::from_slice(&datagram.gcm_tag),
    )
    .ok()?;
    Payload::decode(datagram.header.kind, &plaintext)
}

struct Attempt {
    trigger: RoamTrigger,
    started: Instant,
    nonce_c: u64,
    flags: u32,
    probes: u32,
    /// When the next PROBE, or CONFIRM while confirming, is due.
    next_send: Instant,
    /// Challenges answered with CONFIRM, oldest first.
    confirmed: VecDeque<u64>,
    /// The newest challenge, resent in CONFIRM instead of probing.
    confirming: Option<Confirming>,
    rejected: Rejections,
}

#[derive(Clone, Copy)]
struct Confirming {
    challenge: u64,
    /// The first CONFIRM for it; probing resumes [`CONFIRM_WINDOW`] later.
    since: Instant,
}

/// The last seq a client sent under one launch key. The host keeps the last seq per sender
/// per launch key, so a reconnect under the same `rikey` carries it on; a fresh one starts
/// at 0.
#[derive(Clone, Debug, Default)]
pub struct LaunchSeq(Arc<AtomicU32>);

impl LaunchSeq {
    /// The last seq sent, 0 before the first.
    pub fn last(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Takes the next seq; None once exhausted.
    fn next(&self) -> Option<u32> {
        // A compare-exchange loop: fetch_update is deprecated on new toolchains, and its
        // replacement try_update is newer than the MSRV.
        let mut seq = self.0.load(Ordering::Relaxed);
        loop {
            let next = seq.checked_add(1)?;
            match self
                .0
                .compare_exchange_weak(seq, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Some(next),
                Err(current) => seq = current,
            }
        }
    }
}

/// The client's roaming state for one control connection.
pub struct Roaming {
    gcm: Aes128Gcm,
    session_tag: [u8; SESSION_TAG_BYTES],
    give_up: Duration,
    /// The last seq this client sent under the launch key.
    sent_seq: LaunchSeq,
    /// The last seq accepted from the host.
    host_seq: u32,
    attempt: Option<Attempt>,
    outgoing: VecDeque<[u8; DATAGRAM_BYTES]>,
    events: Vec<RoamEvent>,
}

impl Roaming {
    /// Attempts give up after `give_up`, at most [`MAX_ATTEMPT`].
    pub fn new(rikey: &Key, give_up: Duration) -> Self {
        Self::with_seq(rikey, give_up, LaunchSeq::default())
    }

    /// [`Roaming::new`] continuing the seqs an earlier connection under `rikey` sent.
    pub fn with_seq(rikey: &Key, give_up: Duration, sent_seq: LaunchSeq) -> Self {
        let (key, session_tag) = derive(rikey);
        Self {
            gcm: Aes128Gcm::new(GenericArray::from_slice(&key)),
            session_tag,
            give_up: give_up.min(MAX_ATTEMPT),
            sent_seq,
            host_seq: 0,
            attempt: None,
            outgoing: VecDeque::new(),
            events: Vec::new(),
        }
    }

    /// What started the running attempt.
    pub fn trigger(&self) -> Option<RoamTrigger> {
        self.attempt.as_ref().map(|attempt| attempt.trigger)
    }

    /// Starts an attempt, or restarts the running one's fast probes under a fresh `nonce_c`.
    /// A network change restarts it as a new attempt on the new network: its trigger, start
    /// and give-up time are the change's, and [`RoamEvent::Started`] is reported again.
    /// Challenges confirmed under the old `nonce_c` no longer count. `flags` are PROBE flags;
    /// the next [`Roaming::poll`] sends the first PROBE.
    pub fn start(&mut self, now: Instant, trigger: RoamTrigger, flags: u32, nonce_c: u64) {
        if let Some(attempt) = &mut self.attempt {
            if attempt.nonce_c != nonce_c {
                attempt.confirmed.clear();
            }
            attempt.nonce_c = nonce_c;
            attempt.flags |= flags;
            attempt.probes = 0;
            attempt.next_send = now;
            attempt.confirming = None;
            if trigger == RoamTrigger::NetworkChanged {
                // A straggler from the old path must not cancel it as a silence's attempt.
                attempt.trigger = trigger;
                attempt.started = now;
                attempt.rejected = Rejections::default();
                self.events.push(RoamEvent::Started { trigger });
            }
            return;
        }
        self.attempt = Some(Attempt {
            trigger,
            started: now,
            nonce_c,
            flags,
            probes: 0,
            next_send: now,
            confirmed: VecDeque::new(),
            confirming: None,
            rejected: Rejections::default(),
        });
        self.events.push(RoamEvent::Started { trigger });
    }

    /// Ends the running attempt: the host was heard on the old path.
    pub fn cancel(&mut self, now: Instant) {
        if let Some(attempt) = self.attempt.take() {
            let elapsed = now.saturating_duration_since(attempt.started);
            self.events.push(RoamEvent::Unneeded { elapsed });
        }
    }

    /// Ends the running attempt as failed: the client gave up on roaming, such as onto a
    /// metered network it may not use.
    pub fn give_up(&mut self, now: Instant) {
        self.fail(now);
    }

    /// Holds roaming: the client may not roam onto its network. Ends the running attempt
    /// without a failure and reports [`RoamEvent::Held`].
    pub fn hold(&mut self) {
        self.attempt = None;
        self.outgoing.clear();
        self.events.push(RoamEvent::Held);
    }

    /// Sends due PROBEs or CONFIRMs and gives up when the time is over. Returns the next
    /// deadline.
    pub fn poll(&mut self, now: Instant) -> Option<Instant> {
        let attempt = self.attempt.as_mut()?;
        let give_up_at = attempt.started + self.give_up;
        if now >= give_up_at {
            return self.fail(now);
        }
        if attempt
            .confirming
            .is_some_and(|confirming| now >= confirming.since + CONFIRM_WINDOW)
        {
            // No MIGRATED for the newest challenge: probe for a fresh one.
            attempt.confirming = None;
            attempt.next_send = now;
        }
        if now >= attempt.next_send {
            let payload = match attempt.confirming {
                Some(Confirming { challenge, .. }) => Payload::Confirm { challenge },
                None => Payload::Probe {
                    nonce_c: attempt.nonce_c,
                    flags: attempt.flags,
                },
            };
            if !self.send(payload) {
                return self.fail(now);
            }
            let attempt = self.attempt.as_mut().expect("running");
            attempt.next_send = now
                + if attempt.confirming.is_some() {
                    FAST_PROBE_INTERVAL
                } else {
                    attempt.probes += 1;
                    if attempt.probes < FAST_PROBES {
                        FAST_PROBE_INTERVAL
                    } else {
                        SLOW_PROBE_INTERVAL
                    }
                };
        }
        let attempt = self.attempt.as_ref()?;
        let mut next = attempt.next_send.min(give_up_at);
        if let Some(confirming) = attempt.confirming {
            next = next.min(confirming.since + CONFIRM_WINDOW);
        }
        Some(next)
    }

    /// Handles a datagram that starts with the roaming magic.
    pub fn handle_datagram(&mut self, now: Instant, data: &[u8]) -> Result<(), Rejected> {
        let result = self.handle(now, data);
        if let (Err(rejected), Some(attempt)) = (result, self.attempt.as_mut()) {
            attempt.rejected.count(rejected);
        }
        result
    }

    fn handle(&mut self, now: Instant, data: &[u8]) -> Result<(), Rejected> {
        let datagram = Datagram::decode(data).map_err(Rejected::Decode)?;
        let header = datagram.header;
        if header.kind.direction() != Direction::HostToClient {
            return Err(Rejected::Direction);
        }
        if header.session_tag != self.session_tag {
            return Err(Rejected::Session);
        }
        if header.seq <= self.host_seq {
            return Err(Rejected::Replay);
        }
        let payload = open(&self.gcm, &datagram).ok_or(Rejected::Authentication)?;
        self.host_seq = header.seq;
        let attempt = self.attempt.as_mut().ok_or(Rejected::Idle)?;
        match payload {
            Payload::Challenge { nonce_c, challenge } => {
                if nonce_c != attempt.nonce_c {
                    return Err(Rejected::Nonce);
                }
                // A host that answers each PROBE with the same challenge sends it many times.
                if !attempt.confirmed.contains(&challenge) {
                    if attempt.confirmed.len() == MAX_CONFIRMED {
                        attempt.confirmed.pop_front();
                    }
                    attempt.confirmed.push_back(challenge);
                }
                // The newest challenge is the one the host holds pending: confirm it from now
                // on, and stop probing so the host keeps it.
                if attempt.confirming.map(|c| c.challenge) != Some(challenge) {
                    attempt.confirming = Some(Confirming {
                        challenge,
                        since: now,
                    });
                }
                attempt.next_send = now + FAST_PROBE_INTERVAL;
                if !self.send(Payload::Confirm { challenge }) {
                    self.fail(now);
                }
                Ok(())
            }
            Payload::Migrated { challenge } => {
                if !attempt.confirmed.contains(&challenge) {
                    return Err(Rejected::Challenge);
                }
                let elapsed = now.saturating_duration_since(attempt.started);
                self.attempt = None;
                self.events.push(RoamEvent::Roamed { elapsed });
                Ok(())
            }
            Payload::Probe { .. } | Payload::Confirm { .. } => Err(Rejected::Direction),
        }
    }

    /// The next datagram for the host's control port.
    pub fn poll_transmit(&mut self) -> Option<[u8; DATAGRAM_BYTES]> {
        self.outgoing.pop_front()
    }

    pub fn take_events(&mut self) -> Vec<RoamEvent> {
        std::mem::take(&mut self.events)
    }

    /// Seals `payload` under the next seq; false once seq is exhausted.
    fn send(&mut self, payload: Payload) -> bool {
        let Some(seq) = self.sent_seq.next() else {
            return false;
        };
        let header = Header {
            kind: payload.kind(),
            session_tag: self.session_tag,
            seq,
        };
        if self.outgoing.len() == MAX_QUEUED {
            self.outgoing.pop_front();
        }
        self.outgoing.push_back(seal(&self.gcm, header, payload));
        true
    }

    fn fail(&mut self, now: Instant) -> Option<Instant> {
        if let Some(attempt) = self.attempt.take() {
            let elapsed = now.saturating_duration_since(attempt.started);
            self.events.push(RoamEvent::Failed {
                elapsed,
                rejected: attempt.rejected,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests;
