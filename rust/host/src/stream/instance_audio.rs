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
mod tests {
    use super::*;

    fn id(pid: u32) -> Identity {
        Identity { pid, started: 1 }
    }

    /// A route of a test's program.
    #[derive(Debug, PartialEq)]
    struct Route(u32, &'static str);
    impl Program for Route {
        fn program(&self) -> &str {
            self.1
        }
    }
    fn game(pid: u32) -> Option<Result<Route>> {
        Some(Ok(Route(
            pid,
            r"c:\nucleus\content\game\instance0\game.exe",
        )))
    }

    /// A co-op launch plays game-only audio whatever the host's default: its
    /// instance moves unless the stream plays on the PC.
    #[test]
    fn a_coop_player_plays_on_the_pc_only_when_they_ask() {
        assert!(!plays_on_pc(&Config::default(), false));
        assert!(plays_on_pc(&Config::default(), true));
        let muted = Config::parse("stream_audio = false\n").unwrap();
        assert!(plays_on_pc(&muted, false));
    }

    /// #59: an instance moves in the pass that finds it, before its stream
    /// looks for it; a process it starts moves with it, a failed route is
    /// not retried each pass, and a gone process lets go.
    #[test]
    fn a_session_moves_its_instances_as_it_finds_them() {
        let mut early = Early::<u32>::default();
        let mut asked = Vec::new();
        fn pass(early: &mut Early<u32>, trees: &[BTreeSet<Identity>], asked: &mut Vec<u32>) {
            early.update(trees, &mut |pid| {
                asked.push(pid);
                match pid {
                    // Windows' own program.
                    30 => None,
                    31 => Some(Err(anyhow::anyhow!("no virtual playback device"))),
                    pid => Some(Ok(pid)),
                }
            })
        }
        pass(&mut early, &[BTreeSet::new()], &mut asked);
        assert!(early.routed.is_empty());
        pass(&mut early, &[BTreeSet::from([id(20)])], &mut asked);
        assert_eq!(early.routed[&id(20)], (0, Some(20)));
        let tree = BTreeSet::from([id(20), id(21), id(30), id(31)]);
        pass(&mut early, std::slice::from_ref(&tree), &mut asked);
        pass(&mut early, &[tree], &mut asked);
        assert_eq!(asked, [20, 21, 30, 31]);
        assert_eq!(early.routed[&id(21)], (0, Some(21)));
        assert_eq!(early.routed[&id(30)], (0, None));
        assert_eq!(early.routed[&id(31)], (0, None));
        pass(&mut early, &[BTreeSet::from([id(21)])], &mut asked);
        assert_eq!(early.routed.keys().copied().collect::<Vec<_>>(), [id(21)]);
    }

    /// The session's pass never waits for Windows: a route that takes long
    /// runs on the routes' own thread, which then routes only the latest
    /// instances. The session's end waits until they went back.
    #[test]
    fn the_session_pass_never_waits_for_a_route() {
        let (release, blocked) = mpsc::channel::<()>();
        let blocked = Mutex::new(blocked);
        let (asked_tx, asked) = mpsc::channel();
        let (back_tx, back) = mpsc::channel();
        let routes = InstanceRoutes::start(
            move |pid| {
                asked_tx.send(pid).unwrap();
                if pid == 20 {
                    blocked.lock().unwrap().recv().unwrap();
                }
                game(pid)
            },
            move |pids| {
                back_tx.send(pids.to_vec()).unwrap();
                None
            },
            false,
        )
        .unwrap();
        let started = std::time::Instant::now();
        routes.found(&[BTreeSet::from([id(20)])]);
        assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 20);
        // While pid 20's route hangs, the passes go on.
        routes.found(&[BTreeSet::from([id(20), id(21)])]);
        routes.found(&[BTreeSet::from([id(20), id(22)])]);
        routes.found(&[BTreeSet::from([id(20), id(22)])]);
        assert!(started.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
        assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 22);
        routes.stopping(&[20, 22]);
        assert_eq!(back.try_recv().unwrap(), [20, 22]);
        drop(routes);
        assert!(asked.try_recv().is_err(), "pid 21 was gone before its turn");
    }

