//! Process discovery never runs on the packet sender: enumerating processes
//! once a second must not introduce a once-a-second audio dropout.
use crate::state::{Session, Shared};
use anyhow::Result;
use butterpollo_core::{audio_scope::Identity, config::Config};
use butterpollo_windows::{
    audio::{Loopback, ProcessActivation},
    audio_app_route::AppRoute,
    process::AudioProcess,
};
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
                    let mut routed: Option<(Identity, Option<AppRoute>)> = None;
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
                        if let Ok(selected) = &selected
                            && !s.launch.host_audio
                            && routed.as_ref().map(|(identity, _)| *identity)
                                != Some(selected.identity)
                        {
                            // The new instance is routed before the old one lets
                            // go: both are usually the same program, which must
                            // not flip back to the PC's speakers in between.
                            let new = route(&h, &s, &config, selected);
                            routed = Some((selected.identity, new));
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
/// Play the captured game process on a virtual sink nobody hears, so its
/// sound reaches only the client (#59). Without one it plays on the PC too,
/// and the stream says so.
fn route(h: &Shared, s: &Session, config: &Config, target: &AudioProcess) -> Option<AppRoute> {
    match AppRoute::acquire(config, &h.directory, target.identity.pid) {
        Ok(route) => {
            s.launch.warnings.clear("audio_game_host");
            Some(route)
        }
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "game audio stays on the PC's playback device");
            s.launch.warnings.set("audio_game_host", format!("Game sound also plays on the PC: Rubylight could not move the game to a virtual playback device ({error:#}). Install Steam Streaming Speakers or set a virtual sink in Audio settings."));
            None
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
