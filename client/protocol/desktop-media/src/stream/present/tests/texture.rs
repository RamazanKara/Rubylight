//! Video texture, colour space, HDR/HLG, VideoOnly, plane pass, scaling, and padding tests.

use super::super::*;
use super::{decoded, frame, holding};

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
// Both Linux and macOS use the Vulkan/Metal plane pass.
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

/// Issue 120 on the Vulkan plane pass: a picture the display scales is not drawn by the pass,
/// whose taps would filter its colour codes; its planes stay for the video texture, which SDL
/// scales in linear light. The check comes before anything reads the renderer.
// Both Linux and macOS use the Vulkan/Metal plane pass.
#[cfg(unix)]
#[test]
fn a_scaled_plane_picture_goes_through_the_video_texture() {
    let mut texture = VideoTexture { width: 1280, height: 720, ..VideoTexture::default() };
    let fake = std::ptr::NonNull::<SDL_Texture>::dangling().as_ptr();
    texture.texture = fake;
    texture.planes_pending = Some([7; 8]);
    let scaled = viewport((1280, 720), (1920, 1080), crate::config::VideoScaling::Fit, false);
    assert!(!texture.draw_direct(std::ptr::null_mut(), &scaled, fake));
    assert_eq!(texture.planes_pending, Some([7; 8]), "the planes wait for the texture's draw");
    assert!(texture.direct_planes_scaled_said);
    assert!(!texture.direct_planes_failed, "scaling is no failure: an unscaled picture is drawn directly again");
    texture.texture = std::ptr::null_mut();
}

/// The Vulkan plane pass draws a Frame Warp shift in whole picture pixels, as the Windows pass
/// does: a fractional one would filter the colour codes it converts afterwards.
// Both Linux and macOS use the Vulkan/Metal plane pass.
#[cfg(unix)]
#[test]
fn the_plane_pass_shifts_a_warped_picture_in_whole_pixels() {
    use crate::stream::frame_warp::WarpDraw;
    let warp = WarpDraw { shift: [3.4 / 1280.0, -0.6 / 720.0], ..WarpDraw::default() };
    let drawn = direct_draw::unix_direct_warp(&warp, (1280, 720));
    assert!((drawn.shift[0] - 3.0 / 1280.0).abs() < 1e-7 && (drawn.shift[1] + 1.0 / 720.0).abs() < 1e-7, "{:?}", drawn.shift);
    assert_eq!(drawn.lens, warp.lens);
    assert!(direct_draw::unix_direct_warp(&WarpDraw::default(), (1280, 720)).is_identity());
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
    let display = crate::stream::hlg::hlg_display(4.0, 200.0);
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
// Both Linux and macOS use the Vulkan/Metal plane pass.
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

/// Hardware decoders (VAAPI, NVDEC, V4L2, VideoToolbox) hand SDL NV12, P010, DMA-BUF, or
/// VideoToolbox pictures, and SDL reads an HLG P010 picture as PQ. When the SDL `"gpu"` renderer
/// is active, an HLG picture routes to the plane shader with the HLG flag; on non-GPU renderers
/// (`"metal"`, `"opengl"`) it falls back safely instead of failing inside `PlanesPass`. SDR and
/// PQ pictures always keep SDL's zero-copy path.
#[test]
fn an_hlg_hardware_picture_routes_through_planes_only_on_gpu_renderer() {
    let info = |kind, color_trc| FrameInfo { kind, width: 64, height: 32, ten_bit: i32::from(kind == media::FRAME_P010), color_trc, ..frame(1, 9, 1) };
    for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_DMABUF, media::FRAME_VIDEOTOOLBOX] {
        assert!(hlg_through_planes(&info(kind, TRC_HLG), true), "kind {kind} on gpu renderer");
        assert!(!hlg_through_planes(&info(kind, TRC_HLG), false), "kind {kind} must fall back safely on metal/opengl");
        assert!(!hlg_through_planes(&info(kind, TRC_PQ), true), "PQ kind {kind} stays SDL's");
        assert!(!hlg_through_planes(&info(kind, 1), true), "SDR kind {kind} stays SDL's");
    }
    // Planar pictures already take the plane shader; the others have no NV12 planes.
    for kind in [media::FRAME_YUV420P, media::FRAME_YUV420P10, media::FRAME_WAVELET] {
        assert!(!hlg_through_planes(&info(kind, TRC_HLG), true), "kind {kind}");
    }
    // A P010 picture reads as 10-bit 4:2:0 with its samples moved to the low bits (64), and
    // shown as HDR it gets the HLG flag (128), never PQ.
    let p010 = info(media::FRAME_P010, TRC_HLG);
    let layout = media::PlanarLayout::of_semi_planar(&p010).unwrap();
    assert_eq!(layout.plane_size(1), (32, 16));
    assert_eq!(planar_parameters(&p010, layout, true, 2.5, 4.0)[6], 2 | 8 | 16 | 64 | 128);
    let nv12 = info(media::FRAME_NV12, TRC_HLG);
    assert_eq!(planar_parameters(&nv12, media::PlanarLayout::of_semi_planar(&nv12).unwrap(), true, 2.5, 4.0)[6], 2 | 8 | 128);
    assert!(media::PlanarLayout::of_semi_planar(&info(media::FRAME_DMABUF, TRC_HLG)).is_none(), "VAAPI surface is mapped first");
    assert!(media::PlanarLayout::of_semi_planar(&info(media::FRAME_VIDEOTOOLBOX, TRC_HLG)).is_none(), "VideoToolbox surface is mapped first");
}

