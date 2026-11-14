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
    let none = cfg!(target_os = "linux") && unset("XDG_RUNTIME_DIR") && unset("DISPLAY") && unset("SDL_VIDEO_DRIVER");
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

/// Whether the renderer's backend is `name`, without `renderer_name`'s String.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn renderer_is(renderer: *mut SDL_Renderer, name: &str) -> bool {
    // SAFETY: a live renderer; the name is a static string.
    unsafe {
        let backend = SDL_GetRendererName(renderer);
        !backend.is_null() && CStr::from_ptr(backend).to_bytes() == name.as_bytes()
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
/// renderers ignore it. A stream that accepts tearing under VRR ([`set_vrr_tearing`]) always asks.
#[cfg(target_os = "linux")]
pub fn prefer_immediate(renderer: *mut SDL_Renderer, prefer: bool) {
    let prefer = prefer || VRR_TEARING.load(std::sync::atomic::Ordering::Relaxed);
    // SAFETY: a live renderer and a property name constant.
    unsafe { SDL_SetBooleanProperty(SDL_GetRendererProperties(renderer), PREFER_IMMEDIATE_PROPERTY.as_ptr(), prefer) };
}

/// The coming stream presents IMMEDIATE under VRR too (`stream::wl_scanout::vrr_tearing`: without
/// Tear-free VRR), where `present::prefers_immediate` keeps MAILBOX for a Wayland desktop's VRR.
#[cfg(target_os = "linux")]
static VRR_TEARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set before the stream's first [`prefer_immediate`].
#[cfg(target_os = "linux")]
pub fn set_vrr_tearing(on: bool) {
    VRR_TEARING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The window properties of `vendor/sdl3-src/patches/wayland-scanout.patch`.
#[cfg(target_os = "linux")]
const PASS_THROUGH_PROPERTY: &CStr = c"SDL.window.gpu.vulkan.hdr10_pass_through";
#[cfg(target_os = "linux")]
const PASS_THROUGH_ACTIVE_PROPERTY: &CStr = c"SDL.window.gpu.vulkan.hdr10_pass_through_active";
#[cfg(target_os = "linux")]
const RECREATE_ON_SUBOPTIMAL_PROPERTY: &CStr = c"SDL.window.gpu.vulkan.recreate_on_suboptimal";
#[cfg(target_os = "linux")]
const SUBOPTIMAL_RECREATES_PROPERTY: &CStr = c"SDL.window.gpu.vulkan.suboptimal_recreates";

/// The swapchain of `window`'s GPU renderer, for the end-of-stream scanout line: Mesa's DRM
/// format for it, its present mode, how often the patched SDL made it again for the compositor's
/// dmabuf feedback, and whether its HDR10 is pass-through. Empty without a GPU renderer.
#[cfg(target_os = "linux")]
pub fn swap_facts(window: *mut SDL_Window) -> crate::stream::wl_scanout::SwapFacts {
    use crate::stream::wl_scanout::{drm_fourcc, SwapFacts, SwapFormat};
    if window.is_null() {
        return SwapFacts::default();
    }
    // SAFETY: a live window; its renderer and device, if any, too.
    unsafe {
        let renderer = SDL_GetRenderer(window);
        if renderer.is_null() {
            return SwapFacts::default();
        }
        let window_props = SDL_GetWindowProperties(window);
        let device: *mut SDL_GPUDevice = renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
        let format = (!device.is_null()).then(|| SDL_GetGPUSwapchainTextureFormat(device, window)).and_then(|format| {
            if format == SDL_GPU_TEXTUREFORMAT_B8G8R8A8_UNORM || format == SDL_GPU_TEXTUREFORMAT_B8G8R8A8_UNORM_SRGB {
                Some(SwapFormat::Bgra8)
            } else if format == SDL_GPU_TEXTUREFORMAT_R8G8B8A8_UNORM || format == SDL_GPU_TEXTUREFORMAT_R8G8B8A8_UNORM_SRGB {
                Some(SwapFormat::Rgba8)
            } else if format == SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM {
                Some(SwapFormat::Rgb10a2)
            } else if format == SDL_GPU_TEXTUREFORMAT_R16G16B16A16_FLOAT {
                Some(SwapFormat::Rgba16f)
            } else {
                None
            }
        });
        SwapFacts {
            // SDL's swapchains are VK_COMPOSITE_ALPHA_OPAQUE, which Mesa gives a format without alpha.
            fourcc: format.map(|format| drm_fourcc(format, true)),
            present_mode: present_mode(renderer),
            recreates: SDL_GetNumberProperty(window_props, SUBOPTIMAL_RECREATES_PROPERTY.as_ptr(), 0),
            pass_through: SDL_GetBooleanProperty(window_props, PASS_THROUGH_ACTIVE_PROPERTY.as_ptr(), false),
        }
    }
}

/// Draws the launcher on a PQ renderer at the white the compositor shows as its own SDR white
/// (`stream::wl_scanout::launcher_color_scale`; `launcher`: no stream on screen, which keeps 1.0),
/// and lets Nightjar's HDR description follow the output. Whether the scale changed, so the frame
/// is drawn again.
#[cfg(target_os = "linux")]
pub fn launcher_white(renderer: *mut SDL_Renderer, launcher: bool) -> bool {
    use crate::stream::wl_scanout as scanout;
    if renderer.is_null() {
        return false;
    }
    scanout::pump_installed();
    let pq_output = matches!(output_decision(), Some(Ok(crate::handheld::Output::Pq)));
    // SAFETY: a live renderer on the thread that draws with it.
    unsafe {
        let window = SDL_GetRenderWindow(renderer);
        let white = SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0);
        // The reference white the surface's description declares: Nightjar's, else the driver's;
        // while the stream's thread holds the description, the scale stays as it is.
        let described = if window.is_null() { Some(None) } else { scanout::installed_reference(window) };
        let Some(described) = described else { return false };
        let described = described.unwrap_or(scanout::PQ_DEFAULT_REFERENCE_NITS);
        let want = scanout::launcher_color_scale(pq_output, crate::handheld::gamescope(), described, launcher, scanout::env_on(scanout::LAUNCHER_WHITE_ENV), white);
        let mut have = 1.0;
        if !SDL_GetRenderColorScale(renderer, &mut have) || (have - want).abs() < 1e-4 {
            return false;
        }
        SDL_SetRenderColorScale(renderer, want)
    }
}

/// Drops Nightjar's HDR description of `window` and its pass-through ask, before any renderer is
/// made for it: a swapchain the driver describes must find no description of Nightjar's there.
///
/// # Safety
/// `window` is live.
#[cfg(target_os = "linux")]
unsafe fn forget_pass_through(window: *mut SDL_Window) {
    crate::stream::wl_scanout::uninstall(window);
    SDL_ClearProperty(SDL_GetWindowProperties(window), PASS_THROUGH_PROPERTY.as_ptr());
    crate::stream::wl_scanout::set_label(None);
}

/// The PQ renderer for `window` from `props` (with the PQ present set). On a Wayland desktop it
/// asks for a pass-through HDR10 swapchain (Mesa's driver) and describes the surface itself
/// (`stream::wl_scanout::HdrDescription`), matched to the output; where that cannot be, the driver
/// describes it as before, and the end-of-stream line says why.
///
/// # Safety
/// `window` is live, `props` the renderer's create properties.
#[cfg(any(not(target_os = "macos"), test))]
unsafe fn pq_renderer(window: *mut SDL_Window, props: SDL_PropertiesID) -> *mut SDL_Renderer {
    #[cfg(target_os = "linux")]
    {
        use crate::stream::wl_scanout::{self as scanout, HdrDescription, Label};
        let refused = scanout::description_refused(
            video_driver() == "wayland",
            crate::handheld::gamescope(),
            scanout::env_on(scanout::HDR_DESCRIPTION_ENV),
            scanout::hdr_wsi_layer(std::env::var("ENABLE_HDR_WSI").ok().as_deref()),
        );
        let why = match refused.map_or_else(|| HdrDescription::prepare(window), |why| Err(why.to_owned())) {
            Ok(mut description) => {
                let window_props = SDL_GetWindowProperties(window);
                // Kept while the description lives: a swapchain made again (a resize) stays pass-through.
                SDL_SetBooleanProperty(window_props, PASS_THROUGH_PROPERTY.as_ptr(), true);
                // The present that can also draw in the compositor's blending space; PQ until
                // the description says otherwise (attach).
                set_scanout_present(props, true);
                let renderer = SDL_CreateRendererWithProperties(props);
                set_scanout_present(props, false);
                if !renderer.is_null() && SDL_GetBooleanProperty(window_props, PASS_THROUGH_ACTIVE_PROPERTY.as_ptr(), false) {
                    description.attach();
                    scanout::set_label(Some(description.log_label()));
                    scanout::install(description);
                    return renderer;
                }
                let why = if renderer.is_null() { error() } else { "the swapchain is not HDR10".to_owned() };
                if !renderer.is_null() {
                    crate::sdl::destroy_renderer(renderer);
                }
                SDL_ClearProperty(window_props, PASS_THROUGH_PROPERTY.as_ptr());
                why
            }
            Err(why) => why,
        };
        let renderer = SDL_CreateRendererWithProperties(props);
        scanout::set_label(Some(Label::Driver(why)));
        renderer
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        SDL_CreateRendererWithProperties(props)
    }
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

/// Destroys `renderer` and forgets the cached swapchain answers, so a renderer made later at the
/// same address asks its swapchain again.
///
/// # Safety
/// `renderer` is live and not used after this.
pub unsafe fn destroy_renderer(renderer: *mut SDL_Renderer) {
    forget_swapchain_answers();
    // SAFETY: the caller's.
    unsafe { SDL_DestroyRenderer(renderer) }
}

pub fn hdr_available(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: The renderer is live.
    let enabled = unsafe { SDL_GetBooleanProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, false) };
    enabled && swapchain_takes_hdr(renderer)
}

/// What a GPU renderer's swapchain answer depends on, read with cheap property queries: the device,
/// the window's display and its HDR, the window's HDR request and its size in pixels. A change of
/// any of them can change the answer, so the cache keeps an answer only for the state it was asked in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Compared as a whole (PartialEq); no field is read on its own.
pub struct SwapchainState {
    device: usize,
    display: u32,
    display_hdr: bool,
    window_hdr: bool,
    width: i32,
    height: i32,
}

/// The most renderers whose swapchain answers are kept; the oldest goes first.
const SWAPCHAIN_ANSWERS_KEPT: usize = 8;

/// The swapchain answers of GPU renderers, one per renderer, each kept for the state it was asked
/// in. `forget_all` drops them all (a display changed); an answer computed across a `forget_all`
/// is not stored, by the generation it was started in.
#[derive(Debug, Default)]
pub struct SwapchainAnswers {
    generation: u64,
    entries: Vec<(usize, SwapchainState, bool)>,
}

impl SwapchainAnswers {
    pub const fn new() -> Self {
        Self { generation: 0, entries: Vec::new() }
    }

    /// The generation to hand `store` with the answer computed for it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The answer kept for `renderer` in exactly `state`.
    pub fn get(&self, renderer: usize, state: SwapchainState) -> Option<bool> {
        self.entries
            .iter()
            .find(|(kept, _, _)| *kept == renderer)
            .and_then(|&(_, kept, answer)| (kept == state).then_some(answer))
    }

    /// Keeps `answer` for `renderer` in `state`, unless `forget_all` came since `generation`.
    pub fn store(&mut self, generation: u64, renderer: usize, state: SwapchainState, answer: bool) {
        if generation != self.generation {
            return;
        }
        self.entries.retain(|(kept, _, _)| *kept != renderer);
        if self.entries.len() >= SWAPCHAIN_ANSWERS_KEPT {
            self.entries.remove(0);
        }
        self.entries.push((renderer, state, answer));
    }

    pub fn forget_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
    }
}

