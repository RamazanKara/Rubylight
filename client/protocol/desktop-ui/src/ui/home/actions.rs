//! Home hero action controls, Continue playing panel, and first-pair setup card.

use super::*;

impl Launcher {
    /// The hero's chips, title and buttons.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn hero_actions(
        &mut self,
        ui: &mut Ui,
        host: &Host,
        source: &Source<'_>,
        game: &rubylight_native_api::library::Game,
        at: HeroFoot,
        first: &mut bool,
        commands: &mut Vec<Command>,
    ) {
        let HeroFoot {
            left,
            right,
            chips_y,
            title_y,
            buttons_y,
        } = at;
        let state = self.play_state(host, source, game);
        // What stops Play stands above the buttons, as on the game's page; the words above it rise.
        let problem = state.list_problem(host);
        let rise = problem.as_ref().map_or(0.0, |why| {
            let line = ui.painter().layout(why.clone(), regular(t::TYPE_SUB_SIZE), *ERROR, right - left);
            let (h, ink) = (line.size().y, *ERROR);
            ui.painter().galley(pos2(left, buttons_y - PROBLEM_GAP - h), line, ink);
            h + PROBLEM_GAP
        });
        let (chips_y, title_y) = (chips_y - rise, title_y - rise);
        // Play starts from the PC's app list: read it while the player is here, not on the press.
        if state.detail.can_start && matches!(host.apps, Apps::NotLoaded) {
            commands.push(Command::LoadApps(host.id));
        }
        let mut x = left;
        match state.detail.state {
            GameState::Running => {
                x += info_chip(ui, x, chips_y + CHIP_H / 2.0, Some(*OK), tr("Running")) + 6.0
            }
            GameState::Suspended => {
                x += info_chip(
                    ui,
                    x,
                    chips_y + CHIP_H / 2.0,
                    Some(*TERTIARY),
                    tr("Suspended"),
                ) + 6.0
            }
            GameState::Idle => {}
        }
        if let Some(store) = game
            .store
            .as_ref()
            .map(|s| console::store_name(&s.source, &s.name))
            .filter(|n| !n.is_empty())
        {
            info_chip(ui, x, chips_y + CHIP_H / 2.0, None, &store);
        }
        hero_title(ui, left, title_y, right - left, &game.title);

        let label = if state.resumes {
            tr("Resume")
        } else {
            tr("Play")
        };
        let play_rect =
            Rect::from_min_size(pos2(left, buttons_y), vec2(play_width(ui, label), BUTTON_H));
        // A game that can start takes the focus even while its app list is on the way, so the first
        // press is never spent on the star. A list that came without it is no way to start it.
        let could_play = state.could_play(host);
        let play = play_button(
            ui,
            play_rect,
            Id::unique("hero-play"),
            Icon::Play,
            label,
            could_play,
            self.playing_soon(host.id, game.app_id),
        );
        let play_first = *first && could_play;
        if play_first {
            *first = false;
        }
        self.focus_item(&play, "hero-play", play_first);
        if play.clicked() {
            self.press_play(host, game, &state, commands);
        }
        let target = GameTarget::Library {
            app: game.app_id,
            favourite: false,
        };
        let go = if state.resumes { en("Resume") } else { en("Play") };
        if let Some(anchor) = menu_request_going(ui, &play, None, go) {
            self.open_menu(
                Dialog::GameMenu {
                    host: host.id,
                    game: target,
                },
                anchor,
                "hero-play".into(),
            );
        }
        let mut next_x = play_rect.right() + 10.0;
        if problem.is_some() {
            let rect = Rect::from_min_size(
                pos2(next_x, buttons_y),
                vec2(capsule_width(ui, tr("Try again")), BUTTON_H),
            );
            self.list_retry(ui, rect, "hero-retry", std::mem::take(first), host.id, commands);
            next_x = rect.right() + 10.0;
        }
        let star = console::favourite_of(source, game);
        let starred = console::is_starred(source, game, &self.favourites);
        // The star alone, as on Android's Home: the row keeps its room in the smallest window. Its
        // words, the game menu's, are its tooltip, its hint and what a screen reader says.
        let (said, going) = if starred {
            (tr("Remove from favourites"), en("Remove from favourites"))
        } else {
            (tr("Add to favourites"), en("Add to favourites"))
        };
        let star_rect = Rect::from_min_size(pos2(next_x, buttons_y), Vec2::splat(BUTTON_H));
        let glyph = if starred { Icon::StarOn } else { Icon::Star };
        let r = icon_capsule(
            ui,
            star_rect,
            Id::unique("hero-star"),
            glyph,
            said,
            star.is_some(),
        );
        let star_first = *first && state.detail.end == EndAction::None && star.is_some();
        if star_first {
            *first = false;
        }
        self.focus_item(&r, "hero-star", star_first);
        if let Some(anchor) = menu_request_going(ui, &r, None, going) {
            self.open_menu(
                Dialog::GameMenu {
                    host: host.id,
                    game: target,
                },
                anchor,
                "hero-star".into(),
            );
        }
        if r.clicked()
            && let Some(star) = star {
                commands.push(Command::ToggleFavourite(star));
            }
        if state.detail.end != EndAction::None {
            let closing = host.native.acting;
            let label = tr("Quit game");
            let end_rect = Rect::from_min_size(
                pos2(star_rect.right() + 10.0, buttons_y),
                vec2(capsule_width(ui, label), BUTTON_H),
            );
            let r = control_capsule(
                ui,
                end_rect,
                Id::unique("hero-end"),
                Icon::Power,
                label,
                label,
                !closing,
            );
            let end_first = !closing && std::mem::take(first);
            self.focus_item(&r, "hero-end", end_first);
            if let Some(anchor) = menu_request_going(ui, &r, None, en("Quit game")) {
                self.open_menu(
                    Dialog::GameMenu {
                        host: host.id,
                        game: target,
                    },
                    anchor,
                    "hero-end".into(),
                );
            }
            if r.clicked() {
                self.press_end(host, game, &state);
            }
        }
    }

    pub(super) fn continue_panel(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        rect: Rect,
        lines: &[PanelLine],
        first: &mut bool,
        commands: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let mut out = Vec::new();
        glass::group(
            &mut out,
            &ctx,
            rect,
            t::RADIUS_GROUP,
            shade::TINT_GROUP_ON_ART_FILL.get(),
        );
        ui.painter().extend(out);
        let head = ui.painter().layout_no_wrap(
            tr("Continue").to_owned(),
            semibold(t::TYPE_HEADLINE_SIZE),
            *ON_SURFACE,
        );
        ui.painter()
            .galley(rect.left_top() + vec2(16.0, 12.0), head, *ON_SURFACE);
        let mut y = rect.top() + PANEL_HEAD;
        for (n, item) in lines.iter().enumerate() {
            let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), PANEL_ROW));
            let response = ui.interact(
                row,
                Id::unique(("continue-line", &item.key)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    true,
                    format!("{}, {}", item.title, item.line),
                )
            });
            let feel = glass::feel(&ctx, &response, 1.0, 1.0);
            if feel.hovered || feel.pressed {
                let lift = if feel.pressed {
                    shade::PRESS_FILL_LIFT.get()
                } else {
                    shade::HOVER_FILL_LIFT.get()
                };
                let mut out = Vec::new();
                glass::solid(&mut out, &ctx, row.shrink2(vec2(4.0, 1.0)), 14.0, lift);
                ui.painter().extend(out);
            }
            if n > 0 {
                ui.painter().hline(
                    rect.left() + 16.0..=rect.right(),
                    y,
                    Stroke::new(1.0, shade::TINT_SEPARATOR.get()),
                );
            }
            let cover =
                Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 8.0), vec2(38.0, 57.0));
            let art = shown_art(view, item.host, item.app);
            let mut out = Vec::new();
            cover_shapes(
                ui,
                &mut out,
                cover,
                cover.width(),
                8.0,
                &item.title,
                art,
                1.0,
            );
            ui.painter().extend(out);
            if item.has_cover {
                request_cover(ui, view, item.host, item.app, &response, commands);
            }
            let words = row.width() - 16.0 - 38.0 - 12.0 - 12.0;
            let at = pos2(cover.right() + 12.0, row.center().y);
            let mut job = egui::text::LayoutJob::simple_singleline(
                item.title.clone(),
                semibold(t::TYPE_BODY_SIZE),
                *ON_SURFACE,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(words);
            let title = ui.painter().layout_job(job);
            ui.painter()
                .galley(at - vec2(0.0, title.size().y), title, *ON_SURFACE);
            let mut job = egui::text::LayoutJob::simple_singleline(
                item.line.clone(),
                regular(t::TYPE_SUB_SIZE),
                *ON_SURFACE_VARIANT,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(words);
            let line = ui.painter().layout_job(job);
            ui.painter()
                .galley(at + vec2(0.0, 2.0), line, *ON_SURFACE_VARIANT);
            focus_ring(ui, row.shrink2(vec2(4.0, 4.0)), &response, 14);
            match &item.game {
                Some(game) => self.focus_card(&response, &item.key, game, std::mem::take(first)),
                None => self.focus_item(&response, &item.key, std::mem::take(first)),
            }
            if response.clicked() {
                self.open(Screen::Game {
                    host: item.host,
                    app: item.app,
                });
            }
            if let Some(anchor) = menu_request(ui, &response, None) {
                self.open_menu(
                    Dialog::GameMenu {
                        host: item.host,
                        game: GameTarget::Library {
                            app: item.app,
                            favourite: false,
                        },
                    },
                    anchor,
                    item.key.clone(),
                );
            }
            y += PANEL_ROW;
        }
    }
}

