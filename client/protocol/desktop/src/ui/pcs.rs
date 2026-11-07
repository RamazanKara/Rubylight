//! The PCs page: a large card per PC, with a small screen on a stand, one line of state and one
//! action. The screen, the word and the action all say the same thing.

use super::design_tokens as t;
use super::*;
use super::{glass, spring};
use egui::Shape;

const PC_CARD_H: f32 = 216.0;
const PC_CARD_PAD: f32 = 22.0;
const SCREEN_W: f32 = 132.0;
const ACTION_H: f32 = 48.0;
const HOVER_RISE: f32 = 2.0;
/// Where the PCs' cards start below the page's title.
const TITLE_GAP: f32 = 18.0;
/// Art on a PC's screen sits a little below its middle, where a cover's picture is.
const FACE_ART_Y: f32 = 0.58;
/// An asleep PC's dark screen, what the sky shows through it.
const FACE_OFF: Color32 = Color32::from_rgba_premultiplied(26, 27, 31, 56);
/// The canvas under a card that sits on the moon, so its words keep 4.5:1 on the amber.
const OVER_MOON: f32 = 0.8;
/// How far a card reaches onto the moon before that canvas is whole.
const MOON_CLEAR_OVER: f32 = 24.0;
const MORE_DOTS_R: f32 = 1.9;
const MORE_DOTS_APART: f32 = 6.0;

/// The one thing a PC's card offers. Selecting the card itself does what it always did
/// ([`host_clicked`]), so a quick resume or a look at an asleep PC's covers stays a click away.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PcAction {
    Resume(i32),
    Open,
    Wake,
    TryAgain,
    Pair,
}

pub(super) fn pc_action(host: &Host) -> PcAction {
    match (host.reach, host.paired()) {
        (Reach::Offline, _) if host.record.mac.is_some() => PcAction::Wake,
        (Reach::Offline, _) => PcAction::TryAgain,
        (_, false) => PcAction::Pair,
        (_, true) => running_entry(host).map_or(PcAction::Open, |app| PcAction::Resume(app.id)),
    }
}

impl PcAction {
    fn words(self) -> &'static str {
        match self {
            Self::Resume(_) => tr("Resume"),
            Self::Open => tr("Open"),
            Self::Wake => tr("Wake"),
            Self::TryAgain => tr("Try again"),
            Self::Pair => tr("Pair"),
        }
    }

    fn icon(self) -> Option<Icon> {
        match self {
            Self::Resume(_) => Some(Icon::Play),
            Self::Open => Some(Icon::Grid),
            Self::Wake => Some(Icon::Power),
            Self::TryAgain => Some(Icon::Refresh),
            Self::Pair => None,
        }
    }

    pub(super) fn command(self, id: u64) -> Command {
        match self {
            Self::Resume(app) => Command::Launch {
                host: id,
                app,
                resume: true,
            },
            Self::Open => Command::OpenApps(id),
            Self::Wake => Command::Wake(id),
            Self::TryAgain => Command::Refresh(id),
            Self::Pair => Command::Pair(id),
        }
    }
}

/// The line under a PC's name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum PcLine {
    Running(String),
    Online,
    Offline,
    NotPaired,
    Checking,
}

pub(super) fn pc_line_of(host: &Host) -> PcLine {
    match (host.reach, host.paired()) {
        (Reach::Offline, _) => PcLine::Offline,
        (Reach::Unknown, _) => PcLine::Checking,
        (Reach::Online, false) => PcLine::NotPaired,
        (Reach::Online, true) => match host.running_app() {
            Some(_) => PcLine::Running(at_sentence_start(&running_name(host))),
            None => PcLine::Online,
        },
    }
}

impl PcLine {
    fn words(&self) -> &str {
        match self {
            Self::Running(game) => game,
            Self::Online => tr("Online"),
            Self::Offline => tr("Offline"),
            Self::NotPaired => tr("Not paired"),
            Self::Checking => tr("Checking…"),
        }
    }
}

/// The part of art of size `art` a PC's screen shows: all it can, a little below the art's middle.
pub(super) fn face_uv(art: Vec2, face: Vec2) -> Rect {
    let uv = cover_uv(art, face);
    let spare = 1.0 - uv.height();
    uv.translate(vec2(0.0, spare * FACE_ART_Y - uv.top()))
}

