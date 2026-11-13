//! Several devices in one session (docs/multiclient.md), as pure logic: no sockets, no clock, no
//! global state. The caller passes the time in.
//!
//! The device that started or took over the running game owns the session. Another paired device
//! that joins that game (a `/resume`, a Resume or Running Game tile, Nightjar's Join) streams the
//! same display as a guest: its own stream, encoder and controller slot, with what its guest
//! permission allows. [`Guests`] decides who may join and holds every guest's permission, expiry
//! and stream; [`Kbm`] decides which one of them holds the keyboard and mouse.
use crate::config::Config;
pub use crate::webrtc::guests::Permission;
use std::time::{Duration, Instant};

/// The setting that says whether another device's join becomes a guest, and whether it waits for
/// the owner's approval.
pub const JOIN_SETTING: &str = "guest_join";
/// The setting with what a guest may do when nobody chose otherwise.
pub const PERMISSION_SETTING: &str = "guest_permission";
/// The launch option the host sets on a guest's launch, with its guest id. Never the client's: the
/// host removes it from every launch request first.
pub const LAUNCH_OPTION: &str = "rlGuest";
/// Guests in one session at once, besides the owner. Each has an encoder session of its own: with
/// the owner's that stays within the 8 sessions NVIDIA's consumer drivers allow, and leaves AMD's
/// VCN engines time for the owner's stream.
pub const MAX_GUESTS: usize = 4;
/// Join requests waiting at once.
pub const MAX_REQUESTS: usize = 8;
/// How long a join waits for the owner's answer. Moonlight and Nightjar wait 120 s for a launch's
/// reply; this stays well inside that.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(45);
/// How long the keyboard and mouse must have been left alone before a guest who may use them
/// picks them up.
pub const HANDOFF_IDLE: Duration = Duration::from_secs(2);
/// The shortest and longest a guest's time may be set to; none means until the game ends.
pub const MIN_LIFETIME: Duration = Duration::from_secs(5 * 60);
pub const MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Whether a device joining another device's game becomes a guest (`guest_join`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JoinPolicy {
    /// A join streams beside the owner with the device's own permissions, as before guests.
    Off,
    /// A join waits for the owner, or the console, to let it in.
    #[default]
    Ask,
    /// A join becomes a guest at once, with the default permission.
    Allow,
}

impl JoinPolicy {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "ask" => Some(Self::Ask),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ask => "ask",
            Self::Allow => "allow",
        }
    }

    /// `guest_join`; an unknown value asks, the safe reading.
    pub fn from_config(config: &Config) -> Self {
        Self::parse(config.get(JOIN_SETTING, "ask")).unwrap_or_default()
    }
}

/// `guest_permission`; an unknown value is Controller, the default.
pub fn default_permission(config: &Config) -> Permission {
    Permission::parse(config.get(PERMISSION_SETTING, "controller"))
        .unwrap_or(Permission::Controller)
}

/// Whether a guest with `permission` may send an input event that needs `required`
/// ([`crate::input::Input::required_permission`]). The device's own permissions apply as well.
pub fn guest_allows(permission: Permission, required: u32) -> bool {
    permission.input_mask() & required == required && required != 0
}

/// Whether an input event that needs `required` is keyboard or mouse input, which one holder at a
/// time sends ([`Kbm`]).
pub fn is_keyboard_mouse(required: u32) -> bool {
    const MOUSE: u32 = 1 << 11;
    required & (MOUSE | crate::input::KEYBOARD_PERMISSION) != 0
}

/// One guest in the running game's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guest {
    pub id: u32,
    /// The guest device's uuid, which its grant belongs to.
    pub device: String,
    pub name: String,
    pub permission: Permission,
    /// Unix seconds it was let in.
    pub since: u64,
    /// Unix seconds its time ends; none lasts until the game ends.
    pub expires: Option<u64>,
    /// The launch id of its stream while it streams.
    pub launch: Option<String>,
}

