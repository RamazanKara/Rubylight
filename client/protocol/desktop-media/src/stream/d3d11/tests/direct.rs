//! Direct-to-back-buffer presentation (`planes_direct.hlsl`), back-buffer retention, surface
//! description caching, and decoder open timing tests.

use sdl3_sys::everything::SDL_FRect;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};

use super::*;
use crate::stream::direct_planes::{constants, Constants, Look};

fn warp() -> (ID3D11Device, ID3D11DeviceContext) {
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
        (device.unwrap(), context.unwrap())
    }
}

/// A render target standing for SDL's back buffer, cleared to magenta.
fn back_buffer(device: &ID3D11Device, context: &ID3D11DeviceContext, size: (u32, u32), format: DXGI_FORMAT) -> (ID3D11Texture2D, ID3D11RenderTargetView) {
    let mut texture = None;
    let mut view = None;
    // SAFETY: A plain texture and its view on a live device.
    unsafe {
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: size.0, Height: size.1, MipLevels: 1, ArraySize: 1, Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() }, None, Some(&mut texture)).unwrap();
        device.CreateRenderTargetView(texture.as_ref().unwrap(), None, Some(&mut view)).unwrap();
        context.ClearRenderTargetView(view.as_ref().unwrap(), &[1.0, 0.0, 1.0, 1.0]);
    }
    (texture.unwrap(), view.unwrap())
}

/// The colours of an 8-bit or half-float back buffer, row by row, in 0..1.
fn read_colors(device: &ID3D11Device, context: &ID3D11DeviceContext, target: &ID3D11Texture2D) -> Vec<[f32; 3]> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: A live texture.
    unsafe { target.GetDesc(&mut desc) };
    if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
        return read_back(device, context, target);
    }
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let mut staging = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc },
            None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        context.CopyResource(&staging, target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let mut pixels = Vec::new();
        for y in 0..desc.Height as usize {
            let row = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize), desc.Width as usize * 4);
            pixels.extend(row.chunks(4).map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]));
        }
        context.Unmap(&staging, 0);
        pixels
    }
}

fn srgb_from_linear(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { v.powf(1.0 / 2.4) * 1.055 - 0.055 }
}

/// What SDL's draw of the linear texture does to a texel under `look` (`Look`'s doc): the tone
/// map, the colour scale, and on an sRGB output the encoding and clamp.
fn sdl_look(linear: [f32; 3], look: Look) -> [f32; 3] {
    const TO_2020: [[f32; 3]; 3] = [[0.627404, 0.329283, 0.043313], [0.069097, 0.919541, 0.011362], [0.016391, 0.088013, 0.895595]];
    const TO_709: [[f32; 3]; 3] = [[1.660496, -0.587656, -0.072840], [-0.124547, 1.132895, -0.008348], [-0.018154, -0.100597, 1.118751]];
    let mul = |m: [[f32; 3]; 3], v: [f32; 3]| m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);
    let mut rgb = linear;
    if let Some((t1, t2)) = look.tonemap() {
        let mut v = mul(TO_2020, rgb);
        let max = v[0].max(v[1]).max(v[2]);
        if max > 0.0 { v = v.map(|c| c * (1.0 + t1 * max) / (1.0 + t2 * max)); }
        rgb = mul(TO_709, v);
    }
    let rgb = rgb.map(|c| c * look.sdr_white * look.color_scale);
    if look.linear_output { rgb } else { rgb.map(|c| srgb_from_linear(c).clamp(0.0, 1.0)) }
}

/// Where the direct draw may differ from the texture draw: one 8-bit code, or two half-float steps.
fn assert_within_a_step(got: [f32; 3], expected: [f32; 3], linear: bool, what: &str) {
    for (g, e) in got.iter().zip(expected) {
        let tolerance = if linear { 2.5e-3 * e.abs().max(0.01) } else { 1.0 / 255.0 + 1e-4 };
        assert!((g - e).abs() <= tolerance, "{what}: got {got:?}, expected {expected:?}");
    }
}

