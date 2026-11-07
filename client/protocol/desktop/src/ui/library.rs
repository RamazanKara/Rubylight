//! PCs, Home and a PC's library: the pages that list PCs and games, a game's page, and what
//! selecting a PC or game does.

use super::chrome::{capsule_width, control_capsule, info_chip, info_chip_width};
use super::design_tokens as t;
use super::pcs::pc_line_of;
use super::dialogs::{approve_dialog, short_device_name};
use super::pcs::{action_button, action_width, PcAction};
use super::*;

/// How wide a List line grows: past it, the store and the date sit too far from the title.
pub(super) const LIST_MAX_W: f32 = 760.0;
/// The places a PC's list holds open while it loads.
const LOADING_ROWS: usize = 2;
/// The capsules at the right of a PC's page.
const ACTION_H: f32 = 48.0;
const ACTIONS_GAP: f32 = 14.0;
/// A PC's page with no list: the words' widest, and the room it holds under its header.
const BARE_WORDS_W: f32 = 520.0;
const BARE_FOOT: f32 = 120.0;
const BARE_LEAST: f32 = 200.0;
/// What a page's title keeps before the name of the running game gives way.
const MIN_TITLE_ROOM: f32 = 160.0;

impl Launcher {
    pub(super) fn add_host_dialog(&mut self) {
        self.dialog = Dialog::AddHost {
            text: String::new(),
            error: None,
        };
        self.focus_dialog = true;
    }

