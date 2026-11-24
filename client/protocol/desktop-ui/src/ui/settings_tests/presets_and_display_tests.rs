//! Settings tests for presets, frame pacing, microphone options, V-Sync, and display arrangement choices.

use super::*;

/// Presets sit above the main settings' rows: the defaults read Native, a chip asks before it
/// replaces the stream settings and sets them on OK, and a changed value reads Custom. A PC's
/// settings and a search have none.
#[test]
fn presets_ask_first_set_their_values_and_read_custom_once_changed() {
    let mut f = settings_with(Settings::default());
    assert!(shows(&f, "Presets") && shows(&f, "Native (recommended)") && shows(&f, "Battery saver"), "{:?}", f.drawn);
    assert!(shows(&f, "Native"), "the defaults are Native: {:?}", f.drawn);
    click_text(&mut f, "Battery saver");
    assert!(matches!(f.l.dialog, Dialog::ConfirmPreset(Preset::BatterySaver)));
    assert!(shows(&f, "This replaces resolution, frame rate, bitrate, frame pacing, upscaling, VRR and performance settings with this preset’s values. Codec and HDR choices stay as they are."));
    click_text(&mut f, "Cancel");
    assert!(matches!(f.l.dialog, Dialog::None) && f.l.settings == Settings::default(), "Cancel changes nothing");
    f.l.settings.codec = CodecPreference::Hevc;
    f.l.settings.hdr = true;
    f.settle();
    click_text(&mut f, "Battery saver");
    let commands = click_text(&mut f, "OK");
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "saved at once");
    let s = &f.l.settings;
    assert_eq!((s.width, s.height, s.fps, s.bitrate_kbps, s.frame_pacing), (0, 0, 30, 0, crate::config::FramePacing::CapFps));
    assert!(s.codec == CodecPreference::Hevc && s.hdr, "codec and HDR stay");
    f.settle();
    assert!(shows(&f, "Battery saver") && !shows(&f, "Custom"), "{:?}", f.drawn);
    f.l.settings.bitrate_kbps = 20_000;
    f.settle();
    assert!(shows(&f, "Custom"), "{:?}", f.drawn);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    assert!(!shows(&f, "Presets"), "a search shows rows only");
    f.l.settings_view.query.clear();
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Presets"), "a PC's settings have none");
}

/// The presets sit only on the sections whose rows they change, and their chips are one size
/// whichever is chosen.
#[test]
fn presets_show_only_on_the_sections_they_change_with_chips_of_one_size() {
    let changed: Vec<&str> = SETTINGS_SECTIONS.iter().copied().filter(|s| preset_section(s)).collect();
    assert_eq!(changed, ["Video", "Display & image", "Debug"]);
    let mut f = settings_with(Settings::default());
    let height = |f: &Frames, label: &str| {
        f.placed.iter().find(|(t, ..)| t == label).map(|p| p.1.height()).unwrap_or_else(|| panic!("{label}: {:?}", f.drawn))
    };
    assert_eq!(height(&f, "Native (recommended)"), height(&f, "Battery saver"), "the chosen chip's type is the other's");
    for section in ["Controls", "Audio", "Streaming", "App", "Advanced"] {
        f.l.settings_view.section = SETTINGS_SECTIONS.iter().copied().find(|s| *s == section).unwrap();
        f.settle();
        assert!(!shows(&f, "Presets") && !shows(&f, "Battery saver"), "{section}: {:?}", f.drawn);
    }
    f.l.settings_view.section = SETTINGS_SECTIONS[1];
    f.settle();
    assert!(shows(&f, "Presets"), "{:?}", f.drawn);
}