impl Guest {
    pub fn expired(&self, now: u64) -> bool {
        self.expires.is_some_and(|expires| now >= expires)
    }
}

/// A device waiting to join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub device: String,
    pub name: String,
    pub asked: Instant,
    answer: Option<Answer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    Approved,
    Denied,
}

/// Why a join was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// [`MAX_GUESTS`] guests, or [`MAX_REQUESTS`] waiting joins, already.
    Full,
    /// The owner said no.
    Denied,
    /// Nobody answered within [`APPROVAL_WAIT`].
    Unanswered,
    /// The game ended or changed while the join waited.
    GameEnded,
    /// The owner revoked this device's place in this game.
    Revoked,
}

impl Refusal {
    /// The text a client is sent. Nightjar reads these to say them in the user's language
    /// (`nightjar_policy::continue_here::resume_refusal`), so they stay as they are.
    pub fn message(self) -> &'static str {
        match self {
            Self::Full => GUESTS_FULL,
            Self::Denied => JOIN_DENIED,
            Self::Unanswered => JOIN_UNANSWERED,
            Self::GameEnded => GAME_ENDED,
            Self::Revoked => REVOKED,
        }
    }

    /// The HTTP status of the refused launch.
    pub fn status(self) -> u16 {
        match self {
            Self::Full => 503,
            Self::Denied | Self::Revoked => 403,
            Self::Unanswered => 408,
            Self::GameEnded => 409,
        }
    }
}

pub const GUESTS_FULL: &str = "This game has as many guests as it takes";
pub const JOIN_DENIED: &str = "The player did not let you join";
pub const JOIN_UNANSWERED: &str = "Nobody answered your request to join";
pub const GAME_ENDED: &str = "The game ended before you could join";
/// The text a guest's client is sent when its stream ends because the owner kicked it.
pub const KICKED: &str = "The player removed you from the game";
/// When the owner revoked its guest place: it may not join this game again.
pub const REVOKED: &str = "The player does not let you join this game";
/// When its guest time ran out.
pub const EXPIRED: &str = "Your time as a guest in this game ended";
/// When the game the guest joined ended, or another started.
pub const GAME_OVER: &str = "The game you joined ended";

/// What became of a join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// It streams as guest `id` with `permission`.
    Granted {
        id: u32,
        permission: Permission,
    },
    /// It waits for the owner's answer to request `id` ([`Guests::outcome`]).
    Waiting {
        id: u32,
    },
    Refused(Refusal),
}

/// What a guest's stream ended for, for its client and the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Kicked,
    Revoked,
    Expired,
    /// The game ended or another one started.
    GameEnded,
}

impl Ended {
    pub fn message(self) -> &'static str {
        match self {
            Self::Kicked => KICKED,
            Self::Revoked => REVOKED,
            Self::Expired => EXPIRED,
            Self::GameEnded => GAME_OVER,
        }
    }
}

/// No guest, or waiting request, has that id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoSuchGuest;

impl std::fmt::Display for NoSuchGuest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no such guest")
    }
}

impl std::error::Error for NoSuchGuest {}

/// The guests of the running game, and the joins waiting for its owner.
#[derive(Debug, Default)]
pub struct Guests {
    /// The running game's generation, which the guests and requests belong to.
    game: Option<String>,
    owner: Option<String>,
    guests: Vec<Guest>,
    requests: Vec<Request>,
    /// The devices whose place the owner revoked: refused until the game ends.
    revoked: Vec<String>,
    next_id: u32,
    /// Counts every change, so a client polling the list can tell it changed.
    version: u64,
}

impl Guests {
    fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    fn next_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn guests(&self) -> &[Guest] {
        &self.guests
    }

    /// The joins still waiting for an answer, oldest first.
    pub fn waiting(&self) -> impl Iterator<Item = &Request> {
        self.requests.iter().filter(|r| r.answer.is_none())
    }

