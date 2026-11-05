//! PCs, Home and a PC's library: the pages that list PCs and games, a game's page, and what
//! selecting a PC or game does.

use super::dialogs::{approve_dialog, short_device_name};
use super::*;

/// How wide a List line grows: past it, the store and the date sit too far from the title.
const LIST_MAX_W: f32 = 760.0;

impl Launcher {
    pub(super) fn add_host_dialog(&mut self) {
        self.dialog = Dialog::AddHost {
            text: String::new(),
            error: None,
        };
        self.focus_dialog = true;
    }

    pub(super) fn hosts(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        ui.label(RichText::new(tr("Your PCs")).font(semibold(text::TITLE)).color(ON_SURFACE));
        ui.label(
            RichText::new(tr("PCs running a Moonlight-compatible host on this network appear here. Add one by address if it doesn't."))
                .color(ON_SURFACE_VARIANT),
        );
        ui.add_space(18.0);
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                if self.identity_ready {
                    ui.label(RichText::new(tr("Looking for PCs…")).font(semibold(text::CARD_TITLE)));
                } else {
                    ui.label(RichText::new(tr("Creating this device's pairing key…")).font(semibold(text::CARD_TITLE)));
                }
                ui.label(
                    RichText::new(tr("Start a Moonlight-compatible host on your PC and keep both on the same network."))
                        .color(ON_SURFACE_VARIANT),
                );
                ui.add_space(12.0);
                let r = primary_button(ui, "Add a PC by address");
                if self.focus_screen {
                    r.request_focus();
                    self.focus_screen = false;
                }
                if r.clicked() {
                    self.dialog = Dialog::AddHost {
                        text: String::new(),
                        error: None,
                    };
                    self.focus_dialog = true;
                }
                how_to_connect(ui, commands);
            });
            return;
        }
        scroll(ui, "hosts", |ui| {
            let (columns, width) = card_grid(ui.available_width(), CARD_GAP);
            for (row, chunk) in view.hosts.list.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CARD_GAP;
                    for (i, host) in chunk.iter().enumerate() {
                        let response = host_card(ui, host, None, width);
                        let options = options_button(ui, response.rect, response.id.with("options"));
                        let key = format!("host:{}", host.id);
                        self.focus_item(&response, &key, row == 0 && i == 0);
                        self.focus_item(&options, &format!("options:{}", host.id), false);
                        if response.clicked() {
                            host_clicked(host, self.settings.quick_resume, commands);
                        }
                        if let Some(anchor) = menu_request(ui, &response, Some(&options)) {
                            self.open_menu(Dialog::HostOptions { host: host.id }, anchor, key);
                        }
                    }
                });
                ui.add_space(CARD_GAP);
            }
            // A short list may not be all of them: say the search goes on and where help is.
            if view.hosts.list.len() <= columns {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S;
                    ui.label(RichText::new(tr("Looking for other PCs on this network")).color(ON_SURFACE_VARIANT));
                    let help = how_to_connect(ui, commands);
                    self.focus_item(&help, "how-to-connect", false);
                });
            }
            ui.add_space(space::L);
        });
    }

    pub(super) fn apps(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(id) else {
            self.go(Screen::Hosts);
            return;
        };
        let running = host.running_app();
        match &host.apps {
            Apps::NotLoaded | Apps::Loading => {
                ui.add_space(40.0);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(tr("Loading the library…")).color(ON_SURFACE_VARIANT));
                });
            }
            Apps::Failed(e) => {
                ui.label(RichText::new(tr("The library could not be loaded.")).font(semibold(text::CARD_TITLE)));
                ui.label(RichText::new(e).color(ERROR));
                if quiet_button(ui, "Try again").clicked() {
                    commands.push(Command::Refresh(id));
                }
            }
            Apps::Loaded(apps) => {
                if apps.is_empty() {
                    ui.label(RichText::new(tr("This PC lists no apps.")).color(ON_SURFACE_VARIANT));
                    return;
                }
                scroll(ui, "apps", |ui| {
                    ui.add_space(LIFT);
                    let (columns, width) = grid(ui.available_width());
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
                                let art = host.art.get(&app.id);
                                let response = poster(ui, &app.name, art, if is_running { Badge::Running } else { Badge::None }, width);
                                let first = if running.is_some() { is_running } else { row == 0 && i == 0 };
                                let key = format!("app:{}", app.id);
                                self.focus_card(&response, &key, &key, first);
                                let image = Rect::from_min_size(response.rect.min, vec2(width, (width * 4.0 / 3.0).round()));
                                let dots = cover_dots(ui, &response, image);
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
                        ui.add_space(space::XS);
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
                });
            }
        }
    }

    /// "TV wants to pair" while a PC this device may approve for has a device waiting.
    fn pairing_banner(&mut self, ui: &mut Ui, view: &View<'_>) {
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

    pub(super) fn home(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                ui.label(RichText::new(tr("No PCs yet")).font(semibold(text::CARD_TITLE)));
                ui.label(RichText::new(tr("Pair Nightjar with a PC to see its games here.")).color(ON_SURFACE_VARIANT));
                ui.add_space(12.0);
                let r = primary_button(ui, "Open PCs");
                self.focus_item(&r, "open-hosts", true);
                if r.clicked() {
                    self.open(Screen::Hosts);
                }
                how_to_connect(ui, commands);
            });
            return;
        }
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let (entries, _) = console::split_hidden(console::merge_library(&sources), &sources, &self.hidden_games);
        let group = console::shown_group(self.settings.home_group, &entries);
        let rows = console::home_rows(&entries, group);
        let playing = console::continue_playing(&sources);
        // A running game stays under Continue playing: hiding must not lose the way back to it.
        let recent = console::recently_shown(&sources, &self.recents, &playing, &self.hidden_games, 6);
        let starred: Vec<_> = console::favourite_games(&sources, &self.favourites).into_iter().filter(|s| console::not_hidden(&sources, s.host, s.app_id, &self.hidden_games)).collect();
        let loading = library_loading(view);
        scroll(ui, "home", |ui| {
            // A failed read stays on screen beside the last good data.
            for host in &view.hosts.list {
                if let Some(error) = &host.native.error {
                    ui.label(RichText::new(format!("{}: {}", host.display_name(), tr(error))).color(ERROR));
                }
            }
            self.pairing_banner(ui, view);
            let width = ui.available_width();
            let (columns, cover_w) = grid_up_to(width, POSTER_MAX_W * self.settings.home_card_size.scale());
            let (_, card_w) = card_grid(width, CARD_GAP);
            let mut first = true;
            if !starred.is_empty() {
                shelf(ui, tr("Favourites"), None);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                    for (index, item) in starred.iter().enumerate() {
                        let line = if item.online { trf("On {}", &[&item.host_name]) } else { trf("{} is away", &[&item.host_name]) };
                        let response = continue_card(ui, &item.title, &line, false, card_w);
                        let key = favourite_key(item);
                        let game = console::entry_key_of(&entries, item.host, item.app_id).unwrap_or(&key);
                        self.focus_card(&response, &key, game, std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                        if response.has_focus() && unstar_pressed(ui) {
                            commands.push(Command::ToggleFavourite(item.favourite.clone()));
                            // The focus moves to the next favourite (else the previous), not back to the top.
                            match favourite_neighbour(&starred, index) {
                                Some(near) => self.restore_key = Some(near),
                                None => self.focus_screen = true,
                            }
                        }
                        if let Some(anchor) = menu_request(ui, &response, None) {
                            self.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app_id, favourite: true } }, anchor, key);
                        }
                    }
                });
            }
            if !playing.is_empty() {
                shelf(ui, tr("Continue playing"), None);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                    for item in &playing {
                        let (line, running) = match item.playing {
                            Playing::Running => (trf("Running on {}", &[&item.host_name]), true),
                            Playing::Suspended => (trf("Suspended on {}", &[&item.host_name]), false),
                        };
                        let response = continue_card(ui, &item.title, &line, running, card_w);
                        let key = format!("continue:{}:{}", item.host, item.app_id);
                        self.focus_item(&response, &key, std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                        if let Some(anchor) = menu_request(ui, &response, None) {
                            self.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app_id, favourite: false } }, anchor, key);
                        }
                    }
                });
            }
            if !recent.is_empty() {
                shelf(ui, tr("Recently played"), None);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                    for item in &recent {
                        let line = if item.online { trf("On {}", &[&item.host_name]) } else { trf("{} is away", &[&item.host_name]) };
                        let response = continue_card(ui, &item.title, &line, false, card_w);
                        let key = format!("recent:{}:{}", item.host, item.app_id);
                        let game = console::entry_key_of(&entries, item.host, item.app_id).unwrap_or(&key);
                        self.focus_card(&response, &key, game, std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                        if let Some(anchor) = menu_request(ui, &response, None) {
                            self.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app_id, favourite: false } }, anchor, key);
                        }
                    }
                });
            }
            if entries.is_empty() {
                shelf(ui, tr("Library"), None);
                if loading {
                    loading_games(ui);
                } else {
                    ui.label(RichText::new(tr("No games to show yet. They appear here once a PC that shares its game library is online.")).color(ON_SURFACE_VARIANT));
                }
            }
            // One row per store (or one for the library), as many covers as fit and an All tile for
            // the rest. The Grid view shows each row whole, as does a library of one kind by store.
            let whole = self.settings.home_layout == HomeLayout::Grid || (rows.len() == 1 && group == HomeGroup::Store);
            for row in &rows {
                let name = match &row.kind {
                    console::RowKind::Store { name, .. } => name.as_str(),
                    console::RowKind::Games => tr("Games"),
                    console::RowKind::Other => tr("Other apps"),
                    console::RowKind::Library => tr("Library"),
                    console::RowKind::Collection { name } => name.as_str(),
                    console::RowKind::NoCollection => tr("No collection"),
                };
                shelf(ui, name, Some(row.games.len()));
                // The List layout: every game of the row as a line, under each other.
                if self.settings.home_layout == HomeLayout::List {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    for entry in &row.games {
                        let line = console::list_line(entry, console::last_played(entry, &sources, &self.recents), now);
                        self.library_line(ui, view, &row.key(), entry, &line, width.min(LIST_MAX_W), std::mem::take(&mut first), commands);
                        ui.add_space(space::XS);
                    }
                    continue;
                }
                let (shown, more) = if whole { (row.games.len(), false) } else { row.fit(columns) };
                // What a screen reader says for the tile: whose games it opens, and how many.
                let said = match &row.kind {
                    console::RowKind::Store { name, .. } | console::RowKind::Collection { name } => trf("All {} games, {}", &[name, &row.games.len()]),
                    console::RowKind::Other => trf("All other apps, {}", &[&row.games.len()]),
                    console::RowKind::Games | console::RowKind::Library | console::RowKind::NoCollection => trf("All games, {}", &[&row.games.len()]),
                };
                let games = &row.games[..shown];
                // At one column no cover fits beside the tile: the tile is the row.
                let lines: Vec<&[&console::Entry]> = if games.is_empty() && more { vec![games] } else { games.chunks(columns).collect() };
                for line in lines {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = POSTER_GAP;
                        for entry in line {
                            self.library_cover(ui, view, Some(&row.key()), entry, cover_w, std::mem::take(&mut first), commands);
                        }
                        if more && line.len() < columns {
                            let tile = all_tile(ui, row.games.len(), &said, cover_w);
                            self.focus_item(&tile, &format!("all:{}", row.key()), std::mem::take(&mut first));
                            if tile.clicked() {
                                self.library_filter = row.filter.clone();
                                self.open(Screen::AllGames);
                            }
                        }
                    });
                    ui.add_space(space::XS);
                }
            }
            shelf(ui, tr("PCs"), None);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                for (host, source) in view.hosts.list.iter().zip(&sources) {
                    // A PC to pair has no games to speak of yet: its state alone fits the card.
                    let line = if host.paired() { console::host_line(source).text() } else { String::new() };
                    let response = host_card(ui, host, Some(&line), card_w);
                    let key = format!("host:{}", host.id);
                    self.focus_item(&response, &key, std::mem::take(&mut first));
                    if response.clicked() {
                        host_clicked(host, self.settings.quick_resume, commands);
                    }
                    if let Some(anchor) = menu_request(ui, &response, None) {
                        self.open_menu(Dialog::HostOptions { host: host.id }, anchor, key);
                    }
                }
            });
            ui.add_space(space::L);
        });
    }

    /// Every game of every PC under its filter chips: All, Installed, each store, Other apps.
    pub(super) fn all_games(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let (entries, hidden) = console::split_hidden(console::merge_library(&sources), &sources, &self.hidden_games);
        let mut chips = console::library_filters(&entries);
        if !hidden.is_empty() {
            chips.push(console::FilterChip { filter: console::LibraryFilter::Hidden, store_name: None, count: hidden.len() });
        }
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
        let shown = console::filtered(pool, &self.library_filter);
        // Installed is offered while anything is not, so with nothing installed it holds no game.
        if shown.is_empty() {
            ui.add_space(LIFT);
            ui.label(RichText::new(tr("No games match this filter.")).color(ON_SURFACE_VARIANT));
            return;
        }
        scroll(ui, "all-games", |ui| {
            ui.add_space(LIFT);
            let (columns, width) = grid(ui.available_width());
            let mut first = true;
            for line in shown.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = POSTER_GAP;
                    for entry in line {
                        self.library_cover(ui, view, None, entry, width, std::mem::take(&mut first), commands);
                    }
                });
                ui.add_space(space::XS);
            }
            ui.add_space(space::L);
        });
    }

    /// A game of the merged library as a line of the List layout; it opens and offers as its cover does.
    #[allow(clippy::too_many_arguments)]
    fn library_line(&mut self, ui: &mut Ui, view: &View<'_>, row: &str, entry: &console::Entry, line: &str, width: f32, first: bool, commands: &mut Vec<Command>) {
        let cover = entry.cover();
        let art = cover.and_then(|c| view.hosts.get(c.host).and_then(|h| h.art.get(&(c.app_id as i32))));
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
    fn library_cover(&mut self, ui: &mut Ui, view: &View<'_>, row: Option<&str>, entry: &console::Entry, width: f32, first: bool, commands: &mut Vec<Command>) {
        let cover = entry.cover();
        let art = cover.and_then(|c| view.hosts.get(c.host).and_then(|h| h.art.get(&(c.app_id as i32))));
        let badge = if entry.running() {
            Badge::Running
        } else if !entry.installed() {
            Badge::NotInstalled
        } else {
            Badge::None
        };
        let response = poster(ui, &entry.title, art, badge, width);
        if let Some(c) = cover {
            request_cover(ui, view, c.host, c.app_id, &c.cover_version, &response, commands);
        }
        let key = match row {
            Some(row) => format!("{row}:game:{}", entry.key),
            None => format!("game:{}", entry.key),
        };
        self.focus_card(&response, &key, &entry.key, first);
        let image = Rect::from_min_size(response.rect.min, vec2(width, (width * 4.0 / 3.0).round()));
        let dots = cover_dots(ui, &response, image);
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
                resume: running_app_id(host),
                named: host.running_app(),
            });
        } else {
            commands.push(Command::Launch { host: id, app: app.id, resume: false });
        }
        None
    }

    /// Leaves the current screen for the one it was opened from.
    pub(super) fn leave(&mut self) {
        let root = self.root();
        self.pop(root);
    }

    pub(super) fn game(&mut self, ui: &mut Ui, view: &View<'_>, host_id: u64, app_id: u32, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(host_id) else {
            self.leave();
            return;
        };
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let source = sources.iter().find(|s| s.id == host_id).expect("source of a listed host");
        let Some(game) = source.library.and_then(|l| l.games.iter().find(|g| g.app_id == app_id)) else {
            ui.label(RichText::new(tr("This game is no longer in the PC's library.")).font(semibold(text::CARD_TITLE)));
            let r = primary_button(ui, "Back");
            self.focus_item(&r, "back", true);
            if r.clicked() {
                self.leave();
            }
            return;
        };
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
        let merged = console::merge_library(&sources);
        let others: Vec<console::Copy> = merged
            .iter()
            .find(|e| e.copies.iter().any(|c| c.host == host_id && c.app_id == app_id))
            .map(|e| e.copies.iter().filter(|c| !(c.host == host_id && c.app_id == app_id)).cloned().collect())
            .unwrap_or_default();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H), Sense::hover());
            let art = host.art.get(&(app_id as i32));
            // The state chip beside the title says it; a badge on the cover would say it twice.
            paint_cover(ui, rect, &game.title, art, false);
            request_cover(ui, view, host_id, app_id, &game.artwork.version, &response, commands);
            ui.vertical(|ui| {
                ui.set_max_width(640.0);
                match detail.state {
                    GameState::Running => state_chip(ui, "Running", OK),
                    GameState::Suspended => state_chip(ui, "Suspended", TERTIARY),
                    GameState::Idle => {}
                }
                ui.label(RichText::new(&game.title).font(semibold(text::TITLE)).color(ON_SURFACE));
                // Hosts may send only the key: name it as the chips do (steam reads Steam).
                if let Some(store) = game.store.as_ref().map(|s| console::store_name(&s.source, &s.name)).filter(|n| !n.is_empty()) {
                    ui.label(RichText::new(store).color(ON_SURFACE_VARIANT));
                }
                pc_line(ui, source.name);
                ui.add_space(8.0);
                if host.reach == Reach::Offline {
                    ui.label(RichText::new(tr("This PC is offline. Your saved library is still here. Reconnect to play.")).color(ERROR));
                } else if host.reach == Reach::Unknown {
                    ui.label(RichText::new(tr("Checking the PC…")).color(ON_SURFACE_VARIANT));
                } else if !source.paired {
                    ui.label(RichText::new(tr("Pair Nightjar with this PC to play.")).color(ERROR));
                } else if !detail.can_start && !detail.installed {
                    // Play is off: say why, as Android's game page does.
                    ui.label(RichText::new(trf("Not installed on {}", &[&source.name])).color(ON_SURFACE_VARIANT));
                }
                // Only a game that can start would close it.
                if let Some(other) = other_running.as_ref().filter(|_| detail.can_start) {
                    ui.label(RichText::new(trf("{} is running. Playing this game closes it.", &[&at_sentence_start(other)])).color(TERTIARY));
                }
                ui.add_space(8.0);
                let can_play = detail.can_start && classic.is_some();
                ui.horizontal(|ui| {
                    let r = ui.add_enabled_ui(can_play, |ui| primary_button(ui, if resumes { en("Resume") } else { en("Play") })).inner;
                    self.focus_item(&r, "play", true);
                    if r.clicked() {
                        if let Some(app) = &classic {
                            if resumes {
                                commands.push(Command::Launch { host: host_id, app: app.id, resume: true });
                            } else if let (Some(running), false) = (&other_running, host.confirms_replacement()) {
                                self.dialog = Dialog::ConfirmQuit {
                                    host: host_id,
                                    start: Some((app.id, game.title.clone())),
                                    running: running.clone(),
                                    resume: running_app_id(host),
                                    named: host.running_app(),
                                };
                                self.focus_dialog = true;
                            } else {
                                commands.push(Command::Launch { host: host_id, app: app.id, resume: false });
                            }
                        }
                    }
                    let star = console::favourite_of(source, game);
                    let starred = console::is_starred(source, game, &self.favourites);
                    let r = ui
                        .add_enabled_ui(star.is_some(), |ui| {
                            quiet_button(ui, if starred { en("Remove from favourites") } else { en("Add to favourites") })
                        })
                        .inner;
                    // With Play off (not installed, the PC away) and nothing to end, the star takes the
                    // focus, so the D-pad has somewhere to start.
                    self.focus_item(&r, "favourite", !can_play && detail.end == EndAction::None);
                    if r.clicked() {
                        if let Some(star) = star {
                            commands.push(Command::ToggleFavourite(star));
                        }
                    }
                    if detail.end != EndAction::None {
                        let closing = host.native.acting;
                        let label = if detail.end == EndAction::CloseGame { en("Close game") } else { en("Quit game") };
                        let r = ui.add_enabled_ui(!closing, |ui| quiet_button(ui, label)).inner;
                        self.focus_item(&r, "end", !can_play);
                        if r.clicked() {
                            self.dialog = if detail.end == EndAction::CloseGame {
                                Dialog::ConfirmClose { host: host_id, name: game.title.clone() }
                            } else {
                                Dialog::ConfirmQuit { host: host_id, start: None, running: game.title.clone(), resume: None, named: host.running_app() }
                            };
                            self.focus_dialog = true;
                        }
                        if closing {
                            spinner(ui, 9.0);
                        }
                    }
                });
                // Why Play is not there yet: the app list is what starts a stream.
                if detail.can_start && classic.is_none() {
                    match &host.apps {
                        Apps::NotLoaded | Apps::Loading => {
                            ui.horizontal(|ui| {
                                spinner(ui, 9.0);
                                ui.label(RichText::new(tr("Loading the app list…")).color(ON_SURFACE_VARIANT));
                            });
                        }
                        Apps::Failed(_) | Apps::Loaded(_) => {
                            let why = match &host.apps {
                                Apps::Failed(e) => trf("The app list could not be loaded. {}", &[&e]),
                                _ => tr("The PC's app list has no entry for this game.").to_owned(),
                            };
                            ui.label(RichText::new(why).color(ERROR));
                            let r = quiet_button(ui, "Try again");
                            self.focus_item(&r, "retry", true);
                            if r.clicked() {
                                commands.push(Command::Refresh(host_id));
                            }
                        }
                    }
                }
                if let Some(app) = &classic {
                    self.leave_and_input_only(ui, host, app, can_play, resumes, commands);
                }
                ui.add_space(14.0);
                let kind = self.network.kind(&self.profiles.home_wifi);
                let mut settings = host.record.settings(&self.profiles.settings(&self.settings, kind));
                let game = classic.as_ref().and_then(|a| host.record.app_settings(&a.uuid, a.id));
                if let Some(overrides) = game {
                    settings = overrides.apply(&settings);
                }
                let (w, h, fps, kbps) = stream_mode(&settings, self.display);
                let profile = self
                    .profiles
                    .selected(kind)
                    .and_then(|id| self.profiles.entries.iter().find(|p| p.id == id))
                    .map_or(tr("Main settings"), |p| p.name.as_str());
                let mut summary = format!("{}  \u{b7}  {profile}", console::stream_summary(w, h, fps, kbps));
                for from in summary_sources(self.profiles.profile(kind), host.record.settings.as_ref(), game) {
                    summary.push_str("  \u{b7}  ");
                    summary.push_str(&from);
                }
                row(ui, "Stream settings", &summary, |ui| {
                    match &classic {
                        Some(app) => {
                            let r = quiet_button(ui, "Game settings");
                            self.focus_item(&r, "settings", false);
                            if r.clicked() {
                                commands.push(Command::AppSettings { host: host_id, app: app.clone() });
                            }
                            r
                        }
                        None => ui.add_enabled(false, egui::Button::new(tr("Game settings"))),
                    }
                });
                if !others.is_empty() {
                    section(ui, "Also on");
                    ui.horizontal_wrapped(|ui| {
                        for copy in &others {
                            // A copy there that is not installed opens, but says so first.
                            let name = if copy.installed { copy.host_name.clone() } else { trf("{}, not installed", &[&copy.host_name]) };
                            let r = quiet_button(ui, &name);
                            self.focus_item(&r, &format!("copy:{}", copy.host), false);
                            if r.clicked() {
                                self.go(Screen::Game { host: copy.host, app: copy.app_id });
                            }
                        }
                    });
                }
            });
        });
    }

    /// A game's quit on leave switch and, on a PC without its own mode, Input-only through a
    /// small stream: Android's game menu entries.
    pub(super) fn leave_and_input_only(&mut self, ui: &mut Ui, host: &Host, app: &nightjar_core::http::App, can_play: bool, running: bool, commands: &mut Vec<Command>) {
        use nightjar_policy::launch::{self, InputOnlyBlocker};
        if launch::offers_quit_on_leave(host.paired(), policy_control(app.control), policy_role(app.role(false))) {
            let mut on = host.record.quits_on_leave(&app.uuid, app.id);
            // A switch in a row like every other setting, its effect said under it.
            let note = if on { en("On: Disconnect also quits it on the PC") } else { en("Off: Disconnect leaves it running on the PC") };
            // As far below the buttons as the stream settings card sits below it.
            ui.add_space(space::S);
            let r = row(ui, "Quit the game when I leave the stream", note, |ui| toggle(ui, &mut on));
            self.focus_item(&r, "quit-on-leave", false);
            if r.changed() {
                commands.push(Command::SetQuitOnLeave { host: host.id, uuid: app.uuid.clone(), id: app.id, enabled: on });
            }
        }
        let Some(blocker) = small_stream_offer(host, app, running, self.settings.input_only_any_host) else { return };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ui.add_enabled_ui(blocker.is_none() && can_play, |ui| quiet_button(ui, "Input-only")).inner;
            self.focus_item(&r, "input-only", false);
            if r.clicked() {
                commands.push(Command::LaunchInputOnly { host: host.id, app: app.id, resume: running });
            }
            let note = match blocker {
                Some(InputOnlyBlocker::Pair) => tr("Pair with this PC first").to_owned(),
                Some(InputOnlyBlocker::ViewPermission) => tr("Allow viewing streams for this device on the host").to_owned(),
                Some(InputOnlyBlocker::InputPermission) => tr("Allow input for this device on the host").to_owned(),
                None => trf("Video still streams at the smallest size, about {} Mbit/s ({} MB an hour), but isn’t shown", &[
                    &format!("{:.1}", f64::from(launch::estimated_kbps()) / 1000.0),
                    &launch::estimated_megabytes_per_hour().to_string(),
                ]),
            };
            ui.label(RichText::new(note).color(if blocker.is_some() { ERROR } else { ON_SURFACE_VARIANT }));
        });
    }
}

/// Whether a paired PC that is up and shares its library has not sent it yet.
fn library_loading(view: &View<'_>) -> bool {
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
fn loading_games(ui: &mut Ui) {
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
pub(super) fn at_sentence_start(name: &str) -> String {
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
    let Some(running) = host.running_app() else {
        return tr("the running app").into();
    };
    if let Apps::Loaded(apps) = &host.apps {
        if let Some(app) = apps.iter().find(|a| a.matches_running_app(running, None)) {
            return app.name.clone();
        }
    }
    tr("the running app").into()
}

/// The classic app id of the app the PC runs now, once the app list is loaded.
pub(super) fn running_app_id(host: &Host) -> Option<i32> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None)).map(|a| a.id),
        _ => None,
    }
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
