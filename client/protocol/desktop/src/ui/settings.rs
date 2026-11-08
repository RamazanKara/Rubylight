//! The settings page: a list of sections on the left (chips on a narrow window) and the open
//! section's rows on the right, everyday rows first and the rarely needed ones in labelled groups
//! after them, never folded away. A search finds rows in every section. A PC's, game's or
//! profile's settings show the same rows, each with a box that keeps its value for that layer.

use super::*;
use nightjar_policy::stream_menu::{Hotkey, NO_TILE};

/// The settings sections in the order Android lists them (App only on the main settings).
pub(super) const SETTINGS_SECTIONS: [&str; 8] = [en("Video"), en("Display & image"), en("Controls"), en("Audio"), en("Streaming"), en("App"), en("Advanced"), en("Debug")];

/// Every setting row's keys, by section, in the order the page draws them; the first key names
/// the row. Rows that show only with another setting (Custom resolution, Bitrate, the controller
/// mouse's options) share their keys or are listed where they show. A test keeps this equal to
/// what the page draws.
pub(super) const SECTION_KEYS: [&[&[&str]]; 8] = [
    &[&["width", "height"], &["fps"], &["bitrate_kbps"], &["adaptive_bitrate"], &["codec"], &["hdr"],
        &["adaptive_quality"], &["quality_min_short_edge"], &["quality_min_fps"], &["upscaling"], &["upscale_sharpness"],
        &["prefer_yuv444"], &["sdr_10bit"], &["pyrowave_decoder"]],
    &[&["window_mode"], &["video_scaling"], &["vrr"],
        &["virtual_display"], &["host_display"], &["virtual_display_scale"], &["virtual_display_dpi"],
        &["align_video_top"], &["present_at_host_times"], &["frame_pacing"], &["vsync"]],
    &[&["mouse_mode"], &["controllers"], &["swap_face_buttons"], &["clipboard_sync"], &["guide_hold_ms"], &["back_opens_menu"],
        &["capture_system_keys"], &["mouse_speed"], &["mouse_nav_buttons"], &["native_touchpad"], &["right_alt_as_meta"],
        &["gamepad_quit_combo"], &["stream_menu_buttons"], &["controller_mouse"], &["analog_scrolling"], &["mouse_fixed_step"],
        &["gamepad_touchpad_as_mouse"], &["disable_rumble"], &["controller_motion"], &["controller_audio"], &["background_controllers"]],
    &[&["audio"], &["play_audio_on_host"], &["microphone"], &["mic_echo_cancel"], &["mute_audio"], &["audio_passthrough"]],
    &[&["overlay", "overlay_mode"], &["overlay_position"], &["auto_reconnect"],
        &["client_roaming"], &["roaming_metered"], &["network_grace_ms"], &["usb_forwarding"]],
    &[&["console_home"], &["quick_resume"], &["language"], &["open_on_start"], &["ui_zoom"], &["high_contrast"], &["reduce_motion"],
        &["input_only_any_host"], &["check_updates"]],
    &[&["experimental_packed_444"], &["link_probe"]],
    &[&["error_correction"], &["client_hint_bitrate"], &["tell_host_wireless"], &["tell_host_link_speed"],
        &["network_priority"], &["unbatched_input"], &["full_range"], &["decoder"], &["renderer"], &["cursor_shapes"]],
];

/// The Resolution menu after Native: Android's 16:9 sizes with common PC and laptop ones, then
/// 16:10, then ultrawide. Custom… takes any other size.
pub(super) const RESOLUTIONS: [(u32, u32); 15] = [
    (640, 360), (854, 480), (1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160),
    (1280, 800), (1920, 1200), (2560, 1600), (3840, 2400),
    (2560, 1080), (3440, 1440), (5120, 1440),
];

/// The Frame rate menu after Display: Android's 30-120 and common monitor rates. Custom… takes
/// any rate in `CUSTOM_FPS`, as the stream menu's rate does.
pub(super) const FRAME_RATES: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];
pub(super) const CUSTOM_FPS: std::ops::RangeInclusive<u32> = 10..=500;

/// The page lays the section list beside the rows from this width on; below it, chips.
const SIDE_LIST_FROM: f32 = 940.0;
const SIDE_LIST_W: f32 = 236.0;
const SIDE_INSET: f32 = 6.0;
const SIDE_ITEM_H: f32 = 44.0;
/// The width a row's control column takes, as on the other settings pages.
const CONTROL_W: f32 = 240.0;

/// The settings page's own state between frames.
#[derive(Default)]
pub(super) struct SettingsView {
    /// The section shown while not searching; empty for the first.
    pub(super) section: &'static str,
    /// The search, as typed.
    pub(super) query: String,
    /// The search the rows showed last frame: another one starts its results at the top.
    shown_query: String,
    /// The rows the search found last frame, per section.
    pub(super) found: [usize; 8],
    /// Another section was opened: its rows start at the top.
    pub(super) to_top: bool,
    /// Reset all (or Clear all on a PC's, game's or profile's settings) waits for its confirmation.
    pub(super) confirm_reset: bool,
    /// The search field had focus last frame, so Escape leaves it before it leaves the page.
    search_focused: bool,
    /// The first row drawn takes focus next frame (the search field was left); with none drawn,
    /// the open section's entry does.
    focus_rows: bool,
    /// Left was pressed last frame on this control in the rows, laid out there.
    left_from: Option<(Id, Rect)>,
    /// The setting rows the last frame drew, by first key, each with whether a group heading
    /// came before it in its section.
    pub(super) drawn: Vec<(&'static str, bool)>,
}

impl SettingsView {
    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn open(&mut self, section: &'static str) {
        if self.section != section || self.searching() {
            self.to_top = true;
        }
        self.section = section;
        self.query.clear();
        self.confirm_reset = false;
    }
}

fn search_id() -> Id {
    Id::unique("settings-search")
}

/// The search's words in lower case: a row is found when each of them is in its label, its
/// description, its section or its group.
fn search_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn found(words: &[String], texts: &[&str]) -> bool {
    let texts: Vec<String> = texts.iter().map(|t| t.to_lowercase()).collect();
    words.iter().all(|w| texts.iter().any(|t| t.contains(w.as_str())))
}

impl Launcher {
    /// Escape or B on a settings page: closes the search first (leaves its field, then clears
    /// it). True when it did, so the page stays.
    pub(super) fn escape_search(&mut self) -> bool {
        if !matches!(self.screen, Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_)) {
            return false;
        }
        let view = &mut self.settings_view;
        if view.search_focused {
            // The rows take the focus the field gives up, so the D-pad carries on from there.
            (view.search_focused, view.focus_rows) = (false, true);
            view.to_top = true;
            return true;
        }
        if view.searching() || view.confirm_reset {
            view.query.clear();
            view.confirm_reset = false;
            view.to_top = true;
            return true;
        }
        false
    }

