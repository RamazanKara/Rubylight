//! A game's page: its cover glowing in its own ambient, the state and store over a large title, the
//! PC's line, the one moon button with Favourite and Quit game, and a few rows on the art.

use super::chrome::{capsule_width, control_capsule, info_chip, play_button, play_width};
use super::design_tokens as t;
use super::dialogs::hide_keys_of;
use super::pcs::{pc_action, pc_line_of, PcAction, PcLine};
use super::*;
use super::{glass, spring};
use egui::Shape;

/// The cover's share of the page, its limits, and what it leaves under it in a short window.
const COVER_SHARE: f32 = 0.2835;
const COVER_MIN_W: f32 = 160.0;
const COVER_MAX_W: f32 = 420.0;
const COVER_FOOT: f32 = 48.0;
/// The glow of the cover's colour and the dark shadow under it.
const GLOW_BLUR: f32 = 110.0;
const SHADOW_Y: f32 = 30.0;
const SHADOW_BLUR: f32 = 70.0;
/// Between cover and column, as a share of the page and at most.
const GAP_SHARE: f32 = 0.04;
const GAP_LEAST: f32 = 24.0;
const GAP_MOST: f32 = 52.0;
/// Under this width the column moves below the cover.
const COLUMN_LEAST: f32 = 400.0;
const COLUMN_MOST: f32 = 820.0;
const CHIP_H: f32 = 32.0;
const CHIP_GAP: f32 = 8.0;
const TITLE_LEAST: f32 = 34.0;
const TITLE_PER_WIDTH: f32 = 0.11;
const PLAY_H: f32 = 64.0;
const PLAY_LEAST_W: f32 = 228.0;
const CAPSULE_H: f32 = 56.0;
const BUTTON_GAP: f32 = 12.0;
const ROW_PAD: f32 = 18.0;
const ROW_V_PAD: f32 = 11.0;
const SWITCH: Vec2 = vec2(46.0, 28.0);
const THUMB_R: f32 = 11.0;
const SWITCH_TUNE: spring::Tune = spring::Tune {
    stiffness: t::MOTION_SWITCH_STIFFNESS,
    ratio: t::MOTION_SWITCH_RATIO,
};

/// Where a game's page puts its cover and its column in `width` from `top_left`, with the window's
/// foot at `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GameLayout {
    pub cover: Rect,
    pub column: Rect,
}

pub(super) fn game_layout(top_left: Pos2, width: f32, bottom: f32) -> GameLayout {
    let tall = ((bottom - top_left.y - COVER_FOOT) / 1.5).max(COVER_MIN_W);
    let cover_w = (width * COVER_SHARE)
        .clamp(COVER_MIN_W, COVER_MAX_W)
        .min(tall)
        .floor();
    let cover = Rect::from_min_size(top_left, vec2(cover_w, (cover_w * 1.5).floor()));
    let gap = (width * GAP_SHARE).clamp(GAP_LEAST, GAP_MOST);
    let beside = width - cover_w - gap;
    let column = if beside >= COLUMN_LEAST {
        Rect::from_min_max(
            pos2(cover.right() + gap, cover.top()),
            pos2(cover.right() + gap + beside.min(COLUMN_MOST), f32::INFINITY),
        )
    } else {
        Rect::from_min_max(
            pos2(top_left.x, cover.bottom() + gap),
            pos2(top_left.x + width, f32::INFINITY),
        )
    };
    GameLayout { cover, column }
}

/// What a row on the art does when chosen.
enum RowAct {
    Settings(nightjar_core::http::App),
    QuitOnLeave {
        uuid: String,
        id: i32,
        on: bool,
    },
    Hide {
        games: Vec<crate::config::Favourite>,
        on: bool,
    },
    InputOnly {
        app: i32,
        resume: bool,
    },
    Copy {
        host: u64,
        app: u32,
    },
    Nothing,
}

/// One row of the group on the art. A row with a switch has it from its `act`.
struct ArtRow {
    key: String,
    label: String,
    /// The line under the label, with its ink, and what a pointer or a screen reader gets if it is cut.
    sub: Option<(String, Color32)>,
    said: Option<String>,
    enabled: bool,
    act: RowAct,
}

