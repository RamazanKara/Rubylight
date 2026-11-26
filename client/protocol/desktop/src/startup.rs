//! Desktop startup helpers: display mode/DPI queries, SDL initialization and hints,
//! GPU renderer selection, window/renderer creation and display tearing configuration.

use super::*;

/// Now on the session clock (`nightjar_core::session::now_us`), in nanoseconds, the clock
/// `stream::phase::timing` reports vblanks on.
pub(crate) fn session_now_ns() -> i64 {
    i64::try_from(nightjar_core::session::now_us()).unwrap_or(i64::MAX / 1000).saturating_mul(1000)
}

pub(crate) fn display_of(window: *mut SDL_Window) -> Display {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    unsafe {
        let id = SDL_GetDisplayForWindow(window);
        let mode = SDL_GetCurrentDisplayMode(id);
        if mode.is_null() {
            return display_from_mode(None, None);
        }
        let m = &*mode;
        display_from_mode(Some((m.w, m.h, m.pixel_density, m.refresh_rate)), panel_pixels(id))
    }
}

/// macOS: the panel's own size in pixels, from its native mode (`mac_display::native_pixels`),
/// read once for each display. SDL's mode there is the desktop's backing store, which a scaled
/// mode makes larger than the panel. None elsewhere, and on a Mac whose display lists no native
/// mode: SDL's mode is used then.
#[cfg(target_os = "macos")]
pub(crate) fn panel_pixels(id: SDL_DisplayID) -> Option<(u32, u32)> {
    /// The display last read, and its panel's size.
    type Read = Option<(SDL_DisplayID, Option<(u32, u32)>)>;
    static PANEL: std::sync::Mutex<Read> = std::sync::Mutex::new(None);
    let mut panel = PANEL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((known, pixels)) = *panel
        && known == id
    {
        return pixels;
    }
    let mut bounds = SDL_Rect { x: 0, y: 0, w: 0, h: 0 };
    // SAFETY: a display query with a valid out-pointer.
    if !unsafe { SDL_GetDisplayBounds(id, &mut bounds) } {
        // Not kept: the next call asks again.
        return None;
    }
    let (x, y) = (f64::from(bounds.x) + f64::from(bounds.w) / 2.0, f64::from(bounds.y) + f64::from(bounds.h) / 2.0);
    let pixels = mac_display::native_pixels(&mac_display::modes_at(x, y));
    *panel = Some((id, pixels));
    pixels
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn panel_pixels(_: SDL_DisplayID) -> Option<(u32, u32)> {
    None
}

/// The screen the window is on, for the profile kept for it (`Profiles::screen`): SDL's name for
/// the display, which it reads from the monitor (EDID on Windows and Linux, the system's name for
/// it on macOS) and which stays that monitor's on any port and after a restart. None when SDL
/// gives no name.
pub(crate) fn screen_of(window: *mut SDL_Window) -> Option<config::DisplayMatcher> {
    // SAFETY: a live window; the name is owned by SDL and copied before the next SDL call.
    let name = unsafe {
        let name = SDL_GetDisplayName(SDL_GetDisplayForWindow(window));
        if name.is_null() { return None; }
        std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned()
    };
    config::DisplayMatcher::new(&name, &name)
}

/// Whether the window now stands on another display, or its display on another mode.
pub(crate) fn another_display(kind: SDL_EventType) -> bool {
    matches!(kind, SDL_EVENT_WINDOW_DISPLAY_CHANGED | SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED)
}

/// The display from SDL's current mode (width, height, pixel density, refresh rate), if it gave
/// one, with the panel's own size (`panel_pixels`) in place of the mode's where it is known.
/// Without a mode or a refresh rate, pacing assumes 1920x1080 at 60 Hz, but the rate is not
/// marked reported, so the overlay does not judge frames against a guess.
pub(crate) fn display_from_mode(mode: Option<(i32, i32, f32, f32)>, panel: Option<(u32, u32)>) -> Display {
    let Some((w, h, pixel_density, refresh_rate)) = mode else {
        return Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: false };
    };
    let (width, height) = panel.unwrap_or_else(|| mode_pixels(w, h, pixel_density));
    let reported = refresh_rate.is_finite() && refresh_rate > 0.0;
    Display { width, height, refresh_hz: if reported { refresh_rate } else { 60.0 }, refresh_reported: reported }
}

