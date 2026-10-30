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
//! HDR10+ metadata, else measured on the picture, as V+'s libplacebo renderer does (plvk.cpp,
//! with libplacebo's peak detection defaults: 20 frames of smoothing, scene changes at 1 and
//! 3 dB). `NIGHTJAR_D3D11_PASS=0` leaves drawing to SDL, as before.

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
    /// The picture carries the PQ transfer.
    pub pq: bool,
}

/// What the back buffer wants: scRGB (`linear`) or sRGB, SDL's color scale (with the SDR white
/// level) and the display's headroom over SDR white.
#[derive(Clone, Copy)]
pub struct Output {
    pub linear: bool,
    pub color_scale: f32,
    pub headroom: f32,
}

/// What the tone map knows of an HDR picture: the SDR white in nits, the frame's HDR10+ peak and
/// the stream's static peak (MaxCLL or the mastering display's), in nits.
#[derive(Clone, Copy, Default)]
pub struct Tone {
    pub white_nits: f32,
    pub hdr10_plus: Option<f32>,
    pub static_peak: Option<f32>,
}

/// Where the tone map's content peak came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PeakSource { Hdr10Plus, Measured, Static, None }

impl PeakSource {
    fn describe(self) -> &'static str {
        match self {
            Self::Hdr10Plus => "the stream's HDR10+ metadata",
            Self::Measured => "the peak measured on the pictures",
            Self::Static => "the stream's static HDR metadata",
            Self::None => "nothing (no tone mapping)",
        }
    }
}

/// The content peak in nits: HDR10+ first, then the measured peak (never above the static one),
/// then the static peak.
pub fn content_peak(hdr10_plus: Option<f32>, measured: Option<f32>, static_peak: Option<f32>) -> (Option<f32>, PeakSource) {
    let usable = |peak: Option<f32>| peak.filter(|p| p.is_finite() && *p > 0.0);
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
    padding: [f32; 2],
}

