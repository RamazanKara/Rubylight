//! AMF's video converter against the shader conversion, offline: the same
//! fixture frames through both, scored against `image_quality`'s float
//! reference, with each converter's chroma siting and per-frame time.
//!
//! `vpe_ab` is ignored: it needs a Radeon and its AMF runtime. Run it in
//! release on an idle GPU (no display, encoder or host changes):
//!
//! ```text
//! set RUBYLIGHT_VPE_REPORT=D:\CodexArtifacts\vpe\report.json
//! set RUBYLIGHT_VPE_CAPTURES=D:\CodexArtifacts\vpe\captures   (optional)
//! cargo test -p butterpollo-windows --release --lib amf::vpe::tests::vpe_ab -- --ignored --nocapture
//! ```
//!
//! Captures are `NAME.json` beside `NAME.raw`, as `hdr_snapshot` writes them:
//! `{"width", "height", "stride", "format"}` with `format` starting `RGBA`
//! (scRGB half floats), `BGRA` (8-bit sRGB) or `RGB10A2` (PQ). Each scores
//! natively and downscaled to 1920x1080 and 1280x720; scRGB as PQ and HLG.
use super::*;
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use butterpollo_core::{
    amf_vpe::{Transfer, vui},
    image_quality as q,
    rtsp::Negotiated,
};
use serde_json::{Value, json};
use windows::{
    Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
    core::{GUID, Interface},
};

#[test]
fn amf_numbers_match_the_vendored_headers() {
    let pairs: [(i64, i64); 22] = [
        (
            vui::PRIMARIES_BT709,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_BT709 as i64,
        ),
        (
            vui::PRIMARIES_SMPTE170M,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_SMPTE170M as i64,
        ),
        (
            vui::PRIMARIES_BT2020,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_BT2020 as i64,
        ),
        (
            vui::PRIMARIES_CCCS,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_CCCS as i64,
        ),
        (
            vui::TRANSFER_BT709,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_BT709 as i64,
        ),
        (
            vui::TRANSFER_SMPTE170M,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_SMPTE170M
                as i64,
        ),
        (
            vui::TRANSFER_LINEAR,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_LINEAR as i64,
        ),
        (
            vui::TRANSFER_SRGB,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_IEC61966_2_1
                as i64,
        ),
        (
            vui::TRANSFER_BT2020_10,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_BT2020_10
                as i64,
        ),
        (
            vui::TRANSFER_PQ,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_SMPTE2084
                as i64,
        ),
        (
            vui::TRANSFER_HLG,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_ARIB_STD_B67
                as i64,
        ),
        (
            vui::MATRIX_RGB,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_GBR as i64,
        ),
        (
            vui::MATRIX_BT709,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_BT_709 as i64,
        ),
        (
            vui::MATRIX_BT601,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_BT_601 as i64,
        ),
        (
            vui::MATRIX_BT2020_NCL,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_BT_2020_NCL as i64,
        ),
        (
            vui::RANGE_STUDIO,
            AMF_COLOR_RANGE_ENUM_AMF_COLOR_RANGE_STUDIO as i64,
        ),
        (
            vui::RANGE_FULL,
            AMF_COLOR_RANGE_ENUM_AMF_COLOR_RANGE_FULL as i64,
        ),
        (
            vui::PROFILE_JPEG,
            AMF_VIDEO_CONVERTER_COLOR_PROFILE_ENUM_AMF_VIDEO_CONVERTER_COLOR_PROFILE_JPEG as i64,
        ),
        (
            vui::PROFILE_FULL_2020,
            AMF_VIDEO_CONVERTER_COLOR_PROFILE_ENUM_AMF_VIDEO_CONVERTER_COLOR_PROFILE_FULL_2020
                as i64,
        ),
        (
            vui::ACCELERATION_VPE_PREFERRED,
            AMF_VIDEO_CONVERTER_ACCELERATION_ENUM_AMF_VIDEO_CONVERTER_ACCELERATION_VPE_PREFERRED
                as i64,
        ),
        (
            vui::SCALE_BICUBIC,
            AMF_VIDEO_CONVERTER_SCALE_ENUM_AMF_VIDEO_CONVERTER_SCALE_BICUBIC as i64,
        ),
        (
            vui::TONEMAPPING_COPY,
            AMF_VIDEO_CONVERTER_TONEMAPPING_ENUM_AMF_VIDEO_CONVERTER_TONEMAPPING_COPY as i64,
        ),
    ];
    for (index, (ours, header)) in pairs.into_iter().enumerate() {
        assert_eq!(ours, header, "pair {index}");
    }
    use butterpollo_core::amf_vpe::Format;
    assert_eq!(
        Format::Nv12.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_NV12 as i64
    );
    assert_eq!(
        Format::P010.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_P010 as i64
    );
    assert_eq!(
        Format::Bgra.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_BGRA as i64
    );
    assert_eq!(
        Format::RgbaF16.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_RGBA_F16 as i64
    );
    assert_eq!(
        Format::R10G10B10A2.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_R10G10B10A2 as i64
    );
}

