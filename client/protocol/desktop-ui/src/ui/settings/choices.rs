//! Setting presets, display/VRR/frame-pacing choice mappings, and SDL fullscreen mode matching.

use super::*;

/// A preset's chip, as Android's `stream_preset_names` says it.
pub(in crate::ui) fn preset_label(preset: Preset) -> &'static str {
    match preset {
        Preset::Native => en("Native (recommended)"),
        Preset::BatterySaver => en("Battery saver"),
    }
}

/// A preset as the row's one-line value says it: "Native (recommended)" reads as "Native", as on
/// Android (`PresetPreference`), where the chip keeps the tag.
pub(in crate::ui) fn preset_value(preset: Preset) -> String {
    let name = tr(preset_label(preset));
    match name.rfind(" (") {
        Some(at) if name.ends_with(')') => name[..at].to_owned(),
        _ => name.to_owned(),
    }
}

pub(super) fn preset_key(preset: Preset) -> &'static str {
    match preset {
        Preset::Native => "preset-native",
        Preset::BatterySaver => "preset-battery-saver",
    }
}

/// The settings a preset is compared with (`nightjar_policy::presets`). Resolution, bitrate, VRR
/// and upscaling read as the preset's at their defaults: Native, Automatic, VRR where the display
/// has it, bilinear. The latency switches are the three the desktop has of Android's.
pub(in crate::ui) fn preset_current(s: &Settings, vrr_offered: bool) -> presets::Current {
    let defaults = Settings::default();
    presets::Current {
        native_resolution: s.width == 0 || s.height == 0,
        fps_follows_screen: s.fps == 0,
        fps: s.fps,
        default_bitrate: s.bitrate_kbps == 0,
        default_vrr: !vrr_offered || s.vrr == defaults.vrr,
        default_upscaling: s.upscaling == defaults.upscaling,
        // The PC's frame times are no preset's pacing, as on Android (`StreamPreset.matches`).
        pacing: match s.frame_pacing {
            _ if s.host_frame_times => None,
            FramePacing::Latency => Some(presets::Pacing::Latency),
            FramePacing::CapFps => Some(presets::Pacing::CapFps),
            FramePacing::Balanced | FramePacing::Smoothness => None,
        },
        latency_switches_on: s.drop_late_frames && s.unbatched_input && s.network_priority,
        power: None,
    }
}

/// The keys [`apply_preset`] sets.
pub(super) const PRESET_KEYS: [&str; 11] =
    ["width", "height", "fps", "bitrate_kbps", "vrr", "upscaling", "frame_pacing", "host_frame_times", "drop_late_frames", "unbatched_input", "network_priority"];

/// Whether `section` holds a row a preset changes: only those show the presets.
pub(in crate::ui) fn preset_section(section: &str) -> bool {
    SETTINGS_SECTIONS.iter().position(|&name| name == section).is_some_and(|at| SECTION_KEYS[at].iter().any(|row| row.iter().any(|key| PRESET_KEYS.contains(key))))
}

/// What a preset sets, Android's `StreamPreset.apply` on the desktop's settings. The codec, HDR
/// and 4:4:4 choices stay as they are.
pub(in crate::ui) fn apply_preset(s: &mut Settings, preset: Preset) {
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
    s.host_frame_times = false;
    if preset.sets_latency_switches() {
        (s.drop_late_frames, s.unbatched_input, s.network_priority) = (true, true, true);
    }
}

/// Every setting the page shows back at its default, but the language: Android's Reset all
/// settings keeps it too. Settings with no row (beam racing, early decode) stay as they are.
pub(in crate::ui) fn reset_to_defaults(settings: &Settings) -> Settings {
    let mut defaults = Overrides::default();
    for keys in SECTION_KEYS.iter().flat_map(|keys| keys.iter()) {
        if keys[0] != "language" {
            defaults.capture(&Settings::default(), keys);
        }
    }
    defaults.apply(settings)
}