    /// Each co-op player's choice holds: an instance is routed before its
    /// player is known, and goes back to the PC for one who plays there
    /// (told it does from the game's next start, as its sound opened on the
    /// sink); one not started yet just stays. A player who doesn't plays on
    /// the sink again.
    #[test]
    fn a_player_who_plays_on_the_pc_gets_their_instance_back() {
        let mut early = Early::<Route>::default();
        let (one, two) = (BTreeSet::from([id(20)]), BTreeSet::from([id(21)]));
        let trees = [one, two.clone()];
        let other = |pid| {
            Some(Ok(Route(
                pid,
                r"c:\nucleus\content\game\instance1\game.exe",
            )))
        };
        let mut acquire = |pid| if pid == 20 { game(pid) } else { other(pid) };
        early.update(&trees, &mut acquire);
        assert_eq!(early.routed.len(), 2);
        assert_eq!(early.on_pc(1, true), Some(FROM_NEXT_START));
        early.update(&trees, &mut acquire);
        assert_eq!(early.routed.keys().copied().collect::<Vec<_>>(), [id(20)]);
        assert_eq!(early.on_pc(1, true), None, "said once");
        assert_eq!(early.on_pc(2, true), None, "not started: plays on the PC");
        assert_eq!(early.on_pc(1, false), None);
        early.update(&trees, &mut acquire);
        assert_eq!(early.routed[&id(21)].0, 1);
        // One program for every instance: Windows can't tell them apart.
        let mut shared = Early::<Route>::default();
        shared.update(&trees, &mut game);
        assert_eq!(shared.on_pc(1, true), Some(SHARED_PROGRAM));
    }