static SWAPCHAIN_ANSWERS: std::sync::Mutex<SwapchainAnswers> = std::sync::Mutex::new(SwapchainAnswers::new());

/// Drops the cached swapchain answers: a display's HDR or mode changed, or a window moved to
/// another display (`SDL_EVENT_WINDOW_HDR_STATE_CHANGED`, `SDL_EVENT_WINDOW_DISPLAY_CHANGED`,
/// `SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED`). The next `swapchain_takes_hdr` asks SDL again.
pub fn forget_swapchain_answers() {
    SWAPCHAIN_ANSWERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).forget_all();
}

/// The renderer's swapchain is an HDR one, whatever the window's HDR state is right now. The
/// answer is cached per renderer and state (`SwapchainState`): SDL's composition and format
/// queries each make Vulkan surface queries, and a picture asks several times.
pub fn swapchain_takes_hdr(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: The renderer and its associated window/device are live.
    unsafe {
        let props = SDL_GetRendererProperties(renderer);
        #[cfg(target_os = "macos")]
        if renderer_is(renderer, "metal") {
            return SDL_GetNumberProperty(props, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0)
                == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0);
        }
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
        let state = swapchain_state(device, window);
        let (generation, cached) = {
            let answers = SWAPCHAIN_ANSWERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            (answers.generation(), answers.get(renderer as usize, state))
        };
        if let Some(answer) = cached {
            return answer;
        }
        let format = SDL_GetGPUSwapchainTextureFormat(device, window);
        // scRGB, or HDR10 through the patched SDL's PQ present (vendor/sdl3-src/patches/pq-present.patch):
        // only that swapchain is RGB10A2.
        let answer = (SDL_WindowSupportsGPUSwapchainComposition(device, window, SDL_GPU_SWAPCHAINCOMPOSITION_HDR_EXTENDED_LINEAR)
            && format == SDL_GPU_TEXTUREFORMAT_R16G16B16A16_FLOAT)
            || (SDL_WindowSupportsGPUSwapchainComposition(device, window, SDL_GPU_SWAPCHAINCOMPOSITION_HDR10_ST2084)
                && format == SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM);
        SWAPCHAIN_ANSWERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).store(generation, renderer as usize, state, answer);
        answer
    }
}

