//! Unit tests for SDL helpers, HDR metadata, display luminance, and swapchain answer caching.

use super::*;

#[test]
fn a_wheel_turn_keeps_sdls_values_on_every_platform() {
    // SAFETY: an all-zero SDL_MouseWheelEvent is a valid value.
    let mut w: SDL_MouseWheelEvent = unsafe { std::mem::zeroed() };
    (w.x, w.y) = (1.5, -2.0);
    assert_eq!(wheel_turn(&w), (1.5, -2.0), "an unflipped turn stays as it is");
    w.direction = SDL_MOUSEWHEEL_FLIPPED;
    assert_eq!(wheel_turn(&w), (1.5, -2.0),
        "macOS and Wayland already adjusted the values to the system setting, the flag only reports it");
}

#[test]
fn the_hdr10_swapchain_gets_the_hosts_metadata_capped_at_the_output_peak() {
    // BT.2020 primaries and D65 in 1/50000 units, a 1000-nit mastering display at 0.005 nits.
    let host = nightjar_core::control::HdrMetadata {
        display_primaries: [(35400, 14600), (8500, 39850), (6550, 2300)],
        white_point: (15635, 16450),
        max_display_luminance: 1000,
        min_display_luminance: 50,
        max_content_light_level: 1200,
        max_frame_average_light_level: 400,
        max_full_frame_luminance: 0,
    };
    let metadata = swapchain_hdr_metadata(&host, Some(600.0)).unwrap();
    assert_eq!(metadata.primaries, [[0.708, 0.292], [0.17, 0.797], [0.131, 0.046]]);
    assert_eq!(metadata.white_point, [0.3127, 0.329]);
    assert_eq!((metadata.max_luminance, metadata.max_content_light_level, metadata.max_frame_average_light_level), (600.0, 600.0, 400.0),
        "SDL tone maps to the window's 600 nits first: the compositor must not map again");
    assert_eq!(metadata.min_luminance, 0.005);
    let uncapped = swapchain_hdr_metadata(&host, None).unwrap();
    assert_eq!((uncapped.max_luminance, uncapped.max_content_light_level), (1000.0, 1200.0), "no window peak yet: as sent");
    let unknown_cll = nightjar_core::control::HdrMetadata { max_content_light_level: 0, max_frame_average_light_level: 0, ..host };
    let unknown_cll = swapchain_hdr_metadata(&unknown_cll, Some(600.0)).unwrap();
    assert_eq!((unknown_cll.max_content_light_level, unknown_cll.max_frame_average_light_level), (0.0, 0.0), "unknown light levels stay unknown");
    // The content's peak a description that follows the desktop declares: MaxCLL, else the
    // mastering peak, both capped as the swapchain has them.
    assert_eq!(content_peak(Some(&metadata)), Some(600.0));
    assert_eq!(content_peak(Some(&uncapped)), Some(1200.0));
    assert_eq!(content_peak(Some(&unknown_cll)), Some(600.0));
    assert_eq!(content_peak(None), None);
    // KWin and wlroots end a client whose mastering minimum is not below its maximum.
    assert_eq!(swapchain_hdr_metadata(&nightjar_core::control::HdrMetadata::default(), Some(600.0)), None, "zeroed metadata is none");
    let inverted = nightjar_core::control::HdrMetadata { max_display_luminance: 1, min_display_luminance: 10_000, ..host };
    assert_eq!(swapchain_hdr_metadata(&inverted, None), None, "1 nit over 1 nit");
    assert_eq!(swapchain_hdr_metadata(&host, Some(0.004)), None, "a peak capped below the minimum");
    // VkHdrMetadataEXT: sType, pNext, then 12 floats.
    assert_eq!(std::mem::size_of::<SwapchainHdrMetadata>(), 16 + 12 * 4);
    assert_eq!(metadata.s_type, 1_000_105_000);
}

fn hint(name: *const c_char) -> String {
    // SAFETY: a NUL-terminated name; SDL returns its own string or null.
    let value = unsafe { SDL_GetHint(name) };
    if value.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned()
}