/// A display mode's size in pixels. SDL 3 gives a mode's size in its own units and
/// `pixel_density` turns them into pixels: Windows (per-monitor DPI aware, which SDL declares)
/// and X11 give pixels at density 1 whatever the desktop scaling, Wayland and macOS give points
/// and the scale. On macOS that is the backing store, the panel's size only at the default
/// scaled mode (`panel_pixels`). The display's scaling (`SDL_GetDisplayContentScale`) is not part
/// of it.
pub(crate) fn mode_pixels(w: i32, h: i32, pixel_density: f32) -> (u32, u32) {
    let density = if pixel_density > 0.0 { pixel_density } else { 1.0 };
    let pixels = |points: i32| (points.max(0) as f32 * density).round() as u32;
    (pixels(w), pixels(h))
}

/// The refresh rate exclusive fullscreen asks the display for. With VRR the display's top rate
/// (`modes`: the refresh rates of its fullscreen modes), whatever the desktop runs at: the mode
/// closest to it at the stream's size is that size's fastest, the panel follows the frames up to
/// it, and a mode at the stream's rate would cap the range (a 60 fps stream would put a 240 Hz
/// panel at 60 Hz). Without VRR the stream's rate, so each frame gets whole refreshes.
pub(crate) fn fullscreen_refresh_hz(vrr: bool, modes: &[f32], stream_fps: i32) -> f32 {
    let top = modes.iter().copied().fold(0.0, f32::max);
    if vrr && top > 0.0 { top } else { stream_fps as f32 }
}

/// The refresh rates of a display's fullscreen modes, empty when SDL has none.
pub(crate) fn mode_refresh_rates(id: SDL_DisplayID) -> Vec<f32> {
    fullscreen_modes(id).iter().map(|mode| mode.refresh_hz).collect()
}

/// A display's fullscreen modes in SDL's order, empty when SDL has none.
pub(crate) fn fullscreen_modes(id: SDL_DisplayID) -> Vec<ui::FullscreenMode> {
    let mut count = 0;
    // SAFETY: SDL allocates the list of mode pointers, which is read here and then freed.
    unsafe {
        let modes = SDL_GetFullscreenDisplayModes(id, &mut count);
        if modes.is_null() {
            return Vec::new();
        }
        let list = (0..count.max(0) as usize)
            .filter_map(|i| {
                (*modes.add(i)).as_ref().map(|mode| ui::FullscreenMode { w: mode.w, h: mode.h, refresh_hz: mode.refresh_rate, bytes_per_pixel: SDL_BYTESPERPIXEL(mode.format) })
            })
            .collect();
        SDL_free(modes.cast());
        list
    }
}

/// What the display reported, for the log: the stream's "native" resolution comes from it.
pub(crate) fn log_display(window: *mut SDL_Window, display: Display) {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    let (mode, scale, id) = unsafe {
        let id = SDL_GetDisplayForWindow(window);
        let mode = SDL_GetCurrentDisplayMode(id).as_ref().map(|m| (m.w, m.h, m.pixel_density));
        (mode, SDL_GetDisplayContentScale(id), id)
    };
    let mode = mode.map_or("no mode".to_owned(), |(w, h, d)| format!("mode {w}x{h} at density {d}"));
    // Where a Mac's size came from: its native mode, or SDL's mode when none is listed.
    let panel = match (cfg!(target_os = "macos"), panel_pixels(id)) {
        (false, _) => String::new(),
        (true, Some((w, h))) => format!(", native mode {w}x{h}"),
        (true, None) => ", no native mode listed".to_owned(),
    };
    eprintln!(
        "nightjar: display {}x{}@{} ({mode}, scaling {scale}{panel})",
        display.width, display.height, display.refresh_hz
    );
}

