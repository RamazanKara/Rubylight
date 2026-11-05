//! Continue here: the running game's stream moves to the device that asks for it.
//!
//! `POST /session/continue-here` ends the other devices' plain streams through the same path as
//! their own Disconnect, so the game keeps running, and the caller then starts its stream with
//! the normal resume. Only this explicit press ever displaces a stream; a client's recovery never
//! does. The running game's owner becomes the caller right away, so the displaced device's
//! `/serverinfo` reports no game of its own and even an older client's recovery stops.
//!
//! What stays: Nucleus Co-op and background streams, Input-only and Remote Monitor sessions, the
//! caller's own streams. A stream guests watch is refused, not ended.
//!
//! The displaced device's virtual display stays until the new stream runs, so the monitors do not
//! change twice and a failed start leaves that device able to come back. Once the new stream runs
//! it goes without waiting out the grace period, and Windows moves the game's windows to the
//! display that remains.
//!
//! A press whose stream never comes ends after [`CLAIM_KEPT`], or at once when its launch is
//! refused: the game goes back to its previous owner, still running, nothing says it moved any
//! more, and close-on-disconnect waits for [`Back`] before it counts that device as gone.
use crate::session_control::locked;
use crate::state::{Host, Shared};
use butterpollo_core::{
    native::{Holder, SessionSnapshot},
    native_api::session::{MovedTo, State},
    session_state::Refusal,
    state::Client,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long a displaced device can read where its stream went.
const MOVED_KEPT: Duration = Duration::from_secs(120);
/// How long the pressing device has to reach the host's launch. A second press waits this out,
/// and an unused press gives the game back to its previous owner after it. A launch that is
/// pending holds the press longer, up to the launch's own limit (`session::CLAIM_LIMIT`).
const CLAIM_KEPT: Duration = Duration::from_secs(15);
/// The least a device has to come back after a failed press. Its screen says the game moved and
/// it does not reconnect on its own, so the 3 s of the default disconnect grace would close the
/// game under a person still reading that.
const BACK_AT_LEAST: Duration = Duration::from_secs(60);

/// Why a press changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    Session(Refusal),
    /// Guests watch the stream it would end.
    GuestsWatching,
    /// The caller's own launch of this game would be refused, with this reason.
    Launch(String),
}
impl From<Refusal> for Refused {
    fn from(refusal: Refusal) -> Self {
        Self::Session(refusal)
    }
}

struct Note {
    device: String,
    since_unix_ms: u64,
    at: Instant,
}

/// A press whose stream has not started yet.
#[derive(Clone)]
struct Claim {
    to: String,
    name: String,
    previous_owner: String,
    generation: String,
    until: Instant,
    displaced: BTreeSet<String>,
}

/// A press that failed: its previous owner may still come back to this game.
struct Back {
    generation: String,
    /// `None` when the disconnect grace never ends.
    until: Option<Instant>,
}

/// A press that ended before its stream came. If it still comes, the displays it displaced go
/// as they would have on time.
struct Late {
    to: String,
    generation: String,
    displaced: BTreeSet<String>,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    notes: BTreeMap<String, Note>,
    claim: Option<Claim>,
    /// Displaced devices whose kept display goes now that the new stream runs.
    release: BTreeSet<String>,
    back: Option<Back>,
    late: Option<Late>,
}
impl Inner {
    fn forget_notes(&mut self, displaced: &BTreeSet<String>) {
        for device in displaced {
            self.notes.remove(device);
        }
    }
}

