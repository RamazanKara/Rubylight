//! Settings row widget rendering, section rail/chips, layer header, search field, and banners.

use super::*;

impl Launcher {
    /// Above a PC's, game's or profile's settings: what its rows mean, how many it sets, and the
    /// way to clear them all.
    pub(super) fn layer_header(&mut self, ui: &mut Ui) {
        let clear = match self.screen {
            Screen::ProfileSettings(_) => en("Clear all in this profile"),
            Screen::HostSettings(_) => en("Clear all for this PC"),
            _ => en("Clear all for this game"),
        };
        // The rows set here, and the settings set here that have no row (beam racing set from the
        // stream menu): Clear all clears those too.
        let rows = SECTION_KEYS.iter().flat_map(|keys| keys.iter());
        let in_rows = rows.clone().filter(|keys| self.edit_overrides.has(keys)).count();
        let values = serde_json::to_value(&self.edit_overrides).expect("overrides serialize");
        let without_row = values.as_object().map_or(0, |values| values.keys().filter(|key| !rows.clone().any(|keys| keys.contains(&key.as_str()))).count());
        let set = in_rows + without_row;
        let count = match set {
            0 => tr("Nothing is set here yet.").to_owned(),
            1 => tr("1 setting is set here.").to_owned(),
            n => trf("{} settings are set here.", &[&n]),
        };
        let hidden = match without_row {
            0 => None,
            1 => Some(tr("1 of them has no row on this page; Clear all clears it too.").to_owned()),
            n => Some(trf("{} of them have no row on this page; Clear all clears them too.", &[&n])),
        };
        let confirm = &mut self.settings_view.confirm_reset;
        let mut cleared = false;
        banner(ui, |ui| {
            ui.label(RichText::new(count).font(regular(text::LABEL)).color(ON_SURFACE));
            if let Some(hidden) = hidden {
                ui.label(RichText::new(hidden).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
            }
        }, |ui| {
            if *confirm {
                if primary_button(ui, "Clear all").clicked() {
                    cleared = true;
                    *confirm = false;
                }
                let cancel = quiet_button(ui, "Cancel");
                if cancel.clicked() {
                    *confirm = false;
                }
                cancel
            } else {
                let button = ui.add_enabled_ui(set > 0, |ui| quiet_button(ui, clear)).inner;
                if button.clicked() {
                    *confirm = true;
                }
                button
            }
        });
        if cleared {
            self.edit_overrides = Overrides::default();
            self.edit_settings = self.inherited_settings.clone();
        }
    }

    /// The search field: a magnifier, the words, and a way to clear them. Ctrl+F reaches it.
    pub(super) fn search_field(&mut self, ui: &mut Ui) {
        let view = &mut self.settings_view;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover());
        let focused = ui.memory(|m| m.has_focus(search_id()));
        let hovered = ui.rect_contains_pointer(rect);
        let round = rect.height() / 2.0;
        let mut shapes = Vec::new();
        glass::control(&mut shapes, ui.ctx(), rect, round);
        lift_shapes(&mut shapes, ui.ctx(), rect, round, focused || hovered, false);
        ui.painter().extend(shapes);
        // The magnifier: a ring and its handle.
        let glass = Pos2::new(rect.left() + 22.0, rect.center().y - 1.5);
        let stroke = Stroke::new(2.0, *ON_SURFACE_VARIANT);
        ui.painter().circle_stroke(glass, 6.0, stroke);
        ui.painter().line_segment([glass + vec2(4.5, 4.5), glass + vec2(9.0, 9.0)], stroke);
        let clear_w = if view.query.is_empty() { 0.0 } else { CONTROL_HEIGHT };
        let field = Rect::from_min_max(Pos2::new(rect.left() + 40.0, rect.top()), Pos2::new(rect.right() - 8.0 - clear_w, rect.bottom()));
        let edit = ui
            .scope_builder(UiBuilder::new().max_rect(field).layout(Layout::left_to_right(Align::Center)), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut view.query)
                        .id(search_id())
                        .frame(Frame::NONE)
                        .font(regular(text::BODY))
                        .text_color(*ON_SURFACE)
                        .hint_text(RichText::new(tr("Search settings")).color(OUTLINE))
                        .desired_width(field.width())
                        // The words and the hint on the magnifier's line, not at the field's top.
                        .min_size(vec2(field.width(), field.height()))
                        .align(egui::Align2::LEFT_CENTER)
                        .char_limit(60)
                        // Up and Down leave the field, as from any other control.
                        .event_filter(egui::EventFilter { horizontal_arrows: true, ..Default::default() }),
                )
            })
            .inner;
        name_for_readers(ui, &edit, en("Search settings"), en("Finds settings in every section by their name or description."));
        if edit.changed() {
            view.to_top = true;
            view.confirm_reset = false;
        }
        // The shortcut, where the field is empty and waiting, and only where the placeholder ends
        // before it: a long one (French) in a narrow column keeps the field to itself.
        if view.query.is_empty() && !focused {
            let key = ui.painter().layout_no_wrap(tr("Ctrl+F").to_owned(), regular(text::CAPTION), *OUTLINE);
            let at = Pos2::new(rect.right() - 14.0 - key.size().x, rect.center().y - key.size().y / 2.0);
            let hint = ui.painter().layout_no_wrap(tr("Search settings").to_owned(), regular(text::BODY), *OUTLINE).size().x;
            if at.x > field.left() + hint.max(120.0) + 16.0 {
                ui.painter().galley(at, key, *OUTLINE);
            }
        }
        if !view.query.is_empty() {
            let spot = Rect::from_center_size(Pos2::new(rect.right() - CONTROL_HEIGHT / 2.0 - 2.0, rect.center().y), Vec2::splat(32.0));
            let clear = ui.interact(spot, search_id().with("clear"), Sense::click());
            clear.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Clear search")));
            if clear.hovered() || clear.has_focus() {
                ui.painter().rect_filled(spot, CornerRadius::same(16), *SURFACE_HIGHEST);
            }
            let c = spot.center();
            let cross = Stroke::new(2.0, *ON_SURFACE);
            ui.painter().line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], cross);
            ui.painter().line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], cross);
            focus_ring(ui, spot, &clear, 16);
            if clear.clicked() {
                view.query.clear();
                view.to_top = true;
                edit.request_focus();
            }
        }
        if edit.has_focus() {
            focus_ring_around(ui, rect, round as u8);
        }
        view.search_focused = edit.has_focus();
    }

    /// Profiles, then the sections, the current one marked. While searching each that found rows
    /// says how many, and a click on any ends the search; on a PC's, game's or profile's settings
    /// each says how many it sets. Returns each section entry's id, and whether Profiles was chosen.
    pub(super) fn section_list(&mut self, ui: &mut Ui, sections: &[&'static str], layered: bool, commands: &mut Vec<Command>) -> (Vec<(&'static str, Id)>, bool) {
        let searching = self.settings_view.searching();
        let mut entries = Vec::new();
        let under = ui.painter().add(egui::Shape::Noop);
        let top = ui.cursor().top();
        ui.add_space(SIDE_INSET);
        // Simple first: the sections most people need, then More settings, which unfolds Profiles
        // (a page of its own, as on Android) and the sections of detail under it.
        let mut profiles = false;
        let open = layered || self.settings_view.rail_open();
        let mut toggled = false;
        let ordered: Vec<&'static str> = if layered {
            sections.to_vec()
        } else {
            sections.iter().copied().filter(|s| !rail_folded(s)).chain(sections.iter().copied().filter(|s| rail_folded(s))).collect()
        };
        let mut fold_drawn = layered;
        for name in &ordered {
            if !fold_drawn && rail_folded(name) {
                fold_drawn = true;
                if !searching {
                    let more = rail_fold_item(ui, open);
                    self.focus_item(&more, "rail-more", false);
                    toggled = more.clicked();
                }
                if open {
                    let item = section_item(ui, en("Profiles"), false, Badge::None, egui::Role::Link);
                    name_for_readers(ui, &item, en("Profiles"), en("Different settings for different networks."));
                    self.focus_item(&item, "profiles-link", false);
                    profiles = item.clicked();
                }
            }
            if !open && rail_folded(name) {
                continue;
            }
            let at = section_index(name);
            let badge = if searching {
                Badge::Found(self.settings_view.found[at])
            } else if layered {
                Badge::Set(SECTION_KEYS[at].iter().filter(|keys| self.edit_overrides.has(keys)).count())
            } else {
                Badge::None
            };
            let current = !searching && self.settings_view.section == *name;
            let item = section_item(ui, name, current, badge, egui::Role::Tab);
            self.focus_item(&item, &format!("section-{name}"), false);
            entries.push((*name, item.id));
            if item.clicked() {
                self.settings_view.open(name);
            }
        }
        if toggled {
            self.toggle_more_settings(commands);
        }
        ui.add_space(SIDE_INSET - ui.spacing().item_spacing.y);
        let list = Rect::from_min_max(Pos2::new(ui.max_rect().left(), top), Pos2::new(ui.max_rect().right(), ui.cursor().top()));
        let mut shapes = Vec::new();
        pcs::canvas_on_moon(&mut shapes, ui.ctx(), list, f32::from(radius::PANEL));
        glass::group(&mut shapes, ui.ctx(), list, f32::from(radius::PANEL), rows_fill());
        ui.painter().set(under, egui::Shape::Vec(shapes));
        (entries, profiles)
    }
}