/// An AMF context on the test's device, as the encoder makes one.
struct Runtime {
    _library: libloading::Library,
    factory: *mut AMFFactory,
    context: *mut AMFContext,
}
impl Runtime {
    fn new(gpu: &Device) -> Result<Self> {
        // SAFETY: As in `Encoder::create_attempt`: AMFInit's C ABI, owned interfaces released in
        // Drop, and the library outliving them.
        unsafe {
            let library = libloading::Library::new("amfrt64.dll").context("AMD AMF runtime")?;
            let init: libloading::Symbol<
                unsafe extern "C" fn(u64, *mut *mut AMFFactory) -> AMF_RESULT,
            > = library.get(b"AMFInit\0")?;
            let version = ((AMF_VERSION_MAJOR as u64) << 48)
                | ((AMF_VERSION_MINOR as u64) << 32)
                | ((AMF_VERSION_RELEASE as u64) << 16)
                | AMF_VERSION_BUILD_NUM as u64;
            let mut factory = ptr::null_mut();
            check(init(version, &mut factory)).context("AMFInit")?;
            let mut context = ptr::null_mut();
            check(((*(*factory).pVtbl).CreateContext.unwrap())(
                factory,
                &mut context,
            ))?;
            check(((*(*context).pVtbl).InitDX11.unwrap())(
                context,
                gpu.device.as_raw(),
                AMF_DX_VERSION_AMF_DX11_1,
            ))
            .context("AMF InitDX11")?;
            Ok(Self {
                _library: library,
                factory,
                context,
            })
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: Self owns the context's reference; every converter is dropped first.
        unsafe {
            ((*(*self.context).pVtbl).Terminate.unwrap())(self.context);
            ((*(*self.context).pVtbl).Release.unwrap())(self.context);
        }
    }
}

/// `AMFTextureArrayIndexGUID` (AMF samples' DX11 interop): the slice of a
/// texture array an AMF surface lives in.
const ARRAY_INDEX: GUID = GUID::from_values(
    0x2811_5527,
    0xe7c3,
    0x4b66,
    [0x99, 0xd3, 0x4f, 0x2a, 0xe6, 0xb4, 0x7f, 0xaf],
);
/// The D3D11 texture and subresource behind an AMF surface.
fn native(surface: &gpu::Surface) -> Result<(ID3D11Texture2D, u32)> {
    // SAFETY: `surface` is a live DX11 surface; its plane's native pointer is a texture the
    // surface keeps alive, cloned (AddRef) before use; the private data read writes four bytes.
    unsafe {
        let v = &*(*surface.0).pVtbl;
        check((v.Convert.unwrap())(
            surface.0,
            AMF_MEMORY_TYPE_AMF_MEMORY_DX11,
        ))?;
        let plane = (v.GetPlaneAt.unwrap())(surface.0, 0);
        anyhow::ensure!(!plane.is_null(), "AMF surface has no plane");
        let raw = ((*(*plane).pVtbl).GetNative.unwrap())(plane);
        let texture = ID3D11Texture2D::from_raw_borrowed(&raw)
            .context("AMF surface has no texture")?
            .clone();
        let mut index = 0u32;
        let mut size = 4u32;
        let _ = texture.GetPrivateData(
            &ARRAY_INDEX,
            &mut size,
            Some(ptr::addr_of_mut!(index).cast()),
        );
        Ok((texture, index))
    }
}
/// The stream's `width` x `height` NV12 or P010 codes from a texture.
fn read_codes(
    gpu: &Device,
    texture: &ID3D11Texture2D,
    subresource: u32,
    width: usize,
    height: usize,
) -> Result<Vec<u16>> {
    // SAFETY: The staging copy has the texture's own size and format; each row read lies
    // within the mapped planes (chroma after the allocated luma rows) until Unmap.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let ten_bit = desc.Format == DXGI_FORMAT_P010;
        anyhow::ensure!(
            ten_bit || desc.Format == DXGI_FORMAT_NV12,
            "converter wrote format {:?}",
            desc.Format
        );
        desc.ArraySize = 1;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context
            .CopySubresourceRegion(&staging, 0, 0, 0, 0, texture, subresource, None);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let base = mapped.pData as *const u8;
        let bytes = if ten_bit { 2 } else { 1 };
        let mut codes = Vec::with_capacity(width * height * 3 / 2);
        let row = |at: usize| std::slice::from_raw_parts(base.add(at), width * bytes);
        let mut push = |data: &[u8]| {
            if ten_bit {
                codes.extend(
                    data.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|v| u16::from_le_bytes(*v) >> 6),
                );
            } else {
                codes.extend(data.iter().map(|&v| u16::from(v)));
            }
        };
        for y in 0..height {
            push(row(y * pitch));
        }
        let chroma = pitch * desc.Height as usize;
        for y in 0..height / 2 {
            push(row(chroma + y * pitch));
        }
        gpu.context.Unmap(&staging, 0);
        Ok(codes)
    }
}
/// Wait until the GPU finished writing `texture`, by reading two pixels.
fn finish(gpu: &Device, texture: &ID3D11Texture2D, subresource: u32) -> Result<()> {
    // SAFETY: A 2x2 staging texture of the source's format receives a 2x2 copy (even, as 4:2:0
    // requires) and is mapped and unmapped at once.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Width = 2;
        desc.Height = 2;
        desc.ArraySize = 1;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context.CopySubresourceRegion(
            &staging,
            0,
            0,
            0,
            0,
            texture,
            subresource,
            Some(&D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: 2,
                bottom: 2,
                back: 1,
            }),
        );
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        gpu.context.Unmap(&staging, 0);
        Ok(())
    }
}