/// What the swapchain answer of `window`, drawn by `device`, is asked in (see `SwapchainState`).
/// `device` and `window` are the live GPU device and window of a renderer (`swapchain_takes_hdr`).
fn swapchain_state(device: *mut SDL_GPUDevice, window: *mut SDL_Window) -> SwapchainState {
    // SAFETY: a live window and its display; SDL's property names are constants.
    unsafe {
        let display = SDL_GetDisplayForWindow(window);
        let (mut width, mut height) = (0, 0);
        SDL_GetWindowSizeInPixels(window, &mut width, &mut height);
        SwapchainState {
            device: device as usize,
            display: display.0,
            display_hdr: SDL_GetBooleanProperty(SDL_GetDisplayProperties(display), SDL_PROP_DISPLAY_HDR_ENABLED_BOOLEAN, false),
            window_hdr: SDL_GetBooleanProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_HDR_ENABLED_BOOLEAN, false),
            width,
            height,
        }
    }
}

/// The swapchain colour spaces the window of a GPU renderer is offered (under gamescope, by its
/// WSI layer). None for other renderers.
#[cfg(any(not(target_os = "macos"), test))]
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

/// `VkHdrMetadataEXT`: what the patched SDL hands `vkSetHdrMetadataEXT` for an HDR10 swapchain
/// (`vendor/sdl3-src/patches/window-hdr-metadata.patch`). Chromaticities as CIE 1931 x, y; nits.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct SwapchainHdrMetadata {
    s_type: u32,
    p_next: *const std::ffi::c_void,
    pub primaries: [[f32; 2]; 3],
    pub white_point: [f32; 2],
    pub max_luminance: f32,
    pub min_luminance: f32,
    pub max_content_light_level: f32,
    pub max_frame_average_light_level: f32,
}