/// A setting's row, or None while it is not shown (another section, a closed fold, a setting it
/// depends on, or the search passes it). `keys` empty: an action row (`Rows::action`), with no
/// default and no layer box.
pub(in crate::ui) fn setting_row(
    ui: &mut Ui,
    settings: &mut Settings,
    rows: &mut Rows<'_>,
    keys: &[&'static str],
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, &mut Settings) -> Response,
) -> Option<Response> {
    let (action, mut note, room) = (rows.action.take(), rows.note.take(), std::mem::take(&mut rows.room));
    let (reason, held) = (rows.idle.take(), std::mem::take(&mut rows.held));
    // A row that does nothing now still shows while it holds another value than its default, or
    // the PC's, game's or profile's layer sets it, so the value can be seen and set back.
    // Worked out only for a row that does nothing now: it formats both values.
    let changed = reason.is_some()
        && (held
            || keys.first().is_some_and(|key| {
                value_text(key, settings, &rows.display, rows.style) != value_text(key, &rows.defaults, &rows.display, rows.style)
            })
            || rows.layer.as_ref().is_some_and(|(_, overrides)| !keys.is_empty() && overrides.has(keys)));
    let id = keys.first().copied().or(action).unwrap_or_default();
    // A row whose own control had focus or was dragged when last drawn stays while that control still
    // has it, so setting it back to its default does not take the row from under the slider or the focus.
    let kept_id = egui::Id::unique(("setting row in use", id));
    let in_use_id = ui.ctx().data(|d| d.get_temp::<egui::Id>(kept_id));
    let kept = reason.is_some()
        && in_use_id.is_some_and(|c| ui.ctx().dragged_id() == Some(c) || ui.ctx().memory(|m| m.focused() == Some(c)));
    let has_reason = reason.is_some();
    let idle = has_reason && !changed && !kept;
    if !rows.shows(ui, id, label, description, idle) {
        if in_use_id.is_some() {
            ui.ctx().data_mut(|d| d.remove::<egui::Id>(kept_id));
        }
        return None;
    }
    if let Some(reason) = reason.filter(|_| note.is_none()) {
        note = Some((reason, *TERTIARY));
    }
    let first = keys.first().copied();
    let second = first.is_some_and(|k| rows.reset_drawn.contains(&k));
    if let Some(k) = first {
        rows.reset_drawn.push(k);
    }
    let note = note.as_ref();
    let response = match (rows.layer.as_mut(), first) {
        (Some((inherited, overrides)), Some(key)) => {
            let inherited = *inherited;
            let own = overrides.has(keys);
            let state = if own {
                (trf("Otherwise {}", &[&value_text(key, inherited, &rows.display, rows.style)]), *MOON)
            } else {
                let from = rows.sources.iter().find(|(layer, _)| layer.has(keys)).map(|(_, from)| from.as_str());
                (from.unwrap_or(tr("From main settings")).to_owned(), *ON_SURFACE_VARIANT)
            };
            let box_label = rows.own;
            settings_row(ui, label, description, note, Some(state), own, room, |ui| {
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    ui.set_width(CONTROL_W);
                    // Choosing another value ticks the box, opening a menu and leaving it changes
                    // nothing. Read before the control: a number field consumes the arrow keys that step it.
                    let (before, chose) = (settings.clone(), chosen(ui));
                    let value = control(ui, settings);
                    name_for_readers(ui, &value, label, description);
                    // The box on the row's last line, beside where its value comes from, however
                    // short the control.
                    let room = ui.available_height() - (ui.spacing().interact_size.y + ui.text_style_height(&egui::TextStyle::Small)) / 2.0;
                    if room > 0.0 && room.is_finite() {
                        ui.add_space(room);
                    }
                    let mut ticked = own || (*settings != before && chose);
                    let boxed = layer_box(ui, &mut ticked, box_label);
                    name_for_readers(ui, &boxed, box_label, label);
                    if boxed.changed() && !ticked {
                        overrides.clear(keys);
                        *settings = overrides.apply(inherited);
                    } else if ticked && (boxed.changed() || *settings != before) {
                        // Only on a change: a row over several keys leaves a layer that holds some
                        // of them, as older versions saved it, as it is until a value is chosen.
                        // Ticking the box takes the whole row; a chosen value, the keys it sets.
                        overrides.capture(settings, if boxed.changed() { keys } else { picked_keys(keys, settings) });
                    }
                    value.union(boxed)
                }).inner
            })
        }
        (None, Some(key)) => {
            let default = value_text(key, &rows.defaults, &rows.display, rows.style);
            let differs = !second && value_text(key, settings, &rows.display, rows.style) != default;
            let reset_w = ui.painter().layout_no_wrap(tr("Reset").to_owned(), semibold(text::LABEL), *ON_SURFACE).size().x + 24.0;
            let mut reset = false;
            let response = settings_row(ui, label, description, note, None, false, reset_w + room, |ui| {
                let value = control(ui, settings);
                name_for_readers(ui, &value, label, description);
                if differs {
                    let button = text_button(ui, "Reset").on_hover_text(trf("Default: {}", &[&default]));
                    let name = trf("Reset {} to its default, {}", &[&tr(label), &default]);
                    ui.ctx().accesskit_node_builder(button.id, |node| node.set_label(name));
                    reset = button.clicked();
                }
                value
            });
            if reset {
                let mut defaults = Overrides::default();
                defaults.capture(&rows.defaults, keys);
                *settings = defaults.apply(settings);
            }
            response
        }
        _ => settings_row(ui, label, description, note, None, false, room, |ui| {
            let value = control(ui, settings);
            name_for_readers(ui, &value, label, description);
            value
        }),
    };
    if std::mem::take(&mut rows.focus_first) {
        response.request_focus();
    }
    if has_reason {
        // The control that has the focus or the drag, if it is this row's: a slider's rail or its number
        // field, which have ids of their own.
        let ctx = ui.ctx().clone();
        let in_use = [ctx.memory(|m| m.focused()), ctx.dragged_id()].into_iter().flatten()
            .find(|held| ctx.read_response(*held).is_some_and(|r| response.rect.intersects(r.rect)));
        ctx.data_mut(|d| match in_use {
            Some(held) => {
                d.insert_temp(kept_id, held);
            }
            None => {
                d.remove::<egui::Id>(kept_id);
            }
        });
    }
    Some(response)
}