/// Native puts back what the presets set, the desktop's defaults among them, and turns on every
/// latency switch the desktop has; Battery saver leaves those. VRR the screen can't use never
/// makes a preset Custom, as on Android.
#[test]
fn a_preset_sets_and_reads_the_desktops_settings() {
    let mut s = Settings {
        width: 1280, height: 720, fps: 144, bitrate_kbps: 50_000, vrr: false,
        upscaling: crate::config::Upscaling::Fsr1, frame_pacing: crate::config::FramePacing::Smoothness,
        drop_late_frames: false, unbatched_input: false, network_priority: false, host_frame_times: true,
        codec: CodecPreference::Av1, prefer_yuv444: true,
        ..Settings::default()
    };
    assert_eq!(presets::chosen(&preset_current(&s, true)), None);
    // The PC's frame times are no preset's pacing, whatever pacing is under them.
    let host_times = Settings { host_frame_times: true, ..Settings::default() };
    assert_eq!(presets::chosen(&preset_current(&host_times, true)), None);
    apply_preset(&mut s, Preset::Native);
    assert_eq!(presets::chosen(&preset_current(&s, true)), Some(Preset::Native));
    assert_eq!(s, Settings { codec: CodecPreference::Av1, prefer_yuv444: true, ..Settings::default() }, "the rest are the defaults");
    assert_eq!(presets::chosen(&preset_current(&Settings::default(), true)), Some(Preset::Native), "the defaults are Native");
    s.unbatched_input = false;
    apply_preset(&mut s, Preset::BatterySaver);
    assert_eq!(presets::chosen(&preset_current(&s, true)), Some(Preset::BatterySaver));
    assert!(!s.unbatched_input, "Battery saver leaves the latency switches");
    s.vrr = false;
    assert_eq!(presets::chosen(&preset_current(&s, true)), None, "VRR off where it is offered is Custom");
    assert_eq!(presets::chosen(&preset_current(&s, false)), Some(Preset::BatterySaver), "VRR the screen lacks reads as its default");
    assert_eq!(preset_value(Preset::Native), "Native");
    assert_eq!(preset_value(Preset::BatterySaver), "Battery saver");
}

/// Frame pacing is one row over its three keys, as on Android: Balanced's newest frame and the
/// PC's frame times are among its choices. Every saved combination reads as the pacing a stream
/// runs, and saving that choice back changes nothing a stream uses.
#[test]
fn frame_pacing_reads_and_writes_its_three_keys_as_one_choice() {
    use crate::config::FramePacing;
    for pacing in FramePacing::ALL {
        for drop_late_frames in [true, false] {
            for host_frame_times in [true, false] {
                let s = Settings { frame_pacing: pacing, drop_late_frames, host_frame_times, ..Settings::default() };
                let choice = frame_pacing_choice(&s);
                let expected = match (host_frame_times, pacing, drop_late_frames) {
                    (true, ..) => FramePacingChoice::HostTimes,
                    (_, FramePacing::Latency, _) => FramePacingChoice::Latency,
                    (_, FramePacing::Balanced, true) => FramePacingChoice::Balanced,
                    (_, FramePacing::Balanced, false) => FramePacingChoice::BalancedEvery,
                    (_, FramePacing::CapFps, _) => FramePacingChoice::CapFps,
                    (_, FramePacing::Smoothness, _) => FramePacingChoice::Smoothness,
                };
                assert_eq!(choice, expected, "{pacing:?} {drop_late_frames} {host_frame_times}");
                let mut saved = s.clone();
                set_frame_pacing_choice(&mut saved, choice);
                assert_eq!(saved, s, "saving {choice:?} back changes nothing");
                let paced = |s: &Settings| (crate::stream_types::without_vrr(s), s.host_frame_times);
                assert_eq!(paced(&saved), paced(&s));
            }
        }
    }
    for choice in FramePacingChoice::ALL {
        let mut s = Settings { frame_pacing: FramePacing::Smoothness, host_frame_times: true, ..Settings::default() };
        set_frame_pacing_choice(&mut s, choice);
        assert_eq!(frame_pacing_choice(&s), choice);
    }
    // The PC's frame times leave the pacing under them; another pacing leaves Balanced's choice.
    let mut s = Settings { frame_pacing: FramePacing::Balanced, drop_late_frames: false, ..Settings::default() };
    set_frame_pacing_choice(&mut s, FramePacingChoice::HostTimes);
    assert_eq!((s.frame_pacing, s.drop_late_frames), (FramePacing::Balanced, false));
    set_frame_pacing_choice(&mut s, FramePacingChoice::Smoothness);
    assert!(!s.host_frame_times && !s.drop_late_frames);
    // Its value line names the choice, under each of its keys.
    let host = Settings { host_frame_times: true, ..Settings::default() };
    for key in ["frame_pacing", "drop_late_frames", "host_frame_times"] {
        assert_eq!(value_text(key, &host, &crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true }, crate::glyphs::Style::Xbox), "PC’s frame times");
    }
}

