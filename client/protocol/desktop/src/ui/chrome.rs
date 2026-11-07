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
/// The brand's width with its wordmark, near enough.
const BRAND_WORDED_W: f32 = 118.0;
/// The round Back, at rest at the head of a page and held in the brand's place once scrolled there.
pub(crate) const BACK_SIZE: f32 = 44.0;
const BACK_HELD_AT: f32 = BAR_TOP + (BAR_H - BACK_SIZE) / 2.0;
/// How far a page has scrolled when the brand starts to give way to Back, and when it is gone.
const BRAND_FADES_FROM: f32 = 16.0;
const BRAND_GONE_AT: f32 = 36.0;
/// A page's title beside Back.
pub(crate) const PAGE_TITLE: f32 = 30.0;
const CAPSULE_GAP: f32 = 14.0;
const SEARCH_W: f32 = 220.0;
/// Below this window width the search field folds to its icon, below the second the wordmark goes.
const SEARCH_FOLDS_BELOW: f32 = 1120.0;
const WORDMARK_GOES_BELOW: f32 = 900.0;
const PC_NAME_MAX: f32 = 170.0;
const TAB_FONT: f32 = 15.0;
pub(super) const SEARCH_ID: &str = "top-search";

const NAV_PAD: f32 = 4.0;
const NAV_ICON: f32 = 22.0;
const NAV_ICON_GAP: f32 = 8.0;
const TAB_PAD: f32 = 18.0;
const TAB_PAD_LEAST: f32 = 10.0;

/// How the tabs fit the room a window leaves them, given each one's word: their padding, and whether
/// the words stay. The padding gives way first, then the words go, as the search field folds to its
/// icon; the capsule never runs under what is to its right.
pub(super) fn nav_fit(words: &[f32], room: f32) -> (f32, bool) {
    let tabs = words.len().max(1) as f32;
    let at_full = NAV_PAD * 2.0
        + words
            .iter()
            .map(|word| TAB_PAD * 2.0 + NAV_ICON + NAV_ICON_GAP + word)
            .sum::<f32>();
    let pad = TAB_PAD - (at_full - room).max(0.0) / (2.0 * tabs);
    if pad >= TAB_PAD_LEAST {
        (pad, true)
    } else {
        (TAB_PAD_LEAST, false)
    }
}

/// The page's left and right margin at a window width: as in the prototype at 1280, capped on very
/// wide windows.
pub(super) fn page_margin(width: f32) -> f32 {
    ((width - CONTENT_MAX) / 2.0).max((width * 0.0375).clamp(24.0, 48.0))
}

/// Where the brand sits in a window: the mark, and the wordmark when there is room for it. It has no
/// glass of its own, so the veil over the page keeps its words readable.
pub(super) fn brand_rect(screen: Rect) -> Rect {
    let margin = page_margin(screen.width());
    let width = if screen.width() >= WORDMARK_GOES_BELOW {
        BRAND_WORDED_W
    } else {
        30.0
    };
    Rect::from_min_size(
        pos2(
            screen.left() + margin,
            screen.top() + BAR_TOP + BAR_H / 2.0 - 15.0,
        ),
        vec2(width, 30.0),
    )
}

/// Where Back shows with its page's slot starting at `slot_top`: with the page, then held in the bar.
pub(super) fn back_top(screen: Rect, slot_top: f32) -> f32 {
    slot_top.max(screen.top() + BACK_HELD_AT)
}

/// The brand's strength with Back's slot at `slot_top`, None on a page without Back. The mark never
/// shows through Back.
pub(super) fn brand_alpha(screen: Rect, slot_top: Option<f32>) -> f32 {
    slot_top.map_or(1.0, |top| {
        let scrolled = screen.top() + PAGE_TOP - top;
        1.0 - ((scrolled - BRAND_FADES_FROM) / (BRAND_GONE_AT - BRAND_FADES_FROM)).clamp(0.0, 1.0)
    })
}

/// All the places Back can show, for the live chain: from its rest down the page to the bar.
fn back_column(screen: Rect) -> Rect {
    let margin = page_margin(screen.width());
    Rect::from_min_size(
        pos2(screen.left() + margin, screen.top() + BACK_HELD_AT),
        vec2(BACK_SIZE, PAGE_TOP - BACK_HELD_AT + BACK_SIZE),
    )
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

/// The width of the Add PC capsule.
fn add_pc_width(ui: &Ui) -> f32 {
    16.0 + 19.0 + 8.0 + measure(ui, tr("Add PC"), semibold(TAB_FONT)) + 20.0
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

/// The layer of the chrome over every page.
fn top_bar_layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Middle, Id::unique("top-bar"))
}

