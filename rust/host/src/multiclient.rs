//! Guests from paired devices (docs/multiclient.md): another device joins the game a device plays,
//! with its own stream of the same display, its own controller slot and what its guest permission
//! allows. The policy lives in `butterpollo_core::multiclient`; this module holds its state for
//! the host, makes joins wait for the owner's answer, filters guests' input, ends guests' streams
//! and watches the PC's own keyboard and mouse while a guest holds them.
//!
//! The media need nothing of their own: a guest's launch joins the game's display as Stream here
//! too always did (`display_session::joined_display`), so its capture is the owner's
//! (`stream::CaptureKey`) and its encoder its own, at the codec, size and bitrate it negotiated.
use crate::state::{Host, Launch, Session, Shared};
use butterpollo_core::{
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

/// The host's guests, on [`Host::device_guests`].
#[derive(Default)]
pub struct DeviceGuests {
    state: Mutex<Guests>,
    /// Told when a join request is answered.
    answered: Condvar,
    kbm: Mutex<Kbm>,
    /// Whether any guest streams: the input path looks at guests only then.
    streaming: AtomicBool,
    /// Guests that lost the keyboard and mouse and still hold keys or buttons on the PC.
    release: Mutex<BTreeSet<u32>>,
    releasing: AtomicBool,
    /// The PC's own keyboard and mouse, watched while a guest may hold them.
    local: Mutex<Option<butterpollo_windows::local_input::Watch>>,
    /// Whether the watcher failed to start, so the log says it once.
    local_failed: AtomicBool,
    /// Each guest's controllers in the pool (`controller_sessions::key` of its launch), to
    /// unplug when it may no longer play.
    pads: Mutex<std::collections::BTreeMap<u32, String>>,
}

/// A join, as the launch request asks it.
pub(crate) struct Join<'a> {
    pub client: &'a Client,
    /// The launch asks for the running game: a `/resume`, a Resume or Running Game tile, or a
    /// `/launch` of the game that runs.
    pub joins_game: bool,
    /// A plain stream: no input-only, Remote Monitor, microphone-only or co-op session.
    pub plain: bool,
}

/// What the gate decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Not a guest: no game, the device's own game, or guests are off.
    NotGuest,
    Guest {
        id: u32,
        permission: Permission,
    },
}

/// Starts the sweep that ends guests' streams when the game ends or their time runs out.
pub(crate) fn start(h: &Shared) {
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
    let game = running_game(h);
    let live: BTreeSet<String> = {
        let sessions = locked(&h.sessions);
        sessions
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
            .collect()
    };
    let guests = &h.device_guests;
    let (game_ended, expired, keyboard_mouse) = {
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
        let unplug: Vec<u32> = gone
            .iter()
            .map(|(id, _)| *id)
            .chain(
                locked(&guests.pads)
                    .keys()
                    .copied()
                    .filter(|id| state.get(*id).is_none()),
            )
            .filter(|id| !state.get(*id).is_some_and(|g| g.permission.gamepads()))
            .collect();
        for id in unplug {
            if let Some(key) = locked(&guests.pads).remove(&id) {
                h.controller_sessions.unplug(&key);
                tracing::info!(
                    guest = id,
                    "a guest's controllers were unplugged with its stream"
                );
            }
        }
        guests.streaming.store(state.streaming(), Ordering::Release);
        let keyboard_mouse: BTreeSet<u32> = state
            .guests()
            .iter()
            .filter(|g| g.launch.is_some() && g.permission.keyboard_mouse())
            .map(|g| g.id)
            .collect();
        (game_ended, expired, keyboard_mouse)
    };
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
}

