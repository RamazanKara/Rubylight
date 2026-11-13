//! The in-stream menus in the moonlit glass: a panel at the right of the stream, its fill and rim
//! painted with no backdrop, tiles and rows as tinted controls, one round close, and the ways out
//! as capsules. Everything here is drawn only while a menu is open; a closed menu costs the stream
//! nothing: no blur, no layer, no texture bound.
//!
//! The states follow the launcher's: hover and press lift a control, and keyboard or controller
//! focus is the one amber ring around it, so nothing moves.

use egui::{vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::config::GamepadQuitCombo;
use crate::glyphs::{self, icon, Icon, Piece, Style};
use crate::shortcuts::{self, Action, Chord, Scope};
use crate::strings::tr;
use crate::theme::{high_contrast, radius, regular, semibold, shade, space, text, ERROR, MOON, ON_MOON, ON_MOON_CONTAINER, ON_SURFACE, ON_SURFACE_VARIANT};
use crate::ui::{design_tokens as t, focus_ring, glass, lift_shapes, name_for_readers, round_button, toggle, Tone};

/// The menus' width with their margin.
pub const WIDTH: f32 = 452.0;
/// The panel's margin.
const MARGIN: f32 = 20.0;
/// The panel's distance from the window's edges.
const OUTER: f32 = 24.0;
/// The panel's corner.
const CORNER: f32 = 32.0;
/// How far the focus ring reaches outside a control: a page keeps this much room inside its clip.
const HALO: f32 = 10.0;
/// How far the ring's glow reaches here: it fades out inside the panel's margin, and so inside a
/// page's clip and the panel's round corners. The launcher's wider glow would be cut in a line.
const HALO_REACH: f32 = MARGIN - t::FOCUS_GAP - t::FOCUS_RING_WIDTH;
/// A row's inner padding left and right, so its icon, label and control sit inside its hover fill.
pub const PAD: f32 = 12.0;
/// An action row's height.
const ROW: f32 = 44.0;
/// The icons' size.
const ICON: f32 = 18.0;
/// Where a row's label starts: after its padding, its icon and a gap.
const LABEL_X: f32 = PAD + ICON + PAD;

/// A menu: the panel at the right of the stream, as tall as what it holds, without egui's title bar
/// (the menu draws its own title). The surface goes under `add`, once its size is known.
pub fn show(ctx: &egui::Context, title: &str, id: &str, add: impl FnOnce(&mut Ui)) {
    let inside = (ctx.content_rect().width() - 2.0 * OUTER).min(WIDTH) - 2.0 * MARGIN;
    egui::Window::new(title)
        .id(egui::Id::unique(id))
        .title_bar(false)
        .anchor(Align2::RIGHT_TOP, vec2(-OUTER, OUTER))
        .collapsible(false)
        .resizable(false)
        .min_width(inside)
        .max_width(inside)
        .frame(Frame::new().inner_margin(Margin::same(MARGIN as i8)))
        .show(ctx, |ui| {
            let place = ui.painter().add(Shape::Noop);
            ui.set_width(inside);
            let ctx = ui.ctx().clone();
            glass::with_halo_reach(&ctx, HALO_REACH, || add(ui));
            let mut shapes = Vec::new();
            glass::menu_surface(&mut shapes, ui.ctx(), ui.min_rect().expand(MARGIN), CORNER);
            // The window's clip stops at its edge, and the shadow falls past that.
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(ui.ctx().content_rect());
            painter.set(place, Shape::Vec(shapes));
        });
}

/// A menu's content: as tall as the window allows less a margin, scrolling beyond that. egui
/// lays a new window out in at most 400 points the first time, which cut the menu short.
pub fn body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    body_above(ui, 0.0, add)
}

