//! The chrome over every page: the brand, the destinations in a capsule with a springing pill, the
//! search field and the PC capsule. Also the shared hero controls: solid Play, tinted capsules and
//! the chips above a title. One hover and focus language throughout.

use super::design_tokens as t;
use super::*;
use super::{glass, spring};
use egui::Shape;

/// The capsules' top edge and height.
pub(super) const BAR_TOP: f32 = 20.0;
pub(super) const BAR_H: f32 = t::SIZE_TOP_CAPSULE_HEIGHT;
/// Where a page's content starts: Home's hero, or a page's own first line.
pub(super) const PAGE_TOP: f32 = 88.0;
/// How far the page has scrolled when the scrim under the chrome is whole, and how dark it is.
const SCRIM_FULL_AT: f32 = 40.0;
const SCRIM_ALPHA: f32 = 0.88;
const CAPSULE_GAP: f32 = 14.0;
const SEARCH_W: f32 = 220.0;
/// Below this window width the search field folds to its icon, below the second the wordmark goes.
const SEARCH_FOLDS_BELOW: f32 = 1120.0;
const WORDMARK_GOES_BELOW: f32 = 900.0;
const PC_NAME_MAX: f32 = 170.0;
const TAB_FONT: f32 = 15.0;
const SEARCH_ID: &str = "top-search";

/// The page's left and right margin at a window width: as in the prototype at 1280, capped on very
/// wide windows.
pub(super) fn page_margin(width: f32) -> f32 {
    ((width - CONTENT_MAX) / 2.0).max((width * 0.0375).clamp(24.0, 48.0))
}

/// Where the page passes beneath the chrome: it darkens as it goes, so the brand, which has no glass
/// of its own, stays readable over a bright cover. Nothing at all with the page at its start.
pub(super) fn top_scrim(screen: Rect, scrolled: f32) -> Option<Shape> {
    let strength = (scrolled / SCRIM_FULL_AT).min(1.0);
    if strength <= 0.0 {
        return None;
    }
    let mut mesh = egui::epaint::Mesh::default();
    let bottom = screen.top() + PAGE_TOP + space::S;
    for (y, color) in [
        (
            screen.top(),
            t::TINT_FADE.gamma_multiply(SCRIM_ALPHA * strength),
        ),
        (bottom, Color32::TRANSPARENT),
    ] {
        for x in [screen.left(), screen.right()] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: pos2(x, y),
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
    }
    mesh.indices.extend([0, 1, 3, 0, 3, 2]);
    Some(Shape::mesh(mesh))
}

fn extend(ui: &Ui, shapes: Vec<Shape>) {
    ui.painter().extend(shapes);
}

fn measure(ui: &Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
        .size()
        .x
}

fn paint_text(
    ui: &Ui,
    left: f32,
    center_y: f32,
    text: &str,
    font: egui::FontId,
    color: Color32,
) -> f32 {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let width = galley.size().x;
    ui.painter()
        .galley(pos2(left, center_y - galley.size().y / 2.0), galley, color);
    width
}

/// Secondary ink: white in high contrast, as every text there.
fn secondary() -> Color32 {
    *ON_SURFACE_VARIANT
}

fn on_selected() -> Color32 {
    if high_contrast() {
        *ON_SURFACE
    } else {
        t::COLOR_ON_SELECTED
    }
}

/// A tab, capsule or button under the pointer or held: the one lift every control takes.
fn hover_lift(ui: &Ui, rect: Rect, r: f32, feel: glass::Feel) {
    let mut out = Vec::new();
    lift_shapes(&mut out, ui.ctx(), rect, r, feel.hovered, feel.pressed);
    extend(ui, out);
}