/// `picture` (in its source encoding) as an uploadable capture.
fn image(picture: &q::Picture, pixel: Pixel) -> Image {
    let (w, h) = (picture.width, picture.height);
    let bytes_per = if pixel == Pixel::RgbaF16 { 8 } else { 4 };
    let mut bytes = Vec::with_capacity(w * h * bytes_per);
    for p in &picture.rgb {
        match pixel {
            Pixel::Bgra8 => {
                let c = p.map(|v| (v.clamp(0., 1.) * 255.).round() as u8);
                bytes.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
            Pixel::RgbaF16 => {
                for v in p.iter().copied().chain([1.]) {
                    bytes.extend_from_slice(&half::f16::from_f64(v).to_le_bytes());
                }
            }
            Pixel::Rgba10Pq => {
                let c = p.map(|v| (v.clamp(0., 1.) * 1023.).round() as u32);
                let packed = c[0] | c[1] << 10 | c[2] << 20 | 3 << 30;
                bytes.extend_from_slice(&packed.to_le_bytes());
            }
        }
    }
    Image {
        width: w as u32,
        height: h as u32,
        stride: w * bytes_per,
        bytes,
        captured: std::time::Instant::now(),
        pixel,
    }
}
/// The picture the uploaded capture holds, after its own quantization, so
/// the reference starts from the same pixels the converters read.
fn quantized(image: &Image) -> q::Picture {
    let (w, h) = (image.width as usize, image.height as usize);
    let rgb = match image.pixel {
        Pixel::Bgra8 => image
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| [b[2], b[1], b[0]].map(|v| f64::from(v) / 255.))
            .collect(),
        Pixel::RgbaF16 => image
            .bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| [0, 2, 4].map(|i| half::f16::from_le_bytes([b[i], b[i + 1]]).to_f64()))
            .collect(),
        Pixel::Rgba10Pq => image
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| {
                let v = u32::from_le_bytes(*b);
                [v & 1023, v >> 10 & 1023, v >> 20 & 1023].map(|c| f64::from(c) / 1023.)
            })
            .collect(),
    };
    q::Picture {
        width: w,
        height: h,
        rgb,
    }
}