/// [`body`] for a page that keeps something in view above and under the part that scrolls: what
/// the window drew above it already, and `reserve` points under it (see [`pinned`]).
pub fn body_above<R>(ui: &mut Ui, reserve: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let above = ui.cursor().top() - ui.max_rect().top();
    let room = (ui.ctx().content_rect().height() - 2.0 * (OUTER + MARGIN) - above - reserve).max(TILE);
    // A child as tall as the room rather than Ui::set_max_height, which puts the cursor back at the
    // window's top, under what it drew already.
    ui.allocate_ui(egui::vec2(ui.available_width(), room), |ui| {
        egui::ScrollArea::vertical().max_height(room).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // The scroll area's own edges would cut the focus ring and its glow. Past them on both ends
            // lies only the empty gap to the head and the footer.
            ui.set_clip_rect(ui.clip_rect().expand(HALO));
            ui.add_space(HALO);
            let inner = add(ui);
            ui.add_space(HALO);
            inner
        })
        .inner
    })
    .inner
}

/// The room a [`pinned`] part took last frame, for [`body_above`] to leave under the scrolling
/// part; before the first frame, about Return to stream and one row of exits.
pub fn pinned_room(ctx: &egui::Context, id: &str) -> f32 {
    ctx.data(|d| d.get_temp(egui::Id::unique(("pinned", id)))).unwrap_or(space::L + EXIT)
}

/// A part that stays in view under the scrolling part (the stream menu's ways out); its height is
/// kept for [`pinned_room`].
pub fn pinned<R>(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let response = ui.scope(add);
    let height = response.response.rect.height();
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(("pinned", id)), height));
    response.inner
}

/// The menu's title and, under it, what streams from where.
pub fn header(ui: &mut Ui, title: &str, line: &str) {
    ui.add(egui::Label::new(RichText::new(title).font(semibold(text::SECTION)).color(ON_SURFACE)).truncate());
    ui.add_space(2.0);
    ui.add(egui::Label::new(RichText::new(line).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT)).truncate());
    ui.add_space(space::M);
}

/// The grid's head: [`header`] with the round close beside it. Says whether the close was used.
pub fn head(ui: &mut Ui, title: &str, line: &str) -> bool {
    const CLOSE: f32 = 44.0;
    let top = ui.cursor().min;
    let close = Rect::from_min_size(Pos2::new(top.x + ui.available_width() - CLOSE, top.y), Vec2::splat(CLOSE));
    let closed = round_button(ui, close, crate::ui::Icon::Close, tr("Return to stream")).clicked();
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width() - CLOSE - space::S);
        ui.set_min_height(CLOSE);
        header(ui, title, line);
    });
    closed
}

/// A group's caption: what the rows under it are about.
pub fn caption(ui: &mut Ui, label: &str) {
    ui.add_space(space::M);
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.label(RichText::new(label).font(semibold(text::LABEL)).color(ON_SURFACE));
    });
    ui.add_space(space::XS);
}