/// Keeps a point inside a rounded rect: the nearest point of it, for a point outside.
fn into_rounded(rect: Rect, r: f32, p: Pos2) -> Pos2 {
    let inner = rect.shrink(r);
    let nearest = pos2(
        p.x.clamp(inner.left(), inner.right()),
        p.y.clamp(inner.top(), inner.bottom()),
    );
    let away = p - nearest;
    if away.length() <= r {
        p
    } else {
        nearest + away.normalized() * r
    }
}

/// How much a card covers the moon behind PCs, from none to enough that its words sit on it. A long
/// list or a narrow window brings them there, and scrolling moves them across it.
pub(super) fn over_moon(screen: Rect, card: Rect) -> f32 {
    let (center, r) = backdrop::moon_disc(screen, backdrop::Moon::Low);
    let nearest = pos2(
        center.x.clamp(card.left(), card.right()),
        center.y.clamp(card.top(), card.bottom()),
    );
    ((r - (center - nearest).length()) / MOON_CLEAR_OVER).clamp(0.0, 1.0)
}

/// A small screen on a stand, in the state of its PC. `face` is the screen's rect; the stand hangs
/// below it.
fn monitor(ui: &Ui, out: &mut Vec<Shape>, host: &Host, face: Rect, reveal_id: Id) {
    use nightjar_core::tile::{tile_pair, title_hash};
    let ctx = ui.ctx();
    let w = face.width();
    let r = (w * 0.075).max(5.0);
    let offline = host.reach == Reach::Offline;
    let flat = high_contrast();
    if !offline && !flat {
        glass::soft(
            out,
            ctx,
            face.translate(vec2(0.0, 6.0)),
            r,
            16.0,
            Color32::from_black_alpha(71),
        );
    }
    let art = host
        .running_app()
        .filter(|_| !offline && host.paired())
        .and_then(|app| host.art.get(&app))
        .and_then(|art| match art {
            Art::Ready(texture) => Some(texture),
            Art::Loading | Art::Missing => None,
        });
    let name = host.display_name();
    if offline {
        glass::solid(out, ctx, face, r, if flat { *SURFACE } else { FACE_OFF });
    } else {
        let (top, bottom) = tile_colors(tile_pair(title_hash(name)));
        let fill = glass::gradient(face, 165.0, &[(0.0, top), (1.0, bottom)]);
        glass::shaded(out, ctx, face, r, &[], &fill);
        // The disc of a no-art tile, cut by the screen's round edge. Under art it never shows.
        if art.is_none() {
            let radius = 0.35 * w;
            let center = pos2(
                face.left() + 0.42 * w + radius,
                face.top() + 0.2 * face.height() + radius,
            );
            let edge: Vec<Pos2> = (0..96)
                .map(|i| {
                    let a = i as f32 / 96.0 * std::f32::consts::TAU;
                    into_rounded(face, r, center + radius * vec2(a.cos(), a.sin()))
                })
                .collect();
            out.push(Shape::convex_polygon(
                edge,
                Color32::WHITE.gamma_multiply(t::TILE_DISC_ALPHA),
                Stroke::NONE,
            ));
        }
    }
    if let Some(texture) = art {
        let seconds = if ui.style().animation_time > 0.0 {
            t::MOTION_COVER_IN_MS as f32 / 1000.0
        } else {
            0.0
        };
        let reveal = ctx.animate_bool_with_time(reveal_id, true, seconds);
        let tint = Color32::WHITE.gamma_multiply(reveal);
        glass::image(
            out,
            ctx,
            face,
            r,
            texture.id(),
            face_uv(texture.size_vec2(), face.size()),
            tint,
        );
    } else {
        ctx.animate_bool_with_time(reveal_id, false, 0.0);
    }
    let glyph = if offline {
        Some((Icon::Moon, *ON_SURFACE_VARIANT))
    } else if !host.paired() {
        Some((Icon::Lock, Color32::WHITE))
    } else {
        None
    };
    if let Some((icon, ink)) = glyph {
        out.extend(icons::shapes(
            icon,
            Rect::from_center_size(face.center(), Vec2::splat((0.3 * w).round())),
            ink,
        ));
    }
    if flat {
        glass::rim(out, ctx, face, r, 1.5, &|_| t::HC_COLOR_RIM);
    } else {
        let rim = if offline {
            t::TINT_CONTROL_RIM_BOTTOM
        } else {
            Color32::WHITE.gamma_multiply(0.2)
        };
        glass::rim(out, ctx, face, r, 1.0, &|_| rim);
    }
    // The stand.
    let neck = Rect::from_min_size(
        pos2(face.center().x - 0.05 * w, face.bottom()),
        vec2(0.1 * w, 0.07 * w),
    );
    out.push(Shape::rect_filled(
        neck,
        CornerRadius::ZERO,
        t::TINT_TRACK_FILL,
    ));
    let base = Rect::from_min_size(
        pos2(face.center().x - 0.15 * w, neck.bottom()),
        vec2(0.3 * w, (0.028 * w).max(2.0)),
    );
    out.push(Shape::rect_filled(
        base,
        CornerRadius::same(2),
        ON_SURFACE_VARIANT.gamma_multiply(0.5),
    ));
}

