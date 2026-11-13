//! A Nucleus Co-op session's game instances play on a virtual sink from
//! their start (#59): a stream routes its own instance only once the game's
//! window shows, by then a game such as LEGO Batman opened its sound on the
//! PC's speakers. Windows takes a device only for a process that opened its
//! sound, so one that hasn't is routed again, less often over time
//! ([`RETRY`]). The session
//! tells each pass's instances; a worker of its own routes them, so neither
//! the pass nor its timing waits for Windows. A player whose stream also
//! plays on the PC gets their instance back on the PC's device.
use super::game_audio::{moves_game_sound, routable};
use crate::state::Shared;
use anyhow::Result;
use butterpollo_core::{audio_scope::Identity, config::Config};
use butterpollo_windows::{
    audio_app_route::{AppRoute, Closing, no_sound_yet},
    nucleus::session::Told,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

/// How long the session's end waits for its instances to go back.
const PUT_BACK_WAIT: Duration = Duration::from_secs(5);
/// How long a process that hasn't opened its sound waits for each next
/// route, the last one from then on: a game opens its sound early, a helper
/// that never does is tried less often, as each try lists the PC's playback
/// devices under the routes' lock.
pub(super) const RETRY: [Duration; 4] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
/// How long a process may go without opening its sound before that is said:
/// the host may not see the sound of a game it can't move.
pub(super) const SILENT_FOR: Duration = Duration::from_secs(30);

/// A process that hasn't opened its sound yet.
#[derive(Debug)]
struct Wait {
    first: Instant,
    next: Instant,
    tries: usize,
    said: bool,
}

/// The processes that haven't opened their sound yet, and when each is
/// routed again ([`RETRY`]).
#[derive(Debug, Default)]
pub(super) struct Waiting(BTreeMap<Identity, Wait>);
impl Waiting {
    /// Whether `identity` is due for a route at `now`: always, unless it
    /// waits for its next try.
    pub(super) fn due(&self, identity: &Identity, now: Instant) -> bool {
        self.0.get(identity).is_none_or(|wait| now >= wait.next)
    }
    /// `identity` hadn't opened its sound at `now`. True once it has gone
    /// [`SILENT_FOR`] without: time to say so.
    pub(super) fn wait(&mut self, identity: Identity, now: Instant) -> bool {
        let wait = self.0.entry(identity).or_insert(Wait {
            first: now,
            next: now,
            tries: 0,
            said: false,
        });
        wait.next = now + RETRY[wait.tries.min(RETRY.len() - 1)];
        wait.tries += 1;
        let say = !wait.said && now.saturating_duration_since(wait.first) >= SILENT_FOR;
        wait.said |= say;
        say
    }
    /// Whether a process has gone [`SILENT_FOR`] without its sound.
    pub(super) fn said(&self) -> bool {
        self.0.values().any(|wait| wait.said)
    }
    /// Routed, or failed for good: no more tries.
    pub(super) fn done(&mut self, identity: &Identity) {
        self.0.remove(identity);
    }
    /// Only these processes still run.
    pub(super) fn retain(&mut self, running: impl Fn(&Identity) -> bool) {
        self.0.retain(|identity, _| running(identity));
    }
    /// Nothing waits any more.
    fn clear(&mut self) {
        self.0.clear();
    }
    /// How long from `now` until the next try, when one waits.
    pub(super) fn next(&self, now: Instant) -> Option<Duration> {
        self.0
            .values()
            .map(|wait| wait.next.saturating_duration_since(now))
            .min()
    }
}

/// Whether a co-op player's stream of `config` plays the game on the PC
/// too: with its slot's settings (game-only audio), which the launch's
/// config doesn't have yet.
pub(crate) fn plays_on_pc(config: &Config, host_audio: bool) -> bool {
    let mut config = config.clone();
    crate::nucleus::isolate(&mut config);
    !moves_game_sound(&config, host_audio)
}

/// For a Nucleus Co-op session a launch of `config` starts: every instance's
/// processes move to the virtual sink as the session finds them, before the
/// game's window shows and its stream starts looking; the first player's
/// own one stays when `on_pc` ([`plays_on_pc`]). An instance is routed
/// before its player is known: one who plays on the PC gets theirs back
/// ([`InstanceRoutes::on_pc`]). Each stream still routes its own instance,
/// and says when that fails. As the session stops its instances, they go
/// back first.
pub(crate) fn instance_routes(
    h: &Shared,
    config: &Config,
    on_pc: bool,
) -> Option<Arc<InstanceRoutes>> {
    let mut config = config.clone();
    crate::nucleus::isolate(&mut config);
    let directory = h.directory.clone();
    let back = directory.clone();
    let routes = InstanceRoutes::start(
        move |pid| routable(pid).then(|| AppRoute::acquire(&config, &directory, pid)),
        move |pids| put_back(&back, || Ok(pids.to_vec())),
        on_pc,
    );
    match routes {
        Ok(routes) => Some(Arc::new(routes)),
        Err(error) => {
            tracing::warn!(
                error = format!("{error:#}"),
                "the Nucleus Co-op instances' sound moves only once each stream finds its instance"
            );
            None
        }
    }
}

/// The host is about to stop `pids` and every stream playing them: they
/// go back to their own devices while Windows can still set them.
/// Until the guard drops, once they stopped, no stream routes them again.
pub(crate) fn put_back(
    directory: &Path,
    pids: impl FnOnce() -> Result<Vec<u32>>,
) -> Option<Closing> {
    butterpollo_windows::audio_app_route::put_back(directory, pids)
        .inspect_err(|error| {
            tracing::warn!(
                error = format!("{error:#}"),
                "the game's audio device was not put back before it stopped; it is when the game next runs"
            );
        })
        .ok()
}

enum Work {
    Found(Vec<BTreeSet<Identity>>),
    /// A slot's player plays on the PC, or not: what they should know.
    OnPc(usize, bool, mpsc::Sender<Option<&'static str>>),
    /// The instances' processes are about to stop: put back, then answer.
    Stopping(Vec<u32>, mpsc::Sender<()>),
    /// ...and stopped: they may be routed again.
    Stopped,
}

/// The session's side: what it tells goes to the worker, which routes.
pub(crate) struct InstanceRoutes {
    work: Mutex<Option<mpsc::Sender<Work>>>,
    /// The last instances sent: an unchanged pass wakes nobody.
    sent: Mutex<Vec<BTreeSet<Identity>>>,
}
impl InstanceRoutes {
    /// `acquire` routes a process (None: never routed); `put_back` puts the
    /// stopping instances back. The first player plays on the PC when
    /// `first_on_pc`.
    pub(crate) fn start<R: Program + Send + 'static>(
        mut acquire: impl FnMut(u32) -> Option<Result<R>> + Send + 'static,
        mut put_back: impl FnMut(&[u32]) -> Option<Closing> + Send + 'static,
        first_on_pc: bool,
    ) -> Result<Self> {
        let (work, queue) = mpsc::channel::<Work>();
        let worker = thread::Builder::new()
            .name("nucleus audio routes".into())
            .spawn(move || {
                let mut early = Early::<R>::default();
                if first_on_pc {
                    early.on_pc.insert(0);
                }
                let mut trees = Vec::new();
                // Held until the stop is over: no route between.
                let mut _closing = None;
                loop {
                    let first = if let Some(wait) = early.waiting.next(Instant::now()) {
                        match queue.recv_timeout(wait) {
                            Ok(work) => work,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                early.update(&trees, &mut acquire);
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match queue.recv() {
                            Ok(work) => work,
                            Err(_) => break,
                        }
                    };
                    // Only the latest instances matter: a pass that came
                    // while Windows answered replaces the ones before, and
                    // is routed before the work behind it.
                    let (mut next, mut other) = (Some(first), None);
                    let mut found = None;
                    while let Some(work) = next.take() {
                        match work {
                            Work::Found(trees) => {
                                found = Some(trees);
                                next = queue.try_recv().ok();
                            }
                            work => other = Some(work),
                        }
                    }
                    if let Some(found) = found {
                        trees = found;
                        early.update(&trees, &mut acquire);
                    }
                    match other {
                        None | Some(Work::Found(_)) => {}
                        Some(Work::Stopped) => _closing = None,
                        Some(Work::OnPc(slot, on_pc, said)) => {
                            let message = early.on_pc(slot, on_pc);
                            early.update(&trees, &mut acquire);
                            let _ = said.send(message);
                        }
                        Some(Work::Stopping(pids, done)) => {
                            _closing = put_back(&pids);
                            // Nothing is routed again before the session
                            // finds its instances anew: a stopped one's pid
                            // may be another process's by then.
                            early.routed.clear();
                            early.waiting.clear();
                            trees.clear();
                            // A session that stopped waiting has nothing to hear.
                            let _ = done.send(());
                        }
                    }
                }
            })?;
        // Never joined: the last session's handle can drop under the
        // registry's lock, and the worker ends on its own once every handle
        // is gone, after the route it may be waiting for.
        drop(worker);
        Ok(Self {
            work: Mutex::new(Some(work)),
            sent: Mutex::new(Vec::new()),
        })
    }
    fn send(&self, work: Work) -> bool {
        self.work
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|sender| sender.send(work).is_ok())
    }
    /// Slot `slot`'s player plays the game on the PC too (`on_pc`), or only
    /// on the client: their instance goes back to the PC's device, or to the
    /// sink. What they should know about it, if anything.
    pub(crate) fn on_pc(&self, slot: usize, on_pc: bool) -> Option<&'static str> {
        let (said, answer) = mpsc::channel();
        if !self.send(Work::OnPc(slot, on_pc, said)) {
            return None;
        }
        answer.recv_timeout(PUT_BACK_WAIT).unwrap_or_else(|_| {
            tracing::warn!(
                slot,
                on_pc,
                "the Nucleus Co-op instance's audio device did not change within {} s",
                PUT_BACK_WAIT.as_secs()
            );
            None
        })
    }
}
impl Told for InstanceRoutes {
    fn found(&self, trees: &[BTreeSet<Identity>]) {
        let mut sent = self.sent.lock().unwrap();
        if *sent != trees {
            *sent = trees.to_vec();
            self.send(Work::Found(trees.to_vec()));
        }
    }
    fn stopped(&self) {
        // The next pass is routed even when its instances are the same.
        self.sent.lock().unwrap().clear();
        self.send(Work::Stopped);
    }
    fn stopping(&self, pids: &[u32]) {
        let (done, answer) = mpsc::channel();
        if self.send(Work::Stopping(pids.to_vec(), done))
            && answer.recv_timeout(PUT_BACK_WAIT).is_err()
        {
            tracing::warn!(
                "the Nucleus Co-op instances' audio devices were not put back within {} s; they are when the game next runs",
                PUT_BACK_WAIT.as_secs()
            );
        }
    }
}

