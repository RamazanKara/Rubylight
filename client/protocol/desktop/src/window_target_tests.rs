//! The patched SDL's window target (`vendor/sdl3-src/patches/window-target.patch`): a frame draws
//! straight into its swapchain image unless `read_back_frames` keeps it in the back buffer. These
//! need a Vulkan device and SDL's offscreen video driver, so they are ignored:
//! `cargo test -p nightjar-desktop -- --ignored --test-threads=1 window_target`.

use std::ptr;

use super::*;

/// The red channel at `(x, y)` of a read-back, and its green one.
unsafe fn red_green(surface: *mut SDL_Surface, x: i32, y: i32) -> (u8, u8) {
    let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
    // SAFETY: a live surface from SDL_RenderReadPixels.
    assert!(unsafe { SDL_ReadSurfacePixel(surface, x, y, &mut r, &mut g, &mut b, &mut a) }, "{}", error());
    (r, g)
}

#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn a_frame_draws_into_the_swapchain_unless_a_read_back_keeps_the_back_buffer() {
    let _sdl = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", error());
        // Shown: SDL gives a hidden window no swapchain image (offscreen, nothing appears).
        let window = SDL_CreateWindow(c"Window target".as_ptr(), 64, 32, SDL_WindowFlags(0));
        let renderer = create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", error());
        assert_eq!(renderer_name(renderer), "gpu");
        let square = SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 8.0 };
        let draw = || {
            assert!(SDL_SetRenderDrawColor(renderer, 255, 0, 0, 255) && SDL_RenderClear(renderer));
            assert!(SDL_SetRenderDrawColor(renderer, 0, 255, 0, 255) && SDL_RenderFillRect(renderer, &square));
        };

        // A frame that drew is in its swapchain image: a read-back would present it early, so it fails.
        draw();
        let surface = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(surface.is_null(), "the window's target was read from the back buffer");
        assert!(error().contains("SDL.renderer.gpu.read_back"), "{}", error());
        assert!(SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(window_target_frames(renderer), 1, "the frame went straight into the swapchain");

        // A screenshot's frame stays in the back buffer, read back whole, and is blitted.
        read_back_frames(renderer, true);
        draw();
        let surface = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!surface.is_null(), "{}", error());
        let (square, clear) = (red_green(surface, 2, 2), red_green(surface, 40, 20));
        SDL_DestroySurface(surface);
        assert!(square.0 < 20 && square.1 > 235, "the square: {square:?}");
        assert!(clear.0 > 235 && clear.1 < 20, "the clear: {clear:?}");
        assert!(SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(window_target_frames(renderer), 1, "the read-back frame went through the back buffer");

        // A frame that only clears takes its image at the present, for the clear.
        read_back_frames(renderer, false);
        assert!(SDL_RenderClear(renderer) && SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(window_target_frames(renderer), 2);

        // A render target read back before the frame draws on the window's target (live glass's
        // timed chain, after the frame's clear) takes no image, so it presents nothing early.
        let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_TARGET, 16, 16);
        assert!(!target.is_null(), "{}", error());
        assert!(SDL_RenderClear(renderer) && SDL_SetRenderTarget(renderer, target) && SDL_RenderClear(renderer));
        let surface = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!surface.is_null(), "{}", error());
        SDL_DestroySurface(surface);
        assert!(SDL_SetRenderTarget(renderer, ptr::null_mut()) && SDL_RenderTexture(renderer, target, ptr::null(), ptr::null()));
        assert!(SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(window_target_frames(renderer), 3);

        SDL_DestroyTexture(target);
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

/// SDL on the offscreen video driver, and a shown 64x32 window with the GPU renderer.
///
/// # Safety
/// The caller holds `TEST_LOCK` and ends with [`offscreen_end`].
unsafe fn offscreen_gpu() -> (*mut SDL_Window, *mut SDL_Renderer) {
    // SAFETY: as the function says.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", error());
        let window = SDL_CreateWindow(c"Window target".as_ptr(), 64, 32, SDL_WindowFlags(0));
        assert!(!window.is_null(), "{}", error());
        let renderer = create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", error());
        assert_eq!(renderer_name(renderer), "gpu");
        (window, renderer)
    }
}