/// The row shows while VRR is off, with no row of its own for Balanced's newest frame; V-Sync, one
/// row with its even cadence, stays beside a pacing that waits for the refresh and the PC's frame times.
#[test]
fn frame_pacing_is_one_row_and_the_pcs_frame_times_wait_for_the_refresh() {
    for (vrr, host_frame_times, pacing, frame_pacing_shown, vsync_shown) in [
        (false, false, crate::config::FramePacing::Latency, true, true),
        (false, false, crate::config::FramePacing::Balanced, true, true),
        (false, true, crate::config::FramePacing::Latency, true, true),
        (true, true, crate::config::FramePacing::Latency, false, true),
    ] {
        let mut f = settings_with(Settings { vrr, host_frame_times, frame_pacing: pacing, ..Settings::default() });
        f.l.tearing_supported = true;
        open_folds(&mut f);
        f.l.settings_view.section = "Display & image";
        f.settle();
        let what = format!("VRR {vrr} host {host_frame_times} {pacing:?}: {:?}", f.drawn);
        assert_eq!(shows(&f, "Frame pacing"), frame_pacing_shown, "{what}");
        assert_eq!(shows(&f, "V-Sync"), vsync_shown, "{what}");
        assert!(!shows(&f, "Keep only the newest frame"), "{what}");
        if frame_pacing_shown {
            assert!(setting_keys(&f).contains(&"frame_pacing"), "{what}");
            assert!(!setting_keys(&f).contains(&"drop_late_frames"), "a key of the Frame pacing row, not a row: {what}");
        }
    }
}

/// Noise suppression and Automatic gain follow the microphone, after Echo cancellation.
#[test]
fn the_microphones_noise_suppression_and_gain_follow_it() {
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    f.l.settings_view.section = "Audio";
    f.settle();
    assert!(!shows(&f, "Noise suppression") && !shows(&f, "Automatic gain"), "{:?}", f.drawn);
    f.l.settings.microphone = true;
    f.settle();
    let keys = setting_keys(&f);
    let at = keys.iter().position(|k| *k == "mic_echo_cancel").unwrap();
    assert_eq!(&keys[at + 1..at + 3], ["mic_noise_suppression", "mic_auto_gain"], "{keys:?}");
    assert!(Settings::default().mic_noise_suppression && !Settings::default().mic_auto_gain, "Android's defaults");
}