    /// Follows the running game: `game` is its generation and owner device, or none when no game
    /// runs. A new game, or none, ends every guest place and request; the launches of the guests
    /// that streamed are returned for the caller to stop. A new owner (Continue here) keeps the
    /// guests, and a guest that became the owner is a guest no longer.
    #[must_use = "stop the returned guests' streams"]
    pub fn follow_game(&mut self, game: Option<(&str, &str)>) -> Vec<String> {
        let same_game = match (game, &self.game) {
            (Some((generation, _)), Some(current)) => generation == current,
            (None, None) => true,
            _ => false,
        };
        if !same_game {
            let ended = self.guests.drain(..).filter_map(|g| g.launch).collect();
            self.requests.clear();
            self.revoked.clear();
            self.game = game.map(|(generation, _)| generation.to_owned());
            self.owner = game.map(|(_, owner)| owner.to_owned());
            self.changed();
            return ended;
        }
        let owner = game.map(|(_, owner)| owner);
        if self.owner.as_deref() != owner {
            self.owner = owner.map(str::to_owned);
            if let Some(owner) = owner {
                self.guests.retain(|g| g.device != owner);
                self.requests.retain(|r| r.device != owner);
            }
            self.changed();
        }
        Vec::new()
    }

    /// A device asks to join the running game. `device` must not be its owner. A device with a
    /// guest place gets it again at once (a guest whose stream dropped comes back without asking).
    pub fn admit(
        &mut self,
        device: &str,
        name: &str,
        policy: JoinPolicy,
        permission: Permission,
        now: u64,
        instant: Instant,
    ) -> Admission {
        if self.game.is_none() {
            return Admission::Refused(Refusal::GameEnded);
        }
        if self.revoked.iter().any(|d| d == device) {
            return Admission::Refused(Refusal::Revoked);
        }
        // An expired place is no place: the host's sweep ([`Guests::expire`]) ends its stream.
        if let Some(guest) = self
            .guests
            .iter_mut()
            .find(|g| g.device == device && !g.expired(now))
        {
            if guest.name != name {
                guest.name = name.to_owned();
            }
            return Admission::Granted {
                id: guest.id,
                permission: guest.permission,
            };
        }
        if self.guests.len() >= MAX_GUESTS {
            return Admission::Refused(Refusal::Full);
        }
        match policy {
            // The caller never asks with Off: such a join is no guest. Treat it as Allow.
            JoinPolicy::Allow | JoinPolicy::Off => {
                let id = self.next_id();
                self.grant(id, device, name, permission, None, now);
                Admission::Granted { id, permission }
            }
            JoinPolicy::Ask => {
                self.forget_unanswered(instant);
                if let Some(request) = self
                    .requests
                    .iter_mut()
                    .find(|r| r.device == device && r.answer.is_none())
                {
                    request.name = name.to_owned();
                    return Admission::Waiting { id: request.id };
                }
                if self.waiting().count() >= MAX_REQUESTS {
                    return Admission::Refused(Refusal::Full);
                }
                let id = self.next_id();
                self.requests.push(Request {
                    id,
                    device: device.to_owned(),
                    name: name.to_owned(),
                    asked: instant,
                    answer: None,
                });
                self.changed();
                Admission::Waiting { id }
            }
        }
    }

    fn grant(
        &mut self,
        id: u32,
        device: &str,
        name: &str,
        permission: Permission,
        expires: Option<u64>,
        now: u64,
    ) {
        self.guests.push(Guest {
            id,
            device: device.to_owned(),
            name: name.to_owned(),
            permission,
            since: now,
            expires,
            launch: None,
        });
        self.changed();
    }

    /// Requests nobody answered within [`APPROVAL_WAIT`], and answers nobody collected.
    fn forget_unanswered(&mut self, now: Instant) {
        let before = self.requests.len();
        self.requests
            .retain(|r| now.saturating_duration_since(r.asked) < APPROVAL_WAIT);
        if self.requests.len() != before {
            self.changed();
        }
    }

