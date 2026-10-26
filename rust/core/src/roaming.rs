//! Client roaming on the host: the path validation state machine of wire format v1
//! (`rubylight_protocol::roaming`, Nightjar's `docs/protocol/client-roaming.md`).
//!
//! A client whose address changed sends PROBE from the new address; the host answers
//! CHALLENGE to that address, and only a CONFIRM from exactly that address with the
//! challenge moves the session (QUIC-style return routability). [`RoamTable`] decides;
//! it holds no sockets and moves nothing itself. The stream code feeds it every control
//! datagram that starts with the magic, sends the answer it returns and carries out a
//! [`Action::Commit`]. Every answer is one datagram of the request's size, sent only for
//! an authenticated, fresh, rate-limited request, so the host can't be used to amplify.
use aes::{
    Aes128,
    cipher::{Block, BlockEncrypt, KeyInit},
};
use aes_gcm::{Aes128Gcm, Nonce, Tag, aead::AeadInPlace};
use rubylight_protocol::roaming::{
    DATAGRAM_BYTES, Datagram, DecodeError, Header, KEY_BLOCK, Kind, PAYLOAD_BYTES,
    PROBE_FLAG_MEDIA_REBOUND, Payload, SESSION_TAG_BYTES, TAG_BLOCK,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub use rubylight_protocol::roaming::SDP_ATTRIBUTE;

/// PROBEs and CONFIRMs handled per session and second; the client sends at most four of
/// each.
pub const SESSION_PER_SECOND: u32 = 10;
/// Datagrams carrying one session's tag opened per second, before anything is
/// authenticated. Junk under one tag uses up only that session's budget.
pub const PREAUTH_PER_SECOND: u32 = 50;
/// Datagrams with a known session tag opened per second over all sessions: a bound on
/// the GCM work, well below what one core can open.
pub const GLOBAL_PER_SECOND: u32 = 5_000;
/// How long a probing address may take to confirm.
pub const PENDING_LIFETIME: Duration = Duration::from_secs(3);
/// The shortest time between two moves of one session.
pub const COMMIT_INTERVAL: Duration = Duration::from_secs(1);
/// How many removed launches keep their seqs for a later insert.
const RETIRED: usize = 64;

/// The keys one launch's `rikey` derives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoamKeys {
    /// AES-128-ECB(rikey, [`KEY_BLOCK`]): seals and opens the datagrams.
    pub key: [u8; 16],
    /// AES-128-ECB(rikey, [`TAG_BLOCK`])[0..8]: finds the session.
    pub session_tag: [u8; SESSION_TAG_BYTES],
}

impl RoamKeys {
    pub fn derive(rikey: &[u8; 16]) -> Self {
        let aes = Aes128::new(rikey.into());
        let ecb = |input: [u8; 16]| {
            let mut block = Block::<Aes128>::from(input);
            aes.encrypt_block(&mut block);
            <[u8; 16]>::from(block)
        };
        let mut session_tag = [0; SESSION_TAG_BYTES];
        session_tag.copy_from_slice(&ecb(TAG_BLOCK)[..SESSION_TAG_BYTES]);
        Self {
            key: ecb(KEY_BLOCK),
            session_tag,
        }
    }
}

/// Seals `payload` under `header` (whose kind must match) into wire form.
pub fn seal(key: &[u8; 16], header: Header, payload: Payload) -> [u8; DATAGRAM_BYTES] {
    debug_assert_eq!(header.kind, payload.kind());
    let mut ciphertext = payload.encode();
    let gcm_tag = Aes128Gcm::new(key.into())
        .encrypt_in_place_detached(
            Nonce::from_slice(&header.nonce()),
            &header.aad(),
            &mut ciphertext,
        )
        .expect("16 bytes are within AES-GCM's limits");
    Datagram {
        header,
        gcm_tag: gcm_tag.into(),
        ciphertext,
    }
    .encode()
}

