//! A Nucleus session: one game whose instances Nucleus starts together, one
//! slot per instance. A background stream is a session with one private
//! slot; a co-op session has a slot per player, and devices join free slots.
//!
//! Each slot owns a virtual pad by XInput index, which the profile gives the
//! slot's instance alone, so a device's controller reaches only its own
//! instance. A device that drops keeps its slot for [`GRACE`].
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How long a dropped device keeps its slot, and an empty session runs.
pub const GRACE: Duration = Duration::from_secs(120);
/// XInput has four user indices, so at most four players have a pad.
pub const MAX_PLAYERS: usize = 4;
/// The device permission to join someone's co-op session (a paired
/// client's extra field, granted by the administrator like Host control).
pub const DEVICE_KEY: &str = "coop_join";

/// Whether the administrator let this device join co-op sessions.
pub fn may_join(extra: &BTreeMap<String, serde_json::Value>) -> bool {
    extra
        .get(DEVICE_KEY)
        .is_some_and(|v| v == true || v == "true")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// One device streams a game in the background of the PC.
    Background,
    /// Several devices each play their own instance.
    Coop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason", content = "detail")]
pub enum End {
    /// Ended from the console or by the host.
    Ended,
    /// Every player left and none returned within [`GRACE`].
    Empty,
    /// An instance or Nucleus exited while the session ran.
    Crashed(String),
    /// Nucleus did not start the game.
    Failed(String),
}
impl std::fmt::Display for End {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ended => f.write_str("ended"),
            Self::Empty => f.write_str("every player left"),
            Self::Crashed(why) => write!(f, "the game stopped: {why}"),
            Self::Failed(why) => write!(f, "the game did not start: {why}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nucleus is starting the instances.
    Starting,
    Running,
    Ended(End),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    /// The paired device's id.
    pub device: String,
    pub name: String,
    /// Since when the device's stream is gone; None while it streams.
    pub away_since: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The XInput user index of the slot's virtual pad.
    pub pad: u8,
    /// The instance's game window, once found.
    pub window: Option<u64>,
    pub seat: Option<Seat>,
}

/// Why a device may not take a slot. The text is what the client shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    Full { title: String },
    Ended { title: String },
    Private { title: String },
    Kicked { title: String },
    NotPermitted,
}
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full { title } => write!(f, "Every player slot of the {title} co-op session is taken"),
            Self::Ended { title } => write!(f, "The {title} co-op session has ended"),
            Self::Private { title } => write!(
                f,
                "{title} is streaming in the background for another device; it cannot be joined"
            ),
            Self::Kicked { title } => write!(
                f,
                "This device was removed from the {title} co-op session; it can join the next one"
            ),
            Self::NotPermitted => f.write_str(
                "This device may not join co-op sessions: allow Join co-op for it in the web console",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joined {
    /// A free slot.
    Seated(usize),
    /// The device's own slot, which it kept while away.
    Returned(usize),
}
impl Joined {
    pub fn slot(self) -> usize {
        match self {
            Self::Seated(slot) | Self::Returned(slot) => slot,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A device away for longer than [`GRACE`] lost its slot.
    Freed {
        slot: usize,
        device: String,
    },
    Ended(End),
}

/// A player as the console and the native API show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlayerView {
    pub slot: usize,
    pub device: Option<String>,
    pub name: Option<String>,
    pub away: bool,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: u32,
    pub kind: Kind,
    /// Nucleus' GUID of the game.
    pub game: String,
    pub title: String,
    pub slots: Vec<Slot>,
    pub phase: Phase,
    kicked: BTreeSet<String>,
    empty_since: Option<Instant>,
}

impl Session {
    /// A session whose first player, `owner`, takes the first slot. `pads` are the
    /// slots' XInput indices in slot order.
    pub fn new(
        id: u32,
        kind: Kind,
        game: &str,
        title: &str,
        pads: &[u8],
        owner: (&str, &str),
    ) -> Result<Self, String> {
        if pads.is_empty() || pads.len() > MAX_PLAYERS {
            return Err(format!(
                "a session has 1 to {MAX_PLAYERS} players, not {}",
                pads.len()
            ));
        }
        if kind == Kind::Background && pads.len() != 1 {
            return Err("a background stream has one player".into());
        }
        let unique: BTreeSet<_> = pads.iter().collect();
        if unique.len() != pads.len() || pads.iter().any(|&p| usize::from(p) >= MAX_PLAYERS) {
            return Err(format!(
                "each player needs an XInput pad of its own, not {pads:?}"
            ));
        }
        let mut slots: Vec<Slot> = pads
            .iter()
            .map(|&pad| Slot {
                pad,
                window: None,
                seat: None,
            })
            .collect();
        slots[0].seat = Some(Seat {
            device: owner.0.into(),
            name: owner.1.into(),
            away_since: None,
        });
        Ok(Self {
            id,
            kind,
            game: game.into(),
            title: title.into(),
            slots,
            phase: Phase::Starting,
            kicked: BTreeSet::new(),
            empty_since: None,
        })
    }

    pub fn ended(&self) -> bool {
        matches!(self.phase, Phase::Ended(_))
    }

    pub fn slot_of(&self, device: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.seat.as_ref().is_some_and(|seat| seat.device == device))
    }

    /// Take a slot, or return to the device's own. `permitted` is the
    /// device's Join co-op permission; the owner of a slot needs none to
    /// return to it.
    pub fn join(&mut self, device: &str, name: &str, permitted: bool) -> Result<Joined, Refused> {
        let title = self.title.clone();
        if self.ended() {
            return Err(Refused::Ended { title });
        }
        if let Some(slot) = self.slot_of(device) {
            if let Some(seat) = self.slots[slot].seat.as_mut() {
                seat.away_since = None;
                seat.name = name.into();
            }
            self.empty_since = None;
            return Ok(Joined::Returned(slot));
        }
        if self.kind == Kind::Background {
            return Err(Refused::Private { title });
        }
        if self.kicked.contains(device) {
            return Err(Refused::Kicked { title });
        }
        if !permitted {
            return Err(Refused::NotPermitted);
        }
        let Some(slot) = self.slots.iter().position(|s| s.seat.is_none()) else {
            return Err(Refused::Full { title });
        };
        self.slots[slot].seat = Some(Seat {
            device: device.into(),
            name: name.into(),
            away_since: None,
        });
        self.empty_since = None;
        Ok(Joined::Seated(slot))
    }

    /// The device's stream ended; it keeps its slot for [`GRACE`].
    pub fn leave(&mut self, device: &str, now: Instant) {
        if let Some(slot) = self.slot_of(device)
            && let Some(seat) = self.slots[slot].seat.as_mut()
        {
            seat.away_since.get_or_insert(now);
        }
    }

    /// Remove a player; they cannot rejoin this session. Returns the device.
    pub fn kick(&mut self, slot: usize) -> Option<String> {
        let seat = self.slots.get_mut(slot)?.seat.take()?;
        self.kicked.insert(seat.device.clone());
        Some(seat.device)
    }

    /// Nucleus started the instances; `windows` in slot order.
    pub fn running(&mut self, windows: &[Option<u64>]) {
        if self.ended() {
            return;
        }
        for (slot, window) in self.slots.iter_mut().zip(windows) {
            slot.window = *window;
        }
        self.phase = Phase::Running;
    }

    /// End the session; the first reason stays.
    pub fn end(&mut self, why: End) -> bool {
        if self.ended() {
            return false;
        }
        self.phase = Phase::Ended(why);
        true
    }

    /// Free slots of devices away too long; end a session left empty.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        if self.ended() {
            return events;
        }
        // When the last player left, before their slots are freed.
        let last_left = self
            .slots
            .iter()
            .filter_map(|s| s.seat.as_ref()?.away_since)
            .max();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot
                .seat
                .as_ref()
                .and_then(|s| s.away_since)
                .is_some_and(|since| now.duration_since(since) >= GRACE)
            {
                let seat = slot.seat.take().unwrap();
                events.push(Event::Freed {
                    slot: index,
                    device: seat.device,
                });
            }
        }
        let streaming = self.slots.iter().any(|s| {
            s.seat
                .as_ref()
                .is_some_and(|seat| seat.away_since.is_none())
        });
        if streaming {
            self.empty_since = None;
        } else {
            let empty_since = *self.empty_since.get_or_insert(last_left.unwrap_or(now));
            if now.duration_since(empty_since) >= GRACE && self.end(End::Empty) {
                events.push(Event::Ended(End::Empty));
            }
        }
        events
    }

    /// The pad a device's controller input goes to: its slot's, while it
    /// streams. None for every other device and once the session ended.
    pub fn route(&self, device: &str) -> Option<u8> {
        if self.ended() {
            return None;
        }
        let slot = &self.slots[self.slot_of(device)?];
        slot.seat
            .as_ref()
            .filter(|seat| seat.away_since.is_none())
            .map(|_| slot.pad)
    }

    pub fn players(&self) -> Vec<PlayerView> {
        self.slots
            .iter()
            .enumerate()
            .map(|(slot, s)| PlayerView {
                slot,
                device: s.seat.as_ref().map(|seat| seat.device.clone()),
                name: s.seat.as_ref().map(|seat| seat.name.clone()),
                away: s
                    .seat
                    .as_ref()
                    .is_some_and(|seat| seat.away_since.is_some()),
            })
            .collect()
    }
}

