use super::*;

#[test]
fn a_packed_picture_on_the_other_path_than_the_made_reconstruction_rebuilds_it() {
    assert!(!packed_path_changed(false, true, false), "nothing made yet");
    assert!(!packed_path_changed(true, true, true), "planes for a Vulkan picture");
    assert!(!packed_path_changed(true, false, false), "memory for a picture in memory");
    assert!(packed_path_changed(true, false, true), "the map failed: memory now");
    assert!(packed_path_changed(true, true, false), "a new decoder maps: planes now");
}

const VIDEO_ALONE: VideoOnlyFrame = VideoOnlyFrame { pq_output: true, planes_direct: true, nothing_else: true, read_back: false };

#[test]
fn a_frame_is_video_only_when_it_draws_only_the_planes_on_a_pq_output_unless_the_environment_says_0() {
    let mut video_only = VideoOnly::of(None);
    assert_eq!(video_only.wanted(VIDEO_ALONE, 0), (true, None));
    for frame in [
        VideoOnlyFrame { pq_output: false, ..VIDEO_ALONE },
        VideoOnlyFrame { planes_direct: false, ..VIDEO_ALONE },
        VideoOnlyFrame { nothing_else: false, ..VIDEO_ALONE },
        VideoOnlyFrame { read_back: true, ..VIDEO_ALONE },
    ] {
        assert_eq!(video_only.wanted(frame, 0), (false, None), "{frame:?}");
    }
    assert!(VideoOnly::of(Some(std::ffi::OsStr::new("1"))).wanted(VIDEO_ALONE, 0).0);
    let mut off = VideoOnly::of(Some(std::ffi::OsStr::new("0")));
    assert_eq!(off.wanted(VideoOnlyFrame { pq_output: false, ..VIDEO_ALONE }, 0), (false, None), "nothing to say off PQ");
    let (wanted, line) = off.wanted(VIDEO_ALONE, 0);
    assert!(!wanted && line.is_some_and(|line| line.contains(VIDEO_ONLY_ENVIRONMENT)), "said once");
    assert_eq!(off.wanted(VIDEO_ALONE, 0), (false, None));
}

#[test]
fn what_sdl_settled_is_said_once() {
    let mut video_only = VideoOnly::of(None);
    assert!(video_only.settled(true).is_some_and(|line| line.contains("straight")));
    assert_eq!(video_only.settled(true), None);
    assert_eq!(video_only.settled(false), None);
    assert!(VideoOnly::of(None).settled(false).is_some_and(|line| line.contains("did not")));
}

#[test]
fn dropped_draws_hold_video_only_frames_off_longer_each_time_then_for_the_stream() {
    let mut video_only = VideoOnly::of(None);
    assert_eq!(video_only.presented(0, 0), None);
    assert!(video_only.presented(3, 1_000).is_some_and(|line| line.contains("dropped 3") && line.contains("for 2 s")));
    assert!(!video_only.wanted(VIDEO_ALONE, 2_000_999).0, "held for 2 s");
    assert!(video_only.wanted(VIDEO_ALONE, 2_001_000).0);
    assert_eq!(video_only.presented(3, 2_001_000), None, "the count did not grow");
    assert!(video_only.presented(4, 3_000_000).is_some_and(|line| line.contains("dropped 1") && line.contains("for 4 s")));
    assert!(!video_only.wanted(VIDEO_ALONE, 6_999_999).0);
    assert!(video_only.wanted(VIDEO_ALONE, 7_000_000).0);
    // A renderer made again starts its count at 0: nothing was lost.
    assert_eq!(video_only.presented(0, 7_000_000), None);
    assert!(video_only.presented(1, 8_000_000).is_some_and(|line| line.contains("for 8 s")));
    assert!(video_only.presented(2, 20_000_000).is_some_and(|line| line.contains("rest of the stream")));
    assert!(!video_only.wanted(VIDEO_ALONE, u64::MAX).0, "off for the stream");
}

fn frame(trc: i32, space: i32, range: i32) -> FrameInfo {
    FrameInfo {
        width: 1920,
        height: 1080,
        kind: media::FRAME_D3D11,
        ten_bit: 1,
        pts: 0,
        data: [std::ptr::null_mut(); 3],
        linesize: [0; 3],
        color_space: space,
        color_range: range,
        color_trc: trc,
    }
}

#[test]
fn without_v_sync_tears_except_a_vrr_stream_on_wayland() {
    // (vrr, wayland, gamescope)
    assert!(prefers_immediate(false, false, false), "X11 without V-Sync tears");
    assert!(prefers_immediate(false, true, false), "Wayland without VRR tears");
    assert!(prefers_immediate(true, false, false), "X11 VRR takes IMMEDIATE");
    assert!(!prefers_immediate(true, true, false), "a VRR stream on a Wayland compositor keeps MAILBOX");
    assert!(prefers_immediate(true, true, true), "gamescope's VRR takes IMMEDIATE");
    assert_eq!(crate::sdl::present_mode_name(Some(1)), "Immediate (tears)");
    assert_eq!(crate::sdl::present_mode_name(Some(2)), "Mailbox (no tearing)");
    assert_eq!(crate::sdl::present_mode_name(Some(0)), "V-Sync (FIFO)");
    assert_eq!(crate::sdl::present_mode_name(None), "set by the renderer");
}

#[test]
fn a_pq_picture_is_a_ycbcr_colorspace_so_a_p010_texture_can_be_made() {
    // The YUV conversion matrix of an RGB colorspace (SDL_COLORSPACE_HDR10) does not exist.
    for (range, expected) in [(1, SDL_COLORSPACE_BT2020_LIMITED), (2, SDL_COLORSPACE_BT2020_FULL), (0, SDL_COLORSPACE_BT2020_LIMITED)] {
        let space = colorspace(&frame(16, 9, range));
        assert_eq!(space.0, expected.0);
        assert_ne!(space.0, SDL_COLORSPACE_HDR10.0);
        assert_eq!((space.0 >> 28) & 0xf, SDL_COLOR_TYPE_YCBCR.0, "a YUV texture needs a YCbCr colorspace");
        assert_eq!((space.0 >> 5) & 0x1f, SDL_TRANSFER_CHARACTERISTICS_PQ.0, "PQ must stay the transfer");
    }
    // A PQ stream whose VUI says nothing about the matrix is still BT.2020.
    assert_eq!(colorspace(&frame(16, 2, 0)).0, SDL_COLORSPACE_BT2020_LIMITED.0);
    assert_eq!(colorspace(&frame(1, 1, 0)).0, SDL_COLORSPACE_BT709_LIMITED.0);
}

#[test]
fn the_texture_gets_the_display_white_and_the_peak_as_headroom() {
    let metadata = nightjar_core::control::HdrMetadata { max_display_luminance: 1000, max_content_light_level: 0, ..Default::default() };
    // White level 2.5 is 200 nits; a 1000-nit peak is 5x that.
    assert_eq!(hdr_properties(true, 2.5, Some(metadata)), HdrProperties { white_point: Some(200.0), headroom: Some(5.0) });
    // SDR streams pass no white point.
    assert_eq!(hdr_properties(false, 1.0, None), HdrProperties { white_point: None, headroom: None });
    // A broken display value (zero, NaN, infinity) is never passed on.
    for white in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(hdr_properties(true, white, Some(metadata)), HdrProperties { white_point: None, headroom: None });
    }
    // No metadata, or an empty one, leaves the headroom to SDL.
    assert_eq!(hdr_properties(true, 1.0, None).headroom, None);
    let empty = nightjar_core::control::HdrMetadata::default();
    assert_eq!(hdr_properties(true, 1.0, Some(empty)).headroom, None);
}

#[test]
fn destroying_the_texture_keeps_what_is_known_about_the_stream() {
    // `ensure` destroys first and reads the PQ flag after; a failing stream logs once, not per picture.
    let mut texture = VideoTexture { hdr_frame: true, ..Default::default() };
    texture.fail("detail".into(), "short".into());
    assert_eq!(texture.failure.as_deref(), Some("short"));
    texture.destroy();
    assert!(texture.hdr_frame, "the PQ flag must survive destroy");
    assert!(texture.is_hdr());
    assert_eq!(texture.logged_failure.as_deref(), Some("detail"));
    assert_eq!(texture.failure.as_deref(), Some("short"));
    assert!(hdr_properties(texture.hdr_frame, 2.5, None).white_point == Some(200.0));
    let mut sdr10 = VideoTexture::new(true, false);
    let mut hdr = VideoTexture::new(true, true);
    sdr10.destroy();
    hdr.destroy();
    assert!(!sdr10.hdr_stream && hdr.hdr_stream, "what the session negotiated must survive destroy");
}

/// A failed upload keeps the last picture's planes for the present, where SDL would have
/// drawn the texture they never went into; a picture that is not planes clears them.
#[test]
fn a_failed_upload_keeps_the_last_planes_on_screen() {
    let last = Some([1; 8]);
    assert_eq!(planes_after_upload(false, None, last, true, last), last);
    assert_eq!(planes_after_upload(true, None, last, true, last), None, "a decoded picture in the texture replaces them");
    assert_eq!(planes_after_upload(true, Some([2; 8]), last, true, Some([2; 8])), Some([2; 8]));
    assert_eq!(planes_after_upload(false, None, last, false, last), None, "the pass that held them is gone");
    assert_eq!(planes_after_upload(false, None, last, true, Some([2; 8])), None, "a new pass for a new shape never held them");
}