/// The search found nothing: say so, and offer the way back. True when Clear search was clicked.
pub(super) fn no_results(ui: &mut Ui, query: &str) -> bool {
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
pub(in crate::ui) fn display_scale_gate(availability: nightjar_policy::http::Availability) -> bool {
    use nightjar_policy::http::Availability;
    matches!(availability, Availability::Supported | Availability::Unknown)
}

pub(in crate::ui) fn display_scale_name(percent: u32) -> String {
    if percent == 0 { tr("PC’s own").into() } else { format!("{percent}%") }
}

/// Display arrangement with the Virtual display switch folded in (Android's DisplayArrangementChoice):
/// the PC's own setting, a virtual display in the PC's own arrangement, then the arrangements. Each
/// tuple says whether the choice streams with a virtual display where the PC has display modes.
pub const DISPLAY_CHOICES: [(HostDisplay, bool); 8] = [
    (HostDisplay::HostDefault, false), (HostDisplay::HostDefault, true), (HostDisplay::Exclusive, true),
    (HostDisplay::ExtendedPrimary, true), (HostDisplay::Extended, true), (HostDisplay::ExtendedIsolated, true),
    (HostDisplay::ExtendedPrimaryIsolated, true), (HostDisplay::Physical, false)];

/// The choice the two stored values make. An arrangement wins, as at launch, where it asks for a
/// virtual display by itself (`requests_virtual_display`); the PC's own setting shows the switch. A
/// PC without display modes reads the switch alone, as its launch does. A saved pair stays as it is
/// until a choice is made, so no stream changes by itself.
pub(in crate::ui) fn display_choice(s: &Settings, modes: bool) -> (HostDisplay, bool) {
    if modes && s.host_display != HostDisplay::HostDefault {
        (s.host_display, s.host_display != HostDisplay::Physical)
    } else {
        (HostDisplay::HostDefault, s.virtual_display)
    }
}

/// A pick saves its arrangement in host_display. The PC's own setting and Virtual display also set
/// virtual_display, which the arrangements leave as it is: Remote Monitor keeps the PC's arrangement
/// and follows virtual_display alone, so it asks for what it asked for before.
pub fn pick_display_choice(s: &mut Settings, (mode, virtual_display): (HostDisplay, bool)) {
    s.host_display = mode;
    if mode == HostDisplay::HostDefault {
        s.virtual_display = virtual_display;
    }
}

/// The keys a value chosen in a row saves into a PC, game or profile layer: all of the row's, but an
/// arrangement leaves the virtual display switch as the layer has it, as on Android, so Remote
/// Monitor there keeps following the settings below (pick_display_choice).
pub(super) fn picked_keys<'k>(keys: &'k [&'static str], s: &Settings) -> &'k [&'static str] {
    if keys == ["host_display", "virtual_display"] && s.host_display != HostDisplay::HostDefault { &keys[..1] } else { keys }
}

pub(in crate::ui) fn display_choice_name(choice: (HostDisplay, bool)) -> &'static str {
    match choice {
        (HostDisplay::HostDefault, true) => en("Virtual display"),
        (mode, _) => host_display_name(mode),
    }
}

pub(in crate::ui) fn host_display_name(mode: HostDisplay) -> &'static str {
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

/// An image enhancement's choice: the PC's app setting, on or off.
pub(in crate::ui) fn enhancement_name(choice: Option<bool>) -> String {
    match choice {
        None => tr("PC’s setting"),
        Some(true) => tr("On"),
        Some(false) => tr("Off"),
    }
    .to_owned()
}

pub(super) fn enhancement_choice(ui: &mut Ui, salt: &str, choice: &mut Option<bool>) -> Response {
    Combo::from_id_salt(salt).selected_text(enhancement_name(*choice)).show_ui(ui, |ui| {
        for value in [None, Some(true), Some(false)] {
            ui.selectable_value(choice, value, enhancement_name(value));
        }
    }).response
}

pub(in crate::ui) fn error_correction_name(mode: ErrorCorrection) -> String {
    match mode {
        ErrorCorrection::Host => tr("PC’s setting").to_owned(),
        ErrorCorrection::Auto => tr("Automatic").to_owned(),
        ErrorCorrection::Percent(percentage) => trf("{} %", &[&percentage]),
    }
}

pub(in crate::ui) fn codec_name(codec: CodecPreference) -> &'static str {
    match codec {
        CodecPreference::Auto => tr("Automatic"),
        CodecPreference::H264 => "H.264",
        CodecPreference::Hevc => "HEVC",
        CodecPreference::Av1 => "AV1",
        CodecPreference::PyroWave => "PyroWave 4:2:0",
        CodecPreference::PyroWave444 => "PyroWave 4:4:4",
    }
}

