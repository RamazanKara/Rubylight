//! Datagrams on a punched path. Every one is sealed with ChaCha20-Poly1305 under a key per
//! direction ([`crate::record::PathKeys`]), so only the paired device and its host can speak on
//! the path, and nothing on the way reads it.
//!
//! | Off | Len | Field |
//! |---|---|---|
//! | 0 | 3 | magic `A7 52 4C` |
//! | 3 | 1 | version 1 |
//! | 4 | 1 | kind ([`Kind`]) |
//! | 5 | 1 | channel: the media stream of a [`Kind::Media`] datagram |
//! | 6 | 2 | reserved, 0 |
//! | 8 | 8 | path tag: which device and host session |
//! | 16 | 8 | sequence number, per sender and direction, strictly increasing |
//! | 24 | 16 | punches only: the device's session nonce, in the clear |
//! | .. | .. | ciphertext, then the 16-byte tag; the AAD is every byte before it |
//!
//! The nonce is the sequence number (big-endian) and four zero bytes; the keys differ per
//! direction and per connection, so no nonce repeats under a key. STUN never starts with `A7`,
//! and a DHT message always starts with `d`, so the three share one socket.

use crate::crypto::{Aead, Key, TAG_BYTES};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAGIC: [u8; 3] = [0xa7, b'R', b'L'];
pub const VERSION: u8 = 1;
pub const HEADER_BYTES: usize = 24;
pub const SESSION_BYTES: usize = 16;
/// The bytes a sealed datagram adds to its payload (punches add [`SESSION_BYTES`] more).
pub const OVERHEAD: usize = HEADER_BYTES + TAG_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Opens the path: sent to every candidate address until one answers.
    Punch = 1,
    /// Answers a punch, to the address it came from.
    PunchAck = 2,
    /// A segment of the reliable stream layer ([`crate::stream`]).
    Stream = 3,
    /// A media datagram for the channel's port, forwarded as it came.
    Media = 4,
    /// The sender ends the path.
    Close = 5,
    /// Keeps the NATs' mappings open while the path is idle.
    Keepalive = 6,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Kind::Punch,
            2 => Kind::PunchAck,
            3 => Kind::Stream,
            4 => Kind::Media,
            5 => Kind::Close,
            6 => Kind::Keepalive,
            _ => return None,
        })
    }
    fn carries_session(self) -> bool {
        matches!(self, Kind::Punch | Kind::PunchAck)
    }
}

/// The media channels, in the order of [`crate::record::Ports`]' media ports.
pub const CHANNEL_VIDEO: u8 = 0;
pub const CHANNEL_AUDIO: u8 = 1;
pub const CHANNEL_CONTROL: u8 = 2;
pub const CHANNEL_MIC: u8 = 3;
pub const CHANNELS: usize = 4;

pub fn is_wire(datagram: &[u8]) -> bool {
    datagram.len() >= OVERHEAD && datagram[..3] == MAGIC && datagram[3] == VERSION
}

/// The path tag of a datagram, before it is opened.
pub fn tag(datagram: &[u8]) -> Option<[u8; 8]> {
    is_wire(datagram).then(|| datagram[8..16].try_into().unwrap())
}

/// The device's session nonce a punch carries, before it is opened: the host needs it to make
/// the keys that open it.
pub fn punch_session(datagram: &[u8]) -> Option<[u8; SESSION_BYTES]> {
    if !is_wire(datagram) || !Kind::from_byte(datagram[4]).is_some_and(Kind::carries_session) {
        return None;
    }
    datagram
        .get(HEADER_BYTES..HEADER_BYTES + SESSION_BYTES)
        .map(|bytes| bytes.try_into().unwrap())
}

fn nonce(seq: u64) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[..8].copy_from_slice(&seq.to_be_bytes());
    nonce
}

/// One direction's sending side. Shared between threads: the sequence number is atomic.
pub struct Sealer {
    key: Aead,
    tag: [u8; 8],
    seq: AtomicU64,
    session: [u8; SESSION_BYTES],
}

impl Sealer {
    /// `session` is the device's session nonce, which punches carry.
    pub fn new(key: &Key, tag: [u8; 8], session: [u8; SESSION_BYTES]) -> Self {
        Self {
            key: Aead::new(key),
            tag,
            seq: AtomicU64::new(1),
            session,
        }
    }

