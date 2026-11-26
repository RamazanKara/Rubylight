//! Guests of the console's browser stream (docs/browser-guests.md): the invite links the owner
//! hands out, the guests who joined with one, and each guest's socket and media transport.
//!
//! The owner signs in to the console and streams on its Stream page. While that stream runs, a
//! guest with a link opens `/guest/`, whose page signals over `/guest/signal`: the token first,
//! then an offer for the owner's very stream. The guest's transport follows the owner's
//! ([`Followers`]): the same encoded frames and Opus packets, packetized, encrypted and paced by
//! its own media thread, so a slow guest drops frames alone and never delays the owner. Its
//! controllers and keys go through the owner's input worker ([`GuestHandle`]), which checks them
//! against the invite's permission. The owner's stream ending ends every guest's.
//!
//! Nothing here reaches the console: the guest routes sit outside its sign-in, and a guest's
//! socket speaks only this module's few messages.

#[cfg(test)]
use super::sdp::VideoCodec;
use super::{input::InputHandle, media::Followers, sdp::OfferSpec};
use butterpollo_media_policy::audio::OpusLayout;
use butterpollo_session::webrtc::guests::{
    Invite, InviteError, Invites, Lookups, Permission, guest_name,
};
use butterpollo_stream::{
    state::Session,
    webrtc_bridge::{GuestNetDeadlines, GuestNetworkState, InviteInfo, MAX_GUESTS},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::IpAddr,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// The guest page's WebSocket subprotocol.
pub const PROTOCOL: &str = "rubylight.guest.v1";
/// Guest sockets that have not joined yet, in all and from one address (an IPv6 /64). A joined
/// guest's socket no longer counts: the guests already watching are never locked out.
const MAX_SOCKETS: usize = 16;
const MAX_SOCKETS_PER_PEER: usize = 4;
/// How long a page has to send its token, and to answer the offer. A page sends its token as
/// soon as the socket opens; one that does not counts as a wrong token.
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an answered guest may take to connect its media.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a guest waits for its media port, which the guest before it may still hold.
const BIND_WAIT: Duration = Duration::from_secs(3);
/// Keyframes the guests may ask the owner's encoder for: at most one this often, between them
/// all. The encoder makes one picture for everyone, so a guest on a poor link must not keep the
/// owner's in keyframes: each guest also backs off on its own, from this up to 8 s while its
/// losses go on ([`keyframe_backoff`]). A guest that asks too soon is asked for again by its
/// transport.
pub const KEYFRAME_INTERVAL: Duration = Duration::from_secs(2);
const MAX_MESSAGE: usize = 64 * 1024;
/// How long the invite dialog's "On the internet" keeps the guest ports mapped before a link is
/// made with it: long enough to read what the router did and create the link.
pub const INTERNET_PREPARE: Duration = Duration::from_secs(120);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The key sockets are counted under: the address, an IPv6 address by its /64.
fn peer_key(peer: IpAddr) -> IpAddr {
    match peer.to_canonical() {
        IpAddr::V6(address) => {
            std::net::Ipv6Addr::from(u128::from(address) & (u128::MAX << 64)).into()
        }
        v4 => v4,
    }
}

/// The owner's running console stream, which guests join.
#[derive(Clone)]
pub struct Owner {
    pub spec: OfferSpec,
    /// The multiopus layout the owner's browser answered with; None for stereo.
    pub audio_layout: Option<OpusLayout>,
    pub followers: Arc<Followers>,
    pub input: InputHandle,
    pub session: Weak<Session>,
    /// The address the host listens on, for [`crate::network::media_address`].
    pub bind: IpAddr,
}

/// Why the host ended a guest's stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    Kicked,
    Revoked,
    OwnerEnded,
}

impl Leave {
    fn said(self) -> (&'static str, &'static str) {
        match self {
            Self::Kicked => ("kicked", "The host removed you from the stream."),
            Self::Revoked => ("revoked", "The host withdrew this invite link."),
            Self::OwnerEnded => ("ended", "The host's stream ended."),
        }
    }
}