pub(super) fn decoder_name(decoder: DecoderPreference) -> &'static str {
    match decoder {
        DecoderPreference::Auto => tr("Automatic"),
        DecoderPreference::Software => tr("Software"),
    }
}

pub(super) fn window_name(mode: WindowMode) -> &'static str {
    match mode {
        WindowMode::Fullscreen => tr("Fullscreen"),
        WindowMode::Borderless => tr("Borderless"),
        WindowMode::Windowed => tr("Windowed"),
    }
}

/// A row that does nothing with another setting's value says so under its label, naming the value
/// (client/AGENTS.md), as Android's "Not used with" tags do.
pub(super) fn unused_with(rows: &mut Rows, value: &str) {
    rows.note(trf("Not used with {}", &[&value]), *TERTIARY);
}

pub(super) fn mouse_name(mode: MouseMode) -> &'static str {
    match mode {
        MouseMode::Capture => tr("Captured"),
        MouseMode::Absolute => tr("Desktop pointer"),
    }
}

pub(super) fn scrolling_name(stick: crate::config::AnalogScrolling) -> &'static str {
    use crate::config::AnalogScrolling;
    match stick {
        AnalogScrolling::Off => tr("Off"),
        AnalogScrolling::Left => tr("Left"),
        AnalogScrolling::Right => tr("Right"),
    }
}

/// "Controllers" as the list says each choice (English; the caller translates).
pub(super) fn controllers_name(choice: Controllers) -> &'static str {
    match choice {
        Controllers::Off => en("Off"),
        Controllers::OnePlayer => en("One player"),
        Controllers::PlayerEach => en("A player each"),
    }
}

/// "Lower bitrate" as the list says each choice (English; the caller translates).
pub(super) fn lower_bitrate_name(choice: LowerBitrate) -> &'static str {
    match choice {
        LowerBitrate::Off => en("The PC decides"),
        LowerBitrate::WhenFramesLost => en("Lower it sooner"),
        LowerBitrate::Keep => en("Never: keep my bitrate"),
    }
}

/// "Back button" as the list says each choice (English; the caller translates).
pub(super) fn back_button_name(choice: BackButton) -> &'static str {
    match choice {
        BackButton::Back => en("Back"),
        BackButton::StreamMenu => en("Stream menu"),
        BackButton::Guide => en("Guide"),
    }
}

/// "Send system keys" as the list says each choice (English; the caller translates).
pub(super) fn system_keys_name(choice: SystemKeys) -> &'static str {
    match choice {
        SystemKeys::Off => en("Off"),
        SystemKeys::Always => en("Always"),
        SystemKeys::FullScreen => en("In full screen"),
    }
}

/// "Keep streaming when the network changes" as the list says each choice (English; the caller
/// translates).
pub(super) fn roaming_name(choice: Roaming) -> &'static str {
    match choice {
        Roaming::Off => en("Off"),
        Roaming::NotMetered => en("Except mobile data"),
        Roaming::AnyNetwork => en("Any network"),
    }
}

/// "Sharper colored text" is On while either of its two keys is on (`nightjar_policy::sharp_color`).
pub(in crate::ui) fn sharp_color(s: &Settings) -> bool {
    s.prefer_yuv444 || s.experimental_packed_444
}

pub(super) fn sharp_color_name(on: bool) -> &'static str {
    if on { tr("On") } else { tr("Off") }
}