fn negotiated(transfer: Transfer, width: u32, height: u32) -> Negotiated {
    Negotiated {
        width,
        height,
        codec: 1,
        hdr: transfer != Transfer::Sdr,
        // rubylight_protocol::hdr::HLG
        hdr_format: if transfer == Transfer::Hlg { 2 } else { 0 },
        // Rec.709, studio range.
        csc_mode: 2,
        ..Default::default()
    }
}
fn spec(config: &Negotiated, pixel: Pixel) -> q::Spec {
    let stream = butterpollo_core::amf_vpe::Stream::of(config);
    q::Spec {
        width: config.width as usize,
        height: config.height as usize,
        source: source_of(pixel),
        transfer: stream.transfer,
        matrix: stream.matrix,
        full_range: stream.full_range,
        ten_bit: stream.ten_bit,
    }
}

/// A test card in the stream's own terms: colour bars, a grey ramp, fine
/// lines one and two pixels wide and a zone plate, which aliasing shows on.
fn card(spec: &q::Spec, width: usize, height: usize) -> q::Picture {
    let bars: [[f64; 3]; 8] = [
        [0.75, 0.75, 0.75],
        [0.75, 0.75, 0.],
        [0., 0.75, 0.75],
        [0., 0.75, 0.],
        [0.75, 0., 0.75],
        [0.75, 0., 0.],
        [0., 0., 0.75],
        [1., 1., 1.],
    ];
    let mut rgb = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (x as f64 / width as f64, y as f64 / height as f64);
            let value = if v < 0.25 {
                bars[(u * 8.) as usize % 8]
            } else if v < 0.4 {
                [u; 3]
            } else if v < 0.55 {
                let line = if u < 0.5 { x % 2 } else { (x / 2) % 2 };
                [line as f64 * 0.8 + 0.1; 3]
            } else {
                let (cx, cy) = (u - 0.5, (v - 0.775) * height as f64 / width as f64);
                let phase = (cx * cx + cy * cy) * width as f64 * 3.;
                let s = 0.5 + 0.4 * phase.cos();
                [s, 0.5 + 0.3 * (phase * 0.5).sin(), 1. - s]
            };
            rgb.push(q::source_for(spec, value));
        }
    }
    q::Picture { width, height, rgb }
}

