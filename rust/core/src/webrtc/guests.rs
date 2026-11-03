//! Guest links for the console's browser stream (docs/browser-guests.md), as pure logic: no
//! sockets, no clock, no global RNG. The caller passes the time and the RNG in.
//!
//! An invite is a 128-bit token the owner hands out as a link. The host keeps only its SHA-256,
//! compares every stored hash in constant time, and forgets the invite when it expires or the
//! owner revokes it. Wrong tokens count against the address that sent them ([`Lookups`]), so a
//! guest page left open to the internet cannot be used to guess at full speed.
use crate::crypto;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

/// Random bytes in a token: 128 bits, 32 hex characters in the link.
pub const TOKEN_BYTES: usize = 16;
/// Random bytes in an invite's public id, which the console shows and revokes by.
pub const ID_BYTES: usize = 8;
/// Invites kept at once; creating one more refuses rather than forgetting a link someone holds.
pub const MAX_INVITES: usize = 16;
/// The shortest and longest an invite may last.
pub const MIN_LIFETIME: Duration = Duration::from_secs(5 * 60);
pub const MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A guest's name as the owner sees it, at most this many characters.
pub const MAX_NAME_CHARS: usize = 32;

/// What a guest may do besides watching and listening.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Picture and sound only. Whatever the guest's page sends is dropped on the host.
    View,
    /// Gamepads, each on a virtual pad of its own.
    Controller,
    /// Gamepads, keyboard and mouse.
    ControllerKeyboardMouse,
}

/// The permission bits of controller and mouse input (`Input::required_permission`).
const CONTROLLER: u32 = 1 << 8;
const MOUSE: u32 = 1 << 11;

impl Permission {
    pub fn gamepads(self) -> bool {
        !matches!(self, Self::View)
    }

    pub fn keyboard_mouse(self) -> bool {
        matches!(self, Self::ControllerKeyboardMouse)
    }

    /// The [`Input::required_permission`](crate::input::Input::required_permission) bits a
    /// guest's input may carry, on top of what the owner's device allows: controllers, and the
    /// keyboard and mouse only with that permission. Never touch, pen or anything else.
    pub fn input_mask(self) -> u32 {
        match self {
            Self::View => 0,
            Self::Controller => CONTROLLER,
            Self::ControllerKeyboardMouse => CONTROLLER | MOUSE | crate::input::KEYBOARD_PERMISSION,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(text.into())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Controller => "controller",
            Self::ControllerKeyboardMouse => "controller-keyboard-mouse",
        }
    }
}

/// One guest link. The token itself is never kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    pub id: String,
    hash: [u8; 32],
    pub permission: Permission,
    /// Unix seconds.
    pub created: u64,
    pub expires: u64,
}

impl Invite {
    pub fn expired(&self, now: u64) -> bool {
        now >= self.expires
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteError {
    /// Shorter than [`MIN_LIFETIME`] or longer than [`MAX_LIFETIME`].
    Lifetime,
    /// [`MAX_INVITES`] links are live already.
    Full,
}

impl std::fmt::Display for InviteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lifetime => write!(
                f,
                "an invite lasts {} minutes to {} days",
                MIN_LIFETIME.as_secs() / 60,
                MAX_LIFETIME.as_secs() / 86_400
            ),
            Self::Full => write!(
                f,
                "{MAX_INVITES} invites are open already; revoke one first"
            ),
        }
    }
}

impl std::error::Error for InviteError {}

/// The invites the owner has handed out and not revoked.
#[derive(Debug, Default)]
pub struct Invites {
    invites: Vec<Invite>,
}

/// A token in the link's form: exactly [`TOKEN_BYTES`] in lower-case hex.
fn parse_token(token: &str) -> Option<[u8; TOKEN_BYTES]> {
    if token.len() != TOKEN_BYTES * 2
        || !token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut bytes = [0; TOKEN_BYTES];
    hex::decode_to_slice(token, &mut bytes).ok()?;
    Some(bytes)
}

