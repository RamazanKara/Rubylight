//! Guests from paired devices (docs/multiclient.md): another device joins the game a device plays,
//! with its own stream of the same display, its own controller slot and what its guest permission
//! allows. The policy lives in `butterpollo_session::multiclient`; this module holds its state for
//! the host, makes joins wait for the owner's answer, filters guests' input, ends guests' streams
//! and watches the PC's own keyboard and mouse while a guest holds them.
//!
//! The media need nothing of their own: a guest's launch joins the game's display as Stream here
//! too always did (`display_session::joined_display`), so its capture is the owner's
//! (`stream::CaptureKey`) and its encoder its own, at the codec, size and bitrate it negotiated.
use crate::state::{Host, Launch, Session, Shared};
use butterpollo_session::{
    multiclient::{
        self as policy, APPROVAL_WAIT, Admission, Ended, Guests, JoinPolicy, Kbm, Permission,
        Refusal, Source, Verdict,
    },
    state::{Client, device_uuid},
};
use std::{
    collections::BTreeSet,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How often guests' places, streams and time are looked at.
const SWEEP_EVERY: Duration = Duration::from_secs(1);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Owner-side release state: generation incremented whenever [`Source::Owner`] loses KBM,
/// whether an owner release is still pending delivery, and each owner session's acknowledged
/// generation so every active owner session releases its held keys without keeping the
/// atomic fast path permanently defeated.
#[derive(Default)]
struct OwnerRelease {
    generation: u64,
    pending: bool,
    seen: std::collections::HashMap<String, u64>,
}

/// The host's guests, on [`Host::device_guests`].
#[derive(Default)]
pub struct DeviceGuests {
    state: Mutex<Guests>,
    /// Told when a join request is answered.
    answered: Condvar,
    kbm: Mutex<Kbm>,
    /// Whether any guest streams: the input path looks at guests only then.
    streaming: AtomicBool,
    /// Holders that lost the keyboard and mouse and still hold keys or buttons on the PC.
    release: Mutex<BTreeSet<u32>>,
    owner_release: Mutex<OwnerRelease>,
    releasing: AtomicBool,
    /// The PC's own keyboard and mouse, watched while a guest may hold them.
    local: Mutex<Option<butterpollo_win_io::local_input::Watch>>,
    /// Whether the watcher failed to start, so the log says it once.
    local_failed: AtomicBool,
    /// Each guest's controllers in the pool (`controller_sessions::key` of its launch), to
    /// unplug when it may no longer play.
    pads: Mutex<std::collections::BTreeMap<u32, String>>,
}

/// A join, as the launch request asks it.
pub struct Join<'a> {
    pub client: &'a Client,
    /// The launch asks for the running game: a `/resume`, a Resume or Running Game tile, or a
    /// `/launch` of the game that runs.
    pub joins_game: bool,
    /// A plain stream: no input-only, Remote Monitor, microphone-only or co-op session.
    pub plain: bool,
}

/// What the gate decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Not a guest: no game, the device's own game, or guests are off.
    NotGuest,
    Guest {
        id: u32,
        permission: Permission,
    },
}

/// Starts the sweep that ends guests' streams when the game ends or their time runs out.
pub fn start(h: &Shared) {
    let host = std::sync::Arc::downgrade(h);
    if let Err(error) = std::thread::Builder::new()
        .name("guests".into())
        .spawn(move || sweeper(&host))
    {
        tracing::error!(%error, "no thread for guests; guests' time and the end of their game are checked at their next join");
    }
}

fn sweeper(host: &Weak<Host>) {
    loop {
        std::thread::sleep(SWEEP_EVERY);
        let Some(h) = host.upgrade() else { return };
        if h.stop.load(Ordering::Acquire) {
            return;
        }
        sweep(&h);
    }
}

/// The running game's generation and owner device, when a game of the host's own runs (a
/// background game is Nucleus Co-op's and takes no guests).
fn running_game(h: &Host) -> Option<(String, String)> {
    locked(&h.current_app)
        .as_ref()
        .filter(|app| !app.background)
        .map(|app| (app.generation.clone(), device_uuid(&app.owner).to_owned()))
}

