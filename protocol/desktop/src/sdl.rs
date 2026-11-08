//! Small helpers over sdl3-sys.

use std::ffi::{c_char, CStr};
use std::path::Path;

use sdl3_sys::everything::*;

#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// For an ignored test that opens a window: true, after saying why, when this session has no way
/// to show one (no `DISPLAY`, no `XDG_RUNTIME_DIR` for Wayland, no `SDL_VIDEO_DRIVER` chosen), so the
/// test skips instead of failing in `SDL_Init`.
#[cfg(test)]
pub fn skip_without_display() -> bool {
    let unset = |name: &str| std::env::var_os(name).is_none_or(|v| v.is_empty());
    let none = cfg!(unix) && unset("XDG_RUNTIME_DIR") && unset("DISPLAY") && unset("SDL_VIDEO_DRIVER");
    if none {
        eprintln!("skipped: no display here (XDG_RUNTIME_DIR, DISPLAY and SDL_VIDEO_DRIVER are all unset); run it in a desktop session");
    }
    none
}

/// For an ignored test that reads an input named by `name`: its value, or None after saying that
/// the test skips (`what` says what to set it to).
#[cfg(test)]
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

/// While streaming, Alt+F4 goes to the PC's game instead of closing Nightjar, as in Moonlight
/// (and V+): the stream's own quit shortcut and the window's close button still end it.
pub fn alt_f4_to_host(streaming: bool) {
    set_hint(SDL_HINT_WINDOWS_CLOSE_ON_ALT_F4, if streaming { c"0" } else { c"1" });
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

/// The renderer's backend name ("direct3d11", "opengl", ...).
pub fn renderer_name(renderer: *mut SDL_Renderer) -> String {
    // SAFETY: a live renderer; the name is a static string.
    unsafe {
        let name = SDL_GetRendererName(renderer);
        if name.is_null() {
            return String::new();
        }
        CStr::from_ptr(name).to_string_lossy().into_owned()
    }
}

pub fn renderer_pointer(renderer: *mut SDL_Renderer, name: *const c_char) -> *mut std::ffi::c_void {
    // SAFETY: a live renderer and a property name constant.
    unsafe { SDL_GetPointerProperty(SDL_GetRendererProperties(renderer), name, std::ptr::null_mut()) }
}

/// The GPU renderer's present-mode properties (`vendor/sdl3-src/patches/tearing-present.patch`).
#[cfg(target_os = "linux")]
const PREFER_IMMEDIATE_PROPERTY: &CStr = c"SDL.renderer.gpu.prefer_immediate";
#[cfg(target_os = "linux")]
const PRESENT_MODE_PROPERTY: &CStr = c"SDL.renderer.gpu.present_mode";

/// Asks the GPU renderer to try IMMEDIATE before MAILBOX the next time V-Sync turns off. Other
/// renderers ignore it.
#[cfg(target_os = "linux")]
pub fn prefer_immediate(renderer: *mut SDL_Renderer, prefer: bool) {
    // SAFETY: a live renderer and a property name constant.
    unsafe { SDL_SetBooleanProperty(SDL_GetRendererProperties(renderer), PREFER_IMMEDIATE_PROPERTY.as_ptr(), prefer) };
}

/// The GPU renderer's present mode in use, as SDL_GPUPresentMode's number; None on other
/// renderers, which never set it.
#[cfg(target_os = "linux")]
pub fn present_mode(renderer: *mut SDL_Renderer) -> Option<i64> {
    // SAFETY: a live renderer and a property name constant.
    let mode = unsafe { SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), PRESENT_MODE_PROPERTY.as_ptr(), -1) };
    (mode >= 0).then_some(mode)
}

/// The GPU renderer's window-target properties (`vendor/sdl3-src/patches/window-target.patch`):
/// a frame draws straight into its swapchain image unless the back buffer is asked for.
const READ_BACK_PROPERTY: &CStr = c"SDL.renderer.gpu.read_back";
#[cfg(target_os = "linux")]
const WINDOW_TARGET_FRAMES_PROPERTY: &CStr = c"SDL.renderer.gpu.window_target_frames";

