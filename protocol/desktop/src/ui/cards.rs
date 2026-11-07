//! PC cards, game covers and Home's continue cards: their painting, state lines and menus.

use super::design_tokens as t;
use super::*;
use egui::Shape;

/// The narrowest a PC or continue card gets; cards share a row's width up to [`CARD_MAX_W`].
pub(super) const CARD_W: f32 = 300.0;
pub(super) const CARD_MAX_W: f32 = 420.0;
/// The gap between cards.
pub(super) const CARD_GAP: f32 = 16.0;
/// Home's cards: a game to continue, or a PC with one line of state.
pub(super) const COMPACT_CARD_H: f32 = 84.0;
/// The card's inner margin left and right of its status and detail lines.
pub(super) const CARD_PAD: f32 = 20.0;
/// The widest a cover in a grid gets: past it a row takes one more.
pub(super) const POSTER_MAX_W: f32 = 170.0;
/// The gap between covers in a grid.
pub(super) const POSTER_GAP: f32 = 16.0;
/// The gap between the rows of a grid of bare covers.
pub(super) const GRID_ROW_GAP: f32 = 18.0;
/// Room under a cover for its title: the gap and one line.
pub(super) const POSTER_CAPTION: f32 = 27.0;
/// How far above its slot a focused cover's ring reaches. A grid at the top of a scroll leaves this
/// much room above it, so the risen cover's ring is not cut.
pub(super) const LIFT: f32 = 16.0;

/// A cover's height at `width`: 2:3, the same whatever it shows, so nothing moves as art arrives.
pub(super) fn cover_height(width: f32) -> f32 {
    (width / t::COVER_ASPECT).floor()
}

/// The two colours of no-art gradient `pair`, top and bottom.
pub(super) fn tile_colors(pair: usize) -> (Color32, Color32) {
    const PAIRS: [(Color32, Color32); 12] = [
        (t::TILE_PAIR0_TOP, t::TILE_PAIR0_BOTTOM),
        (t::TILE_PAIR1_TOP, t::TILE_PAIR1_BOTTOM),
        (t::TILE_PAIR2_TOP, t::TILE_PAIR2_BOTTOM),
        (t::TILE_PAIR3_TOP, t::TILE_PAIR3_BOTTOM),
        (t::TILE_PAIR4_TOP, t::TILE_PAIR4_BOTTOM),
        (t::TILE_PAIR5_TOP, t::TILE_PAIR5_BOTTOM),
        (t::TILE_PAIR6_TOP, t::TILE_PAIR6_BOTTOM),
        (t::TILE_PAIR7_TOP, t::TILE_PAIR7_BOTTOM),
        (t::TILE_PAIR8_TOP, t::TILE_PAIR8_BOTTOM),
        (t::TILE_PAIR9_TOP, t::TILE_PAIR9_BOTTOM),
        (t::TILE_PAIR10_TOP, t::TILE_PAIR10_BOTTOM),
        (t::TILE_PAIR11_TOP, t::TILE_PAIR11_BOTTOM),
    ];
    PAIRS[pair % PAIRS.len()]
}

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

/// Where a cover's ⋮ goes on `shown`, the cover as it stands this frame. The art's logotype lives at
/// the top and the running dot at the bottom right, so it takes the bottom left; only the Not installed
/// pill is there, and a cover with that one has no running dot.
pub(super) fn dots_rect(shown: Rect, badge: Badge) -> Rect {
    const SIZE: f32 = 30.0;
    const INSET: f32 = 8.0;
    let y = shown.bottom() - INSET - SIZE;
    let x = match badge {
        Badge::NotInstalled => shown.right() - INSET - SIZE,
        Badge::None | Badge::Running => shown.left() + INSET,
    };
    Rect::from_min_size(pos2(x, y), Vec2::splat(SIZE))
}