/// Follows the running game, ends the streams of guests whose game or time ended, and forgets
/// the streams that ended by themselves.
pub(crate) fn sweep(h: &Shared) {
    let guests = &h.device_guests;
    let owner_gen_before = locked(&guests.owner_release).generation;
    let game = running_game(h);
    let (live, live_owners): (BTreeSet<String>, BTreeSet<String>) = {
        let sessions = locked(&h.sessions);
        let live = sessions
            .pending
            .keys()
            .cloned()
            .chain(
                sessions
                    .active
                    .values()
                    .filter(|s| !s.stopping())
                    .map(|s| s.launch.id.clone()),
            )
            .collect();
        let live_owners = sessions
            .active
            .values()
            .filter(|s| !s.stopping() && s.launch.guest().is_none())
            .map(|s| s.launch.id.clone())
            .collect();
        (live, live_owners)
    };
    let (game_ended, expired, keyboard_mouse, live_guests, streaming, unplug_pads) = {
        let mut state = locked(&guests.state);
        let game_ended = state.follow_game(
            game.as_ref()
                .map(|(generation, owner)| (generation.as_str(), owner.as_str())),
        );
        let expired = state.expire(unix_now());
        let gone: Vec<(u32, String)> = state
            .guests()
            .iter()
            .filter_map(|g| Some((g.id, g.launch.clone()?)))
            .filter(|(_, launch)| !live.contains(launch))
            .collect();
        for (_, launch) in &gone {
            state.left(launch);
            tracing::info!(launch = %launch, "a guest's stream ended");
        }
        // A view-only guest's controllers, or those of a guest that is gone (kicked, revoked,
        // its time or game over), are unplugged with its stream. A guest that may play and lost
        // its connection keeps its place, its pads parked like any device's.
        // Collect pad keys under lock and unplug outside `guests.state` (blocking up to 2 s).
        let unplug_pads: Vec<(u32, String)> = {
            let mut pads = locked(&guests.pads);
            let unplug: Vec<u32> = gone
                .iter()
                .map(|(id, _)| *id)
                .chain(pads.keys().copied().filter(|id| state.get(*id).is_none()))
                .filter(|id| !state.get(*id).is_some_and(|g| g.permission.gamepads()))
                .collect();
            unplug
                .into_iter()
                .filter_map(|id| pads.remove(&id).map(|key| (id, key)))
                .collect()
        };
        let streaming = state.streaming();
        guests.streaming.store(streaming, Ordering::Release);
        let live_guests: BTreeSet<u32> = state
            .guests()
            .iter()
            .filter(|g| g.launch.is_some())
            .map(|g| g.id)
            .collect();
        let keyboard_mouse: BTreeSet<u32> = state
            .guests()
            .iter()
            .filter(|g| g.launch.is_some() && g.permission.keyboard_mouse())
            .map(|g| g.id)
            .collect();
        (
            game_ended,
            expired,
            keyboard_mouse,
            live_guests,
            streaming,
            unplug_pads,
        )
    };
    guests.reconcile_releases(&live_guests, &live_owners, streaming, owner_gen_before);
    if !game_ended.is_empty() {
        guests.answered.notify_all();
    }
    for launch in &game_ended {
        end_stream(h, launch, Ended::GameEnded);
    }
    for launch in &expired {
        end_stream(h, launch, Ended::Expired);
    }
    // A holder that left or may no longer use them gives them back.
    {
        let mut kbm = locked(&guests.kbm);
        if let Source::Guest(id) = kbm.holder()
            && !keyboard_mouse.contains(&id)
        {
            kbm.forget(id);
        }
    }
    follow_local_input(h, !keyboard_mouse.is_empty());
    for (id, key) in unplug_pads {
        h.controller_sessions.unplug(&key);
        tracing::info!(
            guest = id,
            "a guest's controllers were unplugged with its stream"
        );
    }
}

