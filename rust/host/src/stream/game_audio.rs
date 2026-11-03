//! Process discovery never runs on the packet sender: enumerating processes
//! once a second must not introduce a once-a-second audio dropout.
use crate::state::{Session, Shared};
use anyhow::Result;
use butterpollo_core::{
    audio_scope::{Identity, tree},
    config::Config,
};
use butterpollo_windows::{
    audio::{Loopback, ProcessActivation},
    audio_app_route::{AppRoute, OnThePc, no_sound_yet},
    process::AudioProcess,
};
use std::collections::BTreeMap;
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
pub(super) fn routable(pid: u32) -> bool {
    !butterpollo_windows::process::image_path(pid).is_some_and(|path| windows_program(&path))
}
/// Whether a stream of `config` moves its game's sound off the PC: game-only
/// audio it streams, and doesn't also play on the PC.
pub(super) fn moves_game_sound(config: &Config, host_audio: bool) -> bool {
    config.get("audio_capture_mode", "system") == "game"
        && config.boolean("stream_audio", true)
        && !host_audio
}
/// Where the game's sound reaches the PC.
#[derive(Debug, PartialEq)]
enum Elsewhere {
    /// A routed process opened it before its route; or, with `voice`, on
    /// the communications device the PC also plays on, it may be the
    /// game's voice chat.
    Opened { voice: bool },
    /// A Windows program in the game's tree, never routed, plays it.
    Windows(u32),
}
/// Of the game's processes `found` on the PC's device, the one to tell:
/// a routed one first, as the next start fixes it.
fn place(found: &OnThePc, windows: &std::collections::BTreeSet<Identity>) -> Option<Elsewhere> {
    let windows_program = |pid: &u32| windows.iter().any(|id| id.pid == *pid);
    if found.pids.iter().any(|pid| !windows_program(pid)) {
        return Some(Elsewhere::Opened {
            voice: found.maybe_voice,
        });
    }
    found.pids.first().map(|&pid| Elsewhere::Windows(pid))
}
const OPENED_EARLY: &str = "Game sound also plays on the PC: the game opened its sound before Rubylight moved it to the virtual playback device. Windows keeps that choice, so it moves when the game starts again, unless the game picks its own output device in its settings: set it to the default device there.";
/// The PC's communications device is also where it plays the game: a
/// session there may be the game's voice chat, which stays on it, so the
/// warning names both.
const OPENED_EARLY_OR_VOICE: &str = "Sound of the game plays on the PC, on its communications device: either the game's voice chat, which stays there, or game sound the game opened before Rubylight moved it to the virtual playback device, which moves when the game starts again.";
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
            let route = match AppRoute::acquire(config, &h.directory, identity.pid) {
                // Windows takes its device once it opened its sound (#59):
                // tried again at the next look.
                Err(error) if no_sound_yet(&error) => continue,
                route => route.map_err(|error| format!("{error:#}")),
            };
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
        let check = self.ticks.is_multiple_of(PLACE_CHECK);
        let elsewhere = if check {
            let live: Vec<&AppRoute> = self.routed.values().flatten().collect();
            let unrouted: Vec<u32> = self.windows.iter().map(|id| id.pid).collect();
            let found = AppRoute::plays_elsewhere(&live, &unrouted).unwrap_or_else(|error| {
                tracing::debug!(
                    error = format!("{error:#}"),
                    "where the game's sound plays can't be read"
                );
                OnThePc::default()
            });
            place(&found, &self.windows)
        } else {
            None
        };
        match (&self.failed, elsewhere) {
            (Some(error), _) => s.launch.warnings.set("audio_game_host", format!("Game sound also plays on the PC: Rubylight could not move the game to a virtual playback device ({error}).")),
            (None, Some(Elsewhere::Opened { voice: false })) => s.launch.warnings.set("audio_game_host", OPENED_EARLY),
            (None, Some(Elsewhere::Opened { voice: true })) => s.launch.warnings.set("audio_game_host", OPENED_EARLY_OR_VOICE),
            (None, Some(Elsewhere::Windows(pid))) => {
                let program = butterpollo_windows::process::image_path(pid)
                    .and_then(|path| path.rsplit('\\').next().map(str::to_string))
                    .unwrap_or_else(|| format!("process {pid}"));
                s.launch.warnings.set("audio_game_host", format!("Game sound also plays on the PC: the game plays it through {program}, a Windows program Rubylight doesn't move, as that would move every program of it."));
            }
            (None, None) if check => s.launch.warnings.clear("audio_game_host"),
            (None, None) => {}
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

    /// A routed process still on the PC's device is the game's own sound
    /// opened before its route; a Windows program the game plays through is
    /// named, since it is never routed.
    #[test]
    fn sound_on_the_pc_is_told_by_who_plays_it() {
        let windows = std::collections::BTreeSet::from([Identity {
            pid: 30,
            started: 1,
        }]);
        let found = |pids: &[u32], maybe_voice| OnThePc {
            pids: pids.to_vec(),
            maybe_voice,
        };
        let opened = Some(Elsewhere::Opened { voice: false });
        assert_eq!(place(&found(&[], false), &windows), None);
        assert_eq!(place(&found(&[7], false), &windows), opened);
        assert_eq!(place(&found(&[7, 30], false), &windows), opened);
        assert_eq!(
            place(&found(&[7], true), &windows),
            Some(Elsewhere::Opened { voice: true }),
            "on a communications device that is also the PC's default it may be voice chat"
        );
        assert_eq!(
            place(&found(&[30], false), &windows),
            Some(Elsewhere::Windows(30))
        );
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
