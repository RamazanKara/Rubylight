use super::{first_frame_line, window_needs_a_frame};

#[test]
fn hdr_enabled_at_startup_still_prepares_hdr10_for_the_first_stream() {
    use crate::handheld::{self, Output, RendererNow, RendererSwitch};
    let settings = crate::config::Settings { hdr: true, ..Default::default() };
    for choice in [crate::config::Renderer::Auto, crate::config::Renderer::Gpu] {
        let state = super::RendererState::for_launcher(choice, true, None);
        assert!(state.vulkan_video);
        let now = RendererNow {
            gpu: true, hdr_swapchain: true, made_for_hdr: state.made_for_hdr,
            gpu_failed: state.gpu_failed.is_some(),
            pq_unused: handheld::pq_unused(false, Some(Ok(Output::Scrgb)), true, false),
            hdr_unreachable: false,
        };
        assert_eq!(handheld::renderer_switch(state.choice, now, settings.gpu_need(), settings.hdr, true, false),
            Some(RendererSwitch::ForHdr), "the launcher's scRGB output has never tried PQ");
        assert_eq!(handheld::renderer_switch(state.choice, now, None, false, true, false), None,
            "SDR streams keep the launcher's renderer");
        let prepared = RendererNow { made_for_hdr: true, ..now };
        assert_eq!(handheld::renderer_switch(state.choice, prepared, settings.gpu_need(), settings.hdr, true, false), None,
            "a prepared HDR stream does not rebuild again");
    }
}

/// A PC, game or profile that turns the compute queue on asks for it before the renderer, as HDR does.
#[test]
fn any_layer_turning_on_the_compute_queue_asks_for_it() {
    let settings = crate::config::Settings::default();
    let none: Vec<crate::config::Overrides> = vec![crate::config::Overrides::default()];
    assert!(!super::any_async_compute_asked(&settings, &none));
    let own: crate::config::Overrides = serde_json::from_str("{\"pyrowave_async_compute\": true}").unwrap();
    assert!(super::any_async_compute_asked(&settings, &[own]));
    let global = crate::config::Settings { pyrowave_async_compute: true, ..Default::default() };
    assert!(super::any_async_compute_asked(&global, &none));
}

/// A launcher at rest presents nothing, but what the window itself lost has to come back.
#[test]
fn the_window_changing_asks_for_a_frame_and_a_worker_or_the_pointer_does_not() {
    for kind in [SDL_EVENT_WINDOW_EXPOSED, SDL_EVENT_WINDOW_RESIZED, SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED, SDL_EVENT_WINDOW_RESTORED, SDL_EVENT_RENDER_TARGETS_RESET, SDL_EVENT_RENDER_DEVICE_LOST] {
        assert!(window_needs_a_frame(kind), "{}", kind.0);
    }
    for kind in [SDL_EVENT_MOUSE_MOTION, SDL_EVENT_USER, SDL_EVENT_KEY_DOWN] {
        assert!(!window_needs_a_frame(kind), "{}", kind.0);
    }
}

#[test]
fn the_launch_line_comes_once_with_the_time() {
    let said = std::sync::atomic::AtomicBool::new(false);
    assert_eq!(first_frame_line(&said, None), None, "no start, no number");
    let line = first_frame_line(&said, Some(std::time::Duration::from_millis(412)));
    assert_eq!(line.as_deref(), Some("nightjar: launch to first frame 412 ms"));
    assert_eq!(first_frame_line(&said, Some(std::time::Duration::from_secs(9))), None, "once");
}

/// Stream here too on a resume joins; a PC that did not answer never does.
#[test]
fn who_plays_decides_how_a_start_goes_on() {
    use super::{select_step, LaunchKind, SelectStep};
    let tv = || Some(("Game".to_owned(), "TV".to_owned()));
    // Resume on "came back", and a selected game.
    assert_eq!(select_step(true, false, tv(), false), SelectStep::Ask("Game".into(), "TV".into()));
    assert_eq!(select_step(true, false, tv(), true), SelectStep::Ask("Game".into(), "TV".into()), "it asks first");
    assert_eq!(select_step(false, false, tv(), true), SelectStep::Ask("Game".into(), "TV".into()));
    assert_eq!(select_step(true, false, None, true), SelectStep::Join);
    assert_eq!(select_step(true, false, None, false), SelectStep::Start);
    assert_eq!(select_step(false, false, None, true), SelectStep::Start, "a plain start launches beside it");
    // Stream here too on "did not move".
    assert_eq!(select_step(true, true, tv(), false), SelectStep::Join);
    assert_eq!(select_step(true, true, None, true), SelectStep::Join);
    assert_eq!(select_step(true, true, None, false), SelectStep::Start);
    assert_eq!(LaunchKind::stream_here_too(true), LaunchKind::JoinRunning);
    assert_eq!(LaunchKind::stream_here_too(false), LaunchKind::Stream);
}

/// Vulkan video's renderer is left out only when every decoder setting a stream can use says
/// Software: a host's, an app's or a profile's override counts as the global setting does.
#[test]
fn hardware_decoding_counts_every_override_of_the_decoder_setting() {
    use super::{any_hardware_decoding, config::{DecoderPreference, Overrides, Settings}};
    let decoder = |decoder| {
        let mut own = Overrides::default();
        own.capture(&Settings { decoder, ..Default::default() }, &["decoder"]);
        own
    };
    let (software, auto) = (Settings { decoder: DecoderPreference::Software, ..Default::default() }, Settings::default());
    let other = Overrides::default();
    assert!(!any_hardware_decoding(&software, []), "Software everywhere");
    assert!(!any_hardware_decoding(&software, [&other, &decoder(DecoderPreference::Software)]), "overrides that leave it Software");
    assert!(any_hardware_decoding(&software, [&other, &decoder(DecoderPreference::Auto)]), "a host that decodes in hardware");
    assert!(any_hardware_decoding(&auto, []));
    assert!(any_hardware_decoding(&auto, [&decoder(DecoderPreference::Software)]), "other hosts still use the global setting");
}