impl Launcher {
    /// The chrome, in an area above the page so content scrolls beneath it and pointer and focus
    /// reach it first. Its first shape is the boundary the live chain blurs up to.
    pub(super) fn top_bar(
        &mut self,
        ctx: &egui::Context,
        view: &View<'_>,
        logo: egui::TextureId,
        blocked: bool,
        commands: &mut Vec<Command>,
    ) {
        let screen = ctx.content_rect();
        let margin = page_margin(screen.width());
        let bar = Rect::from_min_size(
            pos2(screen.left() + margin, screen.top() + BAR_TOP),
            vec2(screen.width() - 2.0 * margin, BAR_H),
        );
        egui::Area::new(Id::unique("top-bar"))
            .order(egui::Order::Middle)
            .fixed_pos(bar.min)
            .constrain(false)
            .show(ctx, |ui| {
                ui.set_min_size(bar.size());
                glass::boundary(ui.painter(), vec![bar]);
                let sense = if blocked {
                    Sense::hover()
                } else {
                    Sense::click()
                };
                let mut x = bar.left();
                // The brand.
                let mark = Rect::from_min_size(pos2(x, bar.center().y - 15.0), Vec2::splat(30.0));
                egui::Image::from_texture((logo, mark.size())).paint_at(ui, mark);
                x = mark.right() + 10.0;
                if screen.width() >= WORDMARK_GOES_BELOW {
                    x +=
                        paint_text(ui, x, bar.center().y, "Nightjar", semibold(19.0), *PAPER) + 6.0;
                }
                x += CAPSULE_GAP;
                x = self.nav_capsule(ui, x, bar, sense, commands);
                // The right end: the PC capsule, then the search field left of it.
                let pc_w = self.pc_capsule_width(ui, view);
                let pc =
                    Rect::from_min_max(pos2(bar.right() - pc_w, bar.top()), bar.right_bottom());
                let folds = screen.width() < SEARCH_FOLDS_BELOW;
                let room = (pc.left() - CAPSULE_GAP - x - CAPSULE_GAP).max(BAR_H);
                let search_w = if folds && !self.search_open {
                    BAR_H
                } else {
                    SEARCH_W.min(room)
                };
                let search = Rect::from_min_size(
                    pos2(pc.left() - CAPSULE_GAP - search_w, bar.top()),
                    vec2(search_w, BAR_H),
                );
                self.search_capsule(ui, search, folds, sense, blocked);
                self.pc_capsule(ui, pc, view, sense);
            });
    }

