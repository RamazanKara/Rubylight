use super::*;

#[cfg(windows)]
unsafe fn pixels(renderer: *mut SDL_Renderer) -> Vec<f32> {
    // SAFETY: Readback owns the temporary surfaces and copies their initialized rows.
    unsafe {
        let surface = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!surface.is_null(), "{}", crate::sdl::error());
        let converted = SDL_ConvertSurface(surface, SDL_PIXELFORMAT_RGBA128_FLOAT);
        SDL_DestroySurface(surface);
        assert!(!converted.is_null(), "{}", crate::sdl::error());
        let mut result = Vec::new();
        for y in 0..(*converted).h as usize {
            result.extend_from_slice(std::slice::from_raw_parts(
                ((*converted).pixels as *const u8)
                    .add(y * (*converted).pitch as usize)
                    .cast::<f32>(),
                (*converted).w as usize * 4,
            ));
        }
        SDL_DestroySurface(converted);
        result
    }
}

#[cfg(windows)]
unsafe fn overlay(renderer: *mut SDL_Renderer) {
    // SAFETY: The live renderer belongs to this test's thread.
    unsafe {
        assert!(SDL_SetRenderDrawColor(renderer, 213, 55, 17, 255));
        assert!(SDL_RenderFillRect(
            renderer,
            &SDL_FRect {
                x: 3.0,
                y: 4.0,
                w: 12.0,
                h: 9.0
            }
        ));
    }
}