/// A word and its icon in a capsule's middle.
fn action_words(
    ui: &Ui,
    out: &mut Vec<Shape>,
    rect: Rect,
    icon: Option<Icon>,
    words: &str,
    ink: Color32,
) {
    const ICON: f32 = 18.0;
    const GAP: f32 = 8.0;
    let room = rect.width() - 2.0 * space::M - icon.map_or(0.0, |_| ICON + GAP);
    let mut job = egui::text::LayoutJob::simple_singleline(
        words.to_owned(),
        semibold(t::TYPE_BODY_SIZE),
        ink,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let galley = ui.painter().layout_job(job);
    let total = icon.map_or(0.0, |_| ICON + GAP) + galley.size().x;
    let mut x = rect.center().x - total / 2.0;
    if let Some(icon) = icon {
        out.extend(icons::shapes(
            icon,
            Rect::from_min_size(pos2(x, rect.center().y - ICON / 2.0), Vec2::splat(ICON)),
            ink,
        ));
        x += ICON + GAP;
    }
    out.push(Shape::galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    ));
}

/// How wide a PC's action is: Resume is solid and roomier, Pair has no icon.
pub(super) fn action_width(ui: &Ui, action: PcAction, most: f32) -> f32 {
    let words = ui
        .painter()
        .layout_no_wrap(
            action.words().to_owned(),
            semibold(t::TYPE_BODY_SIZE),
            Color32::WHITE,
        )
        .size()
        .x;
    let wanted = match action {
        PcAction::Resume(_) => 24.0 + 18.0 + 8.0 + words + 24.0,
        PcAction::Pair => 20.0 + words + 20.0,
        _ => 20.0 + 18.0 + 8.0 + words + 20.0,
    };
    wanted.min(most)
}

/// A PC's action. Its halo paints after the page so the next card never cuts it.
pub(super) fn action_button(ui: &mut Ui, rect: Rect, id: Id, action: PcAction, said: &str) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), said));
    let ctx = ui.ctx().clone();
    let feel = glass::feel(&ctx, &response, t::FOCUS_SCALE_CONTROL, 1.0);
    let shown = glass::scaled_rect(rect, feel.scale);
    let r = shown.height() / 2.0;
    let solid = matches!(action, PcAction::Resume(_));
    let mut out = Vec::new();
    if solid {
        glass::play(&mut out, &ctx, shown, r);
    } else {
        glass::control(&mut out, &ctx, shown, r);
    }
    lift_shapes(&mut out, &ctx, shown, r, feel.hovered, feel.pressed);
    let ink = match action {
        PcAction::Resume(_) => *ON_MOON,
        // The one PC action in the moon's colour, on a tint: never a second moon button.
        PcAction::Pair if !high_contrast() => t::COLOR_MOON_INK,
        PcAction::Pair => *MOON,
        _ => *ON_SURFACE,
    };
    action_words(ui, &mut out, shown, action.icon(), action.words(), ink);
    ui.painter().extend(out);
    if feel.focused {
        let mut ring = Vec::new();
        glass::focus(&mut ring, &ctx, shown, r, solid);
        glass::defer_focused(ui, ring);
    }
    response
}

