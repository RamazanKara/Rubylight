//! Home: the hero with the running or last game, the Continue panel beside it, then the shelves of
//! favourites, recents, the library's rows and the PCs. All of it scrolls beneath the chrome.

use super::backdrop::Want;
use super::chrome::{capsule_width, control_capsule, icon_capsule, info_chip, play_button, play_width};
use super::design_tokens as t;
use super::glass;
use super::*;

const HERO_COVER: Vec2 = vec2(160.0, 240.0);
/// How wide the colour of the ambient glows around the hero's cover.
const HERO_GLOW_BLUR: f32 = 70.0;
const HERO_GAP: f32 = 28.0;
const TITLE_SIZE: f32 = 50.0;
const TITLE_LINE: f32 = 54.0;
const CHIP_H: f32 = 32.0;
const BUTTON_H: f32 = 52.0;
/// From the line that says what stops Play to the buttons.
const PROBLEM_GAP: f32 = 10.0;
const PANEL_W: f32 = 280.0;
const PANEL_HEAD: f32 = 38.0;
const PANEL_ROW: f32 = 73.0;
/// At most this many lines in the Continue panel; the rest of the recents go to their own shelf.
const PANEL_LINES: usize = 3;
/// Below this content width the panel moves under the hero.
const PANEL_BESIDE_FROM: f32 = 820.0;
/// Above a shelf's heading, and under the hero before the first one.
const SHELF_GAP: f32 = 10.0;
const HERO_FOOT: f32 = 12.0;

/// The game Home is about: running, else the last played, else the library's first.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Hero {
    pub host: u64,
    pub app: u32,
    pub title: String,
}

/// What Home shows, gathered once a frame: the chrome and the ambient need the hero before the page
/// is drawn.
pub(super) struct HomeData {
    /// The merged library: Home shows what of it is not hidden.
    pub library: std::rc::Rc<console::Merged>,
    pub playing: Vec<console::Continue>,
    pub recent: Vec<console::Recently>,
    pub starred: Vec<console::Starred>,
    pub hero: Option<Hero>,
}

