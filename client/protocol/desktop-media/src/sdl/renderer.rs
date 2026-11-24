//! SDL3 renderer creation, HDR output selection, device-loss remake, and backend queries.

use std::ffi::{c_char, CStr};

use sdl3_sys::everything::*;

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

/// Makes Windows' renderer again on `window` after its device was lost (`App::recover_renderer`),
/// in the colorspace the old one had (`linear`: scRGB, `create_renderer_with`'s `gpu`). While the
/// driver restarts a try can fail, fall to another of SDL's renderers or to SDR: a try that is not
/// of `kind` in that colorspace goes, and the next follows 250 ms later, until `patience` has
/// passed; then whatever SDL makes is kept (null: none). The window keeps answering meanwhile.
#[cfg(windows)]
pub fn remake_renderer(window: *mut SDL_Window, linear: bool, kind: &str, patience: std::time::Duration) -> *mut SDL_Renderer {
    let begun = std::time::Instant::now();
    let mut tries = 0u32;
    loop {
        let made = create_renderer_with(window, linear, false);
        tries += 1;
        let name = (!made.is_null()).then(|| renderer_name(made));
        let made_linear = !made.is_null() && renderer_linear(made);
        if remake_keeps(name.as_deref(), made_linear == linear, kind, begun.elapsed() >= patience) {
            if tries > 1 {
                eprintln!("nightjar: the renderer was made again at try {tries}, after {} ms", begun.elapsed().as_millis());
            }
            return made;
        }
        if tries == 1 {
            let what = name.map_or_else(|| format!("failed ({})", super::error()), |name| format!("made {name:?}, scRGB {made_linear}"));
            eprintln!("nightjar: making the {kind:?} renderer again {what}; trying again while the driver restarts");
        }
        if !made.is_null() {
            // SAFETY: the renderer just made, nothing on it.
            unsafe { destroy_renderer(made) };
        }
        // SAFETY: on the main thread. A window that stops answering for 5 s is "not responding".
        unsafe {
            SDL_PumpEvents();
            SDL_Delay(250);
        }
    }
}

/// Whether `remake_renderer` keeps what a try made (`made`: the renderer's name, None for none):
/// the kind the lost one was (`kind`) in its colorspace (`space_kept`), or once its patience is
/// out (`out_of_time`) whatever it is.
#[cfg(any(windows, test, feature = "test-support"))]
pub fn remake_keeps(made: Option<&str>, space_kept: bool, kind: &str, out_of_time: bool) -> bool {
    (made == Some(kind) && space_kept) || out_of_time
}

/// Whether `renderer` draws in scRGB (`SDL_COLORSPACE_SRGB_LINEAR`), as one made for the GPU
/// renderer's start is.
#[cfg(windows)]
pub fn renderer_linear(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: a live renderer; the call only reads its properties.
    let space = unsafe { SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0) };
    space == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0)
}

/// Destroys `renderer` and forgets the cached swapchain answers, so a renderer made later at the
/// same address asks its swapchain again.
///
/// # Safety
/// `renderer` is live and not used after this.
pub unsafe fn destroy_renderer(renderer: *mut SDL_Renderer) {
    super::hdr::forget_swapchain_answers();
    // SAFETY: the caller's.
    unsafe { SDL_DestroyRenderer(renderer) }
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
    let (white, headroom) = hdr.unwrap_or((1.0, 1.0));
    // SAFETY: caller guarantees `window` is live.
    unsafe {
        let props = SDL_GetWindowProperties(window);
        SDL_SetFloatProperty(props, SDL_PROP_WINDOW_SDR_WHITE_LEVEL_FLOAT, white);
        SDL_SetFloatProperty(props, SDL_PROP_WINDOW_HDR_HEADROOM_FLOAT, headroom);
        SDL_SetBooleanProperty(props, SDL_PROP_WINDOW_HDR_ENABLED_BOOLEAN, hdr.is_some());
    }
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
    let name = crate::handheld::RENDERER_SDR_WHITE_PROPERTY.as_ptr();
    // SAFETY: caller guarantees `window` is live.
    unsafe {
        let props = SDL_GetWindowProperties(window);
        match white {
            Some(white) => SDL_SetFloatProperty(props, name, white),
            None => SDL_ClearProperty(props, name),
        };
    }
}