/// Rows that belong together, on a tint with its rim.
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let place = ui.painter().add(Shape::Noop);
    let shown = Frame::new().inner_margin(Margin::same(4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let mut shapes = Vec::new();
    glass::group(&mut shapes, ui.ctx(), shown.response.rect, GROUP_CORNER, shade::TINT_GROUP_FILL.get());
    ui.painter().set(place, Shape::Vec(shapes));
    shown.inner
}

const GROUP_CORNER: f32 = 20.0;

/// A row's fill for its state: none at rest, a tint on hover, a stronger one while pressed.
fn fill(ui: &Ui, response: &Response) -> Color32 {
    if !ui.is_enabled() {
        Color32::TRANSPARENT
    } else if response.is_pointer_button_down_on() {
        shade::TINT_SEG_ON_FILL.get()
    } else if response.hovered() {
        shade::TINT_CONTROL_FILL.get()
    } else {
        Color32::TRANSPARENT
    }
}

/// The focus ring: the launcher's one amber ring around `rect`.
pub fn ring(ui: &Ui, rect: Rect, response: &Response, corner: u8) {
    focus_ring(ui, rect, response, corner);
}

/// What a row shows at its right edge.
enum Trail<'a> {
    None,
    /// A shortcut, as quiet text the way desktop menus show them.
    Keys(&'a str),
    /// A fold's chevron, down when open.
    Chevron(bool),
}

/// An action row: icon, label, and what it trails, across the menu's width. The label wraps if
/// the row is too narrow for it (German), and the row grows.
fn action_row(ui: &mut Ui, glyph: Icon, label: &str, trail: Trail<'_>, ink: Color32) -> Response {
    let width = ui.available_width();
    let enabled = ui.is_enabled();
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    let keys = match trail {
        Trail::Keys(keys) => Some(ui.painter().layout_no_wrap(keys.to_owned(), regular(text::CAPTION), dim(*ON_SURFACE_VARIANT))),
        _ => None,
    };
    let trail_w = match (&keys, &trail) {
        (Some(g), _) => g.size().x + space::M,
        (None, Trail::Chevron(_)) => ICON + space::S,
        _ => 0.0,
    };
    let galley = ui.painter().layout(label.to_owned(), regular(text::BODY), dim(ink), (width - LABEL_X - PAD - trail_w).max(80.0));
    let height = (galley.size().y + 2.0 * 10.0).max(ROW);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(radius::CONTROL), fill(ui, &response));
    let icon_rect = Rect::from_center_size(Pos2::new(rect.left() + PAD + ICON / 2.0, rect.center().y), vec2(ICON, ICON));
    let icon_ink = if ink == *ON_SURFACE { *ON_SURFACE_VARIANT } else { ink };
    icon(painter, icon_rect, glyph, dim(icon_ink));
    painter.galley(Pos2::new(rect.left() + LABEL_X, rect.center().y - galley.size().y / 2.0), galley, dim(ink));
    match trail {
        Trail::Keys(_) => {
            if let Some(g) = keys {
                painter.galley(Pos2::new(rect.right() - PAD - g.size().x, rect.center().y - g.size().y / 2.0), g, dim(*ON_SURFACE_VARIANT));
            }
        }
        Trail::Chevron(open) => {
            let c = Pos2::new(rect.right() - PAD - ICON / 2.0, rect.center().y);
            let (a, b) = if open { (vec2(-4.5, -2.0), vec2(4.5, -2.0)) } else { (vec2(-2.0, -4.5), vec2(-2.0, 4.5)) };
            let tip = if open { vec2(0.0, 2.5) } else { vec2(2.5, 0.0) };
            painter.add(egui::Shape::line(vec![c + a, c + tip, c + b], Stroke::new(1.6, dim(*ON_SURFACE_VARIANT))));
        }
        Trail::None => {}
    }
    ring(ui, rect, &response, radius::CONTROL);
    response
}

/// An action: its icon, its label and, right, the keys that do it in a stream.
pub fn action(ui: &mut Ui, glyph: Icon, label: &str, shortcut: Option<Action>) -> Response {
    let keys = shortcut.and_then(|a| shortcuts::chord_for(Scope::Stream, a)).map(Chord::text);
    action_keys(ui, glyph, label, keys.as_deref())
}

/// An action with the keys it trails given as text (Esc for a page's way back).
pub fn action_keys(ui: &mut Ui, glyph: Icon, label: &str, keys: Option<&str>) -> Response {
    let response = action_row(ui, glyph, label, keys.map_or(Trail::None, Trail::Keys), *ON_SURFACE);
    let spoken = keys.map(|k| format!("{label}, {k}")).unwrap_or_else(|| label.to_owned());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &spoken));
    response
}

/// The way out of the stream, in the error colour: set apart under a line by its caller.
pub fn danger(ui: &mut Ui, glyph: Icon, label: &str, shortcut: Option<Action>) -> Response {
    let keys = shortcut.and_then(|a| shortcuts::chord_for(Scope::Stream, a)).map(Chord::text);
    let response = action_row(ui, glyph, label, keys.as_deref().map_or(Trail::None, Trail::Keys), *ERROR);
    let spoken = keys.map(|k| format!("{label}, {k}")).unwrap_or_else(|| label.to_owned());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &spoken));
    response
}