/// Draws `info` as the pass draws a planar picture directly into a `back_size` buffer of
/// `format`, in `viewport` with `constants`, and reads the buffer back.
fn direct_on_warp(info: &media::FrameInfo, parameters: &[u32; 8], format: DXGI_FORMAT, back_size: (u32, u32), viewport: [f32; 4], constants: &Constants) -> Vec<[f32; 3]> {
    let (device, context) = warp();
    let target = linear_target(&device, info.width as u32, info.height as u32);
    // SAFETY: A pass on the target's device, used on this thread.
    let mut pass = unsafe { PlanesPass::new(target.as_raw()) }.unwrap();
    let (back, view) = back_buffer(&device, &context, back_size, format);
    pass.upload_frame(info).unwrap();
    pass.draw_direct(&view, viewport, &DirectSource::Frame, parameters, constants, &mut |submit| submit()).unwrap();
    read_colors(&device, &context, &back)
}

fn pictures() -> Vec<(i32, u32, u32)> {
    // 8-bit 4:4:4, 10-bit 4:4:4 and 10-bit 4:2:0 with odd sizes, whose chroma is sampled between sites.
    vec![(media::FRAME_YUV444P, 8, 4), (media::FRAME_YUV444P10, 8, 4), (media::FRAME_YUV420P10, 7, 3)]
}

fn spread(kind: i32) -> impl Fn(usize, u32, u32) -> u16 {
    move |plane, x, y| {
        let ten_bit = 64 + ((x * 97 + y * 211 + plane as u32 * 53) % 877) as u16;
        if kind == media::FRAME_YUV444P { ten_bit / 4 } else { ten_bit }
    }
}

#[test]
fn the_direct_draw_matches_the_texture_draw_and_sdls_encoding_on_an_srgb_output() {
    for (kind, width, height) in pictures() {
        let (_planes, info) = picture(kind, width, height, spread(kind));
        let layout = media::PlanarLayout::of(&info).unwrap();
        let parameters = crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0);
        let texture = draw_on_warp(&info, &parameters);
        let look = Look { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: false };
        let rect = SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
        let direct = direct_on_warp(&info, &parameters, DXGI_FORMAT_R8G8B8A8_UNORM, (width, height), [0.0, 0.0, width as f32, height as f32],
            &constants(&rect, (width as i32, height as i32), look));
        for (at, (got, linear)) in direct.iter().zip(&texture).enumerate() {
            assert_within_a_step(*got, sdl_look(*linear, look), false, &format!("kind {kind} pixel {at}"));
        }
    }
}

#[test]
fn the_direct_draw_matches_the_texture_draw_on_a_scrgb_output_with_and_without_the_tone_map() {
    let (_planes, mut info) = picture(media::FRAME_YUV420P10, 8, 4, spread(media::FRAME_YUV420P10));
    // HDR10: BT.2020 and PQ.
    info.color_space = 9;
    info.color_trc = 16;
    let layout = media::PlanarLayout::of(&info).unwrap();
    let parameters = crate::stream::present::planar_parameters(&info, layout, true, 2.5, 5.0);
    let texture = draw_on_warp(&info, &parameters);
    let rect = SDL_FRect { x: 0.0, y: 0.0, w: 8.0, h: 4.0 };
    for (what, look) in [
        ("scRGB", Look { texture_headroom: 0.0, output_headroom: 5.0, sdr_white: 2.5, color_scale: 1.0, linear_output: true }),
        ("scRGB with SDL's tone map", Look { texture_headroom: 12.0, output_headroom: 4.0, sdr_white: 2.5, color_scale: 0.5, linear_output: true }),
    ] {
        let direct = direct_on_warp(&info, &parameters, DXGI_FORMAT_R16G16B16A16_FLOAT, (8, 4), [0.0, 0.0, 8.0, 4.0], &constants(&rect, (8, 4), look));
        for (at, (got, linear)) in direct.iter().zip(&texture).enumerate() {
            assert_within_a_step(*got, sdl_look(*linear, look), true, &format!("{what} pixel {at}"));
        }
    }
}

#[test]
fn the_direct_draw_fills_only_its_rectangle_and_maps_the_picture_onto_it() {
    let (_planes, info) = picture(media::FRAME_YUV444P, 8, 4, spread(media::FRAME_YUV444P));
    let layout = media::PlanarLayout::of(&info).unwrap();
    let parameters = crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0);
    let texture = draw_on_warp(&info, &parameters);
    let look = Look { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: false };
    // The picture's 8 x 4 pixels at (3, 2) of a 16 x 10 back buffer.
    let rect = SDL_FRect { x: 3.0, y: 2.0, w: 8.0, h: 4.0 };
    let direct = direct_on_warp(&info, &parameters, DXGI_FORMAT_R8G8B8A8_UNORM, (16, 10), [3.0, 2.0, 8.0, 4.0], &constants(&rect, (8, 4), look));
    for y in 0..10u32 {
        for x in 0..16u32 {
            let got = direct[(y * 16 + x) as usize];
            if (3..11).contains(&x) && (2..6).contains(&y) {
                assert_within_a_step(got, sdl_look(texture[((y - 2) * 8 + x - 3) as usize], look), false, &format!("pixel ({x}, {y})"));
            } else {
                assert_eq!(got, [1.0, 0.0, 1.0], "pixel ({x}, {y}) is outside the rectangle and keeps what SDL cleared");
            }
        }
    }
}