/// Watches the PC's own keyboard and mouse while a guest may hold them, and only then.
fn follow_local_input(h: &Shared, wanted: bool) {
    let guests = &h.device_guests;
    let mut local = locked(&guests.local);
    if !wanted {
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
    match butterpollo_windows::local_input::watch(move || {
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
    pub(crate) fn gate(&self, h: &Shared, join: &Join) -> Result<Gate, Refusal> {
        let config = h.config_snapshot();
        let join_policy = JoinPolicy::from_config(&config);
        if join_policy == JoinPolicy::Off || !join.plain || !join.joins_game {
            return Ok(Gate::NotGuest);
        }
        let device = device_uuid(&join.client.uuid);
        let Some((generation, owner)) = running_game(h) else {
            return Ok(Gate::NotGuest);
        };
        // The device's own game, and a press of Continue here under way, are no guest's.
        if owner == device || h.claim_live() {
            return Ok(Gate::NotGuest);
        }
        let name = butterpollo_core::webrtc::guests::guest_name(&join.client.name);
        let (admission, ended) = {
            let mut state = locked(&self.state);
            let ended = state.follow_game(Some((&generation, &owner)));
            let admission = state.admit(
                device,
                &name,
                join_policy,
                policy::default_permission(&config),
                unix_now(),
                Instant::now(),
            );
            (admission, ended)
        };
        for launch in &ended {
            end_stream(h, launch, Ended::GameEnded);
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

    /// Guest `id`'s launch was queued as `launch`.
    pub(crate) fn joined(&self, id: u32, launch: &Launch) {
        locked(&self.pads).insert(id, crate::controller_sessions::key(launch));
        let mut state = locked(&self.state);
        if state.joined(id, &launch.id) {
            self.streaming.store(true, Ordering::Release);
        }
    }

    /// Whether `launch` may have controllers on the PC: any stream but a view-only guest's,
    /// which gets none plugged in at launch.
    pub(crate) fn gamepads_allowed(&self, launch: &Launch) -> bool {
        launch.guest().is_none_or(|id| {
            locked(&self.state)
                .get(id)
                .is_some_and(|g| g.permission.gamepads())
        })
    }

    /// Whether `event` from session `s` reaches the PC: a guest's only with its permission, and
    /// keyboard and mouse input only from their holder. Without guests streaming it costs one
    /// atomic load.
    pub(crate) fn admits(&self, s: &Session, required: u32) -> bool {
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
                if let Source::Guest(id) = from {
                    self.release_later(id);
                }
                true
            }
            Verdict::Drop => false,
        }
    }

    /// Whether session `s` may use what reaches past the game: the clipboard and USB forwarding.
    /// A guest needs its keyboard-and-mouse permission for them; every other stream may as its
    /// device may.
    pub(crate) fn full_access(&self, s: &Session) -> bool {
        s.launch.guest().is_none_or(|id| {
            locked(&self.state)
                .get(id)
                .is_some_and(|g| g.permission.keyboard_mouse())
        })
    }

    fn release_later(&self, id: u32) {
        locked(&self.release).insert(id);
        self.releasing.store(true, Ordering::Release);
    }

    /// Whether session `s`, a guest's, must let go of the keys and buttons it holds: another holder
    /// took the keyboard and mouse. Asked once per control pass and peer; without such a guest it
    /// costs one atomic load.
    pub(crate) fn take_release(&self, s: &Session) -> bool {
        if !self.releasing.load(Ordering::Acquire) {
            return false;
        }
        let Some(id) = s.launch.guest() else {
            return false;
        };
        let mut release = locked(&self.release);
        let taken = release.remove(&id);
        if release.is_empty() {
            self.releasing.store(false, Ordering::Release);
        }
        taken
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
            self.release_later(id);
        }
    }

    /// The guests and the joins waiting, for the console and devices.
    pub(crate) fn view(&self, h: &Shared) -> View {
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
    pub(crate) fn approve(
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

    pub(crate) fn deny(&self, id: u32) -> Result<(), ActionError> {
        if !locked(&self.state).deny(id) {
            return Err(ActionError::NotFound);
        }
        self.answered.notify_all();
        tracing::info!(request = id, "join denied");
        Ok(())
    }

    pub(crate) fn kick(&self, h: &Host, id: u32) -> Result<(), ActionError> {
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

    pub(crate) fn revoke(&self, h: &Host, id: u32) -> Result<(), ActionError> {
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

    pub(crate) fn set_permission(
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
            self.release_later(id);
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
    pub(crate) fn give_keyboard_mouse(&self, id: Option<u32>) -> Result<(), ActionError> {
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
        if let Some(Source::Guest(from)) = locked(&self.kbm).give(to, Instant::now()) {
            self.release_later(from);
        }
        tracing::info!(to = ?to, "keyboard and mouse handed over");
        Ok(())
    }

    /// Whether `client` may answer joins and manage guests: the device whose game it is, or one
    /// with Host control.
    pub(crate) fn may_manage(&self, h: &Shared, client: &Client) -> bool {
        client.allows_host_control()
            || running_game(h).is_some_and(|(_, owner)| owner == device_uuid(&client.uuid))
    }
}

/// Why a guest action failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionError {
    /// No such guest or waiting join.
    NotFound,
    /// A value the host does not take.
    Invalid,
}

/// The guests and the joins waiting, as the console and devices see them.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct View {
    #[serde(serialize_with = "policy_text")]
    pub policy: JoinPolicy,
    #[serde(serialize_with = "permission_text")]
    pub default_permission: Permission,
    /// The device whose game it is, when a game runs.
    pub owner: Option<String>,
    pub owner_name: Option<String>,
    pub max_guests: usize,
    pub version: u64,
    /// The guest holding the keyboard and mouse; none when the owner does.
    pub keyboard_mouse: Option<u32>,
    pub guests: Vec<GuestView>,
    pub waiting: Vec<WaitingView>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct GuestView {
    pub id: u32,
    /// The guest device's uuid, to tell a device which guest it is; never sent.
    #[serde(skip)]
    pub device: String,
    pub name: String,
    #[serde(serialize_with = "permission_text")]
    pub permission: Permission,
    pub streaming: bool,
    /// Unix seconds.
    pub since: u64,
    pub expires: Option<u64>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct WaitingView {
    pub id: u32,
    pub name: String,
    pub seconds_left: u64,
}

fn policy_text<S: serde::Serializer>(policy: &JoinPolicy, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(policy.as_str())
}

fn permission_text<S: serde::Serializer>(permission: &Permission, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(permission.as_str())
}