/// VK_STRUCTURE_TYPE_HDR_METADATA_EXT.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const HDR_METADATA_EXT: u32 = 1_000_105_000;

/// The host's mastering metadata for the swapchain, its light levels capped at `output_peak` (nits):
/// SDL's renderer tone maps the picture to the window's headroom first, so the compositor must not
/// map what is left down a second time. None leaves them as the host sent them. Zero (unknown)
/// light levels stay zero. None when the mastering range is empty (a peak at or below the minimum,
/// as zeroed metadata from the host has): compositors end a client sending that.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn swapchain_hdr_metadata(metadata: &nightjar_core::control::HdrMetadata, output_peak: Option<f32>) -> Option<SwapchainHdrMetadata> {
    let xy = |(x, y): (u16, u16)| [f32::from(x) / 50_000.0, f32::from(y) / 50_000.0];
    let cap = |nits: u16| output_peak.map_or(f32::from(nits), |peak| f32::from(nits).min(peak));
    let metadata = SwapchainHdrMetadata {
        s_type: HDR_METADATA_EXT,
        p_next: std::ptr::null(),
        primaries: metadata.display_primaries.map(xy),
        white_point: xy(metadata.white_point),
        max_luminance: cap(metadata.max_display_luminance),
        min_luminance: f32::from(metadata.min_display_luminance) / 10_000.0,
        max_content_light_level: cap(metadata.max_content_light_level),
        max_frame_average_light_level: cap(metadata.max_frame_average_light_level),
    };
    (metadata.max_luminance > metadata.min_luminance).then_some(metadata)
}

/// The peak the GPU renderer draws HDR up to, in nits, once the window's HDR is on: its SDR white
/// (in 80-nit units) times its headroom. None before that, or on other renderers.
#[cfg(target_os = "linux")]
pub fn output_peak_nits(renderer: *mut SDL_Renderer) -> Option<f32> {
    if !hdr_available(renderer) {
        return None;
    }
    // SAFETY: a live renderer and SDL's property names.
    let (white, headroom) = unsafe {
        let props = SDL_GetRendererProperties(renderer);
        (SDL_GetFloatProperty(props, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0), SDL_GetFloatProperty(props, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0))
    };
    let peak = 80.0 * white * headroom;
    (peak.is_finite() && peak > 0.0).then_some(peak)
}

/// The content's peak in `metadata` (nits), as the driver's description would have it: MaxCLL,
/// else the mastering peak; None when neither is known.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn content_peak(metadata: Option<&SwapchainHdrMetadata>) -> Option<f32> {
    let metadata = metadata?;
    [metadata.max_content_light_level, metadata.max_luminance].into_iter().find(|nits| nits.is_finite() && *nits > 0.0)
}

/// Hands `metadata` to the patched SDL for the window's HDR10 swapchain, which passes it to
/// `vkSetHdrMetadataEXT` on its next frame and on each new swapchain (with VK_EXT_hdr_metadata;
/// other swapchains ignore it). None is unknown metadata: nothing is passed on from then, and a
/// swapchain that had some keeps it until it is made again (Vulkan cannot take it back).
#[cfg(target_os = "linux")]
pub fn set_swapchain_hdr_metadata(window: *mut SDL_Window, metadata: Option<SwapchainHdrMetadata>) {
    // Nightjar's description that follows the desktop gives the compositor the same peak.
    crate::stream::wl_scanout::set_content_peak(content_peak(metadata.as_ref()));
    unsafe extern "C" fn free(_: *mut std::ffi::c_void, value: *mut std::ffi::c_void) {
        // SAFETY: allocated with SDL_malloc below; SDL calls this once, when the value goes.
        unsafe { SDL_free(value) };
    }
    // SAFETY: a live window; the copy is SDL's to free with the property.
    unsafe {
        let props = SDL_GetWindowProperties(window);
        match metadata {
            Some(metadata) => {
                let copy = SDL_malloc(std::mem::size_of::<SwapchainHdrMetadata>()).cast::<SwapchainHdrMetadata>();
                if copy.is_null() {
                    eprintln!("nightjar: the HDR10 swapchain metadata could not be stored: {}", error());
                    return;
                }
                copy.write(metadata);
                SDL_SetPointerPropertyWithCleanup(props, c"SDL.window.gpu.hdr_metadata_pointer".as_ptr(), copy.cast(), Some(free), std::ptr::null_mut());
            }
            None => {
                SDL_ClearProperty(props, c"SDL.window.gpu.hdr_metadata_pointer".as_ptr());
            }
        }
        let serial = SDL_GetNumberProperty(props, c"SDL.window.gpu.hdr_metadata_serial".as_ptr(), 0) + 1;
        SDL_SetNumberProperty(props, c"SDL.window.gpu.hdr_metadata_serial".as_ptr(), serial);
    }
}