/// The ⋮ over a cover while it is pointed at or focused: for the pointer only (the keyboard and
/// controller open the menu with F2 or X), so it adds no stop for focus. It stays on the risen cover.
pub(super) fn cover_dots(ui: &mut Ui, cover: &Response, image: Rect, badge: Badge) -> Option<Response> {
    let id = cover.id.with("options");
    let shown = ui.ctx().data(|d| d.get_temp::<Rect>(cover.id.with("shown"))).unwrap_or(image);
    let rect = dots_rect(shown, badge);
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
    // Over the raised cover, which paints after the page.
    let mut shapes = vec![Shape::circle_filled(rect.center(), 15.0, fill)];
    shapes.extend(icons::shapes(Icon::Dots, Rect::from_center_size(rect.center(), Vec2::splat(18.0)), Color32::WHITE));
    glass::defer(ui, shapes);
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

/// A PC's card on Home: as compact as Home's game cards, its state and what it knows of the games on
/// one line.
pub(super) fn host_card(ui: &mut Ui, host: &Host, line: &str, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, COMPACT_CARD_H), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    card_fill(&painter, rect, &response);
    let (status, color, ring) = host_status(host);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{}, {status}", host.display_name())));
    // The status dot: full and green when the PC is up and paired, a ring otherwise. Amber stays the
    // action's colour, so it never stands for a state.
    let top = 28.0;
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + top);
    if host.reach == Reach::Online && host.paired() {
        painter.circle_filled(dot, 7.0, OK);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, ring));
    }
    // A long name ends in "\u{2026}" before the three dots.
    let mut name = egui::text::LayoutJob::simple_singleline(host.display_name().to_owned(), semibold(text::CARD_TITLE), *ON_SURFACE);
    name.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 44.0 - 46.0);
    let name = painter.layout_job(name);
    painter.galley(Pos2::new(rect.left() + 44.0, rect.top() + top - name.size().y / 2.0), name, *ON_SURFACE);
    // "Offline" twice said nothing more.
    let both = if line.is_empty() {
        status.clone()
    } else if line.starts_with(&status[..]) {
        line.to_owned()
    } else {
        format!("{status}  \u{b7}  {line}")
    };
    let cut = card_line(&painter, rect, 58.0, &both, regular(text::LABEL), *color);
    if response.has_focus() {
        // Past the scroll area's edge when the card sits at it, as every focused thing does.
        let mut halo = Vec::new();
        glass::focus(&mut halo, ui.ctx(), rect, f32::from(radius::PANEL), false);
        glass::defer_focused(ui, halo);
    }
    if cut {
        response.on_hover_text(both)
    } else {
        response
    }
}

