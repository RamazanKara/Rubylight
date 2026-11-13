//! Draws hardware-decoded pictures straight into SDL's back buffer on Windows, instead of
//! handing SDL a video texture to draw.
//!
//! Ported from Moonlight V+ PC's D3D11 renderer (d3d11va.cpp): with bound decoder textures the
//! pass reads the picture from its slice of the decoder's texture array through shader resource
//! views, so the per-frame copy into the video texture goes away. V+ binds by default only on
//! Intel GPUs and copies elsewhere (moonlight-qt issue 1304); `NIGHTJAR_D3D11_BIND=1` or `=0`
//! overrides that, as V+'s `D3D11VA_FORCE_BIND` does. Without binding the pass reads the copy in
//! SDL's video texture. Its color conversion follows SDL's D3D11 shaders, so a picture looks the
//! same as SDL draws it, but its tone map takes the content peak per frame: from the stream's
//! Dolby Vision RPU (profiles 8.1 and 10.1 have an HDR10 base layer, drawn as PQ), else its
//! HDR10+ metadata, else measured on the picture, as V+'s libplacebo renderer does (plvk.cpp,
//! with libplacebo's peak detection defaults: 20 frames of smoothing, scene changes at 1 and
//! 3 dB). HLG pictures go through BT.2100's inverse OETF and OOTF for the display's peak, SDL
//! has no HLG. `NIGHTJAR_D3D11_PASS=0` leaves drawing to SDL, as before.

use std::ffi::c_void;

use windows::core::{s, Interface};
use windows::Win32::Graphics::Direct3D::{D3D11_SRV_DIMENSION_TEXTURE2DARRAY, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, DXGI_ERROR_WAS_STILL_DRAWING};

use super::d3d11::compile;

const VENDOR_INTEL: u32 = 0x8086;
const BINS: usize = 256;
/// The share of sampled pixels allowed above the measured peak (libplacebo's 99.995th percentile).
const ABOVE_PEAK: f64 = 0.00005;

/// Whether the pass draws the pictures (`NIGHTJAR_D3D11_PASS=0` leaves it to SDL).
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| pass_wanted(std::env::var("NIGHTJAR_D3D11_PASS").ok().as_deref()))
}

fn pass_wanted(setting: Option<&str>) -> bool {
    setting.map(str::trim) != Some("0")
}

/// Whether the decoder's textures are made readable by shaders, so the pass reads them in place.
pub fn bind_decoder_textures(device: &ID3D11Device) -> bool {
    // SAFETY: Plain DXGI queries on a live device.
    let vendor = unsafe { device.cast::<IDXGIDevice>().and_then(|d| d.GetAdapter()).and_then(|a| a.GetDesc()) }
        .map_or(0, |desc| desc.VendorId);
    let bind = binds(vendor, enabled(), std::env::var("NIGHTJAR_D3D11_BIND").ok().as_deref());
    eprintln!("nightjar: decoder texture access: {}", if bind { "bind" } else { "copy" });
    bind
}

fn binds(vendor: u32, pass: bool, setting: Option<&str>) -> bool {
    if !pass { return false; }
    match setting.map(str::trim) {
        Some("1") => true,
        Some("0") => false,
        _ => vendor == VENDOR_INTEL,
    }
}

/// The picture to draw: `texture` is an NV12 or P010 texture (array) and `slice` the picture's
/// slice in it.
#[derive(Clone, Copy)]
pub struct Picture {
    pub texture: *mut c_void,
    pub slice: u32,
    pub size: (u32, u32),
    pub ten_bit: bool,
    pub full_range: bool,
    /// 0 BT.601, 1 BT.709, 2 BT.2020.
    pub matrix: u32,
    pub transfer: Transfer,
}

/// How a picture's codes map to light: SDR gamma, PQ (SMPTE ST 2084) or HLG (ARIB STD-B67).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer { Sdr, Pq, Hlg }

/// What the back buffer wants: scRGB (`linear`) or sRGB, SDL's color scale (with the SDR white
/// level) and the display's headroom over SDR white.
#[derive(Clone, Copy)]
pub struct Output {
    pub linear: bool,
    pub color_scale: f32,
    pub headroom: f32,
}

/// What the tone map knows of an HDR picture: the SDR white in nits, the frame's Dolby Vision and
/// HDR10+ peaks and the stream's static peak (MaxCLL or the mastering display's), in nits.
#[derive(Clone, Copy, Default)]
pub struct Tone {
    pub white_nits: f32,
    pub dolby_vision: Option<f32>,
    pub hdr10_plus: Option<f32>,
    pub static_peak: Option<f32>,
}