/// When this process entered `main`, for the launch line.
pub(crate) static LAUNCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// The events after which the window has to be drawn even if egui paints the same: it moved, was
/// resized, exposed or restored, or the renderer lost what it held.
pub(crate) fn window_needs_a_frame(kind: SDL_EventType) -> bool {
    (SDL_EVENT_WINDOW_FIRST.0..=SDL_EVENT_WINDOW_LAST.0).contains(&kind.0)
        || matches!(kind, SDL_EVENT_RENDER_TARGETS_RESET | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST)
}

/// The launch line, once: the launcher's first frame is on the screen.
pub(crate) fn first_frame_line(already: &std::sync::atomic::AtomicBool, since: Option<std::time::Duration>) -> Option<String> {
    let since = since?;
    (!already.swap(true, std::sync::atomic::Ordering::Relaxed))
        .then(|| format!("nightjar: launch to first frame {} ms", since.as_millis()))
}

pub(crate) fn message_box(flags: SDL_MessageBoxFlags, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    // SAFETY: NUL-terminated strings; a null parent window is allowed.
    unsafe { SDL_ShowSimpleMessageBox(flags, c"Nightjar".as_ptr(), text.as_ptr(), std::ptr::null_mut()) };
}

/// A Wayland window learns its HDR state after it is mapped: the log says when, launcher or stream.
pub(crate) fn log_window_hdr(event: &SDL_Event) {
    static LAST: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    // SAFETY: union members read by event type.
    unsafe {
        if SDL_EventType(event.r#type) == SDL_EVENT_WINDOW_HDR_STATE_CHANGED
            && LAST.swap(event.window.data1, std::sync::atomic::Ordering::Relaxed) != event.window.data1 {
            eprintln!("nightjar: the compositor reports window HDR {} ({})", if event.window.data1 != 0 { "on" } else { "off" }, sdl::window_hdr_levels(event.window.windowID));
        }
    }
}

pub(crate) fn blend_title_bar(window: *mut SDL_Window, bg: egui::Color32, fg: egui::Color32) {
    sdl::blend_title_bar(window, [bg.r(), bg.g(), bg.b()], [fg.r(), fg.g(), fg.b()]);
}

/// The launcher window's flags for how it opens (handheld.rs).
pub(crate) fn start_flags(start: handheld::StartWindow) -> SDL_WindowFlags {
    SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_HIDDEN
        | if start.fullscreen { SDL_WINDOW_FULLSCREEN } else { SDL_WindowFlags(0) }
        | if start.borderless { SDL_WINDOW_BORDERLESS } else { SDL_WindowFlags(0) }
}

/// Deduplicates repeated PyroWave GPU probe failure log lines while Settings is open.
pub(crate) fn should_log_pyrowave_probe_error(
    last: &mut Option<(bool, String)>,
    ten_bit: bool,
    reason: &str,
) -> bool {
    let unchanged = last.as_ref().is_some_and(|(prev_bit, prev_reason)| *prev_bit == ten_bit && prev_reason == reason);
    if unchanged {
        return false;
    }
    *last = Some((ten_bit, reason.to_owned()));
    true
}

fn log_pyrowave_probe_error(ten_bit: bool, reason: &str) {
    static LAST: std::sync::Mutex<Option<(bool, String)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if should_log_pyrowave_probe_error(&mut last, ten_bit, reason) {
        eprintln!(
            "nightjar: pyrowave: GPU probe failed ({}): {reason}",
            if ten_bit { "10-bit" } else { "8-bit" }
        );
    }
}

/// Proleptic Gregorian civil date to days since 1970-01-01 (Howard Hinnant algorithm).
// Windows and Unix (Linux and macOS) share this date helper via local_day().
#[cfg_attr(not(any(windows, unix, test)), allow(dead_code))]
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Converts Unix UTC seconds to the local calendar day index since 1970-01-01.
pub(crate) fn local_day(secs: u64) -> i64 {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
        use windows::Win32::System::Time::FileTimeToSystemTime;
        let ticks = secs.saturating_mul(10_000_000).saturating_add(116_444_736_000_000_000);
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let (mut local_ft, mut st) = (FILETIME::default(), SYSTEMTIME::default());
        // SAFETY: valid pointers to local structs filled by Win32 time conversion.
        if unsafe { FileTimeToLocalFileTime(&ft, &mut local_ft).is_ok() && FileTimeToSystemTime(&local_ft, &mut st).is_ok() } {
            return days_from_civil(i64::from(st.wYear), u32::from(st.wMonth), u32::from(st.wDay));
        }
    }
    // Unix (Linux and macOS): convert UTC seconds to local calendar day via `localtime_r`.
    #[cfg(unix)]
    {
        if let Ok(t) = libc::time_t::try_from(secs) {
            // SAFETY: zeroed `libc::tm` written by thread-safe POSIX `localtime_r`.
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            let out = unsafe { libc::localtime_r(&t, &mut tm) };
            if !out.is_null() {
                return days_from_civil(
                    i64::from(tm.tm_year) + 1900,
                    (tm.tm_mon + 1).max(1) as u32,
                    tm.tm_mday.max(1) as u32,
                );
            }
        }
    }
    (secs / 86_400) as i64
}