/// The window of a GPU renderer is offered an HDR10 (ST 2084) swapchain.
#[cfg_attr(windows, allow(dead_code))]
#[cfg(not(target_os = "macos"))]
pub fn hdr10_offered(renderer: *mut SDL_Renderer) -> bool {
    offered(renderer).hdr10
}

/// Whether the renderer's swapchain offers scRGB, HDR's fallback once PQ failed.
#[cfg_attr(windows, allow(dead_code))]
#[cfg(not(target_os = "macos"))]
pub fn scrgb_offered(renderer: *mut SDL_Renderer) -> bool {
    offered(renderer).scrgb
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

/// A PQ renderer could not be made this run: its fallback stays, and no later renderer tries PQ
/// again (`handheld::pq_unused`).
#[cfg(any(not(target_os = "macos"), test))]
static PQ_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg_attr(windows, allow(dead_code))]
#[cfg(any(not(target_os = "macos"), test))]
pub fn pq_failed() -> bool {
    PQ_FAILED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The renderers made from here on are for an HDR stream's start (`handheld::choose`): only they
/// may present as HDR10 PQ.
static HDR_STREAM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg_attr(windows, allow(dead_code))]
pub fn set_hdr_stream(hdr_stream: bool) {
    HDR_STREAM.store(hdr_stream, std::sync::atomic::Ordering::Relaxed);
}

/// The PQ present pass's shaders (`pq_present.hlsl`), handed to the patched SDL.
#[cfg(any(not(target_os = "macos"), test))]
const PQ_VERTEX_SPIRV: &[u8] = include_bytes!("pq_present_vertex.spv");
#[cfg(any(not(target_os = "macos"), test))]
const PQ_FRAGMENT_SPIRV: &[u8] = include_bytes!("pq_present_fragment.spv");

/// Sets (or with `on` false clears) the PQ present's create properties of the patched SDL.
///
/// # Safety
/// `props` is a live property set.
#[cfg(any(not(target_os = "macos"), test))]
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

/// The present of a pass-through HDR10 swapchain (`scanout_present.hlsl`): PQ as
/// `pq_present.hlsl`, or gamma 2.2 in the compositor's blending space, as the renderer properties
/// of [`set_present_encoding`] say on each present.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SCANOUT_FRAGMENT_SPIRV: &[u8] = include_bytes!("scanout_present_fragment.spv");

/// The patched SDL's properties of the present's encoding (`pq-present.patch`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const PRESENT_ENCODING_PROPERTY: &CStr = c"SDL.renderer.gpu.present_encoding";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const PRESENT_MIN_NITS_PROPERTY: &CStr = c"SDL.renderer.gpu.present_min_nits";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const PRESENT_MAX_NITS_PROPERTY: &CStr = c"SDL.renderer.gpu.present_max_nits";

/// Swaps the PQ present's fragment shader for the scanout one (with its constant buffer), or with
/// `on` false back; set with [`set_pq_present`] on.
///
/// # Safety
/// `props` is a live property set.
#[cfg(target_os = "linux")]
unsafe fn set_scanout_present(props: SDL_PropertiesID, on: bool) {
    let fragment = if on { SCANOUT_FRAGMENT_SPIRV } else { PQ_FRAGMENT_SPIRV };
    SDL_SetPointerProperty(props, c"SDL.renderer.create.gpu.pq_fragment_spirv".as_ptr(), fragment.as_ptr().cast_mut().cast());
    SDL_SetNumberProperty(props, c"SDL.renderer.create.gpu.pq_fragment_spirv_size".as_ptr(), fragment.len() as i64);
    SDL_SetNumberProperty(props, c"SDL.renderer.create.gpu.pq_fragment_uniform_buffers".as_ptr(), i64::from(on));
}