/// A card's surface: one step lighter while pointed at or focused, two while pressed, the outline
/// high contrast gives every control.
pub(super) fn card_fill(painter: &egui::Painter, rect: Rect, response: &Response) {
    let ctx = painter.ctx();
    let r = f32::from(radius::PANEL);
    let mut shapes = Vec::new();
    glass::group(&mut shapes, ctx, rect, r, group_fill());
    lift_shapes(&mut shapes, ctx, rect, r, response.hovered(), response.is_pointer_button_down_on());
    painter.extend(shapes);
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

/// A cover that shapes itself: the shadow, then the art, or the placeholder while art is on its way,
/// or the no-art tile; then the sheen and the rim. `text_w` is the width its tile's title is set for,
/// so a cover springing bigger does not re-set its words every frame.
#[allow(clippy::too_many_arguments)]
pub(super) fn cover_shapes(ui: &Ui, out: &mut Vec<Shape>, rect: Rect, text_w: f32, r: f32, title: &str, art: Option<&Art>, rim_boost: f32) {
    let ctx = ui.ctx();
    if !high_contrast() {
        glass::soft(out, ctx, rect.translate(vec2(0.0, t::COVER_SHADOW_Y)), r, t::COVER_SHADOW_BLUR, t::COVER_SHADOW);
    }
    match art {
        Some(Art::Ready(texture)) => {
            // Over its placeholder when it arrives while the cover is on screen; a cover that was
            // ready when it first showed is there at once.
            let seconds = if ui.style().animation_time > 0.0 { t::MOTION_COVER_IN_MS as f32 / 1000.0 } else { 0.0 };
            let reveal = ctx.animate_bool_with_time(Id::unique(("cover-in", title)), true, seconds);
            if reveal < 1.0 {
                glass::solid(out, ctx, rect, r, t::COVER_PLACEHOLDER);
            }
            let tint = Color32::WHITE.gamma_multiply(reveal);
            glass::image(out, ctx, rect, r, texture.id(), cover_uv(texture.size_vec2(), rect.size()), tint);
        }
        Some(Art::Loading) => {
            ctx.animate_bool_with_time(Id::unique(("cover-in", title)), false, 0.0);
            glass::solid(out, ctx, rect, r, t::COVER_PLACEHOLDER);
        }
        Some(Art::Missing) | None => tile_shapes(ui, out, rect, text_w, r, title),
    }
    glass::cover_finish(out, ctx, rect, r, rim_boost);
}

/// A game without a cover: its own gradient and disc, the same on every client, with its title in it.
fn tile_shapes(ui: &Ui, out: &mut Vec<Shape>, rect: Rect, text_w: f32, r: f32, title: &str) {
    use nightjar_core::tile::{tile_disc_center, tile_pair, title_hash};
    let ctx = ui.ctx();
    let hash = title_hash(title);
    let (top, bottom) = tile_colors(tile_pair(hash));
    let fill = glass::gradient(rect, 165.0, &[(0.0, top), (1.0, bottom)]);
    glass::shaded(out, ctx, rect, r, &[], &fill);
    // The disc is cut off at the tile's sides, never near its round corners.
    let [cx, cy] = tile_disc_center(hash);
    let center = pos2(rect.left() + rect.width() * cx, rect.top() + rect.height() * cy);
    let radius = rect.width() * t::TILE_DISC_SCALE / 2.0;
    let edge: Vec<Pos2> = (0..64)
        .map(|i| {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            pos2((center.x + radius * a.cos()).clamp(rect.left(), rect.right()), (center.y + radius * a.sin()).clamp(rect.top(), rect.bottom()))
        })
        .collect();
    out.push(Shape::convex_polygon(edge, Color32::WHITE.gamma_multiply(t::TILE_DISC_ALPHA), Stroke::NONE));
    let inset = text_w * t::TILE_INSET_SCALE;
    let room = text_w - 2.0 * inset;
    // The title in up to three lines, one step smaller before anything is cut.
    let set = |scale: f32| {
        let size = (text_w * scale).max(8.0);
        let format = egui::TextFormat {
            font_id: semibold(size),
            color: Color32::WHITE,
            line_height: Some(size * 1.08),
            extra_letter_spacing: -0.01 * size,
            ..Default::default()
        };
        let mut job = egui::text::LayoutJob::single_section(title.to_owned(), format);
        job.wrap = egui::text::TextWrapping { max_width: room, max_rows: 3, break_anywhere: false, overflow_character: Some('\u{2026}') };
        ui.painter().layout_job(job)
    };
    // A word is never cut in two: the longest sets the size when it would not fit a line.
    let measured = (text_w * t::TILE_TITLE_SCALE).max(8.0);
    let longest = title.split_whitespace().map(|word| ui.painter().layout_no_wrap(word.to_owned(), semibold(measured), Color32::WHITE).size().x).fold(0.0, f32::max);
    let fits = if longest > room { t::TILE_TITLE_SCALE * room / longest * 0.98 } else { t::TILE_TITLE_SCALE };
    let mut galley = set(fits);
    if galley.elided {
        galley = set(fits.min(t::TILE_TITLE_MIN_SCALE));
    }
    let shift = (rect.width() - text_w) / 2.0;
    out.push(Shape::galley(rect.left_top() + vec2(inset + shift, inset + shift), galley, Color32::WHITE));
}

/// A running game's mark on its cover: a green dot on a dark disc at the bottom right.
pub(super) fn running_dot(out: &mut Vec<Shape>, cover: Rect) {
    let center = cover.right_bottom() - Vec2::splat(18.0);
    out.push(Shape::circle_filled(center, 10.0, SCRIM));
    out.push(Shape::circle_filled(center, 5.0, *OK));
}

/// A cover outside a grid: the game page's and a List line's. It takes no focus of its own.
pub(super) fn paint_cover(ui: &Ui, image: Rect, name: &str, art: Option<&Art>, running: bool) {
    let r = if image.width() < 80.0 { t::RADIUS_TILE } else { t::RADIUS_COVER };
    let mut shapes = Vec::new();
    cover_shapes(ui, &mut shapes, image, image.width(), r, name, art, 1.0);
    if running {
        running_dot(&mut shapes, image);
    }
    ui.painter().extend(shapes);
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

/// A game's cover, `width` wide at 2:3, its title on one line under it (a tile without art holds
/// the whole title instead). Pointed at it rises a little and its rim brightens; focus springs it
/// bigger, over its neighbours, with the ring. Its slot never changes size.
pub(super) fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, badge: Badge, width: f32) -> Response {
    cover_slot(ui, name, art, badge, width, true)
}

/// A cover of an app grid: no label, as its art carries its title and so does a tile. Until the art
/// comes the placeholder holds the name; a screen reader always hears it.
pub(super) fn bare_poster(ui: &mut Ui, name: &str, art: Option<&Art>, badge: Badge, width: f32) -> Response {
    cover_slot(ui, name, art, badge, width, false)
}

/// A place in a grid while the list is on its way: where a cover will land, so nothing moves.
pub(super) fn cover_placeholder(ui: &mut Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, cover_height(width)), Sense::hover());
    let mut shapes = Vec::new();
    cover_shapes(ui, &mut shapes, rect, width, t::RADIUS_COVER, "", Some(&Art::Loading), 1.0);
    ui.painter().extend(shapes);
}