/// Registers crate-level callback hooks before SDL and the UI initialize.
pub(crate) fn register_hooks() {
    font_loader::register();
    conn_test::register();
    pin_clock::register();
    shortcuts_runtime::register_hooks();
    platform_hooks::register(platform_hooks::PlatformHooks {
        pyrowave_probe: |device, ten_bit| {
            let result = stream::pyrowave_mode::settings_probe(device as *mut std::ffi::c_void, ten_bit);
            if let Some(Err(ref reason)) = result {
                log_pyrowave_probe_error(ten_bit, reason);
            }
            result
        },
        enhance_probe: |device| {
            stream::enhance::settings_probe(device as *mut std::ffi::c_void)
                .map(|cap| cap.is_some())
        },
        clear_scan_sync: stream::scan_sync::clear_all,
        metalfx_supported: stream::metalfx_supported,
        offers_714: stream::offers_714,
        self_update_checks: || self_update::current().checks(),
        kms_active: kms::active,
        gpu_pair: || {
            #[cfg(windows)]
            {
                gpu_adapters::gpus()
                    .map(|g| (g.power_saving.name.clone(), g.high_performance.name.clone()))
            }
            #[cfg(not(windows))]
            {
                None
            }
        },
        video_driver: || {
            #[cfg(target_os = "linux")]
            {
                sdl::video_driver()
            }
            #[cfg(not(target_os = "linux"))]
            {
                String::new()
            }
        },
        local_day,
    });
}