/// "Variable refresh rate" as one choice over its three keys, which settings.json keeps as they
/// are: `vrr`, `present_at_host_times` (the game's timing) and `vrr_tear_free` (whole frames).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum VrrChoice {
    Off,
    /// The game's timing, whole frames.
    On,
    /// Each frame on arrival; it can tear.
    Fastest,
    /// A mix of the keys none of the others writes (the old separate switches, or a hand-edited
    /// file): shown as the value, never offered, and kept until another choice is made.
    Custom,
}

impl VrrChoice {
    /// The choices the list offers.
    pub(super) const OFFERED: [VrrChoice; 3] = [VrrChoice::Off, VrrChoice::On, VrrChoice::Fastest];
}

/// Whether `vrr_tear_free` does something on this platform: Windows and Linux wait out a scan
/// under way; macOS has no such wait, so the key reads as nothing there.
pub(in crate::ui) const fn vrr_tear_free_applies() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// Whether fractional V-Sync can hold pictures under SDL's video driver `video_driver`: Windows'
/// swap chain statistics, and on Linux Wayland's presentation feedback or KMS, not X11, which
/// tells no time a picture was shown; macOS presents without telling either.
pub(in crate::ui) fn fractional_vsync_applies(video_driver: &str) -> bool {
    cfg!(windows) || (cfg!(target_os = "linux") && video_driver != "x11")
}

/// One of a display's fullscreen modes, as `SDL_GetFullscreenDisplayModes` lists them (in its order:
/// largest first, then the deepest format, then the fastest refresh).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullscreenMode {
    /// In SDL's units, as the stream's Fullscreen asks for its size in.
    pub w: i32,
    pub h: i32,
    pub refresh_hz: f32,
    /// `SDL_BYTESPERPIXEL` of the mode's format.
    pub bytes_per_pixel: u8,
}

/// The mode exclusive fullscreen takes for a stream of `w` x `h` at `refresh_hz` (0: the desktop's
/// `desktop_hz`), high-density modes included as the stream asks: SDL's
/// `SDL_GetClosestFullscreenDisplayMode` (SDL 3.4.20, `src/video/SDL_video.c`, zlib license, by
/// Sam Lantinga and the SDL contributors) over `modes` in SDL's order. The smallest size that holds
/// the stream with the closest aspect ratio, and at that size the closest refresh, the later (the
/// slower) one on a tie. None where no mode holds the stream: SDL then keeps the desktop's mode.
/// Translated to Rust and altered from SDL's loop, under SDL's notice:
///
/// Copyright (C) 1997-2026 Sam Lantinga <slouken@libsdl.org>
///
/// This software is provided 'as-is', without any express or implied
/// warranty.  In no event will the authors be held liable for any damages
/// arising from the use of this software.
///
/// Permission is granted to anyone to use this software for any purpose,
/// including commercial applications, and to alter it and redistribute it
/// freely, subject to the following restrictions:
///
/// 1. The origin of this software must not be misrepresented; you must not
///    claim that you wrote the original software. If you use this software
///    in a product, an acknowledgment in the product documentation would be
///    appreciated but is not required.
/// 2. Altered source versions must be plainly marked as such, and must not be
///    misrepresented as being the original software.
/// 3. This notice may not be removed or altered from any source distribution.
///
pub(crate) fn closest_fullscreen_mode(modes: &[FullscreenMode], w: i32, h: i32, refresh_hz: f32, desktop_hz: f32) -> Option<FullscreenMode> {
    let aspect = if h > 0 { w as f32 / h as f32 } else { 1.0 };
    let refresh_hz = if refresh_hz == 0.0 { desktop_hz } else { refresh_hz };
    let mut closest: Option<FullscreenMode> = None;
    for mode in modes {
        if w > mode.w {
            break;
        }
        if h > mode.h {
            continue;
        }
        if let Some(c) = closest {
            let (current_aspect, closest_aspect) = (mode.w as f32 / mode.h as f32, c.w as f32 / c.h as f32);
            if (aspect - closest_aspect).abs() < (aspect - current_aspect).abs() {
                continue;
            }
            if mode.w == c.w && mode.h == c.h {
                if (c.refresh_hz - refresh_hz).abs() < (mode.refresh_hz - refresh_hz).abs() {
                    continue;
                }
                if c.bytes_per_pixel > mode.bytes_per_pixel {
                    continue;
                }
            }
        }
        closest = Some(*mode);
    }
    closest
}