/// # Safety
/// What [`offscreen_gpu`] made, with nothing else left on it.
unsafe fn offscreen_end(window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
    // SAFETY: as the function says.
    unsafe {
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

/// A frame that clears and draws a square on the window's target, presented.
///
/// # Safety
/// A live renderer.
unsafe fn frame(renderer: *mut SDL_Renderer) {
    let square = SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 8.0 };
    // SAFETY: as the function says.
    unsafe {
        assert!(SDL_SetRenderDrawColor(renderer, 255, 0, 0, 255) && SDL_RenderClear(renderer));
        assert!(SDL_SetRenderDrawColor(renderer, 0, 255, 0, 255) && SDL_RenderFillRect(renderer, &square));
        assert!(SDL_RenderPresent(renderer), "{}", error());
    }
}

#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn a_resize_between_frames_goes_through_the_back_buffer_once() {
    let _sdl = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        let (window, renderer) = offscreen_gpu();
        frame(renderer);
        assert_eq!(window_target_frames(renderer), 1);
        assert!(SDL_SetWindowSize(window, 96, 48), "{}", error());
        SDL_PumpEvents();
        // The new swapchain image is larger than the back buffer: drawn there and copied, which
        // makes the back buffer the new size.
        frame(renderer);
        assert_eq!(window_target_frames(renderer), 1, "the frame after a resize went through the back buffer");
        frame(renderer);
        assert_eq!(window_target_frames(renderer), 2, "and the next straight into the swapchain again");
        offscreen_end(window, renderer);
    }
}

#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn a_hidden_window_takes_no_image_and_says_no_error() {
    let _sdl = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        let (window, renderer) = offscreen_gpu();
        frame(renderer);
        assert_eq!(window_target_frames(renderer), 1);
        assert!(SDL_HideWindow(window), "{}", error());
        SDL_ClearError();
        frame(renderer);
        assert_eq!(error(), "", "SDL has no image for a hidden window, which is no failure");
        assert_eq!(window_target_frames(renderer), 1, "nothing went into a swapchain");
        assert!(SDL_ShowWindow(window), "{}", error());
        frame(renderer);
        assert_eq!(window_target_frames(renderer), 2, "shown again: straight into the swapchain");
        offscreen_end(window, renderer);
    }
}

#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn v_sync_set_while_the_frame_holds_its_image_is_refused() {
    let _sdl = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        let (window, renderer) = offscreen_gpu();
        assert!(SDL_SetRenderVSync(renderer, 1), "{}", error());
        let before = present_mode(renderer);
        assert_eq!(before, Some(0), "FIFO");
        assert!(SDL_SetRenderDrawColor(renderer, 255, 0, 0, 255) && SDL_RenderClear(renderer));
        let square = SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 8.0 };
        assert!(SDL_RenderFillRect(renderer, &square) && SDL_FlushRenderer(renderer), "{}", error());
        // SDL_SetRenderVSync turns the renderer's refusal into no V-Sync; the error stays.
        SDL_ClearError();
        SDL_SetRenderVSync(renderer, 0);
        assert!(error().contains("set V-Sync between frames"), "{}", error());
        assert_eq!(present_mode(renderer), before, "the swapchain was not made again under the frame");
        assert!(SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(window_target_frames(renderer), 1, "the frame reached its image");
        assert!(SDL_SetRenderVSync(renderer, 1), "{}", error());
        assert_eq!(present_mode(renderer), Some(0), "between frames it is taken");
        offscreen_end(window, renderer);
    }
}

#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn the_pq_renderer_never_draws_into_the_swapchain() {
    let _sdl = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Plain SDL calls on objects made here, on one thread; the properties are freed here.
    unsafe {
        let (window, renderer) = offscreen_gpu();
        let hdr10 = hdr10_offered(renderer);
        SDL_DestroyRenderer(renderer);
        if !hdr10 {
            eprintln!("this Vulkan device offers the window no HDR10 swapchain: the PQ renderer is not tested");
            SDL_DestroyWindow(window);
            SDL_Quit();
            return;
        }
        // As `hdr_output` makes it for an HDR stream.
        let props = SDL_CreateProperties();
        SDL_SetPointerProperty(props, SDL_PROP_RENDERER_CREATE_WINDOW_POINTER, window.cast());
        SDL_SetStringProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING, c"gpu".as_ptr());
        SDL_SetBooleanProperty(props, SDL_PROP_RENDERER_CREATE_GPU_SHADERS_SPIRV_BOOLEAN, true);
        SDL_SetNumberProperty(props, SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER, i64::from(SDL_COLORSPACE_SRGB_LINEAR.0));
        set_pq_present(props, true);
        let renderer = SDL_CreateRendererWithProperties(props);
        set_pq_present(props, false);
        SDL_DestroyProperties(props);
        assert!(!renderer.is_null(), "{}", error());
        for _ in 0..3 {
            frame(renderer);
        }
        assert!(SDL_RenderClear(renderer) && SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(window_target_frames(renderer), 0, "PQ's present pass reads the back buffer");
        offscreen_end(window, renderer);
    }
}

