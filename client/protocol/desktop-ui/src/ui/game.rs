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
/// The widest any cover is drawn: covers are kept no larger than this page shows them.
pub(super) const COVER_MAX_W: f32 = crate::hosts::COVER_POINTS[0];
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
    Settings(nightjar_policy::http::App),
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
    /// The profile the next stream uses: a list of the profiles, or, when the network's rule picks
    /// the profile (`decided`), the Profiles page where the rules are.
    Profile {
        decided: bool,
    },
    /// Frame generation in library app `game`: on, and whether the PC's notice was accepted.
    Neural {
        game: u32,
        on: bool,
        accepted: bool,
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
    /// The line under the label wraps, all of it shown, instead of being cut to one line.
    wrap: bool,
    enabled: bool,
    act: RowAct,
}

impl ArtRow {
    fn switch(&self) -> Option<bool> {
        match self.act {
            RowAct::QuitOnLeave { on, .. }
            | RowAct::Hide { on, .. }
            | RowAct::Neural { on, .. } => Some(on),
            _ => None,
        }
    }
}

/// The PC lists Neural Rendering's frame generation for this device.
fn offers_neural(host: &Host) -> bool {
    host.info.as_ref().is_some_and(|info| {
        info.capabilities
            .has(nightjar_policy::http::Feature::NeuralFrameGeneration)
    })
}

impl Launcher {
    /// The frame generation row: its switch, and the line that says why it can't change, what
    /// the PC is doing, what turning it on replaces, or what it is for.
    fn neural_row(&self, host: &Host, game: u32) -> ArtRow {
        use crate::neural_page::{
            configurable, installed_text, job_text, switch_text, with_gpu_note,
        };
        let open = self.neural.state_for(host.id, game);
        let state = open.and_then(|open| open.state.as_ref());
        let switch = state.and_then(|state| state.frame_generation());
        let busy = open.is_some_and(|open| open.writing) || state.is_some_and(|s| s.working());
        let failure = self.neural.failure_for(host.id, game).map(str::to_owned);
        let (line, ink) = if let Some(error) =
            failure.or_else(|| open.and_then(|o| o.error.clone()))
        {
            (error, *ERROR)
        } else if let Some(job) = state.and_then(job_text) {
            let failed = state
                .and_then(|s| s.job.as_ref())
                .is_some_and(|job| job.phase == nightjar_policy::http::NeuralPhase::Failed);
            (job, if failed { *ERROR } else { *ON_SURFACE_VARIANT })
        } else if let Some(switch) = switch.filter(|s| !configurable(s)) {
            (switch_text(switch), *ON_SURFACE_VARIANT)
        } else if let Some(installed) = state.and_then(installed_text) {
            (with_gpu_note(state, installed), *ON_SURFACE_VARIANT)
        } else if state.is_some_and(|s| s.replaces_earlier_upscaling)
            && !switch.is_some_and(|s| s.on)
        {
            (
                with_gpu_note(
                    state,
                    tr("Turning it on removes the earlier neural upscaling from the game’s folder first.").to_owned(),
                ),
                *ON_SURFACE_VARIANT,
            )
        } else if state.is_none() {
            (tr("Loading…").to_owned(), *ON_SURFACE_VARIANT)
        } else if switch.is_none() {
            (
                tr("The PC offers no frame generation for this game.").to_owned(),
                *ON_SURFACE_VARIANT,
            )
        } else {
            (
                with_gpu_note(
                    state,
                    tr("Frame generation for games that offer DLSS frame generation, through a mod the PC puts in the game’s folder and takes out again: turn on DLSS frame generation in the game, and this takes its place. Leave it off for online games: one may treat a mod as cheating.").to_owned(),
                ),
                *ON_SURFACE_VARIANT,
            )
        };
        ArtRow {
            key: "neural".into(),
            label: tr("Frame generation").to_owned(),
            sub: Some((line, ink)),
            said: None,
            // What it is for and why it can't change are sentences a player reads whole.
            wrap: true,
            enabled: switch.is_some_and(configurable) && !busy,
            act: match switch {
                Some(switch) => RowAct::Neural {
                    game,
                    on: switch.on,
                    accepted: state.is_some_and(|s| s.accepted),
                },
                None => RowAct::Nothing,
            },
        }
    }
}