/// A 4:4:4 surface as AYUV bytes (Cr, Cb, Y, A) in slice 1 of an R8G8B8A8 array on WARP (which
/// makes no AYUV textures), read in place (`BoundSurface::view`) or from the pass's copy of it.
fn surface_direct_on_warp(in_place: bool, width: u32, height: u32, code: impl Fn(u32, u32) -> [u16; 3]) -> Vec<[f32; 3]> {
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            bytes.extend(texel(DXGI_FORMAT_AYUV, code(x, y)));
        }
    }
    let info = media::FrameInfo { width: width as i32, height: height as i32, kind: media::FRAME_D3D11_444, ten_bit: 0, pts: 0,
        data: [std::ptr::null_mut(); 3], linesize: [0; 3], color_space: 1, color_range: 1, color_trc: 1 };
    let layout = media::PlanarLayout::of_d3d11(&info).unwrap();
    let parameters = surface_parameters(crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0), DXGI_FORMAT_AYUV).unwrap();
    let (device, context) = warp();
    let target = linear_target(&device, width, height);
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let mut pass = PlanesPass::new(target.as_raw()).unwrap();
        let blank = vec![0u8; bytes.len()];
        let data = [blank.as_ptr(), bytes.as_ptr()].map(|bytes| D3D11_SUBRESOURCE_DATA { pSysMem: bytes.cast(), SysMemPitch: width * 4, SysMemSlicePitch: 0 });
        let mut array = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 2, Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(data.as_ptr()), Some(&mut array)).unwrap();
        let array = array.unwrap();
        if in_place {
            let mut locked = 0; let view = pass.slice_view(&array, 1, 1, DXGI_FORMAT_AYUV, &mut |op| { locked += 1; op() }).unwrap();
            assert_eq!(locked, 1); pass.bound = Some(BoundSurface { view: Some(view), parameters });
        } else {
            let copy = SurfaceCopy::new(&pass.device, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, (width, height)).unwrap();
            context.CopySubresourceRegion(&copy.texture, 0, 0, 0, 0, &array, 1, None);
            pass.surface = Some(copy);
            pass.bound = Some(BoundSurface { view: None, parameters });
        }
        let (back, view) = back_buffer(&device, &context, (width, height), DXGI_FORMAT_R8G8B8A8_UNORM);
        let look = Look { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: false };
        let rect = SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
        pass.draw_direct(&view, [0.0, 0.0, width as f32, height as f32], &DirectSource::Surface, &parameters,
            &constants(&rect, (width as i32, height as i32), look), &mut |submit| submit()).unwrap();
        read_colors(&device, &context, &back)
    }
}

#[test]
fn a_4x4_surface_is_drawn_directly_from_the_decoders_texture_or_from_its_copy_as_the_texture_draw_would() {
    let (width, height) = (8, 4);
    let code = |x: u32, y: u32| -> [u16; 3] { [64 + ((x * 23 + y * 41) % 170) as u16 + 20, 128 + (x * 9) as u16, 110 + (y * 17) as u16] };
    // The texture draw of the same surface, converted into the linear texture (limited range).
    let expected = draw_surface_on_warp(DXGI_FORMAT_AYUV, width, height, 1, code);
    let look = Look { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: false };
    for in_place in [true, false] {
        let direct = surface_direct_on_warp(in_place, width, height, code);
        for (at, (got, linear)) in direct.iter().zip(&expected).enumerate() {
            assert_within_a_step(*got, sdl_look(*linear, look), false, &format!("in place {in_place}, pixel {at}"));
        }
    }
}