/// The placeholder's words: a game's name until its art says it.
fn placeholder_title(ui: &Ui, out: &mut Vec<Shape>, rect: Rect, name: &str) {
    let inset = rect.width() * t::TILE_INSET_SCALE;
    let mut job = egui::text::LayoutJob::simple(name.to_owned(), semibold(text::LABEL), *ON_SURFACE_VARIANT, rect.width() - 2.0 * inset);
    job.wrap.max_rows = 3;
    job.wrap.overflow_character = Some('\u{2026}');
    let galley = ui.painter().layout_job(job);
    out.push(Shape::galley(rect.left_top() + Vec2::splat(inset), galley, *ON_SURFACE_VARIANT));
}

fn cover_slot(ui: &mut Ui, name: &str, art: Option<&Art>, badge: Badge, width: f32, captioned: bool) -> Response {
    let height = cover_height(width);
    let caption = if captioned { POSTER_CAPTION } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height + caption), Sense::click());
    let said = match badge {
        Badge::Running => trf("{}, running", &[&name]),
        Badge::NotInstalled => trf("{}, not installed", &[&name]),
        Badge::None => name.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said.clone()));
    if !ui.is_rect_visible(rect.expand(LIFT + 6.0)) {
        return response;
    }
    let ctx = ui.ctx().clone();
    let feel = glass::feel(&ctx, &response, t::FOCUS_SCALE_COVER, t::HOVER_SCALE_COVER);
    let image = Rect::from_min_size(rect.min, vec2(width, height));
    // It rises from its foot: the name under it stays clear.
    let shown = glass::risen_rect(image, feel.scale);
    ctx.data_mut(|d| d.insert_temp(response.id.with("shown"), shown));
    let boost = if feel.hovered || feel.focused { t::HOVER_RIM_BOOST } else { 1.0 };
    let mut shapes = Vec::new();
    cover_shapes(ui, &mut shapes, shown, width, t::RADIUS_COVER, name, art, boost);
    if !captioned && matches!(art, Some(Art::Loading)) {
        placeholder_title(ui, &mut shapes, shown, name);
    }
    match badge {
        Badge::None => {}
        Badge::Running => running_dot(&mut shapes, shown),
        Badge::NotInstalled => {
            glass::solid(&mut shapes, &ctx, shown, t::RADIUS_COVER, Color32::from_black_alpha(120));
            let words = ui.painter().layout_no_wrap(tr("Not installed").to_owned(), semibold(text::CAPTION), Color32::WHITE);
            let pill = Rect::from_min_size(shown.left_bottom() + vec2(space::S, -space::S - space::XL), vec2(words.size().x + 2.0 * space::M, space::XL));
            glass::solid(&mut shapes, &ctx, pill, space::XL / 2.0, SCRIM);
            shapes.push(Shape::galley(pill.center() - words.size() / 2.0, words, Color32::WHITE));
        }
    }
    if feel.focused {
        glass::focus(&mut shapes, &ctx, shown, t::RADIUS_COVER, false);
    }
    // A cover at rest paints in its place; a raised one goes over its neighbours.
    if feel.focused {
        glass::defer_focused(ui, shapes);
    } else if (feel.scale - 1.0).abs() > 0.001 {
        glass::defer(ui, shapes);
    } else {
        ui.painter().extend(shapes);
    }
    if captioned && poster_caption(art) {
        let active = response.has_focus() || response.contains_pointer();
        let color = if active { ON_SURFACE } else { ON_SURFACE_VARIANT };
        let at = Pos2::new(rect.left() + 2.0, rect.top() + height + space::S);
        let room = width - 4.0;
        let full = ui.painter().layout_no_wrap(name.to_owned(), regular(text::LABEL), *color);
        match caption_offset(full.size().x, room, response.has_focus(), ui.style().animation_time > 0.0, ui.input(|i| i.time)) {
            // A focused cover's title that does not fit runs sideways on its line, the art kept.
            Some((offset, span)) => {
                let painter = ui.painter_at(Rect::from_min_size(at, vec2(room, full.size().y)));
                painter.galley(at - vec2(offset, 0.0), full.clone(), *color);
                painter.galley(at + vec2(span - offset, 0.0), full, *color);
                ui.ctx().request_repaint();
            }
            None => {
                let mut job = egui::text::LayoutJob::simple_singleline(name.to_owned(), regular(text::LABEL), *color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(at, galley, *color);
            }
        }
    }
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
    let height = cover_height(width);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height + POSTER_CAPTION), Sense::click());
    let label = trf("All {}", &[&count]);
    // "All 12" on the tile; a screen reader hears whose games: "All Steam games, 12".
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, said));
    let ctx = ui.ctx().clone();
    let feel = glass::feel(&ctx, &response, t::FOCUS_SCALE_COVER, t::HOVER_SCALE_COVER);
    let tile = glass::scaled_rect(Rect::from_min_size(rect.min, vec2(width, height)), feel.scale);
    let mut shapes = Vec::new();
    glass::group(&mut shapes, &ctx, tile, t::RADIUS_COVER, t::TINT_GROUP_FILL);
    lift_shapes(&mut shapes, &ctx, tile, t::RADIUS_COVER, feel.hovered, feel.pressed);
    let ink = if feel.hovered || feel.focused { *ON_SURFACE } else { *ON_SURFACE_VARIANT };
    let words = ui.painter().layout_no_wrap(label, semibold(text::BODY), ink);
    let total = 28.0 + space::S + words.size().y;
    let top = tile.center().y - total / 2.0;
    shapes.extend(icons::shapes(Icon::Grid, Rect::from_center_size(pos2(tile.center().x, top + 14.0), Vec2::splat(28.0)), ink));
    shapes.push(Shape::galley(pos2(tile.center().x - words.size().x / 2.0, top + 28.0 + space::S), words, ink));
    if feel.focused {
        glass::focus(&mut shapes, &ctx, tile, t::RADIUS_COVER, false);
    }
    if feel.focused {
        glass::defer_focused(ui, shapes);
    } else if (feel.scale - 1.0).abs() > 0.001 {
        glass::defer(ui, shapes);
    } else {
        ui.painter().extend(shapes);
    }
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
    let image = Rect::from_min_size(rect.min + vec2(space::S, space::S), vec2((cover_h * t::COVER_ASPECT).round(), cover_h));
    paint_cover(ui, image, name, art, badge == Badge::Running);
    if badge == Badge::NotInstalled {
        ui.painter_at(image).rect_filled(image, CornerRadius::same(t::RADIUS_TILE as u8), Color32::from_black_alpha(120));
    }
    let words = Rect::from_min_max(Pos2::new(image.right() + space::M - CARD_PAD, rect.top()), rect.max);
    let inside = ui.painter_at(rect);
    card_line(&inside, words, LIST_ROW_H / 2.0 - 12.0, name, semibold(text::CARD_TITLE), *ON_SURFACE);
    let below = if badge == Badge::NotInstalled { format!("{line} · {}", tr("Not installed")) } else { line.to_owned() };
    card_line(&inside, words, LIST_ROW_H / 2.0 + 14.0, &below, regular(text::LABEL), *ON_SURFACE_VARIANT);
    if response.has_focus() {
        let mut halo = Vec::new();
        glass::focus(&mut halo, ui.ctx(), rect, f32::from(radius::PANEL), false);
        glass::defer_focused(ui, halo);
    }
    response
}

/// Whether a cover names its game underneath: art and its placeholder say nothing of it, a tile
/// holds the full title already.
pub(super) fn poster_caption(art: Option<&Art>) -> bool {
    matches!(art, Some(Art::Ready(_) | Art::Loading))
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