/// Keeps the GPU renderer's next frames in its back buffer (`read_back`), so that
/// `SDL_RenderReadPixels` can read the window's target (a screenshot); set before the frame draws.
/// Other renderers, and Windows' unpatched SDL, ignore it.
pub fn read_back_frames(renderer: *mut SDL_Renderer, read_back: bool) {
    // SAFETY: a live renderer and a property name constant.
    unsafe { SDL_SetBooleanProperty(SDL_GetRendererProperties(renderer), READ_BACK_PROPERTY.as_ptr(), read_back) };
}

/// How many frames the GPU renderer has drawn straight into the swapchain; 0 on other renderers.
#[cfg(target_os = "linux")]
pub fn window_target_frames(renderer: *mut SDL_Renderer) -> i64 {
    // SAFETY: a live renderer and a property name constant.
    unsafe { SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), WINDOW_TARGET_FRAMES_PROPERTY.as_ptr(), 0) }
}

/// SDL's video driver ("wayland", "x11", ...), empty before SDL_Init.
#[cfg(target_os = "linux")]
pub fn video_driver() -> String {
    // SAFETY: SDL returns a static string or null.
    unsafe {
        let name = SDL_GetCurrentVideoDriver();
        if name.is_null() { String::new() } else { CStr::from_ptr(name).to_string_lossy().into_owned() }
    }
}

/// SDL_GPUPresentMode's name, with what it means for tearing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn present_mode_name(mode: Option<i64>) -> &'static str {
    match mode {
        Some(0) => "V-Sync (FIFO)",
        Some(1) => "Immediate (tears)",
        Some(2) => "Mailbox (no tearing)",
        Some(_) => "unknown",
        None => "set by the renderer",
    }
}

pub fn hdr_available(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: The renderer is live.
    let enabled = unsafe { SDL_GetBooleanProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, false) };
    enabled && swapchain_takes_hdr(renderer)
}

/// The renderer's swapchain is an HDR one, whatever the window's HDR state is right now.
pub fn swapchain_takes_hdr(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: The renderer and its associated window/device are live.
    unsafe {
        let props = SDL_GetRendererProperties(renderer);
        let device: *mut SDL_GPUDevice = SDL_GetPointerProperty(
            props,
            SDL_PROP_RENDERER_GPU_DEVICE_POINTER,
            std::ptr::null_mut(),
        )
        .cast();
        if device.is_null() {
            return crate::handheld::renderer_takes_hdr(cfg!(windows), false, false);
        }
        let window = SDL_GetRenderWindow(renderer);
        let format = SDL_GetGPUSwapchainTextureFormat(device, window);
        // scRGB, or HDR10 through the patched SDL's PQ present (vendor/sdl3-src/patches/pq-present.patch):
        // only that swapchain is RGB10A2.
        (SDL_WindowSupportsGPUSwapchainComposition(device, window, SDL_GPU_SWAPCHAINCOMPOSITION_HDR_EXTENDED_LINEAR)
            && format == SDL_GPU_TEXTUREFORMAT_R16G16B16A16_FLOAT)
            || (SDL_WindowSupportsGPUSwapchainComposition(device, window, SDL_GPU_SWAPCHAINCOMPOSITION_HDR10_ST2084)
                && format == SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM)
    }
}

/// The swapchain colour spaces the window of a GPU renderer is offered (under gamescope, by its
/// WSI layer). None for other renderers.
fn offered(renderer: *mut SDL_Renderer) -> crate::handheld::Offered {
    // SAFETY: a live renderer; its device and window outlive the call.
    unsafe {
        let device: *mut SDL_GPUDevice =
            SDL_GetPointerProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_GPU_DEVICE_POINTER, std::ptr::null_mut()).cast();
        if device.is_null() {
            return Default::default();
        }
        let window = SDL_GetRenderWindow(renderer);
        crate::handheld::Offered {
            scrgb: SDL_WindowSupportsGPUSwapchainComposition(device, window, SDL_GPU_SWAPCHAINCOMPOSITION_HDR_EXTENDED_LINEAR),
            hdr10: SDL_WindowSupportsGPUSwapchainComposition(device, window, SDL_GPU_SWAPCHAINCOMPOSITION_HDR10_ST2084),
        }
    }
}

