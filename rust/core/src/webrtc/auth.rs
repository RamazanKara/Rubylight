//! Pairing and authentication for WebRTC clients, as pure logic: no sockets,
//! no clock, no global RNG. The caller passes the time and the RNG in.
//!
//! A paired TV or browser holds a 128-bit `clientId` and a 256-bit secret. On
//! every connection the host sends a single-use 256-bit nonce, and the client
//! answers with `HMAC-SHA256(secret, "moonlight-webrtc-tv-auth-v1:" + nonce_hex)`.
//! Auth stops other LAN devices from using the stream; it does not make
//! anything confidential (the WebSocket is plain `ws://`).
use super::protocol::ErrorCode;
use rand::RngCore;
use ring::hmac;
use std::time::Duration;
use subtle::ConstantTimeEq;

/// Binds a proof to this protocol.
pub const PROOF_LABEL: &str = "moonlight-webrtc-tv-auth-v1:";
pub const NONCE_BYTES: usize = 32;
pub const CLIENT_ID_BYTES: usize = 16;
pub const SECRET_BYTES: usize = 32;
/// A pairing request lasts this long.
pub const PAIRING_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Connections that have not authenticated; the oldest goes first.
pub const MAX_UNAUTHENTICATED: usize = 8;
/// Paired WebRTC clients kept; the oldest goes first.
pub const MAX_PAIRED: usize = 32;
/// Pairing requests waiting for the admin; the oldest goes first.
pub const MAX_PENDING_PINS: usize = 32;

/// `n` random bytes in lower-case hex.
fn random_hex<R: RngCore + ?Sized>(rng: &mut R, bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rng.fill_bytes(&mut buffer);
    hex::encode(buffer)
}

/// Is this exactly `bytes` bytes of lower-case hex?
pub fn is_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A fresh single-use nonce, 256 bits in lower-case hex.
pub fn generate_nonce<R: RngCore + ?Sized>(rng: &mut R) -> String {
    random_hex(rng, NONCE_BYTES)
}

/// `HMAC-SHA256(secret, label + nonce_hex)` in lower-case hex. `None` when the
/// secret is not 256 bits of hex.
pub fn proof_hex(secret_hex: &str, nonce_hex: &str) -> Option<String> {
    if !is_lower_hex(secret_hex, SECRET_BYTES) {
        return None;
    }
    let secret = hex::decode(secret_hex).ok()?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, &secret);
    let mut message = Vec::with_capacity(PROOF_LABEL.len() + nonce_hex.len());
    message.extend_from_slice(PROOF_LABEL.as_bytes());
    message.extend_from_slice(nonce_hex.as_bytes());
    Some(hex::encode(hmac::sign(&key, &message).as_ref()))
}

/// Constant-time equality of two strings. Lengths are not secret.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// The credentials issued when a client pairs.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// 128 bits, lower-case hex.
    pub client_id: String,
    /// 256 bits, lower-case hex. Never logged and never in an error message.
    pub secret: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl Credentials {
    /// Draw a client ID and a secret from `rng` (the OS RNG in the host).
    pub fn generate<R: RngCore + ?Sized>(rng: &mut R) -> Self {
        Self {
            client_id: random_hex(rng, CLIENT_ID_BYTES),
            secret: random_hex(rng, SECRET_BYTES),
        }
    }
}

/// The nonce of one connection. It is spent by the first `authenticate`,
/// whether the proof is right or not, so a captured proof never works twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    nonce: Option<String>,
}

impl Challenge {
    pub fn new<R: RngCore + ?Sized>(rng: &mut R) -> Self {
        Self {
            nonce: Some(generate_nonce(rng)),
        }
    }

    /// The nonce to send in `auth-required`, until it is spent.
    pub fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    /// Spend the nonce without an attempt (the client paired instead).
    pub fn discard(&mut self) {
        self.nonce = None;
    }

    /// Check a client's proof against its secret. Spends the nonce. `secret`
    /// is `None` for an unknown client; that still takes the same path, so the
    /// nonce is spent either way.
    pub fn verify(&mut self, secret: Option<&str>, proof: &str) -> bool {
        let Some(nonce) = self.nonce.take() else {
            return false;
        };
        let Some(expected) = secret.and_then(|secret| proof_hex(secret, &nonce)) else {
            return false;
        };
        constant_time_eq(&expected, &proof.to_ascii_lowercase())
    }
}

/// Why a pairing request ended without a pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingFailure {
    Expired,
    IncorrectPin,
    Cancelled,
    /// The request was already used, or does not exist.
    Failed,
}

impl PairingFailure {
    /// The `request-pairing` error code for this failure.
    pub fn code(self) -> ErrorCode {
        match self {
            Self::Expired => ErrorCode::PairingExpired,
            Self::IncorrectPin => ErrorCode::IncorrectPin,
            Self::Cancelled => ErrorCode::PairingCancelled,
            Self::Failed => ErrorCode::PairingFailed,
        }
    }
}

