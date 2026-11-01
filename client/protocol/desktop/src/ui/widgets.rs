//! The launcher's building blocks: buttons, switches, chips, the focus ring, scroll areas,
//! section headings and rows.

use super::*;

/// A switch in the brand colours; returns whether it changed.
pub(crate) fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(44.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    // A switch with its state for screen readers; the row names it (name_for_readers).
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, ""));
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let enabled = ui.is_enabled();
    let track: Color32 = if *on { *MOON } else { *SURFACE_HIGHEST };
    let track = if enabled { track } else { track.gamma_multiply(0.4) };
    ui.painter().rect_filled(rect, CornerRadius::same(12), track);
    let knob_x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    let knob = if *on { *ON_MOON } else { *OUTLINE };
    ui.painter().circle_filled(Pos2::new(knob_x, rect.center().y), 8.0, knob);
    focus_ring(ui, rect, &response, 12);
    response
}

/// The focus ring: one amber ring a small gap outside the element, at its own rounding. Only focus
/// draws it (hover lightens the fill instead, [`crate::theme::state_fill`]), and it sits outside the
/// element, so nothing resizes or moves when focus arrives.
pub(crate) fn focus_ring(ui: &Ui, rect: Rect, response: &Response, radius: u8) {
    if response.has_focus() {
        paint_focus_ring(ui.painter(), rect, radius);
    }
}

pub(crate) fn paint_focus_ring(painter: &egui::Painter, rect: Rect, radius: u8) {
    painter.rect_stroke(
        rect.expand(FOCUS_GAP),
        CornerRadius::same(radius.saturating_add(FOCUS_GAP as u8)),
        Stroke::new(focus_ring_width(), MOON),
        StrokeKind::Outside,
    );
}

/// Whether the widget about to be added was hovered (or focused) and pressed last frame: how a
/// control picks its fill before egui knows this frame's answer.
pub(super) fn last_state(ui: &Ui) -> (bool, bool) {
    let id = ui.next_auto_id();
    ui.ctx().read_response(id).map_or((false, false), |r| (r.hovered() || r.has_focus(), r.is_pointer_button_down_on()))
}

/// The outline every control has: none in the night palette, a visible one in high contrast.
pub(super) fn control_stroke() -> Stroke {
    if high_contrast() { Stroke::new(1.5, OUTLINE_VARIANT) } else { Stroke::NONE }
}

/// What a button is filled with, and so how it reacts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tone {
    /// The one main action: amber.
    Primary,
    /// A second choice beside it (Cancel).
    Quiet,
    /// A page action in the header (Add PC, Refresh).
    Raised,
    /// No fill until pointed at: back links, menu rows.
    Plain,
    /// A small pill (filters, section jumps).
    Chip,
    /// A chip that is the current choice.
    Selected,
    /// The top bar's destination of the page shown: raised, its icon amber.
    Current,
}

impl Tone {
    /// Fill and text colour for the state.
    pub(crate) fn colors(self, hovered: bool, pressed: bool) -> (Color32, Color32) {
        let surface = |base: Role, text: Role| (*state_fill(base, hovered, pressed), *text);
        match self {
            Tone::Primary => {
                let fill = if pressed {
                    tint(*MOON, Color32::BLACK, 0.12)
                } else if hovered {
                    tint(*MOON, Color32::WHITE, 0.16)
                } else {
                    *MOON
                };
                (fill, *ON_MOON)
            }
            Tone::Quiet | Tone::Current => surface(SURFACE_HIGHEST, ON_SURFACE),
            Tone::Raised => surface(SURFACE_HIGH, ON_SURFACE),
            Tone::Chip => surface(SURFACE, ON_SURFACE_VARIANT),
            Tone::Plain => {
                let fill = if pressed {
                    *SURFACE_HIGHEST
                } else if hovered {
                    *SURFACE_HIGH
                } else {
                    Color32::TRANSPARENT
                };
                (fill, if hovered || pressed { *ON_SURFACE } else { *ON_SURFACE_VARIANT })
            }
            Tone::Selected => {
                let fill = if hovered || pressed { tint(*MOON_CONTAINER, *MOON, 0.14) } else { *MOON_CONTAINER };
                (fill, *ON_MOON_CONTAINER)
            }
        }
    }
}