/// One of the ways out side by side under the grid, `width` wide: Disconnect a quiet capsule, ending
/// something on the PC a red one. The keys that do it show on hover and are read out with the label.
pub fn exit_button(ui: &mut Ui, width: f32, glyph: Icon, label: &str, keys: Option<&str>, tone: Tone) -> Response {
    let enabled = ui.is_enabled();
    let (hovered, pressed) = (false, false);
    let base_ink = tone.colors(hovered, pressed).1;
    let ink = if enabled { base_ink } else { base_ink.gamma_multiply(0.4) };
    let mut job = egui::text::LayoutJob::simple(label.to_owned(), semibold(text::LABEL), ink, (width - 2.0 * PAD - ICON - space::S).max(space::L));
    job.wrap.max_rows = 2;
    let galley = ui.painter().layout_job(job);
    let height = (galley.size().y + 2.0 * 10.0).max(EXIT);
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let r = (height / 2.0).min(EXIT / 2.0);
    let mut shapes = Vec::new();
    if matches!(tone, Tone::Danger) && !high_contrast() {
        glass::solid(&mut shapes, ui.ctx(), rect, r, tone.colors(false, false).0);
    } else {
        glass::control(&mut shapes, ui.ctx(), rect, r);
    }
    lift_shapes(&mut shapes, ui.ctx(), rect, r, enabled && response.hovered(), enabled && response.is_pointer_button_down_on());
    ui.painter().extend(shapes);
    let left = rect.center().x - (ICON + space::S + galley.size().x) / 2.0;
    icon(ui.painter(), Rect::from_center_size(Pos2::new(left + ICON / 2.0, rect.center().y), vec2(ICON, ICON)), glyph, ink);
    ui.painter().galley(Pos2::new(left + ICON + space::S, rect.center().y - galley.size().y / 2.0), galley, ink);
    ring(ui, rect, &response, r as u8);
    if let Some(keys) = keys {
        response = response.on_hover_text(keys);
    }
    let spoken = keys.map(|k| format!("{label}, {k}")).unwrap_or_else(|| label.to_owned());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &spoken));
    response
}

/// A way out's height.
const EXIT: f32 = 48.0;

/// A fold: a row that opens what is under it (a slider, a report, a list), its body lined up with
/// the label. Returns what the body returned, while open.
pub fn fold<R>(ui: &mut Ui, id: &str, glyph: Icon, label: &str, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let id = ui.make_persistent_id(id);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let open = state.is_open();
    let response = action_row(ui, glyph, label, Trail::Chevron(open), *ON_SURFACE);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, ui.is_enabled(), open, label));
    if response.clicked() {
        state.toggle(ui);
    }
    state
        .show_body_unindented(ui, |ui| {
            Frame::new().inner_margin(Margin { left: LABEL_X as i8, right: PAD as i8, top: 4, bottom: 8 }).show(ui, add).inner
        })
        .map(|r| r.inner)
}

/// A row: its icon and label left, its control (a list, a slider) right.
pub fn row<R>(ui: &mut Ui, glyph: Icon, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let enabled = ui.is_enabled();
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    ui.horizontal(|ui| {
        ui.set_min_height(ROW + 4.0);
        ui.add_space(PAD);
        let (icon_rect, _) = ui.allocate_exact_size(vec2(ICON, ICON), Sense::hover());
        icon(ui.painter(), icon_rect, glyph, dim(*ON_SURFACE_VARIANT));
        ui.add_space(PAD - ui.spacing().item_spacing.x);
        ui.label(RichText::new(label).font(regular(text::BODY)).color(dim(*ON_SURFACE)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(PAD);
            control(ui)
        })
        .inner
    })
    .inner
}

/// A switch row, as the launcher's settings have them; the response says when it changed.
pub fn switch(ui: &mut Ui, glyph: Icon, label: &str, on: &mut bool) -> Response {
    row(ui, glyph, label, |ui| {
        let response = toggle(ui, on);
        name_for_readers(ui, &response, label, "");
        response
    })
}