/// Plane pictures wait for the back buffer (`draw_direct`) instead of going through the
/// texture, until a direct draw fails, which holds for the rest of the stream.
#[cfg(unix)]
#[test]
fn plane_pictures_go_straight_to_the_back_buffer_until_that_fails() {
    let mut texture = VideoTexture::default();
    assert!(texture.planes_target().is_null(), "the plane pass must only copy the planes");
    texture.planes_drawn(texture.planes_target(), [7; 8]);
    assert_eq!(texture.planes_pending, Some([7; 8]));
    // No picture texture yet: SDL has nothing to draw, the planes stay for the next present.
    assert!(!texture.draw_direct(std::ptr::null_mut(), &SDL_FRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, std::ptr::null_mut()));
    assert_eq!(texture.planes_pending, Some([7; 8]));
    texture.direct_planes_failed = true;
    texture.destroy();
    assert!(texture.direct_planes_failed, "a failed direct draw must survive destroy");
    assert_eq!(texture.planes_pending, None, "a new texture has no planes of the old one");
    let fake = std::ptr::NonNull::<SDL_Texture>::dangling().as_ptr();
    texture.texture = fake;
    assert_eq!(texture.planes_target(), fake);
    texture.planes_drawn(fake, [7; 8]);
    assert_eq!(texture.planes_pending, None, "a picture drawn into the texture is not pending");
    texture.texture = std::ptr::null_mut();
}

#[test]
fn a_10_bit_sdr_stream_keeps_the_sdr_white_until_the_host_says_hdr() {
    // The bug: every 10-bit picture without the host's HDR mode took the display's SDR white
    // (and HDR conversion), so 10-bit SDR came out too dark or too bright.
    use nightjar_core::sdp::{VIDEO_FORMAT_AV1_MAIN10, VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10};
    assert!(!hdr_negotiated(false, VIDEO_FORMAT_H265_MAIN10), "10-bit color (SDR) is not HDR");
    assert!(!hdr_negotiated(false, VIDEO_FORMAT_AV1_MAIN10));
    assert!(hdr_negotiated(true, VIDEO_FORMAT_H265_MAIN10));
    assert!(!hdr_negotiated(true, VIDEO_FORMAT_H265), "HDR asked for, 8-bit chosen");
    let bt709 = 1;
    assert!(!shows_hdr(None, hdr_negotiated(false, VIDEO_FORMAT_H265_MAIN10), bt709));
    assert!(shows_hdr(None, hdr_negotiated(true, VIDEO_FORMAT_H265_MAIN10), bt709));
    // The PQ transfer, and the host's HDR mode once it is known, still decide.
    assert!(shows_hdr(None, false, 16));
    assert!(!shows_hdr(Some(false), true, 16));
    assert!(shows_hdr(Some(true), false, bt709));
}

#[test]
fn planar_pictures_get_the_plane_shader_flags_of_their_colour_description() {
    let info = |kind, width, height, space, range| FrameInfo { kind, width, height, ten_bit: 0, ..frame(1, space, range) };
    let layout = |info: &FrameInfo| media::PlanarLayout::of(info).unwrap();
    // 8-bit 4:4:4, BT.709 limited: matrix 1, 4:4:4 (32).
    let yuv444 = info(media::FRAME_YUV444P, 64, 32, 1, 1);
    assert_eq!(planar_parameters(&yuv444, layout(&yuv444), false, 1.0, 1.0), [64, 32, 64, 32, 0, 3, 4 | 32, 1.0f32.to_bits()]);
    // 10-bit 4:2:0, BT.601 full: full (1), matrix 0, ten (16), FFmpeg's low-bit samples (64).
    let main10 = info(media::FRAME_YUV420P10, 7, 3, 6, 2);
    assert_eq!(planar_parameters(&main10, layout(&main10), false, 1.0, 1.0)[6], 1 | 16 | 64);
    // 10-bit 4:4:4 HDR, BT.2020, white 2.5.
    let hdr = info(media::FRAME_YUV444P10, 16, 16, 9, 1);
    assert_eq!(planar_parameters(&hdr, layout(&hdr), true, 2.5, 4.0)[4..], [0, 3, 2 | 8 | 16 | 32 | 64, 2.5f32.to_bits()]);
    // An HLG one gets the HLG flag (128) beside HDR, so the shader does not read it as PQ;
    // with the host's HDR mode off it is drawn as SDR, without it.
    let hlg = FrameInfo { color_trc: TRC_HLG, ..hdr };
    // Its display comes in place of the wavelet pass's level and operation: a 4 x 200-nit
    // output is an 800-nit HLG display with BT.2100's gamma for it, white staying 2.5.
    let display = super::super::hlg::hlg_display(4.0, 200.0);
    assert_eq!(planar_parameters(&hlg, layout(&hlg), true, 2.5, 4.0)[4..],
        [display.peak.to_bits(), display.gamma.to_bits(), 2 | 8 | 16 | 32 | 64 | 128, 2.5f32.to_bits()]);
    assert_eq!(planar_parameters(&hlg, layout(&hlg), false, 1.0, 1.0)[4..], [0, 3, 8 | 16 | 32 | 64, 1.0f32.to_bits()]);
    // On an SDR output: the 1000-nit reference display with its 203-nit white as SDR white,
    // and the texture says the 1000 / 203 headroom SDL tone maps from.
    assert_eq!(planar_parameters(&hlg, layout(&hlg), true, 1.0, 1.0)[4..],
        [1000.0f32.to_bits(), 1.2f32.to_bits(), 2 | 8 | 16 | 32 | 64 | 128, (203.0f32 / 80.0).to_bits()]);
    let sdr = planar_hlg(&hlg, true, 1.0, 1.0).unwrap();
    assert_eq!(sdr.peak / sdr.white, 1000.0 / 203.0);
    assert!(planar_hlg(&hdr, true, 2.5, 4.0).is_none() && planar_hlg(&hlg, false, 1.0, 1.0).is_none());
    // So an HLG picture keeps the stream going when the output loses HDR; a PQ one ends it.
    let texture = VideoTexture { hlg_frame: true, hdr_mode: Some(true), ..Default::default() };
    assert!(texture.is_hdr() && !texture.needs_hdr_output());
    let texture = VideoTexture { hdr_frame: true, ..Default::default() };
    assert!(texture.needs_hdr_output());
    // Odd 4:2:0 pictures round their chroma up, as FFmpeg does.
    assert_eq!([0, 1, 2].map(|p| layout(&main10).plane_size(p)), [(7, 3), (4, 2), (4, 2)]);
    assert_eq!([0, 1, 2].map(|p| layout(&yuv444).plane_size(p)), [(64, 32); 3]);
    // Only the three planar kinds take this path.
    for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_YUV420P, media::FRAME_D3D11, media::FRAME_WAVELET] {
        assert!(media::PlanarLayout::of(&info(kind, 64, 32, 1, 1)).is_none());
    }
    assert!(media::PlanarLayout::of(&info(media::FRAME_YUV444P, 0, 32, 1, 1)).is_none());
}

/// A Vulkan video picture goes to the plane shader as the planar picture of its depth and
/// chroma, with the colour flags a software-decoded one gets, and flag 256 in place of 64: its
/// chroma is interleaved and its 10-bit samples sit in the high bits. Never through SDL's P010,
/// which SDL reads as PQ.
#[cfg(unix)]
#[test]
fn a_vulkan_picture_is_drawn_as_the_planar_picture_of_its_depth_and_chroma() {
    // BT.2020 (matrix 2, flag 8), limited range, SDR.
    let info = |kind, ten_bit, color_trc| FrameInfo { kind, width: 7, height: 5, ten_bit, color_trc, ..frame(1, 9, 1) };
    let layout = |kind, ten_bit| media::PlanarLayout::of_vulkan(&info(kind, ten_bit, 1)).unwrap();
    let shape = |layout: media::PlanarLayout| (layout.chroma444, layout.ten_bit);
    assert_eq!(shape(layout(media::FRAME_VULKAN, 0)), (false, false));
    assert_eq!(shape(layout(media::FRAME_VULKAN, 1)), (false, true));
    assert_eq!(shape(layout(media::FRAME_VULKAN444, 1)), (true, true));
    assert_eq!([0, 1].map(|p| layout(media::FRAME_VULKAN, 0).plane_size(p)), [(7, 5), (4, 3)]);
    for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_DMABUF, media::FRAME_YUV420P10, 4] {
        assert!(media::PlanarLayout::of_vulkan(&info(kind, 1, 1)).is_none(), "kind {kind}");
    }
    assert!(media::PlanarLayout::of(&info(media::FRAME_VULKAN, 1, 1)).is_none());
    // 10-bit SDR: no HDR flag, the 10-bit flag (16) and 256, never 64.
    let sdr10 = info(media::FRAME_VULKAN, 1, 1);
    let flags = vulkan_flags(planar_parameters(&sdr10, layout(media::FRAME_VULKAN, 1), false, 1.0, 1.0), true)[6];
    assert_eq!(flags, 8 | 16 | 256);
    // HDR10 and HLG keep their flags.
    let pq = FrameInfo { color_trc: TRC_PQ, ..sdr10 };
    assert_eq!(vulkan_flags(planar_parameters(&pq, layout(media::FRAME_VULKAN, 1), true, 2.5, 4.0), true)[6], 2 | 8 | 16 | 256);
    let hlg = FrameInfo { color_trc: TRC_HLG, ..pq };
    assert_eq!(vulkan_flags(planar_parameters(&hlg, layout(media::FRAME_VULKAN, 1), true, 2.5, 4.0), true)[6], 2 | 8 | 16 | 128 | 256);
    // 8-bit 4:4:4, full range.
    let full = FrameInfo { color_range: 2, ..info(media::FRAME_VULKAN444, 0, 1) };
    assert_eq!(vulkan_flags(planar_parameters(&full, layout(media::FRAME_VULKAN444, 0), false, 1.0, 1.0), true)[6], 1 | 8 | 32 | 256);
    assert_eq!(vulkan_flags([0, 0, 0, 0, 0, 0, 16 | 64, 0], false)[6], 16 | 64, "other pictures keep 64");
    // A mapped 4:4:4 VAAPI picture: full chroma, 2048 in place of 256, so its whole 10-bit
    // samples are not taken for high bits.
    let interleaved = info(media::FRAME_VULKAN_INTERLEAVED444, 1, 1);
    assert!(media::interleaved_444(&interleaved) && !media::interleaved_444(&sdr10));
    assert_eq!(shape(layout(media::FRAME_VULKAN_INTERLEAVED444, 1)), (true, true));
    let flags = |info: &FrameInfo, interleaved| interleaved_flags(vulkan_flags(planar_parameters(info, layout(info.kind, info.ten_bit), false, 1.0, 1.0), true), interleaved)[6];
    assert_eq!(flags(&interleaved, true), 8 | 16 | 32 | 2048);
    assert_eq!(flags(&info(media::FRAME_VULKAN_INTERLEAVED444, 0, 1), true), 8 | 32 | 2048);
    assert_eq!(flags(&sdr10, false), 8 | 16 | 256, "two-plane pictures keep 256");
}