    fn nav_capsule(
        &mut self,
        ui: &mut Ui,
        left: f32,
        bar: Rect,
        sense: Sense,
        commands: &mut Vec<Command>,
    ) -> f32 {
        const PAD: f32 = 4.0;
        const TAB_PAD: f32 = 18.0;
        const ICON: f32 = 22.0;
        const ICON_GAP: f32 = 8.0;
        let marked = self.screen.destination();
        let offered = self.offered_destinations();
        let shown: Vec<(&str, Screen)> = DESTINATIONS
            .iter()
            .filter(|(_, s)| offered.contains(s))
            .map(|(n, s)| (*n, *s))
            .collect();
        let font = semibold(TAB_FONT);
        let widths: Vec<f32> = shown
            .iter()
            .map(|(name, _)| TAB_PAD * 2.0 + ICON + ICON_GAP + measure(ui, tr(name), font.clone()))
            .collect();
        let capsule = Rect::from_min_size(
            pos2(left, bar.top()),
            vec2(widths.iter().sum::<f32>() + PAD * 2.0, BAR_H),
        );
        let ctx = ui.ctx().clone();
        let mut out = Vec::new();
        glass::surface(
            &mut out,
            &ctx,
            capsule,
            BAR_H / 2.0,
            glass::Material::Chrome,
        );
        extend(ui, out);
        let r = (BAR_H - 2.0 * PAD) / 2.0;
        let mut x = capsule.left() + PAD;
        let mut tabs = Vec::with_capacity(shown.len());
        for ((name, screen), width) in shown.iter().zip(&widths) {
            tabs.push((
                *name,
                *screen,
                Rect::from_min_size(
                    pos2(x, capsule.top() + PAD),
                    vec2(*width, BAR_H - 2.0 * PAD),
                ),
            ));
            x += width;
        }
        // The pill springs from tab to tab, under their labels.
        if let Some((_, _, tab)) = tabs.iter().find(|(_, s, _)| *s == marked) {
            let id = Id::unique("nav-pill");
            let at = spring::animate(&ctx, id.with("x"), tab.left(), spring::TAB_PILL);
            let width = spring::animate(&ctx, id.with("w"), tab.width(), spring::TAB_PILL);
            let pill = Rect::from_min_size(
                pos2(at, tab.top()),
                vec2(width.max(tab.height()), tab.height()),
            );
            let mut out = Vec::new();
            if high_contrast() {
                glass::solid(&mut out, &ctx, pill, r, *MOON_CONTAINER);
                glass::rim(&mut out, &ctx, pill, r, 1.5, &|_| t::HC_COLOR_RIM);
            } else {
                glass::soft(
                    &mut out,
                    &ctx,
                    pill.translate(vec2(0.0, 2.0)),
                    r,
                    10.0,
                    Color32::from_rgba_premultiplied(0, 0, 0, 64),
                );
                glass::solid(&mut out, &ctx, pill, r, t::TINT_SELECTED_FILL);
                let rim = glass::gradient(
                    pill,
                    180.0,
                    &[
                        (0.0, t::TINT_SELECTED_RIM_TOP),
                        (0.06, Color32::TRANSPARENT),
                    ],
                );
                glass::rim(&mut out, &ctx, pill, r, 1.0, &rim);
            }
            extend(ui, out);
        }
        for (name, screen, tab) in tabs {
            let is_marked = marked == screen;
            let response = ui.interact(tab, Id::unique(("nav", name)), sense);
            // The marked one reads as selected, as the chosen filter chip does.
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, is_marked, tr(name))
            });
            let feel = glass::feel(&ctx, &response, 1.0, 1.0);
            if !is_marked {
                hover_lift(ui, tab, r, feel);
            }
            let ink = if is_marked {
                on_selected()
            } else if feel.hovered || feel.focused {
                *ON_SURFACE
            } else {
                secondary()
            };
            let icon = Rect::from_min_size(
                pos2(tab.left() + TAB_PAD, tab.center().y - ICON / 2.0),
                Vec2::splat(ICON),
            );
            icons::paint(ui.painter(), destination_icon(screen), icon, ink);
            paint_text(
                ui,
                icon.right() + ICON_GAP,
                tab.center().y,
                tr(name),
                font.clone(),
                ink,
            );
            focus_ring(ui, tab, &response, r as u8);
            self.focus_chrome(&response, &format!("top-{}", name));
            if response.clicked() && self.screen != screen {
                self.go_destination(screen, commands);
            }
        }
        capsule.right()
    }

    /// Chrome's own [`Self::focus_item`]: remembered and restored like any item, but it floats, so
    /// nothing scrolls to it.
    fn focus_chrome(&mut self, response: &Response, key: &str) {
        // Focus moved by the arrow keys never reads as gained: a new key is what tells.
        if response.has_focus() && self.focused_key.as_deref() != Some(key) {
            self.focused_key = Some(key.to_owned());
            self.home_to_top = true;
        }
        if self.restore_key.as_deref() == Some(key) {
            self.restore_key = None;
            self.focus_screen = false;
            response.request_focus();
        }
    }

    /// Opens the search: All games is where it looks, so that page opens with the field focused.
    pub(super) fn open_search(&mut self) {
        self.search_open = true;
        self.search_focus = true;
        if self.screen != Screen::AllGames {
            self.library_filter = console::LibraryFilter::All;
            self.search_pushed = true;
            self.open(Screen::AllGames);
        }
        self.focus_screen = false;
    }

    /// Escape in the search: the words go, and a page opened for the search goes with them, the focus
    /// back where it was. True when it took the key.
    pub(super) fn escape_library_search(&mut self) -> bool {
        if self.screen != Screen::AllGames || !self.search_open {
            return false;
        }
        let pushed = std::mem::take(&mut self.search_pushed);
        self.search_open = false;
        self.library_query.clear();
        if pushed {
            self.leave();
        } else {
            self.focus_screen = true;
        }
        true
    }

    /// Enter on an empty field: as Escape, without leaving the page's filter or focus behind.
    fn close_search(&mut self) {
        self.search_open = false;
        self.library_query.clear();
        self.focus_screen = true;
    }

    fn search_capsule(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        folded: bool,
        sense: Sense,
        blocked: bool,
    ) {
        const ICON: f32 = 19.0;
        let ctx = ui.ctx().clone();
        if !blocked
            && !ctx.text_edit_focused()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K))
        {
            self.open_search();
        }
        let r = BAR_H / 2.0;
        let mut out = Vec::new();
        glass::surface(&mut out, &ctx, rect, r, glass::Material::Chrome);
        extend(ui, out);
        if self.search_open && self.screen == Screen::AllGames {
            let icon = Rect::from_min_size(
                pos2(rect.left() + 16.0, rect.center().y - ICON / 2.0),
                Vec2::splat(ICON),
            );
            icons::paint(ui.painter(), Icon::Search, icon, *ON_SURFACE);
            let field = Rect::from_min_max(
                pos2(icon.right() + 9.0, rect.top() + 12.0),
                pos2(rect.right() - 16.0, rect.bottom() - 12.0),
            );
            // Down leaves the field for the results: a D-pad has no other way out.
            let field_id = Id::unique(SEARCH_ID);
            if ctx.memory(|m| m.has_focus(field_id))
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown))
            {
                self.focus_screen = true;
            }
            let edit = egui::TextEdit::singleline(&mut self.library_query)
                .id(field_id)
                .frame(Frame::NONE)
                .hint_text(tr("Search"))
                .font(regular(TAB_FONT))
                .text_color(*ON_SURFACE)
                .desired_width(field.width());
            let response = ui.put(field, edit);
            if std::mem::take(&mut self.search_focus) {
                response.request_focus();
            }
            if response.has_focus() {
                paint_focus_ring(ui.painter(), rect, r as u8);
            }
            // Enter leaves the words in place and walks on to the results.
            if response.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter)) {
                if self.library_query.is_empty() {
                    self.close_search();
                } else {
                    self.focus_screen = true;
                }
            }
            return;
        }
        let response = ui.interact(rect, Id::unique("top-search-button"), sense);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Search")));
        self.focus_chrome(&response, "top-search");
        let feel = glass::feel(&ctx, &response, 1.0, 1.0);
        hover_lift(ui, rect, r, feel);
        let ink = if feel.hovered || feel.focused {
            *ON_SURFACE
        } else {
            secondary()
        };
        let icon_x = if folded {
            rect.center().x - ICON / 2.0
        } else {
            rect.left() + 16.0
        };
        let icon = Rect::from_min_size(
            pos2(icon_x, rect.center().y - ICON / 2.0),
            Vec2::splat(ICON),
        );
        icons::paint(ui.painter(), Icon::Search, icon, ink);
        if !folded {
            paint_text(
                ui,
                icon.right() + 9.0,
                rect.center().y,
                tr("Search"),
                regular(TAB_FONT),
                ink,
            );
            let keys = ui.painter().layout_no_wrap(
                "Ctrl K".to_owned(),
                regular(t::TYPE_CAPTION_SIZE),
                ink.gamma_multiply(0.75),
            );
            ui.painter().galley(
                pos2(
                    rect.right() - 16.0 - keys.size().x,
                    rect.center().y - keys.size().y / 2.0,
                ),
                keys,
                ink,
            );
        }
        focus_ring(ui, rect, &response, r as u8);
        if response.clicked() {
            self.open_search();
        }
    }

    /// The PC the capsule speaks of: the hero's, else the first that is up and paired, else the first.
    fn capsule_host<'a>(&self, view: &'a View<'_>) -> Option<&'a Host> {
        self.hero
            .as_ref()
            .and_then(|hero| view.hosts.get(hero.host))
            .or_else(|| {
                view.hosts
                    .list
                    .iter()
                    .find(|h| h.reach == Reach::Online && h.paired())
            })
            .or_else(|| view.hosts.list.first())
    }

    fn pc_capsule_width(&self, ui: &Ui, view: &View<'_>) -> f32 {
        let font = semibold(TAB_FONT);
        match self.capsule_host(view) {
            Some(host) => {
                16.0 + 9.0
                    + 9.0
                    + measure(ui, host.display_name(), font).min(PC_NAME_MAX)
                    + 9.0
                    + 17.0
                    + 14.0
            }
            None => 16.0 + 19.0 + 9.0 + measure(ui, tr("Add PC"), font) + 16.0,
        }
    }

    fn pc_capsule(&mut self, ui: &mut Ui, rect: Rect, view: &View<'_>, sense: Sense) {
        let ctx = ui.ctx().clone();
        let r = BAR_H / 2.0;
        let mut out = Vec::new();
        glass::surface(&mut out, &ctx, rect, r, glass::Material::Chrome);
        extend(ui, out);
        let response = ui.interact(rect, Id::unique("top-pc"), sense);
        let feel = glass::feel(&ctx, &response, 1.0, 1.0);
        hover_lift(ui, rect, r, feel);
        let Some(host) = self.capsule_host(view) else {
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Add PC")));
            let icon = Rect::from_min_size(
                pos2(rect.left() + 16.0, rect.center().y - 9.5),
                Vec2::splat(19.0),
            );
            icons::paint(ui.painter(), Icon::Plus, icon, *ON_SURFACE);
            paint_text(
                ui,
                icon.right() + 9.0,
                rect.center().y,
                tr("Add PC"),
                semibold(TAB_FONT),
                *ON_SURFACE,
            );
            focus_ring(ui, rect, &response, r as u8);
            if response.clicked() {
                self.add_host_dialog();
            }
            return;
        };
        let (status, ..) = host_status(host);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!("{}, {status}", host.display_name()),
            )
        });
        let up = host.reach == Reach::Online && host.paired();
        let dot = pos2(rect.left() + 16.0 + 4.5, rect.center().y);
        let mut out = Vec::new();
        if up && !high_contrast() {
            glass::soft(
                &mut out,
                &ctx,
                Rect::from_center_size(dot, Vec2::splat(9.0)),
                4.5,
                8.0,
                t::COLOR_OK,
            );
        }
        extend(ui, out);
        ui.painter()
            .circle_filled(dot, 4.5, if up { *OK } else { t::COLOR_OFFLINE });
        let mut job = egui::text::LayoutJob::simple_singleline(
            host.display_name().to_owned(),
            semibold(TAB_FONT),
            *ON_SURFACE,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(PC_NAME_MAX);
        let name = ui.painter().layout_job(job);
        ui.painter().galley(
            pos2(dot.x + 4.5 + 9.0, rect.center().y - name.size().y / 2.0),
            name,
            *ON_SURFACE,
        );
        let chevron = Rect::from_min_size(
            pos2(rect.right() - 14.0 - 17.0, rect.center().y - 8.5),
            Vec2::splat(17.0),
        );
        icons::paint(ui.painter(), Icon::Down, chevron, *ON_SURFACE);
        focus_ring(ui, rect, &response, r as u8);
        let id = host.id;
        self.focus_chrome(&response, "top-pc");
        if response.clicked() {
            self.open_menu(
                Dialog::HostOptions { host: id },
                Anchor::Below(rect),
                "top-pc".into(),
            );
        }
        if let Some(anchor) = menu_request(ui, &response, None) {
            self.open_menu(Dialog::HostOptions { host: id }, anchor, "top-pc".into());
        }
    }
}