    pub(super) fn apps(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(id) else {
            self.go(Screen::Hosts);
            return;
        };
        let running = host.running_app();
        let (columns, width) = grid(ui.available_width());
        ui.spacing_mut().item_spacing.y = GRID_ROW_GAP;
        match &host.apps {
            // Read again from an empty list: the same place, its button at work.
            Apps::NotLoaded | Apps::Loading if self.refreshing_empty == Some(id) => {
                self.bare_list(ui, id, tr("This PC lists no apps."), None, tr("Refresh"), true, false, commands);
            }
            Apps::NotLoaded | Apps::Loading => {
                // The covers' own places, so the list lands without a jump, under one word of what is
                // going on.
                let top = ui.cursor().min;
                for _ in 0..LOADING_ROWS {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = POSTER_GAP;
                        for _ in 0..columns {
                            cover_placeholder(ui, width);
                        }
                    });
                }
                let room = Rect::from_min_size(top, vec2(ui.available_width(), LOADING_ROWS as f32 * (cover_height(width) + GRID_ROW_GAP)));
                loading_chip(ui, room);
            }
            Apps::Failed(e) => {
                self.refreshing_empty = None;
                self.bare_list(ui, id, tr("The library could not be loaded."), Some(e), tr("Try again"), false, false, commands);
            }
            Apps::Loaded(apps) => {
                self.refreshing_empty = None;
                if apps.is_empty() {
                    // Nothing listed is nothing to start, so no tile: the way on is to read it again.
                    self.bare_list(ui, id, tr("This PC lists no apps."), None, tr("Refresh"), false, true, commands);
                    return;
                }
                // Hidden games stay out of the PC's list as off Home, unless asked for.
                let source = Source::of(host);
                let hides = |app: &nightjar_core::http::App| console::is_hidden(&source, &app.uuid, app.id as u32, &self.hidden_games);
                let any_hidden = apps.iter().any(hides);
                let shown: Vec<&nightjar_core::http::App> = apps.iter().filter(|app| self.show_hidden_apps || !hides(app)).collect();
                for (row, chunk) in shown.chunks(columns).enumerate() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = POSTER_GAP;
                        for (i, app) in chunk.iter().enumerate() {
                            let is_running = running.is_some_and(|r| app.matches_running_app(r, None));
                            let art = shown_art(view, id, app.id as u32);
                            let response = bare_poster(ui, &app.name, art, if is_running { Badge::Running } else { Badge::None }, width);
                            let first = if running.is_some() { is_running } else { row == 0 && i == 0 };
                            let key = format!("app:{}", app.id);
                            self.focus_card(&response, &key, &key, first);
                            let dots = cover_dots(ui, &response, response.rect, if is_running { Badge::Running } else { Badge::None });
                            if response.clicked() {
                                if let Some(ask) = self.start_app(host, app, commands) {
                                    self.ask(ask);
                                }
                            }
                            if let Some(anchor) = menu_request(ui, &response, dots.as_ref()) {
                                self.open_menu(Dialog::GameMenu { host: id, game: GameTarget::Classic { app: app.id } }, anchor, key);
                            }
                        }
                    });
                }
                // A focused-but-unloaded first poster still gets focus next frame.
                if self.focus_screen && running.is_some() {
                    self.focus_screen = false;
                }
                if any_hidden {
                    let r = quiet_button(ui, if self.show_hidden_apps { tr("Hide hidden games") } else { tr("Show hidden games") });
                    self.focus_item(&r, "show-hidden", shown.is_empty());
                    if r.clicked() {
                        self.show_hidden_apps = !self.show_hidden_apps;
                    }
                }
            }
        }
    }

    /// A PC's page with no list to show: one line (and what went wrong) and the action that reads the
    /// list again, in the middle of the room the covers would have had. A press holds the page as it
    /// is, its action at work, until the answer; only an empty list's (`empty`) stays that page.
    #[allow(clippy::too_many_arguments)]
    fn bare_list(&mut self, ui: &mut Ui, id: u64, line: &str, error: Option<&str>, action: &str, working: bool, empty: bool, commands: &mut Vec<Command>) {
        const GAP: f32 = 20.0;
        let top = ui.cursor().top();
        let room = (ui.ctx().content_rect().bottom() - top - BARE_FOOT).max(BARE_LEAST);
        let width = ui.available_width();
        let (area, _) = ui.allocate_exact_size(vec2(width, room), Sense::hover());
        let head = ui.painter().layout(line.to_owned(), semibold(t::TYPE_SECTION_SIZE), *ON_SURFACE, width.min(BARE_WORDS_W));
        let cause = error.map(|e| ui.painter().layout(e.to_owned(), regular(t::TYPE_BODY_SIZE), *ERROR, width.min(BARE_WORDS_W)));
        let words_h = head.size().y + cause.as_ref().map_or(0.0, |c| c.size().y + 6.0);
        let mut y = area.center().y - (words_h + GAP + ACTION_H) / 2.0;
        ui.painter().galley(pos2(area.center().x - head.size().x / 2.0, y), head.clone(), *ON_SURFACE);
        y += head.size().y;
        if let Some(cause) = cause {
            y += 6.0;
            ui.painter().galley(pos2(area.center().x - cause.size().x / 2.0, y), cause.clone(), *ERROR);
            y += cause.size().y;
        }
        y += GAP;
        let w = capsule_width(ui, action);
        let rect = Rect::from_min_size(pos2(area.center().x - w / 2.0, y), vec2(w, ACTION_H));
        let r = control_capsule(ui, rect, Id::unique(("bare-list", id)), Icon::Refresh, action, action, !working);
        if working {
            let spin = Rect::from_min_size(pos2(rect.left() + 20.0, rect.center().y - 9.5), Vec2::splat(19.0));
            ui.scope_builder(UiBuilder::new().max_rect(spin), |ui| spinner(ui, 9.0));
        }
        self.focus_item(&r, "bare-list", true);
        if r.clicked() {
            self.refreshing_empty = empty.then_some(id);
            commands.push(Command::Refresh(id));
        }
    }

    /// What a running game offers at the right of its PC's page: its name, Resume and Quit game. They
    /// hang on the row's centre, so the row keeps its height whether something runs or not. Returns
    /// how wide they are.
    pub(super) fn running_actions(&mut self, ui: &mut Ui, host: &Host, commands: &mut Vec<Command>) -> f32 {
        if host.running_app().is_none() {
            return 0.0;
        }
        let slot = ui.available_rect_before_wrap();
        let row = ui.min_rect();
        let mid = row.center().y;
        let name = at_sentence_start(&running_name(host));
        let quit_words = tr("Quit game");
        let quit_w = capsule_width(ui, quit_words);
        let resume = resume_target(host).map(|app| (app, action_width(ui, PcAction::Resume(app), f32::MAX)));
        let mut wide = quit_w + resume.map_or(0.0, |(_, w)| w + ACTIONS_GAP);
        let chip_w = info_chip_width(ui, Some(9.0), &name);
        // The title of the page comes first: a narrow window drops the chip, the dot on the cover and
        // Resume still say it.
        let chip = slot.right() - row.left() - chrome::BACK_SIZE - wide - chip_w - ACTIONS_GAP >= MIN_TITLE_ROOM;
        if chip {
            wide += chip_w + ACTIONS_GAP;
        }
        let mut x = slot.right() - wide;
        if chip {
            info_chip(ui, x, mid, Some(*OK), &name);
            x += chip_w + ACTIONS_GAP;
        }
        if let Some((app, w)) = resume {
            let rect = Rect::from_min_size(pos2(x, mid - ACTION_H / 2.0), vec2(w, ACTION_H));
            let said = trf("Resume {}", &[&name]);
            let r = action_button(ui, rect, Id::unique(("apps-resume", host.id)), PcAction::Resume(app), &said);
            self.focus_item(&r, "apps-resume", false);
            if r.clicked() {
                commands.push(PcAction::Resume(app).command(host.id));
            }
            x += w + ACTIONS_GAP;
        }
        let rect = Rect::from_min_size(pos2(x, mid - ACTION_H / 2.0), vec2(quit_w, ACTION_H));
        let r = control_capsule(ui, rect, Id::unique(("apps-quit", host.id)), Icon::Power, quit_words, quit_words, true);
        self.focus_item(&r, "apps-quit", false);
        if r.clicked() {
            self.dialog = Dialog::ConfirmQuit { host: host.id, start: None, running: running_name(host), resume: None, named: host.running_app() };
            self.focus_dialog = true;
        }
        wide
    }

    /// "TV wants to pair" while a PC this device may approve for has a device waiting.
    pub(super) fn pairing_banner(&mut self, ui: &mut Ui, view: &View<'_>) {
        let mut waiting = view.hosts.list.iter().flat_map(|h| h.waiting_to_pair().iter().map(move |r| (h, r)));
        let Some((host, request)) = waiting.next() else {
            return;
        };
        let more = waiting.count();
        let words = if more == 0 {
            trf("{} wants to pair", &[&short_device_name(&request.name)])
        } else {
            trf("{} devices want to pair", &[&(more + 1)])
        };
        let banner = button(ui, Some(Icon::Key), &words, Tone::Raised, Size::CONTROL);
        self.focus_item(&banner, "pairing-banner", false);
        if banner.clicked() {
            self.dialog = if more == 0 { approve_dialog(host, request) } else { Dialog::PairRequests { only: None } };
            self.focus_dialog = true;
        }
        ui.add_space(space::S);
    }

    /// Every game of every PC under its filter chips: All, Installed, each store, Other apps.
    pub(super) fn all_games(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let (entries, hidden) = console::split_hidden(console::merge_library(&sources), &sources, &self.hidden_games);
        let mut chips = console::library_filters(&entries);
        if !hidden.is_empty() {
            chips.push(console::FilterChip { filter: console::LibraryFilter::Hidden, store_name: None, count: hidden.len() });
        }
        let words = self.library_query.trim().to_lowercase();
        // A filter whose games went away falls back to all of them (with only hidden ones left, to those).
        if !chips.iter().any(|c| c.filter == self.library_filter) {
            self.library_filter = chips.first().map_or(console::LibraryFilter::All, |c| c.filter.clone());
        }
        // A chip exists only for games it shows: with none, the library itself is empty, or not
        // read yet.
        if chips.is_empty() {
            if library_loading(view) {
                loading_games(ui);
                return;
            }
            ui.label(RichText::new(tr("No games to show yet. They appear here once a PC that shares its game library is online.")).color(ON_SURFACE_VARIANT));
            return;
        }
        // The chips stay as they are while words narrow them, only their counts follow.
        count_chips(&mut chips, &entries, &hidden, &words);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(space::S, space::S);
            for chip in &chips {
                let name = match &chip.filter {
                    console::LibraryFilter::All => tr("All").to_owned(),
                    console::LibraryFilter::Installed => tr("Installed").to_owned(),
                    console::LibraryFilter::Store(key) => chip.store_name.clone().unwrap_or_else(|| console::store_name(key, "")),
                    console::LibraryFilter::Games => tr("Games").to_owned(),
                    console::LibraryFilter::Other => tr("Other apps").to_owned(),
                    console::LibraryFilter::Collection(name) => chip.store_name.clone().unwrap_or_else(|| name.clone()),
                    console::LibraryFilter::NoCollection => tr("No collection").to_owned(),
                    console::LibraryFilter::Hidden => tr("Hidden").to_owned(),
                };
                let selected = chip.filter == self.library_filter;
                // Known by the filter, not its place: the Installed chip coming or going moves the
                // others, and the focus stays on the one it was on.
                let key = chip.filter.key();
                let tone = if selected { Tone::Selected } else { Tone::Chip };
                let r = button_with(ui, Some(Id::unique(("all-games", &key))), None, &name, Some(chip.count), tone, Size::CHIP);
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, format!("{name}, {}", chip.count)));
                self.focus_item(&r, &key, false);
                if r.clicked() {
                    self.library_filter = chip.filter.clone();
                }
            }
        });
        // The room a raised cover's ring needs at the top of the scroll comes out of the gap.
        ui.add_space(space::L - LIFT);
        let pool = if self.library_filter == console::LibraryFilter::Hidden { &hidden } else { &entries };
        let shown = matching(pool, &self.library_filter, &words);
        // Installed is offered while anything is not, so with nothing installed it holds no game.
        if shown.is_empty() {
            ui.add_space(LIFT);
            ui.label(RichText::new(tr("No games match this filter.")).color(ON_SURFACE_VARIANT));
            return;
        }
        ui.add_space(LIFT);
        let (columns, width) = grid(ui.available_width());
        ui.spacing_mut().item_spacing.y = GRID_ROW_GAP;
        let mut first = true;
        for line in shown.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = POSTER_GAP;
                for entry in line {
                    self.library_cover(ui, view, None, entry, width, std::mem::take(&mut first), commands);
                }
            });
        }
        ui.add_space(space::L);
    }

    /// A game of the merged library as a line of the List layout; it opens and offers as its cover does.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn library_line(&mut self, ui: &mut Ui, view: &View<'_>, row: &str, entry: &console::Entry, line: &str, width: f32, first: bool, commands: &mut Vec<Command>) {
        let cover = entry.cover();
        let art = cover.and_then(|c| shown_art(view, c.host, c.app_id));
        let badge = if entry.running() {
            Badge::Running
        } else if !entry.installed() {
            Badge::NotInstalled
        } else {
            Badge::None
        };
        let response = list_row(ui, &entry.title, line, art, badge, width);
        if let Some(c) = cover {
            request_cover(ui, view, c.host, c.app_id, &c.cover_version, &response, commands);
        }
        let key = format!("{row}:game:{}", entry.key);
        self.focus_card(&response, &key, &entry.key, first);
        let best = entry.best();
        if response.clicked() {
            self.open(Screen::Game { host: best.host, app: best.app_id });
        }
        if let Some(anchor) = menu_request(ui, &response, None) {
            self.open_menu(Dialog::GameMenu { host: best.host, game: GameTarget::Library { app: best.app_id, favourite: false } }, anchor, key);
        }
    }

    /// A game of the merged library as a cover: selecting it opens its page, its menu offers the rest.
    /// On Home a game can show in several rows: `row` tells its cards apart for focus.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn library_cover(&mut self, ui: &mut Ui, view: &View<'_>, row: Option<&str>, entry: &console::Entry, width: f32, first: bool, commands: &mut Vec<Command>) {
        let cover = entry.cover();
        let art = cover.and_then(|c| shown_art(view, c.host, c.app_id));
        let badge = if entry.running() {
            Badge::Running
        } else if !entry.installed() {
            Badge::NotInstalled
        } else {
            Badge::None
        };
        // Home's shelves name their covers; the grid of all games lets the art say it.
        let response = if row.is_some() { poster(ui, &entry.title, art, badge, width) } else { bare_poster(ui, &entry.title, art, badge, width) };
        if let Some(c) = cover {
            request_cover(ui, view, c.host, c.app_id, &c.cover_version, &response, commands);
        }
        let key = match row {
            Some(row) => format!("{row}:game:{}", entry.key),
            None => format!("game:{}", entry.key),
        };
        self.focus_card(&response, &key, &entry.key, first);
        let image = Rect::from_min_size(response.rect.min, vec2(width, cover_height(width)));
        let dots = cover_dots(ui, &response, image, badge);
        let best = entry.best();
        if response.clicked() {
            self.open(Screen::Game { host: best.host, app: best.app_id });
        }
        if let Some(anchor) = menu_request(ui, &response, dots.as_ref()) {
            self.open_menu(Dialog::GameMenu { host: best.host, game: GameTarget::Library { app: best.app_id, favourite: false } }, anchor, key);
        }
    }

    /// Starts an entry of a PC's app list as selecting it does: a control tile asks first, the
    /// running game resumes, another game asks before it replaces the running one. A dialog to ask
    /// first is returned.
    pub(crate) fn start_app(&mut self, host: &Host, app: &nightjar_core::http::App, commands: &mut Vec<Command>) -> Option<Dialog> {
        let id = host.id;
        let running = host.running_app();
        let is_running = running.is_some_and(|r| app.matches_running_app(r, None));
        if app.is_control_action() {
            return Some(Dialog::ConfirmAction { host: id, app: app.id, name: app.name.clone() });
        }
        if app.control != nightjar_core::http::AppControl::None {
            commands.push(Command::Launch { host: id, app: app.id, resume: false });
        } else if is_running {
            commands.push(Command::Launch { host: id, app: app.id, resume: true });
        } else if running.is_some() && !host.confirms_replacement() {
            return Some(Dialog::ConfirmQuit {
                host: id,
                start: Some((app.id, app.name.clone())),
                running: running_name(host),
                resume: resume_target(host),
                named: host.running_app(),
            });
        } else {
            commands.push(Command::Launch { host: id, app: app.id, resume: false });
        }
        None
    }

    /// What a game can do on its PC now, for its Play, Favourite and Quit game. The game page and
    /// Home's hero read the same answer.
    pub(super) fn play_state(&self, host: &Host, source: &Source<'_>, game: &nightjar_core::http::native_api::library::Game) -> PlayState {
        let app_id = game.app_id;
        let detail = console::detail(source, app_id, host.native_api());
        let classic = match &host.apps {
            Apps::Loaded(apps) => {
                let pairs: Vec<(i32, &str)> = apps.iter().map(|a| (a.id, a.uuid.as_str())).collect();
                console::classic_app(&pairs, &game.install_id, app_id).and_then(|id| apps.iter().find(|a| a.id == id)).cloned()
            }
            _ => None,
        };
        let online = host.reach == Reach::Online;
        // The classic app list can know a run the native session does not.
        let classic_running = online
            && classic.as_ref().zip(host.running_app()).is_some_and(|(app, running)| app.matches_running_app(running, None));
        let resumes = detail.resumes() || classic_running;
        let other_running = if resumes {
            None
        } else {
            detail
                .other_running
                .clone()
                .or_else(|| if online { host.running_app().map(|_| running_name(host)) } else { None })
        };
        let can_play = detail.can_start && classic.is_some();
        PlayState { detail, classic, resumes, other_running, can_play }
    }

    /// Play or Resume: asks first when it would replace another running game.
    pub(super) fn press_play(&mut self, host: &Host, game: &nightjar_core::http::native_api::library::Game, state: &PlayState, commands: &mut Vec<Command>) {
        let Some(app) = &state.classic else {
            // The list starts a stream and is not here yet: Play holds until it lands.
            if state.could_play(host) {
                self.pending_play = Some(PendingPlay { host: host.id, app: game.app_id, screen: self.screen });
                commands.push(Command::LoadApps(host.id));
            }
            return;
        };
        self.pending_play = None;
        if state.resumes {
            commands.push(Command::Launch { host: host.id, app: app.id, resume: true });
        } else if let (Some(running), false) = (&state.other_running, host.confirms_replacement()) {
            self.dialog = Dialog::ConfirmQuit {
                host: host.id,
                start: Some((app.id, game.title.clone())),
                running: running.clone(),
                resume: resume_target(host),
                named: host.running_app(),
            };
            self.focus_dialog = true;
        } else {
            commands.push(Command::Launch { host: host.id, app: app.id, resume: false });
        }
    }

    /// Whether Play on this game is waiting for its PC's app list.
    pub(super) fn playing_soon(&self, host: u64, app: u32) -> bool {
        self.pending_play.is_some_and(|p| p.host == host && p.app == app)
    }

    /// A Play that waited for the list: starts once the entry is there, and says so out loud if it
    /// cannot be. Leaving the page where it was pressed lets it go; an open dialog holds it, so no
    /// stream starts under a menu and no typed text gets replaced.
    pub(super) fn settle_play(&mut self, view: &View<'_>, sources: &[Source<'_>], commands: &mut Vec<Command>) {
        let Some(pending) = self.pending_play else { return };
        if self.screen != pending.screen {
            self.pending_play = None;
            return;
        }
        if !matches!(self.dialog, Dialog::None) {
            return;
        }
        let host = view.hosts.get(pending.host);
        let source = sources.iter().find(|s| s.id == pending.host);
        let game = source.and_then(|s| s.library).and_then(|l| l.games.iter().find(|g| g.app_id == pending.app));
        let (Some(host), Some(source), Some(game)) = (host, source, game) else {
            self.pending_play = None;
            return;
        };
        let state = self.play_state(host, source, game);
        let why = if !state.detail.can_start {
            Some(pc_line_of(host).words().to_owned())
        } else {
            match &host.apps {
                Apps::NotLoaded | Apps::Loading => return,
                Apps::Failed(e) => Some(trf("The app list could not be loaded. {}", &[&e])),
                Apps::Loaded(_) if state.classic.is_none() => Some(tr("The PC's app list has no entry for this game.").to_owned()),
                Apps::Loaded(_) => None,
            }
        };
        self.pending_play = None;
        match why {
            Some(why) => self.message(tr("The stream could not start"), why),
            None => self.press_play(host, game, &state, commands),
        }
    }

    /// Quit game, through the native Close game where the PC allows it, else the classic quit.
    pub(super) fn press_end(&mut self, host: &Host, game: &nightjar_core::http::native_api::library::Game, state: &PlayState) {
        self.dialog = if state.detail.end == EndAction::CloseGame {
            Dialog::ConfirmClose { host: host.id, name: game.title.clone() }
        } else {
            Dialog::ConfirmQuit { host: host.id, start: None, running: game.title.clone(), resume: None, named: host.running_app() }
        };
        self.focus_dialog = true;
    }

    /// Leaves the current screen for the one it was opened from.
    pub(super) fn leave(&mut self) {
        let root = self.root();
        self.pop(root);
    }
}