/// A video-only frame (`SDL.renderer.gpu.video_only`, as `stream::render`'s `clear_window` asks
/// for one) on the PQ renderer: SDL settles it on the flush of the frame's clear, takes the HDR10
/// swapchain image without the PQ pass, and drops and counts its own draws; the next frame, not
/// asked, goes through the PQ pass again.
#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn a_video_only_frame_on_the_pq_renderer_skips_the_pq_pass_and_drops_sdls_draws() {
    let _sdl = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Plain SDL calls on objects made here, on one thread; the properties are freed here.
    unsafe {
        let (window, renderer) = offscreen_gpu();
        let hdr10 = hdr10_offered(renderer);
        SDL_DestroyRenderer(renderer);
        if !hdr10 {
            eprintln!("this Vulkan device offers the window no HDR10 swapchain: video-only frames are not tested");
            SDL_DestroyWindow(window);
            SDL_Quit();
            return;
        }
        let props = SDL_CreateProperties();
        SDL_SetPointerProperty(props, SDL_PROP_RENDERER_CREATE_WINDOW_POINTER, window.cast());
        SDL_SetStringProperty(props, SDL_PROP_RENDERER_CREATE_NAME_STRING, c"gpu".as_ptr());
        SDL_SetBooleanProperty(props, SDL_PROP_RENDERER_CREATE_GPU_SHADERS_SPIRV_BOOLEAN, true);
        SDL_SetNumberProperty(props, SDL_PROP_RENDERER_CREATE_OUTPUT_COLORSPACE_NUMBER, i64::from(SDL_COLORSPACE_SRGB_LINEAR.0));
        set_pq_present(props, true);
        let renderer = SDL_CreateRendererWithProperties(props);
        set_pq_present(props, false);
        SDL_DestroyProperties(props);
        assert!(!renderer.is_null(), "{}", error());
        let renderer_props = SDL_GetRendererProperties(renderer);
        let flag = |name: &std::ffi::CStr| SDL_GetBooleanProperty(renderer_props, name.as_ptr(), false);
        let dropped = || SDL_GetNumberProperty(renderer_props, c"SDL.renderer.gpu.video_only_dropped_draws".as_ptr(), 0);
        let square = SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 8.0 };
        for _ in 0..3 {
            frame(renderer);
        }
        let before = window_target_frames(renderer);

        SDL_SetBooleanProperty(renderer_props, c"SDL.renderer.gpu.video_only_frame".as_ptr(), false);
        SDL_SetBooleanProperty(renderer_props, c"SDL.renderer.gpu.video_only".as_ptr(), true);
        assert!(SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255) && SDL_RenderClear(renderer) && SDL_FlushRenderer(renderer), "{}", error());
        SDL_SetBooleanProperty(renderer_props, c"SDL.renderer.gpu.video_only".as_ptr(), false);
        assert!(flag(c"SDL.renderer.gpu.video_only_frame"), "settled on the flush of the clear, before the reset");
        assert!(SDL_SetRenderDrawColor(renderer, 0, 255, 0, 255) && SDL_RenderFillRect(renderer, &square), "{}", error());
        assert!(SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(dropped(), 1, "SDL's own fill writes linear light: dropped");
        assert_eq!(window_target_frames(renderer), before + 1, "straight into the HDR10 swapchain image");

        // Not asked: the PQ pass again, nothing dropped.
        assert!(SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255) && SDL_RenderClear(renderer) && SDL_FlushRenderer(renderer), "{}", error());
        assert!(!flag(c"SDL.renderer.gpu.video_only_frame"));
        assert!(SDL_SetRenderDrawColor(renderer, 0, 255, 0, 255) && SDL_RenderFillRect(renderer, &square), "{}", error());
        assert!(SDL_RenderPresent(renderer), "{}", error());
        assert_eq!(dropped(), 1);
        assert_eq!(window_target_frames(renderer), before + 1, "through the back buffer and the PQ pass");
        offscreen_end(window, renderer);
    }
}