/// A client waiting for the admin to enter its PIN in the console. Time is
/// a `Duration` on any monotonic clock the caller picks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPin {
    request_id: String,
    pin: String,
    client_name: String,
    created: Duration,
    attempted: bool,
}

impl PendingPin {
    /// `request_id` is 128 random bits, so the console's `uniqueid` can't be guessed.
    pub fn new<R: RngCore + ?Sized>(
        rng: &mut R,
        pin: impl Into<String>,
        client_name: impl Into<String>,
        now: Duration,
    ) -> Self {
        Self {
            request_id: random_hex(rng, 16),
            pin: pin.into(),
            client_name: client_name.into(),
            created: now,
            attempted: false,
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn client_name(&self) -> &str {
        &self.client_name
    }

    pub fn created(&self) -> Duration {
        self.created
    }

    pub fn is_expired(&self, now: Duration) -> bool {
        now.saturating_sub(self.created) >= PAIRING_LIFETIME
    }

    /// The one attempt: the PIN the admin entered against the one the TV
    /// showed, in constant time. Whatever the outcome, the request is used up.
    pub fn attempt(&mut self, entered: &str, now: Duration) -> Result<(), PairingFailure> {
        if self.attempted {
            return Err(PairingFailure::Failed);
        }
        self.attempted = true;
        if self.is_expired(now) {
            return Err(PairingFailure::Expired);
        }
        if constant_time_eq(&self.pin, entered) {
            Ok(())
        } else {
            Err(PairingFailure::IncorrectPin)
        }
    }
}

/// The requests waiting in the console. Bounded: past
/// [`MAX_PENDING_PINS`] the oldest goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPins {
    entries: Vec<PendingPin>,
}

impl PendingPins {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a request. Returns the request dropped to make room, if any.
    pub fn add(&mut self, pin: PendingPin) -> Option<PendingPin> {
        self.entries.push(pin);
        (self.entries.len() > MAX_PENDING_PINS).then(|| self.entries.remove(0))
    }

    /// Requests still waiting (an expired one is not listed).
    pub fn waiting(&self, now: Duration) -> impl Iterator<Item = &PendingPin> {
        self.entries
            .iter()
            .filter(move |p| !p.attempted && !p.is_expired(now))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop expired and used requests.
    pub fn prune(&mut self, now: Duration) {
        self.entries.retain(|p| !p.attempted && !p.is_expired(now));
    }

    /// The admin entered `pin` for `request_id`. The request is consumed
    /// either way. An unknown ID is `Failed`.
    pub fn approve(
        &mut self,
        request_id: &str,
        pin: &str,
        now: Duration,
    ) -> Result<PendingPin, PairingFailure> {
        let index = self
            .entries
            .iter()
            .position(|p| constant_time_eq(&p.request_id, request_id))
            .ok_or(PairingFailure::Failed)?;
        let mut entry = self.entries.remove(index);
        entry.attempt(pin, now)?;
        Ok(entry)
    }

    /// The admin declined, or the client went away.
    pub fn cancel(&mut self, request_id: &str) -> Option<PendingPin> {
        let index = self
            .entries
            .iter()
            .position(|p| constant_time_eq(&p.request_id, request_id))?;
        Some(self.entries.remove(index))
    }
}

/// Where a live connection is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Unauthenticated,
    Authenticated { client_id: String },
    Streaming { client_id: String },
}

impl ConnectionState {
    fn client_id(&self) -> Option<&str> {
        match self {
            Self::Unauthenticated => None,
            Self::Authenticated { client_id } | Self::Streaming { client_id } => Some(client_id),
        }
    }
}

/// A connection the table asks the caller to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub id: u64,
    /// It was streaming, so its stream has to stop too.
    pub was_streaming: bool,
}

/// Which connections exist, oldest first, and who has to go when a limit is
/// hit. The caller owns the sockets and closes what this returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionTable {
    max_unauthenticated: usize,
    entries: Vec<(u64, ConnectionState)>,
}

impl Default for ConnectionTable {
    fn default() -> Self {
        Self::new(MAX_UNAUTHENTICATED)
    }
}