/// The width a hero button of `label` takes.
pub(super) fn play_width(ui: &Ui, label: &str) -> f32 {
    (30.0 + 21.0 + 10.0 + measure(ui, label, semibold(t::TYPE_BODY_SIZE + 2.0)) + 30.0).max(176.0)
}

pub(super) fn capsule_width(ui: &Ui, label: &str) -> f32 {
    20.0 + 19.0 + 8.0 + measure(ui, label, semibold(t::TYPE_BODY_SIZE)) + 20.0
}

/// The one solid amber action of a screen.
pub(super) fn play_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    label: &str,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let ctx = ui.ctx().clone();
    let feel = glass::feel(&ctx, &response, t::FOCUS_SCALE_CONTROL, 1.0);
    let shown = glass::scaled_rect(rect, feel.scale);
    let r = shown.height() / 2.0;
    let mut out = Vec::new();
    let ink = if enabled {
        glass::play(&mut out, &ctx, shown, r);
        lift_shapes(&mut out, &ctx, shown, r, feel.hovered, feel.pressed);
        *ON_MOON
    } else {
        // Off is grey, never a faded amber.
        glass::control(&mut out, &ctx, shown, r);
        *OUTLINE
    };
    if feel.focused {
        glass::focus(&mut out, &ctx, shown, r, true);
    }
    extend(ui, out);
    let words = measure(ui, label, semibold(t::TYPE_BODY_SIZE + 2.0));
    let left = shown.center().x - (21.0 + 10.0 + words) / 2.0;
    let glyph = Rect::from_min_size(pos2(left, shown.center().y - 10.5), Vec2::splat(21.0));
    icons::paint(ui.painter(), icon, glyph, ink);
    paint_text(
        ui,
        glyph.right() + 10.0,
        shown.center().y,
        label,
        semibold(t::TYPE_BODY_SIZE + 2.0),
        ink,
    );
    response
}