/// A row: its label on the left, the control with the value on the right. The hint shows under
/// the label while the pointer is over the row or focus is in it: one line, cut, with the whole
/// text as a tooltip, or wrapped whole (the row grows for it) for a row keys or a controller
/// focused. A note shows
/// always. A row a PC, game or profile sets has an amber bar at its left edge and, under its
/// label, where its value comes from beside the layer's box. `reset_w` keeps room left of the
/// control for a Reset, so the text never moves when it comes and goes.
/// Where the settings page says whether keys or a controller are in the hand, for [`settings_row`].
pub(super) const KEYS_IN_HAND: &str = "settings-keys-in-hand";

#[allow(clippy::too_many_arguments)]
pub(super) fn settings_row(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    note: Option<&(String, Color32)>,
    state: Option<(String, Color32)>,
    own: bool,
    reset_w: f32,
    control: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    const GAP: f32 = 2.0;
    let mut out = None;
    let rect = group_row(ui, |ui| {
        ui.horizontal(|ui| {
            let text_w = (ui.available_width() - CONTROL_W - reset_w - space::L).max(180.0);
            let painter = ui.painter().clone();
            let label_g = painter.layout(tr(label).to_owned(), semibold(text::BODY), *ON_SURFACE, text_w);
            // With keys or a controller in the hand the focused row's hint wraps whole and the row
            // grows for it. Under a pointer no row changes size, or the rows below would move away
            // from under it: the hint keeps one line and its whole text comes as a tooltip.
            let open_id = Id::unique(("settings-hint-open", label));
            let was_open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
            let hint_g = if was_open {
                painter.layout(tr(hint).to_owned(), regular(text::CAPTION), *ON_SURFACE_VARIANT, text_w)
            } else {
                let mut job = egui::text::LayoutJob::simple_singleline(tr(hint).to_owned(), regular(text::CAPTION), *ON_SURFACE_VARIANT);
                job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
                painter.layout_job(job)
            };
            let note_g = note.map(|(text, color)| painter.layout(text.clone(), regular(text::CAPTION), *color, text_w));
            let state_g = state.as_ref().map(|(text, color)| painter.layout_no_wrap(text.clone(), regular(text::CAPTION), *color));
            let (label_h, hint_h) = (label_g.size().y, hint_g.size().y);
            let note_h = note_g.as_ref().map_or(0.0, |g| GAP + g.size().y);
            let full = label_h + GAP + hint_h + note_h;
            // Where a list choice's box lands under it: where the value comes from sits beside it.
            let to_state = (COMBO_HEIGHT + 18.0).max(full + GAP);
            let height = state_g.as_ref().map_or(full, |g| to_state + g.size().y);
            let (text_rect, text) = ui.allocate_exact_size(vec2(text_w, height), Sense::hover());
            if state.is_some() {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| out = Some(control(ui)));
            } else {
                // Centred on the row as it is with a one-line hint: a hint that wraps grows the row
                // downwards and the control stays under the pointer that opened it.
                let wrapped = hint_h - hint_h / hint_g.rows.len().max(1) as f32;
                let free = ui.available_rect_before_wrap();
                let slot = Rect::from_min_max(Pos2::new(free.left(), text_rect.top()), Pos2::new(free.right(), text_rect.bottom() - wrapped));
                ui.scope_builder(UiBuilder::new().max_rect(slot).layout(Layout::right_to_left(Align::Center)), |ui| out = Some(control(ui)));
            }
            // The hint while the pointer is over the row or focus is in it.
            let row = ui.min_rect().expand2(vec2(18.0, 8.0));
            let focused = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| row.contains(r.rect.center()));
            let active = focused || ui.rect_contains_pointer(row);
            let open = focused && ui.data(|d| d.get_temp::<bool>(Id::unique(KEYS_IN_HAND))).unwrap_or(false);
            if open != was_open {
                // One more pass lays the hint out whole, or back to one line.
                ui.data_mut(|d| d.insert_temp(open_id, open));
                ui.ctx().request_repaint();
            }
            if hint_g.elided {
                text.on_hover_text(tr(hint).to_owned());
            }
            let t = spring::animate(ui.ctx(), Id::unique(("settings-hint", label)), if active { 1.0 } else { 0.0 }, spring::FOCUS).clamp(0.0, 1.0);
            let content = ui.min_rect();
            // A layered row keeps its lines in place; another one centres its label until the hint comes.
            let top = if state.is_some() {
                content.top()
            } else {
                content.center().y - (label_h + note_h + t * (GAP + hint_h)) / 2.0
            };
            painter.galley(Pos2::new(text_rect.left(), top), label_g, *ON_SURFACE);
            let mut below = top + label_h + GAP;
            if t > 0.01 {
                painter.galley_with_override_text_color(Pos2::new(text_rect.left(), below), hint_g, ON_SURFACE_VARIANT.gamma_multiply(t));
            }
            below += if state.is_some() { hint_h + GAP } else { t * (hint_h + GAP) };
            if let Some(note_g) = note_g {
                let color = note.map_or(*ON_SURFACE_VARIANT, |(_, c)| *c);
                painter.galley(Pos2::new(text_rect.left(), below), note_g, color);
            }
            if let (Some(state_g), Some((_, color))) = (state_g, &state) {
                painter.galley(Pos2::new(text_rect.left(), content.top() + to_state), state_g, *color);
            }
        });
    });
    if own {
        let bar = Rect::from_min_size(Pos2::new(rect.left() + 6.0, rect.top() + 12.0), vec2(3.0, rect.height() - 24.0));
        ui.painter().rect_filled(bar, CornerRadius::same(2), MOON);
    }
    out.expect("control drawn")
}