#[test]
fn a_surface_read_in_place_is_also_drawn_into_the_texture_for_what_reads_it() {
    let (width, height) = (8, 4);
    let code = |x: u32, y: u32| -> [u16; 3] { [90 + ((x * 23 + y * 41) % 120) as u16, 128 + (x * 9) as u16, 100 + (y * 17) as u16] };
    let expected = draw_surface_on_warp(DXGI_FORMAT_AYUV, width, height, 1, code);
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            bytes.extend(texel(DXGI_FORMAT_AYUV, code(x, y)));
        }
    }
    let info = media::FrameInfo { width: width as i32, height: height as i32, kind: media::FRAME_D3D11_444, ten_bit: 0, pts: 0,
        data: [std::ptr::null_mut(); 3], linesize: [0; 3], color_space: 1, color_range: 1, color_trc: 1 };
    let layout = media::PlanarLayout::of_d3d11(&info).unwrap();
    let parameters = surface_parameters(crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0), DXGI_FORMAT_AYUV).unwrap();
    let (device, context) = warp();
    let target = linear_target(&device, width, height);
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    let pixels = unsafe {
        let mut pass = PlanesPass::new(target.as_raw()).unwrap();
        let data = [D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: width * 4, SysMemSlicePitch: 0 }; 2];
        let mut array = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 2, Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(data.as_ptr()), Some(&mut array)).unwrap();
        let mut locked = 0;
        let view = pass.slice_view(array.as_ref().unwrap(), 7, 1, DXGI_FORMAT_AYUV, &mut |op| { locked += 1; op() }).unwrap();
        let _ = pass.slice_view(array.as_ref().unwrap(), 7, 1, DXGI_FORMAT_AYUV, &mut |op| { locked += 1; op() }).unwrap();
        assert_eq!(locked, 1, "slice_view locks only on creation"); pass.bound = Some(BoundSurface { view: Some(view), parameters });
        pass.draw_bound_surface(&mut |submit| { locked += 1; submit() }).unwrap(); assert_eq!(locked, 2, "the submission of a surface read in place runs under the decoder's lock");
        read_back(&device, &context, &target)
    };
    for (at, (got, want)) in pixels.iter().zip(&expected).enumerate() {
        assert_close(*got, *want, &format!("pixel {at}"));
    }
}

#[test]
fn the_back_buffer_view_is_kept_for_its_swap_chain_and_size_and_let_go_otherwise() {
    let (device, context) = warp();
    let (_back, view) = back_buffer(&device, &context, (4, 4), DXGI_FORMAT_R8G8B8A8_UNORM);
    let mut held = BackBuffer::default();
    held.hold(7, (1920, 1080), view);
    held.keep_if(7, (1920, 1080));
    assert!(held.is_held(), "the same swap chain at the same size keeps the view between frames");
    held.keep_if(7, (1280, 720));
    assert!(!held.is_held(), "SDL resizes the buffers at its next flush: the view goes first");
    let (_back, view) = back_buffer(&device, &context, (4, 4), DXGI_FORMAT_R8G8B8A8_UNORM);
    held.hold(7, (1920, 1080), view.clone());
    held.keep_if(8, (1920, 1080));
    assert!(!held.is_held(), "another swap chain");
    held.hold(7, (1920, 1080), view);
    held.release();
    assert!(!held.is_held(), "a frame that does not draw with it lets it go");
}