/// The games of `pool` under `filter` whose title holds the lowercase `words`.
fn matching<'a>(pool: &'a [console::Entry], filter: &console::LibraryFilter, words: &str) -> Vec<&'a console::Entry> {
    let mut shown = console::filtered(pool, filter);
    if !words.is_empty() {
        shown.retain(|entry| entry.title.to_lowercase().contains(words));
    }
    shown
}

/// What each chip would show for the words in the search.
fn count_chips(chips: &mut [console::FilterChip], entries: &[console::Entry], hidden: &[console::Entry], words: &str) {
    if words.is_empty() {
        return;
    }
    for chip in chips {
        let pool = if chip.filter == console::LibraryFilter::Hidden { hidden } else { entries };
        chip.count = matching(pool, &chip.filter, words).len();
    }
}

/// "Loading the library…" with its spinner, over the middle of the places held for the covers.
fn loading_chip(ui: &mut Ui, room: Rect) {
    const H: f32 = 40.0;
    const R: f32 = 8.0;
    let words = ui.painter().layout_no_wrap(tr("Loading the library…").to_owned(), regular(text::LABEL), *ON_SURFACE);
    let rect = Rect::from_center_size(room.center(), vec2(14.0 + 2.0 * R + 10.0 + words.size().x + 18.0, H));
    let mut out = Vec::new();
    glass::control(&mut out, ui.ctx(), rect, H / 2.0);
    ui.painter().extend(out);
    let spinning = Rect::from_min_size(pos2(rect.left() + 14.0, rect.center().y - R), Vec2::splat(2.0 * R));
    ui.scope_builder(UiBuilder::new().max_rect(spinning), |ui| spinner(ui, R));
    ui.painter().galley(pos2(spinning.right() + 10.0, rect.center().y - words.size().y / 2.0), words, *ON_SURFACE);
}