/// Sets process environment variables inspected by gamescope's WSI layer and Mesa before
/// `logpipe::start` spawns threads (`std::env::set_var` is only sound while single-threaded on
/// POSIX). Returns diagnostic lines to print once the log pipe is open.
pub(crate) fn prepare_process_env() -> Vec<String> {
    let mut notes = Vec::new();
    // Before Vulkan loads: gamescope's layer without its socket would block the swapchain.
    if handheld::gamescope() {
        let socket = handheld::gamescope_socket(
            std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
            std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").as_deref(),
        );
        if handheld::gamescope_layer_off(socket.as_deref(), std::path::Path::exists) {
            notes.push(format!(
                "nightjar: gamescope: its socket {} is not reachable here, so gamescope's Vulkan layer is off (DISABLE_GAMESCOPE_WSI=1) and the output is SDR",
                socket.unwrap_or_default().display()
            ));
            // SAFETY: called in `main` before `logpipe::start` or any other thread spawns.
            unsafe { std::env::set_var("DISABLE_GAMESCOPE_WSI", "1") };
        }
    }
    // Before SDL, VAAPI or Vulkan load Mesa.
    #[cfg(target_os = "linux")]
    {
        let get = |name: &str| std::env::var(name).ok();
        notes.push(amd_decode_note(
            get("NIGHTJAR_AMD_LOW_LATENCY_DECODE").as_deref(),
            get("AMD_DEBUG").as_deref(),
            get("R600_DEBUG").as_deref(),
            get("RADV_PERFTEST").as_deref(),
        ));
        amd_decode::request();
    }
    notes
}

/// Formats the AMD low-latency decode environment note before Mesa loads so `main` can write it
/// once `logpipe` captures stderr.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn amd_decode_note(
    switch: Option<&str>,
    amd_debug: Option<&str>,
    r600_debug: Option<&str>,
    radv_perftest: Option<&str>,
) -> String {
    match amd_decode::variables(switch, amd_debug, r600_debug, radv_perftest) {
        Some((amd, radv)) => format!(
            "nightjar: AMD low-latency decode asked for: AMD_DEBUG={amd} RADV_PERFTEST={radv} (Mesa drivers with lowlatencydec use it)"
        ),
        None => "nightjar: AMD low-latency decode not asked for (NIGHTJAR_AMD_LOW_LATENCY_DECODE=0)".to_owned(),
    }
}