/// Opens a decoded datagram; `None` when the GCM tag fails or the padding is not zero.
pub fn open(key: &[u8; 16], datagram: &Datagram) -> Option<Payload> {
    let mut plaintext: [u8; PAYLOAD_BYTES] = datagram.ciphertext;
    Aes128Gcm::new(key.into())
        .decrypt_in_place_detached(
            Nonce::from_slice(&datagram.header.nonce()),
            &datagram.header.aad(),
            &mut plaintext,
            Tag::from_slice(&datagram.gcm_tag),
        )
        .ok()?;
    Payload::decode(datagram.header.kind, &plaintext)
}

/// Why a datagram was dropped. A dropped datagram is never answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The clear header is malformed.
    Decode(DecodeError),
    /// CHALLENGE or MIGRATED, which only a host sends.
    Direction,
    /// No session has this tag.
    UnknownSession,
    /// Over [`GLOBAL_PER_SECOND`].
    GlobalRate,
    /// The session is stopping.
    Stopping,
    /// The GCM tag does not verify or the padding is not zero.
    Authentication,
    /// Seq not above the last one seen from the client.
    Replay,
    /// Over [`SESSION_PER_SECOND`], or over [`PREAUTH_PER_SECOND`] before authentication.
    SessionRate,
    /// CONFIRM without a pending address, or after it expired.
    NoPending,
    /// CONFIRM from another address than the pending one.
    Address,
    /// CONFIRM with another challenge than the pending one.
    Challenge,
    /// The session moved less than [`COMMIT_INTERVAL`] ago.
    CommitRate,
    /// The host's seq would wrap; it never reuses a nonce.
    SeqExhausted,
}

/// What the caller does with an accepted datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action<K> {
    /// Send `datagram` to `to`. Nothing moves.
    Reply {
        to: SocketAddr,
        datagram: [u8; DATAGRAM_BYTES],
    },
    /// Move session `id` from `old` to `new`, then send `datagram` (MIGRATED) to `new`.
    /// `media_rebound` is the PROBE's flag: expect pings from new ports.
    Commit {
        id: K,
        old: SocketAddr,
        new: SocketAddr,
        media_rebound: bool,
        datagram: [u8; DATAGRAM_BYTES],
    },
}

/// Requests counted in one-second windows.
#[derive(Clone, Copy, Debug)]
struct Window {
    start: Option<Instant>,
    count: u32,
}

impl Window {
    const fn new() -> Self {
        Self {
            start: None,
            count: 0,
        }
    }
    fn admit(&mut self, now: Instant, limit: u32) -> bool {
        if self
            .start
            .is_none_or(|start| now.saturating_duration_since(start) >= Duration::from_secs(1))
        {
            *self = Self {
                start: Some(now),
                count: 0,
            };
        }
        if self.count >= limit {
            return false;
        }
        self.count += 1;
        true
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    address: SocketAddr,
    /// The client's nonce of the first probe from `address`.
    nonce_c: u64,
    challenge: u64,
    media_rebound: bool,
    expires: Instant,
}

struct Entry<K> {
    id: K,
    keys: RoamKeys,
    current: SocketAddr,
    stopping: bool,
    /// The last seq accepted from the client.
    client_seq: u32,
    /// The last seq the host sent.
    host_seq: u32,
    pending: Option<Pending>,
    /// The last probe from `current` itself, kept apart so it never replaces `pending`.
    here: Option<Pending>,
    /// The challenge of the last move, for answering a repeated CONFIRM.
    committed: Option<u64>,
    last_commit: Option<Instant>,
    /// Datagrams with this tag, before the GCM open.
    preauth: Window,
    /// Fresh authentic requests.
    window: Window,
}

impl<K> Entry<K> {
    fn answer(&mut self, payload: Payload) -> Result<[u8; DATAGRAM_BYTES], Refusal> {
        let seq = self.host_seq.checked_add(1).ok_or(Refusal::SeqExhausted)?;
        self.host_seq = seq;
        let header = Header {
            kind: payload.kind(),
            session_tag: self.keys.session_tag,
            seq,
        };
        Ok(seal(&self.keys.key, header, payload))
    }
}

/// What a removed entry leaves behind so its launch key never repeats a seq.
struct Retired {
    keys: RoamKeys,
    stopping: bool,
    client_seq: u32,
    host_seq: u32,
}

/// The roaming state of every session with an ENet peer, keyed by the caller's session id.
pub struct RoamTable<K> {
    entries: Vec<Entry<K>>,
    /// The newest [`RETIRED`] removed entries.
    retired: VecDeque<Retired>,
    window: Window,
    challenge: Box<dyn FnMut() -> u64 + Send>,
}

impl<K: Clone + PartialEq> Default for RoamTable<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + PartialEq> RoamTable<K> {
    pub fn new() -> Self {
        Self::with_challenges(|| u64::from_le_bytes(crate::crypto::random()))
    }