/// Why a guest could not join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// Too many wrong tokens from this address lately.
    RateLimited,
    /// No live invite has this token: wrong, expired or revoked.
    InvalidLink,
    NotStreaming,
    Full,
    /// Too many pages are opening sockets.
    Busy,
}

impl JoinError {
    fn said(self) -> (&'static str, String) {
        match self {
            Self::RateLimited => (
                "rate-limited",
                "Too many wrong links came from this network; wait a minute and try again.".into(),
            ),
            Self::InvalidLink => (
                "invalid-link",
                "This invite link does not work: it expired, the host withdrew it, or it was copied incompletely.".into(),
            ),
            Self::NotStreaming => (
                "not-streaming",
                "The host is not streaming right now. This page joins when the stream starts.".into(),
            ),
            Self::Full => (
                "full",
                format!("{MAX_GUESTS} guests are watching already; try again when one leaves."),
            ),
            Self::Busy => (
                "busy",
                "Too many guest pages are open on this host; try again shortly.".into(),
            ),
        }
    }
}

/// A guest let in: who it is, the stream it joins, and where the host tells it to leave.
pub struct Admission {
    pub id: u64,
    /// Its place among the guests, which picks its media port.
    pub slot: u16,
    pub name: String,
    pub permission: Permission,
    pub owner: Owner,
    pub leave: UnboundedReceiver<Leave>,
    /// The address the owner put in the link, which the guest's media is offered at too.
    pub address: Option<String>,
}

struct Joined {
    slot: u16,
    name: String,
    permission: Permission,
    /// The link it joined with, listed while the guest watches even after the link expired.
    link: InviteInfo,
    since: u64,
    leave: UnboundedSender<Leave>,
}

#[derive(Default)]
struct Inner {
    invites: Invites,
    /// The address the owner put in each live link, by invite id.
    addresses: HashMap<String, String>,
    /// The links made for guests on the internet, by invite id, while they work or a guest
    /// watches with them: the guest ports stay mapped on the router for them.
    internet: HashSet<String>,
    /// Until when the invite dialog asked for the guest ports to be mapped before making a link.
    prepared_until: Option<Instant>,
    lookups: Lookups,
    /// The owner's stream and the number it was registered under.
    owner: Option<(u64, Owner)>,
    generation: u64,
    joined: BTreeMap<u64, Joined>,
    next_guest: u64,
    /// Sockets that have not joined, by address ([`MAX_SOCKETS`]); no entry for none.
    pending: HashMap<IpAddr, usize>,
}

/// The invites and guests of the host. Invites live in memory only: a restart revokes them all.
pub struct Guests {
    inner: Mutex<Inner>,
    /// When a guest last had a keyframe asked for, in milliseconds after `epoch`; 0 for never.
    /// Outside the lock: the owner's frame thread asks when a guest's queue drops a frame.
    keyframe_at: AtomicU64,
    epoch: Instant,
    net: Arc<GuestNetworkState>,
}

impl Default for Guests {
    fn default() -> Self {
        Self::with_network(Arc::new(GuestNetworkState::default()))
    }
}

impl Guests {
    /// Creates a [`Guests`] registry sharing `net` with [`butterpollo_stream::state::Host`].
    pub fn with_network(net: Arc<GuestNetworkState>) -> Self {
        Self {
            inner: Mutex::default(),
            keyframe_at: AtomicU64::new(0),
            epoch: Instant::now(),
            net,
        }
    }

    fn sync_network(&self, inner: &mut Inner) {
        let now = unix_now();
        let instant = Instant::now();
        inner.prune(now);
        if inner.prepared_until.is_some_and(|until| until <= instant) {
            inner.prepared_until = None;
        }
        let open_until_unix = inner.invites.live(now).map(|invite| invite.expires).max();
        let internet_until_unix = inner
            .invites
            .live(now)
            .filter(|invite| inner.internet.contains(&invite.id))
            .map(|invite| invite.expires)
            .max();
        let joined_any = !inner.joined.is_empty();
        let joined_internet = inner
            .joined
            .values()
            .any(|guest| inner.internet.contains(&guest.link.id));
        self.net.update(GuestNetDeadlines {
            joined_any,
            joined_internet,
            open_until_unix,
            internet_until_unix,
            prepared_until: inner.prepared_until,
        });
    }
}