    /// What became of join request `id`, for the join waiting on it. A final answer is given
    /// once: the request is forgotten then.
    pub fn outcome(&mut self, id: u32, now: Instant) -> Admission {
        let Some(index) = self.requests.iter().position(|r| r.id == id) else {
            // Forgotten: the game changed while it waited.
            return Admission::Refused(Refusal::GameEnded);
        };
        let request = &self.requests[index];
        match request.answer {
            Some(Answer::Approved) => {
                self.requests.remove(index);
                match self.guests.iter().find(|g| g.id == id) {
                    Some(guest) => Admission::Granted {
                        id,
                        permission: guest.permission,
                    },
                    // Approved and revoked before the join saw it.
                    None => Admission::Refused(Refusal::Denied),
                }
            }
            Some(Answer::Denied) => {
                self.requests.remove(index);
                Admission::Refused(Refusal::Denied)
            }
            None if now.saturating_duration_since(request.asked) >= APPROVAL_WAIT => {
                self.requests.remove(index);
                self.changed();
                Admission::Refused(Refusal::Unanswered)
            }
            None => Admission::Waiting { id },
        }
    }

    /// The owner lets request `id` in with `permission`, for `lifetime` or until the game ends.
    /// False when nothing waits under that id, the session is full or the lifetime is out of
    /// bounds.
    pub fn approve(
        &mut self,
        id: u32,
        permission: Permission,
        lifetime: Option<Duration>,
        now: u64,
    ) -> bool {
        if lifetime.is_some_and(|l| !(MIN_LIFETIME..=MAX_LIFETIME).contains(&l))
            || self.guests.len() >= MAX_GUESTS
        {
            return false;
        }
        let Some(request) = self
            .requests
            .iter_mut()
            .find(|r| r.id == id && r.answer.is_none())
        else {
            return false;
        };
        request.answer = Some(Answer::Approved);
        let (device, name) = (request.device.clone(), request.name.clone());
        self.grant(
            id,
            &device,
            &name,
            permission,
            lifetime.map(|l| now + l.as_secs()),
            now,
        );
        true
    }

    /// The owner turns request `id` down.
    pub fn deny(&mut self, id: u32) -> bool {
        let Some(request) = self
            .requests
            .iter_mut()
            .find(|r| r.id == id && r.answer.is_none())
        else {
            return false;
        };
        request.answer = Some(Answer::Denied);
        self.changed();
        true
    }

    /// Guest `id`'s stream started as launch `launch`.
    pub fn joined(&mut self, id: u32, launch: &str) -> bool {
        let Some(guest) = self.guests.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        guest.launch = Some(launch.to_owned());
        self.changed();
        true
    }

    /// Launch `launch`'s stream ended; its guest keeps its place.
    pub fn left(&mut self, launch: &str) {
        let mut changed = false;
        for guest in &mut self.guests {
            if guest.launch.as_deref() == Some(launch) {
                guest.launch = None;
                changed = true;
            }
        }
        if changed {
            self.changed();
        }
    }

    /// The guest streaming as launch `launch`, if one does.
    pub fn of_launch(&self, launch: &str) -> Option<&Guest> {
        self.guests
            .iter()
            .find(|g| g.launch.as_deref() == Some(launch))
    }

    pub fn get(&self, id: u32) -> Option<&Guest> {
        self.guests.iter().find(|g| g.id == id)
    }

    /// Ends guest `id`'s stream and its place: a new join from that device goes through the
    /// join policy again. Returns the launch to stop, or [`NoSuchGuest`].
    pub fn kick(&mut self, id: u32) -> Result<Option<String>, NoSuchGuest> {
        let index = self
            .guests
            .iter()
            .position(|g| g.id == id)
            .ok_or(NoSuchGuest)?;
        let guest = self.guests.remove(index);
        self.changed();
        Ok(guest.launch)
    }

