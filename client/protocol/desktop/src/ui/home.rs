//! Home: the hero with the running or last game, the Continue panel beside it, then the shelves of
//! favourites, recents, the library's rows and the PCs. All of it scrolls beneath the chrome.

use super::backdrop::Want;
use super::chrome::{capsule_width, control_capsule, info_chip, play_button, play_width, PAGE_TOP};
use super::design_tokens as t;
use super::glass;
use super::*;

const HERO_COVER: Vec2 = vec2(160.0, 240.0);
const HERO_GAP: f32 = 28.0;
const TITLE_SIZE: f32 = 50.0;
const TITLE_LINE: f32 = 54.0;
const CHIP_H: f32 = 32.0;
const BUTTON_H: f32 = 52.0;
const PANEL_W: f32 = 280.0;
const PANEL_HEAD: f32 = 38.0;
const PANEL_ROW: f32 = 73.0;
/// At most this many lines in the Continue panel; the rest of the recents go to their own shelf.
const PANEL_LINES: usize = 3;
/// Below this content width the panel moves under the hero.
const PANEL_BESIDE_FROM: f32 = 820.0;

/// The game Home is about: running, else the last played, else the library's first.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Hero {
    pub host: u64,
    pub app: u32,
    pub title: String,
}

/// One line of the Continue panel.
struct PanelLine {
    key: String,
    game: Option<String>,
    host: u64,
    app: u32,
    title: String,
    line: String,
    cover_version: Option<String>,
}

fn art_of<'a>(view: &'a View<'_>, host: u64, app: u32) -> Option<&'a Art> {
    view.hosts.get(host).and_then(|h| h.art.get(&(app as i32)))
}

/// What Home wants behind it for `hero`.
pub(super) fn ambient_for(view: &View<'_>, hero: Option<&Hero>) -> Want {
    let Some(hero) = hero else { return Want::Sky };
    match art_of(view, hero.host, hero.app) {
        Some(Art::Ready(_)) => match view.hosts.get(hero.host).and_then(|h| h.ambient.get(&(hero.app as i32))) {
            Some(texture) => Want::Art { key: format!("{}:{}", hero.host, hero.app), texture: texture.clone() },
            None => Want::Tile { title: hero.title.clone() },
        },
        Some(Art::Loading) => Want::Keep,
        Some(Art::Missing) | None => Want::Tile { title: hero.title.clone() },
    }
}