/// The setup card's width at most; its left edge is the page's.
const SETUP_CARD_W: f32 = 420.0;
const SETUP_CARD_CLOSE: f32 = 36.0;

/// The line under the setup card's title: what a stream from this PC asks for with `settings`
/// (the PC's resolved settings), as the game page says it, then HDR or SDR as the start requests it,
/// then the codec: Auto is negotiated when the stream starts, a forced one is named.
pub(in crate::ui) fn setup_line(settings: &Settings, display: Display) -> String {
    let (w, h, fps, kbps) = stream_mode(settings, display);
    let mut line = console::stream_summary(w, h, fps, kbps);
    line.push_str(if settings.hdr { " · HDR" } else { " · SDR" });
    line.push_str(" · ");
    line.push_str(match settings.codec {
        CodecPreference::Auto => tr("codec chosen when the stream starts"),
        codec => settings::codec_name(codec),
    });
    line
}

impl Launcher {
    /// A pairing succeeded and `paired_pcs` PCs are paired now: the first one on this install
    /// (exactly one paired, no pairing seen before) gets Home's setup card. Every pairing counts
    /// as seen, so one made beside another PC leaves none for later either. True when the
    /// settings changed and want saving.
    pub fn offer_setup_card(&mut self, host: u64, paired_pcs: usize) -> bool {
        if self.settings.setup_card_seen {
            return false;
        }
        self.settings.setup_card_seen = true;
        if paired_pcs == 1 {
            self.settings.setup_card_host = Some(host);
        }
        true
    }