/// Watches the PC's own keyboard and mouse while a guest may hold them, and only then.
fn follow_local_input(h: &Shared, wanted: bool) {
    let guests = &h.device_guests;
    let mut local = locked(&guests.local);
    if !wanted {
        guests.local_failed.store(false, Ordering::Release);
        // Dropped outside the lock: the watcher's thread is joined.
        let watch = local.take();
        drop(local);
        drop(watch);
        return;
    }
    if local.is_some() || guests.local_failed.load(Ordering::Acquire) {
        return;
    }
    let host = std::sync::Arc::downgrade(h);
    match butterpollo_win_io::local_input::watch(move || {
        if let Some(h) = host.upgrade() {
            h.device_guests.local_input();
        }
    }) {
        Ok(watch) => *local = Some(watch),
        Err(error) => {
            guests.local_failed.store(true, Ordering::Release);
            tracing::warn!(error = %format!("{error:#}"), "the PC's own keyboard and mouse cannot take the keyboard and mouse back from a guest; the owner's device still can");
        }
    }
}

/// Ends launch `launch`'s stream, or its launch still waiting for its client, telling the client
/// why.
fn end_stream(h: &Host, launch: &str, ended: Ended) {
    let canceled = {
        let mut sessions = locked(&h.sessions);
        let canceled = sessions.pending.remove(launch);
        if let Some(session) = sessions.active.get(launch) {
            session.fail(ended.message());
        }
        canceled
    };
    drop(canceled);
    tracing::info!(launch = %launch, reason = ended.message(), "guest's stream ended by the host");
}