/// The round More at a card's end.
fn more_button(ui: &mut Ui, rect: Rect, id: Id, said: &str) -> Response {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_text(tr("Options"));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), said));
    let ctx = ui.ctx().clone();
    let feel = glass::feel(&ctx, &response, t::FOCUS_SCALE_CONTROL, 1.0);
    let shown = glass::scaled_rect(rect, feel.scale);
    let r = shown.height() / 2.0;
    let mut out = Vec::new();
    glass::control(&mut out, &ctx, shown, r);
    lift_shapes(&mut out, &ctx, shown, r, feel.hovered, feel.pressed);
    for step in [-1.0, 0.0, 1.0] {
        out.push(Shape::circle_filled(
            shown.center() + vec2(step * MORE_DOTS_APART, 0.0),
            MORE_DOTS_R,
            *ON_SURFACE,
        ));
    }
    ui.painter().extend(out);
    if feel.focused {
        let mut ring = Vec::new();
        glass::focus(&mut ring, &ctx, shown, r, false);
        glass::defer_focused(ui, ring);
    }
    response
}

/// The dot, or the lock of a PC not paired yet, and the line beside it. True when the line was cut.
fn status_line(ui: &Ui, out: &mut Vec<Shape>, at: Pos2, room: f32, line: &PcLine) -> bool {
    const FONT: f32 = 15.0;
    let ctx = ui.ctx();
    let ink: Color32 = match line {
        PcLine::Running(_) | PcLine::Online => *ON_SURFACE,
        PcLine::Offline | PcLine::NotPaired | PcLine::Checking => *ON_SURFACE_VARIANT,
    };
    let lead = match line {
        PcLine::NotPaired => {
            out.extend(icons::shapes(
                Icon::Lock,
                Rect::from_center_size(pos2(at.x + 7.5, at.y), Vec2::splat(15.0)),
                ink,
            ));
            15.0
        }
        PcLine::Running(_) | PcLine::Online => {
            let dot = pos2(at.x + 4.5, at.y);
            if !high_contrast() {
                glass::soft(
                    out,
                    ctx,
                    Rect::from_center_size(dot, Vec2::splat(9.0)),
                    4.5,
                    8.0,
                    t::COLOR_OK,
                );
            }
            out.push(Shape::circle_filled(dot, 4.5, *OK));
            9.0
        }
        PcLine::Offline | PcLine::Checking => {
            out.push(Shape::circle_filled(
                pos2(at.x + 4.5, at.y),
                4.5,
                t::COLOR_OFFLINE,
            ));
            9.0
        }
    };
    let mut job =
        egui::text::LayoutJob::simple_singleline(line.words().to_owned(), regular(FONT), ink);
    job.wrap = egui::text::TextWrapping::truncate_at_width(room - lead - 8.0);
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    out.push(Shape::galley(
        pos2(at.x + lead + 8.0, at.y - galley.size().y / 2.0),
        galley,
        ink,
    ));
    cut
}