/// Times what a resume or reconnect used to pay for a decoder: opening one, and opening its
/// codec context again on the device it already has. Software and D3D11VA, the backends this
/// PC has; run it in release (`cargo test -p nightjar-desktop --release -- --ignored --nocapture
/// decoder_open_costs`). Not a check: a measurement, so it asserts only that the decoder opens.
#[test]
#[ignore = "measurement: times decoder opens (release build)"]
fn decoder_open_costs() {
    use crate::media::{Codec, Hardware, VideoDecoder};
    use std::time::Instant;

    fn median(what: &str, mut run: impl FnMut()) {
        let mut ms: Vec<f64> = Vec::new();
        for _ in 0..20 {
            let started = Instant::now();
            run();
            ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(f64::total_cmp);
        eprintln!("nightjar: {what}: median {:.2} ms, p10 {:.2} ms, p90 {:.2} ms", ms[10], ms[2], ms[18]);
    }

    let fixture = include_bytes!("../../../../tests/fixtures/hevc-420-8.bin");
    fn first_picture(fixture: &[u8], mut open: impl FnMut() -> VideoDecoder) {
        let mut decoder = open();
        assert_eq!(decoder.send(fixture, 0), 0);
        assert!(matches!(decoder.receive(), crate::media::Received::Frame(_)), "the fixture's picture");
    }

    median("software HEVC open", || drop(VideoDecoder::open(Codec::Hevc, Hardware::None, 8, false).expect("software HEVC")));
    median("software HEVC open and first picture", || first_picture(fixture, || VideoDecoder::open(Codec::Hevc, Hardware::None, 8, false).expect("software HEVC")));
    let mut software = VideoDecoder::open(Codec::Hevc, Hardware::None, 8, false).expect("software HEVC");
    median("software HEVC reopen, context only", || software.reopen().expect("a software reopen"));

    // SAFETY: Plain D3D11 calls on a device made and used on this thread; the decoders hold
    // their own reference to it.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION,
            Some(&mut device), None, Some(&mut context)).expect("a video Direct3D 11 device");
        let device = device.unwrap();
        median("D3D11VA HEVC open", || drop(VideoDecoder::open(Codec::Hevc, Hardware::D3d11(device.as_raw()), 1, true).expect("D3D11VA HEVC")));
        median("D3D11VA HEVC open and first picture", || first_picture(fixture, || VideoDecoder::open(Codec::Hevc, Hardware::D3d11(device.as_raw()), 1, true).expect("D3D11VA HEVC")));
        let mut d3d11 = VideoDecoder::open(Codec::Hevc, Hardware::D3d11(device.as_raw()), 1, true).expect("D3D11VA HEVC");
        median("D3D11VA HEVC reopen, device kept", || d3d11.reopen().expect("a D3D11VA reopen"));
        // The lifecycle, not only the timing: a flushed decoder takes the next stream's key frame
        // on the same device, and so does one opened again after a failure.
        d3d11.flush();
        assert_eq!(d3d11.send(fixture, 1), 0);
        assert!(matches!(d3d11.receive(), crate::media::Received::Frame(_)), "a flushed D3D11VA decoder decodes the next stream");
        d3d11.reopen().expect("a D3D11VA reopen");
        assert_eq!(d3d11.send(fixture, 2), 0);
        assert!(matches!(d3d11.receive(), crate::media::Received::Frame(_)), "a reopened D3D11VA decoder decodes the next stream");
    }
}

/// The decoder's texture is asked for its description and device once, not for each picture of
/// the pool's few textures, and a picture that does not fit is still refused from what is kept.
#[test]
fn a_decoder_texture_is_asked_for_its_description_once() {
    let (width, height) = (8, 4);
    let bytes = vec![0u8; (width * height * 4) as usize];
    let info = media::FrameInfo { width: width as i32, height: height as i32, kind: media::FRAME_D3D11_444, ten_bit: 0, pts: 0,
        data: [std::ptr::null_mut(); 3], linesize: [0; 3], color_space: 1, color_range: 1, color_trc: 1 };
    let layout = media::PlanarLayout::of_d3d11(&info).unwrap();
    let parameters = crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0);
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
        let (device, _context): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), context.unwrap());
        let target = linear_target(&device, width, height);
        let mut pass = PlanesPass::new(target.as_raw()).unwrap();
        let make = |array: u32| {
            let data = vec![D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: width * 4, SysMemSlicePitch: 0 }; array as usize];
            let mut texture = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: array, Format: DXGI_FORMAT_AYUV,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(data.as_ptr()), Some(&mut texture)).ok().and(texture)
        };
        let (Some(first), Some(second)) = (make(2), make(2)) else {
            eprintln!("WARP makes no AYUV textures here: the cache is not exercised");
            return;
        };
        let lock = &mut |copy: &mut dyn FnMut()| copy();
        for slice in [0, 1, 0, 1, 1] {
            pass.draw_texture(&first, slice, (width, height), &parameters, lock).unwrap();
        }
        assert_eq!(pass.surface_queries, 1, "one texture, asked once");
        pass.draw_texture(&second, 0, (width, height), &parameters, lock).unwrap();
        pass.draw_texture(&second, 1, (width, height), &parameters, lock).unwrap();
        assert_eq!(pass.surface_queries, 2, "the next texture is asked once");
        assert!(pass.draw_texture(&second, 2, (width, height), &parameters, lock).is_err(), "a slice past the array is refused from what is kept");
        assert!(pass.draw_texture(&second, 0, (width + 2, height), &parameters, lock).is_err(), "so is a picture larger than the texture");
        assert_eq!(pass.surface_queries, 2);
    }
}
