use super::after_slice_draw;
use std::cell::Cell;

/// pyrowave.hlsl's Vulkan-only parts (Vulkan video's interleaved chroma) leave the plane shader
/// Direct3D compiles as it was.
#[test]
fn the_plane_shader_compiles_for_direct3d() {
    assert_eq!(super::PlanesPass::compiles(), Ok(()));
}

/// The packed presentation shader, both outputs, compiles with Frame Warp's rows and mask, and
/// the block `present` fills is the shader's.
#[test]
fn the_packed_presentation_takes_frame_warps_rows() {
    let shader = include_str!("../../packed444.hlsl");
    let colour = shader.find("float linearOutput; float colorScale; float2 unused;").expect("the colour row");
    let warp = shader.find("float4 warp;").expect("the warp row");
    assert!(colour < warp && shader[colour..warp].matches(';').count() == 3, "the warp row follows the colour row");
    let lens = shader[warp..].trim_start_matches("float4 warp;").trim_start();
    assert!(lens.starts_with("float4 warpLens;"), "the lens row follows it");
    assert!(lens.trim_start_matches("float4 warpLens;").trim_start().starts_with("};"), "and ends the block");
    assert!(shader.contains("Texture2D<float> hudMask : register(t2);"));
    assert_eq!(super::PRESENTATION_BYTES, 64);
    let sdr = [b"#define SDR_PRESENT 1\n".as_slice(), include_bytes!("../../packed444.hlsl")].concat();
    for source in [&include_bytes!("../../packed444.hlsl")[..], sdr.as_slice()] {
        // SAFETY: Plain compiler calls on static source.
        assert!(unsafe { super::compile(source, windows::core::s!("present"), windows::core::s!("ps_5_0")) }.is_ok());
    }
}

#[test]
fn a_slice_draw_that_was_not_recorded_still_signals_the_reuse_fence() {
    let signals = Cell::new(0);
    let signal = || { signals.set(signals.get() + 1); Ok(()) };
    let result = after_slice_draw(Err("cannot read the PyroWave planes".into()), signal);
    assert_eq!(signals.get(), 1, "a decode into the set waits for the value the slice noted");
    assert_eq!(result.unwrap_err(), "cannot read the PyroWave planes", "and the draw's error is the one reported");
    assert!(after_slice_draw(Ok(()), signal).is_ok());
    assert_eq!(signals.get(), 2);
    let result = after_slice_draw(Ok(()), || Err("device removed".into()));
    assert_eq!(result.unwrap_err(), "device removed");
}

use super::*;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};

fn from_half(bits: u16) -> f32 {
    let (exponent, mantissa) = (i32::from((bits >> 10) & 31), f32::from(bits & 1023));
    let magnitude = if exponent == 0 { mantissa * 2f32.powi(-24) } else { (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15) };
    if bits & 0x8000 != 0 { -magnitude } else { magnitude }
}

/// BT.709 limited-range codes to linear RGB, written out from the standard (not the shader).
fn bt709_limited_to_linear(code: [u16; 3], ten_bit: bool) -> [f32; 3] {
    let scale = if ten_bit { 4.0 } else { 1.0 };
    let y = (f32::from(code[0]) - 16.0 * scale) / (219.0 * scale);
    let (cb, cr) = ((f32::from(code[1]) - 128.0 * scale) / (224.0 * scale), (f32::from(code[2]) - 128.0 * scale) / (224.0 * scale));
    let r = y + 1.5748 * cr;
    let b = y + 1.8556 * cb;
    let g = y - 0.187324 * cb - 0.468124 * cr;
    [r, g, b].map(|c| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    })
}