/// Why fractional V-Sync holds nothing at the display the settings screen is on, or None; VRR is off
/// here (the row says so before). The refresh has to be 2 to 4 times the frame rate
/// (`rate_multiple`). Fullscreen on Windows switches to the mode closest to the stream's size and
/// rate (`fullscreen_refresh_hz`, `closest_fullscreen_mode` over `fullscreen_modes`), so the note
/// names that mode's refresh; elsewhere the display keeps its own, and the display can change, so
/// the note names the refresh it is about. Wayland's Fullscreen is the compositor's and keeps the
/// refresh too. Under KMS (`kms`) the video planes switch to a mode of the connector's
/// (`kms::plan::pick_mode`), which the settings screen can't list (SDL's list says nothing of
/// interlaced modes), so there is no note. Nothing without a reported refresh.
pub(in crate::ui) fn fractional_vsync_display_reason(s: &Settings, display: &Display, fullscreen_modes: &[FullscreenMode], windows: bool, kms: bool) -> Option<String> {
    use nightjar_policy::vsync::rate_multiple;
    if kms || !display.refresh_reported || display.refresh_hz <= 0.0 {
        return None;
    }
    let (w, h, fps, _) = crate::stream_types::stream_mode(s, *display);
    let switched = (windows && s.window_mode == WindowMode::Fullscreen)
        .then(|| closest_fullscreen_mode(fullscreen_modes, w as i32, h as i32, fps as f32, display.refresh_hz))
        .flatten()
        .map(|mode| mode.refresh_hz)
        .filter(|&hz| hz > 0.0 && (hz - display.refresh_hz).abs() > 0.01);
    let refresh_hz = switched.unwrap_or(display.refresh_hz);
    if let Some(2..) = rate_multiple(f64::from(refresh_hz), f64::from(fps)) {
        return None;
    }
    let shown = refresh_hz.round() as u32;
    Some(match switched {
        Some(_) => trf("Not used at the {} Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate", &[&shown]),
        None => trf("Not used at this display’s {} Hz: that isn’t 2 to 4 times the frame rate", &[&shown]),
    })
}

/// SDL's video driver, which only Linux asks for.
pub(in crate::ui) fn video_driver() -> String {
    #[cfg(target_os = "linux")]
    return (crate::platform_hooks::hooks().video_driver)();
    #[cfg(not(target_os = "linux"))]
    String::new()
}

/// The choice the saved keys make, on this platform.
pub(in crate::ui) fn vrr_choice(s: &Settings) -> VrrChoice {
    vrr_choice_where(s, vrr_tear_free_applies())
}

/// The choice the saved keys make where `vrr_tear_free` applies, or where it does not (only the
/// game's timing then tells On from Fastest).
pub(in crate::ui) fn vrr_choice_where(s: &Settings, tear_free_applies: bool) -> VrrChoice {
    match (s.vrr, s.present_at_host_times, s.vrr_tear_free) {
        (false, ..) => VrrChoice::Off,
        (true, true, true) => VrrChoice::On,
        (true, false, false) => VrrChoice::Fastest,
        (true, host_times, _) if !tear_free_applies => if host_times { VrrChoice::On } else { VrrChoice::Fastest },
        (true, ..) => VrrChoice::Custom,
    }
}

/// Saves a choice into the three keys: On and Fastest write all three, Off only `vrr`, so the
/// others come back with VRR. Custom writes nothing.
pub(in crate::ui) fn set_vrr_choice(s: &mut Settings, choice: VrrChoice) {
    match choice {
        VrrChoice::Off => s.vrr = false,
        VrrChoice::On => (s.vrr, s.present_at_host_times, s.vrr_tear_free) = (true, true, true),
        VrrChoice::Fastest => (s.vrr, s.present_at_host_times, s.vrr_tear_free) = (true, false, false),
        VrrChoice::Custom => {}
    }
}