    /// A table whose challenges come from `challenge` instead of the OS's random numbers.
    pub fn with_challenges(challenge: impl FnMut() -> u64 + Send + 'static) -> Self {
        Self {
            entries: Vec::new(),
            retired: VecDeque::new(),
            window: Window::new(),
            challenge: Box::new(challenge),
        }
    }

    /// Accepts roaming for session `id`, whose ENet peer is at `current`. Inserting an id
    /// again (an ENet reconnect) updates its address and keeps both seqs, so neither side
    /// reuses a nonce under the launch's key and old datagrams stay replays. That holds
    /// after a [`remove`](Self::remove) too. A stopping session stays stopping.
    pub fn insert(&mut self, id: K, rikey: &[u8; 16], current: SocketAddr) {
        let keys = RoamKeys::derive(rikey);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.keys == keys)
        {
            entry.current = current;
            entry.pending = None;
            entry.here = None;
            return;
        }
        self.remove(&id);
        let retired = self
            .retired
            .iter()
            .position(|retired| retired.keys == keys)
            .and_then(|at| self.retired.remove(at));
        self.entries.push(Entry {
            id,
            keys,
            current,
            stopping: retired.as_ref().is_some_and(|retired| retired.stopping),
            client_seq: retired.as_ref().map_or(0, |retired| retired.client_seq),
            host_seq: retired.as_ref().map_or(0, |retired| retired.host_seq),
            pending: None,
            here: None,
            committed: None,
            last_commit: None,
            preauth: Window::new(),
            window: Window::new(),
        });
    }