impl HomeData {
    pub(super) fn gather(launcher: &Launcher, sources: &[Source<'_>]) -> Self {
        let library = launcher.library.get(sources, &launcher.hidden_games);
        let entries = library.shown();
        let playing = console::continue_playing(sources);
        // A running game stays under Continue: hiding must not lose the way back to it.
        let recent = console::recently_shown(
            sources,
            &launcher.recents,
            &playing,
            &launcher.hidden_games,
            6,
        );
        let starred: Vec<_> = console::favourite_games(sources, &launcher.favourites)
            .into_iter()
            .filter(|s| console::not_hidden(sources, s.host, s.app_id, &launcher.hidden_games))
            .collect();
        let hero = playing
            .first()
            .map(|p| Hero {
                host: p.host,
                app: p.app_id,
                title: p.title.clone(),
            })
            .or_else(|| {
                recent.first().map(|r| Hero {
                    host: r.host,
                    app: r.app_id,
                    title: r.title.clone(),
                })
            })
            .or_else(|| {
                entries
                    .iter()
                    .find(|e| e.installed())
                    .or(entries.first())
                    .map(|e| {
                        let best = e.best();
                        Hero {
                            host: best.host,
                            app: best.app_id,
                            title: e.title.clone(),
                        }
                    })
            });
        Self {
            library,
            playing,
            recent,
            starred,
            hero,
        }
    }
}

/// Where the hero's chips, title and buttons go.
struct HeroFoot {
    left: f32,
    right: f32,
    chips_y: f32,
    title_y: f32,
    buttons_y: f32,
}

/// One line of the Continue panel.
struct PanelLine {
    key: String,
    game: Option<String>,
    host: u64,
    app: u32,
    title: String,
    line: String,
    /// Its game has a cover to ask the PC for.
    has_cover: bool,
}

/// What a page about game `app` wants behind it.
pub(super) fn ambient_for(view: &View<'_>, host: u64, app: u32, title: &str) -> Want {
    let Some(pc) = view.hosts.get(host) else {
        return Want::Keep;
    };
    match pc.art.get(&(app as i32)) {
        // A cover comes with its ambient, baked off this thread and handed over together; until
        // then, and while it is on its way, what shows stays.
        // One whose cover was given up for room kept its ambient.
        Some(Art::Ready(_) | Art::Dropped) => match pc.ambient.get(&(app as i32)) {
            Some(texture) => Want::Art {
                key: format!("{host}:{app}"),
                texture: texture.clone(),
                glow: pc
                    .ambient_glow
                    .get(&(app as i32))
                    .copied()
                    .unwrap_or(Color32::TRANSPARENT),
            },
            None => Want::Keep,
        },
        Some(Art::Loading) => Want::Keep,
        Some(Art::Missing) => Want::Tile {
            title: title.to_owned(),
        },
        None if pc.reach == Reach::Online => Want::Keep,
        None => Want::Tile {
            title: title.to_owned(),
        },
    }
}

/// When a recent game was last played, for its line: the store and the day if known.
fn recent_line(
    entries: &[console::Entry],
    sources: &[Source<'_>],
    recents: &[crate::config::Recent],
    item: &console::Recently,
) -> String {
    if !item.online {
        return trf("{} is away", &[&item.host_name]);
    }
    let entry = entries.iter().find(|e| {
        e.copies
            .iter()
            .any(|c| c.host == item.host && c.app_id == item.app_id)
    });
    let played = entry.and_then(|e| console::last_played(e, sources, recents));
    match entry.zip(played) {
        Some((entry, at)) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            console::list_line(entry, Some(at), now)
        }
        None => trf("On {}", &[&item.host_name]),
    }
}

impl Launcher {
    pub(super) fn home(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        sources: &[Source<'_>],
        data: HomeData,
        commands: &mut Vec<Command>,
    ) {
        let HomeData {
            library,
            playing,
            recent,
            starred,
            hero,
        } = data;
        let entries = library.shown();
        let group = console::shown_group(self.settings.home_group, entries);
        let played = console::Played { sources, recents: &self.recents };
        let rows = console::home_rows(entries, group, console::Sort::shown(&self.settings, entries), played);
        let loading = library_loading(view);
        // The rest of the running games and recents: the panel first, a shelf for what it cannot hold.
        let has_cover = |host: u64, app: u32| {
            entries
                .iter()
                .find(|e| e.copies.iter().any(|c| c.host == host && c.app_id == app))
                .is_some_and(|e| e.cover().is_some())
        };
        let is_hero = |host: u64, app: u32| {
            hero.as_ref()
                .is_some_and(|h| h.host == host && h.app == app)
        };
        let mut lines: Vec<PanelLine> = Vec::new();
        for item in playing.iter().filter(|p| !is_hero(p.host, p.app_id)) {
            let line = match item.playing {
                Playing::Running => trf("Running on {}", &[&item.host_name]),
                Playing::Suspended => trf("Suspended on {}", &[&item.host_name]),
            };
            lines.push(PanelLine {
                key: format!("continue:{}:{}", item.host, item.app_id),
                game: None,
                host: item.host,
                app: item.app_id,
                title: item.title.clone(),
                line,
                has_cover: has_cover(item.host, item.app_id),
            });
        }
        for item in recent.iter().filter(|r| !is_hero(r.host, r.app_id)) {
            let line = recent_line(entries, sources, &self.recents, item);
            let key = format!("recent:{}:{}", item.host, item.app_id);
            let game = console::entry_key_of(entries, item.host, item.app_id).cloned();
            lines.push(PanelLine {
                key,
                game,
                host: item.host,
                app: item.app_id,
                title: item.title.clone(),
                line,
                has_cover: has_cover(item.host, item.app_id),
            });
        }
        let spill = lines.split_off(lines.len().min(PANEL_LINES));

        let to_top = self.chrome_to_top();
        let this = &mut *self;
        // The whole window scrolls, beneath the chrome; only the words and covers keep the margin.
        let scrolled = page_scroll(ui, "home", to_top, |ui| {
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
                    if this.setup_card(ui, view) {
                        commands.push(Command::SaveSettings);
                    }
                    let width = ui.available_width();
                    if let Some(hero) = &hero {
                        this.hero_block(ui, view, sources, hero, &lines, width, &mut first, commands);
                    }
                    let (columns, cover_w) = grid_up_to(width, POSTER_MAX_W * this.settings.home_card_size.scale());
                    let (_, card_w) = card_grid(width, CARD_GAP);
                    let mut actions_shown = false;
                    if !starred.is_empty() {
                        this.shelf_head(ui, tr("Favourites"), None, &mut actions_shown);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(POSTER_GAP, POSTER_GAP);
                            for (index, item) in starred.iter().enumerate() {
                                let art = shown_art(view, item.host, item.app_id);
                                let response = poster(ui, &item.title, art, Badge::None, cover_w);
                                if has_cover(item.host, item.app_id) {
                                    request_cover(ui, view, item.host, item.app_id, &response, commands);
                                }
                                let key = favourite_key(item);
                                let game = console::entry_key_of(entries, item.host, item.app_id).unwrap_or(&key);
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
                                let dots = cover_dots(ui, &response, shelf_image(&response), Badge::None);
                                if let Some(anchor) = menu_request(ui, &response, dots.as_ref()) {
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
                                let art = shown_art(view, item.host, item.app);
                                let response = poster(ui, &item.title, art, Badge::None, cover_w);
                                if item.has_cover {
                                    request_cover(ui, view, item.host, item.app, &response, commands);
                                }
                                match &item.game {
                                    Some(game) => this.focus_card(&response, &item.key, game, std::mem::take(&mut first)),
                                    None => this.focus_item(&response, &item.key, std::mem::take(&mut first)),
                                }
                                if response.clicked() {
                                    this.open(Screen::Game { host: item.host, app: item.app });
                                }
                                let dots = cover_dots(ui, &response, shelf_image(&response), Badge::None);
                                if let Some(anchor) = menu_request(ui, &response, dots.as_ref()) {
                                    this.open_menu(Dialog::GameMenu { host: item.host, game: GameTarget::Library { app: item.app, favourite: false } }, anchor, item.key.clone());
                                }
                            }
                        });
                    }
                    if entries.is_empty() {
                        this.shelf_head(ui, tr("Library"), None, &mut actions_shown);
                        if loading {
                            ui.add_space(space::S);
                            loading_covers(ui, tr("Loading the games…"), 1, true, this.cover_widest());
                        } else {
                            ui.label(RichText::new(tr("No games to show yet. They appear here once a PC that shares its game library is online.")).color(ON_SURFACE_VARIANT));
                            this.empty_library_way(ui, view, commands);
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
                            console::RowKind::Installed => tr("Installed"),
                            console::RowKind::NotInstalled => tr("Not installed"),
                        };
                        this.shelf_head(ui, name, Some(row.games.len()), &mut actions_shown);
                        // The List layout: every game of the row as a line, under each other.
                        if this.settings.home_layout == HomeLayout::List {
                            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                            for entry in &row.games {
                                let line = console::list_line(entry, console::last_played(entry, sources, &this.recents), now);
                                this.library_line(ui, view, &row.key(), entry, &line, width.min(LIST_MAX_W), std::mem::take(&mut first), commands);
                                ui.add_space(space::XS);
                            }
                            continue;
                        }
                        // The hero stands above its own shelf; the count and All still hold it.
                        let games: Vec<&console::Entry> = if whole {
                            row.games.clone()
                        } else {
                            row.games.iter().filter(|e| !e.copies.iter().any(|c| is_hero(c.host, c.app_id))).copied().collect()
                        };
                        let (shown, more) = if whole { (games.len(), false) } else { console::fit_row(games.len(), columns) };
                        // What a screen reader says for the tile: whose games it opens, and how many.
                        let said = match &row.kind {
                            console::RowKind::Store { name, .. } | console::RowKind::Collection { name } => trf("All {} games, {}", &[name, &row.games.len()]),
                            console::RowKind::Other => trf("All other apps, {}", &[&row.games.len()]),
                            console::RowKind::Games
                            | console::RowKind::Library
                            | console::RowKind::NoCollection
                            | console::RowKind::Installed
                            | console::RowKind::NotInstalled => trf("All games, {}", &[&row.games.len()]),
                        };
                        let games = &games[..shown];
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
                        }
                    }
                    this.shelf_head(ui, tr("PCs"), None, &mut actions_shown);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                        for (host, source) in view.hosts.list.iter().zip(sources) {
                            // A PC to pair has no games to speak of yet: its state alone fits the card.
                            let line = if host.paired() { console::host_line(source).text() } else { String::new() };
                            let response = host_card(ui, host, &line, card_w);
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
        });
        self.page_scrolled = scrolled;
    }

