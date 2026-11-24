//! Swapchain HDR capability queries, caching, mastering metadata, and display luminance.

use sdl3_sys::everything::*;

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
    pub(super) device: usize,
    pub(super) display: u32,
    pub(super) display_hdr: bool,
    pub(super) window_hdr: bool,
    pub(super) width: i32,
    pub(super) height: i32,
}

/// The most renderers whose swapchain answers are kept; the oldest goes first.
pub(super) const SWAPCHAIN_ANSWERS_KEPT: usize = 8;

/// The swapchain answers of GPU renderers, one per renderer, each kept for the state it was asked
/// in. `forget_all` drops them all (a display changed); an answer computed across a `forget_all`
/// is not stored, by the generation it was started in.
#[derive(Debug, Default)]
pub struct SwapchainAnswers {
    generation: u64,
    pub(super) entries: Vec<(usize, SwapchainState, bool)>,
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
        if super::renderer_is(renderer, "metal") {
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
pub(super) fn offered(renderer: *mut SDL_Renderer) -> crate::handheld::Offered {
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
    pub(super) s_type: u32,
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
                    eprintln!("nightjar: the HDR10 swapchain metadata could not be stored: {}", super::error());
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

#[cfg(any(not(target_os = "macos"), test))]
pub(super) fn hdr_asked() -> bool {
    HDR_ASKED.load(std::sync::atomic::Ordering::Relaxed)
}

/// How long a stream's start waits for the compositor's HDR report on a window.
pub const WINDOW_HDR_REPORT_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether the window's HDR report can still come (`handheld::hdr_report_can_come`): a start then
/// waits for it over the next frames, up to [`WINDOW_HDR_REPORT_WAIT`], while the event loop runs.
pub fn window_hdr_may_come(renderer: *mut SDL_Renderer) -> bool {
    crate::handheld::hdr_report_can_come(swapchain_takes_hdr(renderer), any_display_hdr())
}

/// What an HDR stream refused here can do about it.
pub fn hdr_refusal(renderer: *mut SDL_Renderer) -> crate::handheld::HdrRefusal {
    crate::handheld::hdr_refusal(swapchain_takes_hdr(renderer), any_display_hdr())
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

pub fn display_caps(renderer: *mut SDL_Renderer) -> rubylight_protocol::control::DisplayCaps {
    let caps = rubylight_protocol::control::DisplayCaps {
        hdr: hdr_available(renderer),
        ..Default::default()
    };
    #[cfg(windows)]
    {
        use windows::core::Interface;
        use windows::Win32::Graphics::Dxgi::{IDXGIOutput6, IDXGISwapChain};
        let chain = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
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

pub(super) fn luminance_of(caps: &rubylight_protocol::control::DisplayCaps) -> Option<nightjar_core::http::DisplayLuminance> {
    nightjar_core::http::DisplayLuminance::from_display(
        caps.max_centinits as f32 / 100.0,
        caps.min_decimillinits as f32 / 10_000.0,
        caps.max_average_centinits as f32 / 100.0,
        f32::NAN,
    )
}