/// A planar picture in padded rows, as FFmpeg hands it over: `code(plane, x, y)` per sample.
fn picture(kind: i32, width: u32, height: u32, code: impl Fn(usize, u32, u32) -> u16) -> (Vec<Vec<u8>>, media::FrameInfo) {
    let mut info = media::FrameInfo { width: width as i32, height: height as i32, kind, ten_bit: i32::from(kind != media::FRAME_YUV444P),
        pts: 0, data: [std::ptr::null_mut(); 3], linesize: [0; 3], color_space: 1, color_range: 1, color_trc: 1 };
    let layout = media::PlanarLayout::of(&info).unwrap();
    let bytes = if layout.ten_bit { 2 } else { 1 };
    let mut planes = Vec::new();
    for plane in 0..3 {
        let (w, h) = layout.plane_size(plane);
        let pitch = w as usize * bytes + 24;
        let mut data = vec![0xEEu8; pitch * h as usize];
        for y in 0..h {
            for x in 0..w {
                let at = y as usize * pitch + x as usize * bytes;
                let value = code(plane, x, y);
                if layout.ten_bit { data[at..at + 2].copy_from_slice(&value.to_le_bytes()); } else { data[at] = value as u8; }
            }
        }
        info.linesize[plane] = pitch as i32;
        planes.push(data);
    }
    for (plane, data) in planes.iter_mut().enumerate() { info.data[plane] = data.as_mut_ptr(); }
    (planes, info)
}

/// Draws `info` through the plane pass on a WARP device and reads the linear RGB back.
fn draw_on_warp(info: &media::FrameInfo, parameters: &[u32; 8]) -> Vec<[f32; 3]> {
    let (width, height) = (info.width as u32, info.height as u32);
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
        let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), context.unwrap());
        let desc = D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, ..Default::default() };
        let mut target = None;
        device.CreateTexture2D(&desc, None, Some(&mut target)).unwrap();
        let target = target.unwrap();
        let mut pass = PlanesPass::new(target.as_raw()).unwrap();
        pass.draw_frame(info, parameters).unwrap();
        read_back(&device, &context, &target)
    }
}

/// An RGBA64F `width` x `height` target texture the passes can draw into, on `device`.
fn linear_target(device: &ID3D11Device, width: u32, height: u32) -> ID3D11Texture2D {
    let desc = D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, ..Default::default() };
    let mut target = None;
    // SAFETY: A plain texture on a live device.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut target)) }.unwrap();
    target.unwrap()
}

/// The linear RGB of an RGBA64F `target`, row by row.
fn read_back(device: &ID3D11Device, context: &ID3D11DeviceContext, target: &ID3D11Texture2D) -> Vec<[f32; 3]> {
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        target.GetDesc(&mut desc);
        let mut staging = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc },
            None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        context.CopyResource(&staging, target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let mut pixels = Vec::new();
        for y in 0..desc.Height as usize {
            let row = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize).cast::<u16>(), desc.Width as usize * 4);
            pixels.extend(row.chunks(4).map(|p| [from_half(p[0]), from_half(p[1]), from_half(p[2])]));
        }
        context.Unmap(&staging, 0);
        pixels
    }
}

/// BT.709 full-range codes to linear RGB, written out from the standard (not the shader).
fn bt709_full_to_linear(code: [u16; 3], ten_bit: bool) -> [f32; 3] {
    let maximum = if ten_bit { 1023.0 } else { 255.0 };
    let half = if ten_bit { 512.0 } else { 128.0 };
    let y = f32::from(code[0]) / maximum;
    let (cb, cr) = ((f32::from(code[1]) - half) / maximum, (f32::from(code[2]) - half) / maximum);
    let (r, b) = (y + 1.5748 * cr, y + 1.8556 * cb);
    let g = y - 0.187324 * cb - 0.468124 * cr;
    [r, g, b].map(|c| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    })
}

/// One texel of a 4:4:4 decoder surface of `format` holding the codes Y, Cb, Cr: AYUV is the
/// bytes Cr, Cb, Y, A; Y410 the word Cb | Y << 10 | Cr << 20 | A << 30; Y416 the words Cb, Y,
/// Cr, A with the 10-bit codes in their high bits.
fn texel(format: DXGI_FORMAT, [y, cb, cr]: [u16; 3]) -> Vec<u8> {
    match format {
        DXGI_FORMAT_AYUV => vec![cr as u8, cb as u8, y as u8, 255],
        DXGI_FORMAT_Y410 => (u32::from(cb) | u32::from(y) << 10 | u32::from(cr) << 20 | 3 << 30).to_le_bytes().to_vec(),
        DXGI_FORMAT_Y416 => [cb << 6, y << 6, cr << 6, 0xffff].iter().flat_map(|w| w.to_le_bytes()).collect(),
        _ => unreachable!("not a 4:4:4 surface format"),
    }
}