    /// The app starts with a PC paired. With no pairing seen, the install comes from a version
    /// before the card and made its first pairing long ago: pairing that PC again offers nothing.
    /// True when the settings changed and want saving.
    pub fn paired_before_setup_card(&mut self) -> bool {
        !std::mem::replace(&mut self.settings.setup_card_seen, true)
    }

    /// The setup card's title and line while it shows: its PC is still paired, no device waits to
    /// pair (that banner goes first), and the settings are what "Auto (recommended)" writes in all
    /// three Recommended choices. With Custom settings it says nothing, and stays for later. The
    /// rule is Android's: Recommended's own settings decide, the ones its page shows, and the line
    /// then gives what a stream to this PC starts with, a value of the PC's own included.
    pub(in crate::ui) fn setup_card_words(&self, view: &View<'_>) -> Option<(String, String)> {
        let host = view.hosts.get(self.settings.setup_card_host?)?;
        if !host.paired() || view.hosts.list.iter().any(|h| !h.waiting_to_pair().is_empty()) {
            return None;
        }
        let kind = self.network.kind(&self.profiles.home_wifi);
        let settings = host.record.settings(&self.profiles.settings(&self.settings, kind));
        let vrr_offered = self.tearing_supported || cfg!(not(windows));
        let caps = settings::simple_caps(&self.display, self.hdr_display, vrr_offered);
        let (choices, custom) = settings::simple_state(&self.settings, &self.display, &caps, vrr_offered);
        if custom || choices != nightjar_policy::simple_profile::recommended(&caps) {
            return None;
        }
        Some((trf("Set up for this device and {}", &[&host.display_name()]), setup_line(&settings, self.display)))
    }

