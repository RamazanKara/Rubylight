//! Drives the launcher from stdin for captures (`demo-library` builds only): one step per line, see
//! [`ui::demo::parse`]. A capture reads the back buffer, so it never holds another window, and it
//! works with the window behind others.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use super::*;
use crate::hosts::Waker;
use crate::ui::demo::{parse, Step};

pub struct Driver {
    steps: Receiver<String>,
    scroll_left: u32,
    scroll_delta: f32,
    capture: Option<PathBuf>,
    presents: u64,
}

/// A demo build writes settings where it is told, never over a player's own.
pub fn require_sandbox() -> Result<(), String> {
    if std::env::var_os("NIGHTJAR_CONFIG_DIR").is_none() {
        return Err("the demo library needs NIGHTJAR_CONFIG_DIR, so it leaves a player's own settings alone".into());
    }
    Ok(())
}

pub fn start(waker: Waker) -> Driver {
    let (sender, steps) = channel();
    let _ = std::thread::Builder::new()
        .name("demo-driver".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
                waker();
            }
        });
    Driver {
        steps,
        scroll_left: 0,
        scroll_delta: 0.0,
        capture: None,
        presents: 0,
    }
}

impl App {
    pub(super) fn demo_steps(&mut self) {
        while let Ok(line) = self.demo.steps.try_recv() {
            match parse(&line) {
                Ok(step) => self.demo_step(step),
                Err(why) => eprintln!("nightjar: demo driver: {why}"),
            }
        }
        if self.demo.scroll_left > 0 {
            self.demo.scroll_left -= 1;
            let delta = self.demo.scroll_delta;
            self.demo_wheel(delta);
        }
    }

    pub(super) fn demo_scrolling(&self) -> bool {
        self.demo.scroll_left > 0
    }

    fn demo_wheel(&mut self, delta: f32) {
        self.gui.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        });
    }

    fn demo_step(&mut self, step: Step) {
        match step {
            Step::Go(page) => {
                // The shelf holds the demo's PCs while a first-run page shows fewer.
                let shelf = &mut self.hosts.demo_shelf;
                if !shelf.is_empty() {
                    let mut all = std::mem::take(shelf);
                    if let Some(found) = self.hosts.list.pop() {
                        all.insert(3, found);
                    }
                    self.hosts.list = all;
                }
                if matches!(page, crate::ui::demo::Page::FirstRun | crate::ui::demo::Page::FirstRunFound) {
                    let mut all = std::mem::take(&mut self.hosts.list);
                    if page == crate::ui::demo::Page::FirstRunFound {
                        self.hosts.list.push(all.remove(3));
                    }
                    self.hosts.demo_shelf = all;
                }
                if let Some(command) =
                    self.launcher
                        .demo_page(page, &self.hosts, self.waker.clone())
                {
                    self.command(command);
                }
            }
            Step::Zoom(percent) => self.launcher.settings.ui_zoom = percent,
            // SAFETY: the live window, on the main thread.
            Step::Size(w, h) => unsafe {
                SDL_SetWindowSize(self.window, w, h);
            },
            Step::Pointer(x, y) => self.gui.push(egui::Event::PointerMoved(egui::pos2(x, y))),
            Step::Click(x, y) => {
                let pos = egui::pos2(x, y);
                self.gui.push(egui::Event::PointerMoved(pos));
                for pressed in [true, false] {
                    self.gui.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
                }
            }
            Step::Wheel(delta) => self.demo_wheel(delta),
            Step::Scroll { frames, delta } => {
                self.demo.scroll_left = frames;
                self.demo.scroll_delta = delta;
            }
            Step::Key(key) => {
                for pressed in [true, false] {
                    self.gui.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            Step::Capture { chain, path } => {
                if chain {
                    self.gui.redo_glass();
                }
                self.demo.capture = Some(path);
                self.window_changed = true;
            }
            Step::Contrast(on) => self.launcher.settings.high_contrast = on,
            Step::Input(source) => self.launcher.input_source = source,
            Step::Stats(path) => {
                if let Err(e) = std::fs::write(&path, format!("presents={}\n", self.demo.presents))
                {
                    eprintln!("nightjar: demo driver: {}: {e}", path.display());
                }
            }
            Step::Setting(key, value) => {
                let mut values = serde_json::to_value(&self.launcher.settings).expect("settings serialize");
                values[key.as_str()] = value;
                match serde_json::from_value(values) {
                    Ok(settings) => self.launcher.settings = settings,
                    Err(why) => eprintln!("nightjar: demo driver: setting {key}: {why}"),
                }
            }
            Step::Launch => {
                let first = self.hosts.list.iter().find_map(|host| match &host.apps {
                    hosts::Apps::Loaded(apps) => apps.first().map(|app| (host.id, app.id)),
                    _ => None,
                });
                match first {
                    Some((host, app)) => self.launch_ready(host, app, false, LaunchKind::Stream),
                    None => eprintln!("nightjar: demo driver: no PC has a game list to launch from"),
                }
            }
            Step::Quit => self.running = false,
        }
    }

    /// A capture waits for the next frame, which it reads back (`sdl::read_back_frames`).
    pub(super) fn demo_capture_pending(&self) -> bool {
        self.demo.capture.is_some()
    }

    /// With a frame drawn, before it is presented.
    pub(super) fn demo_drew(&mut self) {
        self.demo.presents += 1;
        if let Some(path) = self.demo.capture.take() {
            if let Err(why) = sdl::screenshot(self.renderer, &path) {
                eprintln!("nightjar: demo driver: {}: {why}", path.display());
            }
        }
    }
}