/// Sets SDL hints, resolves Linux KMS/desktop backend selection, and initializes SDL subsystems.
pub(crate) fn init_sdl() -> Result<(), String> {
    sdl::set_hint(SDL_HINT_APP_NAME, c"Nightjar");
    sdl::set_hint(SDL_HINT_APP_ID, c"io.github.ramazankara.nightjar");
    #[cfg(windows)]
    {
        // The decoder shares the renderer's D3D11 device from its own thread.
        sdl::set_hint(SDL_HINT_RENDER_DRIVER, c"direct3d11");
        sdl::set_hint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1");
    }
    // VAAPI pictures reach the OpenGL renderer as EGL images (`dmabuf`), which needs an EGL
    // context on X11 as well (Wayland's always is), as moonlight-qt forces it.
    #[cfg(target_os = "linux")]
    sdl::set_hint(SDL_HINT_VIDEO_FORCE_EGL, c"1");
    // Keep Alt+Tab and the Windows key for the host while the keyboard is grabbed.
    sdl::set_hint(SDL_HINT_ALLOW_ALT_TAB_WHILE_GRABBED, c"0");
    sdl::set_hint(SDL_HINT_VIDEO_MINIMIZE_ON_FOCUS_LOSS, c"0");
    sdl::set_hint(SDL_HINT_AUDIO_DEVICE_SAMPLE_FRAMES, c"480");
    sdl::set_hint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, c"1");
    // Keep Bluetooth PlayStation/Switch controllers in enhanced HID report mode (e.g. DS4 0x11)
    // immediately on open so gyro/accel work when connected mid-stream (Nightjar #177).
    sdl::set_hint(SDL_HINT_JOYSTICK_ENHANCED_REPORTS, c"1");
    sdl::set_hint(SDL_HINT_AUTO_UPDATE_JOYSTICKS, c"0"); // Adaptive polling via PadPoll.
    sdl::set_hint(SDL_HINT_MOUSE_RELATIVE_MODE_CENTER, c"1");
    // The second screen's window opens beside the first stream without taking its keyboard and mouse.
    if second_screen::this_process() {
        sdl::set_hint(SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, c"0");
        sdl::set_hint(SDL_HINT_WINDOW_ACTIVATE_WHEN_RAISED, c"0");
    }

    // Straight to the display (KMS) or through the desktop, decided before SDL picks its video driver.
    #[cfg(target_os = "linux")]
    {
        let setting = Store::open().map(|store| store.read_only().settings().display_output).unwrap_or_default();
        let session = kms::Session::read(|name| std::env::var_os(name), std::path::Path::exists);
        let driver = std::env::var("SDL_VIDEO_DRIVER").ok();
        let decision = kms::decide(kms::flag(), setting, session, driver.as_deref());
        eprintln!("nightjar: kms: {} ({})", if decision.backend == kms::Backend::Kms { "drawing straight to the display" } else { "drawing through the desktop" }, decision.why);
        if let Some(drivers) = decision.driver_hint {
            let drivers = std::ffi::CString::new(drivers).expect("driver names have no NUL");
            sdl::set_hint(SDL_HINT_VIDEO_DRIVER, &drivers);
        }
        kms::set(decision.backend);
    }
    #[cfg(not(target_os = "linux"))]
    if let Some(flag) = kms::flag() {
        eprintln!("nightjar: kms: {} is ignored: only Linux draws straight to the display", if flag { kms::KMS_ARG } else { kms::NO_KMS_ARG });
    }
    // The second screen's stream takes no controllers: they stay with the first.
    let gamepads = if second_screen::this_process() { SDL_InitFlags::default() } else { SDL_INIT_GAMEPAD };
    let subsystems = SDL_INIT_VIDEO | gamepads | SDL_INIT_EVENTS;
    // On macOS the first stream opens Core Audio. Enumerating a cold audio driver
    // can otherwise hold the Cocoa thread before the launcher has drawn a window.
    #[cfg(not(target_os = "macos"))]
    let subsystems = subsystems | SDL_INIT_AUDIO;
    sdl::screen_sleeps_outside_streams();
    sdl::fullscreen_over_the_notch();
    // SAFETY: SDL initialisation on the main thread.
    unsafe {
        if !SDL_Init(subsystems) {
            #[cfg(target_os = "linux")]
            {
                if kms::active() {
                    let index = std::env::var("SDL_KMSDRM_DEVICE_INDEX").ok().and_then(|i| i.trim().parse().ok());
                    return Err(kms::start_failure(&sdl::error(), kms::drm::connected_displays(index)));
                }
                let session = kms::Session::read(|name| std::env::var_os(name), std::path::Path::exists);
                if !session.any() {
                    return Err(kms::desktop_failure(&sdl::error()));
                }
            }
            return Err(format!("SDL could not start: {}", sdl::error()));
        }
    }
    Ok(())
}

/// Chooses whether the launcher opens on SDL's GPU renderer and whether Vulkan video is active.
pub(crate) fn choose_renderer_gpu(
    settings: &config::Settings,
    hosts: &[config::HostRecord],
    profiles: &config::Profiles,
) -> (bool, bool) {
    #[cfg(any(target_os = "macos", windows))]
    let _ = (settings, hosts, profiles);
    #[cfg(any(target_os = "macos", windows))]
    let gpu = false; // Start the launcher in SDR (8-bit BGRA on Windows, Metal SDR on macOS); streams switch as needed.
    #[cfg(not(any(target_os = "macos", windows)))]
    let gpu = settings.needs_gpu() || hosts.iter()
        .any(|host| host.settings.as_ref().is_some_and(config::Overrides::needs_gpu) || host.app_settings.values().any(config::Overrides::needs_gpu))
        || profiles.entries.iter().any(|profile| profile.settings.needs_gpu());
    #[cfg(target_os = "linux")]
    let vulkan_choice = vulkan_video::Choice::from_environment();
    #[cfg(target_os = "linux")]
    let vulkan_video = vulkan_video_possible(any_hardware_decoding(settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings))), vulkan_choice, vaapi_map_switched_on());
    // The Renderer setting decides, and at Auto what needs the GPU renderer.
    #[cfg(target_os = "linux")]
    let gpu = handheld::starts_on_gpu(settings.renderer, settings.renderer == config::Renderer::Auto && (gpu || gpu_for_vulkan_video(vulkan_video, vulkan_choice)));
    // Straight to the display, OpenGL keeps DRM master where the video planes can share it.
    #[cfg(target_os = "linux")]
    let gpu = match kms::active().then(|| kms::renderer_for(settings.renderer, settings.video_planes)).flatten() {
        Some(gpu) => gpu,
        None => gpu,
    };
    #[cfg(not(target_os = "linux"))]
    let vulkan_video = false;
    (gpu, vulkan_video)
}

