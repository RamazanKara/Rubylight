//! PC cards, game covers and Home's continue cards: their painting, state lines and menus.

use super::*;

/// The narrowest a PC or continue card gets; cards share a row's width up to [`CARD_MAX_W`].
pub(super) const CARD_W: f32 = 300.0;
pub(super) const CARD_MAX_W: f32 = 420.0;
/// The gap between cards.
pub(super) const CARD_GAP: f32 = 16.0;
pub(super) const CARD_H: f32 = 112.0;
/// Home's cards: a game to continue, or a PC with one line of state.
pub(super) const COMPACT_CARD_H: f32 = 84.0;
/// The card's inner margin left and right of its status and detail lines.
pub(super) const CARD_PAD: f32 = 20.0;
/// The game page's cover. Grids size their covers to fill the row ([`grid`]), 3:4 like it.
pub(super) const POSTER_W: f32 = 168.0;
pub(super) const POSTER_H: f32 = 224.0;
/// The widest a cover in a grid gets: past it a row takes one more.
pub(super) const POSTER_MAX_W: f32 = 196.0;
/// The gap between covers in a grid.
pub(super) const POSTER_GAP: f32 = 18.0;
/// Room under a cover for its title.
pub(super) const POSTER_CAPTION: f32 = 30.0;
/// How far a pointed-at or focused cover rises (none with reduced motion). A grid at the top of a
/// scroll leaves this much room above it, so the risen cover's ring is not cut.
pub(super) const LIFT: f32 = 4.0;

/// How many covers fit a row `width` wide, and how wide each is: as many as fit at
/// [`POSTER_MAX_W`] or less, filling the row exactly.
pub(super) fn grid(width: f32) -> (usize, f32) {
    grid_up_to(width, POSTER_MAX_W)
}

/// [`grid`] with covers at most `widest` wide: Home's covers at its View's card size.
pub(super) fn grid_up_to(width: f32, widest: f32) -> (usize, f32) {
    let columns = ((width + POSTER_GAP) / (widest + POSTER_GAP)).ceil().max(1.0);
    (columns as usize, ((width - POSTER_GAP * (columns - 1.0)) / columns).floor())
}

/// How many cards fit a row `width` wide and how wide each is: at least [`CARD_W`], sharing the
/// row up to [`CARD_MAX_W`].
pub(super) fn card_grid(width: f32, gap: f32) -> (usize, f32) {
    let columns = ((width + gap) / (CARD_W + gap)).floor().max(1.0);
    (columns as usize, ((width - gap * (columns - 1.0)) / columns).floor().clamp(CARD_W.min(width), CARD_MAX_W))
}

/// What selecting a PC's card does: open its apps, check again, or pair.
/// The three dots on a PC's card: a second stop for focus that opens the options.
pub(super) fn options_button(ui: &mut Ui, card: Rect, id: Id) -> Response {
    let rect = Rect::from_min_size(card.right_top() + vec2(-42.0, 10.0), vec2(32.0, 32.0));
    let response = ui.interact(rect, id, Sense::click()).on_hover_text(tr("Options"));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Options")));
    if ui.is_rect_visible(rect) {
        let active = response.hovered() || response.has_focus();
        if active {
            let fill = if response.is_pointer_button_down_on() { SURFACE_BRIGHT } else { SURFACE_HIGHEST };
            ui.painter().rect_filled(rect, CornerRadius::same(16), fill);
        }
        let ink = if active { ON_SURFACE } else { ON_SURFACE_VARIANT };
        icons::paint(ui.painter(), Icon::Dots, Rect::from_center_size(rect.center(), Vec2::splat(20.0)), *ink);
        focus_ring(ui, rect, &response, 16);
    }
    response
}

/// The ⋮ over a cover while it is pointed at or focused: for the pointer only (the keyboard and
/// controller open the menu with F2 or X), so it adds no stop for focus.
pub(super) fn cover_dots(ui: &mut Ui, cover: &Response, image: Rect) -> Option<Response> {
    let rect = Rect::from_min_size(image.right_top() + vec2(-38.0, 8.0), vec2(30.0, 30.0));
    let id = cover.id.with("options");
    let shown = cover.has_focus() || cover.contains_pointer();
    if !shown {
        return None;
    }
    let response = ui.interact(rect, id, Sense::CLICK).on_hover_text(tr("Options"));
    let fill = if response.is_pointer_button_down_on() {
        Color32::from_black_alpha(230)
    } else if response.hovered() {
        Color32::from_black_alpha(200)
    } else {
        SCRIM
    };
    ui.painter().rect_filled(rect, CornerRadius::same(15), fill);
    icons::paint(ui.painter(), Icon::Dots, Rect::from_center_size(rect.center(), Vec2::splat(18.0)), Color32::WHITE);
    Some(response)
}

