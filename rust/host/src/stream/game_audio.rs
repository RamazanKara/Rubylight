//! Process discovery never runs on the packet sender: enumerating processes
//! once a second must not introduce a once-a-second audio dropout.
use crate::state::{Session, Shared};
use anyhow::Result;
use butterpollo_windows::{
    audio::{Loopback, ProcessActivation},
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
    pub fn start(h: Shared, s: Arc<Session>) -> Result<Self> {
        let target = Arc::new(Mutex::new(Err(
            "waiting for the launched game process".to_owned()
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = thread::Builder::new()
            .name("game audio target".into())
            .spawn({
                let (target, stop) = (target.clone(), stop.clone());
                move || {
                    while !stop.load(Ordering::Acquire)
                        && !s.stopping()
                        && !h.stop.load(Ordering::Acquire)
                    {
                        let selected = h
                            .current_app
                            .lock()
                            .unwrap()
                            .as_ref()
                            .filter(|app| app.id == s.launch.app_id)
                            .ok_or_else(|| {
                                anyhow::anyhow!("this stream has no matching launched game")
                            })
                            .and_then(crate::process::RunningApp::audio_process)
                            .map_err(|error| format!("{error:#}"));
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
        s.launch.warnings.set("audio_game_only", format!("Game-only audio is unavailable ({message}); sending silence. No other application audio is captured. Use a verified game launch, or explicitly select System audio."));
        self.pending = None;
        self.check = Instant::now() + Duration::from_secs(5);
    }
    pub fn update(&mut self, s: &Session, capture: &mut Option<Loopback>) {
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
                        *capture = None;
                        match ProcessActivation::new(target) {
                            Ok(pending) => self.pending = Some(pending),
                            Err(error) => self.failed(s, &format!("{error:#}")),
                        }
                    }
                }
                Err(error) => {
                    *capture = None;
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
                    if !s.launch.host_audio {
                        s.launch.warnings.set("audio_game_host", "Game-only audio leaves the PC playback devices unchanged, so game sound remains audible on the host. Set the game to a virtual playback device if you need local silence.");
                    }
                }
                Ok(None) => {}
                Err(error) => self.failed(s, &format!("{error:#}")),
            }
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
