//! Patched SDL GPU renderer present-mode, Wayland scanout, window-target, and PQ/scanout shader properties.

use std::ffi::CStr;

use sdl3_sys::everything::*;

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
        let device: *mut SDL_GPUDevice = super::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
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
    let pq_output = matches!(super::output_decision(), Some(Ok(crate::handheld::Output::Pq)));
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
pub(super) unsafe fn forget_pass_through(window: *mut SDL_Window) {
    // SAFETY: caller guarantees `window` is live.
    unsafe {
        crate::stream::wl_scanout::uninstall(window);
        SDL_ClearProperty(SDL_GetWindowProperties(window), PASS_THROUGH_PROPERTY.as_ptr());
        crate::stream::wl_scanout::set_label(None);
    }
}

/// Resets per-renderer Wayland scanout properties on `window` before a new renderer is created.
///
/// # Safety
/// `window` is live.
#[cfg(target_os = "linux")]
pub(super) unsafe fn prepare_window_scanout(window: *mut SDL_Window, gpu: bool) {
    // SAFETY: caller guarantees `window` is live.
    unsafe {
        // The renderer before this one may have left the window Nightjar's HDR description.
        forget_pass_through(window);
        // A Wayland desktop's swapchain is made again when the compositor's dmabuf feedback asks
        // for other modifiers (a scanout tranche), counted afresh for each renderer.
        use crate::stream::wl_scanout::{env_on, DMABUF_REALLOC_ENV};
        let window_props = SDL_GetWindowProperties(window);
        let realloc = gpu && video_driver() == "wayland" && !crate::handheld::gamescope() && env_on(DMABUF_REALLOC_ENV);
        SDL_SetBooleanProperty(window_props, RECREATE_ON_SUBOPTIMAL_PROPERTY.as_ptr(), realloc);
        SDL_SetNumberProperty(window_props, SUBOPTIMAL_RECREATES_PROPERTY.as_ptr(), 0);
    }
}

/// The PQ renderer for `window` from `props` (with the PQ present set). On a Wayland desktop it
/// asks for a pass-through HDR10 swapchain (Mesa's driver) and describes the surface itself
/// (`stream::wl_scanout::HdrDescription`), matched to the output; where that cannot be, the driver
/// describes it as before, and the end-of-stream line says why.
///
/// # Safety
/// `window` is live, `props` the renderer's create properties.
#[cfg(any(not(target_os = "macos"), test))]
pub(super) unsafe fn pq_renderer(window: *mut SDL_Window, props: SDL_PropertiesID) -> *mut SDL_Renderer {
    #[cfg(target_os = "linux")]
    // SAFETY: caller guarantees `window` is live and `props` is a live property set.
    unsafe {
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
                let why = if renderer.is_null() { super::error() } else { "the swapchain is not HDR10".to_owned() };
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
        // SAFETY: caller guarantees `props` is a live renderer property set.
        unsafe { SDL_CreateRendererWithProperties(props) }
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

/// The PQ present pass's shaders (`pq_present.hlsl`), handed to the patched SDL.
#[cfg(any(not(target_os = "macos"), test))]
pub(super) const PQ_VERTEX_SPIRV: &[u8] = include_bytes!("../pq_present_vertex.spv");
#[cfg(any(not(target_os = "macos"), test))]
pub(super) const PQ_FRAGMENT_SPIRV: &[u8] = include_bytes!("../pq_present_fragment.spv");

/// Sets (or with `on` false clears) the PQ present's create properties of the patched SDL.
///
/// # Safety
/// `props` is a live property set.
#[cfg(any(not(target_os = "macos"), test))]
pub(super) unsafe fn set_pq_present(props: SDL_PropertiesID, on: bool) {
    let shaders = [(c"SDL.renderer.create.gpu.pq_vertex_spirv", c"SDL.renderer.create.gpu.pq_vertex_spirv_size", PQ_VERTEX_SPIRV),
        (c"SDL.renderer.create.gpu.pq_fragment_spirv", c"SDL.renderer.create.gpu.pq_fragment_spirv_size", PQ_FRAGMENT_SPIRV)];
    // SAFETY: caller guarantees `props` is a live property set; shader SPIR-V slices are `'static`.
    unsafe {
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
}

/// The present of a pass-through HDR10 swapchain (`scanout_present.hlsl`): PQ as
/// `pq_present.hlsl`, or gamma 2.2 in the compositor's blending space, as the renderer properties
/// of [`set_present_encoding`] say on each present.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) const SCANOUT_FRAGMENT_SPIRV: &[u8] = include_bytes!("../scanout_present_fragment.spv");

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
    // SAFETY: caller guarantees `props` is a live property set; SPIR-V slices are `'static`.
    unsafe {
        SDL_SetPointerProperty(props, c"SDL.renderer.create.gpu.pq_fragment_spirv".as_ptr(), fragment.as_ptr().cast_mut().cast());
        SDL_SetNumberProperty(props, c"SDL.renderer.create.gpu.pq_fragment_spirv_size".as_ptr(), fragment.len() as i64);
        SDL_SetNumberProperty(props, c"SDL.renderer.create.gpu.pq_fragment_uniform_buffers".as_ptr(), i64::from(on));
    }
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