/// A button's size: its height, side padding, rounding and type.
#[derive(Clone, Copy)]
pub(crate) struct Size {
    pub height: f32,
    pub pad: f32,
    pub radius: u8,
    pub font: f32,
}

impl Size {
    pub(crate) const CONTROL: Size = Size { height: CONTROL_HEIGHT, pad: 16.0, radius: radius::CONTROL, font: text::LABEL };
    pub(crate) const HEADER: Size = Size { height: 40.0, pad: 14.0, radius: radius::CONTROL, font: text::LABEL };
    pub(crate) const CHIP: Size = Size { height: 34.0, pad: 14.0, radius: 17, font: text::LABEL };
}

/// A button drawn the launcher's way: a fill that lightens on hover and press, never an outline
/// (high contrast keeps its own), the focus ring outside it, an optional icon before the words.
/// The words are shown as given (translate them first).
pub(crate) fn button(ui: &mut Ui, icon: Option<Icon>, words: &str, tone: Tone, size: Size) -> Response {
    button_with(ui, None, icon, words, None, tone, size)
}

/// [`button`] with a count after the words, in a quieter colour (a filter chip's games). With an
/// `id` the button is that one wherever it lands in its row, so the focus stays on it when the
/// buttons before it change; without, it is known by its place.
pub(crate) fn button_with(ui: &mut Ui, id: Option<egui::Id>, icon: Option<Icon>, words: &str, count: Option<usize>, tone: Tone, size: Size) -> Response {
    let (hovered, pressed) = match id {
        Some(id) => ui.ctx().read_response(id).map_or((false, false), |r| (r.hovered() || r.has_focus(), r.is_pointer_button_down_on())),
        None => last_state(ui),
    };
    let enabled = ui.is_enabled();
    // A switched-off primary action (Play for a game the PC lacks) goes grey, so faded amber
    // never reads as something to press.
    let tone = if enabled || tone != Tone::Primary { tone } else { Tone::Raised };
    let (fill, ink) = tone.colors(hovered && enabled, pressed && enabled);
    let galley = ui.painter().layout_no_wrap(words.to_owned(), semibold(size.font), ink);
    let count = count.map(|n| ui.painter().layout_no_wrap(n.to_string(), regular(size.font), ink.gamma_multiply(0.72)));
    let icon_w = if icon.is_some() { 18.0 + if words.is_empty() { 0.0 } else { 8.0 } } else { 0.0 };
    let count_w = count.as_ref().map_or(0.0, |c| 6.0 + c.size().x);
    let pad = if words.is_empty() { (size.height - 18.0) / 2.0 } else { size.pad };
    let width = pad * 2.0 + icon_w + galley.size().x + count_w;
    let (rect, response) = match id {
        Some(id) => {
            let (_, rect) = ui.allocate_space(vec2(width, size.height));
            (rect, ui.interact(rect, id, Sense::click()))
        }
        None => ui.allocate_exact_size(vec2(width, size.height), Sense::click()),
    };
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, words));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(rect, CornerRadius::same(size.radius), fill, control_stroke(), StrokeKind::Inside);
        let mut x = rect.left() + pad;
        if let Some(icon) = icon {
            let icon_ink = if tone == Tone::Current { *MOON } else { ink };
            icons::paint(painter, icon, Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)), icon_ink);
            x += icon_w;
        }
        let words_w = galley.size().x;
        painter.galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, ink);
        if let Some(count) = count {
            x += words_w + 6.0;
            painter.galley(Pos2::new(x, rect.center().y - count.size().y / 2.0), count, ink);
        }
    }
    focus_ring(ui, rect, &response, size.radius);
    response
}

pub(crate) fn primary_button(ui: &mut Ui, text: &str) -> Response {
    button(ui, None, tr(text), Tone::Primary, Size::CONTROL)
}

pub(crate) fn quiet_button(ui: &mut Ui, text: &str) -> Response {
    button(ui, None, tr(text), Tone::Quiet, Size::CONTROL)
}