    /// A shelf's heading. The first one on the page also holds Home's own actions at its right.
    fn shelf_head(
        &mut self,
        ui: &mut Ui,
        name: &str,
        count: Option<usize>,
        actions_shown: &mut bool,
    ) {
        let with_actions = !std::mem::replace(actions_shown, true) && self.home_ready;
        ui.add_space(SHELF_GAP);
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.spacing_mut().item_spacing.x = space::S;
            ui.label(
                RichText::new(name)
                    .font(semibold(t::TYPE_SECTION_SIZE))
                    .color(ON_SURFACE),
            );
            if let Some(count) = count {
                ui.label(
                    RichText::new(count.to_string())
                        .font(regular(t::TYPE_BODY_SIZE))
                        .color(ON_SURFACE_VARIANT),
                );
            }
            if with_actions {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // The links sit at the right, where the moon may be once the page scrolls.
                    let scrim = ui.painter().add(egui::Shape::Noop);
                    let view = text_button(ui, "View");
                    self.focus_item(&view, "home-view", false);
                    if view.clicked() {
                        self.open_menu(
                            Dialog::HomeView,
                            Anchor::Below(view.rect),
                            "home-view".into(),
                        );
                    }
                    let all = text_button(ui, "All games");
                    super::pcs::scrim_on_moon(ui, scrim, view.rect.union(all.rect).expand(space::XS));
                    if all.clicked() {
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
    fn hero_block(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        sources: &[Source],
        hero: &Hero,
        lines: &[PanelLine],
        width: f32,
        first: &mut bool,
        commands: &mut Vec<Command>,
    ) {
        let beside = !lines.is_empty() && width >= PANEL_BESIDE_FROM;
        let panel_h = if lines.is_empty() {
            0.0
        } else {
            PANEL_HEAD + PANEL_ROW * lines.len() as f32
        };
        let height = (HERO_COVER.y + HERO_FOOT).max(if beside {
            panel_h - HERO_FOOT / 2.0
        } else {
            0.0
        });
        let (block, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let ctx = ui.ctx().clone();
        let cover_rect = Rect::from_min_size(block.left_top(), HERO_COVER);
        let cover = ui.interact(cover_rect, Id::unique("hero-cover"), Sense::click());
        cover.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, hero.title.clone())
        });
        let art = shown_art(view, hero.host, hero.app);
        let feel = glass::feel(&ctx, &cover, t::FOCUS_SCALE_CONTROL, t::HOVER_SCALE_COVER);
        let shown = glass::scaled_rect(cover_rect, feel.scale);
        let mut out = Vec::new();
        glass::cover_glow(
            &mut out,
            &ctx,
            shown,
            t::RADIUS_COVER,
            self.ambient.glow(),
            HERO_GLOW_BLUR,
            18.0,
            44.0,
        );
        let boost = if feel.hovered || feel.focused {
            t::HOVER_RIM_BOOST
        } else {
            1.0
        };
        cover_shapes(
            ui,
            &mut out,
            shown,
            HERO_COVER.x,
            t::RADIUS_COVER,
            &hero.title,
            art,
            boost,
        );
        if feel.focused {
            glass::focus(&mut out, &ctx, shown, t::RADIUS_COVER, false);
        }
        ui.painter().extend(out);
        self.focus_item(&cover, "hero-cover", false);
        if cover.clicked() {
            self.open(Screen::Game {
                host: hero.host,
                app: hero.app,
            });
        }
        if let Some(anchor) = menu_request(ui, &cover, None) {
            self.open_menu(
                Dialog::GameMenu {
                    host: hero.host,
                    game: GameTarget::Library {
                        app: hero.app,
                        favourite: false,
                    },
                },
                anchor,
                "hero-cover".into(),
            );
        }
        let source = sources.iter().find(|s| s.id == hero.host);
        let game = source
            .and_then(|s| s.library)
            .and_then(|l| l.games.iter().find(|g| g.app_id == hero.app));
        if game.is_some() {
            request_cover(ui, view, hero.host, hero.app, &cover, commands);
        }
        // The words and buttons sit on the cover's foot, between it and the panel.
        let right = if beside {
            block.right() - PANEL_W - HERO_GAP
        } else {
            block.right()
        };
        let left = cover_rect.right() + HERO_GAP;
        let foot = cover_rect.bottom();
        let buttons_y = foot - BUTTON_H;
        let title_y = buttons_y - 18.0 - TITLE_LINE;
        let chips_y = title_y - 10.0 - CHIP_H;
        match (view.hosts.get(hero.host), source, game) {
            (Some(host), Some(source), Some(game)) => self.hero_actions(
                ui,
                host,
                source,
                game,
                HeroFoot {
                    left,
                    right,
                    chips_y,
                    title_y,
                    buttons_y,
                },
                first,
                commands,
            ),
            // A game the library no longer lists: its title, and its page says the rest.
            _ => hero_title(ui, left, title_y, right - left, &hero.title),
        }
        if !lines.is_empty() {
            let panel_rect = if beside {
                Rect::from_min_size(
                    pos2(block.right() - PANEL_W, block.top()),
                    vec2(PANEL_W, panel_h),
                )
            } else {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(width, panel_h + space::M), Sense::hover());
                Rect::from_min_size(rect.left_top() + vec2(0.0, space::M), vec2(width, panel_h))
            };
            self.continue_panel(ui, view, panel_rect, lines, first, commands);
        }
    }
}

/// The picture of a shelf's cover, without the caption under it.
fn shelf_image(cover: &Response) -> Rect {
    Rect::from_min_size(cover.rect.min, vec2(cover.rect.width(), cover_height(cover.rect.width())))
}

mod actions;
#[cfg(test)]
pub(super) use actions::setup_line;
use actions::hero_title;