    pub fn seal(&self, kind: Kind, channel: u8, payload: &[u8]) -> Vec<u8> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let session = if kind.carries_session() {
            SESSION_BYTES
        } else {
            0
        };
        let mut out = Vec::with_capacity(OVERHEAD + session + payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(kind as u8);
        out.push(channel);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&self.tag);
        out.extend_from_slice(&seq.to_be_bytes());
        if kind.carries_session() {
            out.extend_from_slice(&self.session);
        }
        let aad_bytes = out.len();
        out.extend_from_slice(payload);
        self.key.seal_after(nonce(seq), &mut out, aad_bytes);
        out
    }
}

/// How many sequence numbers behind the newest the replay window still takes. All channels
/// share one sequence, and a 4K frame with its FEC runs to hundreds of datagrams, so a window of
/// 64 would drop video datagrams that audio or control overtook.
const WINDOW_BITS: u64 = 512;
const WINDOW_WORDS: usize = (WINDOW_BITS / 64) as usize;

/// One direction's receiving side, with a replay window.
pub struct Opener {
    key: Aead,
    tag: [u8; 8],
    highest: u64,
    /// Bit `n` (word `n / 64`, bit `n % 64`) is set when `highest - n` was taken.
    window: [u64; WINDOW_WORDS],
}

/// What an opened datagram holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub kind: Kind,
    pub channel: u8,
    pub payload: Vec<u8>,
}

impl Opener {
    pub fn new(key: &Key, tag: [u8; 8]) -> Self {
        Self {
            key: Aead::new(key),
            tag,
            highest: 0,
            window: [0; WINDOW_WORDS],
        }
    }

    /// Opens a datagram for this path; None when it is not this path's, does not authenticate
    /// or repeats one already taken.
    pub fn open(&mut self, datagram: &[u8]) -> Option<Opened> {
        if !is_wire(datagram) || datagram[8..16] != self.tag {
            return None;
        }
        let kind = Kind::from_byte(datagram[4])?;
        let seq = u64::from_be_bytes(datagram[16..24].try_into().unwrap());
        let aad_bytes = HEADER_BYTES
            + if kind.carries_session() {
                SESSION_BYTES
            } else {
                0
            };
        if datagram.len() < aad_bytes + TAG_BYTES || seq == 0 || !self.fresh(seq) {
            return None;
        }
        let mut body = datagram[aad_bytes..].to_vec();
        if !self.key.open(nonce(seq), &datagram[..aad_bytes], &mut body) {
            return None;
        }
        self.accept(seq);
        Some(Opened {
            kind,
            channel: datagram[5],
            payload: body,
        })
    }

    fn fresh(&self, seq: u64) -> bool {
        if seq > self.highest {
            return true;
        }
        let behind = self.highest - seq;
        behind < WINDOW_BITS && !self.taken(behind)
    }

    fn taken(&self, behind: u64) -> bool {
        self.window[(behind / 64) as usize] & (1 << (behind % 64)) != 0
    }

    fn mark(&mut self, behind: u64) {
        self.window[(behind / 64) as usize] |= 1 << (behind % 64);
    }

    fn accept(&mut self, seq: u64) {
        if seq > self.highest {
            self.shift(seq - self.highest);
            self.highest = seq;
        }
        self.mark(self.highest - seq);
    }