/// A Play that waits for its PC's app list, and the page it was pressed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PendingPlay {
    pub host: u64,
    pub app: u32,
    pub screen: Screen,
}

/// What a game can do on its PC now ([`Launcher::play_state`]).
pub(super) struct PlayState {
    pub detail: console::Detail,
    /// Its entry in the PC's app list, which is what starts a stream.
    pub classic: Option<nightjar_core::http::App>,
    pub resumes: bool,
    /// The other game that playing this one would close.
    pub other_running: Option<String>,
    pub can_play: bool,
}

impl PlayState {
    /// Whether Play is on: the PC can start it, and its app list has the entry or may yet.
    pub(super) fn could_play(&self, host: &Host) -> bool {
        self.detail.can_start && (self.classic.is_some() || matches!(host.apps, Apps::NotLoaded | Apps::Loading))
    }
}

/// A game's cover as it should show now: what is known of it, and for a PC that is up and has yet to
/// be asked, on its way. A PC that is away sends none, so its covers without art are tiles.
pub(super) fn shown_art<'a>(view: &'a View<'_>, host: u64, app: u32) -> Option<&'a Art> {
    let pc = view.hosts.get(host)?;
    match pc.art.get(&(app as i32)) {
        Some(art) => Some(art),
        None if pc.reach == Reach::Online => Some(&Art::Loading),
        None => None,
    }
}