/// Where the tone map's content peak came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PeakSource { DolbyVision, Hdr10Plus, Measured, Static, Hlg, None }

impl PeakSource {
    fn describe(self) -> &'static str {
        match self {
            Self::DolbyVision => "the stream's Dolby Vision metadata",
            Self::Hdr10Plus => "the stream's HDR10+ metadata",
            Self::Measured => "the peak measured on the pictures",
            Self::Static => "the stream's static HDR metadata",
            Self::Hlg => "the display's peak (HLG)",
            Self::None => "nothing (no tone mapping)",
        }
    }
}

/// The content peak in nits: Dolby Vision first, then HDR10+, then the measured peak (never
/// above the static one), then the static peak.
pub fn content_peak(dolby_vision: Option<f32>, hdr10_plus: Option<f32>, measured: Option<f32>, static_peak: Option<f32>) -> (Option<f32>, PeakSource) {
    let usable = |peak: Option<f32>| peak.filter(|p| p.is_finite() && *p > 0.0);
    if let Some(peak) = usable(dolby_vision) { return (Some(peak), PeakSource::DolbyVision); }
    if let Some(peak) = usable(hdr10_plus) { return (Some(peak), PeakSource::Hdr10Plus); }
    if let Some(peak) = usable(measured) {
        return (Some(usable(static_peak).map_or(peak, |s| peak.min(s))), PeakSource::Measured);
    }
    match usable(static_peak) {
        Some(peak) => (Some(peak), PeakSource::Static),
        None => (None, PeakSource::None),
    }
}

/// SDL's "Chrome" tone map factors for content `peak` nits over `white_nits` on a display with
/// `headroom` (SDL_render_d3d11.c), or None when the content fits.
pub fn tonemap_factors(peak: f32, white_nits: f32, headroom: f32) -> Option<(f32, f32)> {
    let input = peak / white_nits;
    (input.is_finite() && headroom > 0.0 && input > headroom).then(|| (headroom / (input * input), 1.0 / headroom))
}

/// Nits of a PQ code value (SMPTE ST 2084).
pub fn pq_nits(code: f64) -> f64 {
    let p = code.clamp(0.0, 1.0).powf(1.0 / 78.84375);
    10000.0 * ((p - 0.8359375).max(0.0) / (18.8515625 - 18.6875 * p)).powf(1.0 / 0.1593017578125)
}

pub use super::hlg::hlg_display;
#[cfg(test)]
pub use super::hlg::{hlg_nits, HlgDisplay};

/// The peak in nits of a maxRGB histogram over PQ codes: the top of the bin under which all but
/// `ABOVE_PEAK` of the samples lie. None for an empty histogram.
pub fn histogram_peak(histogram: &[u32]) -> Option<f32> {
    let total: u64 = histogram.iter().map(|&n| u64::from(n)).sum();
    if total == 0 { return None; }
    let allowed = (total as f64 * ABOVE_PEAK) as u64;
    let mut above = 0u64;
    for (bin, &count) in histogram.iter().enumerate().rev() {
        above += u64::from(count);
        if above > allowed { return Some(pq_nits((bin + 1) as f64 / histogram.len() as f64) as f32); }
    }
    None
}

/// Smooths the measured peak over frames and follows scene changes at once.
#[derive(Default)]
pub struct PeakTracker { smoothed: Option<f32> }

impl PeakTracker {
    const PERIOD: f32 = 20.0;
    const SCENE_LOW_DB: f32 = 1.0;
    const SCENE_HIGH_DB: f32 = 3.0;

    pub fn update(&mut self, measured: f32) -> f32 {
        let next = match self.smoothed {
            None => measured,
            Some(smoothed) => {
                let db = 10.0 * (measured.max(smoothed) / measured.min(smoothed).max(1e-3)).log10();
                let slow = 1.0 / Self::PERIOD;
                let weight = slow + (1.0 - slow) * ((db - Self::SCENE_LOW_DB) / (Self::SCENE_HIGH_DB - Self::SCENE_LOW_DB)).clamp(0.0, 1.0);
                smoothed + (measured - smoothed) * weight
            }
        };
        self.smoothed = Some(next);
        next
    }

    pub fn value(&self) -> Option<f32> { self.smoothed }
}