impl ArtRow {
    fn switch(&self) -> Option<bool> {
        match self.act {
            RowAct::QuitOnLeave { on, .. } | RowAct::Hide { on, .. } => Some(on),
            _ => None,
        }
    }
}

/// A switch on a tint: moon when on, a paper thumb.
fn paint_switch(ui: &Ui, out: &mut Vec<Shape>, rect: Rect, id: Id, on: bool, enabled: bool) {
    let ctx = ui.ctx();
    let along = spring::animate(ctx, id, if on { 1.0 } else { 0.0 }, SWITCH_TUNE);
    let r = rect.height() / 2.0;
    let off: Color32 = if high_contrast() {
        *SURFACE
    } else {
        t::TINT_TRACK_FILL
    };
    let lit: Color32 = *MOON;
    let track = off.lerp_to_gamma(lit, along.clamp(0.0, 1.0));
    let dim = if enabled { 1.0 } else { 0.4 };
    glass::solid(out, ctx, rect, r, track.gamma_multiply(dim));
    if high_contrast() {
        glass::rim(out, ctx, rect, r, 1.5, &|_| t::HC_COLOR_RIM);
    }
    let reach = rect.width() / 2.0 - r;
    let center = rect.center() + vec2(egui::lerp(-reach..=reach, along), 0.0);
    out.push(Shape::circle_filled(
        center + vec2(0.0, 1.0),
        THUMB_R,
        Color32::from_black_alpha(50),
    ));
    out.push(Shape::circle_filled(
        center,
        THUMB_R,
        (*PAPER).gamma_multiply(dim),
    ));
}

/// The large title: up to two lines, then cut, as large as the column lets a name be.
fn title_galley(ui: &Ui, title: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let size = (width * TITLE_PER_WIDTH).clamp(TITLE_LEAST, t::TYPE_HERO_SIZE);
    let format = egui::TextFormat {
        font_id: semibold(size),
        color: *ON_SURFACE,
        extra_letter_spacing: t::TYPE_HERO_TRACK,
        line_height: Some((size * 1.1).round()),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(title.to_owned(), format);
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 2,
        break_anywhere: false,
        overflow_character: Some('\u{2026}'),
    };
    ui.painter().layout_job(job)
}

