//! Brand mark rasterisation and shared slider/value controls.

use super::*;

/// The brand mark: the nightjar crossing the moon, from `site/assets/og.svg` (a 108-unit
/// adaptive icon: moon radius 27 around (54, 54)).
const BIRD: &str = "M 51.75,41.24 C 53.31,40.96 54.50,41.92 54.89,43.31 C 55.16,44.00 55.33,44.55 55.55,44.95 C 58.63,42.95 61.40,40.41 64.45,38.63 C 68.66,39.72 73.12,42.22 78.13,45.72 C 72.88,44.97 68.19,45.36 64.07,46.82 C 60.97,47.94 58.40,49.93 56.11,51.87 L 56.49,56.49 L 59.86,66.49 C 58.86,67.47 57.71,67.97 56.50,68.18 C 55.30,68.39 54.04,68.32 52.77,67.74 L 52.51,57.19 L 51.29,52.72 C 48.47,51.68 45.38,50.70 42.08,50.69 C 37.70,50.73 33.17,51.97 28.50,54.48 C 32.00,49.47 35.34,45.59 38.92,43.13 C 42.40,43.76 45.87,45.20 49.45,46.03 C 49.52,45.58 49.49,45.00 49.51,44.26 C 49.40,42.82 50.19,41.51 51.75,41.24 Z";

/// The bird alone over the moon's box (27 to 81 units), white with coverage as its alpha, to tint.
pub fn bird_mask(size: usize) -> egui::ColorImage {
    const SUB: usize = 4;
    const LEFT: f32 = 27.0;
    const SPAN: f32 = 54.0;
    let polygon = bird_polygon();
    let mut coverage = vec![0.0f32; size * size];
    let mut crossings = Vec::new();
    for row in 0..size * SUB {
        let y = LEFT + (row as f32 + 0.5) / (size * SUB) as f32 * SPAN;
        crossings.clear();
        for (i, a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            if (a.1 > y) != (b.1 > y) {
                let x = a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0);
                crossings.push((x - LEFT) / SPAN * size as f32);
            }
        }
        crossings.sort_by(f32::total_cmp);
        for span in crossings.as_chunks::<2>().0 {
            let (from, to) = (span[0].clamp(0.0, size as f32), span[1].clamp(0.0, size as f32));
            let line = row / SUB;
            for x in from as usize..(to.ceil() as usize).min(size) {
                let over = (to.min(x as f32 + 1.0) - from.max(x as f32)).max(0.0);
                coverage[line * size + x] += over / SUB as f32;
            }
        }
    }
    let pixels = coverage
        .iter()
        .map(|c| egui::Color32::from_white_alpha((c.min(1.0) * 255.0).round() as u8))
        .collect();
    egui::ColorImage {
        size: [size, size],
        source_size: egui::vec2(size as f32, size as f32),
        pixels,
    }
}

/// The bird outline flattened to a polygon in icon units.
pub(super) fn bird_polygon() -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let mut tokens = BIRD
        .split([' ', ','])
        .filter(|t| !t.is_empty())
        .peekable();
    let mut current = (0.0f32, 0.0f32);
    let mut command = 'M';
    let number = |tokens: &mut std::iter::Peekable<_>| -> f32 {
        let t: &str = tokens.next().unwrap_or("0");
        t.parse().unwrap_or(0.0)
    };
    while let Some(&token) = tokens.peek() {
        if let Some(c) = token.chars().next().filter(char::is_ascii_alphabetic) {
            command = c;
            tokens.next();
            if c == 'Z' {
                break;
            }
            continue;
        }
        match command {
            'M' | 'L' => {
                current = (number(&mut tokens), number(&mut tokens));
                points.push(current);
            }
            'C' => {
                let c1 = (number(&mut tokens), number(&mut tokens));
                let c2 = (number(&mut tokens), number(&mut tokens));
                let end = (number(&mut tokens), number(&mut tokens));
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    let x = u * u * u * current.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * end.0;
                    let y = u * u * u * current.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * end.1;
                    points.push((x, y));
                }
                current = end;
            }
            _ => {
                tokens.next();
            }
        }
    }
    points
}

