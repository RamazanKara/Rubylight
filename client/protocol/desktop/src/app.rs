//! What the app does: the methods of `App`, whose state and main loop are in main.rs.
//!
//! Here: saving, shutting down, pairing links and adding a PC. The rest by area: the launcher's events
//! and frames (`launcher`), what a page asks for (`command`), the window's renderer (`renderer`), starting
//! a stream (`launch`), a running stream (`session`) and what is drawn over the picture (`overlay`).

use super::*;

mod command;
mod launch;
mod launcher;
mod overlay;
mod renderer;
mod session;

pub(crate) use launch::*;
pub(crate) use launcher::*;
pub(crate) use overlay::*;
pub(crate) use renderer::*;
pub(crate) use session::*;

/// The name of app `app` on `host`: its entry in the PC's list, else its game in the PC's library.
fn app_name_of(host: &hosts::Host, app: i32) -> Option<String> {
    if let Apps::Loaded(apps) = &host.apps {
        if let Some(entry) = apps.iter().find(|a| a.id == app) {
            return Some(entry.name.clone());
        }
    }
    host.library()?.games.iter().find(|g| g.app_id as i32 == app).map(|g| g.title.clone())
}

impl App {
    fn save_edits(&mut self) {
        // Closing Nightjar is leaving it: a start within the grace time stays unlocked.
        if let Some(pin) = self.launcher.pin.as_mut() {
            pin.left();
            pin.flush();
        }
        if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
    }

    fn save_hosts_and_place(&self) {
        if let Err(e) = self.store.save_hosts(&self.hosts.records()) { eprintln!("nightjar: PCs not saved: {e}"); }
        // Under gamescope, or straight to the display, the window was never placed from the saved
        // place, so the desktop's stays.
        if handheld::start_window(handheld::fills_screen()).restore_place {
            if let Some(placement) = window_place::current(self.window, self.store.window_placement().as_ref()) {
                if let Err(e) = self.store.save_window_placement(&placement) { eprintln!("nightjar: window place not saved: {e}"); }
            }
        }
    }

    /// Before an update's hand-over, with nothing streaming: the next Nightjar can read these files
    /// as soon as it starts (a new AppImage, or the old program after a setup that stopped), and
    /// would overwrite with what it read what shutdown writes only later.
    pub(crate) fn save_for_next_run(&mut self) {
        self.save_edits();
        self.save_hosts_and_place();
    }

    pub(crate) fn shutdown(&mut self) {
        self.save_edits();
        self.network_test = None;
        // Its decoders may run on the renderer's device, which goes with the process.
        self.launcher.latency_bench.finish();
        // Its input closes now; the process going closes it too.
        if let Some(companion) = self.second_screen.take() {
            companion.end();
            self.second_screen_gone();
        }
        // The PC's automatic bitrate goes off after the window has gone, not with the process.
        self.gui_back_from_overlay();
        let abr_stop = self.streaming.take().and_then(|(streaming, _)| streaming.finish(self.window, &mut self.pads));
        if let Some(c) = &self.connecting {
            c.cancel();
        }
        self.save_hosts_and_place();
        self.pads.close_all();
        self.gui.destroy();
        if self.connecting.is_some() {
            // Ending a launch can take seconds: the window goes now rather than sitting frozen meanwhile.
            // SAFETY: the window run() created, still alive.
            unsafe { SDL_HideWindow(self.window) };
        }
        // The connect thread may be using the renderer's GPU device (a decoder being made on it),
        // so a launch in progress ends, and a session it started stops, before the renderer goes.
        let (renderer, window) = (self.renderer, self.window);
        // SAFETY: tearing down what run() created, with nothing else using it any more.
        stream::end_before(self.connecting.take(), stream::LEAVE_WAIT, || unsafe {
            crate::sdl::destroy_renderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        });
        if let Some(stop) = abr_stop { stop.wait(); }
    }

    /// Drops the saved library of a PC that is unpaired or removed.
    pub(crate) fn forget_library(&self, host: u64) {
        if let Some(unique_id) = self.hosts.get(host).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
            self.store.forget_library(&unique_id);
        }
    }

    pub(crate) fn save_hosts(&self) {
        save_hosts(&self.store, &self.hosts);
    }

    /// Adds the PC a pairing link names and pairs with its one-time PIN and certificate.
    pub(crate) fn pair_link(&mut self, link: rubylight_protocol::pairing_uri::PairingUri) {
        let id = self.hosts.add_manual(&link.host, Some(link.port));
        self.save_hosts();
        self.launcher.dialog = Dialog::Pairing { host: id, pin: String::new() };
        self.hosts.pair_link(id, link);
    }

    /// Shows a link the system opened with Nightjar, once nothing else is in the way: a stream
    /// (this runs only between them), a stream being started, an open dialog or the PIN screen.
    fn open_waiting_link(&mut self) {
        if self.connecting.is_some() || self.network_launch.is_some() || !self.launcher.can_open_link() {
            return;
        }
        let Some(link) = self.links.take() else { return };
        // SAFETY: window calls on the main thread with a live window.
        unsafe {
            if SDL_GetWindowFlags(self.window) & SDL_WINDOW_MINIMIZED != 0 {
                SDL_RestoreWindow(self.window);
            }
            SDL_RaiseWindow(self.window);
        }
        self.launcher.open_link(&link);
    }

    /// Add PC: an address adds the PC; a QR pairing link adds it and pairs with the link's
    /// one-time PIN and certificate, at once: it was typed or pasted here on purpose.
    fn add_entry(&mut self, text: &str) {
        match hosts::parse_entry(text) {
            Ok(hosts::Entry::Address(address, port)) => {
                self.hosts.add_manual(&address, port);
                self.save_hosts();
            }
            Ok(hosts::Entry::Link(link)) => self.pair_link(*link),
            // The link holds a PIN and passphrase: it is not shown back.
            Err(hosts::LINK_INVALID) => self.launcher.message(tr("Could not pair"), tr(hosts::LINK_INVALID)),
            Err(why) => self.launcher.message(tr(why), text.trim()),
        }
    }
}

pub(crate) fn save_hosts(store: &config::Store, hosts: &Hosts) {
    if let Err(e) = store.save_hosts(&hosts.records()) {
        eprintln!("nightjar: saving hosts failed: {e}");
    }
}

/// Keeps the name chosen for a PC on this device, and saves it.
pub(crate) fn rename_host(hosts: &mut Hosts, store: &config::Store, id: u64, alias: &str) {
    if hosts.rename(id, alias) {
        save_hosts(store, hosts);
    }
}

/// Forgets a PC: its saved library, covers, recent games and favourites go with it.
pub(crate) fn remove_host(
    hosts: &mut Hosts,
    store: &config::Store,
    recents: &mut Vec<config::Recent>,
    favourites: &mut Vec<config::Favourite>,
    open_game: &mut Option<config::OpenGame>,
    id: u64,
) {
    if let Some(unique_id) = hosts.get(id).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
        (*recents, *favourites) = store.forget_host(&unique_id, recents, favourites);
        *open_game = store.open_play().game;
    }
    hosts.remove(id);
    save_hosts(store, hosts);
}