/// A layered row's box, "For this PC" and the like: outlined while unticked, so it shows on the
/// row's surface, amber with a check once ticked. The words beside it click it too.
pub(super) fn layer_box(ui: &mut Ui, ticked: &mut bool, label: &'static str) -> Response {
    const SIDE: f32 = 18.0;
    let words = ui.painter().layout_no_wrap(tr(label).to_owned(), regular(text::BODY), *ON_SURFACE);
    let size = vec2(SIDE + space::S + words.size().x, ui.spacing().interact_size.y.max(words.size().y));
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *ticked = !*ticked;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *ticked, tr(label)));
    let square = Rect::from_center_size(Pos2::new(rect.left() + SIDE / 2.0, rect.center().y), Vec2::splat(SIDE));
    let painter = ui.painter();
    if *ticked {
        painter.rect_filled(square, CornerRadius::same(4), MOON);
        let check = Stroke::new(2.0, *ON_MOON);
        let c = square.center();
        painter.add(egui::Shape::line(vec![c + vec2(-4.5, 0.0), c + vec2(-1.5, 3.5), c + vec2(4.5, -3.5)], check));
    } else {
        let fill = if response.hovered() || response.has_focus() { *SURFACE_HIGH } else { *SURFACE };
        painter.rect(square, CornerRadius::same(4), fill, Stroke::new(1.5, *OUTLINE), StrokeKind::Inside);
    }
    painter.galley(Pos2::new(square.right() + space::S, rect.center().y - words.size().y / 2.0), words, *ON_SURFACE);
    focus_ring(ui, rect, &response, radius::SMALL);
    response
}

