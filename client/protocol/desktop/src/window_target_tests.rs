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
