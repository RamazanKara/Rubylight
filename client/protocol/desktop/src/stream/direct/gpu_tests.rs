use super::*;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;

fn device() -> (ID3D11Device, ID3D11DeviceContext) {
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None,
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
        (device.unwrap(), context.unwrap())
    }
}

/// A shader-readable NV12 or P010 texture array of `slices`, every slice holding the picture
/// `code(x, y) -> [Y, Cb, Cr]` (codes, 8 or 10 bits) in its top left `width` x `height`.
fn picture_array(device: &ID3D11Device, size: (u32, u32), picture: (u32, u32), slices: u32, ten_bit: bool, code: impl Fn(u32, u32) -> [u16; 3]) -> ID3D11Texture2D {
    let (w, h) = size;
    let bytes = if ten_bit { 2 } else { 1 };
    let mut data = vec![0u8; (w * h * 3 / 2 * bytes) as usize];
    let mut put = |at: usize, value: u16| if ten_bit { data[at * 2..at * 2 + 2].copy_from_slice(&(value << 6).to_le_bytes()) } else { data[at] = value as u8 };
    for y in 0..h {
        for x in 0..w {
            let [luma, cb, cr] = if x < picture.0 && y < picture.1 { code(x, y) } else if ten_bit { [64, 512, 512] } else { [16, 128, 128] };
            put((y * w + x) as usize, luma);
            let at = (w * h + (y / 2) * w + (x / 2) * 2) as usize;
            put(at, cb);
            put(at + 1, cr);
        }
    }
    // SAFETY: The initial data holds both planes; the array's slices are copies of that picture.
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC { Width: w, Height: h, MipLevels: 1, ArraySize: 1,
            Format: if ten_bit { DXGI_FORMAT_P010 } else { DXGI_FORMAT_NV12 }, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() };
        let mut single = None;
        device.CreateTexture2D(&desc, Some(&D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: w * bytes, SysMemSlicePitch: 0 }),
            Some(&mut single)).unwrap();
        let mut array = None;
        // As FFmpeg makes a decoder pool with bound textures (`media::bind_frames`).
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { ArraySize: slices, BindFlags: (D3D11_BIND_DECODER.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, ..desc },
            None, Some(&mut array)).unwrap();
        let (single, array) = (single.unwrap(), array.unwrap());
        let context = device.GetImmediateContext().unwrap();
        for slice in 0..slices {
            context.CopySubresourceRegion(&array, slice, 0, 0, 0, &single, 0, None);
        }
        array
    }
}

/// Draws into a fresh `format` target and returns its pixels as RGB floats.
#[allow(clippy::too_many_arguments)]
fn render(device: &ID3D11Device, context: &ID3D11DeviceContext, pass: &mut Direct, picture: &Picture,
    target_size: (u32, u32), format: DXGI_FORMAT, output: Output, tone: Tone) -> Vec<[f32; 3]> {
    // SAFETY: Resources of the test's device, used on this thread.
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC { Width: target_size.0, Height: target_size.1, MipLevels: 1, ArraySize: 1, Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() };
        let mut target = None;
        device.CreateTexture2D(&desc, None, Some(&mut target)).unwrap();
        let target = target.unwrap();
        let mut view = None;
        device.CreateRenderTargetView(&target, None, Some(&mut view)).unwrap();
        pass.draw(view.as_ref().unwrap(), [0.0, 0.0, target_size.0 as f32, target_size.1 as f32], picture, output, tone, |submit| submit()).unwrap();
        let mut staging = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc }, None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        context.CopyResource(&staging, &target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let mut pixels = Vec::new();
        for y in 0..target_size.1 as usize {
            let row = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
            for x in 0..target_size.0 as usize {
                pixels.push(if format == DXGI_FORMAT_R16G16B16A16_FLOAT {
                    let at = row.add(x * 8).cast::<u16>();
                    [half(*at), half(*at.add(1)), half(*at.add(2))]
                } else {
                    let at = row.add(x * 4);
                    [f32::from(*at) / 255.0, f32::from(*at.add(1)) / 255.0, f32::from(*at.add(2)) / 255.0]
                });
            }
        }
        context.Unmap(&staging, 0);
        pixels
    }
}

fn half(bits: u16) -> f32 {
    let (exponent, mantissa) = (i32::from((bits >> 10) & 31), f32::from(bits & 1023));
    let magnitude = if exponent == 0 { mantissa * 2f32.powi(-24) } else { (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15) };
    if bits & 0x8000 != 0 { -magnitude } else { magnitude }
}

/// The 10-bit PQ code of `nits`.
fn pq_code(nits: f64) -> u16 {
    let y = (nits / 10000.0).powf(0.1593017578125);
    let code = ((0.8359375 + 18.8515625 * y) / (1.0 + 18.6875 * y)).powf(78.84375);
    (64.0 + code * 876.0).round() as u16
}