/// Where a menu asked for on a card opens, if one was: under its ⋮ when that was clicked, at the
/// pointer for a right-click, beside the card for F2, Shift+F10 or the controller's X.
pub(super) fn menu_request(ui: &Ui, card: &Response, dots: Option<&Response>) -> Option<Anchor> {
    if let Some(dots) = dots.filter(|d| d.clicked()) {
        return Some(Anchor::Below(dots.rect));
    }
    if card.secondary_clicked() || dots.is_some_and(Response::secondary_clicked) {
        let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or(card.rect.center());
        return Some(Anchor::Point(at));
    }
    let focused = card.has_focus() || dots.is_some_and(Response::has_focus);
    let keys = focused && ui.input(|i| i.key_pressed(Key::F2) || (i.modifiers.shift && i.key_pressed(Key::F10)));
    keys.then_some(Anchor::Beside(card.rect))
}

/// A PC's card. With `line` (Home: what it knows of the games) the card is as compact as Home's
/// game cards, and the line follows the state on one row instead of the address and GPU.
pub(super) fn host_card(ui: &mut Ui, host: &Host, line: Option<&str>, width: f32) -> Response {
    let height = if line.is_some() { COMPACT_CARD_H } else { CARD_H };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    card_fill(&painter, rect, &response);
    let (status, color, ring) = host_status(host);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{}, {status}", host.display_name())));
    // The status dot: full and green when the PC is up and paired, a ring otherwise. Amber stays the
    // action's colour, so it never stands for a state.
    let top = if line.is_some() { 28.0 } else { 30.0 };
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + top);
    if host.reach == Reach::Online && host.paired() {
        painter.circle_filled(dot, 7.0, OK);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, ring));
    }
    // A long name ends in "…" before the three dots.
    let mut name = egui::text::LayoutJob::simple_singleline(host.display_name().to_owned(), semibold(text::CARD_TITLE), *ON_SURFACE);
    name.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 44.0 - 46.0);
    let name = painter.layout_job(name);
    painter.galley(Pos2::new(rect.left() + 44.0, rect.top() + top - name.size().y / 2.0), name, *ON_SURFACE);
    if let Some(line) = line {
        // "Offline" twice said nothing more.
        let both = if line.is_empty() {
            status.clone()
        } else if line.starts_with(&status[..]) {
            line.to_owned()
        } else {
            format!("{status}  ·  {line}")
        };
        let cut = card_line(&painter, rect, 58.0, &both, regular(text::LABEL), *color);
        focus_ring(ui, rect, &response, radius::PANEL);
        return if cut { response.on_hover_text(both) } else { response };
    }
    let detail = match host.info.as_ref().and_then(|i| i.gpu_type.clone()) {
        Some(gpu) if !gpu.is_empty() => format!("{}  ·  {gpu}", host.record.address),
        _ => host.record.address.clone(),
    };
    // The state and the address with the GPU end in "…" inside the card's padding; whatever was cut
    // is in the tooltip.
    let status_cut = card_line(&painter, rect, 62.0, &status, regular(text::LABEL), *color);
    let detail_cut = card_line(&painter, rect, 88.0, &detail, regular(text::CAPTION), *OUTLINE);
    let tooltip = match (status_cut, detail_cut) {
        (false, false) => None,
        (true, false) => Some(status),
        (false, true) => Some(detail),
        (true, true) => Some(format!("{status}\n{detail}")),
    };
    focus_ring(ui, rect, &response, radius::PANEL);
    match tooltip {
        Some(text) => response.on_hover_text(text),
        None => response,
    }
}

