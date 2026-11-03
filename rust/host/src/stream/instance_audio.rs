//! A Nucleus Co-op session's game instances play on a virtual sink from
//! their start (#59). Windows never moves sound a process already opened,
//! and a stream routes its own instance only once the game's window shows:
//! by then a game such as LEGO Batman opened its sound on the PC's
//! speakers. The session tells each pass's instances; a worker of its own
//! routes them, so neither the pass nor its timing waits for Windows. A
//! player whose stream also plays on the PC gets their instance back on the
//! PC's device.
use super::game_audio::{moves_game_sound, routable};
use crate::state::Shared;
use anyhow::Result;
use butterpollo_core::{audio_scope::Identity, config::Config};
use butterpollo_windows::{audio_app_route::AppRoute, nucleus::session::Told};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

/// How long the session's end waits for its instances to go back.
const PUT_BACK_WAIT: Duration = Duration::from_secs(5);

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
pub(crate) fn put_back(directory: &Path, pids: impl FnOnce() -> Result<Vec<u32>>) {
    if let Err(error) = butterpollo_windows::audio_app_route::put_back(directory, pids) {
        tracing::warn!(
            error = format!("{error:#}"),
            "the game's audio device was not put back before it stopped; it is when the game next runs"
        );
    }
}

enum Work {
    Found(Vec<BTreeSet<Identity>>),
    /// A slot's player plays on the PC, or not: what they should know.
    OnPc(usize, bool, mpsc::Sender<Option<&'static str>>),
    /// The instances' processes are about to stop: put back, then answer.
    Stopping(Vec<u32>, mpsc::Sender<()>),
}

/// The session's side: what it tells goes to the worker, which routes.
pub(crate) struct InstanceRoutes {
    work: Mutex<Option<mpsc::Sender<Work>>>,
    /// The last instances sent: an unchanged pass wakes nobody.
    sent: Mutex<Vec<BTreeSet<Identity>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}
impl InstanceRoutes {
    /// `acquire` routes a process (None: never routed); `put_back` puts the
    /// stopping instances back. The first player plays on the PC when
    /// `first_on_pc`.
    pub(crate) fn start<R: Program + Send + 'static>(
        mut acquire: impl FnMut(u32) -> Option<Result<R>> + Send + 'static,
        mut put_back: impl FnMut(&[u32]) + Send + 'static,
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
                while let Ok(mut next) = queue.recv() {
                    // Only the latest instances matter: a pass that came
                    // while Windows answered replaces the ones before.
                    while let Work::Found(_) = next {
                        match queue.try_recv() {
                            Ok(newer) => next = newer,
                            Err(_) => break,
                        }
                    }
                    match next {
                        Work::Found(found) => {
                            trees = found;
                            early.update(&trees, &mut acquire);
                        }
                        Work::OnPc(slot, on_pc, said) => {
                            let message = early.on_pc(slot, on_pc);
                            early.update(&trees, &mut acquire);
                            let _ = said.send(message);
                        }
                        Work::Stopping(pids, done) => {
                            put_back(&pids);
                            early.routed.clear();
                            // A session that stopped waiting has nothing to hear.
                            let _ = done.send(());
                        }
                    }
                }
            })?;
        Ok(Self {
            work: Mutex::new(Some(work)),
            sent: Mutex::new(Vec::new()),
            worker: Mutex::new(Some(worker)),
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
impl Drop for InstanceRoutes {
    fn drop(&mut self) {
        drop(self.work.lock().unwrap().take());
        if let Some(worker) = self.worker.lock().unwrap().take()
            && worker.join().is_err()
        {
            tracing::error!("the Nucleus Co-op audio routes' thread panicked");
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
}
impl<R> Default for Early<R> {
    fn default() -> Self {
        Self {
            routed: BTreeMap::new(),
            on_pc: BTreeSet::new(),
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
        let mut all: BTreeMap<Identity, usize> = BTreeMap::new();
        for (slot, tree) in trees.iter().enumerate() {
            if !self.on_pc.contains(&slot) {
                for identity in tree {
                    all.entry(*identity).or_insert(slot);
                }
            }
        }
        for (identity, &slot) in &all {
            if self.routed.contains_key(identity) {
                continue;
            }
            let route = acquire(identity.pid).and_then(|route| {
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
            move |pids| back_tx.send(pids.to_vec()).unwrap(),
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
            |_| {},
            true,
        )
        .unwrap();
        routes.found(&[BTreeSet::from([id(20)]), BTreeSet::from([id(21)])]);
        assert_eq!(asked.recv_timeout(Duration::from_secs(5)).unwrap(), 21);
        assert_eq!(routes.on_pc(1, true), Some(FROM_NEXT_START));
        drop(routes);
        assert!(asked.try_recv().is_err());
    }
}