/// What a co-op player who plays on the PC is told when another player's
/// instance runs the same program.
pub(crate) const SHARED_PROGRAM: &str = "The game plays only on the clients: Windows keeps one playback device per program, and another player's game instance runs the same one.";
/// ...and when their instance opened its sound before they joined.
pub(crate) const FROM_NEXT_START: &str = "The game plays on the PC from its next start: it opened its sound on the virtual playback device before you joined.";

/// A route's program, to tell instances that share one.
pub(crate) trait Program {
    fn program(&self) -> &str;
}
impl Program for AppRoute {
    fn program(&self) -> &str {
        AppRoute::program(self)
    }
}

/// The routes of a session's instances; any route type, for tests.
struct Early<R = AppRoute> {
    /// Each process's slot, and None when not routed (Windows' own program,
    /// or the route failed).
    routed: BTreeMap<Identity, (usize, Option<R>)>,
    /// The slots whose player plays on the PC: never routed.
    on_pc: BTreeSet<usize>,
    /// The processes that haven't opened their sound yet, tried again.
    waiting: Waiting,
}
impl<R> Default for Early<R> {
    fn default() -> Self {
        Self {
            routed: BTreeMap::new(),
            on_pc: BTreeSet::new(),
            waiting: Waiting::default(),
        }
    }
}
impl<R: Program> Early<R> {
    /// Slot `slot` plays on the PC from the next [`update`](Self::update),
    /// or not; what its player should know.
    fn on_pc(&mut self, slot: usize, on_pc: bool) -> Option<&'static str> {
        let changed = if on_pc {
            self.on_pc.insert(slot)
        } else {
            self.on_pc.remove(&slot)
        };
        if !on_pc || !changed {
            return None;
        }
        let routes = |mine: bool| {
            self.routed
                .values()
                .filter(move |(at, _)| (*at == slot) == mine)
                .filter_map(|(_, route)| route.as_ref())
        };
        let shared = routes(true).any(|route| {
            routes(false).any(|other| other.program().eq_ignore_ascii_case(route.program()))
        });
        match routes(true).next() {
            None => None,
            Some(_) if shared => Some(SHARED_PROGRAM),
            Some(_) => Some(FROM_NEXT_START),
        }
    }
}
impl<R> Early<R> {
    /// `acquire`: None for a process never routed.
    fn update(
        &mut self,
        trees: &[BTreeSet<Identity>],
        acquire: &mut dyn FnMut(u32) -> Option<Result<R>>,
    ) {
        self.update_at(trees, acquire, Instant::now());
    }
    /// [`update`](Self::update) at `now`.
    fn update_at(
        &mut self,
        trees: &[BTreeSet<Identity>],
        acquire: &mut dyn FnMut(u32) -> Option<Result<R>>,
        now: Instant,
    ) {
        let mut all: BTreeMap<Identity, usize> = BTreeMap::new();
        for (slot, tree) in trees.iter().enumerate() {
            if !self.on_pc.contains(&slot) {
                for identity in tree {
                    all.entry(*identity).or_insert(slot);
                }
            }
        }
        self.waiting.retain(|identity| all.contains_key(identity));
        for (identity, &slot) in &all {
            if self.routed.contains_key(identity) || !self.waiting.due(identity, now) {
                continue;
            }
            let route = match acquire(identity.pid) {
                // Tried again until it opened its sound.
                Some(Err(error)) if no_sound_yet(&error) => {
                    if self.waiting.wait(*identity, now) {
                        tracing::warn!(
                            pid = identity.pid,
                            "the Nucleus Co-op instance has opened no sound Rubylight can see for {} s: Windows takes its device only once it does",
                            SILENT_FOR.as_secs()
                        );
                    }
                    continue;
                }
                route => route,
            };
            self.waiting.done(identity);
            let route = route.and_then(|route| {
                route
                    .inspect_err(|error| {
                        tracing::warn!(
                            pid = identity.pid,
                            error = format!("{error:#}"),
                            "the Nucleus Co-op instance's sound stays on the PC's playback device until its stream moves it"
                        );
                    })
                    .ok()
            });
            self.routed.insert(*identity, (slot, route));
        }
        self.routed.retain(|identity, _| all.contains_key(identity));
    }
}

#[cfg(test)]
mod tests;