/// Draws a `width` x `height` 4:4:4 surface of `format` (codes from `code`) through the
/// surface entry on a WARP device, as `draw_surface` draws a decoder's (the picture in slice 1
/// of two, copied out), and reads the linear RGB back. Where WARP makes no textures of the
/// video format, the same bytes are read through a texture of its view format.
fn draw_surface_on_warp(format: DXGI_FORMAT, width: u32, height: u32, color_range: i32, code: impl Fn(u32, u32) -> [u16; 3]) -> Vec<[f32; 3]> {
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            bytes.extend(texel(format, code(x, y)));
        }
    }
    let pitch = bytes.len() as u32 / height;
    let info = media::FrameInfo { width: width as i32, height: height as i32, kind: media::FRAME_D3D11_444,
        ten_bit: i32::from(format != DXGI_FORMAT_AYUV), pts: 0, data: [std::ptr::null_mut(); 3], linesize: [0; 3],
        color_space: 1, color_range, color_trc: 1 };
    let layout = media::PlanarLayout::of_d3d11(&info).unwrap();
    let parameters = crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0);
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
        let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), context.unwrap());
        let target = linear_target(&device, width, height);
        let mut pass = PlanesPass::new(target.as_raw()).unwrap();
        let blank = vec![0u8; bytes.len()];
        let data = [blank.as_ptr(), bytes.as_ptr()].map(|bytes| D3D11_SUBRESOURCE_DATA { pSysMem: bytes.cast(), SysMemPitch: pitch, SysMemSlicePitch: 0 });
        let mut source = None;
        let made = device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 2, Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(data.as_ptr()), Some(&mut source));
        match (made, source) {
            (Ok(()), Some(source)) => {
                pass.draw_texture(&source, 1, (width, height), &parameters, &mut |copy: &mut dyn FnMut()| copy()).unwrap();
            }
            _ => {
                let (view_format, _) = surface_view(format).unwrap();
                eprintln!("WARP makes no DXGI format {} textures: the same bytes are read as format {}", format.0, view_format.0);
                let mut texture = None;
                device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: view_format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(data[1..].as_ptr()), Some(&mut texture)).unwrap();
                let mut view = None;
                device.CreateShaderResourceView(texture.as_ref().unwrap(), None, Some(&mut view)).unwrap();
                let fragment = pass.surface_fragment().unwrap();
                pass.record_pass(&fragment, 4, &[view], &surface_parameters(parameters, format).unwrap(), None).unwrap();
            }
        }
        read_back(&device, &context, &target)
    }
}

/// The surface views and flags are the ones Direct3D allows on AYUV, Y410 and Y416, and the
/// flags replace the planes' depth and sample flags, keeping range, HDR, matrix and HLG.
#[test]
fn each_444_surface_format_is_read_through_its_view_with_its_flags() {
    assert_eq!(surface_view(DXGI_FORMAT_AYUV), Some((DXGI_FORMAT_R8G8B8A8_UNORM, 0)));
    assert_eq!(surface_view(DXGI_FORMAT_Y410), Some((DXGI_FORMAT_R10G10B10A2_UNORM, 16)));
    assert_eq!(surface_view(DXGI_FORMAT_Y416), Some((DXGI_FORMAT_R16G16B16A16_UNORM, 16 | 256)));
    for other in [DXGI_FORMAT_NV12, DXGI_FORMAT_P010, DXGI_FORMAT_Y210, DXGI_FORMAT_R8G8B8A8_UNORM] {
        assert_eq!(surface_view(other), None, "format {}", other.0);
    }
    // planar_parameters of a 10-bit full-range BT.2020 HLG picture: range 1, HDR 2, matrix
    // 2 << 2, 10-bit 16, 4:4:4 32, low-bit planes 64, HLG 128.
    let planar = [64, 32, 64, 32, 7, 9, 1 | 2 | 8 | 16 | 32 | 64 | 128, 5];
    assert_eq!(surface_parameters(planar, DXGI_FORMAT_Y410).unwrap()[6], 1 | 2 | 8 | 16 | 32 | 128);
    assert_eq!(surface_parameters(planar, DXGI_FORMAT_Y416).unwrap()[6], 1 | 2 | 8 | 16 | 32 | 128 | 256);
    assert_eq!(surface_parameters(planar, DXGI_FORMAT_AYUV).unwrap()[6], 1 | 2 | 8 | 32 | 128);
    assert_eq!(surface_parameters(planar, DXGI_FORMAT_AYUV).unwrap()[..6], planar[..6]);
    assert_eq!(surface_parameters(planar, DXGI_FORMAT_NV12), None);
}