/// A card's surface: one step lighter while pointed at or focused, two while pressed, the outline
/// high contrast gives every control.
pub(super) fn card_fill(painter: &egui::Painter, rect: Rect, response: &Response) {
    let fill = state_fill(SURFACE, response.hovered() || response.has_focus(), response.is_pointer_button_down_on());
    painter.rect(rect, CornerRadius::same(radius::PANEL), fill, control_stroke(), StrokeKind::Inside);
}

/// One line of a card at `y` from its top, cut with "…" at the card's padding. True when it was cut.
pub(super) fn card_line(painter: &egui::Painter, card: Rect, y: f32, text: &str, font: egui::FontId, color: Color32) -> bool {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(card.width() - 2.0 * CARD_PAD);
    let galley = painter.layout_job(job);
    let cut = galley.elided;
    painter.galley(Pos2::new(card.left() + CARD_PAD, card.top() + y - galley.size().y / 2.0), galley, color);
    cut
}

/// What a PC's card says of it: the words, their colour, and the colour of the dot's ring.
pub(super) fn host_status(host: &Host) -> (String, Role, Role) {
    match (host.reach, host.paired()) {
        (Reach::Offline, _) => (tr("Offline").to_owned(), OUTLINE, OUTLINE),
        (Reach::Unknown, _) => (tr("Checking…").to_owned(), ON_SURFACE_VARIANT, ON_SURFACE_VARIANT),
        (Reach::Online, false) => (tr("Ready to pair").to_owned(), TERTIARY, ON_SURFACE_VARIANT),
        (Reach::Online, true) => match host.running_app() {
            Some(_) => (tr("Paired · an app is running").to_owned(), OK, OK),
            None => (tr("Paired · ready").to_owned(), OK, OK),
        },
    }
}

/// A Home shelf's heading: its name and, for a library row, how many games it holds.
pub(super) fn shelf(ui: &mut Ui, name: &str, count: Option<usize>) {
    ui.add_space(space::XL);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        ui.label(RichText::new(name).font(semibold(text::CARD_TITLE)).color(ON_SURFACE));
        if let Some(count) = count {
            ui.label(RichText::new(count.to_string()).font(regular(text::BODY)).color(OUTLINE));
        }
    });
    ui.add_space(space::S);
}

/// The cover art, or the night surface with the banner's orbits when the PC has none.
/// The part of art of size `art` that fills a box of size `box_size` without stretching it: the
/// middle of it, cut at the sides or at top and bottom (CSS's `object-fit: cover`).
pub(super) fn cover_uv(art: Vec2, box_size: Vec2) -> Rect {
    if art.x <= 0.0 || art.y <= 0.0 || box_size.x <= 0.0 || box_size.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let (art_aspect, box_aspect) = (art.x / art.y, box_size.x / box_size.y);
    if art_aspect > box_aspect {
        let w = box_aspect / art_aspect;
        Rect::from_min_max(Pos2::new((1.0 - w) / 2.0, 0.0), Pos2::new((1.0 + w) / 2.0, 1.0))
    } else {
        let h = art_aspect / box_aspect;
        Rect::from_min_max(Pos2::new(0.0, (1.0 - h) / 2.0), Pos2::new(1.0, (1.0 + h) / 2.0))
    }
}