/// The luma and chroma views of a picture.
type Planes = [Option<ID3D11ShaderResourceView>; 2];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    picture: [f32; 2],
    texture: [f32; 2],
    slice: f32,
    flags: u32,
    sdr_white: f32,
    color_scale: f32,
    tonemap: [f32; 2],
    /// The HLG display's peak in nits and system gamma.
    hlg: [f32; 2],
    /// Frame Warp's rows (`WarpDraw::row`, `WarpDraw::lens_row`).
    warp: [f32; 4],
    warp_lens: [f32; 4],
}

fn flags(picture: &Picture, output: &Output, tonemap: bool) -> u32 {
    u32::from(picture.ten_bit) | u32::from(picture.full_range) << 1 | (picture.matrix & 3) << 2
        | u32::from(picture.transfer == Transfer::Pq) << 4 | u32::from(output.linear) << 5 | u32::from(tonemap) << 6
        | u32::from(picture.transfer == Transfer::Hlg) << 7
}

/// The pass. It records on a deferred context, so SDL's cached immediate-context state holds.
pub struct Direct {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    compute: ID3D11ComputeShader,
    sampler: [Option<ID3D11SamplerState>; 1],
    constants: [Option<ID3D11Buffer>; 1],
    histogram: ID3D11Buffer,
    histogram_view: ID3D11UnorderedAccessView,
    /// Read-back copies of the histogram, measured in turn and read once the GPU is done.
    staging: [ID3D11Buffer; 3],
    pending: std::collections::VecDeque<usize>,
    /// The texture the views are for, and its views per slice.
    views: Option<(usize, bool, std::collections::HashMap<u32, Planes>)>,
    tracker: PeakTracker,
    source: Option<PeakSource>,
    /// The view of SDL's back buffer, for the swap chain and output size it was made at. It is
    /// let go before SDL resizes the buffers (a size change, or a frame the pass does not draw):
    /// ResizeBuffers fails while a buffer is still referenced.
    back_buffer: Option<(usize, (i32, i32), ID3D11RenderTargetView)>,
    /// Frame Warp's HUD mask (`t2`).
    mask: super::frame_warp::D3d11Mask,
}