// nightjar#80: SDL's internals, reached through the static SDL of a Linux build, as a Wayland
// output's late description reaches them. The dummy driver opens no device or window.
#[cfg(target_os = "linux")]
#[test]
fn a_display_hdr_state_that_arrives_after_the_display_is_seen() {
    unsafe extern "C" {
        fn SDL_GetVideoDisplay(display: SDL_DisplayID) -> *mut std::ffi::c_void;
        fn SDL_SetDisplayHDRProperties(display: *mut std::ffi::c_void, hdr: *const [f32; 2]);
    }
    let _sdl = TEST_LOCK.lock().unwrap();
    // SAFETY: on one thread under the lock; the display is live until the subsystem quits, and
    // [SDR white, headroom] is SDL_HDROutputProperties.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"dummy".as_ptr());
        assert!(SDL_InitSubSystem(SDL_INIT_VIDEO), "{}", error());
        let display = SDL_GetVideoDisplay(SDL_GetPrimaryDisplay());
        assert!(!display.is_null(), "{}", error());
        assert!(!any_display_hdr(), "added without HDR");
        SDL_SetDisplayHDRProperties(display, &[2.5, 4.0]);
        assert!(any_display_hdr(), "the compositor's answer turned it on");
        SDL_SetDisplayHDRProperties(display, &[1.0, 1.0]);
        assert!(!any_display_hdr(), "and off again");
        SDL_QuitSubSystem(SDL_INIT_VIDEO);
        SDL_ResetHint(SDL_HINT_VIDEO_DRIVER);
    }
}

// nightjar#132: SDL kept the screen awake from the first window until Nightjar quit.
#[test]
fn the_screen_is_kept_awake_only_while_streaming() {
    let _sdl = TEST_LOCK.lock().unwrap();
    screen_sleeps_outside_streams();
    // SAFETY: on one thread under the lock; the dummy driver opens no device or window.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"dummy".as_ptr());
        assert!(SDL_InitSubSystem(SDL_INIT_VIDEO), "{}", error());
        assert!(SDL_ScreenSaverEnabled(), "the launcher lets the screen sleep");
        keep_screen_awake(true);
        assert!(!SDL_ScreenSaverEnabled(), "a stream keeps it awake");
        keep_screen_awake(false);
        assert!(SDL_ScreenSaverEnabled(), "and lets it go when it ends");
        // In the launcher, controller input holds it for a while: the desktop does not see it.
        let mut launcher = LauncherAwake::default();
        let now = std::time::Instant::now();
        launcher.apply(now);
        assert!(SDL_ScreenSaverEnabled(), "no controller input yet");
        launcher.active(now);
        launcher.apply(now);
        assert!(!SDL_ScreenSaverEnabled(), "a controller was used");
        launcher.apply(now + LAUNCHER_AWAKE - std::time::Duration::from_secs(1));
        assert!(!SDL_ScreenSaverEnabled());
        launcher.apply(now + LAUNCHER_AWAKE);
        assert!(SDL_ScreenSaverEnabled(), "free to sleep once it has been idle");
        SDL_QuitSubSystem(SDL_INIT_VIDEO);
        SDL_ResetHint(SDL_HINT_VIDEO_DRIVER);
        SDL_ResetHint(SDL_HINT_VIDEO_ALLOW_SCREENSAVER);
        SDL_ResetHint(SDL_HINT_SCREENSAVER_INHIBIT_ACTIVITY_NAME);
    }
}

#[test]
fn alt_f4_reaches_the_game_only_while_streaming() {
    let _sdl = TEST_LOCK.lock().unwrap();
    alt_f4_to_host(true);
    assert_eq!(hint(SDL_HINT_WINDOWS_CLOSE_ON_ALT_F4), "0");
    alt_f4_to_host(false);
    assert_eq!(hint(SDL_HINT_WINDOWS_CLOSE_ON_ALT_F4), "1");
}