/// What a launch does while a session may be running. Nucleus runs one
/// game at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    Start,
    /// Join the running co-op session of the same game.
    Join(u32),
    /// Another session holds Nucleus; the text says which.
    Busy(String),
}

pub fn admit(current: Option<&Session>, game: &str, kind: Kind) -> Admit {
    let Some(session) = current.filter(|s| !s.ended()) else {
        return Admit::Start;
    };
    if kind == Kind::Coop && session.kind == Kind::Coop && session.game == game {
        return Admit::Join(session.id);
    }
    let what = match session.kind {
        Kind::Background => "a background stream",
        Kind::Coop => "a co-op session",
    };
    Admit::Busy(format!(
        "Nucleus Co-op is running {} for {what}; it runs one game at a time. End it in the web console first",
        session.title
    ))
}

/// How many players a new co-op session gets: the handler's maximum, capped
/// by the setting and by the XInput indices the PC's own pads leave free.
pub fn players(handler_max: u32, setting: u32, free_pads: usize) -> Result<usize, String> {
    if free_pads == 0 {
        return Err("All four XInput controller slots are taken by controllers on the PC: disconnect one to stream through Nucleus Co-op".into());
    }
    let count = (handler_max as usize)
        .min(setting.clamp(1, MAX_PLAYERS as u32) as usize)
        .min(free_pads);
    Ok(count.max(1))
}