/// "Frame pacing" as one choice over its three keys, which settings.json keeps as they are:
/// `frame_pacing`, `drop_late_frames` (Balanced's newest frame) and `host_frame_times` (the PC's
/// frame times, which take over from the pacing). Android's `FramePacingChoice`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum FramePacingChoice {
    Latency,
    /// Balanced, the newest picture at each refresh.
    Balanced,
    /// Balanced, each picture in turn.
    BalancedEvery,
    CapFps,
    Smoothness,
    HostTimes,
}

impl FramePacingChoice {
    /// The choices the list offers, in Android's order.
    pub(in crate::ui) const ALL: [FramePacingChoice; 6] = [FramePacingChoice::Latency, FramePacingChoice::Balanced, FramePacingChoice::BalancedEvery,
        FramePacingChoice::CapFps, FramePacingChoice::Smoothness, FramePacingChoice::HostTimes];
}

/// The choice the saved keys make: the PC's frame times whatever pacing is under them, and
/// Balanced's newest-frame key only with Balanced.
pub(in crate::ui) fn frame_pacing_choice(s: &Settings) -> FramePacingChoice {
    if s.host_frame_times { return FramePacingChoice::HostTimes; }
    match s.frame_pacing {
        FramePacing::Latency => FramePacingChoice::Latency,
        FramePacing::Balanced if s.drop_late_frames => FramePacingChoice::Balanced,
        FramePacing::Balanced => FramePacingChoice::BalancedEvery,
        FramePacing::CapFps => FramePacingChoice::CapFps,
        FramePacing::Smoothness => FramePacingChoice::Smoothness,
    }
}

/// Saves a choice into the three keys: the PC's frame times leave the pacing under them for when
/// another choice comes back, and only Balanced's two choices write its newest-frame key.
pub(in crate::ui) fn set_frame_pacing_choice(s: &mut Settings, choice: FramePacingChoice) {
    s.host_frame_times = choice == FramePacingChoice::HostTimes;
    match choice {
        FramePacingChoice::Latency => s.frame_pacing = FramePacing::Latency,
        FramePacingChoice::Balanced => (s.frame_pacing, s.drop_late_frames) = (FramePacing::Balanced, true),
        FramePacingChoice::BalancedEvery => (s.frame_pacing, s.drop_late_frames) = (FramePacing::Balanced, false),
        FramePacingChoice::CapFps => s.frame_pacing = FramePacing::CapFps,
        FramePacingChoice::Smoothness => s.frame_pacing = FramePacing::Smoothness,
        FramePacingChoice::HostTimes => {}
    }
}

pub(in crate::ui) fn frame_pacing_choice_name(choice: FramePacingChoice) -> &'static str {
    match choice {
        FramePacingChoice::Latency => tr(FramePacing::Latency.label()),
        FramePacingChoice::Balanced => tr(FramePacing::Balanced.label()),
        FramePacingChoice::BalancedEvery => tr("Balanced, keep every frame"),
        FramePacingChoice::CapFps => tr(FramePacing::CapFps.label()),
        FramePacingChoice::Smoothness => tr(FramePacing::Smoothness.label()),
        FramePacingChoice::HostTimes => tr("PC’s frame times"),
    }
}

pub(super) fn vrr_choice_name(choice: VrrChoice) -> &'static str {
    match choice {
        VrrChoice::Off => tr("Off"),
        VrrChoice::On => tr("On"),
        VrrChoice::Fastest => tr("Fastest"),
        VrrChoice::Custom => tr("Custom"),
    }
}

/// Video position: the picture at the window's centre or its top (`align_video_top`).
pub(super) fn video_position_name(top: bool) -> &'static str {
    if top { tr("Top") } else { tr("Center") }
}

/// A Minimum resolution choice by its shorter side, as Android lists it: 720p.
pub(super) fn short_edge_name(edge: u32) -> String {
    format!("{edge}p")
}