/// Linux hardware decoders (VAAPI, NVDEC, V4L2) hand SDL NV12 and P010 pictures, and SDL read
/// an HLG P010 picture as PQ. An HLG one goes to the plane shader with the HLG flag; SDR and
/// PQ ones keep SDL's path (no copy, no added latency).
#[cfg(unix)]
#[test]
fn an_hlg_hardware_picture_is_drawn_as_hlg_on_linux_never_as_pq() {
    let info = |kind, color_trc| FrameInfo { kind, width: 64, height: 32, ten_bit: i32::from(kind == media::FRAME_P010), color_trc, ..frame(1, 9, 1) };
    for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_DMABUF] {
        assert!(hlg_through_planes(&info(kind, TRC_HLG)), "kind {kind}");
        assert!(!hlg_through_planes(&info(kind, TRC_PQ)), "PQ kind {kind} stays SDL's");
        assert!(!hlg_through_planes(&info(kind, 1)), "SDR kind {kind} stays SDL's");
    }
    // Planar pictures already take the plane shader; the others have no NV12 planes.
    for kind in [media::FRAME_YUV420P, media::FRAME_YUV420P10, media::FRAME_WAVELET] {
        assert!(!hlg_through_planes(&info(kind, TRC_HLG)), "kind {kind}");
    }
    // A P010 picture reads as 10-bit 4:2:0 with its samples moved to the low bits (64), and
    // shown as HDR it gets the HLG flag (128), never PQ.
    let p010 = info(media::FRAME_P010, TRC_HLG);
    let layout = media::PlanarLayout::of_semi_planar(&p010).unwrap();
    assert_eq!(layout.plane_size(1), (32, 16));
    assert_eq!(planar_parameters(&p010, layout, true, 2.5, 4.0)[6], 2 | 8 | 16 | 64 | 128);
    let nv12 = info(media::FRAME_NV12, TRC_HLG);
    assert_eq!(planar_parameters(&nv12, media::PlanarLayout::of_semi_planar(&nv12).unwrap(), true, 2.5, 4.0)[6], 2 | 8 | 128);
    assert!(media::PlanarLayout::of_semi_planar(&info(media::FRAME_DMABUF, TRC_HLG)).is_none(), "a surface is mapped first");
}

/// The report: an HDR stream showed "Cannot show d3d11 pictures". Needs a Direct3D 11 display.
#[cfg(windows)]
#[test]
#[ignore = "requires a Direct3D 11 display"]
fn sdl_makes_a_p010_texture_in_the_pq_colorspace_we_choose() {
    let _guard = crate::sdl::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: This test owns SDL and all its resources on one thread.
    unsafe {
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
        let window = SDL_CreateWindow(c"HDR texture test".as_ptr(), 128, 64, SDL_WINDOW_HIDDEN);
        let renderer = crate::sdl::create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        let make = |space: SDL_Colorspace| {
            let metadata = nightjar_core::control::HdrMetadata { max_display_luminance: 1000, ..Default::default() };
            let mut texture = VideoTexture { hdr_frame: true, hdr_mode: Some(true), hdr_metadata: Some(metadata), ..Default::default() };
            // A white level of 2.5 is 200 nits, so a 1000-nit peak is 5x headroom.
            let made = texture.ensure(renderer, SDL_PIXELFORMAT_P010, 1920, 1080, space, 2.5);
            let failure = texture.failure.take();
            let properties = if made { SDL_GetTextureProperties(texture.texture) } else { SDL_PropertiesID(0) };
            let white = SDL_GetFloatProperty(properties, SDL_PROP_TEXTURE_SDR_WHITE_POINT_FLOAT, 0.0);
            let headroom = SDL_GetFloatProperty(properties, SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT, 0.0);
            texture.destroy();
            (made, failure, white, headroom)
        };
        let (made, why, white, headroom) = make(colorspace(&frame(16, 9, 1)));
        assert!(made, "{why:?}");
        assert_eq!((white, headroom), (200.0, 5.0), "the texture must keep the display's white and the peak");
        let (made, why, ..) = make(SDL_COLORSPACE_HDR10);
        assert!(!made, "SDL accepts an RGB colorspace for P010 now; the HDR10 workaround is unnecessary");
        assert!(why.is_some_and(|why| why.contains("YUV colorspace")));
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

#[test]
fn the_pyrowave_stage_timers_survive_destroying_the_texture() {
    let mut texture = VideoTexture::default();
    texture.pyrowave_stages.add(0, std::time::Instant::now());
    texture.pyrowave_stages.commit();
    texture.destroy();
    assert_eq!(texture.pyrowave_stages.frames(), 1);
    assert!(texture.pyrowave_stages.summary().is_some());
}

#[test]
fn scaling_matches_android_fit_fill_stretch_native_and_top_alignment() {
    use crate::config::VideoScaling::*;
    for (mode, top, expected) in [
        (Fit, false, [0.0, 152.0, 1280.0, 720.0]),
        (Fit, true, [0.0, 0.0, 1280.0, 720.0]),
        (Fill, false, [-270.0, 0.0, 1820.0, 1024.0]),
        (Fill, true, [-270.0, 0.0, 1820.0, 1024.0]),
        (Stretch, true, [0.0, 0.0, 1280.0, 1024.0]),
        (Native, false, [-320.0, -28.0, 1920.0, 1080.0]),
        (Native, true, [-320.0, 0.0, 1920.0, 1080.0]),
    ] {
        let r = viewport((1920, 1080), (1280, 1024), mode, top);
        assert_eq!([r.x, r.y, r.w, r.h], expected);
    }
    let r = viewport((0, 0), (1280, 1024), Native, true);
    assert_eq!([r.x, r.y, r.w, r.h], [0.0, 0.0, 1280.0, 1024.0]);
}

#[test]
fn letterbox_keeps_the_aspect_ratio() {
    let r = letterbox((1920, 1080), (2560, 1600));
    assert_eq!((r.x, r.y, r.w, r.h), (0.0, 80.0, 2560.0, 1440.0));
    let r = letterbox((1920, 1080), (1920, 1080));
    assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 1920.0, 1080.0));
    let r = letterbox((1280, 1024), (1920, 1080));
    assert_eq!((r.w, r.h), (1350.0, 1080.0));
}

#[test]
fn the_default_config_shows_frames_immediately_with_vrr_and_waits_for_vsync_without() {
    // Measured on a 240 Hz tearing display: HostTimes held frames about 7 ms and had the
    // less even present intervals, so it is opt-in (docs/pyrowave-decode.md).
    let settings = crate::config::Settings::default();
    assert!(!settings.present_at_host_times);
    assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
    // No tearing support: VRR is not available and the display's refresh is awaited, as before.
    assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
    let no_vrr = crate::config::Settings { vrr: false, ..settings.clone() };
    assert_eq!(pacing(&no_vrr, true), (PacingMode::Vsync, false));
    let no_vsync = crate::config::Settings { vrr: false, vsync: false, ..settings.clone() };
    assert_eq!(pacing(&no_vsync, true), (PacingMode::Immediate, false));
}

#[test]
fn pacing_preserves_vrr_without_host_time_delay() {
    let mut settings = crate::config::Settings { present_at_host_times: true, ..Default::default() };
    assert_eq!(pacing(&settings, true), (PacingMode::HostTimes, true));
    settings.present_at_host_times = false;
    assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
    settings.present_at_host_times = true;
    settings.experimental_packed_444 = true;
    let expected = if cfg!(windows) {
        PacingMode::Immediate
    } else {
        PacingMode::HostTimes
    };
    assert_eq!(pacing(&settings, true), (expected, true));
    assert!(settings.present_at_host_times);
}