#[derive(Serialize)]
pub struct GuestInfo {
    pub id: u64,
    pub name: String,
    pub permission: Permission,
    pub invite: String,
    pub since: u64,
}

/// What the console's guest panel shows. Never a token: the host does not have them.
#[derive(Serialize)]
pub struct Status {
    pub streaming: bool,
    pub max_guests: u16,
    pub invites: Vec<InviteInfo>,
    pub guests: Vec<GuestInfo>,
}

impl Inner {
    /// Forget expired invites and the addresses of links that are gone. An internet link stays
    /// marked while a guest still watches with it, so its ports stay mapped.
    fn prune(&mut self, now: u64) {
        self.invites.prune(now);
        let Self {
            invites,
            addresses,
            internet,
            joined,
            ..
        } = self;
        addresses.retain(|id, _| invites.live(now).any(|invite| invite.id == *id));
        internet.retain(|id| {
            invites.live(now).any(|invite| invite.id == *id)
                || joined.values().any(|guest| guest.link.id == *id)
        });
    }

    /// Whether the guest ports should be mapped for the internet: the dialog asked within
    /// [`INTERNET_PREPARE`], an internet link works, or a guest who joined with one watches.
    #[cfg(test)]
    fn internet_wanted(&mut self, now: u64, instant: Instant) -> bool {
        self.prune(now);
        if self.prepared_until.is_some_and(|until| until <= instant) {
            self.prepared_until = None;
        }
        self.prepared_until.is_some() || !self.internet.is_empty()
    }
}

impl Guests {
    /// A new invite and its token, which only the caller ever sees. `address` is the one the
    /// owner puts in the link.
    pub fn create(
        &self,
        permission: Permission,
        lifetime: Duration,
        address: Option<String>,
    ) -> Result<(Invite, String), InviteError> {
        self.create_for(permission, lifetime, address, false)
    }

    /// [`create`](Self::create), and for `internet` keeps the guest ports mapped on the router
    /// while the link works or a guest watches with it.
    pub fn create_for(
        &self,
        permission: Permission,
        lifetime: Duration,
        address: Option<String>,
        internet: bool,
    ) -> Result<(Invite, String), InviteError> {
        let now = unix_now();
        let mut inner = self.inner.lock().unwrap();
        inner.prune(now);
        let (invite, token) =
            inner
                .invites
                .create(&mut rand::rngs::OsRng, permission, lifetime, now)?;
        if let Some(address) = address {
            inner.addresses.insert(invite.id.clone(), address);
        }
        if internet {
            inner.internet.insert(invite.id.clone());
            // The link now holds the ports the dialog asked for.
            inner.prepared_until = None;
            self.sync_network(&mut inner);
            drop(inner);
            self.net.bump_internet_round();
        } else {
            self.sync_network(&mut inner);
        }
        // With automatic port forwarding on, any link wants the guest ports: map them now.
        self.net.notify_mapping_changed();
        Ok((invite, token))
    }

