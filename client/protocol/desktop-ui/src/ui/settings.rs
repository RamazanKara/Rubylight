//! The settings page: a list of sections on the left (chips on a narrow window), Profiles above
//! them, and the open section's rows on the right. Each section shows its main rows first, as
//! Android's `settings_tiers.xml` lists them, then a counted fold, "More … settings (n)", over the
//! rest in their groups. A row shows its value; its one-line hint shows while the pointer is over
//! it or focus is in it. A row that another setting or this device rules out is not drawn. A search
//! finds rows in every section, folded or not. A PC's, game's or profile's settings show the same
//! rows, each with a box that keeps its value for that layer.

use super::*;
use nightjar_policy::choices::{BackButton, LowerBitrate, Roaming, SystemKeys};
use nightjar_policy::controllers::Controllers;
use nightjar_policy::open_play::StartScreen;
use nightjar_policy::presets::{self, Preset};
use nightjar_policy::simple_profile as simple;
use crate::config::FramePacing;
use nightjar_policy::stream_menu::{Hotkey, NO_TILE};

/// The settings sections in the order Android lists them (App only on the main settings). Recommended is
/// the simple layer (SimpleProfile): three plain choices over the settings the other sections hold.
pub(super) const SETTINGS_SECTIONS: [&str; 9] = [en("Recommended"), en("Video"), en("Display & image"), en("Controls"), en("Audio"), en("Streaming"), en("App"), en("Advanced"), en("Debug")];

/// The sections the list always shows, simple first; "More settings" unfolds Profiles and the
/// other sections under them, as Android's overview does (`StreamSettings.RAIL_FOLDED`). A PC's,
/// game's or profile's settings list every section they have: they are the detail already.
pub(super) const RAIL_MAIN: [&str; 4] = [en("Recommended"), en("Controls"), en("Audio"), en("App")];

/// Whether this section's entry sits behind "More settings" on the main settings.
pub(super) fn rail_folded(section: &str) -> bool {
    !RAIL_MAIN.contains(&section)
}

/// Whether a Debug setting is not at its default: the section then shows without "Show debug
/// settings", so a changed diagnostic is never out of sight.
pub(super) fn debug_changed(s: &Settings) -> bool {
    // The defaults' Debug values, serialized once: this runs every frame.
    static DEFAULTS: std::sync::OnceLock<Option<serde_json::Value>> = std::sync::OnceLock::new();
    let defaults = DEFAULTS.get_or_init(|| serde_json::to_value(Settings::default()).ok());
    let now = serde_json::to_value(s).ok();
    SECTION_KEYS[section_index(en("Debug"))].iter().flat_map(|keys| keys.iter()).any(|key| {
        now.as_ref().and_then(|v| v.get(*key)) != defaults.as_ref().and_then(|v| v.get(*key))
    })
}

/// Whether the list of sections shows Debug: asked for with "Show debug settings", a Debug setting
/// is changed, or the layer being edited (a PC's, game's or profile's) sets one.
pub(super) fn debug_listed(s: &Settings, layer: Option<&Overrides>) -> bool {
    s.show_debug_settings || debug_changed(s) || layer.is_some_and(|layer| SECTION_KEYS[section_index(en("Debug"))].iter().any(|keys| layer.has(keys)))
}