#[cfg(windows)]
unsafe fn direct_matches_reconstruction(window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
    use super::super::pipeline::Decoded;
    use super::super::present::{viewport, VideoTexture};
    // SAFETY: The test owns the window, renderer, textures and decoder on this thread.
    unsafe {
        let hardware = media::Hardware::D3d11(crate::sdl::renderer_pointer(
            renderer,
            SDL_PROP_RENDERER_D3D11_DEVICE_POINTER,
        ));
        let target = SDL_CreateTexture(
            renderer,
            SDL_PIXELFORMAT_RGBA64_FLOAT,
            SDL_TEXTUREACCESS_TARGET,
            128,
            64,
        );
        assert!(!target.is_null());
        assert!(SDL_SetTextureScaleMode(target, SDL_SCALEMODE_LINEAR));
        let linear_output = SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0)
            == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0);
        for (codec, format, data) in [
            (
                media::Codec::Hevc,
                nightjar_core::sdp::VIDEO_FORMAT_H265,
                &include_bytes!("../../../../../app/src/main/assets/packed444/hevc-8.bin")[..],
            ),
            (
                media::Codec::Av1,
                nightjar_core::sdp::VIDEO_FORMAT_AV1_MAIN8,
                &include_bytes!("../../../../../app/src/main/assets/packed444/av1-8.bin")[..],
            ),
            (
                media::Codec::Hevc,
                nightjar_core::sdp::VIDEO_FORMAT_H265_MAIN10,
                &include_bytes!("../../../../../app/src/main/assets/packed444/hevc-10.bin")[..],
            ),
            (
                media::Codec::Av1,
                nightjar_core::sdp::VIDEO_FORMAT_AV1_MAIN10,
                &include_bytes!("../../../../../app/src/main/assets/packed444/av1-10.bin")[..],
            ),
        ] {
            let mut decoder = media::VideoDecoder::open(codec, hardware, 1, true).unwrap();
            assert_eq!(decoder.send(data, 0), 0);
            let media::Received::Frame(frame) = decoder.receive() else {
                panic!("no hardware fixture frame")
            };
            let mut decoded = Decoded {
                picture: crate::media::Picture::Frame(frame),
                setup: nightjar_core::session::VideoSetup {
                    format,
                    width: 128,
                    height: 128,
                    fps: 60,
                    chroma_packing: 1,
                    logical_width: 128,
                    logical_height: 64,
                    color_space: 1,
                    color_range: 1,
                },
                rtp: 0,
                number: 0,
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
            let ten_bit = format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
            let mut parameters = [128, 64, 128, 64, u32::from(ten_bit), 1, 1, 0, 1.0f32.to_bits(), 0, 0, 0];
            let mut reference = Reconstruction::new(renderer, target, &parameters).unwrap();
            let mut video = VideoTexture::default();
            video.set_hdr_mode(false, None);
            for (range, packing, hdr) in (0..=1).flat_map(|range| (1..=2).flat_map(move |packing| [false, true].into_iter().filter(move |&hdr| !hdr || (ten_bit && linear_output)).map(move |hdr| (range, packing, hdr)))) {
                decoded.setup.chroma_packing = packing;
                parameters[7] = u32::from(hdr);
                parameters[9] = u32::from(packing);
                let mut direct_hdr = hdr.then(|| Reconstruction::new(renderer, ptr::null_mut(), &parameters).unwrap());
                if let Some(direct) = &mut direct_hdr { assert!(direct.upload(decoded.picture.frame().unwrap())); }
                for matrix in 0..=2 {
                    decoded.setup.color_range = range;
                    decoded.setup.color_space = matrix;
                    parameters[5] = range as u32;
                    parameters[6] = matrix as u32;
                    assert!(reference.draw(renderer, target, decoded.picture.frame().unwrap(), &parameters));
                    if !hdr {
                        assert!(video.upload(renderer, &decoded, None));
                        assert!(video.texture.is_null(), "direct presentation allocated an intermediate RGB texture");
                    }
                    for (width, height) in [(128, 64), (240, 160), (99, 59)] {
                        assert!(SDL_SetWindowSize(window, width, height));
                        SDL_PumpEvents();
                        let (mut w, mut h) = (0, 0);
                        assert!(SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h));
                        assert_eq!((w, h), (width, height));
                        for (mode, top) in [(crate::config::VideoScaling::Fit, false), (crate::config::VideoScaling::Fill, false),
                            (crate::config::VideoScaling::Stretch, false), (crate::config::VideoScaling::Native, false), (crate::config::VideoScaling::Fit, true)] {
                            let destination = viewport((128, 64), (w, h), mode, top);
                            for scale in [1.0, 0.6] {
                                assert!(SDL_SetRenderColorScale(renderer, scale));
                                assert!(SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255));
                                assert!(SDL_RenderClear(renderer));
                                assert!(SDL_RenderTexture(
                                    renderer,
                                    target,
                                    ptr::null(),
                                    &destination
                                ));
                                overlay(renderer);
                                let expected = pixels(renderer);
                                // The reference stores FP16 and uses fixed-point hardware
                                // filtering. Bound HDR interpolation error by its scene range.
                                let hdr_range = if hdr {
                                    expected.iter().fold(1.0f32, |peak, value| peak.max(value.abs()))
                                } else { 1.0 };
                                assert!(SDL_RenderPresent(renderer));
                                // Redraws after Present and resize must retain the uploaded picture.
                                for _ in 0..2 {
                                    assert!(SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255));
                                    assert!(SDL_RenderClear(renderer));
                                    assert!(
                                        if let Some(direct) = &mut direct_hdr { direct.present(renderer, &parameters, &destination) } else { video.draw(renderer, &destination) },
                                        "{}",
                                        crate::sdl::error()
                                    );
                                    overlay(renderer);
                                    let actual = pixels(renderer);
                                    assert_eq!(actual.len(), expected.len());
                                    let mut maximum = 0.0f32;
                                    let mut sum = 0.0;
                                    let mut worst = (0.0, 0.0);
                                    for (&a, &b) in expected.iter().zip(&actual) {
                                        assert!(b.is_finite());
                                        let error = (a - b).abs() / hdr_range;
                                        if error > maximum { worst = (a, b); }
                                        maximum = maximum.max(error);
                                        sum += error;
                                    }
                                    let mean = sum / actual.len() as f32;
                                    let tolerance = if hdr && destination.w == 128.0 && destination.h == 64.0 { 0.001 } else { 0.01 };
                                    assert!(maximum < tolerance && mean < 0.0008,
                                        "{codec:?} ten_bit={ten_bit} packing={packing} hdr={hdr} range={range} matrix={matrix} {w}x{h} scale={scale} {mode:?} top={top}: max={maximum}, mean={mean}, worst={worst:?}");
                                    assert!(SDL_RenderPresent(renderer));
                                }
                            }
                        }
                    }
                }
            }
            video.destroy();
            if ten_bit && linear_output {
                // Emulate display limits to exercise path selection without changing Windows HDR.
                let properties = SDL_GetRendererProperties(renderer);
                let enabled = SDL_GetBooleanProperty(properties, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, false);
                let white = SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0);
                let headroom = SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0);
                assert!(SDL_SetBooleanProperty(properties, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, true));
                assert!(SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 2.0));
                for direct in [true, false] {
                    let mut video = VideoTexture::new(direct, true);
                    for (peak, available) in [(320, 16.0), (4000, 16.0), (320, 16.0), (320, 1.0), (320, 16.0)] {
                        assert!(SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, available));
                        video.set_hdr_mode(true, Some(nightjar_core::control::HdrMetadata {
                            max_content_light_level: peak,
                            ..Default::default()
                        }));
                        assert!(video.upload(renderer, &decoded, None));
                        assert_eq!(video.texture.is_null(), direct && peak == 320 && available == 16.0);
                        assert!(video.draw(renderer, &viewport((128, 64), (99, 59), crate::config::VideoScaling::Fit, false)));
                        assert!(SDL_RenderPresent(renderer));
                    }
                    video.destroy();
                }
                SDL_SetBooleanProperty(properties, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, enabled);
                SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, white);
                SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, headroom);
            }
            drop(reference);
        }
        SDL_DestroyTexture(target);
    }
}