/// Whether a paired PC that is up and shares its library has not sent it yet.
pub(super) fn library_loading(view: &View<'_>) -> bool {
    view.hosts.list.iter().any(|h| {
        h.reach == Reach::Online && h.paired() && h.native_api() && h.native.library.is_none() && h.native.error.is_none()
    })
}

/// A Favourites card's focus key.
pub(super) fn favourite_key(item: &console::Starred) -> String {
    format!("favourite:{}:{}", item.host, item.app_id)
}

/// The card to focus once favourite `index` loses its star: the next, else the previous; None
/// when it was the last.
pub(super) fn favourite_neighbour(starred: &[console::Starred], index: usize) -> Option<String> {
    console::neighbour(starred.len(), index).map(|near| favourite_key(&starred[near]))
}

/// The card to focus once the game of card `key` leaves the screen: the next card of another game,
/// else the previous; None when there is none. `cards` are the screen's, in the order drawn, each
/// with its game: a game shown in several rows leaves all of them.
pub(super) fn card_neighbour(cards: &[(String, String)], key: &str) -> Option<String> {
    let at = cards.iter().position(|(card, _)| card == key)?;
    let game = &cards[at].1;
    cards[at + 1..].iter().chain(cards[..at].iter().rev()).find(|(_, other)| other != game).map(|(card, _)| card.clone())
}

