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
    let picture = Picture { texture: texture.as_raw(), slice: 2, size: (96, 32), ten_bit: false, full_range: false, matrix: 1, transfer: Transfer::Sdr, centred_chroma: false };
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
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 32), ten_bit: true, full_range: false, matrix: 2, transfer: Transfer::Sdr, centred_chroma: false };
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
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 64), ten_bit: true, full_range: false, matrix: 2, transfer: Transfer::Pq, centred_chroma: false };
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
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 32), ten_bit: true, full_range: false, matrix: 2, transfer: Transfer::Hlg, centred_chroma: false };
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

#[test]
#[ignore = "needs a GPU"]
fn chroma_is_drawn_on_the_left_luma_sample_of_its_pair() {
    let (device, context) = device();
    // Grey, with one reddish chroma sample: the third, which hosts take at luma column 4
    // (chroma_sample_loc_type 0). Drawn in the middle of its pair, columns 4 and 5 were equally
    // red (Rubylight #130).
    let texture = picture_array(&device, (16, 8), (16, 8), 1, false, |x, _| [126, 128, if x / 2 == 2 { 168 } else { 128 }]);
    let picture = Picture { texture: texture.as_raw(), slice: 0, size: (16, 8), ten_bit: false, full_range: false, matrix: 1, transfer: Transfer::Sdr, centred_chroma: false };
    // SAFETY: The test's device, on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    let output = Output { linear: false, color_scale: 1.0, headroom: 1.0 };
    let pixels = render(&device, &context, &mut pass, &picture, (16, 8), DXGI_FORMAT_R8G8B8A8_UNORM, output, Tone::default());
    let red: Vec<f32> = (0..16).map(|x| pixels[4 * 16 + x][0] - pixels[4 * 16 + x][1]).collect();
    assert!(red[4] > 0.3, "the sample's own column is not fully red: {red:?}");
    assert!((red[3] - red[4] / 2.).abs() < 0.03 && (red[5] - red[4] / 2.).abs() < 0.03, "neighbours not half red: {red:?}");
    assert!(red[2].abs() < 0.03 && red[6].abs() < 0.03, "red beyond the neighbours: {red:?}");
    // A stream that says its chroma is centred (AVCHROMA_LOC_CENTER) gets it between columns 4
    // and 5, which are then equally red.
    let centred = Picture { centred_chroma: true, ..picture };
    let pixels = render(&device, &context, &mut pass, &centred, (16, 8), DXGI_FORMAT_R8G8B8A8_UNORM, output, Tone::default());
    let red: Vec<f32> = (0..16).map(|x| pixels[4 * 16 + x][0] - pixels[4 * 16 + x][1]).collect();
    assert!((red[4] - red[5]).abs() < 0.03 && red[4] > red[3] + 0.1, "centred chroma: {red:?}");
}

/// A decoder texture is asked for its description once, not for each picture, and the kept
/// reference stops another texture from taking its address.
#[test]
fn a_picture_texture_is_asked_for_its_description_once() {
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
    // SAFETY: Plain D3D11 calls on a device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
        let device: ID3D11Device = device.unwrap();
        let make = |width: u32| {
            let mut texture = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: 4, MipLevels: 1, ArraySize: 2, Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default() }, None, Some(&mut texture)).ok().and(texture)
        };
        let (Some(first), Some(second)) = (make(8), make(16)) else {
            eprintln!("WARP makes no textures here: the cache is not exercised");
            return;
        };
        let mut facts = KeptDesc::default();
        for _ in 0..5 {
            assert_eq!(facts.of(&first).Width, 8);
        }
        assert_eq!(facts.queries, 1, "one texture, asked once");
        assert_eq!((facts.of(&second).Width, facts.of(&second).ArraySize), (16, 2));
        assert_eq!(facts.queries, 2, "the next texture is asked once");
        facts.clear();
        assert_eq!(facts.of(&second).Width, 16);
        assert_eq!(facts.queries, 3, "a cleared entry is asked again");
    }
}

