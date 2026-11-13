use super::*;
use crate::media::{FrameInfo, PlanarLayout, FRAME_NV12, FRAME_P010};

const SDR: SdlLook = SdlLook { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: true, pq_target: false, target_encoding: [0.0; 4] };

/// planesDirect after the conversion, in the shader's order, for checking its numbers.
fn shader_output(rgb: [f32; 3], uniforms: &Uniforms) -> [f32; 3] {
    let mul = |m: [[f32; 3]; 3], v: [f32; 3]| m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);
    let to2020 = [[0.627404, 0.329283, 0.043313], [0.069097, 0.919541, 0.011362], [0.016391, 0.088013, 0.895595]];
    let to709 = [[1.660496, -0.587656, -0.072840], [-0.124547, 1.132895, -0.008348], [-0.018154, -0.100597, 1.118751]];
    let [tonemap1, tonemap2] = uniforms.tonemap;
    let mut rgb = rgb;
    if tonemap2 > 0.0 {
        let mut v = mul(to2020, rgb);
        let vmax = v[0].max(v[1]).max(v[2]);
        if vmax > 0.0 { v = v.map(|c| c * (1.0 + tonemap1 * vmax) / (1.0 + tonemap2 * vmax)); }
        rgb = mul(to709, v);
    }
    rgb = rgb.map(|c| c * uniforms.output_scale);
    if uniforms.parameters[6] & GAMMA22_TARGET_FLAG != 0 {
        return gamma22_of_scrgb(rgb, uniforms.encoding);
    }
    if uniforms.parameters[6] & PQ_TARGET_FLAG != 0 {
        return pq_of_scrgb(rgb);
    }
    if uniforms.parameters[6] & SRGB_TARGET_FLAG != 0 {
        rgb = rgb.map(|c| if c <= 0.0031308 { c * 12.92 } else { c.abs().powf(1.0 / 2.4) * 1.055 - 0.055 }.clamp(0.0, 1.0));
    }
    rgb
}

/// The bug: on an sRGB back buffer (an SDR display) the direct draw wrote linear light, which
/// the display reads as sRGB, so the picture came out too dark. SDL encodes and clamps.
#[test]
fn an_srgb_target_gets_the_picture_srgb_encoded() {
    let rect = SDL_FRect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 };
    let srgb = SdlLook { linear_output: false, ..SDR };
    let encoded = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), srgb);
    assert_eq!(encoded.parameters[6], DIRECT_FLAG | SRGB_TARGET_FLAG);
    let mid = shader_output([0.18; 3], &encoded);
    assert!((mid[0] - 0.4613).abs() < 0.001, "18 % grey is sRGB 0.46, not {}", mid[0]);
    assert_eq!(shader_output([0.001; 3], &encoded)[0], 0.001 * 12.92, "the linear toe");
    assert_eq!(shader_output([2.0, -0.5, 4.0], &encoded), [1.0, 0.0, 1.0], "clamped, as SDL saturates");
    let linear = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SDR);
    assert_eq!(linear.parameters[6], DIRECT_FLAG, "an scRGB target takes linear light");
    assert_eq!(shader_output([0.18; 3], &linear), [0.18; 3]);
    // HLG tone mapped onto an SDR display: tone map, then the encoding.
    let hlg = SdlLook { texture_headroom: 4.0, ..srgb };
    let mapped = shader_output([1.0; 3], &direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), hlg));
    // (1 + 1/16) / (1 + 1) of white, sRGB encoded.
    assert!(mapped.iter().all(|c| (c - 0.7556).abs() < 0.002), "white is tone mapped below the clip, then encoded: {mapped:?}");
}

/// BT.709 to BT.2020 primaries, as the present shaders write it, digit for digit.
#[allow(clippy::excessive_precision)]
const TO_2020: [[f32; 3]; 3] = [[0.6274040, 0.3292820, 0.0433136], [0.0690970, 0.9195400, 0.0113612], [0.0163916, 0.0880132, 0.8955950]];