pub(super) fn inside(polygon: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A number field a controller or the arrow keys can leave. egui's `DragValue` edits as soon as it has
/// focus and steps on Up and Down, so those keys changed the value and the user was stuck on the row.
/// Here Up and Down stay with focus navigation like every other row, and Left and Right step the value
/// by `step`. Once the user types into the field, Left and Right move the caret again until focus leaves.
pub fn step_value<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    style: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    let id = ui.next_auto_id();
    let typing = id.with("typing");
    let mut stepped = false;
    if ui.is_enabled() && ui.memory(|m| m.has_focus(id)) {
        let typed = ui.data(|d| d.get_temp::<bool>(typing).unwrap_or(false))
            || ui.input(|i| i.events.iter().any(|e| match e {
                egui::Event::Text(_) | egui::Event::Paste(_) => true,
                egui::Event::Key { key, pressed: true, .. } => matches!(key, egui::Key::Backspace | egui::Key::Delete),
                _ => false,
            }));
        if typed { ui.data_mut(|d| d.insert_temp(typing, true)); }
        let steps = ui.input_mut(|i| {
            // Focus has already moved on these (egui reads them before the frame); the field must not step too.
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
            if typed { return 0; }
            i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) as i64
                - i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) as i64
        });
        if steps != 0 {
            let (low, high) = (range.start().to_f64(), range.end().to_f64());
            let before = value.to_f64();
            *value = N::from_f64((before + steps as f64 * step).clamp(low, high));
            stepped = value.to_f64() != before;
        }
    } else {
        ui.data_mut(|d| d.remove::<bool>(typing));
    }
    let mut response = ui.add(style(egui::DragValue::new(value).range(range)));
    // The value changed before the DragValue saw it, so the response would not say so; callers that
    // write the value back on `changed()` (the stream menu's fps) must hear about the step.
    if stepped { response.mark_changed(); }
    if ui.is_enabled() && response.has_focus() {
        // Its text field has just locked every arrow key to the caret; give Up and Down back to focus.
        let filter = egui::EventFilter { horizontal_arrows: true, ..Default::default() };
        ui.memory_mut(|m| m.set_focus_lock_filter(id, filter));
    }
    response
}

/// A slider a controller or the arrow keys can leave. egui's slider shows its value in a `DragValue`
/// of its own, which steps on Up and Down once it has focus: Up from the row below landed there and
/// only raised the value (the stream menu's bitrate). Here the value is a [`step_value`] beside the
/// rail, so Up and Down move focus on both, and Left and Right step on both. While the rail is
/// dragged, and for [`VALUE_AFTER_KEY`] seconds after a key step, `shown` of the value floats over the
/// thumb, where a finger on a touch screen does not cover it (nightjar#61).
pub fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    shown: impl Fn(f64) -> String,
    rail: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
    field: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    slider_along(ui, value, range, step, false, shown, rail, field)
}

/// [`slider`] whose rail runs in equal ratios, for a value across orders of magnitude. The rail
/// has to be told so with `logarithmic(true)` in `rail`.
pub fn log_slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    shown: impl Fn(f64) -> String,
    rail: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
    field: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    slider_along(ui, value, range, step, true, shown, rail, field)
}

/// A whole percent as a slider's value shows it, `50%`.
pub fn percent(value: f64) -> String {
    format!("{value:.0}%")
}

/// A bitrate in Mbps as the settings' field shows it, `20.0 Mbps`.
pub fn mbps_text(mbps: f64) -> String {
    format!("{mbps:.1} Mbps")
}

/// How long a slider's value stays over its thumb after the last key step, in seconds; Android's
/// `StepSlider.KEY_VALUE_MS`.
pub const VALUE_AFTER_KEY: f64 = 1.5;