#[test]
#[ignore = "needs a GPU"]
fn an_sdr_slice_of_a_decoder_array_draws_as_sdl_draws_it() {
    let (device, context) = device();
    // BT.709 limited-range white, black and red bands in a 96x32 picture of a 128x48 surface.
    let bands = [[235, 128, 128], [16, 128, 128], [63, 102, 240]];
    let texture = picture_array(&device, (128, 48), (96, 32), 3, false, |x, _| bands[(x / 32) as usize]);
    let picture = Picture { texture: texture.as_raw(), slice: 2, size: (96, 32), ten_bit: false, full_range: false, matrix: 1, transfer: Transfer::Sdr };
    // SAFETY: The test's device, on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    let output = Output { linear: false, color_scale: 1.0, headroom: 1.0 };
    let pixels = render(&device, &context, &mut pass, &picture, (192, 64), DXGI_FORMAT_R8G8B8A8_UNORM, output, Tone::default());
    for (band, want) in [[1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]].into_iter().enumerate() {
        let got = pixels[32 * 192 + band * 64 + 32];
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.03), "band {band}: {got:?}, expected {want:?}");
    }
    // The bottom row does not pick up the surface's padding (black) below the picture.
    let bottom = pixels[63 * 192 + 32];
    assert!(bottom.iter().all(|c| *c > 0.97), "bottom edge {bottom:?}");
    // On an HDR display (scRGB, SDR white at 2.5 x 80 nits) white is 2.5.
    let output = Output { linear: true, color_scale: 2.5, headroom: 4.0 };
    let pixels = render(&device, &context, &mut pass, &picture, (96, 32), DXGI_FORMAT_R16G16B16A16_FLOAT, output, Tone::default());
    assert!(pixels[16 * 96 + 16].iter().all(|c| (c - 2.5).abs() < 0.02), "{:?}", pixels[16 * 96 + 16]);
}

#[test]
#[ignore = "needs a GPU"]
fn a_ten_bit_sdr_picture_converts_as_gamma_not_pq() {
    let (device, context) = device();
    // BT.2020 limited-range 10-bit white and half grey (luma 940 and 502).
    let texture = picture_array(&device, (64, 32), (64, 32), 1, true, |x, _| if x < 32 { [940, 512, 512] } else { [502, 512, 512] });
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 32), ten_bit: true, full_range: false, matrix: 2, transfer: Transfer::Sdr };
    // SAFETY: The test's device, on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    // What `convert` draws for SDL: gamma-coded, unscaled.
    let output = Output { linear: false, color_scale: 1.0, headroom: 1.0 };
    let pixels = render(&device, &context, &mut pass, &picture, (64, 32), DXGI_FORMAT_R8G8B8A8_UNORM, output, Tone::default());
    assert!(pixels[16 * 64 + 16].iter().all(|c| *c > 0.98), "white {:?}", pixels[16 * 64 + 16]);
    // As PQ the grey would be about 92 nits; as gamma it stays half.
    assert!(pixels[16 * 64 + 48].iter().all(|c| (c - 0.5).abs() < 0.03), "grey {:?}", pixels[16 * 64 + 48]);
}

#[test]
#[ignore = "needs a GPU"]
fn hdr_pictures_follow_the_frames_peak_and_measure_it_without_hdr10_plus() {
    let (device, context) = device();
    // A grey 100-nit picture with a 1000-nit block: 10% of the pixels.
    let (grey, bright) = (pq_code(100.0), pq_code(1000.0));
    let texture = picture_array(&device, (64, 64), (64, 64), 1, true, |x, y| if x < 20 && y < 20 { [bright, 512, 512] } else { [grey, 512, 512] });
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 64), ten_bit: true, full_range: false, matrix: 2, transfer: Transfer::Pq };
    // SAFETY: The test's device, on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    // SDR white 200 nits (2.5 x 80), a 400-nit display: headroom 2.
    let output = Output { linear: true, color_scale: 2.5, headroom: 2.0 };
    let white_nits = 200.0;
    let at = |pixels: &[[f32; 3]], x: usize, y: usize| pixels[y * 64 + x][1] * 80.0;
    // HDR10+ says 1000 nits: the block lands on the display's 400 nits.
    let mapped = render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
        Tone { white_nits, dolby_vision: None, hdr10_plus: Some(1000.0), static_peak: Some(4000.0) });
    assert!((at(&mapped, 5, 5) - 400.0).abs() < 15.0, "{}", at(&mapped, 5, 5));
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Without any peak (none measured yet) the picture is drawn as coded, as SDL does.
    let plain = render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
        Tone { white_nits, dolby_vision: None, hdr10_plus: None, static_peak: None });
    assert!(pass.measured_peak().is_none(), "nothing was measured while HDR10+ was there");
    assert!((at(&plain, 40, 40) - 100.0).abs() < 3.0 && (at(&plain, 5, 5) - 1000.0).abs() < 30.0, "{} {}", at(&plain, 40, 40), at(&plain, 5, 5));
    // Without HDR10+ the pass measures the frames and gets to about 1000 nits.
    for _ in 0..200 {
        render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
            Tone { white_nits, dolby_vision: None, hdr10_plus: None, static_peak: Some(4000.0) });
        if pass.measured_peak().is_some() { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let measured = pass.measured_peak().expect("a measured peak within 200 frames");
    assert!((measured - 1000.0).abs() < 60.0, "measured {measured} nits");
    // A Dolby Vision peak of 1000 nits takes over from HDR10+ (here 4000): the block lands on
    // the display's 400 nits again.
    let mapped = render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
        Tone { white_nits, dolby_vision: Some(1000.0), hdr10_plus: Some(4000.0), static_peak: Some(4000.0) });
    assert!((at(&mapped, 5, 5) - 400.0).abs() < 15.0, "{}", at(&mapped, 5, 5));
}