pub(super) fn paint_cover(ui: &Ui, image: Rect, name: &str, art: Option<&Art>, running: bool) {
    let painter = ui.painter_at(image.expand(6.0));
    match art {
        Some(Art::Ready(texture)) => {
            egui::Image::from_texture((texture.id(), image.size()))
                .uv(cover_uv(texture.size_vec2(), image.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, image);
        }
        _ => {
            painter.rect_filled(image, CornerRadius::same(10), SURFACE);
            let orbits = ui.painter_at(image.shrink(1.0));
            for r in [70.0, 105.0, 140.0] {
                orbits.circle_stroke(image.right_bottom(), r, Stroke::new(1.5, ORBIT));
            }
            // No art: the whole title, as Android's cover text, instead of initials ("C2" said nothing).
            let mut job = egui::text::LayoutJob::single_section(
                name.to_owned(),
                egui::TextFormat { font_id: semibold(text::CARD_TITLE), color: *ON_SURFACE_VARIANT, ..Default::default() },
            );
            job.halign = Align::Center;
            job.wrap = egui::text::TextWrapping { max_width: image.width() - 2.0 * space::L, max_rows: 4, break_anywhere: false, overflow_character: Some('…') };
            let galley = painter.layout_job(job);
            painter.galley(Pos2::new(image.center().x, image.center().y - galley.size().y / 2.0), galley, *ON_SURFACE_VARIANT);
        }
    }
    if running {
        running_badge(&painter, image.left_top() + vec2(space::S, space::S));
    }
}

/// "Running" over a cover: a green dot and white words on the scrim, which reads over any art
/// (the console's media pill). Returns where it went.
pub(super) fn running_badge(painter: &egui::Painter, at: Pos2) -> Rect {
    const DOT: f32 = 4.0;
    let words = painter.layout_no_wrap(tr("Running").to_owned(), semibold(text::CAPTION), Color32::WHITE);
    let height = space::XL;
    // The dot sits closer to the edge than the words do: it is lighter.
    let width = space::S + DOT * 2.0 + space::S + words.size().x + space::M;
    let badge = Rect::from_min_size(at, vec2(width, height));
    painter.rect_filled(badge, CornerRadius::same((height / 2.0) as u8), SCRIM);
    let dot = Pos2::new(badge.left() + space::S + DOT, badge.center().y);
    painter.circle_filled(dot, DOT, OK);
    let words_at = Pos2::new(dot.x + DOT + space::S, badge.center().y - words.size().y / 2.0);
    painter.galley(words_at, words, Color32::WHITE);
    badge
}

/// A game's state over its title, as Android's game page: a dot and the word in the state's
/// colour on a pill of the same hue.
pub(super) fn state_chip(ui: &mut Ui, label: &str, color: Role) {
    const DOT: f32 = 4.0;
    let color: Color32 = color.into();
    let words = ui.painter().layout_no_wrap(tr(label).to_owned(), semibold(text::CAPTION), color);
    let size = vec2(space::M + DOT * 2.0 + space::S + words.size().x + space::M, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(13), color.gamma_multiply(0.16));
    let dot = Pos2::new(rect.left() + space::M + DOT, rect.center().y);
    painter.circle_filled(dot, DOT, color);
    painter.galley(Pos2::new(dot.x + DOT + space::S, rect.center().y - words.size().y / 2.0), words, color);
    name_for_readers(ui, &response, tr(label), "");
    ui.add_space(space::XS);
}

/// The PC a game is on: a monitor and its name on their own line.
pub(super) fn pc_line(ui: &mut Ui, name: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        let ink: Color32 = ON_SURFACE_VARIANT.into();
        let stroke = Stroke::new(1.5, ink);
        let screen = Rect::from_min_max(rect.left_top() + vec2(1.0, 2.0), rect.right_top() + vec2(-1.0, 13.0));
        ui.painter().rect_stroke(screen, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
        let foot = rect.center_bottom() - vec2(0.0, 2.0);
        ui.painter().line_segment([Pos2::new(foot.x, screen.bottom()), foot], stroke);
        ui.painter().line_segment([foot - vec2(4.0, 0.0), foot + vec2(4.0, 0.0)], stroke);
        ui.label(RichText::new(name).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
    });
}

/// Delete, or Y on a gamepad (mapped to F3), takes the star off the focused favourite.
pub(super) fn unstar_pressed(ui: &Ui) -> bool {
    ui.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) | i.consume_key(egui::Modifiers::NONE, egui::Key::F3)
    })
}

/// What a cover says over its art.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Badge {
    None,
    Running,
    /// In the PC's library but not installed: the art dims under a "Not installed" pill.
    NotInstalled,
}