/// What the present of `renderer` writes into its pass-through swapchain from its next present
/// (the scanout present and a video-only frame read it); other renderers ignore it.
#[cfg(target_os = "linux")]
pub fn set_present_encoding(renderer: *mut SDL_Renderer, encoding: crate::stream::wl_scanout::Encoding) {
    if renderer.is_null() {
        return;
    }
    let (mode, min_nits, max_nits) = encoding.properties();
    // SAFETY: a live renderer and property name constants; SDL's properties take any thread.
    unsafe {
        let props = SDL_GetRendererProperties(renderer);
        SDL_SetNumberProperty(props, PRESENT_ENCODING_PROPERTY.as_ptr(), mode);
        SDL_SetFloatProperty(props, PRESENT_MIN_NITS_PROPERTY.as_ptr(), min_nits);
        SDL_SetFloatProperty(props, PRESENT_MAX_NITS_PROPERTY.as_ptr(), max_nits);
    }
}

/// The constants the scanout present gets for `renderer` ([`set_present_encoding`],
/// `wl_scanout::Encoding::constants`): all 0 (PQ) without them, and off Linux.
#[cfg_attr(windows, allow(dead_code))]
pub fn present_encoding(renderer: *mut SDL_Renderer) -> [f32; 4] {
    #[cfg(target_os = "linux")]
    if !renderer.is_null() {
        // SAFETY: a live renderer and property name constants.
        let (mode, min_nits, max_nits) = unsafe {
            let props = SDL_GetRendererProperties(renderer);
            (
                SDL_GetNumberProperty(props, PRESENT_ENCODING_PROPERTY.as_ptr(), 0),
                SDL_GetFloatProperty(props, PRESENT_MIN_NITS_PROPERTY.as_ptr(), 0.0),
                SDL_GetFloatProperty(props, PRESENT_MAX_NITS_PROPERTY.as_ptr(), 0.0),
            )
        };
        return crate::stream::wl_scanout::Encoding::constants(mode, min_nits, max_nits);
    }
    let _ = renderer;
    [0.0; 4]
}

/// Nightjar set the window's HDR state itself ([`set_window_hdr`]): a later renderer for the
/// window must not take it for the compositor's, and an SDR fallback puts it back.
#[cfg(any(not(target_os = "macos"), test))]
static WINDOW_HDR_SET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets the window's HDR state SDL's renderer draws for: under gamescope SDL has none to read
/// (`handheld::gamescope_hdr_properties`), and an SDR fallback puts it back.
///
/// # Safety
/// `window` is live.
#[cfg(any(not(target_os = "macos"), test))]
unsafe fn set_window_hdr(window: *mut SDL_Window, hdr: Option<(f32, f32)>) {
    let props = SDL_GetWindowProperties(window);
    let (white, headroom) = hdr.unwrap_or((1.0, 1.0));
    SDL_SetFloatProperty(props, SDL_PROP_WINDOW_SDR_WHITE_LEVEL_FLOAT, white);
    SDL_SetFloatProperty(props, SDL_PROP_WINDOW_HDR_HEADROOM_FLOAT, headroom);
    SDL_SetBooleanProperty(props, SDL_PROP_WINDOW_HDR_ENABLED_BOOLEAN, hdr.is_some());
    WINDOW_HDR_SET.store(hdr.is_some(), std::sync::atomic::Ordering::Relaxed);
}

/// Sets, before a renderer for `window` is made, the SDR white the patched SDL's renderer draws
/// its linear output with (`handheld::renderer_sdr_white`); None leaves it the window's own level.
/// SDL reads it when it makes the renderer and on each HDR report for the window.
///
/// # Safety
/// `window` is live.
#[cfg(any(not(target_os = "macos"), test))]
unsafe fn set_renderer_sdr_white(window: *mut SDL_Window, white: Option<f32>) {
    let props = SDL_GetWindowProperties(window);
    let name = crate::handheld::RENDERER_SDR_WHITE_PROPERTY.as_ptr();
    match white {
        Some(white) => SDL_SetFloatProperty(props, name, white),
        None => SDL_ClearProperty(props, name),
    };
}

/// What the log adds to a window's HDR report (`handheld::describe_levels`): the compositor's SDR
/// white and headroom for the window, and the SDR white its renderer draws with.
pub fn window_hdr_levels(window: SDL_WindowID) -> String {
    // SAFETY: SDL's video is initialized on this thread; a window that is gone has no levels.
    unsafe {
        let window = SDL_GetWindowFromID(window);
        if window.is_null() {
            return crate::handheld::describe_levels(1.0, 1.0, None);
        }
        let props = SDL_GetWindowProperties(window);
        let renderer = SDL_GetRenderer(window);
        let drawn = (!renderer.is_null()).then(|| SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0));
        crate::handheld::describe_levels(
            SDL_GetFloatProperty(props, SDL_PROP_WINDOW_SDR_WHITE_LEVEL_FLOAT, 1.0),
            SDL_GetFloatProperty(props, SDL_PROP_WINDOW_HDR_HEADROOM_FLOAT, 1.0),
            drawn,
        )
    }
}

