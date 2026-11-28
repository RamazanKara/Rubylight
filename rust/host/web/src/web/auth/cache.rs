//! Sign-in attempt rate limiter and verified Basic-credential cache for the web console.

use butterpollo_session::state::Credentials;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

/// Sign-in attempts per address over the last minute. The login form and
/// Basic credentials share it, so neither lets a peer guess the password at
/// full speed. This PC is never throttled, and IPv6 counts per /64, the
/// block one LAN host can take addresses from.
pub(crate) struct SignIns {
    pub(crate) attempts: std::collections::HashMap<std::net::IpAddr, (Instant, u32)>,
}

pub(super) static SIGN_INS: std::sync::LazyLock<std::sync::Mutex<SignIns>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(SignIns {
            attempts: Default::default(),
        })
    });

impl SignIns {
    const LIMIT: u32 = 10;
    const WINDOW: Duration = Duration::from_secs(60);
    pub(crate) const TRACKED: usize = 1024;

    fn key(peer: std::net::IpAddr) -> std::net::IpAddr {
        let peer = peer.to_canonical();
        match peer {
            std::net::IpAddr::V6(address) if !address.is_loopback() => {
                std::net::Ipv6Addr::from(u128::from(address) & (u128::MAX << 64)).into()
            }
            other => other,
        }
    }

    /// Whether `peer` may attempt sign-in without counting against the limit.
    pub(crate) fn allowed(&mut self, peer: std::net::IpAddr, now: Instant) -> bool {
        let key = Self::key(peer);
        self.attempts
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
        self.attempts
            .get(&key)
            .is_none_or(|(_, count)| *count < Self::LIMIT)
    }

    /// Record a failed sign-in attempt for `peer`.
    pub(crate) fn failed(&mut self, peer: std::net::IpAddr, now: Instant) {
        let key = Self::key(peer);
        self.attempts
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
        // Full: forget the oldest address instead of refusing every new
        // one, which would let a flood of addresses lock everyone out.
        if self.attempts.len() >= Self::TRACKED
            && !self.attempts.contains_key(&key)
            && let Some(oldest) = self
                .attempts
                .iter()
                .min_by_key(|(_, (since, _))| *since)
                .map(|(address, _)| *address)
        {
            self.attempts.remove(&oldest);
        }
        let (_, count) = self.attempts.entry(key).or_insert((now, 0));
        *count = count.saturating_add(1);
    }

    pub(crate) fn succeeded(&mut self, peer: std::net::IpAddr) {
        let key = Self::key(peer);
        self.attempts.remove(&key);
    }
}

/// Basic credentials that passed PBKDF2 in the last five minutes, so a
/// script that sends them with every request is not slowed (and the host
/// not loaded) by 600,000 rounds each time. A key binds the stored username,
/// salt and hash, so it never matches once the password or its hash changes.
pub(crate) struct BasicCache {
    /// Key: when it was verified, and the stored credentials it was checked against.
    pub(crate) entries: std::collections::HashMap<[u8; 32], (Instant, [u8; 32])>,
}

pub(crate) static BASIC_CACHE: std::sync::LazyLock<std::sync::Mutex<BasicCache>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(BasicCache {
            entries: Default::default(),
        })
    });

impl BasicCache {
    pub(crate) const TTL: Duration = Duration::from_secs(300);
    pub(crate) const CAPACITY: usize = 64;

    fn digest(parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        hasher.finalize().into()
    }

    /// The stored credentials, without the attempt.
    fn owner(stored: &Credentials) -> [u8; 32] {
        Self::digest(&[
            stored.username.as_bytes(),
            stored.salt.as_bytes(),
            stored.password.as_bytes(),
        ])
    }

    /// The attempt `user`/`pass` against `stored`; usernames compare as
    /// [`Credentials::verifies`] does.
    pub(crate) fn key(stored: &Credentials, user: &str, pass: &str) -> [u8; 32] {
        Self::digest(&[
            &Self::owner(stored),
            user.trim().to_ascii_lowercase().as_bytes(),
            pass.as_bytes(),
        ])
    }

    fn expire(&mut self, now: Instant) {
        self.entries
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::TTL);
    }

    /// Whether `key` was verified within the last [`Self::TTL`].
    pub(crate) fn hit(&mut self, key: &[u8; 32], now: Instant) -> bool {
        self.expire(now);
        self.entries.contains_key(key)
    }

    /// Remember that `key` passed against `stored` at `now`; full, the oldest entry goes.
    pub(crate) fn insert(&mut self, key: [u8; 32], stored: &Credentials, now: Instant) {
        self.expire(now);
        if self.entries.len() >= Self::CAPACITY
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (since, _))| *since)
                .map(|(key, _)| *key)
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (now, Self::owner(stored)));
    }

    /// Drop every attempt verified against `stored`, which was just replaced.
    pub(crate) fn forget(&mut self, stored: &Credentials) {
        let owner = Self::owner(stored);
        self.entries.retain(|_, (_, of)| *of != owner);
    }
}