/// A game's cover, `width` wide at 3:4, its title on one line under it (a cover without art shows
/// the whole title inside instead). Pointed at or focused it rises a little and its title
/// brightens; focus adds the ring. Nothing about it changes size.
pub(super) fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, badge: Badge, width: f32) -> Response {
    let height = (width * 4.0 / 3.0).round();
    let (rect, response) = ui.allocate_exact_size(vec2(width, height + POSTER_CAPTION), Sense::click());
    let said = match badge {
        Badge::Running => trf("{}, running", &[&name]),
        Badge::NotInstalled => trf("{}, not installed", &[&name]),
        Badge::None => name.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said.clone()));
    if !ui.is_rect_visible(rect.expand(LIFT + 6.0)) {
        return response;
    }
    let active = response.has_focus() || response.contains_pointer();
    let lift = ui.ctx().animate_bool_responsive(response.id.with("lift"), active);
    let image = Rect::from_min_size(rect.min - vec2(0.0, lift * LIFT), vec2(width, height));
    if lift > 0.0 && !high_contrast() {
        // A soft shadow under the risen cover.
        let shadow = egui::Shadow { offset: [0, 8], blur: 18, spread: 0, color: Color32::from_black_alpha((110.0 * lift) as u8) };
        ui.painter().add(shadow.as_shape(image, CornerRadius::same(10)));
    }
    paint_cover(ui, image, name, art, badge == Badge::Running);
    if badge == Badge::NotInstalled {
        let painter = ui.painter_at(image);
        painter.rect_filled(image, CornerRadius::same(10), Color32::from_black_alpha(120));
        let words = painter.layout_no_wrap(tr("Not installed").to_owned(), semibold(text::CAPTION), Color32::WHITE);
        let pill = Rect::from_min_size(image.left_bottom() + vec2(space::S, -space::S - space::XL), vec2(words.size().x + 2.0 * space::M, space::XL));
        painter.rect_filled(pill, CornerRadius::same((space::XL / 2.0) as u8), SCRIM);
        painter.galley(pill.center() - words.size() / 2.0, words, Color32::WHITE);
    }
    if high_contrast() {
        ui.painter().rect_stroke(image, CornerRadius::same(10), control_stroke(), StrokeKind::Inside);
    }
    if poster_caption(art) {
        let color = if active { ON_SURFACE } else { ON_SURFACE_VARIANT };
        let at = Pos2::new(rect.left() + 2.0, rect.top() + height + space::S);
        let room = width - 4.0;
        let full = ui.painter().layout_no_wrap(name.to_owned(), semibold(text::LABEL), *color);
        match caption_offset(full.size().x, room, response.has_focus(), ui.style().animation_time > 0.0, ui.input(|i| i.time)) {
            // A focused cover's title that does not fit runs sideways on its line, the art kept.
            Some((offset, span)) => {
                let painter = ui.painter_at(Rect::from_min_size(at, vec2(room, full.size().y)));
                painter.galley(at - vec2(offset, 0.0), full.clone(), *color);
                painter.galley(at + vec2(span - offset, 0.0), full, *color);
                ui.ctx().request_repaint();
            }
            None => {
                let mut job = egui::text::LayoutJob::simple_singleline(name.to_owned(), semibold(text::LABEL), *color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(at, galley, *color);
            }
        }
    }
    focus_ring(ui, image, &response, 10);
    response
}

/// How far a cover's title has run sideways `time` seconds in, and the distance from one copy of
/// it to the next: only while the cover has focus, motion is on and the title is wider than its
/// `room`. Each pass waits a moment at the start so the beginning can be read. None draws the
/// title cut short as usual.
pub(super) fn caption_offset(text_w: f32, room: f32, focused: bool, motion: bool, time: f64) -> Option<(f32, f32)> {
    const SPEED: f32 = 36.0;
    const PAUSE: f32 = 1.2;
    const GAP: f32 = 32.0;
    if !focused || !motion || text_w <= room {
        return None;
    }
    let span = text_w + GAP;
    let pass = PAUSE + span / SPEED;
    let phase = (time as f32).rem_euclid(pass);
    Some((((phase - PAUSE).max(0.0) * SPEED).min(span), span))
}

/// The last tile of a Home row with more games than fit: how many there are, and the way to see
/// them all.
pub(super) fn all_tile(ui: &mut Ui, count: usize, said: &str, width: f32) -> Response {
    let height = (width * 4.0 / 3.0).round();
    let (rect, response) = ui.allocate_exact_size(vec2(width, height + POSTER_CAPTION), Sense::click());
    let label = trf("All {}", &[&count]);
    // "All 12" on the tile; a screen reader hears whose games: "All Steam games, 12".
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said));
    let tile = Rect::from_min_size(rect.min, vec2(width, height));
    let active = response.hovered() || response.has_focus();
    let fill = state_fill(SURFACE, active, response.is_pointer_button_down_on());
    ui.painter().rect(tile, CornerRadius::same(10), fill, control_stroke(), StrokeKind::Inside);
    let ink = if active { ON_SURFACE } else { ON_SURFACE_VARIANT };
    icons::paint(ui.painter(), Icon::Grid, Rect::from_center_size(tile.center() - vec2(0.0, 18.0), Vec2::splat(32.0)), *ink);
    let words = ui.painter().layout_no_wrap(label, semibold(text::CARD_TITLE), *ink);
    ui.painter().galley(Pos2::new(tile.center().x - words.size().x / 2.0, tile.center().y + 10.0), words, *ink);
    focus_ring(ui, tile, &response, 10);
    response
}