/// The GPU renderer's device gets Vulkan video's extensions and decode queues, or the DMA-BUF
/// import ones VAAPI's pictures are mapped with, only when a stream can use them: on Linux,
/// with hardware decoding somewhere and `NIGHTJAR_VULKAN_VIDEO` or `NIGHTJAR_VAAPI_VULKAN_MAP`
/// not 0. Otherwise it is made as before Vulkan video (PyroWave's plan alone).
#[test]
fn the_renderer_asks_for_vulkan_video_only_when_a_stream_can_decode_with_it() {
    use super::{vulkan_video::Choice, vulkan_video_possible};
    for choice in [Choice::Default, Choice::First] {
        for map in [false, true] {
            assert_eq!(vulkan_video_possible(true, choice, map), cfg!(target_os = "linux"), "{choice:?}");
            assert!(!vulkan_video_possible(false, choice, map), "{choice:?} with Software everywhere");
        }
    }
    assert!(!vulkan_video_possible(true, Choice::Off, false), "both switched off");
    assert_eq!(vulkan_video_possible(true, Choice::Off, true), cfg!(target_os = "linux"), "NIGHTJAR_VULKAN_VIDEO=0 still maps VAAPI's pictures");
}

#[test]
fn input_only_through_a_small_stream_asks_for_the_smallest_stream_and_leaves_the_pc_alone() {
    let mut settings = config::Settings { width: 3840, height: 2160, fps: 120, bitrate_kbps: 80_000, vrr: true,
        virtual_display: true, audio: config::AudioChannels::Surround71, ..Default::default() };
    settings.prefer_yuv444 = true;
    settings.sdr_10bit = true;
    settings.upscaling = config::Upscaling::Fsr1;
    settings.frame_warp = true;
    super::small_stream(&mut settings);
    assert!(!settings.prefer_yuv444 && !settings.sdr_10bit, "a stream nobody sees offers no 4:4:4 or 10-bit");
    assert_eq!(settings.upscaling, config::Upscaling::Bilinear, "nor upscaling");
    assert!(!settings.frame_warp, "nor Frame Warp");
    assert_eq!((settings.width, settings.height, settings.fps, settings.bitrate_kbps), (640, 360, 30, 500));
    assert_eq!(settings.audio, config::AudioChannels::Stereo);
    assert!(!settings.virtual_display && !settings.vrr && settings.play_audio_on_host, "the PC's display and speakers stay as they are");
}

/// Input-only and microphone-only sessions need no GPU renderer: Linux neither switches to it
/// for them nor refuses them with Renderer set to OpenGL.
#[test]
fn a_session_nobody_sees_needs_no_gpu_renderer() {
    let mut settings = config::Settings { hdr: true, prefer_yuv444: true, sdr_10bit: true, experimental_packed_444: true,
        upscaling: config::Upscaling::Fsr1, codec: config::CodecPreference::PyroWave, frame_warp: true, ..Default::default() };
    assert!(settings.needs_gpu());
    super::input_only_settings(&mut settings);
    assert_eq!(settings.gpu_need(), None);
    assert!(!settings.frame_warp, "no picture to shift");
    assert_eq!((settings.codec, settings.width, settings.height, settings.fps), (config::CodecPreference::H264, 1280, 720, 60));
}

#[test]
fn microphone_only_sends_the_microphone_and_names_no_app() {
    let mut settings = config::Settings { microphone: false, controllers: true, clipboard_sync: true, ..Default::default() };
    super::mic_only_settings(&mut settings);
    assert!(settings.microphone, "the microphone goes, whatever the stream setting says");
    assert!(!settings.controllers && !settings.clipboard_sync && !settings.usb_forwarding);
    assert!(settings.play_audio_on_host, "the PC keeps its sound");
    let app = super::mic_only_app();
    assert_eq!(app.id, super::MIC_ONLY_APP_ID);
    assert!(!app.is_control_action());
    assert_eq!(super::LaunchKind::MicOnly.log_suffix(), ", microphone only");
    assert_eq!(super::LaunchKind::JoinRunning.log_suffix(), ", joining the running game");
    // A start made again (full chroma refused after its launch) keeps its kind.
    assert_eq!(super::LaunchKind::of_plan(false, false, false), super::LaunchKind::Stream);
    assert_eq!(super::LaunchKind::of_plan(true, false, false), super::LaunchKind::JoinRunning);
    assert_eq!(super::LaunchKind::of_plan(false, true, false), super::LaunchKind::SmallInputOnly);
    assert_eq!(super::LaunchKind::of_plan(false, false, true), super::LaunchKind::MicOnly);
    assert_eq!(super::mic_only_title(false, false, false), "Microphone on");
    assert_eq!(super::mic_only_title(true, false, false), "Microphone muted");
    // A failed microphone is not called muted, also once it was muted for the failure.
    assert_eq!(super::mic_only_title(true, true, false), "Microphone unavailable");
    // The PC plays another device's microphone; muted says more.
    assert_eq!(super::mic_only_title(false, false, true), "Microphone in use on another device");
    assert_eq!(super::mic_only_title(true, false, true), "Microphone muted");
}

/// Holding Ctrl+Alt+Shift+H or Escape doesn't flip the sheet or the menu with each repeat,
/// and Back acts on the page that is drawn: the Session page before a sheet it hides.
#[test]
fn menu_keys_ignore_repeats_and_back_leaves_the_drawn_page() {
    use super::{menu_step, MenuStep};
    use shortcuts::Action;
    let sheet = Some(Action::StreamShortcuts);
    let controls = Some(Action::StreamControls);
    assert_eq!(menu_step(sheet, false, false, false, false, false), MenuStep::ToggleSheet);
    assert_eq!(menu_step(sheet, false, true, true, false, false), MenuStep::Swallow);
    assert_eq!(menu_step(controls, false, true, false, false, false), MenuStep::Swallow);
    assert_eq!(menu_step(None, true, true, false, true, false), MenuStep::Swallow, "a held Escape");
    assert_eq!(menu_step(None, true, false, true, true, false), MenuStep::CloseSessionPage);
    assert_eq!(menu_step(controls, false, false, false, true, false), MenuStep::CloseSessionPage);
    assert_eq!(menu_step(None, true, false, true, false, false), MenuStep::CloseSheet);
    assert_eq!(menu_step(controls, false, false, true, false, false), MenuStep::CloseMenu);
    assert_eq!(menu_step(None, true, false, false, false, false), MenuStep::CloseMenu);
    assert_eq!(menu_step(Some(Action::SendClipboard), false, true, false, false, false), MenuStep::Pass);
    assert_eq!(menu_step(None, false, false, false, false, false), MenuStep::Pass);
    // Back from a page a tile opened goes to the grid; the sheet and the Session page draw over it first.
    assert_eq!(menu_step(None, true, false, false, false, true), MenuStep::ClosePage);
    assert_eq!(menu_step(None, true, false, true, false, true), MenuStep::CloseSheet);
    assert_eq!(menu_step(None, true, false, false, true, true), MenuStep::CloseSessionPage);
    assert_eq!(menu_step(controls, false, false, false, false, true), MenuStep::CloseMenu, "the controls chord closes from any page");
}