/// A display of the compositor has HDR on, by what has arrived so far: a Wayland compositor answers
/// SDL about its outputs in its own time, so this takes what is waiting first. A compositor with
/// only frog colour management tells a window, never SDL about a display: this stays false there.
pub fn any_display_hdr() -> bool {
    // SAFETY: SDL's video is initialized on this thread; the list is SDL's copy, freed here.
    unsafe {
        SDL_PumpEvents();
        let mut count = 0;
        let displays = SDL_GetDisplays(&mut count);
        if displays.is_null() {
            return false;
        }
        let hdr = std::slice::from_raw_parts(displays, count.max(0) as usize)
            .iter()
            .any(|&display| SDL_GetBooleanProperty(SDL_GetDisplayProperties(display), SDL_PROP_DISPLAY_HDR_ENABLED_BOOLEAN, false));
        SDL_free(displays.cast());
        hdr
    }
}

/// A saved setting of a PC, game or profile asks for HDR. Set before the first renderer: an SDR-only
/// user keeps the SDR swapchain on a window with no HDR report yet, whatever another display has.
static HDR_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_hdr_asked(asked: bool) {
    HDR_ASKED.store(asked, std::sync::atomic::Ordering::Relaxed);
}

/// How long a stream's start waits for the compositor's HDR report on a window.
const WINDOW_HDR_REPORT_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Waits briefly for the window's HDR report when it can still come (`handheld::hdr_report_can_come`).
/// Whether HDR output is available now. Events stay queued for the event loop.
pub fn await_window_hdr(renderer: *mut SDL_Renderer) -> bool {
    if !crate::handheld::hdr_report_can_come(swapchain_takes_hdr(renderer), any_display_hdr()) {
        return false;
    }
    let deadline = std::time::Instant::now() + WINDOW_HDR_REPORT_WAIT;
    while std::time::Instant::now() < deadline {
        // SAFETY: SDL's video is initialized on this thread.
        unsafe { SDL_PumpEvents() };
        if hdr_available(renderer) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// What an HDR stream refused here can do about it.
pub fn hdr_refusal(renderer: *mut SDL_Renderer) -> crate::handheld::HdrRefusal {
    crate::handheld::hdr_refusal(swapchain_takes_hdr(renderer), any_display_hdr())
}

/// The last renderer's output decision on Linux (`make_renderer`); None on Windows.
static OUTPUT: std::sync::Mutex<Option<Result<crate::handheld::Output, crate::handheld::SdrReason>>> = std::sync::Mutex::new(None);

pub fn output_decision() -> Option<Result<crate::handheld::Output, crate::handheld::SdrReason>> {
    *OUTPUT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The PQ present pass's shaders (`pq_present.hlsl`), handed to the patched SDL.
const PQ_VERTEX_SPIRV: &[u8] = include_bytes!("pq_present_vertex.spv");
const PQ_FRAGMENT_SPIRV: &[u8] = include_bytes!("pq_present_fragment.spv");

/// Sets (or with `on` false clears) the PQ present's create properties of the patched SDL.
///
/// # Safety
/// `props` is a live property set.
unsafe fn set_pq_present(props: SDL_PropertiesID, on: bool) {
    let shaders = [(c"SDL.renderer.create.gpu.pq_vertex_spirv", c"SDL.renderer.create.gpu.pq_vertex_spirv_size", PQ_VERTEX_SPIRV),
        (c"SDL.renderer.create.gpu.pq_fragment_spirv", c"SDL.renderer.create.gpu.pq_fragment_spirv_size", PQ_FRAGMENT_SPIRV)];
    for (code, size, spirv) in shaders {
        if on {
            SDL_SetPointerProperty(props, code.as_ptr(), spirv.as_ptr().cast_mut().cast());
            SDL_SetNumberProperty(props, size.as_ptr(), spirv.len() as i64);
        } else {
            SDL_ClearProperty(props, code.as_ptr());
            SDL_ClearProperty(props, size.as_ptr());
        }
    }
}

/// Nightjar set the window's HDR state itself ([`set_window_hdr`]): a later renderer for the
/// window must not take it for the compositor's, and an SDR fallback puts it back.
static WINDOW_HDR_SET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets the window's HDR state SDL's renderer draws for: under gamescope SDL has none to read
/// (`handheld::gamescope_hdr_properties`), and an SDR fallback puts it back.
///
/// # Safety
/// `window` is live.
unsafe fn set_window_hdr(window: *mut SDL_Window, hdr: Option<(f32, f32)>) {
    let props = SDL_GetWindowProperties(window);
    let (white, headroom) = hdr.unwrap_or((1.0, 1.0));
    SDL_SetFloatProperty(props, SDL_PROP_WINDOW_SDR_WHITE_LEVEL_FLOAT, white);
    SDL_SetFloatProperty(props, SDL_PROP_WINDOW_HDR_HEADROOM_FLOAT, headroom);
    SDL_SetBooleanProperty(props, SDL_PROP_WINDOW_HDR_ENABLED_BOOLEAN, hdr.is_some());
    WINDOW_HDR_SET.store(hdr.is_some(), std::sync::atomic::Ordering::Relaxed);
}

/// Linux's GPU renderer `renderer`, made from `props` with linear output: the HDR output for its
/// window (`handheld::plan`) or null for SDR. scRGB keeps it, on a desktop with [`hdr_available`]
/// following the window's state once the compositor reports it (made again under gamescope, after
/// the window's HDR state is set); PQ makes it again with the PQ present. Logs the renderer, the
/// colour spaces offered and the decision, which [`output_decision`] keeps.
///
/// # Safety
/// `window` and `renderer` are live, `props` is the renderer's create properties.
unsafe fn hdr_output(window: *mut SDL_Window, props: SDL_PropertiesID, renderer: *mut SDL_Renderer) -> *mut SDL_Renderer {
    use crate::handheld::{self, Output};
    let gamescope = handheld::gamescope();
    let window_hdr = handheld::window_hdr_reported(
        SDL_GetBooleanProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_HDR_ENABLED_BOOLEAN, false),
        WINDOW_HDR_SET.load(std::sync::atomic::Ordering::Relaxed),
    );
    let offered = offered(renderer);
    let name = renderer_name(renderer);
    let display_hdr = any_display_hdr();
    let hdr_asked = HDR_ASKED.load(std::sync::atomic::Ordering::Relaxed);
    let plan = handheld::plan(gamescope, window_hdr, display_hdr, hdr_asked, offered);
    let mut decision = plan.decision;
    let mut renderer = renderer;
    let mut pq_error = None;
    if let Ok(output) = decision {
        if plan.set_window_hdr {
            set_window_hdr(window, Some(handheld::gamescope_hdr_properties()));
        }
        // SDL reads the window's HDR state when it makes the renderer.
        if plan.rebuild {
            SDL_DestroyRenderer(renderer);
            if output == Output::Pq { set_pq_present(props, true); }
            renderer = SDL_CreateRendererWithProperties(props);
            if output == Output::Pq {
                set_pq_present(props, false);
                if renderer.is_null() { pq_error = Some(error()); }
            }
        }
        decision = handheld::settle(output, !renderer.is_null(), !renderer.is_null() && swapchain_takes_hdr(renderer));
    }
    if decision.is_err() {
        if !renderer.is_null() { SDL_DestroyRenderer(renderer); }
        renderer = std::ptr::null_mut();
        if WINDOW_HDR_SET.load(std::sync::atomic::Ordering::Relaxed) { set_window_hdr(window, None); }
    }
    let line = handheld::describe(&name, gamescope, window_hdr, display_hdr, hdr_asked, offered, decision);
    match pq_error {
        Some(why) => eprintln!("nightjar: {line} ({why})"),
        None => eprintln!("nightjar: {line}"),
    }
    *OUTPUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(decision);
    renderer
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
fn title_bar_attributes(background: egui::Color32, text: egui::Color32) -> [(i32, u32); 4] {
    let colorref = |c: egui::Color32| u32::from(c.r()) | u32::from(c.g()) << 8 | u32::from(c.b()) << 16;
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
pub fn blend_title_bar(window: *mut SDL_Window, background: egui::Color32, text: egui::Color32) {
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

pub fn display_caps(renderer: *mut SDL_Renderer) -> rubylight_protocol::control::DisplayCaps {
    let caps = rubylight_protocol::control::DisplayCaps {
        hdr: hdr_available(renderer),
        ..Default::default()
    };
    #[cfg(windows)]
    {
        use windows::core::Interface;
        use windows::Win32::Graphics::Dxgi::{IDXGIOutput6, IDXGISwapChain};
        let chain = renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let desc = unsafe {
            IDXGISwapChain::from_raw_borrowed(&chain)
                .and_then(|chain| chain.GetContainingOutput().ok())
                .and_then(|output| output.cast::<IDXGIOutput6>().ok())
                .and_then(|output| output.GetDesc1().ok())
        };
        if let Some(desc) = desc {
            let measured = rubylight_protocol::control::DisplayCaps {
                max_centinits: (desc.MaxLuminance * 100.0).round() as u32,
                max_average_centinits: (desc.MaxFullFrameLuminance * 100.0).round() as u32,
                min_decimillinits: (desc.MinLuminance * 10_000.0).round() as u32,
                ..caps
            };
            if rubylight_protocol::control::DisplayCaps::decode(&measured.encode()).is_some() {
                return measured;
            }
        }
    }
    caps
}

/// The display's luminance for the launch query of hosts other than Rubylight, from the same
/// output description as [`display_caps`] (Moonlight V+ PC sends the DXGI values too). The
/// desktop has no HDR/SDR ratio to estimate a missing peak from, so such a display sends none.
pub fn launch_luminance(renderer: *mut SDL_Renderer) -> Option<nightjar_core::http::DisplayLuminance> {
    luminance_of(&display_caps(renderer))
}

fn luminance_of(caps: &rubylight_protocol::control::DisplayCaps) -> Option<nightjar_core::http::DisplayLuminance> {
    nightjar_core::http::DisplayLuminance::from_display(
        caps.max_centinits as f32 / 100.0,
        caps.min_decimillinits as f32 / 10_000.0,
        caps.max_average_centinits as f32 / 100.0,
        f32::NAN,
    )
}

/// The tests' renderer: `create_renderer_with` without Vulkan video.
#[cfg(test)]
pub fn create_renderer(window: *mut SDL_Window, gpu: bool) -> *mut SDL_Renderer {
    create_renderer_with(window, gpu, false)
}

/// `create_renderer`, with the GPU renderer's device on Linux shared with PyroWave's GPU decoder
/// when its library is there, and with FFmpeg's Vulkan video when `vulkan_video` (a stream can
/// decode with it).
pub fn create_renderer_with(window: *mut SDL_Window, gpu: bool, vulkan_video: bool) -> *mut SDL_Renderer {
    #[cfg(unix)]
    let pyrowave = gpu && crate::stream::pyrowave_sdk::dll_path().is_some_and(|path| path.exists());
    #[cfg(not(unix))]
    let pyrowave = false;
    create_renderer_for(window, gpu, pyrowave, vulkan_video)
}

/// `create_renderer`, on Linux with `pyrowave` asking for the Vulkan features PyroWave's GPU decoder
/// needs and, apart from them, with `vulkan_video` the video extensions and decode queues FFmpeg's
/// Vulkan video needs, to decode on the renderer's own device (`zero_copy.rs`, `vulkan_video.rs`).
/// When the devices lack what is asked for, or the renderer cannot be made with it, it is made as
/// without either, and the reason is logged.
pub fn create_renderer_for(window: *mut SDL_Window, gpu: bool, pyrowave: bool, vulkan_video: bool) -> *mut SDL_Renderer {
    *GPU_FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    #[cfg(unix)]
    if gpu && (pyrowave || vulkan_video) {
        use crate::stream::pyrowave_sdk::zero_copy;
        match zero_copy::RendererRequest::new(pyrowave, vulkan_video) {
            Ok(mut request) => {
                let video = request.video();
                // The request outlives the call, which copies what it needs of it.
                let renderer = make_renderer(window, gpu, request.options(), !video.usable().is_empty());
                // SAFETY: A live renderer made with the request.
                if !renderer.is_null() && unsafe { request.mark(renderer) } {
                    return renderer;
                }
                let why = if renderer.is_null() { error() } else { format!("it draws with {:?}", renderer_name(renderer)) };
                // SAFETY: Made above, unused.
                if !renderer.is_null() { unsafe { SDL_DestroyRenderer(renderer) }; }
                let extensions: Vec<_> = video.names().iter().map(|name| name.to_string_lossy()).collect();
                zero_copy::note(&format!("the GPU renderer cannot be made with the Vulkan features {:?} and the video extensions {extensions:?} ({why}), so PyroWave GPU decoding will read its planes back and Vulkan video is not used", request.features().names()));
            }
            Err(why) => zero_copy::note(&format!("the GPU renderer is made without PyroWave's Vulkan features or Vulkan video, so PyroWave GPU decoding will read its planes back and Vulkan video is not used: {why}")),
        }
    }
    let _ = (pyrowave, vulkan_video);
    make_renderer(window, gpu, std::ptr::null_mut(), false)
}

/// Why the last GPU renderer asked for could not be made, when [`create_renderer_for`] made
/// another kind instead (or none).
static GPU_FAILURE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg_attr(windows, allow(dead_code))]
pub fn gpu_renderer_failure() -> Option<String> {
    GPU_FAILURE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `window` must be live and `vulkan` null or valid for the call. With `vulkan`, a GPU renderer
/// that cannot be made is not replaced by another kind; `decode_queues` asks the patched SDL for
/// its extra video decode queues (`vulkan_video::DECODE_QUEUES_PROPERTY`).
fn make_renderer(window: *mut SDL_Window, gpu: bool, vulkan: *mut SDL_GPUVulkanOptions, decode_queues: bool) -> *mut SDL_Renderer {
    // SAFETY: The window is live, `vulkan` null or valid, and all property strings are static.
    unsafe {
        let props = SDL_CreateProperties();
        if !vulkan.is_null() {
            SDL_SetPointerProperty(props, SDL_PROP_GPU_DEVICE_CREATE_VULKAN_OPTIONS_POINTER, vulkan.cast());
        }
        #[cfg(unix)]
        if decode_queues {
            SDL_SetBooleanProperty(props, crate::vulkan_video::DECODE_QUEUES_PROPERTY.as_ptr(), true);
        }
        #[cfg(not(unix))]
        let _ = decode_queues;
        SDL_SetPointerProperty(
            props,
            SDL_PROP_RENDERER_CREATE_WINDOW_POINTER,
            window.cast(),
        );
        if !cfg!(windows) && gpu {
            SDL_SetStringProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING, c"gpu".as_ptr());
            SDL_SetBooleanProperty(
                props,
                SDL_PROP_RENDERER_CREATE_GPU_SHADERS_SPIRV_BOOLEAN,
                true,
            );
        }
        SDL_SetNumberProperty(
            props,
            SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER,
            i64::from(if gpu {
                SDL_COLORSPACE_SRGB_LINEAR.0
            } else {
                SDL_COLORSPACE_SRGB.0
            }),
        );
        let mut renderer = SDL_CreateRendererWithProperties(props);
        if !cfg!(windows) && gpu && !renderer.is_null() {
            renderer = hdr_output(window, props, renderer);
        }
        if renderer.is_null() {
            SDL_SetNumberProperty(
                props,
                SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER,
                i64::from(SDL_COLORSPACE_SRGB.0),
            );
            renderer = SDL_CreateRendererWithProperties(props);
        }
        if renderer.is_null() && !cfg!(windows) && gpu && vulkan.is_null() {
            let why = error();
            SDL_ClearProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING);
            renderer = SDL_CreateRendererWithProperties(props);
            if !renderer.is_null() {
                eprintln!("nightjar: the GPU renderer could not be made ({why}), so Nightjar draws with {:?}: no HDR, PyroWave GPU decoding or shader passes", renderer_name(renderer));
            }
            *GPU_FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        }
        // Linux without the GPU renderer: no `hdr_output` decision, so an HDR stream's refusal
        // names the renderer instead of the compositor.
        if !cfg!(windows) && !renderer.is_null() && renderer_name(renderer) != "gpu" {
            *OUTPUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(crate::handheld::SdrReason::NoGpuRenderer));
        }
        SDL_DestroyProperties(props);
        renderer
    }
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
mod tests {
    use super::*;

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
        let night = egui::Color32::from_rgb(0x14, 0x16, 0x1F);
        let text = egui::Color32::from_rgb(0xE3, 0xE4, 0xEE);
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
}

#[cfg(test)]
#[path = "pq_present_tests.rs"]
mod pq_present_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "window_target_tests.rs"]
mod window_target_tests;

#[cfg(test)]
mod luminance_tests {
    use super::*;
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