/// The PQ present pass's encode of scRGB (`pq_present.hlsl`), written apart from the shader's.
fn pq_of_scrgb(rgb: [f32; 3]) -> [f32; 3] {
    let to2020 = TO_2020;
    let pq = |nits: f32| {
        // SMPTE ST 2084's constants, as it defines them.
        let (m1, m2) = (2610.0f32 / 16384.0, 2523.0f32 / 4096.0 * 128.0);
        let (c1, c2, c3) = (3424.0f32 / 4096.0, 2413.0f32 / 4096.0 * 32.0, 2392.0f32 / 4096.0 * 32.0);
        let y = (nits / 10000.0).clamp(0.0, 1.0).powf(m1);
        ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
    };
    to2020.map(|row| pq((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]).max(0.0) * 80.0))
}

/// The scanout present's gamma 2.2 encode of scRGB (`scanout_present.hlsl`) before its dither,
/// written apart from the shader's: BT.2020 nits over the range `[min, 1 / (peak - min)]`.
fn gamma22_of_scrgb(rgb: [f32; 3], encoding: [f32; 4]) -> [f32; 3] {
    let to2020 = TO_2020;
    let [min_nits, inv_range, _, _] = encoding;
    let peak = min_nits + 1.0 / inv_range;
    to2020.map(|row| crate::stream::wl_scanout::Encoding::gamma22_code((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]) * 80.0, min_nits, peak))
}

/// A video-only frame on a swapchain the present draws in the compositor's gamma 2.2 blending
/// space (KWin 6.5 to 6.8): the draw encodes as that present would, never PQ.
#[test]
fn a_gamma22_target_gets_the_scrgb_picture_in_the_blending_space() {
    let rect = SDL_FRect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 };
    // KWin's blending space on a 700-nit output with reference white 250 (3.125 in scRGB).
    let encoding = crate::stream::wl_scanout::Encoding::constants(1, 0.005, 700.0);
    let look = SdlLook { texture_headroom: 0.0, output_headroom: 2.8, sdr_white: 3.125, color_scale: 1.0, linear_output: true, pq_target: true, target_encoding: encoding };
    let uniforms = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), look);
    assert_eq!(uniforms.parameters[6], DIRECT_FLAG | PQ_TARGET_FLAG | GAMMA22_TARGET_FLAG);
    assert_eq!(uniforms.encoding, [0.005, encoding[2], 0.0, 0.0], "the range, as the present gets it");
    let white = shader_output([1.0; 3], &uniforms);
    let expected = (249.995f32 / 699.995).powf(1.0 / 2.2);
    assert!(white.iter().all(|c| (c - expected).abs() < 0.0005), "reference white at the output's reference: {white:?}");
    assert_eq!(shader_output([0.0; 3], &uniforms), [0.0; 3], "black is the range's minimum");
    assert_eq!(shader_output([10.0; 3], &uniforms), [1.0; 3], "above the peak clips");
    // The PQ encoding, an empty range and a draw that is not video only stay as they were.
    let pq = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { target_encoding: [0.0; 4], ..look });
    assert_eq!((pq.parameters[6], pq.encoding), (DIRECT_FLAG | PQ_TARGET_FLAG, [0.0; 4]));
    let empty = SdlLook { target_encoding: crate::stream::wl_scanout::Encoding::constants(1, 700.0, 700.0), ..look };
    assert_eq!(direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), empty).parameters[6], DIRECT_FLAG | PQ_TARGET_FLAG);
    let scrgb = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { pq_target: false, ..look });
    assert_eq!((scrgb.parameters[6], scrgb.encoding), (DIRECT_FLAG, [0.0; 4]), "the back buffer is scRGB; the present encodes it");
}