fn flags(picture: &Picture, output: &Output, tonemap: bool) -> u32 {
    u32::from(picture.ten_bit) | u32::from(picture.full_range) << 1 | (picture.matrix & 3) << 2
        | u32::from(picture.pq) << 4 | u32::from(output.linear) << 5 | u32::from(tonemap) << 6
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

    /// Converts `picture` to gamma-coded RGB in `target` (its whole size), for what SDL draws
    /// or upscales instead of a picture its own shaders would decode wrongly (10-bit SDR).
    ///
    /// # Safety
    /// As `draw`.
    pub unsafe fn convert(&mut self, target: &ID3D11RenderTargetView, size: (u32, u32), picture: &Picture,
        lock: impl FnOnce(&mut dyn FnMut())) -> Result<(), String> {
        let output = Output { linear: false, color_scale: 1.0, headroom: 1.0 };
        self.draw(target, [0.0, 0.0, size.0 as f32, size.1 as f32], picture, output, Tone::default(), lock)
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
                    let counts = std::slice::from_raw_parts(mapped.pData.cast::<u32>(), BINS).to_vec();
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
        let planes = self.views(picture)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        ID3D11Texture2D::from_raw_borrowed(&picture.texture).ok_or("no picture texture")?.GetDesc(&mut desc);
        let texture_size = (desc.Width, desc.Height);
        self.collect();
        let measure = picture.pq && tone.hdr10_plus.is_none();
        let (peak, source) = if picture.pq { content_peak(tone.hdr10_plus, self.tracker.value(), tone.static_peak) } else { (None, PeakSource::None) };
        if picture.pq && self.source != Some(source) {
            eprintln!("nightjar: HDR tone mapping follows {}", source.describe());
            self.source = Some(source);
        }
        let factors = peak.and_then(|peak| tonemap_factors(peak, tone.white_nits, output.headroom));
        let constants = Constants {
            picture: [picture.size.0 as f32, picture.size.1 as f32],
            texture: [texture_size.0 as f32, texture_size.1 as f32],
            slice: picture.slice as f32,
            flags: flags(picture, &output, factors.is_some()),
            sdr_white: tone.white_nits.max(1e-3),
            color_scale: output.color_scale,
            tonemap: factors.map_or([0.0; 2], |(a, b)| [a, b]),
            padding: [0.0; 2],
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
        picture: &Picture, tone: Tone, lock: impl FnOnce(&mut dyn FnMut())) -> Result<(), String> {
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
        self.draw(&target, viewport, picture, output, tone, lock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switches_follow_v_plus() {
        assert!(pass_wanted(None) && pass_wanted(Some("1")) && !pass_wanted(Some("0")) && !pass_wanted(Some(" 0 ")));
        assert!(binds(VENDOR_INTEL, true, None), "V+ binds on Intel");
        assert!(!binds(0x1002, true, None) && !binds(0x10de, true, None), "and copies elsewhere");
        assert!(binds(0x1002, true, Some("1")) && !binds(VENDOR_INTEL, true, Some("0")), "unless told");
        assert!(!binds(VENDOR_INTEL, false, Some("1")), "without the pass nothing reads the decoder's textures");
    }

    #[test]
    fn hdr10_plus_wins_then_the_measured_peak_then_the_static_one() {
        assert_eq!(content_peak(Some(600.0), Some(900.0), Some(1000.0)), (Some(600.0), PeakSource::Hdr10Plus));
        assert_eq!(content_peak(None, Some(900.0), Some(1000.0)), (Some(900.0), PeakSource::Measured));
        assert_eq!(content_peak(None, Some(1500.0), Some(1000.0)), (Some(1000.0), PeakSource::Measured), "never above the static peak");
        assert_eq!(content_peak(Some(0.0), None, Some(1000.0)), (Some(1000.0), PeakSource::Static), "an empty HDR10+ peak is no peak");
        assert_eq!(content_peak(None, None, None), (None, PeakSource::None));
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
mod gpu_tests {
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
        let picture = Picture { texture: texture.as_raw(), slice: 2, size: (96, 32), ten_bit: false, full_range: false, matrix: 1, pq: false };
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
        let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 32), ten_bit: true, full_range: false, matrix: 2, pq: false };
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
        let picture = Picture { texture: texture.as_raw(), slice: 0, size: (64, 64), ten_bit: true, full_range: false, matrix: 2, pq: true };
        // SAFETY: The test's device, on this thread.
        let mut pass = unsafe { Direct::new(&device) }.unwrap();
        // SDR white 200 nits (2.5 x 80), a 400-nit display: headroom 2.
        let output = Output { linear: true, color_scale: 2.5, headroom: 2.0 };
        let white_nits = 200.0;
        let at = |pixels: &[[f32; 3]], x: usize, y: usize| pixels[y * 64 + x][1] * 80.0;
        // HDR10+ says 1000 nits: the block lands on the display's 400 nits.
        let mapped = render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
            Tone { white_nits, hdr10_plus: Some(1000.0), static_peak: Some(4000.0) });
        assert!((at(&mapped, 5, 5) - 400.0).abs() < 15.0, "{}", at(&mapped, 5, 5));
        std::thread::sleep(std::time::Duration::from_millis(50));
        // Without any peak (none measured yet) the picture is drawn as coded, as SDL does.
        let plain = render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
            Tone { white_nits, hdr10_plus: None, static_peak: None });
        assert!(pass.measured_peak().is_none(), "nothing was measured while HDR10+ was there");
        assert!((at(&plain, 40, 40) - 100.0).abs() < 3.0 && (at(&plain, 5, 5) - 1000.0).abs() < 30.0, "{} {}", at(&plain, 40, 40), at(&plain, 5, 5));
        // Without HDR10+ the pass measures the frames and gets to about 1000 nits.
        for _ in 0..200 {
            render(&device, &context, &mut pass, &picture, (64, 64), DXGI_FORMAT_R16G16B16A16_FLOAT, output,
                Tone { white_nits, hdr10_plus: None, static_peak: Some(4000.0) });
            if pass.measured_peak().is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let measured = pass.measured_peak().expect("a measured peak within 200 frames");
        assert!((measured - 1000.0).abs() < 60.0, "measured {measured} nits");
    }
}