/// "Loading the games…" by a spinner, on Home and All games alike.
pub(super) fn loading_games(ui: &mut Ui) {
    ui.horizontal(|ui| {
        spinner(ui, 10.0);
        ui.label(RichText::new(tr("Loading the games…")).color(ON_SURFACE_VARIANT));
    });
}

/// Quick resume: a paired PC running a game from its known library goes straight back into it.
pub(super) fn host_clicked(host: &Host, quick_resume: bool, commands: &mut Vec<Command>) {
    let running = running_entry(host).filter(|_| host.paired());
    if let Some(app) = running.filter(|_| nightjar_policy::launch::resumes_on_pc_tap(quick_resume, true)) {
        commands.push(Command::Launch { host: host.id, app: app.id, resume: true });
    } else if host.paired() {
        commands.push(Command::OpenApps(host.id));
    } else if host.reach == Reach::Offline {
        commands.push(Command::Refresh(host.id));
    } else {
        commands.push(Command::Pair(host.id));
    }
}

/// Whether a game offers Input-only through a small stream, and what blocks it now: only an
/// ordinary game on a PC without its own Input-only mode, with the setting on, that leaves the
/// PC's running game alone.
pub(super) fn small_stream_offer(host: &Host, app: &nightjar_core::http::App, running: bool, any_host: bool) -> Option<Option<nightjar_policy::launch::InputOnlyBlocker>> {
    use nightjar_policy::launch::{self, InputOnlyMode};
    let lists_input_only = matches!(&host.apps, Apps::Loaded(apps) if apps.iter().any(|a| a.role(false) == nightjar_core::http::AppRole::InputOnly));
    let tiles = host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::RemoteSessionTiles));
    let offered = app.control == nightjar_core::http::AppControl::None
        && launch::keeps_running_game(host.running_app().is_some(), running)
        && launch::input_only_mode(any_host, tiles, lists_input_only) == InputOnlyMode::SmallStream;
    let permission = host.info.as_ref().and_then(|i| i.permission).map(u64::from);
    offered.then(|| launch::small_stream_blocker(host.paired(), permission))
}

