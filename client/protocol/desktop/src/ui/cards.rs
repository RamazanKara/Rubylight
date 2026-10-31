//! PC cards, game covers and Home's continue cards: their painting, state lines and menus.

use super::*;

pub(super) const CARD_W: f32 = 300.0;
pub(super) const CARD_H: f32 = 112.0;
/// Home's cards: a game to continue, or a PC with one line of state.
pub(super) const COMPACT_CARD_H: f32 = 84.0;
/// The card's inner margin left and right of its status and detail lines.
pub(super) const CARD_PAD: f32 = 20.0;
pub(super) const POSTER_W: f32 = 168.0;
pub(super) const POSTER_H: f32 = 224.0;
/// Room under a cover for its title, which shows while it has focus.
pub(super) const POSTER_CAPTION: f32 = 28.0;

/// What selecting a PC's card does: open its apps, check again, or pair.
/// The three dots on a PC's card: a second stop for focus that opens the options.
pub(super) fn options_button(ui: &mut Ui, card: Rect, id: Id) -> Response {
    let rect = Rect::from_min_size(card.right_top() + vec2(-42.0, 10.0), vec2(32.0, 32.0));
    let response = ui.interact(rect, id, Sense::click()).on_hover_text(tr("Options"));
    if ui.is_rect_visible(rect) {
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, CornerRadius::same(16), SURFACE_HIGHEST);
        }
        for dy in [-6.0, 0.0, 6.0] {
            ui.painter().circle_filled(rect.center() + vec2(0.0, dy), 2.0, ON_SURFACE_VARIANT);
        }
        focus_ring(ui, rect, &response, 16);
    }
    response
}

/// Right-click, F2 or Shift+F10 on the card or its dots.
pub(super) fn options_requested(ui: &Ui, responses: &[&Response]) -> bool {
    responses.iter().any(|r| r.secondary_clicked())
        || (responses.iter().any(|r| r.has_focus())
            && ui.input(|i| i.key_pressed(Key::F2) || (i.modifiers.shift && i.key_pressed(Key::F10))))
}

/// A PC's card. With `line` (Home: what it knows of the games) the card is as compact as Home's
/// game cards, and the line follows the state on one row instead of the address and GPU.
pub(super) fn host_card(ui: &mut Ui, host: &Host, line: Option<&str>) -> Response {
    let height = if line.is_some() { COMPACT_CARD_H } else { CARD_H };
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, height), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() {
        SURFACE_HIGH
    } else {
        SURFACE
    };
    painter.rect_filled(rect, CornerRadius::same(radius::PANEL), fill);
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

/// The right-click menu of a game card: star the game or take its star off.
pub(super) fn favourite_menu(
    response: &Response,
    sources: &[Source<'_>],
    favourites: &[crate::config::Favourite],
    host: u64,
    app: u32,
    commands: &mut Vec<Command>,
) {
    let Some(star) = console::favourite_for(sources, host, app) else { return };
    let starred = favourites.iter().any(|f| f.same_game(&star));
    response.context_menu(|ui| {
        if ui.button(if starred { tr("Remove from favourites") } else { tr("Add to favourites") }).clicked() {
            commands.push(Command::ToggleFavourite(star));
            ui.close();
        }
    });
}

/// A game's cover. Its title shows on one line under it only while it has focus or the pointer:
/// cover art carries the title already, and a cover without art shows it inside. The focused cover
/// lifts a little (not with reduced motion).
pub(super) fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H + POSTER_CAPTION), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if running { trf("{}, running", &[&name]) } else { name.to_owned() }));
    let lifted = response.has_focus() || response.hovered();
    let lift = ui.ctx().animate_bool_responsive(response.id.with("lift"), lifted);
    let image = Rect::from_min_size(rect.min, vec2(POSTER_W, POSTER_H)).expand(lift * 3.0);
    paint_cover(ui, image, name, art, running);
    if lifted && poster_caption(art) {
        let painter = ui.painter_at(rect.expand(6.0));
        let color = if response.has_focus() { MOON } else { ON_SURFACE };
        let mut job = egui::text::LayoutJob::simple_singleline(name.to_owned(), semibold(text::LABEL), *color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(POSTER_W);
        let galley = painter.layout_job(job);
        painter.galley(Pos2::new(rect.left() + 2.0, image.bottom() + space::S), galley, *color);
    }
    focus_ring(ui, image, &response, 10);
    response
}

/// Whether a lifted cover names its game underneath: only over art, as a cover without art shows
/// the full title inside already.
pub(super) fn poster_caption(art: Option<&Art>) -> bool {
    matches!(art, Some(Art::Ready(_)))
}

/// A game to carry on with: title, and where it runs.
pub(super) fn continue_card(ui: &mut Ui, title: &str, line: &str, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, COMPACT_CARD_H), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{title}, {line}")));
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() { SURFACE_HIGH } else { SURFACE };
    painter.rect_filled(rect, CornerRadius::same(radius::PANEL), fill);
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