    pub(super) fn settings_page(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let layered = self.screen != Screen::Settings;
        let sections: Vec<&'static str> = SETTINGS_SECTIONS.into_iter().filter(|s| !(layered && *s == "App")).collect();
        if !sections.contains(&self.settings_view.section) {
            self.settings_view.section = sections[0];
        }
        // A page opened anew starts without the last visit's search; the section stays.
        if self.focus_screen && self.restore_key.is_none() {
            let view = &mut self.settings_view;
            view.query.clear();
            (view.confirm_reset, view.to_top) = (false, true);
        }
        // Not under a dialog, which keeps the focus as long as it is open.
        if matches!(self.dialog, Dialog::None) && ui.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
            ui.memory_mut(|m| m.request_focus(search_id()));
        }
        // A changed search, typed or set, shows its results from the top, heading and all.
        if self.settings_view.query != self.settings_view.shown_query {
            self.settings_view.shown_query.clone_from(&self.settings_view.query);
            self.settings_view.to_top = true;
        }
        let to_top = if std::mem::take(&mut self.settings_view.to_top) { ToTop::Now } else { self.chrome_to_top() };
        let open = self.settings_view.section;
        self.page_scrolled = page_scroll(ui, ("settings", open), to_top, |ui| self.settings_body(ui, view, &sections, layered, commands));
    }

    /// What scrolls on a settings page. The side list stays in its place below the chrome while the
    /// rows go by, for as long as the window has the room for all of it.
    fn settings_body(&mut self, ui: &mut Ui, view: &View<'_>, sections: &[&'static str], layered: bool, commands: &mut Vec<Command>) {
        self.header(ui, view, commands);
        if layered {
            self.layer_header(ui);
        } else if let Some(profile) = self.active_profile().map(|p| p.name.clone()) {
            let manage = notice(ui, &trf("Streams on this network use the profile {}. The settings it changes win over the ones here.", &[&profile]), en("Manage profiles"));
            self.focus_item(&manage, "profiles-notice", false);
            if manage.clicked() {
                commands.push(Command::SaveSettings);
                self.open(Screen::Profiles);
                return;
            }
        }
        let wide = ui.available_width() >= SIDE_LIST_FROM;
        let page = ui.available_rect_before_wrap();
        let mut entries: Vec<(&'static str, Id)> = Vec::new();
        let mut side_bottom = page.top();
        let rows_rect = if wide {
            let height_id = Id::unique("settings-side-height");
            let height = ui.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(0.0);
            let view_top = ui.clip_rect().top() + chrome::PAGE_TOP;
            let fits = height <= ui.clip_rect().bottom() - view_top;
            let top = if fits { page.top().max(view_top) } else { page.top() };
            let list = Rect::from_min_max(Pos2::new(page.left(), top), Pos2::new(page.left() + SIDE_LIST_W, page.bottom()));
            let mut side = ui.new_child(UiBuilder::new().id_salt("settings-sections").max_rect(list).layout(Layout::top_down(Align::Min)));
            self.search_field(&mut side);
            side.add_space(space::M);
            entries = self.section_list(&mut side, sections, layered);
            let drawn = side.min_rect().height();
            ui.data_mut(|d| d.insert_temp(height_id, drawn));
            side_bottom = page.top() + drawn;
            Rect::from_min_max(Pos2::new(page.left() + SIDE_LIST_W + space::XL, page.top()), page.max)
        } else {
            self.search_field(ui);
            ui.add_space(space::S);
            ui.horizontal_wrapped(|ui| {
                for name in sections {
                    let current = !self.settings_view.searching() && self.settings_view.section == *name;
                    let chip = section_chip(ui, name, current);
                    entries.push((*name, chip.id));
                    if chip.clicked() {
                        self.settings_view.open(name);
                    }
                }
            });
            ui.add_space(space::S);
            let rest = ui.available_rect_before_wrap();
            Rect::from_min_max(rest.min, page.max)
        };
        let mut column = ui.new_child(UiBuilder::new().id_salt("settings-rows").max_rect(rows_rect).layout(Layout::top_down(Align::Min)));
        let screen = self.screen;
        self.settings_rows(&mut column, view, commands);
        // Neither child moved the page along: the rows and the list are what it holds.
        let bottom = column.min_rect().bottom().max(side_bottom);
        ui.advance_cursor_after_rect(Rect::from_min_max(page.min, Pos2::new(page.right(), bottom)));
        if self.screen == screen {
            if std::mem::take(&mut self.settings_view.focus_rows) {
                if let Some(&(_, entry)) = entries.iter().find(|&&(name, _)| name == self.settings_view.section) {
                    ui.memory_mut(|m| m.request_focus(entry));
                }
            }
            self.left_to_sections(ui, rows_rect, &entries);
        }
    }

    /// Left from a control in the rows goes to the open section's entry in the list (its chip
    /// on a narrow window), never to whatever egui's nearest-widget search finds above, such as
    /// the header, another chip or another row's Reset. egui moves the focus at the end of the
    /// frame, so a move to anything outside the row it left is corrected on the next one: a
    /// control with another left of it in its row (its Reset) still goes there, and a slider or a
    /// number field keeps the key for itself.
    fn left_to_sections(&mut self, ui: &Ui, rows: Rect, entries: &[(&'static str, Id)]) {
        let focused = ui.memory(|m| m.focused());
        let rect = |id: Id| ui.ctx().read_response(id).map(|r| r.rect);
        let in_rows = |id: Id| rect(id).is_some_and(|r| rows.contains(r.center()));
        if let Some((from, from_rect)) = self.settings_view.left_from.take() {
            let open = entries.iter().find(|&&(name, _)| name == self.settings_view.section);
            // In the same row: beside the control focus left, not above or below it.
            let same_row = |id: Id| in_rows(id) && rect(id).is_some_and(|r| r.top() < from_rect.bottom() && from_rect.top() < r.bottom());
            let strayed = focused.is_some_and(|id| id != from && !same_row(id) && open.is_none_or(|&(_, entry)| entry != id));
            if let (true, Some(&(_, entry))) = (strayed, open) {
                ui.memory_mut(|m| m.request_focus(entry));
                ui.ctx().request_repaint();
            }
        }
        let left = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::ArrowLeft, pressed: true, modifiers, .. } if !modifiers.any())));
        if let Some(from) = focused.filter(|&id| left && in_rows(id)) {
            self.settings_view.left_from = rect(from).map(|r| (from, r));
            // The frame that corrects the move, even when nothing else asks for one.
            ui.ctx().request_repaint();
        }
    }

    /// Above a PC's, game's or profile's settings: what its rows mean, how many it sets, and the
    /// way to clear them all.
    fn layer_header(&mut self, ui: &mut Ui) {
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
    fn search_field(&mut self, ui: &mut Ui) {
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

    /// The sections, the current one marked. While searching each that found rows says how many,
    /// and a click on any ends the search; on a PC's, game's or profile's settings each says how
    /// many it sets. Returns each entry's id.
    fn section_list(&mut self, ui: &mut Ui, sections: &[&'static str], layered: bool) -> Vec<(&'static str, Id)> {
        let searching = self.settings_view.searching();
        let mut entries = Vec::new();
        let under = ui.painter().add(egui::Shape::Noop);
        let top = ui.cursor().top();
        ui.add_space(SIDE_INSET);
        for name in sections {
            let at = SETTINGS_SECTIONS.iter().position(|s| s == name).unwrap_or(0);
            let badge = if searching {
                Badge::Found(self.settings_view.found[at])
            } else if layered {
                Badge::Set(SECTION_KEYS[at].iter().filter(|keys| self.edit_overrides.has(keys)).count())
            } else {
                Badge::None
            };
            let current = !searching && self.settings_view.section == *name;
            let item = section_item(ui, name, current, badge);
            self.focus_item(&item, &format!("section-{name}"), false);
            entries.push((*name, item.id));
            if item.clicked() {
                self.settings_view.open(name);
            }
        }
        ui.add_space(SIDE_INSET - ui.spacing().item_spacing.y);
        let list = Rect::from_min_max(Pos2::new(ui.max_rect().left(), top), Pos2::new(ui.max_rect().right(), ui.cursor().top()));
        let mut shapes = Vec::new();
        pcs::canvas_on_moon(&mut shapes, ui.ctx(), list, f32::from(radius::PANEL));
        glass::group(&mut shapes, ui.ctx(), list, f32::from(radius::PANEL), rows_fill());
        ui.painter().set(under, egui::Shape::Vec(shapes));
        entries
    }

    /// The open section's rows, or every row the search finds.
    fn settings_rows(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let display = self.display;
        let tearing = self.tearing_supported;
        let layered = self.screen != Screen::Settings;
        let searching = self.settings_view.searching();
        let open = self.settings_view.section;
        let mut links: Vec<(&str, Response)> = Vec::new();
        let test_host = if let Screen::HostSettings(id) = self.screen { view.hosts.get(id) } else { None };
        let inherited = self.inherited_settings.clone();
        let overrides = &mut self.edit_overrides;
        let own = match self.screen {
            Screen::HostSettings(_) => en("For this PC"),
            Screen::ProfileSettings(_) => en("In this profile"),
            _ => en("For this game"),
        };
        let style = crate::glyphs::Style::of(view.pads);
        // Coming back from a sub-page, the link that opened it takes focus instead.
        let focus_first = self.focus_screen && self.restore_key.is_none();
        if focus_first {
            self.focus_screen = false;
        }
        let focus_first = focus_first || self.settings_view.focus_rows;
        let mut rows = Rows {
            layer: layered.then_some((&inherited, overrides)),
            sources: &self.inherited_from,
            own,
            words: search_words(&self.settings_view.query),
            open,
            section: "",
            group: "",
            heading: None,
            found: [0; 8],
            enabled: true,
            why: None,
            focus_first,
            reset_drawn: Vec::new(),
            defaults: Settings::default(),
            display,
            style,
            drawn: Vec::new(),
        };
        let screen = self.screen;
        let (custom_size_on, custom_fps_on) = (&mut self.custom_size, &mut self.custom_fps);
        let confirm_reset = &mut self.settings_view.confirm_reset;
        let s = if layered { &mut self.edit_settings } else { &mut self.settings };
        let before = s.clone();
        let (mut reset_all, mut clear_search) = (false, false);
        {
            if searching {
                ui.label(RichText::new(trf("Results for “{}”", &[&self.settings_view.query.trim()])).font(semibold(text::SECTION)).color(PAPER));
            } else {
                // Under a layer's own title and notice, the section is one step down.
                let size = if layered { text::SECTION } else { chrome::PAGE_TITLE };
                ui.label(RichText::new(tr(open)).font(semibold(size)).color(PAPER));
            }
            ui.add_space(space::M);

            rows.section(en("Video"));
            let native = trf("Native ({}x{})", &[&display.width, &display.height]);
            let custom_size = s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height)));
            let current = if s.width == 0 {
                native.clone()
            } else if custom_size {
                trf("Custom ({}x{})", &[&s.width, &s.height])
            } else {
                format!("{}x{}", s.width, s.height)
            };
            let mut chose = None;
            setting_row(ui, s, &mut rows, &["width", "height"], "Resolution", "The picture size the PC sends. Native matches this screen; a smaller size needs less network and PC power.", |ui, s| {
                // Taller than egui's default menu; it scrolls where the window is shorter.
                Combo::from_id_salt("resolution")
                    .selected_text(current)
                    .height(640.0)
                    .show_ui(ui, |ui| {
                        let mut v = (s.width, s.height);
                        if ui.selectable_value(&mut v, (0, 0), native).clicked() {
                            ((s.width, s.height), chose) = (v, Some(false));
                        }
                        for (w, h) in RESOLUTIONS {
                            let mut v = (s.width, s.height);
                            if ui.selectable_value(&mut v, (w, h), format!("{w}x{h}")).clicked() {
                                ((s.width, s.height), chose) = (v, Some(false));
                            }
                        }
                        if ui.selectable_label(custom_size, tr("Custom…")).clicked() {
                            if s.width == 0 { (s.width, s.height) = (display.width, display.height); }
                            chose = Some(true);
                        }
                    })
                    .response
            });
            if let Some(custom) = chose { *custom_size_on = custom.then_some(screen); }
            // Read again after the menu: a row drawn at Native or Display would clamp it to its range.
            // Stepping onto a preset keeps the row.
            if s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height))) {
                let before = (s.width, s.height);
                setting_row(ui, s, &mut rows, &["width", "height"], "Custom resolution", "Width and height in pixels.", |ui, s| {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        step_value(ui, &mut s.width, 320..=7680, 16.0, |d| d);
                        ui.label("×");
                        step_value(ui, &mut s.height, 240..=4320, 16.0, |d| d);
                    }).response
                });
                if (s.width, s.height) != before { *custom_size_on = Some(screen); }
            }
            let display_fps = display.refresh_hz.round() as u32;
            let custom_fps = s.fps != 0 && (*custom_fps_on == Some(screen) || !FRAME_RATES.contains(&s.fps));
            let fps_text = if s.fps == 0 {
                trf("Display ({} Hz)", &[&display_fps])
            } else if custom_fps {
                trf("Custom ({} fps)", &[&s.fps])
            } else {
                trf("{} fps", &[&s.fps])
            };
            let mut chose = None;
            setting_row(ui, s, &mut rows, &["fps"], "Frame rate", "Pictures per second. Display matches this monitor; a higher rate looks smoother and needs a faster network.", |ui, s| {
                Combo::from_id_salt("fps")
                    .selected_text(fps_text)
                    .height(640.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut s.fps, 0, trf("Display ({} Hz)", &[&display_fps])).clicked() { chose = Some(false); }
                        for fps in FRAME_RATES {
                            if ui.selectable_value(&mut s.fps, fps, trf("{} fps", &[&fps])).clicked() { chose = Some(false); }
                        }
                        if ui.selectable_label(custom_fps, tr("Custom…")).clicked() {
                            if s.fps == 0 { s.fps = display_fps.clamp(*CUSTOM_FPS.start(), *CUSTOM_FPS.end()); }
                            chose = Some(true);
                        }
                    })
                    .response
            });
            if let Some(custom) = chose { *custom_fps_on = custom.then_some(screen); }
            if s.fps != 0 && (*custom_fps_on == Some(screen) || !FRAME_RATES.contains(&s.fps)) {
                let before = s.fps;
                setting_row(ui, s, &mut rows, &["fps"], "Custom frame rate", "Pictures per second.", |ui, s| {
                    step_value(ui, &mut s.fps, CUSTOM_FPS, 1.0, |d| d.suffix(" fps"))
                });
                if s.fps != before { *custom_fps_on = Some(screen); }
            }
            let (w, h, fps, bitrate) = stream_mode(s, display);
            let recommended = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, display).3;
            let mut automatic = s.bitrate_kbps == 0;
            setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Automatic bitrate",
                &trf("Nightjar picks how much video data to send: {} Mbps for {}x{} at {} fps. Turn off to choose it yourself.",
                    &[&format!("{:.0}", f64::from(recommended) / 1000.0), &w, &h, &fps]),
                |ui, s| {
                    let r = toggle(ui, &mut automatic);
                    if r.changed() {
                        s.bitrate_kbps = if automatic { 0 } else { recommended };
                    }
                    r
                },
            );
            if !automatic {
                let mut mbps = bitrate as f32 / 1000.0;
                // A chosen bitrate below PyroWave's minimum stays; the row says what it costs.
                let warning = pyrowave_bitrate_warning(s, w, h, fps);
                let note = match &warning {
                    Some(warning) => warning.as_str(),
                    None if s.codec.is_pyrowave() => en("PyroWave needs a high bitrate: below its minimum the picture breaks up."),
                    None => en("Higher is sharper and needs a faster network."),
                };
                setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Bitrate",
                    note,
                    |ui, s| {
                        let r = log_slider(ui, &mut mbps, 0.5..=2000.0, 0.5, mbps_text,
                            |rail| rail.logarithmic(true).step_by(0.5),
                            |field| field.suffix(" Mbps").max_decimals(1).speed(0.5));
                        if r.changed() {
                            s.bitrate_kbps = (mbps * 1000.0) as u32;
                        }
                        r
                    },
                );
            }
            setting_row(ui, s, &mut rows, &["adaptive_bitrate"], "Lower bitrate when the network is busy", "Turns the bitrate down while the network keeps falling behind, and slowly back up once it recovers. Works with PCs that support it.", |ui, s| {
                toggle(ui, &mut s.adaptive_bitrate)
            });
            // The GPU decoder's probe runs in the background, on the renderer's adapter and the HDR
            // plane format, and only when it matters: PyroWave is selected or the codec menu is
            // open. PyroWave is not offered while it fails, and the reason is shown with the
            // decoder setting.
            let probe_device = self.gpu_device.map_or(std::ptr::null_mut(), |d| d.0);
            let (gpu_mode, ten_bit) = (s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu, s.hdr);
            let gpu_probe = move |ui: &Ui| {
                if !gpu_mode { return None; }
                let probe = crate::stream::pyrowave_mode::settings_probe(probe_device, ten_bit);
                if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                probe.and_then(Result::err)
            };
            let gpu_decoder_error = if s.codec.is_pyrowave() { gpu_probe(ui) } else { None };
            setting_row(ui, s, &mut rows, &["codec"], "Codec", "How the video is compressed. Automatic suits most setups: it picks HEVC, then H.264. PyroWave needs a very high bitrate and GPU rendering.", |ui, s| {
                Combo::from_id_salt("codec")
                    .selected_text(codec_name(s.codec))
                    .show_ui(ui, |ui| {
                        for codec in [CodecPreference::Auto, CodecPreference::H264, CodecPreference::Hevc, CodecPreference::Av1] {
                            ui.selectable_value(&mut s.codec, codec, codec_name(codec));
                        }
                        let menu_error = gpu_probe(ui);
                        if menu_error.is_none() {
                            for codec in [CodecPreference::PyroWave, CodecPreference::PyroWave444] {
                                ui.selectable_value(&mut s.codec, codec, codec_name(codec));
                            }
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["hdr"], "HDR",
                "Streams in HDR to an HDR screen, for brighter highlights and richer color. Turn on HDR in the system's display settings first.",
                |ui, s| toggle(ui, &mut s.hdr),
            );
            rows.group(en("Fine-tuning"));
            setting_row(ui, s, &mut rows, &["adaptive_quality"], "Automatic quality", "Lowers the resolution or frame rate while the PC or network can't keep up, and raises them again later. Works with PCs that can change a stream while it runs.", |ui, s| {
                Combo::from_id_salt("quality").selected_text(tr(s.adaptive_quality.label())).show_ui(ui, |ui| {
                    for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                        ui.selectable_value(&mut s.adaptive_quality, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.adaptive_quality != crate::config::QualityMode::Off {
                setting_row(ui, s, &mut rows, &["quality_min_short_edge"], "Minimum resolution", "Automatic quality never goes below this, measured on the picture's shorter side. It never makes the picture larger than you chose.", |ui, s| {
                    step_value(ui, &mut s.quality_min_short_edge, 256..=2160, 8.0, |d| d.speed(2).suffix(" px"))
                });
                setting_row(ui, s, &mut rows, &["quality_min_fps"], "Minimum frame rate", "Automatic quality never goes below this rate.", |ui, s| {
                    step_value(ui, &mut s.quality_min_fps, 15..=240, 5.0, |d| d.suffix(" fps"))
                });
            }
            let upscaling_help = if s.upscaling == crate::config::Upscaling::Ai {
                en("Uses the graphics card's AI video upscaling, for SDR and HDR. On NVIDIA with an HDR screen it also turns SDR streams into HDR. Uses more GPU power.")
            } else {
                en("How a stream smaller than the window is enlarged. FSR 1 and SGSR 1 look sharper than Bilinear; HDR always uses Bilinear.")
            };
            setting_row(ui, s, &mut rows, &["upscaling"], "Upscaling", upscaling_help, |ui, s| {
                Combo::from_id_salt("upscaling").selected_text(tr(s.upscaling.label())).show_ui(ui, |ui| {
                    // AI enhancement is offered only where the GPU's probe finds the vendor's super
                    // resolution or RTX Video HDR. It loads the AMF runtime, so it runs once per
                    // renderer device, in the background, when the menu is first opened.
                    let probe = crate::stream::enhance::settings_probe(probe_device);
                    if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                    let ai_offered = probe.flatten().is_some();
                    for mode in [crate::config::Upscaling::Bilinear, crate::config::Upscaling::Fsr1, crate::config::Upscaling::Sgsr1, crate::config::Upscaling::Ai] {
                        if mode == crate::config::Upscaling::Ai && !ai_offered && s.upscaling != mode { continue; }
                        ui.selectable_value(&mut s.upscaling, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.upscaling.is_shader() {
                setting_row(ui, s, &mut rows, &["upscale_sharpness"], "Sharpness", "How much extra sharpening the upscaler adds.", |ui, s| {
                    slider(ui, &mut s.upscale_sharpness, 0..=100, 5.0, percent, |rail| rail, |field| field.suffix("%"))
                });
            }
            rows.when(!s.codec.is_pyrowave() && !s.experimental_packed_444, en("Not used with PyroWave or Full chroma."));
            setting_row(ui, s, &mut rows, &["prefer_yuv444"], "Sharper colored text (YUV 4:4:4)",
                "Keeps full color detail, so colored text and thin lines stay crisp, when the PC and this device support it. HDR takes priority.",
                |ui, s| toggle(ui, &mut s.prefer_yuv444));
            setting_row(ui, s, &mut rows, &["sdr_10bit"], "Less color banding (10-bit SDR)",
                "Streams non-HDR video in 10-bit HEVC or AV1, so gradients show fewer bands. Needs a 10-bit (Main10) decoder on this device, and the PC's display in SDR.",
                |ui, s| toggle(ui, &mut s.sdr_10bit));
            rows.always();
            let decoder_note = match &gpu_decoder_error {
                Some(reason) => trf("Only for PyroWave. GPU decodes on the graphics card and never falls back to the processor. CPU decodes on the processor and finishes on the graphics card. GPU decoding is unavailable here: {}", &[reason]),
                None => tr("Only for PyroWave. GPU decodes on the graphics card and never falls back to the processor. CPU decodes on the processor and finishes on the graphics card.").to_owned(),
            };
            setting_row(ui, s, &mut rows, &["pyrowave_decoder"], "PyroWave decoder", &decoder_note, |ui, s| {
                Combo::from_id_salt("pyrowave-decoder")
                    .selected_text(tr(s.pyrowave_decoder.label()))
                    .show_ui(ui, |ui| {
                        for mode in [crate::config::PyroWaveDecoder::Cpu, crate::config::PyroWaveDecoder::Gpu] {
                            ui.selectable_value(&mut s.pyrowave_decoder, mode, tr(mode.label()));
                        }
                    })
                    .response
            });

            rows.section(en("Display & image"));
            setting_row(ui, s, &mut rows, &["window_mode"], "Window", "Borderless covers the screen without changing the display's mode, so switching to other apps stays quick.", |ui, s| {
                Combo::from_id_salt("window")
                    .selected_text(window_name(s.window_mode))
                    .show_ui(ui, |ui| {
                        for mode in [WindowMode::Fullscreen, WindowMode::Borderless, WindowMode::Windowed] {
                            ui.selectable_value(&mut s.window_mode, mode, window_name(mode));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["video_scaling"], "Video size", "How the picture fits the window. Fit shows all of it, Fill crops the edges, Stretch fills the window out of shape, Native pixels shows one screen pixel per stream pixel.", |ui, s| {
                use crate::config::VideoScaling;
                Combo::from_id_salt("video-scaling").selected_text(tr(s.video_scaling.label())).show_ui(ui, |ui| {
                    for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                        ui.selectable_value(&mut s.video_scaling, mode, tr(mode.label()));
                    }
                }).response
            });
            let vrr_note = if tearing || cfg!(not(windows)) {
                en("Keep on when G-Sync, FreeSync or HDMI VRR is on for this screen: frames show as soon as they arrive, without tearing.")
            } else {
                en("This system can't show frames with tearing, which VRR needs here.")
            };
            setting_row(ui, s, &mut rows, &["vrr"], "Variable refresh rate", vrr_note, |ui, s| toggle(ui, &mut s.vrr));
            rows.group(en("On the PC"));
            setting_row(ui, s, &mut rows, &["virtual_display"], "Virtual display", "Asks the PC for a separate virtual display that matches the stream. Works with PCs that support it.", |ui, s| toggle(ui, &mut s.virtual_display));
            setting_row(ui, s, &mut rows, &["host_display"], "Display arrangement", "How the PC arranges the streamed display with its own screens. Only for normal streams on PCs that support display modes.", |ui, s| {
                Combo::from_id_salt("host-display")
                    .selected_text(tr(host_display_name(s.host_display)))
                    .show_ui(ui, |ui| {
                        for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary,
                            HostDisplay::Extended, HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
                            ui.selectable_value(&mut s.host_display, mode, tr(host_display_name(mode)));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["virtual_display_scale"], "PC render resolution", "The size the PC renders at, as a percent of the stream resolution. The video it sends keeps its size. To send fewer pixels and upscale here, lower Resolution instead.", |ui, s| {
                slider(ui, &mut s.virtual_display_scale, crate::config::RENDER_SCALE, 5.0, percent, |rail| rail.step_by(5.0), |field| field.suffix("%"))
            });
            // A PC's own settings offer it only if its host reads rlDisplayScale (DisplayScale).
            let dpi_gate = display_scale_gate(test_host.and_then(|h| h.info.as_ref())
                .map_or(nightjar_core::http::Availability::Unknown, |i| i.capabilities.availability(nightjar_core::http::Feature::DisplayScale)));
            if let Some(note) = dpi_gate {
                let description = match note {
                    Some(note) => format!("{} {}", tr("Text and interface size on the PC’s virtual display."), tr(note)),
                    None => tr("Text and interface size on the PC’s virtual display.").to_owned(),
                };
                rows.when(note.is_none(), "");
                setting_row(ui, s, &mut rows, &["virtual_display_dpi"], "PC display scaling", &description, |ui, s| {
                    Combo::from_id_salt("virtual-display-dpi")
                        .selected_text(display_scale_name(s.virtual_display_dpi))
                        .show_ui(ui, |ui| {
                            for percent in crate::config::DISPLAY_SCALES {
                                ui.selectable_value(&mut s.virtual_display_dpi, percent, display_scale_name(percent));
                            }
                        }).response
                });
                rows.always();
            }
            rows.group(en("Fine-tuning"));
            setting_row(ui, s, &mut rows, &["align_video_top"], "Align video to top", "Puts Fit and Native pixels video at the top of the window instead of the middle.", |ui, s| toggle(ui, &mut s.align_video_top));
            let immediate_packed = cfg!(windows) && s.experimental_packed_444 && tearing;
            rows.when(s.vrr || immediate_packed, en("Turn on Variable refresh rate to use this."));
            rows.when(s.vrr && !immediate_packed, "");
            setting_row(ui, s, &mut rows, &["present_at_host_times"], "Show frames at the PC's frame times",
                if immediate_packed {
                    en("With Full chroma, VRR shows each frame at once, without this delay.")
                } else {
                    en("With VRR: each frame appears in step with the game, after a short steady delay, so motion looks even.")
                },
                |ui, s| toggle(ui, &mut s.present_at_host_times),
            );
            rows.always();
            rows.when(!s.vrr, en("Used only with Variable refresh rate off."));
            setting_row(ui, s, &mut rows, &["frame_pacing"], "Frame pacing",
                "Without VRR: Lowest latency shows the newest frame at once. Balanced waits for the screen's refresh. With an FPS limit, early frames wait their turn. Smoothest video shows every frame in turn and adds up to a frame of delay.",
                |ui, s| {
                    Combo::from_id_salt("frame-pacing").selected_text(tr(s.frame_pacing.label())).show_ui(ui, |ui| {
                        for mode in crate::config::FramePacing::ALL {
                            ui.selectable_value(&mut s.frame_pacing, mode, tr(mode.label()));
                        }
                    }).response
                });
            // Still narrowed to VRR off, as Frame pacing.
            rows.when(!s.frame_pacing.forces_vsync(), en("Always on with the chosen frame pacing."));
            setting_row(ui, s, &mut rows, &["vsync"], "V-Sync", "Without VRR: waits for the screen's refresh, so the picture never tears.", |ui, s| {
                toggle(ui, &mut s.vsync)
            });
            rows.always();

            rows.section(en("Controls"));
            let mouse = with_hotkey(tr("Captured suits games: the pointer stays in the stream. Desktop pointer suits remote work."), crate::shortcuts::Action::MouseMode);
            setting_row(ui, s, &mut rows, &["mouse_mode"], "Mouse", &mouse, |ui, s| {
                Combo::from_id_salt("mouse")
                    .selected_text(mouse_name(s.mouse_mode))
                    .show_ui(ui, |ui| {
                        for mode in [MouseMode::Capture, MouseMode::Absolute] {
                            ui.selectable_value(&mut s.mouse_mode, mode, mouse_name(mode));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["controllers"], "Controllers", "Every connected controller becomes a controller on the PC.", |ui, s| {
                toggle(ui, &mut s.controllers)
            });
            rows.when(s.controllers, en("Turn on Controllers to use this."));
            setting_row(ui, s, &mut rows, &["swap_face_buttons"], "Nintendo button layout", "Swaps A with B and X with Y, as on Nintendo controllers.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
            rows.always();
            setting_row(ui, s, &mut rows, &["clipboard_sync"], "Clipboard sync", "Text you copy here can be pasted on the PC, and the other way round. The PC must allow this device to use its clipboard.", |ui, s| {
                toggle(ui, &mut s.clipboard_sync)
            });
            if !layered {
                setting_row(ui, s, &mut rows, &[], "Controller buttons", "Remap a controller's buttons and set its stick and trigger dead zones.", |ui, _| {
                    let controllers = quiet_button(ui, "Controllers");
                    if controllers.clicked() { commands.push(Command::ControllerSettings); }
                    links.push(("controllers-link", controllers.clone()));
                    controllers
                });
            }
            // How the stream menu opens and what it holds, as Android's Stream menu group. Android's floating menu
            // button has no desktop row: a window has a pointer and keys to open the menu.
            rows.group(en("Stream menu"));
            rows.when(s.controllers, en("Turn on Controllers to use this."));
            setting_row(ui, s, &mut rows, &["guide_hold_ms"], "Hold Guide to open the stream menu", "A short press still goes to the PC.", |ui, s| {
                Combo::from_id_salt("guide-hold").selected_text(guide_hold_name(s.guide_hold_ms)).show_ui(ui, |ui| {
                    for ms in crate::config::GUIDE_HOLD_MS {
                        ui.selectable_value(&mut s.guide_hold_ms, ms, guide_hold_name(ms));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["back_opens_menu"], "Back opens stream controls", "The controller's Back or Select button opens the stream menu.", |ui, s| toggle(ui, &mut s.back_opens_menu));
            rows.always();
            if !layered {
                setting_row(ui, s, &mut rows, &[], "Shortcuts", "Key combinations as tiles in the stream menu.", |ui, _| {
                    let shortcuts = quiet_button(ui, "Shortcuts");
                    if shortcuts.clicked() { commands.push(Command::Shortcuts); }
                    links.push(("shortcuts-link", shortcuts.clone()));
                    shortcuts
                });
            }
            rows.group(en("Mouse and keyboard"));
            if !layered {
                setting_row(ui, s, &mut rows, &[], "Stream hotkeys", "Nightjar's own key combinations in a stream.", |ui, _| {
                    let hotkeys = quiet_button(ui, "Change");
                    if hotkeys.clicked() { commands.push(Command::StreamHotkeys); }
                    links.push(("stream-hotkeys-link", hotkeys.clone()));
                    hotkeys
                });
            }
            setting_row(ui, s, &mut rows, &["capture_system_keys"], "Send system keys",
                "Alt+Tab, the Windows key and similar go to the PC while input is captured.",
                |ui, s| toggle(ui, &mut s.capture_system_keys),
            );
            setting_row(ui, s, &mut rows, &["mouse_speed"], "Mouse speed", "How fast the captured pointer moves.", |ui, s| slider(ui, &mut s.mouse_speed, 25..=400, 5.0, percent, |rail| rail, |field| field.suffix("%")));
            setting_row(ui, s, &mut rows, &["mouse_nav_buttons"], "Mouse navigation buttons", "Sends the mouse's Back and Forward buttons to the PC.", |ui, s| toggle(ui, &mut s.mouse_nav_buttons));
            if cfg!(windows) {
                setting_row(ui, s, &mut rows, &["native_touchpad"], "Touchpad gestures on the PC", "Sends your touchpad's fingers to a PC that accepts them, so its gestures work there. Other PCs get a normal pointer.", |ui, s| toggle(ui, &mut s.native_touchpad));
            }
            setting_row(ui, s, &mut rows, &["right_alt_as_meta"], "Right Alt as Windows key", "Use Right Alt for Windows-key shortcuts on the PC.", |ui, s| toggle(ui, &mut s.right_alt_as_meta));
            rows.group(en("Controllers"));
            rows.when(s.controllers, en("Turn on Controllers to use this."));
            setting_row(ui, s, &mut rows, &["gamepad_quit_combo"], "Buttons to leave the stream", "Hold these buttons together to leave the stream. Pick others if a game or your controller needs these.", |ui, s| {
                Combo::from_id_salt("gamepad-quit-combo").selected_text(style.quit_combo(s.gamepad_quit_combo)).show_ui(ui, |ui| {
                    for combo in crate::config::GamepadQuitCombo::ALL {
                        ui.selectable_value(&mut s.gamepad_quit_combo, combo, style.quit_combo(combo));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "Y in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::Y));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "X in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::X));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "LB in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::Lb));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "RB in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::Rb));
            setting_row(ui, s, &mut rows, &["controller_mouse"], "Controller mouse mode","Hold Start for 750 ms, then let go, to move the PC's pointer with the controller. A and B click, the D-pad scrolls.", |ui, s| toggle(ui, &mut s.controller_mouse));
            if s.controller_mouse {
                setting_row(ui, s, &mut rows, &["analog_scrolling"], "Stick for scrolling", "In controller mouse mode this stick scrolls and the other moves the pointer. Off lets both sticks move it.", |ui, s| {
                    use crate::config::AnalogScrolling;
                    Combo::from_id_salt("analog-scrolling").selected_text(scrolling_name(s.analog_scrolling)).show_ui(ui, |ui| {
                        for stick in [AnalogScrolling::Off, AnalogScrolling::Left, AnalogScrolling::Right] {
                            ui.selectable_value(&mut s.analog_scrolling, stick, scrolling_name(stick));
                        }
                    }).response
                });
                setting_row(ui, s, &mut rows, &["mouse_fixed_step"], "Precise controller mouse", "In controller mouse mode, hold X to move in fixed pixel steps. Press Y to double the step, from 1 to 128 pixels.", |ui, s| toggle(ui, &mut s.mouse_fixed_step));
            }
            setting_row(ui, s, &mut rows, &["gamepad_touchpad_as_mouse"], "Controller touchpad as mouse", "Move with one finger, scroll with two. Press the pad for a left click, or with two fingers for a right click.", |ui, s| toggle(ui, &mut s.gamepad_touchpad_as_mouse));
            setting_row(ui, s, &mut rows, &["disable_rumble"], "Turn off vibration", "Stops controller rumble, trigger rumble and haptics.", |ui, s| toggle(ui, &mut s.disable_rumble));
            setting_row(ui, s, &mut rows, &["controller_motion"], "Motion controls", "Sends the controller's motion sensors to games that ask for them.", |ui, s| {
                toggle(ui, &mut s.controller_motion)
            });
            setting_row(ui, s, &mut rows, &["controller_audio"], "DualSense USB audio", "With a DualSense on USB, its haptics, speaker, headset and microphone work with the PC. Choose them in the stream menu; the microphone also needs Microphone turned on.", |ui, s| {
                toggle(ui, &mut s.controller_audio)
            });
            setting_row(ui, s, &mut rows, &["background_controllers"], "Input in the background", "Controllers keep working while Nightjar isn't the active window.", |ui, s| {
                toggle(ui, &mut s.background_controllers)
            });
            rows.always();
            // The keyboard's chords are on the Stream hotkeys page, as the user bound them.
            rows.caption(ui, &trf("On a controller, {} leaves the stream.", &[&style.quit_combo(s.gamepad_quit_combo)]));

            rows.section(en("Audio"));
            setting_row(ui, s, &mut rows, &["audio"], "Channels", "Stereo suits most speakers and headphones. Surround needs a PC and speakers that support it.", |ui, s| {
                Combo::from_id_salt("audio")
                    .selected_text(audio_name(s.audio))
                    .show_ui(ui, |ui| {
                        for channels in [AudioChannels::Stereo, AudioChannels::Surround51, AudioChannels::Surround71] {
                            ui.selectable_value(&mut s.audio, channels, audio_name(channels));
                        }
                        if crate::stream::offers_714() {
                            ui.selectable_value(&mut s.audio, AudioChannels::Surround714, audio_name(AudioChannels::Surround714));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["play_audio_on_host"], "Play audio on the PC too", "Keeps sound on the PC's speakers while you stream.", |ui, s| {
                toggle(ui, &mut s.play_audio_on_host)
            });
            setting_row(ui, s, &mut rows, &["microphone"], "Microphone", "Sends this device's default microphone to the PC while streaming. Works with PCs that support it.", |ui, s| {
                toggle(ui, &mut s.microphone)
            });
            rows.group(en("Fine-tuning"));
            rows.when(s.microphone, en("Turn on Microphone to use this."));
            setting_row(ui, s, &mut rows, &["mic_echo_cancel"], "Echo cancellation",
                "Keeps the stream's sound from your speakers out of the microphone. On Windows, Automatic turns it off for headphones and headsets. On Linux it needs PipeWire's or PulseAudio's echo-cancel module.",
                |ui, s| {
                    Combo::from_id_salt("mic-echo-cancel").selected_text(tr(s.mic_echo_cancel.label())).show_ui(ui, |ui| {
                        for mode in crate::config::MicEcho::ALL {
                            ui.selectable_value(&mut s.mic_echo_cancel, mode, tr(mode.label()));
                        }
                    }).response
                });
            rows.always();
            setting_row(ui, s, &mut rows, &["mute_audio"], "Mute audio", "Plays no sound from the stream on this device. The microphone and the PC's sound are unaffected.", |ui, s| {
                toggle(ui, &mut s.mute_audio)
            });
            setting_row(ui, s, &mut rows, &["audio_passthrough"], "Uncompressed audio", "Asks the PC for lossless sound instead of compressed audio. Uses 1.5 Mbps for stereo, up to 6 Mbps for 7.1. PCs that don't support it keep sending compressed audio.", |ui, s| {
                toggle(ui, &mut s.audio_passthrough)
            });

            rows.section(en("Streaming"));
            let overlay = with_hotkey(tr("Shows how the stream is doing. Compact is one line; Statistics adds frame rates, network and decode times; Debug shows every measurement."), crate::shortcuts::Action::Overlay);
            setting_row(ui, s, &mut rows, &["overlay", "overlay_mode"], "Performance overlay", &overlay, |ui, s| {
                let shown = if s.overlay { tr(s.overlay_mode.label()) } else { tr("Off") };
                Combo::from_id_salt("overlay-mode").selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(!s.overlay, tr("Off")).clicked() { s.overlay = false; }
                    for mode in crate::config::OverlayMode::ALL {
                        if ui.selectable_label(s.overlay && s.overlay_mode == mode, tr(mode.label())).clicked() {
                            (s.overlay, s.overlay_mode) = (true, mode);
                        }
                    }
                }).response
            });
            rows.when(s.overlay, en("Turn on the performance overlay to use this."));
            setting_row(ui, s, &mut rows, &["overlay_position"], "Overlay position", "The corner the performance overlay sits in.", |ui, s| {
                Combo::from_id_salt("overlay-position").selected_text(tr(s.overlay_position.label())).show_ui(ui, |ui| {
                    for corner in crate::config::OverlayCorner::ALL {
                        ui.selectable_value(&mut s.overlay_position, corner, tr(corner.label()));
                    }
                }).response
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["auto_reconnect"], "Recover interrupted streams", "If the stream drops, Nightjar reconnects to the same game, up to five times within a minute.", |ui, s| {
                toggle(ui, &mut s.auto_reconnect)
            });
            if !layered {
                let active = self.profiles.entries.iter().find(|p| Some(p.id) == self.profiles.active)
                    .map_or(tr("Main settings"), |p| p.name.as_str());
                let about = trf("Keep different settings for different networks, such as a lower bitrate on mobile data. In use now: {}.", &[&active]);
                setting_row(ui, s, &mut rows, &[], "Profiles", &about, |ui, _| {
                    let manage = quiet_button(ui, "Manage profiles");
                    links.push(("profiles-link", manage.clone()));
                    manage
                });
            }
            rows.group(en("Connection"));
            setting_row(ui, s, &mut rows, &["client_roaming"], "Keep streaming when the network changes", "The stream carries on when this device switches networks, such as from Wi-Fi to Ethernet. Works with PCs that support it.", |ui, s| {
                toggle(ui, &mut s.client_roaming)
            });
            if s.client_roaming {
                setting_row(ui, s, &mut rows, &["roaming_metered"], "Continue on mobile data", "Let the stream move onto mobile data or another metered network. Streaming uses a lot of data.", |ui, s| {
                    toggle(ui, &mut s.roaming_metered)
                });
            }
            setting_row(ui, s, &mut rows, &["network_grace_ms"], "Wait through network dropouts", "How long the stream waits through a brief network dropout before it gives up.", |ui, s| {
                Combo::from_id_salt("network-grace")
                    .selected_text(trf("{} seconds", &[&(s.network_grace_ms / 1000)]))
                    .show_ui(ui, |ui| {
                        for ms in [0, 3000, 5000, 10000, 15000] {
                            ui.selectable_value(&mut s.network_grace_ms, ms, trf("{} seconds", &[&(ms / 1000)]));
                        }
                    }).response
            });
            if let Some(host) = test_host {
                rows.group(en("Network tests"));
                let online = host.paired() && host.reach == Reach::Online;
                setting_row(ui, s, &mut rows, &[], "Network speed test", "Downloads 32 MiB from this PC to measure the connection. Uses network data, and needs a paired PC that supports it.", |ui, _| {
                    let enabled = online && host.bandwidth_test();
                    let button = ui.add_enabled(enabled, egui::Button::new(tr("Test speed")));
                    if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: false }); }
                    button
                });
                let note = host.record.pace.as_ref().map_or_else(|| tr("Finds a frame pace for PyroWave on this network. Can take several minutes and uses a lot of network data; stop other streams first.").to_owned(),
                    |p| trf("Saved {} Mbps · {}. Calibrate again after changing the network or the bitrate.", &[&p.pace_mbps, &if p.qualified { tr("confirmed") } else { tr("best available") }]));
                setting_row(ui, s, &mut rows, &[], "PyroWave frame pace", &note, |ui, _| {
                    ui.horizontal(|ui| {
                        let enabled = online && host.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe);
                        let button = ui.add_enabled(enabled, egui::Button::new(tr("Calibrate")));
                        if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: true }); }
                        if host.record.pace.is_some() && text_button(ui, "Clear").clicked() { commands.push(Command::ClearPace(host.id)); }
                        button
                    }).inner
                });
            }
            rows.group(en("Fine-tuning"));
            setting_row(ui, s, &mut rows, &["usb_forwarding"], "USB forwarding", "Lets you attach USB devices plugged into this computer to the PC from the stream menu. The PC must allow USB devices for this device. On Windows, a device needs the WinUSB driver.", |ui, s| {
                toggle(ui, &mut s.usb_forwarding)
            });

            if !layered {
                rows.section(en("App"));
                setting_row(ui, s, &mut rows, &["console_home"], "Open on Home", "Starts on your games and library. Off starts on the list of PCs.", |ui, s| {
                    toggle(ui, &mut s.console_home)
                });
                setting_row(ui, s, &mut rows, &["quick_resume"], "Quick resume", "Selecting a PC that's running a game goes straight back into the game instead of opening its library.", |ui, s| {
                    toggle(ui, &mut s.quick_resume)
                });
                setting_row(ui, s, &mut rows, &[], "PIN lock", "Ask for a PIN to open Nightjar or to start a stream.", |ui, _| {
                    let set = self.pin.as_ref().is_some_and(|p| p.store.has_pin());
                    let button = quiet_button(ui, if set { en("Change PIN lock") } else { en("Set up") });
                    if button.clicked() {
                        if let Some(pin) = self.pin.as_mut() { pin.open_settings(); }
                    }
                    button
                });
                setting_row(ui, s, &mut rows, &["language"], "Language", "The language of Nightjar's menus. System default follows the operating system.", |ui, s| {
                    Combo::from_id_salt("language")
                        .selected_text(drawable_language_name(ui, s.language))
                        .show_ui(ui, |ui| {
                            for language in Language::ALL {
                                ui.selectable_value(&mut s.language, language, drawable_language_name(ui, language));
                            }
                        })
                        .response
                });
                rows.group(en("Starting up"));
                let open_game = self.open_game.as_ref().map(|g| g.title.clone());
                let open_name = |choice: crate::config::OpenOnStart| match (choice, &open_game) {
                    (crate::config::OpenOnStart::Chosen, Some(title)) => title.clone(),
                    _ => tr(choice.label()).to_owned(),
                };
                setting_row(ui, s, &mut rows, &["open_on_start"], "When Nightjar opens", "Your PCs, or a 3-second countdown into a game.", |ui, s| {
                    Combo::from_id_salt("open-on-start")
                        .selected_text(open_name(s.open_on_start))
                        .show_ui(ui, |ui| {
                            for choice in crate::config::OpenOnStart::ALL {
                                ui.selectable_value(&mut s.open_on_start, choice, open_name(choice));
                            }
                        })
                        .response
                });
                if let Some(on) = self.sign_in {
                    setting_row(ui, s, &mut rows, &[], "Start Nightjar when I sign in", "Nightjar opens by itself after you sign in to this computer.", |ui, _| {
                        let mut wanted = on;
                        let response = toggle(ui, &mut wanted);
                        if wanted != on {
                            commands.push(Command::SetSignIn(wanted));
                        }
                        response
                    });
                }
                rows.group(en("Accessibility"));
                setting_row(ui, s, &mut rows, &["ui_zoom"], "Text and button size", "Makes everything in Nightjar bigger or smaller. Ctrl+= and Ctrl+- change it, Ctrl+0 resets it.", |ui, s| {
                    Combo::from_id_salt("ui-zoom")
                        .selected_text(format!("{}%", crate::config::nearest_zoom(s.ui_zoom)))
                        .show_ui(ui, |ui| {
                            for step in UI_ZOOM_STEPS {
                                ui.selectable_value(&mut s.ui_zoom, step, format!("{step}%"));
                            }
                        })
                        .response
                });
                setting_row(ui, s, &mut rows, &["high_contrast"], "High contrast", "Black surfaces, white text, and thicker outlines and focus rings.", |ui, s| {
                    toggle(ui, &mut s.high_contrast)
                });
                setting_row(ui, s, &mut rows, &["reduce_motion"], "Reduce motion", "Stops fades, slides and the spinning wheel. Follow system uses the Windows animation setting; other systems don't report one, so motion stays on.", |ui, s| {
                    Combo::from_id_salt("reduce-motion")
                        .selected_text(reduce_motion_name(s.reduce_motion))
                        .show_ui(ui, |ui| {
                            for mode in [ReduceMotion::System, ReduceMotion::Reduce, ReduceMotion::Full] {
                                ui.selectable_value(&mut s.reduce_motion, mode, reduce_motion_name(mode));
                            }
                        })
                        .response
                });
                rows.group(en("Fine-tuning"));
                setting_row(ui, s, &mut rows, &["input_only_any_host"], "Input-only on any PC", "Adds Input-only to a game’s menu on PCs without an Input-only mode of their own. The PC still sends video and audio at the smallest size, under 1 Mbit/s, which Nightjar doesn’t show or play.", |ui, s| {
                    toggle(ui, &mut s.input_only_any_host)
                });
                if crate::self_update::current().checks() {
                    setting_row(ui, s, &mut rows, &["check_updates"], "Check for updates", "Once per launch, asks GitHub whether a newer Nightjar release is out and says so here. Nothing is downloaded until you choose Update now.", |ui, s| {
                        toggle(ui, &mut s.check_updates)
                    });
                }
                setting_row(ui, s, &mut rows, &[], "Logs", "Nightjar keeps a log of the last two runs. Attach nightjar.log, and nightjar.prev.log if the problem was before a restart, to a bug report.", |ui, _| {
                    let button = quiet_button(ui, "Open logs folder");
                    if button.clicked() { commands.push(Command::OpenLogs); }
                    button
                });
                let (reset_label, reset_about) = if *confirm_reset {
                    (en("Reset all settings?"), en("Every setting on this page goes back to its default. Your language, PCs, games, profiles, PIN lock, controller buttons, shortcuts and anything not on this page stay as they are. This can't be undone."))
                } else {
                    (en("Reset all settings"), en("Puts every setting on this page back to its default. Your PCs, games, profiles and language keep theirs."))
                };
                setting_row(ui, s, &mut rows, &[], reset_label, reset_about, |ui, _| {
                    if *confirm_reset {
                        if primary_button(ui, "Reset all").clicked() {
                            reset_all = true;
                            *confirm_reset = false;
                        }
                        let cancel = quiet_button(ui, "Cancel");
                        if cancel.clicked() { *confirm_reset = false; }
                        cancel
                    } else {
                        let button = quiet_button(ui, "Reset all…");
                        if button.clicked() { *confirm_reset = true; }
                        button
                    }
                });
            }

            rows.section(en("Advanced"));
            rows.when(!s.codec.is_pyrowave(), en("Not used with PyroWave."));
            setting_row(ui, s, &mut rows, &["experimental_packed_444"], "Full chroma", "Sharp colored text through HEVC or AV1, when the PC has its Full chroma option on.",
                |ui, s| toggle(ui, &mut s.experimental_packed_444));
            rows.always();
            setting_row(ui, s, &mut rows, &["link_probe"], "Test the link before starting", "A new stream at the automatic bitrate first measures the connection to the PC and starts no higher than it holds. Works with PCs that support it.", |ui, s| {
                toggle(ui, &mut s.link_probe)
            });

            rows.section(en("Debug"));
            // Android's main rows first, in its order.
            setting_row(ui, s, &mut rows, &["error_correction"], "Error correction", "Recovery data the PC adds so lost packets don't break up the picture. Automatic follows this device's losses. PCs that don't take this choice use their own setting.", |ui, s| {
                Combo::from_id_salt("error-correction")
                    .selected_text(error_correction_name(s.error_correction))
                    .show_ui(ui, |ui| {
                        let fixed = ErrorCorrection::PERCENTAGES.map(ErrorCorrection::Percent);
                        for mode in [ErrorCorrection::Host, ErrorCorrection::Auto].into_iter().chain(fixed) {
                            ui.selectable_value(&mut s.error_correction, mode, error_correction_name(mode));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["client_hint_bitrate"], "Lower bitrate when frames are lost", "Turns the bitrate down as soon as frames go missing, and back up after a few clean seconds. Works with PCs that can change the bitrate during a stream, and replaces “Lower bitrate when the network is busy” there.", |ui, s| {
                toggle(ui, &mut s.client_hint_bitrate)
            });
            setting_row(ui, s, &mut rows, &["tell_host_wireless"], "Tell the PC when on Wi-Fi", "The PC then paces the stream for Wi-Fi from the start. PCs that don't support it aren't told.", |ui, s| {
                toggle(ui, &mut s.tell_host_wireless)
            });
            setting_row(ui, s, &mut rows, &["tell_host_link_speed"], "Tell the PC the link speed", "Lets the PC limit its sending rate to this device's wired port speed.", |ui, s| {
                toggle(ui, &mut s.tell_host_link_speed)
            });
            // The engine sets socket and thread priorities on Linux only; on Windows the switch would do nothing.
            if cfg!(target_os = "linux") {
                setting_row(ui, s, &mut rows, &["network_priority"], "Prioritize stream traffic", "Gives the stream's network traffic and work priority on this device.", |ui, s| {
                    toggle(ui, &mut s.network_priority)
                });
            }
            setting_row(ui, s, &mut rows, &["unbatched_input"], "Instant mouse and pen input", "Sends mouse and pen movement right away instead of in small batches.", |ui, s| {
                toggle(ui, &mut s.unbatched_input)
            });
            setting_row(ui, s, &mut rows, &["full_range"], "Full color range", "Asks the PC for the full range of brightness levels. Turn on only if dark scenes look washed out or crushed.", |ui, s| toggle(ui, &mut s.full_range));
            rows.group(en("Fine-tuning"));
            rows.when(!s.codec.is_pyrowave(), en("Not used with PyroWave."));
            setting_row(ui, s, &mut rows, &["decoder"], "Video decoding", "Automatic decodes on the graphics card when it can. Software uses the processor; try it only if the picture shows errors.", |ui, s| {
                Combo::from_id_salt("decoder")
                    .selected_text(decoder_name(s.decoder))
                    .show_ui(ui, |ui| {
                        for decoder in [DecoderPreference::Auto, DecoderPreference::Software] {
                            ui.selectable_value(&mut s.decoder, decoder, decoder_name(decoder));
                        }
                    })
                    .response
            });
            rows.always();
            // Linux only (Windows draws with Direct3D 11), and for the whole app, not a PC or a profile.
            if cfg!(unix) && !layered {
                setting_row(ui, s, &mut rows, &["renderer"], "Renderer", "What draws this window. Automatic starts on OpenGL and moves to the GPU renderer when a stream needs it.", |ui, s| {
                    Combo::from_id_salt("renderer")
                        .selected_text(tr(s.renderer.label()))
                        .show_ui(ui, |ui| {
                            for choice in crate::config::Renderer::ALL {
                                ui.selectable_value(&mut s.renderer, choice, tr(choice.label()));
                            }
                        })
                        .response
                });
            }
            setting_row(ui, s, &mut rows, &["cursor_shapes"], "Local cursor", "In desktop pointer mode, draws the PC's cursor here so it moves without video delay. Works with PCs that send their cursor.", |ui, s| {
                toggle(ui, &mut s.cursor_shapes)
            });

            if searching && rows.found.iter().sum::<usize>() == 0 {
                clear_search = no_results(ui, self.settings_view.query.trim());
            }
            ui.add_space(space::XL);
        }
        if clear_search {
            self.settings_view.query.clear();
            self.settings_view.to_top = true;
            ui.memory_mut(|m| m.request_focus(search_id()));
        }
        if reset_all {
            *s = reset_to_defaults(s);
            (*custom_size_on, *custom_fps_on) = (None, None);
        }
        if *s != before {
            self.settings_dirty = true;
        }
        if searching && rows.found != self.settings_view.found {
            ui.ctx().request_repaint();
        }
        self.settings_view.found = rows.found;
        self.settings_view.drawn = rows.drawn;
        // Still waiting when no row was drawn to take it.
        self.settings_view.focus_rows &= rows.focus_first;
        for (key, response) in links {
            self.focus_item(&response, key, false);
            if key == "profiles-link" && response.clicked() {
                commands.push(Command::SaveSettings);
                self.open(Screen::Profiles);
                return;
            }
        }
    }
}

/// Every setting the page shows back at its default, but the language: Android's Reset all
/// settings keeps it too. Settings with no row (beam racing, early decode) stay as they are.
pub(super) fn reset_to_defaults(settings: &Settings) -> Settings {
    let mut defaults = Overrides::default();
    for keys in SECTION_KEYS.iter().flat_map(|keys| keys.iter()) {
        if keys[0] != "language" {
            defaults.capture(&Settings::default(), keys);
        }
    }
    defaults.apply(settings)
}

/// The search found nothing: say so, and offer the way back. True when Clear search was clicked.
fn no_results(ui: &mut Ui, query: &str) -> bool {
    ui.add_space(space::XXL);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(trf("No settings match “{}”", &[&query])).font(semibold(text::CARD_TITLE)).color(ON_SURFACE));
        ui.add_space(space::XS);
        ui.label(RichText::new(tr("Try another word, or pick a section.")).font(regular(text::LABEL)).color(ON_SURFACE_VARIANT));
        ui.add_space(space::L);
        quiet_button(ui, "Clear search").clicked()
    })
    .inner
}

/// How a PC's own settings offer PC display scaling, as Android's per-PC "dpi" row: None hides
/// it on a host that can never read `rlDisplayScale`; Some(note) draws it, turned off with the
/// note on a host that could (an older Rubylight, a virtual display that is off or not ready).
/// The main settings and a PC not reached yet offer it; the launch sends it only to hosts with
/// `Feature::DisplayScale`.
pub(super) fn display_scale_gate(availability: nightjar_core::http::Availability) -> Option<Option<&'static str>> {
    use nightjar_core::http::{Availability, Reason};
    match availability {
        Availability::Supported | Availability::Unknown => Some(None),
        Availability::Unsupported(Some(Reason::UpdateHost)) => Some(Some(en("Update the host software to use this."))),
        Availability::Unsupported(Some(Reason::VirtualDisplayOff)) => Some(Some(en("Turn on the virtual display in the host’s settings."))),
        Availability::Unsupported(Some(Reason::VirtualDisplayDriver)) => Some(Some(en("The host’s virtual display driver isn’t ready."))),
        Availability::Unsupported(_) => None,
    }
}

pub(super) fn display_scale_name(percent: u32) -> String {
    if percent == 0 { tr("PC’s own").into() } else { format!("{percent}%") }
}

pub(super) fn host_display_name(mode: HostDisplay) -> &'static str {
    match mode {
        HostDisplay::HostDefault => en("Host default"),
        HostDisplay::Exclusive => en("Virtual display only"),
        HostDisplay::ExtendedPrimary => en("Extend as primary"),
        HostDisplay::Extended => en("Extend"),
        HostDisplay::ExtendedIsolated => en("Extend, isolated"),
        HostDisplay::ExtendedPrimaryIsolated => en("Extend as primary, isolated"),
        HostDisplay::Physical => en("Physical display"),
    }
}

pub(super) fn error_correction_name(mode: ErrorCorrection) -> String {
    match mode {
        ErrorCorrection::Host => tr("PC's setting").to_owned(),
        ErrorCorrection::Auto => tr("Automatic").to_owned(),
        ErrorCorrection::Percent(percentage) => trf("{} %", &[&percentage]),
    }
}

fn codec_name(codec: CodecPreference) -> &'static str {
    match codec {
        CodecPreference::Auto => tr("Automatic"),
        CodecPreference::H264 => "H.264",
        CodecPreference::Hevc => "HEVC",
        CodecPreference::Av1 => "AV1",
        CodecPreference::PyroWave => "PyroWave 4:2:0",
        CodecPreference::PyroWave444 => "PyroWave 4:4:4",
    }
}

fn decoder_name(decoder: DecoderPreference) -> &'static str {
    match decoder {
        DecoderPreference::Auto => tr("Automatic"),
        DecoderPreference::Software => tr("Software"),
    }
}

fn window_name(mode: WindowMode) -> &'static str {
    match mode {
        WindowMode::Fullscreen => tr("Fullscreen"),
        WindowMode::Borderless => tr("Borderless"),
        WindowMode::Windowed => tr("Windowed"),
    }
}

fn mouse_name(mode: MouseMode) -> &'static str {
    match mode {
        MouseMode::Capture => tr("Captured"),
        MouseMode::Absolute => tr("Desktop pointer"),
    }
}

fn scrolling_name(stick: crate::config::AnalogScrolling) -> &'static str {
    use crate::config::AnalogScrolling;
    match stick {
        AnalogScrolling::Off => tr("Off"),
        AnalogScrolling::Left => tr("Left"),
        AnalogScrolling::Right => tr("Right"),
    }
}

/// How long Guide is held to open the stream menu, as the settings show it: Off, or "1.5 s".
fn guide_hold_name(ms: u32) -> String {
    if ms == 0 {
        return tr("Off").to_owned();
    }
    let seconds = format!("{}", f64::from(ms) / 1000.0);
    let seconds = if crate::strings::current().decimal_comma() { seconds.replace('.', ",") } else { seconds };
    trf("{} s", &[&seconds])
}

fn audio_name(channels: AudioChannels) -> &'static str {
    match channels {
        AudioChannels::Stereo => tr("Stereo"),
        AudioChannels::Surround51 => tr("5.1 surround"),
        AudioChannels::Surround71 => tr("7.1 surround"),
        AudioChannels::Surround714 => tr("7.1.4 surround"),
    }
}

fn language_name(language: Language) -> &'static str {
    crate::strings::chosen(language).map_or_else(|| tr("System default"), crate::strings::Lang::name)
}

/// [`language_name`], or the English name where the loaded fonts lack its letters: they hold
/// the system font of the language in use only, so a Russian menu cannot draw 日本語.
fn drawable_language_name(ui: &egui::Ui, language: Language) -> &'static str {
    let name = language_name(language);
    let font = egui::FontId::proportional(crate::theme::text::BODY);
    match crate::strings::chosen(language) {
        Some(lang) if !ui.ctx().fonts_mut(|f| f.has_glyphs(&font, name)) => lang.english_name(),
        _ => name,
    }
}

fn reduce_motion_name(mode: ReduceMotion) -> &'static str {
    match mode {
        ReduceMotion::System => tr("Follow system"),
        ReduceMotion::Reduce => tr("Reduce"),
        ReduceMotion::Full => tr("Full motion"),
    }
}

/// What a row shows for its value, as its control says it: the default beside a Reset, and the
/// value a PC's or game's own setting stands in for.
pub(super) fn value_text(key: &str, s: &Settings, display: &Display, style: crate::glyphs::Style) -> String {
    match key {
        "open_on_start" => tr(s.open_on_start.label()).to_owned(),
        "width" | "height" if s.width == 0 => trf("Native ({}x{})", &[&display.width, &display.height]),
        "width" | "height" => format!("{}x{}", s.width, s.height),
        "fps" if s.fps == 0 => trf("Display ({} Hz)", &[&(display.refresh_hz.round() as u32)]),
        "fps" => trf("{} fps", &[&s.fps]),
        "bitrate_kbps" if s.bitrate_kbps == 0 => tr("Automatic").to_owned(),
        "bitrate_kbps" => format!("{} Mbps", f64::from(s.bitrate_kbps) / 1000.0),
        "adaptive_quality" => tr(s.adaptive_quality.label()).to_owned(),
        "quality_min_short_edge" => format!("{} px", s.quality_min_short_edge),
        "quality_min_fps" => trf("{} fps", &[&s.quality_min_fps]),
        "codec" => codec_name(s.codec).to_owned(),
        "upscaling" => tr(s.upscaling.label()).to_owned(),
        "upscale_sharpness" => format!("{}%", s.upscale_sharpness),
        "decoder" => decoder_name(s.decoder).to_owned(),
        "renderer" => tr(s.renderer.label()).to_owned(),
        "pyrowave_decoder" => tr(s.pyrowave_decoder.label()).to_owned(),
        "window_mode" => window_name(s.window_mode).to_owned(),
        "video_scaling" => tr(s.video_scaling.label()).to_owned(),
        "host_display" => tr(host_display_name(s.host_display)).to_owned(),
        "virtual_display_scale" => format!("{}%", s.virtual_display_scale),
        "virtual_display_dpi" => display_scale_name(s.virtual_display_dpi),
        "frame_pacing" => tr(s.frame_pacing.label()).to_owned(),
        "mouse_mode" => mouse_name(s.mouse_mode).to_owned(),
        "mouse_speed" => format!("{}%", s.mouse_speed),
        "gamepad_quit_combo" => style.quit_combo(s.gamepad_quit_combo),
        "stream_menu_buttons" if s.stream_menu_buttons.is_empty() => tr("Default").to_owned(),
        "stream_menu_buttons" => s.stream_menu_buttons.iter().map(|(key, id)| format!("{}: {}", key.cap(), stream_menu_button_name(Some(id)))).collect::<Vec<_>>().join(", "),
        "analog_scrolling" => scrolling_name(s.analog_scrolling).to_owned(),
        "guide_hold_ms" => guide_hold_name(s.guide_hold_ms),
        "audio" => audio_name(s.audio).to_owned(),
        "mic_echo_cancel" => tr(s.mic_echo_cancel.label()).to_owned(),
        "overlay" | "overlay_mode" if s.overlay => tr(s.overlay_mode.label()).to_owned(),
        "overlay" | "overlay_mode" => tr("Off").to_owned(),
        "overlay_position" => tr(s.overlay_position.label()).to_owned(),
        "network_grace_ms" => trf("{} seconds", &[&(s.network_grace_ms / 1000)]),
        "error_correction" => error_correction_name(s.error_correction),
        "language" => language_name(s.language).to_owned(),
        "ui_zoom" => format!("{}%", crate::config::nearest_zoom(s.ui_zoom)),
        "reduce_motion" => reduce_motion_name(s.reduce_motion).to_owned(),
        // The rest are switches.
        _ => match serde_json::to_value(s).ok().and_then(|v| v.get(key).and_then(serde_json::Value::as_bool)) {
            Some(true) => tr("On").to_owned(),
            Some(false) => tr("Off").to_owned(),
            None => String::new(),
        },
    }
}

/// The main rows of each section, as `client/app/src/main/res/values/settings_tiers.xml` lists them
/// by Android preference key, in `SETTINGS_SECTIONS` order. A test keeps the two equal, and another
/// that each section draws its main rows first, before its groups.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const MAIN_KEYS: [(&str, &[&str]); 8] = [
    ("video", &["list_resolution", "list_fps", "seekbar_bitrate_kbps", "checkbox_auto_bitrate", "list_mobile_bitrate_kbps", "video_format", "checkbox_enable_hdr"]),
    ("display", &["checkbox_stretch_video", "stream_orientation"]),
    ("input", &["checkbox_multi_controller", "controller_button_mapping", "controller_test", "checkbox_flip_face_buttons", "checkbox_touchscreen_trackpad", "checkbox_show_onscreen_controls", "checkbox_keyboard_follows_text_fields", "checkbox_clipboard_sync"]),
    ("audio", &["list_audio_config", "checkbox_host_audio", "checkbox_enable_mic", "checkbox_mic_button"]),
    ("streaming", &["checkbox_enable_perf_overlay", "checkbox_auto_reconnect"]),
    ("app", &["checkbox_always_dark", "list_languages", "checkbox_console_home", "checkbox_quick_resume", "pin_lock", "help", "report_problem", "check_updates_now"]),
    ("advanced", &["checkbox_rust_engine", "low_latency_decoding", "low_latency_customize", "gpu_driver", "checkbox_gpu_max_clocks", "checkbox_texture_view", "checkbox_vulkan_render"]),
    ("debug", &["list_fec_choice", "checkbox_rate_change_bitrate", "checkbox_tell_host_wireless", "checkbox_tell_host_link_speed", "checkbox_network_priority", "checkbox_unbatched_input", "checkbox_full_range"]),
];

/// Desktop rows and the Android preference each one matches. A row is main when its match is.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const ANDROID_KEYS: [(&str, &str); 39] = [
    ("open_on_start", "list_open_on_start"),
    ("width", "list_resolution"), ("fps", "list_fps"), ("bitrate_kbps", "seekbar_bitrate_kbps"),
    ("adaptive_bitrate", "checkbox_auto_bitrate"), ("codec", "video_format"), ("hdr", "checkbox_enable_hdr"),
    ("video_scaling", "checkbox_stretch_video"), ("controllers", "checkbox_multi_controller"),
    ("swap_face_buttons", "checkbox_flip_face_buttons"), ("clipboard_sync", "checkbox_clipboard_sync"),
    ("audio", "list_audio_config"), ("play_audio_on_host", "checkbox_host_audio"), ("mute_audio", "checkbox_mute_audio"),
    ("microphone", "checkbox_enable_mic"),
    ("overlay", "checkbox_enable_perf_overlay"), ("auto_reconnect", "checkbox_auto_reconnect"),
    ("console_home", "checkbox_console_home"), ("quick_resume", "checkbox_quick_resume"), ("language", "list_languages"),
    ("full_range", "checkbox_full_range"), ("error_correction", "list_fec_choice"),
    ("client_hint_bitrate", "checkbox_rate_change_bitrate"), ("experimental_packed_444", "experimental_packed_444"),
    ("tell_host_wireless", "checkbox_tell_host_wireless"), ("tell_host_link_speed", "checkbox_tell_host_link_speed"),
    ("link_probe", "checkbox_link_probe"), ("client_roaming", "checkbox_client_roaming"),
    ("audio_passthrough", "checkbox_audio_passthrough"), ("usb_forwarding", "checkbox_usb_forwarding"),
    ("unbatched_input", "checkbox_unbatched_input"), ("network_priority", "checkbox_network_priority"),
    ("input_only_any_host", "checkbox_input_only_any_host"),
    ("gamepad_quit_combo", "list_gamepad_quit_combo"), ("guide_hold_ms", "list_guide_menu_hold"), ("check_updates", "checkbox_check_updates"),
    ("frame_pacing", "frame_pacing"), ("mic_echo_cancel", "list_mic_echo_cancel"),
    ("pin_lock", "pin_lock"),
];

/// Main rows Android has no match for: the desktop's window and mouse, variable refresh, which
/// Android's display has none of, and the overlay's corner beside the overlay.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const DESKTOP_MAIN: [&str; 4] = ["window_mode", "mouse_mode", "vrr", "overlay_position"];

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn main_row(key: &str) -> bool {
    DESKTOP_MAIN.contains(&key)
        || ANDROID_KEYS.iter().any(|&(desktop, android)| desktop == key && MAIN_KEYS.iter().any(|(_, main)| main.contains(&android)))
}

/// The settings page's rows as it draws them: the open section's while browsing, every row the
/// search finds while searching, each section's and group's heading over its first row drawn.
pub(super) struct Rows<'a> {
    layer: Option<(&'a Settings, &'a mut Overrides)>,
    /// Where an unchecked row's value comes from when not the main settings (`Launcher::inherited_from`).
    sources: &'a [(Overrides, String)],
    /// What a layered row's box says: "For this PC", "For this game" or "In this profile".
    own: &'static str,
    /// The search's words; none while browsing.
    words: Vec<String>,
    /// The section shown while browsing.
    open: &'static str,
    section: &'static str,
    group: &'static str,
    /// The heading waiting for the first row drawn under it: the section's while searching
    /// (true), a group's title and note.
    heading: Option<(bool, &'static str)>,
    /// Rows drawn, per section.
    found: [usize; 8],
    /// Whether the rows that follow can be changed, and why not.
    enabled: bool,
    why: Option<&'static str>,
    /// The first row drawn takes focus (a new page).
    focus_first: bool,
    /// The keys whose row was drawn: a second row of the same setting (Custom resolution,
    /// Bitrate) gets no Reset of its own.
    reset_drawn: Vec<&'static str>,
    defaults: Settings,
    display: Display,
    style: crate::glyphs::Style,
    /// The first key of each setting row drawn, in order, with whether a group heading came
    /// before it in its section (tests).
    drawn: Vec<(&'static str, bool)>,
}

impl Rows<'_> {
    fn searching(&self) -> bool {
        !self.words.is_empty()
    }

    /// Starts `title`'s rows.
    fn section(&mut self, title: &'static str) {
        (self.section, self.group) = (title, "");
        self.heading = self.searching().then_some((true, title));
        self.always();
    }

    /// Starts a group of rows inside the section, such as Fine-tuning.
    fn group(&mut self, title: &'static str) {
        self.group = title;
        let section = match self.heading {
            Some((true, section)) => Some(section),
            _ => None,
        };
        self.heading = match section {
            // Searching, with no row of the section drawn yet: the section's heading still comes first.
            Some(section) => Some((true, section)),
            None => Some((false, title)),
        };
        self.always();
    }

    /// The rows that follow can be changed only while `enabled`, else they say `why` (empty for
    /// rows that explain it themselves). Every call narrows; `always` lifts it.
    fn when(&mut self, enabled: bool, why: &'static str) {
        if !enabled && self.enabled {
            self.why = (!why.is_empty()).then_some(why);
        }
        self.enabled &= enabled;
    }

    fn always(&mut self) {
        (self.enabled, self.why) = (true, None);
    }

    /// Whether the row is drawn; draws the headings waiting for it.
    fn shows(&mut self, ui: &mut Ui, label: &str, description: &str) -> bool {
        let at = SETTINGS_SECTIONS.iter().position(|s| *s == self.section).unwrap_or(0);
        let visible = if self.searching() {
            found(&self.words, &[tr(label), tr(description), tr(self.section), if self.group.is_empty() { "" } else { tr(self.group) }])
        } else {
            self.section == self.open
        };
        if !visible {
            return false;
        }
        if let Some((section, title)) = self.heading.take() {
            if section {
                search_heading(ui, title);
                // The group this row is in, right under the section, as close as a row would be.
                if !self.group.is_empty() {
                    group_heading(ui, self.group, 0.0);
                }
            } else {
                group_heading(ui, title, space::XL);
            }
        }
        self.found[at] += 1;
        true
    }

    /// A line of help under the section's rows, such as the stream's shortcuts.
    fn caption(&mut self, ui: &mut Ui, text: &str) {
        if !self.searching() && self.section == self.open {
            ui.add_space(space::M);
            ui.label(RichText::new(text).color(OUTLINE).font(regular(text::CAPTION)));
        }
    }
}

/// A section's heading among search results.
fn search_heading(ui: &mut Ui, title: &str) {
    ui.add_space(space::L);
    ui.label(RichText::new(tr(title)).font(semibold(text::SECTION)).color(ON_SURFACE));
    ui.add_space(space::XS);
}

/// A group's heading inside a section, `above` apart from what comes before it.
fn group_heading(ui: &mut Ui, title: &str, above: f32) {
    ui.add_space(above);
    ui.label(RichText::new(tr(title)).font(semibold(text::SECTION)).color(ON_SURFACE));
    ui.add_space(space::XS);
}

/// A setting's row, or None while it is not shown (another section, or the search passes it).
/// `keys` empty: a row of its own (a link, an action) with no default and no layer box.
/// A setting's description and, while `action` has a stream hotkey, the chord the user bound for it.
fn with_hotkey(description: &str, action: crate::shortcuts::Action) -> String {
    match crate::shortcuts::chord_for(crate::shortcuts::Scope::Stream, action) {
        Some(chord) => format!("{description} {}", trf("{} switches it during a stream.", &[&chord.text()])),
        None => description.to_owned(),
    }
}

fn stream_menu_button_name(choice: Option<&str>) -> String {
    match choice.map(nightjar_policy::stream_menu::saved_choice) {
        None => tr("Default").to_owned(),
        Some(NO_TILE) => tr("Off").to_owned(),
        Some(id) => crate::stream_menu::ASSIGNABLE.iter().find(|(tile, _)| *tile == id).map_or(id, |(_, name)| tr(name)).to_owned(),
    }
}

/// A button's saved choice as the list shows it: close_game, saved before Quit game was one
/// choice, is Quit game, so the list highlights it.
fn stream_menu_button_choice(s: &Settings, key: Hotkey) -> Option<String> {
    s.stream_menu_buttons.get(&key).map(|saved| nightjar_policy::stream_menu::saved_choice(saved).to_owned())
}

/// The list of what a controller button can run in the stream menu: its own tile, nothing, or any of ours.
fn stream_menu_button(ui: &mut Ui, s: &mut Settings, key: Hotkey) -> Response {
    let mut choice = stream_menu_button_choice(s, key);
    let response = egui::ComboBox::from_id_salt(("stream-menu-button", key.cap())).selected_text(stream_menu_button_name(choice.as_deref())).show_ui(ui, |ui| {
        ui.selectable_value(&mut choice, None, tr("Default"));
        ui.selectable_value(&mut choice, Some(NO_TILE.to_owned()), tr("Off"));
        for (tile, label) in crate::stream_menu::ASSIGNABLE {
            ui.selectable_value(&mut choice, Some(tile.to_owned()), tr(label));
        }
    }).response;
    match choice {
        Some(id) => { s.stream_menu_buttons.insert(key, id); }
        None => { s.stream_menu_buttons.remove(&key); }
    }
    response
}

pub(super) fn setting_row(
    ui: &mut Ui,
    settings: &mut Settings,
    rows: &mut Rows<'_>,
    keys: &[&'static str],
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, &mut Settings) -> Response,
) -> Option<Response> {
    if !rows.shows(ui, label, description) {
        return None;
    }
    if let Some(&first) = keys.first() {
        let grouped = !rows.group.is_empty();
        rows.drawn.push((first, grouped));
    }
    let (enabled, why) = (rows.enabled, rows.why.filter(|_| !rows.enabled));
    let first = keys.first().copied();
    let second = first.is_some_and(|k| rows.reset_drawn.contains(&k));
    if let Some(k) = first {
        rows.reset_drawn.push(k);
    }
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
            settings_row(ui, label, description, why, Some(state), own, enabled, 0.0, |ui| {
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    ui.set_width(CONTROL_W);
                    // An unchecked row's control is reachable too (a controller's focus skips
                    // disabled ones, which left only the box): choosing another value ticks the
                    // box, opening a menu and leaving it changes nothing.
                    // Read before the control: a number field consumes the arrow keys that step it.
                    let (before, chose) = (settings.clone(), chosen(ui));
                    let value = ui.add_enabled_ui(enabled, |ui| control(ui, settings)).inner;
                    name_for_readers(ui, &value, label, description);
                    // The box on the row's last line, beside where its value comes from, however
                    // short the control or long the description.
                    let room = ui.available_height() - (ui.spacing().interact_size.y + ui.text_style_height(&egui::TextStyle::Small)) / 2.0;
                    if room > 0.0 && room.is_finite() {
                        ui.add_space(room);
                    }
                    let mut ticked = own || (*settings != before && chose);
                    // A value of its own on a row turned off can still be given back.
                    let boxed = ui.add_enabled_ui(enabled || own, |ui| layer_box(ui, &mut ticked, box_label)).inner;
                    name_for_readers(ui, &boxed, box_label, label);
                    if boxed.changed() && !ticked {
                        overrides.clear(keys);
                        *settings = overrides.apply(inherited);
                    } else if ticked {
                        overrides.capture(settings, keys);
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
            let response = settings_row(ui, label, description, why, None, false, enabled, reset_w, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let value = control(ui, settings);
                    name_for_readers(ui, &value, label, description);
                    if differs {
                        let button = text_button(ui, "Reset").on_hover_text(trf("Default: {}", &[&default]));
                        let name = trf("Reset {} to its default, {}", &[&tr(label), &default]);
                        ui.ctx().accesskit_node_builder(button.id, |node| node.set_label(name));
                        reset = button.clicked();
                    }
                    value
                }).inner
            });
            if reset {
                let mut defaults = Overrides::default();
                defaults.capture(&rows.defaults, keys);
                *settings = defaults.apply(settings);
            }
            response
        }
        _ => settings_row(ui, label, description, why, None, false, enabled, 0.0, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let value = control(ui, settings);
                name_for_readers(ui, &value, label, description);
                value
            }).inner
        }),
    };
    if std::mem::take(&mut rows.focus_first) {
        response.request_focus();
    }
    Some(response)
}

/// A row: label, description and any notes on the left, the control on the right. A row a PC,
/// game or profile sets has an amber bar at its left edge. `reset_w` keeps room left of the
/// control for a Reset, so the text never moves when it comes and goes. `enabled` only dims the
/// text: `control` turns off what it draws itself, so a layer's box can stay on.
#[allow(clippy::too_many_arguments)]
fn settings_row(
    ui: &mut Ui,
    label: &str,
    description: &str,
    why: Option<&str>,
    state: Option<(String, Color32)>,
    own: bool,
    enabled: bool,
    reset_w: f32,
    control: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    let mut out = None;
    let rect = group_row(ui, |ui| {
        ui.horizontal(|ui| {
            let text_w = (ui.available_width() - CONTROL_W - reset_w - space::L).max(180.0);
            // Where a list choice's box lands under it: where the value comes from sits beside it.
            let to_state = COMBO_HEIGHT + 18.0;
            ui.vertical(|ui| {
                let top = ui.cursor().top();
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.set_width(text_w);
                let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.6) };
                ui.label(RichText::new(tr(label)).font(semibold(text::BODY)).color(dim(*ON_SURFACE)));
                ui.label(RichText::new(tr(description)).font(regular(text::CAPTION)).color(dim(*ON_SURFACE_VARIANT)));
                if let Some(why) = why {
                    ui.label(RichText::new(tr(why)).font(semibold(text::CAPTION)).color(TERTIARY));
                }
                if let Some((state, color)) = &state {
                    ui.add_space((top + to_state - ui.cursor().top()).max(0.0));
                    ui.label(RichText::new(state).font(regular(text::CAPTION)).color(*color));
                }
            });
            let align = if state.is_some() { Align::Min } else { Align::Center };
            ui.with_layout(Layout::right_to_left(align), |ui| out = Some(control(ui)));
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
fn layer_box(ui: &mut Ui, ticked: &mut bool, label: &'static str) -> Response {
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
fn chosen(ui: &Ui) -> bool {
    ui.input(|i| {
        i.pointer.any_released() || i.pointer.any_down()
            || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::AccessKitActionRequest(_)))
    })
}

/// What a section's entry says on its right.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Badge {
    None,
    /// Rows the search found in it.
    Found(usize),
    /// Rows a PC, game or profile sets in it.
    Set(usize),
}

/// A section's mark in the list beside the rows.
fn section_icon(name: &str) -> Icon {
    match name {
        n if n == en("Video") => Icon::Video,
        n if n == en("Display & image") => Icon::Sun,
        n if n == en("Controls") => Icon::Gamepad,
        n if n == en("Audio") => Icon::Speaker,
        n if n == en("Streaming") => Icon::Wifi,
        n if n == en("App") => Icon::App,
        n if n == en("Advanced") => Icon::Sliders,
        _ => Icon::Info,
    }
}

/// A section in the list beside the rows: the current one in the selection's tint.
fn section_item(ui: &mut Ui, name: &str, current: bool, badge: Badge) -> Response {
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
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, current, &spoken));
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

/// A section's chip on a narrow window, where the list beside the rows has no room.
fn section_chip(ui: &mut Ui, name: &str, current: bool) -> Response {
    if !current {
        return chip(ui, name);
    }
    let button = egui::Button::new(RichText::new(tr(name)).color(*ON_MOON_CONTAINER).font(semibold(text::CAPTION)))
        .fill(*MOON_CONTAINER)
        .selected(true)
        .corner_radius(CornerRadius::same(14))
        .min_size(vec2(0.0, 28.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 14);
    response
}

/// A card across the page above the rows: words on the left, an action on the right.
fn banner(ui: &mut Ui, words: impl FnOnce(&mut Ui), action: impl FnOnce(&mut Ui) -> Response) -> Response {
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
fn notice(ui: &mut Ui, text: &str, button: &'static str) -> Response {
    banner(ui, |ui| {
        ui.label(RichText::new(text).font(regular(text::LABEL)).color(ON_SURFACE));
    }, |ui| quiet_button(ui, button))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_needs_every_word_somewhere_in_the_row() {
        let words = search_words("  Frame   RATE ");
        assert_eq!(words, ["frame", "rate"]);
        assert!(found(&words, &["Frame rate", "Pictures per second.", "Video", ""]));
        assert!(found(&search_words("video pictures"), &["Frame rate", "Pictures per second.", "Video", ""]), "words may come from the section and the description");
        assert!(!found(&search_words("frame audio"), &["Frame rate", "Pictures per second.", "Video", ""]));
    }

    #[test]
    fn reset_all_keeps_the_language_and_the_settings_without_a_row() {
        let changed = Settings {
            fps: 120,
            hdr: true,
            overlay: true,
            overlay_mode: crate::config::OverlayMode::Debug,
            language: Language::German,
            beam_race: true,
            ui_zoom: 150,
            ..Settings::default()
        };
        let reset = reset_to_defaults(&changed);
        assert_eq!(reset, Settings { language: Language::German, beam_race: true, ..Settings::default() });
    }

    #[test]
    fn every_listed_row_names_a_setting_and_says_its_value() {
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
        for keys in SECTION_KEYS.iter().flat_map(|keys| keys.iter()) {
            for key in *keys {
                assert!(defaults.get(*key).is_some(), "{key} is a setting");
            }
            assert!(!value_text(keys[0], &Settings::default(), &display, crate::glyphs::Style::Xbox).is_empty(), "{} says its value", keys[0]);
        }
        assert_eq!(value_text("fps", &Settings::default(), &display, crate::glyphs::Style::Xbox), "Display (60 Hz)");
        assert_eq!(value_text("overlay", &Settings { overlay_mode: crate::config::OverlayMode::Debug, ..Settings::default() }, &display, crate::glyphs::Style::Xbox), "Off");
    }

    /// A button given Close game before it became Quit game reads as the one Quit game choice.
    #[test]
    fn a_saved_close_game_button_reads_quit_game() {
        assert_eq!(stream_menu_button_name(Some("close_game")), "Quit game");
        assert_eq!(stream_menu_button_name(Some("quit_game")), "Quit game");
        assert_eq!(crate::stream_menu::ASSIGNABLE.iter().filter(|(_, name)| *name == "Quit game").count(), 1, "one Quit game in the list");
        // The list highlights it too.
        let mut s = Settings::default();
        s.stream_menu_buttons.insert(Hotkey::Lb, "close_game".to_owned());
        s.stream_menu_buttons.insert(Hotkey::Y, "overlay".to_owned());
        assert_eq!(stream_menu_button_choice(&s, Hotkey::Lb).as_deref(), Some("quit_game"));
        assert_eq!(stream_menu_button_choice(&s, Hotkey::Y).as_deref(), Some("overlay"));
        assert_eq!(stream_menu_button_choice(&s, Hotkey::X), None);
    }
}

#[cfg(test)]
mod banner_tests {
    use super::*;

    #[test]
    fn a_banners_words_and_action_share_a_middle() {
        let ctx = egui::Context::default();
        crate::theme::own_palette();
        crate::theme::apply(&ctx);
        let (mut words, mut action) = (Rect::NOTHING, Rect::NOTHING);
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 400.0))), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                action = banner(ui, |ui| words = ui.label("A line").rect, |ui| ui.add(egui::Button::new("Go").min_size(vec2(80.0, CONTROL_HEIGHT)))).rect;
            });
            output.textures_delta.clear();
        }
        assert!((words.center().y - action.center().y).abs() <= 1.0, "words {words:?}, action {action:?}");
    }
}