impl DeviceGuests {
    /// Decides a join before the launch takes the launch lock: it may wait up to
    /// [`APPROVAL_WAIT`] for the owner's answer, and other launches, the owner's among them, go on
    /// meanwhile.
    pub fn gate(&self, h: &Shared, join: &Join) -> Result<Gate, Refusal> {
        let config = h.config_snapshot();
        // A device paired from an invite only ever joins the game being played, as a guest the
        // player lets in, with its invite's permission, whatever `guest_join` says.
        let invited = crate::invites::invited(join.client);
        let join_policy = if invited.is_some() {
            JoinPolicy::Ask
        } else {
            JoinPolicy::from_config(&config)
        };
        if invited.is_some() && (!join.plain || !join.joins_game) {
            return Err(Refusal::InvitedJoinsOnly);
        }
        // Its time ended: the sweep unpairs it soon, and until then it joins nothing.
        if invited.is_some_and(|guest| guest.ended(unix_now())) {
            return Err(Refusal::Revoked);
        }
        if join_policy == JoinPolicy::Off || !join.plain || !join.joins_game {
            return Ok(Gate::NotGuest);
        }
        let device = device_uuid(&join.client.uuid);
        let Some((generation, owner)) = running_game(h) else {
            return match invited {
                Some(_) => Err(Refusal::InvitedJoinsOnly),
                None => Ok(Gate::NotGuest),
            };
        };
        // The device's own game, and a press of Continue here under way, are no guest's.
        if owner == device || h.claim_live() {
            return match invited {
                Some(_) => Err(Refusal::InvitedJoinsOnly),
                None => Ok(Gate::NotGuest),
            };
        }
        let name = butterpollo_session::webrtc::guests::guest_name(&join.client.name);
        let now = unix_now();
        let (admission, ended, expired) = {
            let mut state = locked(&self.state);
            let ended = state.follow_game(Some((&generation, &owner)));
            let expired = state.expire(now);
            let admission = state.admit(
                device,
                &name,
                join_policy,
                invited.map_or_else(|| policy::default_permission(&config), |g| g.permission),
                now,
                Instant::now(),
            );
            (admission, ended, expired)
        };
        for launch in &ended {
            end_stream(h, launch, Ended::GameEnded);
        }
        for launch in &expired {
            end_stream(h, launch, Ended::Expired);
        }
        let id = match admission {
            Admission::Granted { id, permission } => {
                tracing::info!(client = %join.client.name, guest = id, permission = permission.as_str(), "device joins as a guest");
                return Ok(Gate::Guest { id, permission });
            }
            Admission::Refused(refusal) => {
                tracing::info!(client = %join.client.name, reason = refusal.message(), "guest join refused");
                return Err(refusal);
            }
            Admission::Waiting { id } => id,
        };
        tracing::info!(client = %join.client.name, request = id, "device asks to join the running game; waiting for the player or the console");
        let deadline = Instant::now() + APPROVAL_WAIT + Duration::from_secs(1);
        let mut state = locked(&self.state);
        loop {
            match state.outcome(id, Instant::now()) {
                Admission::Waiting { .. } => {}
                Admission::Granted { id, permission } => {
                    tracing::info!(client = %join.client.name, guest = id, permission = permission.as_str(), "guest let in");
                    return Ok(Gate::Guest { id, permission });
                }
                Admission::Refused(refusal) => {
                    tracing::info!(client = %join.client.name, reason = refusal.message(), "guest join refused");
                    return Err(refusal);
                }
            }
            if Instant::now() >= deadline || h.stop.load(Ordering::Acquire) {
                return Err(Refusal::Unanswered);
            }
            state = self
                .answered
                .wait_timeout(state, Duration::from_millis(500))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Syncs guest state against the running game and returns whether guest `id` still has a live
    /// place.
    pub(crate) fn guest_live(&self, h: &Shared, id: u32) -> bool {
        let game = running_game(h);
        let now = unix_now();
        let (ended, expired, live) = {
            let mut state = locked(&self.state);
            let ended = state.follow_game(
                game.as_ref()
                    .map(|(generation, owner)| (generation.as_str(), owner.as_str())),
            );
            let expired = state.expire(now);
            let live = state.get(id).is_some_and(|g| !g.expired(now));
            (ended, expired, live)
        };
        for launch in &ended {
            end_stream(h, launch, Ended::GameEnded);
        }
        for launch in &expired {
            end_stream(h, launch, Ended::Expired);
        }
        live
    }

    /// Removes and unplugs any controller slot recorded for guest `id`.
    pub(crate) fn unplug_guest_pad(&self, h: &Host, id: u32) {
        if let Some(key) = locked(&self.pads).remove(&id) {
            h.controller_sessions.unplug(&key);
        }
    }

    /// Guest `id`'s launch was queued as `launch`; returns whether the guest is still valid and
    /// attached to the active game.
    pub fn joined(&self, id: u32, launch: &Launch) -> bool {
        let now = unix_now();
        let mut state = locked(&self.state);
        let valid = launch.guest() == Some(id)
            && state.get(id).is_some_and(|g| !g.expired(now))
            && state.joined(id, &launch.id);
        drop(state);
        if valid {
            locked(&self.pads).insert(id, crate::controller_sessions::key(launch));
            self.streaming.store(true, Ordering::Release);
            true
        } else {
            locked(&self.pads).remove(&id);
            false
        }
    }

    /// Whether `launch` may have controllers on the PC: any stream but a view-only guest's,
    /// which gets none plugged in at launch.
    pub fn gamepads_allowed(&self, launch: &Launch) -> bool {
        launch.guest().is_none_or(|id| {
            locked(&self.state)
                .get(id)
                .is_some_and(|g| g.permission.gamepads())
        })
    }

    /// Whether `event` from session `s` reaches the PC: a guest's only with its permission, and
    /// keyboard and mouse input only from their holder. Without guests streaming it costs one
    /// atomic load.
    pub fn admits(&self, s: &Session, required: u32) -> bool {
        if !self.streaming.load(Ordering::Acquire) {
            return true;
        }
        let source = match s.launch.guest() {
            Some(id) => {
                let permission = locked(&self.state).get(id).map(|g| g.permission);
                match permission {
                    Some(permission) if policy::guest_allows(permission, required) => {}
                    // Kicked, revoked, or not allowed this kind of input.
                    _ => return false,
                }
                Source::Guest(id)
            }
            None => Source::Owner,
        };
        if !policy::is_keyboard_mouse(required) {
            return true;
        }
        let verdict = locked(&self.kbm).input(source, Instant::now());
        match verdict {
            Verdict::Accept => true,
            Verdict::Took { from } => {
                tracing::info!(from = ?from, to = ?source, "keyboard and mouse change hands");
                self.release_later(from);
                true
            }
            Verdict::Drop => false,
        }
    }

    /// Whether session `s` may use what reaches past the game: the clipboard and USB forwarding.
    /// A guest needs its keyboard-and-mouse permission for them; every other stream may as its
    /// device may.
    pub fn full_access(&self, s: &Session) -> bool {
        s.launch.guest().is_none_or(|id| {
            locked(&self.state)
                .get(id)
                .is_some_and(|g| g.permission.keyboard_mouse())
        })
    }

    fn release_later(&self, source: Source) {
        match source {
            Source::Guest(id) => {
                let mut release = locked(&self.release);
                release.insert(id);
                self.releasing.store(true, Ordering::Release);
            }
            Source::Owner => {
                let mut owner = locked(&self.owner_release);
                owner.generation = owner.generation.wrapping_add(1).max(1);
                if owner.generation == 1 {
                    owner.seen.clear();
                }
                owner.pending = true;
                self.releasing.store(true, Ordering::Release);
            }
        }
    }

    /// Prunes ended sessions from release tracking and restores the lock-free fast path once
    /// every active owner session has received the release or no guests are streaming.
    fn reconcile_releases(
        &self,
        live_guests: &BTreeSet<u32>,
        live_owners: &BTreeSet<String>,
        streaming: bool,
        owner_gen_before: u64,
    ) {
        let mut release = locked(&self.release);
        release.retain(|id| live_guests.contains(id));
        let mut owner = locked(&self.owner_release);
        owner.seen.retain(|id, _| live_owners.contains(id));
        if owner.pending
            && (!streaming
                || (owner.generation == owner_gen_before
                    && live_owners
                        .iter()
                        .all(|id| owner.seen.get(id).copied() == Some(owner.generation))))
        {
            owner.pending = false;
            owner.seen.clear();
        }
        if release.is_empty() && !owner.pending {
            self.releasing.store(false, Ordering::Release);
        }
    }

    /// Whether session `s` must let go of the keys and buttons it holds: another holder took the
    /// keyboard and mouse. Asked once per control pass and peer; without a pending release it
    /// costs one atomic load.
    pub fn take_release(&self, s: &Session) -> bool {
        if !self.releasing.load(Ordering::Acquire) {
            return false;
        }
        match s.launch.guest() {
            Some(id) => {
                let mut release = locked(&self.release);
                let taken = release.remove(&id);
                if release.is_empty() && !locked(&self.owner_release).pending {
                    self.releasing.store(false, Ordering::Release);
                }
                taken
            }
            None => {
                let mut owner = locked(&self.owner_release);
                if !owner.pending {
                    drop(owner);
                    let release = locked(&self.release);
                    let owner = locked(&self.owner_release);
                    if release.is_empty() && !owner.pending {
                        self.releasing.store(false, Ordering::Release);
                    }
                    return false;
                }
                let current = owner.generation;
                if owner.seen.get(&s.launch.id).copied() == Some(current) {
                    return false;
                }
                owner.seen.insert(s.launch.id.clone(), current);
                if owner.seen.len() > 32 {
                    owner.seen.retain(|_, seen| *seen == current);
                }
                true
            }
        }
    }

    /// Someone used the PC's own keyboard or mouse.
    fn local_input(&self) {
        let verdict = locked(&self.kbm).input(Source::Owner, Instant::now());
        if let Verdict::Took {
            from: Source::Guest(id),
        } = verdict
        {
            tracing::info!(
                guest = id,
                "the PC's own keyboard and mouse took them back from a guest"
            );
            self.release_later(Source::Guest(id));
        }
    }

    /// The guests and the joins waiting, for the console and devices.
    pub fn view(&self, h: &Shared) -> View {
        let config = h.config_snapshot();
        let game = running_game(h);
        let holder = locked(&self.kbm).holder();
        let state = locked(&self.state);
        let owner = game.as_ref().map(|(_, owner)| owner.clone());
        View {
            policy: JoinPolicy::from_config(&config),
            default_permission: policy::default_permission(&config),
            owner_name: owner.as_deref().and_then(|owner| h.device_name(owner)),
            owner,
            max_guests: policy::MAX_GUESTS,
            version: state.version(),
            keyboard_mouse: match holder {
                Source::Guest(id) => Some(id),
                Source::Owner => None,
            },
            guests: state
                .guests()
                .iter()
                .map(|g| GuestView {
                    id: g.id,
                    device: g.device.clone(),
                    name: g.name.clone(),
                    permission: g.permission,
                    streaming: g.launch.is_some(),
                    since: g.since,
                    expires: g.expires,
                })
                .collect(),
            waiting: state
                .waiting()
                .map(|r| WaitingView {
                    id: r.id,
                    name: r.name.clone(),
                    seconds_left: APPROVAL_WAIT.saturating_sub(r.asked.elapsed()).as_secs(),
                })
                .collect(),
        }
    }

    /// Lets join request `id` in.
    pub fn approve(
        &self,
        id: u32,
        permission: Permission,
        lifetime: Option<Duration>,
    ) -> Result<(), ActionError> {
        if lifetime.is_some_and(|l| !(policy::MIN_LIFETIME..=policy::MAX_LIFETIME).contains(&l)) {
            return Err(ActionError::Invalid);
        }
        let approved = locked(&self.state).approve(id, permission, lifetime, unix_now());
        if !approved {
            return Err(ActionError::NotFound);
        }
        self.answered.notify_all();
        tracing::info!(
            request = id,
            permission = permission.as_str(),
            "join approved"
        );
        Ok(())
    }

    pub fn deny(&self, id: u32) -> Result<(), ActionError> {
        if !locked(&self.state).deny(id) {
            return Err(ActionError::NotFound);
        }
        self.answered.notify_all();
        tracing::info!(request = id, "join denied");
        Ok(())
    }

    pub fn kick(&self, h: &Host, id: u32) -> Result<(), ActionError> {
        let launch = locked(&self.state)
            .kick(id)
            .map_err(|_| ActionError::NotFound)?;
        locked(&self.kbm).forget(id);
        tracing::info!(guest = id, "guest kicked");
        if let Some(launch) = launch {
            end_stream(h, &launch, Ended::Kicked);
        }
        Ok(())
    }

    pub fn revoke(&self, h: &Host, id: u32) -> Result<(), ActionError> {
        let launch = locked(&self.state)
            .revoke(id)
            .map_err(|_| ActionError::NotFound)?;
        self.answered.notify_all();
        locked(&self.kbm).forget(id);
        tracing::info!(guest = id, "guest revoked for this game");
        if let Some(launch) = launch {
            end_stream(h, &launch, Ended::Revoked);
        }
        Ok(())
    }

    pub fn set_permission(
        &self,
        h: &Host,
        id: u32,
        permission: Permission,
    ) -> Result<(), ActionError> {
        if !locked(&self.state).set_permission(id, permission) {
            return Err(ActionError::NotFound);
        }
        if !permission.keyboard_mouse() {
            locked(&self.kbm).forget(id);
            self.release_later(Source::Guest(id));
        }
        // View only: its controllers leave the PC now; another permission plugs them in again
        // with the guest's next controller input.
        if !permission.gamepads()
            && let Some(key) = locked(&self.pads).get(&id).cloned()
        {
            h.controller_sessions.unplug(&key);
        }
        tracing::info!(
            guest = id,
            permission = permission.as_str(),
            "guest permission changed"
        );
        Ok(())
    }

    /// Hands the keyboard and mouse to guest `id`, or back to the owner with none.
    pub fn give_keyboard_mouse(&self, id: Option<u32>) -> Result<(), ActionError> {
        let to = match id {
            Some(id) => {
                let state = locked(&self.state);
                let guest = state.get(id).ok_or(ActionError::NotFound)?;
                if !guest.permission.keyboard_mouse() || guest.launch.is_none() {
                    return Err(ActionError::Invalid);
                }
                Source::Guest(id)
            }
            None => Source::Owner,
        };
        if let Some(from) = locked(&self.kbm).give(to, Instant::now()) {
            self.release_later(from);
        }
        tracing::info!(to = ?to, "keyboard and mouse handed over");
        Ok(())
    }

    /// Whether `client` may answer joins and manage guests: the device whose game it is, or one
    /// with Host control.
    pub fn may_manage(&self, h: &Shared, client: &Client) -> bool {
        client.allows_host_control()
            || running_game(h).is_some_and(|(_, owner)| owner == device_uuid(&client.uuid))
    }
}

mod view;
pub use view::{ActionError, GuestView, View, WaitingView};

#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod tests;