/// Whether a slider's value floats over its thumb: while it is dragged, and while it has the focus
/// within [`VALUE_AFTER_KEY`] of a key step `since_step` seconds ago.
pub(super) fn value_floats(dragged: bool, focused: bool, since_step: Option<f64>) -> bool {
    dragged || (focused && since_step.is_some_and(|since| (0.0..VALUE_AFTER_KEY).contains(&since)))
}

/// Where the value floats: centred over the thumb at `thumb`, clear of it, inside `screen`.
pub(super) fn value_rect(thumb: egui::Pos2, label: egui::Vec2, screen: egui::Rect) -> egui::Rect {
    let size = label + egui::vec2(2.0 * space::M, 2.0 * space::XS);
    let bottom = thumb.y - t::SIZE_SLIDER_THUMB / 2.0 - space::S;
    let rect = egui::Rect::from_min_size(egui::pos2(thumb.x - size.x / 2.0, bottom - size.y), size);
    let shift = (screen.left() - rect.left()).max(0.0) + (screen.right() - rect.right()).min(0.0);
    rect.translate(egui::vec2(shift, (screen.top() - rect.top()).max(0.0)))
}

/// The value over the thumb, in Android's slider label: the inverse surface, on top of everything.
fn paint_value(ui: &egui::Ui, thumb: egui::Pos2, shown: String) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("slider-value")));
    let galley = painter.layout_no_wrap(shown, FontId::proportional(text::LABEL), *SURFACE);
    let rect = value_rect(thumb, galley.size(), ui.ctx().content_rect());
    painter.rect_filled(rect, egui::CornerRadius::same(radius::SMALL), *ON_SURFACE);
    painter.galley(rect.center() - galley.size() / 2.0, galley, *SURFACE);
}

/// Where `value` stands along a rail, 0 to 1. Only a range above zero runs in ratios here.
pub(super) fn rail_position(value: f64, low: f64, high: f64, ratios: bool) -> f32 {
    let along = if ratios && low > 0.0 {
        (value.max(low) / low).ln() / (high / low).ln()
    } else {
        (value - low) / (high - low)
    };
    if along.is_finite() { along.clamp(0.0, 1.0) as f32 } else { 0.0 }
}

#[allow(clippy::too_many_arguments)] // the slider's value, range, step and look, passed as egui takes them
fn slider_along<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    ratios: bool,
    shown: impl Fn(f64) -> String,
    rail: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
    field: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>,
) -> egui::Response {
    ui.horizontal(|ui| {
        let (low, high) = (range.start().to_f64(), range.end().to_f64());
        // egui's slider keeps the input; the rail and thumb are ours, a moon fill and a paper
        // thumb, which egui's one colour for rail and thumb cannot draw.
        let slid = ui.scope(|ui| {
            ui.spacing_mut().slider_width = 150.0;
            let v = ui.visuals_mut();
            v.selection.bg_fill = Color32::TRANSPARENT;
            for look in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                look.bg_fill = Color32::TRANSPARENT;
                look.fg_stroke = Stroke::NONE;
                look.bg_stroke = Stroke::NONE;
            }
            ui.add(rail(egui::Slider::new(value, range.clone()).show_value(false)))
        }).inner;
        let typed = step_value(ui, value, range, step, field);
        let along = rail_position(value.to_f64(), low, high, ratios);
        let thumb = paint_rail(ui, &slid, along);
        let stepped_at = slid.id.with("stepped-at");
        let now = ui.input(|i| i.time);
        if (slid.changed() || typed.changed()) && slid.has_focus() && !slid.dragged() {
            ui.data_mut(|d| d.insert_temp(stepped_at, now));
        }
        let since_step = ui.data(|d| d.get_temp::<f64>(stepped_at)).map(|at| now - at);
        if value_floats(slid.dragged(), slid.has_focus(), since_step) {
            paint_value(ui, thumb, shown(value.to_f64()));
            if !slid.dragged() {
                ui.ctx().request_repaint_after_secs((VALUE_AFTER_KEY - since_step.unwrap_or(0.0)) as f32);
            }
        }
        slid | typed
    })
    .inner
}

