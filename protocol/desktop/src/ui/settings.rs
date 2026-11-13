//! The settings page: a list of sections on the left (chips on a narrow window), Profiles above
//! them, and the open section's rows on the right. Each section shows its main rows first, as
//! Android's `settings_tiers.xml` lists them, then a counted fold, "More … settings (n)", over the
//! rest in their groups. A row shows its value; its one-line hint shows while the pointer is over
//! it or focus is in it. A row that another setting or this device rules out is not drawn. A search
//! finds rows in every section, folded or not. A PC's, game's or profile's settings show the same
//! rows, each with a box that keeps its value for that layer.

use super::*;
use nightjar_policy::controllers::Controllers;
use nightjar_policy::open_play::StartScreen;
use nightjar_policy::presets::{self, Preset};
use crate::config::FramePacing;
use nightjar_policy::stream_menu::{Hotkey, NO_TILE};

/// The settings sections in the order Android lists them (App only on the main settings).
pub(super) const SETTINGS_SECTIONS: [&str; 8] = [en("Video"), en("Display & image"), en("Controls"), en("Audio"), en("Streaming"), en("App"), en("Advanced"), en("Debug")];

/// Every setting row's keys, by section, in the order the page draws them with every fold open;
/// the first key names the row. Rows that show only with another setting (Custom resolution,
/// Bitrate, the controller mouse's options) share their keys or are listed where they show. Action
/// rows (a page, a test) store nothing and are not listed. A test keeps this equal to what the
/// page draws.
pub(super) const SECTION_KEYS: [&[&[&str]]; 8] = [
    &[&["width", "height"], &["fps"], &["bitrate_kbps"], &["adaptive_bitrate"], &["codec"], &["hdr"],
        &["prefer_yuv444", "experimental_packed_444"], &["sdr_10bit"], &["frame_warp"], &["adaptive_quality"], &["quality_min_short_edge"], &["quality_min_fps"],
        &["hdr_desktop_brightness"]],
    &[&["window_mode"], &["video_scaling"], &["vrr"],
        &["present_at_host_times"], &["frame_pacing"], &["drop_late_frames"], &["vsync"],
        &["upscaling"], &["upscale_sharpness"], &["align_video_top"],
        &["virtual_display"], &["virtual_display_scale"], &["virtual_display_dpi"], &["host_display"]],
    &[&["mouse_mode"], &["controllers", "multi_controller"], &["clipboard_sync"],
        &["swap_face_buttons"], &["gamepad_quit_combo"], &["deadzone"], &["trigger_deadzone"], &["controller_motion"], &["disable_rumble"],
        &["controller_mouse"], &["analog_scrolling"], &["mouse_fixed_step"], &["gamepad_touchpad_as_mouse"], &["background_controllers"],
        &["guide_hold_ms"], &["back_opens_menu"], &["back_as_guide"], &["stream_menu_buttons"],
        &["mouse_speed"], &["mouse_nav_buttons"], &["native_touchpad"],
        &["capture_system_keys"], &["system_keys_fullscreen_only"], &["right_alt_as_meta"],
        &["controller_audio"], &["usb_forwarding"]],
    &[&["audio"], &["play_audio_on_host"], &["microphone"], &["mic_echo_cancel"], &["mic_noise_suppression"], &["mic_auto_gain"], &["mic_gain"],
        &["mute_audio"], &["audio_passthrough"]],
    &[&["overlay", "overlay_mode"], &["overlay_position"], &["auto_reconnect"],
        &["client_roaming"], &["roaming_metered"], &["network_grace_ms"], &["receive_coalescing"], &["hide_connection_warnings"],
        &["dual_screen"], &["second_screen_width", "second_screen_height"], &["input_only_any_host"]],
    &[&["theme"], &["language"], &["open_on_start", "console_home"], &["quick_resume"], &["check_updates"], &["home_card_size"],
        &["ui_zoom"], &["high_contrast"], &["reduce_motion"]],
    &[&["graphics_card"], &["renderer"], &["sops"], &["link_probe"], &["post_stream_decode_time"]],
    &[&["error_correction"], &["client_hint_bitrate"], &["tell_host_wireless"], &["tell_host_link_speed"],
        &["network_priority"], &["unbatched_input"], &["full_range"],
        &["pyrowave_decoder"], &["pyrowave_async_compute"],
        &["decoder"], &["software_slices"], &["vulkan_video_first"], &["vulkan_rfi"], &["cursor_shapes"]],
];

/// The Resolution menu after Native: Android's 16:9 sizes with common PC and laptop ones, then
/// 16:10, then ultrawide. Custom… takes any other size.
pub(super) const RESOLUTIONS: [(u32, u32); 15] = [
    (640, 360), (854, 480), (1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160),
    (1280, 800), (1920, 1200), (2560, 1600), (3840, 2400),
    (2560, 1080), (3440, 1440), (5120, 1440),
];