/// The large title: up to two lines, then cut, as large as the column lets a name be.
/// A game's facts as the game page writes them, one line each: its genres, its themes and its
/// time to beat ("Story 20 h · Story + extras 24 h · Everything 90 h"); a line without anything
/// to say is left out.
pub(super) fn facts_lines(facts: &nightjar_policy::game_info::cache::Facts) -> Vec<String> {
    use nightjar_policy::game_info::library::hours;
    let mut lines = Vec::new();
    if !facts.genres.is_empty() {
        lines.push(format!("{}: {}", tr("Genres"), facts.genres.join(" · ")));
    }
    if !facts.themes.is_empty() {
        lines.push(format!("{}: {}", tr("Themes"), facts.themes.join(" · ")));
    }
    let times: Vec<String> = [(en("Story"), facts.hastily), (en("Story + extras"), facts.normally), (en("Everything"), facts.completely)]
        .into_iter()
        .filter_map(|(label, seconds)| Some(format!("{} {}", tr(label), trf("{} h", &[&hours(seconds)?]))))
        .collect();
    if !times.is_empty() {
        lines.push(format!("{}: {}", tr("Time to beat"), times.join(" · ")));
    }
    lines
}

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
                RichText::new(tr("This game is no longer in the PC’s library."))
                    .font(semibold(t::TYPE_SECTION_SIZE))
                    .color(ON_SURFACE),
            );
            return;
        };
        let state = self.play_state(host, source, game);
        // Neural Rendering reads the PC while the page is open, if the PC offers it.
        if offers_neural(host) && self.neural.wanted(host_id, app_id) {
            commands.push(Command::Neural {
                host: host_id,
                game: app_id,
                app: game.install_id.clone(),
            });
        }
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
        request_cover(ui, view, host_id, app_id, &cover_response, commands);
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
        // What IGDB says of the game, with the player's keys: its genres, themes and time to beat.
        // By the game id alone, as All games keys the same game's facts.
        let looked_up = crate::hosts::game_of(&game.title, game.game_id.as_deref(), None);
        let facts_key = nightjar_policy::game_info::names::key(&looked_up.name, looked_up.steam);
        if view.hosts.facts_offered() && !view.hosts.facts_asked(&facts_key) {
            commands.push(Command::WantFacts(vec![looked_up]));
        }
        if let Some(facts) = view.hosts.facts(&facts_key) {
            for line in facts_lines(facts) {
                let galley = ui.painter().layout(line, regular(t::TYPE_SUB_SIZE), *ON_SURFACE_VARIANT, room);
                ui.painter().galley(pos2(left, y), galley.clone(), *ON_SURFACE_VARIANT);
                y += galley.size().y + 4.0;
            }
            y += 6.0;
        }
        y = self.pc_words(ui, host, &state, source.name, left, y, room) + 22.0;

        // One line, only when there is something to know.
        let problem = state.list_problem(host);
        let list_problem = problem.is_some();
        let note = if let Some(why) = problem {
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
            PcLine::Online | PcLine::Running(_) | PcLine::Away(_)
                if !state.detail.can_start && !state.detail.installed =>
            {
                Some(tr("Not installed"))
            }
            PcLine::Online | PcLine::Running(_) | PcLine::Away(_) => None,
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
        game: &rubylight_native_api::library::Game,
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
            let working = self.playing_soon(host.id, game.app_id);
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
            let go = if state.resumes { en("Resume") } else { en("Play") };
            if let Some(anchor) = menu_request_going(ui, &r, None, go) {
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
            let rect = flow.place(capsule_width(ui, tr("Try again")), CAPSULE_H);
            self.list_retry(ui, rect, "game-retry", !any_first, host.id, commands);
            any_first = true;
        }
        let star = console::favourite_of(source, game);
        let starred = console::is_starred(source, game, &self.favourites);
        // The button says what it does, in the game menu's words (and Android's game page).
        let words = if starred {
            tr("Remove from favourites")
        } else {
            tr("Add to favourites")
        };
        let rect = flow.place(capsule_width(ui, words), CAPSULE_H);
        let glyph = if starred { Icon::StarOn } else { Icon::Star };
        let r = control_capsule(
            ui,
            rect,
            Id::unique("game-star"),
            glyph,
            words,
            words,
            star.is_some(),
        );
        // With Play off and nothing to end, the star takes the focus, so the D-pad has somewhere to start.
        self.focus_item(
            &r,
            "favourite",
            !any_first && state.detail.end == EndAction::None,
        );
        if r.clicked()
            && let Some(star) = star {
                commands.push(Command::ToggleFavourite(star));
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
}

mod rows;

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