#[test]
fn the_overlay_sits_in_its_corner() {
    use config::OverlayCorner;
    assert_eq!(super::overlay_anchor(OverlayCorner::TopLeft), (egui::Align2::LEFT_TOP, egui::vec2(16.0, 16.0)));
    assert_eq!(super::overlay_anchor(OverlayCorner::BottomRight), (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0)));
    assert_eq!(super::overlay_anchor(OverlayCorner::TopRight).0, egui::Align2::RIGHT_TOP);
    assert_eq!(super::overlay_anchor(OverlayCorner::BottomLeft).0, egui::Align2::LEFT_BOTTOM);
}

use super::*;
use std::sync::Arc;

#[test]
fn exclusive_fullscreen_keeps_a_vrr_panels_top_refresh() {
    // A 240 Hz panel whose desktop runs at 120 Hz.
    let modes = [240.0, 165.0, 144.0, 120.0, 60.0, 120.0, 60.0];
    assert_eq!(fullscreen_refresh_hz(true, &modes, 60), 240.0, "a 60 fps stream gets the 240 Hz mode, not the desktop's 120");
    assert_eq!(fullscreen_refresh_hz(false, &modes, 60), 60.0, "without VRR a mode at the stream's rate");
    assert_eq!(fullscreen_refresh_hz(true, &[], 120), 120.0, "no modes known");
}

#[test]
fn native_resolution_is_the_display_in_pixels_at_any_scaling() {
    // Windows and X11: SDL gives pixels at density 1 at any scaling (4K at 200% and 175%).
    assert_eq!(mode_pixels(3840, 2160, 1.0), (3840, 2160));
    // Wayland at 200%, and at 175% through fractional scaling (3840 / 2194 points).
    assert_eq!(mode_pixels(1920, 1080, 2.0), (3840, 2160));
    assert_eq!(mode_pixels(2194, 1234, 3840.0 / 2194.0), (3840, 2160));
    // macOS: a 14" MacBook Pro looks like 1512x982 at density 2.
    assert_eq!(mode_pixels(1512, 982, 2.0), (3024, 1964));
    // No density reported counts as 1.
    assert_eq!(mode_pixels(2560, 1440, 0.0), (2560, 1440));
}

// nightjar#118: a Mac's scaled mode draws a backing store larger than the panel (a 3840x2160 Dell
// at "looks like 2560x1440" is 5120x2880), and Native asked the PC for that. The panel's own size
// from its native mode wins; without one SDL's mode stays.
#[test]
fn native_resolution_on_a_mac_is_the_panel_not_the_scaled_backing_store() {
    let dell = display_from_mode(Some((2560, 1440, 2.0, 60.0)), Some((3840, 2160)));
    assert_eq!((dell.width, dell.height), (3840, 2160));
    let built_in = display_from_mode(Some((1512, 982, 2.0, 120.0)), Some((3024, 1964)));
    assert_eq!((built_in.width, built_in.height), (3024, 1964), "the default scaled mode of a 14\" MacBook Pro");
    let scaled = display_from_mode(Some((1800, 1169, 2.0, 120.0)), Some((3024, 1964)));
    assert_eq!((scaled.width, scaled.height), (3024, 1964), "its \"more space\" mode draws 3600x2338");
    let unlisted = display_from_mode(Some((2560, 1440, 2.0, 60.0)), None);
    assert_eq!((unlisted.width, unlisted.height), (5120, 2880), "no native mode listed: SDL's mode");
    assert_eq!(dell.refresh_hz, 60.0);
}

// Regression (review): a display without a reported rate paced at the 60 Hz fallback, and the
// overlay judged frames against it and could say "60 Hz screen".
#[test]
fn the_overlay_only_gets_a_refresh_rate_the_display_reported() {
    let reported = display_from_mode(Some((2560, 1440, 1.0, 143.9)), None);
    assert_eq!((reported.refresh_hz, reported.reported_refresh_hz()), (143.9, Some(143.9)));
    for refresh in [0.0, -1.0, f32::NAN] {
        let fallback = display_from_mode(Some((2560, 1440, 1.0, refresh)), None);
        assert_eq!((fallback.refresh_hz, fallback.reported_refresh_hz()), (60.0, None), "pacing keeps 60 Hz");
    }
    let none = display_from_mode(None, None);
    assert_eq!((none.width, none.refresh_hz, none.reported_refresh_hz()), (1920, 60.0, None));
}

#[test]
fn the_decoder_checks_run_again_when_vulkan_video_first_changes() {
    let mut settings = config::Settings::default();
    let mut cache = None;
    let mut runs = 0;
    let mut probe = |settings: &config::Settings| probed_once(&mut cache, probe_key(Hardware::Vaapi, 1, settings), || { runs += 1; runs });
    assert_eq!(probe(&settings), 1);
    settings.vulkan_rfi = true;
    assert_eq!(probe(&settings), 1, "recovery does not change what decodes");
    settings.videotoolbox_rfi = true;
    assert_eq!(probe(&settings), 1, "nor does VideoToolbox's");
    settings.vulkan_video_first = true;
    assert_eq!(probe(&settings), 2, "the backend order the check decodes through changed");
    assert_eq!(probe(&settings), 2);
}

#[test]
fn the_color_probe_runs_once_per_decoder() {
    let mut cache = None;
    let mut runs = 0;
    let mut probe = |hardware| probed_once(&mut cache, hardware, || { runs += 1; (runs, format!("run {runs}")) });
    assert_eq!(probe(Hardware::None), (1, "run 1".to_owned()));
    assert_eq!(probe(Hardware::None), (1, "run 1".to_owned()), "the same decoder is not checked again");
    assert_eq!(probe(Hardware::Vaapi), (2, "run 2".to_owned()), "another decoder is");
    assert_eq!(probe(Hardware::None), (3, "run 3".to_owned()));
}