/// The menu's one main action, across its width in amber (Return to stream), with its key right.
/// Disabled (Resume while End or Close game runs), it loses its amber for a tinted capsule's look
/// and takes no hover.
pub fn primary(ui: &mut Ui, label: &str, keys: Option<&str>) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let enabled = ui.is_enabled();
    let r = rect.height() / 2.0;
    let mut shapes = Vec::new();
    if enabled {
        glass::play(&mut shapes, ui.ctx(), rect, r);
        lift_shapes(&mut shapes, ui.ctx(), rect, r, response.hovered(), response.is_pointer_button_down_on());
    } else {
        glass::control(&mut shapes, ui.ctx(), rect, r);
    }
    ui.painter().extend(shapes);
    let ink = if enabled { *ON_MOON } else { ON_SURFACE.gamma_multiply(0.4) };
    let painter = ui.painter();
    let left = Rect::from_center_size(Pos2::new(rect.left() + PAD + 4.0 + ICON / 2.0, rect.center().y), vec2(ICON, ICON));
    icon(painter, left, Icon::Play, ink);
    let galley = painter.layout_no_wrap(label.to_owned(), semibold(text::BODY), ink);
    painter.galley(Pos2::new(rect.left() + 4.0 + LABEL_X, rect.center().y - galley.size().y / 2.0), galley, ink);
    if let Some(keys) = keys {
        let g = painter.layout_no_wrap(keys.to_owned(), semibold(text::CAPTION), ink);
        let cap = Rect::from_min_size(Pos2::new(rect.right() - PAD - g.size().x - 14.0, rect.center().y - 12.0), vec2(g.size().x + 14.0, 24.0));
        painter.rect_stroke(cap, CornerRadius::same(radius::SMALL), Stroke::new(1.0, ink.gamma_multiply(0.5)), StrokeKind::Inside);
        painter.galley(cap.center() - g.size() / 2.0, g, ink);
    }
    ring(ui, rect, &response, r as u8);
    let spoken = keys.map(|k| format!("{label}, {k}")).unwrap_or_else(|| label.to_owned());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &spoken));
    response
}

/// A tile's height in the stream menu's grid.
pub const TILE: f32 = 76.0;
/// The gap between tiles.
pub const TILE_GAP: f32 = 8.0;
/// A tile's corner.
const TILE_CORNER: f32 = 20.0;

