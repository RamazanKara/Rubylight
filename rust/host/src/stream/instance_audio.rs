//! A Nucleus Co-op session's game instances play on a virtual sink from
//! their start (#59). Windows never moves sound a process already opened,
//! and a stream routes its own instance only once the game's window shows:
//! by then a game such as LEGO Batman opened its sound on the PC's
//! speakers. The session tells each pass's instances; a worker of its own
//! routes them, so neither the pass nor its timing waits for Windows.
use super::game_audio::{moves_game_sound, routable};
use crate::state::Shared;
use anyhow::Result;
use butterpollo_core::{audio_scope::Identity, config::Config};
use butterpollo_windows::{
    audio_app_route::AppRoute,
    nucleus::session::{Instances, Told},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

/// How long the session's end waits for its instances to go back.
const PUT_BACK_WAIT: Duration = Duration::from_secs(5);

/// For a Nucleus Co-op session a launch of `config` starts: every instance's
/// processes move to the virtual sink as the session finds them, before the
/// game's window shows and its stream starts looking. Each stream still
/// routes its own instance, and says when that fails. As the session stops
/// its instances, they go back first.
pub(crate) fn instance_routes(h: &Shared, config: &Config, host_audio: bool) -> Option<Instances> {
    // As the session's streams play: with a co-op slot's settings, which
    // the launch's config doesn't have yet.
    let mut config = config.clone();
    crate::nucleus::isolate(&mut config);
    if !moves_game_sound(&config, host_audio) {
        return None;
    }
    let directory = h.directory.clone();
    let back = directory.clone();
    let routes = InstanceRoutes::start(
        move |pid| routable(pid).then(|| AppRoute::acquire(&config, &directory, pid)),
        move |pids| put_back(&back, || Ok(pids.to_vec())),
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
    /// The instances' processes are about to stop: put back, then answer.
    Stopping(Vec<u32>, mpsc::Sender<()>),
}

/// The session's side: what it tells goes to the worker, which routes.
struct InstanceRoutes {
    work: Mutex<Option<mpsc::Sender<Work>>>,
    /// The last instances sent: an unchanged pass wakes nobody.
    sent: Mutex<Vec<BTreeSet<Identity>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}
impl InstanceRoutes {
    /// `acquire` routes a process (None: never routed); `put_back` puts the
    /// stopping instances back.
    fn start<R: Send + 'static>(
        mut acquire: impl FnMut(u32) -> Option<Result<R>> + Send + 'static,
        mut put_back: impl FnMut(&[u32]) + Send + 'static,
    ) -> Result<Self> {
        let (work, queue) = mpsc::channel::<Work>();
        let worker = thread::Builder::new()
            .name("nucleus audio routes".into())
            .spawn(move || {
                let mut early = Early::<R>::default();
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
                        Work::Found(trees) => early.update(&trees, &mut acquire),
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

/// The routes of a session's instances; any route type, for tests.
struct Early<R = AppRoute> {
    /// None: not routed (Windows' own program, or the route failed).
    routed: BTreeMap<Identity, Option<R>>,
}
impl<R> Default for Early<R> {
    fn default() -> Self {
        Self {
            routed: BTreeMap::new(),
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
        let all: BTreeSet<Identity> = trees.iter().flatten().copied().collect();
        for identity in &all {
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
            self.routed.insert(*identity, route);
        }
        self.routed.retain(|identity, _| all.contains(identity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    fn id(pid: u32) -> Identity {
        Identity { pid, started: 1 }
    }

    /// A co-op launch plays game-only audio whatever the host's default:
    /// its session moves the instances unless the stream plays on the PC.
    #[test]
    fn a_coop_session_moves_its_instances_with_the_hosts_defaults() {
        let fixture = Fixture::new();
        assert!(instance_routes(&fixture.host, &Config::default(), false).is_some());
        assert!(instance_routes(&fixture.host, &Config::default(), true).is_none());
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
        assert_eq!(early.routed[&id(20)], Some(20));
        let tree = BTreeSet::from([id(20), id(21), id(30), id(31)]);
        pass(&mut early, std::slice::from_ref(&tree), &mut asked);
        pass(&mut early, &[tree], &mut asked);
        assert_eq!(asked, [20, 21, 30, 31]);
        assert_eq!(early.routed[&id(21)], Some(21));
        assert_eq!(early.routed[&id(30)], None);
        assert_eq!(early.routed[&id(31)], None);
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
                Some(Ok(pid))
            },
            move |pids| back_tx.send(pids.to_vec()).unwrap(),
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
}