impl Launcher {
    pub(super) fn home(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        if view.hosts.list.is_empty() {
            self.hero = None;
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
        // A running game stays under Continue: hiding must not lose the way back to it.
        let recent = console::recently_shown(&sources, &self.recents, &playing, &self.hidden_games, 6);
        let starred: Vec<_> = console::favourite_games(&sources, &self.favourites).into_iter().filter(|s| console::not_hidden(&sources, s.host, s.app_id, &self.hidden_games)).collect();
        let loading = library_loading(view);
        let hero = playing
            .first()
            .map(|p| Hero { host: p.host, app: p.app_id, title: p.title.clone() })
            .or_else(|| recent.first().map(|r| Hero { host: r.host, app: r.app_id, title: r.title.clone() }))
            .or_else(|| {
                entries.iter().find(|e| e.installed()).or(entries.first()).map(|e| {
                    let best = e.best();
                    Hero { host: best.host, app: best.app_id, title: e.title.clone() }
                })
            });
        self.hero = hero.clone();
        // The rest of the running games and recents: the panel first, a shelf for what it cannot hold.
        let cover_version = |host: u64, app: u32| {
            entries.iter().find(|e| e.copies.iter().any(|c| c.host == host && c.app_id == app)).and_then(|e| e.cover()).map(|c| c.cover_version.clone())
        };
        let is_hero = |host: u64, app: u32| hero.as_ref().is_some_and(|h| h.host == host && h.app == app);
        let mut lines: Vec<PanelLine> = Vec::new();
        for item in playing.iter().filter(|p| !is_hero(p.host, p.app_id)) {
            let line = match item.playing {
                Playing::Running => trf("Running on {}", &[&item.host_name]),
                Playing::Suspended => trf("Suspended on {}", &[&item.host_name]),
            };
            lines.push(PanelLine { key: format!("continue:{}:{}", item.host, item.app_id), game: None, host: item.host, app: item.app_id, title: item.title.clone(), line, cover_version: cover_version(item.host, item.app_id) });
        }
        for item in recent.iter().filter(|r| !is_hero(r.host, r.app_id)) {
            let line = if item.online { trf("On {}", &[&item.host_name]) } else { trf("{} is away", &[&item.host_name]) };
            let key = format!("recent:{}:{}", item.host, item.app_id);
            let game = console::entry_key_of(&entries, item.host, item.app_id).map(str::to_owned);
            lines.push(PanelLine { key, game, host: item.host, app: item.app_id, title: item.title.clone(), line, cover_version: cover_version(item.host, item.app_id) });
        }
        let spill = lines.split_off(lines.len().min(PANEL_LINES));

        let full = ui.ctx().content_rect();
        let margin = chrome::page_margin(full.width());
        let this = &mut *self;
        ui.scope_builder(UiBuilder::new().max_rect(full), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("home")
                .auto_shrink(false)
                .content_margin(Margin { left: margin as i8, right: margin as i8, top: PAGE_TOP as i8, bottom: space::XL as i8 })
                .show(ui, |ui| {
                    crate::gui::follow_focus_under(ui, "home", PAGE_TOP);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut first = true;
                    // A failed read stays on screen beside the last good data.
                    for host in &view.hosts.list {
                        if let Some(error) = &host.native.error {
                            ui.label(RichText::new(format!("{}: {}", host.display_name(), tr(error))).color(ERROR));
                            ui.add_space(space::XS);
                        }
                    }
                    this.pairing_banner(ui, view);
                    let width = ui.available_width();
                    if let Some(hero) = &hero {
                        this.hero_block(ui, view, &sources, hero, &lines, width, &mut first, commands);
                    }
                    let (columns, cover_w) = grid_up_to(width, POSTER_MAX_W * this.settings.home_card_size.scale());
                    let (_, card_w) = card_grid(width, CARD_GAP);
                    let mut actions_shown = false;
                    if !starred.is_empty() {
                        this.shelf_head(ui, tr("Favourites"), None, &mut actions_shown);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(POSTER_GAP, POSTER_GAP);
                            for (index, item) in starred.iter().enumerate() {
                                let art = art_of(view, item.host, item.app_id);
                                let response = poster(ui, &item.title, art, Badge::None, cover_w);
                                if let Some(version) = cover_version(item.host, item.app_id) {
                                    request_cover(ui, view, item.host, item.app_id, &version, &response, commands);
                                }
                                let key = favourite_key(item);
                                let game = console::entry_key_of(&entries, item.host, item.app_id).unwrap_or(&key);
                                this.focus_card(&response, &key, game, std::mem::take(&mut first));
                                if response.clicked() {
                                    this.open(Screen::Game { host: item.host, app: item.app_id });
                                }
                                if response.has_focus() && unstar_pressed(ui) {
                                    commands.push(Command::ToggleFavourite(item.favourite.clone()));
                                    // The focus moves to the next favourite (else the previous), not back to the top.
                                    match favourite_neighbour(&starred, index) {
                                        Some(near) => this.restore_key = Some(near),
                                        None => this.focus_screen = true,
                                    }
                                }
                                if let Some(anchor) = menu_request(ui, &response, None) {
                                    this.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app_id, favourite: true } }, anchor, key);
                                }
                            }
                        });
                    }
                    if !spill.is_empty() {
                        this.shelf_head(ui, tr("Recently played"), None, &mut actions_shown);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(POSTER_GAP, POSTER_GAP);
                            for item in &spill {
                                let art = art_of(view, item.host, item.app);
                                let response = poster(ui, &item.title, art, Badge::None, cover_w);
                                if let Some(version) = &item.cover_version {
                                    request_cover(ui, view, item.host, item.app, version, &response, commands);
                                }
                                match &item.game {
                                    Some(game) => this.focus_card(&response, &item.key, game, std::mem::take(&mut first)),
                                    None => this.focus_item(&response, &item.key, std::mem::take(&mut first)),
                                }
                                if response.clicked() {
                                    this.open(Screen::Game { host: item.host, app: item.app });
                                }
                                if let Some(anchor) = menu_request(ui, &response, None) {
                                    this.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app, favourite: false } }, anchor, item.key.clone());
                                }
                            }
                        });
                    }
                    if entries.is_empty() {
                        this.shelf_head(ui, tr("Library"), None, &mut actions_shown);
                        if loading {
                            loading_games(ui);
                        } else {
                            ui.label(RichText::new(tr("No games to show yet. They appear here once a PC that shares its game library is online.")).color(ON_SURFACE_VARIANT));
                        }
                    }
                    // One row per store (or one for the library), as many covers as fit and an All tile for
                    // the rest. The Grid view shows each row whole, as does a library of one kind by store.
                    let whole = this.settings.home_layout == HomeLayout::Grid || (rows.len() == 1 && group == HomeGroup::Store);
                    for row in &rows {
                        let name = match &row.kind {
                            console::RowKind::Store { name, .. } => name.as_str(),
                            console::RowKind::Games => tr("Games"),
                            console::RowKind::Other => tr("Other apps"),
                            console::RowKind::Library => tr("Library"),
                            console::RowKind::Collection { name } => name.as_str(),
                            console::RowKind::NoCollection => tr("No collection"),
                        };
                        this.shelf_head(ui, name, Some(row.games.len()), &mut actions_shown);
                        // The List layout: every game of the row as a line, under each other.
                        if this.settings.home_layout == HomeLayout::List {
                            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                            for entry in &row.games {
                                let line = console::list_line(entry, console::last_played(entry, &sources, &this.recents), now);
                                this.library_line(ui, view, &row.key(), entry, &line, width.min(LIST_MAX_W), std::mem::take(&mut first), commands);
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
                                    this.library_cover(ui, view, Some(&row.key()), entry, cover_w, std::mem::take(&mut first), commands);
                                }
                                if more && line.len() < columns {
                                    let tile = all_tile(ui, row.games.len(), &said, cover_w);
                                    this.focus_item(&tile, &format!("all:{}", row.key()), std::mem::take(&mut first));
                                    if tile.clicked() {
                                        this.library_filter = row.filter.clone();
                                        this.open(Screen::AllGames);
                                    }
                                }
                            });
                            ui.add_space(space::XS);
                        }
                    }
                    this.shelf_head(ui, tr("PCs"), None, &mut actions_shown);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                        for (host, source) in view.hosts.list.iter().zip(&sources) {
                            // A PC to pair has no games to speak of yet: its state alone fits the card.
                            let line = if host.paired() { console::host_line(source).text() } else { String::new() };
                            let response = host_card(ui, host, Some(&line), card_w);
                            let key = format!("host:{}", host.id);
                            this.focus_item(&response, &key, std::mem::take(&mut first));
                            if response.clicked() {
                                host_clicked(host, this.settings.quick_resume, commands);
                            }
                            if let Some(anchor) = menu_request(ui, &response, None) {
                                this.open_menu(Dialog::HostOptions { host: host.id }, anchor, key);
                            }
                        }
                    });
                    ui.add_space(space::L);
                    glass::flush_deferred(ui);
                });
        });
    }

    /// A shelf's heading. The first one on the page also holds Home's own actions at its right.
    fn shelf_head(&mut self, ui: &mut Ui, name: &str, count: Option<usize>, actions_shown: &mut bool) {
        let with_actions = !std::mem::replace(actions_shown, true) && self.home_ready;
        ui.add_space(space::M);
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.spacing_mut().item_spacing.x = space::S;
            ui.label(RichText::new(name).font(semibold(t::TYPE_SECTION_SIZE)).color(ON_SURFACE));
            if let Some(count) = count {
                ui.label(RichText::new(count.to_string()).font(regular(t::TYPE_BODY_SIZE)).color(ON_SURFACE_VARIANT));
            }
            if with_actions {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let view = text_button(ui, "View");
                    self.focus_item(&view, "home-view", false);
                    if view.clicked() {
                        self.open_menu(Dialog::HomeView, Anchor::Below(view.rect), "home-view".into());
                    }
                    if text_button(ui, "All games").clicked() {
                        self.library_filter = console::LibraryFilter::All;
                        self.open(Screen::AllGames);
                    }
                });
            }
        });
        ui.add_space(space::M);
    }

    /// The hero: the cover, the state and store chips, the huge title and the actions; the Continue
    /// panel beside it, or under it in a narrow window.
    #[allow(clippy::too_many_arguments)]
    fn hero_block(&mut self, ui: &mut Ui, view: &View<'_>, sources: &[Source], hero: &Hero, lines: &[PanelLine], width: f32, first: &mut bool, commands: &mut Vec<Command>) {
        let beside = !lines.is_empty() && width >= PANEL_BESIDE_FROM;
        let panel_h = if lines.is_empty() { 0.0 } else { PANEL_HEAD + PANEL_ROW * lines.len() as f32 + 2.0 };
        let height = if beside { HERO_COVER.y.max(panel_h) } else { HERO_COVER.y };
        let (block, cover_hover) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let _ = cover_hover;
        let ctx = ui.ctx().clone();
        let cover_rect = Rect::from_min_size(block.left_top(), HERO_COVER);
        let cover = ui.interact(cover_rect, Id::new("hero-cover"), Sense::click());
        cover.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, hero.title.clone()));
        let art = art_of(view, hero.host, hero.app);
        let feel = glass::feel(&ctx, &cover, t::FOCUS_SCALE_CONTROL, t::HOVER_SCALE_COVER);
        let shown = glass::scaled_rect(cover_rect, feel.scale);
        let mut out = Vec::new();
        cover_shapes(ui, &mut out, shown, t::RADIUS_COVER, &hero.title, art, 1.0 + 0.4);
        if feel.focused {
            glass::focus(&mut out, &ctx, shown, t::RADIUS_COVER, false);
        }
        ui.painter().extend(out);
        self.focus_item(&cover, "hero-cover", false);
        if cover.clicked() {
            self.open(Screen::Game { host: hero.host, app: hero.app });
        }
        if let Some(anchor) = menu_request(ui, &cover, None) {
            self.open_menu(Dialog::GameMenu { host: hero.host, game: GameTarget::Library { app: hero.app, favourite: false } }, anchor, "hero-cover".into());
        }
        let Some(host) = view.hosts.get(hero.host) else { return };
        let Some(source) = sources.iter().find(|s| s.id == hero.host) else { return };
        let game = source.library.and_then(|l| l.games.iter().find(|g| g.app_id == hero.app));
        if let Some(c) = game {
            request_cover(ui, view, hero.host, hero.app, &c.artwork.version, &cover, commands);
        }
        // The words and buttons sit on the cover's foot, between it and the panel.
        let right = if beside { block.right() - PANEL_W - HERO_GAP } else { block.right() };
        let left = cover_rect.right() + HERO_GAP;
        let foot = cover_rect.bottom() - 2.0;
        let buttons_y = foot - BUTTON_H;
        let title_y = buttons_y - 18.0 - TITLE_LINE;
        let chips_y = title_y - 10.0 - CHIP_H;
        let Some(game) = game else {
            // A game of the classic list alone: its title, and its page holds the rest.
            hero_title(ui, left, title_y, right - left, &hero.title);
            return;
        };
        let state = self.play_state(host, source, game);
        let mut x = left;
        match state.detail.state {
            GameState::Running => x += info_chip(ui, x, chips_y + CHIP_H / 2.0, Some(*OK), tr("Running")) + 6.0,
            GameState::Suspended => x += info_chip(ui, x, chips_y + CHIP_H / 2.0, Some(*TERTIARY), tr("Suspended")) + 6.0,
            GameState::Idle => {}
        }
        if let Some(store) = game.store.as_ref().map(|s| console::store_name(&s.source, &s.name)).filter(|n| !n.is_empty()) {
            info_chip(ui, x, chips_y + CHIP_H / 2.0, None, &store);
        }
        hero_title(ui, left, title_y, right - left, &game.title);

        let label = if state.resumes { tr("Resume") } else { tr("Play") };
        let width_of = play_width(ui, label);
        let play_rect = Rect::from_min_size(pos2(left, buttons_y), vec2(width_of, BUTTON_H));
        let play = play_button(ui, play_rect, Id::new("hero-play"), Icon::Play, label, state.can_play);
        self.focus_item(&play, "hero-play", std::mem::take(first) && state.can_play);
        if play.clicked() {
            self.press_play(host, game, &state, commands);
        }
        let mut x = play_rect.right() + 10.0;
        let star = console::favourite_of(source, game);
        let starred = console::is_starred(source, game, &self.favourites);
        let star_w = capsule_width(ui, tr("Favourite"));
        let said = if starred { tr("Remove from favourites") } else { tr("Add to favourites") };
        let star_rect = Rect::from_min_size(pos2(x, buttons_y), vec2(star_w, BUTTON_H));
        let glyph = if starred { Icon::Star } else { Icon::StarOff };
        let r = control_capsule(ui, star_rect, Id::new("hero-star"), glyph, tr("Favourite"), said, star.is_some());
        self.focus_item(&r, "hero-star", !state.can_play && state.detail.end == EndAction::None);
        if r.clicked() {
            if let Some(star) = star {
                commands.push(Command::ToggleFavourite(star));
            }
        }
        x = star_rect.right() + 10.0;
        if state.detail.end != EndAction::None {
            let closing = host.native.acting;
            let label = if state.detail.end == EndAction::CloseGame { tr("Close game") } else { tr("Quit game") };
            let end_rect = Rect::from_min_size(pos2(x, buttons_y), vec2(capsule_width(ui, label), BUTTON_H));
            let r = control_capsule(ui, end_rect, Id::new("hero-end"), Icon::Power, label, label, !closing);
            self.focus_item(&r, "hero-end", !state.can_play);
            if r.clicked() {
                self.press_end(host, game, &state);
            }
        }
        if !lines.is_empty() {
            let panel_rect = if beside {
                Rect::from_min_size(pos2(block.right() - PANEL_W, block.top()), vec2(PANEL_W, panel_h))
            } else {
                let (rect, _) = ui.allocate_exact_size(vec2(width, panel_h + space::M), Sense::hover());
                Rect::from_min_size(rect.left_top() + vec2(0.0, space::M), vec2(width, panel_h))
            };
            self.continue_panel(ui, view, panel_rect, lines, first, commands);
        }
    }

    fn continue_panel(&mut self, ui: &mut Ui, view: &View<'_>, rect: Rect, lines: &[PanelLine], first: &mut bool, commands: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let mut out = Vec::new();
        glass::group(&mut out, &ctx, rect, t::RADIUS_GROUP, t::TINT_GROUP_ON_ART_FILL);
        ui.painter().extend(out);
        let head = ui.painter().layout_no_wrap(tr("Continue").to_owned(), semibold(t::TYPE_HEADLINE_SIZE), *ON_SURFACE);
        ui.painter().galley(rect.left_top() + vec2(16.0, 12.0), head, *ON_SURFACE);
        let mut y = rect.top() + PANEL_HEAD;
        for (n, item) in lines.iter().enumerate() {
            let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), PANEL_ROW));
            let response = ui.interact(row, Id::new(("continue-line", &item.key)), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{}, {}", item.title, item.line)));
            let feel = glass::feel(&ctx, &response, 1.0, 1.0);
            if feel.hovered || feel.pressed {
                let lift = if feel.pressed { t::PRESS_FILL_LIFT } else { t::HOVER_FILL_LIFT };
                let mut out = Vec::new();
                glass::solid(&mut out, &ctx, row.shrink2(vec2(4.0, 1.0)), 14.0, lift);
                ui.painter().extend(out);
            }
            if n > 0 {
                ui.painter().hline(rect.left() + 16.0..=rect.right(), y, Stroke::new(1.0, t::TINT_SEPARATOR));
            }
            let cover = Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 8.0), vec2(38.0, 57.0));
            let art = art_of(view, item.host, item.app);
            let mut out = Vec::new();
            cover_shapes(ui, &mut out, cover, 8.0, &item.title, art, 1.0);
            ui.painter().extend(out);
            if let Some(version) = &item.cover_version {
                request_cover(ui, view, item.host, item.app, version, &response, commands);
            }
            let words = row.width() - 16.0 - 38.0 - 12.0 - 12.0;
            let at = pos2(cover.right() + 12.0, row.center().y);
            let mut job = egui::text::LayoutJob::simple_singleline(item.title.clone(), semibold(t::TYPE_BODY_SIZE), *ON_SURFACE);
            job.wrap = egui::text::TextWrapping::truncate_at_width(words);
            let title = ui.painter().layout_job(job);
            ui.painter().galley(at - vec2(0.0, title.size().y), title, *ON_SURFACE);
            let mut job = egui::text::LayoutJob::simple_singleline(item.line.clone(), regular(t::TYPE_SUB_SIZE), *ON_SURFACE_VARIANT);
            job.wrap = egui::text::TextWrapping::truncate_at_width(words);
            let line = ui.painter().layout_job(job);
            ui.painter().galley(at + vec2(0.0, 2.0), line, *ON_SURFACE_VARIANT);
            focus_ring(ui, row.shrink2(vec2(4.0, 4.0)), &response, 14);
            match &item.game {
                Some(game) => self.focus_card(&response, &item.key, game, std::mem::take(first)),
                None => self.focus_item(&response, &item.key, std::mem::take(first)),
            }
            if response.clicked() {
                self.open(Screen::Game { host: item.host, app: item.app });
            }
            if let Some(anchor) = menu_request(ui, &response, None) {
                self.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app, favourite: false } }, anchor, item.key.clone());
            }
            y += PANEL_ROW;
        }
    }
}

/// The huge title on one line, cut with "…" when it does not fit.
fn hero_title(ui: &Ui, left: f32, top: f32, width: f32, title: &str) {
    let format = egui::TextFormat { font_id: semibold(TITLE_SIZE), color: *ON_SURFACE, extra_letter_spacing: t::TYPE_HERO_TRACK, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(title.to_owned(), format);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(40.0));
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos2(left, top + (TITLE_LINE - galley.size().y) / 2.0), galley, *ON_SURFACE);
}
