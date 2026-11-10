//! D3D11 conversion owned by the Rust host. No readback or vendor tone mapping.

use crate::capture::{Device, GpuImage, Pixel};
use anyhow::{Context, Result, bail};
use std::{borrow::Cow, sync::Arc};
use windows::{
    Win32::Graphics::{
        Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
        Direct3D11::*,
        Dxgi::Common::*,
    },
    core::PCSTR,
};

// Resize scRGB in linear light, convert Rec.709 to Rec.2020, apply absolute
// ST.2084 (1.0 scRGB = 80 nits), then subsample nonlinear RGB into limited YUV.
// R16/R16G16 render views address the two P010 planes; stored codes occupy the
// upper ten bits. Standard chroma is filtered at the left luma sample's
// horizontal position, vertically centred, including during scaling.
const SHADER: &str = include_str!("shaders/color.hlsl");
include!(concat!(env!("OUT_DIR"), "/shader_bytecode.rs"));

/// The precompiled bytecode for `entry` and `target`, or the HLSL compiled now.
///
/// # Safety
///
/// `entry` and `target` must be NUL-terminated.
unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Cow<'static, [u8]>> {
    if let Some(bytecode) = shader_bytecode(entry, target) {
        return Ok(Cow::Borrowed(bytecode));
    }
    // SAFETY: `SHADER` is passed with its exact length, callers pass NUL-terminated `entry` and
    // `target`, and each blob is read only within GetBufferSize bytes while it is still alive.
    unsafe {
        let mut code = None;
        let mut errors = None;
        let result = D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Err(error) = result {
            let detail = errors
                .map(|blob| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_default();
            bail!("HDR shader compilation: {error}: {detail}");
        }
        let code = code.context("shader compiler returned no bytecode")?;
        Ok(Cow::Owned(
            std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize())
                .to_vec(),
        ))
    }
}