/// A video-only frame on a PQ output: the draw keeps every step of the scRGB one (scale, tone
/// map) and finishes with the PQ pass's encode, and never takes the sRGB encoding.
#[test]
fn a_pq_target_gets_the_scrgb_picture_pq_encoded() {
    let rect = SDL_FRect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 };
    // An HDR output: 203-nit SDR white (2.5375 in scRGB), 1000-nit peak.
    let hdr = SdlLook { texture_headroom: 0.0, output_headroom: 12.5, sdr_white: 2.5375, color_scale: 1.0, linear_output: true, pq_target: true, target_encoding: [0.0; 4] };
    let uniforms = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), hdr);
    assert_eq!(uniforms.parameters[6], DIRECT_FLAG | PQ_TARGET_FLAG);
    assert_eq!(uniforms.output_scale, 2.5375, "the scale is the scRGB draw's");
    let white = shader_output([1.0; 3], &uniforms);
    // 203 nits is ST 2084 code 0.5807 (BT.2408's reference white).
    assert!(white.iter().all(|c| (c - 0.5807).abs() < 0.0005), "SDR white: {white:?}");
    assert_eq!(shader_output([0.0; 3], &uniforms), [pq_of_scrgb([0.0; 3])[0]; 3], "black is PQ's black, not 0 after a clamp of its own");
    let red = shader_output([1.0, 0.0, 0.0], &uniforms);
    assert!(red[0] > red[1] && red[1] > 0.0 && red[2] > 0.0, "BT.709 red lies inside BT.2020: {red:?}");
    // Without a PQ target, an sRGB output still takes the sRGB flag.
    let srgb = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { linear_output: false, pq_target: false, ..hdr });
    assert_eq!(srgb.parameters[6], DIRECT_FLAG | SRGB_TARGET_FLAG);
    assert_eq!(direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { pq_target: false, ..hdr }).parameters[6], DIRECT_FLAG);
}

/// The constants are the shader's 128-byte `Picture` block, in its order.
#[test]
fn the_direct_constants_match_the_shader_block() {
    assert_eq!(std::mem::size_of::<Uniforms>(), 128);
    assert_eq!(std::mem::offset_of!(Uniforms, destination), 32);
    assert_eq!(std::mem::offset_of!(Uniforms, picture), 48);
    assert_eq!(std::mem::offset_of!(Uniforms, output_scale), 64);
    assert_eq!(std::mem::offset_of!(Uniforms, tonemap), 68);
    assert_eq!(std::mem::offset_of!(Uniforms, encoding), 80);
    assert_eq!(std::mem::offset_of!(Uniforms, warp), 96);
    assert_eq!(std::mem::offset_of!(Uniforms, warp_lens), 112);
    let shader = include_str!("../../pyrowave.hlsl");
    let rows = shader.lines().map(str::trim).collect::<Vec<_>>();
    let at = |row: &str| rows.iter().position(|line| *line == row).unwrap_or_else(|| panic!("no {row}"));
    assert_eq!(at("float minNits; float invRange; float2 unusedEncoding;"), at("float outputScale; float tonemap1; float tonemap2; float unused;") + 2,
        "the encoding's row follows the row at 64 (after its comment)");
    assert_eq!(at("float4 warp;"), at("float minNits; float invRange; float2 unusedEncoding;") + 4,
        "Frame Warp's row follows the encoding's (after its comment)");
    assert_eq!(at("float4 warpLens;"), at("float4 warp;") + 1, "and its lens row ends the block");
    assert_eq!(rows[at("float4 warpLens;") + 1], "#endif");
    assert_eq!(MASK_ROW_BYTES, 256, "a 2048-cell row of the mask in bytes");
    assert_eq!(direct_uniforms(&[0; 8], &SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 8.0 }, (0, 0), (8, 8),
        SdlLook { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: false, pq_target: false, target_encoding: [0.0; 4] }).warp,
        [0.0; 4], "no shift unless Frame Warp sets one");
}

/// The draw into the back buffer flags itself, places the picture in the viewport and keeps
/// the picture texture's parameters otherwise, so it converts exactly as the texture would.
#[test]
fn a_direct_draw_lands_where_sdl_would_draw_the_texture() {
    let parameters = [1920, 1080, 1920, 1080, 0, 3, 0b1_0101, 2.5f32.to_bits()];
    let destination = SDL_FRect { x: 240.0, y: 0.0, w: 1440.0, h: 810.0 };
    let uniforms = direct_uniforms(&parameters, &destination, (0, 135), (1920, 1080), SDR);
    assert_eq!(uniforms.parameters[6], 0b1_0101 | DIRECT_FLAG);
    assert_eq!(uniforms.parameters[..6], parameters[..6]);
    assert_eq!(uniforms.parameters[7], parameters[7]);
    assert_eq!(uniforms.destination, [240.0, 135.0, 1440.0, 810.0]);
    assert_eq!(uniforms.picture, [1920.0, 1080.0, 0.0, 0.0]);
    assert_eq!(uniforms.output_scale, 1.0);
    assert_eq!(uniforms.tonemap, [0.0, 0.0], "an SDR picture on an SDR output is not tone mapped");
}

