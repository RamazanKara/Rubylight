//! A background stream's game opens on the PC's primary monitor like any
//! other program; this worker moves its windows onto the stream's display,
//! without activating them, and keeps telling the game it is the active
//! window, for as long as the stream runs.
use crate::state::{Session, Shared};
use anyhow::Result;
use butterpollo_core::background::Rect;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const WARNING: &str = "background_stream";

pub(super) struct GameWindows {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl GameWindows {
    pub fn start(h: Shared, s: Arc<Session>) -> Result<Self> {
        let generation = h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .filter(|app| app.id == s.launch.app_id && app.owner == s.launch.client.uuid)
            .map(|app| app.generation.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let worker = thread::Builder::new()
            .name("background windows".into())
            .spawn({
                let stop = stop.clone();
                move || run(&h, &s, generation.as_deref(), &stop)
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for GameWindows {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            super::finish_worker(worker, "background windows");
        }
    }
}

fn target(output: &str) -> Option<Rect> {
    butterpollo_windows::capture::displays()
        .ok()?
        .into_iter()
        .find(|display| display.display_name == output)
        .map(|display| Rect {
            x: display.x,
            y: display.y,
            width: i32::try_from(display.width).unwrap_or(i32::MAX),
            height: i32::try_from(display.height).unwrap_or(i32::MAX),
        })
}

fn run(h: &Shared, s: &Session, generation: Option<&str>, stop: &AtomicBool) {
    let mut keeper = butterpollo_windows::window_placement::Keeper::default();
    let mut moved_total = 0usize;
    while !stop.load(Ordering::Acquire) && !s.stopping() && !h.stop.load(Ordering::Acquire) {
        let output = s.output.read().unwrap().clone();
        let processes = h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .filter(|app| app.id == s.launch.app_id && Some(app.generation.as_str()) == generation)
            .ok_or_else(|| anyhow::anyhow!("this stream has no matching launched game"))
            .and_then(crate::process::RunningApp::window_processes);
        match (processes, target(&output)) {
            (Err(error), _) => {
                s.launch.warnings.set(WARNING, format!("Background streaming cannot find the game's windows ({error:#}), so they stay where they open, usually on the PC's main monitor. Use a library entry that starts the game's executable, or a Steam or Playnite game."));
            }
            (Ok(_), None) => {
                s.launch.warnings.set(WARNING, "The background stream's virtual display is unavailable. Waiting for it to recover; the PC's monitor is not used.");
            }
            (Ok(processes), Some(target)) => {
                let pass = keeper.pass(&processes, target);
                if let Some((window, _)) = pass.main {
                    butterpollo_windows::window_placement::fake_focus(window, &processes, target);
                }
                if pass.released_clip {
                    tracing::debug!("released a cursor clip the background game set");
                }
                if pass.moved > 0 {
                    moved_total += pass.moved;
                    tracing::info!(
                        client = %s.launch.client.name,
                        moved = pass.moved,
                        moved_total,
                        output,
                        "background stream moved game windows to its display"
                    );
                }
                if pass.given_up > 0 {
                    s.launch.warnings.set(WARNING, "The game keeps moving its window back to the PC's monitor, so background streaming stopped moving it. In the game's settings choose windowed or borderless mode, or select the stream's display.");
                } else if pass.windows == 0 {
                    s.launch.warnings.set(WARNING, "Waiting for the launched game's window. If it does not appear, use a direct game executable, Steam or Playnite entry and windowed or borderless mode.");
                } else if pass.pending > 0 {
                    s.launch.warnings.set(WARNING, "Waiting for the game's windows to reach the background display. If they stay on the PC, choose windowed or borderless mode in the game.");
                } else {
                    s.launch.warnings.clear(WARNING);
                }
            }
        }
        thread::park_timeout(Duration::from_secs(1));
    }
}