/// What the host remembers of moved streams.
#[derive(Default)]
pub struct Moves(Mutex<Inner>);
impl Moves {
    /// Where `device`'s stream went, while that is still news.
    pub fn note_for(&self, device: &str) -> Option<MovedTo> {
        let mut inner = locked(&self.0);
        inner.notes.retain(|_, note| note.at.elapsed() < MOVED_KEPT);
        inner.notes.get(device).map(|note| MovedTo {
            device: note.device.clone(),
            since_unix_ms: note.since_unix_ms,
        })
    }
    /// `device` starts a stream of its own, or asks for the game: its old move is over.
    pub fn forget(&self, device: &str) {
        let mut inner = locked(&self.0);
        inner.notes.remove(device);
        inner.release.remove(device);
        if let Some(late) = &mut inner.late {
            late.displaced.remove(device);
        }
    }
    fn begin(&self, claim: Claim) {
        let mut inner = locked(&self.0);
        let since_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64);
        for device in &claim.displaced {
            inner.notes.insert(
                device.clone(),
                Note {
                    device: claim.name.clone(),
                    since_unix_ms,
                    at: Instant::now(),
                },
            );
            inner.release.remove(device);
        }
        inner.back = None;
        inner.late = None;
        inner.claim = Some(claim);
    }
    /// The displays to release for moved streams.
    pub fn displays_to_release(&self) -> BTreeSet<String> {
        locked(&self.0).release.clone()
    }
    /// The displays a live press keeps through their grace: they go once its stream runs.
    pub fn displays_held(&self) -> BTreeSet<String> {
        locked(&self.0)
            .claim
            .as_ref()
            .map(|claim| claim.displaced.clone())
            .unwrap_or_default()
    }
    /// These displays are being released.
    pub fn released(&self, devices: &[String]) {
        let mut inner = locked(&self.0);
        for device in devices {
            inner.release.remove(device);
        }
    }
    #[cfg(test)]
    pub fn expire_claim(&self) {
        if let Some(claim) = &mut locked(&self.0).claim {
            claim.until = Instant::now();
        }
    }
    #[cfg(test)]
    pub fn expire_back(&self) {
        if let Some(back) = &mut locked(&self.0).back {
            back.until = Some(Instant::now());
        }
    }
    #[cfg(test)]
    pub fn expire_notes(&self) {
        for note in locked(&self.0).notes.values_mut() {
            note.at = Instant::now() - MOVED_KEPT;
        }
    }
}

/// A stream launch by the device that pressed: refused, it ends the press at once.
pub(crate) struct Attempt<'a> {
    host: &'a Host,
    device: String,
    open: bool,
}
impl<'a> Attempt<'a> {
    /// `stream` is false for a launch that would not start the device's stream of the game.
    pub(crate) fn new(host: &'a Host, device: &str, stream: bool) -> Self {
        Self {
            host,
            device: device.to_owned(),
            open: stream,
        }
    }
    pub(crate) fn succeeded(mut self) {
        self.open = false;
    }
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if self.open {
            self.host.launch_failed(&self.device);
        }
    }
}