/// App: Theme first; Check for updates now where this copy checks; Use small box art is the
/// card size; About opens its page. Advanced: the decode time, the latency benchmark and the
/// latency CSV, on the main settings only.
#[test]
fn the_app_and_diagnostics_rows_from_android_do_what_they_say() {
    let mut f = settings_read_with(Settings::default());
    open_folds(&mut f);
    f.l.settings_view.section = "App";
    f.settle();
    assert_eq!(setting_keys(&f).first(), Some(&"theme"), "Theme comes first");
    assert_eq!(row_words(&f, "Theme"), ["Dark"]);
    assert_eq!(shows(&f, "Check for updates now"), (crate::platform_hooks::hooks().self_update_checks)());
    if (crate::platform_hooks::hooks().self_update_checks)() {
        let commands = click_text(&mut f, "Check now");
        assert!(matches!(commands[..], [Command::CheckUpdatesNow]), "{} commands", commands.len());
    }
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Medium);
    let small_box_art = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.label() == Some("Use small box art") && n.toggled().is_some())
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the Use small box art switch")
    };
    let (toggled, bounds) = small_box_art(&f);
    assert_eq!(toggled, Some(egui::accesskit::Toggled::False));
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Small, "on is Home's Small cards");
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Medium, "off is Medium");
    f.l.settings.home_card_size = crate::config::CardSize::Large;
    f.settle();
    assert_eq!(small_box_art(&f).0, Some(egui::accesskit::Toggled::False), "Large is not small");
    f.l.settings.home_card_size = crate::config::CardSize::Medium;
    f.settle();
    let about = f.placed.iter().find(|(t, ..)| t == "About").expect("the About row").1;
    let open = f.placed.iter().find(|(t, r, _)| t == "Open" && (r.center().y - about.center().y).abs() < 24.0).expect("its button").1.center();
    f.click_at(open, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::About);
    assert!(shows(&f, &format!("Version {} · Based on Moonlight (GPL-3.0)", crate::VERSION)) && shows(&f, "Source and licenses"), "{:?}", f.drawn);
    assert!(shows(&f, "Moonlight Android") && shows(&f, "Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy and contributors · GPL-3.0"), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Source and licenses");
    assert!(matches!(commands[..], [Command::OpenSource]), "{} commands", commands.len());
    f.l.back(&mut Vec::new(), false);
    f.settle();
    assert_eq!(f.l.screen, Screen::Settings);

    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(setting_keys(&f).contains(&"post_stream_decode_time"), "{:?}", setting_keys(&f));
    let commands = click_text(&mut f, "Export");
    assert!(matches!(commands[..], [Command::ExportLatencyCsv]), "{} commands", commands.len());
    let bench = f.placed.iter().find(|(t, ..)| t == "Latency benchmark").expect("the row").1;
    let open = f.placed.iter().find(|(t, r, _)| t == "Open" && (r.center().y - bench.center().y).abs() < 24.0).expect("its button").1.center();
    f.click_at(open, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::LatencyBenchmark);

    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(!shows(&f, "Show decode time after streaming") && !shows(&f, "Latency benchmark") && !shows(&f, "Export latency CSV"), "{:?}", f.drawn);
}

/// A game's own settings offer the PC's image enhancements as the PC's own settings do, and
/// only when its host reports them (NvidiaEnhancements).
#[test]
fn a_games_settings_offer_the_rtx_enhancements_its_host_reports() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    let at = SECTION_KEYS.iter().position(|keys| keys.iter().any(|k| k[0] == "rtx_hdr")).unwrap();
    let rows = |f: &mut Frames, page: Screen| {
        f.l.go(Screen::Apps(id));
        if page == Screen::HostSettings(id) { f.l.open_host_settings(id, &f.hosts.list[0]) } else { f.l.open_app_settings(id, &f.hosts.list[0], game(7)) }
        f.l.settings_view.section = SETTINGS_SECTIONS[at];
        open_folds(f);
        f.settle();
        let keys = setting_keys(f);
        (keys.contains(&"rtx_hdr"), keys.contains(&"rtx_video"))
    };
    let mut info = answer(true, 0);
    info.capabilities = nightjar_policy::http::HostCapabilities::from_evidence(nightjar_policy::http::Evidence { nvidia_enhancements: 0, ..Default::default() });
    f.hosts.list[0].info = Some(info.clone());
    assert_eq!(rows(&mut f, Screen::AppSettings(id)), (false, false), "a host without them offers neither");
    info.capabilities = nightjar_policy::http::HostCapabilities::from_evidence(nightjar_policy::http::Evidence { nvidia_enhancements: rubylight_protocol::nvidia_enhance::ALL, ..Default::default() });
    f.hosts.list[0].info = Some(info);
    for page in [Screen::HostSettings(id), Screen::AppSettings(id)] {
        assert_eq!(rows(&mut f, page), (true, true), "{page:?} offers RTX HDR and RTX Video");
    }
}

