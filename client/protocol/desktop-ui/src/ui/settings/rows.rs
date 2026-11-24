//! Settings row collector (`Rows`), value formatters, key tier tables, and test-PC selectors.

use super::*;

/// A Wait through network dropouts choice in seconds; 0 is Off.
/// "Frames waiting for the decoder"'s choices, as Android's `decode_queue_frames_values`.
pub(in crate::ui) const DECODE_QUEUE_CHOICES: [u32; 4] = [3, 5, 8, 15];

/// The choice a saved bound shows as: the engine reads anything outside 1 to 15 as 15, and a
/// value between the choices as the next larger one.
pub(in crate::ui) fn decode_queue_choice(frames: u32) -> u32 {
    if frames == 0 || frames > 15 {
        return 15;
    }
    DECODE_QUEUE_CHOICES.into_iter().find(|&c| c >= frames).unwrap_or(15)
}

pub(super) fn decode_queue_name(frames: u32) -> String {
    trf("{} frames", &[&frames])
}

pub(super) fn network_grace_name(seconds: u32) -> String {
    if seconds == 0 { tr("Off").to_owned() } else { trf("{} s", &[&seconds]) }
}

pub(super) fn mobile_bitrate_name(kbps: u32) -> String {
    if kbps == 0 { tr("Off").to_owned() } else { format!("{} Mbps", f64::from(kbps) / 1000.0) }
}

/// "When Nightjar opens" as the two keys it is kept in make it ([`StartScreen::from_saved`]).
pub(in crate::ui) fn start_screen(s: &Settings) -> StartScreen {
    use crate::config::OpenOnStart;
    let open = match s.open_on_start {
        OpenOnStart::Pcs => "pcs",
        OpenOnStart::Last => "last",
        OpenOnStart::Chosen => "chosen",
    };
    StartScreen::from_saved(open, s.console_home)
}

/// Saves a "When Nightjar opens" choice into its two keys ([`StartScreen::to_saved`]).
pub(in crate::ui) fn set_start_screen(s: &mut Settings, screen: StartScreen) {
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
pub(super) fn start_screen_name(screen: StartScreen, game: Option<&str>) -> String {
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
pub(super) fn guide_hold_name(ms: u32) -> String {
    if ms == 0 {
        return tr("Off").to_owned();
    }
    let seconds = format!("{}", f64::from(ms) / 1000.0);
    let seconds = if crate::strings::current().decimal_comma() { seconds.replace('.', ",") } else { seconds };
    trf("{} s", &[&seconds])
}

pub(super) fn audio_name(channels: AudioChannels) -> &'static str {
    match channels {
        AudioChannels::Stereo => tr("Stereo"),
        AudioChannels::Surround51 => tr("5.1 surround"),
        AudioChannels::Surround71 => tr("7.1 surround"),
        AudioChannels::Surround714 => tr("7.1.4 surround"),
    }
}

pub(super) fn language_name(language: Language) -> &'static str {
    crate::strings::chosen(language).map_or_else(|| tr("System default"), crate::strings::Lang::name)
}

/// [`language_name`], or the English name where the loaded fonts lack its letters: they hold
/// the system font of the language in use only, so a Russian menu cannot draw 日本語.
pub(super) fn drawable_language_name(ui: &egui::Ui, language: Language) -> &'static str {
    let name = language_name(language);
    let font = egui::FontId::proportional(crate::theme::text::BODY);
    match crate::strings::chosen(language) {
        Some(lang) if !ui.ctx().fonts_mut(|f| f.has_glyphs(&font, name)) => lang.english_name(),
        _ => name,
    }
}

pub(super) fn reduce_motion_name(mode: ReduceMotion) -> &'static str {
    match mode {
        ReduceMotion::System => tr("Follow system"),
        ReduceMotion::Reduce => tr("Reduce"),
        ReduceMotion::Full => tr("Full motion"),
    }
}