/// A tile of the stream menu's grid: its icon over its label, a quieter line under that (a
/// shortcut's keys), and the controller button that runs it in the top left. On, the tile takes the
/// selected tint and its icon the moon.
pub fn tile(ui: &mut Ui, width: f32, glyph: Icon, label: &str, detail: Option<&str>, on: Option<bool>, hotkey: Option<&str>) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, TILE), Sense::click());
    let enabled = ui.is_enabled();
    let lit = on == Some(true);
    let mut shapes = Vec::new();
    if lit && high_contrast() {
        glass::solid(&mut shapes, ui.ctx(), rect, TILE_CORNER, t::SOLID_CONTROL_FILL);
        glass::rim(&mut shapes, ui.ctx(), rect, TILE_CORNER, 2.0, &|_| *MOON);
    } else if lit {
        glass::solid(&mut shapes, ui.ctx(), rect, TILE_CORNER, shade::TINT_SELECTED_FILL.get());
        let rim = glass::gradient(rect, 180.0, &[(0.0, shade::TINT_SELECTED_RIM_TOP.get()), (0.5, Color32::TRANSPARENT)]);
        glass::rim(&mut shapes, ui.ctx(), rect, TILE_CORNER, 1.0, &rim);
    } else {
        glass::control(&mut shapes, ui.ctx(), rect, TILE_CORNER);
    }
    lift_shapes(&mut shapes, ui.ctx(), rect, TILE_CORNER, enabled && response.hovered(), enabled && response.is_pointer_button_down_on());
    ui.painter().extend(shapes);
    let ink = match (enabled, lit) {
        (false, _) => ON_SURFACE.gamma_multiply(0.4),
        (true, true) if !high_contrast() => *ON_MOON_CONTAINER,
        _ => *ON_SURFACE,
    };
    let painter = ui.painter();
    let mut job = egui::text::LayoutJob::simple(label.to_owned(), regular(text::LABEL), ink, width - 2.0 * space::S);
    job.halign = Align::Center;
    job.wrap.max_rows = 2;
    let galley = painter.layout_job(job);
    let quiet = detail.map(|detail| {
        // Keys too wide for the tile close up (Ctrl+Shift+Esc under the German Task-Manager)
        // and then wrap, rather than end in an ellipsis.
        let room = width - 2.0 * space::S;
        let spaced = painter.layout_no_wrap(detail.to_owned(), regular(text::CAPTION), *ON_SURFACE_VARIANT).size().x <= room;
        let keys = if spaced { detail.to_owned() } else { detail.replace(" + ", "+") };
        let mut job = egui::text::LayoutJob::simple(keys, regular(text::CAPTION), *ON_SURFACE_VARIANT, room);
        job.halign = Align::Center;
        job.wrap.max_rows = 2;
        painter.layout_job(job)
    });
    // Icon, label and keys sit together in the middle of the tile, however many rows they take. A
    // tile too full for its icon (a long name over long keys) keeps its words.
    let words = galley.size().y + quiet.as_ref().map_or(0.0, |g| g.size().y + 1.0);
    let room_for_icon = 22.0 + 6.0 + words <= TILE - 2.0 * space::XS;
    let block = if room_for_icon { 22.0 + 6.0 + words } else { words };
    let top = rect.top() + ((TILE - block) / 2.0).max(space::XS);
    if room_for_icon {
        let icon_rect = Rect::from_min_size(Pos2::new(rect.center().x - 11.0, top), vec2(22.0, 22.0));
        icon(painter, icon_rect, glyph, if lit { *MOON } else { ink });
    }
    let label_top = if room_for_icon { top + 28.0 } else { top };
    painter.galley(Pos2::new(rect.center().x, label_top), galley.clone(), ink);
    if let Some(g) = quiet {
        painter.galley(Pos2::new(rect.center().x, label_top + galley.size().y + 1.0), g, *ON_SURFACE_VARIANT);
    }
    if let Some(keys) = hotkey {
        let g = painter.layout_no_wrap(keys.to_owned(), semibold(text::CAPTION), *ON_SURFACE_VARIANT);
        let cap = Rect::from_min_size(Pos2::new(rect.left() + 10.0, rect.top() + 8.0), vec2(g.size().x + 10.0, 16.0));
        painter.rect_stroke(cap, CornerRadius::same(radius::SMALL), Stroke::new(1.0, ON_SURFACE_VARIANT.gamma_multiply(0.6)), StrokeKind::Inside);
        painter.galley(cap.center() - g.size() / 2.0, g, *ON_SURFACE_VARIANT);
    }
    ring(ui, rect, &response, TILE_CORNER as u8);
    let mut spoken = label.to_owned();
    if let Some(detail) = detail {
        spoken = format!("{spoken}, {detail}");
    }
    if let Some(keys) = hotkey {
        spoken = format!("{spoken}, {keys}");
    }
    response.widget_info(|| match on {
        Some(on) => egui::WidgetInfo::selected(egui::Role::Switch, enabled, on, &spoken),
        None => egui::WidgetInfo::labeled(egui::Role::Button, enabled, &spoken),
    });
    response
}

/// A smaller action inside a fold or a group (Copy report, Apply): a filled button. A label wider
/// than the room (a PC command's name) ends in an ellipsis and shows whole on hover.
pub fn button(ui: &mut Ui, label: &str) -> Response {
    let enabled = ui.is_enabled();
    let ink = if enabled { *ON_SURFACE } else { ON_SURFACE.gamma_multiply(0.4) };
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), semibold(text::LABEL), ink);
    job.wrap = egui::text::TextWrapping::truncate_at_width((ui.available_width() - 2.0 * space::L).max(space::L));
    let galley = ui.painter().layout_job(job);
    let size = vec2((galley.size().x + 2.0 * space::L).min(ui.available_width()), 36.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if galley.elided {
        response = response.on_hover_text(label);
    }
    let r = rect.height() / 2.0;
    let mut shapes = Vec::new();
    glass::control(&mut shapes, ui.ctx(), rect, r);
    lift_shapes(&mut shapes, ui.ctx(), rect, r, enabled && response.hovered(), enabled && response.is_pointer_button_down_on());
    ui.painter().extend(shapes);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ink);
    ring(ui, rect, &response, r as u8);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response
}