/// The launch policy's names for an app's role and control tile.
pub fn policy_role(role: nightjar_core::http::AppRole) -> nightjar_policy::launch::Role {
    use nightjar_core::http::AppRole;
    use nightjar_policy::launch::Role;
    match role {
        AppRole::Stream => Role::Stream,
        AppRole::RemoteMonitor => Role::RemoteMonitor,
        AppRole::InputOnly => Role::InputOnly,
    }
}

pub(super) fn policy_control(control: nightjar_core::http::AppControl) -> nightjar_policy::launch::Control {
    use nightjar_core::http::AppControl;
    use nightjar_policy::launch::Control;
    match control {
        AppControl::None => Control::None,
        AppControl::Resume => Control::Resume,
        AppControl::DisconnectMonitor => Control::DisconnectMonitor,
        AppControl::DisconnectInput => Control::DisconnectInput,
        AppControl::Terminate => Control::Terminate,
        AppControl::Monitor => Control::Monitor,
        AppControl::Input => Control::Input,
        AppControl::RunningGame => Control::RunningGame,
    }
}

/// The library entry of the app the PC is running, when its library is known.
pub(super) fn running_entry(host: &Host) -> Option<&nightjar_core::http::App> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None) && !a.is_control_action()),
        _ => None,
    }
}