    /// Home's card after the first pairing: the page's first content, where the pairing banner
    /// stands, so the hero and the shelves start beneath it and it scrolls away with them. Its
    /// close button is the only control, reached by Tab and the arrow keys as Home's others are.
    /// True when it was dismissed.
    pub(in crate::ui) fn setup_card(&mut self, ui: &mut Ui, view: &View<'_>) -> bool {
        if self.settings.setup_card_host.is_none() {
            return false;
        }
        // HDR Auto depends on the screen: read it as the Recommended section does, once.
        if !std::mem::replace(&mut self.setup_card_probed, true)
            && let Some(probe) = self.hdr_display_probe {
                self.hdr_display = probe();
            }
        let Some((title, line)) = self.setup_card_words(view) else {
            return false;
        };
        let width = SETUP_CARD_W.min(ui.available_width()).max(200.0);
        let place = ui.painter().add(egui::Shape::Noop);
        // The close button ends the card, as on Android: above the hero's buttons, where Up from them lands.
        let margin = Margin { left: 18, right: (SETUP_CARD_CLOSE + 18.0) as i8, top: 14, bottom: 14 };
        let shown = Frame::new().inner_margin(margin).show(ui, |ui| {
            ui.set_width(width - f32::from(margin.left) - f32::from(margin.right));
            ui.spacing_mut().item_spacing.y = space::XS;
            ui.label(RichText::new(&title).font(semibold(text::BODY)).color(ON_SURFACE));
            ui.label(RichText::new(&line).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
        });
        let rect = shown.response.rect;
        let mut shapes = Vec::new();
        glass::without_backdrop(&mut shapes, ui.ctx(), rect, t::RADIUS_GROUP, glass::Material::Menu);
        ui.painter().set(place, egui::Shape::Vec(shapes));
        let close = Rect::from_min_size(pos2(rect.right() - 8.0 - SETUP_CARD_CLOSE, rect.top() + 8.0), Vec2::splat(SETUP_CARD_CLOSE));
        let close = round_button(ui, close, Icon::Close, tr("Dismiss"));
        self.focus_item(&close, "setup-card-dismiss", false);
        // The button is small and stands between the top bar and the hero: egui's arrow keys pick
        // what lies straight above or below and would pass it. A move by Up or Down from one side
        // of the card to the other stops on it instead.
        let focused = ui.ctx().memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).map(|r| r.rect);
        if let (Some(from), Some(now)) = (self.setup_card_from.take(), focused) {
            let crossed = (from.top() >= rect.bottom() && now.bottom() <= rect.top())
                || (from.bottom() <= rect.top() && now.top() >= rect.bottom());
            if crossed && matches!(self.dialog, Dialog::None) && !egui::Popup::is_any_open(ui.ctx()) {
                close.request_focus();
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowDown)) {
            self.setup_card_from = focused;
        }
        ui.add_space(space::M);
        if close.clicked() {
            self.settings.setup_card_host = None;
        }
        close.clicked()
    }
}

/// The huge title on one line, cut with "…" when it does not fit.
pub(super) fn hero_title(ui: &Ui, left: f32, top: f32, width: f32, title: &str) {
    let format = egui::TextFormat {
        font_id: semibold(TITLE_SIZE),
        color: *ON_SURFACE,
        extra_letter_spacing: t::TYPE_HERO_TRACK,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(title.to_owned(), format);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(40.0));
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(left, top + (TITLE_LINE - galley.size().y) / 2.0),
        galley,
        *ON_SURFACE,
    );
}