#[test]
fn kept_desc_caches_multiple_ring_textures_without_requerying() {
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
    let mut device = None;
    // SAFETY: Plain D3D11 WARP device made and used on this thread.
    unsafe {
        D3D11CreateDevice(
            None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0),
            Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, None,
        ).unwrap();
    }
    let device = device.unwrap();
    let textures: Vec<ID3D11Texture2D> = (0..24)
        .map(|_| {
            let mut tex = None;
            // SAFETY: Plain D3D11 texture on the test's WARP device.
            unsafe {
                device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 16, Height: 16, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_NV12,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut tex),
                ).unwrap();
            }
            tex.unwrap()
        })
        .collect();
    let mut kept = KeptDesc::default();
    for round in 0..5 {
        for tex in &textures {
            let desc = kept.of(tex);
            assert_eq!((desc.Width, desc.Height), (16, 16), "round {round}");
        }
    }
    assert_eq!(kept.queries, 24, "each ring texture (up to 32) is queried once without thrashing");
}

fn warp_device() -> (ID3D11Device, ID3D11DeviceContext) {
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
    // SAFETY: Plain D3D11 WARP device made and used on this thread.
    unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(
            None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0),
            Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context),
        ).unwrap();
        (device.unwrap(), context.unwrap())
    }
}

/// A shader-readable NV12 or P010 texture array whose slices hold distinct pictures, uploaded
/// one single-slice texture at a time via `CopySubresourceRegion` (matching `picture_array` and
/// avoiding driver bugs with multi-slice planar YUV `pInitialData`).
fn picture_array_per_slice(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    size: (u32, u32),
    slices: u32,
    ten_bit: bool,
    code: impl Fn(u32, u32, u32) -> [u16; 3],
) -> Option<ID3D11Texture2D> {
    let (w, h) = size;
    let bytes = if ten_bit { 2 } else { 1 };
    let slice_bytes = (w * h * 3 / 2 * bytes) as usize;
    let single_desc = D3D11_TEXTURE2D_DESC {
        Width: w, Height: h, MipLevels: 1, ArraySize: 1,
        Format: if ten_bit { DXGI_FORMAT_P010 } else { DXGI_FORMAT_NV12 },
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default()
    };
    let array_desc = D3D11_TEXTURE2D_DESC { ArraySize: slices, ..single_desc };
    let mut array = None;
    // SAFETY: Plain D3D11 texture array on the caller's device.
    unsafe { device.CreateTexture2D(&array_desc, None, Some(&mut array)) }.ok()?;
    let array = array?;
    for slice in 0..slices {
        let mut data = vec![0u8; slice_bytes];
        let mut put = |at: usize, value: u16| {
            if ten_bit {
                data[at * 2..at * 2 + 2].copy_from_slice(&(value << 6).to_le_bytes());
            } else {
                data[at] = value as u8;
            }
        };
        for y in 0..h {
            for x in 0..w {
                let [luma, cb, cr] = code(slice, x, y);
                put((y * w + x) as usize, luma);
                let at = (w * h + (y / 2) * w + (x / 2) * 2) as usize;
                put(at, cb);
                put(at + 1, cr);
            }
        }
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: w * bytes, SysMemSlicePitch: 0 };
        let mut single = None;
        // SAFETY: `data` holds both planes of one `w x h` NV12/P010 slice.
        unsafe { device.CreateTexture2D(&single_desc, Some(&init), Some(&mut single)) }.ok()?;
        let single = single?;
        // SAFETY: Both textures belong to `device` and `slice < slices`.
        unsafe { context.CopySubresourceRegion(&array, slice, 0, 0, 0, &single, 0, None) };
    }
    Some(array)
}

/// Picks a WARP device if it creates a 2-slice P010 SRV texture, else falls back to hardware
/// (returning `None` on headless machines where neither WARP nor hardware supports P010 SRVs).
fn p010_array_device(
    size: (u32, u32),
    code: impl Fn(u32, u32, u32) -> [u16; 3],
) -> Option<(ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D)> {
    let (warp, warp_ctx) = warp_device();
    if let Some(tex) = picture_array_per_slice(&warp, &warp_ctx, size, 2, true, &code) {
        return Some((warp, warp_ctx, tex));
    }
    // SAFETY: Optional hardware device fallback for local machines with a P010-capable GPU.
    let (mut hw, mut hw_ctx) = (None, None);
    let ok = unsafe {
        D3D11CreateDevice(
            None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None, D3D11_SDK_VERSION, Some(&mut hw), None, Some(&mut hw_ctx),
        )
    };
    let (hw, hw_ctx) = (hw.filter(|_| ok.is_ok())?, hw_ctx?);
    let tex = picture_array_per_slice(&hw, &hw_ctx, size, 2, true, code)?;
    Some((hw, hw_ctx, tex))
}