#[test]
fn only_a_display_that_scans_at_its_own_rate_turns_a_vrr_stream_to_v_sync() {
    use super::super::scanline::{Beam, RaceCapability, Reason, Verdict};
    let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
    let off = |reason| Verdict::Off(reason, capability);
    let fixed = off(Reason::NoVrr);
    let vsync = PacingMode::Vsync;
    assert_eq!(after_display_check(PacingMode::Immediate, &fixed, vsync), PacingMode::Vsync, "the laptop's 120 Hz panel");
    assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, vsync), PacingMode::Vsync);
    assert_eq!(after_display_check(PacingMode::Vsync, &fixed, vsync), PacingMode::Vsync);
    // V-Sync turned off in the settings: what the stream would have without VRR.
    let immediate = PacingMode::Immediate;
    assert_eq!(after_display_check(PacingMode::Immediate, &fixed, immediate), PacingMode::Immediate);
    assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, immediate), PacingMode::Immediate);
    // Still checking, VRR seen, and every reason that says nothing about the refresh keep the mode.
    let beam = Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0,
        vblank_fraction: 0.7, scans: 50, idle_from: 2040, longest_idle_ns: 0, idle_scans: 0 };
    let mut kept = vec![Verdict::Checking, Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability })];
    kept.extend([Reason::NoTearing, Reason::NoScanline, Reason::NoScan, Reason::NoRestart, Reason::Windowed, Reason::NoIdle, Reason::RowsDiffer, Reason::VrrNoScanline].map(off));
    for verdict in &kept {
        for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace, PacingMode::Vsync] {
            assert_eq!(after_display_check(mode, verdict, PacingMode::Vsync), mode, "{verdict:?}");
        }
    }
}

#[test]
fn the_vrr_sync_switch_sets_the_sync_interval_of_tearing_modes_only() {
    assert_eq!(VrrSync::parse(None), VrrSync::Off);
    assert_eq!(VrrSync::parse(Some("off")), VrrSync::Off);
    assert_eq!(VrrSync::parse(Some(" present1 ")), VrrSync::Present1);
    assert_eq!(VrrSync::parse(Some("scan")), VrrSync::Scan);
    assert_eq!(VrrSync::parse(Some("vsync")), VrrSync::Off, "an unknown value is said and left off");
    assert_eq!(VrrSync::default(), VrrSync::Off, "presents on arrival unless measured otherwise");
    for sync in [VrrSync::Off, VrrSync::Present1, VrrSync::Scan] {
        assert_eq!(sync_interval(PacingMode::Vsync, sync), 1);
        assert_eq!(sync_interval(PacingMode::BeamRace, sync), 0, "slices need tearing");
    }
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Off), 0);
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Present1), 1);
    assert_eq!(sync_interval(PacingMode::HostTimes, VrrSync::Present1), 1);
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Scan), 0, "waits on this thread, then tears");
}

#[test]
fn a_vrr_streams_pacing_follows_the_display_check_once_and_sets_the_interval_once() {
    use super::super::scanline::{RaceCapability, Reason, Verdict};
    let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
    let fixed = Verdict::Off(Reason::NoVrr, capability);
    // V-Sync off: no pacing to change, but the present mode follows the display.
    let mut vrr = VrrPacing::new(false, PacingMode::Immediate, VrrSync::Off, PacingMode::Immediate).with_present_check(true);
    let mut mode = PacingMode::Immediate;
    assert!(vrr.display_vrr(), "VRR until the check says otherwise");
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
    assert!(vrr.display_vrr());
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None, "no new sync interval");
    assert_eq!((mode, vrr.fell_back, vrr.display_vrr()), (PacingMode::Immediate, false, false));
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Off(Reason::VrrNoScanline, RaceCapability { vrr: true, ..capability })), 0), None);
    assert!(vrr.display_vrr(), "the display refreshes on present again");
    // Without either check, a verdict means nothing to it.
    let mut unchecked = VrrPacing::new(false, PacingMode::Immediate, VrrSync::Off, PacingMode::Immediate);
    unchecked.step(&mut mode, Some(&fixed), 0);
    assert!(unchecked.display_vrr());
    let settings = crate::config::Settings::default();
    let (start, _) = pacing(&settings, true);
    assert_eq!(start, PacingMode::Immediate);
    let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
    let mut mode = start;
    assert_eq!(vrr.step(&mut mode, None, 0), None, "no probe yet: the renderer keeps interval 0");
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1), "V-Sync waits for the refresh");
    assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None, "given once");
    // V-Sync off in the settings: immediate stays, with nothing for the renderer.
    let no_vsync = crate::config::Settings { vsync: false, present_at_host_times: true, ..Default::default() };
    let (start, _) = pacing(&no_vsync, true);
    assert_eq!(start, PacingMode::HostTimes);
    let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, start);
    let mut mode = start;
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None);
    assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, true));
    // NIGHTJAR_VRR_CHECK=0: the verdict is not applied; present1 still sets interval 1.
    let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Present1, PacingMode::Immediate);
    let mut mode = PacingMode::Immediate;
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
    assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
    // A stream that starts on V-Sync has interval 1 already.
    let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync);
    let mut mode = PacingMode::Vsync;
    assert_eq!(vrr.step(&mut mode, None, 0), None);
}

#[test]
fn a_v_sync_fall_back_is_rechecked_and_returns_once_the_display_refreshes_on_present() {
    use super::super::scanline::{Beam, RaceCapability, Reason, Verdict};
    let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
    let fixed = Verdict::Off(Reason::NoVrr, capability);
    let beam = Beam { rows: 2160, blank_rows: 90, total_rows: 2250, scan_ns: 8_310_000, refresh_hz: 120.3, lines_per_ms: 270.8,
        vblank_fraction: 0.45, scans: 80, idle_from: 2521, longest_idle_ns: 17_000_000, idle_scans: 30 };
    let on = Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability });
    let idles = Verdict::Off(Reason::NoIdle, RaceCapability { vrr: true, ..capability });
    let busy = Verdict::Off(Reason::NoIdle, capability);
    let second = 1_000_000;
    let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Immediate);
    let mut mode = PacingMode::Immediate;
    // nightjar#72's resume: no VRR, V-Sync.
    assert!(vrr.probe_tears(mode));
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
    assert!(vrr.probe_tears(mode), "rechecked with the tearing it had");
    assert!(!VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync).probe_tears(PacingMode::Vsync), "V-Sync from the settings");
    assert!(!vrr.recheck_due(0));
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None);
    assert!(!vrr.recheck_due(RECHECK_FIRST_US - 1), "nothing until the first recheck");
    assert!(vrr.recheck_due(RECHECK_FIRST_US));
    assert!(!vrr.recheck_due(RECHECK_FIRST_US), "asked once");
    // The probe measures again; the old verdict until its next present plans nothing.
    assert_eq!(vrr.step(&mut mode, Some(&fixed), RECHECK_FIRST_US), None);
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), RECHECK_FIRST_US + second), None);
    assert!(!vrr.recheck_due(100 * RECHECK_MAX_US), "none while one runs");
    // At 120 fps on the 120 Hz display the presents never let it idle, and the calibration
    // saw no idle either: nothing says it refreshes on present, V-Sync stays and waits twice as long.
    let ended = RECHECK_FIRST_US + 2 * second;
    assert_eq!(vrr.step(&mut mode, Some(&busy), ended), None);
    assert_eq!(mode, PacingMode::Vsync);
    assert!(!vrr.recheck_due(ended + 2 * RECHECK_FIRST_US - 1));
    assert!(vrr.recheck_due(ended + 2 * RECHECK_FIRST_US));
    // Now the presents restart the scan: Immediate again, interval 0.
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
    assert_eq!(vrr.step(&mut mode, Some(&on), 0), Some(0));
    assert_eq!((mode, vrr.fell_back, vrr.recovered), (PacingMode::Immediate, false, true));
    assert!(vrr.probe_tears(mode));
    assert_eq!(vrr.step(&mut mode, Some(&on), 0), None);
    assert!(!vrr.recheck_due(u64::MAX), "nothing to recheck on Immediate");
    // No VRR once more after returning: V-Sync for good, no flapping.
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
    for at in [0, RECHECK_MAX_US, u64::MAX] {
        assert_eq!(vrr.step(&mut mode, Some(&fixed), at), None);
        assert!(!vrr.recheck_due(at));
    }
    assert_eq!(vrr.step(&mut mode, Some(&on), 0), Some(0), "the probe's own later verdict still counts");
    // A recheck with no idle to test, on a display the calibration saw idle, is no restart seen:
    // V-Sync stays; the waits never grow past the longest, and a passed check returns.
    let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::HostTimes);
    let mut mode = PacingMode::HostTimes;
    vrr.step(&mut mode, Some(&fixed), 0);
    let mut at = 0;
    for _ in 0..10 {
        vrr.step(&mut mode, Some(&fixed), at);
        at += RECHECK_MAX_US;
        assert!(vrr.recheck_due(at));
    }
    assert_eq!(vrr.step(&mut mode, Some(&idles), at), None);
    assert_eq!(mode, PacingMode::Vsync);
    assert_eq!(vrr.step(&mut mode, Some(&on), at), Some(0));
    assert_eq!(mode, PacingMode::HostTimes, "the pacing it had before");
}