#[test]
fn a_stream_starts_with_what_became_of_4_4_4_and_10_bit_next_to_the_warning() {
    assert_eq!(start_toast(None, None, false), "Ctrl+Alt+Shift+Tab opens controls, Ctrl+Alt+Shift+Q disconnects, Ctrl+Alt+Shift+Z releases input");
    // It names the key the layout has there: the key labelled Y on QWERTZ.
    crate::shortcuts_runtime::use_layout(&[(sdl3_sys::everything::SDL_SCANCODE_Z, sdl3_sys::everything::SDLK_Y)]);
    assert_eq!(start_toast(None, None, false), "Ctrl+Alt+Shift+Tab opens controls, Ctrl+Alt+Shift+Q disconnects, Ctrl+Alt+Shift+Y releases input");
    crate::shortcuts_runtime::use_layout(&[]);
    // The hint follows the hotkeys as bound.
    shortcuts::record_hotkey("quit", &["CTRL".into(), "F9".into()]).unwrap();
    shortcuts::turn_off_hotkey("capture").unwrap();
    assert_eq!(start_toast(None, None, false), "Ctrl+Alt+Shift+Tab opens controls, Ctrl+F9 disconnects");
    shortcuts::set_hotkeys(Default::default());
    assert_eq!(start_toast(None, Some("Streaming in YUV 4:4:4."), false), "Streaming in YUV 4:4:4.");
    assert_eq!(start_toast(Some("Wi-Fi name unavailable."), Some("Streaming in YUV 4:4:4."), false), "Wi-Fi name unavailable.\nStreaming in YUV 4:4:4.");
    assert_eq!(start_toast(Some("Wi-Fi name unavailable."), None, false), "Wi-Fi name unavailable.");
}

#[test]
fn under_gamescope_the_launcher_window_opens_borderless_fullscreen() {
    let deck = start_flags(handheld::start_window(WindowMode::Windowed, true));
    assert!(deck.0 & SDL_WINDOW_FULLSCREEN.0 != 0 && deck.0 & SDL_WINDOW_BORDERLESS.0 != 0);
    assert!(deck.0 & SDL_WINDOW_HIDDEN.0 != 0, "shown after the GUI is ready");
    let windowed = start_flags(handheld::start_window(WindowMode::Windowed, false));
    assert_eq!(windowed.0, (SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_HIDDEN).0);
    // The Window setting opens the launcher as it opens its streams' window, on a desktop.
    for mode in [WindowMode::Fullscreen, WindowMode::Borderless] {
        let desktop = start_flags(handheld::start_window(mode, false));
        assert!(desktop.0 & SDL_WINDOW_FULLSCREEN.0 != 0, "{mode:?}");
        assert_eq!(desktop.0 & SDL_WINDOW_BORDERLESS.0, 0, "SDL's fullscreen has no border");
    }
    assert_eq!(stream_window_mode(WindowMode::Fullscreen, true, false), WindowMode::Borderless, "no exclusive mode under gamescope");
    assert_eq!(stream_window_mode(WindowMode::Fullscreen, false, false), WindowMode::Fullscreen);
    assert_eq!(stream_window_mode(WindowMode::Windowed, true, false), WindowMode::Windowed);
    // Straight to the display the window keeps the display's mode whatever the setting.
    for mode in [WindowMode::Fullscreen, WindowMode::Windowed, WindowMode::Borderless] {
        assert_eq!(stream_window_mode(mode, false, true), WindowMode::Borderless);
    }
}

#[test]
fn the_pointer_follows_real_controller_and_mouse_input_only() {
    // SAFETY: plain C unions, zeroed then filled by type.
    let event = |kind: SDL_EventType, fill: &dyn Fn(&mut SDL_Event)| unsafe {
        let mut e: SDL_Event = std::mem::zeroed();
        e.r#type = kind.0;
        fill(&mut e);
        e
    };
    let axis = |value: i16| event(SDL_EVENT_GAMEPAD_AXIS_MOTION, &|e| e.gaxis.value = value);
    let motion = |which: SDL_MouseID| event(SDL_EVENT_MOUSE_MOTION, &|e| e.motion.which = which);
    let button = |which: SDL_MouseID| event(SDL_EVENT_MOUSE_BUTTON_DOWN, &|e| e.button.which = which);
    assert_eq!(pointer_input(&event(SDL_EVENT_GAMEPAD_BUTTON_DOWN, &|_| {})), handheld::Input::Controller);
    assert_eq!(pointer_input(&axis(20000)), handheld::Input::Controller);
    assert_eq!(pointer_input(&axis(-20000)), handheld::Input::Controller);
    assert_eq!(pointer_input(&axis(1500)), handheld::Input::Other, "stick or trigger jitter");
    assert_eq!(pointer_input(&axis(-1500)), handheld::Input::Other);
    assert_eq!(pointer_input(&motion(SDL_MouseID(1))), handheld::Input::Mouse, "the trackpad");
    assert_eq!(pointer_input(&button(SDL_MouseID(1))), handheld::Input::Mouse);
    assert_eq!(pointer_input(&motion(SDL_TOUCH_MOUSEID)), handheld::Input::Other, "a touch tap");
    assert_eq!(pointer_input(&button(SDL_TOUCH_MOUSEID)), handheld::Input::Other);
    assert_eq!(pointer_input(&button(SDL_PEN_MOUSEID)), handheld::Input::Other);
    assert_eq!(pointer_input(&event(SDL_EVENT_MOUSE_WHEEL, &|e| e.wheel.which = SDL_TOUCH_MOUSEID)), handheld::Input::Other);
    assert_eq!(pointer_input(&event(SDL_EVENT_KEY_DOWN, &|_| {})), handheld::Input::Other);
}