/// Linux's GPU renderer `renderer`, made from `props` with linear output: the HDR output for its
/// window (`handheld::plan`) or null for SDR. scRGB keeps it, on a desktop with [`hdr_available`]
/// following the window's state once the compositor reports it (made again under gamescope, after
/// the window's HDR state is set); PQ (for an HDR stream's renderer, [`set_hdr_stream`], under
/// gamescope too unless `handheld::keeps_scrgb_first`) makes it
/// again with the PQ present, and once more as scRGB when that fails where scRGB is offered
/// (`handheld::fallback`), which then stays for the run ([`pq_failed`]). Logs the renderer, the
/// colour spaces offered and the decision, which [`output_decision`] keeps.
///
/// # Safety
/// `window` and `renderer` are live, `props` is the renderer's create properties.
#[cfg(any(not(target_os = "macos"), test))]
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
    let hdr_stream = HDR_STREAM.load(std::sync::atomic::Ordering::Relaxed);
    // Gamescope kept at scRGB (`NIGHTJAR_GAMESCOPE_PQ=0`) plans as for no HDR stream.
    let scrgb_first = handheld::keeps_scrgb_first();
    // PQ that failed once this run is not tried again: its fallback serves HDR streams. Gamescope
    // offering HDR10 alone has no other, and tries it each time as before.
    let pq_failed_before = hdr_stream && !scrgb_first && offered.hdr10 && (!gamescope || offered.scrgb) && pq_failed();
    let plan = handheld::plan(gamescope, window_hdr, display_hdr, hdr_asked, hdr_stream && !scrgb_first && !pq_failed_before, offered);
    let mut decision = plan.decision;
    // The stream asked for HDR and PQ failed before: say that, not that PQ is for HDR streams only.
    if pq_failed_before && decision == Err(handheld::SdrReason::PqWithoutHdrStream) {
        decision = Err(handheld::SdrReason::PqFailed);
    }
    let mut renderer = renderer;
    let mut pq_error = None;
    let mut pq_failed = false;
    if let Ok(output) = decision {
        if plan.set_window_hdr {
            set_window_hdr(window, Some(handheld::gamescope_hdr_properties()));
        }
        // SDL reads the window's HDR state when it makes the renderer.
        if plan.rebuild {
            crate::sdl::destroy_renderer(renderer);
            set_renderer_sdr_white(window, handheld::renderer_sdr_white(gamescope, output == Output::Pq));
            if output == Output::Pq {
                set_pq_present(props, true);
                renderer = pq_renderer(window, props);
            } else {
                renderer = SDL_CreateRendererWithProperties(props);
            }
            if output == Output::Pq {
                set_pq_present(props, false);
                if renderer.is_null() { pq_error = Some(error()); }
            }
        }
        decision = handheld::settle(output, !renderer.is_null(), !renderer.is_null() && swapchain_takes_hdr(renderer));
        if output == Output::Pq && decision.is_err() {
            PQ_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // A desktop's PQ that could not be made: scRGB where it is offered, not SDR.
        if let (Err(_), Some(next)) = (decision, handheld::fallback(output, offered)) {
            if !renderer.is_null() { crate::sdl::destroy_renderer(renderer); }
            #[cfg(target_os = "linux")]
            forget_pass_through(window);
            set_renderer_sdr_white(window, handheld::renderer_sdr_white(gamescope, next == Output::Pq));
            renderer = SDL_CreateRendererWithProperties(props);
            pq_failed = true;
            decision = handheld::settle(next, !renderer.is_null(), !renderer.is_null() && swapchain_takes_hdr(renderer));
        }
    }
    if decision.is_err() {
        if !renderer.is_null() { crate::sdl::destroy_renderer(renderer); }
        #[cfg(target_os = "linux")]
        forget_pass_through(window);
        renderer = std::ptr::null_mut();
        if WINDOW_HDR_SET.load(std::sync::atomic::Ordering::Relaxed) { set_window_hdr(window, None); }
    }
    let line = handheld::describe(&name, gamescope, window_hdr, display_hdr, hdr_asked, hdr_stream, offered, decision, pq_failed || pq_failed_before);
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
#[cfg(target_os = "macos")]
pub fn create_renderer_with(window: *mut SDL_Window, shaders: bool, _: bool) -> *mut SDL_Renderer {
    // The normal Metal renderer imports the decoder's IOSurface. Custom passes
    // use SDL's GPU renderer on Metal, which accepts our MSL fragment shaders.
    unsafe {
        let props = SDL_CreateProperties();
        SDL_SetPointerProperty(props, SDL_PROP_RENDERER_CREATE_WINDOW_POINTER, window.cast());
        SDL_SetStringProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING,
            if shaders { c"gpu".as_ptr() } else { c"metal".as_ptr() });
        if shaders {
            SDL_SetStringProperty(props, SDL_PROP_GPU_DEVICE_CREATE_NAME_STRING, c"metal".as_ptr());
            SDL_SetBooleanProperty(props, SDL_PROP_RENDERER_CREATE_GPU_SHADERS_MSL_BOOLEAN, true);
        }
        if HDR_STREAM.load(std::sync::atomic::Ordering::Relaxed) {
            SDL_SetNumberProperty(props, SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER, SDL_COLORSPACE_SRGB_LINEAR.0.into());
        }
        let mut renderer = SDL_CreateRendererWithProperties(props);
        if renderer.is_null() && !shaders {
            eprintln!("nightjar: Metal renderer unavailable ({}); trying SDL's default SDR renderer", error());
            SDL_ClearProperty(props, SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER);
            SDL_ClearProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING);
            renderer = SDL_CreateRendererWithProperties(props);
        }
        SDL_DestroyProperties(props);
        renderer
    }
}