/// The XInput index a new virtual pad took: the one index connected now
/// that was not before. None when none or several changed.
pub fn new_pad(before: [bool; MAX_PLAYERS], after: [bool; MAX_PLAYERS]) -> Option<u8> {
    let mut added = (0..MAX_PLAYERS).filter(|&i| after[i] && !before[i]);
    let first = added.next()?;
    added.next().is_none().then_some(first as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn coop(pads: &[u8]) -> Session {
        Session::new(1, Kind::Coop, "Racer", "Racer", pads, ("owner", "Phone")).unwrap()
    }

    #[test]
    fn a_session_needs_one_pad_per_player() {
        assert!(Session::new(1, Kind::Coop, "g", "G", &[], ("a", "A")).is_err());
        assert!(Session::new(1, Kind::Coop, "g", "G", &[0, 1, 2, 3, 0], ("a", "A")).is_err());
        assert!(Session::new(1, Kind::Coop, "g", "G", &[1, 1], ("a", "A")).is_err());
        assert!(Session::new(1, Kind::Coop, "g", "G", &[4], ("a", "A")).is_err());
        assert!(Session::new(1, Kind::Background, "g", "G", &[0, 1], ("a", "A")).is_err());
        let s = coop(&[1, 2, 3]);
        assert_eq!(s.slot_of("owner"), Some(0));
        assert_eq!(s.phase, Phase::Starting);
        assert_eq!(s.route("owner"), Some(1));
    }

    #[test]
    fn devices_join_free_slots_and_each_reaches_only_its_own_pad() {
        let mut s = coop(&[1, 2, 3]);
        assert_eq!(s.join("laptop", "Laptop", true), Ok(Joined::Seated(1)));
        assert_eq!(s.join("tv", "TV", true), Ok(Joined::Seated(2)));
        assert_eq!(
            s.join("fourth", "Fourth", true),
            Err(Refused::Full {
                title: "Racer".into()
            })
        );
        assert_eq!(s.route("owner"), Some(1));
        assert_eq!(s.route("laptop"), Some(2));
        assert_eq!(s.route("tv"), Some(3));
        assert_eq!(s.route("stranger"), None);
    }

    #[test]
    fn joining_needs_the_permission_but_returning_does_not() {
        let mut s = coop(&[0, 1]);
        assert_eq!(
            s.join("laptop", "Laptop", false),
            Err(Refused::NotPermitted)
        );
        assert_eq!(s.join("owner", "Phone", false), Ok(Joined::Returned(0)));
        assert!(may_join(
            &serde_json::from_value(json!({"coop_join": true})).unwrap()
        ));
        assert!(may_join(
            &serde_json::from_value(json!({"coop_join": "true"})).unwrap()
        ));
        assert!(!may_join(
            &serde_json::from_value(json!({"coop_join": 1})).unwrap()
        ));
        assert!(!may_join(&BTreeMap::new()));
    }

    #[test]
    fn a_background_stream_is_private() {
        let mut s = Session::new(
            1,
            Kind::Background,
            "Racer",
            "Racer",
            &[0],
            ("owner", "Phone"),
        )
        .unwrap();
        assert!(matches!(
            s.join("laptop", "Laptop", true),
            Err(Refused::Private { .. })
        ));
        assert_eq!(s.join("owner", "Phone", true), Ok(Joined::Returned(0)));
    }

    #[test]
    fn a_dropped_device_keeps_its_slot_for_the_grace_period() {
        let start = Instant::now();
        let mut s = coop(&[0, 1]);
        s.join("laptop", "Laptop", true).unwrap();
        s.leave("laptop", start);
        // Away: no input reaches its pad, and its slot is not free.
        assert_eq!(s.route("laptop"), None);
        assert!(matches!(
            s.join("tv", "TV", true),
            Err(Refused::Full { .. })
        ));
        assert!(s.tick(start + GRACE - Duration::from_secs(1)).is_empty());
        assert_eq!(s.join("laptop", "Laptop", true), Ok(Joined::Returned(1)));
        assert_eq!(s.route("laptop"), Some(1));
        s.leave("laptop", start + Duration::from_secs(10));
        assert_eq!(
            s.tick(start + Duration::from_secs(10) + GRACE),
            vec![Event::Freed {
                slot: 1,
                device: "laptop".into()
            }]
        );
        assert_eq!(s.join("tv", "TV", true), Ok(Joined::Seated(1)));
    }

    #[test]
    fn a_session_everyone_left_ends_after_the_grace_period() {
        let start = Instant::now();
        let mut s = coop(&[0, 1]);
        s.join("laptop", "Laptop", true).unwrap();
        s.leave("owner", start);
        assert!(
            s.tick(start + GRACE).len() == 1,
            "the owner's slot is freed"
        );
        assert!(!s.ended(), "the laptop still plays");
        s.leave("laptop", start + GRACE);
        let later = start + GRACE + GRACE;
        let events = s.tick(later);
        assert!(events.contains(&Event::Ended(End::Empty)), "{events:?}");
        assert_eq!(s.phase, Phase::Ended(End::Empty));
        assert!(s.tick(later + GRACE).is_empty());
        assert_eq!(s.route("laptop"), None);
        assert!(matches!(
            s.join("owner", "Phone", true),
            Err(Refused::Ended { .. })
        ));
    }

    #[test]
    fn a_kicked_device_cannot_rejoin_and_the_first_end_reason_stays() {
        let mut s = coop(&[0, 1]);
        s.join("laptop", "Laptop", true).unwrap();
        assert_eq!(s.kick(1), Some("laptop".into()));
        assert_eq!(s.kick(1), None);
        assert_eq!(s.route("laptop"), None);
        assert!(matches!(
            s.join("laptop", "Laptop", true),
            Err(Refused::Kicked { .. })
        ));
        assert_eq!(s.join("tv", "TV", true), Ok(Joined::Seated(1)));
        s.running(&[Some(10), Some(11)]);
        assert_eq!(s.phase, Phase::Running);
        assert_eq!(s.slots[1].window, Some(11));
        assert!(s.end(End::Crashed("Racer.exe exited".into())));
        assert!(!s.end(End::Ended));
        assert_eq!(
            s.phase,
            Phase::Ended(End::Crashed("Racer.exe exited".into()))
        );
        // A late start report does not revive it.
        s.running(&[Some(12)]);
        assert!(s.ended());
        assert_eq!(
            End::Crashed("Racer.exe exited".into()).to_string(),
            "the game stopped: Racer.exe exited"
        );
    }

    #[test]
    fn nucleus_runs_one_game_and_only_co_op_of_the_same_game_joins() {
        let s = coop(&[0, 1]);
        assert_eq!(admit(None, "Racer", Kind::Coop), Admit::Start);
        assert_eq!(admit(Some(&s), "Racer", Kind::Coop), Admit::Join(1));
        let busy = admit(Some(&s), "Shooter", Kind::Coop);
        assert!(
            matches!(&busy, Admit::Busy(text) if text.starts_with("Nucleus Co-op is running Racer for a co-op session")),
            "{busy:?}"
        );
        assert!(matches!(
            admit(Some(&s), "Racer", Kind::Background),
            Admit::Busy(_)
        ));
        let mut ended = coop(&[0]);
        ended.end(End::Ended);
        assert_eq!(
            admit(Some(&ended), "Shooter", Kind::Background),
            Admit::Start
        );
    }

    #[test]
    fn players_follow_the_handler_the_setting_and_free_pads() {
        assert_eq!(players(4, 4, 4), Ok(4));
        assert_eq!(players(8, 4, 4), Ok(4));
        assert_eq!(players(4, 2, 4), Ok(2));
        assert_eq!(players(4, 4, 3), Ok(3));
        assert_eq!(players(4, 0, 4), Ok(1));
        assert_eq!(players(4, 9, 4), Ok(4));
        assert!(
            players(4, 4, 0)
                .unwrap_err()
                .contains("XInput controller slots")
        );
    }

    #[test]
    fn a_new_pad_is_the_one_index_that_appeared() {
        assert_eq!(
            new_pad([true, false, false, false], [true, true, false, false]),
            Some(1)
        );
        assert_eq!(new_pad([false; 4], [false; 4]), None);
        // A PC controller plugged in at the same moment: unknown, not a guess.
        assert_eq!(new_pad([false; 4], [true, true, false, false]), None);
    }
}