/// SDL scales a linear texture by the output's SDR white and its colour scale, and tone maps
/// it (TONEMAP_CHROME) only when the texture carries more headroom than an HDR output has.
#[test]
fn a_direct_draw_scales_and_tone_maps_as_sdl_does() {
    let hdr = SdlLook { texture_headroom: 12.0, output_headroom: 4.0, sdr_white: 2.5, color_scale: 0.5, linear_output: true, pq_target: false, target_encoding: [0.0; 4] };
    let uniforms = direct_uniforms(&[0; 8], &SDL_FRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, (0, 0), (2, 2), hdr);
    assert_eq!(uniforms.output_scale, 1.25);
    assert_eq!(uniforms.tonemap, [4.0 / 144.0, 0.25]);
    assert_eq!(uniforms.destination[2..], [1.0, 1.0], "an empty rectangle must not divide by zero in the shader");
    assert_eq!(SdlLook { output_headroom: 12.0, ..hdr }.tonemap(), None, "equal headroom needs no tone map");
    assert_eq!(SdlLook { output_headroom: 0.0, ..hdr }.tonemap(), None, "SDL skips the tone map without an output headroom");
    assert_eq!(SDR.tonemap(), None);
}

fn picture(kind: i32, luma: &mut [u8], chroma: &mut [u8], linesize: i32) -> FrameInfo {
    FrameInfo {
        width: 3, height: 2, kind, ten_bit: i32::from(kind == FRAME_P010), pts: 0,
        data: [luma.as_mut_ptr(), chroma.as_mut_ptr(), ptr::null_mut()],
        linesize: [linesize; 3], color_space: 9, color_range: 1, color_trc: 18,
    }
}

/// An HLG P010 picture from a hardware decoder reaches the plane shader as the 10-bit planar
/// picture it reads (SDL's P010 texture read it as PQ): Cb and Cr in planes of their own,
/// every sample in the low 10 bits. Row padding is skipped.
#[test]
fn a_p010_picture_is_uploaded_as_ten_bit_planes() {
    // 3x2 luma, 2x1 chroma pairs; rows of 4 words (8 bytes), the last word padding.
    let words = |w: [u16; 8]| w.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
    let mut luma = words([64 << 6, 512 << 6, 940 << 6, 0xffff, 1 << 6, 2 << 6, 1023 << 6, 0xffff]);
    let mut chroma = words([100 << 6, 900 << 6, 512 << 6, 513 << 6, 0xffff, 0xffff, 0xffff, 0xffff]);
    let info = picture(FRAME_P010, &mut luma, &mut chroma, 8);
    let layout = PlanarLayout::of_semi_planar(&info).unwrap();
    assert_eq!(layout, PlanarLayout { width: 3, height: 2, chroma444: false, ten_bit: true });
    let sizes = [0, 1, 2].map(|plane| layout.plane_size(plane));
    let mut buffer = vec![0xaau8; (6 + 2 + 2) * 2];
    // SAFETY: the planes above hold `sizes` samples; the buffer holds the three planes.
    unsafe { fill_planes(&info, sizes, 2, buffer.as_mut_ptr(), [0, 12, 16]) };
    let samples: Vec<u16> = buffer.as_chunks::<2>().0.iter().map(|s| u16::from_le_bytes(*s)).collect();
    assert_eq!(samples, [64, 512, 940, 1, 2, 1023, 100, 512, 900, 513]);
}