/// T7-5 regression: per-slice SRVs have `FirstArraySlice = slice, ArraySize = 1`, so `direct.hlsl`
/// must index slice `0` in both `draw` and `peak`. Measuring slice 1 must reflect slice 1's bright
/// pixels, not slice 0's dark pixels or out-of-bounds ~19.3 nits.
#[test]
fn hdr10_peak_measurement_reads_the_bound_slice_when_slice_is_nonzero() {
    let (dark, grey, bright) = (pq_code(10.0), pq_code(100.0), pq_code(1000.0));
    let Some((device, context, texture)) = p010_array_device((64, 64), |slice, x, y| {
        if slice == 0 {
            [dark, 512, 512]
        } else if x < 24 && y < 24 {
            [bright, 512, 512]
        } else {
            [grey, 512, 512]
        }
    }) else {
        eprintln!("no P010 2-slice SRV device here: skipping slice-1 GPU measurement check");
        return;
    };
    let picture = Picture {
        texture: texture.as_raw(), slice: 1, size: (64, 64), ten_bit: true, full_range: false,
        matrix: 2, transfer: Transfer::Pq, centred_chroma: false,
    };
    // SAFETY: Test device used on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    let output = Output { linear: true, color_scale: 2.5, headroom: 2.0 };
    // Without static/measured peak yet, the picture draws un-tone-mapped: slice 1's 1000-nit block,
    // not slice 0's 10-nit dark pixels.
    let plain = render(
        &device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
        Tone { white_nits: 200.0, dolby_vision: None, hdr10_plus: None, static_peak: None },
    );
    let bright_nits = plain[5 * 64 + 5][1] * 80.0;
    assert!((bright_nits - 1000.0).abs() < 35.0, "slice 1 draw got {bright_nits} nits, not ~1000");
    let tone = Tone { white_nits: 200.0, dolby_vision: None, hdr10_plus: None, static_peak: Some(4000.0) };
    for _ in 0..200 {
        render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output, tone);
        if pass.measured_peak().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let measured = pass.measured_peak().expect("measured peak on slice 1");
    assert!((measured - 1000.0).abs() < 60.0, "slice 1 measured {measured} nits (expected ~1000, not ~19.3 or slice 0's 10)");
}

