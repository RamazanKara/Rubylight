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
use crate::session_control::locked;
use crate::state::Host;
use butterpollo_core::{
    native::{Holder, SessionSnapshot},
    native_api::session::{MovedTo, State},
    session_state::Refusal,
    state::Client,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long a displaced device can read where its stream went.
const MOVED_KEPT: Duration = Duration::from_secs(120);
/// How long the pressing device has to reach the host's launch. A second press waits this out,
/// and an unused press gives the game back to its previous owner after it.
const CLAIM_KEPT: Duration = Duration::from_secs(15);

/// Why a press changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Session(Refusal),
    /// Guests watch the stream it would end.
    GuestsWatching,
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

#[derive(Default)]
struct Inner {
    notes: BTreeMap<String, Note>,
    claim: Option<Claim>,
    /// Displaced devices whose kept display goes now that the new stream runs.
    release: BTreeSet<String>,
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
    }
    /// Whether the game must outlive having no stream: a press waits for its stream.
    pub fn claim_live(&self) -> bool {
        locked(&self.0).claim.is_some()
    }
    fn claim(&self) -> Option<Claim> {
        locked(&self.0).claim.clone()
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
        inner.claim = Some(claim);
    }
    /// The displays to release for moved streams.
    pub fn displays_to_release(&self) -> BTreeSet<String> {
        locked(&self.0).release.clone()
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
    pub fn expire_notes(&self) {
        for note in locked(&self.0).notes.values_mut() {
            note.at = Instant::now() - MOVED_KEPT;
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
        if let Some(claim) = self.moves.claim() {
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
    pub fn continue_here(&self, caller: &Client) -> Result<(), Refused> {
        let _transition = self.transition()?;
        let device = caller.uuid.as_str();
        self.settle_move();
        // Two devices pressing at once: the first one wins until its stream runs.
        if self.moves.claim().is_some_and(|claim| claim.to != device) {
            return Err(Refusal::Busy.into());
        }
        let (state, machine_owner) = self
            .with_session(|machine, _, _| (machine.state(), machine.owner().map(str::to_owned)));
        let another_launches = matches!(state, State::Preparing | State::Launching)
            && machine_owner.as_deref() != Some(device);
        if state == State::Stopping || another_launches {
            return Err(Refusal::Busy.into());
        }
        // A background game belongs to Nucleus Co-op, and a co-op session has no running app.
        let (previous_owner, generation, game) = locked(&self.current_app)
            .as_ref()
            .filter(|app| !app.background)
            .map(|app| (app.owner.clone(), app.generation.clone(), app.name.clone()))
            .ok_or(Refusal::NoSession)?;
        let guests = !self.guests.status().guests.is_empty();
        let retained: Vec<String> = locked(&self.app_display).keys().cloned().collect();
        // No launch can add a stream while this holds the launch lock.
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
        displaced.extend(machine_owner);
        displaced.insert(previous_owner.clone());
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
        let Some(claim) = self.moves.claim() else {
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
            let retained = locked(&self.app_display);
            let mut inner = locked(&self.moves.0);
            let kept = claim
                .displaced
                .iter()
                .filter(|device| retained.contains_key(*device))
                .cloned();
            inner.release.extend(kept);
            inner.claim = None;
            return;
        }
        if launching || Instant::now() < claim.until {
            return;
        }
        tracing::warn!(to = %claim.name, "the device that asked for the game never started its stream");
        if let Some(app) = locked(&self.current_app)
            .as_mut()
            .filter(|app| app.generation == claim.generation && app.owner == claim.to)
        {
            app.owner = claim.previous_owner.clone();
        }
        locked(&self.moves.0).claim = None;
    }
}