#[test]
fn a_linux_vrr_stream_on_a_display_without_vrr_waits_for_the_refresh() {
    use super::super::drm_vrr::{verdict, Reading};
    // nightjar#80 and the Fedora 8600G report: Linux has no scan line, so the check said "no
    // scanline" and the stream stayed immediate on a 144 Hz display without VRR. SDL's GPU
    // renderer takes vsync 0 as MAILBOX first, which at the panel's own rate replaces and
    // repeats pictures ("replaced before they were shown" at the stream's end), and HostTimes
    // was worse still (display wait p95 31-65 ms against 19 ms with V-Sync). DRM's
    // VRR_ENABLED 0 now turns both to V-Sync.
    let off = Reading::Vrr { enabled: false, output: "DP-1".into(), capable: Some(1) };
    let on = Reading::Vrr { enabled: true, output: "DP-1".into(), capable: Some(1) };
    let settings = crate::config::Settings::default();
    for start in [PacingMode::Immediate, PacingMode::HostTimes] {
        let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
        let mut mode = start;
        assert_eq!(vrr.step(&mut mode, Some(&verdict(true, None)), 0), None, "still reading: the pacing stays");
        assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&off))), 0), Some(1), "V-Sync waits for the refresh");
        assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
        // A recheck that reads VRR on (the user turned it on) returns to the pacing it had.
        let tears = vrr.probe_tears(mode);
        assert!(tears, "rechecked with the pacing it had");
        assert_eq!(vrr.step(&mut mode, Some(&verdict(tears, Some(&on))), 0), Some(0));
        assert_eq!(mode, start);
    }
    // Where VRR runs (KDE adaptive sync, GNOME VRR, gamescope) nothing changes.
    let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, PacingMode::Immediate);
    let mut mode = PacingMode::Immediate;
    assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&on))), 0), None);
    assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
    // Nor where the output cannot be told: today's behaviour.
    assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&Reading::Unknown("two outputs".into())))), 0), None);
    assert_eq!(mode, PacingMode::Immediate);
    // V-Sync off in the settings: immediate, as without VRR.
    let no_vsync = crate::config::Settings { vsync: false, ..Default::default() };
    let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, PacingMode::HostTimes);
    let mut mode = PacingMode::HostTimes;
    vrr.step(&mut mode, Some(&verdict(true, Some(&off))), 0);
    assert_eq!(mode, PacingMode::Immediate);
}

#[test]
fn pacing_keeps_the_non_vrr_fallback() {
    let mut settings = crate::config::Settings {
        experimental_packed_444: true,
        ..Default::default()
    };
    assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
    settings.vrr = false;
    assert_eq!(pacing(&settings, true), (PacingMode::Vsync, false));
    settings.vsync = false;
    assert_eq!(pacing(&settings, true), (PacingMode::Immediate, false));
}

/// A video texture of `texture` size holding a decoded picture of that size from a `stream`
/// of that size, cropped as `pump` and `upload` crop it. The texture pointer is never
/// dereferenced.
fn holding(texture: (i32, i32), stream: (i32, i32)) -> VideoTexture {
    let mut video = VideoTexture::new(true, false);
    video.texture = std::ptr::NonNull::dangling().as_ptr();
    (video.width, video.height) = texture;
    video.format = SDL_PIXELFORMAT_NV12;
    video.visible = visible_part(nightjar_core::video::crop::visible_size(texture, &[stream]), texture);
    video
}

#[test]
fn a_padded_av1_picture_is_drawn_without_its_padding() {
    // AMD's AV1 encoder codes a 2184x1968 stream as 2240x1968 (rust/PERFORMANCE_WORK.md).
    let video = holding((2240, 1968), (2184, 1968));
    assert_eq!(video.size(), (2184, 1968));
    let source = video.source_rect(video.texture).expect("SDL draws part of the texture");
    assert_eq!((source.x, source.y, source.w, source.h), (0.0, 0.0, 2183.5, 1967.5), "the 56 padding columns stay out, half a texel in");
    assert!(video.source_rect(std::ptr::NonNull::<SDL_Texture>::dangling().as_ptr().wrapping_add(1)).is_none(), "an upscaled or enhanced texture is all picture");
    // Every scaling mode places the stream's size: Fit fills a window of its aspect ratio.
    let fit = viewport(video.size(), (1092, 984), crate::config::VideoScaling::Fit, false);
    assert_eq!((fit.x, fit.y, fit.w, fit.h), (0.0, 0.0, 1092.0, 984.0));
    let fill = viewport(video.size(), (1092, 984), crate::config::VideoScaling::Fill, false);
    assert_eq!((fill.w, fill.h), (1092.0, 984.0));
    // 1920x1080 is coded as 1920x1082.
    let video = holding((1920, 1082), (1920, 1080));
    assert_eq!(video.size(), (1920, 1080));
    assert_eq!(video.source_rect(video.texture).map(|rect| (rect.w, rect.h)), Some((1919.5, 1079.5)));
}

#[test]
fn a_picture_of_the_stream_size_or_smaller_is_drawn_whole() {
    for (texture, stream) in [((1920, 1080), (1920, 1080)), ((2184, 1968), (2184, 1968)), ((1280, 720), (1920, 1080)), ((2560, 1440), (1920, 1080))] {
        let video = holding(texture, stream);
        assert_eq!(video.size(), texture, "{texture:?} for a {stream:?} stream");
        assert!(video.source_rect(video.texture).is_none(), "SDL draws the whole texture, as before");
    }
    // No stream size known: no crop.
    assert_eq!(holding((2240, 1968), (0, 0)).size(), (2240, 1968));
    // A visible part never reaches past the texture.
    assert_eq!(visible_part((2184, 1968), (2184, 1960)), None);
}

#[test]
fn a_new_texture_drops_the_crop() {
    let mut video = holding((2240, 1968), (2184, 1968));
    video.texture = std::ptr::null_mut();
    video.destroy();
    assert_eq!(video.visible, None);
}

#[test]
fn an_uploaded_padded_picture_is_shown_without_its_padding() {
    // A real decoded 128x128 picture through `upload` on a software renderer (no window), as a
    // 120x120 stream's picture with its padding.
    // SAFETY: SDL calls on a surface and renderer made and used on this thread.
    unsafe {
        let surface = SDL_CreateSurface(256, 256, SDL_PIXELFORMAT_RGBA32);
        assert!(!surface.is_null(), "{}", crate::sdl::error());
        let renderer = SDL_CreateSoftwareRenderer(surface);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        let mut video = VideoTexture::new(true, false);
        let mut picture = decoded(0);
        picture.setup.chroma_packing = 0;
        let coded = picture.picture.frame().map(|frame| (frame.info().width, frame.info().height)).unwrap();
        assert_eq!(coded, (128, 128));
        picture.visible = Some(nightjar_core::video::crop::visible_size(coded, &[(120, 120)]));
        assert!(video.upload(renderer, &picture, None), "{:?}", video.failure);
        assert_eq!(video.size(), (120, 120));
        let rect = video.source_rect(video.texture).expect("SDL draws part of the texture");
        assert_eq!((rect.x, rect.y, rect.w, rect.h), (0.0, 0.0, 119.5, 119.5));
        // A picture of the stream's size is drawn whole.
        picture.visible = Some(coded);
        assert!(video.upload(renderer, &picture, None), "{:?}", video.failure);
        assert_eq!(video.size(), coded);
        assert!(video.source_rect(video.texture).is_none());
        // A PyroWave or packed picture is never cropped by it.
        picture.setup.chroma_packing = 1;
        picture.visible = Some((120, 120));
        video.upload(renderer, &picture, None);
        assert_ne!(video.size(), (120, 120));
        video.destroy();
        SDL_DestroyRenderer(renderer);
        SDL_DestroySurface(surface);
    }
}

#[cfg(windows)]
#[test]
fn the_direct_pass_reads_only_the_stream_part_of_a_padded_picture() {
    let mut video = holding((2240, 1968), (2184, 1968));
    video.d3d11_picture = true;
    video.copied = true;
    video.d3d11 = std::ptr::NonNull::<c_void>::dangling().as_ptr();
    // The pass's shader samples `picture / textureSize` of the 2240-wide texture.
    assert_eq!(video.picture().map(|picture| picture.size), Some((2184, 1968)));
    assert_eq!(video.enhance_source().map(|source| source.picture), Some((2184, 1968)));
}

pub(crate) fn decoded(ready_us: u64) -> Decoded {
    let mut decoder =
        media::VideoDecoder::open(media::Codec::Hevc, media::Hardware::None, 1, false).unwrap();
    assert_eq!(
        decoder.send(
            include_bytes!("../../../../../app/src/main/assets/packed444/hevc-8.bin"),
            0
        ),
        0
    );
    let media::Received::Frame(frame) = decoder.receive() else {
        panic!("fixture did not decode");
    };
    Decoded {
        picture: media::Picture::Frame(frame),
        setup: nightjar_core::session::VideoSetup {
            format: nightjar_core::sdp::VIDEO_FORMAT_H265,
            width: 128,
            height: 128,
            fps: 60,
            chroma_packing: 1,
            logical_width: 128,
            logical_height: 64,
            color_space: 1,
            color_range: 1,
        },
        rtp: ready_us as u32,
        number: 0,
        receive_us: 0,
        enqueue_us: 0,
        input_us: 0,
        ready_us,
        host_latency: 0,
        early: 0,
        first_block_us: 0,
        race: Default::default(),
        bands: Vec::new(),
        race_seq: 0,
        visible: None,
    }
}

#[test]
fn immediate_pacing_takes_the_newest_ready_picture_without_holding_it() {
    let mut pacer = Pacer::new(PacingMode::Immediate, 16_667);
    assert!(pacer.next_due().is_none());
    for ready in [10, 20, 30] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(25);
    let picture = picture.unwrap();
    assert_eq!(picture.decoded.ready_us, 20);
    assert_eq!(picture.present_us, 20);
    assert_eq!(picture.hold_us, 0);
    assert_eq!(dropped, 1);
    assert_eq!(pacer.next_due(), Some(30));
    let (picture, dropped) = pacer.take_due(30);
    assert_eq!(picture.unwrap().decoded.ready_us, 30);
    assert_eq!(dropped, 0);
    assert!(pacer.next_due().is_none());
}