/// V-Sync's even cadence says why where it holds nothing (client/AGENTS.md): VRR, the PC's
/// frame times, and a desktop that tells no time a picture was shown.
#[test]
fn the_even_cadence_says_why_where_it_holds_nothing() {
    let mut f = settings_with(Settings { vsync: true, vrr: false, ..Settings::default() });
    f.l.tearing_supported = true;
    open_folds(&mut f);
    in_section(&mut f, "Display & image");
    assert!(shows(&f, "V-Sync") && !shows(&f, "Fractional V-Sync"), "{:?}", f.drawn);
    let desktop_note = !fractional_vsync_applies(&video_driver());
    assert_eq!(shows(&f, "Not used: this desktop doesn’t report when pictures are shown"), desktop_note, "{:?}", f.drawn);
    f.l.settings.host_frame_times = true;
    f.settle();
    assert!(shows(&f, "V-Sync") && shows(&f, "Not used with PC’s frame times"), "{:?}", f.drawn);
    f.l.settings.host_frame_times = false;
    f.l.settings.vrr = true;
    f.settle();
    assert!(shows(&f, "V-Sync") && shows(&f, "Not used with Variable refresh rate"), "{:?}", f.drawn);
    f.l.settings.vrr = false;
    f.l.settings.vsync = false;
    f.settle();
    assert!(shows(&f, "V-Sync") && !shows(&f, "Not used: this desktop doesn’t report when pictures are shown"), "V-Sync off: no cadence note: {:?}", f.drawn);
}