/// One converter's scores and times on one case.
fn score(spec: &q::Spec, codes: &[u16], reference: &q::Planes) -> Value {
    let Some(planes) = q::Planes::from_semi_planar(codes, spec.width, spec.height) else {
        return json!({"error": "short readback"});
    };
    let peak = spec.peak();
    let [mean, p99, worst] = q::delta_e(&planes, reference, spec);
    json!({
        "psnr_y": q::psnr(&planes.y, &reference.y, peak),
        "psnr_cb": q::psnr(&planes.cb, &reference.cb, peak),
        "psnr_cr": q::psnr(&planes.cr, &reference.cr, peak),
        "ssim_y": q::ssim(&planes.y, &reference.y, spec.width, spec.height, peak),
        "delta_e_itp_mean": mean,
        "delta_e_itp_p99": p99,
        "delta_e_itp_max": worst,
    })
}
fn times(mut micros: Vec<f64>) -> Value {
    if micros.is_empty() {
        return Value::Null;
    }
    micros.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f64| micros[((micros.len() - 1) as f64 * p).round() as usize];
    json!({
        "frames": micros.len(),
        "median_us": at(0.5),
        "p95_us": at(0.95),
        "worst_us": micros[micros.len() - 1],
        "mean_us": micros.iter().sum::<f64>() / micros.len() as f64,
    })
}
/// Chroma siting from a horizontal and a vertical ramp's planes.
fn siting(spec: &q::Spec, across: &[u16], down: &[u16]) -> Value {
    let planes = |codes: &[u16]| q::Planes::from_semi_planar(codes, spec.width, spec.height);
    let (Some(across), Some(down)) = (planes(across), planes(down)) else {
        return Value::Null;
    };
    let (cw, ch) = (spec.width / 2, spec.height / 2);
    let row: Vec<f64> = across.cr[(ch / 2) * cw..(ch / 2 + 1) * cw].to_vec();
    let column: Vec<f64> = (0..ch).map(|j| down.cr[j * cw + cw / 2]).collect();
    json!({
        // 0: co-sited with the left luma sample (chroma_sample_loc_type 0); 0.5: centred.
        "horizontal_luma_samples": q::siting(&row, |x| q::ramp_chroma(spec, true, x).1),
        // 0.5: vertically centred, as type 0 is.
        "vertical_luma_samples": q::siting(&column, |y| q::ramp_chroma(spec, false, y).1),
    })
}

const FRAMES: usize = 60;
const WARMUP: usize = 5;

