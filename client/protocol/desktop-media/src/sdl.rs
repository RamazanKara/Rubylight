//! Small helpers over sdl3-sys.

use std::ffi::{c_char, CStr};
use std::path::Path;

use sdl3_sys::everything::*;

mod gpu_present;
mod hdr;
mod renderer;

pub use gpu_present::*;
pub use hdr::*;
pub use renderer::*;

#[cfg(test)]
use gpu_present::{PQ_FRAGMENT_SPIRV, PQ_VERTEX_SPIRV, SCANOUT_FRAGMENT_SPIRV};
#[cfg(all(test, target_os = "linux"))]
use gpu_present::set_pq_present;

/// A wheel turn as the launcher and the PC take it. SDL 3.4 already applies the system's natural
/// scrolling to `x` and `y` on macOS and Wayland, where the compositor adjusts the values, and
/// reports it in `direction` only (SDL_MOUSEWHEEL_FLIPPED: "multiply by -1 to change them back");
/// turning it back scrolled against the system setting on both (moonlight-qt keeps SDL's values).
/// Windows and X11 never set the flag.
pub fn wheel_turn(w: &SDL_MouseWheelEvent) -> (f32, f32) {
    (w.x, w.y)
}

#[cfg(any(test, feature = "test-support"))]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// For an ignored test that opens a window: true, after saying why, when this session has no way
/// to show one (no `DISPLAY`, no `XDG_RUNTIME_DIR` for Wayland, no `SDL_VIDEO_DRIVER` chosen), so the
/// test skips instead of failing in `SDL_Init`.
#[cfg(test)]
pub fn skip_without_display() -> bool {
    let unset = |name: &str| std::env::var_os(name).is_none_or(|v| v.is_empty());
    let none = cfg!(target_os = "linux") && unset("XDG_RUNTIME_DIR") && unset("DISPLAY") && unset("SDL_VIDEO_DRIVER");
    if none {
        eprintln!("skipped: no display here (XDG_RUNTIME_DIR, DISPLAY and SDL_VIDEO_DRIVER are all unset); run it in a desktop session");
    }
    none
}

/// For an ignored test that reads an input named by `name`: its value, or None after saying that
/// the test skips (`what` says what to set it to).
#[cfg(any(test, feature = "test-support"))]
pub fn test_input(name: &str, what: &str) -> Option<std::ffi::OsString> {
    let value = std::env::var_os(name).filter(|v| !v.is_empty());
    if value.is_none() {
        eprintln!("skipped: {name} is not set; set it to {what}");
    }
    value
}

/// SDL's last error message.
pub fn error() -> String {
    // SAFETY: SDL_GetError always returns a valid string.
    unsafe { CStr::from_ptr(SDL_GetError()) }
        .to_string_lossy()
        .into_owned()
}

/// Whether the system asks apps to be light (`SDL_GetSystemTheme`: Windows' app mode, macOS'
/// appearance, the desktop portal's colour scheme): the System theme's day look. An unknown answer
/// is dark, the look Nightjar always had.
pub fn system_light() -> bool {
    // SAFETY: a query without arguments, on the main thread after SDL_Init.
    unsafe { SDL_GetSystemTheme() == SDL_SYSTEM_THEME_LIGHT }
}

/// Opens `url` in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = std::ffi::CString::new(url).map_err(|e| e.to_string())?;
    // SAFETY: a NUL-terminated string that outlives the call.
    if unsafe { SDL_OpenURL(url.as_ptr()) } {
        Ok(())
    } else {
        Err(error())
    }
}

pub type FileChoice = std::sync::mpsc::Receiver<Result<Option<std::path::PathBuf>, String>>;

/// What a file dialog's callback was given: an error, nothing chosen, or the first path.
unsafe fn chosen_path(files: *const *const c_char) -> Result<Option<std::path::PathBuf>, String> {
    if files.is_null() { Err(error()) }
    else if unsafe { (*files).is_null() } { Ok(None) }
    else { Ok(Some(std::path::PathBuf::from(unsafe { CStr::from_ptr(*files) }.to_string_lossy().into_owned()))) }
}