    /// Ends guest `id`'s stream and place, and refuses that device for the rest of this game. A
    /// waiting request by that id is turned down, and its device refused the same way.
    pub fn revoke(&mut self, id: u32) -> Result<Option<String>, NoSuchGuest> {
        if let Some(index) = self.guests.iter().position(|g| g.id == id) {
            let guest = self.guests.remove(index);
            self.revoked.push(guest.device);
            self.changed();
            return Ok(guest.launch);
        }
        let device = self
            .requests
            .iter()
            .find(|r| r.id == id && r.answer.is_none())
            .map(|r| r.device.clone());
        if let Some(device) = device {
            self.deny(id);
            self.revoked.push(device);
            return Ok(None);
        }
        Err(NoSuchGuest)
    }

    /// Lets a revoked device ask again.
    pub fn forgive(&mut self, device: &str) -> bool {
        let before = self.revoked.len();
        self.revoked.retain(|d| d != device);
        let changed = self.revoked.len() != before;
        if changed {
            self.changed();
        }
        changed
    }

    /// The devices refused for this game.
    pub fn revoked(&self) -> &[String] {
        &self.revoked
    }

    /// Changes what guest `id` may do; its stream follows at once.
    pub fn set_permission(&mut self, id: u32, permission: Permission) -> bool {
        let Some(guest) = self.guests.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        if guest.permission != permission {
            guest.permission = permission;
            self.changed();
        }
        true
    }

    /// Forgets the guests whose time ended, returning the launches to stop.
    #[must_use = "stop the returned guests' streams"]
    pub fn expire(&mut self, now: u64) -> Vec<String> {
        let (expired, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.guests)
            .into_iter()
            .partition(|g| g.expired(now));
        self.guests = live;
        if expired.is_empty() {
            return Vec::new();
        }
        self.changed();
        expired.into_iter().filter_map(|g| g.launch).collect()
    }

    /// Whether any guest streams now: input from streams is checked against guest permissions and
    /// the keyboard and mouse holder only then.
    pub fn streaming(&self) -> bool {
        self.guests.iter().any(|g| g.launch.is_some())
    }
}

/// Who sends an input event, for [`Kbm`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The owner's stream, any other stream that is no guest's, or the PC's own keyboard and mouse.
    Owner,
    Guest(u32),
}

/// What happens to a keyboard or mouse event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Its sender holds the keyboard and mouse.
    Accept,
    /// Its sender took the keyboard and mouse over from `from`, whose held keys and buttons the
    /// caller releases (a guest's; the owner's own are left alone).
    Took { from: Source },
    /// Someone else holds them.
    Drop,
}

/// The one holder of the keyboard and mouse while guests stream. The owner, and the PC's own
/// keyboard and mouse, take them back at once by using them; a guest that may use them picks
/// them up once the holder left them alone for [`HANDOFF_IDLE`], or when the owner hands them
/// over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kbm {
    holder: Source,
    /// The holder's last keyboard or mouse event.
    last: Option<Instant>,
}

impl Default for Kbm {
    fn default() -> Self {
        Self {
            holder: Source::Owner,
            last: None,
        }
    }
}

impl Kbm {
    pub fn holder(&self) -> Source {
        self.holder
    }

    /// A keyboard or mouse event from `source` at `now`.
    pub fn input(&mut self, source: Source, now: Instant) -> Verdict {
        if source == self.holder {
            self.last = Some(now);
            return Verdict::Accept;
        }
        let idle = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= HANDOFF_IDLE);
        if source == Source::Owner || idle {
            let from = std::mem::replace(&mut self.holder, source);
            self.last = Some(now);
            return Verdict::Took { from };
        }
        Verdict::Drop
    }

    /// The owner hands the keyboard and mouse to `to` (a guest, or back to itself).
    pub fn give(&mut self, to: Source, now: Instant) -> Option<Source> {
        if to == self.holder {
            return None;
        }
        self.last = Some(now);
        Some(std::mem::replace(&mut self.holder, to))
    }

    /// Guest `id` left or lost its permission: the owner holds the keyboard and mouse again.
    pub fn forget(&mut self, id: u32) {
        if self.holder == Source::Guest(id) {
            self.holder = Source::Owner;
            self.last = None;
        }
    }
}

#[cfg(test)]
mod tests;