impl Invites {
    /// A new invite lasting `lifetime` from `now` (Unix seconds), and its token, which only the
    /// caller ever sees.
    pub fn create<R: RngCore + ?Sized>(
        &mut self,
        rng: &mut R,
        permission: Permission,
        lifetime: Duration,
        now: u64,
    ) -> Result<(Invite, String), InviteError> {
        if !(MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) {
            return Err(InviteError::Lifetime);
        }
        self.prune(now);
        if self.invites.len() >= MAX_INVITES {
            return Err(InviteError::Full);
        }
        let mut token = [0u8; TOKEN_BYTES];
        rng.fill_bytes(&mut token);
        let mut id = [0u8; ID_BYTES];
        rng.fill_bytes(&mut id);
        let invite = Invite {
            id: hex::encode(id),
            hash: crypto::hash(&token),
            permission,
            created: now,
            expires: now + lifetime.as_secs(),
        };
        self.invites.push(invite.clone());
        Ok((invite, hex::encode(token)))
    }

    /// The live invite `token` opens. Every stored hash is compared, in constant time, whether
    /// or not an earlier one matched; an expired invite opens nothing.
    pub fn find(&self, token: &str, now: u64) -> Option<&Invite> {
        let hash = crypto::hash(&parse_token(token)?);
        let mut found = None;
        for invite in &self.invites {
            if crypto::equal(&invite.hash, &hash) && found.is_none() {
                found = Some(invite);
            }
        }
        found.filter(|invite| !invite.expired(now))
    }

    pub fn get(&self, id: &str, now: u64) -> Option<&Invite> {
        self.invites
            .iter()
            .find(|invite| invite.id == id && !invite.expired(now))
    }

    /// Forget an invite; false when there was none by that id.
    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.invites.len();
        self.invites.retain(|invite| invite.id != id);
        self.invites.len() != before
    }

    /// Forget the expired invites, and say which went.
    pub fn prune(&mut self, now: u64) -> Vec<Invite> {
        let (expired, live) = std::mem::take(&mut self.invites)
            .into_iter()
            .partition(|invite| invite.expired(now));
        self.invites = live;
        expired
    }

    /// The live invites, oldest first.
    pub fn live(&self, now: u64) -> impl Iterator<Item = &Invite> {
        self.invites
            .iter()
            .filter(move |invite| !invite.expired(now))
    }
}

/// Wrong tokens per address over the last minute. A right token never counts, and an address
/// over the limit is refused before its token is looked at, right or wrong. IPv6 counts per /64,
/// the block one site can take addresses from.
#[derive(Debug, Default)]
pub struct Lookups {
    failures: HashMap<IpAddr, (Instant, u32)>,
}

impl Lookups {
    pub const LIMIT: u32 = 5;
    pub const WINDOW: Duration = Duration::from_secs(60);
    pub const TRACKED: usize = 1024;

    fn key(peer: IpAddr) -> IpAddr {
        match peer.to_canonical() {
            IpAddr::V6(address) => Ipv6Addr::from(u128::from(address) & (u128::MAX << 64)).into(),
            v4 => v4,
        }
    }

    fn forget_old(&mut self, now: Instant) {
        self.failures
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
    }

    /// Whether `peer` may try a token now.
    pub fn allowed(&mut self, peer: IpAddr, now: Instant) -> bool {
        self.forget_old(now);
        self.failures
            .get(&Self::key(peer))
            .is_none_or(|(_, count)| *count < Self::LIMIT)
    }

    /// `peer` sent a token that opened nothing.
    pub fn failed(&mut self, peer: IpAddr, now: Instant) {
        self.forget_old(now);
        let key = Self::key(peer);
        // Full: forget the oldest address instead of refusing every new one, which would let a
        // flood of addresses lock everyone out.
        if self.failures.len() >= Self::TRACKED
            && !self.failures.contains_key(&key)
            && let Some(oldest) = self
                .failures
                .iter()
                .min_by_key(|(_, (since, _))| *since)
                .map(|(address, _)| *address)
        {
            self.failures.remove(&oldest);
        }
        self.failures.entry(key).or_insert((now, 0)).1 += 1;
    }
}