impl ConnectionTable {
    pub fn new(max_unauthenticated: usize) -> Self {
        Self {
            max_unauthenticated,
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn state(&self, id: u64) -> Option<&ConnectionState> {
        self.entries.iter().find(|(i, _)| *i == id).map(|(_, s)| s)
    }

    pub fn unauthenticated(&self) -> usize {
        self.entries
            .iter()
            .filter(|(_, s)| *s == ConnectionState::Unauthenticated)
            .count()
    }

    /// A new connection, unauthenticated. If that makes more than the limit,
    /// the oldest unauthenticated connections go. Authenticated and streaming
    /// connections are never touched, so a stranger can't displace a stream.
    pub fn open(&mut self, id: u64) -> Vec<Dropped> {
        self.entries.push((id, ConnectionState::Unauthenticated));
        let mut dropped = Vec::new();
        while self.unauthenticated() > self.max_unauthenticated {
            let Some(index) = self
                .entries
                .iter()
                .position(|(_, s)| *s == ConnectionState::Unauthenticated)
            else {
                break;
            };
            dropped.push(Dropped {
                id: self.entries.remove(index).0,
                was_streaming: false,
            });
        }
        dropped
    }

    /// A new connection that is authenticated as `client_id` from the start (the console's own
    /// browser, which signed in to the console). It takes no unauthenticated slot, so it never
    /// pushes out a waiting TV; any other connection of the same client is replaced, as in
    /// [`Self::authenticate`].
    pub fn open_authenticated(&mut self, id: u64, client_id: &str) -> Vec<Dropped> {
        let mut dropped = Vec::new();
        self.entries.retain(|(other, state)| {
            let same_client = *other != id && state.client_id() == Some(client_id);
            if same_client {
                dropped.push(Dropped {
                    id: *other,
                    was_streaming: matches!(state, ConnectionState::Streaming { .. }),
                });
            }
            !same_client
        });
        self.entries.push((
            id,
            ConnectionState::Authenticated {
                client_id: client_id.to_string(),
            },
        ));
        dropped
    }

    /// The connection proved itself as `client_id`. Any other connection of
    /// the same client is replaced, and its stream, if any, stops. A
    /// connection that is already authenticated or streaming is refused with
    /// `already-authenticated` and left as it is, so a second `authenticate`
    /// can't end a running stream's state.
    pub fn authenticate(&mut self, id: u64, client_id: &str) -> Result<Vec<Dropped>, ErrorCode> {
        match self.state(id) {
            None => return Ok(Vec::new()),
            Some(ConnectionState::Unauthenticated) => {}
            Some(_) => return Err(ErrorCode::AlreadyAuthenticated),
        }
        let mut dropped = Vec::new();
        self.entries.retain(|(other, state)| {
            let same_client = *other != id && state.client_id() == Some(client_id);
            if same_client {
                dropped.push(Dropped {
                    id: *other,
                    was_streaming: matches!(state, ConnectionState::Streaming { .. }),
                });
            }
            !same_client
        });
        if let Some((_, state)) = self.entries.iter_mut().find(|(i, _)| *i == id) {
            *state = ConnectionState::Authenticated {
                client_id: client_id.to_string(),
            };
        }
        Ok(dropped)
    }

    /// A stream started or ended on an authenticated connection.
    pub fn set_streaming(&mut self, id: u64, streaming: bool) {
        if let Some((_, state)) = self.entries.iter_mut().find(|(i, _)| *i == id) {
            *state = match (state.client_id(), streaming) {
                (Some(client_id), true) => ConnectionState::Streaming {
                    client_id: client_id.to_string(),
                },
                (Some(client_id), false) => ConnectionState::Authenticated {
                    client_id: client_id.to_string(),
                },
                (None, _) => ConnectionState::Unauthenticated,
            };
        }
    }

    /// The connection closed on its own.
    pub fn close(&mut self, id: u64) {
        self.entries.retain(|(i, _)| *i != id);
    }

    /// Every connection of a client (it was disabled or unpaired).
    pub fn drop_client(&mut self, client_id: &str) -> Vec<Dropped> {
        let mut dropped = Vec::new();
        self.entries.retain(|(id, state)| {
            let hit = state.client_id() == Some(client_id);
            if hit {
                dropped.push(Dropped {
                    id: *id,
                    was_streaming: matches!(state, ConnectionState::Streaming { .. }),
                });
            }
            !hit
        });
        dropped
    }
}

/// The paired WebRTC clients, oldest first. At most [`MAX_PAIRED`] are kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairedRoster {
    /// `(client_id, order)` where a smaller order is older.
    entries: Vec<(String, u64)>,
}

impl PairedRoster {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the stored clients as `(client_id, paired_at)`; the oldest `paired_at`
    /// is dropped first. Over the limit, the surplus is returned.
    pub fn from_entries<I: IntoIterator<Item = (String, u64)>>(entries: I) -> (Self, Vec<String>) {
        let mut roster = Self {
            entries: entries.into_iter().collect(),
        };
        roster.entries.sort_by_key(|(_, at)| *at);
        let surplus = roster.trim();
        (roster, surplus)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, client_id: &str) -> bool {
        self.entries.iter().any(|(id, _)| id == client_id)
    }

    /// A client paired at `paired_at` (any increasing number). Returns the
    /// clients dropped because the roster was full: their credentials must go.
    pub fn insert(&mut self, client_id: &str, paired_at: u64) -> Vec<String> {
        self.entries.retain(|(id, _)| id != client_id);
        self.entries.push((client_id.to_string(), paired_at));
        self.entries.sort_by_key(|(_, at)| *at);
        self.trim()
    }

    pub fn remove(&mut self, client_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(id, _)| id != client_id);
        self.entries.len() != before
    }

    fn trim(&mut self) -> Vec<String> {
        let surplus = self.entries.len().saturating_sub(MAX_PAIRED);
        self.entries.drain(..surplus).map(|(id, _)| id).collect()
    }
}

#[cfg(test)]
mod tests;