impl Launcher {
    pub(super) fn game(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        host_id: u64,
        app_id: u32,
        commands: &mut Vec<Command>,
    ) {
        let Some(host) = view.hosts.get(host_id) else {
            self.leave();
            return;
        };
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let source = sources
            .iter()
            .find(|s| s.id == host_id)
            .expect("source of a listed host");
        let Some(game) = source
            .library
            .and_then(|l| l.games.iter().find(|g| g.app_id == app_id))
        else {
            ui.label(
                RichText::new(tr("This game is no longer in the PC's library."))
                    .font(semibold(t::TYPE_SECTION_SIZE))
                    .color(ON_SURFACE),
            );
            return;
        };
        let state = self.play_state(host, source, game);
        // Play starts from the PC's app list: read it while the player is here, not on the press.
        if state.detail.can_start && matches!(host.apps, Apps::NotLoaded) {
            commands.push(Command::LoadApps(host_id));
        }
        let ctx = ui.ctx().clone();
        let width = ui.available_width();
        let origin = ui.cursor().min;
        let GameLayout { cover, column } = game_layout(origin, width, ctx.content_rect().bottom());
        // The cover takes no focus of its own; a right-click offers what its card does.
        let cover_response = ui.interact(cover, Id::unique("game-cover"), Sense::CLICK);
        cover_response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, game.title.clone()));
        let art = shown_art(view, host_id, app_id);
        let mut out = Vec::new();
        glass::cover_glow(
            &mut out,
            &ctx,
            cover,
            t::RADIUS_COVER,
            self.ambient.glow(),
            GLOW_BLUR,
            SHADOW_Y,
            SHADOW_BLUR,
        );
        cover_shapes(
            ui,
            &mut out,
            cover,
            cover.width(),
            t::RADIUS_COVER,
            &game.title,
            art,
            1.0,
        );
        ui.painter().extend(out);
        request_cover(
            ui,
            view,
            host_id,
            app_id,
            &game.artwork.version,
            &cover_response,
            commands,
        );
        let target = GameTarget::Library {
            app: app_id,
            favourite: false,
        };
        if let Some(anchor) = menu_request(ui, &cover_response, None) {
            let key = self.focused_key.clone().unwrap_or_default();
            self.open_menu(
                Dialog::GameMenu {
                    host: host_id,
                    game: target,
                },
                anchor,
                key,
            );
        }

        let (left, right) = (column.left(), column.right());
        let room = column.width();
        let mut y = column.top() + 2.0;
        // The state and the store.
        let store = game
            .store
            .as_ref()
            .map(|s| console::store_name(&s.source, &s.name))
            .filter(|n| !n.is_empty());
        let mut x = left;
        let mid = y + CHIP_H / 2.0;
        match state.detail.state {
            GameState::Running => {
                x += info_chip(ui, x, mid, Some(*OK), tr("Running")) + CHIP_GAP;
            }
            GameState::Suspended => {
                x += info_chip(ui, x, mid, Some(*TERTIARY), tr("Suspended")) + CHIP_GAP;
            }
            GameState::Idle => {}
        }
        if let Some(store) = &store {
            info_chip(ui, x, mid, None, store);
        }
        // Their line is held without any, so a game that starts moves nothing.
        y += CHIP_H + 14.0;
        let title = title_galley(ui, &game.title, room);
        ui.painter()
            .galley(pos2(left, y), title.clone(), *ON_SURFACE);
        let title_h = title.size().y;
        if title.elided {
            let _ = cover_response.clone().on_hover_text(game.title.clone());
        }
        y += title_h + 8.0;
        y = self.pc_words(ui, host, &state, source.name, left, y, room) + 22.0;

        // One line, only when there is something to know.
        let list_problem = state.detail.can_start
            && state.classic.is_none()
            && matches!(host.apps, Apps::Failed(_) | Apps::Loaded(_));
        let note = if list_problem {
            let why = match &host.apps {
                Apps::Failed(e) => trf("The app list could not be loaded. {}", &[&e]),
                _ => tr("The PC's app list has no entry for this game.").to_owned(),
            };
            Some((why, *ERROR))
        } else {
            state
                .other_running
                .as_ref()
                .filter(|_| state.detail.can_start)
                .map(|other| {
                    (
                        trf(
                            "{} is running. Playing this game closes it.",
                            &[&at_sentence_start(other)],
                        ),
                        *TERTIARY,
                    )
                })
        };
        if let Some((words, ink)) = note {
            let line = ui
                .painter()
                .layout(words, regular(t::TYPE_SUB_SIZE), ink, room);
            ui.painter().galley(pos2(left, y - 8.0), line.clone(), ink);
            y += line.size().y + 14.0;
        }

        y = self.game_buttons(
            ui,
            host,
            source,
            game,
            &state,
            list_problem,
            left,
            right,
            y,
            commands,
        );
        y += 26.0;
        let rows = self.game_rows(view, host, source, game, &state, &sources);
        y = self.paint_rows(
            ui,
            &rows,
            Rect::from_min_max(pos2(left, y), pos2(right, f32::INFINITY)),
            host,
            commands,
        );

        let bottom = y.max(cover.bottom());
        ui.advance_cursor_after_rect(Rect::from_min_max(origin, pos2(origin.x + width, bottom)));
    }

    /// The PC the game is on, and what keeps it from playing if something does. Returns its foot.
    #[allow(clippy::too_many_arguments)]
    fn pc_words(
        &self,
        ui: &Ui,
        host: &Host,
        state: &PlayState,
        name: &str,
        left: f32,
        y: f32,
        room: f32,
    ) -> f32 {
        const ICON: f32 = 20.0;
        let why = match pc_line_of(host) {
            PcLine::Offline => Some(tr("Offline")),
            PcLine::NotPaired => Some(tr("Not paired")),
            PcLine::Checking => Some(tr("Checking…")),
            PcLine::Online | PcLine::Running(_)
                if !state.detail.can_start && !state.detail.installed =>
            {
                Some(tr("Not installed"))
            }
            PcLine::Online | PcLine::Running(_) => None,
        };
        let words = why.map_or_else(|| name.to_owned(), |why| format!("{name}  \u{b7}  {why}"));
        let ink = *ON_SURFACE_VARIANT;
        let mut job =
            egui::text::LayoutJob::simple_singleline(words, regular(t::TYPE_HEADLINE_SIZE), ink);
        job.wrap = egui::text::TextWrapping::truncate_at_width(room - ICON - 8.0);
        let galley = ui.painter().layout_job(job);
        let h = galley.size().y.max(ICON);
        icons::paint(
            ui.painter(),
            Icon::Monitor,
            Rect::from_min_size(pos2(left, y + (h - ICON) / 2.0), Vec2::splat(ICON)),
            ink,
        );
        ui.painter().galley(
            pos2(left + ICON + 8.0, y + (h - galley.size().y) / 2.0),
            galley,
            ink,
        );
        y + h
    }

    /// The hero's buttons, in as many lines as the column needs. Returns their foot.
    #[allow(clippy::too_many_arguments)]
    fn game_buttons(
        &mut self,
        ui: &mut Ui,
        host: &Host,
        source: &Source<'_>,
        game: &nightjar_core::http::native_api::library::Game,
        state: &PlayState,
        list_problem: bool,
        left: f32,
        right: f32,
        top: f32,
        commands: &mut Vec<Command>,
    ) -> f32 {
        let mut flow = Flow {
            left,
            right,
            x: left,
            top,
            line_h: PLAY_H,
        };
        let target = GameTarget::Library {
            app: game.app_id,
            favourite: false,
        };
        // A PC that cannot play yet offers what would let it: the one moon button either way.
        let away = host.reach == Reach::Offline || (host.reach == Reach::Online && !host.paired());
        let could_play = state.could_play(host);
        let mut any_first = false;
        if away {
            let action = pc_action(host);
            let words = action.words();
            let w = play_width(ui, words).max(PLAY_LEAST_W);
            let rect = flow.place(w, PLAY_H);
            let icon = match action {
                PcAction::Pair => Icon::Lock,
                other => other.icon().unwrap_or(Icon::Refresh),
            };
            let r = play_button(ui, rect, Id::unique("game-play"), icon, words, true, false);
            self.focus_item(&r, "play", true);
            any_first = true;
            if r.clicked() {
                commands.push(action.command(host.id));
            }
        } else {
            let label = if state.resumes {
                tr("Resume")
            } else {
                tr("Play")
            };
            let working = false;
            let w = play_width(ui, label).max(PLAY_LEAST_W);
            let rect = flow.place(w, PLAY_H);
            let r = play_button(
                ui,
                rect,
                Id::unique("game-play"),
                Icon::Play,
                label,
                could_play,
                working,
            );
            self.focus_item(&r, "play", could_play);
            any_first |= could_play;
            if r.clicked() {
                self.press_play(host, game, state, commands);
            }
            if let Some(anchor) = menu_request(ui, &r, None) {
                self.open_menu(
                    Dialog::GameMenu {
                        host: host.id,
                        game: target,
                    },
                    anchor,
                    "play".into(),
                );
            }
        }
        if list_problem {
            let words = tr("Try again");
            let rect = flow.place(capsule_width(ui, words), CAPSULE_H);
            let r = control_capsule(
                ui,
                rect,
                Id::unique("game-retry"),
                Icon::Refresh,
                words,
                words,
                true,
            );
            self.focus_item(&r, "retry", !any_first);
            any_first = true;
            if r.clicked() {
                commands.push(Command::Refresh(host.id));
            }
        }
        let star = console::favourite_of(source, game);
        let starred = console::is_starred(source, game, &self.favourites);
        let words = tr("Favourite");
        let rect = flow.place(capsule_width(ui, words), CAPSULE_H);
        let said = if starred {
            tr("Remove from favourites")
        } else {
            tr("Add to favourites")
        };
        let glyph = if starred { Icon::StarOn } else { Icon::Star };
        let r = control_capsule(
            ui,
            rect,
            Id::unique("game-star"),
            glyph,
            words,
            said,
            star.is_some(),
        );
        // With Play off and nothing to end, the star takes the focus, so the D-pad has somewhere to start.
        self.focus_item(
            &r,
            "favourite",
            !any_first && state.detail.end == EndAction::None,
        );
        if r.clicked() {
            if let Some(star) = star {
                commands.push(Command::ToggleFavourite(star));
            }
        }
        if state.detail.end != EndAction::None {
            let closing = host.native.acting;
            let words = tr("Quit game");
            let rect = flow.place(capsule_width(ui, words), CAPSULE_H);
            let r = control_capsule(
                ui,
                rect,
                Id::unique("game-end"),
                Icon::Power,
                words,
                words,
                !closing,
            );
            self.focus_item(&r, "end", !any_first);
            if closing {
                let spin = Rect::from_min_size(
                    pos2(rect.left() + 20.0, rect.center().y - 9.5),
                    Vec2::splat(19.0),
                );
                ui.scope_builder(UiBuilder::new().max_rect(spin), |ui| spinner(ui, 9.0));
            }
            if r.clicked() {
                self.press_end(host, game, state);
            }
        }
        flow.foot()
    }

    /// The rows on the art: stream settings, what happens on leaving, hiding, and the same game on
    /// other PCs.
    fn game_rows(
        &self,
        view: &View<'_>,
        host: &Host,
        source: &Source<'_>,
        game: &nightjar_core::http::native_api::library::Game,
        state: &PlayState,
        sources: &[Source<'_>],
    ) -> Vec<ArtRow> {
        use nightjar_policy::launch::{self, InputOnlyBlocker};
        let classic = state.classic.as_ref();
        let kind = self.network.kind(&self.profiles.home_wifi);
        let mut settings = host
            .record
            .settings(&self.profiles.settings(&self.settings, kind));
        let own = classic.and_then(|a| host.record.app_settings(&a.uuid, a.id));
        if let Some(overrides) = own {
            settings = overrides.apply(&settings);
        }
        let (w, h, fps, kbps) = stream_mode(&settings, self.display);
        let mut summary = console::stream_summary(w, h, fps, kbps);
        if let Some(profile) = self
            .profiles
            .selected(kind)
            .and_then(|id| self.profiles.entries.iter().find(|p| p.id == id))
        {
            summary.push_str("  \u{b7}  ");
            summary.push_str(&profile.name);
        }
        for from in summary_sources(
            self.profiles.profile(kind),
            host.record.settings.as_ref(),
            own,
        ) {
            summary.push_str("  \u{b7}  ");
            summary.push_str(&from);
        }
        let mut rows = vec![ArtRow {
            key: "settings".into(),
            label: tr("Stream settings").to_owned(),
            sub: Some((summary, *ON_SURFACE_VARIANT)),
            said: None,
            enabled: classic.is_some(),
            act: classic.map_or(RowAct::Nothing, |app| RowAct::Settings(app.clone())),
        }];
        if let Some(app) = classic {
            if launch::offers_quit_on_leave(
                host.paired(),
                policy_control(app.control),
                policy_role(app.role(false)),
            ) {
                let on = host.record.quits_on_leave(&app.uuid, app.id);
                rows.push(ArtRow {
                    key: "quit-on-leave".into(),
                    label: tr("Quit game when I leave").to_owned(),
                    sub: None,
                    said: Some(
                        if on {
                            tr("On: Disconnect also quits it on the PC")
                        } else {
                            tr("Off: Disconnect leaves it running on the PC")
                        }
                        .to_owned(),
                    ),
                    enabled: true,
                    act: RowAct::QuitOnLeave {
                        uuid: app.uuid.clone(),
                        id: app.id,
                        on,
                    },
                });
            }
        }
        if !host.record.unique_id.is_empty() {
            let on = console::is_hidden(source, &game.install_id, game.app_id, &self.hidden_games);
            let target = GameTarget::Library {
                app: game.app_id,
                favourite: false,
            };
            rows.push(ArtRow {
                key: "hide".into(),
                label: tr("Hide game").to_owned(),
                sub: None,
                said: None,
                enabled: true,
                act: RowAct::Hide {
                    games: hide_keys_of(host, target, sources),
                    on,
                },
            });
        }
        if let Some(app) = classic {
            if let Some(blocker) =
                small_stream_offer(host, app, state.resumes, self.settings.input_only_any_host)
            {
                let (note, ink) = match blocker {
                    Some(InputOnlyBlocker::Pair) => (tr("Pair with this PC first").to_owned(), *ERROR),
                    Some(InputOnlyBlocker::ViewPermission) => (tr("Allow viewing streams for this device on the host").to_owned(), *ERROR),
                    Some(InputOnlyBlocker::InputPermission) => (tr("Allow input for this device on the host").to_owned(), *ERROR),
                    None => (
                        trf("Video still streams at the smallest size, about {} Mbit/s ({} MB an hour), but isn’t shown", &[
                            &format!("{:.1}", f64::from(launch::estimated_kbps()) / 1000.0),
                            &launch::estimated_megabytes_per_hour().to_string(),
                        ]),
                        *ON_SURFACE_VARIANT,
                    ),
                };
                rows.push(ArtRow {
                    key: "input-only".into(),
                    label: tr("Input-only").to_owned(),
                    sub: Some((note, ink)),
                    said: None,
                    enabled: blocker.is_none() && state.can_play,
                    act: RowAct::InputOnly {
                        app: app.id,
                        resume: state.resumes,
                    },
                });
            }
        }
        let merged = console::merge_library(sources);
        let others = merged
            .iter()
            .find(|e| {
                e.copies
                    .iter()
                    .any(|c| c.host == host.id && c.app_id == game.app_id)
            })
            .map(|e| {
                e.copies
                    .iter()
                    .filter(|c| !(c.host == host.id && c.app_id == game.app_id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for copy in others {
            // A copy there that is not installed opens, but says so first.
            let name = if copy.installed {
                copy.host_name.clone()
            } else {
                trf("{}, not installed", &[&copy.host_name])
            };
            rows.push(ArtRow {
                key: format!("copy:{}", copy.host),
                label: tr("Also on").to_owned(),
                sub: Some((name, *ON_SURFACE_VARIANT)),
                said: None,
                enabled: true,
                act: RowAct::Copy {
                    host: copy.host,
                    app: copy.app_id,
                },
            });
        }
        let _ = view;
        rows
    }

    /// The group of rows at the top of `area`. Returns its foot.
    fn paint_rows(
        &mut self,
        ui: &mut Ui,
        rows: &[ArtRow],
        area: Rect,
        host: &Host,
        commands: &mut Vec<Command>,
    ) -> f32 {
        let ctx = ui.ctx().clone();
        let width = area.width();
        // Set first: the group's height is what its words need.
        let set: Vec<_> = rows
            .iter()
            .map(|row| {
                let trailing = if row.switch().is_some() {
                    SWITCH.x
                } else {
                    16.0
                };
                let room = width - 2.0 * ROW_PAD - trailing - 12.0;
                let ink = if row.enabled { *ON_SURFACE } else { *OUTLINE };
                let mut label = egui::text::LayoutJob::simple(
                    row.label.clone(),
                    semibold(t::TYPE_BODY_SIZE),
                    ink,
                    room,
                );
                label.wrap.max_rows = 2;
                label.wrap.overflow_character = Some('\u{2026}');
                let label = ui.painter().layout_job(label);
                let sub = row.sub.as_ref().map(|(words, sub_ink)| {
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        words.clone(),
                        regular(t::TYPE_SUB_SIZE),
                        *sub_ink,
                    );
                    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                    ui.painter().layout_job(job)
                });
                let words = label.size().y + sub.as_ref().map_or(0.0, |s| s.size().y + 2.0);
                let h = (words + 2.0 * ROW_V_PAD).max(t::SIZE_ROW_HEIGHT);
                (label, sub, h)
            })
            .collect();
        let total: f32 = set.iter().map(|s| s.2).sum();
        let group = Rect::from_min_size(area.min, vec2(width, total));
        let mut out = Vec::new();
        glass::group(
            &mut out,
            &ctx,
            group,
            t::RADIUS_GROUP,
            t::TINT_GROUP_ON_ART_FILL,
        );
        ui.painter().extend(out);
        let mut y = group.top();
        for (n, (row, (label, sub, h))) in rows.iter().zip(set).enumerate() {
            let rect = Rect::from_min_size(pos2(group.left(), y), vec2(width, h));
            let id = Id::unique(("game-row", &row.key));
            let sense = if row.enabled {
                Sense::click()
            } else {
                Sense::hover()
            };
            let response = ui.interact(rect, id, sense);
            let said = row
                .said
                .clone()
                .or_else(|| row.sub.as_ref().map(|(w, _)| w.clone()))
                .unwrap_or_default();
            response.widget_info(|| match row.switch() {
                Some(on) => egui::WidgetInfo::selected(
                    egui::Role::Switch,
                    row.enabled,
                    on,
                    row.label.clone(),
                ),
                None => {
                    egui::WidgetInfo::labeled(egui::Role::Button, row.enabled, row.label.clone())
                }
            });
            if !said.is_empty() {
                name_for_readers(ui, &response, &row.label, &said);
            }
            let feel = glass::feel(&ctx, &response, 1.0, 1.0);
            let inner = rect.shrink2(vec2(4.0, 3.0));
            let mut out = Vec::new();
            if row.enabled {
                lift_shapes(
                    &mut out,
                    &ctx,
                    inner,
                    ROW_INNER_R,
                    feel.hovered,
                    feel.pressed,
                );
            }
            if n > 0 {
                out.push(Shape::hline(
                    rect.left() + ROW_PAD..=rect.right(),
                    y,
                    Stroke::new(1.0, t::TINT_SEPARATOR),
                ));
            }
            let words_h = label.size().y + sub.as_ref().map_or(0.0, |s| s.size().y + 2.0);
            let mut text_y = rect.center().y - words_h / 2.0;
            let cut = sub.as_ref().is_some_and(|s| s.elided);
            let ink = if row.enabled { *ON_SURFACE } else { *OUTLINE };
            let label_h = label.size().y;
            out.push(Shape::galley(
                pos2(rect.left() + ROW_PAD, text_y),
                label,
                ink,
            ));
            text_y += label_h + 2.0;
            if let Some(sub) = sub {
                let sub_ink = row.sub.as_ref().map_or(*ON_SURFACE_VARIANT, |s| s.1);
                out.push(Shape::galley(
                    pos2(rect.left() + ROW_PAD, text_y),
                    sub,
                    sub_ink,
                ));
            }
            match row.switch() {
                Some(on) => {
                    let track = Rect::from_min_size(
                        pos2(
                            rect.right() - ROW_PAD - SWITCH.x,
                            rect.center().y - SWITCH.y / 2.0,
                        ),
                        SWITCH,
                    );
                    paint_switch(ui, &mut out, track, id.with("switch"), on, row.enabled);
                }
                None if matches!(row.act, RowAct::Nothing) => {}
                None => {
                    let tip = pos2(rect.right() - ROW_PAD - 3.0, rect.center().y);
                    let ink: Color32 = if row.enabled {
                        *ON_SURFACE_VARIANT
                    } else {
                        *OUTLINE
                    };
                    out.push(Shape::line(
                        vec![tip + vec2(-5.0, -6.0), tip, tip + vec2(-5.0, 6.0)],
                        Stroke::new(1.8, ink),
                    ));
                }
            }
            if feel.focused {
                glass::focus(&mut out, &ctx, inner, ROW_INNER_R, false);
            }
            ui.painter().extend(out);
            if cut {
                let full = row.sub.as_ref().map_or_else(String::new, |s| s.0.clone());
                let _ = response.clone().on_hover_text(full);
            }
            self.focus_item(&response, &row.key, false);
            if response.clicked() {
                match &row.act {
                    RowAct::Settings(app) => commands.push(Command::AppSettings {
                        host: host.id,
                        app: app.clone(),
                    }),
                    RowAct::QuitOnLeave { uuid, id, on } => {
                        commands.push(Command::SetQuitOnLeave {
                            host: host.id,
                            uuid: uuid.clone(),
                            id: *id,
                            enabled: !on,
                        })
                    }
                    RowAct::Hide { games, on } => commands.push(Command::SetHidden {
                        games: games.clone(),
                        hidden: !on,
                    }),
                    RowAct::InputOnly { app, resume } => commands.push(Command::LaunchInputOnly {
                        host: host.id,
                        app: *app,
                        resume: *resume,
                    }),
                    RowAct::Copy { host, app } => self.go(Screen::Game {
                        host: *host,
                        app: *app,
                    }),
                    RowAct::Nothing => {}
                }
            }
            y += h;
        }
        y
    }
}

const ROW_INNER_R: f32 = 16.0;

/// Places buttons left to right on a common centre line, starting a new line when one would not fit.
struct Flow {
    left: f32,
    right: f32,
    x: f32,
    top: f32,
    line_h: f32,
}

impl Flow {
    fn place(&mut self, w: f32, h: f32) -> Rect {
        if self.x > self.left && self.x + w > self.right {
            self.x = self.left;
            self.top += self.line_h + BUTTON_GAP;
        }
        let rect =
            Rect::from_min_size(pos2(self.x, self.top + (self.line_h - h) / 2.0), vec2(w, h));
        self.x += w + BUTTON_GAP;
        rect
    }

    fn foot(&self) -> f32 {
        self.top + self.line_h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_is_the_targets_at_1280_and_keeps_its_cover_in_a_short_window() {
        let at = pos2(48.0, 156.0);
        let wide = game_layout(at, 1184.0, 800.0);
        assert_eq!(wide.cover.size(), vec2(335.0, 502.0));
        assert!(wide.column.left() - wide.cover.right() >= 47.0);
        assert!(
            (wide.column.right() - (48.0 + 1184.0)).abs() < 1.0,
            "{wide:?}"
        );
        // A short window shrinks the cover, not its shape.
        let short = game_layout(at, 1184.0, 520.0);
        assert!(
            short.cover.bottom() <= 520.0 - COVER_FOOT + 1.0,
            "{short:?}"
        );
        assert!((short.cover.height() / short.cover.width() - 1.5).abs() < 0.01);
        // Too narrow for a column beside it, the column goes under.
        let narrow = game_layout(at, 480.0, 800.0);
        assert!(
            narrow.column.top() >= narrow.cover.bottom() && narrow.column.width() == 480.0,
            "{narrow:?}"
        );
    }

    #[test]
    fn buttons_flow_to_a_new_line_on_a_common_centre() {
        let mut flow = Flow {
            left: 0.0,
            right: 300.0,
            x: 0.0,
            top: 10.0,
            line_h: PLAY_H,
        };
        let play = flow.place(228.0, PLAY_H);
        let star = flow.place(120.0, CAPSULE_H);
        assert_eq!(play.left(), 0.0);
        assert_eq!(star.left(), 0.0, "no room beside Play");
        assert!(star.top() > play.bottom());
        assert_eq!(flow.foot(), 10.0 + PLAY_H + BUTTON_GAP + PLAY_H);
        let mut flow = Flow {
            left: 0.0,
            right: 600.0,
            x: 0.0,
            top: 0.0,
            line_h: PLAY_H,
        };
        let (a, b) = (flow.place(228.0, PLAY_H), flow.place(120.0, CAPSULE_H));
        assert_eq!(a.center().y, b.center().y);
    }
}