/// The name a guest typed, as the owner sees it: no control characters, no surrounding space,
/// at most [`MAX_NAME_CHARS`]; "Guest" when nothing is left.
pub fn guest_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let name = name.trim().to_owned();
    if name.is_empty() {
        "Guest".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    const HOUR: Duration = Duration::from_secs(3600);
    const NOW: u64 = 1_800_000_000;

    fn invites() -> (Invites, StdRng) {
        (Invites::default(), StdRng::seed_from_u64(7))
    }

    #[test]
    fn a_token_opens_its_invite_until_it_expires() {
        let (mut invites, mut rng) = invites();
        let (invite, token) = invites
            .create(&mut rng, Permission::Controller, HOUR, NOW)
            .unwrap();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(invite.expires, NOW + 3600);
        assert_eq!(invites.find(&token, NOW).map(|i| &i.id), Some(&invite.id));
        assert_eq!(
            invites.find(&token, NOW + 3599).map(|i| i.permission),
            Some(Permission::Controller)
        );
        // The second it expires it opens nothing, and pruning forgets it.
        assert!(invites.find(&token, NOW + 3600).is_none());
        assert!(invites.get(&invite.id, NOW + 3600).is_none());
        assert_eq!(invites.live(NOW + 3600).count(), 0);
        assert_eq!(invites.prune(NOW + 3600), vec![invite]);
        assert!(invites.find(&token, NOW).is_none());
    }

    #[test]
    fn the_token_is_not_stored_only_its_hash() {
        let (mut invites, mut rng) = invites();
        let (invite, token) = invites
            .create(&mut rng, Permission::View, HOUR, NOW)
            .unwrap();
        let mut bytes = [0u8; TOKEN_BYTES];
        hex::decode_to_slice(&token, &mut bytes).unwrap();
        assert_eq!(invite.hash, crypto::hash(&bytes));
        let stored = format!("{invites:?}");
        assert!(!stored.contains(&token), "{stored}");
        assert!(!invite.id.contains(&token[..16]));
    }

    #[test]
    fn a_revoked_invite_opens_nothing_and_others_stay() {
        let (mut invites, mut rng) = invites();
        let (first, first_token) = invites
            .create(&mut rng, Permission::View, HOUR, NOW)
            .unwrap();
        let (second, second_token) = invites
            .create(&mut rng, Permission::ControllerKeyboardMouse, HOUR, NOW)
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first_token, second_token);
        assert!(invites.revoke(&first.id));
        assert!(!invites.revoke(&first.id), "already gone");
        assert!(invites.find(&first_token, NOW).is_none());
        assert_eq!(
            invites.find(&second_token, NOW).map(|i| i.id.clone()),
            Some(second.id)
        );
    }

    #[test]
    fn wrong_and_malformed_tokens_open_nothing() {
        let (mut invites, mut rng) = invites();
        let (_, token) = invites
            .create(&mut rng, Permission::View, HOUR, NOW)
            .unwrap();
        let mut flipped = token.clone().into_bytes();
        flipped[31] = if flipped[31] == b'0' { b'1' } else { b'0' };
        let flipped = String::from_utf8(flipped).unwrap();
        for wrong in [
            flipped.as_str(),
            &token[..31],
            &format!("{token}0"),
            &token.to_ascii_uppercase(),
            "",
            &format!(" {}", &token[1..]),
            "zz000000000000000000000000000000",
        ] {
            assert!(invites.find(wrong, NOW).is_none(), "{wrong:?}");
        }
    }

    #[test]
    fn lifetimes_are_bounded_and_the_number_of_invites_too() {
        let (mut invites, mut rng) = invites();
        for lifetime in [
            MIN_LIFETIME - Duration::from_secs(1),
            MAX_LIFETIME + Duration::from_secs(1),
        ] {
            assert_eq!(
                invites
                    .create(&mut rng, Permission::View, lifetime, NOW)
                    .unwrap_err(),
                InviteError::Lifetime
            );
        }
        for _ in 0..MAX_INVITES {
            invites
                .create(&mut rng, Permission::View, MIN_LIFETIME, NOW)
                .unwrap();
        }
        assert_eq!(
            invites
                .create(&mut rng, Permission::View, MAX_LIFETIME, NOW)
                .unwrap_err(),
            InviteError::Full
        );
        // Expired ones make room.
        let later = NOW + MIN_LIFETIME.as_secs();
        assert!(
            invites
                .create(&mut rng, Permission::View, MAX_LIFETIME, later)
                .is_ok()
        );
        assert_eq!(invites.live(later).count(), 1);
    }

    #[test]
    fn wrong_tokens_are_limited_per_address_and_a_right_one_waits_too() {
        let mut lookups = Lookups::default();
        let now = Instant::now();
        let peer: IpAddr = "203.0.113.9".parse().unwrap();
        for _ in 0..Lookups::LIMIT {
            assert!(lookups.allowed(peer, now));
            lookups.failed(peer, now);
        }
        assert!(!lookups.allowed(peer, now));
        assert!(!lookups.allowed(peer, now + Duration::from_secs(59)));
        // Another address is not held up.
        assert!(lookups.allowed("203.0.113.10".parse().unwrap(), now));
        // A minute after its first failure it may try again.
        assert!(lookups.allowed(peer, now + Lookups::WINDOW));
    }

    #[test]
    fn ipv6_counts_per_slash_64_and_mapped_ipv4_as_ipv4() {
        let mut lookups = Lookups::default();
        let now = Instant::now();
        for i in 0..Lookups::LIMIT {
            lookups.failed(format!("2001:db8:1:2::{i:x}").parse().unwrap(), now);
        }
        assert!(!lookups.allowed("2001:db8:1:2:ffff::1".parse().unwrap(), now));
        assert!(lookups.allowed("2001:db8:1:3::1".parse().unwrap(), now));
        for _ in 0..Lookups::LIMIT {
            lookups.failed("::ffff:198.51.100.7".parse().unwrap(), now);
        }
        assert!(!lookups.allowed("198.51.100.7".parse().unwrap(), now));
    }

    #[test]
    fn a_flood_of_addresses_forgets_the_oldest_not_everyone() {
        let mut lookups = Lookups::default();
        let start = Instant::now();
        let first: IpAddr = "198.51.100.1".parse().unwrap();
        for _ in 0..Lookups::LIMIT {
            lookups.failed(first, start);
        }
        for i in 0..Lookups::TRACKED as u32 {
            let address = IpAddr::V4(std::net::Ipv4Addr::from(
                u32::from(std::net::Ipv4Addr::new(10, 0, 0, 0)) + i,
            ));
            lookups.failed(address, start + Duration::from_millis(1));
        }
        assert!(lookups.failures.len() <= Lookups::TRACKED);
        assert!(lookups.allowed(first, start + Duration::from_millis(2)));
    }

    #[test]
    fn guest_names_are_cleaned_and_bounded() {
        assert_eq!(guest_name("  Alex \n"), "Alex");
        assert_eq!(guest_name("A\u{0}l\u{7}e\tx"), "A l e x");
        assert_eq!(guest_name(""), "Guest");
        assert_eq!(guest_name(" \r\n"), "Guest");
        let long = "é".repeat(100);
        assert_eq!(guest_name(&long).chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn permissions_say_what_they_allow_and_round_trip() {
        for (permission, gamepads, keyboard_mouse) in [
            (Permission::View, false, false),
            (Permission::Controller, true, false),
            (Permission::ControllerKeyboardMouse, true, true),
        ] {
            assert_eq!(permission.gamepads(), gamepads);
            assert_eq!(permission.keyboard_mouse(), keyboard_mouse);
            assert_eq!(Permission::parse(permission.as_str()), Some(permission));
        }
        assert_eq!(Permission::parse("admin"), None);
    }

    #[test]
    fn a_guests_input_mask_follows_the_events_it_may_send() {
        use crate::input::Input;
        let pad = Input::Controller {
            id: 0,
            active: 1,
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        };
        let key = Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        };
        let mouse = Input::Relative { x: 1, y: 1 };
        let allows =
            |p: Permission, event: &Input| p.input_mask() & event.required_permission() != 0;
        for event in [&pad, &key, &mouse] {
            assert!(!allows(Permission::View, event));
            assert!(allows(Permission::ControllerKeyboardMouse, event));
        }
        assert!(allows(Permission::Controller, &pad));
        assert!(!allows(Permission::Controller, &key));
        assert!(!allows(Permission::Controller, &mouse));
    }
}
