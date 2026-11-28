//! Launcher navigation, focus/scroll tracking, dialog queueing, toast, and header helpers.

use super::*;

impl Launcher {
    /// Brings egui's style, the zoom and the language in line with the settings. Cheap when
    /// nothing changed: the style is rebuilt only for a new palette or motion setting.
    pub(super) fn sync_look(&mut self, ctx: &egui::Context, hosts: &Hosts) {
        let s = &self.settings;
        let still = match s.reduce_motion {
            ReduceMotion::Reduce => true,
            ReduceMotion::Full => false,
            ReduceMotion::System => crate::theme::system_reduces_motion(),
        };
        // The Theme setting's look, high contrast over it; a change shows on the next frame.
        let look = crate::theme::Look::of(s.theme, self.system_light, s.high_contrast);
        if self.look != Some((look, still)) {
            crate::theme::set_theme_look(ctx, look, still);
            self.look = Some((look, still));
        }
        // The stream's overlay runs without the launcher and resolves Auto itself (`Gui::follow_auto_zoom`).
        ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, crate::gui::AutoZoom(s.ui_zoom == 0)));
        let zoom = if s.ui_zoom == 0 {
            auto_zoom_now(ctx) as f32 / 100.0
        } else {
            crate::config::nearest_zoom(s.ui_zoom) as f32 / 100.0
        };
        if (ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(zoom);
        }
        let lang = crate::strings::resolve(s.language, &self.system_languages);
        crate::strings::set(lang);
        let sig = hosts_script_sig(hosts);
        if sig != self.title_scripts.0 || sig == 0 {
            self.title_scripts = (sig, collect_title_scripts(hosts));
        }
        crate::theme::fonts_for_scripts(ctx, lang, self.title_scripts.1);
    }

    /// Ctrl+= and Ctrl+- step the zoom from Auto's current size, Ctrl+0 returns to Auto. Returns
    /// whether it changed.
    pub(super) fn zoom_keys(&mut self, ctx: &egui::Context) -> bool {
        // In a text field the keys belong to the field.
        if ctx.text_edit_focused() {
            return false;
        }
        let step = ctx.input(|i| {
            if !i.modifiers.command {
                None
            } else if i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus) {
                Some(1)
            } else if i.key_pressed(Key::Minus) {
                Some(-1)
            } else if i.key_pressed(Key::Num0) {
                Some(0)
            } else {
                None
            }
        });
        let Some(step) = step else { return false };
        let stored = self.settings.ui_zoom;
        let current = if stored == 0 { auto_zoom_now(ctx) } else { stored };
        let zoom = if step == 0 { 0 } else { step_zoom(current, step) };
        let changed = zoom != stored;
        self.settings.ui_zoom = zoom;
        changed
    }

    pub fn go(&mut self, screen: Screen) {
        if matches!(screen, Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_)) {
            self.settings_return = self.screen;
        }
        if self.screen == Screen::Settings && screen != Screen::Settings && self.settings_dirty {
            self.settings_dirty = false;
        }
        // The search lives on All games and the game pages it opens.
        if !matches!(screen, Screen::AllGames | Screen::Game { .. }) {
            self.search_open = false;
            self.search_pushed = false;
            self.library_query.clear();
        }
        self.screen = screen;
        self.focus_screen = true;
        self.focused_key = None;
        self.restore_key = None;
    }

    /// Start button and the Settings link: Settings from anywhere, once.
    pub fn open_settings(&mut self) {
        match self.screen {
            Screen::Settings => {}
            Screen::Profiles | Screen::Controllers | Screen::Shortcuts | Screen::StreamHotkeys | Screen::About | Screen::LatencyBenchmark => self.leave_subpage(),
            Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.go(Screen::Settings),
            // A library or game page is not a destination: Settings opens over it, and Back
            // returns to the page with its item focused.
            Screen::Apps(_) | Screen::Game { .. } | Screen::AllGames | Screen::HostDetails(_) | Screen::Files(_) => self.open(Screen::Settings),
            _ => self.switch(Screen::Settings),
        }
    }

    /// Moves to another top-level destination (Home, PCs, Settings). They replace each other, so
    /// they never stack: Back from a destination that is not the root goes to the root.
    pub fn switch(&mut self, destination: Screen) {
        if destination == self.screen {
            return;
        }
        let root = self.root();
        if destination == root {
            // Back to the root this came from (with its item focused), else a fresh one.
            match self.trail.iter().rposition(|(screen, _)| *screen == root) {
                Some(at) => {
                    self.trail.truncate(at + 1);
                    self.pop(root);
                }
                None => {
                    self.trail.clear();
                    self.go(root);
                }
            }
            return;
        }
        let remembered = if self.screen == root {
            Some((root, self.focused_key.take()))
        } else {
            self.trail.iter().find(|(screen, _)| *screen == root).cloned()
        };
        self.trail.clear();
        self.trail.extend(remembered);
        self.go(destination);
    }

    /// Back from a Settings page (Profiles, Controllers, Shortcuts) to the Settings row that
    /// opened it.
    pub(super) fn leave_subpage(&mut self) {
        if self.trail.last().is_some_and(|(screen, _)| *screen == Screen::Settings) {
            self.pop(Screen::Settings);
        } else {
            self.go(Screen::Settings);
        }
    }

    /// Moves forward, so Back returns here with the same item focused.
    pub fn open(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        if self.trail.len() >= 16 {
            self.trail.remove(0);
        }
        self.trail.push((self.screen, self.focused_key.take()));
        self.go(screen);
    }

    /// Back to where the user came from, else `fallback`.
    pub(super) fn pop(&mut self, fallback: Screen) {
        match self.trail.pop() {
            Some((screen, key)) => {
                self.go(screen);
                self.restore_key = key;
            }
            None => self.go(fallback),
        }
    }

    /// Where the app starts and where Back ends: Home when it is on and has a native library's games.
    pub fn root(&self) -> Screen {
        if self.settings.console_home && self.home_ready && self.home_root {
            Screen::Home
        } else {
            Screen::Hosts
        }
    }

    /// [`Self::focus_item`] for a card of a game, noted for the focus to move on when Hide takes it.
    pub(super) fn focus_card(&mut self, response: &Response, key: &str, game: &str, first: bool) {
        self.cards.push((key.to_owned(), game.to_owned()));
        self.focus_item(response, key, first);
    }

    /// Gives an item focus: the first one on a new screen, or the one that had it before the
    /// user went forward. Records which item has focus so `open` can remember it.
    pub(super) fn focus_item(&mut self, response: &Response, key: &str, first: bool) {
        // Arrow keys and the D-pad move the focus at the end of a pass, where egui never reports
        // `gained_focus`: the move shows here as a new focused key, and the item scrolls into view
        // on that too (Tab moves report both; scrolling twice is one scroll).
        if response.has_focus() && self.focused_key.as_deref() != Some(key) {
            self.focused_key = Some(key.to_owned());
            self.reveal(response);
            super::feedback_hook::report(&response.ctx, nightjar_policy::ui_feedback_contract::Event::FocusTick);
        }
        if response.gained_focus() {
            self.reveal(response);
        }
        if !response.enabled() {
            return;
        }
        if self.restore_key.as_deref() == Some(key) {
            self.restore_key = None;
            self.focus_screen = false;
            response.request_focus();
            self.reveal(response);
        } else if first && self.focus_screen && self.restore_key.is_none() {
            self.focus_screen = false;
            response.request_focus();
        }
    }

    /// Scrolls an item into view, clear of the chrome content passes beneath.
    pub(super) fn reveal(&self, response: &Response) {
        let mut clear = response.clone();
        clear.rect = clear.rect.with_min_y(clear.rect.top() - self.scroll_inset);
        clear.scroll_to_me(None);
    }

    /// Gives the screen's first item focus again (after a stream or a dialog).
    pub fn refocus(&mut self) {
        self.focus_screen = true;
    }

    /// The events a controller's D-pad, stick or A (as `key`) gives egui. With nothing focused (the
    /// window just opened, a click on blank space, B, which drops egui's focus), egui moves nowhere
    /// and the press was lost: it puts focus on the open dialog's or the page's first item instead,
    /// with Tab for a page that names none (egui then focuses its first widget).
    pub fn controller_key(&mut self, ctx: &egui::Context, key: Key) -> Vec<egui::Event> {
        if key == Key::Escape {
            self.pad_back = true;
        }
        let lost = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Enter)
            && ctx.memory(|m| m.focused().is_none())
            && !egui::Popup::is_any_open(ctx);
        if !lost {
            return crate::gui::tap(key).to_vec();
        }
        if matches!(self.dialog, Dialog::None) {
            self.focus_screen = true;
        } else {
            self.focus_dialog = true;
        }
        ctx.request_repaint();
        crate::gui::tap(Key::Tab).to_vec()
    }

    /// Opens `dialog` with focus on its first control.
    pub fn ask(&mut self, dialog: Dialog) {
        self.dialog = dialog;
        self.focus_dialog = true;
    }

    /// Opens `dialog` once the open one is closed: for an answer that arrives by itself, which must
    /// not take away what the player is reading or answering.
    pub fn ask_after(&mut self, dialog: Dialog) {
        if matches!(self.dialog, Dialog::None) {
            self.ask(dialog);
        } else {
            self.queued.push_back(dialog);
        }
    }

    /// The "Now playing on ..." that is open or waits, if any.
    pub fn moved_prompt(&self) -> Option<(u64, i32)> {
        std::iter::once(&self.dialog).chain(&self.queued).find_map(|dialog| match dialog {
            Dialog::NowPlaying { host, app, .. } => Some((*host, *app)),
            _ => None,
        })
    }

    /// The PC gave the game back: what said it moved now says that, and offers to resume it.
    pub fn game_came_back(&mut self, host: u64, app: i32) {
        self.correct_moved(host, app, || Dialog::GameCameBack { host, app });
    }

    /// The press failed and the game is another device's: what said it moved now says that, and
    /// offers to stream beside it.
    pub fn move_failed(&mut self, host: u64, app: i32) {
        self.correct_moved(host, app, || Dialog::MoveFailed { host, app });
    }

    fn correct_moved(&mut self, host: u64, app: i32, correction: impl Fn() -> Dialog) {
        let said = |dialog: &Dialog| matches!(dialog, Dialog::NowPlaying { host: h, app: a, .. } if (*h, *a) == (host, app));
        if said(&self.dialog) {
            self.ask(correction());
        }
        for dialog in self.queued.iter_mut().filter(|dialog| said(dialog)) {
            *dialog = correction();
        }
    }

    /// Opens the next waiting dialog, if one waits.
    pub(crate) fn next_queued(&mut self) -> bool {
        let next = self.queued.pop_front();
        let any = next.is_some();
        if let Some(dialog) = next {
            self.ask(dialog);
        }
        any
    }

    /// The profile a stream on the current network uses, None for the main settings.
    pub fn active_profile(&self) -> Option<&crate::config::Profile> {
        self.profiles.profile(self.network.kind(&self.profiles.home_wifi))
    }

    /// A PC's own settings, over the profile in use and the main settings.
    pub fn open_host_settings(&mut self, id: u64, host: &Host) {
        self.inherited_settings = self.profiles.settings(&self.settings, self.network.kind(&self.profiles.home_wifi));
        self.edit_overrides = host.record.settings.clone().unwrap_or_default();
        self.edit_settings = self.edit_overrides.apply(&self.inherited_settings);
        self.inherited_from = inherited_from(None, self.active_profile());
        self.edit_name = trf("{} settings", &[&host.display_name()]);
        self.go(Screen::HostSettings(id));
    }

    /// A game's own settings, over its PC's, the profile in use and the main settings.
    pub fn open_app_settings(&mut self, id: u64, host: &Host, app: nightjar_policy::http::App) {
        self.inherited_settings = host.record.settings(&self.profiles.settings(&self.settings, self.network.kind(&self.profiles.home_wifi)));
        self.edit_overrides = host.record.app_settings(&app.uuid, app.id).cloned().unwrap_or_default();
        self.edit_settings = self.edit_overrides.apply(&self.inherited_settings);
        self.inherited_from = inherited_from(host.record.settings.as_ref(), self.active_profile());
        self.edit_name = trf("{} settings", &[&app.name]);
        self.editing_app = Some(app);
        self.go(Screen::AppSettings(id));
    }

    pub fn message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.dialog = Dialog::Message {
            title: title.into(),
            body: body.into(),
        };
        self.focus_dialog = true;
    }

    /// A message that arrives by itself, from a worker or a timer: shown at once over nothing, a
    /// running network test or a failed start's card, else once the dialog the player is reading
    /// or answering is closed (a PIN, a typed name, the replace prompt).
    pub fn notify(&mut self, title: impl Into<String>, body: impl Into<String>) {
        match self.dialog {
            Dialog::None | Dialog::NetworkTest { .. } | Dialog::ConnectFailed { .. } => self.message(title, body),
            _ => self.queued.push_back(Dialog::Message { title: title.into(), body: body.into() }),
        }
    }

    /// Whether a link from the system can be shown now, in front of nothing the user is doing.
    pub fn can_open_link(&self) -> bool {
        matches!(self.dialog, Dialog::None) && !self.pin.as_ref().is_some_and(crate::pin_ui::PinLock::app_locked)
    }

    /// Asks about a link from the system, or refuses it. The link is not shown back: it holds a
    /// PIN and passphrase.
    pub fn open_link(&mut self, text: &str) {
        if rubylight_protocol::invite_uri::InviteUri::is_link(text) {
            match crate::hosts::parse_invite(text) {
                Ok(link) => self.ask(Dialog::ConfirmInvite { link, shown: None }),
                Err(why) => self.message(tr("Could not join over the internet"), tr(why)),
            }
            return;
        }
        match crate::hosts::parse_link(text) {
            Ok(link) => self.ask(Dialog::ConfirmLink { link, shown: None }),
            Err(why) => self.message(tr("Could not pair"), tr(why)),
        }
    }

    /// Shows `text` over the launcher for a few seconds, like Android's toasts.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), std::time::Instant::now()));
    }

    pub(super) fn draw_toast(&mut self, ctx: &egui::Context) {
        const SHOWN: std::time::Duration = std::time::Duration::from_secs(4);
        let Some((notice, at)) = &self.toast else { return };
        let Some(left) = SHOWN.checked_sub(at.elapsed()) else {
            self.toast = None;
            ctx.request_repaint();
            return;
        };
        egui::Area::new(Id::unique("launcher-toast"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .interactable(false)
            .show(ctx, |ui| {
                let place = ui.painter().add(egui::Shape::Noop);
                let shown = Frame::new().inner_margin(Margin::symmetric(18, 10)).show(ui, |ui| {
                    ui.label(RichText::new(notice).font(regular(text::LABEL)).color(ON_SURFACE));
                });
                let rect = shown.response.rect;
                let mut shapes = Vec::new();
                // The stream menu's recipe, not the thin chip: with no blur, the words of a page under
                // a chip read through a toast's own.
                glass::without_backdrop(&mut shapes, ctx, rect, rect.height() / 2.0, glass::Material::Menu);
                // The shadow falls past the area's clip.
                let mut painter = ui.painter().clone();
                painter.set_clip_rect(ctx.content_rect());
                painter.set(place, egui::Shape::Vec(shapes));
            });
        ctx.request_repaint_after(left);
    }

    /// A new renderer (Linux's switch to the GPU renderer): every texture the launcher made went with
    /// the old one; each is made again where it is next drawn.
    pub fn renderer_changed(&mut self, ctx: &egui::Context) {
        self.logo = None;
        #[cfg(any(test, feature = "demo-library"))]
        {
            self.demo_video = None;
        }
        self.ambient.renderer_changed(ctx);
        glass::forget_soft(ctx);
    }

    pub(super) fn logo(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.logo
            .get_or_insert_with(|| ctx.load_texture("logo", logo_image(128), egui::TextureOptions::LINEAR))
            .id()
    }

    /// Back: Escape, the Back button or the controller's B button (`_pad`).
    pub(super) fn back(&mut self, commands: &mut Vec<Command>, _pad: bool) {
        match &self.dialog {
            Dialog::None => match self.screen {
                // The end of the line: Escape or B asks to close Nightjar.
                Screen::Home => self.ask(Dialog::ConfirmExit),
                Screen::Hosts => {
                    if !self.trail.is_empty() || self.root() == Screen::Home {
                        let root = self.root();
                        self.pop(root);
                    } else {
                        self.ask(Dialog::ConfirmExit);
                    }
                }
                Screen::Apps(_) | Screen::Game { .. } | Screen::AllGames | Screen::HostDetails(_) | Screen::Files(_) | Screen::Help => {
                    let root = self.root();
                    self.pop(root);
                }
                Screen::Settings => {
                    commands.push(Command::SaveSettings);
                    let root = self.root();
                    self.pop(root);
                }
                Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {
                    commands.extend(self.screen.save_command());
                    self.go(self.settings_return);
                }
                Screen::Profiles | Screen::Shortcuts | Screen::StreamHotkeys | Screen::About | Screen::LatencyBenchmark => self.leave_subpage(),
                Screen::Controllers => {
                    commands.push(Command::SaveControllerMapping);
                    self.leave_subpage();
                }
            },
            Dialog::NetworkTest { .. } => { commands.push(Command::CancelNetworkTest); self.dialog = Dialog::None; }
            Dialog::Pairing { .. } => {
                commands.push(Command::CancelPair);
                self.dialog = Dialog::None;
            }
            _ => self.close_dialog(),
        }
    }

    /// Whether the page leads back: anything below the top level.
    pub(super) fn has_back(&self) -> bool {
        !matches!(self.screen, Screen::Home | Screen::Hosts | Screen::Settings | Screen::Help)
    }

    /// The top bar's destinations: all of them, always in the same places. Home stays without a
    /// library to show (it says what to do then), as Android's Start does.
    pub(super) fn offered_destinations(&self) -> Vec<Screen> {
        DESTINATIONS.iter().map(|(_, s)| *s).collect()
    }

    /// L1 and R1: to the destination left or right of the current one, round from the last to the
    /// first. Not over a dialog or while a text field takes the keys.
    pub fn step_destination(&mut self, ctx: &egui::Context, forward: bool, commands: &mut Vec<Command>) {
        if !matches!(self.dialog, Dialog::None) || ctx.text_edit_focused() {
            return;
        }
        // With no PC at all Home is the PCs' first run, so the shoulders step past it.
        let mut offered = self.offered_destinations();
        if self.no_pcs {
            offered.retain(|s| *s != Screen::Home);
        }
        let Some(at) = offered.iter().position(|s| *s == self.screen.destination()) else { return };
        let to = offered[if forward { (at + 1) % offered.len() } else { (at + offered.len() - 1) % offered.len() }];
        if to != self.screen {
            self.go_destination(to, commands);
            self.focus_screen = true;
        }
    }

    /// Opens destination `screen` as its top-bar button does.
    pub(super) fn go_destination(&mut self, screen: Screen, commands: &mut Vec<Command>) {
        match screen {
            // As Start does: over a library, game or PC page, so Back returns to it and the
            // page keeps its check or transfers.
            Screen::Settings => {
                if let Some(save) = self.screen.save_command() {
                    commands.push(save);
                }
                self.open_settings();
            }
            // Home and PCs drop the trail, and with it a PC page and its shared folders' transfer.
            // A Files page's transfers go on.
            _ if self.transferring() => {
                self.dialog = Dialog::ConfirmLeave { to: screen };
                self.focus_dialog = true;
            }
            _ => self.leave_for(screen, commands),
        }
    }

    /// Moves to destination `to`, saving the page it leaves.
    pub(super) fn leave_for(&mut self, to: Screen, commands: &mut Vec<Command>) {
        if let Some(save) = self.screen.save_command() {
            commands.push(save);
        }
        self.switch(to);
    }

    /// Whether a shared folders' transfer runs on a PC page, which leaving it cancels.
    pub(super) fn transferring(&self) -> bool {
        self.folders.transferring()
    }

    /// The page's actions at the right edge of its row, registered left to right so Tab reaches
    /// them in reading order. What the row's title leaves them is said to it next frame.
    pub(super) fn actions(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let mut used = 0.0;
        if let Screen::Apps(id) = self.screen
            && let Some(host) = view.hosts.get(id) {
                used = self.apps_head(ui, host, commands);
            }
        if (used - self.header_width).abs() > 0.5 {
            self.header_width = used;
            ui.ctx().request_repaint();
        }
    }
}

/// A destination's icon in the top bar.
pub(super) fn destination_icon(screen: Screen) -> Icon {
    match screen {
        Screen::Home => Icon::Gamepad,
        Screen::Hosts => Icon::Monitor,
        Screen::Settings => Icon::Gear,
        _ => Icon::Help,
    }
}

/// Which scroll area a page keeps its place in. Each game has its own.
pub(super) fn page_area(screen: Screen) -> Id {
    match screen {
        Screen::Game { host, app } => Id::unique(("game", host, app)),
        Screen::Home => Id::unique("home"),
        Screen::AllGames => Id::unique("all-games"),
        Screen::Hosts => Id::unique("hosts"),
        Screen::Apps(_) => Id::unique("apps"),
        Screen::HostDetails(_) => Id::unique("host-details"),
        Screen::Files(_) => Id::unique("files"),
        Screen::Profiles => Id::unique("profiles"),
        Screen::Controllers => Id::unique("controller-mapping"),
        Screen::Shortcuts => Id::unique("shortcuts"),
        Screen::StreamHotkeys => Id::unique("stream-hotkeys"),
        Screen::Help => Id::unique("help"),
        Screen::About => Id::unique("about"),
        Screen::LatencyBenchmark => Id::unique("latency-benchmark"),
        Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => Id::unique("settings"),
    }
}

fn mix_u64(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn mix_bytes(hash: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn hosts_script_sig(hosts: &Hosts) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    mix_u64(&mut hash, hosts.list.len() as u64);
    for host in &hosts.list {
        mix_u64(&mut hash, host.id);
        mix_bytes(&mut hash, host.display_name().as_bytes());
        match &host.apps {
            Apps::Loaded(apps) => mix_u64(&mut hash, apps.len() as u64 + 1),
            Apps::Failed(msg) => mix_bytes(&mut hash, msg.as_bytes()),
            _ => mix_u64(&mut hash, 0),
        }
        if let Some(lib) = &host.native.library {
            mix_bytes(&mut hash, lib.revision.as_bytes());
            mix_u64(&mut hash, lib.games.len() as u64);
        }
        if let Some(lib) = &host.classic {
            mix_bytes(&mut hash, lib.revision.as_bytes());
            mix_u64(&mut hash, lib.games.len() as u64);
        }
    }
    hash.max(1)
}

fn collect_title_scripts(hosts: &Hosts) -> crate::theme::ScriptSet {
    let mut set = crate::theme::ScriptSet::default();
    for host in &hosts.list {
        set.merge(crate::theme::scripts_for_text(host.display_name()));
        if let Apps::Loaded(apps) = &host.apps {
            for app in apps.iter() {
                set.merge(crate::theme::scripts_for_text(&app.name));
            }
        }
        if let Some(lib) = &host.native.library {
            for game in &lib.games {
                set.merge(crate::theme::scripts_for_text(&game.title));
            }
        }
        if let Some(lib) = &host.classic {
            for game in &lib.games {
                set.merge(crate::theme::scripts_for_text(&game.title));
            }
        }
    }
    set
}