    /// From now on the session refuses every datagram.
    pub fn set_stopping(&mut self, id: &K) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == *id) {
            entry.stopping = true;
            entry.pending = None;
            entry.here = None;
        }
    }

    /// Drops the session's roaming (its ENet peer is gone, or the launch ended). The
    /// launch key's seqs are kept for the newest [`RETIRED`] removals, so a peer that
    /// reconnects under the same launch neither repeats a nonce nor accepts a replay.
    pub fn remove(&mut self, id: &K) {
        let Some(at) = self.entries.iter().position(|entry| entry.id == *id) else {
            return;
        };
        let entry = self.entries.swap_remove(at);
        if self.retired.len() == RETIRED {
            self.retired.pop_front();
        }
        self.retired.push_back(Retired {
            keys: entry.keys,
            stopping: entry.stopping,
            client_seq: entry.client_seq,
            host_seq: entry.host_seq,
        });
    }

    /// The address the session's client was last validated at.
    pub fn current(&self, id: &K) -> Option<SocketAddr> {
        self.entries
            .iter()
            .find(|entry| entry.id == *id)
            .map(|entry| entry.current)
    }

    /// Handles one datagram that starts with the roaming magic, received from `from`.
    pub fn on_datagram(
        &mut self,
        now: Instant,
        from: SocketAddr,
        data: &[u8],
    ) -> Result<Action<K>, Refusal> {
        let datagram = Datagram::decode(data).map_err(Refusal::Decode)?;
        if !matches!(datagram.header.kind, Kind::Probe | Kind::Confirm) {
            return Err(Refusal::Direction);
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.keys.session_tag == datagram.header.session_tag)
            .ok_or(Refusal::UnknownSession)?;
        // Bound the GCM opens before anything is authenticated: per session first, so
        // junk under one tag can't use up the overall budget for the others.
        if !entry.preauth.admit(now, PREAUTH_PER_SECOND) {
            return Err(Refusal::SessionRate);
        }
        if !self.window.admit(now, GLOBAL_PER_SECOND) {
            return Err(Refusal::GlobalRate);
        }
        if entry.stopping {
            return Err(Refusal::Stopping);
        }
        let payload = open(&entry.keys.key, &datagram).ok_or(Refusal::Authentication)?;
        if datagram.header.seq <= entry.client_seq {
            return Err(Refusal::Replay);
        }
        entry.client_seq = datagram.header.seq;
        // Counted only once fresh and authentic, so a replay can't use up the client's share.
        if !entry.window.admit(now, SESSION_PER_SECOND) {
            return Err(Refusal::SessionRate);
        }
        match payload {
            Payload::Probe { nonce_c, flags } => {
                Self::on_probe(entry, now, from, nonce_c, flags, &mut *self.challenge)
            }
            Payload::Confirm { challenge } => Self::on_confirm(entry, now, from, challenge),
            Payload::Challenge { .. } | Payload::Migrated { .. } => Err(Refusal::Direction),
        }
    }

    /// A newer probe replaces the pending address. A probe from the current address is
    /// answered too, so a client that only suspected a move hears back, but it is kept
    /// apart: it neither replaces a pending address elsewhere nor moves anything.
    ///
    /// A probe repeated from the same address with the same `nonce_c` hears the same
    /// challenge again: the client probes every 250 ms, so over a longer round trip its
    /// CONFIRM answers an earlier probe's CHALLENGE. Its expiry stays
    /// [`PENDING_LIFETIME`] after the first probe, however often it repeats.
    fn on_probe(
        entry: &mut Entry<K>,
        now: Instant,
        from: SocketAddr,
        nonce_c: u64,
        flags: u32,
        challenge: &mut dyn FnMut() -> u64,
    ) -> Result<Action<K>, Refusal> {
        let media_rebound = flags & PROBE_FLAG_MEDIA_REBOUND != 0;
        let here = from == entry.current;
        let slot = if here { entry.here } else { entry.pending };
        let repeat = slot
            .filter(|pending| {
                now < pending.expires && pending.address == from && pending.nonce_c == nonce_c
            })
            .map(|pending| pending.challenge);
        let challenge = repeat.unwrap_or_else(challenge);
        let datagram = entry.answer(Payload::Challenge { nonce_c, challenge })?;
        let slot = if here {
            &mut entry.here
        } else {
            &mut entry.pending
        };
        match slot {
            Some(pending) if repeat.is_some() => pending.media_rebound |= media_rebound,
            _ => {
                *slot = Some(Pending {
                    address: from,
                    nonce_c,
                    challenge,
                    media_rebound,
                    expires: now + PENDING_LIFETIME,
                })
            }
        }
        Ok(Action::Reply { to: from, datagram })
    }

    fn on_confirm(
        entry: &mut Entry<K>,
        now: Instant,
        from: SocketAddr,
        challenge: u64,
    ) -> Result<Action<K>, Refusal> {
        if from == entry.current {
            // A CONFIRM repeated after the move (its MIGRATED was lost) hears MIGRATED
            // again; one answering a probe from the current address hears it too.
            if entry.committed != Some(challenge) {
                let here = entry
                    .here
                    .filter(|here| now < here.expires)
                    .ok_or(Refusal::NoPending)?;
                if challenge != here.challenge {
                    return Err(Refusal::Challenge);
                }
                entry.here = None;
            }
            let datagram = entry.answer(Payload::Migrated { challenge })?;
            return Ok(Action::Reply { to: from, datagram });
        }
        let pending = entry
            .pending
            .filter(|pending| now < pending.expires)
            .ok_or(Refusal::NoPending)?;
        if from != pending.address {
            return Err(Refusal::Address);
        }
        if challenge != pending.challenge {
            return Err(Refusal::Challenge);
        }
        if entry
            .last_commit
            .is_some_and(|last| now.saturating_duration_since(last) < COMMIT_INTERVAL)
        {
            return Err(Refusal::CommitRate);
        }
        let datagram = entry.answer(Payload::Migrated { challenge })?;
        let old = entry.current;
        entry.current = from;
        entry.pending = None;
        entry.here = None;
        entry.committed = Some(challenge);
        entry.last_commit = Some(now);
        Ok(Action::Commit {
            id: entry.id.clone(),
            old,
            new: from,
            media_rebound: pending.media_rebound,
            datagram,
        })
    }
}

#[cfg(test)]
mod tests;