/// Both converters on `picture`: scores, and per-frame times.
fn compare(
    gpu: &Device,
    runtime: &Runtime,
    acceleration: i64,
    name: &str,
    picture: &q::Picture,
    pixel: Pixel,
    config: &Negotiated,
) -> Result<Value> {
    let spec = spec(config, pixel);
    let upload = image(picture, pixel);
    let source = quantized(&upload);
    let reference = q::reference(&source, &spec);
    let capture = GpuImage::upload(gpu, &upload)?;
    let mut result = json!({
        "case": name,
        "source": [picture.width, picture.height],
        "stream": [config.width, config.height],
        "transfer": format!("{:?}", spec.transfer),
        "pixel": format!("{pixel:?}"),
    });
    // The shader conversion.
    let mut shader =
        crate::gpu_color::Converter::new(gpu, config, (capture.width, capture.height, pixel))?;
    let mut micros = vec![];
    let mut last = None;
    for frame in 0..WARMUP + FRAMES {
        let started = std::time::Instant::now();
        let texture = shader.convert(&capture)?;
        finish(gpu, &texture, 0)?;
        if frame >= WARMUP {
            micros.push(started.elapsed().as_secs_f64() * 1e6);
        }
        last = Some(texture);
    }
    let codes = read_codes(gpu, last.as_ref().unwrap(), 0, spec.width, spec.height)?;
    result["shader"] = score(&spec, &codes, &reference);
    result["shader"]["time"] = times(micros);
    // AMF's video converter.
    // SAFETY: `runtime` holds a live factory and D3D11 context that outlive `converter`.
    let made = unsafe { Vpe::new(runtime.factory, runtime.context, config) };
    let mut converter = match made {
        Ok(converter) => converter,
        Err(error) => {
            result["vpe"] = json!({"error": format!("{error:#}")});
            return Ok(result);
        }
    };
    converter.acceleration = acceleration;
    result["vpe_engine_reported"] = json!(converter.engine);
    let input = match converter.plan(&capture, [100., 1.], false) {
        Ok(input) => input,
        Err(refusal) => {
            result["vpe"] = json!({"refused": refusal.code()});
            return Ok(result);
        }
    };
    let mut ownership = gpu::Ownership::new();
    let mut micros = vec![];
    let mut codes = None;
    for frame in 0..WARMUP + FRAMES {
        let started = std::time::Instant::now();
        let converted = match converter.convert(&capture, input, &mut ownership) {
            Ok(converted) => converted,
            Err(error) => {
                result["vpe"] = json!({"error": format!("{error:#}")});
                return Ok(result);
            }
        };
        let (texture, index) = native(&converted.surface)?;
        finish(gpu, &texture, index)?;
        if frame >= WARMUP {
            micros.push(started.elapsed().as_secs_f64() * 1e6);
        }
        if frame + 1 == WARMUP + FRAMES {
            codes = Some(read_codes(gpu, &texture, index, spec.width, spec.height)?);
        }
    }
    result["vpe"] = score(&spec, codes.as_deref().unwrap_or_default(), &reference);
    result["vpe"]["time"] = times(micros);
    result["vpe"]["output"] = json!(format!("{:?}", converter.output().format));
    drop(converter);
    Ok(result)
}
/// Chroma siting of both converters for one stream kind, at native size.
fn siting_case(
    gpu: &Device,
    runtime: &Runtime,
    acceleration: i64,
    pixel: Pixel,
    config: &Negotiated,
) -> Result<Value> {
    let spec = spec(config, pixel);
    let mut out =
        json!({"transfer": format!("{:?}", spec.transfer), "pixel": format!("{pixel:?}")});
    let ramps = [q::ramp(&spec, true), q::ramp(&spec, false)].map(|ramp| image(&ramp, pixel));
    let captures = [
        GpuImage::upload(gpu, &ramps[0])?,
        GpuImage::upload(gpu, &ramps[1])?,
    ];
    let mut shader = crate::gpu_color::Converter::new(
        gpu,
        config,
        (captures[0].width, captures[0].height, pixel),
    )?;
    let mut codes = vec![];
    for capture in &captures {
        let texture = shader.convert(capture)?;
        codes.push(read_codes(gpu, &texture, 0, spec.width, spec.height)?);
    }
    out["shader"] = siting(&spec, &codes[0], &codes[1]);
    // SAFETY: `runtime` outlives `converter`.
    let mut converter = unsafe { Vpe::new(runtime.factory, runtime.context, config)? };
    converter.acceleration = acceleration;
    let mut ownership = gpu::Ownership::new();
    let mut codes = vec![];
    for capture in &captures {
        let Ok(input) = converter.plan(capture, [100., 1.], false) else {
            return Ok(out);
        };
        let converted = converter.convert(capture, input, &mut ownership)?;
        let (texture, index) = native(&converted.surface)?;
        codes.push(read_codes(gpu, &texture, index, spec.width, spec.height)?);
    }
    out["vpe"] = siting(&spec, &codes[0], &codes[1]);
    Ok(out)
}

/// Captures from `RUBYLIGHT_VPE_CAPTURES`: name, picture and pixel.
fn captures() -> Result<Vec<(String, q::Picture, Pixel)>> {
    let Ok(folder) = std::env::var("RUBYLIGHT_VPE_CAPTURES") else {
        return Ok(vec![]);
    };
    let mut found = vec![];
    for entry in std::fs::read_dir(&folder)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let meta: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let raw = path.with_extension("raw");
        let (Some(width), Some(height), Some(stride), Some(format)) = (
            meta["width"].as_u64(),
            meta["height"].as_u64(),
            meta["stride"].as_u64(),
            meta["format"].as_str(),
        ) else {
            continue;
        };
        let pixel = if format.starts_with("RGBA") {
            Pixel::RgbaF16
        } else if format.starts_with("BGRA") {
            Pixel::Bgra8
        } else if format.starts_with("RGB10A2") {
            Pixel::Rgba10Pq
        } else {
            continue;
        };
        let bytes = std::fs::read(&raw).with_context(|| format!("{}", raw.display()))?;
        let per = if pixel == Pixel::RgbaF16 { 8 } else { 4 };
        let (width, height, stride) = (width as usize, height as usize, stride as usize);
        let mut packed = Vec::with_capacity(width * height * per);
        for y in 0..height {
            packed.extend_from_slice(&bytes[y * stride..y * stride + width * per]);
        }
        let picture = quantized(&Image {
            width: width as u32,
            height: height as u32,
            stride: width * per,
            bytes: packed,
            captured: std::time::Instant::now(),
            pixel,
        });
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        found.push((name, picture, pixel));
    }
    Ok(found)
}