/// Whether this frame's input could have chosen a value: a click, a drag, a key, typing or a
/// screen reader's action. A range row clamping a value it was handed as it is drawn chooses nothing.
pub(super) fn chosen(ui: &Ui) -> bool {
    ui.input(|i| {
        i.pointer.any_released() || i.pointer.any_down()
            || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::AccessKitActionRequest(_)))
    })
}

/// What a section's entry says on its right.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Badge {
    None,
    /// Rows the search found in it.
    Found(usize),
    /// Rows a PC, game or profile sets in it.
    Set(usize),
}

/// A section's mark in the list beside the rows, the same glyph as Android's (`SettingsRail.iconOf`).
pub(super) fn section_icon(name: &str) -> Icon {
    match name {
        n if n == en("Profiles") => Icon::Folder,
        n if n == en("Recommended") => Icon::Star,
        n if n == en("Video") => Icon::Video,
        n if n == en("Display & image") => Icon::Monitor,
        n if n == en("Controls") => Icon::Gamepad,
        n if n == en("Audio") => Icon::Speaker,
        n if n == en("Streaming") => Icon::Network,
        n if n == en("App") => Icon::App,
        n if n == en("Advanced") => Icon::Sliders,
        _ => Icon::Terminal,
    }
}

/// An entry in the list beside the rows (a section, or Profiles above them): the current one in
/// the selection's tint. `role` is what a screen reader calls it.
pub(super) fn section_item(ui: &mut Ui, name: &str, current: bool, badge: Badge, role: egui::Role) -> Response {
    let width = ui.max_rect().width() - 2.0 * SIDE_INSET;
    let (_, slot) = ui.allocate_space(vec2(ui.available_width(), SIDE_ITEM_H));
    let rect = Rect::from_min_size(Pos2::new(ui.max_rect().left() + SIDE_INSET, slot.top()), vec2(width, SIDE_ITEM_H));
    let response = ui.interact(rect, ui.next_auto_id(), Sense::click());
    ui.skip_ahead_auto_ids(1);
    let words = tr(name);
    let spoken = match badge {
        Badge::Found(n) => trf("{}, {} found", &[&words, &n]),
        Badge::Set(n) if n > 0 => trf("{}, {} set here", &[&words, &n]),
        _ => words.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::selected(role, true, current, &spoken));
    let pressed = response.is_pointer_button_down_on();
    let round = f32::from(radius::CONTROL);
    let mut shapes = Vec::new();
    if current {
        glass::solid(&mut shapes, ui.ctx(), rect, round, if high_contrast() { *MOON_CONTAINER } else { design_tokens::TINT_SELECTED_FILL });
    }
    lift_shapes(&mut shapes, ui.ctx(), rect, round, response.hovered(), pressed);
    let painter = ui.painter();
    painter.extend(shapes);
    // While searching, a section with rows found stands out; one without stays as dim as any
    // other entry, with no count.
    let found = matches!(badge, Badge::Found(n) if n > 0);
    let color = if current {
        *ON_MOON_CONTAINER
    } else if found {
        *ON_SURFACE
    } else {
        *ON_SURFACE_VARIANT
    };
    let font = if current { semibold(text::BODY) } else { regular(text::BODY) };
    icons::paint(painter, section_icon(name), Rect::from_center_size(Pos2::new(rect.left() + 22.0, rect.center().y), Vec2::splat(20.0)), color);
    let galley = painter.layout_no_wrap(words.to_owned(), font, color);
    painter.galley(Pos2::new(rect.left() + 44.0, rect.center().y - galley.size().y / 2.0), galley, color);
    let count = match badge {
        Badge::Found(n) if n > 0 => Some((n.to_string(), *ON_SURFACE_VARIANT, None)),
        Badge::Set(n) if n > 0 => Some((n.to_string(), *ON_MOON_CONTAINER, Some(*MOON_CONTAINER))),
        _ => None,
    };
    if let Some((count, color, pill)) = count {
        let galley = painter.layout_no_wrap(count, semibold(text::CAPTION), color);
        let size = vec2((galley.size().x + 12.0).max(22.0), 20.0);
        let spot = Rect::from_center_size(Pos2::new(rect.right() - 10.0 - size.x / 2.0, rect.center().y), size);
        if let Some(pill) = pill {
            painter.rect_filled(spot, CornerRadius::same(10), pill);
        }
        painter.galley(spot.center() - galley.size() / 2.0, galley, color);
    }
    focus_ring(ui, rect, &response, radius::CONTROL);
    response
}

