//! Hardware checks invoked by the demo build on Cocoa's main thread.
use std::ptr;

use nightjar_core::sdp::{VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10};
use nightjar_core::session::VideoSetup;
use sdl3_sys::everything::*;

use super::{Decoded, VideoTexture};
use crate::media::{self, Codec, Frame, Hardware, Picture};
use crate::stream::{color_probe, packed444, pyrowave_gpu, upscale};

/// Submit through the stream presenter, release the caller's AVFrame, then sample
/// the rendered picture. Only the presenter keeps an imported pool slot alive.
fn draw(
    renderer: *mut SDL_Renderer,
    texture: &mut VideoTexture,
    frame: Frame,
    format: u32,
) -> Vec<[u8; 3]> {
    let info = frame.info();
    let decoded = Decoded {
        picture: Picture::Frame(frame),
        setup: VideoSetup {
            format,
            width: info.width,
            height: info.height,
            fps: 60,
            chroma_packing: 0,
            logical_width: info.width,
            logical_height: info.height,
            color_space: 1,
            color_range: 0,
        },
        number: 1,
        rtp: 0,
        receive_us: 0,
        enqueue_us: 0,
        input_us: 0,
        ready_us: 0,
        host_latency: 0,
        early: 0,
        first_block_us: 0,
        race: Default::default(),
        bands: Vec::new(),
        race_seq: 0,
        visible: None,
    };
    assert!(
        texture.upload(renderer, &decoded, None),
        "{:?}",
        texture.failure
    );
    drop(decoded);
    // The renderer, texture and readback surface are live on the main thread.
    unsafe {
        assert!(SDL_RenderTexture(
            renderer,
            texture.sdl_texture(renderer),
            ptr::null(),
            ptr::null()
        ));
        let surface = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!surface.is_null(), "{}", crate::sdl::error());
        let mut colors = Vec::new();
        for y in [1, 3, 5, 7] {
            for x in [1, 3, 5, 7] {
                let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
                assert!(
                    SDL_ReadSurfacePixel(
                        surface,
                        x * (*surface).w / 8,
                        y * (*surface).h / 8,
                        &mut r,
                        &mut g,
                        &mut b,
                        &mut a
                    ),
                    "{}",
                    crate::sdl::error()
                );
                colors.push([r, g, b]);
            }
        }
        SDL_DestroySurface(surface);
        colors
    }
}

fn check_surfaces(renderer: *mut SDL_Renderer) {
    let mut texture = VideoTexture::new(false, false);
    for (format, ten_bit, fixture) in [
        (
            VIDEO_FORMAT_H265,
            0,
            include_bytes!("../../tests/fixtures/hevc-420-8.bin").as_slice(),
        ),
        (
            VIDEO_FORMAT_H265_MAIN10,
            1,
            include_bytes!("../../tests/fixtures/hevc-420-10.bin").as_slice(),
        ),
    ] {
        let mut decoder =
            media::VideoDecoder::open(Codec::Hevc, Hardware::VideoToolbox, 1, true).unwrap();
        assert_eq!(decoder.send(fixture, 42), 0);
        let media::Received::Frame(frame) = decoder.receive() else {
            panic!("no VideoToolbox frame")
        };
        assert!(
            matches!(decoder.receive(), media::Received::None),
            "polling an empty decoder must return EAGAIN"
        );
        let info = frame.info();
        assert_eq!(
            (info.kind, info.ten_bit, info.pts),
            (media::FRAME_VIDEOTOOLBOX, ten_bit, 42)
        );
        let mapped = frame.map_to_memory().expect("readable NV12/P010 planes");
        let imported = draw(renderer, &mut texture, frame, format);
        assert!(
            texture.held.is_some(),
            "the native Metal renderer must import the pixel buffer"
        );
        let uploaded = draw(renderer, &mut texture, mapped, format);
        assert!(
            texture.held.is_none(),
            "a CPU upload must replace the external texture"
        );
        assert!(
            uploaded.iter().any(|rgb| *rgb != uploaded[0]),
            "the fixture must have visible detail"
        );
        for (pixel, reference) in imported.iter().zip(uploaded) {
            assert!(
                pixel
                    .iter()
                    .zip(reference)
                    .all(|(&a, b)| a.abs_diff(b) <= 2),
                "imported {pixel:?}, uploaded {reference:?}"
            );
        }
    }
    texture.destroy();
}

pub(crate) fn run() {
    upscale::check_scalers();
    // The test subprocess owns SDL and all its resources until this check exits.
    unsafe {
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(
            c"Nightjar Metal check".as_ptr(),
            256,
            256,
            SDL_WINDOW_HIDDEN,
        );
        assert!(!window.is_null(), "{}", crate::sdl::error());
        let native = crate::sdl::create_renderer_with(window, false, false);
        assert!(!native.is_null(), "{}", crate::sdl::error());
        assert_eq!(crate::sdl::renderer_name(native), "metal");
        check_surfaces(native);
        SDL_DestroyRenderer(native);

        let renderer = crate::sdl::create_renderer_with(window, true, false);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        for depth in [false, true] {
            pyrowave_gpu::probe(renderer, depth).expect("PyroWave fixture reconstruction");
            color_probe::planes_drawable(renderer, depth).expect("Metal plane shader");
        }
        let modes = packed444::probe(renderer, Hardware::VideoToolbox)
            .expect("packed 4:4:4 pixel comparison");
        assert_ne!(modes, 0, "no packed format passed");
        println!("Metal readback passed for NV12/P010 imports and uploads, both scalers, PyroWave and packed modes {modes:#x}");
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}