#[test]
fn vendored_sdl3_metal_patch_exposes_command_queue_property_and_is_wired_for_macos() {
    let patch = include_str!("../../../../../vendor/sdl3-src/patches/metal-command-queue.patch");
    assert!(patch.contains("src/render/metal/SDL_render_metal.m"));
    assert!(patch.contains("\"SDL.renderer.metal.command_queue\""));
    assert!(patch.contains("data.mtlcmdqueue"));
    let deps = include_str!("../../../../macos-deps.sh");
    assert!(deps.contains("metal-command-queue.patch"), "macos-deps.sh must apply the Metal queue patch");
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

// Issue 120 on every platform's plane pass (the Vulkan `planesDirect` too): only a picture the
// display shows at its own size skips the video texture.
#[test]
fn only_a_picture_at_the_displays_own_size_skips_the_video_texture() {
    let fit = |picture, area| viewport(picture, area, crate::config::VideoScaling::Fit, false);
    assert!(draws_one_to_one(&fit((1920, 1080), (1920, 1080)), (1920, 1080)));
    assert!(draws_one_to_one(&fit((1920, 1080), (1920, 1200)), (1920, 1080)), "letterboxed at its own size");
    assert!(!draws_one_to_one(&fit((1280, 720), (1920, 1080)), (1280, 720)), "upscaled");
    assert!(!draws_one_to_one(&fit((2560, 1440), (1920, 1080)), (2560, 1440)), "downscaled");
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

#[cfg(windows)]
#[test]
fn an_hdr_stream_is_pq_without_a_pq_transfer_in_the_bitstream() {
    use crate::stream::direct::Transfer::{Pq, Sdr};
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
    use crate::stream::direct::Transfer::{Hlg, Pq, Sdr};
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
    use crate::stream::direct::Transfer::{Pq, Sdr};
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
fn failed_upload_preserves_visible_crop_while_texture_survives() {
    // T7-7: re-running or failing upload while the prior texture stays alive must not wipe crop.
    assert_eq!(
        visible_after_upload(Some((1920, 1080)), Some((1920, 1080)), false, true, (1920, 1088), true),
        Some((1920, 1080))
    );
    // Without a live texture, crop clears to None.
    assert_eq!(
        visible_after_upload(Some((1920, 1080)), Some((1920, 1080)), false, true, (1920, 1088), false),
        None
    );
    // Successful plain upload updates visible crop normally.
    assert_eq!(
        visible_after_upload(None, Some((1920, 1080)), true, true, (1920, 1088), true),
        Some((1920, 1080))
    );
    assert_eq!(
        visible_after_upload(Some((1920, 1080)), Some((1920, 1088)), true, true, (1920, 1088), true),
        None
    );
}

#[test]
fn planes_mode_mismatch_detects_vulkan_and_semi_planar_hlg_transitions() {
    // T7-18: FRAME_VULKAN and semi-planar HLG share identical shader parameters, so mode
    // transitions must force recreating PlanesPass.
    assert!(planes_mode_mismatch(Some(PlanesMode::Vulkan), PlanesMode::SemiPlanar));
    assert!(planes_mode_mismatch(Some(PlanesMode::SemiPlanar), PlanesMode::Vulkan));
    assert!(planes_mode_mismatch(Some(PlanesMode::PyroWave), PlanesMode::Planar));
    assert!(!planes_mode_mismatch(Some(PlanesMode::Vulkan), PlanesMode::Vulkan));
    assert!(!planes_mode_mismatch(None, PlanesMode::SemiPlanar));
}

#[test]
fn invalidated_planes_are_not_restored_after_failed_upload() {
    // T7-3: when a failed upload invalidated previously sampled/held planes (`held == false`),
    // `planes_after_upload` must drop `planes_pending` rather than restoring stale parameters.
    let params = [1920, 1080, 1920, 1080, 0, 3, 256, 1.0f32.to_bits()];
    assert_eq!(planes_after_upload(false, None, Some(params), false, Some(params)), None);
    assert_eq!(planes_after_upload(false, None, Some(params), true, Some(params)), Some(params));
}