#[test]
fn the_front_picture_says_which_raced_frame_it_is_and_when_it_is_due() {
    let mut pacer = Pacer::new(PacingMode::BeamRace, 16_667);
    assert_eq!(pacer.front_race(), None);
    let mut picture = decoded(40);
    picture.race_seq = 7;
    pacer.accept(picture);
    pacer.accept(decoded(50));
    assert_eq!(pacer.front_race(), Some((7, 40)));
}

#[cfg(windows)]
#[test]
fn an_hdr_stream_is_pq_without_a_pq_transfer_in_the_bitstream() {
    use super::super::direct::Transfer::{Pq, Sdr};
    // The host's HDR mode decides, not the VUI: a host that leaves the transfer out still streams PQ.
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(true), false, false, false), Pq);
    // Before the HDR mode arrives, the negotiated HDR or the PQ transfer decides.
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, true, false, false), Pq);
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, false, true, false), Pq);
    // 10-bit SDR, and HDR mode off, are not PQ.
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, false, false, false), Sdr);
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(false), true, true, false), Sdr);
    // An 8-bit picture is never PQ.
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_NV12, Some(true), true, true, false), Sdr);
}

#[cfg(windows)]
#[test]
fn an_hlg_picture_is_hlg_on_every_path_never_pq() {
    use super::super::direct::Transfer::{Hlg, Pq, Sdr};
    // The HLG transfer in the VUI makes the picture HLG, with or without the host's HDR mode
    // or negotiated HDR, 10-bit or 8-bit.
    for (hdr_mode, hdr_stream) in [(Some(true), true), (None, true), (None, false), (Some(true), false)] {
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, hdr_mode, hdr_stream, false, true), Hlg, "{hdr_mode:?} {hdr_stream}");
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_NV12, hdr_mode, hdr_stream, false, true), Hlg, "{hdr_mode:?} {hdr_stream}");
    }
    assert!(shows_hdr(None, false, TRC_HLG), "HLG is HDR");
    // The host's HDR mode off still means SDR, as it does for PQ.
    assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(false), true, false, true), Sdr);
    // SDL's shaders know no HLG: whatever it is in, SDL gets the pass's conversion.
    assert!(sdl_needs_rgb(SDL_PIXELFORMAT_P010, Hlg) && sdl_needs_rgb(SDL_PIXELFORMAT_NV12, Hlg));
    assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_P010, Pq));
    // And the picture counts as HDR (upscaler gate, HDR output check).
    let texture = VideoTexture { hlg_frame: true, ..Default::default() };
    assert!(texture.is_hdr());
    assert_eq!(texture.transfer(), Sdr, "no picture format yet");
    let texture = VideoTexture { hlg_frame: true, format: SDL_PIXELFORMAT_P010, ..Default::default() };
    assert_eq!(texture.transfer(), Hlg);
}

#[cfg(windows)]
#[test]
fn sdl_gets_rgb_exactly_when_its_shaders_would_decode_the_picture_wrongly() {
    use super::super::direct::Transfer::{Pq, Sdr};
    // SDL decodes P010 as PQ and NV12 as gamma: every case of the one decision.
    for (format, hdr_mode, hdr_stream, hdr_frame) in [
        (SDL_PIXELFORMAT_P010, Some(true), false, false),
        (SDL_PIXELFORMAT_P010, None, false, false),
        (SDL_PIXELFORMAT_P010, Some(false), true, true),
        (SDL_PIXELFORMAT_NV12, Some(true), true, true),
        (SDL_PIXELFORMAT_NV12, None, false, false),
    ] {
        let transfer = picture_transfer(format, hdr_mode, hdr_stream, hdr_frame, false);
        let sdl_decodes_pq = format == SDL_PIXELFORMAT_P010;
        assert_eq!(sdl_needs_rgb(format, transfer), sdl_decodes_pq != (transfer == Pq), "{} {hdr_mode:?} {hdr_stream} {hdr_frame}", format.0);
    }
    // 10-bit SDR is the case: converted, not shown as PQ.
    assert!(sdl_needs_rgb(SDL_PIXELFORMAT_P010, Sdr));
    assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_P010, Pq));
    assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_NV12, Sdr));
}

#[test]
fn frame_pacing_other_than_lowest_latency_waits_for_the_refresh_without_vrr_only() {
    let mut settings = crate::config::Settings { vrr: false, vsync: false, ..Default::default() };
    assert_eq!(settings.frame_pacing, FramePacing::Latency, "the default keeps today's pacing");
    assert_eq!(pacing(&settings, true).0, PacingMode::Immediate, "lowest latency follows the V-Sync setting");
    for mode in [FramePacing::Balanced, FramePacing::CapFps, FramePacing::Smoothness] {
        settings.frame_pacing = mode;
        assert_eq!(pacing(&settings, true).0, PacingMode::Vsync, "{mode:?}");
    }
    let vrr = crate::config::Settings { frame_pacing: FramePacing::Smoothness, ..Default::default() };
    assert_eq!(pacing(&vrr, true), (PacingMode::Immediate, true), "VRR ignores frame pacing, as on Android");
    assert_eq!(frame_us(60), 16_666);
    assert_eq!(frame_us(0), 250_000, "an unknown rate holds at most 250 ms");
}

/// The report: 120 fps on a 120 Hz panel took 22 ms from arrival to the display, 7 ms when it
/// skips the pictures that would wait, and a 240 Hz panel never waited.
#[test]
fn a_stream_with_no_refreshes_to_spare_skips_instead_of_waiting() {
    let skips = |pacing, fps, hz| skips_instead_of_waiting(PacingMode::Vsync, pacing, frame_us(fps), hz);
    for pacing in [FramePacing::Latency, FramePacing::Balanced] {
        for (fps, hz) in [(120, 120.0), (60, 60.0), (60, 59.94), (60, 60.5), (240, 240.0), (120, 60.0), (144, 120.0)] {
            assert!(skips(pacing, fps, hz), "{pacing:?} {fps} fps at {hz} Hz has none to spare");
        }
        for (fps, hz) in [(120, 240.0), (60, 120.0), (90, 120.0), (120, 144.0), (30, 60.0), (115, 120.0)] {
            assert!(!skips(pacing, fps, hz), "{pacing:?} {fps} fps at {hz} Hz drains by itself, and keeps every picture");
        }
        assert!(!skips(pacing, 120, 0.0), "no refresh rate known");
    }
    for pacing in [FramePacing::CapFps, FramePacing::Smoothness] {
        assert!(!skips(pacing, 120, 120.0), "{pacing:?} keeps its pictures in order");
    }
    for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace] {
        assert!(!skips_instead_of_waiting(mode, FramePacing::Latency, frame_us(120), 120.0), "{mode:?} does not wait");
    }
    let pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Latency, 120);
    assert!(pacer.skips_instead_of_waiting(120.0));
    assert!(!pacer.skips_instead_of_waiting(240.0));
}

/// A stream dragged to a faster display, or changed by automatic quality, follows its new rate.
#[test]
fn a_new_frame_rate_decides_whether_a_stream_skips() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 16_666).with_frame_pacing(FramePacing::Latency, 60);
    assert!(!pacer.skips_instead_of_waiting(120.0), "60 fps has refreshes to spare at 120 Hz");
    pacer.set_fps(120);
    assert!(pacer.skips_instead_of_waiting(120.0), "120 fps has none");
    assert_eq!(pacer.frame_us(), frame_us(120));
    pacer.set_fps(60);
    assert!(!pacer.skips_instead_of_waiting(120.0));
}

/// The next picture replaces a refused one, which is shown after all only if none comes.
#[test]
fn a_kept_picture_gives_way_to_a_newer_one_and_is_drawn_once_when_none_comes() {
    let mut kept = Some(1);
    assert_eq!(take_over_kept(None, &mut kept, false), (None, 0), "not yet");
    assert_eq!(kept, Some(1));
    assert_eq!(take_over_kept(None, &mut kept, true), (Some(1), 0), "drawn, so not replaced");
    assert_eq!(kept, None);
    assert_eq!(take_over_kept(None, &mut kept, true), (None, 0), "once");

    let mut kept = Some(1);
    assert_eq!(take_over_kept(Some(2), &mut kept, false), (Some(2), 1), "replaced");
    assert_eq!(kept, None);
    let mut kept = Some(1);
    assert_eq!(take_over_kept(Some(2), &mut kept, true), (Some(2), 1), "the newer one wins when both are due");
    let mut kept = None;
    assert_eq!(take_over_kept(Some(2), &mut kept, false), (Some(2), 0), "nothing to replace");
}

/// A still desktop sends no next picture, and the screen must not stay one behind.
#[test]
fn a_skipped_picture_is_drawn_after_all_unless_a_newer_one_reaches_the_display() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    assert!(!skipped.take_due(u64::MAX));
    skipped.skipped(1_000, 8_333, 8_333);
    assert_eq!(skipped.due_us(), Some(13_499), "after the next picture would have come");
    assert!(!skipped.take_due(13_498));
    assert!(!skipped.must_wait());
    assert!(skipped.take_due(13_499));
    assert!(skipped.must_wait(), "nothing later would take its place, so it gets its turn");
    assert!(!skipped.take_due(u64::MAX), "once");
    skipped.shown();
    assert!(!skipped.must_wait());
    skipped.skipped(1_000, 8_333, 8_333);
    skipped.shown();
    assert_eq!(skipped.due_us(), None);
    assert!(!skipped.take_due(u64::MAX), "a newer one showed");
}

/// A GPU that is behind gets its work held back: pictures nobody sees would pile up on it.
#[test]
fn presents_refused_again_and_again_wait_once() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..REFUSED_IN_A_ROW {
        assert!(!skipped.must_wait());
        skipped.skipped(1_000, 8_333, 8_333);
    }
    assert!(skipped.must_wait());
    skipped.shown();
    assert!(!skipped.must_wait());
}