impl Launcher {
    /// The chrome, in an area above the page so content scrolls beneath it and pointer and focus
    /// reach it first. Its first shape is the boundary the live chain blurs up to.
    pub(super) fn top_bar(
        &mut self,
        ctx: &egui::Context,
        view: &View<'_>,
        blocked: bool,
        commands: &mut Vec<Command>,
    ) -> egui::layers::ShapeIdx {
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
                let mut regions = vec![bar];
                if self.has_back() {
                    regions.push(back_column(screen));
                }
                // A panel sits anywhere on the page, so all of it is blurred under one.
                if self.panel_is_up(view) {
                    regions = vec![screen];
                }
                glass::boundary(ui.painter(), regions);
                let sense = if blocked {
                    Sense::hover()
                } else {
                    Sense::click()
                };
                let mut x = bar.left();
                // The brand's place: what it shows waits for the page, which tells how far it scrolled.
                let brand = ui.painter().add(Shape::Noop);
                x += 30.0 + 10.0;
                if screen.width() >= WORDMARK_GOES_BELOW {
                    x += measure(ui, "Nightjar", semibold(19.0)) + 6.0;
                }
                x += CAPSULE_GAP;
                if !blocked
                    && !ctx.text_edit_focused()
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K))
                {
                    self.open_search();
                }
                // On PCs the right end is where a PC is added: there is no PC to speak of or game to find.
                if self.screen == Screen::Hosts && first_run::first_run_of(view.hosts).is_some() {
                    // The first run: Add PC is the page's own.
                    self.nav_capsule(ui, x, bar.right(), bar, sense, commands);
                    return brand;
                }
                if self.screen == Screen::Hosts {
                    let add_w = add_pc_width(ui);
                    self.nav_capsule(ui, x, bar.right() - add_w - CAPSULE_GAP, bar, sense, commands);
                    let add = Rect::from_min_max(
                        pos2(bar.right() - add_w, bar.top()),
                        bar.right_bottom(),
                    );
                    self.add_pc_capsule(ui, add, sense);
                    return brand;
                }
                // The right end: the PC capsule, then the search field left of it.
                let pc_w = self.pc_capsule_width(ui, view);
                let nav_ends_by = bar.right() - pc_w - BAR_H - 2.0 * CAPSULE_GAP;
                x = self.nav_capsule(ui, x, nav_ends_by, bar, sense, commands);
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
                self.search_capsule(ui, search, folds, sense);
                self.pc_capsule(ui, pc, view, sense);
                brand
            })
            .inner
    }

    /// The brand in the place [`Self::top_bar`] kept for it, giving way to Back as a page scrolls.
    pub(super) fn paint_brand(
        &self,
        ctx: &egui::Context,
        place: egui::layers::ShapeIdx,
        logo: egui::TextureId,
    ) {
        let screen = ctx.content_rect();
        let alpha = brand_alpha(screen, self.back_slot);
        if alpha <= 0.0 {
            return;
        }
        let area = brand_rect(screen);
        let mark = Rect::from_min_size(area.min, Vec2::splat(30.0));
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        let tint = Color32::WHITE.gamma_multiply(alpha);
        let mut shapes = vec![Shape::image(logo, mark, uv, tint)];
        if screen.width() >= WORDMARK_GOES_BELOW {
            let ink = PAPER.gamma_multiply(alpha);
            let word =
                ctx.fonts_mut(|f| f.layout_no_wrap("Nightjar".to_owned(), semibold(19.0), ink));
            let at = pos2(mark.right() + 10.0, mark.center().y - word.size().y / 2.0);
            shapes.push(Shape::galley(at, word, ink));
        }
        ctx.layer_painter(top_bar_layer())
            .set(place, Shape::Vec(shapes));
    }

    /// Back, in the chrome's layer so it passes over the page and takes the pointer first, drawn from
    /// the page's own header so its slot's place is this frame's and Tab meets it first. `slot` is the
    /// room the header keeps for it.
    pub(super) fn sticky_back(&mut self, page: &Ui, slot: Rect, said: &str) -> Response {
        let ctx = page.ctx().clone();
        self.back_slot = Some(slot.top());
        let rect = Rect::from_min_size(
            pos2(slot.left(), back_top(ctx.content_rect(), slot.top())),
            Vec2::splat(BACK_SIZE),
        );
        let mut ui = Ui::new(
            ctx.clone(),
            Id::unique("sticky-back"),
            UiBuilder::new().layer_id(top_bar_layer()).max_rect(rect),
        );
        // A new Ui clips to its rect, which would square off the shadow and the focus halo.
        ui.set_clip_rect(ctx.content_rect());
        let enabled = page.is_enabled();
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(rect, Id::unique("back"), sense);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, said));
        let r = BACK_SIZE / 2.0;
        let feel = glass::feel(&ctx, &response, 1.0, 1.0);
        let mut out = Vec::new();
        glass::surface(&mut out, &ctx, rect, r, glass::Material::Chrome);
        extend(&ui, out);
        hover_lift(&ui, rect, r, feel);
        icons::paint(
            ui.painter(),
            Icon::Back,
            Rect::from_center_size(rect.center(), Vec2::splat(21.0)),
            *ON_SURFACE,
        );
        focus_ring(&ui, rect, &response, r as u8);
        response.on_hover_text(said)
    }

    fn nav_capsule(
        &mut self,
        ui: &mut Ui,
        left: f32,
        ends_by: f32,
        bar: Rect,
        sense: Sense,
        commands: &mut Vec<Command>,
    ) -> f32 {
        const PAD: f32 = NAV_PAD;
        const ICON: f32 = NAV_ICON;
        const ICON_GAP: f32 = NAV_ICON_GAP;
        let marked = self.screen.destination();
        let offered = self.offered_destinations();
        let shown: Vec<(&str, Screen)> = DESTINATIONS
            .iter()
            .filter(|(_, s)| offered.contains(s))
            .map(|(n, s)| (*n, *s))
            .collect();
        let font = semibold(TAB_FONT);
        let words: Vec<f32> = shown
            .iter()
            .map(|(name, _)| measure(ui, tr(name), font.clone()))
            .collect();
        let (tab_pad, worded) = nav_fit(&words, ends_by - left);
        let widths: Vec<f32> = words
            .iter()
            .map(|word| {
                tab_pad * 2.0 + ICON + if worded { ICON_GAP + word } else { 0.0 }
            })
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
                pos2(tab.left() + tab_pad, tab.center().y - ICON / 2.0),
                Vec2::splat(ICON),
            );
            icons::paint(ui.painter(), destination_icon(screen), icon, ink);
            if worded {
                paint_text(
                    ui,
                    icon.right() + ICON_GAP,
                    tab.center().y,
                    tr(name),
                    font.clone(),
                    ink,
                );
            } else {
                response.clone().on_hover_text(tr(name));
            }
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
            self.page_to_top = true;
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
        // The page stays, so it is no longer one a later Escape leaves.
        self.search_pushed = false;
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
    ) {
        const ICON: f32 = 19.0;
        let ctx = ui.ctx().clone();
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
            // The whole height, centred: its words on the line the icon and the closed field's sit on.
            let field = Rect::from_min_max(
                pos2(icon.right() + 9.0, rect.top()),
                pos2(rect.right() - 16.0, rect.bottom()),
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
                .margin(egui::Margin::ZERO)
                .align(egui::Align2::LEFT_CENTER)
                .min_size(field.size())
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

    /// The PC the capsule speaks of: the page's, else the hero's, else the first that is up and
    /// paired, else the first.
    fn capsule_host<'a>(&self, view: &'a View<'_>) -> Option<&'a Host> {
        let page = match self.screen {
            Screen::Apps(id) | Screen::HostDetails(id) | Screen::Files(id) => Some(id),
            Screen::Game { host, .. } => Some(host),
            _ => None,
        };
        page.and_then(|id| view.hosts.get(id))
            .or_else(|| self.hero.as_ref().and_then(|hero| view.hosts.get(hero.host)))
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
            None => add_pc_width(ui),
        }
    }

    /// Add PC, where the PC capsule would be.
    fn add_pc_capsule(&mut self, ui: &mut Ui, rect: Rect, sense: Sense) {
        let ctx = ui.ctx().clone();
        let r = BAR_H / 2.0;
        let mut out = Vec::new();
        glass::surface(&mut out, &ctx, rect, r, glass::Material::Chrome);
        extend(ui, out);
        let response = ui.interact(rect, Id::unique("top-add-pc"), sense);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Add PC")));
        let feel = glass::feel(&ctx, &response, 1.0, 1.0);
        hover_lift(ui, rect, r, feel);
        let icon = Rect::from_min_size(
            pos2(rect.left() + 16.0, rect.center().y - 9.5),
            Vec2::splat(19.0),
        );
        icons::paint(ui.painter(), Icon::Plus, icon, *ON_SURFACE);
        paint_text(
            ui,
            icon.right() + 8.0,
            rect.center().y,
            tr("Add PC"),
            semibold(TAB_FONT),
            *ON_SURFACE,
        );
        focus_ring(ui, rect, &response, r as u8);
        self.focus_chrome(&response, "top-add-pc");
        if response.clicked() {
            self.add_host_dialog();
        }
    }

    fn pc_capsule(&mut self, ui: &mut Ui, rect: Rect, view: &View<'_>, sense: Sense) {
        let Some(host) = self.capsule_host(view) else {
            self.add_pc_capsule(ui, rect, sense);
            return;
        };
        let ctx = ui.ctx().clone();
        let r = BAR_H / 2.0;
        let mut out = Vec::new();
        glass::surface(&mut out, &ctx, rect, r, glass::Material::Chrome);
        extend(ui, out);
        let response = ui.interact(rect, Id::unique("top-pc"), sense);
        let feel = glass::feel(&ctx, &response, 1.0, 1.0);
        hover_lift(ui, rect, r, feel);
        let (status, ..) = host_status(host);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!("{}, {status}", host.display_name()),
            )
        });
        let dot = pos2(rect.left() + 16.0 + 4.5, rect.center().y);
        let mut out = Vec::new();
        pc_dot(&mut out, &ctx, dot, host);
        extend(ui, out);
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
            let many = view.hosts.list.len() >= 2;
            let dialog = if many {
                Dialog::PcSheet { current: id }
            } else {
                Dialog::HostOptions { host: id }
            };
            self.open_menu(dialog, Anchor::Below(rect), "top-pc".into());
            // The ring on its first row is for who came by keys; the next key starts there.
            if many && response.clicked_by(egui::PointerButton::Primary) {
                self.focus_dialog = false;
            }
        }
        if let Some(anchor) = menu_request(ui, &response, None) {
            self.open_menu(Dialog::HostOptions { host: id }, anchor, "top-pc".into());
        }
    }
}

