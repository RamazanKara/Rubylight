//! The Control Center's cards and quiet rows (client/design/README.md, "Control Center"): a card
//! is `SIZE_CONTROL_CENTER_CARD` tall with an icon, a label and one line of state, lit amber while
//! what it switches is on; a slider is a wide card with its track inside; status and notices are
//! rows focus passes over. All of it draws on `SURFACE_CONTROL_CENTER_*`, the same in the night,
//! day and OLED themes, since it only ever shows over video. `control_center.rs` is the frame
//! around them.

use std::sync::Arc;

use egui::{pos2, vec2, Color32, FontId, Galley, Id, Margin, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo};

use super::design_tokens as tokens;
use crate::glyphs::{self, Icon};
use crate::theme::{regular, semibold, text};

/// Between two cards, across and down.
pub const GAP: f32 = 8.0;
/// Cards to a row: the dock is narrow, and a card has to hold a label and its state.
pub const COLUMNS: usize = 2;
/// A status row's height.
pub const ROW: f32 = 40.0;
/// What a slider's label and value field take of a wide card, beside its track.
const SLIDER_ROOM: f32 = 176.0;
const RADIUS: u8 = 16;
const PAD: f32 = 11.0;
const ICON: f32 = 18.0;

/// The label's ink and the quieter one of the line under it. High contrast takes white for both.
pub fn inks() -> (Color32, Color32) {
    if crate::theme::high_contrast() {
        (Color32::WHITE, Color32::WHITE)
    } else {
        (tokens::SURFACE_CONTROL_CENTER_LABEL, tokens::SURFACE_CONTROL_CENTER_LABEL_SECONDARY)
    }
}

/// The amber of what is on or selected.
pub fn amber() -> Color32 {
    *crate::theme::MOON
}

/// A card's fill and rim: lit while `on`. High contrast is black with a 2 px rim, amber while on.
pub fn card_colors(on: bool) -> (Color32, Color32, f32) {
    match (crate::theme::high_contrast(), on) {
        (true, true) => (Color32::BLACK, amber(), 2.0),
        (true, false) => (Color32::BLACK, tokens::GLASS_MENU_SOLID_RIM_TOP, 2.0),
        (false, true) => (tokens::SURFACE_CONTROL_CENTER_CARD_FILL_ON, tokens::SURFACE_CONTROL_CENTER_CARD_RIM_ON, 1.0),
        (false, false) => (tokens::SURFACE_CONTROL_CENTER_CARD_FILL, tokens::SURFACE_CONTROL_CENTER_CARD_RIM, 1.0),
    }
}

/// One line of `text` no wider than `width`, cut with an ellipsis.
pub fn line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// `text` in at most `rows` lines no wider than `width`, the last cut with an ellipsis.
pub fn lines(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = rows;
    ui.painter().layout_job(job)
}

/// The card surface under a control: fill, the lift of a pointer, rim and the focus ring.
pub fn surface(ui: &Ui, rect: Rect, on: bool, response: &Response) {
    let (fill, rim, width) = card_colors(on);
    ui.painter().rect_filled(rect, RADIUS, fill);
    let mut lift = Vec::new();
    super::lift_shapes(&mut lift, ui.ctx(), rect, f32::from(RADIUS), response.hovered(), response.is_pointer_button_down_on());
    ui.painter().extend(lift);
    ui.painter().rect_stroke(rect, RADIUS, Stroke::new(width, rim), StrokeKind::Inside);
    super::focus_ring(ui, rect, response, RADIUS);
}

/// What a card shows.
#[derive(Clone, Copy, Debug)]
pub struct Card<'a> {
    pub icon: Icon,
    pub label: &'a str,
    /// The line under the label: On or Off, a value, a shortcut's keys.
    pub state: Option<&'a str>,
    /// Some for a switch, and how it stands.
    pub on: Option<bool>,
    /// The controller button the user gave it, in its corner.
    pub cap: Option<&'a str>,
}

/// A card `width` wide. `id` is its own, so the focus finds it again when the Control Center
/// opens the next time.
pub fn card(ui: &mut Ui, id: Id, width: f32, card: &Card<'_>) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, tokens::SIZE_CONTROL_CENTER_CARD), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let on = card.on == Some(true);
    surface(ui, rect, on, &response);
    let (ink, quiet) = inks();
    let lit = if on { amber() } else { ink };
    glyphs::icon(ui.painter(), Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(ICON, ICON)), card.icon, lit);
    if let Some(cap) = card.cap {
        let cap = line(ui, cap, semibold(text::CAPTION), quiet, 56.0);
        let at = pos2(rect.right() - PAD - cap.size().x, rect.top() + PAD);
        ui.painter().galley(at, cap, quiet);
    }
    let inner = width - 2.0 * PAD;
    let label = line(ui, card.label, semibold(text::LABEL), ink, inner);
    let left = rect.left() + PAD;
    match card.state {
        Some(state) => {
            let state = line(ui, state, regular(text::CAPTION), if on { amber() } else { quiet }, inner);
            let state_top = rect.bottom() - PAD + 2.0 - state.size().y;
            ui.painter().galley(pos2(left, state_top - label.size().y), label, ink);
            ui.painter().galley(pos2(left, state_top), state, quiet);
        }
        None => {
            // Without a state line a long label takes a second line, as low as a state's, so
            // "Session and graphics" stays whole beside the icon instead of losing its end.
            if label.elided {
                let label = lines(ui, card.label, semibold(text::LABEL), ink, inner, 2);
                ui.painter().galley(pos2(left, rect.bottom() - PAD + 2.0 - label.size().y), label, ink);
            } else {
                ui.painter().galley(pos2(left, rect.bottom() - PAD - label.size().y), label, ink);
            }
        }
    }
    let (enabled, name, switch) = (ui.is_enabled(), card.label.to_owned(), card.on);
    response.widget_info(|| match switch {
        Some(on) => WidgetInfo::selected(egui::Role::CheckBox, enabled, on, &name),
        None => WidgetInfo::labeled(egui::Role::Button, enabled, &name),
    });
    response
}