/// Linux's GPU renderer `renderer`, made from `props` with linear output: the HDR output for its
/// window (`handheld::plan`) or null for SDR. scRGB keeps it, on a desktop with [`crate::sdl::hdr_available`]
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
    // SAFETY: caller guarantees `window`, `renderer`, and `props` are live.
    unsafe {
        let gamescope = handheld::gamescope();
        let window_hdr = handheld::window_hdr_reported(
            SDL_GetBooleanProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_HDR_ENABLED_BOOLEAN, false),
            WINDOW_HDR_SET.load(std::sync::atomic::Ordering::Relaxed),
        );
        let offered = super::hdr::offered(renderer);
        let name = renderer_name(renderer);
        let display_hdr = super::hdr::any_display_hdr();
        let hdr_asked = super::hdr::hdr_asked();
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
                    super::gpu_present::set_pq_present(props, true);
                    renderer = super::gpu_present::pq_renderer(window, props);
                } else {
                    renderer = SDL_CreateRendererWithProperties(props);
                }
                if output == Output::Pq {
                    super::gpu_present::set_pq_present(props, false);
                    if renderer.is_null() { pq_error = Some(super::error()); }
                }
            }
            decision = handheld::settle(output, !renderer.is_null(), !renderer.is_null() && super::hdr::swapchain_takes_hdr(renderer));
            if output == Output::Pq && decision.is_err() {
                PQ_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // A desktop's PQ that could not be made: scRGB where it is offered, not SDR.
            if let (Err(_), Some(next)) = (decision, handheld::fallback(output, offered)) {
                if !renderer.is_null() { crate::sdl::destroy_renderer(renderer); }
                #[cfg(target_os = "linux")]
                super::gpu_present::forget_pass_through(window);
                set_renderer_sdr_white(window, handheld::renderer_sdr_white(gamescope, next == Output::Pq));
                renderer = SDL_CreateRendererWithProperties(props);
                pq_failed = true;
                decision = handheld::settle(next, !renderer.is_null(), !renderer.is_null() && super::hdr::swapchain_takes_hdr(renderer));
            }
        }
        if decision.is_err() {
            if !renderer.is_null() { crate::sdl::destroy_renderer(renderer); }
            #[cfg(target_os = "linux")]
            super::gpu_present::forget_pass_through(window);
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
            eprintln!("nightjar: Metal renderer unavailable ({}); trying SDL's default SDR renderer", super::error());
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
    // Linux (Unix, excluding macOS via fn cfg): PyroWave GPU SDK zero-copy shared library probe.
    #[cfg(unix)]
    let pyrowave = gpu && crate::stream::pyrowave_sdk::dll_path().is_some_and(|path| path.exists());
    // Windows (non-Unix): PyroWave SDK zero-copy path is Unix-only.
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
    // Unix (Linux, and macOS under tests): Vulkan zero-copy request for PyroWave and Vulkan video.
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
                let why = if renderer.is_null() { super::error() } else { format!("it draws with {:?}", renderer_name(renderer)) };
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
        #[cfg(target_os = "linux")]
        super::gpu_present::prepare_window_scanout(window, gpu);
        let props = SDL_CreateProperties();
        if !vulkan.is_null() {
            SDL_SetPointerProperty(props, SDL_PROP_GPU_DEVICE_CREATE_VULKAN_OPTIONS_POINTER, vulkan.cast());
        }
        // Unix (Linux, and macOS under tests): patched SDL property for Vulkan video decode queues.
        #[cfg(unix)]
        if decode_queues {
            SDL_SetBooleanProperty(props, crate::vulkan_video::DECODE_QUEUES_PROPERTY.as_ptr(), true);
        }
        // Unix (Linux, and macOS under tests): patched SDL property for PyroWave async compute queue.
        #[cfg(unix)]
        if compute_queue {
            SDL_SetBooleanProperty(props, crate::stream::pyrowave_sdk::zero_copy::COMPUTE_QUEUE_PROPERTY.as_ptr(), true);
        }
        // Windows (non-Unix): extra Vulkan decode/compute queue properties are Unix-only.
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
            let why = super::error();
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