/// The SPIR-V instructions' opcodes of `code`.
fn opcodes(code: &[u8]) -> Vec<u32> {
    let words: Vec<u32> = code.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
    assert_eq!(words[0], 0x0723_0203, "SPIR-V magic");
    let mut at = 5;
    let mut opcodes = Vec::new();
    while at < words.len() {
        opcodes.push(words[at] & 0xffff);
        at += (words[at] >> 16).max(1) as usize;
    }
    opcodes
}

/// #80: packed pictures mapped into Vulkan are rebuilt from their plane textures, not from a
/// buffer copied through memory.
#[test]
fn the_planes_variant_samples_two_textures_and_the_memory_one_reads_a_buffer() {
    const TYPE_IMAGE: u32 = 25;
    const TYPE_SAMPLED_IMAGE: u32 = 27;
    const TYPE_RUNTIME_ARRAY: u32 = 29;
    let planes = opcodes(include_bytes!("../../packed444-planes.spv"));
    let memory = opcodes(include_bytes!("../../packed444.spv"));
    assert!(planes.contains(&TYPE_SAMPLED_IMAGE) && planes.iter().filter(|&&op| op == TYPE_IMAGE).count() >= 1);
    assert!(!planes.contains(&TYPE_RUNTIME_ARRAY), "the planes variant reads no sample buffer");
    assert!(memory.contains(&TYPE_RUNTIME_ARRAY) && !memory.contains(&TYPE_SAMPLED_IMAGE));
}

#[test]
#[ignore = "requires an HEVC/AV1 hardware decoder and a D3D11 or Vulkan display"]
fn packed_decode_and_gpu_reconstruction() {
    if crate::sdl::skip_without_display() { return; }
    // SAFETY: This test owns SDL and all its resources on one thread.
    unsafe {
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        #[cfg(windows)]
        {
            SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
            SDL_SetHint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1".as_ptr());
        }
        let window =
            SDL_CreateWindow(c"Packed chroma test".as_ptr(), 128, 64, SDL_WINDOW_HIDDEN);
        let renderer = crate::sdl::create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        #[cfg(windows)]
        let hardware = media::Hardware::D3d11(crate::sdl::renderer_pointer(
            renderer,
            SDL_PROP_RENDERER_D3D11_DEVICE_POINTER,
        ));
        #[cfg(not(windows))]
        let hardware = media::Hardware::Vaapi;
        let modes = probe(renderer, hardware);
        #[cfg(windows)]
        direct_matches_reconstruction(window, renderer);
        SDL_DestroyRenderer(renderer);
        #[cfg(windows)]
        {
            let renderer = crate::sdl::create_renderer(window, false);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            direct_matches_reconstruction(window, renderer);
            SDL_DestroyRenderer(renderer);
        }
        SDL_DestroyWindow(window);
        SDL_Quit();
        assert_eq!(
            modes.unwrap(),
            15,
            "8/10-bit HEVC/AV1 reconstruction must match CPU color conversion"
        );
    }
}