/// The bug this closes: D3D11VA's 4:4:4 pictures (AYUV, Y410) had no draw, so Windows showed
/// 4:4:4 only from the software decoder. Each surface format comes out as the BT.709 colours
/// of its codes: Y, Cb and Cr taken from the right channels, at the right depth and range.
#[test]
fn d3d11_444_surfaces_convert_to_the_bt709_colours_of_their_codes() {
    // White, black and red first, then a spread of codes; 8-bit codes are the 10-bit ones / 4.
    let anchors: [[u16; 3]; 3] = [[940, 512, 512], [64, 512, 512], [250, 409, 960]];
    for format in [DXGI_FORMAT_AYUV, DXGI_FORMAT_Y410, DXGI_FORMAT_Y416] {
        let ten = format != DXGI_FORMAT_AYUV;
        let (width, height) = (8, 4);
        let code = |x: u32, y: u32| -> [u16; 3] {
            let ten_bit = if x < 3 && y == 0 { anchors[x as usize] }
                else { [0, 1, 2].map(|plane| 64 + ((x * 97 + y * 211 + plane * 53) % 877) as u16) };
            if ten { ten_bit } else { ten_bit.map(|c| c / 4) }
        };
        let pixels = draw_surface_on_warp(format, width, height, 1, code);
        for y in 0..height {
            for x in 0..width {
                assert_close(pixels[(y * width + x) as usize], bt709_limited_to_linear(code(x, y), ten), &format!("format {} pixel ({x}, {y})", format.0));
            }
        }
        assert_close(pixels[0], [1.0, 1.0, 1.0], "white");
        assert_close(pixels[1], [0.0, 0.0, 0.0], "black");
        let red = pixels[2];
        assert!(red[0] > 0.98 && red[1] < 0.01 && red[2] < 0.01, "format {}: red came out {red:?}", format.0);
        // Full range: the extremes are white and black, and a colour follows the full-range formula.
        let (top, half) = if ten { (1023, 512) } else { (255, 128) };
        let full = [[top, half, half], [0, half, half], if ten { [600, 300, 700] } else { [150, 75, 175] }];
        let pixels = draw_surface_on_warp(format, 3, 1, 2, |x, _| full[x as usize]);
        assert_close(pixels[0], [1.0, 1.0, 1.0], "full-range white");
        assert_close(pixels[1], [0.0, 0.0, 0.0], "full-range black");
        assert_close(pixels[2], bt709_full_to_linear(full[2], ten), "full-range colour");
    }
}