/// A name that starts a sentence: the stand-in "the running app" gets its capital, a game's own
/// name stays as it is spelled.
pub fn at_sentence_start(name: &str) -> String {
    if name != tr("the running app") {
        return name.to_owned();
    }
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The game's title and the device playing it, when selecting `app` asks first because another
/// device plays that very game. Never on a PC that does not offer moving the stream.
pub fn playing_elsewhere(host: &Host, app: i32, read: &nightjar_core::http::StreamMove) -> Option<(String, String)> {
    let offered = host.info.as_ref().is_some_and(|info| {
        info.capabilities.availability(nightjar_core::http::Feature::ContinueHere) == nightjar_core::http::Availability::Supported
    });
    let playing = read.playing_on.as_ref();
    let known = match &host.apps {
        crate::hosts::Apps::Loaded(apps) => apps.iter().find(|known| known.id == app),
        _ => None,
    };
    let same = playing.is_some_and(|playing| plays_this_game(known, app, playing));
    if !nightjar_policy::continue_here::asks_on_select(offered, true, same) {
        return None;
    }
    playing.map(|playing| (playing.app.title.clone(), playing.device.clone()))
}

/// The app a join starts: `app`, or the game the PC says runs when `app` is a tile that only names
/// a way to reach it (a stream's Resume or Running Game).
pub fn join_target(known: Option<&nightjar_core::http::App>, app: i32, playing: Option<&nightjar_core::http::PlayingOn>) -> i32 {
    let tile = known.is_some_and(|known| known.control != nightjar_core::http::AppControl::None);
    match playing {
        Some(playing) if tile => playing.app.app_id as i32,
        _ => app,
    }
}

/// What the prompt about another device's game starts when answered, app and resume: a tile that
/// only names a way to reach the game resumes the game itself, since the stream refuses a resume of
/// anything but the app the PC runs.
pub fn asked_start(known: Option<&nightjar_core::http::App>, app: i32, resume: bool, playing: Option<&nightjar_core::http::PlayingOn>) -> (i32, bool) {
    let target = join_target(known, app, playing);
    (target, resume || target != app)
}

/// The same game by its install id where both sides have one, as a stream's resume decides it: an
/// id alone can have been given to another game since the list was read. A tile that only names a
/// way to reach the game (a stream's Resume or Running Game) means whatever runs, as `join_target`
/// has it.
pub(super) fn plays_this_game(known: Option<&nightjar_core::http::App>, app: i32, playing: &nightjar_core::http::PlayingOn) -> bool {
    use nightjar_core::http::AppControl;
    match known {
        Some(known) if matches!(known.control, AppControl::Resume | AppControl::RunningGame) => true,
        Some(known) => known.matches_running_app(playing.app.app_id as i32, Some(&playing.app.install_id)),
        None => playing.app.app_id as i32 == app,
    }
}

pub fn running_name(host: &Host) -> String {
    running_entry(host).map_or_else(|| tr("the running app").into(), |app| app.name.clone())
}

/// The classic app id of the app the PC runs now, once the app list is loaded.
pub(super) fn running_app_id(host: &Host) -> Option<i32> {
    running_entry(host).map(|app| app.id)
}

/// What a Resume names: the list's entry, else the id the PC reports, which is all a resume needs.
pub(super) fn resume_target(host: &Host) -> Option<i32> {
    running_app_id(host).or(host.running_app())
}

/// Where the game page's stream size, rate and bitrate come from, for those not from the main
/// settings: a PC or profile override would otherwise hold them out of sight of the game's
/// settings ("Frame rate set for this PC").
pub(super) fn summary_sources(profile: Option<&crate::config::Profile>, pc: Option<&Overrides>, game: Option<&Overrides>) -> Vec<String> {
    use crate::config::{source, Source as Layer};
    let rows: [(&str, &[&str]); 3] = [(en("Resolution"), &["width", "height"]), (en("Frame rate"), &["fps"]), (en("Bitrate"), &["bitrate_kbps"])];
    rows.into_iter()
        .filter_map(|(name, keys)| match source(keys, profile, pc, game) {
            Layer::Main => None,
            Layer::Game => Some(trf("{} set for this game", &[&tr(name)])),
            Layer::Pc => Some(trf("{} set for this PC", &[&tr(name)])),
            Layer::Profile(profile) => Some(trf("{} from profile {}", &[&tr(name), &profile])),
        })
        .collect()
}
