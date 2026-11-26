//! Draws a page into the 1x panel the encoder composites (section 6.5) in
//! the Rubylight 7.0 look: a STAGE container with a hairline border, a
//! header with the badge, and each row as a rounded card inset in its text
//! line, its value drawn as a switch, slider, chevrons or text. Text stays
//! crisp 1-bit Spleen (the GPU scales the panel by whole numbers); shapes are
//! anti-aliased against the opaque colour beneath them, so alpha stays 255
//! everywhere. Colours come from [`super::theme_70`] only.
use super::model::{Line, Menu, Snapshot, Style, Widget, fit};
use super::theme_70::{CARD, ERROR, FOCUS, HAIRLINE, INK, MUTED, Rgb, STAGE};
use super::widgets::{self, Canvas, Tone};
use butterpollo_input_policy::couch_menu::{
    Frame,
    font::Font,
    layout::{COLUMNS, Layout},
};

/// Text columns a slider's rail takes.
const RAIL_COLUMNS: u32 = 5;

/// One page's geometry at 1x.
struct Grid {
    font: &'static Font,
    pad: [u32; 2],
    /// Width of the canvas.
    width: u32,
    /// Cell height over 32: the widgets' unit.
    u: f32,
}

impl Grid {
    /// Left edge of text column `column`.
    fn x(&self, column: u32) -> u32 {
        self.pad[0] + column * self.font.width
    }
    /// Top of text line `line`.
    fn y(&self, line: usize) -> u32 {
        self.pad[1] + line as u32 * self.font.height
    }
}

/// Draws `menu`'s current page for `snap` at `layout`'s font, centred in the
/// target.
pub fn render(menu: &Menu, snap: &Snapshot, layout: &Layout) -> Frame {
    let lines = menu.lines(snap);
    let count = lines.len() as u32;
    let rect = layout.panel(count);
    let [width, height] = layout.size_1x(count);
    let font = layout.font.font();
    let grid = Grid {
        font,
        pad: layout.padding(),
        width,
        u: font.height as f32 / 32.,
    };
    let mut canvas = Canvas::new(width, height, STAGE);
    canvas.border(HAIRLINE);
    for (index, line) in lines.iter().enumerate() {
        let y = grid.y(index);
        match line.style {
            Style::Title => header(&mut canvas, &grid, y, &line.text),
            Style::Row { focus, locked } => row(&mut canvas, &grid, y, line, focus, locked),
            Style::Info | Style::Note | Style::Hint => {
                text(&mut canvas, font, [grid.x(0), y], &line.text, MUTED, false);
            }
            Style::Message => text(&mut canvas, font, [grid.x(0), y], &line.text, INK, false),
            Style::Error => text(&mut canvas, font, [grid.x(0), y], &line.text, ERROR, false),
        }
    }
    Frame {
        origin: rect.origin,
        size: rect.size,
        target: layout.target,
        scale: layout.scale,
        rgba: canvas.rgba.into(),
    }
}

/// Draws `chars` from (`x`, `y`) in 1-bit glyphs; `bold` strikes each glyph
/// twice, one pixel apart.
fn text(canvas: &mut Canvas, font: &Font, [x, y]: [u32; 2], chars: &str, ink: Rgb, bold: bool) {
    let limit = canvas.width - 1;
    for (column, c) in chars.chars().enumerate() {
        let left = x + column as u32 * font.width;
        if left + font.width > limit {
            break;
        }
        for gy in 0..font.height {
            for gx in 0..font.width {
                if font.pixel(c, gx, gy) {
                    let wide = if bold { 2 } else { 1 };
                    canvas.fill([left + gx, y + gy, wide.min(limit - left - gx), 1], ink);
                }
            }
        }
    }
}

/// The header strip: the badge, the title in bold INK and a hairline rule
/// under the strip, across the container.
fn header(canvas: &mut Canvas, grid: &Grid, y: u32, title: &str) {
    let cell = grid.font.height as f32;
    let size = (cell * 0.75).round();
    widgets::badge(
        canvas,
        [grid.x(0) as f32, y as f32 + ((cell - size) / 2.).round()],
        size,
    );
    let title = fit(title, COLUMNS as usize - 2);
    text(canvas, grid.font, [grid.x(2), y], &title, INK, true);
    let rule = y + grid.font.height - 1;
    canvas.fill([1, rule, grid.width - 2, 1], HAIRLINE);
}