    /// The first player's choice is known before the instances start: their
    /// own instance is never routed.
    #[test]
    fn the_first_player_who_plays_on_the_pc_is_never_routed() {
        let (asked_tx, asked) = mpsc::channel();
        let routes = InstanceRoutes::start(
            move |pid| {
                asked_tx.send(pid).unwrap();
                game(pid)
            },
            |_| None,
            true,
        )
        .unwrap();
        routes.found(&[BTreeSet::from([id(20)]), BTreeSet::from([id(21)])]);
        assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 21);
        assert_eq!(routes.on_pc(1, true), Some(FROM_NEXT_START));
        drop(routes);
        assert!(asked.try_recv().is_err());
    }

    /// A pass's instances that came just before a player's choice are
    /// routed first, not skipped: the session never sends them again.
    #[test]
    fn instances_just_before_a_choice_are_routed() {
        let (hold, held) = mpsc::channel::<()>();
        let (asked_tx, asked) = mpsc::channel();
        let routes = InstanceRoutes::start(
            move |pid| {
                asked_tx.send(pid).unwrap();
                if pid == 20 {
                    held.recv().unwrap();
                }
                game(pid)
            },
            |_| None,
            false,
        )
        .unwrap();
        routes.found(&[BTreeSet::from([id(20)])]);
        assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 20);
        routes.found(&[BTreeSet::from([id(20)]), BTreeSet::from([id(21)])]);
        let choice = std::thread::scope(|scope| {
            let choice = scope.spawn(|| routes.on_pc(0, false));
            // The choice is queued behind the pass before the route ends.
            std::thread::sleep(Duration::from_millis(50));
            hold.send(()).unwrap();
            choice.join().unwrap()
        });
        assert_eq!(choice, None);
        assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 21);
    }

    fn silent() -> Option<Result<Route>> {
        Some(Err(butterpollo_windows::audio_app_route::NoSoundYet.into()))
    }

    /// #59: an instance routed as it starts hasn't opened its sound, which
    /// Windows refuses: it is routed again on its own, with no new pass and
    /// no warning, until it has.
    #[test]
    fn an_instance_without_sound_yet_is_routed_again_until_it_opened_it() {
        let (asked_tx, asked) = mpsc::channel();
        let opened = Arc::new(Mutex::new(false));
        let open = opened.clone();
        let routes = InstanceRoutes::start(
            move |pid| {
                let route = if *open.lock().unwrap() {
                    game(pid)
                } else {
                    silent()
                };
                asked_tx.send(pid).unwrap();
                route
            },
            |_| None,
            false,
        )
        .unwrap();
        routes.found(&[BTreeSet::from([id(20)])]);
        let wait = RETRY[RETRY.len() - 1] + Duration::from_secs(1);
        assert_eq!(asked.recv_timeout(wait).unwrap(), 20);
        assert_eq!(asked.recv_timeout(wait).unwrap(), 20, "tried again");
        *opened.lock().unwrap() = true;
        assert_eq!(asked.recv_timeout(wait).unwrap(), 20, "routed");
        let after = asked.recv_timeout(wait);
        assert!(after.is_err(), "routed: not tried again ({after:?})");
    }

    /// The session's stop ends the tries: a stopping instance is put back
    /// and never routed again, nor after it stopped (its pid may be another
    /// process's by then), until the session finds its instances anew; and
    /// none once the handles dropped.
    #[test]
    fn the_session_stop_ends_the_tries() {
        let (asked_tx, asked) = mpsc::channel();
        let routes = InstanceRoutes::start(
            move |pid| {
                asked_tx.send(pid).unwrap();
                silent()
            },
            |_| None,
            false,
        )
        .unwrap();
        let instances = [BTreeSet::from([id(20)])];
        routes.found(&instances);
        let wait = RETRY[2] + Duration::from_millis(500);
        assert_eq!(asked.recv_timeout(wait).unwrap(), 20);
        routes.stopping(&[20]);
        // A try may have been under way as the stop came.
        let _ = asked.try_recv();
        assert!(asked.recv_timeout(wait).is_err(), "no try while stopping");
        routes.stopped();
        assert!(asked.recv_timeout(wait).is_err(), "no try once stopped");
        routes.found(&instances);
        assert_eq!(asked.recv_timeout(wait).unwrap(), 20, "found anew");
        drop(routes);
        let _ = asked.try_recv();
        assert!(asked.recv_timeout(wait).is_err(), "no try once dropped");
    }

    /// A process that never opens its sound (a launcher's helper) is tried
    /// less and less often ([`RETRY`]): each try lists the PC's playback
    /// devices. After [`SILENT_FOR`] that is said, once. One that opens it
    /// is routed at its next try, and a gone one waits for nothing.
    #[test]
    fn a_process_that_never_opens_its_sound_is_tried_less_often() {
        let mut early = Early::<u32>::default();
        let tree = [BTreeSet::from([id(20)])];
        let t0 = Instant::now();
        let mut tries = Vec::new();
        let mut said = Vec::new();
        let opened = false;
        let mut at = Duration::ZERO;
        while at < Duration::from_secs(60) {
            let now = t0 + at;
            early.update_at(
                &tree,
                &mut |pid| {
                    tries.push(at.as_millis());
                    if opened {
                        Some(Ok(pid))
                    } else {
                        Some(Err(butterpollo_windows::audio_app_route::NoSoundYet.into()))
                    }
                },
                now,
            );
            if early.waiting.said() && said.is_empty() {
                said.push(at.as_millis());
            }
            at += Duration::from_millis(100);
        }
        let mut expected = vec![0, 500, 1500, 3500];
        while *expected.last().unwrap() + 5000 < 60_000 {
            expected.push(expected.last().unwrap() + 5000);
        }
        assert_eq!(tries, expected, "0.5, 1, 2, then every 5 s");
        assert_eq!(said, [33_500], "the first try past 30 s says it");
        let now = t0 + Duration::from_secs(70);
        assert_eq!(early.waiting.next(now), Some(Duration::ZERO), "due");
        let opened = true;
        early.update_at(
            &tree,
            &mut |pid| {
                assert!(opened);
                Some(Ok(pid))
            },
            now,
        );
        assert_eq!(early.routed[&id(20)], (0, Some(20)));
        assert_eq!(early.waiting.next(now), None);
        let mut gone = Early::<u32>::default();
        gone.update_at(
            &tree,
            &mut |_| Some(Err(butterpollo_windows::audio_app_route::NoSoundYet.into())),
            t0,
        );
        assert!(gone.waiting.next(t0).is_some());
        gone.update_at(&[BTreeSet::new()], &mut |_| unreachable!(), t0);
        assert_eq!(gone.waiting.next(t0), None);
    }

    /// Dropping the last handle never waits for a route in progress.
    #[test]
    fn dropping_the_routes_never_waits_for_windows() {
        let (hold, held) = mpsc::channel::<()>();
        let (asked_tx, asked) = mpsc::channel();
        let routes = InstanceRoutes::start(
            move |pid| {
                asked_tx.send(pid).unwrap();
                held.recv().unwrap();
                game(pid)
            },
            |_| None,
            false,
        )
        .unwrap();
        routes.found(&[BTreeSet::from([id(20)])]);
        asked.recv_timeout(Duration::from_secs(5)).unwrap();
        let dropped = std::time::Instant::now();
        drop(routes);
        assert!(dropped.elapsed() < Duration::from_secs(1));
        hold.send(()).unwrap();
    }
}