    /// The invite dialog picked "On the internet": map the guest ports now, before a link is
    /// made, for [`INTERNET_PREPARE`], so it can show what the router did. Returns the request's
    /// number, which `MappingStatus::guest_round` reaches once the port mapping task has asked
    /// the router.
    pub fn prepare_internet(&self) -> u64 {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.prepared_until = Some(Instant::now() + INTERNET_PREPARE);
            self.sync_network(&mut inner);
        }
        self.net.bump_internet_round();
        let round = self.net.internet_round();
        self.net.notify_mapping_changed();
        round
    }

    /// Whether the guest ports should be mapped for guests on the internet now.
    pub fn internet_wanted(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        self.sync_network(&mut inner);
        self.net.internet_wanted()
    }

    /// The last request for the guest ports ([`prepare_internet`](Self::prepare_internet) or an
    /// internet link): the port mapping task reads it before it asks the router.
    pub fn internet_round(&self) -> u64 {
        self.net.internet_round()
    }

    /// Resolves when the guest ports became wanted or unwanted at once (a link made or revoked).
    pub async fn mapping_changed(&self) {
        self.net.mapping_changed().await;
    }

    /// Whether a guest page may load: a link is live or a guest is watching. Until the owner
    /// makes a link, the guest port answers nothing.
    pub fn open(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        self.sync_network(&mut inner);
        self.net.open()
    }

    pub fn status(&self) -> Status {
        let now = unix_now();
        let mut inner = self.inner.lock().unwrap();
        inner.prune(now);
        let mut invites: Vec<InviteInfo> = inner
            .invites
            .live(now)
            .map(|invite| InviteInfo::from_invite(invite, false))
            .collect();
        for invite in &mut invites {
            invite.internet = inner.internet.contains(&invite.id);
        }
        // An expired link lets nobody new in, but whoever joined with it is still watching: it
        // stays listed, so it can be revoked.
        for joined in inner.joined.values() {
            if invites.iter().all(|i| i.id != joined.link.id) {
                invites.push(joined.link.clone());
            }
        }
        self.sync_network(&mut inner);
        Status {
            streaming: inner.owner.is_some(),
            max_guests: MAX_GUESTS,
            invites,
            guests: inner
                .joined
                .iter()
                .map(|(id, joined)| GuestInfo {
                    id: *id,
                    name: joined.name.clone(),
                    permission: joined.permission,
                    invite: joined.link.id.clone(),
                    since: joined.since,
                })
                .collect(),
        }
    }

    /// Withdraw an invite: it opens nothing from now on, and whoever joined with it is
    /// disconnected, also after it expired. False when there was no such invite and nobody
    /// watches with it.
    pub fn revoke(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let mut found = inner.invites.revoke(id);
        inner.addresses.remove(id);
        // Its guests are told to leave; until they have, their media still needs the ports.
        if !inner.joined.values().any(|j| j.link.id == id) && inner.internet.remove(id) {
            self.net.notify_mapping_changed();
        }
        for joined in inner.joined.values().filter(|j| j.link.id == id) {
            let _ = joined.leave.send(Leave::Revoked);
            found = true;
        }
        self.sync_network(&mut inner);
        found
    }

    /// Disconnect a guest; its link still works. False when there was no such guest.
    pub fn kick(&self, id: u64) -> bool {
        let inner = self.inner.lock().unwrap();
        let Some(joined) = inner.joined.get(&id) else {
            return false;
        };
        let _ = joined.leave.send(Leave::Kicked);
        true
    }

    /// The owner's stream started; guests of a stream it replaced are disconnected. Returns the
    /// number [`owner_ended`](Self::owner_ended) takes.
    pub fn owner_started(&self, owner: Owner) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        let generation = inner.generation;
        for joined in inner.joined.values() {
            let _ = joined.leave.send(Leave::OwnerEnded);
        }
        inner.owner = Some((generation, owner));
        self.keyframe_at.store(0, Ordering::Release);
        generation
    }

    /// The owner's stream `generation` ended: every guest is disconnected. Nothing happens for a
    /// stream that another replaced already.
    pub fn owner_ended(&self, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.owner.as_ref().is_none_or(|(g, _)| *g != generation) {
            return;
        }
        inner.owner = None;
        for joined in inner.joined.values() {
            let _ = joined.leave.send(Leave::OwnerEnded);
        }
    }

    /// Let in the guest whose page sent `token` from `peer`, unless that address sent too many
    /// wrong ones lately: a right token waits out the limit too.
    pub fn join(
        &self,
        peer: IpAddr,
        token: &str,
        name: &str,
        now: Instant,
    ) -> Result<Admission, JoinError> {
        self.join_at(peer, token, name, now, unix_now())
    }

    fn join_at(
        &self,
        peer: IpAddr,
        token: &str,
        name: &str,
        now: Instant,
        unix: u64,
    ) -> Result<Admission, JoinError> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.lookups.allowed(peer, now) {
            return Err(JoinError::RateLimited);
        }
        let Some((mut link, permission)) = inner
            .invites
            .find(token, unix)
            .map(|invite| (InviteInfo::from_invite(invite, false), invite.permission))
        else {
            inner.lookups.failed(peer, now);
            return Err(JoinError::InvalidLink);
        };
        let Some((_, owner)) = inner.owner.clone() else {
            return Err(JoinError::NotStreaming);
        };
        let Some(slot) = (0..MAX_GUESTS).find(|s| inner.joined.values().all(|j| j.slot != *s))
        else {
            return Err(JoinError::Full);
        };
        inner.next_guest += 1;
        let id = inner.next_guest;
        let name = guest_name(name);
        let (leave, receiver) = unbounded_channel();
        let address = inner.addresses.get(&link.id).cloned();
        link.internet = inner.internet.contains(&link.id);
        inner.joined.insert(
            id,
            Joined {
                slot,
                name: name.clone(),
                permission,
                link,
                since: unix,
                leave,
            },
        );
        self.sync_network(&mut inner);
        Ok(Admission {
            id,
            slot,
            name,
            permission,
            owner,
            leave: receiver,
            address,
        })
    }

    /// A guest's stream ended, however it did.
    pub fn left(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.joined.remove(&id).is_some() {
            let before = inner.internet.len();
            inner.prune(unix_now());
            self.sync_network(&mut inner);
            if inner.internet.len() < before {
                drop(inner);
                self.net.notify_mapping_changed();
            }
        }
    }

    /// A guest in a seat, without its media.
    #[cfg(any(test, feature = "test-support"))]
    pub fn seat_for_test(&self) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.next_guest += 1;
        let id = inner.next_guest;
        let link = InviteInfo {
            id: "test".into(),
            permission: Permission::View,
            created: 0,
            expires: u64::MAX,
            internet: false,
        };
        inner.joined.insert(
            id,
            Joined {
                slot: 0,
                name: "Guest".into(),
                permission: Permission::View,
                link,
                since: 0,
                leave: unbounded_channel().0,
            },
        );
        self.sync_network(&mut inner);
        id
    }

    /// Ask the owner's encoder (`session`, the owner's) for a keyframe for a guest that lost its
    /// picture, unless a guest had one less than [`KEYFRAME_INTERVAL`] ago. False when it was
    /// not asked. Takes no lock: the owner's frame thread calls it.
    pub fn keyframe(&self, session: &Weak<Session>, now: Instant) -> bool {
        let at = now.saturating_duration_since(self.epoch).as_millis() as u64 + 1;
        let last = self.keyframe_at.load(Ordering::Acquire);
        if last != 0 && at.saturating_sub(last) < KEYFRAME_INTERVAL.as_millis() as u64 {
            return false;
        }
        let Some(session) = session.upgrade() else {
            return false;
        };
        if self
            .keyframe_at
            .compare_exchange(last, at, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        // The host's own request, not the client's: it is no evidence about the owner's network.
        session.request_host_keyframe();
        true
    }

    /// Count a socket from `peer` that has not joined yet. Refused while the address is waiting
    /// out wrong tokens, or too many sockets wait already.
    fn open_socket(&self, peer: IpAddr, now: Instant) -> Result<(), JoinError> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.lookups.allowed(peer, now) {
            return Err(JoinError::RateLimited);
        }
        let key = peer_key(peer);
        let total: usize = inner.pending.values().sum();
        let count = inner.pending.get(&key).copied().unwrap_or(0);
        if total >= MAX_SOCKETS || count >= MAX_SOCKETS_PER_PEER {
            return Err(JoinError::Busy);
        }
        inner.pending.insert(key, count + 1);
        Ok(())
    }

    /// A socket from `peer` joined, or closed before it did.
    fn close_socket(&self, peer: IpAddr) {
        let mut inner = self.inner.lock().unwrap();
        let key = peer_key(peer);
        if let Some(count) = inner.pending.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                inner.pending.remove(&key);
            }
        }
    }

    /// A page from `peer` sent no token in time, or something else first: as a wrong token.
    fn unanswered(&self, peer: IpAddr, now: Instant) {
        self.inner.lock().unwrap().lookups.failed(peer, now);
    }
}

mod guards;
pub use guards::OwnerGuard;

pub mod keyframe_backoff;

mod signal;
#[cfg(test)]
use signal::host_name;
pub use signal::upgrade;

#[cfg(test)]
mod tests;