impl Direct {
    /// # Safety
    /// `device` must be the renderer's live device, used on this thread.
    pub unsafe fn new(device: &ID3D11Device) -> Result<Self, String> {
        fn fail(what: &'static str) -> impl Fn(windows::core::Error) -> String { move |e| format!("{what}: {e}") }
        let source = include_bytes!("../direct.hlsl");
        let blob = |code: &windows::Win32::Graphics::Direct3D::ID3DBlob| std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize()).to_vec();
        let vertex_code = blob(&compile(source, s!("vertex"), s!("vs_5_0")).map_err(fail("the vertex shader does not compile"))?);
        let pixel_code = blob(&compile(source, s!("draw"), s!("ps_5_0")).map_err(fail("the pixel shader does not compile"))?);
        let compute_code = blob(&compile(source, s!("peak"), s!("cs_5_0")).map_err(fail("the peak shader does not compile"))?);
        let (mut vertex, mut pixel, mut compute) = (None, None, None);
        device.CreateVertexShader(&vertex_code, None, Some(&mut vertex)).map_err(fail("CreateVertexShader"))?;
        device.CreatePixelShader(&pixel_code, None, Some(&mut pixel)).map_err(fail("CreatePixelShader"))?;
        device.CreateComputeShader(&compute_code, None, Some(&mut compute)).map_err(fail("CreateComputeShader"))?;
        let mut sampler = None;
        device.CreateSamplerState(&D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, AddressV: D3D11_TEXTURE_ADDRESS_CLAMP, AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_NEVER, MaxLOD: f32::MAX, ..Default::default()
        }, Some(&mut sampler)).map_err(fail("CreateSamplerState"))?;
        let mut constants = None;
        device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: std::mem::size_of::<Constants>() as u32, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() }, None, Some(&mut constants)).map_err(fail("the constant buffer"))?;
        let bytes = (BINS * 4) as u32;
        let mut histogram = None;
        device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: bytes, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32, ..Default::default() }, None, Some(&mut histogram)).map_err(fail("the histogram"))?;
        let histogram = histogram.unwrap();
        let mut histogram_view = None;
        device.CreateUnorderedAccessView(&histogram, Some(&D3D11_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS, ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
            Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 { Buffer: D3D11_BUFFER_UAV { FirstElement: 0, NumElements: BINS as u32, Flags: D3D11_BUFFER_UAV_FLAG_RAW.0 as u32 } },
        }), Some(&mut histogram_view)).map_err(fail("the histogram view"))?;
        let staging = |_| {
            let mut buffer = None;
            device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: bytes, Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..Default::default() },
                None, Some(&mut buffer)).map(|_| buffer.unwrap()).map_err(fail("the histogram read-back"))
        };
        let staging = [staging(0)?, staging(1)?, staging(2)?];
        let mut deferred = None;
        device.CreateDeferredContext(0, Some(&mut deferred)).map_err(fail("CreateDeferredContext"))?;
        Ok(Self {
            device: device.clone(),
            context: device.GetImmediateContext().map_err(fail("GetImmediateContext"))?,
            deferred: deferred.unwrap(),
            vertex: vertex.unwrap(),
            pixel: pixel.unwrap(),
            compute: compute.unwrap(),
            sampler: [sampler],
            constants: [constants],
            histogram,
            histogram_view: histogram_view.unwrap(),
            staging,
            pending: Default::default(),
            views: None,
            tracker: PeakTracker::default(),
            source: None,
            back_buffer: None,
            mask: Default::default(),
        })
    }

    /// Forgets the stream's measured peak and views (a new picture stream follows).
    pub fn reset(&mut self) {
        self.tracker = PeakTracker::default();
        self.source = None;
        self.views = None;
        self.back_buffer = None;
    }

    /// Lets go of SDL's back buffer, for a frame the pass does not draw (SDL may resize then).
    pub fn release_back_buffer(&mut self) {
        self.back_buffer = None;
    }

    /// Converts `picture` to RGB in `target` (its whole size), for what SDL draws or upscales
    /// instead of a picture its own shaders would decode wrongly: 10-bit SDR as gamma-coded RGB,
    /// HLG as linear light relative to SDR white (scRGB) for a display with `headroom` over SDR
    /// white at `white_nits`.
    ///
    /// # Safety
    /// As `draw`.
    pub unsafe fn convert(&mut self, target: &ID3D11RenderTargetView, size: (u32, u32), picture: &Picture,
        white_nits: f32, headroom: f32, lock: impl FnOnce(&mut dyn FnMut())) -> Result<(), String> {
        let hlg = picture.transfer == Transfer::Hlg;
        let output = Output { linear: hlg, color_scale: 1.0, headroom: if hlg { headroom } else { 1.0 } };
        let tone = Tone { white_nits, ..Tone::default() };
        self.draw(target, [0.0, 0.0, size.0 as f32, size.1 as f32], picture, output, tone, lock)
    }

    /// The smoothed measured peak in nits, once one is in.
    #[cfg(test)]
    pub fn measured_peak(&self) -> Option<f32> { self.tracker.value() }

    /// The luma and chroma views of `picture`'s slice.
    unsafe fn views(&mut self, picture: &Picture) -> Result<Planes, String> {
        let key = picture.texture as usize;
        if !self.views.as_ref().is_some_and(|(texture, ten, _)| *texture == key && *ten == picture.ten_bit) {
            self.views = Some((key, picture.ten_bit, Default::default()));
        }
        let (_, _, views) = self.views.as_mut().unwrap();
        if let Some(found) = views.get(&picture.slice) { return Ok(found.clone()); }
        let texture = ID3D11Texture2D::from_raw_borrowed(&picture.texture).ok_or("no picture texture")?;
        let formats = if picture.ten_bit { [DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM] } else { [DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM] };
        let mut made: Planes = [None, None];
        for (view, format) in made.iter_mut().zip(formats) {
            self.device.CreateShaderResourceView(texture, Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: format, ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                    MostDetailedMip: 0, MipLevels: 1, FirstArraySlice: picture.slice, ArraySize: 1 } },
            }), Some(view)).map_err(|e| format!("cannot read slice {} of the picture texture: {e}", picture.slice))?;
        }
        views.insert(picture.slice, made.clone());
        Ok(made)
    }

    /// Reads the histograms the GPU has finished, oldest first, into the tracker.
    unsafe fn collect(&mut self) {
        while let Some(&slot) = self.pending.front() {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            match self.context.Map(&self.staging[slot], 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut mapped)) {
                Ok(()) => {
                    // Read once, into the stack: no allocation per picture.
                    let mut counts = [0u32; BINS];
                    counts.copy_from_slice(std::slice::from_raw_parts(mapped.pData.cast::<u32>(), BINS));
                    self.context.Unmap(&self.staging[slot], 0);
                    self.pending.pop_front();
                    if let Some(peak) = histogram_peak(&counts) { self.tracker.update(peak); }
                }
                Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => break,
                Err(e) => {
                    eprintln!("nightjar: reading the HDR peak histogram failed: {e}");
                    self.pending.pop_front();
                }
            }
        }
    }

    /// Draws `picture` into `target` at `viewport` (x, y, width, height). `lock` runs the
    /// submission (the decoder's context lock when the texture is the decoder's).
    ///
    /// # Safety
    /// The picture texture and the target must belong to this pass's device, on this thread.
    pub unsafe fn draw(&mut self, target: &ID3D11RenderTargetView, viewport: [f32; 4], picture: &Picture,
        output: Output, tone: Tone, lock: impl FnOnce(&mut dyn FnMut())) -> Result<(), String> {
        self.draw_warped(target, viewport, picture, output, tone, &super::frame_warp::WarpDraw::default(), lock)
    }

    /// `draw` with the picture shifted by Frame Warp's `warp`.
    ///
    /// # Safety
    /// As `draw`.
    #[allow(clippy::too_many_arguments)] // `draw`'s arguments plus the warp
    unsafe fn draw_warped(&mut self, target: &ID3D11RenderTargetView, viewport: [f32; 4], picture: &Picture,
        output: Output, tone: Tone, warp: &super::frame_warp::WarpDraw, lock: impl FnOnce(&mut dyn FnMut())) -> Result<(), String> {
        let planes = self.views(picture)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        ID3D11Texture2D::from_raw_borrowed(&picture.texture).ok_or("no picture texture")?.GetDesc(&mut desc);
        let texture_size = (desc.Width, desc.Height);
        self.collect();
        let pq = picture.transfer == Transfer::Pq;
        let measure = pq && tone.dolby_vision.is_none() && tone.hdr10_plus.is_none();
        let hlg = hlg_display(output.headroom, tone.white_nits);
        let (peak, white, source) = match picture.transfer {
            Transfer::Pq => {
                let (peak, source) = content_peak(tone.dolby_vision, tone.hdr10_plus, self.tracker.value(), tone.static_peak);
                (peak, tone.white_nits, source)
            }
            Transfer::Hlg => (Some(hlg.peak), hlg.white, PeakSource::Hlg),
            Transfer::Sdr => (None, tone.white_nits, PeakSource::None),
        };
        if picture.transfer != Transfer::Sdr && self.source != Some(source) {
            let peak = peak.map_or(String::new(), |peak| format!(", peak {peak:.0} nits"));
            eprintln!("nightjar: HDR tone mapping follows {}{peak}", source.describe());
            self.source = Some(source);
        }
        let factors = peak.and_then(|peak| tonemap_factors(peak, white, output.headroom));
        let mask = self.mask.view(&self.device, warp);
        let constants = Constants {
            picture: [picture.size.0 as f32, picture.size.1 as f32],
            texture: [texture_size.0 as f32, texture_size.1 as f32],
            slice: picture.slice as f32,
            flags: flags(picture, &output, factors.is_some()),
            sdr_white: white.max(1e-3),
            color_scale: output.color_scale,
            tonemap: factors.map_or([0.0; 2], |(a, b)| [a, b]),
            hlg: [hlg.peak, hlg.gamma],
            warp: warp.row(mask.is_some()),
            warp_lens: warp.lens_row(),
        };
        let c = &self.deferred;
        c.ClearState();
        c.UpdateSubresource(self.constants[0].as_ref().unwrap(), 0, None, std::ptr::from_ref(&constants).cast(), 0, 0);
        let free = (0..self.staging.len()).find(|slot| !self.pending.contains(slot));
        if let (true, Some(slot)) = (measure, free) {
            c.ClearUnorderedAccessViewUint(&self.histogram_view, &[0; 4]);
            c.CSSetShader(&self.compute, None);
            c.CSSetShaderResources(0, Some(&planes));
            c.CSSetConstantBuffers(0, Some(&self.constants));
            let bound = [Some(self.histogram_view.clone())];
            c.CSSetUnorderedAccessViews(0, 1, Some(bound.as_ptr()), None);
            c.Dispatch(picture.size.0.div_ceil(32), picture.size.1.div_ceil(32), 1);
            let unbound: [Option<ID3D11UnorderedAccessView>; 1] = [None];
            c.CSSetUnorderedAccessViews(0, 1, Some(unbound.as_ptr()), None);
            c.CSSetShaderResources(0, Some(&[None, None]));
            c.CopyResource(&self.staging[slot], &self.histogram);
            self.pending.push_back(slot);
        }
        c.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
        c.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: viewport[0], TopLeftY: viewport[1], Width: viewport[2], Height: viewport[3], MinDepth: 0.0, MaxDepth: 1.0 }]));
        c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        c.VSSetShader(&self.vertex, None);
        c.VSSetConstantBuffers(0, Some(&self.constants));
        c.PSSetShader(&self.pixel, None);
        c.PSSetShaderResources(0, Some(&planes));
        if mask.is_some() { c.PSSetShaderResources(2, Some(&[mask])); }
        c.PSSetSamplers(0, Some(&self.sampler));
        c.PSSetConstantBuffers(0, Some(&self.constants));
        c.Draw(3, 0);
        let mut commands = None;
        c.FinishCommandList(false, Some(&mut commands)).map_err(|e| format!("FinishCommandList: {e}"))?;
        let commands = commands.ok_or("no command list")?;
        let context = &self.context;
        lock(&mut || context.ExecuteCommandList(&commands, true));
        Ok(())
    }

    /// Draws `picture` into SDL's back buffer at `destination` (pixels). Call after SDL's clear
    /// and before the overlay.
    ///
    /// # Safety
    /// `renderer` is the live D3D11 renderer of this thread, whose device made this pass.
    pub unsafe fn present(&mut self, renderer: *mut sdl3_sys::everything::SDL_Renderer, destination: &sdl3_sys::everything::SDL_FRect,
        picture: &Picture, tone: Tone, warp: &super::frame_warp::WarpDraw, lock: impl FnOnce(&mut dyn FnMut())) -> Result<(), String> {
        use sdl3_sys::everything::*;
        let properties = SDL_GetRendererProperties(renderer);
        let swapchain_raw = SDL_GetPointerProperty(properties, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER, std::ptr::null_mut());
        let mut size = (0, 0);
        SDL_GetCurrentRenderOutputSize(renderer, &mut size.0, &mut size.1);
        // A new size is resized by SDL's next command run, the flush below: let go first.
        if !self.back_buffer.as_ref().is_some_and(|(swapchain, at, _)| *swapchain == swapchain_raw as usize && *at == size) {
            self.back_buffer = None;
        }
        // SDL's clear has to land before the picture (it runs SDL's queue; no GPU flush).
        if !SDL_FlushRenderer(renderer) { return Err(format!("SDL_FlushRenderer: {}", crate::sdl::error())); }
        if self.back_buffer.is_none() {
            let swapchain = windows::Win32::Graphics::Dxgi::IDXGISwapChain::from_raw_borrowed(&swapchain_raw).ok_or("the renderer has no swap chain")?;
            let buffer = swapchain.GetBuffer::<ID3D11Texture2D>(0).map_err(|e| format!("the back buffer: {e}"))?;
            let mut target = None;
            self.device.CreateRenderTargetView(&buffer, None, Some(&mut target)).map_err(|e| format!("the back buffer's view: {e}"))?;
            self.back_buffer = Some((swapchain_raw as usize, size, target.ok_or("no back buffer view")?));
        }
        let target = self.back_buffer.as_ref().unwrap().2.clone();
        let mut scale = 1.0;
        SDL_GetRenderColorScale(renderer, &mut scale);
        let output = Output {
            linear: SDL_GetNumberProperty(properties, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, i64::from(SDL_COLORSPACE_SRGB.0)) == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0),
            color_scale: scale * SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0),
            headroom: SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0),
        };
        let viewport = [destination.x, destination.y, destination.w, destination.h];
        self.draw_warped(&target, viewport, picture, output, tone, warp, lock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constants fill the shader's `Draw` block, Frame Warp's rows last.
    #[test]
    fn the_constants_end_with_frame_warps_rows() {
        assert_eq!(std::mem::size_of::<Constants>(), 80);
        assert_eq!(std::mem::offset_of!(Constants, warp), 48);
        assert_eq!(std::mem::offset_of!(Constants, warp_lens), 64);
        let shader = include_str!("../direct.hlsl");
        let gamma = shader.find("float hlgGamma;").expect("the HLG row");
        let warp = shader.find("float4 warp;").expect("the warp row");
        assert!(gamma < warp && !shader[gamma + "float hlgGamma;".len()..warp].contains(';'), "the warp row follows the HLG row");
        let lens = shader[warp..].trim_start_matches("float4 warp;").trim_start();
        assert!(lens.starts_with("float4 warpLens;") && lens.trim_start_matches("float4 warpLens;").trim_start().starts_with("};"),
            "the lens row follows it and ends the block");
        assert!(shader.contains("Texture2D<float> hudMask : register(t2);"));
    }

    #[test]
    fn the_switches_follow_v_plus() {
        assert!(pass_wanted(None) && pass_wanted(Some("1")) && !pass_wanted(Some("0")) && !pass_wanted(Some(" 0 ")));
        assert!(binds(VENDOR_INTEL, true, None), "V+ binds on Intel");
        assert!(!binds(0x1002, true, None) && !binds(0x10de, true, None), "and copies elsewhere");
        assert!(binds(0x1002, true, Some("1")) && !binds(VENDOR_INTEL, true, Some("0")), "unless told");
        assert!(!binds(VENDOR_INTEL, false, Some("1")), "without the pass nothing reads the decoder's textures");
    }

    #[test]
    fn dolby_vision_wins_then_hdr10_plus_then_the_measured_peak_then_the_static_one() {
        assert_eq!(content_peak(Some(450.0), Some(600.0), Some(900.0), Some(1000.0)), (Some(450.0), PeakSource::DolbyVision));
        assert_eq!(content_peak(None, Some(600.0), Some(900.0), Some(1000.0)), (Some(600.0), PeakSource::Hdr10Plus));
        assert_eq!(content_peak(None, None, Some(900.0), Some(1000.0)), (Some(900.0), PeakSource::Measured));
        assert_eq!(content_peak(None, None, Some(1500.0), Some(1000.0)), (Some(1000.0), PeakSource::Measured), "never above the static peak");
        assert_eq!(content_peak(None, Some(0.0), None, Some(1000.0)), (Some(1000.0), PeakSource::Static), "an empty HDR10+ peak is no peak");
        assert_eq!(content_peak(Some(f32::NAN), Some(600.0), None, None), (Some(600.0), PeakSource::Hdr10Plus), "nor an unreadable RPU peak");
        assert_eq!(content_peak(None, None, None, None), (None, PeakSource::None));
    }

    #[test]
    fn hlg_follows_bt2100() {
        // The 1000-nit reference display: gamma 1.2, 75% HLG (reference white) at 203 nits
        // (BT.2408), full code at the peak, black at 0.
        let reference = hlg_display(1.0, 80.0);
        assert_eq!(reference, HlgDisplay { peak: 1000.0, gamma: 1.2, white: 203.0 }, "an SDR output");
        let white = hlg_nits([0.75; 3], reference);
        assert!(white.iter().all(|n| (n - 203.0).abs() < 1.5), "{white:?}");
        assert!(hlg_nits([1.0; 3], reference).iter().all(|n| (n - 1000.0).abs() < 0.1));
        assert_eq!(hlg_nits([0.0; 3], reference), [0.0; 3]);
        // The lower half is the square-root part: 0.5 is a twelfth of scene white.
        let half = hlg_nits([0.5; 3], HlgDisplay { peak: 1000.0, gamma: 1.0, white: 203.0 });
        assert!((half[0] - 1000.0 / 12.0).abs() < 1e-6, "{half:?}");
        // An HDR display with 4x headroom over a 200-nit SDR white: rendered for its 800 nits.
        let hdr = hlg_display(4.0, 200.0);
        assert_eq!((hdr.peak, hdr.white), (800.0, 200.0));
        assert!((hdr.gamma - (1.2 + 0.42 * 0.8f32.log10())).abs() < 1e-6);
        assert!(hlg_nits([1.0; 3], hdr).iter().all(|n| (n - 800.0).abs() < 0.1), "full code lands on the display's peak");
        assert_eq!(tonemap_factors(hdr.peak, hdr.white, 4.0), None, "so it needs no tone map");
        // A dim display does not get a gamma below 1; a broken headroom is the reference display.
        assert_eq!(hlg_display(1.5, 100.0).gamma, 1.0);
        assert_eq!(hlg_display(f32::INFINITY, 200.0).peak, 1000.0);
        // The OOTF keeps the colour's ratios: a red is as red as it was in scene light.
        let red = hlg_nits([0.9, 0.3, 0.3], reference);
        assert!(red[0] > 5.0 * red[1] && (red[1] - red[2]).abs() < 1e-9);
    }

    #[test]
    fn transfers_set_their_own_shader_flags() {
        let picture = |transfer| Picture { texture: std::ptr::null_mut(), slice: 0, size: (1, 1), ten_bit: true, full_range: false, matrix: 2, transfer };
        let output = Output { linear: true, color_scale: 1.0, headroom: 4.0 };
        assert_eq!(flags(&picture(Transfer::Pq), &output, false) & (1 << 4 | 1 << 7), 1 << 4);
        assert_eq!(flags(&picture(Transfer::Hlg), &output, false) & (1 << 4 | 1 << 7), 1 << 7, "HLG is never drawn as PQ");
        assert_eq!(flags(&picture(Transfer::Sdr), &output, false) & (1 << 4 | 1 << 7), 0);
    }

    #[test]
    fn the_tone_map_is_sdls() {
        // SDL: factor1 = output / input², factor2 = 1 / output, only when the input exceeds the output.
        let (a, b) = tonemap_factors(1000.0, 200.0, 2.5).unwrap();
        assert!((a - 2.5 / 25.0).abs() < 1e-6 && (b - 0.4).abs() < 1e-6);
        assert_eq!(tonemap_factors(400.0, 200.0, 2.5), None);
        assert_eq!(tonemap_factors(1000.0, 200.0, 0.0), None);
        // The content peak lands on the display's peak.
        let peak = 5.0f32;
        assert!((peak * (1.0 + a * peak) / (1.0 + b * peak) - 2.5).abs() < 1e-5);
    }

    #[test]
    fn the_histogram_peak_ignores_a_few_stray_pixels() {
        assert_eq!(histogram_peak(&[0; BINS]), None);
        let mut counts = [0u32; BINS];
        counts[100] = 1_000_000;
        counts[250] = 10;
        let peak = histogram_peak(&counts).unwrap();
        assert!((f64::from(peak) - pq_nits(101.0 / 256.0)).abs() < 1e-3, "{peak}");
        counts[250] = 1000;
        assert!(histogram_peak(&counts).unwrap() > 5000.0, "1 in 1000 is too many to ignore");
        assert!((pq_nits(0.508) - 100.0).abs() < 1.0 && (pq_nits(0.7518) - 1000.0).abs() < 5.0);
    }

    #[test]
    fn the_measured_peak_drifts_within_a_scene_and_jumps_at_a_cut() {
        let mut tracker = PeakTracker::default();
        assert_eq!(tracker.update(1000.0), 1000.0);
        let drift = tracker.update(1100.0);
        assert!((drift - 1005.0).abs() < 0.01, "0.4 dB moves a twentieth: {drift}");
        assert_eq!(tracker.update(4000.0), 4000.0, "6 dB is a new scene");
        let between = PeakTracker { smoothed: Some(1000.0) }.clone_update(1585.0);
        assert!(between > 1100.0 && between < 1585.0, "2 dB moves part of the way: {between}");
    }

    impl PeakTracker {
        fn clone_update(mut self, measured: f32) -> f32 { self.update(measured) }
    }

    #[test]
    fn a_frames_hdr10_plus_peak_is_its_largest_maxscl() {
        use crate::media::ffi::*;
        let frame = crate::media::Frame::wavelet(16, 16).unwrap();
        assert_eq!(frame.hdr10_plus_peak(), None, "no side data");
        // SAFETY: Side data of the right size on a live frame.
        unsafe {
            let side = av_frame_new_side_data(frame.raw.as_ptr(), AV_FRAME_DATA_DYNAMIC_HDR_PLUS, std::mem::size_of::<AVDynamicHDRPlus>());
            let metadata = &mut *(*side).data.cast::<AVDynamicHDRPlus>();
            std::ptr::write_bytes(metadata, 0, 1);
            assert_eq!(frame.hdr10_plus_peak(), None, "no processing window");
            metadata.num_windows = 1;
            assert_eq!(frame.hdr10_plus_peak(), None, "a zero peak is no peak");
            // FFmpeg keeps MaxSCL in units of 100000 cd/m² over 1 (0.1 cd/m² codes over 100000).
            metadata.params[0].maxscl = [AVRational { num: 4000, den: 100000 }, AVRational { num: 9000, den: 100000 }, AVRational { num: 2500, den: 100000 }];
        }
        assert_eq!(frame.hdr10_plus_peak(), Some(900.0));
    }
}

#[cfg(test)]
mod gpu_tests;
