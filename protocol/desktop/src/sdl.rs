//! Small helpers over sdl3-sys.

use std::ffi::{c_char, CStr};
use std::path::Path;

use sdl3_sys::everything::*;

/// SDL's last error message.
pub fn error() -> String {
    // SAFETY: SDL_GetError always returns a valid string.
    unsafe { CStr::from_ptr(SDL_GetError()) }
        .to_string_lossy()
        .into_owned()
}

pub fn set_hint(name: *const c_char, value: &CStr) {
    // SAFETY: both are NUL-terminated strings.
    unsafe { SDL_SetHint(name, value.as_ptr()) };
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