/// A line of Home's List layout.
pub(super) const LIST_ROW_H: f32 = 88.0;

/// A game as a line of Home's List layout: a small cover, the title, and under it `line` (its
/// store and when it was last played).
pub(super) fn list_row(ui: &mut Ui, name: &str, line: &str, art: Option<&Art>, badge: Badge, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, LIST_ROW_H), Sense::click());
    let said = match badge {
        Badge::Running => trf("{}, running", &[&name]),
        Badge::NotInstalled => trf("{}, not installed", &[&name]),
        Badge::None => name.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{said}, {line}")));
    if !ui.is_rect_visible(rect.expand(6.0)) {
        return response;
    }
    let painter = ui.painter_at(rect.expand(6.0));
    card_fill(&painter, rect, &response);
    let cover_h = LIST_ROW_H - 2.0 * space::S;
    let image = Rect::from_min_size(rect.min + vec2(space::S, space::S), vec2((cover_h * 3.0 / 4.0).round(), cover_h));
    paint_cover(ui, image, name, art, badge == Badge::Running);
    if badge == Badge::NotInstalled {
        ui.painter_at(image).rect_filled(image, CornerRadius::same(6), Color32::from_black_alpha(120));
    }
    let words = Rect::from_min_max(Pos2::new(image.right() + space::M - CARD_PAD, rect.top()), rect.max);
    let inside = ui.painter_at(rect);
    card_line(&inside, words, LIST_ROW_H / 2.0 - 12.0, name, semibold(text::CARD_TITLE), *ON_SURFACE);
    let below = if badge == Badge::NotInstalled { format!("{line} · {}", tr("Not installed")) } else { line.to_owned() };
    card_line(&inside, words, LIST_ROW_H / 2.0 + 14.0, &below, regular(text::LABEL), *ON_SURFACE_VARIANT);
    focus_ring(ui, rect, &response, radius::PANEL);
    response
}

/// Whether a cover names its game underneath: only over art, as a cover without art shows the
/// full title inside already.
pub(super) fn poster_caption(art: Option<&Art>) -> bool {
    matches!(art, Some(Art::Ready(_)))
}

/// A game to carry on with: title, and where it runs.
pub(super) fn continue_card(ui: &mut Ui, title: &str, line: &str, running: bool, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, COMPACT_CARD_H), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{title}, {line}")));
    let painter = ui.painter_at(rect.expand(6.0));
    card_fill(&painter, rect, &response);
    let color = if running { OK } else { TERTIARY };
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + 28.0);
    if running {
        painter.circle_filled(dot, 7.0, color);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, color));
    }
    let inside = ui.painter_at(rect);
    let mut job = egui::text::LayoutJob::simple_singleline(title.to_owned(), semibold(text::CARD_TITLE), *ON_SURFACE);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 44.0 - CARD_PAD);
    let galley = inside.layout_job(job);
    inside.galley(Pos2::new(rect.left() + 44.0, rect.top() + 28.0 - galley.size().y / 2.0), galley, *ON_SURFACE);
    card_line(&inside, rect, 58.0, line, regular(text::LABEL), *color);
    focus_ring(ui, rect, &response, radius::PANEL);
    response
}

/// Asks for a cover when it is on screen and the PC can send it.
pub(super) fn request_cover(
    ui: &Ui,
    view: &View<'_>,
    host: u64,
    app: u32,
    version: &str,
    response: &Response,
    commands: &mut Vec<Command>,
) {
    let wanted = view
        .hosts
        .get(host)
        .is_some_and(|h| h.reach == Reach::Online && h.art_wanted(app as i32, version));
    if wanted && ui.is_rect_visible(response.rect) {
        commands.push(Command::RequestArt { host, app: app as i32, version: version.to_owned() });
    }
}