pub(super) fn text_button(ui: &mut Ui, text: &str) -> Response {
    button(ui, None, tr(text), Tone::Plain, Size { height: 36.0, pad: 12.0, ..Size::CONTROL })
}

/// A link in running text (How to connect): amber words, underlined while pointed at.
pub(super) fn link_button(ui: &mut Ui, text: &str) -> Response {
    let words = RichText::new(tr(text)).color(MOON).font(semibold(text::LABEL));
    let hovered = last_state(ui).0;
    let button = egui::Button::new(if hovered { words.underline() } else { words }).frame(false).min_size(vec2(0.0, 32.0));
    let response = ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand);
    focus_ring(ui, response.rect, &response, radius::CONTROL);
    response
}

/// How to connect: the pairing guide Android's help opens, wherever PCs are still to find.
pub(super) fn how_to_connect(ui: &mut Ui, commands: &mut Vec<Command>) -> Response {
    let help = link_button(ui, "How to connect");
    if help.clicked() {
        commands.push(Command::OpenGuide("pairing.md"));
    }
    help
}

/// An action in the header (Add PC, Refresh): a filled button with its icon, so it never reads as
/// one of the destinations beside it, at the header's height.
pub(super) fn header_button(ui: &mut Ui, text: &str, icon: Icon) -> Response {
    button(ui, Some(icon), tr(text), Tone::Raised, Size::HEADER)
}

/// A small pill that jumps to a section of the settings.
pub(super) fn chip(ui: &mut Ui, text: &str) -> Response {
    button(ui, None, tr(text), Tone::Chip, Size::CHIP)
}

/// A spinning arc. With reduced motion (egui's animation time is 0) the arc stands still and
/// nothing asks for the next frame.
pub(super) fn spinner(ui: &mut Ui, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0), Sense::hover());
    let still = ui.style().animation_time <= 0.0;
    let t = if still { 0.0 } else { ui.input(|i| i.time) as f32 };
    let center = rect.center();
    let start = t * 4.0;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + i as f32 / 24.0 * std::f32::consts::PI * 1.4;
            center + radius * 0.8 * vec2(a.cos(), a.sin())
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(2.5, MOON)));
    if !still {
        ui.ctx().request_repaint();
    }
}

/// A vertical scroll area with room for focus rings at its edges.
pub(super) fn scroll(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    let rect = ui.available_rect_before_wrap();
    let rect = Rect::from_min_max(rect.min - vec2(6.0, 6.0), rect.max + vec2(6.0, 0.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink(false)
            .content_margin(Margin::same(6))
            .show(ui, |ui| {
                crate::gui::follow_focus_here(ui);
                add(ui);
            });
    });
}

/// [`scroll`] for a column of rows `width` wide: the scroll bar sits just right of the rows, not at
/// the far edge of a wide window, and the wheel still scrolls the page from beside the column.
pub(super) fn scroll_column(ui: &mut Ui, id: &str, width: f32, add: impl FnOnce(&mut Ui)) {
    let page = ui.available_rect_before_wrap();
    // The rows, their margin each side, and a gap the floating bar sits in.
    let right = (page.left() + width + 6.0 + space::L).min(page.right() + 6.0);
    let rect = Rect::from_min_max(page.min - vec2(6.0, 6.0), Pos2::new(right, page.bottom()));
    let beside = Rect::from_min_max(Pos2::new(rect.right(), page.top()), page.max);
    // Where the list was and how far it goes, from the last frame, for a wheel turned beside it.
    let memory = Id::unique(("scroll-column", id));
    let wheel = if beside.is_positive() && ui.rect_contains_pointer(beside) { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
    let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).content_margin(Margin::same(6));
    if wheel != 0.0 {
        if let Some((offset, end)) = ui.ctx().data(|d| d.get_temp::<(f32, f32)>(memory)) {
            area = area.vertical_scroll_offset((offset - wheel).clamp(0.0, end));
        }
    }
    let list = ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| area.show(ui, |ui| {
        crate::gui::follow_focus_here(ui);
        add(ui);
    })).inner;
    let end = (list.content_size.y - list.inner_rect.height()).max(0.0);
    ui.ctx().data_mut(|d| d.insert_temp(memory, (list.state.clamped_offset().y, end)));
}