/// A PC's status dot: lit with a glow when it is up and paired.
pub(super) fn pc_dot(out: &mut Vec<Shape>, ctx: &egui::Context, at: Pos2, host: &Host) {
    let up = host.reach == Reach::Online && host.paired();
    if up && !high_contrast() {
        glass::soft(
            out,
            ctx,
            Rect::from_center_size(at, Vec2::splat(9.0)),
            4.5,
            8.0,
            t::COLOR_OK,
        );
    }
    out.push(Shape::circle_filled(
        at,
        4.5,
        if up { *OK } else { t::COLOR_OFFLINE },
    ));
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
    working: bool,
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
    if working {
        ink_spinner(ui, glyph.center(), glyph.width() / 2.0 - 1.0, ink);
    } else {
        icons::paint(ui.painter(), icon, glyph, ink);
    }
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

/// A spinner in the ink of what it sits on; still under reduced motion.
fn ink_spinner(ui: &Ui, center: Pos2, radius: f32, ink: Color32) {
    const POINTS: usize = 24;
    const TURN_SECONDS: f64 = 0.9;
    let motion = spring::motion(ui.ctx());
    let time = if motion { ui.input(|i| i.time) } else { 0.0 };
    let start = (time / TURN_SECONDS).fract() as f32 * std::f32::consts::TAU;
    let line: Vec<Pos2> = (0..=POINTS)
        .map(|n| {
            let a = start + n as f32 / POINTS as f32 * std::f32::consts::PI * 1.5;
            center + radius * vec2(a.cos(), a.sin())
        })
        .collect();
    ui.painter().add(Shape::line(line, Stroke::new(2.0, ink)));
    if motion {
        ui.ctx().request_repaint();
    }
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

/// How wide an [`info_chip`] with a `dot` of that size and `label` is.
pub(super) fn info_chip_width(ui: &Ui, dot: Option<f32>, label: &str) -> f32 {
    let dot_room = dot.map_or(0.0, |d| d + 7.0);
    13.0 + dot_room + measure(ui, label, semibold(t::TYPE_SUB_SIZE)) + 13.0
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

/// A key cap and a word for Go and More on what has focus, for the device in the hand. No animation:
/// they change on the next input, and are gone for a mouse and over a dialog.
impl Launcher {
    pub(super) fn button_hints(&self, ctx: &egui::Context, view: &View<'_>) {
        let source = self.input_source;
        if source == crate::input_source::InputSource::None || self.panel_is_up(view) {
            return;
        }
        let Some(go) = hint_said(ctx) else { return };
        let swap = self.settings.swap_face_buttons;
        egui::Area::new(Id::unique("button-hints"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
            .interactable(false)
            .fade_in(false)
            .show(ctx, |ui| {
                let place = ui.painter().add(egui::Shape::Noop);
                let shown = Frame::new().inner_margin(Margin { left: 6, right: 16, top: 5, bottom: 5 }).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        crate::glyphs::show(ui, &source.go(swap));
                        ui.label(RichText::new(tr(go)).font(semibold(text::LABEL)).color(ON_SURFACE));
                        ui.add_space(8.0);
                        crate::glyphs::show(ui, &source.more(swap));
                        ui.label(RichText::new(tr("Options")).font(semibold(text::LABEL)).color(ON_SURFACE));
                    });
                });
                let rect = shown.response.rect;
                let mut shapes = Vec::new();
                glass::without_backdrop(&mut shapes, ctx, rect, rect.height() / 2.0, glass::Material::Chip);
                // The shadow falls past the area's clip.
                let mut painter = ui.painter().clone();
                painter.set_clip_rect(ctx.content_rect());
                painter.set(place, egui::Shape::Vec(shapes));
            });
    }
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
