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

pub fn choose_json_file(window: *mut SDL_Window, save: bool) -> FileChoice {
    struct Request {
        sender: std::sync::mpsc::Sender<Result<Option<std::path::PathBuf>, String>>,
        filter: SDL_DialogFileFilter,
    }
    unsafe extern "C" fn chosen(userdata: *mut std::ffi::c_void, files: *const *const c_char, _: i32) {
        // SDL calls once; the request keeps the filter alive until this callback.
        let request = unsafe { Box::from_raw(userdata.cast::<Request>()) };
        let result = if files.is_null() { Err(error()) }
            else if unsafe { (*files).is_null() } { Ok(None) }
            else { Ok(Some(std::path::PathBuf::from(unsafe { CStr::from_ptr(*files) }.to_string_lossy().into_owned()))) };
        let _ = request.sender.send(result);
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

pub fn set_hint(name: *const c_char, value: &CStr) {
    // SAFETY: both are NUL-terminated strings.
    unsafe { SDL_SetHint(name, value.as_ptr()) };
}

/// The system's preferred languages, most preferred first, as lower-case codes ("de", "en").
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
                out.push(CStr::from_ptr((*locale).language).to_string_lossy().to_ascii_lowercase());
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

pub fn hdr_available(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: The renderer and its associated window/device are live.
    unsafe {
        let props = SDL_GetRendererProperties(renderer);
        if !SDL_GetBooleanProperty(props, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, false) {
            return false;
        }
        let device: *mut SDL_GPUDevice = SDL_GetPointerProperty(
            props,
            SDL_PROP_RENDERER_GPU_DEVICE_POINTER,
            std::ptr::null_mut(),
        )
        .cast();
        device.is_null()
            || (SDL_WindowSupportsGPUSwapchainComposition(
                device,
                SDL_GetRenderWindow(renderer),
                SDL_GPU_SWAPCHAINCOMPOSITION_HDR_EXTENDED_LINEAR,
            ) && SDL_GetGPUSwapchainTextureFormat(device, SDL_GetRenderWindow(renderer))
                == SDL_GPU_TEXTUREFORMAT_R16G16B16A16_FLOAT)
    }
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

pub fn create_renderer(window: *mut SDL_Window, gpu: bool) -> *mut SDL_Renderer {
    // SAFETY: The window is live, and all property strings are static.
    unsafe {
        let props = SDL_CreateProperties();
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
        if !cfg!(windows) && gpu && !renderer.is_null() && !hdr_available(renderer) {
            SDL_DestroyRenderer(renderer);
            renderer = std::ptr::null_mut();
        }
        if renderer.is_null() {
            SDL_SetNumberProperty(
                props,
                SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER,
                i64::from(SDL_COLORSPACE_SRGB.0),
            );
            renderer = SDL_CreateRendererWithProperties(props);
        }
        if renderer.is_null() && !cfg!(windows) && gpu {
            SDL_ClearProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING);
            renderer = SDL_CreateRendererWithProperties(props);
        }
        SDL_DestroyProperties(props);
        renderer
    }
}

/// The window's icon from straight RGBA pixels.
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