/// At 60 Hz the stream arrived 1.6 ms before the refresh: no hold showed 37 pictures a second,
/// a 1 ms hold 50, 3 ms 60. A display whose deadline is earlier must not stay at half the rate.
#[test]
fn too_short_a_hold_grows_until_every_other_picture_stops_being_refused() {
    let (frame, refresh) = (16_666, 16_666);
    let mut skipped = SkippedPicture::new(std::time::Duration::ZERO);
    let mut grew = 0;
    for _ in 0..4_000 {
        if skipped.hold_us >= 3_000 { break; }
        if skipped.skipped(0, frame, refresh).is_some() { grew += 1; }
        skipped.shown();
    }
    assert_eq!((skipped.hold_us, grew), (3_000, 6), "half the pictures refused: it grows step by step");
}

#[test]
fn a_hold_that_grew_comes_back_once_the_pictures_stop_being_refused() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    while skipped.skipped(0, 8_333, 8_333).is_none() {}
    assert_eq!(skipped.hold_us, 5_500);
    let left = PICTURES_SHORTEN - skipped.pictures;
    for _ in 0..left - 1 { skipped.shown(); }
    assert_eq!(skipped.hold_us, 5_500);
    skipped.shown();
    assert_eq!(skipped.hold(), REFUSED_HOLD, "a step shorter");
    for _ in 0..PICTURES_SHORTEN { skipped.shown(); }
    assert_eq!(skipped.hold(), REFUSED_HOLD, "never below where it started");
}

/// A late picture is refused now and then for good, or the hold would only ever grow.
#[test]
fn late_pictures_now_and_then_do_not_keep_a_hold_long() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    while skipped.skipped(0, 8_333, 8_333).is_none() {}
    assert_eq!(skipped.hold_us, 5_500);
    for n in 0..2 * PICTURES_SHORTEN {
        if n % 100 == 0 { skipped.skipped(0, 8_333, 8_333); } else { skipped.shown(); }
    }
    assert_eq!(skipped.hold(), REFUSED_HOLD, "1 in 100 refused");
    // More than that, and it stays: this display needs it.
    while skipped.skipped(0, 8_333, 8_333).is_none() {}
    assert_eq!(skipped.hold_us, 5_500);
    for n in 0..2 * PICTURES_SHORTEN {
        if n % 16 == 0 { skipped.skipped(0, 8_333, 8_333); } else { skipped.shown(); }
    }
    assert_eq!(skipped.hold_us, 5_500, "1 in 16 refused");
}

/// Frame statistics for a picture on `refresh`, read on the same refresh.
fn on_display(count: u32, refresh: u32, at_us: u64) -> crate::timing_csv::Flip {
    crate::timing_csv::Flip { present_count: count.wrapping_add(1), stat_present_count: count, stat_present_refresh: refresh, stat_sync_refresh: refresh, stat_sync_us: at_us }
}

/// The report: a picture presented 4 ms into a refresh reached the display two refreshes later.
#[test]
fn a_picture_a_refresh_late_counts_as_missed_and_a_gap_in_the_stream_does_not() {
    const REFRESH: u64 = 8_333;
    let mut watch = MissWatch::default();
    watch.presented(10, 1_000);
    assert!(!watch.seen(on_display(10, 100, 8_333), REFRESH), "the first statistics have nothing before them");
    watch.presented(11, 9_400);
    assert!(!watch.seen(on_display(11, 101, 16_666), REFRESH), "the next refresh");
    assert!(!watch.seen(on_display(11, 101, 16_666), REFRESH), "the same picture again");
    watch.presented(12, 20_700);
    assert!(watch.seen(on_display(12, 103, 33_332), REFRESH), "presented 12.6 ms before it was shown, and a refresh repeated");
    // The stream paused: the next picture came just before its refresh.
    watch.presented(13, 78_000);
    assert!(!watch.seen(on_display(13, 109, 83_330), REFRESH), "a gap, not a miss");
    // Read a refresh after the flip.
    watch.presented(14, 92_000);
    let read_later = crate::timing_csv::Flip { stat_sync_refresh: 112, stat_sync_us: 108_329, ..on_display(14, 111, 0) };
    assert!(!watch.seen(read_later, REFRESH), "under a refresh from the present to the flip, whenever it was read");
    // Statistics that skipped a picture still add up.
    let mut watch = MissWatch::default();
    assert!(!watch.seen(on_display(20, 200, 8_333), REFRESH));
    watch.presented(22, 0);
    assert!(!watch.seen(on_display(22, 202, 25_000), REFRESH), "two pictures on two refreshes");
    // The counters wrap.
    let mut watch = MissWatch::default();
    assert!(!watch.seen(on_display(u32::MAX, u32::MAX, 8_333), REFRESH));
    watch.presented(1, 0);
    assert!(watch.seen(on_display(1, 2, 33_000), REFRESH), "two pictures on three refreshes, across the wrap");
    // A renderer without statistics.
    assert!(!MissWatch::default().seen(Default::default(), REFRESH));
}

#[test]
fn many_missed_refreshes_make_each_picture_wait_its_turn() {
    const REFRESH: u64 = 8_333;
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(REFRESH, REFRESH), None, "one bad spell passes"); }
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(REFRESH, REFRESH), None); }
    assert_eq!(skipped.hold(), REFUSED_HOLD, "now and then is the price of no queue");
    assert_eq!(skipped.missed(REFRESH, REFRESH), Some(std::time::Duration::from_micros(REFRESH + HOLD_STEP_US)));
    assert!(skipped.hold() > std::time::Duration::from_micros(REFRESH), "a whole refresh always gets its turn");
    for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(REFRESH, REFRESH), None, "it holds no longer than that"); }
    // It comes back, and tries again.
    let mut looks = 0;
    while skipped.hold() > REFUSED_HOLD {
        for _ in 0..PICTURES_SHORTEN { skipped.shown(); }
        looks += 1;
    }
    assert_eq!(looks, 8, "half a minute a step at 120 Hz");
}

#[test]
fn misses_spread_over_many_pictures_leave_the_hold_as_it_is() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..20 {
        for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(8_333, 8_333), None); }
        for _ in 0..MISSED_OF { skipped.shown(); }
    }
    assert_eq!(skipped.hold(), REFUSED_HOLD);
}

/// A spell on Wi-Fi: a window of many misses among clean ones.
#[test]
fn a_bad_spell_now_and_then_leaves_the_hold_as_it_is() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..10 {
        for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(8_333, 8_333), None); }
        for _ in 0..2 * MISSED_OF { skipped.shown(); }
    }
    assert_eq!(skipped.hold(), REFUSED_HOLD);
}

/// Every run missed most in its first seconds, while the window was still being placed.
#[test]
fn misses_while_a_stream_settles_do_not_count() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..MISSED_OF - 1 {
        assert_eq!(skipped.missed(8_333, 8_333), None);
        skipped.shown();
    }
    assert_eq!(skipped.hold(), REFUSED_HOLD);
    skipped.shown();
    for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(8_333, 8_333), None, "the count starts here"); }
    assert_eq!(skipped.missed(8_333, 8_333), None, "the first window of many");
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(8_333, 8_333), None); }
    assert!(skipped.missed(8_333, 8_333).is_some(), "the second in a row");
}

#[test]
fn the_desktop_deadline_stays_under_most_of_a_refresh() {
    assert_eq!(deadline_before_refresh(60.0), 7_166);
    assert_eq!(deadline_before_refresh(120.0), 5_000);
    assert_eq!(deadline_before_refresh(240.0), 2_500);
    assert_eq!(deadline_before_refresh(0.0), 0, "no display timing");
}

#[test]
fn the_phase_lock_aims_at_the_deadline_only_where_a_picture_has_to_make_it() {
    assert_eq!(phase_lock_aim_us(true, false, None, 120.0), 5_000);
    assert_eq!(phase_lock_aim_us(true, false, Some(1_000), 120.0), 1_000, "a measurement run's");
    assert_eq!(phase_lock_aim_us(false, false, None, 120.0), 0, "a present that waits, Linux's among them");
    assert_eq!(phase_lock_aim_us(true, true, None, 120.0), 0, "exclusive fullscreen");
}

/// 120 fps on 60 Hz waiting its turn was the 51 ms queue.
#[test]
fn a_stream_faster_than_the_display_never_waits_for_its_misses() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..4 {
        for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(8_333, 16_666), None); }
        for _ in 0..MISSED_OF { skipped.shown(); }
    }
    assert_eq!(skipped.hold(), REFUSED_HOLD);
}

/// 17 ms grown at 60 Hz would be a queue at 120 Hz.
#[test]
fn another_display_starts_from_the_first_hold() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    while skipped.skipped(0, 16_666, 16_666).is_none() {}
    assert!(skipped.hold() > REFUSED_HOLD);
    skipped.restart();
    assert_eq!(skipped.hold(), REFUSED_HOLD);
    for _ in 0..REFUSED_OF_64_WIDENS - 1 { assert_eq!(skipped.skipped(0, 8_333, 8_333), None, "what it counted went too"); }
    // The new display settles as a new stream does.
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..MISSED_OF { skipped.shown(); }
    skipped.restart();
    for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(8_333, 8_333), None); }
    for _ in 0..MISSED_OF { skipped.shown(); }
    for _ in 0..MISSED_WAIT { assert_eq!(skipped.missed(8_333, 8_333), None, "those did not count"); }
}