/// How wide a card is in `ui`, [`COLUMNS`] to a row.
pub fn card_width(ui: &Ui) -> f32 {
    let columns = COLUMNS as f32;
    ((ui.available_width() - GAP * (columns - 1.0)) / columns).floor()
}

/// Lays `count` cards out [`COLUMNS`] to a row; `draw` draws the one at an index, as wide as it is told.
pub fn grid(ui: &mut Ui, count: usize, mut draw: impl FnMut(&mut Ui, usize, f32)) {
    let width = card_width(ui);
    for row in 0..count.div_ceil(COLUMNS) {
        if row > 0 {
            ui.add_space(GAP - ui.spacing().item_spacing.y);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for index in row * COLUMNS..count.min((row + 1) * COLUMNS) {
                draw(ui, index, width);
            }
        });
    }
}

/// Paints a card's surface behind what `add` laid out, once its size is known.
fn framed<R>(ui: &mut Ui, margin: Margin, radius: u8, add: impl FnOnce(&mut Ui) -> R) -> (R, Rect) {
    let place = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::new().inner_margin(margin).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = inner.response.rect;
    let (fill, rim, width) = card_colors(false);
    let shapes = vec![
        Shape::rect_filled(rect, radius, fill),
        Shape::rect_stroke(rect, radius, Stroke::new(width, rim), StrokeKind::Inside),
    ];
    ui.painter().set(place, Shape::Vec(shapes));
    (inner.inner, rect)
}

/// A wide card across the page: its icon, label and value on one line, and under it what `add`
/// draws, a slider's track.
pub fn wide<R>(ui: &mut Ui, icon: Icon, label: &str, value: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (inner, _) = framed(ui, Margin::same(PAD as i8), RADIUS, |ui| {
        ui.set_min_height(tokens::SIZE_CONTROL_CENTER_CARD - 2.0 * PAD);
        let (ink, quiet) = inks();
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(ICON, ICON), Sense::hover());
            glyphs::icon(ui.painter(), rect, icon, ink);
            ui.label(RichText::new(label).font(semibold(text::LABEL)).color(ink));
            if let Some(value) = value {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(value).font(regular(text::CAPTION)).color(quiet));
                });
            }
        });
        // A slider's track takes what its label and value leave of the card.
        ui.spacing_mut().slider_width = (ui.available_width() - SLIDER_ROOM).max(48.0);
        add(ui)
    });
    inner
}

/// A row whose control is too wide to sit beside its label in the dock (a list): the label with
/// its icon, and under it the control, lined up with the label. `add` is told the width it has.
pub fn stacked<R>(ui: &mut Ui, icon: Icon, label: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
    crate::menu_style::row(ui, icon, label, |_| {});
    ui.horizontal(|ui| {
        ui.add_space(2.0 * crate::menu_style::PAD + ICON - ui.spacing().item_spacing.x);
        let width = (ui.available_width() - crate::menu_style::PAD).max(96.0);
        add(ui, width)
    })
    .inner
}

/// A read-only line of status: what it is on the left, how it stands on the right. Not a control,
/// so focus passes over it. Answers where it was drawn.
pub fn status_row(ui: &mut Ui, icon: Icon, label: &str, value: &str) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let (ink, quiet) = inks();
    let middle = rect.center().y;
    glyphs::icon(ui.painter(), Rect::from_center_size(pos2(rect.left() + PAD + ICON / 2.0, middle), vec2(ICON, ICON)), icon, quiet);
    let room = rect.width() - 3.0 * PAD - ICON - GAP;
    let value = line(ui, value, semibold(text::LABEL), ink, room * 0.45);
    let label = line(ui, label, regular(text::LABEL), quiet, room - value.size().x);
    ui.painter().galley(pos2(rect.left() + 2.0 * PAD + ICON, middle - label.size().y / 2.0), label, quiet);
    ui.painter().galley(pos2(rect.right() - PAD - value.size().x, middle - value.size().y / 2.0), value, ink);
    rect
}

/// A quiet notice at the top of a page: something the stream said that still holds. An amber bar
/// marks it; the text wraps. Answers where it was drawn.
pub fn notice_row(ui: &mut Ui, text_: &str) -> Rect {
    let margin = Margin { left: 20, right: 12, top: 10, bottom: 10 };
    let (_, rect) = framed(ui, margin, 12, |ui| {
        ui.label(RichText::new(text_).font(regular(text::CAPTION)).color(inks().1));
    });
    let bar = Rect::from_min_max(pos2(rect.left() + 9.0, rect.top() + 10.0), pos2(rect.left() + 12.0, rect.bottom() - 10.0));
    ui.painter().rect_filled(bar, 2, amber());
    rect
}