/// One list over the two stored fields: Off, On, On with an even cadence.
#[test]
fn v_sync_is_one_choice_over_the_two_stored_fields() {
    let mut s = Settings { vsync: true, fractional_vsync: false, ..Settings::default() };
    assert_eq!(vsync_choice(&s), VsyncChoice::On);
    set_vsync_choice(&mut s, VsyncChoice::EvenCadence);
    assert!(s.vsync && s.fractional_vsync);
    assert_eq!(vsync_choice(&s), VsyncChoice::EvenCadence);
    set_vsync_choice(&mut s, VsyncChoice::Off);
    assert!(!s.vsync && s.fractional_vsync, "Off keeps the cadence field");
    assert_eq!(vsync_choice(&s), VsyncChoice::Off, "V-Sync off wins over a stored cadence");
    set_vsync_choice(&mut s, VsyncChoice::On);
    assert!(s.vsync && !s.fractional_vsync);
    // Stored files read as before: both fields keep their names and meaning.
    let old: Settings = serde_json::from_str(r#"{"vsync":true,"fractional_vsync":true}"#).unwrap();
    assert_eq!(vsync_choice(&old), VsyncChoice::EvenCadence);
    let off: Settings = serde_json::from_str(r#"{"vsync":false,"fractional_vsync":true}"#).unwrap();
    assert_eq!(vsync_choice(&off), VsyncChoice::Off);
    assert_eq!(VSYNC_CHOICES.map(vsync_name), ["Off", "On", "On, even cadence"]);
}

/// The display's refresh has to be 2 to 4 times the frame rate, and Windows' Fullscreen without VRR
/// switches the display to the stream's rate where it has a mode at it.
#[test]
fn fractional_vsync_names_the_refresh_where_it_holds_nothing() {
    let at = |hz: f32| crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: hz, refresh_reported: true };
    let s = |fps: u32, window_mode: WindowMode| Settings { fps, window_mode, ..Settings::default() };
    let borderless = WindowMode::Borderless;
    assert_eq!(fractional_vsync_display_reason(&s(60, borderless), &at(120.0), &[], true, false), None, "twice");
    assert_eq!(fractional_vsync_display_reason(&s(30, borderless), &at(119.88), &[], true, false), None, "four times, within the tolerance");
    assert_eq!(fractional_vsync_display_reason(&s(60, borderless), &at(144.0), &[], true, false).as_deref(),
        Some("Not used at this display’s 144 Hz: that isn’t 2 to 4 times the frame rate"));
    assert!(fractional_vsync_display_reason(&s(60, borderless), &at(60.0), &[], true, false).is_some(), "the stream's own rate");
    assert!(fractional_vsync_display_reason(&s(0, borderless), &at(120.0), &[], true, false).is_some(), "Display: the display's rate");
    assert!(fractional_vsync_display_reason(&s(24, borderless), &at(120.0), &[], true, false).is_some(), "five times is past the hold's 4");
    let unreported = crate::stream_types::Display { refresh_reported: false, ..at(144.0) };
    assert_eq!(fractional_vsync_display_reason(&s(60, borderless), &unreported, &[], true, false), None, "no refresh known");

    // Windows' Fullscreen takes the mode closest to the stream's size and rate (SDL's order).
    let full = WindowMode::Fullscreen;
    let modes = |w: i32, h: i32, rates: &[f32]| rates.iter().map(|&refresh_hz| FullscreenMode { w, h, refresh_hz, bytes_per_pixel: 4 }).collect::<Vec<_>>();
    let fhd = |rates: &[f32]| modes(1920, 1080, rates);
    let reason = |fps, desktop: f32, list: &[FullscreenMode], windows, kms| {
        fractional_vsync_display_reason(&s(fps, full), &at(desktop), list, windows, kms)
    };
    assert_eq!(reason(60, 120.0, &fhd(&[120.0, 60.0]), true, false).as_deref(),
        Some("Not used at the 60 Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate"));
    assert_eq!(reason(30, 120.0, &fhd(&[120.0, 60.0]), true, false), None, "no 30 Hz mode: the closest, 60 Hz, holds 2");
    assert_eq!(reason(30, 144.0, &fhd(&[144.0, 120.0, 60.0]), true, false), None, "60 Hz, the closest to 30, holds 2");
    assert_eq!(reason(40, 120.0, &fhd(&[120.0, 60.0, 50.0]), true, false).as_deref(),
        Some("Not used at the 50 Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate"), "50 Hz, the closest to 40");
    assert_eq!(reason(50, 144.0, &fhd(&[60.0, 40.0]), true, false).as_deref(),
        Some("Not used at the 40 Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate"), "a tie goes to the later, slower mode");
    assert_eq!(reason(60, 120.0, &fhd(&[120.0]), true, false), None, "the closest is the desktop's own 120 Hz, which holds 2");
    // The size comes first: the smallest that holds the stream, its rates alone.
    let sizes = [modes(2560, 1440, &[144.0, 60.0, 30.0]), fhd(&[60.0])].concat();
    assert_eq!(reason(30, 144.0, &sizes, true, false), None, "1080p's 60 Hz, not 1440p's 30 Hz");
    // No mode holds the stream: SDL keeps the desktop's mode.
    let small = modes(1280, 720, &[60.0]);
    assert_eq!(reason(60, 144.0, &small, true, false).as_deref(), Some("Not used at this display’s 144 Hz: that isn’t 2 to 4 times the frame rate"));
    assert_eq!(reason(60, 120.0, &small, true, false), None);
    assert_eq!(reason(60, 120.0, &fhd(&[120.0, 60.0]), false, false), None, "Wayland's Fullscreen keeps the display's refresh");
    // KMS's planes switch to a mode of the connector's, which the settings can't list.
    let borderless_kms = fractional_vsync_display_reason(&s(60, borderless), &at(144.0), &fhd(&[144.0, 120.0]), false, true);
    assert_eq!(borderless_kms, None, "KMS: 144 Hz now, but the planes may switch to 120 Hz and hold");
}

/// SDL's own choice (`SDL_GetClosestFullscreenDisplayMode`), mode for mode.
#[test]
fn the_closest_fullscreen_mode_is_sdls() {
    let m = |w, h, refresh_hz, bytes_per_pixel| FullscreenMode { w, h, refresh_hz, bytes_per_pixel };
    let list = [m(3840, 2160, 60.0, 4), m(2560, 1440, 144.0, 4), m(2560, 1440, 60.0, 4), m(1920, 1080, 120.0, 4), m(1920, 1080, 60.0, 4), m(1920, 1080, 60.0, 2)];
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1080, 60.0, 144.0), Some(list[4]), "the deeper format of two at 60 Hz");
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1080, 90.0, 144.0), Some(list[4]), "equally close: the later");
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1080, 0.0, 144.0), Some(list[3]), "0 asks for the desktop's rate");
    assert_eq!(closest_fullscreen_mode(&list, 2000, 1100, 60.0, 144.0), Some(list[2]), "the smallest size that holds it");
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1200, 60.0, 144.0), Some(list[2]), "taller than 1080: 1440p");
    assert_eq!(closest_fullscreen_mode(&list, 5120, 2880, 60.0, 144.0), None, "none holds it");
    // A wider ratio's mode that holds the stream gives way to one closer to the stream's.
    let ratios = [m(3440, 1440, 100.0, 4), m(2560, 1440, 144.0, 4)];
    assert_eq!(closest_fullscreen_mode(&ratios, 2560, 1440, 60.0, 144.0), Some(ratios[1]));
}