pub fn choose_json_file(window: *mut SDL_Window, save: bool) -> FileChoice {
    struct Request {
        sender: std::sync::mpsc::Sender<Result<Option<std::path::PathBuf>, String>>,
        filter: SDL_DialogFileFilter,
    }
    unsafe extern "C" fn chosen(userdata: *mut std::ffi::c_void, files: *const *const c_char, _: i32) {
        // SDL calls once; the request keeps the filter alive until this callback.
        let request = unsafe { Box::from_raw(userdata.cast::<Request>()) };
        let _ = request.sender.send(unsafe { chosen_path(files) });
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let request = Box::into_raw(Box::new(Request {
        sender,
        filter: SDL_DialogFileFilter { name: c"JSON files".as_ptr(), pattern: c"json".as_ptr() },
    }));
    // SAFETY: invoked on the UI thread; SDL owns the callback's lifetime.
    unsafe {
        if save {
            SDL_ShowSaveFileDialog(Some(chosen), request.cast(), window, &(*request).filter, 1, c"nightjar-shortcuts.json".as_ptr());
        } else {
            SDL_ShowOpenFileDialog(Some(chosen), request.cast(), window, &(*request).filter, 1, std::ptr::null(), false);
        }
    }
    receiver
}

/// Where to save the latency CSV (Settings → Advanced → Export latency CSV), Android's
/// `nightjar-latency.csv` offered by name.
pub fn choose_csv_file(window: *mut SDL_Window) -> FileChoice {
    struct Request {
        sender: std::sync::mpsc::Sender<Result<Option<std::path::PathBuf>, String>>,
        filter: SDL_DialogFileFilter,
    }
    unsafe extern "C" fn chosen(userdata: *mut std::ffi::c_void, files: *const *const c_char, _: i32) {
        // SDL calls once; the request keeps the filter alive until this callback.
        let request = unsafe { Box::from_raw(userdata.cast::<Request>()) };
        let _ = request.sender.send(unsafe { chosen_path(files) });
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let request = Box::into_raw(Box::new(Request {
        sender,
        filter: SDL_DialogFileFilter { name: c"CSV files".as_ptr(), pattern: c"csv".as_ptr() },
    }));
    // SAFETY: invoked on the UI thread; SDL owns the callback's lifetime.
    unsafe {
        SDL_ShowSaveFileDialog(Some(chosen), request.cast(), window, &(*request).filter, 1, c"nightjar-latency.csv".as_ptr());
    }
    receiver
}

/// A folder (`folder`) or a file of any type, to open.
pub fn choose_path(window: *mut SDL_Window, folder: bool) -> FileChoice {
    type Sender = std::sync::mpsc::Sender<Result<Option<std::path::PathBuf>, String>>;
    unsafe extern "C" fn chosen(userdata: *mut std::ffi::c_void, files: *const *const c_char, _: i32) {
        // SDL calls once.
        let sender = unsafe { Box::from_raw(userdata.cast::<Sender>()) };
        let _ = sender.send(unsafe { chosen_path(files) });
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let request = Box::into_raw(Box::new(sender));
    // SAFETY: invoked on the UI thread; SDL owns the callback's lifetime.
    unsafe {
        if folder {
            SDL_ShowOpenFolderDialog(Some(chosen), request.cast(), window, std::ptr::null(), false);
        } else {
            SDL_ShowOpenFileDialog(Some(chosen), request.cast(), window, std::ptr::null(), 0, std::ptr::null(), false);
        }
    }
    receiver
}

/// The files a callback gets from [`choose_files`]: none when the dialog was closed.
pub type FilesChosen = Box<dyn FnOnce(Result<Vec<std::path::PathBuf>, String>) + Send>;

/// Shows the system's dialog to open any number of files of any type. `done` gets them, on
/// whatever thread SDL calls back on.
pub fn choose_files(window: *mut SDL_Window, done: FilesChosen) {
    unsafe extern "C" fn chosen(userdata: *mut std::ffi::c_void, files: *const *const c_char, _: i32) {
        // SDL calls once: NULL on an error, else a NULL-terminated list (empty when closed).
        let done = unsafe { Box::from_raw(userdata.cast::<FilesChosen>()) };
        let result = if files.is_null() {
            Err(error())
        } else {
            let mut paths = Vec::new();
            let mut at = files;
            // SAFETY: SDL's list ends with NULL and lives until this callback returns.
            while let Some(file) = unsafe { (*at).as_ref() } {
                paths.push(std::path::PathBuf::from(unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned()));
                at = unsafe { at.add(1) };
            }
            Ok(paths)
        };
        done(result);
    }
    let request = Box::into_raw(Box::new(done));
    // SAFETY: invoked on the UI thread; SDL owns the callback's lifetime.
    unsafe { SDL_ShowOpenFileDialog(Some(chosen), request.cast(), window, std::ptr::null(), 0, std::ptr::null(), true) };
}

pub fn set_hint(name: *const c_char, value: &CStr) {
    // SAFETY: both are NUL-terminated strings.
    unsafe { SDL_SetHint(name, value.as_ptr()) };
}

/// macOS only: SDL's value for `SDL_HINT_VIDEO_MAC_FULLSCREEN_SPACES`, or None where it does not
/// apply. Fullscreen through a Space (SDL's default) stops below the camera housing of a notched
/// MacBook, so the stream is shrunk (nightjar issue 136); SDL's own fullscreen (a borderless
/// window over the whole display, above the menu bar) draws into the notch area. The cost: a
/// fullscreen window gets no Space of its own and no Mission Control entry, and the window's
/// green button zooms instead of entering fullscreen. SDL reads it once, in `SDL_Init`.
pub fn fullscreen_spaces_hint(macos: bool) -> Option<&'static CStr> {
    macos.then_some(c"0")
}

/// Before `SDL_Init` (see `fullscreen_spaces_hint`).
pub fn fullscreen_over_the_notch() {
    if let Some(value) = fullscreen_spaces_hint(cfg!(target_os = "macos")) {
        set_hint(SDL_HINT_VIDEO_MAC_FULLSCREEN_SPACES, value);
    }
}

/// While streaming, Alt+F4 goes to the PC's game instead of closing Nightjar, as in Moonlight
/// (and V+): the stream's own quit shortcut and the window's close button still end it.
pub fn alt_f4_to_host(streaming: bool) {
    set_hint(SDL_HINT_WINDOWS_CLOSE_ON_ALT_F4, if streaming { c"0" } else { c"1" });
}

/// Before SDL starts: SDL would keep the screen awake for as long as a window exists ("Playing a
/// game" on a Linux desktop, nightjar#132), so the launcher lets it sleep and
/// [`keep_screen_awake`] holds it only while a stream runs.
pub fn screen_sleeps_outside_streams() {
    set_hint(SDL_HINT_VIDEO_ALLOW_SCREENSAVER, c"1");
    set_hint(SDL_HINT_SCREENSAVER_INHIBIT_ACTIVITY_NAME, c"Streaming a game");
}

/// While streaming the screen stays awake: the desktop counts no controller input as activity.
pub fn keep_screen_awake(streaming: bool) {
    // SAFETY: plain SDL calls; without the video subsystem they only return false.
    unsafe {
        if streaming {
            SDL_DisableScreenSaver();
        } else {
            SDL_EnableScreenSaver();
        }
    }
}

/// How long the launcher keeps the screen awake after the last controller input or the end of a
/// stream: the desktop counts no controller input as activity, so a couch player browsing games
/// with a controller would see the screen dim and blank.
pub const LAUNCHER_AWAKE: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The launcher's hold on the screen: awake for [`LAUNCHER_AWAKE`] after controller input or a
/// stream, then free to sleep.
#[derive(Default)]
pub struct LauncherAwake {
    until: Option<std::time::Instant>,
}

impl LauncherAwake {
    /// Controller input arrived, or a stream ended.
    pub fn active(&mut self, now: std::time::Instant) {
        self.until = Some(now + LAUNCHER_AWAKE);
    }

    /// Whether the launcher holds the screen awake at `now`.
    pub fn wants(&self, now: std::time::Instant) -> bool {
        self.until.is_some_and(|until| now < until)
    }

    /// Sets SDL's screen saver to [`Self::wants`], on each launcher frame (never during a stream,
    /// which holds the screen itself).
    pub fn apply(&self, now: std::time::Instant) {
        let want = self.wants(now);
        // SAFETY: a plain SDL query.
        if want == unsafe { SDL_ScreenSaverEnabled() } {
            keep_screen_awake(want);
        }
    }
}

/// The system's preferred languages, most preferred first, as lower-case codes with the country
/// when the system names one ("de", "pt-br", "zh-tw").
pub fn preferred_languages() -> Vec<String> {
    let mut count = 0;
    let mut out = Vec::new();
    // SAFETY: SDL returns a NULL-terminated array of locales that we free, or NULL.
    unsafe {
        let list = SDL_GetPreferredLocales(&mut count);
        if list.is_null() {
            return out;
        }
        for i in 0..usize::try_from(count).unwrap_or(0) {
            let locale = *list.add(i);
            if !locale.is_null() && !(*locale).language.is_null() {
                let mut code = CStr::from_ptr((*locale).language).to_string_lossy().to_ascii_lowercase();
                if !(*locale).country.is_null() {
                    let country = CStr::from_ptr((*locale).country).to_string_lossy().to_ascii_lowercase();
                    if !country.is_empty() {
                        code = format!("{code}-{country}");
                    }
                }
                out.push(code);
            }
        }
        SDL_free(list.cast());
    }
    out
}

/// Measurement runs on one PC (`NIGHTJAR_EXCLUDE_FROM_CAPTURE=1`): keeps the window out of
/// screen capture, so a host capturing this display does not see the stream it is sending and
/// the client's presents do not change the host's capture rate. No effect elsewhere.
pub fn exclude_from_capture_if_asked(window: *mut SDL_Window) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
        if !std::env::var_os("NIGHTJAR_EXCLUDE_FROM_CAPTURE").is_some_and(|v| v == "1") {
            return;
        }
        // SAFETY: a live window; the property is its HWND.
        let hwnd = unsafe { SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut()) };
        // SAFETY: the window belongs to this process, which is what the call requires.
        if let Err(error) = unsafe { SetWindowDisplayAffinity(HWND(hwnd), WDA_EXCLUDEFROMCAPTURE) } {
            eprintln!("nightjar: could not keep the window out of screen capture: {error}");
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// The title bar's DWM attributes, in the order they are set: dark mode first, which Windows 10
/// has (20H1 and later as 20, earlier as [`title_bar_fallback`]), then the caption, border and title colours, which only Windows 11 has.
/// Colours are COLORREFs (0x00BBGGRR). The border takes the background too, so a maximized or
/// snapped window shows one dark surface from the caption down.
#[cfg_attr(not(windows), allow(dead_code))]
fn title_bar_attributes(background: [u8; 3], text: [u8; 3]) -> [(i32, u32); 4] {
    let colorref = |c: [u8; 3]| u32::from(c[0]) | u32::from(c[1]) << 8 | u32::from(c[2]) << 16;
    // DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR.
    [(20, 1), (34, colorref(background)), (35, colorref(background)), (36, colorref(text))]
}

/// The number an older Windows knows a refused title bar attribute by: Windows 10 1809 to 1909
/// take immersive dark mode only as 19, before it became 20.
#[cfg_attr(not(windows), allow(dead_code))]
fn title_bar_fallback(attribute: i32) -> Option<i32> {
    (attribute == 20).then_some(19)
}

/// What a refused title bar attribute needs, for the log.
#[cfg_attr(not(windows), allow(dead_code))]
fn title_bar_needs(attribute: i32) -> &'static str {
    if attribute == 20 { "Windows 10 1809 or later" } else { "Windows 11" }
}

/// Gives the Windows title bar the launcher's colours: dark mode and the page's background as the
/// caption, so a maximized window does not wear a white or accent-coloured bar over the dark page.
/// The page stays opaque: Mica would only show where the caption colour is not set, and the
/// caption matching the page is the point. Windows 10 takes dark mode only; each attribute it
/// refuses is logged once per call (calls come only when the palette changes). Elsewhere a no-op.
pub fn blend_title_bar(window: *mut SDL_Window, background: [u8; 3], text: [u8; 3]) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        // SAFETY: a live window; the property is its HWND.
        let hwnd = unsafe { SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut()) };
        if hwnd.is_null() {
            eprintln!("nightjar: title bar colours: the window has no HWND");
            return;
        }
        // SAFETY: the window belongs to this process; the value is a 4-byte BOOL or COLORREF.
        let apply = |attribute: i32, value: u32| unsafe { DwmSetWindowAttribute(HWND(hwnd), DWMWINDOWATTRIBUTE(attribute), (&raw const value).cast(), 4) };
        for (attribute, value) in title_bar_attributes(background, text) {
            let set = apply(attribute, value).or_else(|error| match title_bar_fallback(attribute) {
                Some(older) => apply(older, value).map_err(|_| error),
                None => Err(error),
            });
            if let Err(error) = set {
                eprintln!("nightjar: title bar attribute {attribute} not set (needs {}): {error}", title_bar_needs(attribute));
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (window, background, text);
}

/// The window's icon from straight RGBA pixels (Windows uses the executable's).
#[cfg(not(windows))]
pub fn set_icon(window: *mut SDL_Window, rgba: &[u8], size: i32) {
    // SAFETY: the surface borrows `rgba` only for the call.
    unsafe {
        let surface = SDL_CreateSurfaceFrom(
            size,
            size,
            SDL_PIXELFORMAT_RGBA32,
            rgba.as_ptr() as *mut _,
            size * 4,
        );
        if !surface.is_null() {
            SDL_SetWindowIcon(window, surface);
            SDL_DestroySurface(surface);
        }
    }
}

/// Saves the renderer's current back buffer as a PNG (call before presenting).
pub fn screenshot(renderer: *mut SDL_Renderer, path: &Path) -> Result<(), String> {
    // SAFETY: SDL surface calls on a live renderer; the surface is freed before returning.
    unsafe {
        let surface = SDL_RenderReadPixels(renderer, std::ptr::null());
        if surface.is_null() {
            return Err(error());
        }
        let rgba = SDL_ConvertSurface(surface, SDL_PIXELFORMAT_RGBA32);
        SDL_DestroySurface(surface);
        if rgba.is_null() {
            return Err(error());
        }
        let s = &*rgba;
        let (w, h, pitch) = (s.w as usize, s.h as usize, s.pitch as usize);
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let row = std::slice::from_raw_parts((s.pixels as *const u8).add(y * pitch), w * 4);
            pixels.extend_from_slice(row);
        }
        SDL_DestroySurface(rgba);
        for p in pixels.chunks_mut(4) {
            p[3] = 255;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        image::save_buffer(path, &pixels, w as u32, h as u32, image::ColorType::Rgba8).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "pq_present_tests.rs"]
mod pq_present_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "window_target_tests.rs"]
mod window_target_tests;