/// Creates the main SDL window, restores placement, creates the renderer and registers the wake event.
pub(crate) fn open_window_and_renderer(
    store: &Store,
    settings: &config::Settings,
    gpu: bool,
    vulkan_video: bool,
) -> Result<(*mut SDL_Window, *mut SDL_Renderer, u32, WindowMode), String> {
    // Steam's Game Mode, and straight to the display: fullscreen without a border from the start
    // (handheld.rs), at the display's current mode in KMS mode. On a desktop the Window setting
    // opens the launcher as it opens its streams' window: Fullscreen and Borderless fill the
    // screen on the desktop's mode, Windowed is an ordinary window.
    let launcher_mode = if second_screen::this_process() {
        WindowMode::Windowed
    } else {
        app::effective_launcher_window_mode(settings.window_mode, settings.windowed_launcher)
    };
    let start = handheld::start_window(launcher_mode, handheld::fills_screen());
    if handheld::gamescope() {
        eprintln!("nightjar: gamescope: starting fullscreen without a border; Start+Back opens the stream menu");
    }
    // SAFETY: display queries on the main thread after SDL_Init; the mode is SDL's and read at once.
    let (screen, os_scale) = unsafe {
        let mode = SDL_GetDesktopDisplayMode(SDL_GetPrimaryDisplay());
        ((!mode.is_null()).then(|| ((*mode).w, (*mode).h)), SDL_GetDisplayContentScale(SDL_GetPrimaryDisplay()))
    };
    // Auto only enlarges unscaled screens, where SDL's window size is in pixels and points alike.
    let zoom = match settings.ui_zoom {
        0 => config::auto_zoom(screen.map(|(_, h)| h as f32), os_scale),
        _ => 100,
    };
    let (width, height) = handheld::start_size(kms::active(), screen, zoom);
    // SAFETY: window and renderer creation on the main thread.
    let (window, renderer, wake_event) = unsafe {
        let window = SDL_CreateWindow(
            c"Nightjar".as_ptr(),
            width,
            height,
            start_flags(start),
        );
        if window.is_null() {
            return Err(format!("No window: {}", sdl::error()));
        }
        window_place::set_minimum_size(window);
        if let Some(point) = second_screen::point() {
            if !window_place::move_to_display_at(window, point) {
                return Err(format!("No monitor at {},{} for the second screen: {}", point.0, point.1, sdl::error()));
            }
        } else if start.restore_place {
            window_place::apply(window, store.window_placement().as_ref());
        }
        let renderer =
            sdl::create_renderer_with(window, gpu, vulkan_video);
        if renderer.is_null() {
            return Err(format!("No renderer: {}", sdl::error()));
        }
        SDL_SetRenderVSync(renderer, 1);
        (window, renderer, SDL_RegisterEvents(1))
    };
    // Windows windows take the executable's icon (build.rs) in every size the shell asks for.
    #[cfg(not(windows))]
    sdl::set_icon(window, &theme::logo_rgba(64), 64);
    sdl::exclude_from_capture_if_asked(window);
    // Before the window shows, so its first frame already has the dark bar; again when the palette changes.
    blend_title_bar(window, *theme::NIGHT, *theme::ON_SURFACE);
    Ok((window, renderer, wake_event, launcher_mode))
}