/// What a row shows for its value, as its control says it: the default beside a Reset, and the
/// value a PC's or game's own setting stands in for.
pub(in crate::ui) fn value_text(key: &str, s: &Settings, display: &Display, style: crate::glyphs::Style) -> String {
    match key {
        "open_on_start" | "console_home" => start_screen_name(start_screen(s), None),
        "simple_smoothness" => tr(simple_smoothness_name(s.simple_smoothness)).to_owned(),
        "simple_picture" => tr(simple_picture_name(s.simple_picture)).to_owned(),
        "simple_hdr" => tr(simple_hdr_name(simple_effective_hdr(s))).to_owned(),
        "controllers" | "multi_controller" => tr(controllers_name(Controllers::from_saved(s.controllers, s.multi_controller))).to_owned(),
        "lower_bitrate" => tr(lower_bitrate_name(s.lower_bitrate)).to_owned(),
        "back_opens_menu" | "back_as_guide" => tr(back_button_name(BackButton::from_saved(s.back_opens_menu, s.back_as_guide))).to_owned(),
        "capture_system_keys" | "system_keys_fullscreen_only" => tr(system_keys_name(SystemKeys::from_saved(s.capture_system_keys, s.system_keys_fullscreen_only))).to_owned(),
        "client_roaming" | "roaming_metered" => tr(roaming_name(Roaming::from_saved(s.client_roaming, s.roaming_metered))).to_owned(),
        "prefer_yuv444" | "experimental_packed_444" => sharp_color_name(sharp_color(s)).to_owned(),
        "vrr" | "present_at_host_times" | "vrr_tear_free" => vrr_choice_name(vrr_choice(s)).to_owned(),
        "vsync" | "fractional_vsync" => vsync_name(vsync_choice(s)).to_owned(),
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
        "vrr_wait" => tr(s.vrr_wait.label()).to_owned(),
        "display_output" => tr(s.display_output.label()).to_owned(),
        "pyrowave_decoder" => tr(s.pyrowave_decoder.label()).to_owned(),
        "window_mode" => window_name(s.window_mode).to_owned(),
        "video_scaling" => tr(s.video_scaling.label()).to_owned(),
        "host_display" => tr(display_choice_name(display_choice(s, true))).to_owned(),
        "virtual_display_scale" => format!("{}%", s.virtual_display_scale),
        "virtual_display_dpi" => display_scale_name(s.virtual_display_dpi),
        "frame_pacing" | "drop_late_frames" | "host_frame_times" => frame_pacing_choice_name(frame_pacing_choice(s)).to_owned(),
        "mouse_mode" => mouse_name(s.mouse_mode).to_owned(),
        "mouse_speed" => format!("{}%", s.mouse_speed),
        "deadzone" => format!("{}%", s.deadzone),
        "trigger_deadzone" => format!("{}%", s.trigger_deadzone),
        "gamepad_quit_combo" => style.quit_combo(s.gamepad_quit_combo),
        "disable_rumble" | "rumble_feel" => vibration_name(vibration_choice(s)).to_owned(),
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
        "decode_queue_frames" => decode_queue_name(decode_queue_choice(s.decode_queue_frames)),
        "error_correction" => error_correction_name(s.error_correction),
        "rtx_hdr" => enhancement_name(s.rtx_hdr),
        "rtx_video" => enhancement_name(s.rtx_video),
        "language" => language_name(s.language).to_owned(),
        "ui_zoom" => if s.ui_zoom == 0 {
            tr("Auto (follows the screen)").to_owned()
        } else {
            format!("{}%", crate::config::nearest_zoom(s.ui_zoom))
        },
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
pub(in crate::ui) const MAIN_KEYS: [(&str, &[&str]); 9] = [
    ("simple", &["simple_smoothness", "simple_picture", "simple_screen", "simple_hdr", "checkbox_console_home", "simple_back_to_recommended"]),
    ("video", &["list_resolution", "list_fps", "automatic_bitrate", "seekbar_bitrate_kbps", "lower_bitrate", "list_mobile_bitrate_kbps", "video_format", "checkbox_enable_hdr"]),
    ("display", &["video_scaling", "stream_orientation", "checkbox_vrr", "frame_pacing"]),
    ("input", &["touch_mode", "checkbox_absolute_mouse_mode", "controllers", "controller_button_mapping", "controller_test", "checkbox_show_onscreen_controls", "checkbox_keyboard_follows_text_fields", "checkbox_clipboard_sync"]),
    ("audio", &["list_audio_config", "checkbox_host_audio", "checkbox_enable_mic", "checkbox_mic_button"]),
    ("streaming", &["checkbox_enable_perf_overlay", "performance_overlay_position", "checkbox_auto_reconnect", "network_tests"]),
    ("app", &["theme", "list_languages", "list_open_on_start", "checkbox_quick_resume", "pin_lock", "help", "report_problem", "check_updates_now"]),
    ("advanced", &["low_latency_decoding", "low_latency_customize", "gpu_driver", "checkbox_gpu_max_clocks", "checkbox_texture_view", "checkbox_vulkan_render"]),
    ("debug", &["list_fec_choice", "checkbox_tell_host_wireless", "checkbox_network_priority", "checkbox_unbatched_input"]),
];

/// Every desktop row on both platforms and the Android preference it is: a setting's first key or
/// an action row's name, then Android's key. A row is main when its match is in `MAIN_KEYS`.
/// `present_at_host_times` matches Android's `checkbox_real_vrr`; on the desktop it is a key of
/// the Variable refresh rate row, as the desktop-only `vrr_tear_free` is.
pub(in crate::ui) const ANDROID_KEYS: &[(&str, &str)] = &[
    // Recommended (the simple layer)
    ("simple_smoothness", "simple_smoothness"), ("simple_picture", "simple_picture"), ("simple_hdr", "simple_hdr"),
    ("simple_console", "checkbox_console_home"), ("simple_screen", "simple_screen"), ("simple_back", "simple_back_to_recommended"),
    // Video
    ("width", "list_resolution"), ("fps", "list_fps"), ("bitrate_kbps", "seekbar_bitrate_kbps"),
    ("lower_bitrate", "lower_bitrate"), ("mobile_bitrate", "list_mobile_bitrate_kbps"), ("codec", "video_format"),
    ("hdr", "checkbox_enable_hdr"), ("prefer_yuv444", "sharp_color"), ("sdr_10bit", "checkbox_enable_sdr_10bit"),
    ("frame_warp", "checkbox_frame_warp"),
    ("adaptive_quality", "adaptive_quality_mode"), ("quality_min_short_edge", "adaptive_quality_min_resolution"),
    ("quality_min_fps", "adaptive_quality_min_fps"),
    // Display & image
    ("video_scaling", "video_scaling"), ("vrr", "checkbox_vrr"), ("present_at_host_times", "checkbox_real_vrr"),
    ("frame_pacing", "frame_pacing"), ("fractional_vsync", "checkbox_fractional_vsync"), ("upscaling", "upscaling_mode"),
    ("upscale_sharpness", "upscaling_sharpness"),
    ("align_video_top", "video_alignment"),
    ("virtual_display_scale", "seekbar_virtual_display_scale"), ("virtual_display_dpi", "list_virtual_display_dpi"),
    ("host_display", "host_display_location"),
    // Controls
    ("mouse_mode", "checkbox_absolute_mouse_mode"), ("controllers", "controllers"), ("controller_buttons", "controller_button_mapping"),
    ("controller_test", "controller_test"), ("keyboard_follows_text_fields", "checkbox_keyboard_follows_text_fields"), ("clipboard_sync", "checkbox_clipboard_sync"), ("swap_face_buttons", "checkbox_flip_face_buttons"),
    ("gamepad_quit_combo", "list_gamepad_quit_combo"), ("deadzone", "seekbar_deadzone"), ("trigger_deadzone", "seekbar_trigger_deadzone"),
    ("controller_motion", "checkbox_gamepad_motion_sensors"), ("disable_rumble", "list_rumble_feel"), ("rumble_feel", "list_rumble_feel"),
    ("controller_mouse", "checkbox_mouse_emulation"), ("analog_scrolling", "analog_scrolling"),
    ("mouse_fixed_step", "checkbox_mouse_emulation_fixed_step"), ("gamepad_touchpad_as_mouse", "checkbox_gamepad_touchpad_as_mouse"),
    ("guide_hold_ms", "list_guide_menu_hold"), ("back_opens_menu", "back_button"),
    ("stream_menu_buttons", "list_stream_menu_button_y"), ("keyboard_shortcuts", "keyboard_shortcuts_editor"),
    ("mouse_speed", "seekbar_mouse_speed"), ("mouse_nav_buttons", "checkbox_mouse_nav_buttons"), ("stream_hotkeys", "stream_hotkeys"),
    ("capture_system_keys", "checkbox_capture_system_keys"),
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
    ("network_grace_ms", "list_network_grace"),
    ("receive_coalescing", "checkbox_receive_coalescing"), ("jumbo_frames", "jumbo_frames"), ("internet_play", "checkbox_internet_play"),
    ("hide_connection_warnings", "checkbox_disable_warnings"), ("dual_screen", "checkbox_dual_screen"),
    ("second_screen_width", "list_resolution_screen2"), ("input_only_any_host", "checkbox_input_only_any_host"),
    // App
    ("theme", "theme"), ("language", "list_languages"), ("open_on_start", "list_open_on_start"),
    ("quick_resume", "checkbox_quick_resume"), ("pin_lock", "pin_lock"), ("help", "help"), ("report_problem", "report_problem"), ("check_updates_now", "check_updates_now"),
    ("check_updates", "checkbox_check_updates"), ("home_card_size", "checkbox_small_icon_mode"),
    ("show_debug_settings", "checkbox_show_debug_settings"), ("about_app", "about_app"),
    ("reset_all", "reset_all"),
    ("ui_zoom", "list_ui_scale"), ("high_contrast", "checkbox_high_contrast"), ("reduce_motion", "list_reduce_motion"),
    // Advanced
    ("sops", "checkbox_enable_sops"), ("decode_queue_frames", "list_decode_queue_frames"),
    // Debug
    ("error_correction", "list_fec_choice"),
    ("tell_host_wireless", "checkbox_tell_host_wireless"), ("tell_host_link_speed", "checkbox_tell_host_wireless"),
    ("network_priority", "checkbox_network_priority"), ("unbatched_input", "checkbox_unbatched_input"), ("full_range", "checkbox_full_range"),
    ("link_probe", "checkbox_link_probe"), ("pyrowave_pace", "pace_calibration"),
    ("post_stream_decode_time", "checkbox_enable_post_stream_toast"), ("latency_benchmark", "latency_benchmark"),
    ("export_latency_csv", "export_latency_csv"),
];

/// Main rows Android has no match for: the desktop's window, and the graphics card and renderer
/// beside Android's own Advanced main rows.
pub(in crate::ui) const DESKTOP_MAIN: [&str; 3] = ["window_mode", "graphics_card", "renderer"];

/// The action rows: they open a page or run a test and store nothing, so no setting key names them.
#[cfg_attr(not(test), allow(dead_code))]
pub(in crate::ui) const ACTION_ROWS: [&str; 23] = ["mobile_bitrate", "controller_buttons", "controller_test", "keyboard_shortcuts", "stream_hotkeys",
    "connection_test", "pin_lock", "help", "report_problem", "check_updates_now", "sign_in", "about_app", "logs", "reset_all", "pyrowave_pace",
    "latency_benchmark", "export_latency_csv", "steamgriddb_key", "igdb_client_id", "igdb_client_secret",
    "simple_console", "simple_screen", "simple_back"];

/// Whether the row `id` (a setting's first key, an action's name) is one of its section's main rows.
pub(in crate::ui) fn main_row(id: &str) -> bool {
    DESKTOP_MAIN.contains(&id)
        || ANDROID_KEYS.iter().any(|&(desktop, android)| desktop == id && MAIN_KEYS.iter().any(|(_, main)| main.contains(&android)))
}

/// The settings page's rows as it draws them: the open section's while browsing, every row the
/// search finds while searching, each section's and group's heading over its first row drawn.
pub(in crate::ui) struct Rows<'a> {
    pub(super) layer: Option<(&'a Settings, &'a mut Overrides)>,
    /// Where an unchecked row's value comes from when not the main settings (`Launcher::inherited_from`).
    pub(super) sources: &'a [(Overrides, String)],
    /// What a layered row's box says: "For this PC", "For this game" or "In this profile".
    pub(super) own: &'static str,
    /// The search's words; none while browsing.
    pub(super) words: Vec<String>,
    /// The section shown while browsing.
    pub(super) open: &'static str,
    pub(super) section: &'static str,
    pub(super) group: &'static str,
    /// The heading waiting for the first row drawn under it: the section's while searching
    /// (true), a group's title (empty: a gap only).
    pub(super) heading: Option<(bool, &'static str)>,
    /// Rows drawn, per section.
    pub(super) found: [usize; 9],
    /// Whether the rows that follow are drawn: a setting they depend on or this device can rule
    /// them out (`when`).
    pub(super) shown: bool,
    /// Each section's fold is open.
    pub(super) more: [bool; 9],
    /// The section's main rows and folded rows its gates show so far, and whether its first
    /// folded row came.
    pub(super) mains: usize,
    pub(super) folded: usize,
    pub(super) past_fold: bool,
    /// The open section's fold row, waiting for its count at the section's end.
    pub(super) fold: Option<FoldRow>,
    /// The open section has rows behind its fold: the fold row comes at the section's end, the
    /// last line of its card, under the rows it opens.
    pub(super) fold_at: Option<usize>,
    /// The section whose fold row was clicked.
    pub(super) toggled: Option<usize>,
    /// The first row drawn takes focus (a new page).
    pub(super) focus_first: bool,
    /// The keys whose row was drawn: a second row of the same setting (Custom resolution,
    /// Bitrate) gets no Reset of its own.
    pub(super) reset_drawn: Vec<&'static str>,
    pub(super) defaults: Settings,
    pub(super) display: Display,
    pub(super) style: crate::glyphs::Style,
    /// The id of each row drawn, in order, with whether it sits behind its section's fold.
    pub(super) drawn: Vec<(&'static str, bool)>,
    /// The next row's action name (an action row has no key), its note, and room its control
    /// takes beyond the usual.
    pub(super) action: Option<&'static str>,
    pub(super) note: Option<(String, Color32)>,
    pub(super) room: f32,
    /// Why the next row does nothing now (`applies`): it is left out while it holds its default,
    /// and search finds it with this reason. `held` says an action row holds a value of its own.
    pub(super) idle: Option<String>,
    pub(super) held: bool,
}

/// The fold row of the open section, drawn before its first folded row; its words come once the
/// section's folded rows are counted.
pub(super) struct FoldRow {
    painter: egui::Painter,
    words: egui::layers::ShapeIdx,
    rect: Rect,
    response: Response,
    open: bool,
}

impl Rows<'_> {
    pub(super) fn searching(&self) -> bool {
        !self.words.is_empty()
    }

    /// Starts `title`'s rows.
    pub(super) fn section(&mut self, ui: &mut Ui, title: &'static str) {
        self.finish(ui);
        (self.section, self.group) = (title, "");
        self.heading = self.searching().then_some((true, title));
        (self.mains, self.folded, self.past_fold) = (0, 0, false);
        self.always();
    }

    /// Starts a group of rows inside the section, such as Refresh rate; an empty title parts the
    /// rows that follow from the group before without a heading.
    pub(super) fn group(&mut self, title: &'static str) {
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
    pub(super) fn when(&mut self, shown: bool) {
        self.shown &= shown;
    }

    pub(super) fn always(&mut self) {
        self.shown = true;
    }

    /// The next row is the action `id`.
    pub(super) fn action(&mut self, id: &'static str) {
        self.action = Some(id);
    }

    /// A line the next row always shows under its label, such as a warning about its value.
    pub(super) fn note(&mut self, text: String, color: Color32) {
        self.note = Some((text, color));
    }

    /// Room the next row's control takes beyond the usual column (a PC's list beside its button).
    pub(super) fn room(&mut self, width: f32) {
        self.room = width;
    }

    /// The next row can do something only while `applies` (its mode, the setting it belongs to, or
    /// what this device has): otherwise it is left out while it holds its default, and search
    /// finds it saying `reason`. A row holding another value stays, with the reason as its note.
    pub(super) fn applies(&mut self, applies: bool, reason: impl FnOnce() -> String) {
        self.idle = (!applies).then(reason);
    }

    /// The next row is never left out, since a PC's, game's or profile's settings or the stream
    /// menu can make it act, but while the main settings leave it nothing to do it says `reason`.
    pub(super) fn unused(&mut self, applies: bool, reason: impl FnOnce() -> String) {
        if !applies && self.note.is_none() {
            self.note = Some((reason(), *TERTIARY));
        }
    }

    /// Whether the row `id` is drawn; draws the fold and the headings waiting for it. An `idle`
    /// row (`applies`) shows only in search.
    pub(super) fn shows(&mut self, ui: &mut Ui, id: &'static str, label: &str, description: &str, idle: bool) -> bool {
        if !self.shown || (idle && !self.searching()) {
            return false;
        }
        let at = section_index(self.section);
        let folded = !main_row(id);
        // A section without main rows here (Advanced on most computers) lists its rows straight away.
        let behind = folded && self.mains > 0;
        if folded { self.folded += 1 } else { self.mains += 1 }
        let browsing = !self.searching() && self.section == self.open;
        if behind && !self.past_fold && browsing {
            // The rows it opens follow the main rows in the same card; the fold row ends it.
            self.fold_at = Some(at);
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

    /// The fold over the section's other rows, the last line of the section: "More … settings
    /// (n)", or "Fewer … settings" under the rows while open. Its words wait for the count (`finish`).
    pub(super) fn fold_row(&mut self, ui: &mut Ui, at: usize) {
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

    /// Ends the section: its fold row, last, says how many rows it holds.
    pub(super) fn finish(&mut self, ui: &mut Ui) {
        if let Some(at) = self.fold_at.take() {
            self.fold_row(ui, at);
        }
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
pub(super) fn search_heading(ui: &mut Ui, title: &str) {
    ui.add_space(space::L);
    ui.label(RichText::new(tr(title)).font(semibold(text::SECTION)).color(ON_SURFACE));
    ui.add_space(space::XS);
}

/// A group's heading inside a section, `above` apart from what comes before it.
pub(super) fn group_heading(ui: &mut Ui, title: &str, above: f32) {
    ui.add_space(above);
    ui.label(RichText::new(tr(title)).font(semibold(text::SECTION)).color(ON_SURFACE));
    ui.add_space(space::XS);
}

pub(super) fn stream_menu_button_name(choice: Option<&str>) -> String {
    match choice.map(nightjar_policy::stream_menu::saved_choice) {
        None => tr("Default").to_owned(),
        Some(NO_TILE) => tr("Off").to_owned(),
        Some(id) => crate::stream_menu::ASSIGNABLE.iter().find(|(tile, _)| *tile == id).map_or(id, |(_, name)| tr(name)).to_owned(),
    }
}

/// A button's saved choice as the list shows it: close_game, saved before Quit game was one
/// choice, is Quit game, so the list highlights it.
pub(super) fn stream_menu_button_choice(s: &Settings, key: Hotkey) -> Option<String> {
    s.stream_menu_buttons.get(&key).map(|saved| nightjar_policy::stream_menu::saved_choice(saved).to_owned())
}

/// The list of what a controller button can run in the stream menu: its own tile, nothing, or any of ours.
pub(super) fn stream_menu_button(ui: &mut Ui, s: &mut Settings, key: Hotkey) -> Response {
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
pub(in crate::ui) fn test_pcs(screen: Screen, hosts: &Hosts, can: impl Fn(&Host) -> bool) -> Vec<&Host> {
    match screen {
        Screen::HostSettings(id) => hosts.get(id).filter(|h| h.paired() && can(h)).into_iter().collect(),
        Screen::Settings => hosts.list.iter().filter(|h| h.paired() && can(h)).collect(),
        _ => Vec::new(),
    }
}

/// The PC of `pcs` a test runs on: the one chosen while it is still there, else the first that is
/// online, else the first.
pub(in crate::ui) fn network_test_pc<'h>(pcs: &[&'h Host], chosen: Option<u64>) -> Option<&'h Host> {
    chosen.and_then(|id| pcs.iter().find(|h| h.id == id)).or_else(|| pcs.iter().find(|h| h.reach == Reach::Online)).or_else(|| pcs.first()).copied()
}

/// The list beside a test's button that chooses its PC among several; the choice holds for both
/// tests until Nightjar closes.
pub(super) fn test_pc_choice(ui: &mut Ui, salt: &str, pcs: &[&Host], pc: &Host, chosen: &mut Option<u64>) {
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
pub(super) fn offline(ui: &mut Ui) -> Response {
    ui.label(RichText::new(tr("Offline")).font(regular(text::BODY)).color(ON_SURFACE_VARIANT))
}

/// Vibration's choices: Off, then the feels. One list over the two stored fields, as on Android.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Vibration {
    Off,
    Feel(nightjar_policy::rumble::RumbleFeel),
}

pub(in crate::ui) const VIBRATION_CHOICES: [Vibration; 4] = {
    use nightjar_policy::rumble::RumbleFeel;
    [Vibration::Off, Vibration::Feel(RumbleFeel::Subtle), Vibration::Feel(RumbleFeel::Standard), Vibration::Feel(RumbleFeel::Strong)]
};

pub(in crate::ui) fn vibration_choice(s: &Settings) -> Vibration {
    if s.disable_rumble { Vibration::Off } else { Vibration::Feel(s.rumble_feel) }
}

/// Off keeps the stored feel, so turning vibration back on restores it; a feel turns vibration on.
pub(in crate::ui) fn set_vibration_choice(s: &mut Settings, choice: Vibration) {
    match choice {
        Vibration::Off => s.disable_rumble = true,
        Vibration::Feel(feel) => {
            s.disable_rumble = false;
            s.rumble_feel = feel;
        }
    }
}

pub(in crate::ui) fn vibration_name(choice: Vibration) -> &'static str {
    match choice {
        Vibration::Off => tr("Off"),
        Vibration::Feel(feel) => tr(crate::config::rumble_feel_label(feel)),
    }
}

/// V-Sync's choices: Off, On, or On with every frame held the same number of refreshes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum VsyncChoice {
    Off,
    On,
    EvenCadence,
}

pub(in crate::ui) const VSYNC_CHOICES: [VsyncChoice; 3] = [VsyncChoice::Off, VsyncChoice::On, VsyncChoice::EvenCadence];

/// A pacing that waits for the refresh keeps V-Sync on whatever the switch holds.
pub(in crate::ui) fn vsync_choice(s: &Settings) -> VsyncChoice {
    if !s.vsync && !s.frame_pacing.forces_vsync() { VsyncChoice::Off }
    else if s.fractional_vsync { VsyncChoice::EvenCadence }
    else { VsyncChoice::On }
}

/// Off keeps the even-cadence field, so it is not lost to a look at Off; On and the cadence set both.
pub(in crate::ui) fn set_vsync_choice(s: &mut Settings, choice: VsyncChoice) {
    match choice {
        VsyncChoice::Off => s.vsync = false,
        VsyncChoice::On => { s.vsync = true; s.fractional_vsync = false; }
        VsyncChoice::EvenCadence => { s.vsync = true; s.fractional_vsync = true; }
    }
}

pub(in crate::ui) fn vsync_name(choice: VsyncChoice) -> &'static str {
    match choice {
        VsyncChoice::Off => tr("Off"),
        VsyncChoice::On => tr("On"),
        VsyncChoice::EvenCadence => tr("On, even cadence"),
    }
}