/// The 10-bit 4:4:4 probe fixture through this PC's D3D11VA decoder and the surface entry
/// (ignored: `cargo test -p nightjar-desktop -- --ignored hevc_444_10_decodes_through_d3d11va`).
/// It skips itself where the GPU has no HEVC RExt 4:4:4 10-bit decoder (AMD) or hands back
/// pictures of another kind, and says so; on NVIDIA and Intel every block is its codes' colour.
#[test]
#[ignore = "needs a GPU that decodes HEVC RExt 4:4:4 10-bit through D3D11VA (NVIDIA, Intel)"]
fn hevc_444_10_decodes_through_d3d11va_and_draws_its_codes() {
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    // SAFETY: Plain D3D11 calls on a device made and used on this thread; the decoder holds
    // its own reference to the device.
    unsafe {
        let (mut device, mut context) = (None, None);
        if D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).is_err() {
            eprintln!("skipped: no hardware Direct3D 11 device with video support");
            return;
        }
        let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), context.unwrap());
        let mut decoder = match media::VideoDecoder::open(media::Codec::Hevc, media::Hardware::D3d11(device.as_raw()), 1, true) {
            Ok(decoder) => decoder,
            Err(why) => { eprintln!("skipped: D3D11VA does not open for HEVC: {why}"); return; }
        };
        let status = decoder.send(include_bytes!("../../../tests/fixtures/hevc-444-10.bin"), 0);
        let frame = match decoder.receive() {
            media::Received::Frame(frame) if status >= 0 && !decoder.refused() => frame,
            _ => { eprintln!("skipped: this GPU's D3D11VA decoder does not decode HEVC RExt 4:4:4 10-bit (status {status})"); return; }
        };
        let info = frame.info();
        if info.kind != media::FRAME_D3D11_444 {
            eprintln!("skipped: the picture came out as {} (kind {}), not a 4:4:4 surface", frame.format_name(), info.kind);
            return;
        }
        assert_eq!((info.ten_bit, info.width, info.height), (1, 256, 256));
        let target = linear_target(&device, 256, 256);
        let mut pass = PlanesPass::new(target.as_raw()).unwrap();
        let layout = media::PlanarLayout::of_d3d11(&info).unwrap();
        pass.draw_surface(&frame, &info, &crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0)).unwrap();
        let pixels = read_back(&device, &context, &target);
        for block in 0..16 {
            let (x, y) = (block % 4 * 64 + 32, block / 4 * 64 + 32);
            let code = [0, 1, 2].map(|plane| media::hevc_444_10_fixture_code(plane, x, y));
            assert_close(pixels[y * 256 + x], bt709_limited_to_linear(code, true), &format!("block {block} ({}), codes {code:?}", frame.format_name()));
        }
    }
}

fn assert_close(got: [f32; 3], expected: [f32; 3], what: &str) {
    assert!(got.iter().zip(expected).all(|(g, e)| (g - e).abs() <= 0.003), "{what}: got {got:?}, expected {expected:?}");
}

#[test]
fn software_planar_pictures_convert_to_the_bt709_colours_of_their_codes() {
    // White, black and red first, then a spread of codes; 8-bit codes are the 10-bit ones / 4.
    let anchors: [[u16; 3]; 3] = [[940, 512, 512], [64, 512, 512], [250, 409, 960]];
    for kind in [media::FRAME_YUV444P, media::FRAME_YUV444P10, media::FRAME_YUV420P10] {
        let ten = kind != media::FRAME_YUV444P;
        let chroma444 = kind != media::FRAME_YUV420P10;
        // Odd sizes: FFmpeg's 4:2:0 chroma planes round up.
        let (width, height) = if chroma444 { (8, 4) } else { (7, 3) };
        let code = |plane: usize, x: u32, y: u32| -> u16 {
            let ten_bit = if x < 3 && y == 0 && chroma444 { anchors[x as usize][plane] }
                // 4:2:0 chroma is sampled between sites, so it stays one value there.
                else if plane > 0 && !chroma444 { 300 }
                else { 64 + ((x * 97 + y * 211 + plane as u32 * 53) % 877) as u16 };
            if ten { ten_bit } else { ten_bit / 4 }
        };
        let (_planes, info) = picture(kind, width, height, code);
        let layout = media::PlanarLayout::of(&info).unwrap();
        // The presenter's constants for an SDR picture.
        let pixels = draw_on_warp(&info, &crate::stream::present::planar_parameters(&info, layout, false, 1.0, 1.0));
        for y in 0..height {
            for x in 0..width {
                let (cx, cy) = if layout.chroma444 { (x, y) } else { (x / 2, y / 2) };
                let expected = bt709_limited_to_linear([code(0, x, y), code(1, cx, cy), code(2, cx, cy)], ten);
                assert_close(pixels[(y * width + x) as usize], expected, &format!("kind {kind} pixel ({x}, {y})"));
            }
        }
        if chroma444 {
            assert_close(pixels[0], [1.0, 1.0, 1.0], "white");
            assert_close(pixels[1], [0.0, 0.0, 0.0], "black");
            let red = pixels[2];
            assert!(red[0] > 0.98 && red[1] < 0.01 && red[2] < 0.01, "kind {kind}: red came out {red:?}");
        }
    }
}

