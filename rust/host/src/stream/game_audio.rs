//! Process discovery never runs on the packet sender: enumerating processes
//! once a second must not introduce a once-a-second audio dropout.
use crate::state::{Session, Shared};
use anyhow::Result;
use butterpollo_core::{
    audio_scope::{Identity, tree},
    config::Config,
};
use butterpollo_windows::nucleus::session::Instances;
use butterpollo_windows::{
    audio::{Loopback, ProcessActivation},
    audio_app_route::AppRoute,
    process::AudioProcess,
};
use std::collections::{BTreeMap, BTreeSet};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct GameAudio {
    target: Arc<Mutex<std::result::Result<AudioProcess, String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    pending: Option<ProcessActivation>,
    check: Instant,
}
impl GameAudio {
    pub fn start(h: Shared, s: Arc<Session>, config: Config) -> Result<Self> {
        let generation = h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .filter(|app| app.id == s.launch.app_id)
            .map(|app| app.generation.clone());
        let target = Arc::new(Mutex::new(Err(
            "waiting for the launched game process".to_owned()
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = thread::Builder::new()
            .name("game audio target".into())
            .spawn({
                let (target, stop) = (target.clone(), stop.clone());
                move || {
                    // The game's process plays on a virtual sink while it is
                    // captured, unless the stream also plays on the PC.
                    let mut routes = Routes::default();
                    while !stop.load(Ordering::Acquire)
                        && !s.stopping()
                        && !h.stop.load(Ordering::Acquire)
                    {
                        // A co-op player hears its own slot's game instance.
                        let selected = match s.launch.nucleus {
                            Some(seat) => crate::nucleus::audio(&h, seat),
                            None => h
                                .current_app
                                .lock()
                                .unwrap()
                                .as_ref()
                                .filter(|app| {
                                    app.id == s.launch.app_id
                                        && Some(&app.generation) == generation.as_ref()
                                })
                                .ok_or_else(|| {
                                    anyhow::anyhow!("this stream has no matching launched game")
                                })
                                .and_then(crate::process::RunningApp::audio_process),
                        }
                        .map_err(|error| format!("{error:#}"));
                        if !s.launch.host_audio {
                            routes.update(&h, &s, &config, selected.as_ref().ok());
                        }
                        *target.lock().unwrap() = selected;
                        thread::park_timeout(Duration::from_secs(1));
                    }
                }
            })?;
        Ok(Self {
            target,
            stop,
            worker: Some(worker),
            pending: None,
            check: Instant::now(),
        })
    }
    fn failed(&mut self, s: &Session, message: &str) {
        s.launch.warnings.set("audio_game_only", format!("Game-only audio is unavailable ({message}); sending silence. No other application audio is captured. Check the game's launch entry and the Windows Audio service. Process capture requires Windows build 20348 or newer."));
        self.pending = None;
        self.check = Instant::now() + Duration::from_secs(5);
    }
    /// `retire` takes a capture this drops, to count what it lost.
    pub fn update(
        &mut self,
        s: &Session,
        capture: &mut Option<Loopback>,
        retire: &mut dyn FnMut(Loopback),
    ) {
        if Instant::now() >= self.check {
            self.check = Instant::now() + Duration::from_secs(1);
            let selected = self.target.lock().unwrap().clone();
            match selected {
                Ok(target) => {
                    if capture.as_ref().and_then(Loopback::process_identity)
                        != Some(target.identity)
                        && self.pending.as_ref().map(ProcessActivation::identity)
                            != Some(target.identity)
                    {
                        if let Some(old) = capture.take() {
                            retire(old);
                        }
                        match ProcessActivation::new(target) {
                            Ok(pending) => self.pending = Some(pending),
                            Err(error) => self.failed(s, &format!("{error:#}")),
                        }
                    }
                }
                Err(error) => {
                    if let Some(old) = capture.take() {
                        retire(old);
                    }
                    self.failed(s, &error);
                    // A newly started game should become audible promptly.
                    // Only native activation failures need the longer backoff.
                    self.check = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        if let Some(pending) = &self.pending {
            match pending.poll(s.config.audio_channels as usize) {
                Ok(Some(value)) => {
                    *capture = Some(value);
                    self.pending = None;
                    s.launch.warnings.clear("audio_game_only");
                    s.launch.warnings.clear("audio_capture");
                    if s.config.audio_channels > 2 {
                        s.launch.warnings.set("audio_surround", "Game-only audio currently captures stereo and maps it into the negotiated surround layout. Choose stereo for this mode.");
                    }
                }
                Ok(None) => {}
                Err(error) => self.failed(s, &format!("{error:#}")),
            }
        }
    }
}
/// Every process of the captured tree plays on a virtual sink nobody hears,
/// so the game's sound reaches only the client (#59): a launcher's game
/// child too. When that fails, or the game keeps playing on the PC's
/// device, the stream says so.
#[derive(Default)]
struct Routes {
    /// None: routing this process failed (said once, not retried).
    routed: BTreeMap<Identity, Option<AppRoute>>,
    /// Windows programs in the tree, never routed ([`windows_program`]).
    windows: std::collections::BTreeSet<Identity>,
    failed: Option<String>,
    ticks: u32,
}
/// A program in the Windows folder.
fn windows_program(path: &str) -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let root = format!("{}\\", root.trim_end_matches('\\')).to_lowercase();
    path.to_lowercase().starts_with(&root)
}
/// Whether process `pid` may be moved: Windows' own programs a game starts
/// (conhost, WerFault) share one setting with every other process of that
/// program.
fn routable(pid: u32) -> bool {
    !butterpollo_windows::process::image_path(pid).is_some_and(|path| windows_program(&path))
}
/// Whether a stream of `config` moves its game's sound off the PC: game-only
/// audio it streams, and doesn't also play on the PC.
fn moves_game_sound(config: &Config, host_audio: bool) -> bool {
    config.get("audio_capture_mode", "system") == "game"
        && config.boolean("stream_audio", true)
        && !host_audio
}
/// For a Nucleus Co-op session a launch of `config` starts: every instance's
/// processes move to the virtual sink in the session's pass that finds them,
/// before the game's window shows and its stream starts looking (#59). A
/// game routed only then often opened its sound already, and Windows never
/// moves an open sound. Each stream still routes its own instance, and says
/// when that fails; these routes end with the session.
pub(crate) fn instance_routes(h: &Shared, config: &Config, host_audio: bool) -> Option<Instances> {
    if !moves_game_sound(config, host_audio) {
        return None;
    }
    let (config, directory) = (config.clone(), h.directory.clone());
    let early = Mutex::new(Early::default());
    Some(Arc::new(move |trees: &[BTreeSet<Identity>]| {
        early.lock().unwrap().update(trees, &mut |pid| {
            routable(pid).then(|| AppRoute::acquire(&config, &directory, pid))
        });
    }))
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
/// Seconds between the checks of where the game's sound plays.
const PLACE_CHECK: u32 = 5;
impl Routes {
    fn update(&mut self, h: &Shared, s: &Session, config: &Config, target: Option<&AudioProcess>) {
        let tree = match target
            .map(|t| butterpollo_windows::process::processes().map(|all| tree(t.identity, &all)))
        {
            None => vec![],
            Some(Ok(tree)) => tree,
            // Keep what is routed until the next look.
            Some(Err(_)) => return,
        };
        // New processes are routed before gone ones let go: they are often
        // the same program, which must not flip back to the PC in between.
        for identity in &tree {
            if self.routed.contains_key(identity) || self.windows.contains(identity) {
                continue;
            }
            if !routable(identity.pid) {
                self.windows.insert(*identity);
                continue;
            }
            let route = AppRoute::acquire(config, &h.directory, identity.pid)
                .map_err(|error| format!("{error:#}"));
            match route {
                Ok(route) => {
                    if let Some(device) = route.users_device() {
                        s.launch.warnings.set("audio_game_device", format!("The game plays on the device chosen for it in Windows' Volume mixer ({device}), not only on the client."));
                    }
                    self.routed.insert(*identity, Some(route));
                }
                Err(error) => {
                    tracing::warn!(
                        pid = identity.pid,
                        error,
                        "game audio stays on the PC's playback device"
                    );
                    self.failed = Some(error);
                    self.routed.insert(*identity, None);
                }
            }
        }
        self.routed.retain(|identity, _| tree.contains(identity));
        self.windows.retain(|identity| tree.contains(identity));
        if self.routed.values().all(Option::is_some) {
            self.failed = None;
        }
        self.ticks += 1;
        let elsewhere = self.ticks.is_multiple_of(PLACE_CHECK) && {
            let live: Vec<&AppRoute> = self.routed.values().flatten().collect();
            AppRoute::plays_elsewhere(&live).unwrap_or(false)
        };
        match &self.failed {
            Some(error) => s.launch.warnings.set("audio_game_host", format!("Game sound also plays on the PC: Rubylight could not move the game to a virtual playback device ({error}).")),
            None if elsewhere => s.launch.warnings.set("audio_game_host", "Game sound also plays on the PC: the game opened its sound before Rubylight moved it to the virtual playback device. Windows keeps that choice, so it moves when the game starts again."),
            None if self.ticks.is_multiple_of(PLACE_CHECK) => s.launch.warnings.clear("audio_game_host"),
            None => {}
        }
    }
}
impl Drop for GameAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            super::finish_worker(worker, "game audio target");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_core::session::Role;

    #[test]
    fn only_game_only_audio_off_the_pc_moves_the_game() {
        let game = Config::parse("audio_capture_mode = game\n").unwrap();
        assert!(moves_game_sound(&game, false));
        assert!(!moves_game_sound(&game, true));
        assert!(!moves_game_sound(&Config::default(), false));
        let muted = Config::parse("audio_capture_mode = game\nstream_audio = false\n").unwrap();
        assert!(!moves_game_sound(&muted, false));
    }

    /// #59: an instance moves in the pass that finds it, before its stream
    /// looks for it; a process it starts moves with it, a failed route is
    /// not retried each pass, and a gone process lets go.
    #[test]
    fn a_session_moves_its_instances_as_it_finds_them() {
        let id = |pid| Identity { pid, started: 1 };
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

    #[test]
    fn windows_programs_a_game_starts_are_never_routed() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(windows_program(&format!(r"{root}\System32\conhost.exe")));
        assert!(windows_program(&format!(
            r"{}\SYSTEM32\WerFault.exe",
            root.to_uppercase()
        )));
        assert!(!windows_program(
            r"E:\Games\LEGO Batman\LEGOBatmanLotDK-Win64-Shipping.exe"
        ));
        assert!(!windows_program(&format!(r"{root}Apps\game.exe")));
    }

    #[test]
    fn discovery_does_not_follow_a_replacement_with_the_same_app_id() {
        let fixture = Fixture::new();
        let app = fixture.host.apps.read().unwrap()[0].clone();
        *fixture.host.current_app.lock().unwrap() =
            Some(crate::process::RunningApp::with_environment(&app, Default::default()).unwrap());
        let session = fixture.session(fixture.client(u32::MAX), Role::Stream);
        let audio = GameAudio::start(fixture.host.clone(), session, Config::default()).unwrap();
        let wait_for = |message: &str| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if audio
                    .target
                    .lock()
                    .unwrap()
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.contains(message))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "audio discovery did not report {message}"
                );
                audio.worker.as_ref().unwrap().thread().unpark();
                thread::sleep(Duration::from_millis(5));
            }
        };
        wait_for("no owned game process");
        *fixture.host.current_app.lock().unwrap() =
            Some(crate::process::RunningApp::with_environment(&app, Default::default()).unwrap());
        wait_for("no matching launched game");
        drop(audio);
    }
}