/// Every setting row's keys, by section, in the order the page draws them with every fold open;
/// the first key names the row. Rows that show only with another setting (Custom resolution,
/// Bitrate, the controller mouse's options) share their keys or are listed where they show. Action
/// rows (a page, a test) store nothing and are not listed. A test keeps this equal to what the
/// page draws.
pub(super) const SECTION_KEYS: [&[&[&str]]; 9] = [
    &[&["simple_smoothness"], &["simple_picture"], &["simple_hdr"]],
    &[&["width", "height"], &["fps"], &["bitrate_kbps"], &["lower_bitrate"], &["codec"], &["hdr"],
        &["prefer_yuv444", "experimental_packed_444"], &["sdr_10bit"], &["frame_warp"], &["adaptive_quality"], &["quality_min_short_edge"], &["quality_min_fps"],
        &["hdr_desktop_brightness"], &["rtx_hdr"], &["rtx_video"]],
    &[&["window_mode"], &["video_scaling"], &["vrr", "present_at_host_times", "vrr_tear_free"], &["vrr_wait"],
        &["frame_pacing", "drop_late_frames", "host_frame_times"], &["vsync", "fractional_vsync"],
        &["upscaling"], &["upscale_sharpness"], &["align_video_top"], &["full_range"],
        &["host_display", "virtual_display"], &["virtual_display_scale"], &["virtual_display_dpi"]],
    &[&["mouse_mode"], &["controllers", "multi_controller"], &["keyboard_follows_text_fields"], &["clipboard_sync"],
        &["swap_face_buttons"], &["gamepad_quit_combo"], &["deadzone"], &["trigger_deadzone"], &["controller_motion"], &["disable_rumble", "rumble_feel"],
        &["controller_mouse"], &["analog_scrolling"], &["mouse_fixed_step"], &["gamepad_touchpad_as_mouse"], &["background_controllers"],
        &["guide_hold_ms"], &["back_opens_menu", "back_as_guide"], &["stream_menu_buttons"],
        &["mouse_speed"], &["mouse_nav_buttons"], &["native_touchpad"],
        &["capture_system_keys", "system_keys_fullscreen_only"], &["right_alt_as_meta"],
        &["controller_audio"], &["usb_forwarding"]],
    &[&["audio"], &["play_audio_on_host"], &["microphone"], &["mic_echo_cancel"], &["mic_noise_suppression"], &["mic_auto_gain"], &["mic_gain"],
        &["mute_audio"], &["audio_passthrough"]],
    &[&["overlay", "overlay_mode"], &["overlay_position"], &["auto_reconnect"],
        &["client_roaming", "roaming_metered"], &["network_grace_ms"], &["receive_coalescing"], &["jumbo_frames"], &["internet_play"], &["hide_connection_warnings"],
        &["dual_screen"], &["second_screen_width", "second_screen_height"], &["input_only_any_host"]],
    &[&["theme"], &["language"], &["open_on_start", "console_home"], &["quick_resume"], &["check_updates"], &["home_card_size"],
        &["show_debug_settings"], &["ui_zoom"], &["high_contrast"], &["reduce_motion"]],
    &[&["graphics_card"], &["renderer"], &["display_output"], &["video_planes"], &["high_performance_gpu_power"], &["sops"], &["decode_queue_frames"]],
    &[&["error_correction"], &["tell_host_wireless", "tell_host_link_speed"],
        &["network_priority"], &["unbatched_input"],
        &["pyrowave_decoder"], &["pyrowave_async_compute"],
        &["decoder"], &["software_slices"], &["vulkan_video_first"], &["vulkan_rfi"], &["videotoolbox_rfi"], &["cursor_shapes"],
        &["link_probe"], &["post_stream_decode_time"]],
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

mod advanced_sections;
mod choices;
mod controls_sections;
mod rows;
mod view;
mod video_sections;
mod widgets;

pub use choices::{pick_display_choice, FullscreenMode, DISPLAY_CHOICES};
#[cfg(test)]
pub(crate) use choices::closest_fullscreen_mode;
pub(crate) use view::{simple_apply, simple_caps, simple_effective_hdr, simple_state};
pub(super) use choices::{
    apply_preset, codec_name, display_choice, display_choice_name, display_scale_gate,
    display_scale_name, enhancement_name, error_correction_name, fractional_vsync_applies,
    fractional_vsync_display_reason, frame_pacing_choice, frame_pacing_choice_name,
    host_display_name, preset_current, preset_label, preset_section, preset_value,
    reset_to_defaults, set_frame_pacing_choice, set_vrr_choice, sharp_color, video_driver,
    vrr_choice, FramePacingChoice, VrrChoice,
};
#[cfg(test)]
pub(super) use choices::{vrr_choice_where, vrr_tear_free_applies};
pub(super) use rows::{
    decode_queue_choice, network_test_pc, set_start_screen, set_vibration_choice,
    set_vsync_choice, start_screen, test_pcs, value_text, vibration_choice, vibration_name,
    vsync_choice, vsync_name, Rows, DECODE_QUEUE_CHOICES, VIBRATION_CHOICES, VSYNC_CHOICES,
};
#[cfg(test)]
pub(super) use rows::{
    main_row, Vibration, VsyncChoice, ACTION_ROWS, ANDROID_KEYS, DESKTOP_MAIN, MAIN_KEYS,
};
pub(super) use view::SettingsView;
pub(super) use widgets::setting_row;

use choices::{
    back_button_name, controllers_name, decoder_name, enhancement_choice, lower_bitrate_name,
    mouse_name, no_results, picked_keys, preset_key, roaming_name, scrolling_name,
    sharp_color_name, short_edge_name, system_keys_name, unused_with, video_position_name,
    vrr_choice_name, window_name,
};
use rows::{
    audio_name, decode_queue_name, drawable_language_name, guide_hold_name,
    mobile_bitrate_name, network_grace_name, offline, reduce_motion_name,
    start_screen_name, stream_menu_button, test_pc_choice,
};
#[cfg(test)]
use rows::{stream_menu_button_choice, stream_menu_button_name};
use view::{
    found, search_id, search_words, section_index, simple_hdr_name, simple_picture_name,
    simple_smoothness_name,
};
use widgets::{notice, section_chip, KEYS_IN_HAND};
#[cfg(test)]
use widgets::banner;

#[cfg(test)]
mod tests;

/// Borrowed state passed into the section-drawing submodules of [`Launcher::settings_rows`].
pub(super) struct SectionsCtx<'a, 'h> {
    pub(super) display: Display,
    pub(super) screen_keyboard: bool,
    pub(super) wayland: bool,
    pub(super) layered: bool,
    pub(super) open: &'static str,
    pub(super) vrr_offered: bool,
    pub(super) mobile_shown: bool,
    pub(super) jumbo_offered: bool,
    pub(super) mobile_limit: u32,
    pub(super) connection_pcs: &'a [&'h Host],
    pub(super) pace_pcs: &'a [&'h Host],
    pub(super) open_game: &'a Option<String>,
    pub(super) custom_size_on: &'a mut Option<Screen>,
    pub(super) custom_fps_on: &'a mut Option<Screen>,
    pub(super) confirm_reset: &'a mut bool,
    pub(super) test_pc: &'a mut Option<u64>,
    pub(super) game_info_keys: &'a mut nightjar_policy::game_info::job::Keys,
    pub(super) links: &'a mut Vec<(&'static str, Response)>,
    pub(super) reset_all: bool,
    pub(super) open_controllers: Option<bool>,
    pub(super) open_help: bool,
    pub(super) open_page: Option<Screen>,
    pub(super) probe_device: usize,
    pub(super) gpu_mode: bool,
    pub(super) ten_bit: bool,
    pub(super) gpu_decoder_error: Option<String>,
    pub(super) fullscreen_modes: &'a [FullscreenMode],
    pub(super) hdr_display: bool,
    pub(super) hdr_display_probe: Option<fn() -> bool>,
    pub(super) hdr_probed: bool,
    pub(super) pin: &'a mut Option<crate::pin_ui::PinLock>,
    pub(super) sign_in: Option<bool>,
    pub(super) screen: Screen,
    pub(super) style: crate::glyphs::Style,
}