/// NVDEC's 10-bit 4:4:4 in memory (YUV444P16) holds its codes in the high bits, as P010 does:
/// the upload moves them to the low bits the shader reads (flag 64), so a code comes out as
/// itself, not 64 times too bright. FFmpeg's own 10-bit planes go as they are.
#[test]
fn high_bit_planes_are_uploaded_with_their_codes_in_the_low_bits() {
    use crate::media::{FRAME_YUV444P10, FRAME_YUV444P16};
    let words = |w: [u16; 4]| w.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
    // 3x1 planes in rows of 4 words, the last padding.
    let mut planes = [words([64 << 6, 940 << 6, 1023 << 6, 0xffff]), words([512 << 6, 1 << 6, 0, 0xffff]), words([960 << 6, 513 << 6, 2 << 6, 0xffff])];
    let mut info = FrameInfo {
        width: 3, height: 1, kind: FRAME_YUV444P16, ten_bit: 1, pts: 0,
        data: [planes[0].as_mut_ptr(), planes[1].as_mut_ptr(), planes[2].as_mut_ptr()],
        linesize: [8; 3], color_space: 1, color_range: 1, color_trc: 1,
    };
    let layout = PlanarLayout::of(&info).unwrap();
    assert_eq!(layout, PlanarLayout { width: 3, height: 1, chroma444: true, ten_bit: true });
    assert!(high_bit_planes(&info));
    let sizes = [0, 1, 2].map(|plane| layout.plane_size(plane));
    let fill = |info: &FrameInfo| {
        let mut buffer = vec![0xaau8; 9 * 2];
        // SAFETY: the planes above hold `sizes` samples; the buffer holds the three planes.
        unsafe { fill_planes(info, sizes, 2, buffer.as_mut_ptr(), [0, 6, 12]) };
        buffer.as_chunks::<2>().0.iter().map(|s| u16::from_le_bytes(*s)).collect::<Vec<u16>>()
    };
    assert_eq!(fill(&info), [64, 940, 1023, 512, 1, 0, 960, 513, 2]);
    // FFmpeg's software 10-bit 4:4:4 is already in the low bits: copied as it is.
    info.kind = FRAME_YUV444P10;
    assert!(!high_bit_planes(&info));
    assert_eq!(fill(&info), [64 << 6, 940 << 6, 1023 << 6, 512 << 6, 1 << 6, 0, 960 << 6, 513 << 6, 2 << 6]);
}

#[test]
fn an_nv12_picture_is_uploaded_as_eight_bit_planes() {
    let mut luma = vec![16, 128, 235, 0xff, 17, 18, 19, 0xff];
    let mut chroma = vec![60, 200, 128, 129, 0xff, 0xff, 0xff, 0xff];
    let info = picture(FRAME_NV12, &mut luma, &mut chroma, 4);
    let layout = PlanarLayout::of_semi_planar(&info).unwrap();
    assert!(!layout.ten_bit && !layout.chroma444);
    let mut buffer = vec![0xaau8; 6 + 2 + 2];
    // SAFETY: as above.
    unsafe { fill_planes(&info, [0, 1, 2].map(|plane| layout.plane_size(plane)), 1, buffer.as_mut_ptr(), [0, 6, 8]) };
    assert_eq!(buffer, [16, 128, 235, 17, 18, 19, 60, 128, 200, 129]);
}