/// Shades the edge of a scroll area that has more beyond it into `fill` (what it sits on), so a
/// row cut there reads as "more" and not as broken. Over the rows only, not the scroll bar.
pub(super) fn scroll_fades<R>(ui: &Ui, list: &egui::scroll_area::ScrollAreaOutput<R>, fill: Color32) {
    let scroll = ui.spacing().scroll;
    let view = list.inner_rect;
    let view = Rect::from_min_max(view.min, Pos2::new(view.right() - scroll.bar_width - scroll.bar_outer_margin, view.bottom()));
    let offset = list.state.clamped_offset().y;
    let hidden = list.content_size.y - list.inner_rect.height();
    let shade = |rect: Rect, down: bool| {
        let (top, bottom) = if down { (Color32::TRANSPARENT, fill) } else { (fill, Color32::TRANSPARENT) };
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(rect.left_top(), top);
        mesh.colored_vertex(rect.right_top(), top);
        mesh.colored_vertex(rect.left_bottom(), bottom);
        mesh.colored_vertex(rect.right_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        ui.painter().add(mesh);
    };
    if offset > 0.5 {
        shade(Rect::from_min_size(view.min, vec2(view.width(), space::XL)), false);
    }
    if offset < hidden - 0.5 {
        shade(Rect::from_min_max(Pos2::new(view.left(), view.bottom() - space::XL), view.max), true);
    }
}

/// A line of a details page: what it is, and its value.
pub(super) fn detail(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(label).font(semibold(text::BODY)).color(ON_SURFACE));
        ui.label(RichText::new(value).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
    });
}

pub(super) fn section(ui: &mut Ui, title: &str) {
    section_at(ui, title, &mut None);
}

/// A section heading; scrolls itself to the top when `jump` names it (a section chip was used).
pub(super) fn section_at(ui: &mut Ui, title: &str, jump: &mut Option<&'static str>) {
    ui.add_space(14.0);
    if jump.is_some_and(|j| j == title) {
        ui.scroll_to_cursor(Some(Align::TOP));
        *jump = None;
    }
    ui.label(RichText::new(tr(title).to_uppercase()).font(semibold(text::CAPTION)).color(MOON));
    ui.add_space(2.0);
}

/// A settings row: label and description on the left, the control on the right.
/// Every row takes the settings column's width at most, so rows line up on every page.
pub(super) fn row(ui: &mut Ui, label: &str, description: &str, control: impl FnOnce(&mut Ui) -> Response) -> Response {
    let mut out = None;
    // The frame's margin comes on top of the width set inside it.
    let width = ui.available_width().min(SETTINGS_W) - 2.0 * 18.0;
    Frame::new()
        .fill(*SURFACE)
        .corner_radius(CornerRadius::same(radius::CONTROL))
        .inner_margin(Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let control_width = 240.0;
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - control_width - 16.0).max(200.0));
                    ui.label(RichText::new(tr(label)).font(semibold(text::BODY)).color(ON_SURFACE));
                    ui.label(RichText::new(tr(description)).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let response = control(ui);
                    name_for_readers(ui, &response, label, description);
                    out = Some(response);
                });
            });
        });
    ui.add_space(2.0);
    out.expect("control drawn")
}

/// A row's switch, list or slider has no text of its own: a screen reader says the row's label for
/// it, and its description, as Android's preferences read their title and summary.
pub(crate) fn name_for_readers(ui: &Ui, response: &Response, label: &str, description: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| {
        if node.role() != egui::Role::Label && node.label().is_none_or(|l| l.trim().is_empty()) && node.labelled_by().is_empty() {
            node.set_label(tr(label));
        }
        if node.description().is_none() {
            node.set_description(tr(description));
        }
    });
}

pub(super) trait UiExt {
    fn ui(self, ui: &mut Ui) -> Response;
}

impl UiExt for egui::Image<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.add(self)
    }
}
