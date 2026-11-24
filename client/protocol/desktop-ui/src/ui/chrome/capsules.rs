//! Top-bar search and PC capsules, hero action buttons, info chips, and controller button hints.

use super::*;

impl Launcher {
    pub(super) fn search_capsule(
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
                .hint_text(hint(tr("Search"), regular(TAB_FONT)))
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

    pub(super) fn pc_capsule_width(&self, ui: &Ui, view: &View<'_>) -> f32 {
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
    pub(super) fn add_pc_capsule(&mut self, ui: &mut Ui, rect: Rect, sense: Sense) {
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

    pub(super) fn pc_capsule(&mut self, ui: &mut Ui, rect: Rect, view: &View<'_>, sense: Sense) {
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
pub(in crate::ui) fn pc_dot(out: &mut Vec<Shape>, ctx: &egui::Context, at: Pos2, host: &Host) {
    let up = host.reach == Reach::Online && host.paired();
    if up && !high_contrast() {
        glass::soft(
            out,
            ctx,
            Rect::from_center_size(at, Vec2::splat(9.0)),
            4.5,
            8.0,
            shade::COLOR_OK.get(),
        );
    }
    out.push(Shape::circle_filled(
        at,
        4.5,
        if up { *OK } else { shade::COLOR_OFFLINE.get() },
    ));
}

/// The width a hero button of `label` takes.
pub(in crate::ui) fn play_width(ui: &Ui, label: &str) -> f32 {
    (30.0 + 21.0 + 10.0 + measure(ui, label, semibold(t::TYPE_BODY_SIZE + 2.0)) + 30.0).max(176.0)
}

pub(in crate::ui) fn capsule_width(ui: &Ui, label: &str) -> f32 {
    20.0 + 19.0 + 8.0 + measure(ui, label, semibold(t::TYPE_BODY_SIZE)) + 20.0
}

/// The one solid amber action of a screen.
pub(in crate::ui) fn play_button(
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
        glass::on_play()
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
pub(in crate::ui) fn control_capsule(
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

/// A round tinted control showing only its icon, for a row with no room for words (the hero's
/// star, as on Android's Home): its words are its tooltip and what a screen reader says.
pub(in crate::ui) fn icon_capsule(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    said: &str,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, id, sense).on_hover_text(said);
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
    icons::paint(
        ui.painter(),
        icon,
        Rect::from_center_size(shown.center(), Vec2::splat(19.0)),
        ink,
    );
    if feel.focused {
        paint_focus_ring(ui.painter(), shown, r as u8);
    }
    response
}

/// How wide an [`info_chip`] with a `dot` of that size and `label` is.
pub(in crate::ui) fn info_chip_width(ui: &Ui, dot: Option<f32>, label: &str) -> f32 {
    let dot_room = dot.map_or(0.0, |d| d + 7.0);
    13.0 + dot_room + measure(ui, label, semibold(t::TYPE_SUB_SIZE)) + 13.0
}

/// A chip over a title: a state or a store. Takes no focus.
pub(in crate::ui) fn info_chip(
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
    pub(in crate::ui) fn button_hints(&self, ctx: &egui::Context, view: &View<'_>) {
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
