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
    /// Under an empty library, the way to games: wake a paired PC that can be woken, else go to the
    /// PCs, else add one.
    pub(super) fn empty_library_way(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let asleep = view.hosts.list.iter().find(|h| h.paired() && h.reach == Reach::Offline && h.record.can_wake());
        ui.add_space(space::S);
        let (words, key) = match asleep {
            Some(host) => (trf("Wake {}", &[&host.display_name()]), "empty-library-wake"),
            None if !view.hosts.list.is_empty() => (en("Go to PCs").to_owned(), "empty-library-pcs"),
            None => (en("Add PC").to_owned(), "empty-library-add"),
        };
        let r = ui.horizontal(|ui| quiet_button(ui, &words)).inner;
        self.focus_item(&r, key, true);
        if r.clicked() {
            match asleep {
                Some(host) => commands.push(Command::Wake(host.id)),
                None if !view.hosts.list.is_empty() => self.go(Screen::Hosts),
                None => self.add_host_dialog(),
            }
        }
    }

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
        let (columns, width) = grid_up_to(ui.available_width(), self.cover_widest());
        ui.spacing_mut().item_spacing.y = GRID_ROW_GAP;
        match &host.apps {
            // Read again from an empty list: the same place, its button at work.
            Apps::NotLoaded | Apps::Loading if self.refreshing_empty == Some(id) => {
                self.bare_list(ui, id, tr("This PC lists no apps."), None, tr("Refresh"), true, false, commands);
            }
            Apps::NotLoaded | Apps::Loading => {
                // The covers' own places, so the list lands without a jump, under one word of what is
                // going on.
                loading_covers(ui, tr("Loading the library…"), LOADING_ROWS, false, self.cover_widest());
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
                let hides = |app: &nightjar_policy::http::App| console::is_hidden(&source, &app.uuid, app.id as u32, &self.hidden_games);
                let any_hidden = apps.iter().any(hides);
                let shown: Vec<&nightjar_policy::http::App> = apps.iter().filter(|app| self.show_hidden_apps || !hides(app)).collect();
                // The running game lights the page from its cover, on screen or not (`page_backdrop`).
                if let Some(app) = running_app_id(host) {
                    request_cover_unseen(ui, view, id, app as u32, commands);
                }
                for (row, chunk) in shown.chunks(columns).enumerate() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = POSTER_GAP;
                        for (i, app) in chunk.iter().enumerate() {
                            let is_running = running.is_some_and(|r| app.matches_running_app(r, None));
                            let art = shown_art(view, id, app.id as u32);
                            let response = bare_poster(ui, &app.name, art, if is_running { Badge::Running } else { Badge::None }, width);
                            request_cover(ui, view, id, app.id as u32, &response, commands);
                            let first = if running.is_some() { is_running } else { row == 0 && i == 0 };
                            let key = format!("app:{}", app.id);
                            self.focus_card(&response, &key, &key, first);
                            let dots = cover_dots(ui, &response, response.rect, if is_running { Badge::Running } else { Badge::None });
                            if response.clicked()
                                && let Some(ask) = self.start_app(host, app, commands) {
                                    self.ask(ask);
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

    /// The right of a PC's page: what a running game offers, then the list's round Refresh. Returns
    /// how wide they are.
    pub(super) fn apps_head(&mut self, ui: &mut Ui, host: &Host, commands: &mut Vec<Command>) -> f32 {
        let refresh = head_refresh(host, self.refreshing_empty);
        let kept = refresh.map_or(0.0, |_| ACTION_H);
        let running = self.running_actions(ui, host, kept + ACTIONS_GAP, commands);
        let Some(working) = refresh else { return running };
        let slot = ui.available_rect_before_wrap();
        let rect = Rect::from_min_size(pos2(slot.right() - ACTION_H, ui.min_rect().center().y - ACTION_H / 2.0), Vec2::splat(ACTION_H));
        let r = round_button_working(ui, rect, Icon::Refresh, tr("Refresh"), working);
        self.focus_item(&r, "apps-refresh", false);
        if r.clicked() && !working {
            commands.push(Command::Refresh(host.id));
        }
        running + kept + if running > 0.0 { ACTIONS_GAP } else { 0.0 }
    }

    /// What a running game offers at the right of its PC's page, `kept` from its edge: its name, Resume
    /// and Quit game. They hang on the row's centre, so the row keeps its height whether something runs
    /// or not. Returns how wide they are.
    fn running_actions(&mut self, ui: &mut Ui, host: &Host, kept: f32, commands: &mut Vec<Command>) -> f32 {
        if host.running_app().is_none() {
            return 0.0;
        }
        let mut slot = ui.available_rect_before_wrap();
        slot.max.x -= kept;
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
        // The search's keys for a controller where the system shows none: Down from the field
        // lands on them first.
        if self.search_open && self.wants_text_pad() {
            let first = text_pad(ui, &mut self.library_query, None);
            self.focus_item(&first, "search-pad", true);
            ui.add_space(space::M);
        }
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let library = self.library.get(&sources, &self.hidden_games);
        let (entries, hidden) = (library.shown(), library.hidden());
        let mut chips = console::library_filters(entries);
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
                ui.add_space(space::L);
                loading_covers(ui, tr("Loading the games…"), LOADING_ROWS, true, self.cover_widest());
                return;
            }
            ui.label(RichText::new(tr("No games to show yet. They appear here once a PC that shares its game library is online.")).color(ON_SURFACE_VARIANT));
            self.empty_library_way(ui, view, commands);
            return;
        }
        // While words narrow them each chip counts what it would show, and one that would show
        // nothing goes; All stays, and so does the chip chosen.
        count_chips(&mut chips, entries, hidden, &words);
        if !words.is_empty() {
            chips.retain(|c| c.count > 0 || c.filter == console::LibraryFilter::All || c.filter == self.library_filter);
        }
        // IGDB's facts, with the player's keys: asked for once per game; the Genre and Length
        // chips come once some game has them.
        if view.hosts.facts_offered() {
            let wanted: Vec<_> = entries.iter().chain(hidden).map(entry_game).filter(|(key, _)| !view.hosts.facts_asked(key)).map(|(_, game)| game).collect();
            if !wanted.is_empty() {
                commands.push(Command::WantFacts(wanted));
            }
        }
        let facts_of = |entry: &console::Entry| view.hosts.facts(&entry_game(entry).0);
        let pool_facts: Vec<_> = entries.iter().map(facts_of).collect();
        let genres = nightjar_policy::game_info::library::genres(&pool_facts);
        let lengths = nightjar_policy::game_info::library::lengths(&pool_facts);
        // A choice whose games went away is dropped.
        if self.genre_filter.as_ref().is_some_and(|g| !genres.iter().any(|c| c.value.eq_ignore_ascii_case(g))) {
            self.genre_filter = None;
        }
        if self.length_filter.is_some_and(|l| !lengths.iter().any(|c| c.value == l)) {
            self.length_filter = None;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(space::S, space::S);
            for chip in &chips {
                let name = match &chip.filter {
                    console::LibraryFilter::All => tr("All").to_owned(),
                    console::LibraryFilter::Installed => tr("Installed").to_owned(),
                    console::LibraryFilter::NotInstalled => tr("Not installed").to_owned(),
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
            if !genres.is_empty() {
                let name = self.genre_filter.clone().unwrap_or_else(|| tr("Genre").to_owned());
                let tone = if self.genre_filter.is_some() { Tone::Selected } else { Tone::Chip };
                let r = button_with(ui, Some(Id::unique(("all-games", "genre"))), None, &name, None, tone, Size::CHIP);
                self.focus_item(&r, "genre", false);
                let mut picked = None;
                let _ = egui::Popup::menu(&r).show(|ui| {
                    if ui.selectable_label(self.genre_filter.is_none(), tr("Any genre")).clicked() {
                        picked = Some(None);
                    }
                    for choice in &genres {
                        let on = self.genre_filter.as_ref().is_some_and(|g| g.eq_ignore_ascii_case(&choice.value));
                        if ui.selectable_label(on, format!("{} ({})", choice.value, choice.count)).clicked() {
                            picked = Some(Some(choice.value.clone()));
                        }
                    }
                });
                if let Some(genre) = picked {
                    self.genre_filter = genre;
                }
            }
            if !lengths.is_empty() {
                let name = self.length_filter.map_or_else(|| tr("Length").to_owned(), |l| tr(length_name(l)).to_owned());
                let tone = if self.length_filter.is_some() { Tone::Selected } else { Tone::Chip };
                let r = button_with(ui, Some(Id::unique(("all-games", "length"))), None, &name, None, tone, Size::CHIP);
                self.focus_item(&r, "length", false);
                let mut picked = None;
                let _ = egui::Popup::menu(&r).show(|ui| {
                    if ui.selectable_label(self.length_filter.is_none(), tr("Any length")).clicked() {
                        picked = Some(None);
                    }
                    for choice in &lengths {
                        if ui.selectable_label(self.length_filter == Some(choice.value), format!("{} ({})", tr(length_name(choice.value)), choice.count)).clicked() {
                            picked = Some(Some(choice.value));
                        }
                    }
                });
                if let Some(length) = picked {
                    self.length_filter = length;
                }
            }
        });
        // The room a raised cover's ring needs at the top of the scroll comes out of the gap.
        ui.add_space(space::L - LIFT);
        let pool = if self.library_filter == console::LibraryFilter::Hidden { hidden } else { entries };
        let played = console::Played { sources: &sources, recents: &self.recents };
        let mut shown = matching(pool, &self.library_filter, &words, console::Sort::shown(&self.settings, entries), played);
        let (genre, length) = (self.genre_filter.as_deref(), self.length_filter);
        shown.retain(|entry| nightjar_policy::game_info::library::shows(facts_of(entry), genre, length));
        // Installed is offered while anything is not, so with nothing installed it holds no game.
        if shown.is_empty() {
            ui.add_space(LIFT);
            ui.label(RichText::new(tr("No games match this filter.")).color(ON_SURFACE_VARIANT));
            return;
        }
        let (columns, width) = grid_up_to(ui.available_width(), self.cover_widest());
        // A focused cover rises from its foot with its ring around it: the first line keeps that
        // clear of the chips, with a gap.
        ui.add_space(ring_clearance(width).max(LIFT));
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
            request_cover(ui, view, c.host, c.app_id, &response, commands);
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
        // Named under the cover, on Home's shelves and in the grid of all games alike.
        let response = poster(ui, &entry.title, art, badge, width);
        if let Some(c) = cover {
            request_cover(ui, view, c.host, c.app_id, &response, commands);
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
    pub fn start_app(&mut self, host: &Host, app: &nightjar_policy::http::App, commands: &mut Vec<Command>) -> Option<Dialog> {
        let id = host.id;
        let running = host.running_app();
        let is_running = running.is_some_and(|r| app.matches_running_app(r, None));
        if app.is_control_action() {
            return Some(Dialog::ConfirmAction { host: id, app: app.id, name: app.name.clone() });
        }
        if app.control != nightjar_policy::http::AppControl::None {
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
    pub(super) fn play_state(&self, host: &Host, source: &Source<'_>, game: &rubylight_native_api::library::Game) -> PlayState {
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
    pub(super) fn press_play(&mut self, host: &Host, game: &rubylight_native_api::library::Game, state: &PlayState, commands: &mut Vec<Command>) {
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
        } else if matches!(host.apps, Apps::NotLoaded | Apps::Loading) {
            return;
        } else {
            state.list_problem(host)
        };
        self.pending_play = None;
        match why {
            Some(why) => self.message(tr("The stream could not start"), why),
            None => self.press_play(host, game, &state, commands),
        }
    }

    /// Try again beside a Play the list stopped: reads the list again, whatever it last said.
    pub(super) fn list_retry(&mut self, ui: &mut Ui, rect: Rect, key: &'static str, first: bool, host: u64, commands: &mut Vec<Command>) {
        let words = tr("Try again");
        let r = control_capsule(ui, rect, Id::unique(key), Icon::Refresh, words, words, true);
        self.focus_item(&r, key, first);
        if r.clicked() {
            commands.push(Command::ReadAppsAgain(host));
        }
    }

    /// Quit game, through the native Close game where the PC allows it, else the classic quit.
    pub(super) fn press_end(&mut self, host: &Host, game: &rubylight_native_api::library::Game, state: &PlayState) {
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

    /// A cover's widest in a PC's library and All games: Home's card size, which Settings' Use
    /// small box art sets (Android's `checkbox_small_icon_mode`).
    pub(super) fn cover_widest(&self) -> f32 {
        POSTER_MAX_W * self.settings.home_card_size.scale()
    }
}

mod helpers;

pub use helpers::{
    asked_start, at_sentence_start, join_target, playing_elsewhere, policy_role, running_name,
};
pub(super) use helpers::{
    card_neighbour, entry_game, favourite_key, favourite_neighbour, head_refresh, host_clicked,
    length_name, library_loading, loading_covers, policy_control,
    resume_target, ring_clearance, running_app_id, running_entry, shown_art,
    small_stream_offer, summary_sources, PendingPlay, PlayState,
};
#[cfg(test)]
pub(super) use helpers::plays_this_game;
use helpers::{count_chips, matching};