impl Host {
    /// A device's name, as it was paired.
    fn device_name(&self, uuid: &str) -> Option<String> {
        self.paired
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clients
            .iter()
            .find(|client| client.uuid == uuid)
            .map(|client| client.name.clone())
    }
    /// The press waiting for its stream. One whose game is gone is over: nothing moved.
    fn live_claim(&self) -> Option<Claim> {
        let generation = locked(&self.current_app)
            .as_ref()
            .map(|app| app.generation.clone());
        let mut inner = locked(&self.moves.0);
        if inner
            .claim
            .as_ref()
            .is_some_and(|claim| Some(&claim.generation) != generation.as_ref())
            && let Some(claim) = inner.claim.take()
        {
            tracing::info!(to = %claim.name, "the game the stream was moving with is gone");
            inner.forget_notes(&claim.displaced);
        }
        inner.claim.clone()
    }
    /// Whether a press waits for its stream.
    pub fn claim_live(&self) -> bool {
        self.live_claim().is_some()
    }
    /// Whether another device than `device` pressed and its stream has not started yet.
    pub(crate) fn claimed_by_other(&self, device: &str) -> bool {
        self.live_claim().is_some_and(|claim| claim.to != device)
    }
    /// Whether the game must outlive having no stream (`streaming` says if it has one): a press
    /// waits for its stream, or the device a failed press took it from may still come back.
    pub fn game_awaited(&self, streaming: bool) -> bool {
        let claimed = self.claim_live();
        let generation = locked(&self.current_app)
            .as_ref()
            .map(|app| app.generation.clone());
        let mut inner = locked(&self.moves.0);
        inner.back = inner.back.take().filter(|back| {
            !streaming
                && Some(&back.generation) == generation.as_ref()
                && back.until.is_none_or(|until| Instant::now() < until)
        });
        claimed || inner.back.is_some()
    }
    /// The device another one can continue the game from: the one pressing Continue here until
    /// its stream runs, else the session's owner while it holds a plain stream or suspended, else
    /// the first plain stream. A Nucleus Co-op or background game has none.
    pub(crate) fn session_holder(&self, snapshot: &SessionSnapshot) -> Option<Holder> {
        if locked(&self.current_app)
            .as_ref()
            .is_none_or(|app| app.background)
        {
            return None;
        }
        if let Some(claim) = self.live_claim() {
            return Some(Holder {
                uuid: claim.to,
                device: claim.name,
            });
        }
        let streams = {
            let sessions = locked(&self.sessions);
            let mut streams = sessions.plain_streams();
            streams.append(&mut sessions.plain_launches());
            streams
        };
        let without_stream = matches!(
            snapshot.state,
            State::Suspended | State::Preparing | State::Launching
        );
        let uuid = snapshot
            .owner
            .clone()
            .filter(|owner| without_stream || streams.contains_key(owner))
            .or_else(|| streams.keys().next().cloned())?;
        let device = streams
            .get(&uuid)
            .cloned()
            .or_else(|| self.device_name(&uuid))?;
        Some(Holder { uuid, device })
    }
    /// The press: see the module's comment. Changes nothing when refused.
    pub fn continue_here(self: &Shared, caller: &Client) -> Result<(), Refused> {
        let _transition = self.transition()?;
        let device = caller.uuid.as_str();
        self.settle_move();
        // Two devices pressing at once: the first one wins until its stream runs. The winner
        // pressing again changes nothing: it must not become its own previous owner.
        match self.live_claim() {
            Some(claim) if claim.to == device => return Ok(()),
            Some(_) => return Err(Refusal::Busy.into()),
            None => {}
        }
        let (state, machine_owner) = self
            .with_session(|machine, _, _| (machine.state(), machine.owner().map(str::to_owned)));
        let another_launches = matches!(state, State::Preparing | State::Launching)
            && machine_owner.as_deref() != Some(device);
        if state == State::Stopping || another_launches {
            return Err(Refusal::Busy.into());
        }
        // A background game belongs to Nucleus Co-op, and a co-op session has no running app.
        let (previous_owner, generation, game, app_id) = locked(&self.current_app)
            .as_ref()
            .filter(|app| !app.background)
            .map(|app| {
                (
                    app.owner.clone(),
                    app.generation.clone(),
                    app.name.clone(),
                    app.id,
                )
            })
            .ok_or(Refusal::NoSession)?;
        // The resume that follows would be refused with the other device's stream already gone.
        crate::nvhttp::check_background_mode(self, caller, app_id)
            .map_err(|error| Refused::Launch(format!("{error:#}")))?;
        let guests = !self.guests.status().guests.is_empty();
        let retained: Vec<String> = locked(&self.app_display).keys().cloned().collect();
        // Who loses something to this press, and so reads where it went: a device with a plain
        // stream or a kept display. The machine's owner may be a co-op or background stream, and
        // the game's owner may have left long ago. No launch can add a stream while this holds
        // the launch lock.
        let mut displaced: BTreeSet<String> = {
            let sessions = locked(&self.sessions);
            if guests && sessions.plain_webrtc_of_other(device) {
                return Err(Refused::GuestsWatching);
            }
            let mut streams = sessions.plain_streams();
            streams.append(&mut sessions.plain_launches());
            streams.into_keys().collect()
        };
        displaced.extend(retained);
        displaced.remove("");
        displaced.remove(device);
        self.moves.forget(device);
        if displaced.is_empty() && previous_owner == device {
            return Ok(());
        }
        let names: Vec<String> = displaced
            .iter()
            .map(|uuid| self.device_name(uuid).unwrap_or_else(|| uuid.clone()))
            .collect();
        tracing::info!(
            from = %names.join(", "),
            to = %caller.name,
            %game,
            "the game's stream moves to the device that asked for it"
        );
        // The note first: a device that loses its stream finds out why, however soon it asks.
        self.moves.begin(Claim {
            to: device.to_owned(),
            name: caller.name.clone(),
            previous_owner,
            generation,
            until: Instant::now() + CLAIM_KEPT,
            displaced,
        });
        if let Some(app) = locked(&self.current_app).as_mut() {
            app.owner = device.to_owned();
        }
        let canceled = locked(&self.sessions).stop_plain_streams_of_others(device);
        drop(canceled);
        self.with_session(|machine, facts, now| machine.disconnected(facts, now));
        Ok(())
    }
    /// Follow a press to its end: the new stream runs, or it never came and the game goes back to
    /// its previous owner. Runs under the launch lock.
    pub fn settle_move(&self) {
        let Some(claim) = self.live_claim() else {
            self.settle_late_arrival();
            return;
        };
        let (arrived, launching) = {
            let sessions = locked(&self.sessions);
            (
                sessions.plain_streams().contains_key(&claim.to),
                sessions.plain_launches().contains_key(&claim.to),
            )
        };
        if arrived {
            self.release_displaced(&claim.displaced);
            locked(&self.moves.0).claim = None;
            return;
        }
        if launching || Instant::now() < claim.until {
            return;
        }
        tracing::warn!(to = %claim.name, "the device that asked for the game never started its stream");
        self.give_back(claim);
    }
    /// The device that pressed had its launch refused: nothing will come of the press. Runs under
    /// the launch lock.
    pub(crate) fn launch_failed(&self, device: &str) {
        let Some(claim) = self.live_claim().filter(|claim| claim.to == device) else {
            return;
        };
        // An earlier launch of its own can still connect, or already did.
        let on_its_way = {
            let sessions = locked(&self.sessions);
            sessions.plain_streams().contains_key(device)
                || sessions.plain_launches().contains_key(device)
        };
        if !on_its_way {
            tracing::warn!(to = %claim.name, "the launch of the device that asked for the game failed");
            self.give_back(claim);
        }
    }
    /// The stream that was pressed for runs, also a late one: these kept displays go now.
    fn release_displaced(&self, displaced: &BTreeSet<String>) {
        let retained = locked(&self.app_display);
        let mut inner = locked(&self.moves.0);
        let kept = displaced
            .iter()
            .filter(|device| retained.contains_key(*device))
            .cloned();
        inner.release.extend(kept);
    }
    /// A press is over without its stream: the game is its previous owner's again, nothing says
    /// it moved, and that device has the disconnect grace to come back. The stream may still
    /// arrive, a little late.
    fn give_back(&self, claim: Claim) {
        if let Some(app) = locked(&self.current_app)
            .as_mut()
            .filter(|app| app.generation == claim.generation && app.owner == claim.to)
        {
            app.owner = claim.previous_owner.clone();
        }
        let exclusive = locked(&self.app_display)
            .get(&claim.previous_owner)
            .is_some_and(|(lease, _)| lease.exclusive_layout());
        let grace = self
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .display_disconnect_delay(exclusive);
        let now = Instant::now();
        let mut inner = locked(&self.moves.0);
        inner.claim = None;
        inner.forget_notes(&claim.displaced);
        inner.back = Some(Back {
            generation: claim.generation.clone(),
            until: grace.map(|grace| now + grace.max(BACK_AT_LEAST)),
        });
        inner.late = Some(Late {
            to: claim.to,
            generation: claim.generation,
            displaced: claim.displaced,
            at: now,
        });
    }
    /// The stream of a press that ended without it comes after all: the displays that press
    /// would have released go, instead of waiting out a grace that can last hours.
    fn settle_late_arrival(&self) {
        let generation = locked(&self.current_app)
            .as_ref()
            .map(|app| app.generation.clone());
        let late = {
            let mut inner = locked(&self.moves.0);
            inner.late = inner.late.take().filter(|late| {
                Some(&late.generation) == generation.as_ref() && late.at.elapsed() < MOVED_KEPT
            });
            inner
                .late
                .as_ref()
                .map(|late| (late.to.clone(), late.displaced.clone()))
        };
        let Some((to, displaced)) = late else {
            return;
        };
        if locked(&self.sessions).plain_streams().contains_key(&to) {
            self.release_displaced(&displaced);
            locked(&self.moves.0).late = None;
        }
    }
}