/// The Second screen resolution menu after the screen's own size: Android's
/// `resolution_screen2_values`.
pub(super) const SECOND_SCREEN_RESOLUTIONS: [(u32, u32); 6] =
    [(640, 360), (854, 480), (1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// The Frame rate menu after Display: Android's 30-120 and common monitor rates. Custom… takes
/// any rate in `CUSTOM_FPS`, as the stream menu's rate does.
pub(super) const FRAME_RATES: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];
pub(super) const CUSTOM_FPS: std::ops::RangeInclusive<u32> = 10..=500;

/// Bitrate on mobile data's choices in kbps, Off first: Android's `mobile_bitrate_values`. It is
/// the profiles' `mobile_limit_kbps`, which the Profiles page sets to any value too.
pub(super) const MOBILE_BITRATES_KBPS: [u32; 8] = [0, 3_000, 5_000, 10_000, 15_000, 20_000, 30_000, 50_000];

/// The page lays the section list beside the rows from this width on; below it, chips.
const SIDE_LIST_FROM: f32 = 940.0;
const SIDE_LIST_W: f32 = 236.0;
const SIDE_INSET: f32 = 6.0;
const SIDE_ITEM_H: f32 = 44.0;
/// The width a row's control column takes, as on the other settings pages.
const CONTROL_W: f32 = 240.0;
/// The PC list in a test's row.
const TEST_PC_W: f32 = 150.0;

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
    /// Each section's fold is open (its "More … settings" chosen), until Nightjar closes.
    pub(super) more: [bool; 8],
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
    /// The rows the last frame drew, by id (a setting's first key, an action's name), each with
    /// whether it sits behind its section's fold.
    pub(super) drawn: Vec<(&'static str, bool)>,
    /// The PC chosen for the main settings' Connection test and PyroWave frame pace, until
    /// Nightjar closes.
    pub(super) test_pc: Option<u64>,
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

fn section_index(name: &str) -> usize {
    SETTINGS_SECTIONS.iter().position(|s| *s == name).unwrap_or(0)
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
        let mut profiles = false;
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
            (entries, profiles) = self.section_list(&mut side, sections, layered);
            let drawn = side.min_rect().height();
            ui.data_mut(|d| d.insert_temp(height_id, drawn));
            side_bottom = page.top() + drawn;
            Rect::from_min_max(Pos2::new(page.left() + SIDE_LIST_W + space::XL, page.top()), page.max)
        } else {
            self.search_field(ui);
            ui.add_space(space::S);
            ui.horizontal_wrapped(|ui| {
                // Profiles first, as above the list on a wide window: a page of its own, not a section.
                if !layered {
                    let chip = chip(ui, "Profiles");
                    self.focus_item(&chip, "profiles-link", false);
                    profiles = chip.clicked();
                }
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
        if profiles {
            commands.push(Command::SaveSettings);
            self.open(Screen::Profiles);
            return;
        }
        let mut column = ui.new_child(UiBuilder::new().id_salt("settings-rows").max_rect(rows_rect).layout(Layout::top_down(Align::Min)));
        let screen = self.screen;
        // Android's Presets, above the sections' rows; a PC's, game's or profile's settings and a
        // search have none.
        if !layered && !self.settings_view.searching() {
            self.presets_row(&mut column);
        }
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

    /// Presets: Android's chips for one-tap sets of resolution, frame rate and bitrate, the one
    /// the settings are marked, else the value says Custom. A chip asks before it replaces them
    /// (`Dialog::ConfirmPreset`).
    fn presets_row(&mut self, ui: &mut Ui) {
        let vrr_offered = self.tearing_supported || cfg!(not(windows));
        let chosen = presets::chosen(&preset_current(&self.settings, vrr_offered));
        let value = chosen.map_or_else(|| tr("Custom").to_owned(), preset_value);
        let mut asked = None;
        group_row(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(design_tokens::SIZE_ROW_HEIGHT - 2.0 * 8.0);
                // The label takes what the chips leave inside the row, so the chips end at the
                // row's right edge: a wider row would widen the whole page of rows below it. The
                // chips' width is the last frame's, their labels' before the first.
                let chips_id = Id::unique("settings-preset-chips");
                let chips_w = ui.data(|d| d.get_temp::<f32>(chips_id)).unwrap_or_else(|| {
                    let font = semibold(text::CAPTION);
                    Preset::ALL.iter().map(|&p| ui.painter().layout_no_wrap(tr(preset_label(p)).to_owned(), font.clone(), Color32::WHITE).size().x + 40.0).sum()
                });
                let gap = ui.spacing().item_spacing.x;
                let label_w = (ui.available_width() - gap - chips_w).max(120.0);
                ui.vertical(|ui| {
                    ui.set_width(label_w);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(tr("Presets")).font(semibold(text::BODY)).color(ON_SURFACE));
                    ui.label(RichText::new(value).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                });
                let start = ui.cursor().left();
                let mut end = start;
                for preset in Preset::ALL {
                    let chip = section_chip(ui, preset_label(preset), chosen == Some(preset));
                    name_for_readers(ui, &chip, preset_label(preset), en("One-tap sets of resolution, frame rate and bitrate."));
                    self.focus_item(&chip, preset_key(preset), false);
                    end = chip.rect.right();
                    if chip.clicked() {
                        asked = Some(preset);
                    }
                }
                if (end - start - chips_w).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(chips_id, end - start));
                    ui.ctx().request_repaint();
                }
            });
        });
        if let Some(preset) = asked {
            self.dialog = Dialog::ConfirmPreset(preset);
            self.focus_dialog = true;
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

    /// Profiles, then the sections, the current one marked. While searching each that found rows
    /// says how many, and a click on any ends the search; on a PC's, game's or profile's settings
    /// each says how many it sets. Returns each section entry's id, and whether Profiles was chosen.
    fn section_list(&mut self, ui: &mut Ui, sections: &[&'static str], layered: bool) -> (Vec<(&'static str, Id)>, bool) {
        let searching = self.settings_view.searching();
        let mut entries = Vec::new();
        let under = ui.painter().add(egui::Shape::Noop);
        let top = ui.cursor().top();
        ui.add_space(SIDE_INSET);
        // Profiles is a page of its own above the sections, as on Android.
        let mut profiles = false;
        if !layered {
            let item = section_item(ui, en("Profiles"), false, Badge::None, egui::Role::Link);
            name_for_readers(ui, &item, en("Profiles"), en("Different settings for different networks."));
            self.focus_item(&item, "profiles-link", false);
            profiles = item.clicked();
        }
        for name in sections {
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
        ui.add_space(SIDE_INSET - ui.spacing().item_spacing.y);
        let list = Rect::from_min_max(Pos2::new(ui.max_rect().left(), top), Pos2::new(ui.max_rect().right(), ui.cursor().top()));
        let mut shapes = Vec::new();
        pcs::canvas_on_moon(&mut shapes, ui.ctx(), list, f32::from(radius::PANEL));
        glass::group(&mut shapes, ui.ctx(), list, f32::from(radius::PANEL), rows_fill());
        ui.painter().set(under, egui::Shape::Vec(shapes));
        (entries, profiles)
    }

    /// The open section's rows, or every row the search finds.
    fn settings_rows(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let display = self.display;
        let layered = self.screen != Screen::Settings;
        let searching = self.settings_view.searching();
        let open = self.settings_view.section;
        let mut links: Vec<(&str, Response)> = Vec::new();
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
            shown: true,
            more: self.settings_view.more,
            mains: 0,
            past_fold: false,
            folded: 0,
            fold: None,
            toggled: None,
            focus_first,
            reset_drawn: Vec::new(),
            defaults: Settings::default(),
            display,
            style,
            drawn: Vec::new(),
            action: None,
            note: None,
            room: 0.0,
        };
        let screen = self.screen;
        // Variable refresh rate needs tearing on Windows: where the swap chain can't, the row is not
        // drawn and the rows that apply without VRR are.
        let vrr_offered = self.tearing_supported || cfg!(not(windows));
        // Bitrate on mobile data is the profiles' cap, for every stream: on the main settings, once
        // this device was on mobile data or a cap is set (Android's mobileBitrateShown).
        let mut mobile_limit = self.profiles.mobile_limit_kbps;
        let mobile_shown = !layered && (self.network.kind(&self.profiles.home_wifi) == crate::network::Kind::Mobile || mobile_limit > 0);
        // The tests a PC's own page runs on that PC and the main settings on a paired one.
        let connection_pcs = test_pcs(screen, view.hosts, |_| true);
        let pace_pcs = test_pcs(screen, view.hosts, |h| h.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe) || h.record.pace.is_some());
        let open_game = self.open_game.as_ref().map(|g| g.title.clone());
        let (custom_size_on, custom_fps_on) = (&mut self.custom_size, &mut self.custom_fps);
        let confirm_reset = &mut self.settings_view.confirm_reset;
        let test_pc = &mut self.settings_view.test_pc;
        let s = if layered { &mut self.edit_settings } else { &mut self.settings };
        let before = s.clone();
        let (mut reset_all, mut clear_search) = (false, false);
        // A page the rows open: Controller buttons (with the test on or off), Help, About, the
        // latency benchmark.
        let (mut open_controllers, mut open_help) = (None, false);
        let mut open_page: Option<Screen> = None;
        {
            if searching {
                ui.label(RichText::new(trf("Results for “{}”", &[&self.settings_view.query.trim()])).font(semibold(text::SECTION)).color(PAPER));
            } else {
                // Under a layer's own title and notice, the section is one step down.
                let size = if layered { text::SECTION } else { chrome::PAGE_TITLE };
                ui.label(RichText::new(tr(open)).font(semibold(size)).color(PAPER));
            }
            ui.add_space(space::M);

            // The GPU decoder's probe runs in the background, on the renderer's adapter and the HDR
            // plane format, and only when it matters: PyroWave is selected or the codec menu is
            // open. PyroWave is not offered while it fails, and the reason is shown with the
            // decoder setting in Debug.
            let probe_device = self.gpu_device.map_or(std::ptr::null_mut(), |d| d.0);
            let (gpu_mode, ten_bit) = (s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu, s.hdr);
            let gpu_probe = move |ui: &Ui| {
                if !gpu_mode { return None; }
                let probe = crate::stream::pyrowave_mode::settings_probe(probe_device, ten_bit);
                if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                probe.and_then(Result::err)
            };
            let gpu_decoder_error = if s.codec.is_pyrowave() { gpu_probe(ui) } else { None };

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
            setting_row(ui, s, &mut rows, &["width", "height"], "Resolution", "The picture size the PC sends; smaller needs less network and decoding power.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["fps"], "Frame rate", "Pictures per second; higher looks smoother and needs a faster network.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Automatic bitrate", "Nightjar picks the bitrate for the resolution, frame rate and codec.", |ui, s| {
                let r = toggle(ui, &mut automatic);
                if r.changed() {
                    s.bitrate_kbps = if automatic { 0 } else { recommended };
                }
                r
            });
            if !automatic {
                let mut mbps = bitrate as f32 / 1000.0;
                // A chosen bitrate below PyroWave's minimum stays; the row says what it costs.
                if let Some(warning) = pyrowave_bitrate_warning(s, w, h, fps) {
                    rows.note(warning, *TERTIARY);
                }
                setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Bitrate", "Video data per second; higher is sharper and needs a faster network.", |ui, s| {
                    let r = log_slider(ui, &mut mbps, 0.5..=2000.0, 0.5, mbps_text,
                        |rail| rail.logarithmic(true).step_by(0.5),
                        |field| field.suffix(" Mbps").max_decimals(1).speed(0.5));
                    if r.changed() {
                        s.bitrate_kbps = (mbps * 1000.0) as u32;
                    }
                    r
                });
            }
            setting_row(ui, s, &mut rows, &["adaptive_bitrate"], "Lower bitrate when the network is busy", "Turns the bitrate down while the connection falls behind, and back up once it recovers.", |ui, s| {
                toggle(ui, &mut s.adaptive_bitrate)
            });
            if mobile_shown {
                rows.action("mobile_bitrate");
                setting_row(ui, s, &mut rows, &[], "Bitrate on mobile data", "The highest bitrate a stream uses on mobile data or a metered network.", |ui, _| {
                    Combo::from_id_salt("mobile-bitrate").selected_text(mobile_bitrate_name(mobile_limit)).show_ui(ui, |ui| {
                        // A cap set to another value on the Profiles page stays on the list.
                        let listed = MOBILE_BITRATES_KBPS.contains(&mobile_limit);
                        for kbps in MOBILE_BITRATES_KBPS.into_iter().chain((!listed).then_some(mobile_limit)) {
                            ui.selectable_value(&mut mobile_limit, kbps, mobile_bitrate_name(kbps));
                        }
                    }).response
                });
            }
            setting_row(ui, s, &mut rows, &["codec"], "Video codec", "How the video is compressed; Automatic suits most setups.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["hdr"], "HDR", "Brighter highlights and richer color on an HDR screen.", |ui, s| toggle(ui, &mut s.hdr));
            // PyroWave's 4:4:4 is a codec of its own.
            rows.when(!s.codec.is_pyrowave());
            setting_row(ui, s, &mut rows, &["prefer_yuv444", "experimental_packed_444"], "Sharper colored text", "Full color detail, so colored text and thin lines stay crisp.", |ui, s| {
                let mut on = sharp_color(s);
                let response = Combo::from_id_salt("sharp-color").selected_text(sharp_color_name(on)).show_ui(ui, |ui| {
                    for choice in [false, true] {
                        ui.selectable_value(&mut on, choice, sharp_color_name(choice));
                    }
                }).response;
                if on != sharp_color(s) {
                    (s.prefer_yuv444, s.experimental_packed_444) = (on, on);
                }
                response
            });
            rows.when(!s.hdr);
            setting_row(ui, s, &mut rows, &["sdr_10bit"], "Less color banding", "Streams non-HDR video in 10-bit, so gradients show fewer bands.", |ui, s| toggle(ui, &mut s.sdr_10bit));
            rows.always();
            setting_row(ui, s, &mut rows, &["frame_warp"], "Frame Warp",
                "Shifts the picture with your mouse or right stick until the PC’s next frame shows the turn, and keeps the game’s HUD in place. Needs a PC that supports it.",
                |ui, s| toggle(ui, &mut s.frame_warp));
            setting_row(ui, s, &mut rows, &["adaptive_quality"], "Automatic quality", "Lowers the resolution or frame rate while the PC, network or device can’t keep up.", |ui, s| {
                Combo::from_id_salt("quality").selected_text(tr(s.adaptive_quality.label())).show_ui(ui, |ui| {
                    for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                        ui.selectable_value(&mut s.adaptive_quality, mode, tr(mode.label()));
                    }
                }).response
            });
            rows.when(s.adaptive_quality != crate::config::QualityMode::Off);
            setting_row(ui, s, &mut rows, &["quality_min_short_edge"], "Minimum resolution", "The lowest resolution Automatic quality goes to.", |ui, s| {
                Combo::from_id_salt("quality-min-size").selected_text(short_edge_name(s.quality_min_short_edge)).show_ui(ui, |ui| {
                    for edge in nightjar_policy::quality::MIN_SHORT_EDGE_CHOICES {
                        ui.selectable_value(&mut s.quality_min_short_edge, edge, short_edge_name(edge));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["quality_min_fps"], "Minimum frame rate", "The lowest frame rate Automatic quality goes to.", |ui, s| {
                Combo::from_id_salt("quality-min-fps").selected_text(trf("{} fps", &[&s.quality_min_fps])).show_ui(ui, |ui| {
                    for fps in nightjar_policy::quality::MIN_FPS_CHOICES {
                        ui.selectable_value(&mut s.quality_min_fps, fps, trf("{} fps", &[&fps]));
                    }
                }).response
            });
            rows.always();
            // Only a Wayland desktop's HDR description has the choice; Windows maps HDR itself.
            if cfg!(target_os = "linux") {
                rows.when(s.hdr);
                setting_row(ui, s, &mut rows, &["hdr_desktop_brightness"], "HDR brightness follows the desktop", "HDR streams follow the desktop’s brightness instead of the PC’s.", |ui, s| {
                    toggle(ui, &mut s.hdr_desktop_brightness)
                });
                rows.always();
            }

            rows.section(en("Display & image"));
            setting_row(ui, s, &mut rows, &["window_mode"], "Window", "Full screen, borderless or a window.", |ui, s| {
                Combo::from_id_salt("window")
                    .selected_text(window_name(s.window_mode))
                    .show_ui(ui, |ui| {
                        for mode in [WindowMode::Fullscreen, WindowMode::Borderless, WindowMode::Windowed] {
                            ui.selectable_value(&mut s.window_mode, mode, window_name(mode));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["video_scaling"], "Video size", "How the picture fits the screen: Fit, Fill, Stretch or 1:1 pixels.", |ui, s| {
                use crate::config::VideoScaling;
                Combo::from_id_salt("video-scaling").selected_text(tr(s.video_scaling.label())).show_ui(ui, |ui| {
                    for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                        ui.selectable_value(&mut s.video_scaling, mode, tr(mode.label()));
                    }
                }).response
            });
            rows.when(vrr_offered);
            setting_row(ui, s, &mut rows, &["vrr"], "Variable refresh rate", "Shows each frame as it arrives on a G-Sync, FreeSync or adaptive-refresh screen.", |ui, s| toggle(ui, &mut s.vrr));
            rows.always();
            let vrr = s.vrr && vrr_offered;
            rows.group(en("Refresh rate"));
            // With Sharper colored text packed on Windows, VRR shows each frame at once.
            let immediate_packed = cfg!(windows) && s.experimental_packed_444 && vrr_offered;
            rows.when(vrr && !immediate_packed);
            setting_row(ui, s, &mut rows, &["present_at_host_times"], "Show frames at the PC’s frame times", "Holds each frame briefly so it appears when the game drew it, evening out motion.", |ui, s| {
                toggle(ui, &mut s.present_at_host_times)
            });
            rows.always();
            rows.when(!vrr);
            setting_row(ui, s, &mut rows, &["frame_pacing"], "Frame pacing", "Balances response time against smoothness while VRR is off.", |ui, s| {
                Combo::from_id_salt("frame-pacing").selected_text(tr(s.frame_pacing.label())).show_ui(ui, |ui| {
                    for mode in crate::config::FramePacing::ALL {
                        ui.selectable_value(&mut s.frame_pacing, mode, tr(mode.label()));
                    }
                }).response
            });
            // Balanced's choice: the newest picture at each refresh, or each in turn (`Pacer::take_due`).
            // The other pacings decide that themselves, as on Android (`SettingRules.dropLateFramesApplies`).
            rows.when(s.frame_pacing == crate::config::FramePacing::Balanced);
            setting_row(ui, s, &mut rows, &["drop_late_frames"], "Keep only the newest frame", "Skips frames the screen can no longer show in time.", |ui, s| {
                toggle(ui, &mut s.drop_late_frames)
            });
            rows.always();
            rows.when(!vrr);
            // Still narrowed to VRR off, as Frame pacing; a pacing that waits for the refresh keeps it on.
            rows.when(!s.frame_pacing.forces_vsync());
            setting_row(ui, s, &mut rows, &["vsync"], "V-Sync", "Waits for the screen’s refresh, so the picture never tears.", |ui, s| toggle(ui, &mut s.vsync));
            rows.always();
            rows.group(en("Image"));
            setting_row(ui, s, &mut rows, &["upscaling"], "Upscaling", "How a stream smaller than the screen is enlarged.", |ui, s| {
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
            rows.when(s.upscaling.is_shader());
            setting_row(ui, s, &mut rows, &["upscale_sharpness"], "Upscaling sharpness", "How much sharpening the upscaler adds.", |ui, s| {
                slider(ui, &mut s.upscale_sharpness, 0..=100, 5.0, percent, |rail| rail, |field| field.suffix("%"))
            });
            rows.always();
            rows.when(matches!(s.video_scaling, crate::config::VideoScaling::Fit | crate::config::VideoScaling::Native));
            setting_row(ui, s, &mut rows, &["align_video_top"], "Video position", "Where a Fit or 1:1 picture sits: center or top.", |ui, s| {
                Combo::from_id_salt("video-position").selected_text(video_position_name(s.align_video_top)).show_ui(ui, |ui| {
                    for top in [false, true] {
                        ui.selectable_value(&mut s.align_video_top, top, video_position_name(top));
                    }
                }).response
            });
            rows.always();
            rows.group(en("PC display"));
            setting_row(ui, s, &mut rows, &["virtual_display"], "Virtual display", "The PC streams from a display of its own that matches the stream.", |ui, s| toggle(ui, &mut s.virtual_display));
            rows.when(s.virtual_display);
            setting_row(ui, s, &mut rows, &["virtual_display_scale"], "PC render resolution", "The size the PC renders at, as a percent of the stream resolution.", |ui, s| {
                slider(ui, &mut s.virtual_display_scale, crate::config::RENDER_SCALE, 5.0, percent, |rail| rail.step_by(5.0), |field| field.suffix("%"))
            });
            rows.always();
            // A PC's own settings offer it only if its host reads rlDisplayScale (DisplayScale).
            let test_host = if let Screen::HostSettings(id) = screen { view.hosts.get(id) } else { None };
            rows.when(display_scale_gate(test_host.and_then(|h| h.info.as_ref())
                .map_or(nightjar_core::http::Availability::Unknown, |i| i.capabilities.availability(nightjar_core::http::Feature::DisplayScale))));
            setting_row(ui, s, &mut rows, &["virtual_display_dpi"], "PC display scaling", "Text and interface size on the PC’s virtual display.", |ui, s| {
                Combo::from_id_salt("virtual-display-dpi")
                    .selected_text(display_scale_name(s.virtual_display_dpi))
                    .show_ui(ui, |ui| {
                        for percent in crate::config::DISPLAY_SCALES {
                            ui.selectable_value(&mut s.virtual_display_dpi, percent, display_scale_name(percent));
                        }
                    }).response
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["host_display"], "Display arrangement", "How the PC places the streamed display beside its own screens.", |ui, s| {
                Combo::from_id_salt("host-display")
                    .selected_text(tr(host_display_name(s.host_display)))
                    .show_ui(ui, |ui| {
                        for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary,
                            HostDisplay::Extended, HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
                            ui.selectable_value(&mut s.host_display, mode, tr(host_display_name(mode)));
                        }
                    }).response
            });

            rows.section(en("Controls"));
            setting_row(ui, s, &mut rows, &["mouse_mode"], "Mouse mode", "Captured keeps the pointer in the stream; Desktop pointer moves like an ordinary mouse.", |ui, s| {
                Combo::from_id_salt("mouse")
                    .selected_text(mouse_name(s.mouse_mode))
                    .show_ui(ui, |ui| {
                        for mode in [MouseMode::Capture, MouseMode::Absolute] {
                            ui.selectable_value(&mut s.mouse_mode, mode, mouse_name(mode));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["controllers", "multi_controller"], "Controllers", "Off, every controller as one player, or each controller its own player.", |ui, s| {
                let current = Controllers::from_saved(s.controllers, s.multi_controller);
                let mut choice = current;
                let response = Combo::from_id_salt("controllers").selected_text(tr(controllers_name(current))).show_ui(ui, |ui| {
                    for each in Controllers::ALL {
                        ui.selectable_value(&mut choice, each, tr(controllers_name(each)));
                    }
                }).response;
                if choice != current {
                    (s.controllers, s.multi_controller) = choice.to_saved(s.multi_controller);
                }
                response
            });
            if !layered {
                rows.action("controller_buttons");
                setting_row(ui, s, &mut rows, &[], "Controller buttons", "Remaps a controller model’s buttons and sticks.", |ui, _| {
                    let button = quiet_button(ui, "Controller buttons");
                    if button.clicked() { open_controllers = Some(false); }
                    links.push(("controllers-link", button.clone()));
                    button
                });
                rows.action("controller_test");
                setting_row(ui, s, &mut rows, &[], "Controller test", "Shows the buttons, sticks and triggers a controller sends.", |ui, _| {
                    let button = quiet_button(ui, "Test");
                    if button.clicked() { open_controllers = Some(true); }
                    links.push(("controller-test-link", button.clone()));
                    button
                });
            }
            setting_row(ui, s, &mut rows, &["clipboard_sync"], "Clipboard sync", "Copied text goes between this device and the PC.", |ui, s| {
                toggle(ui, &mut s.clipboard_sync)
            });
            rows.group(en("Controllers"));
            rows.when(s.controllers);
            setting_row(ui, s, &mut rows, &["swap_face_buttons"], "Nintendo button layout", "Swaps A with B and X with Y.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
            setting_row(ui, s, &mut rows, &["gamepad_quit_combo"], "Buttons to leave the stream", "The buttons held together to leave the stream.", |ui, s| {
                Combo::from_id_salt("gamepad-quit-combo").selected_text(style.quit_combo(s.gamepad_quit_combo)).show_ui(ui, |ui| {
                    for combo in crate::config::GamepadQuitCombo::ALL {
                        ui.selectable_value(&mut s.gamepad_quit_combo, combo, style.quit_combo(combo));
                    }
                }).response
            });
            // For every controller whose buttons page sets none of its own.
            setting_row(ui, s, &mut rows, &["deadzone"], "Stick dead zone", "Ignores small stick movements near the center.", |ui, s| {
                slider(ui, &mut s.deadzone, crate::config::DEADZONE, 1.0, percent, |rail| rail, |field| field.suffix("%"))
            });
            setting_row(ui, s, &mut rows, &["trigger_deadzone"], "Trigger dead zone", "Ignores light trigger presses.", |ui, s| {
                slider(ui, &mut s.trigger_deadzone, 0..=u32::from(crate::gamepad::MAX_TRIGGER_DEADZONE), 1.0, percent, |rail| rail, |field| field.suffix("%"))
            });
            setting_row(ui, s, &mut rows, &["controller_motion"], "Motion controls", "Sends the controller’s motion sensors to games.", |ui, s| toggle(ui, &mut s.controller_motion));
            setting_row(ui, s, &mut rows, &["disable_rumble"], "Turn off vibration", "Stops all rumble, trigger rumble and haptics.", |ui, s| toggle(ui, &mut s.disable_rumble));
            setting_row(ui, s, &mut rows, &["controller_mouse"], "Controller mouse mode", "Holding Start switches the controller between gamepad and mouse.", |ui, s| toggle(ui, &mut s.controller_mouse));
            rows.when(s.controller_mouse);
            setting_row(ui, s, &mut rows, &["analog_scrolling"], "Stick for scrolling", "The stick that scrolls in controller mouse mode.", |ui, s| {
                use crate::config::AnalogScrolling;
                Combo::from_id_salt("analog-scrolling").selected_text(scrolling_name(s.analog_scrolling)).show_ui(ui, |ui| {
                    for stick in [AnalogScrolling::Off, AnalogScrolling::Left, AnalogScrolling::Right] {
                        ui.selectable_value(&mut s.analog_scrolling, stick, scrolling_name(stick));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["mouse_fixed_step"], "Precise controller mouse", "Moves the pointer in fixed pixel steps while X is held.", |ui, s| toggle(ui, &mut s.mouse_fixed_step));
            rows.always();
            rows.when(s.controllers);
            setting_row(ui, s, &mut rows, &["gamepad_touchpad_as_mouse"], "Controller touchpad as mouse", "The controller’s touchpad moves the PC’s pointer.", |ui, s| toggle(ui, &mut s.gamepad_touchpad_as_mouse));
            setting_row(ui, s, &mut rows, &["background_controllers"], "Input in the background", "Controllers keep working while another window is active.", |ui, s| {
                toggle(ui, &mut s.background_controllers)
            });
            rows.always();
            // How the stream menu opens and what it holds, as Android's Stream menu group. Android's
            // floating menu button has no desktop row: a window has a pointer and keys to open the menu.
            rows.group(en("Stream menu"));
            rows.when(s.controllers);
            setting_row(ui, s, &mut rows, &["guide_hold_ms"], "Hold Guide to open the stream menu", "How long Guide is held to open the menu; a short press goes to the PC.", |ui, s| {
                Combo::from_id_salt("guide-hold").selected_text(guide_hold_name(s.guide_hold_ms)).show_ui(ui, |ui| {
                    for ms in crate::config::GUIDE_HOLD_MS {
                        ui.selectable_value(&mut s.guide_hold_ms, ms, guide_hold_name(ms));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["back_opens_menu"], "Back opens the stream menu", "The controller’s or handheld’s Back button opens the stream menu.", |ui, s| toggle(ui, &mut s.back_opens_menu));
            setting_row(ui, s, &mut rows, &["back_as_guide"], "Back button as Guide", "Back sends Guide (Home or PS) to the PC.", |ui, s| toggle(ui, &mut s.back_as_guide));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "Y in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::Y));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "X in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::X));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "LB in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::Lb));
            setting_row(ui, s, &mut rows, &["stream_menu_buttons"], "RB in the stream menu", "What this button runs while the stream menu is open.", |ui, s| stream_menu_button(ui, s, Hotkey::Rb));
            rows.always();
            if !layered {
                rows.action("keyboard_shortcuts");
                setting_row(ui, s, &mut rows, &[], "Keyboard shortcuts", "Key combinations sent to the PC with one press.", |ui, _| {
                    let shortcuts = quiet_button(ui, "Change");
                    if shortcuts.clicked() { commands.push(Command::Shortcuts); }
                    links.push(("shortcuts-link", shortcuts.clone()));
                    shortcuts
                });
            }
            rows.group(en("Touch & mouse"));
            setting_row(ui, s, &mut rows, &["mouse_speed"], "Mouse speed", "How fast the pointer moves.", |ui, s| slider(ui, &mut s.mouse_speed, 25..=400, 5.0, percent, |rail| rail, |field| field.suffix("%")));
            setting_row(ui, s, &mut rows, &["mouse_nav_buttons"], "Mouse back/forward buttons", "Sends the mouse’s Back and Forward buttons to the PC.", |ui, s| toggle(ui, &mut s.mouse_nav_buttons));
            if cfg!(windows) {
                setting_row(ui, s, &mut rows, &["native_touchpad"], "Touchpad gestures on the PC", "Sends touchpad fingers to a PC that takes them, so its gestures work there.", |ui, s| toggle(ui, &mut s.native_touchpad));
            }
            rows.group(en("Keyboard"));
            if !layered {
                rows.action("stream_hotkeys");
                setting_row(ui, s, &mut rows, &[], "Stream hotkeys", "Nightjar’s own key combinations in a stream.", |ui, _| {
                    let hotkeys = quiet_button(ui, "Change");
                    if hotkeys.clicked() { commands.push(Command::StreamHotkeys); }
                    links.push(("stream-hotkeys-link", hotkeys.clone()));
                    hotkeys
                });
            }
            setting_row(ui, s, &mut rows, &["capture_system_keys"], "Send system keys", "Alt+Tab, the Windows key and similar go to the PC.", |ui, s| toggle(ui, &mut s.capture_system_keys));
            rows.when(s.capture_system_keys);
            setting_row(ui, s, &mut rows, &["system_keys_fullscreen_only"], "System keys only in full screen", "In a window, system keys stay with this device.", |ui, s| {
                toggle(ui, &mut s.system_keys_fullscreen_only)
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["right_alt_as_meta"], "Right Alt as Windows key", "Right Alt sends the Windows key, which a Mac uses as Command.", |ui, s| toggle(ui, &mut s.right_alt_as_meta));
            rows.group(en("USB"));
            rows.when(s.controllers);
            setting_row(ui, s, &mut rows, &["controller_audio"], "USB DualSense audio and haptics", "A wired DualSense’s haptics, speaker, headset and microphone work with the PC.", |ui, s| {
                toggle(ui, &mut s.controller_audio)
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["usb_forwarding"], "Forward USB devices", "USB devices plugged in here can be attached to the PC from the stream menu.", |ui, s| {
                toggle(ui, &mut s.usb_forwarding)
            });

            rows.section(en("Audio"));
            setting_row(ui, s, &mut rows, &["audio"], "Channels", "Stereo, or 5.1, 7.1 or 7.1.4 surround.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["play_audio_on_host"], "Play audio on the PC too", "Keeps sound on the PC’s speakers while streaming.", |ui, s| {
                toggle(ui, &mut s.play_audio_on_host)
            });
            setting_row(ui, s, &mut rows, &["microphone"], "Send microphone to the PC", "Sends this device’s microphone to the PC.", |ui, s| {
                toggle(ui, &mut s.microphone)
            });
            rows.when(s.microphone);
            setting_row(ui, s, &mut rows, &["mic_echo_cancel"], "Echo cancellation", "Keeps the stream’s sound from the speakers out of the microphone.", |ui, s| {
                Combo::from_id_salt("mic-echo-cancel").selected_text(tr(s.mic_echo_cancel.label())).show_ui(ui, |ui| {
                    for mode in crate::config::MicEcho::ALL {
                        ui.selectable_value(&mut s.mic_echo_cancel, mode, tr(mode.label()));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["mic_noise_suppression"], "Noise suppression", "Filters steady background noise out of the microphone.", |ui, s| {
                toggle(ui, &mut s.mic_noise_suppression)
            });
            setting_row(ui, s, &mut rows, &["mic_auto_gain"], "Automatic gain", "Evens out quiet and loud speech.", |ui, s| {
                toggle(ui, &mut s.mic_auto_gain)
            });
            setting_row(ui, s, &mut rows, &["mic_gain"], "Microphone gain", "Makes the microphone quieter or louder before it is sent.", |ui, s| {
                let step = f64::from(crate::config::MIC_GAIN_STEP);
                slider(ui, &mut s.mic_gain, crate::config::MIC_GAIN, step, percent, |rail| rail.step_by(step), |field| field.suffix("%"))
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["mute_audio"], "Mute audio", "Plays no stream sound on this device.", |ui, s| toggle(ui, &mut s.mute_audio));
            setting_row(ui, s, &mut rows, &["audio_passthrough"], "Audio passthrough", "The PC sends uncompressed sound that the output plays as is.", |ui, s| {
                toggle(ui, &mut s.audio_passthrough)
            });

            rows.section(en("Streaming"));
            setting_row(ui, s, &mut rows, &["overlay", "overlay_mode"], "Performance overlay", "Shows how the stream is doing: Compact, Statistics or Debug.", |ui, s| {
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
            rows.when(s.overlay);
            setting_row(ui, s, &mut rows, &["overlay_position"], "Overlay position", "The corner the performance overlay sits in.", |ui, s| {
                Combo::from_id_salt("overlay-position").selected_text(tr(s.overlay_position.label())).show_ui(ui, |ui| {
                    for corner in crate::config::OverlayCorner::ALL {
                        ui.selectable_value(&mut s.overlay_position, corner, tr(corner.label()));
                    }
                }).response
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["auto_reconnect"], "Recover interrupted streams", "Reconnects to the same game after a dropped connection, up to five times a minute.", |ui, s| {
                toggle(ui, &mut s.auto_reconnect)
            });
            // A PC's own page tests that PC; the main settings test the paired one chosen. Every
            // paired PC has ping, jitter and loss; the speed where it has a test (`network_test`).
            if let Some(pc) = network_test_pc(&connection_pcs, *test_pc) {
                rows.action("connection_test");
                rows.room(if connection_pcs.len() > 1 { TEST_PC_W } else { 0.0 });
                setting_row(ui, s, &mut rows, &[], "Connection test", "Measures ping, jitter and loss, and download speed where the PC supports it.", |ui, _| {
                    let action = if pc.reach == Reach::Online {
                        let button = quiet_button(ui, "Test");
                        if button.clicked() { commands.push(Command::NetworkTest { host: pc.id, calibrate: false }); }
                        button
                    } else {
                        offline(ui)
                    };
                    if connection_pcs.len() > 1 { test_pc_choice(ui, "speed-test-pc", &connection_pcs, pc, test_pc); }
                    action
                });
            }
            rows.group(en("Connection"));
            setting_row(ui, s, &mut rows, &["client_roaming"], "Keep streaming when the network changes", "The stream carries on when this device switches networks.", |ui, s| {
                toggle(ui, &mut s.client_roaming)
            });
            rows.when(s.client_roaming);
            setting_row(ui, s, &mut rows, &["roaming_metered"], "Continue on mobile data", "Lets the stream move onto mobile data or another metered network.", |ui, s| {
                toggle(ui, &mut s.roaming_metered)
            });
            rows.always();
            setting_row(ui, s, &mut rows, &["network_grace_ms"], "Wait through network dropouts", "How long a silent network is waited out before the stream ends.", |ui, s| {
                use nightjar_policy::recovery::{network_grace_choice_s, NETWORK_GRACE_CHOICES_S};
                let current = network_grace_choice_s(s.network_grace_ms);
                // Off keeps a value that is already off; another choice is off at the default.
                let off_ms = if current == 0 { s.network_grace_ms } else { Settings::default().network_grace_ms };
                Combo::from_id_salt("network-grace").selected_text(network_grace_name(current)).show_ui(ui, |ui| {
                    for seconds in NETWORK_GRACE_CHOICES_S {
                        if ui.selectable_label(current == seconds, network_grace_name(seconds)).clicked() {
                            s.network_grace_ms = if seconds == 0 { off_ms } else { seconds * 1000 };
                        }
                    }
                }).response
            });
            // Receive coalescing on Windows, UDP_GRO on Linux; macOS's engine does neither.
            if cfg!(any(target_os = "linux", windows)) {
                setting_row(ui, s, &mut rows, &["receive_coalescing"], "Group incoming video packets", "Takes several video packets from the network at once. Turn it off if the picture turns blocky or black.", |ui, s| {
                    toggle(ui, &mut s.receive_coalescing)
                });
            }
            setting_row(ui, s, &mut rows, &["hide_connection_warnings"], "Hide connection warnings", "No warnings about a poor connection during streams.", |ui, s| {
                toggle(ui, &mut s.hide_connection_warnings)
            });
            rows.group("");
            setting_row(ui, s, &mut rows, &["dual_screen"], "Dual screen mode", "A second stream of the same game on a second screen.", |ui, s| {
                toggle(ui, &mut s.dual_screen)
            });
            rows.when(s.dual_screen);
            let own_size = tr("Second screen’s own size");
            setting_row(ui, s, &mut rows, &["second_screen_width", "second_screen_height"], "Second screen resolution", "The resolution of the second screen’s stream.", |ui, s| {
                let shown = if s.second_screen_width == 0 { own_size.to_owned() } else { format!("{}x{}", s.second_screen_width, s.second_screen_height) };
                Combo::from_id_salt("second-screen-resolution").selected_text(shown).show_ui(ui, |ui| {
                    let mut v = (s.second_screen_width, s.second_screen_height);
                    ui.selectable_value(&mut v, (0, 0), own_size);
                    for (w, h) in SECOND_SCREEN_RESOLUTIONS {
                        ui.selectable_value(&mut v, (w, h), format!("{w}x{h}"));
                    }
                    (s.second_screen_width, s.second_screen_height) = v;
                }).response
            });
            rows.always();
            // A game's menu reads it from the main settings.
            if !layered {
                setting_row(ui, s, &mut rows, &["input_only_any_host"], "Input-only on any PC", "Offers Input-only in a game’s menu on PCs without their own Input-only mode.", |ui, s| {
                    toggle(ui, &mut s.input_only_any_host)
                });
            }

            if !layered {
                rows.section(en("App"));
                setting_row(ui, s, &mut rows, &["theme"], "Theme", "System, Dark or OLED black.", |ui, s| {
                    Combo::from_id_salt("theme").selected_text(tr(s.theme.label())).show_ui(ui, |ui| {
                        for theme in crate::config::Theme::ALL {
                            ui.selectable_value(&mut s.theme, theme, tr(theme.label()));
                        }
                    }).response
                });
                setting_row(ui, s, &mut rows, &["language"], "Language", "The language of Nightjar’s menus.", |ui, s| {
                    Combo::from_id_salt("language")
                        .selected_text(drawable_language_name(ui, s.language))
                        .show_ui(ui, |ui| {
                            for language in Language::ALL {
                                ui.selectable_value(&mut s.language, language, drawable_language_name(ui, language));
                            }
                        })
                        .response
                });
                setting_row(ui, s, &mut rows, &["open_on_start", "console_home"], "When Nightjar opens", "Home, your PCs, or a short countdown into a game.", |ui, s| {
                    let current = start_screen(s);
                    let mut choice = current;
                    let name = |screen: StartScreen| start_screen_name(screen, open_game.as_deref());
                    let response = Combo::from_id_salt("open-on-start").selected_text(name(current)).show_ui(ui, |ui| {
                        // A chosen game is on the list once a game was chosen.
                        for each in StartScreen::ALL.into_iter().filter(|each| *each != StartScreen::Chosen || open_game.is_some() || current == StartScreen::Chosen) {
                            ui.selectable_value(&mut choice, each, name(each));
                        }
                    }).response;
                    if choice != current {
                        set_start_screen(s, choice);
                    }
                    response
                });
                setting_row(ui, s, &mut rows, &["quick_resume"], "Quick resume", "Selecting a PC that runs a game goes straight back into it.", |ui, s| {
                    toggle(ui, &mut s.quick_resume)
                });
                rows.action("pin_lock");
                setting_row(ui, s, &mut rows, &[], "PIN lock", "Asks for a PIN to open Nightjar or to start a stream.", |ui, _| {
                    let set = self.pin.as_ref().is_some_and(|p| p.store.has_pin());
                    let button = quiet_button(ui, if set { en("Change PIN lock") } else { en("Set up") });
                    if button.clicked() {
                        if let Some(pin) = self.pin.as_mut() { pin.open_settings(); }
                    }
                    button
                });
                rows.action("help");
                setting_row(ui, s, &mut rows, &[], "Help", "Setup, pairing and common fixes.", |ui, _| {
                    let button = quiet_button(ui, "Open help");
                    open_help = button.clicked();
                    button
                });
                rows.action("report_problem");
                setting_row(ui, s, &mut rows, &[], "Report a problem", "Saves a report with personal details removed.", |ui, _| {
                    let button = quiet_button(ui, "Save report");
                    if button.clicked() { commands.push(Command::ReportProblem); }
                    button
                });
                if crate::self_update::current().checks() {
                    rows.action("check_updates_now");
                    setting_row(ui, s, &mut rows, &[], "Check for updates now", "Looks for a newer version now.", |ui, _| {
                        let button = quiet_button(ui, "Check now");
                        if button.clicked() { commands.push(Command::CheckUpdatesNow); }
                        button
                    });
                    setting_row(ui, s, &mut rows, &["check_updates"], "Check for updates automatically", "Looks for a newer version when Nightjar opens; nothing installs by itself.", |ui, s| {
                        toggle(ui, &mut s.check_updates)
                    });
                }
                if let Some(on) = self.sign_in {
                    rows.action("sign_in");
                    setting_row(ui, s, &mut rows, &[], "Start Nightjar when I sign in", "Nightjar opens by itself after you sign in.", |ui, _| {
                        let mut wanted = on;
                        let response = toggle(ui, &mut wanted);
                        if wanted != on {
                            commands.push(Command::SetSignIn(wanted));
                        }
                        response
                    });
                }
                // Home's card size, which a PC's library and All games follow too: Small or Medium.
                setting_row(ui, s, &mut rows, &["home_card_size"], "Use small box art", "Fits more games on screen in a PC’s library.", |ui, s| {
                    let mut small = s.home_card_size == crate::config::CardSize::Small;
                    let response = toggle(ui, &mut small);
                    if small != (s.home_card_size == crate::config::CardSize::Small) {
                        s.home_card_size = if small { crate::config::CardSize::Small } else { crate::config::CardSize::Medium };
                    }
                    response
                });
                rows.action("about_app");
                setting_row(ui, s, &mut rows, &[], "About", "Version, licences and credits.", |ui, _| {
                    let button = quiet_button(ui, "Open");
                    if button.clicked() { open_page = Some(Screen::About); }
                    links.push(("about-link", button.clone()));
                    button
                });
                rows.action("logs");
                setting_row(ui, s, &mut rows, &[], "Logs", "Opens the folder with Nightjar’s logs.", |ui, _| {
                    let button = quiet_button(ui, "Open logs folder");
                    if button.clicked() { commands.push(Command::OpenLogs); }
                    button
                });
                let reset_label = if *confirm_reset {
                    rows.note(tr("Every setting on this page goes back to its default. Your language, PCs, games, profiles, PIN lock, controller buttons, shortcuts and anything not on this page stay as they are. This can’t be undone.").to_owned(), *TERTIARY);
                    en("Reset all settings?")
                } else {
                    en("Reset all settings")
                };
                rows.action("reset_all");
                setting_row(ui, s, &mut rows, &[], reset_label, "Puts every setting back to its default; PCs, games, profiles and language stay.", |ui, _| {
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
                rows.group(en("Accessibility"));
                setting_row(ui, s, &mut rows, &["ui_zoom"], "Text and button size", "Makes Nightjar’s menus bigger or smaller; the stream is not scaled.", |ui, s| {
                    Combo::from_id_salt("ui-zoom")
                        .selected_text(format!("{}%", crate::config::nearest_zoom(s.ui_zoom)))
                        .show_ui(ui, |ui| {
                            for step in UI_ZOOM_STEPS {
                                ui.selectable_value(&mut s.ui_zoom, step, format!("{step}%"));
                            }
                        })
                        .response
                });
                setting_row(ui, s, &mut rows, &["high_contrast"], "High contrast", "Black surfaces, stronger text and thicker focus rings.", |ui, s| {
                    toggle(ui, &mut s.high_contrast)
                });
                setting_row(ui, s, &mut rows, &["reduce_motion"], "Reduce motion", "Turns off fades, slides and zooms; Follow system uses the system’s setting.", |ui, s| {
                    Combo::from_id_salt("reduce-motion")
                        .selected_text(reduce_motion_name(s.reduce_motion))
                        .show_ui(ui, |ui| {
                            for mode in [ReduceMotion::System, ReduceMotion::Reduce, ReduceMotion::Full] {
                                ui.selectable_value(&mut s.reduce_motion, mode, reduce_motion_name(mode));
                            }
                        })
                        .response
                });
            }

            rows.section(en("Advanced"));
            // Windows PCs with two GPUs, for the whole app, not a PC or a profile.
            #[cfg(windows)]
            if let Some(gpus) = crate::gpu_adapters::gpus().filter(|_| !layered) {
                setting_row(ui, s, &mut rows, &["graphics_card"], "Graphics card", "The GPU that draws and decodes the stream.", |ui, s| {
                    use crate::config::GraphicsCard;
                    let name = |choice: GraphicsCard| match choice {
                        GraphicsCard::Automatic => tr(choice.label()).to_owned(),
                        GraphicsCard::PowerSaving => format!("{} ({})", tr(choice.label()), gpus.power_saving.name),
                        GraphicsCard::HighPerformance => format!("{} ({})", tr(choice.label()), gpus.high_performance.name),
                    };
                    Combo::from_id_salt("graphics_card")
                        .selected_text(name(s.graphics_card))
                        .show_ui(ui, |ui| {
                            for choice in GraphicsCard::ALL {
                                ui.selectable_value(&mut s.graphics_card, choice, name(choice));
                            }
                        })
                        .response
                });
            }
            // Linux only (Windows draws with Direct3D 11), and for the whole app, not a PC or a profile.
            if cfg!(target_os = "linux") && !layered {
                setting_row(ui, s, &mut rows, &["renderer"], "Renderer", "What draws Nightjar’s window: OpenGL or the GPU renderer.", |ui, s| {
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
            rows.group(en("Compatibility"));
            setting_row(ui, s, &mut rows, &["sops"], "Game optimization hint", "Lets the PC adjust the game’s settings for the stream.", |ui, s| toggle(ui, &mut s.sops));
            rows.group(en("Diagnostics"));
            setting_row(ui, s, &mut rows, &["link_probe"], "Test the link before starting", "A new stream at the automatic bitrate measures the connection first and starts no higher.", |ui, s| {
                toggle(ui, &mut s.link_probe)
            });
            // Where a PC measures it, or a pace was saved; a PC's own page also has it.
            if let Some(pc) = network_test_pc(&pace_pcs, *test_pc) {
                let pace = pc.record.pace.as_ref();
                if let Some(pace) = pace {
                    rows.note(trf("{} Mbps · {}", &[&pace.pace_mbps, &if pace.qualified { tr("confirmed") } else { tr("best available") }]), *ON_SURFACE_VARIANT);
                }
                rows.action("pyrowave_pace");
                rows.room((if pace_pcs.len() > 1 { TEST_PC_W } else { 0.0 }) + (if pace.is_some() { 80.0 } else { 0.0 }));
                setting_row(ui, s, &mut rows, &[], "PyroWave frame pace", "Finds the fastest pace at which a PC’s PyroWave frames arrive whole.", |ui, _| {
                    let online = pc.reach == Reach::Online;
                    let mut action = None;
                    if online && pc.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe) {
                        let button = quiet_button(ui, "Calibrate");
                        if button.clicked() { commands.push(Command::NetworkTest { host: pc.id, calibrate: true }); }
                        action = Some(button);
                    } else if !online {
                        action = Some(offline(ui));
                    }
                    if pace.is_some() {
                        let clear = text_button(ui, "Clear");
                        if clear.clicked() { commands.push(Command::ClearPace(pc.id)); }
                        action.get_or_insert(clear);
                    }
                    if pace_pcs.len() > 1 { test_pc_choice(ui, "pace-pc", &pace_pcs, pc, test_pc); }
                    action.unwrap_or_else(|| offline(ui))
                });
            }
            if !layered {
                setting_row(ui, s, &mut rows, &["post_stream_decode_time"], "Show decode time after streaming", "Shows the average decode time when a stream ends.", |ui, s| {
                    toggle(ui, &mut s.post_stream_decode_time)
                });
                rows.action("latency_benchmark");
                setting_row(ui, s, &mut rows, &[], "Latency benchmark", "Tests this device’s decode and display timing without a PC.", |ui, _| {
                    let button = quiet_button(ui, "Open");
                    if button.clicked() { open_page = Some(Screen::LatencyBenchmark); }
                    links.push(("latency-benchmark-link", button.clone()));
                    button
                });
                rows.action("export_latency_csv");
                setting_row(ui, s, &mut rows, &[], "Export latency CSV", "Saves the latest latency measurements.", |ui, _| {
                    let button = quiet_button(ui, "Export");
                    if button.clicked() { commands.push(Command::ExportLatencyCsv); }
                    button
                });
            }

            rows.section(en("Debug"));
            // Android's main rows first, in its order.
            setting_row(ui, s, &mut rows, &["error_correction"], "Error correction", "Recovery data the PC adds, so lost packets don’t break up the picture.", |ui, s| {
                Combo::from_id_salt("error-correction")
                    .selected_text(error_correction_name(s.error_correction))
                    .show_ui(ui, |ui| {
                        let fixed = ErrorCorrection::PERCENTAGES.map(ErrorCorrection::Percent);
                        for mode in [ErrorCorrection::Host, ErrorCorrection::Auto].into_iter().chain(fixed) {
                            ui.selectable_value(&mut s.error_correction, mode, error_correction_name(mode));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["client_hint_bitrate"], "Lower bitrate when frames are lost", "Turns the bitrate down as soon as frames go missing.", |ui, s| {
                toggle(ui, &mut s.client_hint_bitrate)
            });
            setting_row(ui, s, &mut rows, &["tell_host_wireless"], "Tell the PC when on Wi-Fi", "The PC paces the stream for Wi-Fi from the start.", |ui, s| {
                toggle(ui, &mut s.tell_host_wireless)
            });
            setting_row(ui, s, &mut rows, &["tell_host_link_speed"], "Tell the PC the link speed", "The PC limits its sending rate to this device’s wired port speed.", |ui, s| {
                toggle(ui, &mut s.tell_host_link_speed)
            });
            // The engine sets socket and thread priorities on Linux and thread priorities on Windows.
            if cfg!(any(target_os = "linux", windows)) {
                setting_row(ui, s, &mut rows, &["network_priority"], "Prioritize stream traffic", "Gives the stream’s traffic and work priority on this device.", |ui, s| {
                    toggle(ui, &mut s.network_priority)
                });
            }
            setting_row(ui, s, &mut rows, &["unbatched_input"], "Instant mouse and pen input", "Sends mouse and pen movement at once instead of in batches.", |ui, s| {
                toggle(ui, &mut s.unbatched_input)
            });
            setting_row(ui, s, &mut rows, &["full_range"], "Full color range", "Asks for full-range video, for a PC and screen that both use it.", |ui, s| toggle(ui, &mut s.full_range));
            rows.group(en("PyroWave"));
            if let Some(reason) = &gpu_decoder_error {
                rows.note(trf("GPU decoding isn’t available here: {}", &[reason]), *TERTIARY);
            }
            setting_row(ui, s, &mut rows, &["pyrowave_decoder"], "PyroWave decoder", "Decodes PyroWave on the graphics card or the processor.", |ui, s| {
                Combo::from_id_salt("pyrowave-decoder")
                    .selected_text(tr(s.pyrowave_decoder.label()))
                    .show_ui(ui, |ui| {
                        for mode in [crate::config::PyroWaveDecoder::Cpu, crate::config::PyroWaveDecoder::Gpu] {
                            ui.selectable_value(&mut s.pyrowave_decoder, mode, tr(mode.label()));
                        }
                    })
                    .response
            });
            rows.when(s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu);
            setting_row(ui, s, &mut rows, &["pyrowave_async_compute"], "Decode PyroWave on a compute queue", "Decodes on the GPU’s separate compute queue where it has one.", |ui, s| {
                toggle(ui, &mut s.pyrowave_async_compute)
            });
            rows.always();
            rows.group(en("Decoder"));
            rows.when(!s.codec.is_pyrowave());
            setting_row(ui, s, &mut rows, &["decoder"], "Video decoding", "Automatic decodes on the graphics card; Software on the processor.", |ui, s| {
                Combo::from_id_salt("decoder")
                    .selected_text(decoder_name(s.decoder))
                    .show_ui(ui, |ui| {
                        for decoder in [DecoderPreference::Auto, DecoderPreference::Software] {
                            ui.selectable_value(&mut s.decoder, decoder, decoder_name(decoder));
                        }
                    })
                    .response
            });
            rows.when(s.decoder == DecoderPreference::Software);
            setting_row(ui, s, &mut rows, &["software_slices"], "Slices for software decoding", "The PC splits each frame so the processor decodes parts in parallel.", |ui, s| {
                toggle(ui, &mut s.software_slices)
            });
            rows.always();
            // Vulkan video is Linux's.
            if cfg!(target_os = "linux") {
                setting_row(ui, s, &mut rows, &["vulkan_video_first"], "Vulkan video first", "Decodes with Vulkan video before VAAPI.", |ui, s| {
                    toggle(ui, &mut s.vulkan_video_first)
                });
                setting_row(ui, s, &mut rows, &["vulkan_rfi"], "Vulkan video recovery", "Repairs a lost frame with Vulkan video without waiting for a whole new one.", |ui, s| {
                    toggle(ui, &mut s.vulkan_rfi)
                });
            }
            rows.group("");
            setting_row(ui, s, &mut rows, &["cursor_shapes"], "Local cursor", "Draws the PC’s cursor here in Desktop pointer mode, without video delay.", |ui, s| {
                toggle(ui, &mut s.cursor_shapes)
            });
            rows.finish();

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
        // Switching it is how to ask a PC again that refused it before.
        if sharp_color(s) != sharp_color(&before) || s.experimental_packed_444 != before.experimental_packed_444 {
            commands.push(Command::AskFullChromaAgain);
        }
        if mobile_limit != self.profiles.mobile_limit_kbps {
            self.profiles.mobile_limit_kbps = mobile_limit;
            commands.push(Command::SaveProfiles);
        }
        if searching && rows.found != self.settings_view.found {
            ui.ctx().request_repaint();
        }
        if let Some(at) = rows.toggled {
            self.settings_view.more[at] = !self.settings_view.more[at];
        }
        self.settings_view.found = rows.found;
        self.settings_view.drawn = rows.drawn;
        // Still waiting when no row was drawn to take it.
        self.settings_view.focus_rows &= rows.focus_first;
        for (key, response) in links {
            self.focus_item(&response, key, false);
        }
        if let Some(test) = open_controllers {
            self.controller_test = test;
            commands.push(Command::ControllerSettings);
        }
        if open_help {
            commands.push(Command::SaveSettings);
            self.open(Screen::Help);
        }
        if let Some(page) = open_page {
            commands.push(Command::SaveSettings);
            self.open(page);
        }
    }
}

/// A preset's chip, as Android's `stream_preset_names` says it.
pub(super) fn preset_label(preset: Preset) -> &'static str {
    match preset {
        Preset::Native => en("Native (recommended)"),
        Preset::BatterySaver => en("Battery saver"),
    }
}

/// A preset as the row's one-line value says it: "Native (recommended)" reads as "Native", as on
/// Android (`PresetPreference`), where the chip keeps the tag.
pub(super) fn preset_value(preset: Preset) -> String {
    let name = tr(preset_label(preset));
    match name.rfind(" (") {
        Some(at) if name.ends_with(')') => name[..at].to_owned(),
        _ => name.to_owned(),
    }
}

fn preset_key(preset: Preset) -> &'static str {
    match preset {
        Preset::Native => "preset-native",
        Preset::BatterySaver => "preset-battery-saver",
    }
}

/// The settings a preset is compared with (`nightjar_policy::presets`). Resolution, bitrate, VRR
/// and upscaling read as the preset's at their defaults: Native, Automatic, VRR where the display
/// has it, bilinear. The latency switches are the three the desktop has of Android's.
pub(super) fn preset_current(s: &Settings, vrr_offered: bool) -> presets::Current {
    let defaults = Settings::default();
    presets::Current {
        native_resolution: s.width == 0 || s.height == 0,
        fps_follows_screen: s.fps == 0,
        fps: s.fps,
        default_bitrate: s.bitrate_kbps == 0,
        default_vrr: !vrr_offered || s.vrr == defaults.vrr,
        default_upscaling: s.upscaling == defaults.upscaling,
        pacing: match s.frame_pacing {
            FramePacing::Latency => Some(presets::Pacing::Latency),
            FramePacing::CapFps => Some(presets::Pacing::CapFps),
            FramePacing::Balanced | FramePacing::Smoothness => None,
        },
        latency_switches_on: s.drop_late_frames && s.unbatched_input && s.network_priority,
        power: None,
    }
}

/// What a preset sets, Android's `StreamPreset.apply` on the desktop's settings. The codec, HDR
/// and 4:4:4 choices stay as they are.
pub(super) fn apply_preset(s: &mut Settings, preset: Preset) {
    let defaults = Settings::default();
    (s.width, s.height) = (0, 0);
    s.fps = preset.fps().unwrap_or(0);
    s.bitrate_kbps = 0;
    s.vrr = defaults.vrr;
    s.upscaling = defaults.upscaling;
    s.frame_pacing = match preset.pacing() {
        presets::Pacing::Latency => FramePacing::Latency,
        presets::Pacing::CapFps => FramePacing::CapFps,
    };
    if preset.sets_latency_switches() {
        (s.drop_late_frames, s.unbatched_input, s.network_priority) = (true, true, true);
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

/// Whether a PC's own settings offer PC display scaling, as Android's per-PC "dpi" row: not on a
/// host whose capabilities say it doesn't read `rlDisplayScale`. The main settings and a PC not
/// reached yet offer it; the launch sends it only to hosts with `Feature::DisplayScale`.
pub(super) fn display_scale_gate(availability: nightjar_core::http::Availability) -> bool {
    use nightjar_core::http::Availability;
    matches!(availability, Availability::Supported | Availability::Unknown)
}

pub(super) fn display_scale_name(percent: u32) -> String {
    if percent == 0 { tr("PC’s own").into() } else { format!("{percent}%") }
}

pub(super) fn host_display_name(mode: HostDisplay) -> &'static str {
    match mode {
        HostDisplay::HostDefault => en("The PC’s own setting"),
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
        ErrorCorrection::Host => tr("PC’s setting").to_owned(),
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

/// "Controllers" as the list says each choice (English; the caller translates).
fn controllers_name(choice: Controllers) -> &'static str {
    match choice {
        Controllers::Off => en("Off"),
        Controllers::OnePlayer => en("One player"),
        Controllers::PlayerEach => en("A player each"),
    }
}

/// "Sharper colored text" is On while either of its two keys is on (`nightjar_core::sharp_color`).
pub(super) fn sharp_color(s: &Settings) -> bool {
    s.prefer_yuv444 || s.experimental_packed_444
}

fn sharp_color_name(on: bool) -> &'static str {
    if on { tr("On") } else { tr("Off") }
}

/// Video position: the picture at the window's centre or its top (`align_video_top`).
fn video_position_name(top: bool) -> &'static str {
    if top { tr("Top") } else { tr("Center") }
}

/// A Minimum resolution choice by its shorter side, as Android lists it: 720p.
fn short_edge_name(edge: u32) -> String {
    format!("{edge}p")
}

/// A Wait through network dropouts choice in seconds; 0 is Off.
fn network_grace_name(seconds: u32) -> String {
    if seconds == 0 { tr("Off").to_owned() } else { trf("{} s", &[&seconds]) }
}

fn mobile_bitrate_name(kbps: u32) -> String {
    if kbps == 0 { tr("Off").to_owned() } else { format!("{} Mbps", f64::from(kbps) / 1000.0) }
}

/// "When Nightjar opens" as the two keys it is kept in make it ([`StartScreen::from_saved`]).
pub(super) fn start_screen(s: &Settings) -> StartScreen {
    use crate::config::OpenOnStart;
    let open = match s.open_on_start {
        OpenOnStart::Pcs => "pcs",
        OpenOnStart::Last => "last",
        OpenOnStart::Chosen => "chosen",
    };
    StartScreen::from_saved(open, s.console_home)
}

/// Saves a "When Nightjar opens" choice into its two keys ([`StartScreen::to_saved`]).
pub(super) fn set_start_screen(s: &mut Settings, screen: StartScreen) {
    use crate::config::OpenOnStart;
    let (open, console_home) = screen.to_saved(s.console_home);
    s.open_on_start = match open {
        "last" => OpenOnStart::Last,
        "chosen" => OpenOnStart::Chosen,
        _ => OpenOnStart::Pcs,
    };
    s.console_home = console_home;
}

/// A "When Nightjar opens" choice as the list says it: A chosen game by the game's title once one
/// was chosen.
fn start_screen_name(screen: StartScreen, game: Option<&str>) -> String {
    use crate::config::OpenOnStart;
    match (screen, game) {
        (StartScreen::Home, _) => tr("Home").to_owned(),
        (StartScreen::Pcs, _) => tr(OpenOnStart::Pcs.label()).to_owned(),
        (StartScreen::Last, _) => tr(OpenOnStart::Last.label()).to_owned(),
        (StartScreen::Chosen, Some(title)) => title.to_owned(),
        (StartScreen::Chosen, None) => tr(OpenOnStart::Chosen.label()).to_owned(),
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
        "open_on_start" | "console_home" => start_screen_name(start_screen(s), None),
        "controllers" | "multi_controller" => tr(controllers_name(Controllers::from_saved(s.controllers, s.multi_controller))).to_owned(),
        "prefer_yuv444" | "experimental_packed_444" => sharp_color_name(sharp_color(s)).to_owned(),
        "align_video_top" => video_position_name(s.align_video_top).to_owned(),
        "width" | "height" if s.width == 0 => trf("Native ({}x{})", &[&display.width, &display.height]),
        "width" | "height" => format!("{}x{}", s.width, s.height),
        "fps" if s.fps == 0 => trf("Display ({} Hz)", &[&(display.refresh_hz.round() as u32)]),
        "fps" => trf("{} fps", &[&s.fps]),
        "bitrate_kbps" if s.bitrate_kbps == 0 => tr("Automatic").to_owned(),
        "bitrate_kbps" => format!("{} Mbps", f64::from(s.bitrate_kbps) / 1000.0),
        "adaptive_quality" => tr(s.adaptive_quality.label()).to_owned(),
        "quality_min_short_edge" => short_edge_name(s.quality_min_short_edge),
        "quality_min_fps" => trf("{} fps", &[&s.quality_min_fps]),
        "codec" => codec_name(s.codec).to_owned(),
        "upscaling" => tr(s.upscaling.label()).to_owned(),
        "upscale_sharpness" => format!("{}%", s.upscale_sharpness),
        "decoder" => decoder_name(s.decoder).to_owned(),
        "renderer" => tr(s.renderer.label()).to_owned(),
        "graphics_card" => tr(s.graphics_card.label()).to_owned(),
        "pyrowave_decoder" => tr(s.pyrowave_decoder.label()).to_owned(),
        "window_mode" => window_name(s.window_mode).to_owned(),
        "video_scaling" => tr(s.video_scaling.label()).to_owned(),
        "host_display" => tr(host_display_name(s.host_display)).to_owned(),
        "virtual_display_scale" => format!("{}%", s.virtual_display_scale),
        "virtual_display_dpi" => display_scale_name(s.virtual_display_dpi),
        "frame_pacing" => tr(s.frame_pacing.label()).to_owned(),
        "mouse_mode" => mouse_name(s.mouse_mode).to_owned(),
        "mouse_speed" => format!("{}%", s.mouse_speed),
        "deadzone" => format!("{}%", s.deadzone),
        "trigger_deadzone" => format!("{}%", s.trigger_deadzone),
        "gamepad_quit_combo" => style.quit_combo(s.gamepad_quit_combo),
        "stream_menu_buttons" if s.stream_menu_buttons.is_empty() => tr("Default").to_owned(),
        "stream_menu_buttons" => s.stream_menu_buttons.iter().map(|(key, id)| format!("{}: {}", key.cap(), stream_menu_button_name(Some(id)))).collect::<Vec<_>>().join(", "),
        "analog_scrolling" => scrolling_name(s.analog_scrolling).to_owned(),
        "guide_hold_ms" => guide_hold_name(s.guide_hold_ms),
        "audio" => audio_name(s.audio).to_owned(),
        "mic_echo_cancel" => tr(s.mic_echo_cancel.label()).to_owned(),
        "mic_gain" => format!("{}%", s.mic_gain),
        "overlay" | "overlay_mode" if s.overlay => tr(s.overlay_mode.label()).to_owned(),
        "overlay" | "overlay_mode" => tr("Off").to_owned(),
        "overlay_position" => tr(s.overlay_position.label()).to_owned(),
        "second_screen_width" | "second_screen_height" if s.second_screen_width == 0 => tr("Second screen’s own size").to_owned(),
        "second_screen_width" | "second_screen_height" => format!("{}x{}", s.second_screen_width, s.second_screen_height),
        "network_grace_ms" => network_grace_name(nightjar_policy::recovery::network_grace_choice_s(s.network_grace_ms)),
        "error_correction" => error_correction_name(s.error_correction),
        "language" => language_name(s.language).to_owned(),
        "ui_zoom" => format!("{}%", crate::config::nearest_zoom(s.ui_zoom)),
        "reduce_motion" => reduce_motion_name(s.reduce_motion).to_owned(),
        "theme" => tr(s.theme.label()).to_owned(),
        "home_card_size" => tr(s.home_card_size.label()).to_owned(),
        // The rest are switches.
        _ => match serde_json::to_value(s).ok().and_then(|v| v.get(key).and_then(serde_json::Value::as_bool)) {
            Some(true) => tr("On").to_owned(),
            Some(false) => tr("Off").to_owned(),
            None => String::new(),
        },
    }
}

/// The main rows of each section, as `client/app/src/main/res/values/settings_tiers.xml` lists them
/// by Android preference key, in `SETTINGS_SECTIONS` order. A test keeps the two equal.
pub(super) const MAIN_KEYS: [(&str, &[&str]); 8] = [
    ("video", &["list_resolution", "list_fps", "automatic_bitrate", "seekbar_bitrate_kbps", "checkbox_auto_bitrate", "list_mobile_bitrate_kbps", "video_format", "checkbox_enable_hdr"]),
    ("display", &["checkbox_stretch_video", "stream_orientation", "checkbox_vrr"]),
    ("input", &["checkbox_touchscreen_trackpad", "checkbox_absolute_mouse_mode", "controllers", "controller_button_mapping", "controller_test", "checkbox_show_onscreen_controls", "checkbox_keyboard_follows_text_fields", "checkbox_clipboard_sync"]),
    ("audio", &["list_audio_config", "checkbox_host_audio", "checkbox_enable_mic", "checkbox_mic_button"]),
    ("streaming", &["checkbox_enable_perf_overlay", "performance_overlay_position", "checkbox_auto_reconnect", "network_tests"]),
    ("app", &["checkbox_always_dark", "list_languages", "list_open_on_start", "checkbox_quick_resume", "pin_lock", "help", "report_problem", "check_updates_now"]),
    ("advanced", &["checkbox_rust_engine", "low_latency_decoding", "low_latency_customize", "gpu_driver", "checkbox_gpu_max_clocks", "checkbox_texture_view", "checkbox_vulkan_render"]),
    ("debug", &["list_fec_choice", "checkbox_rate_change_bitrate", "checkbox_tell_host_wireless", "checkbox_tell_host_link_speed", "checkbox_network_priority", "checkbox_unbatched_input", "checkbox_full_range"]),
];

/// Every desktop row on both platforms and the Android preference it is: a setting's first key or
/// an action row's name, then Android's key. A row is main when its match is in `MAIN_KEYS`.
pub(super) const ANDROID_KEYS: [(&str, &str); 102] = [
    // Video
    ("width", "list_resolution"), ("fps", "list_fps"), ("bitrate_kbps", "seekbar_bitrate_kbps"),
    ("adaptive_bitrate", "checkbox_auto_bitrate"), ("mobile_bitrate", "list_mobile_bitrate_kbps"), ("codec", "video_format"),
    ("hdr", "checkbox_enable_hdr"), ("prefer_yuv444", "sharp_color"), ("sdr_10bit", "checkbox_enable_sdr_10bit"),
    ("frame_warp", "checkbox_frame_warp"),
    ("adaptive_quality", "adaptive_quality_mode"), ("quality_min_short_edge", "adaptive_quality_min_resolution"),
    ("quality_min_fps", "adaptive_quality_min_fps"),
    // Display & image
    ("video_scaling", "checkbox_stretch_video"), ("vrr", "checkbox_vrr"), ("present_at_host_times", "checkbox_host_frame_times"),
    ("frame_pacing", "frame_pacing"), ("drop_late_frames", "checkbox_drop_late_frames"), ("upscaling", "upscaling_mode"),
    ("upscale_sharpness", "upscaling_sharpness"),
    ("align_video_top", "video_alignment"), ("virtual_display", "checkbox_virtual_display"),
    ("virtual_display_scale", "seekbar_virtual_display_scale"), ("virtual_display_dpi", "list_virtual_display_dpi"),
    ("host_display", "host_display_location"),
    // Controls
    ("mouse_mode", "checkbox_absolute_mouse_mode"), ("controllers", "controllers"), ("controller_buttons", "controller_button_mapping"),
    ("controller_test", "controller_test"), ("clipboard_sync", "checkbox_clipboard_sync"), ("swap_face_buttons", "checkbox_flip_face_buttons"),
    ("gamepad_quit_combo", "list_gamepad_quit_combo"), ("deadzone", "seekbar_deadzone"), ("trigger_deadzone", "seekbar_trigger_deadzone"),
    ("controller_motion", "checkbox_gamepad_motion_sensors"), ("disable_rumble", "checkbox_disable_rumble"),
    ("controller_mouse", "checkbox_mouse_emulation"), ("analog_scrolling", "analog_scrolling"),
    ("mouse_fixed_step", "checkbox_mouse_emulation_fixed_step"), ("gamepad_touchpad_as_mouse", "checkbox_gamepad_touchpad_as_mouse"),
    ("guide_hold_ms", "list_guide_menu_hold"), ("back_opens_menu", "checkbox_back_opens_menu"), ("back_as_guide", "checkbox_back_as_guide"),
    ("stream_menu_buttons", "list_stream_menu_button_y"), ("keyboard_shortcuts", "keyboard_shortcuts_editor"),
    ("mouse_speed", "seekbar_mouse_speed"), ("mouse_nav_buttons", "checkbox_mouse_nav_buttons"), ("stream_hotkeys", "stream_hotkeys"),
    ("capture_system_keys", "checkbox_capture_system_keys"), ("system_keys_fullscreen_only", "checkbox_system_keys_fullscreen_only"),
    ("right_alt_as_meta", "checkbox_right_alt_as_meta"), ("controller_audio", "checkbox_usb_dualsense_haptics"),
    ("usb_forwarding", "checkbox_usb_forwarding"),
    // Audio
    ("audio", "list_audio_config"), ("play_audio_on_host", "checkbox_host_audio"), ("microphone", "checkbox_enable_mic"),
    ("mic_echo_cancel", "list_mic_echo_cancel"), ("mic_noise_suppression", "checkbox_mic_noise_suppression"),
    ("mic_auto_gain", "checkbox_mic_auto_gain"), ("mic_gain", "seekbar_mic_gain"), ("mute_audio", "checkbox_mute_audio"),
    ("audio_passthrough", "checkbox_audio_passthrough"),
    // Streaming
    ("overlay", "checkbox_enable_perf_overlay"), ("overlay_position", "performance_overlay_position"),
    ("auto_reconnect", "checkbox_auto_reconnect"), ("connection_test", "network_tests"), ("client_roaming", "checkbox_client_roaming"),
    ("roaming_metered", "checkbox_roaming_metered"), ("network_grace_ms", "list_network_grace"),
    ("receive_coalescing", "checkbox_receive_coalescing"),
    ("hide_connection_warnings", "checkbox_disable_warnings"), ("dual_screen", "checkbox_dual_screen"),
    ("second_screen_width", "list_resolution_screen2"), ("input_only_any_host", "checkbox_input_only_any_host"),
    // App
    ("theme", "checkbox_always_dark"), ("language", "list_languages"), ("open_on_start", "list_open_on_start"),
    ("quick_resume", "checkbox_quick_resume"), ("pin_lock", "pin_lock"), ("help", "help"), ("report_problem", "report_problem"), ("check_updates_now", "check_updates_now"),
    ("check_updates", "checkbox_check_updates"), ("home_card_size", "checkbox_small_icon_mode"), ("about_app", "about_app"),
    ("reset_all", "reset_all"),
    ("ui_zoom", "list_ui_scale"), ("high_contrast", "checkbox_high_contrast"), ("reduce_motion", "list_reduce_motion"),
    // Advanced
    ("sops", "checkbox_enable_sops"), ("link_probe", "checkbox_link_probe"), ("pyrowave_pace", "pace_calibration"),
    ("post_stream_decode_time", "checkbox_enable_post_stream_toast"), ("latency_benchmark", "latency_benchmark"),
    ("export_latency_csv", "export_latency_csv"),
    // Debug
    ("error_correction", "list_fec_choice"), ("client_hint_bitrate", "checkbox_rate_change_bitrate"),
    ("tell_host_wireless", "checkbox_tell_host_wireless"), ("tell_host_link_speed", "checkbox_tell_host_link_speed"),
    ("network_priority", "checkbox_network_priority"), ("unbatched_input", "checkbox_unbatched_input"), ("full_range", "checkbox_full_range"),
];

/// Main rows Android has no match for: the desktop's window, and the graphics card and renderer
/// beside Android's own Advanced main rows.
pub(super) const DESKTOP_MAIN: [&str; 3] = ["window_mode", "graphics_card", "renderer"];

/// The action rows: they open a page or run a test and store nothing, so no setting key names them.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const ACTION_ROWS: [&str; 17] = ["mobile_bitrate", "controller_buttons", "controller_test", "keyboard_shortcuts", "stream_hotkeys",
    "connection_test", "pin_lock", "help", "report_problem", "check_updates_now", "sign_in", "about_app", "logs", "reset_all", "pyrowave_pace",
    "latency_benchmark", "export_latency_csv"];

/// Whether the row `id` (a setting's first key, an action's name) is one of its section's main rows.
pub(super) fn main_row(id: &str) -> bool {
    DESKTOP_MAIN.contains(&id)
        || ANDROID_KEYS.iter().any(|&(desktop, android)| desktop == id && MAIN_KEYS.iter().any(|(_, main)| main.contains(&android)))
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
    /// (true), a group's title (empty: a gap only).
    heading: Option<(bool, &'static str)>,
    /// Rows drawn, per section.
    found: [usize; 8],
    /// Whether the rows that follow are drawn: a setting they depend on or this device can rule
    /// them out (`when`).
    shown: bool,
    /// Each section's fold is open.
    more: [bool; 8],
    /// The section's main rows and folded rows its gates show so far, and whether its first
    /// folded row came.
    mains: usize,
    folded: usize,
    past_fold: bool,
    /// The open section's fold row, waiting for its count at the section's end.
    fold: Option<FoldRow>,
    /// The section whose fold row was clicked.
    toggled: Option<usize>,
    /// The first row drawn takes focus (a new page).
    focus_first: bool,
    /// The keys whose row was drawn: a second row of the same setting (Custom resolution,
    /// Bitrate) gets no Reset of its own.
    reset_drawn: Vec<&'static str>,
    defaults: Settings,
    display: Display,
    style: crate::glyphs::Style,
    /// The id of each row drawn, in order, with whether it sits behind its section's fold.
    drawn: Vec<(&'static str, bool)>,
    /// The next row's action name (an action row has no key), its note, and room its control
    /// takes beyond the usual.
    action: Option<&'static str>,
    note: Option<(String, Color32)>,
    room: f32,
}

/// The fold row of the open section, drawn before its first folded row; its words come once the
/// section's folded rows are counted.
struct FoldRow {
    painter: egui::Painter,
    words: egui::layers::ShapeIdx,
    rect: Rect,
    response: Response,
    open: bool,
}

impl Rows<'_> {
    fn searching(&self) -> bool {
        !self.words.is_empty()
    }

    /// Starts `title`'s rows.
    fn section(&mut self, title: &'static str) {
        self.finish();
        (self.section, self.group) = (title, "");
        self.heading = self.searching().then_some((true, title));
        (self.mains, self.folded, self.past_fold) = (0, 0, false);
        self.always();
    }

    /// Starts a group of rows inside the section, such as Refresh rate; an empty title parts the
    /// rows that follow from the group before without a heading.
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

    /// The rows that follow are drawn only while `shown`. Every call narrows; `always` lifts it.
    fn when(&mut self, shown: bool) {
        self.shown &= shown;
    }

    fn always(&mut self) {
        self.shown = true;
    }

    /// The next row is the action `id`.
    fn action(&mut self, id: &'static str) {
        self.action = Some(id);
    }

    /// A line the next row always shows under its label, such as a warning about its value.
    fn note(&mut self, text: String, color: Color32) {
        self.note = Some((text, color));
    }

    /// Room the next row's control takes beyond the usual column (a PC's list beside its button).
    fn room(&mut self, width: f32) {
        self.room = width;
    }

    /// Whether the row `id` is drawn; draws the fold and the headings waiting for it.
    fn shows(&mut self, ui: &mut Ui, id: &'static str, label: &str, description: &str) -> bool {
        if !self.shown {
            return false;
        }
        let at = section_index(self.section);
        let folded = !main_row(id);
        // A section without main rows here (Advanced on most computers) lists its rows straight away.
        let behind = folded && self.mains > 0;
        if folded { self.folded += 1 } else { self.mains += 1 }
        let browsing = !self.searching() && self.section == self.open;
        if behind && !self.past_fold && browsing {
            self.fold_row(ui, at);
            // The rows it opens start a group of their own, apart from the fold.
            if self.heading.is_none() {
                self.heading = Some((false, ""));
            }
        }
        self.past_fold |= folded;
        let visible = if self.searching() {
            found(&self.words, &[tr(label), tr(description), tr(self.section), if self.group.is_empty() { "" } else { tr(self.group) }])
        } else {
            browsing && !(behind && !self.more[at])
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
            } else if title.is_empty() {
                ui.add_space(space::XL);
            } else {
                group_heading(ui, title, space::XL);
            }
        }
        self.found[at] += 1;
        self.drawn.push((id, folded));
        true
    }

    /// The fold over the section's other rows, last in its first group: "More … settings (n)",
    /// or "Fewer … settings" while open. Its words wait for the count (`finish`).
    fn fold_row(&mut self, ui: &mut Ui, at: usize) {
        let open = self.more[at];
        let rect = group_row(ui, |ui| {
            ui.allocate_space(vec2(ui.available_width(), design_tokens::SIZE_ROW_HEIGHT - 16.0));
        });
        let spot = rect.shrink(4.0);
        let response = ui.interact(spot, Id::unique("settings-fold").with(self.section), Sense::click());
        let painter = ui.painter().clone();
        let mut shapes = Vec::new();
        lift_shapes(&mut shapes, ui.ctx(), spot, f32::from(radius::CONTROL), response.hovered(), response.is_pointer_button_down_on());
        painter.extend(shapes);
        let words = painter.add(egui::Shape::Noop);
        // A chevron that points where the rows go: down to open, up to close.
        let c = Pos2::new(rect.right() - 30.0, rect.center().y);
        let (tip, wing) = if open { (-3.5, 3.5) } else { (3.5, -3.5) };
        let stroke = Stroke::new(2.0, *ON_SURFACE_VARIANT);
        painter.add(egui::Shape::line(vec![c + vec2(-6.0, wing), c + vec2(0.0, tip), c + vec2(6.0, wing)], stroke));
        focus_ring(ui, spot, &response, radius::CONTROL);
        if response.clicked() {
            self.toggled = Some(at);
        }
        self.fold = Some(FoldRow { painter, words, rect, response, open });
    }

    /// Ends the section: its fold row says how many rows it holds.
    fn finish(&mut self) {
        let Some(fold) = self.fold.take() else { return };
        let name = tr(self.section);
        let words = if fold.open { trf("Fewer {} settings", &[&name]) } else { trf("More {} settings ({})", &[&name, &self.folded]) };
        let galley = fold.painter.layout_no_wrap(words.clone(), semibold(text::BODY), *MOON);
        let at = Pos2::new(fold.rect.left() + 18.0, fold.rect.center().y - galley.size().y / 2.0);
        fold.painter.set(fold.words, egui::Shape::galley(at, galley, *MOON));
        fold.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &words));
        fold.painter.ctx().accesskit_node_builder(fold.response.id, |node| node.set_expanded(fold.open));
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

/// The PCs a test can run on from `screen`: a PC's own page its PC, the main settings every paired
/// PC, each only where `can` says the test works.
pub(super) fn test_pcs(screen: Screen, hosts: &Hosts, can: impl Fn(&Host) -> bool) -> Vec<&Host> {
    match screen {
        Screen::HostSettings(id) => hosts.get(id).filter(|h| h.paired() && can(h)).into_iter().collect(),
        Screen::Settings => hosts.list.iter().filter(|h| h.paired() && can(h)).collect(),
        _ => Vec::new(),
    }
}

/// The PC of `pcs` a test runs on: the one chosen while it is still there, else the first that is
/// online, else the first.
pub(super) fn network_test_pc<'h>(pcs: &[&'h Host], chosen: Option<u64>) -> Option<&'h Host> {
    chosen.and_then(|id| pcs.iter().find(|h| h.id == id)).or_else(|| pcs.iter().find(|h| h.reach == Reach::Online)).or_else(|| pcs.first()).copied()
}

/// The list beside a test's button that chooses its PC among several; the choice holds for both
/// tests until Nightjar closes.
fn test_pc_choice(ui: &mut Ui, salt: &str, pcs: &[&Host], pc: &Host, chosen: &mut Option<u64>) {
    let mut pick = pc.id;
    let response = Combo::from_id_salt(salt).width(TEST_PC_W - space::S).selected_text(pc.display_name()).show_ui(ui, |ui| {
        for each in pcs {
            ui.selectable_value(&mut pick, each.id, each.display_name());
        }
    }).response;
    name_for_readers(ui, &response, en("PC"), en("The paired PC these tests use."));
    if pick != pc.id {
        *chosen = Some(pick);
    }
}

/// What a test's row shows for a PC that is not on the network: its state, no action.
fn offline(ui: &mut Ui) -> Response {
    ui.label(RichText::new(tr("Offline")).font(regular(text::BODY)).color(ON_SURFACE_VARIANT))
}

/// A setting's row, or None while it is not shown (another section, a closed fold, a setting it
/// depends on, or the search passes it). `keys` empty: an action row (`Rows::action`), with no
/// default and no layer box.
pub(super) fn setting_row(
    ui: &mut Ui,
    settings: &mut Settings,
    rows: &mut Rows<'_>,
    keys: &[&'static str],
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, &mut Settings) -> Response,
) -> Option<Response> {
    let (action, note, room) = (rows.action.take(), rows.note.take(), std::mem::take(&mut rows.room));
    let id = keys.first().copied().or(action).unwrap_or_default();
    if !rows.shows(ui, id, label, description) {
        return None;
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
    Some(response)
}

/// A row: its label on the left, the control with the value on the right. The one-line hint
/// shows under the label while the pointer is over the row or focus is in it; a note shows
/// always. A row a PC, game or profile sets has an amber bar at its left edge and, under its
/// label, where its value comes from beside the layer's box. `reset_w` keeps room left of the
/// control for a Reset, so the text never moves when it comes and goes.
#[allow(clippy::too_many_arguments)]
fn settings_row(
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
            let mut job = egui::text::LayoutJob::simple_singleline(tr(hint).to_owned(), regular(text::CAPTION), *ON_SURFACE_VARIANT);
            job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
            let hint_g = painter.layout_job(job);
            let note_g = note.map(|(text, color)| painter.layout(text.clone(), regular(text::CAPTION), *color, text_w));
            let state_g = state.as_ref().map(|(text, color)| painter.layout_no_wrap(text.clone(), regular(text::CAPTION), *color));
            let (label_h, hint_h) = (label_g.size().y, hint_g.size().y);
            let note_h = note_g.as_ref().map_or(0.0, |g| GAP + g.size().y);
            let full = label_h + GAP + hint_h + note_h;
            // Where a list choice's box lands under it: where the value comes from sits beside it.
            let to_state = (COMBO_HEIGHT + 18.0).max(full + GAP);
            let height = state_g.as_ref().map_or(full, |g| to_state + g.size().y);
            let (text_rect, _) = ui.allocate_exact_size(vec2(text_w, height), Sense::hover());
            let align = if state.is_some() { Align::Min } else { Align::Center };
            ui.with_layout(Layout::right_to_left(align), |ui| out = Some(control(ui)));
            // The hint while the pointer is over the row or focus is in it.
            let row = ui.min_rect().expand2(vec2(18.0, 8.0));
            let focused = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| row.contains(r.rect.center()));
            let active = focused || ui.rect_contains_pointer(row);
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

/// A section's mark in the list beside the rows, the same glyph as Android's (`SettingsRail.iconOf`).
fn section_icon(name: &str) -> Icon {
    match name {
        n if n == en("Profiles") => Icon::Folder,
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
fn section_item(ui: &mut Ui, name: &str, current: bool, badge: Badge, role: egui::Role) -> Response {
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
            multi_controller: false,
            deadzone: 12,
            console_home: false,
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
        // The merged choices say the one value both keys make.
        let value = |s: Settings, key: &str| value_text(key, &s, &display, crate::glyphs::Style::Xbox);
        assert_eq!(value(Settings::default(), "controllers"), "A player each");
        assert_eq!(value(Settings { multi_controller: false, ..Settings::default() }, "controllers"), "One player");
        assert_eq!(value(Settings { controllers: false, ..Settings::default() }, "controllers"), "Off");
        assert_eq!(value(Settings::default(), "open_on_start"), "Home");
        assert_eq!(value(Settings { console_home: false, ..Settings::default() }, "open_on_start"), "Show my PCs");
        assert_eq!(value(Settings::default(), "prefer_yuv444"), "Off");
        assert_eq!(value(Settings { experimental_packed_444: true, ..Settings::default() }, "prefer_yuv444"), "Automatic");
        assert_eq!(value(Settings::default(), "network_grace_ms"), "Off", "the desktop's 3 s default is within the connection's own timeout");
        assert_eq!(value(Settings { network_grace_ms: 30_000, ..Settings::default() }, "network_grace_ms"), "30 s");
        assert_eq!(value(Settings::default(), "align_video_top"), "Center");
    }

    /// "When Nightjar opens" saves both of its keys, and each choice reads back as itself.
    #[test]
    fn when_nightjar_opens_writes_both_of_its_keys() {
        for screen in StartScreen::ALL {
            for console_home in [true, false] {
                let mut s = Settings { console_home, ..Settings::default() };
                set_start_screen(&mut s, screen);
                assert_eq!(start_screen(&s), screen, "{screen:?} from console_home {console_home}");
            }
        }
        let mut s = Settings::default();
        set_start_screen(&mut s, StartScreen::Pcs);
        assert_eq!((s.open_on_start, s.console_home), (crate::config::OpenOnStart::Pcs, false));
        set_start_screen(&mut s, StartScreen::Last);
        assert_eq!((s.open_on_start, s.console_home), (crate::config::OpenOnStart::Last, false), "Continue keeps the screen behind it");
        assert_eq!(start_screen_name(StartScreen::Chosen, Some("Racing Game")), "Racing Game");
    }

    /// The tier tables name every row once, and an action row is never a setting.
    #[test]
    fn the_tier_tables_name_each_row_once() {
        let settings = serde_json::to_value(Settings::default()).unwrap();
        let mut seen = std::collections::HashSet::new();
        for (desktop, _) in ANDROID_KEYS {
            assert!(seen.insert(desktop), "{desktop} is listed once");
            assert!(settings.get(desktop).is_some() != ACTION_ROWS.contains(&desktop), "{desktop} is a setting or an action, not both");
        }
        for action in ACTION_ROWS {
            assert!(settings.get(action).is_none(), "{action} stores nothing");
        }
        assert!(main_row("window_mode") && main_row("vrr") && main_row("mouse_mode") && main_row("overlay_position") && main_row("connection_test"));
        assert!(!main_row("swap_face_buttons") && !main_row("prefer_yuv444") && !main_row("pyrowave_pace") && !main_row("usb_forwarding"));
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