/// The HLG picture on the GPU (a Vulkan device and SDL's offscreen video driver, so ignored:
/// `cargo test -p nightjar-desktop -- --ignored stream::planes_gpu`): HLG's reference white
/// (75 %) in P010 and NV12 comes out at BT.2408's 203 nits on a 1000-nit display, and at SDR
/// white on an SDR one. Read as PQ, the same code would be 983 nits.
#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn hlg_reference_white_is_drawn_at_203_nits_from_p010_and_nv12() {
    let (width, height) = (16usize, 8usize);
    let p010_luma = (0..width * height).flat_map(|_| (721u16 << 6).to_le_bytes()).collect::<Vec<u8>>();
    let p010_chroma = (0..width * height / 2).flat_map(|_| (512u16 << 6).to_le_bytes()).collect::<Vec<u8>>();
    let nv12_luma = vec![180u8; width * height];
    let nv12_chroma = vec![128u8; width * height / 2];
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(c"HLG planes".as_ptr(), 64, 64, SDL_WINDOW_HIDDEN);
        let renderer = crate::sdl::create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        assert_eq!(crate::sdl::renderer_name(renderer), "gpu");
        let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, width as i32, height as i32);
        assert!(!target.is_null(), "{}", crate::sdl::error());
        // A 1000-nit HDR output (headroom 12.5 over 80 nits), where 1.0 is 80 nits, and an SDR
        // one, drawn for the reference display with its 203-nit white at 1.0.
        let outputs = [(12.5, 80.0), (1.0, 203.0)];
        for ((kind, luma, chroma, bytes), (headroom, unit)) in [(FRAME_P010, &p010_luma, &p010_chroma, 2), (FRAME_NV12, &nv12_luma, &nv12_chroma, 1)]
            .into_iter().flat_map(|picture| outputs.map(|output| (picture, output)))
        {
            let (mut luma, mut chroma) = (luma.clone(), chroma.clone());
            let mut info = picture(kind, &mut luma, &mut chroma, (width * bytes) as i32);
            (info.width, info.height) = (width as i32, height as i32);
            assert!(super::super::present::hlg_through_planes(&info));
            let layout = PlanarLayout::of_semi_planar(&info).unwrap();
            let parameters = super::super::present::planar_parameters(&info, layout, true, 1.0, headroom);
            let mut pass = PlanesPass::for_frames(renderer, layout).unwrap();
            pass.draw_frame(renderer, target, &info, &parameters).unwrap();
            assert!(SDL_SetRenderTarget(renderer, target), "{}", crate::sdl::error());
            let readback = SDL_RenderReadPixels(renderer, ptr::null());
            assert!(!readback.is_null(), "{}", crate::sdl::error());
            let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
            SDL_DestroySurface(readback);
            SDL_SetRenderTarget(renderer, ptr::null_mut());
            let rgb = std::slice::from_raw_parts((*pixels).pixels.cast::<f32>(), 3).to_vec();
            SDL_DestroySurface(pixels);
            let nits = rgb.iter().map(|value| value * unit).collect::<Vec<f32>>();
            eprintln!("kind {kind}, headroom {headroom}: {nits:?} nits");
            for value in nits {
                assert!((value - 203.0).abs() < 203.0 * 0.03, "kind {kind}: {value} nits, not HLG's reference white");
            }
        }
        SDL_DestroyTexture(target);
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}
/// The direct draw gives what the texture path gives (the plane pass into the RGBA64F picture
/// texture, then SDL's draw of it), on an sRGB target, the SDR back buffer's format, and on an
/// scRGB one, with and without SDL's tone map: `cargo test -p nightjar-desktop -- --ignored
/// stream::planes_gpu`.
#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn the_direct_draw_matches_the_texture_path() {
    let (width, height) = (16usize, 8usize);
    // A ramp in luma and both chroma planes, so matrix, range and sampling all show.
    let mut luma = (0..width * height).map(|i| (16 + (i * 219) / (width * height)) as u8).collect::<Vec<u8>>();
    let mut chroma = (0..width * height / 2).map(|i| if i % 2 == 0 { (64 + i * 2) as u8 } else { (192 - i * 2) as u8 }).collect::<Vec<u8>>();
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(c"direct planes".as_ptr(), 64, 64, SDL_WINDOW_HIDDEN);
        let renderer = crate::sdl::create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        assert_eq!(crate::sdl::renderer_name(renderer), "gpu");
        let mut info = picture(FRAME_NV12, &mut luma, &mut chroma, width as i32);
        (info.width, info.height) = (width as i32, height as i32);
        let layout = PlanarLayout::of_semi_planar(&info).unwrap();
        let parameters = super::super::present::planar_parameters(&info, layout, false, 1.0, 1.0);
        let texture = |format: SDL_PixelFormat, space: SDL_Colorspace, headroom: Option<f32>| {
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_TARGET.0));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, width as i64);
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, height as i64);
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
            if let Some(headroom) = headroom { SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT, headroom); }
            let texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            assert!(!texture.is_null(), "{}", crate::sdl::error());
            texture
        };
        let read = |target: *mut SDL_Texture| {
            assert!(SDL_SetRenderTarget(renderer, target), "{}", crate::sdl::error());
            let readback = SDL_RenderReadPixels(renderer, ptr::null());
            assert!(!readback.is_null(), "{}", crate::sdl::error());
            let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
            SDL_DestroySurface(readback);
            let values = std::slice::from_raw_parts((*pixels).pixels.cast::<f32>(), width * height * 4).to_vec();
            SDL_DestroySurface(pixels);
            values
        };
        let dst = SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
        let targets = [("sRGB", SDL_PIXELFORMAT_RGBA32, SDL_COLORSPACE_SRGB), ("scRGB", SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR)];
        for ((name, format, space), headroom) in targets.into_iter().flat_map(|target| [None, Some(4.0)].map(|headroom| (target, headroom))) {
            let pictured = texture(SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR, headroom);
            let (through_texture, direct) = (texture(format, space, None), texture(format, space, None));
            let mut pass = PlanesPass::for_frames(renderer, layout).unwrap();
            pass.draw_frame(renderer, pictured, &info, &parameters).unwrap();
            assert!(SDL_SetRenderTarget(renderer, through_texture), "{}", crate::sdl::error());
            assert!(SDL_RenderTexture(renderer, pictured, ptr::null(), &dst), "{}", crate::sdl::error());
            let expected = read(through_texture);
            pass.draw_frame(renderer, ptr::null_mut(), &info, &parameters).unwrap();
            assert!(SDL_SetRenderTarget(renderer, direct), "{}", crate::sdl::error());
            let look = SdlLook::of(renderer, pictured);
            assert_eq!(look.linear_output, space == SDL_COLORSPACE_SRGB_LINEAR);
            pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), look).unwrap();
            let drawn = read(direct);
            SDL_SetRenderTarget(renderer, ptr::null_mut());
            let worst = expected.iter().zip(&drawn).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            eprintln!("{name}, texture headroom {headroom:?} ({look:?}): largest difference {worst}");
            assert!(worst < 0.01, "{name}, headroom {headroom:?}: the direct draw differs from SDL's by {worst}");
            drop(pass);
            for texture in [pictured, through_texture, direct] { SDL_DestroyTexture(texture); }
        }
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