#[test]
fn under_gamescope_a_stream_starts_with_start_back_and_the_hdr_note() {
    assert_eq!(start_toast(None, None, true), "Start+Back opens the Control Center");
    assert_eq!(start_toast(None, Some(HDR_OFF_NOTE), true), HDR_OFF_NOTE, "a note replaces the hint");
    assert_eq!(shortcuts::stream_controls_mask(), 1 << SDL_GAMEPAD_BUTTON_START.0 | 1 << SDL_GAMEPAD_BUTTON_BACK.0,
        "the hint's chord is the one the stream opens its menu with");
}

#[test]
fn a_launch_logs_what_asked_for_it() {
    assert_eq!(launch_line(LaunchFrom::Launcher, 5, false, false), "launch from launcher: app 5");
    assert_eq!(launch_line(LaunchFrom::Launcher, 5, true, false), "launch from launcher: app 5, resume");
    assert_eq!(
        launch_line(LaunchFrom::Replace, 7, false, true),
        "launch from launcher, quit and start: app 7, ignored: another stream is starting"
    );
    assert_eq!(launch_line(LaunchFrom::Autostream, 1, false, false), "launch from autostream: app 1");
    assert_eq!(launch_line(LaunchFrom::CommandLine, 2, true, false), "launch from command line: app 2, resume");
}

#[test]
fn a_launch_waits_for_an_app_list_being_read_again_instead_of_dropping() {
    let app = nightjar_core::http::App {
        name: "Desktop".into(),
        id: 5,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: String::new(),
        control: nightjar_core::http::AppControl::None,
        monitor_resume: false,
    };
    assert!(matches!(launch_app(&Apps::Loading, 5), LaunchApp::Wait), "a reload in flight keeps the launch");
    assert!(matches!(launch_app(&Apps::Loaded(std::sync::Arc::new(vec![app.clone()])), 5), LaunchApp::Ready(a) if a.id == 5));
    assert!(matches!(launch_app(&Apps::Loaded(std::sync::Arc::new(vec![app])), 6), LaunchApp::Missing));
    assert!(matches!(launch_app(&Apps::NotLoaded, 5), LaunchApp::Gone));
    // A reload that ends in an error is shown, not dropped.
    assert!(matches!(launch_app(&Apps::Failed("x".into()), 5), LaunchApp::Failed(why) if why == "x"));
}

#[test]
fn the_window_on_another_display_or_mode_is_told_apart() {
    assert!(another_display(SDL_EVENT_WINDOW_DISPLAY_CHANGED));
    assert!(another_display(SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED));
    assert!(!another_display(SDL_EVENT_WINDOW_MOVED) && !another_display(SDL_EVENT_WINDOW_RESIZED));
}

#[test]
fn a_second_file_dialog_is_refused_on_the_page_and_a_late_choice_is_reported() {
    use folders_page::tests::{calls, settle, waker, Fake};
    let fake = Arc::new(Fake::default());
    let mut folders = folders_page::SharedFolders::default();
    folders.open(7, fake.clone(), waker());
    settle(&mut folders, 7);
    let mut slot = None;
    let (_sender, receiver) = std::sync::mpsc::channel();
    start_folder_pick(&mut slot, &mut folders, 7, |at| FolderPick::Upload { host: 7, at }, || receiver);
    assert!(slot.is_some());
    start_folder_pick(&mut slot, &mut folders, 7, |_| panic!("no second pick"), || panic!("no second dialog"));
    let said = folders.state_for(7).unwrap().transfer_error.clone();
    assert_eq!(said.as_deref(), Some("A file dialog is already open. Finish it first."));

    let (pick, _) = slot.take().unwrap();
    assert_eq!(finish_folder_pick(&mut folders, pick, Ok(None)), None, "cancelled");
    let pick = FolderPick::Download { host: 7, at: folders_page::Place::Folders, name: "a.txt".into(), size: 3 };
    let (title, body) = finish_folder_pick(&mut folders, pick, Err("no portal".into())).unwrap();
    assert_eq!((title.as_str(), body.as_str()), ("File dialog unavailable", "no portal"));

    // The page was left while the dialog was open.
    folders.poll(None);
    let at = folders_page::Place::Folders;
    let pick = FolderPick::Upload { host: 7, at };
    let (title, body) = finish_folder_pick(&mut folders, pick, Ok(Some(std::env::temp_dir()))).unwrap();
    assert_eq!(title, "Transfer not started");
    assert!(body.contains("closed while the file dialog was open"), "{body}");
    assert_eq!(calls(&fake), ["folders"]);
}

fn record(unique_id: &str) -> config::HostRecord {
    config::HostRecord {
        unique_id: unique_id.into(),
        name: format!("{unique_id} on the PC"),
        address: "192.0.2.7".into(),
        manual: true,
        ..Default::default()
    }
}

fn temp_store(tag: &str) -> (config::Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nightjar-main-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    (config::Store::at(dir.clone()).unwrap(), dir)
}