/// T7-15 unit tests: generation filtering ignores stale histograms while still freeing slots,
/// and `Direct::reset()` invalidates in-flight read-backs without leaking prior-stream peaks.
#[test]
fn pending_histogram_generation_filtering_and_reset_invalidation() {
    let mut pending = std::collections::VecDeque::new();
    let mut tracker = PeakTracker::default();
    let mut bright = [0u32; BINS];
    bright[192] = 1000; // ~1000 nits
    let mut dim = [0u32; BINS];
    dim[128] = 1000; // ~90 nits

    assert_eq!(free_staging_slot(&pending, 3), Some(0));
    pending.push_back((0, 0));
    pending.push_back((1, 0));
    assert_eq!(free_staging_slot(&pending, 3), Some(2));
    // Stream resets -> generation becomes 1; gen-0 slots are popped & freed, never applied.
    finish_pending_histogram(&mut pending, 1, &mut tracker, Some(&bright));
    assert_eq!(tracker.value(), None, "stale gen-0 histogram must not update tracker");
    assert_eq!(free_staging_slot(&pending, 3), Some(0), "slot 0 freed for reuse");
    // Queue slot 0 for gen 1 while slot 1 (gen 0) is still ahead of it.
    pending.push_back((0, 1));
    finish_pending_histogram(&mut pending, 1, &mut tracker, Some(&bright));
    assert_eq!(tracker.value(), None, "second stale gen-0 slot also ignored");
    assert_eq!(pending.len(), 1);
    // Now gen-1 slot finishes and updates the tracker.
    finish_pending_histogram(&mut pending, 1, &mut tracker, Some(&dim));
    assert!(pending.is_empty());
    let peak = tracker.value().expect("gen-1 histogram applied");
    assert!(peak < 150.0, "only gen-1 peak applied, got {peak}");

    // End-to-end Direct::reset() + failed/unexecuted submit check.
    let bright_code = pq_code(1000.0);
    let Some((device, context, texture)) = p010_array_device((64, 64), |_, _, _| [bright_code, 512, 512]) else {
        eprintln!("no P010 2-slice SRV device here: skipping end-to-end Direct::reset() GPU check");
        return;
    };
    let picture = Picture {
        texture: texture.as_raw(), slice: 0, size: (64, 64), ten_bit: true, full_range: false,
        matrix: 2, transfer: Transfer::Pq, centred_chroma: false,
    };
    // SAFETY: Test device used on this thread.
    let mut pass = unsafe { Direct::new(&device) }.unwrap();
    let output = Output { linear: true, color_scale: 2.5, headroom: 2.0 };
    let measure_tone = Tone { white_nits: 200.0, dolby_vision: None, hdr10_plus: None, static_peak: Some(4000.0) };
    // Unexecuted submit never queues a staging slot.
    let target_desc = D3D11_TEXTURE2D_DESC {
        Width: 64, Height: 64, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default()
    };
    // SAFETY: Plain target view on the test device.
    let target_view = unsafe {
        let mut tex = None;
        device.CreateTexture2D(&target_desc, None, Some(&mut tex)).unwrap();
        let mut view = None;
        device.CreateRenderTargetView(tex.as_ref().unwrap(), None, Some(&mut view)).unwrap();
        view.unwrap()
    };
    // First create views with lock running, then skip ExecuteCommandList on the next call.
    // SAFETY: Test resources on this thread.
    unsafe {
        pass.views(&picture, &mut |op| op()).unwrap();
        pass.draw(&target_view, [0.0, 0.0, 64.0, 64.0], &picture, output, measure_tone, |_submit| {}).unwrap();
    }
    assert!(pass.pending.is_empty(), "skipped submission must not queue an uncopied staging slot");
    // Now submit a real measuring frame, then reset before collect() runs.
    render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output, measure_tone);
    assert_eq!(pass.pending.len(), 1);
    pass.reset();
    assert_eq!(pass.generation, 1);
    // Drain the completed gen-0 slot with a non-measuring draw: slot frees, peak stays None.
    let no_measure = Tone { white_nits: 200.0, dolby_vision: None, hdr10_plus: Some(1000.0), static_peak: Some(4000.0) };
    for _ in 0..200 {
        render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output, no_measure);
        if pass.pending.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pass.pending.is_empty(), "stale gen-0 slot was mapped, unmapped, and popped");
    assert_eq!(pass.measured_peak(), None, "stale gen-0 histogram did not leak across reset()");
}