#[test]
fn fractional_vsync_is_offered_only_where_it_holds_pictures() {
    for driver in ["windows", "wayland", "kmsdrm", "x11", "cocoa"] {
        let expected = cfg!(windows) || (cfg!(target_os = "linux") && driver != "x11");
        assert_eq!(fractional_vsync_applies(driver), expected, "{driver}");
    }
    if cfg!(target_os = "linux") {
        assert!(!fractional_vsync_applies("x11"), "X11 tells no time a picture was shown");
        assert!(fractional_vsync_applies("wayland"));
    }
}

#[test]
fn display_arrangement_shows_every_saved_pair_as_the_choice_that_streams_the_same() {
    use crate::config::HostDisplay;
    let sent = |(mode, virtual_display): (HostDisplay, bool)| nightjar_policy::http::HostDisplay::from(mode).requests_virtual_display(virtual_display);
    for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary, HostDisplay::Extended,
        HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
        for virtual_display in [false, true] {
            let s = Settings { host_display: mode, virtual_display, ..Settings::default() };
            // With display modes the arrangement is sent, and the switch only counts with the PC's own setting.
            let shown = display_choice(&s, true);
            assert!(DISPLAY_CHOICES.contains(&shown), "{mode:?}/{virtual_display}");
            assert_eq!(shown.0, mode, "{mode:?}/{virtual_display}");
            assert_eq!(sent(shown), sent((mode, virtual_display)), "{mode:?}/{virtual_display}");
            // Without display modes only the switch is sent.
            assert_eq!(display_choice(&s, false), (HostDisplay::HostDefault, virtual_display));
        }
    }
    // Each choice reads back as itself; only the PC's own setting and Virtual display set the switch.
    for before in [false, true] {
        for choice in DISPLAY_CHOICES {
            let mut s = Settings { virtual_display: before, ..Settings::default() };
            pick_display_choice(&mut s, choice);
            assert_eq!(display_choice(&s, true), choice);
            assert_eq!(s.virtual_display, if choice.0 == HostDisplay::HostDefault { choice.1 } else { before });
        }
    }
    assert_eq!(display_choice_name((HostDisplay::HostDefault, true)), "Virtual display");
    assert_eq!(display_choice_name((HostDisplay::HostDefault, false)), "The PC’s own setting");
}