/// "More settings" in the list beside the rows: an entry without a page, with a chevron where a
/// section has its icon, down to unfold Profiles and the sections of detail and up to fold them.
pub(super) fn rail_fold_item(ui: &mut Ui, open: bool) -> Response {
    let width = ui.max_rect().width() - 2.0 * SIDE_INSET;
    let (_, slot) = ui.allocate_space(vec2(ui.available_width(), SIDE_ITEM_H));
    let rect = Rect::from_min_size(Pos2::new(ui.max_rect().left() + SIDE_INSET, slot.top()), vec2(width, SIDE_ITEM_H));
    let response = ui.interact(rect, ui.next_auto_id(), Sense::click());
    ui.skip_ahead_auto_ids(1);
    let words = tr("More settings");
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, words));
    let round = f32::from(radius::CONTROL);
    let mut shapes = Vec::new();
    lift_shapes(&mut shapes, ui.ctx(), rect, round, response.hovered(), response.is_pointer_button_down_on());
    let painter = ui.painter();
    painter.extend(shapes);
    let color = *ON_SURFACE_VARIANT;
    let c = Pos2::new(rect.left() + 22.0, rect.center().y);
    let (tip, wing) = if open { (-3.5, 3.5) } else { (3.5, -3.5) };
    painter.add(egui::Shape::line(vec![c + vec2(-6.0, wing), c + vec2(0.0, tip), c + vec2(6.0, wing)], Stroke::new(2.0, color)));
    let galley = painter.layout_no_wrap(words.to_owned(), regular(text::BODY), color);
    painter.galley(Pos2::new(rect.left() + 44.0, rect.center().y - galley.size().y / 2.0), galley, color);
    focus_ring(ui, rect, &response, radius::CONTROL);
    response
}