/// T7-14 regression: `packed444.hlsl` uses bilinear reconstruction at 1:1 when Frame Warp has a
/// non-flat perspective lens, while keeping crisp 1:1 sampling when Frame Warp is flat.
#[test]
fn packed444_present_uses_bilinear_for_one_to_one_perspective_lens() {
    let source = include_bytes!("../../packed444.hlsl");
    let sdr = [b"#define SDR_PRESENT 1\n".as_slice(), source.as_slice()].concat();
    // SAFETY: Plain D3DCompile + WARP rendering on this thread.
    unsafe {
        let vertex_code = compile(source, s!("vertex"), s!("vs_5_0")).unwrap();
        let pixel_code = compile(source, s!("present"), s!("ps_5_0")).unwrap();
        let _sdr_ok = compile(&sdr, s!("present"), s!("ps_5_0")).unwrap();
        let (device, context) = warp_device();
        let bytes = |blob: &windows::Win32::Graphics::Direct3D::ID3DBlob| {
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
        };
        let (mut vs, mut ps) = (None, None);
        device.CreateVertexShader(bytes(&vertex_code), None, Some(&mut vs)).unwrap();
        device.CreatePixelShader(bytes(&pixel_code), None, Some(&mut ps)).unwrap();
        let (w, h) = (16u32, 16u32);
        // Alternating black/white vertical stripes (packing 0, 8-bit full-range, neutral chroma).
        let mut y_plane = vec![128u8; (w * h * 2) as usize];
        for y in 0..h {
            for x in 0..w {
                y_plane[(y * w + x) as usize] = if x % 2 == 0 { 0 } else { 255 };
            }
        }
        let uv_plane = vec![128u8; (w * h) as usize];
        let mut y_tex = None;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: w, Height: h * 2, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default()
            },
            Some(&D3D11_SUBRESOURCE_DATA { pSysMem: y_plane.as_ptr().cast(), SysMemPitch: w, SysMemSlicePitch: 0 }),
            Some(&mut y_tex),
        ).unwrap();
        let mut uv_tex = None;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: w / 2, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R8G8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default()
            },
            Some(&D3D11_SUBRESOURCE_DATA { pSysMem: uv_plane.as_ptr().cast(), SysMemPitch: w, SysMemSlicePitch: 0 }),
            Some(&mut uv_tex),
        ).unwrap();
        let (mut y_srv, mut uv_srv) = (None, None);
        device.CreateShaderResourceView(y_tex.as_ref().unwrap(), None, Some(&mut y_srv)).unwrap();
        device.CreateShaderResourceView(uv_tex.as_ref().unwrap(), None, Some(&mut uv_srv)).unwrap();
        let picture_params: [u32; 12] = [w, h, w, h, 0, 1, 1, 0, 1.0f32.to_bits(), 0, 0, 0];
        let mut cb0 = None;
        device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: 48, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default()
            },
            Some(&D3D11_SUBRESOURCE_DATA { pSysMem: picture_params.as_ptr().cast(), SysMemPitch: 0, SysMemSlicePitch: 0 }),
            Some(&mut cb0),
        ).unwrap();
        let mut cb1 = None;
        device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: 80, Usage: D3D11_USAGE_DYNAMIC, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32, ..Default::default()
            },
            None, Some(&mut cb1),
        ).unwrap();
        let target_desc = D3D11_TEXTURE2D_DESC {
            Width: w, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default()
        };
        let mut target = None;
        device.CreateTexture2D(&target_desc, None, Some(&mut target)).unwrap();
        let target = target.unwrap();
        let mut rtv = None;
        device.CreateRenderTargetView(&target, None, Some(&mut rtv)).unwrap();

        let run_present = |warp: [f32; 4], lens: [f32; 4]| -> Vec<[f32; 3]> {
            // The fill and probe row stays zero: no darkening, no probe.
            let presentation: [f32; 20] = [
                0.0, 0.0, w as f32, h as f32, 1.0, 1.0, 0.0, 0.0,
                warp[0], warp[1], warp[2], warp[3], lens[0], lens[1], lens[2], lens[3],
                0.0, 0.0, 0.0, 0.0,
            ];
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(cb1.as_ref().unwrap(), 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).unwrap();
            std::ptr::copy_nonoverlapping(presentation.as_ptr().cast::<u8>(), mapped.pData.cast::<u8>(), 80);
            context.Unmap(cb1.as_ref().unwrap(), 0);
            context.OMSetRenderTargets(Some(&[rtv.clone()]), None);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0,
            }]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(vs.as_ref().unwrap(), None);
            context.PSSetShader(ps.as_ref().unwrap(), None);
            context.PSSetShaderResources(0, Some(&[y_srv.clone(), uv_srv.clone()]));
            context.PSSetConstantBuffers(0, Some(&[cb0.clone(), cb1.clone()]));
            context.Draw(3, 0);
            read_half(&device, &context, &target, target_desc)
        };

        // 1:1 with non-flat perspective lens must bilinearly interpolate between black/white stripes.
        let warped = run_present([0.02, 0.0, 0.0, 0.0], [0.3, 0.0, 1.0, 1.0]);
        let blended = (4..12)
            .filter(|&x| {
                let v = warped[8 * w as usize + x][0];
                v > 0.05 && v < 0.95
            })
            .count();
        assert!(blended > 0, "1:1 perspective lens used nearest-neighbor instead of bilinear: {:?}", &warped[8 * w as usize..8 * w as usize + 16]);

        // 1:1 with flat warp keeps integer-rounded sharp sampling (0.0 or 1.0 only).
        let flat = run_present([0.02, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 1.0]);
        for x in 4..12 {
            let v = flat[8 * w as usize + x][0];
            assert!(v < 0.01 || (v - 1.0).abs() < 0.01, "1:1 flat warp blurred pixel {x}: {v}");
        }
    }
}