#[test]
#[ignore = "requires a Radeon with its AMF runtime; no display, encoder or host changes"]
fn vpe_ab() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let runtime = Runtime::new(&gpu)?;
    // `RUBYLIGHT_VPE_ACCELERATION=compute` asks AMF for its compute path,
    // to tell the VPE apart from the driver's own fallback.
    let acceleration = match std::env::var("RUBYLIGHT_VPE_ACCELERATION").as_deref() {
        Ok("compute") => vui::ACCELERATION_COMPUTE,
        _ => vui::ACCELERATION_VPE_PREFERRED,
    };
    let kinds = [
        (Transfer::Sdr, Pixel::Bgra8),
        (Transfer::Pq, Pixel::RgbaF16),
        (Transfer::Pq, Pixel::Rgba10Pq),
        (Transfer::Hlg, Pixel::RgbaF16),
    ];
    let mut cases = vec![];
    let mut sitings = vec![];
    for (transfer, pixel) in kinds {
        let native = negotiated(transfer, 1920, 1080);
        sitings.push(siting_case(&gpu, &runtime, acceleration, pixel, &native)?);
        // Native, the two common integer reductions, a fractional one, an
        // enlargement and a letterbox (21:9 on 16:9).
        for (source, target) in [
            ((1920, 1080), (1920, 1080)),
            ((3840, 2160), (1920, 1080)),
            ((3840, 2160), (1280, 720)),
            ((2560, 1440), (1920, 1080)),
            ((1280, 720), (1920, 1080)),
            ((3440, 1440), (1920, 1080)),
        ] {
            let config = negotiated(transfer, target.0, target.1);
            let picture = card(&spec(&config, pixel), source.0, source.1);
            let name = format!(
                "card {}x{} to {}x{}",
                source.0, source.1, target.0, target.1
            );
            let result = compare(
                &gpu,
                &runtime,
                acceleration,
                &name,
                &picture,
                pixel,
                &config,
            )?;
            eprintln!("{result}");
            cases.push(result);
        }
    }
    for (name, picture, pixel) in captures()? {
        let transfers: &[Transfer] = if pixel == Pixel::Bgra8 {
            &[Transfer::Sdr]
        } else {
            &[Transfer::Pq, Transfer::Hlg]
        };
        for &transfer in transfers {
            for target in [
                (picture.width as u32, picture.height as u32),
                (1920, 1080),
                (1280, 720),
            ] {
                if target.0 % 2 != 0 || target.1 % 2 != 0 {
                    continue;
                }
                let config = negotiated(transfer, target.0, target.1);
                let label = format!("capture {name} to {}x{}", target.0, target.1);
                let result = compare(
                    &gpu,
                    &runtime,
                    acceleration,
                    &label,
                    &picture,
                    pixel,
                    &config,
                )?;
                eprintln!("{result}");
                cases.push(result);
            }
        }
    }
    let report = json!({
        "scope": "offline conversion only: the same frames through the shader conversion and AMF's video converter, scored against a float reference; no encode",
        "adapter": gpu.display.adapter,
        "acceleration": if acceleration == vui::ACCELERATION_COMPUTE { "compute" } else { "vpe_preferred" },
        "frames_timed": FRAMES,
        "time": "CPU time from submitting a conversion to reading two of its pixels back, per frame",
        "siting": sitings,
        "cases": cases,
    });
    let text = serde_json::to_string_pretty(&report)?;
    match std::env::var("RUBYLIGHT_VPE_REPORT") {
        Ok(path) => std::fs::write(&path, &text).with_context(|| path.clone())?,
        Err(_) => println!("{text}"),
    }
    Ok(())
}