/// A video-only frame's draw (`PQ_TARGET_FLAG`) into an RGB10A2 target, the HDR10 swapchain
/// image's format, is the PQ pass's encode of the scRGB direct draw, within a 10-bit code and a
/// half: `cargo test -p nightjar-desktop -- --ignored stream::planes_gpu`.
#[test]
#[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
fn the_pq_draw_is_the_pq_pass_of_the_scrgb_draw() {
    let (width, height) = (16usize, 8usize);
    let mut luma = (0..width * height).map(|i| (16 + (i * 219) / (width * height)) as u8).collect::<Vec<u8>>();
    let mut chroma = (0..width * height / 2).map(|i| if i % 2 == 0 { (64 + i * 2) as u8 } else { (192 - i * 2) as u8 }).collect::<Vec<u8>>();
    // SAFETY: Plain SDL calls on objects made here, on one thread.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(c"PQ planes".as_ptr(), 64, 64, SDL_WINDOW_HIDDEN);
        let renderer = crate::sdl::create_renderer(window, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        assert_eq!(crate::sdl::renderer_name(renderer), "gpu");
        let mut info = picture(FRAME_NV12, &mut luma, &mut chroma, width as i32);
        (info.width, info.height) = (width as i32, height as i32);
        let layout = PlanarLayout::of_semi_planar(&info).unwrap();
        let parameters = super::super::present::planar_parameters(&info, layout, false, 1.0, 1.0);
        let texture = |format: SDL_PixelFormat, space: SDL_Colorspace| {
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_TARGET.0));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, width as i64);
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, height as i64);
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
            let texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            assert!(!texture.is_null(), "{}", crate::sdl::error());
            texture
        };
        let scrgb = texture(SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR);
        let pq = texture(SDL_PIXELFORMAT_ABGR2101010, SDL_COLORSPACE_HDR10);
        let dst = SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
        // An HDR output's look: 203-nit SDR white over 1.0 = 80 nits, so the ramp spans most codes.
        let look = SdlLook { texture_headroom: 0.0, output_headroom: 12.5, sdr_white: 2.5375, color_scale: 1.0, linear_output: true, pq_target: false, target_encoding: [0.0; 4] };
        let mut pass = PlanesPass::for_frames(renderer, layout).unwrap();
        pass.draw_frame(renderer, ptr::null_mut(), &info, &parameters).unwrap();
        assert!(SDL_SetRenderTarget(renderer, scrgb), "{}", crate::sdl::error());
        pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), look).unwrap();
        let readback = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!readback.is_null(), "{}", crate::sdl::error());
        // RGBA64F to floats, both scRGB.
        let floats = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
        SDL_DestroySurface(readback);
        assert!(!floats.is_null(), "{}", crate::sdl::error());
        let linear = std::slice::from_raw_parts((*floats).pixels.cast::<f32>(), width * height * 4).to_vec();
        SDL_DestroySurface(floats);
        assert!(SDL_SetRenderTarget(renderer, pq), "{}", crate::sdl::error());
        pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), SdlLook { pq_target: true, ..look }).unwrap();
        let readback = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!readback.is_null(), "{}", crate::sdl::error());
        assert_eq!((*readback).format.0, SDL_PIXELFORMAT_ABGR2101010.0, "read back as drawn, without a conversion");
        let pitch = (*readback).pitch as usize;
        // ABGR2101010 is one 32-bit word: red in its low 10 bits, then green, then blue.
        let codes = (0..height).flat_map(|y| (0..width).map(move |x| (y, x)))
            .map(|(y, x)| {
                let word = (*readback).pixels.cast::<u8>().add(y * pitch + x * 4).cast::<u32>().read_unaligned();
                [word & 0x3ff, (word >> 10) & 0x3ff, (word >> 20) & 0x3ff]
            })
            .collect::<Vec<[u32; 3]>>();
        SDL_DestroySurface(readback);
        SDL_SetRenderTarget(renderer, ptr::null_mut());
        let mut worst = 0.0f32;
        for (pixel, drawn) in linear.as_chunks::<4>().0.iter().zip(&codes) {
            let expected = pq_of_scrgb([pixel[0], pixel[1], pixel[2]]);
            for (want, got) in expected.iter().zip(drawn) {
                worst = worst.max((want * 1023.0 - *got as f32).abs());
            }
        }
        eprintln!("PQ draw against the PQ pass of the scRGB draw: largest difference {worst} codes");
        assert!(worst <= 1.5, "the PQ draw differs from the PQ pass of the scRGB draw by {worst} 10-bit codes");
        assert!(codes.iter().flatten().any(|&code| code > 500), "the ramp reaches up to about 203-nit white's code 594");
        // The same draw in the compositor's gamma 2.2 blending space (flag 8192), 0.005 to 700
        // nits: the scanout present's encode, within its half code of dither and a code.
        let encoding = crate::stream::wl_scanout::Encoding::constants(1, 0.005, 700.0);
        assert!(SDL_SetRenderTarget(renderer, pq), "{}", crate::sdl::error());
        pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), SdlLook { pq_target: true, target_encoding: encoding, ..look }).unwrap();
        let readback = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!readback.is_null(), "{}", crate::sdl::error());
        let pitch = (*readback).pitch as usize;
        let codes = (0..height).flat_map(|y| (0..width).map(move |x| (y, x)))
            .map(|(y, x)| {
                let word = (*readback).pixels.cast::<u8>().add(y * pitch + x * 4).cast::<u32>().read_unaligned();
                [word & 0x3ff, (word >> 10) & 0x3ff, (word >> 20) & 0x3ff]
            })
            .collect::<Vec<[u32; 3]>>();
        SDL_DestroySurface(readback);
        SDL_SetRenderTarget(renderer, ptr::null_mut());
        let mut worst = 0.0f32;
        for (pixel, drawn) in linear.as_chunks::<4>().0.iter().zip(&codes) {
            let expected = gamma22_of_scrgb([pixel[0], pixel[1], pixel[2]], [encoding[1], encoding[2], 0.0, 0.0]);
            for (want, got) in expected.iter().zip(drawn) {
                worst = worst.max((want * 1023.0 - *got as f32).abs());
            }
        }
        eprintln!("gamma 2.2 draw against the scanout pass's encode of the scRGB draw: largest difference {worst} codes");
        assert!(worst <= 1.5, "the gamma 2.2 draw differs from the scanout pass's encode by {worst} 10-bit codes");
        drop(pass);
        for texture in [scrgb, pq] { SDL_DestroyTexture(texture); }
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}