pub(super) fn run_gpu_probe(ui: &Ui, probe_device: usize, gpu_mode: bool, ten_bit: bool) -> Option<String> {
    if !gpu_mode {
        return None;
    }
    let probe = (crate::platform_hooks::hooks().pyrowave_probe)(probe_device, ten_bit);
    if probe.is_none() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    probe.and_then(Result::err)
}

impl Launcher {
    /// "More settings" was chosen: the fold opens or closes, and the choice is saved for the next
    /// start. Only this saves it; a link or a search result unfolds for its visit alone.
    fn toggle_more_settings(&mut self, commands: &mut Vec<Command>) {
        self.settings_view.toggle_rail();
        self.settings.settings_more_open = self.settings_view.rail_more;
        commands.push(Command::SaveSettings);
    }

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
        // Debug only on request, while one of its settings is changed (here or in the layer being
        // edited), while it is open (a link led there) or while a search looks everywhere.
        let debug = self.settings_view.section == en("Debug") || self.settings_view.searching()
            || debug_listed(&self.settings, layered.then_some(&self.edit_overrides));
        let sections: Vec<&'static str> = SETTINGS_SECTIONS.into_iter()
            .filter(|s| !(layered && (*s == "App" || *s == "Recommended")) && (debug || *s != en("Debug"))).collect();
        // The main settings keep their own section: a PC's, game's or profile's settings, which share
        // the view and open on Video, would otherwise leave it there with the list unfolded.
        if layered {
            self.settings_view.from_layered = true;
        } else if std::mem::take(&mut self.settings_view.from_layered) {
            self.settings_view.section = self.settings_view.main_section;
        }
        if !sections.contains(&self.settings_view.section) {
            // A page opened anew starts on its first section: Recommended on the main settings, the
            // few choices most people need, and Video on a PC's, game's or profile's.
            self.settings_view.section = sections[0];
        }
        if !layered {
            self.settings_view.main_section = self.settings_view.section;
        }
        // A page opened anew starts without the last visit's search; the section stays.
        if self.focus_screen && self.restore_key.is_none() {
            let view = &mut self.settings_view;
            view.query.clear();
            (view.confirm_reset, view.to_top) = (false, true);
            view.hdr_probed = false;
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
            (entries, profiles) = self.section_list(&mut side, sections, layered, commands);
            let drawn = side.min_rect().height();
            ui.data_mut(|d| d.insert_temp(height_id, drawn));
            side_bottom = page.top() + drawn;
            Rect::from_min_max(Pos2::new(page.left() + SIDE_LIST_W + space::XL, page.top()), page.max)
        } else {
            self.search_field(ui);
            ui.add_space(space::S);
            ui.horizontal_wrapped(|ui| {
                // The list's order on a wide window: the simple sections, More settings, then
                // Profiles (a page of its own, not a section) and the sections of detail.
                let open = layered || self.settings_view.rail_open();
                let mut toggled = false;
                for folded in [false, true] {
                    if folded && !layered {
                        if !self.settings_view.searching() {
                            let more = section_chip(ui, en("More settings"), false);
                            more.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, tr("More settings")));
                            self.focus_item(&more, "rail-more", false);
                            toggled = more.clicked();
                        }
                        if open {
                            let chip = chip(ui, "Profiles");
                            self.focus_item(&chip, "profiles-link", false);
                            profiles = chip.clicked();
                        }
                    }
                    for name in sections.iter().filter(|s| if layered { !folded } else { rail_folded(s) == folded }) {
                        if folded && !open {
                            break;
                        }
                        let current = !self.settings_view.searching() && self.settings_view.section == *name;
                        let chip = section_chip(ui, name, current);
                        entries.push((*name, chip.id));
                        if chip.clicked() {
                            self.settings_view.open(name);
                        }
                    }
                }
                if toggled {
                    self.toggle_more_settings(commands);
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
        // A row grows for its help only where no pointer aims at the rows (`settings_row`).
        let keys = !matches!(self.input_source, crate::input_source::InputSource::None);
        column.data_mut(|d| d.insert_temp(Id::unique(KEYS_IN_HAND), keys));
        let screen = self.screen;
        // Android's Presets, above the rows of a section they change; a PC's, game's or profile's
        // settings and a search have none.
        if !layered && !self.settings_view.searching() && preset_section(self.settings_view.section) {
            self.presets_row(&mut column);
        }
        self.settings_rows(&mut column, view, commands);
        // Neither child moved the page along: the rows and the list are what it holds.
        let bottom = column.min_rect().bottom().max(side_bottom);
        ui.advance_cursor_after_rect(Rect::from_min_max(page.min, Pos2::new(page.right(), bottom)));
        if self.screen == screen {
            if std::mem::take(&mut self.settings_view.focus_rows)
                && let Some(&(_, entry)) = entries.iter().find(|&&(name, _)| name == self.settings_view.section) {
                    ui.memory_mut(|m| m.request_focus(entry));
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

    /// The open section's rows, or every row the search finds.
    fn settings_rows(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let display = self.display;
        let (screen_keyboard, wayland) = (self.screen_keyboard, self.wayland);
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
            found: [0; 9],
            shown: true,
            more: self.settings_view.more,
            mains: 0,
            past_fold: false,
            folded: 0,
            fold: None,
            fold_at: None,
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
            idle: None,
            held: false,
        };
        let screen = self.screen;
        // Variable refresh rate needs tearing on Windows: where the swap chain can't, the row is not
        // drawn and the rows that apply without VRR are.
        let vrr_offered = self.tearing_supported || cfg!(not(windows));
        // Bitrate on mobile data is the profiles' cap, for every stream: on the main settings, once
        // this device was on mobile data or a cap is set (Android's mobileBitrateShown).
        let mut mobile_limit = self.profiles.mobile_limit_kbps;
        let mobile_shown = !layered && (self.network.kind(&self.profiles.home_wifi) == crate::network_types::Kind::Mobile || mobile_limit > 0);
        // Jumbo video packets are shown only where a saved PC offers more than stock packets.
        let jumbo_offered = !layered && view.hosts.list.iter().any(|h| {
            h.info.as_ref().is_some_and(|i| i.capabilities.max_video_packet() > 0)
        });
        // The tests a PC's own page runs on that PC and the main settings on a paired one.
        let connection_pcs = test_pcs(screen, view.hosts, |_| true);
        let pace_pcs = test_pcs(screen, view.hosts, |h| h.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe) || h.record.pace.is_some());
        let open_game = self.open_game.as_ref().map(|g| g.title.clone());
        let (custom_size_on, custom_fps_on) = (&mut self.custom_size, &mut self.custom_fps);
        let confirm_reset = &mut self.settings_view.confirm_reset;
        let test_pc = &mut self.settings_view.test_pc;
        let game_info_keys = &mut self.game_info_keys;
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
            let probe_device = self.gpu_device.unwrap_or(0);
            let (gpu_mode, ten_bit) = (s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu, s.hdr);
            let gpu_decoder_error = if s.codec.is_pyrowave() {
                run_gpu_probe(ui, probe_device, gpu_mode, ten_bit)
            } else {
                None
            };

            let mut ctx = SectionsCtx {
                display,
                screen_keyboard,
                wayland,
                layered,
                open,
                vrr_offered,
                mobile_shown,
                jumbo_offered,
                mobile_limit,
                connection_pcs: &connection_pcs,
                pace_pcs: &pace_pcs,
                open_game: &open_game,
                custom_size_on,
                custom_fps_on,
                confirm_reset,
                test_pc,
                game_info_keys,
                links: &mut links,
                reset_all,
                open_controllers,
                open_help,
                open_page,
                probe_device,
                gpu_mode,
                ten_bit,
                gpu_decoder_error,
                fullscreen_modes: &self.fullscreen_modes,
                hdr_display: self.hdr_display,
                hdr_display_probe: self.hdr_display_probe,
                hdr_probed: self.settings_view.hdr_probed,
                pin: &mut self.pin,
                sign_in: self.sign_in,
                screen,
                style,
            };
            video_sections::draw_video_sections(ui, view, s, &mut rows, &mut ctx);
            controls_sections::draw_controls_sections(ui, commands, s, &mut rows, &mut ctx);
            advanced_sections::draw_advanced_sections(ui, commands, s, &mut rows, &mut ctx);
            mobile_limit = ctx.mobile_limit;
            reset_all = ctx.reset_all;
            open_controllers = ctx.open_controllers;
            open_help = ctx.open_help;
            open_page = ctx.open_page;
            self.hdr_display = ctx.hdr_display;
            self.settings_view.hdr_probed = ctx.hdr_probed;
            rows.finish(ui);

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
        // The Window row moves the launcher's own window at once, as Fullscreen moves a stream's;
        // the other rows wait for the page to close. A PC's, game's or profile's Window value is
        // theirs alone and acts when it streams.
        if !layered && s.window_mode != before.window_mode {
            commands.push(Command::SaveSettings);
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