impl Launcher {
    pub(super) fn hosts(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        commands: &mut Vec<Command>,
        logo: egui::TextureId,
    ) {
        ui.horizontal(|ui| {
            ui.set_min_height(44.0);
            ui.label(
                RichText::new(tr("PCs"))
                    .font(semibold(text::LARGE_TITLE))
                    .color(ON_SURFACE),
            );
        });
        ui.add_space(TITLE_GAP - ui.spacing().item_spacing.y);
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
                    self.add_host_dialog();
                }
                how_to_connect(ui, commands);
            });
            return;
        }
        let (columns, width) = card_grid(ui.available_width(), CARD_GAP);
        for (row, chunk) in view.hosts.list.chunks(columns).enumerate() {
            let (band, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), PC_CARD_H), Sense::hover());
            for (i, host) in chunk.iter().enumerate() {
                let slot = Rect::from_min_size(
                    band.min + vec2(i as f32 * (width + CARD_GAP), 0.0),
                    vec2(width, PC_CARD_H),
                );
                self.pc_card(ui, view, host, slot, row == 0 && i == 0, commands);
            }
            ui.add_space(CARD_GAP - ui.spacing().item_spacing.y);
        }
        // A short list may not be all of them: say the search goes on and where help is.
        if view.hosts.list.len() <= columns {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                ui.label(
                    RichText::new(tr("Looking for other PCs on this network"))
                        .color(ON_SURFACE_VARIANT),
                );
                let help = how_to_connect(ui, commands);
                self.focus_item(&help, "how-to-connect", false);
            });
        }
        ui.add_space(space::L);
    }

    fn pc_card(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        host: &Host,
        slot: Rect,
        first: bool,
        commands: &mut Vec<Command>,
    ) {
        let id = Id::unique(("pc-card", host.id));
        let line = pc_line_of(host);
        let action = pc_action(host);
        let name = host.display_name();
        let ctx = ui.ctx().clone();
        // Clicking the card itself does what it always did; only its action and More take focus.
        let card = ui.interact(slot, id, Sense::CLICK);
        let said = format!("{name}, {}", line.words());
        card.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said.clone()));
        // Over its action the card is still pointed at.
        let hovered = card.contains_pointer() && ui.is_enabled();
        let rise = if spring::motion(&ctx) {
            spring::animate(
                &ctx,
                id.with("rise"),
                if hovered { HOVER_RISE } else { 0.0 },
                spring::FOCUS,
            )
        } else {
            0.0
        };
        let rect = slot.translate(vec2(0.0, -rise));
        let r = t::RADIUS_PANEL;
        let inner = rect.shrink(PC_CARD_PAD);
        let face = Rect::from_min_size(inner.min, vec2(SCREEN_W, (0.62 * SCREEN_W).round()));
        let stand = 0.07 * SCREEN_W + (0.028 * SCREEN_W).max(2.0);
        let words_left = face.right() + 18.0;
        let words_room = inner.right() - words_left;
        let name_center = face.top() + (face.height() + stand) / 2.0 - 25.0 + 13.0;
        let line_center = name_center + 13.0 + 4.0 + 10.0;
        let mut name_cut = false;
        let mut line_cut = false;
        if ui.is_rect_visible(slot.expand(40.0)) {
            let mut out = Vec::new();
            if rise > 0.01 && !high_contrast() {
                glass::soft(
                    &mut out,
                    &ctx,
                    rect.translate(vec2(0.0, 14.0)),
                    r,
                    34.0,
                    Color32::from_black_alpha(102).gamma_multiply(rise / HOVER_RISE),
                );
            }
            let moon = over_moon(ctx.content_rect(), rect);
            if moon > 0.0 && !high_contrast() {
                glass::solid(
                    &mut out,
                    &ctx,
                    rect,
                    r,
                    t::COLOR_CANVAS.gamma_multiply(OVER_MOON * moon),
                );
            }
            glass::group(&mut out, &ctx, rect, r, group_fill());
            lift_shapes(
                &mut out,
                &ctx,
                rect,
                r,
                hovered,
                card.is_pointer_button_down_on(),
            );
            monitor(ui, &mut out, host, face, id.with("art-in"));
            let mut job = egui::text::LayoutJob::simple_singleline(
                name.to_owned(),
                semibold(t::TYPE_SECTION_SIZE),
                *ON_SURFACE,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(words_room);
            let galley = ui.painter().layout_job(job);
            name_cut = galley.elided;
            out.push(Shape::galley(
                pos2(words_left, name_center - galley.size().y / 2.0),
                galley,
                *ON_SURFACE,
            ));
            line_cut = status_line(
                ui,
                &mut out,
                pos2(words_left, line_center),
                words_room,
                &line,
            );
            ui.painter().extend(out);
        }
        if name_cut || line_cut {
            let _ = card.clone().on_hover_text(said.clone());
        }
        // A running PC's art comes as any cover does; its list comes first when nothing asked yet.
        if host.reach == Reach::Online && host.paired() && host.running_app().is_some() {
            match running_entry(host) {
                Some(app) => request_cover(
                    ui,
                    view,
                    host.id,
                    app.id as u32,
                    &app.art_version,
                    &card,
                    commands,
                ),
                None if matches!(host.apps, Apps::NotLoaded) => {
                    commands.push(Command::LoadApps(host.id))
                }
                None => {}
            }
        }
        let foot = pos2(inner.left(), inner.bottom() - ACTION_H);
        let more_rect = Rect::from_min_size(
            pos2(inner.right() - ACTION_H, foot.y),
            Vec2::splat(ACTION_H),
        );
        let most = (more_rect.left() - 10.0 - foot.x).max(ACTION_H);
        let action_rect = Rect::from_min_size(foot, vec2(action_width(ui, action, most), ACTION_H));
        let action_id = Id::unique(("pc-action", host.id));
        let action_response = action_button(
            ui,
            action_rect,
            action_id,
            action,
            &format!("{}, {name}, {}", action.words(), line.words()),
        );
        let more = more_button(
            ui,
            more_rect,
            action_id.with("options"),
            &format!("{}, {name}", tr("Options")),
        );
        // Focus on either brings the whole card into view, clear of the chrome.
        let key = format!("host:{}", host.id);
        let mut whole = action_response.clone();
        whole.rect = slot;
        self.focus_item(&whole, &key, first);
        let mut whole_more = more.clone();
        whole_more.rect = slot;
        self.focus_item(&whole_more, &format!("options:{}", host.id), false);
        if action_response.clicked() {
            commands.push(action.command(host.id));
        } else if card.clicked() {
            host_clicked(host, self.settings.quick_resume, commands);
        }
        let anchor = menu_request(ui, &card, Some(&more))
            .or_else(|| menu_request(ui, &action_response, None));
        if let Some(anchor) = anchor {
            let beside_card = if matches!(anchor, Anchor::Beside(_)) {
                Anchor::Beside(slot)
            } else {
                anchor
            };
            self.open_menu(Dialog::HostOptions { host: host.id }, beside_card, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_shows_art_a_little_below_its_middle_and_never_outside_it() {
        // A 3:4 cover on a screen of 1:0.62: the height is what gets cut.
        let uv = face_uv(vec2(300.0, 400.0), vec2(132.0, 82.0));
        assert!(
            uv.left().abs() < 1e-3 && (uv.right() - 1.0).abs() < 1e-3,
            "{uv:?}"
        );
        let spare = 1.0 - uv.height();
        assert!((uv.top() - spare * FACE_ART_Y).abs() < 1e-3, "{uv:?}");
        assert!(uv.bottom() <= 1.0 + 1e-3);
        // Wide art fills the height whatever the place: nothing to shift.
        let wide = face_uv(vec2(1600.0, 400.0), vec2(132.0, 82.0));
        assert!(
            wide.top().abs() < 1e-3 && (wide.bottom() - 1.0).abs() < 1e-3,
            "{wide:?}"
        );
    }

    #[test]
    fn a_card_on_the_moon_gets_its_canvas_and_one_beside_it_none() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
        // The design's third column, first row: clear of the moon at (1040, 520), r 118.
        assert_eq!(
            over_moon(
                screen,
                Rect::from_min_size(pos2(848.0, 150.0), vec2(384.0, 216.0))
            ),
            0.0
        );
        // A sixth PC sits right on it.
        assert_eq!(
            over_moon(
                screen,
                Rect::from_min_size(pos2(848.0, 382.0), vec2(384.0, 216.0))
            ),
            1.0
        );
        // The edge eases in.
        let edge = over_moon(
            screen,
            Rect::from_min_size(
                pos2(1040.0 - 118.0 - 384.0 + 12.0, 412.0),
                vec2(384.0, 216.0),
            ),
        );
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
    }

    #[test]
    fn the_disc_stays_inside_the_round_screen() {
        let face = Rect::from_min_size(pos2(10.0, 20.0), vec2(132.0, 82.0));
        let r = 10.0;
        assert_eq!(into_rounded(face, r, face.center()), face.center());
        // Far past the corner it lands on the corner's arc.
        let corner = into_rounded(face, r, face.right_bottom() + vec2(50.0, 50.0));
        let arc_center = face.right_bottom() - Vec2::splat(r);
        assert!(
            ((corner - arc_center).length() - r).abs() < 1e-3,
            "{corner:?}"
        );
        // Past the right edge it lands on the edge.
        let side = into_rounded(face, r, pos2(face.right() + 30.0, face.center().y));
        assert_eq!(side, pos2(face.right(), face.center().y));
    }
}