/// The rail and its thumb; answers where the thumb's centre is.
fn paint_rail(ui: &egui::Ui, slid: &egui::Response, along: f32) -> egui::Pos2 {
    use crate::ui::design_tokens as t;
    let rect = slid.rect;
    // Where egui's slider has its thumb, so a drag stays under the pointer.
    let reach = (rect.height() / 2.5).round();
    let thumb_x = egui::lerp(rect.left() + reach..=rect.right() - reach, along);
    let middle = rect.center().y;
    let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), middle - t::SIZE_SLIDER_TRACK / 2.0), egui::pos2(rect.right(), middle + t::SIZE_SLIDER_TRACK / 2.0));
    let round = egui::CornerRadius::same((t::SIZE_SLIDER_TRACK / 2.0) as u8);
    let high = high_contrast();
    let painter = ui.painter();
    painter.rect_filled(bar, round, if high { *SURFACE_HIGHEST } else { shade::TINT_TRACK_FILL.get() });
    if high {
        painter.rect_stroke(bar, round, Stroke::new(1.0, t::HC_COLOR_RIM), egui::StrokeKind::Outside);
    }
    let filled = egui::Rect::from_min_max(bar.min, egui::pos2(thumb_x, bar.bottom()));
    painter.rect_filled(filled, round, *MOON);
    let center = egui::pos2(thumb_x, middle);
    painter.circle_filled(center + egui::vec2(0.0, 1.0), t::SIZE_SLIDER_THUMB / 2.0, Color32::from_black_alpha(70));
    painter.circle_filled(center, t::SIZE_SLIDER_THUMB / 2.0, knob());
    if slid.has_focus() {
        let spot = egui::Rect::from_center_size(center, egui::Vec2::splat(t::SIZE_SLIDER_THUMB));
        crate::ui::focus_ring_around(ui, spot, (t::SIZE_SLIDER_THUMB / 2.0) as u8);
    }
    center
}

/// The mark as straight RGBA pixels, `size` square: the amber moon with the night bird cut
/// out, 4x4 supersampled.
pub fn logo_rgba(size: usize) -> Vec<u8> {
    let bird = bird_polygon();
    // The mark keeps the brand colours in every palette.
    let (brand_moon, brand_night) = (NORMAL[Index::MOON as usize], NORMAL[Index::NIGHT as usize]);
    let mut out = vec![0u8; size * size * 4];
    let scale = 54.0 / size as f32; // the moon fills the image: 27 units of radius
    for py in 0..size {
        for px in 0..size {
            let (mut moon, mut cut) = (0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = 27.0 + (px as f32 + (sx as f32 + 0.5) / 4.0) * scale;
                    let y = 27.0 + (py as f32 + (sy as f32 + 0.5) / 4.0) * scale;
                    let in_moon = (x - 54.0).powi(2) + (y - 54.0).powi(2) <= 27.0 * 27.0;
                    if in_moon {
                        moon += 1;
                        if inside(&bird, x, y) {
                            cut += 1;
                        }
                    }
                }
            }
            let i = (py * size + px) * 4;
            if moon == 0 {
                continue;
            }
            // Bird pixels are the night colour; moon pixels amber; edges blend.
            let amber = (moon - cut) as f32;
            let night = cut as f32;
            let total = amber + night;
            let mix = |a: u8, n: u8| ((f32::from(a) * amber + f32::from(n) * night) / total) as u8;
            out[i] = mix(brand_moon.r(), brand_night.r());
            out[i + 1] = mix(brand_moon.g(), brand_night.g());
            out[i + 2] = mix(brand_moon.b(), brand_night.b());
            out[i + 3] = (moon * 255 / 16) as u8;
        }
    }
    out
}

pub fn logo_image(size: usize) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([size, size], &logo_rgba(size))
}