/// A section's chip on a narrow window, where the list beside the rows has no room, and a preset's
/// chip. The current one differs only in colour: the same size and type as the others.
pub(super) fn section_chip(ui: &mut Ui, name: &str, current: bool) -> Response {
    if !current {
        return chip(ui, name);
    }
    button(ui, None, tr(name), Tone::Selected, Size::CHIP)
}

/// A card across the page above the rows: words on the left, an action on the right.
pub(super) fn banner(ui: &mut Ui, words: impl FnOnce(&mut Ui), action: impl FnOnce(&mut Ui) -> Response) -> Response {
    let mut out = None;
    group_row(ui, |ui| {
        // The words take the action's middle. A column starts at its top, so how far down comes from
        // their height, known from the last pass.
        let memory = ui.next_auto_id().with("banner-words");
        let tall = ui.ctx().data(|d| d.get_temp::<f32>(memory)).unwrap_or(0.0);
        let size = vec2(ui.available_width(), CONTROL_HEIGHT);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 300.0).max(200.0));
                ui.add_space(((CONTROL_HEIGHT - tall) / 2.0).max(0.0));
                let height = ui.vertical(words).response.rect.height();
                if (height - tall).abs() > 0.5 {
                    ui.ctx().data_mut(|d| d.insert_temp(memory, height));
                    ui.ctx().request_repaint();
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| out = Some(action(ui)));
        });
    });
    ui.add_space(space::M);
    out.expect("action drawn")
}

/// A notice above the main settings, with the button that deals with it.
pub(super) fn notice(ui: &mut Ui, text: &str, button: &'static str) -> Response {
    banner(ui, |ui| {
        ui.label(RichText::new(text).font(regular(text::LABEL)).color(ON_SURFACE));
    }, |ui| quiet_button(ui, button))
}