#[test]
fn a_software_decoded_hlg_picture_is_drawn_as_hlg_not_pq() {
    // 10-bit 4:2:0 BT.2020 limited, the HLG transfer: grey at 75% (reference white, luma 721).
    let (_planes, mut info) = picture(media::FRAME_YUV420P10, 4, 2, |plane, _, _| if plane == 0 { 721 } else { 512 });
    info.color_space = 9;
    info.color_trc = 18;
    let layout = media::PlanarLayout::of(&info).unwrap();
    // An HDR display with SDR white at 2.5 x 80 nits and a 1000-nit peak (headroom 5): 1.0 is
    // 200 nits, and BT.2100's 1000-nit display puts it at 203 nits; read as PQ it would be 983.
    let pixels = draw_on_warp(&info, &crate::stream::present::planar_parameters(&info, layout, true, 2.5, 5.0));
    let display = super::super::direct::HlgDisplay { peak: 1000.0, gamma: 1.2, white: 200.0 };
    let nits = super::super::direct::hlg_nits([0.75; 3], display)[1] as f32;
    assert_close(pixels[0], [nits / 200.0; 3], "HLG reference white");
    // A 400-nit display (headroom 2) takes its own peak and the lower gamma BT.2100 gives it.
    let pixels = draw_on_warp(&info, &crate::stream::present::planar_parameters(&info, layout, true, 2.5, 2.0));
    let display = super::super::direct::hlg_display(2.0, 200.0);
    let nits = super::super::direct::hlg_nits([0.75; 3], display)[1] as f32;
    assert_close(pixels[0], [nits / 200.0; 3], "HLG reference white on a 400-nit display");
    // An SDR output: the 1000-nit reference display, reference white at SDR white (1.0).
    let pixels = draw_on_warp(&info, &crate::stream::present::planar_parameters(&info, layout, true, 1.0, 1.0));
    let nits = super::super::direct::hlg_nits([0.75; 3], super::super::direct::hlg_display(1.0, 80.0))[1] as f32;
    assert_close(pixels[0], [nits / 203.0; 3], "HLG reference white on an SDR output");
}

#[test]
fn a_picture_with_a_missing_plane_is_refused_by_name() {
    let (_planes, mut info) = picture(media::FRAME_YUV444P, 4, 2, |_, _, _| 128);
    info.data[2] = std::ptr::null_mut();
    let error = draw_on_warp_error(&info);
    assert!(error.contains("plane 2"), "{error}");
}

fn draw_on_warp_error(info: &media::FrameInfo) -> String {
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let mut device = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, None).unwrap();
        let device: ID3D11Device = device.unwrap();
        let mut target = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: 4, Height: 2, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() }, None, Some(&mut target)).unwrap();
        let mut pass = PlanesPass::new(target.unwrap().as_raw()).unwrap();
        pass.draw_frame(info, &[4, 2, 4, 2, 0, 3, 1 << 5, 1.0f32.to_bits()]).unwrap_err()
    }
}

// ---- The direct draw (planes_direct.hlsl) against the texture draw plus SDL's own draw. ----

use super::super::direct_planes::{constants, Constants, Look};
use sdl3_sys::everything::SDL_FRect;

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
            let view = pass.slice_view(&array, 1, 1, DXGI_FORMAT_AYUV).unwrap();
            pass.bound = Some(BoundSurface { view: Some(view), parameters });
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
        let view = pass.slice_view(array.as_ref().unwrap(), 7, 1, DXGI_FORMAT_AYUV).unwrap();
        pass.bound = Some(BoundSurface { view: Some(view), parameters });
        let mut locked = 0;
        pass.draw_bound_surface(&mut |submit| { locked += 1; submit() }).unwrap();
        assert_eq!(locked, 1, "the submission of a surface read in place runs under the decoder's lock");
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