#[test]
fn a_name_chosen_for_a_pc_is_saved_and_comes_back_after_a_restart() {
    let (store, dir) = temp_store("rename");
    let mut hosts = Hosts::without_discovery(vec![record("a")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    rename_host(&mut hosts, &store, id, "  Living room  ").unwrap();
    assert_eq!(store.hosts()[0].alias.as_deref(), Some("Living room"), "saved trimmed");
    let restarted = Hosts::without_discovery(store.hosts(), Arc::new(|| {}));
    assert_eq!(restarted.list[0].display_name(), "Living room");
    rename_host(&mut hosts, &store, id, "").unwrap();
    assert_eq!(store.hosts()[0].alias, None, "an empty name brings the PC's own back");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn removing_a_pc_leaves_nothing_of_it_behind() {
    let (store, dir) = temp_store("remove");
    let mut hosts = Hosts::without_discovery(vec![record("gone"), record("kept")], Arc::new(|| {}));
    let gone = hosts.list[0].id;
    let recent = |host: &str| config::Recent { host: host.into(), install_id: String::new(), app_id: 1, title: "G".into(), at: 1 };
    let mut recents = vec![recent("gone"), recent("kept")];
    let star = |host: &str| config::Favourite { host: host.into(), install_id: String::new(), app_id: 1, title: "G".into() };
    let mut favourites = vec![star("gone"), star("kept")];
    store.save_favourites(&favourites).unwrap();
    for host in ["gone", "kept"] {
        let cover = store.art_path(host, 1, "");
        std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
        std::fs::write(cover, b"x").unwrap();
    }
    save_hosts(&store, &hosts).unwrap();
    let game = config::OpenGame { host: "gone".into(), install_id: String::new(), app_id: 1, title: "G".into() };
    store.save_open_play(&config::OpenPlayState { game: Some(game.clone()), armed: false }).unwrap();
    let mut open_game = Some(game);
    remove_host(&mut hosts, &store, &mut recents, &mut favourites, &mut open_game, gone).unwrap();
    assert_eq!(open_game, None, "the settings page and game menu forget its chosen game");
    assert_eq!(store.open_play().game, None);
    assert_eq!(store.hosts().len(), 1);
    assert_eq!(store.hosts()[0].unique_id, "kept");
    assert_eq!(recents.len(), 1);
    assert_eq!(store.recents().len(), 1, "recent.json is filtered");
    assert_eq!(favourites.len(), 1);
    assert_eq!(store.favourites()[0].host, "kept", "favourites.json is filtered");
    assert!(!store.art_path("gone", 1, "").exists());
    assert!(store.art_path("kept", 1, "").exists());
    std::fs::remove_dir_all(dir).ok();
}

// nightjar#115: SDL's Direct3D 11 renderer lost its device while connecting and could not make
// another, and the window kept the connecting screen for minutes with nothing answering. A lost or
// remade device makes the renderer again; a resize of its targets alone does not.
#[test]
fn a_lost_or_remade_render_device_makes_the_renderer_again() {
    assert!(renderer_gone(SDL_EVENT_RENDER_DEVICE_LOST));
    assert!(renderer_gone(SDL_EVENT_RENDER_DEVICE_RESET), "SDL's own remake loses every texture");
    assert!(!renderer_gone(SDL_EVENT_RENDER_TARGETS_RESET));
    assert!(!renderer_gone(SDL_EVENT_WINDOW_RESIZED));
}

#[test]
fn a_remade_renderer_keeps_its_kind_and_colorspace_until_the_wait_is_over() {
    assert!(sdl::remake_keeps(Some("direct3d11"), true, "direct3d11", false));
    assert!(!sdl::remake_keeps(None, false, "direct3d11", false), "a failed try while the driver restarts is tried again");
    assert!(!sdl::remake_keeps(Some("direct3d12"), true, "direct3d11", false), "SDL's next renderer is not kept while there is time");
    assert!(!sdl::remake_keeps(Some("direct3d11"), false, "direct3d11", false), "nor SDR in place of scRGB");
    assert!(sdl::remake_keeps(Some("software"), false, "direct3d11", true), "then whatever SDL made");
    assert!(sdl::remake_keeps(None, false, "direct3d11", true), "and none at all ends Nightjar");
}

#[test]
fn a_driver_that_loses_each_new_device_is_not_followed_forever() {
    let mut guard = RemakeGuard::default();
    let start = Instant::now();
    for i in 0..RemakeGuard::MOST {
        assert!(guard.allow(start + Duration::from_secs(i as u64)), "remake {i}");
    }
    assert!(!guard.allow(start + Duration::from_secs(10)), "one more within the minute");
    assert!(guard.allow(start + RemakeGuard::WITHIN + Duration::from_secs(1)), "the first has left the minute");
}

/// SDL lets a window take a new renderer once the old one is gone, of the same kind and in the
/// same colorspace (what `App::recover_renderer` relies on).
#[cfg(windows)]
#[test]
#[ignore = "requires a Direct3D 11 display"]
fn a_lost_renderer_is_made_again_on_the_same_window_in_its_colorspace() {
    let _guard = sdl::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: This test owns SDL and all its resources on one thread.
    unsafe {
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", sdl::error());
        let window = SDL_CreateWindow(c"Remake test".as_ptr(), 128, 64, SDL_WINDOW_HIDDEN);
        assert!(!window.is_null(), "{}", sdl::error());
        for linear in [false, true] {
            let old = sdl::create_renderer_with(window, linear, false);
            assert!(!old.is_null(), "{}", sdl::error());
            let kind = sdl::renderer_name(old);
            assert_eq!(kind, "direct3d11");
            sdl::destroy_renderer(old);
            let made = sdl::remake_renderer(window, linear, &kind, Duration::from_secs(2));
            assert!(!made.is_null(), "{}", sdl::error());
            assert_eq!(sdl::renderer_name(made), kind);
            let space = SDL_GetNumberProperty(SDL_GetRendererProperties(made), SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0);
            assert_eq!(space, i64::from(if linear { SDL_COLORSPACE_SRGB_LINEAR.0 } else { SDL_COLORSPACE_SRGB.0 }));
            assert!(!sdl::renderer_pointer(made, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER).is_null(), "a device to decode on");
            sdl::destroy_renderer(made);
        }
        // A device still held elsewhere (a launch's, its decoder's) keeps the old swap chain on the
        // window until that device is flushed; without the flush SDL fell to "direct3d" here.
        let old = sdl::create_renderer_with(window, false, false);
        assert!(!old.is_null(), "{}", sdl::error());
        let held = media::DeviceRef::of_device(sdl::renderer_pointer(old, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER));
        sdl::destroy_renderer(old);
        held.flush_released();
        // No patience: the first try is kept, whatever it is.
        let made = sdl::remake_renderer(window, false, "direct3d11", Duration::ZERO);
        assert!(!made.is_null(), "{}", sdl::error());
        assert_eq!(sdl::renderer_name(made), "direct3d11", "the held device's swap chain left the window");
        sdl::destroy_renderer(made);
        drop(held);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

/// A save that fails comes back as an error, so the app tells the player once a run with the
/// folder, instead of only logging it: a read-only config folder otherwise drops every change
/// at the next start without a word.
#[test]
fn a_failed_save_is_told_once_with_the_folder() {
    use crate::app::{first_save_failure, save_failed_text, save_hosts};
    let (store, dir) = temp_store("save-failed");
    // A directory where hosts.json should be: the read fails for a reason that is not "no file",
    // and the store keeps what is there instead of replacing it.
    std::fs::create_dir(dir.join("hosts.json")).unwrap();
    let _ = store.hosts();
    let hosts = Hosts::without_discovery(Vec::new(), Arc::new(|| {}));
    assert!(save_hosts(&store, &hosts).is_err(), "the PC list that cannot be saved is reported");
    let text = save_failed_text(store.dir());
    assert!(text.contains(&store.dir().display().to_string()), "the folder is named: {text}");
    let mut told = false;
    assert!(first_save_failure(&mut told), "the first failure is told");
    assert!(!first_save_failure(&mut told), "one notice a run");
    std::fs::remove_dir_all(dir).ok();
}

/// Overrides holding `keys` from `settings`, as a settings page saves a row.
fn layer(settings: &config::Settings, keys: &[&str]) -> config::Overrides {
    let mut own = config::Overrides::default();
    own.capture(settings, keys);
    own
}

#[test]
fn the_topmost_layer_holding_controllers_is_named() {
    let off = config::Settings { controllers: false, ..Default::default() };
    let on = config::Settings::default();
    let both = &["controllers", "multi_controller"];
    let profile = config::Profile::new(1, "Travel", layer(&off, both));
    let pc = layer(&off, both);
    let game = layer(&on, both);
    let unrelated = layer(&on, &["fps"]);
    assert_eq!(ControllersFrom::of_layers(true, None, None, None, false), ControllersFrom::Settings);
    assert_eq!(ControllersFrom::of_layers(true, Some(&profile), Some(&unrelated), Some(&unrelated), false),
        ControllersFrom::Profile("Travel".into()), "layers without the row are passed over");
    assert_eq!(ControllersFrom::of_layers(true, Some(&profile), Some(&pc), None, false), ControllersFrom::Pc);
    assert_eq!(ControllersFrom::of_layers(false, Some(&profile), Some(&pc), Some(&game), true), ControllersFrom::CommandLine,
        "--multi-controller decides the players over every layer");
    assert_eq!(ControllersFrom::of_layers(false, Some(&profile), Some(&pc), Some(&game), false), ControllersFrom::Game);
    // A game that keeps only the player switch did not turn controllers off: the PC's Off did.
    let players_only = layer(&on, &["multi_controller"]);
    assert_eq!(ControllersFrom::of_layers(true, None, Some(&pc), Some(&players_only), true), ControllersFrom::Pc,
        "the command line never makes Off");
}

#[test]
fn a_launch_logs_the_controllers_choice_its_layer_and_the_connected_ones() {
    let off = config::Settings { controllers: false, ..Default::default() };
    assert_eq!(controllers_line(&off, &ControllersFrom::Pc, &["Xbox Controller"], 0),
        "controllers: off from this PC's settings, 1 connected (Xbox Controller), gamepad mask 0x0");
    assert_eq!(controllers_line(&config::Settings::default(), &ControllersFrom::Settings, &["A", "B"], 3),
        "controllers: player_each from main settings, 2 connected (A, B), gamepad mask 0x3");
    let one = config::Settings { multi_controller: false, ..Default::default() };
    assert_eq!(controllers_line(&one, &ControllersFrom::Profile("Home".into()), &[], 1),
        "controllers: one_player from profile \"Home\", 0 connected, gamepad mask 0x1");
}

#[test]
fn controllers_off_with_one_connected_says_where_it_was_turned_off() {
    let off = config::Settings { controllers: false, ..Default::default() };
    assert_eq!(controllers_notice(&off, &ControllersFrom::Game, 1).as_deref(),
        Some("Controllers is set to Off for this game, so no controller reaches the PC."));
    assert_eq!(controllers_notice(&off, &ControllersFrom::Pc, 2).as_deref(),
        Some("Controllers is set to Off for this PC, so no controller reaches it."));
    assert_eq!(controllers_notice(&off, &ControllersFrom::Profile("Travel".into()), 1).as_deref(),
        Some("Controllers is Off in the profile Travel, so no controller reaches the PC."));
    assert_eq!(controllers_notice(&off, &ControllersFrom::Settings, 1).as_deref(),
        Some("Controllers is Off in Settings, so no controller reaches the PC."));
    assert_eq!(controllers_notice(&off, &ControllersFrom::Pc, 0), None, "no controller here, nothing to say");
    assert_eq!(controllers_notice(&off, &ControllersFrom::StreamKind, 1), None,
        "a microphone-only stream or a second screen sends none by design");
    for on in [config::Settings::default(), config::Settings { multi_controller: false, ..Default::default() }] {
        assert_eq!(controllers_notice(&on, &ControllersFrom::Game, 2), None, "every connected controller is sent");
    }
    // Once a stream: at its start, else for the first controller connected during it.
    let mut stream = ControllersOffNotice::default();
    assert_eq!(stream.take(&off, &ControllersFrom::Pc, 0), None, "nothing connected at the start");
    assert!(stream.take(&off, &ControllersFrom::Pc, 1).is_some(), "the first controller connected during the stream");
    assert_eq!(stream.take(&off, &ControllersFrom::Pc, 2), None, "a second one is not said again");
    let mut started = ControllersOffNotice::default();
    assert!(started.take(&off, &ControllersFrom::Game, 1).is_some(), "said at the start");
    assert_eq!(started.take(&off, &ControllersFrom::Game, 2), None, "not again for one connected later");
    let mut kind = ControllersOffNotice::default();
    assert_eq!(kind.take(&off, &ControllersFrom::StreamKind, 1), None);
    assert_eq!(kind.take(&off, &ControllersFrom::StreamKind, 2), None, "a microphone-only stream or second screen never says it");
    crate::strings::with_lang(crate::strings::Lang::De, || {
        assert_eq!(controllers_notice(&off, &ControllersFrom::Profile("Reise".into()), 1).as_deref(),
            Some("„Controller“ ist im Profil Reise auf Aus gestellt, daher erreicht kein Controller den PC."));
    });
}

#[test]
fn macos_fullscreen_leaves_spaces_for_the_notch() {
    assert_eq!(crate::sdl::fullscreen_spaces_hint(true), Some(c"0"), "SDL's own fullscreen covers the notch area");
    assert_eq!(crate::sdl::fullscreen_spaces_hint(false), None, "Windows and Linux keep SDL's defaults");
}

/// A device still in use is not a renderer failure: only the stream that asked is refused, and the
/// GPU renderer stays on offer for the next one.
#[test]
#[cfg(unix)]
fn a_busy_switch_does_not_mark_the_gpu_renderer_failed() {
    use super::Rebuild;
    let mut state = super::RendererState::for_launcher(crate::config::Renderer::Auto, false, None);
    let said = state.rebuild_refused(&Rebuild::Busy("busy".into()));
    assert_eq!(said, "busy");
    assert!(state.gpu_failed.is_none(), "busy is no failure");
    assert_eq!(state.rebuild_refused(&Rebuild::Failed("no vulkan".into())), "no vulkan");
    assert_eq!(state.gpu_failed.as_deref(), Some("no vulkan"), "a renderer that could not be made is remembered");
}

#[test]
fn every_bound_key_has_a_handler() {
    use crate::shortcuts::{keys_in, Action, Button, Press, Scope, KEYS, PAD};
    fn strip(source: &str) -> &str {
        let end = source.find("\n#[cfg(test)]").unwrap_or(source.len());
        &source[..end]
    }
    let input = strip(include_str!("../../desktop-media/src/stream/input.rs"));
    let main = strip(include_str!("main.rs")).to_owned()
        + strip(include_str!("app/launcher.rs"))
        + strip(include_str!("app/session.rs"))
        + strip(include_str!("app/overlay.rs"))
        + strip(include_str!("app/control_center.rs"))
        + strip(include_str!("app/control_center/pages.rs"));
    let help = strip(include_str!("../../desktop-ui/src/ui/help.rs"));
    for binding in KEYS.iter().cloned().chain(keys_in(Scope::Stream)) {
        let name = format!("Action::{:?}", binding.action);
        let handled = match binding.scope {
            Scope::Stream => input.contains(&name) || main.contains(&name),
            Scope::Anywhere => main.contains("Scope::Stream") && main.matches(&name).count() >= 2,
            Scope::Launcher => match binding.action {
                Action::Fullscreen => main.contains(&name),
                Action::Help => help.contains(&name),
                Action::Move
                | Action::Select
                | Action::Back
                | Action::Options
                | Action::AddPc
                | Action::Unstar
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::ZoomReset => true,
                _ => false,
            },
        };
        assert!(handled, "{:?} in {:?} has no handler", binding.action, binding.scope);
    }
    for binding in PAD {
        let name = format!("Action::{:?}", binding.action);
        let handled = match (binding.scope, binding.press) {
            (Scope::Launcher, Press::Any(_)) => binding.action == Action::Move || main.contains(&name),
            (Scope::Stream, Press::Together(_)) => {
                include_str!("../../desktop-media/src/gamepad.rs").contains("stream_controls_mask()")
            }
            (Scope::Stream, Press::QuitCombo) => {
                include_str!("../../desktop-media/src/gamepad.rs").contains("quit_mask(self.quit_combo)")
            }
            (Scope::Stream, Press::Hold(Button::Start)) => {
                include_str!("../../desktop-media/src/controller_mouse.rs").contains("750")
            }
            _ => false,
        };
        assert!(handled, "{:?} {:?} has no handler", binding.action, binding.press);
    }
}

#[test]
fn the_pages_that_show_the_link_speed_are_the_ones_read_often() {
    use crate::network::Pace;
    use crate::ui::Screen;
    assert_eq!(Screen::Profiles.network_pace(), Pace::Live);
    assert_eq!(Screen::HostSettings(1).network_pace(), Pace::Live);
    for screen in [
        Screen::Home,
        Screen::AllGames,
        Screen::Game { host: 1, app: 2 },
        Screen::Hosts,
        Screen::Apps(1),
        Screen::Settings,
    ] {
        assert_eq!(screen.network_pace(), Pace::Idle, "{screen:?}");
    }
}

#[test]
fn desktop_host_store_roundtrips_android_slots_and_unknown_extra_fields() {
    use nightjar_core::hosts::{EndpointSlot, HostEntry, HostStore};
    let (_, dir) = temp_store("bidirectional-hosts");
    let store = HostStore::open(&dir);

    let mut android = HostEntry::default();
    android.shared.unique_id = "pc-bi".into();
    android.shared.name = "Rig".into();
    android.shared.address = "192.168.1.50".into();
    android.shared.http_port = 47989;
    android.shared.manual = true;
    android.local_endpoint = EndpointSlot::new("192.168.1.50", 47989);
    android.remote_endpoint = EndpointSlot::new("203.0.113.50", 48989);
    android.manual_endpoint = EndpointSlot::new("rig.example", 47989);
    android.ipv6_endpoint = EndpointSlot::new("2001:db8::50", 47989);
    android.pinned_endpoint = EndpointSlot::new("192.168.1.50", 47989);
    android.extra.insert("future_android_key".into(), serde_json::json!({"v": 2}));
    assert!(store.upsert(android.clone()).unwrap());

    let mut desktop_records =
        store.load_records::<config::HostRecord, _>(config::keep_readable_settings_value);
    assert_eq!(desktop_records.len(), 1);
    desktop_records[0].https_port = 47984;
    desktop_records[0].other_addresses = vec!["10.0.0.50".into()];
    desktop_records[0].own_wake_macs = vec!["aa:bb:cc:dd:ee:01".into()];
    desktop_records[0].internet_play = Some("secret".into());
    store.save_records(&desktop_records).unwrap();

    let after_desktop = store.get_by_uuid("pc-bi").unwrap();
    assert_eq!(after_desktop.local_endpoint, android.local_endpoint);
    assert_eq!(after_desktop.remote_endpoint, android.remote_endpoint);
    assert_eq!(after_desktop.manual_endpoint, android.manual_endpoint);
    assert_eq!(after_desktop.ipv6_endpoint, android.ipv6_endpoint);
    assert_eq!(after_desktop.pinned_endpoint, android.pinned_endpoint);
    assert_eq!(after_desktop.extra.get("future_android_key"), Some(&serde_json::json!({"v": 2})));
    assert_eq!(after_desktop.shared.https_port, 47984);
    assert_eq!(after_desktop.shared.other_addresses, vec!["10.0.0.50"]);
    assert_eq!(after_desktop.shared.own_wake_macs, vec!["aa:bb:cc:dd:ee:01"]);
    assert_eq!(after_desktop.shared.internet_play.as_deref(), Some("secret"));
    std::fs::remove_dir_all(dir).ok();
}