    /// Moves the window `shift` sequence numbers on: every bit `n` becomes bit `n + shift`.
    fn shift(&mut self, shift: u64) {
        if shift >= WINDOW_BITS {
            self.window = [0; WINDOW_WORDS];
            return;
        }
        let words = (shift / 64) as usize;
        let bits = (shift % 64) as u32;
        for i in (0..WINDOW_WORDS).rev() {
            let low = i.checked_sub(words).map_or(0, |j| self.window[j]);
            let carry = match i.checked_sub(words + 1) {
                Some(j) if bits != 0 => self.window[j] >> (64 - bits),
                _ => 0,
            };
            self.window[i] = (low << bits) | carry;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagrams_open_once_and_only_on_their_path() {
        let sealer = Sealer::new(&[1; 32], [2; 8], [3; 16]);
        let mut opener = Opener::new(&[1; 32], [2; 8]);
        let media = sealer.seal(Kind::Media, CHANNEL_AUDIO, b"opus");
        assert_eq!(media.len(), OVERHEAD + 4);
        assert_eq!(tag(&media), Some([2; 8]));
        assert_eq!(punch_session(&media), None);
        assert_eq!(
            opener.open(&media),
            Some(Opened {
                kind: Kind::Media,
                channel: CHANNEL_AUDIO,
                payload: b"opus".to_vec()
            })
        );
        // A replay is refused.
        assert_eq!(opener.open(&media), None);
        // Another path's key or tag is refused.
        assert_eq!(
            Opener::new(&[9; 32], [2; 8]).open(&sealer.seal(Kind::Stream, 0, b"x")),
            None
        );
        assert_eq!(
            Opener::new(&[1; 32], [7; 8]).open(&sealer.seal(Kind::Stream, 0, b"x")),
            None
        );
        // A changed header byte fails the seal.
        let mut changed = sealer.seal(Kind::Media, CHANNEL_VIDEO, b"frame");
        changed[5] = CHANNEL_MIC;
        assert_eq!(opener.open(&changed), None);
    }

    #[test]
    fn punches_carry_the_device_session_in_the_clear() {
        let sealer = Sealer::new(&[1; 32], [2; 8], [3; 16]);
        let punch = sealer.seal(Kind::Punch, 0, &[]);
        assert_eq!(punch_session(&punch), Some([3; 16]));
        let mut opener = Opener::new(&[1; 32], [2; 8]);
        assert_eq!(
            opener.open(&punch).map(|opened| opened.kind),
            Some(Kind::Punch)
        );
        // The session is authenticated: another one in its place fails.
        let mut forged = sealer.seal(Kind::Punch, 0, &[]);
        forged[HEADER_BYTES] ^= 1;
        assert_eq!(opener.open(&forged), None);
    }

    #[test]
    fn late_datagrams_within_the_window_still_open() {
        let sealer = Sealer::new(&[1; 32], [2; 8], [0; 16]);
        let datagrams: Vec<Vec<u8>> = (0..100)
            .map(|_| sealer.seal(Kind::Media, 0, b"x"))
            .collect();
        let mut opener = Opener::new(&[1; 32], [2; 8]);
        assert!(opener.open(&datagrams[80]).is_some());
        assert!(opener.open(&datagrams[50]).is_some());
        assert!(opener.open(&datagrams[50]).is_none());
        assert!(opener.open(&datagrams[10]).is_some());
        assert!(opener.open(&datagrams[10]).is_none());
        assert!(opener.open(&datagrams[99]).is_some());
        assert!(!is_wire(b"d1:t2:aae"));
    }

    #[test]
    fn datagrams_far_behind_open_once_and_too_old_ones_never() {
        let sealer = Sealer::new(&[1; 32], [2; 8], [0; 16]);
        let datagrams: Vec<Vec<u8>> = (0..1200)
            .map(|n| sealer.seal(Kind::Media, (n % 4) as u8, &[n as u8; 3]))
            .collect();
        let mut opener = Opener::new(&[1; 32], [2; 8]);
        // The newest first, then 200 behind it, across several word boundaries.
        assert!(opener.open(&datagrams[300]).is_some());
        assert!(opener.open(&datagrams[100]).is_some());
        assert!(opener.open(&datagrams[100]).is_none());
        for n in (101..300).rev() {
            assert!(opener.open(&datagrams[n]).is_some(), "{n}");
        }
        for (n, datagram) in datagrams.iter().enumerate().take(301).skip(100) {
            assert!(opener.open(datagram).is_none(), "{n}");
        }
        // Moving on by 70 and by 130 keeps what was taken.
        assert!(opener.open(&datagrams[370]).is_some());
        assert!(opener.open(&datagrams[300]).is_none());
        assert!(opener.open(&datagrams[500]).is_some());
        assert!(opener.open(&datagrams[100]).is_none());
        assert!(opener.open(&datagrams[370]).is_none());
        assert!(opener.open(&datagrams[50]).is_some());
        assert!(opener.open(&datagrams[50]).is_none());
        // 512 or more behind is too old to tell from a replay.
        assert!(opener.open(&datagrams[1100]).is_some());
        assert!(opener.open(&datagrams[1100 - 512]).is_none());
        assert!(opener.open(&datagrams[1100 - 511]).is_some());
        // Moving on by 99 keeps what was taken.
        assert!(opener.open(&datagrams[1199]).is_some());
        assert!(opener.open(&datagrams[1100]).is_none());
        assert!(opener.open(&datagrams[1150]).is_some());
    }
}