/// Announces direct touch screens to egui before the window shows so finger-drag scrolling works.
pub(crate) fn prime_gui_touch(gui: &mut Gui) {
    // A launcher on a touch screen scrolls by dragging from the first finger: egui offers that
    // once it has seen a touch, so one is announced before the window shows (gui::prime_touch).
    // SAFETY: SDL is up; the list SDL returns is freed here.
    unsafe {
        let mut count = 0;
        let ids = SDL_GetTouchDevices(&mut count);
        let screen = !ids.is_null()
            && (0..count.max(0)).any(|i| SDL_GetTouchDeviceType(*ids.add(i as usize)).0 == SDL_TOUCH_DEVICE_DIRECT.0);
        if !ids.is_null() {
            SDL_free(ids.cast());
        }
        // SDL on Windows lists a touch screen only after its first WM_TOUCH; Windows knows sooner.
        #[cfg(windows)]
        let screen = screen
            || windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_MAXIMUMTOUCHES) > 0;
        gui.prime_touch(screen);
    }
}

/// Probes display tearing / VRR support for the launcher and logs the active renderer and display.
pub(crate) fn configure_display_tearing(
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
    launcher: &mut Launcher,
    display: Display,
) {
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    #[cfg(windows)]
    {
        launcher.tearing_supported =
            media::swapchain_allows_tearing(sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
    }
    #[cfg(target_os = "linux")]
    {
        launcher.tearing_supported = true;
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: SDL's live window, on the main thread.
        let timing = unsafe {
            mac_display::of_window(SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_COCOA_WINDOW_POINTER, std::ptr::null_mut()))
        };
        let variable = timing.is_some_and(|timing| timing.variable_refresh());
        match timing {
            Some(timing) => {
                let (slowest, fastest) = timing.range_hz();
                eprintln!("nightjar: the display refreshes at {slowest:.0} to {fastest:.0} Hz{}", if variable { ", a variable refresh rate" } else { "" });
            }
            None => eprintln!("nightjar: the display reports no refresh range"),
        }
        // Real VRR on a Mac is built and was not seen on a display: `NIGHTJAR_MAC_VRR=1` turns
        // it on for a display with a refresh range, where a stream then presents synchronised at
        // the PC's frame times (`stream::present::sync_interval`).
        launcher.tearing_supported = variable && stream::env_flag("NIGHTJAR_MAC_VRR") == Some(true);
        if launcher.tearing_supported { eprintln!("nightjar: Real VRR on this display (NIGHTJAR_MAC_VRR)"); }
    }
    launcher.gpu_device = renderer_gpu_device(renderer).map(|d| d.0 as usize);
    // Off Windows the swapchain's present mode decides tearing, logged when a stream starts.
    eprintln!(
        "nightjar: renderer {}, display {}x{} at {:.2} Hz{}",
        sdl::renderer_name(renderer),
        display.width,
        display.height,
        display.refresh_hz,
        if cfg!(windows) { format!(", tearing {}", launcher.tearing_supported) } else { String::new() }
    );
    #[cfg(windows)]
    if let Some(gpu) = gpu_adapters::renderer_gpu(sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)) {
        eprintln!("nightjar: the renderer draws and D3D11VA decodes on {gpu}");
    }
}

/// Local wall-clock time as `HH:MM` for the compact performance overlay.
pub(crate) fn local_clock_hh_mm() -> String {
    #[cfg(windows)]
    {
        let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!("{:02}:{:02}", st.wHour, st.wMinute)
    }
    // POSIX `localtime_r` resolves the device's local timezone on Unix desktop targets.
    #[cfg(unix)]
    {
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        if !unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
            format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
        } else {
            String::new()
        }
    }
    // Other non-Windows, non-Unix targets omit the local clock field.
    #[cfg(not(any(windows, unix)))]
    {
        String::new()
    }
}