#[test]
fn a_late_picture_now_and_then_leaves_the_hold_as_it_is() {
    let (frame, refresh) = (8_333, 8_333);
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for n in 0..100_000 {
        if n % 16 == 0 {
            assert_eq!(skipped.skipped(0, frame, refresh), None, "1 in 16, more than the 0.5 to 1% of the runs");
        }
        skipped.shown();
    }
    assert_eq!(skipped.hold(), REFUSED_HOLD);
}

#[test]
fn the_hold_stops_at_a_refresh_and_never_grows_on_a_stream_faster_than_the_display() {
    let mut skipped = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..100_000 {
        skipped.skipped(0, 8_333, 8_333);
    }
    assert_eq!(skipped.hold_us, 8_500, "a hold longer than a refresh always gets its turn");
    let mut faster = SkippedPicture::new(REFUSED_HOLD);
    for _ in 0..100_000 {
        assert_eq!(faster.skipped(0, 8_333, 16_666), None, "120 fps at 60 Hz is refused by design");
    }
}

/// The real swap chain: presents back to back never wait for a refresh, the ones the display
/// has no room for say so, and SDL's next draw still reaches the back buffer. Needs a display
/// that shows the window, which a hidden one is not.
#[cfg(windows)]
#[test]
#[ignore = "requires a Direct3D 11 display, and shows a small window"]
fn a_present_without_waiting_is_refused_while_the_last_picture_waits() {
    let _guard = crate::sdl::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: This test owns SDL and all its resources on one thread.
    unsafe {
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
        let window = SDL_CreateWindow(c"Present test".as_ptr(), 128, 64, SDL_WindowFlags(0));
        let renderer = crate::sdl::create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        assert!(SDL_SetRenderVSync(renderer, 1));
        let clear = |red: u8| {
            assert!(SDL_SetRenderDrawColor(renderer, red, 0, 0, 255));
            assert!(SDL_RenderClear(renderer));
        };
        // Reading back takes longer than a refresh, so not during the burst.
        let drawn = |red: u8| {
            clear(red);
            let surface = SDL_RenderReadPixels(renderer, std::ptr::null());
            assert!(!surface.is_null(), "{}", crate::sdl::error());
            let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
            assert!(SDL_ReadSurfacePixel(surface, 0, 0, &mut r, &mut g, &mut b, &mut a));
            SDL_DestroySurface(surface);
            assert_eq!(r, red, "the draw after a present must reach the back buffer");
        };
        let settle = |renderer| for _ in 0..5 { SDL_PumpEvents(); SDL_RenderPresent(renderer); };
        settle(renderer);
        let (mut shown, mut refused, mut longest) = (0, 0, std::time::Duration::ZERO);
        for n in 0..60u8 {
            clear(n + 1);
            let begun = std::time::Instant::now();
            match present_without_waiting(renderer, std::time::Duration::ZERO) {
                NoWait::Shown => shown += 1,
                NoWait::Refused => refused += 1,
                NoWait::Failed => panic!("the present failed"),
            }
            longest = longest.max(begun.elapsed());
            // A burst over several refreshes: some find the last picture gone, most do not.
            SDL_DelayPrecise(1_000_000);
        }
        assert!(shown >= 1, "shown {shown}, refused {refused}");
        assert!(refused >= 1, "a picture a millisecond, the display has no room for all: shown {shown}");
        assert!(longest < std::time::Duration::from_millis(4), "a present waited {longest:?}, shown {shown}, refused {refused}");
        drawn(99);
        // With a hold as long as a refresh it gets its turn.
        let before = shown;
        for _ in 0..3 {
            drawn(200);
            if present_without_waiting(renderer, std::time::Duration::from_millis(100)) == NoWait::Shown { shown += 1; }
        }
        assert_eq!(shown, before + 3, "a hold longer than a refresh always gets its turn");
        drawn(201);
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

// Real VRR waits at most 2 ms past the typical frame for a late one: on Wi-Fi the unbounded
// percentile rule held every frame for the lateness tail (ADAPTIVE_SPREAD_US).
#[test]
fn host_times_bound_the_wait_for_late_frames() {
    let pacer = Pacer::new(PacingMode::HostTimes, 6_944);
    assert_eq!(pacer.clock.settings.max_spread_us, Some(2_000));
    assert_eq!(pacer.clock.settings.min_interval_us, 6_944);
    assert_eq!(pacer.clock.settings.max_hold_us, ClockSettings::default().max_hold_us, "the backlog bound is unchanged");
}

// Regression (nightjar#80): PC's frame times held pictures by the lateness a decoder behind
// the stream made, up to the longest hold.
#[test]
fn host_times_show_a_picture_at_once_while_the_decoder_is_behind() {
    let mut pacer = Pacer::new(PacingMode::HostTimes, 0);
    let mut reference = pacer.clock.clone();
    let mut ready = 0;
    let mut accept = |pacer: &mut Pacer, queued: usize| {
        ready += 10_000;
        let rtp = (ready * 9 / 100) as u32;
        let mut picture = decoded(ready);
        picture.rtp = rtp;
        pacer.set_decode_queue(queued);
        pacer.accept(picture);
        let last = crate::stream::pipeline::lock(&pacer.pending).back().map(|p| (p.present_us, p.hold_us));
        (rtp, ready, last.unwrap())
    };
    // Review of nightjar#80: a network burst of three frames leaves the queue non-empty for
    // two of them; they are paced as usual.
    for queued in [2, 1, 0] {
        let (rtp, ready, got) = accept(&mut pacer, queued);
        let s = reference.schedule(rtp, ready);
        assert_eq!(got, (s.present_us, s.hold_us), "burst frame with {queued} queued");
    }
    // A decoder that cannot keep up keeps the queue non-empty: after a few frames each
    // picture is shown as soon as it is ready and its lateness is not learnt.
    for i in 1..=12 {
        let (rtp, ready, got) = accept(&mut pacer, 3);
        let s = if i < rubylight_protocol::present_clock::BEHIND_AFTER_FRAMES {
            reference.schedule(rtp, ready)
        } else {
            reference.schedule_behind(rtp, ready)
        };
        assert_eq!(got, (s.present_us, s.hold_us), "backlog frame {i}");
    }
    // Caught up: scheduled as before.
    let (rtp, ready, got) = accept(&mut pacer, 0);
    let s = reference.schedule(rtp, ready);
    assert_eq!(got, (s.present_us, s.hold_us));
}

#[test]
fn lowest_and_balanced_pacing_show_the_newest_due_picture() {
    for mode in [FramePacing::Latency, FramePacing::Balanced] {
        let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(mode, 60);
        for ready in [1_000, 9_000, 17_000] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(17_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 2), "{mode:?}");
    }
}

#[test]
fn balanced_without_keep_only_the_newest_frame_shows_each_picture_in_turn() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Balanced, 60).with_drop_late_frames(false);
    for ready in [1_000, 9_000, 17_000] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(17_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0), "the oldest due one, as Android's queue");
    assert!(!pacer.skips_instead_of_waiting(60.0), "a kept picture waits its turn");
    // The switch is Balanced's: the lowest latency pacing and VRR always take the newest.
    for (mode, pacing) in [(PacingMode::Vsync, FramePacing::Latency), (PacingMode::Immediate, FramePacing::Balanced)] {
        let mut pacer = Pacer::new(mode, 8_333).with_frame_pacing(pacing, 60).with_drop_late_frames(false);
        for ready in [1_000, 9_000, 17_000] {
            pacer.accept(decoded(ready));
        }
        assert_eq!(pacer.take_due(17_000).1, 2, "{mode:?} {pacing:?}");
    }
    let on = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Balanced, 60);
    assert!(on.skips_instead_of_waiting(60.0), "on, the default, keeps today's skip");
}

#[test]
fn smoothest_pacing_shows_each_picture_in_turn_and_skips_only_stale_ones() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
    for ready in [1_000, 9_000, 17_000] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(17_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0), "16 ms old: still within a frame");
    let (picture, dropped) = pacer.take_due(26_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 1), "17 ms old with a newer one due: skipped");
    assert!(pacer.next_due().is_none());
    pacer.accept(decoded(30_000));
    let (picture, dropped) = pacer.take_due(60_000);
    assert_eq!((picture.unwrap().decoded.ready_us, dropped), (30_000, 0), "a lone late picture still shows");
    let mut vrr = Pacer::new(PacingMode::Immediate, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
    vrr.accept(decoded(1_000));
    vrr.accept(decoded(2_000));
    assert_eq!(vrr.take_due(2_000).1, 1, "without V-Sync the setting does not apply");
}

#[test]
fn the_fps_limit_gives_each_picture_its_own_slot_holding_it_at_most_a_frame() {
    let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::CapFps, 60);
    // Two pictures that came 2 ms apart, then one late.
    for ready in [10_000, 12_000, 60_000] {
        pacer.accept(decoded(ready));
    }
    let (picture, dropped) = pacer.take_due(12_000);
    let picture = picture.unwrap();
    assert_eq!((picture.decoded.ready_us, picture.present_us, picture.hold_us, dropped), (10_000, 10_000, 0, 0));
    assert_eq!(pacer.next_due(), Some(26_666), "the early one waits for the next slot");
    assert!(pacer.take_due(20_000).0.is_none());
    let (picture, dropped) = pacer.take_due(26_666);
    let picture = picture.unwrap();
    assert_eq!((picture.decoded.ready_us, picture.hold_us, dropped), (12_000, 14_666, 0));
    assert_eq!(pacer.next_due(), Some(60_000), "a late picture starts the slots again");
    pacer.accept(decoded(60_500));
    pacer.accept(decoded(61_000));
    assert_eq!(pacer.take_due(100_000).1, 2, "pictures more than a frame old give way to the newest due one");
}