struct Target {
    texture: Arc<ID3D11Texture2D>,
    luma: ID3D11RenderTargetView,
    chroma: Option<ID3D11RenderTargetView>,
    unordered: Option<[ID3D11UnorderedAccessView; 2]>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Nv12,
    P010,
    /// NVENC's 8-bit native 4:4:4 (`nvenc::Input444::Ayuv`): one packed
    /// AYUV picture, written through an R8G8B8A8_UINT view.
    Ayuv,
    /// NVENC's 10-bit native 4:4:4 (`nvenc::Input444::Planar16`): the Y, U
    /// and V planes stacked in one R16_UINT texture three pictures tall.
    Planes444,
}
impl Layout {
    fn of(config: &butterpollo_core::rtsp::Negotiated) -> Self {
        match (config.yuv444, config.ten_bit()) {
            (false, false) => Self::Nv12,
            (false, true) => Self::P010,
            (true, _) => match butterpollo_core::nvenc::input_444(config.ten_bit()) {
                butterpollo_core::nvenc::Input444::Ayuv => Self::Ayuv,
                butterpollo_core::nvenc::Input444::Planar16 => Self::Planes444,
            },
        }
    }
    /// The target texture's format, and the formats of its luma (or only)
    /// and chroma render views.
    // The 4:4:4 texture and view formats are ported from LizardByte Sunshine
    // src/platform/windows/display_vram.cpp and src/nvenc/nvenc_utils.cpp
    // (ec2676b60bb8220a1807fc761bc2157e5e6dba63), GPL-3.0.
    fn formats(self) -> (DXGI_FORMAT, DXGI_FORMAT, Option<DXGI_FORMAT>) {
        match self {
            Self::Nv12 => (
                DXGI_FORMAT_NV12,
                DXGI_FORMAT_R8_UNORM,
                Some(DXGI_FORMAT_R8G8_UNORM),
            ),
            Self::P010 => (
                DXGI_FORMAT_P010,
                DXGI_FORMAT_R16_UNORM,
                Some(DXGI_FORMAT_R16G16_UNORM),
            ),
            Self::Ayuv => (DXGI_FORMAT_AYUV, DXGI_FORMAT_R8G8B8A8_UINT, None),
            Self::Planes444 => (DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R16_UINT, None),
        }
    }
    /// The target's rows for a picture `height` rows tall.
    fn rows(self, height: u32) -> u32 {
        if self == Self::Planes444 {
            height * 3
        } else {
            height
        }
    }
}
pub(crate) struct Converter {
    gpu: Device,
    vertex: ID3D11VertexShader,
    luma: ID3D11PixelShader,
    chroma: ID3D11PixelShader,
    compute: Option<ID3D11ComputeShader>,
    constants: ID3D11Buffer,
    values: [u32; 20],
    dirty: bool,
    width: u32,
    height: u32,
    layout: Layout,
    targets: Vec<Target>,
    /// The couch menu's pass, while an overlay is set.
    menu: menu::Menu,
    sources: SourceViews,
}
/// Capture textures' views, by texture and the source's size and format.
type SourceViews = view_cache::ViewCache<ID3D11ShaderResourceView, (u32, u32, Pixel)>;
/// `image.texture`'s view from `cache`, made on `gpu` the first time.
fn source_view(
    cache: &mut SourceViews,
    gpu: &Device,
    image: &GpuImage,
) -> Result<ID3D11ShaderResourceView> {
    let texture = image.texture.as_ref();
    let key = windows::core::Interface::as_raw(texture) as usize;
    cache.view(key, (image.width, image.height, image.pixel), || {
        let mut view = None;
        // SAFETY: `texture` is live and on this device, which callers check
        // with `accepts_gpu_device`.
        unsafe {
            gpu.device
                .CreateShaderResourceView(texture, None, Some(&mut view))?;
        }
        view.context("no capture texture view")
    })
}
/// `cbuffer Config` in color.hlsl, for a `source` (width, height, pixel)
/// converted to the stream, as both converters fill it.
pub(crate) fn constants(
    config: &butterpollo_core::rtsp::Negotiated,
    source: (u32, u32, Pixel),
    luminance: [f32; 2],
    pointer: Option<&crate::cursor::Cursor>,
) -> [u32; 20] {
    let mut values = [0; 20];
    values[..11].copy_from_slice(&[
        source.0,
        source.1,
        config.width,
        config.height,
        match source.2 {
            Pixel::Bgra8 => 0,
            Pixel::RgbaF16 => 1,
            Pixel::Rgba10Pq => 2,
        },
        hdr_constant(config),
        u32::from(config.color_matrix()),
        u32::from(config.full_range()),
        u32::from(config.ten_bit()),
        (luminance[0] / 80.).to_bits(),
        luminance[1].to_bits(),
    ]);
    values[11] = u32::from(config.chroma_packing);
    values[12..].copy_from_slice(&pointer_constants(pointer));
    values
}
/// `cbuffer Config`'s `hdr`: 0 for SDR, 1 for PQ, 2 for HLG (`hlg` in
/// color.hlsl), which Dolby Vision 8.4's base layer is too.
fn hdr_constant(config: &butterpollo_core::rtsp::Negotiated) -> u32 {
    if config.hlg() {
        2
    } else {
        u32::from(config.hdr)
    }
}
/// The pointer's part of `cbuffer Config`: position, size and blend mode.
fn pointer_constants(pointer: Option<&crate::cursor::Cursor>) -> [u32; 8] {
    pointer.map_or([0; 8], |c| {
        [
            c.position[0] as u32,
            c.position[1] as u32,
            c.width,
            c.height,
            if c.logic { 2 } else { 1 },
            0,
            0,
            0,
        ]
    })
}
impl Converter {
    pub fn new(
        gpu: &Device,
        config: &butterpollo_core::rtsp::Negotiated,
        source: (u32, u32, Pixel),
    ) -> Result<Self> {
        if config.width == 0
            || config.height == 0
            || (!config.yuv444
                && config.chroma_packing == 0
                && (!config.width.is_multiple_of(2) || !config.height.is_multiple_of(2)))
        {
            bail!("GPU conversion requires nonzero dimensions, and even dimensions for 4:2:0");
        }
        // An HDR desktop hands an SDR stream FP16 frames, which the shader
        // takes back to sRGB; only PQ has no SDR path. Refusing FP16 failed
        // every fallback from the compute queue on such a desktop.
        if !config.hdr && source.2 == Pixel::Rgba10Pq {
            bail!("HDR surface supplied to an SDR converter");
        }
        // SAFETY: `gpu.device` is a live D3D11 device, `compile` gets NUL-terminated names, and
        // every descriptor and the 80-byte `values` outlive the calls that read them.
        unsafe {
            let mut vertex = None;
            let mut luma = None;
            let mut chroma = None;
            gpu.device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            let layout = Layout::of(config);
            // A 4:4:4 target is one draw: the AYUV picture, or the three
            // stacked planes.
            let first: &'static [u8] = match (config.chroma_packing != 0, layout) {
                (true, _) => b"chroma_pack_y\0",
                (false, Layout::Ayuv) => b"yuv444_ayuv\0",
                (false, Layout::Planes444) => b"yuv444_planes\0",
                (false, _) => b"luma\0",
            };
            gpu.device
                .CreatePixelShader(&compile(first, b"ps_5_0\0")?, None, Some(&mut luma))?;
            gpu.device.CreatePixelShader(
                &compile(
                    if config.chroma_packing != 0 {
                        b"chroma_pack_uv\0"
                    } else {
                        b"chroma\0"
                    },
                    b"ps_5_0\0",
                )?,
                None,
                Some(&mut chroma),
            )?;
            let mut compute = None;
            if !config.yuv444 {
                let _ = gpu.device.CreateComputeShader(
                    &compile(
                        if config.chroma_packing != 0 {
                            b"chroma_pack_cs\0"
                        } else {
                            b"yuv420_cs\0"
                        },
                        b"cs_5_0\0",
                    )?,
                    None,
                    Some(&mut compute),
                );
            }
            let values = constants(config, source, [100., 1.], None);
            let mut constants = None;
            gpu.device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 80,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: values.as_ptr().cast(),
                    ..Default::default()
                }),
                Some(&mut constants),
            )?;
            Ok(Self {
                gpu: gpu.clone(),
                vertex: vertex.unwrap(),
                luma: luma.unwrap(),
                chroma: chroma.unwrap(),
                compute,
                constants: constants.unwrap(),
                values,
                dirty: false,
                width: config.coded_size().0,
                height: config.coded_size().1,
                layout,
                targets: vec![],
                menu: Default::default(),
                sources: Default::default(),
            })
        }
    }
    pub fn set_luminance(&mut self, luminance: [f32; 2]) {
        // Kept in step with `constants`.
        let white = (luminance[0] / 80.).to_bits();
        let scale = luminance[1].to_bits();
        if self.values[9] != white || self.values[10] != scale {
            self.values[9] = white;
            self.values[10] = scale;
            self.dirty = true;
        }
    }
    /// Draw `overlay` over the next conversions, or stop with `None`.
    pub fn set_overlay(&mut self, overlay: Option<&crate::overlay::Overlay>) {
        self.menu.set(overlay);
    }
    /// Panel images uploaded and menu passes run so far.
    #[cfg(test)]
    pub(crate) fn menu_counts(&self) -> (u64, u64) {
        (self.menu.uploads, self.menu.passes)
    }
    fn target(&mut self) -> Result<usize> {
        if let Some(index) = self
            .targets
            .iter()
            .position(|t| Arc::strong_count(&t.texture) == 1)
        {
            return Ok(index);
        }
        if self.targets.len() >= 8 {
            bail!("GPU conversion queue reached its bounded limit");
        }
        let (format, luma_view, chroma_view) = self.layout.formats();
        // SAFETY: `gpu.device` is live, each descriptor outlives its call, and the views target the
        // new texture.
        unsafe {
            let mut texture = None;
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.layout.rows(self.height),
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0
                    | D3D11_BIND_SHADER_RESOURCE.0
                    | if self.compute.is_some() {
                        D3D11_BIND_UNORDERED_ACCESS.0
                    } else {
                        0
                    }) as u32,
                ..Default::default()
            };
            if let Err(error) = self
                .gpu
                .device
                .CreateTexture2D(&desc, None, Some(&mut texture))
            {
                if self.compute.is_none() {
                    return Err(error).context("GPU YUV texture allocation");
                }
                self.compute = None;
                desc.BindFlags &= !(D3D11_BIND_UNORDERED_ACCESS.0 as u32);
                self.gpu
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
                    .context("GPU YUV texture allocation")?;
            }
            let texture = texture.unwrap();
            let view = |format| -> Result<ID3D11RenderTargetView> {
                let mut view = None;
                self.gpu.device.CreateRenderTargetView(
                    &texture,
                    Some(&D3D11_RENDER_TARGET_VIEW_DESC {
                        Format: format,
                        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                            Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
                        },
                    }),
                    Some(&mut view),
                )?;
                Ok(view.unwrap())
            };
            let luma = view(luma_view)?;
            let chroma = chroma_view.map(view).transpose()?;
            let unordered = if self.compute.is_some() {
                let formats = if self.layout == Layout::Nv12 {
                    [DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM]
                } else {
                    [DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM]
                };
                let views = formats.map(|format| -> Result<ID3D11UnorderedAccessView> {
                    let mut view = None;
                    self.gpu.device.CreateUnorderedAccessView(
                        &texture,
                        Some(&D3D11_UNORDERED_ACCESS_VIEW_DESC {
                            Format: format,
                            ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
                            Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                                Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
                            },
                        }),
                        Some(&mut view),
                    )?;
                    Ok(view.unwrap())
                });
                match views {
                    [Ok(y), Ok(uv)] => Some([y, uv]),
                    _ => {
                        self.compute = None;
                        None
                    }
                }
            } else {
                None
            };
            self.targets.push(Target {
                texture: Arc::new(texture),
                luma,
                chroma,
                unordered,
            });
            Ok(self.targets.len() - 1)
        }
    }
    fn source_views(&mut self, image: &GpuImage) -> Result<[Option<ID3D11ShaderResourceView>; 2]> {
        // A fence-ring image orders graphics-queue readers only when they need it.
        image.order_d3d11()?;
        let pointer = image.cursor.as_ref();
        let values = pointer_constants(pointer);
        if self.values[12..] != values {
            self.values[12..].copy_from_slice(&values);
            self.dirty = true;
        }
        let source = source_view(&mut self.sources, &self.gpu, image)?;
        Ok([Some(source), pointer.map(|c| c.view.clone())])
    }
    pub fn convert(&mut self, image: &GpuImage) -> Result<Arc<ID3D11Texture2D>> {
        let index = self.target()?;
        let source = self.source_views(image)?;
        let computes = self.compute.is_some() && self.targets[index].unordered.is_some();
        let packing = self.values[11];
        let menu = self.menu.prepare(
            &self.gpu,
            &self.values,
            if packing == 1 { [2, 1] } else { [2, 2] },
            match (computes, packing, self.layout) {
                (true, _, _) => &[],
                (false, 0, Layout::Ayuv) => &[b"menu_yuv444_ayuv\0"],
                (false, 0, Layout::Planes444) => &[b"menu_yuv444_planes\0"],
                (false, 0, _) => &[b"menu_luma\0", b"menu_chroma\0"],
                (false, _, _) => &[b"menu_chroma_pack_y\0", b"menu_chroma_pack_uv\0"],
            },
            computes.then_some(if packing == 0 {
                &b"yuv420_menu_cs\0"[..]
            } else {
                &b"chroma_pack_menu_cs\0"[..]
            }),
        )?;
        // SAFETY: The context is held from Enter to Leave with no early return between, all bound
        // objects are on this device, and `values` is the buffer's 80 bytes.
        unsafe {
            let context = &self.gpu.context;
            // Capture and the codec share this device. Keep the complete draw
            // together rather than merely protecting individual context calls.
            let lock: ID3D11Multithread = windows::core::Interface::cast(context)?;
            lock.Enter();
            if self.dirty {
                context.UpdateSubresource(
                    &self.constants,
                    0,
                    None,
                    self.values.as_ptr().cast(),
                    0,
                    0,
                );
                self.dirty = false;
            }
            if let Some(menu) = &menu {
                menu.update(context);
            }
            let target = &self.targets[index];
            if let (Some(shader), Some(views)) = (&self.compute, &target.unordered) {
                context.CSSetShader(shader, None);
                context.CSSetShaderResources(0, Some(&source));
                context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                context.CSSetUnorderedAccessViews(
                    0,
                    2,
                    Some(views.each_ref().map(|v| Some(v.clone())).as_ptr()),
                    None,
                );
                context.Dispatch(self.width.div_ceil(16), self.height.div_ceil(16), 1);
                if let Some(menu) = &menu {
                    menu.dispatch(context);
                }
                context.CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
                context.CSSetShaderResources(0, Some(&[None, None]));
                context.CSSetShader(None, None);
            } else {
                context.IASetInputLayout(None);
                context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                context.VSSetShader(&self.vertex, None);
                context.PSSetShaderResources(0, Some(&source));
                context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                context.RSSetState(None);
                context.OMSetBlendState(None, None, u32::MAX);
                if let Some(menu) = &menu {
                    menu.bind(context);
                }
                for (plane, (view, shader, divisor)) in
                    std::iter::once((&target.luma, &self.luma, 1))
                        .chain(target.chroma.as_ref().map(|view| (view, &self.chroma, 2)))
                        .enumerate()
                {
                    // The stacked 4:4:4 planes are one viewport three
                    // pictures tall.
                    context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                        Width: (self.width / divisor) as f32,
                        Height: (self.layout.rows(self.height) / divisor) as f32,
                        MinDepth: 0.,
                        MaxDepth: 1.,
                        ..Default::default()
                    }]));
                    context.OMSetRenderTargets(Some(&[Some(view.clone())]), None);
                    context.PSSetShader(shader, None);
                    context.Draw(3, 0);
                    if let Some(menu) = &menu {
                        context.PSSetShader(&menu.pixel[plane], None);
                        context.Draw(3, 0);
                    }
                }
                if menu.is_some() {
                    menu::Pass::unbind(context);
                }
                context.PSSetShaderResources(0, Some(&[None, None]));
                context.OMSetRenderTargets(None, None);
            }
            context.Flush();
            lock.Leave();
        }
        Ok(self.targets[index].texture.clone())
    }
}
pub(crate) struct PlanarConverter {
    base: Converter,
    shaders: [ID3D11PixelShader; 3],
    combined: Option<ID3D11PixelShader>,
    compute: Option<(ID3D11ComputeShader, [ID3D11UnorderedAccessView; 3])>,
    pub textures: [Arc<ID3D11Texture2D>; 3],
    views: [ID3D11RenderTargetView; 3],
    divisor: u32,
}
impl PlanarConverter {
    pub fn new(
        gpu: &Device,
        config: &butterpollo_core::rtsp::Negotiated,
        source: (u32, u32, Pixel),
    ) -> Result<Self> {
        let mut base = Converter::new(gpu, config, source)?;
        base.values[11] = u32::from(config.yuv444);
        base.dirty = true;
        let divisor = if config.yuv444 { 1 } else { 2 };
        // SAFETY: `gpu.device` is a live D3D11 device, `compile` gets NUL-terminated names, and
        // every descriptor outlives the call that reads it.
        unsafe {
            let mut textures = Vec::new();
            let mut views = Vec::new();
            let mut shaders = Vec::new();
            let mut combined = None;
            if config.yuv444 {
                gpu.device.CreatePixelShader(
                    &compile(b"pyro_444\0", b"ps_5_0\0")?,
                    None,
                    Some(&mut combined),
                )?;
            }
            for (plane, entry) in [b"pyro_y\0", b"pyro_u\0", b"pyro_v\0"].iter().enumerate() {
                let mut shader = None;
                gpu.device.CreatePixelShader(
                    &compile(*entry, b"ps_5_0\0")?,
                    None,
                    Some(&mut shader),
                )?;
                shaders.push(shader.unwrap());
                let mut texture = None;
                gpu.device.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: config.width / if plane == 0 { 1 } else { divisor },
                        Height: config.height / if plane == 0 { 1 } else { divisor },
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: if config.ten_bit() {
                            DXGI_FORMAT_R16_UNORM
                        } else {
                            DXGI_FORMAT_R8_UNORM
                        },
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        // Unordered access for the conversion on the compute queue.
                        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                            | D3D11_BIND_RENDER_TARGET.0
                            | D3D11_BIND_UNORDERED_ACCESS.0)
                            as u32,
                        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0
                            | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0)
                            as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )?;
                let texture = texture.unwrap();
                let mut view = None;
                gpu.device
                    .CreateRenderTargetView(&texture, None, Some(&mut view))?;
                textures.push(Arc::new(texture));
                views.push(view.unwrap());
            }
            // `pyro_cs` covers 4:4:4 too: with `padding` set it writes full-size
            // chroma (`color.hlsl`), so only a failure falls back to the pixel shaders.
            let compute = (|| -> Result<_> {
                let mut shader = None;
                gpu.device.CreateComputeShader(
                    &compile(b"pyro_cs\0", b"cs_5_0\0")?,
                    None,
                    Some(&mut shader),
                )?;
                let mut unordered = Vec::new();
                for texture in &textures {
                    let mut view = None;
                    gpu.device.CreateUnorderedAccessView(
                        texture.as_ref(),
                        None,
                        Some(&mut view),
                    )?;
                    unordered.push(view.unwrap());
                }
                Ok((shader.unwrap(), unordered.try_into().ok().unwrap()))
            })()
            .ok();
            Ok(Self {
                base,
                shaders: shaders.try_into().ok().unwrap(),
                combined,
                compute,
                textures: textures.try_into().ok().unwrap(),
                views: views.try_into().ok().unwrap(),
                divisor,
            })
        }
    }
    pub fn set_overlay(&mut self, overlay: Option<&crate::overlay::Overlay>) {
        self.base.set_overlay(overlay);
    }
    #[cfg(test)]
    pub(crate) fn menu_counts(&self) -> (u64, u64) {
        self.base.menu_counts()
    }
    pub fn convert(&mut self, image: &GpuImage, luminance: [f32; 2]) -> Result<()> {
        self.base.set_luminance(luminance);
        let source = self.base.source_views(image)?;
        let menu = self.base.menu.prepare(
            &self.base.gpu,
            &self.base.values,
            [2, 2],
            match (&self.compute, &self.combined) {
                (Some(_), _) => &[],
                (None, Some(_)) => &[b"menu_pyro_444\0"],
                (None, None) => &[b"menu_pyro_y\0", b"menu_pyro_u\0", b"menu_pyro_v\0"],
            },
            self.compute.is_some().then_some(&b"pyro_menu_cs\0"[..]),
        )?;
        // SAFETY: The context is held from Enter to Leave with no early return between, all bound
        // objects are on this device, and `values` is the buffer's 80 bytes.
        unsafe {
            let context = &self.base.gpu.context;
            let lock: ID3D11Multithread = windows::core::Interface::cast(context)?;
            lock.Enter();
            if self.base.dirty {
                context.UpdateSubresource(
                    &self.base.constants,
                    0,
                    None,
                    self.base.values.as_ptr().cast(),
                    0,
                    0,
                );
                self.base.dirty = false;
            }
            if let Some(menu) = &menu {
                menu.update(context);
            }
            if let Some((shader, views)) = &self.compute {
                context.CSSetShader(shader, None);
                context.CSSetShaderResources(0, Some(&source));
                context.CSSetConstantBuffers(0, Some(&[Some(self.base.constants.clone())]));
                context.CSSetUnorderedAccessViews(
                    2,
                    3,
                    Some(views.each_ref().map(|v| Some(v.clone())).as_ptr()),
                    None,
                );
                context.Dispatch(
                    (self.base.width / 2).div_ceil(8),
                    (self.base.height / 2).div_ceil(8),
                    1,
                );
                if let Some(menu) = &menu {
                    menu.dispatch(context);
                }
                context.CSSetUnorderedAccessViews(2, 3, Some([None, None, None].as_ptr()), None);
                context.CSSetShaderResources(0, Some(&[None, None]));
                context.CSSetShader(None, None);
            } else {
                context.IASetInputLayout(None);
                context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                context.VSSetShader(&self.base.vertex, None);
                context.PSSetShaderResources(0, Some(&source));
                context.PSSetConstantBuffers(0, Some(&[Some(self.base.constants.clone())]));
                context.RSSetState(None);
                context.OMSetBlendState(None, None, u32::MAX);
                if let Some(menu) = &menu {
                    menu.bind(context);
                }
                if let Some(shader) = &self.combined {
                    context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                        Width: self.base.width as f32,
                        Height: self.base.height as f32,
                        MinDepth: 0.,
                        MaxDepth: 1.,
                        ..Default::default()
                    }]));
                    context.OMSetRenderTargets(
                        Some(&self.views.each_ref().map(|view| Some(view.clone()))),
                        None,
                    );
                    context.PSSetShader(shader, None);
                    context.Draw(3, 0);
                    if let Some(menu) = &menu {
                        context.PSSetShader(&menu.pixel[0], None);
                        context.Draw(3, 0);
                    }
                } else {
                    for plane in 0..3 {
                        let divisor = if plane == 0 { 1 } else { self.divisor };
                        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                            Width: (self.base.width / divisor) as f32,
                            Height: (self.base.height / divisor) as f32,
                            MinDepth: 0.,
                            MaxDepth: 1.,
                            ..Default::default()
                        }]));
                        context.OMSetRenderTargets(Some(&[Some(self.views[plane].clone())]), None);
                        context.PSSetShader(&self.shaders[plane], None);
                        context.Draw(3, 0);
                        if let Some(menu) = &menu {
                            context.PSSetShader(&menu.pixel[plane], None);
                            context.Draw(3, 0);
                        }
                    }
                }
                if menu.is_some() {
                    menu::Pass::unbind(context);
                }
                context.PSSetShaderResources(0, Some(&[None, None]));
                context.OMSetRenderTargets(None, None);
            }
            lock.Leave();
        }
        Ok(())
    }
}
mod hdr_stats;
mod menu;
mod view_cache;
pub(crate) use hdr_stats::HdrStats;
#[cfg(test)]
mod tests;