/// A tinted capsule beside Play.
pub(super) fn control_capsule(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    label: &str,
    said: &str,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, said));
    let ctx = ui.ctx().clone();
    let feel = glass::feel(&ctx, &response, t::FOCUS_SCALE_CONTROL, 1.0);
    let shown = glass::scaled_rect(rect, feel.scale);
    let r = shown.height() / 2.0;
    let mut out = Vec::new();
    glass::control(&mut out, &ctx, shown, r);
    extend(ui, out);
    hover_lift(ui, shown, r, feel);
    let ink = if enabled { *ON_SURFACE } else { *OUTLINE };
    let glyph = Rect::from_min_size(
        pos2(shown.left() + 20.0, shown.center().y - 9.5),
        Vec2::splat(19.0),
    );
    icons::paint(ui.painter(), icon, glyph, ink);
    paint_text(
        ui,
        glyph.right() + 8.0,
        shown.center().y,
        label,
        semibold(t::TYPE_BODY_SIZE),
        ink,
    );
    if feel.focused {
        paint_focus_ring(ui.painter(), shown, r as u8);
    }
    response
}

/// A chip over a title: a state or a store. Takes no focus.
pub(super) fn info_chip(
    ui: &Ui,
    left: f32,
    center_y: f32,
    dot: Option<Color32>,
    label: &str,
) -> f32 {
    const H: f32 = 32.0;
    let words = measure(ui, label, semibold(t::TYPE_SUB_SIZE));
    let dot_room = if dot.is_some() { 9.0 + 7.0 } else { 0.0 };
    let rect = Rect::from_min_size(
        pos2(left, center_y - H / 2.0),
        vec2(13.0 + dot_room + words + 13.0, H),
    );
    let mut out = Vec::new();
    glass::control(&mut out, ui.ctx(), rect, H / 2.0);
    extend(ui, out);
    if let Some(color) = dot {
        ui.painter()
            .circle_filled(pos2(rect.left() + 13.0 + 4.5, rect.center().y), 4.5, color);
    }
    paint_text(
        ui,
        rect.left() + 13.0 + dot_room,
        rect.center().y,
        label,
        semibold(t::TYPE_SUB_SIZE),
        *ON_SURFACE,
    );
    rect.width()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_margin_matches_the_prototype_at_1280_and_centres_on_very_wide_windows() {
        assert_eq!(page_margin(1280.0), 48.0);
        assert_eq!(page_margin(640.0), 24.0);
        assert_eq!(page_margin(3840.0), (3840.0 - CONTENT_MAX) / 2.0);
    }
}