/// A row card inset in its line, the label left and the value right.
fn row(canvas: &mut Canvas, grid: &Grid, y: u32, line: &Line, focus: bool, locked: bool) {
    let font = grid.font;
    let u = grid.u;
    let tone = match (focus, locked) {
        (_, true) => Tone::Locked,
        (true, false) => Tone::Focused,
        (false, false) => Tone::Normal,
    };
    let gap = 2. * u;
    let card = [
        (grid.pad[0] / 2) as f32,
        y as f32 + gap,
        (grid.width - grid.pad[0]) as f32,
        font.height as f32 - 2. * gap,
    ];
    let radius = 6. * u;
    if focus && locked {
        // A locked row cannot take the focus fill: a thin ruby outline and a
        // marker at the card's left edge show the focus, the text stays muted.
        let ring = (2. * u).max(1.);
        canvas.rounded(card, radius, FOCUS);
        let [x, y, w, h] = card;
        let inner = [x + ring, y + ring, w - 2. * ring, h - 2. * ring];
        canvas.rounded(inner, radius - ring, CARD);
        let inset = 5. * u;
        let marker = [x + ring + u, y + inset, 3. * u, h - 2. * inset];
        canvas.rounded(marker, 1.5 * u, FOCUS);
    } else {
        let fill = if tone == Tone::Focused { FOCUS } else { CARD };
        canvas.rounded(card, radius, fill);
    }
    let (label_ink, value_ink) = widgets::inks(tone);
    let chars: Vec<char> = line.text.chars().collect();
    let split = chars.len().saturating_sub(line.value);
    let label: String = chars[..split].iter().collect::<String>().trim_end().into();
    let value: String = chars[split..].iter().collect();
    let right = grid.x(COLUMNS);
    let cy = y as f32 + font.height as f32 / 2.;
    // The first column the label may not reach.
    let label_end = match line.widget {
        _ if line.value == 0 => COLUMNS + 1,
        Widget::Switch(on) => {
            let left = widgets::switch(canvas, right as f32, cy, u, on, tone);
            ((left - grid.x(0) as f32) / font.width as f32).floor() as u32
        }
        Widget::Slider {
            value: at,
            min,
            max,
        } => {
            let shown = unwrap_chevrons(&value).unwrap_or(&value);
            let columns = shown.chars().count() as u32;
            text(
                canvas,
                font,
                [grid.x(COLUMNS - columns), y],
                shown,
                value_ink,
                false,
            );
            let start = COLUMNS.saturating_sub(columns + 1 + RAIL_COLUMNS);
            let span = (max - min).max(1) as f32;
            let fraction = (at - min) as f32 / span;
            let rail = [grid.x(start) as f32, (RAIL_COLUMNS * font.width) as f32];
            widgets::slider(canvas, rail, cy, u, fraction, tone);
            start
        }
        Widget::Choice if unwrap_chevrons(&value).is_some() => {
            let n = line.value as u32;
            let middle = |column: u32| (grid.x(column) + font.width / 2) as f32;
            widgets::chevron(canvas, [middle(COLUMNS - n), cy], u, false, value_ink);
            widgets::chevron(canvas, [middle(COLUMNS - 1), cy], u, true, value_ink);
            let inner = unwrap_chevrons(&value).unwrap_or_default();
            text(
                canvas,
                font,
                [grid.x(COLUMNS - n + 2), y],
                inner,
                value_ink,
                false,
            );
            COLUMNS - n
        }
        Widget::Link => {
            let middle = (grid.x(COLUMNS - 1) + font.width / 2) as f32;
            widgets::chevron(canvas, [middle, cy], u, true, value_ink);
            COLUMNS - 1
        }
        _ => {
            let n = line.value as u32;
            text(
                canvas,
                font,
                [grid.x(COLUMNS - n), y],
                &value,
                value_ink,
                false,
            );
            COLUMNS - n
        }
    };
    let room = label_end.saturating_sub(1) as usize;
    let label = if label.chars().count() > room && line.value > 0 {
        fit(&label, room)
    } else {
        label
    };
    text(canvas, font, [grid.x(0), y], &label, label_ink, false);
}

/// `value` without the model's `< ` and ` >`, if it has them.
fn unwrap_chevrons(value: &str) -> Option<&str> {
    value.strip_prefix("< ")?.strip_suffix(" >")
}

#[cfg(test)]
mod tests;