#[test]
fn the_title_bar_takes_dark_mode_first_and_the_page_colours_as_colorrefs() {
    let night = [0x14, 0x16, 0x1F];
    let text = [0xE3, 0xE4, 0xEE];
    let attributes = title_bar_attributes(night, text);
    // Dark mode first: Windows 10 refuses the colours but still takes it.
    assert_eq!(attributes[0], (20, 1));
    assert_eq!(attributes[1..], [(34, 0x001F_1614), (35, 0x001F_1614), (36, 0x00EE_E4E3)]);
    // Windows 10 1809 to 1909 know dark mode as 19; the colours have no older number.
    assert_eq!(title_bar_fallback(20), Some(19));
    assert!(attributes[1..].iter().all(|(attribute, _)| title_bar_fallback(*attribute).is_none()));
    assert_eq!(title_bar_needs(20), "Windows 10 1809 or later");
    assert_eq!(title_bar_needs(35), "Windows 11");
}

mod luminance_tests {
    use super::super::hdr::luminance_of;
    use rubylight_protocol::control::DisplayCaps;

    #[test]
    fn the_launch_gets_the_values_the_control_message_carries() {
        let caps = DisplayCaps { hdr: true, max_centinits: 100_000, max_average_centinits: 40_000, min_decimillinits: 50 };
        let luminance = luminance_of(&caps).unwrap();
        assert_eq!(luminance.query_parameters(), "&minBrightness=0.005&maxBrightness=1000.0&maxAverageBrightness=400.0");
        // No output description: nothing is sent and the host picks its own.
        assert_eq!(luminance_of(&DisplayCaps::default()), None);
    }
}

mod swapchain_answer_tests {
    use super::super::hdr::SWAPCHAIN_ANSWERS_KEPT;
    use super::*;

    fn state(width: i32) -> SwapchainState {
        SwapchainState { device: 0x1000, display: 1, display_hdr: true, window_hdr: true, width, height: 1080 }
    }

    #[test]
    fn an_answer_is_kept_for_the_state_it_was_asked_in() {
        let mut answers = SwapchainAnswers::new();
        let generation = answers.generation();
        assert_eq!(answers.get(7, state(1920)), None);
        answers.store(generation, 7, state(1920), true);
        assert_eq!(answers.get(7, state(1920)), Some(true));
        // A resize, a display change or a renderer of another device asks SDL again.
        assert_eq!(answers.get(7, state(2560)), None);
        assert_eq!(answers.get(8, state(1920)), None);
        let other_display = SwapchainState { display: 2, ..state(1920) };
        assert_eq!(answers.get(7, other_display), None);
        let other_device = SwapchainState { device: 0x2000, ..state(1920) };
        assert_eq!(answers.get(7, other_device), None);
    }

    #[test]
    fn a_new_answer_for_a_renderer_replaces_its_old_one() {
        let mut answers = SwapchainAnswers::new();
        let generation = answers.generation();
        answers.store(generation, 7, state(1920), true);
        answers.store(generation, 7, state(2560), false);
        assert_eq!(answers.get(7, state(1920)), None);
        assert_eq!(answers.get(7, state(2560)), Some(false));
        assert_eq!(answers.entries.len(), 1);
    }

    #[test]
    fn forgetting_drops_every_answer_and_a_stale_one_is_not_stored() {
        let mut answers = SwapchainAnswers::new();
        let before = answers.generation();
        answers.store(before, 7, state(1920), true);
        answers.store(before, 8, state(1920), false);
        answers.forget_all();
        assert_eq!(answers.get(7, state(1920)), None);
        assert_eq!(answers.get(8, state(1920)), None);
        // An answer computed before the forget (while SDL was still asked) is dropped, not kept.
        answers.store(before, 7, state(1920), true);
        assert_eq!(answers.get(7, state(1920)), None);
        // One computed after it is kept.
        let after = answers.generation();
        assert_ne!(before, after);
        answers.store(after, 7, state(1920), false);
        assert_eq!(answers.get(7, state(1920)), Some(false));
    }

    #[test]
    fn the_oldest_renderer_goes_when_too_many_are_kept() {
        let mut answers = SwapchainAnswers::new();
        let generation = answers.generation();
        for renderer in 0..=SWAPCHAIN_ANSWERS_KEPT {
            answers.store(generation, renderer, state(1920), true);
        }
        assert_eq!(answers.entries.len(), SWAPCHAIN_ANSWERS_KEPT);
        assert_eq!(answers.get(0, state(1920)), None);
        assert_eq!(answers.get(SWAPCHAIN_ANSWERS_KEPT, state(1920)), Some(true));
    }
}