#[test]
#[ignore = "needs a GPU"]
fn hlg_pictures_are_drawn_for_the_display_never_as_pq() {
    let (device, context) = device();
    // BT.2020 limited-range 10-bit HLG grey: 75% (reference white) and 50% (luma 721 and 502).
    let texture = picture_array(&device, (64, 32), (64, 32), 1, true, |x, _| if x < 32 { [721, 512, 512] } else { [502, 512, 512] });
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 32), ten_bit: true, full_range: false, matrix: 2, transfer: Transfer::Hlg };
    // SAFETY: The test's device, on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    let code = |luma: u16| f64::from(luma - 64) / 876.0;
    // An HDR display (scRGB): 200-nit SDR white (2.5 x 80), 800-nit peak.
    let output = Output { linear: true, color_scale: 2.5, headroom: 4.0 };
    let tone = Tone { white_nits: 200.0, ..Tone::default() };
    let pixels = render(&device, &context, &mut pass, &picture, (64, 32), DXGI_FORMAT_R16G16B16A16_FLOAT, output, tone);
    let display = hlg_display(4.0, 200.0);
    for (x, luma) in [(16, 721), (48, 502)] {
        let want = hlg_nits([code(luma); 3], display)[1];
        let got = f64::from(pixels[16 * 64 + x][1]) * 80.0;
        assert!((got - want).abs() < want * 0.03, "HLG {luma}: {got} nits, expected {want}");
        assert!((got - pq_nits(code(luma))).abs() > want * 0.5, "HLG {luma} drawn as PQ ({got} nits)");
    }
    // What SDL gets instead of the P010 it would decode as PQ: linear light relative to SDR
    // white, the same light once SDL scales it by its 2.5.
    let mut target = None;
    let desc = D3D11_TEXTURE2D_DESC { Width: 64, Height: 32, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() };
    // SAFETY: Resources of the test's device, used on this thread.
    let converted = unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut target)).unwrap();
        let target = target.unwrap();
        let mut view = None;
        device.CreateRenderTargetView(&target, None, Some(&mut view)).unwrap();
        pass.convert(view.as_ref().unwrap(), (64, 32), &picture, 200.0, 4.0, |submit| submit()).unwrap();
        read_half(&device, &context, &target, desc)
    };
    assert!((converted[16 * 64 + 16][1] * 2.5 - pixels[16 * 64 + 16][1]).abs() < 0.02, "{:?} {:?}", converted[16 * 64 + 16], pixels[16 * 64 + 16]);
    // An SDR display: reference white at SDR white, highlights tone-mapped into it like PQ's.
    let output = Output { linear: false, color_scale: 1.0, headroom: 1.0 };
    let pixels = render(&device, &context, &mut pass, &picture, (64, 32), DXGI_FORMAT_R8G8B8A8_UNORM, output, Tone { white_nits: 80.0, ..Tone::default() });
    let reference = hlg_display(1.0, 80.0);
    let (a, b) = tonemap_factors(reference.peak, reference.white, 1.0).unwrap();
    let linear = hlg_nits([code(721); 3], reference)[1] as f32 / reference.white;
    let want = linear * (1.0 + a * linear) / (1.0 + b * linear);
    let want = want.powf(1.0 / 2.4) * 1.055 - 0.055;
    let got = pixels[16 * 64 + 16];
    assert!(got.iter().all(|c| (c - want).abs() < 0.02), "{got:?}, expected {want}");
}

fn read_half(device: &ID3D11Device, context: &ID3D11DeviceContext, target: &ID3D11Texture2D, desc: D3D11_TEXTURE2D_DESC) -> Vec<[f32; 3]> {
    // SAFETY: Resources of the test's device, used on this thread.
    unsafe {
        let mut staging = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc }, None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        context.CopyResource(&staging, target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let mut pixels = Vec::new();
        for y in 0..desc.Height as usize {
            let row = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
            for x in 0..desc.Width as usize {
                let at = row.add(x * 8).cast::<u16>();
                pixels.push([half(*at), half(*at.add(1)), half(*at.add(2))]);
            }
        }
        context.Unmap(&staging, 0);
        pixels
    }
}