/// Space and a thin line between groups.
pub fn divider(ui: &mut Ui) {
    ui.add_space(space::S);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, shade::TINT_SEPARATOR.get());
    ui.add_space(space::S);
}

/// A quiet line under a row: a hint or why something is off, lined up with the labels.
pub fn note(ui: &mut Ui, text_: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(LABEL_X);
        ui.label(RichText::new(text_).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
    });
}

/// Why something is off, in the error colour, lined up with the labels.
pub fn problem(ui: &mut Ui, text_: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(LABEL_X);
        ui.label(RichText::new(text_).font(regular(text::CAPTION)).color(*ERROR));
    });
}

/// Body text inside a fold.
pub fn text_block(ui: &mut Ui, text_: &str) {
    ui.label(RichText::new(text_).font(regular(text::LABEL)).color(ON_SURFACE));
}

/// One shortcut on the sheet: what it does left, its keys or buttons right, one focus stop a
/// screen reader reads whole.
fn shortcut(ui: &mut Ui, label: &str, pieces: &[Piece], spoken: &str) {
    let width = ui.available_width();
    let glyphs_w = glyphs::width(ui, pieces);
    let galley = ui.painter().layout(label.to_owned(), regular(text::BODY), *ON_SURFACE, (width - 2.0 * PAD - glyphs_w - space::M).max(80.0));
    let height = (galley.size().y + 16.0).max(ROW);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::focusable_noninteractive());
    ui.painter().galley(Pos2::new(rect.left() + PAD, rect.center().y - galley.size().y / 2.0), galley, *ON_SURFACE);
    glyphs::paint(ui, Pos2::new(rect.right() - PAD - glyphs_w, rect.center().y), pieces);
    ring(ui, rect, &response, radius::CONTROL);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{label}: {spoken}")));
}

/// The stream's shortcuts, keyboard and controller, from the same list the key handling reads.
/// `controller_mouse` lists holding Start for the mouse when the stream offers it, `back_opens`
/// Select for the controls as the Help page does. Returns true when the back row was chosen.
pub fn sheet(ui: &mut Ui, style: Style, quit: GamepadQuitCombo, back_opens: bool, controller_mouse: bool, from_menu: bool) -> bool {
    let back_label = if from_menu { tr("Back to stream menu") } else { tr("Return to stream") };
    let back = action_row(ui, if from_menu { Icon::Sliders } else { Icon::Play }, back_label, Trail::Keys(shortcuts::Key::Escape.cap().as_ref()), *ON_SURFACE);
    back.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, back_label));
    if ui.ctx().memory(|m| m.focused().is_none()) {
        back.request_focus();
    }
    caption(ui, tr("Keyboard"));
    let (chords, alone): (Vec<shortcuts::KeyBinding>, Vec<_>) = shortcuts::keys_in(Scope::Stream).into_iter().partition(|b| b.chords.iter().all(|c| c.is_nightjar()));
    // Said once above the chords that share Ctrl+Alt+Shift, while some do.
    if !chords.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(PAD);
            ui.label(RichText::new(tr("Hold")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
            let mut held = glyphs::chord(Chord::nightjar(shortcuts::Key::Tab), false);
            held.truncate(held.len() - 2);
            glyphs::show(ui, &held);
            ui.label(RichText::new(tr("and press:")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
        });
        ui.add_space(space::XS);
    }
    for binding in chords {
        let spoken = glyphs::spoken(&glyphs::chords(&binding.chords, false));
        shortcut(ui, tr(binding.action.label()), &glyphs::chords(&binding.chords, true), &spoken);
    }
    for binding in alone {
        let pieces = glyphs::chords(&binding.chords, false);
        shortcut(ui, tr(binding.action.label()), &pieces, &glyphs::spoken(&pieces));
    }
    caption(ui, tr("Controllers"));
    for binding in shortcuts::pad_in(Scope::Stream) {
        if binding.action == Action::ControllerMouse && !controller_mouse {
            continue;
        }
        let pieces = style.stream_press(binding, quit, back_opens);
        shortcut(ui, tr(binding.action.label()), &pieces, &glyphs::spoken(&pieces));
    }
    back.clicked()
}

#[cfg(test)]
mod tests;