#[cfg(not(target_os = "macos"))]
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
#[cfg(any(not(target_os = "macos"), test))]
pub fn create_renderer_for(window: *mut SDL_Window, gpu: bool, pyrowave: bool, vulkan_video: bool) -> *mut SDL_Renderer {
    *GPU_FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    #[cfg(unix)]
    if gpu && (pyrowave || vulkan_video) {
        use crate::stream::pyrowave_sdk::zero_copy;
        match zero_copy::RendererRequest::new(pyrowave, vulkan_video) {
            Ok(mut request) => {
                let video = request.video();
                // The request outlives the call, which copies what it needs of it.
                // PyroWave's compute queue only with the setting on (`queue::async_compute`).
                let compute_queue = request.pyrowave() && crate::stream::pyrowave_sdk::queue::async_compute();
                let renderer = make_renderer(window, gpu, request.options(), !video.usable().is_empty(), compute_queue);
                // SAFETY: A live renderer made with the request.
                if !renderer.is_null() && unsafe { request.mark(renderer) } {
                    return renderer;
                }
                let why = if renderer.is_null() { error() } else { format!("it draws with {:?}", renderer_name(renderer)) };
                // SAFETY: Made above, unused.
                if !renderer.is_null() { unsafe { crate::sdl::destroy_renderer(renderer) }; }
                let extensions: Vec<_> = video.names().iter().map(|name| name.to_string_lossy()).collect();
                zero_copy::note(&format!("the GPU renderer cannot be made with the Vulkan features {:?} and the video extensions {extensions:?} ({why}), so PyroWave GPU decoding will read its planes back and Vulkan video is not used", request.features().names()));
            }
            Err(why) => zero_copy::note(&format!("the GPU renderer is made without PyroWave's Vulkan features or Vulkan video, so PyroWave GPU decoding will read its planes back and Vulkan video is not used: {why}")),
        }
    }
    let _ = (pyrowave, vulkan_video);
    make_renderer(window, gpu, std::ptr::null_mut(), false, false)
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
/// its extra video decode queues (`vulkan_video::DECODE_QUEUES_PROPERTY`), `compute_queue` for
/// one more queue for PyroWave's decodes (`zero_copy::COMPUTE_QUEUE_PROPERTY`).
#[cfg(any(not(target_os = "macos"), test))]
fn make_renderer(window: *mut SDL_Window, gpu: bool, vulkan: *mut SDL_GPUVulkanOptions, decode_queues: bool, compute_queue: bool) -> *mut SDL_Renderer {
    // SAFETY: The window is live, `vulkan` null or valid, and all property strings are static.
    unsafe {
        // The renderer before this one may have left the window Nightjar's HDR description.
        #[cfg(target_os = "linux")]
        {
            forget_pass_through(window);
            // A Wayland desktop's swapchain is made again when the compositor's dmabuf feedback asks
            // for other modifiers (a scanout tranche), counted afresh for each renderer.
            use crate::stream::wl_scanout::{env_on, DMABUF_REALLOC_ENV};
            let window_props = SDL_GetWindowProperties(window);
            let realloc = gpu && video_driver() == "wayland" && !crate::handheld::gamescope() && env_on(DMABUF_REALLOC_ENV);
            SDL_SetBooleanProperty(window_props, RECREATE_ON_SUBOPTIMAL_PROPERTY.as_ptr(), realloc);
            SDL_SetNumberProperty(window_props, SUBOPTIMAL_RECREATES_PROPERTY.as_ptr(), 0);
        }
        let props = SDL_CreateProperties();
        if !vulkan.is_null() {
            SDL_SetPointerProperty(props, SDL_PROP_GPU_DEVICE_CREATE_VULKAN_OPTIONS_POINTER, vulkan.cast());
        }
        #[cfg(unix)]
        if decode_queues {
            SDL_SetBooleanProperty(props, crate::vulkan_video::DECODE_QUEUES_PROPERTY.as_ptr(), true);
        }
        #[cfg(unix)]
        if compute_queue {
            SDL_SetBooleanProperty(props, crate::stream::pyrowave_sdk::zero_copy::COMPUTE_QUEUE_PROPERTY.as_ptr(), true);
        }
        #[cfg(not(unix))]
        let _ = (decode_queues, compute_queue);
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
        // Linux's GPU renderer starts as scRGB or SDR; `hdr_output` sets it again for another output.
        if !cfg!(windows) && gpu {
            set_renderer_sdr_white(window, crate::handheld::renderer_sdr_white(crate::handheld::gamescope(), false));
        }
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

#[cfg(test)]
mod swapchain_answer_tests {
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
